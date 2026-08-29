//! Concrete driver pool for `pluk-db` — re-exports `pluk-ssh` pool budgets and
//! pending logic, and provides a `Driver`-typed pool.

pub use pluk_ssh::pool::{
    CONNECT_TIMEOUT_DIRECT_MS, CONNECT_TIMEOUT_SSH_MS, HEALTHCHECK_TIMEOUT_MS, IDLE_MS,
    MAX_RECONNECT_ATTEMPTS, RECONNECT_AUTH_DELAY_MS, RECONNECT_DELAYS_MS, STALE_AFTER_MS,
    TOOL_TIMEOUT_MS, driver_key,
};

pub use pluk_ssh::pool::{DriverPool, PoolDriver, PoolError};

use std::sync::Arc;

use crate::driver::Driver as DbDriver;
use crate::error::DriverError;

/// Adapter: `pluk-ssh` PoolDriver implemented for any `DbDriver`.
struct DbDriverAdapter(Arc<dyn DbDriver>);

#[async_trait::async_trait]
impl pluk_ssh::pool::PoolDriver for DbDriverAdapter {
    async fn test_connection(&self) -> Result<(), pluk_ssh::pool::PoolError> {
        self.0
            .test_connection()
            .await
            .map_err(|e| pluk_ssh::pool::PoolError::Connection(e.to_string()))
    }
    async fn close(&self) -> Result<(), pluk_ssh::pool::PoolError> {
        self.0
            .close()
            .await
            .map_err(|e| pluk_ssh::pool::PoolError::Other(e.to_string()))
    }
}

impl From<DriverError> for pluk_ssh::pool::PoolError {
    fn from(_e: DriverError) -> Self {
        // Handled via PoolDriver adapter; this Impl is for completeness.
        pluk_ssh::pool::PoolError::Other("driver error".into())
    }
}

/// Factory that creates real `Driver` instances via `create_driver`.
pub struct DbDriverFactory {
    pub base_cfg: crate::config::SqlConfig,
}

#[async_trait::async_trait]
impl pluk_ssh::pool::DriverFactory for DbDriverFactory {
    async fn create_driver(
        &self,
        _owner_id: &str,
        _integration_id: &str,
        database: Option<&str>,
        _use_ssh: bool,
        _on_fatal: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Arc<dyn pluk_ssh::pool::PoolDriver>, pluk_ssh::pool::PoolError> {
        let mut cfg = self.base_cfg.clone();
        if let Some(db) = database {
            cfg.database = Some(db.to_string());
        }
        let opts = crate::factory::CreateDriverOpts::new(cfg);
        let dw = crate::factory::create_driver(opts)
            .await
            .map_err(|e| pluk_ssh::pool::PoolError::Connection(e.to_string()))?;
        let driver: Arc<dyn DbDriver> = Arc::from(dw.driver);
        let tunnel = dw.tunnel;
        struct TunnelDriver {
            inner: Arc<dyn DbDriver>,
            tunnel: Option<crate::factory::OwnedTunnel>,
        }
        #[async_trait::async_trait]
        impl pluk_ssh::pool::PoolDriver for TunnelDriver {
            async fn test_connection(&self) -> Result<(), pluk_ssh::pool::PoolError> {
                self.inner
                    .test_connection()
                    .await
                    .map_err(|e| pluk_ssh::pool::PoolError::Connection(e.to_string()))
            }
            async fn close(&self) -> Result<(), pluk_ssh::pool::PoolError> {
                let r = self
                    .inner
                    .close()
                    .await
                    .map_err(|e| pluk_ssh::pool::PoolError::Other(e.to_string()));
                if let Some(t) = &self.tunnel {
                    t.close();
                }
                r
            }
        }
        if tunnel.is_some() {
            Ok(Arc::new(TunnelDriver {
                inner: driver,
                tunnel,
            }))
        } else {
            Ok(Arc::new(DbDriverAdapter(driver)))
        }
    }
}
