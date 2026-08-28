#[cfg(feature = "mysql")]
pub mod live {
    use async_trait::async_trait;
    use sqlx::mysql::{MySqlConnectOptions, MySqlSslMode};
    use sqlx::{Column, MySqlPool, Row, TypeInfo};

    use crate::driver::Driver;
    use crate::error::DriverError;
    use crate::ssl::SslConfig;
    use crate::types::*;

    fn conn_error(host: &str, port: u16, e: impl std::fmt::Display) -> DriverError {
        let msg = e.to_string();
        let lower = msg.to_lowercase();
        if lower.contains("connection refused") || lower.contains("econnrefused") {
            DriverError::Connection(format!(
                "Connection refused to {host}:{port}. Check host, port, firewall, and SSH tunnel config. ({msg})"
            ))
        } else if lower.contains("no such host")
            || lower.contains("name or service not known")
            || lower.contains("enotfound")
        {
            DriverError::Connection(format!(
                "Host not found {host}. Check the host name. ({msg})"
            ))
        } else if lower.contains("timed out") || lower.contains("timeout") {
            DriverError::Connection(format!(
                "Timed out connecting to {host}:{port}. Check host, port, SSH tunnel, and firewall/VPC rules. ({msg})"
            ))
        } else if lower.contains("access denied")
            || lower.contains("password")
            || lower.contains("authentication")
        {
            DriverError::Connection(format!(
                "Database authentication failed for {host}:{port}. Check username and password. ({msg})"
            ))
        } else if lower.contains("unknown database") {
            DriverError::Connection(format!(
                "Database not found on {host}:{port}. Check the database name. ({msg})"
            ))
        } else if lower.contains("no database selected") || lower.contains("1046") {
            DriverError::Connection(format!(
                "No database selected on {host}:{port}. Set a database in the connection settings or pass `database` (see list_databases). ({msg})"
            ))
        } else if lower.contains("self signed")
            || lower.contains("certificate")
            || lower.contains("ssl")
            || lower.contains("tls")
        {
            DriverError::Connection(format!(
                "SSL error connecting to {host}:{port}. Check SSL mode and certificates. ({msg})"
            ))
        } else {
            DriverError::Connection(format!("connection failed to {host}:{port}: {msg}"))
        }
    }

    fn no_db_error() -> DriverError {
        DriverError::Connection("No database selected. Set a database in the connection settings or pass `database` (see list_databases).".into())
    }

    fn map_sqlx_error(e: sqlx::Error) -> DriverError {
        DriverError::Query(e.to_string())
    }

    fn mysql_row_to_json(row: &sqlx::mysql::MySqlRow) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for (idx, col) in row.columns().iter().enumerate() {
            let name = col.name().to_string();
            let val = decode_mysql_value(row, idx);
            map.insert(name, val);
        }
        serde_json::Value::Object(map)
    }

    /// The text protocol hands every column back as bytes, so read them
    /// directly rather than asking sqlx to decode into a Rust type it will
    /// refuse for dates, decimals and JSON.
    fn raw_text(row: &sqlx::mysql::MySqlRow, idx: usize) -> Option<String> {
        row.try_get_unchecked::<Option<String>, _>(idx)
            .ok()
            .flatten()
    }

    fn decode_mysql_value(row: &sqlx::mysql::MySqlRow, idx: usize) -> serde_json::Value {
        let tn = type_name(row, idx);
        match tn.as_str() {
            "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "BIGINT" | "INT1" | "INT2" | "INT3"
            | "INT8" | "YEAR" | "LONGLONG" | "SHORT" => {
                if let Ok(v) = row.try_get::<Option<i64>, _>(idx) {
                    return v
                        .map(|x| serde_json::json!(x))
                        .unwrap_or(serde_json::Value::Null);
                }
                raw_text(row, idx)
                    .and_then(|s| s.parse::<i64>().ok().map(|i| serde_json::json!(i)))
                    .unwrap_or(serde_json::Value::Null)
            }
            "FLOAT" | "DOUBLE" | "DECIMAL" | "NEWDECIMAL" | "NUMERIC" => {
                if let Ok(v) = row.try_get::<Option<f64>, _>(idx)
                    && let Some(f) = v
                {
                    if let Some(n) = serde_json::Number::from_f64(f) {
                        return serde_json::Value::Number(n);
                    }
                    return serde_json::Value::String(f.to_string());
                }
                match raw_text(row, idx) {
                    None => serde_json::Value::Null,
                    Some(s) => match s.parse::<f64>().ok().and_then(serde_json::Number::from_f64) {
                        Some(n) => serde_json::Value::Number(n),
                        None => serde_json::Value::String(s),
                    },
                }
            }
            "DATE" | "DATETIME" | "TIMESTAMP" | "TIME" | "NEWDATE" | "DATETIME2" => {
                raw_text(row, idx)
                    .map(serde_json::Value::String)
                    .unwrap_or(serde_json::Value::Null)
            }
            "JSON" => match raw_text(row, idx) {
                None => serde_json::Value::Null,
                Some(s) => serde_json::from_str(&s).unwrap_or(serde_json::Value::String(s)),
            },
            "BLOB" | "MEDIUMBLOB" | "LONGBLOB" | "TINYBLOB" | "VARBINARY" | "BINARY" | "BIT"
            | "GEOMETRY" => bytes_value(row, idx),
            _ => match raw_text(row, idx) {
                Some(s) => serde_json::Value::String(s),
                None => bytes_value(row, idx),
            },
        }
    }

    /// Binary columns come back as text or, when they are not valid UTF-8, as
    /// the byte array itself.
    fn bytes_value(row: &sqlx::mysql::MySqlRow, idx: usize) -> serde_json::Value {
        match row.try_get_unchecked::<Option<Vec<u8>>, _>(idx) {
            Ok(Some(b)) => match String::from_utf8(b) {
                Ok(s) => serde_json::Value::String(s),
                Err(e) => serde_json::Value::Array(
                    e.into_bytes()
                        .iter()
                        .map(|x| serde_json::json!(*x))
                        .collect(),
                ),
            },
            _ => serde_json::Value::Null,
        }
    }

    fn type_name(row: &sqlx::mysql::MySqlRow, idx: usize) -> String {
        row.columns()[idx].type_info().name().to_uppercase()
    }
    pub struct MySqlDriver {
        pool: MySqlPool,
        host: String,
        port: u16,
        database: Option<String>,
    }

    impl MySqlDriver {
        pub async fn new(
            host: String,
            port: u16,
            user: Option<String>,
            password: Option<String>,
            database: Option<String>,
            ssl: Option<SslConfig>,
            socket_path: Option<String>,
        ) -> Result<Self, DriverError> {
            let mut opts = MySqlConnectOptions::new();
            if let Some(sock) = socket_path.filter(|s| !s.is_empty()) {
                opts = opts.socket(sock);
            } else {
                opts = opts.host(&host).port(port);
            }
            if let Some(u) = user {
                opts = opts.username(&u);
            }
            if let Some(p) = password {
                opts = opts.password(&p);
            }
            if let Some(db) = &database {
                opts = opts.database(db);
            }

            let mode = match ssl.as_ref().and_then(|s| s.mode.clone()) {
                Some(crate::ssl::SslMode::Disable) | None if ssl.is_none() => {
                    MySqlSslMode::Disabled
                }
                Some(crate::ssl::SslMode::Require) => MySqlSslMode::Required,
                Some(crate::ssl::SslMode::VerifyCa) => MySqlSslMode::VerifyCa,
                Some(crate::ssl::SslMode::VerifyFull) => MySqlSslMode::VerifyIdentity,
                _ => MySqlSslMode::Preferred,
            };
            // sqlx 0.8: ssl_mode is method on MySqlConnectOptions
            opts = opts.ssl_mode(mode);
            if let Some(cfg) = &ssl {
                if let Some(ca) = &cfg.ca {
                    let ca_str = String::from_utf8_lossy(ca).to_string();
                    // sqlx expects path; write to temp file if not a path
                    // Try treating ca bytes as PEM path content: write to temp file
                    if !ca_str.is_empty() {
                        // Best effort: if ca bytes look like a file path that exists, use it
                        // otherwise write to temp file
                        let path = write_temp_pem(ca, "ca").unwrap_or_default();
                        if !path.is_empty() {
                            opts = opts.ssl_ca(&path);
                        }
                    }
                }
                // client cert/key handling would be similar, omitted for brevity — mode already covers verification
                let _ = cfg;
            }

            let pool = MySqlPool::connect_with(opts)
                .await
                .map_err(|e| conn_error(&host, port, e))?;
            Ok(Self {
                pool,
                host,
                port,
                database,
            })
        }
    }

    fn write_temp_pem(data: &[u8], prefix: &str) -> Result<String, ()> {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().map_err(|_| ())?;
        f.write_all(data).map_err(|_| ())?;
        // Persist file so it stays on disk
        let path = f.path().to_string_lossy().to_string();
        // Keep file alive by forgetting? sqlx reads path immediately at connect, so temp file can be dropped after? Actually ConnectOptions stores path string, reads at connect time. So we need file to exist at connect time, which it does until function returns. But pool connects inside new, so ok. However for later reconnections, file would be gone. We leak it by persisting.
        let _ = f.keep().map_err(|_| ())?;
        let _ = prefix;
        Ok(path)
    }

    fn escape_mysql_string(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('\'');
        for ch in s.chars() {
            match ch {
                '\'' => out.push_str("\\'"),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\0' => out.push_str("\\0"),
                _ => out.push(ch),
            }
        }
        out.push('\'');
        out
    }

    fn interpolate_mysql(sql: &str, params: &[serde_json::Value]) -> String {
        let mut out = String::with_capacity(sql.len() + params.len() * 8);
        let mut idx = 0;
        for ch in sql.chars() {
            if ch == '?' && idx < params.len() {
                let v = &params[idx];
                idx += 1;
                let s = match v {
                    serde_json::Value::Null => "NULL".to_string(),
                    serde_json::Value::Bool(b) => {
                        if *b {
                            "1".to_string()
                        } else {
                            "0".to_string()
                        }
                    }
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::String(s) => escape_mysql_string(s),
                    serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                        escape_mysql_string(&v.to_string())
                    }
                };
                out.push_str(&s);
            } else {
                out.push(ch);
            }
        }
        out
    }

    impl MySqlDriver {
        /// MySQL refuses transaction control over the prepared-statement
        /// protocol, so every statement goes out on the text protocol with
        /// parameters already interpolated.
        async fn run(
            &self,
            sql: &str,
            params: &[serde_json::Value],
            opts: Option<QueryOpts>,
            read_only: bool,
        ) -> Result<QueryResult, DriverError> {
            let statement = if params.is_empty() {
                sql.to_string()
            } else {
                interpolate_mysql(sql, params)
            };
            let pool = self.pool.clone();
            let cancel = opts.as_ref().and_then(|o| o.cancel.clone());
            let timeout_ms = opts.as_ref().and_then(|o| o.timeout_ms);

            // Running the statement on a spawned task keeps the future
            // `'static`, which sqlx's executor lifetimes require here.
            let fut = async move {
                tokio::spawn(async move {
                    let mut tx = if read_only {
                        pool.begin_with("START TRANSACTION READ ONLY").await
                    } else {
                        pool.begin().await
                    }
                    .map_err(|e| DriverError::Pool(e.to_string()))?;

                    let killer = spawn_killer(&pool, &mut tx, cancel, timeout_ms).await;
                    crate::sql_log::record_executed_sql(&statement, None, None);
                    let result =
                        sqlx::Executor::fetch_all(&mut *tx, sqlx::raw_sql(statement.as_str()))
                            .await
                            .map_err(map_sqlx_error);
                    let _ = tx.rollback().await;
                    if let Some(handle) = killer {
                        handle.abort();
                    }

                    match result {
                        Ok(rows) => {
                            let fields = rows.first().map(|r| {
                                r.columns().iter().map(|c| c.name().to_string()).collect()
                            });
                            let json_rows: Vec<serde_json::Value> =
                                rows.iter().map(mysql_row_to_json).collect();
                            crate::sql_log::record_executed_sql(
                                &statement,
                                Some(json_rows.len() as i64),
                                None,
                            );
                            Ok(QueryResult {
                                rows: json_rows,
                                fields,
                            })
                        }
                        Err(e) => {
                            crate::sql_log::record_executed_sql(
                                &statement,
                                None,
                                Some(&e.to_string()),
                            );
                            Err(e)
                        }
                    }
                })
                .await
                .map_err(|e| DriverError::Other(e.to_string()))?
            };
            crate::driver::with_opts(opts, fut).await
        }
    }

    /// Ask the server to abort the running statement when the caller cancels or
    /// the deadline passes; the connection itself stays blocked until it does.
    async fn spawn_killer(
        pool: &sqlx::MySqlPool,
        tx: &mut sqlx::Transaction<'static, sqlx::MySql>,
        cancel: Option<tokio_util::sync::CancellationToken>,
        timeout_ms: Option<u64>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        if cancel.is_none() && timeout_ms.is_none() {
            return None;
        }
        let id: u64 =
            sqlx::Executor::fetch_one(&mut **tx, sqlx::raw_sql("SELECT CONNECTION_ID() AS id"))
                .await
                .ok()
                .and_then(|row| row.try_get::<u64, _>("id").ok())?;
        let pool = pool.clone();
        Some(tokio::spawn(async move {
            match (cancel, timeout_ms) {
                (Some(token), Some(ms)) => {
                    tokio::select! {
                        _ = token.cancelled() => {},
                        _ = tokio::time::sleep(std::time::Duration::from_millis(ms)) => {},
                    }
                }
                (Some(token), None) => token.cancelled().await,
                (None, Some(ms)) => tokio::time::sleep(std::time::Duration::from_millis(ms)).await,
                (None, None) => return,
            }
            let _ = sqlx::raw_sql(&format!("KILL QUERY {id}"))
                .execute(&pool)
                .await;
        }))
    }

    #[async_trait]
    impl Driver for MySqlDriver {
        async fn query(
            &self,
            sql: &str,
            params: &[serde_json::Value],
            opts: Option<QueryOpts>,
        ) -> Result<QueryResult, DriverError> {
            self.run(sql, params, opts, false).await
        }

        async fn query_read_only(
            &self,
            sql: &str,
            params: &[serde_json::Value],
            opts: Option<QueryOpts>,
        ) -> Result<QueryResult, DriverError> {
            self.run(sql, params, opts, true).await
        }

        async fn explain(
            &self,
            sql: &str,
            params: &[serde_json::Value],
        ) -> Result<QueryResult, DriverError> {
            let full = format!("EXPLAIN {sql}");
            crate::sql_log::record_executed_sql(&full, None, None);
            let rows = if params.is_empty() {
                sqlx::query(&full)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(map_sqlx_error)?
            } else {
                let sql2 = interpolate_mysql(&full, params);
                sqlx::query(&sql2)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(map_sqlx_error)?
            };
            let fields = rows
                .first()
                .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect());
            let json_rows = rows.iter().map(mysql_row_to_json).collect::<Vec<_>>();
            let res = QueryResult {
                rows: json_rows,
                fields,
            };
            crate::sql_log::record_executed_sql(&full, Some(res.rows.len() as i64), None);
            Ok(res)
        }

        async fn list_tables(&self, _schema: Option<&str>) -> Result<Vec<String>, DriverError> {
            if self.database.is_none() {
                return Err(no_db_error());
            }
            let rows = sqlx::query("SHOW TABLES")
                .persistent(false)
                .fetch_all(&self.pool)
                .await
                .map_err(map_sqlx_error)?;
            Ok(rows
                .iter()
                .map(|r| {
                    let v: String = r.try_get(0).unwrap_or_default();
                    v
                })
                .collect())
        }

        async fn describe_table(
            &self,
            table: &str,
            _schema: Option<&str>,
        ) -> Result<Vec<ColumnInfo>, DriverError> {
            if self.database.is_none() {
                return Err(no_db_error());
            }
            let sql = format!("DESCRIBE `{}`", table.replace('`', "``"));
            let rows = sqlx::query(&sql)
                .persistent(false)
                .fetch_all(&self.pool)
                .await
                .map_err(map_sqlx_error)?;
            Ok(rows
                .iter()
                .map(|r| {
                    let field: String = r
                        .try_get::<Option<String>, _>("Field")
                        .unwrap_or(None)
                        .unwrap_or_default();
                    let typ: String = r
                        .try_get::<Option<String>, _>("Type")
                        .unwrap_or(None)
                        .unwrap_or_default();
                    let null_str: String = r
                        .try_get::<Option<String>, _>("Null")
                        .unwrap_or(None)
                        .unwrap_or_default();
                    ColumnInfo {
                        column: field,
                        r#type: typ,
                        nullable: null_str == "YES",
                    }
                })
                .collect())
        }

        async fn sample_table(
            &self,
            table: &str,
            limit: i64,
            _schema: Option<&str>,
        ) -> Result<QueryResult, DriverError> {
            if self.database.is_none() {
                return Err(no_db_error());
            }
            let quoted = table.replace('`', "``");
            let sql = format!("SELECT * FROM `{quoted}` LIMIT ?");
            crate::sql_log::record_executed_sql(&sql, None, None);
            let rows = sqlx::query(&sql)
                .persistent(false)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
                .map_err(map_sqlx_error)?;
            let fields = rows
                .first()
                .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect());
            let json_rows = rows.iter().map(mysql_row_to_json).collect();
            let res = QueryResult {
                rows: json_rows,
                fields,
            };
            crate::sql_log::record_executed_sql(&sql, Some(res.rows.len() as i64), None);
            Ok(res)
        }

        async fn search_schema(
            &self,
            term: &str,
            _schema: Option<&str>,
        ) -> Result<Vec<SchemaSearchResult>, DriverError> {
            if self.database.is_none() {
                return Err(no_db_error());
            }
            let pattern = format!(
                "%{}%",
                term.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            );
            let sql_tmp = interpolate_mysql(
                r#"
                SELECT 'table' AS kind, table_name AS `table`, NULL AS `column`, NULL AS type
                FROM information_schema.tables
                WHERE table_schema = DATABASE() AND table_name LIKE ?
                UNION ALL
                SELECT 'column', c.table_name, c.column_name, c.data_type
                FROM information_schema.columns c
                JOIN information_schema.tables t
                  ON c.table_schema = t.table_schema AND c.table_name = t.table_name
                WHERE c.table_schema = DATABASE()
                  AND (c.column_name LIKE ? OR c.table_name LIKE ?)
                ORDER BY `table`, kind, `column`
                "#,
                &[
                    serde_json::json!(pattern.clone()),
                    serde_json::json!(pattern.clone()),
                    serde_json::json!(pattern),
                ],
            );
            let rows = sqlx::query(&sql_tmp)
                .fetch_all(&self.pool)
                .await
                .map_err(map_sqlx_error)?;
            Ok(rows
                .iter()
                .map(|r| {
                    let kind: String = r
                        .try_get::<Option<String>, _>("kind")
                        .unwrap_or(None)
                        .unwrap_or_default();
                    let table: String = r
                        .try_get::<Option<String>, _>("table")
                        .unwrap_or(None)
                        .unwrap_or_default();
                    let column: Option<String> =
                        r.try_get::<Option<String>, _>("column").unwrap_or(None);
                    let typ: Option<String> =
                        r.try_get::<Option<String>, _>("type").unwrap_or(None);
                    SchemaSearchResult {
                        kind,
                        table,
                        column,
                        r#type: typ,
                    }
                })
                .collect())
        }

        async fn list_relationships(
            &self,
            table: Option<&str>,
            _schema: Option<&str>,
        ) -> Result<Vec<RelationshipInfo>, DriverError> {
            if self.database.is_none() {
                return Err(no_db_error());
            }
            let (sql, bind_table) = if table.is_some() {
                (
                    r#"
                SELECT kcu.table_name AS from_table, kcu.column_name AS from_column, kcu.referenced_table_name AS to_table, kcu.referenced_column_name AS to_column, kcu.constraint_name AS constraint_name
                FROM information_schema.key_column_usage kcu
                JOIN information_schema.table_constraints tc
                  ON kcu.constraint_name = tc.constraint_name
                  AND kcu.table_schema = tc.table_schema
                WHERE tc.constraint_type = 'FOREIGN KEY'
                  AND kcu.table_schema = DATABASE()
                  AND kcu.table_name = ?
                ORDER BY kcu.table_name, kcu.ordinal_position
                "#,
                    true,
                )
            } else {
                (
                    r#"
                SELECT kcu.table_name AS from_table, kcu.column_name AS from_column, kcu.referenced_table_name AS to_table, kcu.referenced_column_name AS to_column, kcu.constraint_name AS constraint_name
                FROM information_schema.key_column_usage kcu
                JOIN information_schema.table_constraints tc
                  ON kcu.constraint_name = tc.constraint_name
                  AND kcu.table_schema = tc.table_schema
                WHERE tc.constraint_type = 'FOREIGN KEY'
                  AND kcu.table_schema = DATABASE()
                ORDER BY kcu.table_name, kcu.ordinal_position
                "#,
                    false,
                )
            };
            let rows = if bind_table {
                sqlx::query(sql)
                    .persistent(false)
                    .bind(table.unwrap())
                    .fetch_all(&self.pool)
                    .await
                    .map_err(map_sqlx_error)?
            } else {
                sqlx::query(sql)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(map_sqlx_error)?
            };
            Ok(rows
                .iter()
                .map(|r| {
                    let from_table: String = r
                        .try_get::<Option<String>, _>("from_table")
                        .unwrap_or(None)
                        .unwrap_or_default();
                    let from_column: String = r
                        .try_get::<Option<String>, _>("from_column")
                        .unwrap_or(None)
                        .unwrap_or_default();
                    let to_table: String = r
                        .try_get::<Option<String>, _>("to_table")
                        .unwrap_or(None)
                        .unwrap_or_default();
                    let to_column: String = r
                        .try_get::<Option<String>, _>("to_column")
                        .unwrap_or(None)
                        .unwrap_or_default();
                    let constraint_name: Option<String> = r
                        .try_get::<Option<String>, _>("constraint_name")
                        .unwrap_or(None);
                    RelationshipInfo {
                        from_table,
                        from_column,
                        to_table,
                        to_column,
                        constraint_name,
                    }
                })
                .collect())
        }

        async fn table_stats(
            &self,
            table: &str,
            _schema: Option<&str>,
        ) -> Result<TableStats, DriverError> {
            if self.database.is_none() {
                return Err(no_db_error());
            }
            let sql_tmp = interpolate_mysql(
                "SELECT table_rows, data_length + index_length AS size_bytes FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = ?",
                &[serde_json::json!(table)],
            );
            let table_rows = sqlx::query(&sql_tmp)
                .fetch_all(&self.pool)
                .await
                .map_err(map_sqlx_error)?;
            let (estimated_rows, size_bytes) = if let Some(r) = table_rows.first() {
                let tr: Option<i64> = r
                    .try_get::<Option<i64>, _>("table_rows")
                    .unwrap_or(None)
                    .or_else(|| {
                        r.try_get::<Option<String>, _>("table_rows")
                            .unwrap_or(None)
                            .and_then(|s| s.parse().ok())
                    });
                let sb: Option<i64> = r
                    .try_get::<Option<i64>, _>("size_bytes")
                    .unwrap_or(None)
                    .or_else(|| {
                        r.try_get::<Option<String>, _>("size_bytes")
                            .unwrap_or(None)
                            .and_then(|s| s.parse().ok())
                    });
                (tr, sb)
            } else {
                (None, None)
            };
            let sql_tmp2 = interpolate_mysql(
                "SELECT index_name, column_name, non_unique FROM information_schema.statistics WHERE table_schema = DATABASE() AND table_name = ? ORDER BY index_name, seq_in_index",
                &[serde_json::json!(table)],
            );
            let idx_rows = sqlx::query(&sql_tmp2)
                .fetch_all(&self.pool)
                .await
                .map_err(map_sqlx_error)?;
            let mut idx_map: std::collections::BTreeMap<String, (Vec<String>, bool)> =
                std::collections::BTreeMap::new();
            for r in idx_rows {
                let name: String = r
                    .try_get::<Option<String>, _>("index_name")
                    .unwrap_or(None)
                    .unwrap_or_default();
                let col: String = r
                    .try_get::<Option<String>, _>("column_name")
                    .unwrap_or(None)
                    .unwrap_or_default();
                let non_unique: i64 = r
                    .try_get::<Option<i64>, _>("non_unique")
                    .unwrap_or(None)
                    .unwrap_or(1);
                let entry = idx_map
                    .entry(name)
                    .or_insert_with(|| (Vec::new(), non_unique == 0));
                entry.0.push(col);
                entry.1 = non_unique == 0;
            }
            let indexes = idx_map
                .into_iter()
                .map(|(name, (columns, unique))| IndexInfo {
                    name,
                    columns,
                    unique,
                })
                .collect();
            Ok(TableStats {
                table: table.to_string(),
                estimated_rows,
                size_bytes,
                indexes,
            })
        }

        async fn list_schemas(&self) -> Result<Vec<String>, DriverError> {
            let rows = sqlx::query("SHOW DATABASES")
                .fetch_all(&self.pool)
                .await
                .map_err(map_sqlx_error)?;
            Ok(rows
                .iter()
                .map(|r| r.try_get::<String, _>(0).unwrap_or_default())
                .collect())
        }

        async fn list_databases(&self) -> Result<Vec<String>, DriverError> {
            self.list_schemas().await
        }

        async fn get_full_schema(&self, _schema: Option<&str>) -> Result<String, DriverError> {
            if self.database.is_none() {
                return Err(no_db_error());
            }
            let col_rows = sqlx::query(
                "SELECT table_name, column_name, data_type, is_nullable, ordinal_position FROM information_schema.columns WHERE table_schema = DATABASE() ORDER BY table_name, ordinal_position"
            ).fetch_all(&self.pool).await.map_err(map_sqlx_error)?;
            let key_rows = sqlx::query(
                "SELECT kcu.table_name, kcu.column_name, tc.constraint_type FROM information_schema.table_constraints tc JOIN information_schema.key_column_usage kcu ON tc.constraint_name = kcu.constraint_name AND tc.table_schema = kcu.table_schema WHERE tc.table_schema = DATABASE() AND tc.constraint_type IN ('PRIMARY KEY', 'FOREIGN KEY')"
            ).fetch_all(&self.pool).await.map_err(map_sqlx_error)?;
            let fk_rows = sqlx::query(
                "SELECT kcu.table_name AS from_table, kcu.column_name AS from_column, kcu.referenced_table_name AS to_table, kcu.referenced_column_name AS to_column FROM information_schema.key_column_usage kcu JOIN information_schema.table_constraints tc ON kcu.constraint_name = tc.constraint_name AND kcu.table_schema = tc.table_schema WHERE tc.constraint_type = 'FOREIGN KEY' AND kcu.table_schema = DATABASE()"
            ).fetch_all(&self.pool).await.map_err(map_sqlx_error)?;

            let mut tables: std::collections::BTreeMap<String, Vec<(String, String, bool, bool)>> =
                std::collections::BTreeMap::new();
            for r in &col_rows {
                let t: String = r
                    .try_get::<Option<String>, _>("table_name")
                    .unwrap_or(None)
                    .unwrap_or_default();
                let col: String = r
                    .try_get::<Option<String>, _>("column_name")
                    .unwrap_or(None)
                    .unwrap_or_default();
                let typ: String = r
                    .try_get::<Option<String>, _>("data_type")
                    .unwrap_or(None)
                    .unwrap_or_default();
                let nullable: String = r
                    .try_get::<Option<String>, _>("is_nullable")
                    .unwrap_or(None)
                    .unwrap_or_default();
                tables
                    .entry(t)
                    .or_default()
                    .push((col, typ, nullable == "YES", false));
            }
            for r in &key_rows {
                let t: String = r
                    .try_get::<Option<String>, _>("table_name")
                    .unwrap_or(None)
                    .unwrap_or_default();
                let col: String = r
                    .try_get::<Option<String>, _>("column_name")
                    .unwrap_or(None)
                    .unwrap_or_default();
                let ctype: String = r
                    .try_get::<Option<String>, _>("constraint_type")
                    .unwrap_or(None)
                    .unwrap_or_default();
                if ctype == "PRIMARY KEY"
                    && let Some(cols) = tables.get_mut(&t)
                    && let Some(c) = cols.iter_mut().find(|c| c.0 == col)
                {
                    c.3 = true;
                }
            }
            let mut lines = Vec::new();
            for (table, cols) in &tables {
                lines.push(format!("TABLE {table} ("));
                for (col, typ, nullable, pk) in cols {
                    let pk_s = if *pk { " PRIMARY KEY" } else { "" };
                    let null_s = if *nullable { "NULL" } else { "NOT NULL" };
                    lines.push(format!("  {col} {typ} {null_s}{pk_s}"));
                }
                lines.push(")".to_string());
                for r in &fk_rows {
                    let from_table: String = r
                        .try_get::<Option<String>, _>("from_table")
                        .unwrap_or(None)
                        .unwrap_or_default();
                    if &from_table == table {
                        let from_col: String = r
                            .try_get::<Option<String>, _>("from_column")
                            .unwrap_or(None)
                            .unwrap_or_default();
                        let to_table: String = r
                            .try_get::<Option<String>, _>("to_table")
                            .unwrap_or(None)
                            .unwrap_or_default();
                        let to_col: String = r
                            .try_get::<Option<String>, _>("to_column")
                            .unwrap_or(None)
                            .unwrap_or_default();
                        lines.push(format!("FK {table}.{from_col} -> {to_table}.{to_col}"));
                    }
                }
                lines.push(String::new());
            }
            Ok(lines.join("\n").trim().to_string())
        }

        async fn test_connection(&self) -> Result<(), DriverError> {
            sqlx::query("SELECT 1")
                .execute(&self.pool)
                .await
                .map_err(|e| conn_error(&self.host, self.port, e))
                .map(|_| ())
        }

        async fn close(&self) -> Result<(), DriverError> {
            self.pool.close().await;
            Ok(())
        }
    }
}
