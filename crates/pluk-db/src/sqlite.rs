//! Local SQLite driver — opens a file directly.
//!
//! Crate choice: `rusqlite` with `bundled` SQLite, same as `pluk-store`.
//! The TS driver used `bun:sqlite` (`Database`) synchronously; `rusqlite`
//! is the closest Rust equivalent. Because `Connection` is `!Send`, every
//! operation is dispatched through `spawn_blocking` holding a shared
//! `Arc<Mutex<Connection>>`, matching the single-connection semantics of the
//! JS driver (one file, one connection). Read-only is enforced by toggling
//! `PRAGMA query_only` per call and clearing it in a finally-equivalent
//! path — this is per-connection state, so the clear must run even when the
//! query errors, otherwise the next read-write call stays locked.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rusqlite::types::ValueRef;

use crate::driver::Driver;
use crate::error::DriverError;
use crate::types::*;

/// Quote an identifier for SQLite (double-quote, doubling internal quotes).
fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn json_to_rusqlite(v: &serde_json::Value) -> rusqlite::types::Value {
    match v {
        serde_json::Value::Null => rusqlite::types::Value::Null,
        serde_json::Value::Bool(b) => rusqlite::types::Value::Integer(if *b { 1 } else { 0 }),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                rusqlite::types::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                rusqlite::types::Value::Real(f)
            } else {
                rusqlite::types::Value::Text(n.to_string())
            }
        }
        serde_json::Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            rusqlite::types::Value::Text(v.to_string())
        }
    }
}

fn value_ref_to_json(v: ValueRef<'_>) -> serde_json::Value {
    match v {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Integer(i) => serde_json::json!(i),
        ValueRef::Real(f) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        ValueRef::Text(t) => serde_json::Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => {
            // Blobs have no canonical JSON form; mirror sqlite3 -json which
            // base64-encodes or returns text. Return as string if valid UTF-8,
            // otherwise as array of bytes.
            if let Ok(s) = std::str::from_utf8(b) {
                serde_json::Value::String(s.to_string())
            } else {
                serde_json::Value::Array(b.iter().map(|x| serde_json::json!(*x)).collect())
            }
        }
    }
}

fn run_query_blocking(
    conn: &Arc<Mutex<rusqlite::Connection>>,
    sql: &str,
    params: &[serde_json::Value],
    readonly: bool,
) -> Result<QueryResult, DriverError> {
    let sql_owned = sql.to_string();
    let params_owned: Vec<serde_json::Value> = params.to_vec();
    // Record via sql_log mechanism (proper path, not monkey-patch)
    crate::sql_log::record_executed_sql(&sql_owned, None, None);

    let conn = conn.clone();
    // We are already inside spawn_blocking caller; do synchronous work here.
    // This helper is called from spawn_blocking context.
    let guard = conn
        .lock()
        .map_err(|e| DriverError::Other(format!("mutex poisoned: {e}")))?;

    if readonly {
        guard
            .execute_batch("PRAGMA query_only = ON")
            .map_err(|e| DriverError::Query(e.to_string()))?;
    }

    let result: Result<QueryResult, DriverError> = (|| {
        let mut stmt = guard
            .prepare(&sql_owned)
            .map_err(|e| DriverError::Query(e.to_string()))?;
        let col_names: Vec<String> = stmt
            .column_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let vals: Vec<rusqlite::types::Value> = params_owned.iter().map(json_to_rusqlite).collect();

        let mut rows: Vec<serde_json::Value> = Vec::new();
        let mut rows_iter = stmt
            .query_map(rusqlite::params_from_iter(vals.iter()), |row| {
                let mut map = serde_json::Map::new();
                for (i, name) in col_names.iter().enumerate() {
                    let v: ValueRef<'_> = row.get_ref(i)?;
                    map.insert(name.clone(), value_ref_to_json(v));
                }
                Ok(serde_json::Value::Object(map))
            })
            .map_err(|e| DriverError::Query(e.to_string()))?;

        for r in rows_iter.by_ref() {
            rows.push(r.map_err(|e| DriverError::Query(e.to_string()))?);
        }

        let fields = if col_names.is_empty() {
            None
        } else {
            Some(col_names)
        };
        // For statements that produce no columns (e.g. INSERT), fields is None.
        // Keep consistency: if rows empty, still expose column names when available.
        Ok(QueryResult { rows, fields })
    })();

    if readonly {
        // Must clear even if query failed — per-connection state.
        let _ = guard.execute_batch("PRAGMA query_only = OFF");
    }

    match result {
        Ok(qr) => {
            crate::sql_log::record_executed_sql(&sql_owned, Some(qr.rows.len() as i64), None);
            Ok(qr)
        }
        Err(e) => {
            crate::sql_log::record_executed_sql(&sql_owned, None, Some(&e.to_string()));
            Err(e)
        }
    }
}

pub struct SqliteDriver {
    conn: Arc<Mutex<rusqlite::Connection>>,
    closed: Arc<Mutex<bool>>,
}

impl SqliteDriver {
    pub fn open(path: &str) -> Result<Self, DriverError> {
        let conn =
            rusqlite::Connection::open(path).map_err(|e| DriverError::Connection(e.to_string()))?;
        // Mirror pluk-store busy timeout to avoid SQLITE_BUSY cross-process
        conn.busy_timeout(std::time::Duration::from_millis(5_000))
            .map_err(|e| DriverError::Connection(e.to_string()))?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            closed: Arc::new(Mutex::new(false)),
        })
    }

    /// Open an in-memory database (useful for tests that don't need a temp file).
    pub fn open_in_memory() -> Result<Self, DriverError> {
        let conn = rusqlite::Connection::open_in_memory()
            .map_err(|e| DriverError::Connection(e.to_string()))?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            closed: Arc::new(Mutex::new(false)),
        })
    }
}

#[async_trait]
impl Driver for SqliteDriver {
    async fn query(
        &self,
        sql: &str,
        params: &[serde_json::Value],
        opts: Option<QueryOpts>,
    ) -> Result<QueryResult, DriverError> {
        let sql = sql.to_string();
        let params = params.to_vec();
        let conn = self.conn.clone();
        let fut = async move {
            let sql2 = sql.clone();
            let p2 = params.clone();
            let c2 = conn.clone();
            tokio::task::spawn_blocking(move || run_query_blocking(&c2, &sql2, &p2, false))
                .await
                .map_err(|e| DriverError::Other(format!("join error: {e}")))?
        };
        crate::driver::with_opts(opts, fut).await
    }

    async fn query_read_only(
        &self,
        sql: &str,
        params: &[serde_json::Value],
        opts: Option<QueryOpts>,
    ) -> Result<QueryResult, DriverError> {
        let sql = sql.to_string();
        let params = params.to_vec();
        let conn = self.conn.clone();
        let fut = async move {
            let sql2 = sql.clone();
            let p2 = params.clone();
            let c2 = conn.clone();
            tokio::task::spawn_blocking(move || run_query_blocking(&c2, &sql2, &p2, true))
                .await
                .map_err(|e| DriverError::Other(format!("join error: {e}")))?
        };
        crate::driver::with_opts(opts, fut).await
    }

    async fn explain(
        &self,
        sql: &str,
        params: &[serde_json::Value],
    ) -> Result<QueryResult, DriverError> {
        let full = format!("EXPLAIN QUERY PLAN {sql}");
        self.query(&full, params, None).await
    }

    async fn list_tables(&self, _schema: Option<&str>) -> Result<Vec<String>, DriverError> {
        let qr = self
            .query(
                "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name",
                &[],
                None,
            )
            .await?;
        Ok(qr
            .rows
            .into_iter()
            .filter_map(|v| {
                v.get("name")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string())
            })
            .collect())
    }

    async fn describe_table(
        &self,
        table: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<ColumnInfo>, DriverError> {
        let sql = format!("PRAGMA table_info({})", quote_ident(table));
        let qr = self.query(&sql, &[], None).await?;
        Ok(qr
            .rows
            .into_iter()
            .map(|r| ColumnInfo {
                column: r
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                r#type: r
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                nullable: r.get("notnull").and_then(|v| v.as_i64()).unwrap_or(0) == 0,
            })
            .collect())
    }

    async fn sample_table(
        &self,
        table: &str,
        limit: i64,
        _schema: Option<&str>,
    ) -> Result<QueryResult, DriverError> {
        let sql = format!("SELECT * FROM {} LIMIT ?", quote_ident(table));
        self.query(&sql, &[serde_json::json!(limit)], None).await
    }

    async fn search_schema(
        &self,
        term: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<SchemaSearchResult>, DriverError> {
        // Escape LIKE wildcards in pattern, mirroring TS
        let pattern = format!("%{}%", term.replace('%', "\\%").replace('_', "\\_"));
        let escaped_like = pattern.clone();

        // Tables matching LIKE
        let qr = self
            .query(
                "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE ? ESCAPE '\\' ORDER BY name",
                &[serde_json::json!(escaped_like)],
                None,
            )
            .await?;
        let mut results: Vec<SchemaSearchResult> = qr
            .rows
            .into_iter()
            .filter_map(|v| {
                v.get("name")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string())
            })
            .map(|t| SchemaSearchResult {
                kind: "table".into(),
                table: t,
                column: None,
                r#type: None,
            })
            .collect();

        // Columns matching via per-table pragma (avoid LIKE on PRAGMA output; do Rust contains + LIKE check mirror)
        // Fetch all tables then inspect columns — mirrors TS behaviour.
        let tables_qr = self
            .query(
                "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name",
                &[],
                None,
            )
            .await?;
        let all_tables: Vec<String> = tables_qr
            .rows
            .into_iter()
            .filter_map(|v| {
                v.get("name")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string())
            })
            .collect();

        for t in &all_tables {
            let cols = self.describe_table(t, None).await?;
            for c in cols {
                // Mirror TS: prefer LIKE match via SQL, but here we do Rust check equivalent to LIKE with ESCAPE.
                // Use case-insensitive contains? TS uses LIKE which is case-insensitive for ASCII; we simulate with
                // checking if column name contains term (case-sensitive matching for simplicity) OR do a LIKE query.
                // Do a LIKE via secondary query to stay faithful: SELECT ? LIKE ? ESCAPE '\\'
                let like_qr = self
                    .query(
                        "SELECT ? LIKE ? ESCAPE '\\' AS matches",
                        &[
                            serde_json::json!(c.column),
                            serde_json::json!(pattern.clone()),
                        ],
                        None,
                    )
                    .await?;
                let matched = like_qr
                    .rows
                    .first()
                    .and_then(|r| r.get("matches"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
                    == 1;
                if matched {
                    results.push(SchemaSearchResult {
                        kind: "column".into(),
                        table: t.clone(),
                        column: Some(c.column.clone()),
                        r#type: Some(c.r#type.clone()),
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

    async fn list_relationships(
        &self,
        table: Option<&str>,
        _schema: Option<&str>,
    ) -> Result<Vec<RelationshipInfo>, DriverError> {
        let tables: Vec<String> = if let Some(t) = table {
            vec![t.to_string()]
        } else {
            self.list_tables(None).await?
        };
        let mut out = Vec::new();
        for t in tables {
            let sql = format!("PRAGMA foreign_key_list({})", quote_ident(&t));
            let qr = self.query(&sql, &[], None).await?;
            for r in qr.rows {
                out.push(RelationshipInfo {
                    from_table: t.clone(),
                    from_column: r
                        .get("from")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    to_table: r
                        .get("table")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    to_column: r
                        .get("to")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
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

    async fn table_stats(
        &self,
        table: &str,
        _schema: Option<&str>,
    ) -> Result<TableStats, DriverError> {
        let sql = format!("PRAGMA index_list({})", quote_ident(table));
        let qr = self.query(&sql, &[], None).await?;
        let mut indexes = Vec::new();
        for r in qr.rows {
            let name = r
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let unique = r.get("unique").and_then(|v| v.as_i64()).unwrap_or(0) == 1;
            let cols_qr = self
                .query(
                    &format!("PRAGMA index_info({})", quote_ident(&name)),
                    &[],
                    None,
                )
                .await?;
            let cols: Vec<String> = cols_qr
                .rows
                .into_iter()
                .filter_map(|v| {
                    v.get("name")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string())
                })
                .collect();
            indexes.push(IndexInfo {
                name,
                columns: cols,
                unique,
            });
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
        let qr = self.query("PRAGMA database_list", &[], None).await?;
        Ok(qr
            .rows
            .into_iter()
            .filter_map(|v| {
                v.get("name")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string())
            })
            .collect())
    }

    async fn get_full_schema(&self, _schema: Option<&str>) -> Result<String, DriverError> {
        let tables = self.list_tables(None).await?;
        let mut lines: Vec<String> = Vec::new();
        for t in tables {
            let cols = self.describe_table(&t, None).await?;
            // Need PK info for full schema: fetch pragma directly to get pk flag
            let pk_qr = self
                .query(
                    &format!("PRAGMA table_info({})", quote_ident(&t)),
                    &[],
                    None,
                )
                .await?;
            let fks = self.list_relationships(Some(&t), None).await?;
            lines.push(format!("TABLE {t} ("));
            for (i, c) in cols.iter().enumerate() {
                let pk_val = pk_qr
                    .rows
                    .get(i)
                    .and_then(|r| r.get("pk"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let pk = if pk_val != 0 { " PRIMARY KEY" } else { "" };
                let nullability = if c.nullable { "NULL" } else { "NOT NULL" };
                lines.push(format!("  {} {} {}{}", c.column, c.r#type, nullability, pk));
            }
            lines.push(")".into());
            for fk in fks {
                lines.push(format!(
                    "FK {}.{} -> {}.{}",
                    t, fk.from_column, fk.to_table, fk.to_column
                ));
            }
            lines.push(String::new());
        }
        Ok(lines.join("\n").trim().to_string())
    }

    async fn test_connection(&self) -> Result<(), DriverError> {
        self.query("SELECT 1", &[], None).await.map(|_| ())
    }

    async fn close(&self) -> Result<(), DriverError> {
        // Mark closed; actual Connection drop happens on struct drop.
        if let Ok(mut c) = self.closed.lock() {
            *c = true;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::Driver;
    use tempfile::NamedTempFile;

    fn temp_path() -> (NamedTempFile, String) {
        let f = NamedTempFile::new().expect("tempfile");
        let p = f.path().to_string_lossy().to_string();
        (f, p)
    }

    #[tokio::test]
    async fn read_only_rejects_write_then_recovers() {
        let (_tmp, path) = temp_path();
        let d = SqliteDriver::open(&path).expect("open");
        d.query("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)", &[], None)
            .await
            .unwrap();
        d.query("INSERT INTO t (v) VALUES ('hello')", &[], None)
            .await
            .unwrap();

        // Read-only write must be rejected via PRAGMA query_only
        let err = d
            .query_read_only("INSERT INTO t (v) VALUES ('bad')", &[], None)
            .await
            .unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("readonly") || msg.contains("read-only") || msg.contains("query_only"),
            "expected read-only error, got: {msg}"
        );

        // Subsequent read-write must succeed — pragma was cleared
        d.query("INSERT INTO t (v) VALUES ('after')", &[], None)
            .await
            .expect("pragma should have been cleared");
        let qr = d
            .query("SELECT COUNT(*) as c FROM t", &[], None)
            .await
            .unwrap();
        let c = qr.rows[0].get("c").and_then(|v| v.as_i64()).unwrap();
        assert_eq!(c, 2, "both inserts should be present");

        // Read-only read must still work
        let ro = d
            .query_read_only("SELECT * FROM t ORDER BY id", &[], None)
            .await
            .unwrap();
        assert_eq!(ro.rows.len(), 2);
    }

    #[tokio::test]
    async fn introspection_output_shape() {
        let (_tmp, path) = temp_path();
        let d = SqliteDriver::open(&path).unwrap();
        d.query(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
            &[],
            None,
        )
        .await
        .unwrap();
        d.query(
            "CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER REFERENCES users(id), amount REAL)",
            &[],
            None,
        )
        .await
        .unwrap();

        let tables = d.list_tables(None).await.unwrap();
        assert!(tables.contains(&"users".to_string()));
        assert!(tables.contains(&"orders".to_string()));

        let cols = d.describe_table("users", None).await.unwrap();
        assert_eq!(cols.len(), 2);
        let id_col = cols.iter().find(|c| c.column == "id").unwrap();
        assert_eq!(id_col.r#type.to_uppercase(), "INTEGER");
        // id is PRIMARY KEY, nullable false (notnull=0 but pk); our mapping uses notnull flag
        let name_col = cols.iter().find(|c| c.column == "name").unwrap();
        assert!(!name_col.nullable, "TEXT NOT NULL should be nullable=false");

        let schemas = d.list_schemas().await.unwrap();
        assert_eq!(schemas, vec!["main"]);

        let dbs = d.list_databases().await.unwrap();
        assert!(dbs.contains(&"main".to_string()));

        let rels = d.list_relationships(Some("orders"), None).await.unwrap();
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].from_table, "orders");
        assert_eq!(rels[0].to_table, "users");

        let stats = d.table_stats("users", None).await.unwrap();
        assert_eq!(stats.table, "users");
        assert!(stats.estimated_rows.is_none());
        assert!(stats.size_bytes.is_none());

        let full = d.get_full_schema(None).await.unwrap();
        assert!(full.contains("TABLE users"));
        assert!(full.contains("TABLE orders"));
        assert!(full.contains("FK orders.user_id -> users.id"));

        d.test_connection().await.expect("test_connection");
    }

    #[tokio::test]
    async fn row_capping_via_helper() {
        let (_tmp, path) = temp_path();
        let d = SqliteDriver::open(&path).unwrap();
        d.query("CREATE TABLE t (x INTEGER)", &[], None)
            .await
            .unwrap();
        for i in 0..10 {
            d.query(
                "INSERT INTO t (x) VALUES (?)",
                &[serde_json::json!(i)],
                None,
            )
            .await
            .unwrap();
        }
        let qr = d.sample_table("t", 100, None).await.unwrap();
        assert_eq!(qr.rows.len(), 10);

        // Cap to 3 via capping helper (mirrors R09 behaviour)
        let (capped, was_capped, total) = crate::capping::cap_rows(qr.rows.clone(), 3);
        assert_eq!(capped.len(), 3);
        assert!(was_capped);
        assert_eq!(total, 10);

        // Full driver query without cap returns all
        let all = d
            .query("SELECT * FROM t ORDER BY x", &[], None)
            .await
            .unwrap();
        assert_eq!(all.rows.len(), 10);
    }

    #[tokio::test]
    async fn explain_returns_rows() {
        let (_tmp, path) = temp_path();
        let d = SqliteDriver::open(&path).unwrap();
        d.query("CREATE TABLE t (x INTEGER)", &[], None)
            .await
            .unwrap();
        let qr = d.explain("SELECT * FROM t WHERE x = 1", &[]).await.unwrap();
        // EXPLAIN QUERY PLAN returns rows with fields like selectid, order, from, detail
        assert!(!qr.rows.is_empty());
    }

    #[tokio::test]
    async fn sample_and_search_schema() {
        let (_tmp, path) = temp_path();
        let d = SqliteDriver::open(&path).unwrap();
        d.query(
            "CREATE TABLE my_table (my_col TEXT, other INTEGER)",
            &[],
            None,
        )
        .await
        .unwrap();
        d.query("INSERT INTO my_table VALUES ('a', 1)", &[], None)
            .await
            .unwrap();

        let sample = d.sample_table("my_table", 10, None).await.unwrap();
        assert_eq!(sample.rows.len(), 1);

        let results = d.search_schema("my_", None).await.unwrap();
        assert!(
            results
                .iter()
                .any(|r| r.kind == "table" && r.table == "my_table")
        );
        assert!(
            results
                .iter()
                .any(|r| r.kind == "column" && r.column.as_deref() == Some("my_col"))
        );
    }
}
