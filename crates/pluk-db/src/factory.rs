use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::{SqlConfig, SshExecProvider, SshTunnelProvider, TunnelEndpoint, resolve_ssl};
use crate::driver::Driver;
use crate::error::DriverError;
use crate::ssh_provider::{PlukSshExecProvider, PlukSshTunnelProvider};

pub struct CreateDriverOpts {
    pub cfg: SqlConfig,
    pub database_override: Option<String>,
    pub ssh_provider: Option<Box<dyn SshTunnelProvider>>,
    pub ssh_exec_provider: Option<Box<dyn SshExecProvider>>,
}

impl CreateDriverOpts {
    pub fn new(cfg: SqlConfig) -> Self {
        Self {
            cfg,
            database_override: None,
            ssh_provider: None,
            ssh_exec_provider: None,
        }
    }
    pub fn with_database(mut self, db: impl Into<String>) -> Self {
        self.database_override = Some(db.into());
        self
    }
    pub fn with_ssh_provider(mut self, p: Box<dyn SshTunnelProvider>) -> Self {
        self.ssh_provider = Some(p);
        self
    }
    pub fn with_ssh_exec(mut self, p: Box<dyn SshExecProvider>) -> Self {
        self.ssh_exec_provider = Some(p);
        self
    }
}

/// A tunnel whose lifetime is the value that holds it: dropping it closes the
/// forward, so no exit path — an early return, an error, a cancelled request —
/// can leave a forwarded port and its `ssh` child behind.
pub struct OwnedTunnel {
    endpoint: TunnelEndpoint,
    closed: AtomicBool,
}

impl OwnedTunnel {
    pub fn new(endpoint: TunnelEndpoint) -> Self {
        Self {
            endpoint,
            closed: AtomicBool::new(false),
        }
    }

    pub fn local_port(&self) -> u16 {
        self.endpoint.local_port
    }

    pub fn close(&self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            self.endpoint.close();
        }
    }
}

impl Drop for OwnedTunnel {
    fn drop(&mut self) {
        self.close();
    }
}

/// Result of `create_driver`: a boxed driver and, when the connection goes
/// through SSH, the tunnel it runs over. Closing both is [`close`](Self::close);
/// whoever ends up owning the tunnel closes it by dropping it.
pub struct DriverWithTunnel {
    pub driver: Box<dyn Driver>,
    pub tunnel: Option<OwnedTunnel>,
}

impl std::fmt::Debug for DriverWithTunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DriverWithTunnel")
            .field("tunnel", &self.tunnel.as_ref().map(|t| t.local_port()))
            .finish()
    }
}

impl DriverWithTunnel {
    pub async fn close(&self) -> Result<(), DriverError> {
        let r = self.driver.close().await;
        if let Some(t) = &self.tunnel {
            t.close();
        }
        r
    }
}

pub async fn create_driver(mut opts: CreateDriverOpts) -> Result<DriverWithTunnel, DriverError> {
    let configured = opts.cfg.database.clone();
    let effective_db = pluk_policy::resolve_override_database(
        configured.as_deref(),
        opts.database_override.as_deref(),
    )
    .map_err(DriverError::from)?;
    opts.cfg.database = effective_db;

    let ssl = resolve_ssl(&opts.cfg)?;

    let mut effective_host = opts.cfg.effective_host();
    let mut effective_port = opts.cfg.effective_port();
    let mut tunnel: Option<OwnedTunnel> = None;

    let use_ssh = opts.cfg.is_use_ssh();
    if opts.cfg.r#type != "sqlite" && use_ssh && opts.cfg.ssh_host.is_some() {
        let provider: Box<dyn SshTunnelProvider> = opts
            .ssh_provider
            .unwrap_or_else(|| Box::new(PlukSshTunnelProvider));
        let t = provider
            .open_tunnel(&opts.cfg, &effective_host, effective_port)
            .await?;
        effective_host = t.local_host.clone();
        effective_port = t.local_port;
        tunnel = Some(OwnedTunnel::new(t));
    }

    let driver: Box<dyn Driver> = match opts.cfg.r#type.as_str() {
        "sqlite" => {
            let is_ssh = opts.cfg.is_use_ssh();
            if is_ssh {
                if opts
                    .cfg
                    .ssh_host
                    .as_deref()
                    .map(|s| s.trim().is_empty())
                    .unwrap_or(true)
                {
                    return Err(DriverError::Connection(
                        "SQLite SSH host is missing. Set it in the connection settings.".into(),
                    ));
                }
                let filename = opts.cfg.sqlite_filename().ok_or_else(|| {
                    DriverError::Connection(
                        "SQLite path is missing. Set the remote database file path.".into(),
                    )
                })?;
                let exec: Box<dyn SshExecProvider> = opts.ssh_exec_provider.unwrap_or_else(|| {
                    let cfg = opts.cfg.clone();
                    Box::new(PlukSshExecProvider::new(cfg))
                });
                Box::new(crate::sqlite_remote::RemoteSqliteDriver::new(
                    filename, exec,
                ))
            } else {
                let filename = opts.cfg.sqlite_filename().ok_or_else(|| {
                    DriverError::Connection(
                        "SQLite path is missing. Set the database file path.".into(),
                    )
                })?;
                Box::new(crate::sqlite::SqliteDriver::open(&filename)?)
            }
        }
        "postgres" => {
            #[cfg(feature = "postgres")]
            {
                let d = crate::postgres::live::PostgresDriver::new(
                    effective_host,
                    effective_port,
                    opts.cfg.user.clone(),
                    opts.cfg.password.clone(),
                    opts.cfg.database.clone(),
                    ssl,
                )?;
                Box::new(d)
            }
            #[cfg(not(feature = "postgres"))]
            return Err(DriverError::UnsupportedType("postgres".into()));
        }
        "mysql" => {
            #[cfg(feature = "mysql")]
            {
                let d = crate::mysql::live::MySqlDriver::new(
                    effective_host,
                    effective_port,
                    opts.cfg.user.clone(),
                    opts.cfg.password.clone(),
                    opts.cfg.database.clone(),
                    ssl,
                    opts.cfg.socket_path.clone(),
                )
                .await?;
                Box::new(d)
            }
            #[cfg(not(feature = "mysql"))]
            return Err(DriverError::UnsupportedType("mysql".into()));
        }
        other => return Err(DriverError::UnsupportedType(other.to_string())),
    };

    Ok(DriverWithTunnel { driver, tunnel })
}
