use crate::config::{SqlConfig, SshTunnelProvider, NoopTunnelProvider, TunnelEndpoint, resolve_ssl};
use crate::driver::Driver;
use crate::error::DriverError;
use crate::fake::FakeDriver;

/// Parameters for driver construction, mirroring `createDriver(integration, ownerId, onFatal, databaseOverride)`.
pub struct CreateDriverOpts {
    pub cfg: SqlConfig,
    /// Per-call database override (multi-db). Pin rule enforced here.
    pub database_override: Option<String>,
    /// Optional SSH tunnel provider. R08 will supply a real one.
    pub ssh_provider: Option<Box<dyn SshTunnelProvider>>,
}

impl CreateDriverOpts {
    pub fn new(cfg: SqlConfig) -> Self { Self { cfg, database_override: None, ssh_provider: None } }
    pub fn with_database(mut self, db: impl Into<String>) -> Self { self.database_override = Some(db.into()); self }
}

/// Result of `create_driver`: a boxed driver and an optional tunnel handle that
/// must be closed together. The `close()` on the driver is expected to also
/// close the tunnel (see `TunnelDriver`).
pub struct DriverWithTunnel {
    pub driver: Box<dyn Driver>,
    pub tunnel: Option<TunnelEndpoint>,
}

impl std::fmt::Debug for DriverWithTunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DriverWithTunnel").field("tunnel", &self.tunnel.as_ref().map(|t| &t.local_port)).finish()
    }
}

impl DriverWithTunnel {
    pub async fn close(&self) -> Result<(), DriverError> {
        let r = self.driver.close().await;
        if let Some(t) = &self.tunnel { t.close(); }
        r
    }
}

/// Port of `createDriver` — resolves pin rule, builds SSL config, rewrites
/// host/port through SSH tunnel when configured, dispatches to engine, and
/// wraps close to tear down the tunnel.
///
/// Today engines are faked (no live DB). Real pool construction will replace
/// the fake branches when `pool` features are wired; the pin/SSL/tunnel
/// logic is already live and tested.
pub async fn create_driver(mut opts: CreateDriverOpts) -> Result<DriverWithTunnel, DriverError> {
    // Pin rule (fail closed before any pool is built)
    let configured = opts.cfg.database.clone();
    let effective_db = pluk_policy::resolve_override_database(
        configured.as_deref(),
        opts.database_override.as_deref(),
    ).map_err(DriverError::from)?;
    opts.cfg.database = effective_db;

    // SSL config — loads CA/cert/key from disk with verification enforced on verify modes
    let _ssl = resolve_ssl(&opts.cfg)?;

    // SSH tunnel seam — leave host/port rewrite to provider
    let mut effective_host = opts.cfg.effective_host();
    let mut effective_port = opts.cfg.effective_port();
    let mut tunnel: Option<TunnelEndpoint> = None;

    let use_ssh = opts.cfg.use_ssh.as_deref() == Some("true");
    if opts.cfg.r#type != "sqlite" && use_ssh && opts.cfg.ssh_host.is_some() {
        let provider: Box<dyn SshTunnelProvider> = opts.ssh_provider.unwrap_or_else(|| Box::new(NoopTunnelProvider));
        let t = provider.open_tunnel(&opts.cfg, &effective_host, effective_port).await?;
        effective_host = t.local_host.clone();
        effective_port = t.local_port;
        tunnel = Some(t);
    }

    let driver: Box<dyn Driver> = match opts.cfg.r#type.as_str() {
        "postgres" => {
            // Real Postgres pool would be built here with `effective_host`, `effective_port`, `_ssl`.
            // For now return a fake that records the resolved endpoint so R08 can be tested.
            let mut d = FakeDriver::new_postgres();
            d.host = effective_host;
            d.port = effective_port;
            d.database = opts.cfg.database.clone();
            d.ssl_mode = opts.cfg.ssl_mode.clone();
            Box::new(d)
        }
        "mysql" => {
            let mut d = FakeDriver::new_mysql();
            d.host = effective_host;
            d.port = effective_port;
            d.database = opts.cfg.database.clone();
            d.ssl_mode = opts.cfg.ssl_mode.clone();
            Box::new(d)
        }
        other => return Err(DriverError::UnsupportedType(other.to_string())),
    };

    // Close wrapping is handled by DriverWithTunnel::close; no need to monkey-patch trait object.
    Ok(DriverWithTunnel { driver, tunnel })
}
