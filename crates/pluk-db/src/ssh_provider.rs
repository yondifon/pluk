//! SSH provider implementations bridging `pluk-ssh` transport to `pluk-db` driver seams.
//!
//! Implements `SshTunnelProvider` (port forwarding) and `SshExecProvider`
//! (remote `sqlite3` exec) on top of the unified `pluk-ssh` transport.

use std::sync::Arc;

use crate::config::{SqlConfig, SshExecProvider, SshTunnelProvider, TunnelEndpoint};
use crate::error::DriverError;

/// Tunnel provider that opens a real SSH tunnel via `pluk-ssh`.
///
/// Chooses transport deliberately: agent/passphrase-less keys via OpenSSH
/// ControlMaster, password/encrypted keys via russh.
pub struct PlukSshTunnelProvider;

#[async_trait::async_trait]
impl SshTunnelProvider for PlukSshTunnelProvider {
    async fn open_tunnel(
        &self,
        cfg: &SqlConfig,
        remote_host: &str,
        remote_port: u16,
    ) -> Result<TunnelEndpoint, DriverError> {
        let ssh_host = cfg.ssh_host.clone().unwrap_or_else(|| cfg.effective_host());
        let ssh_port = cfg.ssh_port.unwrap_or(22);
        let ssh_user = cfg.ssh_user.clone().unwrap_or_default();

        let auth_type = cfg
            .ssh_auth_type
            .clone()
            .unwrap_or_else(|| "agent".to_string());
        let key_path = cfg.ssh_key_path.clone();
        let passphrase = cfg.ssh_password.clone();

        let tunnel_cfg = pluk_ssh::SshTunnelConfig {
            host: ssh_host,
            port: ssh_port,
            user: ssh_user,
            auth_type,
            key_path,
            passphrase,
            remote_host: remote_host.to_string(),
            remote_port,
        };

        let tunnel = pluk_ssh::open_ssh_tunnel(tunnel_cfg, None)
            .await
            .map_err(|e| DriverError::Connection(format!("SSH tunnel failed: {e}")))?;

        let port = tunnel.local_port;
        let tunnel_arc = Arc::new(tunnel);
        let close_tunnel = tunnel_arc.clone();
        Ok(TunnelEndpoint {
            local_host: "127.0.0.1".into(),
            local_port: port,
            close_fn: Some(Arc::new(move || close_tunnel.close())),
        })
    }
}

/// Exec provider that runs a shell command over an SSH exec channel.
///
/// Used by `RemoteSqliteDriver` to shell out `sqlite3 -json`.
pub struct PlukSshExecProvider {
    cfg: SqlConfig,
}

impl PlukSshExecProvider {
    pub fn new(cfg: SqlConfig) -> Self {
        Self { cfg }
    }
}

#[async_trait::async_trait]
impl SshExecProvider for PlukSshExecProvider {
    async fn exec(&self, command: String, timeout_ms: Option<u64>) -> Result<String, DriverError> {
        // Open an exec channel, run command, collect output with timeout.
        // For now, delegate to a simple SSH exec via russh or OpenSSH `ssh host command`.
        // Use OpenSSH `ssh` binary for simplicity (works for all auth types via ControlMaster).
        let ssh_host = self
            .cfg
            .ssh_host
            .clone()
            .unwrap_or_else(|| self.cfg.effective_host());
        let ssh_port = self.cfg.ssh_port.unwrap_or(22);
        let ssh_user = self.cfg.ssh_user.clone().unwrap_or_default();

        let ssh_config = pluk_ssh::config::parse_ssh_config(&ssh_host);
        let user = if !ssh_user.is_empty() {
            ssh_user.clone()
        } else if let Some(u) = ssh_config.user {
            u
        } else {
            std::env::var("USER").unwrap_or_else(|_| "root".into())
        };

        let control_path = pluk_ssh::control_path();
        let mut args = vec![
            "-o".to_string(),
            format!("ControlPath={control_path}"),
            "-p".to_string(),
            ssh_port.to_string(),
        ];
        if !user.is_empty() {
            args.push("-l".to_string());
            args.push(user);
        }
        args.push(ssh_host.clone());
        args.push(command.clone());

        let timeout = timeout_ms.unwrap_or(30_000);
        let output = tokio::time::timeout(std::time::Duration::from_millis(timeout), async {
            let out = tokio::process::Command::new("ssh")
                .args(&args)
                .output()
                .await
                .map_err(|e| DriverError::Connection(format!("ssh exec spawn failed: {e}")))?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let msg = if !stderr.is_empty() { stderr } else { stdout };
                return Err(DriverError::Query(format!("ssh exec failed: {msg}")));
            }
            Ok::<String, DriverError>(String::from_utf8_lossy(&out.stdout).to_string())
        })
        .await
        .map_err(|_| DriverError::Timeout(timeout))??;

        // Cap output at 1_000_000 bytes (matches TS)
        if output.len() > 1_000_000 {
            return Ok(output[..1_000_000].to_string());
        }
        Ok(output)
    }
}
