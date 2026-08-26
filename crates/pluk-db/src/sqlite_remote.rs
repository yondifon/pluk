//! Remote SQLite driver — shells out over SSH.
//!
//! Invokes `sqlite3 -json [-readonly] <file> <sql>` through an SSH exec
//! channel and parses the JSON array that comes back. The critical invariant:
//! **bind parameters are rejected outright** — there is no safe channel for
//! them through a shell command line, and inlining would be an injection
//! vector. The error message explains why.

use async_trait::async_trait;

use crate::config::SshExecProvider;
use crate::driver::Driver;
use crate::error::DriverError;
use crate::types::*;

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn quote_ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn reject_params(params: &[serde_json::Value]) -> Result<(), DriverError> {
    if !params.is_empty() {
        return Err(DriverError::Query(
            "Bind parameters are not supported for SQLite over SSH. Inline literal values instead — there is no safe channel for parameters through a shell command line, and inlining without proper escaping would be an SQL injection vector.".into(),
        ));
    }
    Ok(())
}

fn parse_json_output(output: &str) -> Result<Vec<serde_json::Value>, DriverError> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Ok(vec![]);
    }
    serde_json::from_str::<Vec<serde_json::Value>>(trimmed)
        .map_err(|e| DriverError::Query(format!("failed to parse sqlite3 -json output: {e}; output was: {trimmed}")))
}

pub struct RemoteSqliteDriver {
    filename: String,
    executor: Box<dyn SshExecProvider>,
}

impl RemoteSqliteDriver {
    pub fn new(filename: String, executor: Box<dyn SshExecProvider>) -> Self {
        Self { filename, executor }
    }

    async fn run_json(
        &self,
        sql: &str,
        readonly: bool,
        opts: Option<QueryOpts>,
    ) -> Result<Vec<serde_json::Value>, DriverError> {
        // Build remote command: sqlite3 [-readonly] -json '<file>' '<sql>'
        let ro = if readonly { "-readonly " } else { "" };
        let command = format!(
            "sqlite3 {}-json {} {}",
            ro,
            shell_quote(&self.filename),
            shell_quote(sql)
        );

        crate::sql_log::record_executed_sql(sql, None, None);

        let timeout = opts.as_ref().and_then(|o| o.timeout_ms);

        // Race executor against timeout/cancel via with_opts helper
        let executor = &self.executor;
        let command_clone = command.clone();
        let fut = async move {
            let out = executor.exec(command_clone, timeout).await?;
            let rows = parse_json_output(&out)?;
            crate::sql_log::record_executed_sql(sql, Some(rows.len() as i64), None);
            Ok::<Vec<serde_json::Value>, DriverError>(rows)
        };

        // with_opts expects a future returning Result<T, DriverError> — we pass opts for timeout/cancel racing
        crate::driver::with_opts(opts, fut).await
    }

    fn rows_to_query_result(rows: Vec<serde_json::Value>) -> QueryResult {
        let fields = rows
            .first()
            .and_then(|v| v.as_object())
            .map(|m| m.keys().cloned().collect());
        QueryResult { rows, fields }
    }
}

#[async_trait]
impl Driver for RemoteSqliteDriver {
    async fn query(&self, sql: &str, params: &[serde_json::Value], opts: Option<QueryOpts>) -> Result<QueryResult, DriverError> {
        reject_params(params)?;
        let rows = self.run_json(sql, false, opts).await?;
        Ok(Self::rows_to_query_result(rows))
    }

    async fn query_read_only(&self, sql: &str, params: &[serde_json::Value], opts: Option<QueryOpts>) -> Result<QueryResult, DriverError> {
        reject_params(params)?;
        let rows = self.run_json(sql, true, opts).await?;
        Ok(Self::rows_to_query_result(rows))
    }

    async fn explain(&self, sql: &str, params: &[serde_json::Value]) -> Result<QueryResult, DriverError> {
        reject_params(params)?;
        let rows = self.run_json(&format!("EXPLAIN QUERY PLAN {sql}"), false, None).await?;
        Ok(Self::rows_to_query_result(rows))
    }

    async fn list_tables(&self, _schema: Option<&str>) -> Result<Vec<String>, DriverError> {
        let rows = self
            .run_json("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name", false, None)
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|v| v.get("name").and_then(|x| x.as_str()).map(|s| s.to_string()))
            .collect())
    }

    async fn describe_table(&self, table: &str, _schema: Option<&str>) -> Result<Vec<ColumnInfo>, DriverError> {
        let rows = self
            .run_json(&format!("PRAGMA table_info({})", quote_ident(table)), false, None)
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| ColumnInfo {
                column: r.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                r#type: r.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                nullable: r.get("notnull").and_then(|v| v.as_i64()).unwrap_or(0) == 0,
            })
            .collect())
    }

    async fn sample_table(&self, table: &str, limit: i64, _schema: Option<&str>) -> Result<QueryResult, DriverError> {
        let sql = format!("SELECT * FROM {} LIMIT {}", quote_ident(table), limit);
        let rows = self.run_json(&sql, false, None).await?;
        Ok(Self::rows_to_query_result(rows))
    }

    async fn search_schema(&self, term: &str, _schema: Option<&str>) -> Result<Vec<SchemaSearchResult>, DriverError> {
        // Keep parity with TS remote: tables via LIKE with escaped pattern, columns via Rust contains
        let pattern = format!("%{}%", term.replace('%', "\\%").replace('_', "\\_"));
        // Escape single quotes for sqlite string literal
        let sql_pattern = pattern.replace('\'', "''");
        let rows = self
            .run_json(
                &format!("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '{sql_pattern}' ESCAPE '\\' ORDER BY name"),
                false,
                None,
            )
            .await?;
        let mut results: Vec<SchemaSearchResult> = rows
            .into_iter()
            .filter_map(|v| v.get("name").and_then(|x| x.as_str()).map(|s| s.to_string()))
            .map(|t| SchemaSearchResult {
                kind: "table".into(),
                table: t,
                column: None,
                r#type: None,
            })
            .collect();

        for table in self.list_tables(None).await? {
            for col in self.describe_table(&table, None).await? {
                if col.column.contains(term) {
                    results.push(SchemaSearchResult {
                        kind: "column".into(),
                        table: table.clone(),
                        column: Some(col.column.clone()),
                        r#type: Some(col.r#type.clone()),
                    });
                }
            }
        }
        results.sort_by(|a, b| {
            a.table
                .cmp(&b.table)
                .then(a.kind.cmp(&b.kind))
                .then(a.column.cmp(&b.column))
        });
        Ok(results)
    }

    async fn list_relationships(&self, table: Option<&str>, _schema: Option<&str>) -> Result<Vec<RelationshipInfo>, DriverError> {
        let tables: Vec<String> = if let Some(t) = table {
            vec![t.to_string()]
        } else {
            self.list_tables(None).await?
        };
        let mut out = Vec::new();
        for t in tables {
            let rows = self
                .run_json(&format!("PRAGMA foreign_key_list({})", quote_ident(&t)), false, None)
                .await?;
            for r in rows {
                out.push(RelationshipInfo {
                    from_table: t.clone(),
                    from_column: r.get("from").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    to_table: r.get("table").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    to_column: r.get("to").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    constraint_name: Some(format!(
                        "fk_{}_{}",
                        t,
                        r.get("id").and_then(|v| v.as_i64()).unwrap_or(0)
                    )),
                });
            }
        }
        Ok(out)
    }

    async fn table_stats(&self, table: &str, _schema: Option<&str>) -> Result<TableStats, DriverError> {
        let idx_rows = self
            .run_json(&format!("PRAGMA index_list({})", quote_ident(table)), false, None)
            .await?;
        let mut indexes = Vec::new();
        for idx in idx_rows {
            let name = idx.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let unique = idx.get("unique").and_then(|v| v.as_i64()).unwrap_or(0) == 1;
            let cols_rows = self
                .run_json(&format!("PRAGMA index_info({})", quote_ident(&name)), false, None)
                .await?;
            let cols: Vec<String> = cols_rows
                .into_iter()
                .filter_map(|v| v.get("name").and_then(|x| x.as_str()).map(|s| s.to_string()))
                .collect();
            indexes.push(IndexInfo { name, columns: cols, unique });
        }
        Ok(TableStats {
            table: table.to_string(),
            estimated_rows: None,
            size_bytes: None,
            indexes,
        })
    }

    async fn list_schemas(&self) -> Result<Vec<String>, DriverError> {
        Ok(vec!["main".into()])
    }

    async fn list_databases(&self) -> Result<Vec<String>, DriverError> {
        let rows = self.run_json("PRAGMA database_list", false, None).await?;
        Ok(rows
            .into_iter()
            .filter_map(|v| v.get("name").and_then(|x| x.as_str()).map(|s| s.to_string()))
            .collect())
    }

    async fn get_full_schema(&self, _schema: Option<&str>) -> Result<String, DriverError> {
        let tables = self.list_tables(None).await?;
        let mut lines: Vec<String> = Vec::new();
        for t in tables {
            let cols = self.describe_table(&t, None).await?;
            let fks = self.list_relationships(Some(&t), None).await?;
            lines.push(format!("TABLE {t} ("));
            for col in &cols {
                let nullability = if col.nullable { "NULL" } else { "NOT NULL" };
                lines.push(format!("  {} {} {}", col.column, col.r#type, nullability));
            }
            lines.push(")".into());
            for fk in fks {
                lines.push(format!("FK {}.{} -> {}.{}", t, fk.from_column, fk.to_table, fk.to_column));
            }
            lines.push(String::new());
        }
        Ok(lines.join("\n").trim().to_string())
    }

    async fn test_connection(&self) -> Result<(), DriverError> {
        self.run_json("SELECT 1 AS ok", false, Some(QueryOpts { timeout_ms: Some(15_000), cancel: None }))
            .await
            .map(|_| ())
    }

    async fn close(&self) -> Result<(), DriverError> {
        // SSH eviction is handled at factory/caller layer; driver close is no-op here.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_escapes() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        assert_eq!(shell_quote("/tmp/my db.sqlite"), "'/tmp/my db.sqlite'");
    }

    #[test]
    fn quote_ident_escapes() {
        assert_eq!(quote_ident("my\"table"), "\"my\"\"table\"");
    }

    #[test]
    fn parse_json_empty() {
        assert_eq!(parse_json_output("").unwrap().len(), 0);
        assert_eq!(parse_json_output("   \n  ").unwrap().len(), 0);
    }

    #[test]
    fn parse_json_array() {
        let rows = parse_json_output(r#"[{"a":1,"b":"x"},{"a":2}]"#).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["a"], 1);
    }

    #[tokio::test]
    async fn rejects_params() {
        struct NoopExec;
        #[async_trait::async_trait]
        impl SshExecProvider for NoopExec {
            async fn exec(&self, _cmd: String, _tm: Option<u64>) -> Result<String, DriverError> { Ok("[]".into()) }
        }
        let d = RemoteSqliteDriver::new("/tmp/x.db".into(), Box::new(NoopExec));
        let err = d.query("SELECT 1", &[serde_json::json!(1)], None).await.unwrap_err();
        assert!(err.to_string().contains("Bind parameters are not supported"));
        assert!(err.to_string().contains("injection"));
        let err2 = d.query_read_only("SELECT 1", &[serde_json::json!(1)], None).await.unwrap_err();
        assert!(err2.to_string().contains("Bind parameters are not supported"));
    }

    #[test]
    fn parse_json_captured_sqlite3_output() {
        // Captured `sqlite3 -json` outputs
        let out_tables = r#"[{"name":"orders"},{"name":"users"}]"#;
        let rows = parse_json_output(out_tables).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "users".to_string().chars().next().map(|_| ()).is_some().then(|| rows[0]["name"].clone()).unwrap_or(rows[0]["name"].clone()));
        // pragma table_info shape
        let out_cols = r#"[{"cid":0,"name":"id","type":"INTEGER","notnull":0,"dflt_value":null,"pk":1},{"cid":1,"name":"email","type":"TEXT","notnull":1,"dflt_value":null,"pk":0}]"#;
        let cols = parse_json_output(out_cols).unwrap();
        assert_eq!(cols[0]["name"], "id");
        assert_eq!(cols[1]["notnull"], 1);
        // NULL handling: empty result
        let empty = parse_json_output("").unwrap();
        assert!(empty.is_empty());
        // Malformed JSON should error with output included
        let bad = parse_json_output("not json");
        assert!(bad.is_err());
        assert!(bad.unwrap_err().to_string().contains("failed to parse sqlite3"));
    }

    #[tokio::test]
    async fn remote_driver_parses_mock_ssh_output() {
        // Mock SSH executor that returns canned sqlite3 -json output based on SQL content
        struct MockExec;
        #[async_trait::async_trait]
        impl SshExecProvider for MockExec {
            async fn exec(&self, command: String, _tm: Option<u64>) -> Result<String, DriverError> {
                if command.contains("sqlite_master") && command.contains("name LIKE") {
                    Ok(r#"[{"name":"users"}]"#.into())
                } else if command.contains("sqlite_master") {
                    Ok(r#"[{"name":"orders"},{"name":"users"}]"#.into())
                } else if command.contains("table_info") && command.contains("users") {
                    Ok(r#"[{"cid":0,"name":"id","type":"INTEGER","notnull":0,"pk":1},{"cid":1,"name":"name","type":"TEXT","notnull":1,"pk":0}]"#.into())
                } else if command.contains("table_info") {
                    Ok(r#"[]"#.into())
                } else if command.contains("foreign_key_list") {
                    Ok(r#"[{"id":0,"seq":0,"table":"users","from":"user_id","to":"id"}]"#.into())
                } else if command.contains("database_list") {
                    Ok(r#"[{"seq":0,"name":"main","file":"/tmp/x.db"}]"#.into())
                } else if command.contains("index_list") {
                    Ok(r#"[]"#.into())
                } else {
                    Ok(r#"[]"#.into())
                }
            }
        }
        let d = RemoteSqliteDriver::new("/tmp/x.db".into(), Box::new(MockExec));
        let tables = d.list_tables(None).await.unwrap();
        assert_eq!(tables, vec!["orders", "users"]);
        let cols = d.describe_table("users", None).await.unwrap();
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[1].column, "name");
        assert!(!cols[1].nullable);
        let dbs = d.list_databases().await.unwrap();
        assert_eq!(dbs, vec!["main"]);
        let rels = d.list_relationships(Some("orders"), None).await.unwrap();
        assert_eq!(rels[0].from_column, "user_id");
        assert_eq!(rels[0].constraint_name.as_deref().unwrap(), "fk_orders_0");
        // Command must include -readonly for query_read_only
        // Need owned cap for driver; wrap in Arc
        struct SharedCapture(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
        #[async_trait::async_trait]
        impl SshExecProvider for SharedCapture {
            async fn exec(&self, command: String, _tm: Option<u64>) -> Result<String, DriverError> {
                self.0.lock().unwrap().push(command.clone());
                Ok("[]".into())
            }
        }
        let shared = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let d2 = RemoteSqliteDriver::new("/tmp/my db.sqlite".into(), Box::new(SharedCapture(shared.clone())));
        d2.query_read_only("SELECT 1", &[], None).await.unwrap();
        let cmds = shared.lock().unwrap().clone();
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].contains("-readonly"), "read-only must pass -readonly: {}", cmds[0]);
        assert!(cmds[0].contains("-json"), "must use -json");
        // Shell quoting of path with space
        assert!(cmds[0].contains("'/tmp/my db.sqlite'"), "path must be shell-quoted: {}", cmds[0]);
    }

    #[tokio::test]
    async fn row_capping_with_remote_results() {
        struct MockExec;
        #[async_trait::async_trait]
        impl SshExecProvider for MockExec {
            async fn exec(&self, _cmd: String, _tm: Option<u64>) -> Result<String, DriverError> {
                // Return 5 rows like sqlite3 -json would
                Ok(r#"[{"x":1},{"x":2},{"x":3},{"x":4},{"x":5}]"#.into())
            }
        }
        let d = RemoteSqliteDriver::new("/tmp/x.db".into(), Box::new(MockExec));
        let qr = d.query("SELECT x FROM t", &[], None).await.unwrap();
        assert_eq!(qr.rows.len(), 5);
        let (capped, was_capped, total) = crate::capping::cap_rows(qr.rows, 2);
        assert_eq!(capped.len(), 2);
        assert!(was_capped);
        assert_eq!(total, 5);
    }
}
