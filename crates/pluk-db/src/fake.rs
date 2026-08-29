use crate::driver::Driver;
use crate::error::DriverError;
use crate::types::*;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct FakeDriver {
    pub engine: String,
    pub host: String,
    pub port: u16,
    pub database: Option<String>,
    pub ssl_mode: Option<String>,
    /// Controls: next query that looks like a write will be rejected in read-only mode.
    pub reject_writes_in_read_only: bool,
    inner: Arc<Mutex<FakeState>>,
}

#[derive(Debug, Default)]
struct FakeState {
    closed: bool,
    queries: Vec<String>,
}

impl FakeDriver {
    pub fn new_postgres() -> Self {
        Self {
            engine: "postgres".into(),
            host: "localhost".into(),
            port: 5432,
            database: None,
            ssl_mode: None,
            reject_writes_in_read_only: true,
            inner: Arc::new(Mutex::new(FakeState::default())),
        }
    }
    pub fn new_mysql() -> Self {
        Self {
            engine: "mysql".into(),
            host: "localhost".into(),
            port: 3306,
            database: None,
            ssl_mode: None,
            reject_writes_in_read_only: true,
            inner: Arc::new(Mutex::new(FakeState::default())),
        }
    }
    /// A generic fake for trait tests (engine agnostic)
    pub fn new_generic() -> Self {
        Self {
            engine: "fake".into(),
            host: "localhost".into(),
            port: 5432,
            database: None,
            ssl_mode: None,
            reject_writes_in_read_only: true,
            inner: Arc::new(Mutex::new(FakeState::default())),
        }
    }
    fn is_write(sql: &str) -> bool {
        let s = sql.trim().to_ascii_lowercase();
        s.starts_with("insert")
            || s.starts_with("update")
            || s.starts_with("delete")
            || s.starts_with("drop")
            || s.starts_with("create")
            || s.starts_with("alter")
            || s.starts_with("truncate")
    }
}

#[async_trait]
impl Driver for FakeDriver {
    async fn query(
        &self,
        sql: &str,
        _params: &[serde_json::Value],
        opts: Option<QueryOpts>,
    ) -> Result<QueryResult, DriverError> {
        // Simulate timeout/cancellation via opts similar to real driver helper
        if let Some(o) = opts {
            if let Some(ms) = o.timeout_ms {
                // Simulate a slow query that exceeds timeout: delay 50ms, timeout maybe 5ms
                let delay = tokio::time::Duration::from_millis(50);
                if let Some(token) = o.cancel.clone() {
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {},
                        _ = token.cancelled() => return Err(DriverError::Cancelled),
                    }
                    if ms < 50 {
                        return Err(DriverError::Timeout(ms));
                    }
                } else if ms < 50 {
                    tokio::time::sleep(delay).await;
                    return Err(DriverError::Timeout(ms));
                }
            } else if let Some(token) = o.cancel {
                // No timeout but cancellation token — if cancelled before we return, surface it
                if token.is_cancelled() {
                    return Err(DriverError::Cancelled);
                }
                // Also handle cancellation during a brief delay
                tokio::select! {
                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(10)) => {},
                    _ = token.cancelled() => return Err(DriverError::Cancelled),
                }
            }
        }
        self.inner.lock().unwrap().queries.push(sql.to_string());
        Ok(QueryResult {
            rows: vec![serde_json::json!({"ok": 1})],
            fields: Some(vec!["ok".into()]),
        })
    }

    async fn query_read_only(
        &self,
        sql: &str,
        params: &[serde_json::Value],
        opts: Option<QueryOpts>,
    ) -> Result<QueryResult, DriverError> {
        // Enforce read-only: reject writes (simulates BEGIN READ ONLY / START TRANSACTION READ ONLY)
        if self.reject_writes_in_read_only && Self::is_write(sql) {
            return Err(DriverError::Query(
                "read-only transaction: write rejected".into(),
            ));
        }
        self.query(sql, params, opts).await
    }

    async fn explain(
        &self,
        sql: &str,
        _params: &[serde_json::Value],
    ) -> Result<QueryResult, DriverError> {
        Ok(QueryResult {
            rows: vec![serde_json::json!({"explain": sql})],
            fields: Some(vec!["explain".into()]),
        })
    }
    async fn list_tables(&self, _schema: Option<&str>) -> Result<Vec<String>, DriverError> {
        Ok(vec!["users".into(), "orders".into()])
    }
    async fn describe_table(
        &self,
        _table: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<ColumnInfo>, DriverError> {
        Ok(vec![ColumnInfo {
            column: "id".into(),
            r#type: "int".into(),
            nullable: false,
        }])
    }
    async fn sample_table(
        &self,
        _table: &str,
        limit: i64,
        _schema: Option<&str>,
    ) -> Result<QueryResult, DriverError> {
        Ok(QueryResult {
            rows: vec![serde_json::json!({"id": 1})]
                .into_iter()
                .take(limit as usize)
                .collect(),
            fields: Some(vec!["id".into()]),
        })
    }
    async fn list_relationships(
        &self,
        _table: Option<&str>,
        _schema: Option<&str>,
    ) -> Result<Vec<RelationshipInfo>, DriverError> {
        Ok(vec![])
    }
    async fn search_schema(
        &self,
        _term: &str,
        _schema: Option<&str>,
    ) -> Result<Vec<SchemaSearchResult>, DriverError> {
        Ok(vec![])
    }
    async fn table_stats(
        &self,
        table: &str,
        _schema: Option<&str>,
    ) -> Result<TableStats, DriverError> {
        Ok(TableStats {
            table: table.into(),
            estimated_rows: Some(100),
            size_bytes: Some(8192),
            indexes: vec![],
        })
    }
    async fn list_schemas(&self) -> Result<Vec<String>, DriverError> {
        Ok(vec!["public".into()])
    }
    async fn list_databases(&self) -> Result<Vec<String>, DriverError> {
        Ok(vec!["postgres".into()])
    }
    async fn get_full_schema(&self, _schema: Option<&str>) -> Result<String, DriverError> {
        Ok("TABLE users (id int NOT NULL)".into())
    }
    async fn test_connection(&self) -> Result<(), DriverError> {
        Ok(())
    }
    async fn close(&self) -> Result<(), DriverError> {
        self.inner.lock().unwrap().closed = true;
        Ok(())
    }
}
