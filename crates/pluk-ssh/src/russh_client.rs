//! In-process SSH client path using `russh`.
//!
//! Handles password authentication and encrypted keys — the cases that cannot
//! be driven through the OpenSSH binary non-interactively.
//!
//! Budgets: ready timeout 180s, keepalive every 30s up to 3 missed.

use std::sync::Arc;


use crate::config::parse_ssh_config;
use crate::openssh::{SshError, SshTunnelConfig};

#[cfg(feature = "russh-transport")]
mod russh_impl {
    use super::*;

    use russh::client::{self, Handle};
    use russh_keys::key::KeyPair;
    use std::collections::HashMap;

    struct ClientHandler;

    impl client::Handler for ClientHandler {
        type Error = russh::Error;
        async fn check_server_key(
            &mut self,
            _server_public_key: &russh_keys::key::PublicKey,
        ) -> Result<bool, Self::Error> {
            Ok(true) // TODO: host key verification
        }
    }

    pub async fn open_russh_tunnel(
        config: &SshTunnelConfig,
        ssh_config: &crate::config::SshConfigEntry,
        username: &str,
        on_fatal: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<crate::openssh::Tunnel, SshError> {
        let host = ssh_config.host_name.as_deref().unwrap_or(&config.host);
        let port = ssh_config.port.unwrap_or(config.port);

        let mut ssh_handle = connect_with_proxy(host, port, username, config, ssh_config).await?;

        // Set up local forward: listen on 127.0.0.1:0 and forward via russh channel
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(SshError::Io)?;
        let local_port = listener.local_addr().map_err(SshError::Io)?.port();

        let handle_clone = ssh_handle.clone();
        let remote_host = config.remote_host.clone();
        let remote_port = config.remote_port;

        // Spawn accept loop
        let accept_handle = tokio::spawn(async move {
            loop {
                let (mut socket, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let mut handle = handle_clone.clone();
                let rh = remote_host.clone();
                tokio::spawn(async move {
                    let channel = match handle
                        .channel_open_direct_tcpip(
                            rh.clone(),
                            remote_port as u32,
                            "127.0.0.1".to_string(),
                            0,
                        )
                        .await
                    {
                        Ok(ch) => ch,
                        Err(e) => {
                            eprintln!("[pluk] forward channel open error: {e}");
                            return;
                        }
                    };
                    let (mut ri, mut wi) = tokio::io::split(channel.into_stream());
                    let (mut ro, mut wo) = socket.split();
                    let _ = tokio::join!(
                        tokio::io::copy(&mut ro, &mut wi),
                        tokio::io::copy(&mut ri, &mut wo),
                    );
                });
            }
        });

        // Keepalive / close detection
        if let Some(cb) = on_fatal {
            let mut h = ssh_handle.clone();
            tokio::spawn(async move {
                // Wait for disconnect
                let _ = h.wait().await;
                cb();
            });
        }

        eprintln!("[pluk] russh tunnel ready on localhost:{local_port}");

        let close_handle = ssh_handle.clone();
        let close_fn: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(move || {
            let h = close_handle.clone();
            tokio::spawn(async move {
                let _ = h.disconnect(russh::Disconnect::ByApplication, String::new(), String::new()).await;
            });
            accept_handle.abort();
        });

        Ok(crate::openssh::Tunnel {
            local_port,
            close_fn: Some(close_fn),
            _poll_handle: None,
        })
    }

    async fn connect_with_proxy(
        host: &str,
        port: u16,
        username: &str,
        config: &SshTunnelConfig,
        ssh_config: &crate::config::SshConfigEntry,
    ) -> Result<Handle<ClientHandler>, SshError> {
        let addr = format!("{host}:{port}");

        // If ProxyCommand is configured, we need to tunnel through it
        // russh doesn't natively support ProxyCommand streams, so we spawn the command
        // and use its stdin/stdout as the transport
        if let Some(ref proxy_cmd) = ssh_config.proxy_command {
            let expanded = expand_proxy_command(proxy_cmd, host, port, username);
            return connect_via_proxy_command(&expanded, host, port, username, config).await;
        }

        let config_russh = Arc::new(russh::client::Config {
            inactivity_timeout: Some(Duration::from_secs(90)),
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max: 3,
            ..Default::default()
        });

        let handler = ClientHandler;
        let mut handle = russh::client::connect(config_russh, addr, handler)
            .await
            .map_err(|e| SshError::Tunnel(e.to_string()))?;

        authenticate(&mut handle, username, config).await?;
        Ok(handle)
    }

    async fn connect_via_proxy_command(
        proxy_cmd: &str,
        host: &str,
        port: u16,
        username: &str,
        config: &SshTunnelConfig,
    ) -> Result<Handle<ClientHandler>, SshError> {
        // Spawn proxy command and use its stdio as transport
        let parts = crate::config::split_command(proxy_cmd);
        if parts.is_empty() {
            return Err(SshError::Tunnel("empty ProxyCommand".into()));
        }
        let mut child = tokio::process::Command::new(&parts[0])
            .args(&parts[1..])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .map_err(SshError::Io)?;

        let stdin = child.stdin.take().expect("proxy stdin");
        let stdout = child.stdout.take().expect("proxy stdout");
        let stream = tokio::io::join(stdout, stdin);

        // russh connect_stream
        let config_russh = Arc::new(russh::client::Config {
            inactivity_timeout: Some(Duration::from_secs(90)),
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max: 3,
            ..Default::default()
        });
        let handler = ClientHandler;
        // Use low-level connect_stream if available, else fallback
        // russh 0.52 has connect_stream
        let mut handle = russh::client::connect_stream(config_russh, stream, handler)
            .await
            .map_err(|e| SshError::Tunnel(e.to_string()))?;

        authenticate(&mut handle, username, config).await?;
        Ok(handle)
    }

    async fn authenticate(
        handle: &mut Handle<ClientHandler>,
        username: &str,
        config: &SshTunnelConfig,
    ) -> Result<(), SshError> {
        // Try keyboard-interactive first for password, then password auth
        let auth_res = tokio::time::timeout(
            Duration::from_millis(HANDSHAKE_TIMEOUT_MS),
            do_auth(handle, username, config),
        )
        .await
        .map_err(|_| SshError::Timeout("SSH handshake timed out after 180s".into()))?;

        auth_res
    }

    async fn do_auth(
        handle: &mut Handle<ClientHandler>,
        username: &str,
        config: &SshTunnelConfig,
    ) -> Result<(), SshError> {
        match config.auth_type.as_str() {
            "password" => {
                let password = config.passphrase.clone().unwrap_or_default();
                let ok = handle
                    .authenticate_password(username, password)
                    .await
                    .map_err(|e| SshError::Tunnel(e.to_string()))?;
                if !ok {
                    return Err(SshError::Tunnel("password authentication failed".into()));
                }
                Ok(())
            }
            "key" => {
                // Load key with passphrase if present, try agent if available
                if let Some(ref key_path) = config.key_path {
                    let key_data = tokio::fs::read(expand_proxy_command(key_path, "", 0, ""))
                        .await
                        .map_err(|e| SshError::Tunnel(format!("key read error: {e}")))?;
                    // Try to decode with russh-keys
                    let passphrase = config.passphrase.as_deref();
                    // russh-keys decode logic
                    let key_pair = decode_key(&key_data, passphrase)?;
                    let ok = handle
                        .authenticate_publickey(
                            username,
                            russh_keys::key::PrivateKeyWithHashAlg::new(
                                Arc::new(key_pair),
                                None,
                            ),
                        )
                        .await
                        .map_err(|e| SshError::Tunnel(e.to_string()))?;
                    if !ok {
                        return Err(SshError::Tunnel("publickey authentication failed".into()));
                    }
                    Ok(())
                } else {
                    Err(SshError::Tunnel("no key path for key auth".into()))
                }
            }
            _ => Err(SshError::Tunnel(format!(
                "unsupported auth type for russh: {}",
                config.auth_type
            ))),
        }
    }

    fn decode_key(data: &[u8], passphrase: Option<&str>) -> Result<KeyPair, SshError> {
        // Try without passphrase first, then with
        let key_str = String::from_utf8_lossy(data);
        russh_keys::decode_secret_key(&key_str, passphrase)
            .map_err(|e| SshError::Tunnel(format!("key decode failed: {e}")))
    }
}

#[cfg(not(feature = "russh-transport"))]
mod russh_impl {
    use super::*;
    pub async fn open_russh_tunnel(
        _config: &SshTunnelConfig,
        _ssh_config: &crate::config::SshConfigEntry,
        _username: &str,
        _on_fatal: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Result<crate::openssh::Tunnel, SshError> {
        Err(SshError::Tunnel(
            "russh transport not compiled (enable russh-transport feature)".into(),
        ))
    }
}

pub use russh_impl::open_russh_tunnel;

pub async fn open_tunnel_russh(
    config: SshTunnelConfig,
    on_fatal: Option<Arc<dyn Fn() + Send + Sync>>,
) -> Result<crate::openssh::Tunnel, SshError> {
    let ssh_config = parse_ssh_config(&config.host);
    let username = if !config.user.is_empty() {
        config.user.clone()
    } else if let Some(u) = ssh_config.user.clone() {
        u
    } else {
        std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "root".into())
    };
    open_russh_tunnel(&config, &ssh_config, &username, on_fatal).await
}
