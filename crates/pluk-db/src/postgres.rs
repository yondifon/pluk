#[cfg(feature = "postgres")]
pub mod live {
    use async_trait::async_trait;
    use deadpool_postgres::{Config, ManagerConfig, RecyclingMethod, Runtime};
    use tokio_postgres::types::Type;
    use tokio_postgres::NoTls;

    use crate::driver::Driver;
    use crate::error::DriverError;
    use crate::ssl::SslConfig;
    use crate::types::*;

    fn conn_error(host: &str, port: u16, e: impl std::fmt::Display) -> DriverError {
        let msg = e.to_string();
        let lower = msg.to_lowercase();
        if lower.contains("connection refused") || lower.contains("econnrefused") {
            DriverError::Connection(format!("Connection refused to {host}:{port}. Check host, port, firewall, and SSH tunnel config. ({msg})"))
        } else if lower.contains("no such host") || lower.contains("name or service not known") || lower.contains("enotfound") {
            DriverError::Connection(format!("Host not found {host}. Check the host name. ({msg})"))
        } else if lower.contains("timed out") || lower.contains("timeout") {
            DriverError::Connection(format!("Timed out connecting to {host}:{port}. Check host, port, SSH tunnel, and firewall/VPC rules. ({msg})"))
        } else if lower.contains("password") || lower.contains("authentication") || lower.contains("28p01") || lower.contains("28000") {
            DriverError::Connection(format!("Database authentication failed for {host}:{port}. Check username and password. ({msg})"))
        } else if lower.contains("database") && lower.contains("does not exist") || lower.contains("3d000") {
            DriverError::Connection(format!("Database not found on {host}:{port}. Check the database name. ({msg})"))
        } else if lower.contains("self signed") || lower.contains("certificate") || lower.contains("ssl") || lower.contains("tls") {
            DriverError::Connection(format!("SSL error connecting to {host}:{port}. Check SSL mode and certificates. ({msg})"))
        } else {
            DriverError::Connection(format!("connection failed to {host}:{port}: {msg}"))
        }
    }

    fn map_query_error(e: tokio_postgres::Error) -> DriverError {
        let msg = e.to_string();
        if let Some(db_err) = e.as_db_error() {
            let code = db_err.code().code().to_string();
            return DriverError::Query(format!("{msg} (code {code})"));
        }
        DriverError::Query(msg)
    }

    fn build_pg_params(params: &[serde_json::Value]) -> Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> {
        params
            .iter()
            .map(|v| -> Box<dyn tokio_postgres::types::ToSql + Sync + Send> {
                match v {
                    serde_json::Value::Null => Box::new(Option::<String>::None),
                    serde_json::Value::Bool(b) => Box::new(*b),
                    serde_json::Value::Number(n) => {
                        if let Some(i) = n.as_i64() {
                            Box::new(i)
                        } else if let Some(u) = n.as_u64() {
                            if u <= i64::MAX as u64 {
                                Box::new(u as i64)
                            } else {
                                Box::new(n.to_string())
                            }
                        } else if let Some(f) = n.as_f64() {
                            Box::new(f)
                        } else {
                            Box::new(n.to_string())
                        }
                    }
                    serde_json::Value::String(s) => Box::new(s.clone()),
                    serde_json::Value::Array(_) | serde_json::Value::Object(_) => Box::new(v.to_string()),
                }
            })
            .collect()
    }

    fn pg_row_to_json(row: &tokio_postgres::Row) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for (idx, col) in row.columns().iter().enumerate() {
            let ty = col.type_();
            let val = match *ty {
                Type::BOOL => {
                    let v: Option<bool> = row.get(idx);
                    v.map(serde_json::Value::Bool).unwrap_or(serde_json::Value::Null)
                }
                Type::INT2 => {
                    let v: Option<i16> = row.get(idx);
                    v.map(|x| serde_json::json!(x as i64)).unwrap_or(serde_json::Value::Null)
                }
                Type::INT4 => {
                    let v: Option<i32> = row.get(idx);
                    v.map(|x| serde_json::json!(x as i64)).unwrap_or(serde_json::Value::Null)
                }
                Type::INT8 | Type::OID => {
                    let v: Option<i64> = row.get(idx);
                    v.map(|x| serde_json::json!(x)).unwrap_or(serde_json::Value::Null)
                }
                Type::FLOAT4 => {
                    let v: Option<f32> = row.get(idx);
                    v.and_then(|f| serde_json::Number::from_f64(f as f64).map(serde_json::Value::Number))
                        .unwrap_or(serde_json::Value::Null)
                }
                Type::FLOAT8 => {
                    let v: Option<f64> = row.get(idx);
                    v.and_then(|f| serde_json::Number::from_f64(f).map(serde_json::Value::Number))
                        .unwrap_or(serde_json::Value::Null)
                }
                Type::NUMERIC => {
                    let v: Option<String> = row.get(idx);
                    match v {
                        None => serde_json::Value::Null,
                        Some(s) => {
                            if let Ok(i) = s.parse::<i64>() {
                                serde_json::json!(i)
                            } else if let Ok(f) = s.parse::<f64>() {
                                serde_json::Number::from_f64(f).map(serde_json::Value::Number).unwrap_or(serde_json::Value::String(s))
                            } else {
                                serde_json::Value::String(s)
                            }
                        }
                    }
                }
                Type::BYTEA => {
                    let v: Option<Vec<u8>> = row.get(idx);
                    match v {
                        None => serde_json::Value::Null,
                        Some(b) => {
                            if let Ok(s) = String::from_utf8(b.clone()) {
                                serde_json::Value::String(s)
                            } else {
                                serde_json::Value::Array(b.iter().map(|x| serde_json::json!(*x)).collect())
                            }
                        }
                    }
                }
                Type::JSON | Type::JSONB => {
                    let s: Option<String> = row.get(idx);
                    match s {
                        None => serde_json::Value::Null,
                        Some(txt) => serde_json::from_str(&txt).unwrap_or(serde_json::Value::String(txt)),
                    }
                }
                _ => {
                    let v: Option<String> = row.get(idx);
                    v.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null)
                }
            };
            map.insert(col.name().to_string(), val);
        }
        serde_json::Value::Object(map)
    }

    fn rows_to_result(rows: Vec<tokio_postgres::Row>) -> QueryResult {
        let fields = rows.first().map(|r| r.columns().iter().map(|c| c.name().to_string()).collect());
        let json_rows = rows.iter().map(pg_row_to_json).collect();
        QueryResult { rows: json_rows, fields }
    }

    pub struct PostgresDriver {
        pool: deadpool_postgres::Pool,
        host: String,
        port: u16,
    }

    impl PostgresDriver {
        pub fn new(
            host: String,
            port: u16,
            user: Option<String>,
            password: Option<String>,
            database: Option<String>,
            ssl: Option<SslConfig>,
        ) -> Result<Self, DriverError> {
            let mut cfg = Config::new();
            cfg.host = Some(host.clone());
            cfg.port = Some(port);
            cfg.user = user.clone();
            cfg.password = password;
            cfg.dbname = database.or_else(|| user.or(Some("postgres".to_string())));
            cfg.manager = Some(ManagerConfig { recycling_method: RecyclingMethod::Fast });
            cfg.application_name = Some("pluk".to_string());

            let pool = if let Some(ssl_cfg) = ssl {
                if ssl_cfg.is_disabled() {
                    cfg.create_pool(Some(Runtime::Tokio1), NoTls)
                        .map_err(|e| DriverError::Pool(e.to_string()))?
                } else {
                    let mut builder = native_tls::TlsConnector::builder();
                    if let Some(ca) = &ssl_cfg.ca {
                        let cert = native_tls::Certificate::from_pem(ca)
                            .map_err(|e| DriverError::Ssl(format!("ca read error: {e}")))?;
                        builder.add_root_certificate(cert);
                    }
                    if let (Some(cert), Some(key)) = (&ssl_cfg.cert, &ssl_cfg.key) {
                        let identity = native_tls::Identity::from_pkcs8(cert, key)
                            .map_err(|e| DriverError::Ssl(format!("cert/key error: {e}")))?;
                        builder.identity(identity);
                    }
                    if !ssl_cfg.reject_unauthorized {
                        builder.danger_accept_invalid_certs(true);
                        builder.danger_accept_invalid_hostnames(true);
                    }
                    let connector = builder.build().map_err(|e| DriverError::Ssl(e.to_string()))?;
                    let tls = postgres_native_tls::MakeTlsConnector::new(connector);
                    cfg.create_pool(Some(Runtime::Tokio1), tls)
                        .map_err(|e| DriverError::Pool(e.to_string()))?
                }
            } else {
                cfg.create_pool(Some(Runtime::Tokio1), NoTls)
                    .map_err(|e| DriverError::Pool(e.to_string()))?
            };
            Ok(Self { pool, host, port })
        }

        async fn query_inner(
            &self,
            sql: &str,
            params: &[serde_json::Value],
            opts: Option<QueryOpts>,
        ) -> Result<QueryResult, DriverError> {
            if let Some(o) = opts.clone()
                && (o.cancel.is_some() || o.timeout_ms.is_some()) {
                    let pool2 = self.pool.clone();
                    let host2 = self.host.clone();
                    let port2 = self.port;
                    let sql2 = sql.to_string();
                    let params2 = params.to_vec();
                    let cancel = o.cancel.clone();
                    let timeout_ms = o.timeout_ms;
                    let fut = async move {
                        let client = pool2.get().await.map_err(|e| conn_error(&host2, port2, e))?;
                        let pid: i32 = client
                            .query("SELECT pg_backend_pid()", &[])
                            .await
                            .map_err(map_query_error)
                            .and_then(|r| {
                                r.first()
                                    .map(|row| row.get::<_, i32>(0))
                                    .ok_or_else(|| DriverError::Query("no pid".into()))
                            })?;
                        let owned = build_pg_params(&params2);
                        let refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
                            owned.iter().map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync)).collect();
                        let cancel_task = cancel.clone().map(|tok| {
                            let p = pool2.clone();
                            tokio::spawn(async move {
                                tok.cancelled().await;
                                if let Ok(c) = p.get().await {
                                    let _ = c.query("SELECT pg_cancel_backend($1)", &[&pid]).await;
                                }
                            })
                        });
                        let timeout_task = timeout_ms.map(|ms| {
                            let p = pool2.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                                if let Ok(c) = p.get().await {
                                    let _ = c.query("SELECT pg_cancel_backend($1)", &[&pid]).await;
                                }
                            })
                        });
                        crate::sql_log::record_executed_sql(&sql2, None, None);
                        let res = tokio::select! {
                            r = client.query(&sql2, &refs) => r.map_err(map_query_error),
                            _ = async {
                                if let Some(t) = cancel.as_ref() { t.cancelled().await } else { std::future::pending::<()>().await }
                            } => Err(DriverError::Cancelled),
                        };
                        if let Some(h) = cancel_task { h.abort(); }
                        if let Some(h) = timeout_task { h.abort(); }
                        match res {
                            Ok(rows) => {
                                let qr = rows_to_result(rows);
                                crate::sql_log::record_executed_sql(&sql2, Some(qr.rows.len() as i64), None);
                                Ok(qr)
                            }
                            Err(e) => {
                                let is_cancel = matches!(e, DriverError::Cancelled);
                                if !is_cancel {
                                    crate::sql_log::record_executed_sql(&sql2, None, Some(&e.to_string()));
                                }
                                Err(e)
                            }
                        }
                    };
                    return crate::driver::with_opts(Some(o), fut).await;
                }
            let sql_owned = sql.to_string();
            let params_owned = params.to_vec();
            let pool = self.pool.clone();
            let host = self.host.clone();
            let port = self.port;
            let fut = async move {
                let client = pool.get().await.map_err(|e| conn_error(&host, port, e))?;
                crate::sql_log::record_executed_sql(&sql_owned, None, None);
                let owned = build_pg_params(&params_owned);
                let refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
                    owned.iter().map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync)).collect();
                let rows = client.query(&sql_owned, &refs).await.map_err(map_query_error)?;
                let res = rows_to_result(rows);
                crate::sql_log::record_executed_sql(&sql_owned, Some(res.rows.len() as i64), None);
                Ok::<QueryResult, DriverError>(res)
            };
            crate::driver::with_opts(opts, fut).await
        }

        async fn query_read_only_inner(
            &self,
            sql: &str,
            params: &[serde_json::Value],
            opts: Option<QueryOpts>,
        ) -> Result<QueryResult, DriverError> {
            if let Some(o) = opts.clone()
                && (o.cancel.is_some() || o.timeout_ms.is_some()) {
                    let pool2 = self.pool.clone();
                    let host2 = self.host.clone();
                    let port2 = self.port;
                    let sql2 = sql.to_string();
                    let params2 = params.to_vec();
                    let cancel = o.cancel.clone();
                    let timeout_ms = o.timeout_ms;
                    let fut2 = async move {
                        let mut client = pool2.get().await.map_err(|e| conn_error(&host2, port2, e))?;
                        let pid: i32 = client
                            .query("SELECT pg_backend_pid()", &[])
                            .await
                            .map_err(map_query_error)
                            .and_then(|r| {
                                r.first()
                                    .map(|row| row.get::<_, i32>(0))
                                    .ok_or_else(|| DriverError::Query("no pid".into()))
                            })?;
                        let cancel_task = cancel.clone().map(|tok| {
                            let p = pool2.clone();
                            tokio::spawn(async move {
                                tok.cancelled().await;
                                if let Ok(c) = p.get().await { let _ = c.query("SELECT pg_cancel_backend($1)", &[&pid]).await; }
                            })
                        });
                        let timeout_task = timeout_ms.map(|ms| {
                            let p = pool2.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                                if let Ok(c) = p.get().await { let _ = c.query("SELECT pg_cancel_backend($1)", &[&pid]).await; }
                            })
                        });

                        crate::sql_log::record_executed_sql(&sql2, None, None);
                        let res: Result<QueryResult, DriverError> = tokio::select! {
                            r = async {
                                let tx = client.transaction().await.map_err(map_query_error)?;
                                tx.execute("BEGIN READ ONLY", &[]).await.map_err(map_query_error)?;
                                let owned = build_pg_params(&params2);
                                let refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = owned.iter().map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync)).collect();
                                let rows = tx.query(&sql2, &refs).await.map_err(map_query_error)?;
                                let qr = rows_to_result(rows);
                                tx.rollback().await.map_err(map_query_error)?;
                                Ok(qr)
                            } => r,
                            _ = async { if let Some(t) = cancel.as_ref() { t.cancelled().await } else { std::future::pending::<()>().await } } => Err(DriverError::Cancelled),
                        };
                        if let Some(h) = cancel_task { h.abort(); }
                        if let Some(h) = timeout_task { h.abort(); }
                        match &res {
                            Ok(qr) => crate::sql_log::record_executed_sql(&sql2, Some(qr.rows.len() as i64), None),
                            Err(e) if !matches!(e, DriverError::Cancelled) => crate::sql_log::record_executed_sql(&sql2, None, Some(&e.to_string())),
                            _ => {}
                        }
                        res
                    };
                    return crate::driver::with_opts(Some(o), fut2).await;
                }
            let pool = self.pool.clone();
            let host = self.host.clone();
            let port = self.port;
            let sql_owned = sql.to_string();
            let params_owned = params.to_vec();
            let fut = async move {
                let mut client = pool.get().await.map_err(|e| conn_error(&host, port, e))?;
                crate::sql_log::record_executed_sql(&sql_owned, None, None);
                let tx = client.transaction().await.map_err(map_query_error)?;
                tx.execute("BEGIN READ ONLY", &[]).await.map_err(map_query_error)?;
                let owned = build_pg_params(&params_owned);
                let refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
                    owned.iter().map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync)).collect();
                let rows = tx.query(&sql_owned, &refs).await.map_err(map_query_error)?;
                let res = rows_to_result(rows);
                tx.rollback().await.map_err(map_query_error)?;
                crate::sql_log::record_executed_sql(&sql_owned, Some(res.rows.len() as i64), None);
                Ok::<QueryResult, DriverError>(res)
            };
            crate::driver::with_opts(opts, fut).await
        }
    }

    #[async_trait]
    impl Driver for PostgresDriver {
        async fn query(&self, sql: &str, params: &[serde_json::Value], opts: Option<QueryOpts>) -> Result<QueryResult, DriverError> {
            self.query_inner(sql, params, opts).await
        }

        async fn query_read_only(&self, sql: &str, params: &[serde_json::Value], opts: Option<QueryOpts>) -> Result<QueryResult, DriverError> {
            self.query_read_only_inner(sql, params, opts).await
        }

        async fn explain(&self, sql: &str, params: &[serde_json::Value]) -> Result<QueryResult, DriverError> {
            let full = format!("EXPLAIN (FORMAT JSON) {sql}");
            let client = self.pool.get().await.map_err(|e| conn_error(&self.host, self.port, e))?;
            crate::sql_log::record_executed_sql(&full, None, None);
            let owned = build_pg_params(params);
            let refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = owned.iter().map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync)).collect();
            let rows = client.query(&full, &refs).await.map_err(map_query_error)?;
            let res = rows_to_result(rows);
            crate::sql_log::record_executed_sql(&full, Some(res.rows.len() as i64), None);
            Ok(res)
        }

        async fn list_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DriverError> {
            let s = schema.unwrap_or("public");
            let client = self.pool.get().await.map_err(|e| conn_error(&self.host, self.port, e))?;
            let rows = client.query("SELECT tablename FROM pg_tables WHERE schemaname = $1 ORDER BY tablename", &[&s]).await.map_err(map_query_error)?;
            Ok(rows.iter().map(|r| r.get::<_, String>(0)).collect())
        }

        async fn describe_table(&self, table: &str, schema: Option<&str>) -> Result<Vec<ColumnInfo>, DriverError> {
            let s = schema.unwrap_or("public");
            let client = self.pool.get().await.map_err(|e| conn_error(&self.host, self.port, e))?;
            let rows = client.query(
                "SELECT column_name, data_type, is_nullable FROM information_schema.columns WHERE table_schema = $2 AND table_name = $1 ORDER BY ordinal_position",
                &[&table, &s],
            ).await.map_err(map_query_error)?;
            Ok(rows.iter().map(|r| ColumnInfo { column: r.get(0), r#type: r.get(1), nullable: r.get::<_, String>(2) == "YES" }).collect())
        }

        async fn sample_table(&self, table: &str, limit: i64, schema: Option<&str>) -> Result<QueryResult, DriverError> {
            let s = schema.unwrap_or("public");
            let q = format!(r#"SELECT * FROM "{}"."{}" LIMIT $1"#, s.replace('"', "\"\""), table.replace('"', "\"\""));
            let client = self.pool.get().await.map_err(|e| conn_error(&self.host, self.port, e))?;
            crate::sql_log::record_executed_sql(&q, None, None);
            let rows = client.query(&q, &[&limit]).await.map_err(map_query_error)?;
            let res = rows_to_result(rows);
            crate::sql_log::record_executed_sql(&q, Some(res.rows.len() as i64), None);
            Ok(res)
        }

        async fn list_relationships(&self, table: Option<&str>, schema: Option<&str>) -> Result<Vec<RelationshipInfo>, DriverError> {
            let s = schema.unwrap_or("public");
            let client = self.pool.get().await.map_err(|e| conn_error(&self.host, self.port, e))?;
            let (sql, params): (String, Vec<String>) = if let Some(t) = table {
                (
                    "SELECT tc.table_name AS from_table, kcu.column_name AS from_column, ccu.table_name AS to_table, ccu.column_name AS to_column, tc.constraint_name AS constraint_name FROM information_schema.table_constraints tc JOIN information_schema.key_column_usage kcu ON tc.constraint_name = kcu.constraint_name AND tc.table_schema = kcu.table_schema JOIN information_schema.constraint_column_usage ccu ON ccu.constraint_name = tc.constraint_name AND ccu.table_schema = tc.table_schema WHERE tc.constraint_type = 'FOREIGN KEY' AND tc.table_schema = $1 AND tc.table_name = $2 ORDER BY tc.table_name, kcu.ordinal_position".to_string(),
                    vec![s.to_string(), t.to_string()],
                )
            } else {
                (
                    "SELECT tc.table_name AS from_table, kcu.column_name AS from_column, ccu.table_name AS to_table, ccu.column_name AS to_column, tc.constraint_name AS constraint_name FROM information_schema.table_constraints tc JOIN information_schema.key_column_usage kcu ON tc.constraint_name = kcu.constraint_name AND tc.table_schema = kcu.table_schema JOIN information_schema.constraint_column_usage ccu ON ccu.constraint_name = tc.constraint_name AND ccu.table_schema = tc.table_schema WHERE tc.constraint_type = 'FOREIGN KEY' AND tc.table_schema = $1 ORDER BY tc.table_name, kcu.ordinal_position".to_string(),
                    vec![s.to_string()],
                )
            };
            let owned: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> = params.iter().map(|x| Box::new(x.clone()) as Box<dyn tokio_postgres::types::ToSql + Sync + Send>).collect();
            let refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = owned.iter().map(|b| b.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync)).collect();
            let rows = client.query(&sql, &refs).await.map_err(map_query_error)?;
            Ok(rows.iter().map(|r| RelationshipInfo {
                from_table: r.get(0),
                from_column: r.get(1),
                to_table: r.get(2),
                to_column: r.get(3),
                constraint_name: Some(r.get::<_, String>(4)),
            }).collect())
        }

        async fn search_schema(&self, term: &str, schema: Option<&str>) -> Result<Vec<SchemaSearchResult>, DriverError> {
            let s = schema.unwrap_or("public");
            let pattern = format!("%{}%", term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
            let client = self.pool.get().await.map_err(|e| conn_error(&self.host, self.port, e))?;
            let rows = client.query(
                r#"
                SELECT 'table' AS kind, table_name AS "table", NULL::text AS "column", NULL::text AS type
                FROM information_schema.tables
                WHERE table_schema = $2 AND table_name ILIKE $1
                UNION ALL
                SELECT 'column', c.table_name, c.column_name, c.data_type
                FROM information_schema.columns c
                JOIN information_schema.tables t
                  ON c.table_schema = t.table_schema AND c.table_name = t.table_name
                WHERE c.table_schema = $2
                  AND (c.column_name ILIKE $1 OR c.table_name ILIKE $1)
                ORDER BY "table", kind, "column"
                "#,
                &[&pattern, &s],
            ).await.map_err(map_query_error)?;
            Ok(rows.iter().map(|r| SchemaSearchResult {
                kind: r.get(0),
                table: r.get(1),
                column: r.get::<_, Option<String>>(2),
                r#type: r.get::<_, Option<String>>(3),
            }).collect())
        }

        async fn table_stats(&self, table: &str, schema: Option<&str>) -> Result<TableStats, DriverError> {
            let s = schema.unwrap_or("public");
            let client = self.pool.get().await.map_err(|e| conn_error(&self.host, self.port, e))?;
            let rel = client.query(
                "SELECT c.reltuples AS estimated_rows, pg_total_relation_size(c.oid) AS size_bytes FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = $2 AND c.relname = $1",
                &[&table, &s],
            ).await.map_err(map_query_error)?;
            let (estimated_rows, size_bytes) = if let Some(row) = rel.first() {
                let est: Option<f64> = row.get(0);
                let sz: Option<i64> = row.get(1);
                (est.map(|x| x.round() as i64), sz)
            } else { (None, None) };
            let idx_rows = client.query(
                "SELECT indexname, indexdef FROM pg_indexes WHERE schemaname = $2 AND tablename = $1 ORDER BY indexname",
                &[&table, &s],
            ).await.map_err(map_query_error)?;
            let indexes = idx_rows.iter().map(|r| {
                let name: String = r.get(0);
                let def: String = r.get(1);
                let cols = def.split('(').nth(1).and_then(|x| x.split(')').next()).map(|inside| inside.split(',').map(|c| c.trim().trim_matches('"').to_string()).collect()).unwrap_or_default();
                let unique = def.to_uppercase().contains("UNIQUE");
                IndexInfo { name, columns: cols, unique }
            }).collect();
            Ok(TableStats { table: table.to_string(), estimated_rows, size_bytes, indexes })
        }

        async fn list_schemas(&self) -> Result<Vec<String>, DriverError> {
            let client = self.pool.get().await.map_err(|e| conn_error(&self.host, self.port, e))?;
            let rows = client.query("SELECT schema_name FROM information_schema.schemata ORDER BY schema_name", &[]).await.map_err(map_query_error)?;
            Ok(rows.iter().map(|r| r.get(0)).collect())
        }

        async fn list_databases(&self) -> Result<Vec<String>, DriverError> {
            let client = self.pool.get().await.map_err(|e| conn_error(&self.host, self.port, e))?;
            let rows = client.query("SELECT datname FROM pg_database WHERE datistemplate = false AND datallowconn = true ORDER BY datname", &[]).await.map_err(map_query_error)?;
            Ok(rows.iter().map(|r| r.get(0)).collect())
        }

        async fn get_full_schema(&self, schema: Option<&str>) -> Result<String, DriverError> {
            let s = schema.unwrap_or("public");
            let client = self.pool.get().await.map_err(|e| conn_error(&self.host, self.port, e))?;
            let col_rows = client.query(
                "SELECT table_name, column_name, data_type, is_nullable, ordinal_position FROM information_schema.columns WHERE table_schema = $1 ORDER BY table_name, ordinal_position",
                &[&s],
            ).await.map_err(map_query_error)?;
            let key_rows = client.query(
                "SELECT kcu.table_name, kcu.column_name, tc.constraint_type FROM information_schema.table_constraints tc JOIN information_schema.key_column_usage kcu ON tc.constraint_name = kcu.constraint_name AND tc.table_schema = kcu.table_schema WHERE tc.table_schema = $1 AND tc.constraint_type IN ('PRIMARY KEY', 'FOREIGN KEY')",
                &[&s],
            ).await.map_err(map_query_error)?;
            let fk_rows = client.query(
                "SELECT tc.table_name AS from_table, kcu.column_name AS from_column, ccu.table_name AS to_table, ccu.column_name AS to_column FROM information_schema.table_constraints tc JOIN information_schema.key_column_usage kcu ON tc.constraint_name = kcu.constraint_name AND tc.table_schema = kcu.table_schema JOIN information_schema.constraint_column_usage ccu ON ccu.constraint_name = tc.constraint_name AND ccu.table_schema = tc.table_schema WHERE tc.constraint_type = 'FOREIGN KEY' AND tc.table_schema = $1",
                &[&s],
            ).await.map_err(map_query_error)?;

            let mut tables: std::collections::BTreeMap<String, Vec<(String, String, bool, bool)>> = std::collections::BTreeMap::new();
            for r in &col_rows {
                let t: String = r.get(0);
                let col: String = r.get(1);
                let typ: String = r.get(2);
                let nullable: String = r.get(3);
                tables.entry(t).or_default().push((col, typ, nullable == "YES", false));
            }
            for r in &key_rows {
                let t: String = r.get(0);
                let col: String = r.get(1);
                let ctype: String = r.get(2);
                if ctype == "PRIMARY KEY"
                    && let Some(cols) = tables.get_mut(&t)
                        && let Some(c) = cols.iter_mut().find(|c| c.0 == col) { c.3 = true; }
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
                    let from_table: String = r.get(0);
                    if &from_table == table {
                        let from_col: String = r.get(1);
                        let to_table: String = r.get(2);
                        let to_col: String = r.get(3);
                        lines.push(format!("FK {table}.{from_col} -> {to_table}.{to_col}"));
                    }
                }
                lines.push(String::new());
            }
            Ok(lines.join("\n").trim().to_string())
        }

        async fn test_connection(&self) -> Result<(), DriverError> {
            let client = self.pool.get().await.map_err(|e| conn_error(&self.host, self.port, e))?;
            client.execute("SELECT 1", &[]).await.map_err(map_query_error).map(|_| ())
        }

        async fn close(&self) -> Result<(), DriverError> { Ok(()) }
    }
}
