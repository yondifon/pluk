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
    pub use_ssh: Option<String>,
    pub ssh_host: Option<String>,
    pub ssh_port: Option<u16>,
    pub ssh_user: Option<String>,
    pub ssh_auth_type: Option<String>,
    pub ssh_key_path: Option<String>,
    pub ssh_password: Option<String>,
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
    pub fn is_use_ssh(&self) -> bool {
        matches!(self.use_ssh.as_deref(), Some("true") | Some("1"))
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

#[async_trait::async_trait]
pub trait SshTunnelProvider: Send + Sync {
    async fn open_tunnel(&self, cfg: &SqlConfig, remote_host: &str, remote_port: u16) -> Result<TunnelEndpoint, DriverError>;
}

#[derive(Clone)]
pub struct TunnelEndpoint {
    pub local_host: String,
    pub local_port: u16,
    pub close_fn: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

impl TunnelEndpoint {
    pub fn close(&self) { if let Some(f) = &self.close_fn { f(); } }
}

#[async_trait::async_trait]
pub trait SshExecProvider: Send + Sync {
    async fn exec(&self, command: String, timeout_ms: Option<u64>) -> Result<String, DriverError>;
}
