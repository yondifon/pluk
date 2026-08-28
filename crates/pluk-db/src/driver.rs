use crate::error::DriverError;
use crate::types::*;
use async_trait::async_trait;

#[async_trait]
pub trait Driver: Send + Sync {
    async fn query(
        &self,
        sql: &str,
        params: &[serde_json::Value],
        opts: Option<QueryOpts>,
    ) -> Result<QueryResult, DriverError>;
    async fn query_read_only(
        &self,
        sql: &str,
        params: &[serde_json::Value],
        opts: Option<QueryOpts>,
    ) -> Result<QueryResult, DriverError>;
    async fn explain(
        &self,
        sql: &str,
        params: &[serde_json::Value],
    ) -> Result<QueryResult, DriverError>;
    async fn list_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DriverError>;
    async fn describe_table(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<Vec<ColumnInfo>, DriverError>;
    async fn sample_table(
        &self,
        table: &str,
        limit: i64,
        schema: Option<&str>,
    ) -> Result<QueryResult, DriverError>;
    async fn list_relationships(
        &self,
        table: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<RelationshipInfo>, DriverError>;
    async fn search_schema(
        &self,
        term: &str,
        schema: Option<&str>,
    ) -> Result<Vec<SchemaSearchResult>, DriverError>;
    async fn table_stats(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<TableStats, DriverError>;
    async fn list_schemas(&self) -> Result<Vec<String>, DriverError>;
    async fn list_databases(&self) -> Result<Vec<String>, DriverError>;
    async fn get_full_schema(&self, schema: Option<&str>) -> Result<String, DriverError>;
    async fn test_connection(&self) -> Result<(), DriverError>;
    async fn close(&self) -> Result<(), DriverError>;
}

/// Wrap a driver so every introspection call records to the activity log
/// tagged with the tool that caused it. Mirrors `instrumentDriver` in TS.
pub fn instrument_driver<D: Driver + 'static>(
    driver: D,
    ctx: crate::sql_log::SqlLogContext,
) -> InstrumentedDriver<D> {
    InstrumentedDriver { inner: driver, ctx }
}

pub struct InstrumentedDriver<D> {
    inner: D,
    ctx: crate::sql_log::SqlLogContext,
}

#[async_trait]
impl<D: Driver + Send + Sync> Driver for InstrumentedDriver<D> {
    async fn query(
        &self,
        sql: &str,
        params: &[serde_json::Value],
        opts: Option<QueryOpts>,
    ) -> Result<QueryResult, DriverError> {
        self.inner.query(sql, params, opts).await
    }
    async fn query_read_only(
        &self,
        sql: &str,
        params: &[serde_json::Value],
        opts: Option<QueryOpts>,
    ) -> Result<QueryResult, DriverError> {
        self.inner.query_read_only(sql, params, opts).await
    }
    async fn explain(
        &self,
        sql: &str,
        params: &[serde_json::Value],
    ) -> Result<QueryResult, DriverError> {
        crate::sql_log::run_with_sql_log_async(self.ctx.clone(), self.inner.explain(sql, params))
            .await
    }
    async fn list_tables(&self, schema: Option<&str>) -> Result<Vec<String>, DriverError> {
        let s = schema.map(|x| x.to_string());
        crate::sql_log::run_with_sql_log_async(
            self.ctx.clone(),
            self.inner.list_tables(s.as_deref()),
        )
        .await
    }
    async fn describe_table(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<Vec<ColumnInfo>, DriverError> {
        let t = table.to_string();
        let s = schema.map(|x| x.to_string());
        crate::sql_log::run_with_sql_log_async(
            self.ctx.clone(),
            self.inner.describe_table(&t, s.as_deref()),
        )
        .await
    }
    async fn sample_table(
        &self,
        table: &str,
        limit: i64,
        schema: Option<&str>,
    ) -> Result<QueryResult, DriverError> {
        let t = table.to_string();
        let s = schema.map(|x| x.to_string());
        crate::sql_log::run_with_sql_log_async(
            self.ctx.clone(),
            self.inner.sample_table(&t, limit, s.as_deref()),
        )
        .await
    }
    async fn list_relationships(
        &self,
        table: Option<&str>,
        schema: Option<&str>,
    ) -> Result<Vec<RelationshipInfo>, DriverError> {
        let t = table.map(|x| x.to_string());
        let s = schema.map(|x| x.to_string());
        crate::sql_log::run_with_sql_log_async(
            self.ctx.clone(),
            self.inner.list_relationships(t.as_deref(), s.as_deref()),
        )
        .await
    }
    async fn search_schema(
        &self,
        term: &str,
        schema: Option<&str>,
    ) -> Result<Vec<SchemaSearchResult>, DriverError> {
        let term = term.to_string();
        let s = schema.map(|x| x.to_string());
        crate::sql_log::run_with_sql_log_async(
            self.ctx.clone(),
            self.inner.search_schema(&term, s.as_deref()),
        )
        .await
    }
    async fn table_stats(
        &self,
        table: &str,
        schema: Option<&str>,
    ) -> Result<TableStats, DriverError> {
        let t = table.to_string();
        let s = schema.map(|x| x.to_string());
        crate::sql_log::run_with_sql_log_async(
            self.ctx.clone(),
            self.inner.table_stats(&t, s.as_deref()),
        )
        .await
    }
    async fn list_schemas(&self) -> Result<Vec<String>, DriverError> {
        crate::sql_log::run_with_sql_log_async(self.ctx.clone(), self.inner.list_schemas()).await
    }
    async fn list_databases(&self) -> Result<Vec<String>, DriverError> {
        crate::sql_log::run_with_sql_log_async(self.ctx.clone(), self.inner.list_databases()).await
    }
    async fn get_full_schema(&self, schema: Option<&str>) -> Result<String, DriverError> {
        let s = schema.map(|x| x.to_string());
        crate::sql_log::run_with_sql_log_async(
            self.ctx.clone(),
            self.inner.get_full_schema(s.as_deref()),
        )
        .await
    }
    async fn test_connection(&self) -> Result<(), DriverError> {
        self.inner.test_connection().await
    }
    async fn close(&self) -> Result<(), DriverError> {
        self.inner.close().await
    }
}

/// Helper: run a future with timeout and cancellation, mapping to DriverError.
pub async fn with_opts<F, T>(opts: Option<QueryOpts>, fut: F) -> Result<T, DriverError>
where
    F: std::future::Future<Output = Result<T, DriverError>>,
{
    let Some(opts) = opts else {
        return fut.await;
    };
    let cancel = opts.cancel.clone();
    let timeout = opts.timeout_ms;

    // If both timeout and cancel are set, we need to race all three.
    match (timeout, cancel) {
        (None, None) => fut.await,
        (Some(ms), None) => {
            match tokio::time::timeout(std::time::Duration::from_millis(ms), fut).await {
                Ok(r) => r,
                Err(_) => Err(DriverError::Timeout(ms)),
            }
        }
        (None, Some(token)) => {
            tokio::select! {
                r = fut => r,
                _ = token.cancelled() => Err(DriverError::Cancelled),
            }
        }
        (Some(ms), Some(token)) => {
            tokio::select! {
                r = tokio::time::timeout(std::time::Duration::from_millis(ms), fut) => {
                    match r {
                        Ok(inner) => inner,
                        Err(_) => Err(DriverError::Timeout(ms)),
                    }
                },
                _ = token.cancelled() => Err(DriverError::Cancelled),
            }
        }
    }
}
