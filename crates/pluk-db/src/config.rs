use crate::ssl::{build_ssl_config, SslConfig};
use crate::error::DriverError;

#[derive(Debug, Clone, Default)]
pub struct SqlConfig {
    pub r#type: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub database: Option<String>,
    pub socket_path: Option<String>,
    pub filename: Option<String>,
    pub use_ssl: bool,
    pub ssl_mode: Option<String>,
    pub ssl_ca_path: Option<String>,
    pub ssl_cert_path: Option<String>,
    pub ssl_key_path: Option<String>,
    // SSH fields are parsed but tunnelling is deferred to R08
    pub use_ssh: Option<String>, // "true" or bool string
    pub ssh_host: Option<String>,
    pub ssh_port: Option<u16>,
    pub ssh_user: Option<String>,
}

impl SqlConfig {
    pub fn effective_host(&self) -> String { self.host.clone().unwrap_or_else(|| "localhost".into()) }
    pub fn effective_port(&self) -> u16 {
        if self.r#type == "sqlite" { return self.port.unwrap_or(0); }
        self.port.unwrap_or_else(|| if self.r#type == "mysql" { 3306 } else { 5432 })
    }
    pub fn sqlite_filename(&self) -> Option<String> {
        self.filename.clone().or_else(|| self.database.clone())
    }
}

pub fn resolve_ssl(cfg: &SqlConfig) -> Result<Option<SslConfig>, DriverError> {
    build_ssl_config(
        cfg.use_ssl,
        cfg.ssl_mode.as_deref(),
        cfg.ssl_ca_path.as_deref(),
        cfg.ssl_cert_path.as_deref(),
        cfg.ssl_key_path.as_deref(),
    )
}

/// Seam for R08: SSH tunnel provider.
///
/// R08 will implement this trait to open an SSH tunnel and return a local
/// endpoint. The factory calls it when `use_ssh` is set; in this crate the
/// default is a no-op that returns an error if tunnelling is requested but
/// not wired, so callers fail closed rather than silently ignoring SSH.
#[async_trait::async_trait]
pub trait SshTunnelProvider: Send + Sync {
    async fn open_tunnel(&self, cfg: &SqlConfig, remote_host: &str, remote_port: u16) -> Result<TunnelEndpoint, DriverError>;
}

#[derive(Clone)]
pub struct TunnelEndpoint {
    pub local_host: String,
    pub local_port: u16,
    /// Called on driver close to tear down the tunnel.
    pub close_fn: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

impl TunnelEndpoint {
    pub fn close(&self) { if let Some(f) = &self.close_fn { f(); } }
}

/// No-op provider used until R08 plugs in.
pub struct NoopTunnelProvider;
#[async_trait::async_trait]
impl SshTunnelProvider for NoopTunnelProvider {
    async fn open_tunnel(&self, _cfg: &SqlConfig, _remote_host: &str, _remote_port: u16) -> Result<TunnelEndpoint, DriverError> {
        Err(DriverError::Other("SSH tunnelling not yet implemented (R08)".into()))
    }
}

/// Seam for R07/R08: SSH command execution for remote SQLite.
///
/// R08 owns the SSH transport. This trait is the minimal surface R07 needs to
/// shell out `sqlite3 -json [-readonly] <file> <sql>` over an SSH exec channel.
/// The `command` is the exact shell string to run on the remote host. The
/// implementation is responsible for SSH connection pooling, timeout, output
/// capping (1_000_000 bytes), and converting non-zero exit into an error.
#[async_trait::async_trait]
pub trait SshExecProvider: Send + Sync {
    async fn exec(&self, command: String, timeout_ms: Option<u64>) -> Result<String, DriverError>;
}

/// No-op exec provider used until R08 plugs in.
pub struct NoopSshExecProvider;
#[async_trait::async_trait]
impl SshExecProvider for NoopSshExecProvider {
    async fn exec(&self, _command: String, _timeout_ms: Option<u64>) -> Result<String, DriverError> {
        Err(DriverError::Other("SSH exec not yet implemented (R08)".into()))
    }
}
