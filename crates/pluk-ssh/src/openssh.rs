//! OpenSSH ControlMaster transport.
//!
//! Mirrors `pluk/src/db/ssh.ts` OpenSSH path: agent and passphrase-less key
//! tunnels go through the system `ssh` binary with `ControlMaster=auto` and
//! `ControlPersist=10m`, forwards added/removed with `ssh -O forward/cancel`,
//! master polled every 30s.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::process::Command;

use crate::config::{SshConfigEntry, expand_home, parse_ssh_config};
use crate::pending::is_ssh_auth_error;

pub const HANDSHAKE_TIMEOUT_MS: u64 = 180_000;
pub const FAST_RETRY_WINDOW_MS: u64 = 10_000;
pub const CONTROL_PERSIST: &str = "10m";
pub const CONTROL_CMD_TIMEOUT_MS: u64 = 10_000;
pub const MASTER_POLL_MS: u64 = 30_000;
pub const READINESS_TIMEOUT_MS: u64 = 15_000;

/// Template uses `%C` hash token so the full path stays under the 104-byte
/// `sun_path` limit (see `pluk_core::platform::ssh_control_dir`).
pub fn control_path() -> String {
    // Use platform ssh_control_dir + "/cm-%C" — the %C is expanded by ssh itself.
    let dir = pluk_core::platform::ssh_control_dir();
    format!("{}/cm-%C", dir.display())
}

pub fn control_dir() -> PathBuf {
    pluk_core::platform::ssh_control_dir()
}

#[derive(Debug, Clone)]
pub struct SshTunnelConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth_type: String, // "agent" | "key" | "password"
    pub key_path: Option<String>,
    pub passphrase: Option<String>,
    pub remote_host: String,
    pub remote_port: u16,
}

#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("{0}")]
    Tunnel(String),
    #[error("agent unreachable: {0}")]
    AgentUnreachable(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl SshError {
    pub fn is_auth(&self) -> bool {
        match self {
            Self::AgentUnreachable(_) => true,
            Self::Tunnel(msg) => is_ssh_auth_error(msg),
            _ => false,
        }
    }
}

/// The args that identify one master: they feed `%C`, so every command that has
/// to find the same socket must pass them.
fn master_target(
    config: &SshTunnelConfig,
    ssh_config: &SshConfigEntry,
    username: &str,
) -> Vec<String> {
    let mut args = vec!["-o".to_string(), format!("ControlPath={}", control_path())];
    if !username.is_empty() {
        args.push("-l".to_string());
        args.push(username.to_string());
    }
    let port = ssh_config.port.unwrap_or(config.port);
    args.push("-p".to_string());
    args.push(port.to_string());
    args
}

static MASTER_STARTS: OnceLock<tokio::sync::Mutex<HashMap<String, ()>>> = OnceLock::new();

fn master_starts() -> &'static tokio::sync::Mutex<HashMap<String, ()>> {
    MASTER_STARTS.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

async fn run_ssh_command(args: &[String], timeout_ms: u64) -> (i32, String) {
    let mut cmd = Command::new("ssh");
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .envs(std::env::vars());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (1, e.to_string()),
    };

    let stderr_handle = child.stderr.take();
    let wait_fut = child.wait();

    let result = tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        let stderr_bytes = if let Some(mut stderr) = stderr_handle {
            use tokio::io::AsyncReadExt;
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf).await;
            buf
        } else {
            Vec::new()
        };
        let status = wait_fut.await;
        (status, stderr_bytes)
    })
    .await;

    match result {
        Ok((status_res, stderr_bytes)) => {
            let code = match status_res {
                Ok(s) => s.code().unwrap_or(1),
                Err(e) => return (1, e.to_string()),
            };
            let stderr = String::from_utf8_lossy(&stderr_bytes).trim().to_string();
            let filtered = stderr
                .lines()
                .filter(|l| !l.is_empty() && !l.to_ascii_lowercase().contains("closed by unknown"))
                .collect::<Vec<_>>()
                .join("\n");
            (code, filtered)
        }
        Err(_) => {
            let _ = child.kill().await;
            (1, "ssh command timed out".into())
        }
    }
}

async fn ensure_master(
    config: &SshTunnelConfig,
    target: &[String],
    timeout_ms: u64,
) -> Result<(), SshError> {
    let key = format!("{} {}", target.join(" "), config.host);

    // Simple in-flight dedup: if another task is starting the same master, wait briefly
    {
        let map = master_starts().lock().await;
        if map.contains_key(&key) {
            drop(map);
            tokio::time::sleep(Duration::from_millis(200)).await;
            // Recheck
            let (code, _) = run_ssh_command(
                &{
                    let mut a = vec!["-O".to_string(), "check".to_string()];
                    a.extend_from_slice(target);
                    a.push(config.host.clone());
                    a
                },
                CONTROL_CMD_TIMEOUT_MS,
            )
            .await;
            if code == 0 {
                return Ok(());
            }
        }
    }

    // Insert guard
    master_starts().lock().await.insert(key.clone(), ());
    struct Guard(String);
    impl Drop for Guard {
        fn drop(&mut self) {
            // Use try_lock to avoid blocking in Drop; if contended, leak entry (harmless)
            if let Ok(mut map) = master_starts().try_lock() {
                map.remove(&self.0);
            }
        }
    }
    let _guard = Guard(key.clone());

    // Ensure control dir exists
    let _ = tokio::fs::create_dir_all(control_dir()).await;

    // Check if already up
    let check_args: Vec<String> = {
        let mut a = vec!["-O".to_string(), "check".to_string()];
        a.extend_from_slice(target);
        a.push(config.host.clone());
        a
    };
    let (code, _) = run_ssh_command(&check_args, CONTROL_CMD_TIMEOUT_MS).await;
    if code == 0 {
        return Ok(());
    }

    let mut args = vec![
        "-N".to_string(),
        "-f".to_string(),
        "-o".to_string(),
        "ControlMaster=auto".to_string(),
        "-o".to_string(),
        format!("ControlPersist={CONTROL_PERSIST}"),
        "-o".to_string(),
        "ServerAliveInterval=30".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=3".to_string(),
    ];
    args.extend_from_slice(target);

    if config.auth_type == "key"
        && let Some(ref kp) = config.key_path
    {
        args.push("-i".to_string());
        args.push(expand_home(kp));
        args.push("-o".to_string());
        args.push("IdentitiesOnly=yes".to_string());
        args.push("-o".to_string());
        args.push("IdentityAgent=none".to_string());
    }

    if config.auth_type == "agent" {
        let agent = crate::agent::resolve_live_agent(&config.host).await;
        match agent {
            Some(a) => {
                eprintln!(
                    "[pluk] SSH agent socket: {} ({})",
                    a.socket,
                    a.probe.state_str()
                );
                // Quote socket path for ssh's tokenizer
                args.push("-o".to_string());
                args.push(format!("IdentityAgent=\"{}\"", a.socket));
            }
            None => {
                let e = crate::agent::agent_unreachable_error();
                return Err(SshError::AgentUnreachable(e.message));
            }
        }
    }

    args.push(config.host.clone());

    eprintln!("[pluk] OpenSSH master: ssh {}", args.join(" "));

    let (code, stderr) = run_ssh_command(&args, timeout_ms).await;
    if code != 0 {
        let msg = if stderr.is_empty() {
            format!("ssh master failed (exit {code})")
        } else {
            stderr
        };
        return Err(SshError::Tunnel(msg));
    }
    Ok(())
}

async fn reserve_local_port() -> std::io::Result<u16> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

async fn wait_for_port(port: u16, timeout_ms: u64) -> Result<(), SshError> {
    let start = std::time::Instant::now();
    let timeout = Duration::from_millis(timeout_ms);
    loop {
        match tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await {
            Ok(_) => return Ok(()),
            Err(e) => {
                if start.elapsed() > timeout {
                    return Err(SshError::Timeout(format!(
                        "SSH tunnel did not become ready within {}s: {}",
                        timeout_ms / 1000,
                        e
                    )));
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

pub struct Tunnel {
    pub local_port: u16,
    close_fn: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    _poll_handle: Option<tokio::task::JoinHandle<()>>,
}

unsafe impl Send for Tunnel {}
unsafe impl Sync for Tunnel {}

impl Tunnel {
    pub fn close(&self) {
        if let Some(f) = &self.close_fn {
            f();
        }
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.close();
        if let Some(h) = self._poll_handle.take() {
            h.abort();
        }
    }
}

/// Open a tunnel via OpenSSH ControlMaster. Mirrors `openOpenSSHTunnel` in ssh.ts.
pub async fn open_openssh_tunnel(
    config: &SshTunnelConfig,
    ssh_config: &SshConfigEntry,
    username: &str,
    readiness_timeout_ms: u64,
    on_fatal: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
) -> Result<Tunnel, SshError> {
    let target = master_target(config, ssh_config, username);
    let started = std::time::Instant::now();

    ensure_master(config, &target, readiness_timeout_ms).await?;

    let local_port = reserve_local_port().await.map_err(SshError::Io)?;
    let spec = format!(
        "127.0.0.1:{local_port}:{}:{}",
        config.remote_host, config.remote_port
    );

    let fwd_args: Vec<String> = {
        let mut a = vec![
            "-O".to_string(),
            "forward".to_string(),
            "-L".to_string(),
            spec.clone(),
        ];
        a.extend_from_slice(&target);
        a.push(config.host.clone());
        a
    };
    let (code, stderr) = run_ssh_command(&fwd_args, CONTROL_CMD_TIMEOUT_MS).await;
    if code != 0 {
        let msg = if stderr.is_empty() {
            format!("ssh -O forward failed (exit {code})")
        } else {
            stderr
        };
        return Err(SshError::Tunnel(msg));
    }

    let elapsed_ms = started.elapsed().as_millis() as u64;
    let remaining = readiness_timeout_ms.saturating_sub(elapsed_ms).max(1_000);

    if let Err(e) = wait_for_port(local_port, remaining).await {
        let cancel_args: Vec<String> = {
            let mut a = vec![
                "-O".to_string(),
                "cancel".to_string(),
                "-L".to_string(),
                spec.clone(),
            ];
            a.extend_from_slice(&target);
            a.push(config.host.clone());
            a
        };
        let _ = run_ssh_command(&cancel_args, CONTROL_CMD_TIMEOUT_MS).await;
        return Err(e);
    }

    eprintln!("[pluk] tunnel ready on localhost:{local_port}");

    // Master poll — self-heal if master dies
    let poll_target = target.clone();
    let poll_host = config.host.clone();
    let poll_spec = spec.clone();
    let poll_handle = on_fatal.clone().map(|cb| {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(MASTER_POLL_MS));
            loop {
                interval.tick().await;
                let check_args: Vec<String> = {
                    let mut a = vec!["-O".to_string(), "check".to_string()];
                    a.extend_from_slice(&poll_target);
                    a.push(poll_host.clone());
                    a
                };
                let (code, _) = run_ssh_command(&check_args, CONTROL_CMD_TIMEOUT_MS).await;
                if code != 0 {
                    cb();
                    break;
                }
            }
            let _ = poll_spec;
        })
    });

    let close_target = target.clone();
    let close_host = config.host.clone();
    let close_spec = spec.clone();
    let close_fn: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(move || {
        let t = close_target.clone();
        let h = close_host.clone();
        let s = close_spec.clone();
        tokio::spawn(async move {
            let args: Vec<String> = {
                let mut a = vec!["-O".to_string(), "cancel".to_string(), "-L".to_string(), s];
                a.extend_from_slice(&t);
                a.push(h);
                a
            };
            let _ = run_ssh_command(&args, CONTROL_CMD_TIMEOUT_MS).await;
        });
    });

    Ok(Tunnel {
        local_port,
        close_fn: Some(close_fn),
        _poll_handle: poll_handle,
    })
}

/// High-level entry mirroring `openSSHTunnel` routing: agent/key via OpenSSH,
/// password/encrypted-key via in-process client (delegated to russh module).
pub async fn open_ssh_tunnel_via_openssh(
    config: SshTunnelConfig,
    on_fatal: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
) -> Result<Tunnel, SshError> {
    let ssh_config = parse_ssh_config(&config.host);
    let username = if !config.user.is_empty() {
        config.user.clone()
    } else if let Some(u) = ssh_config.user.clone() {
        u
    } else {
        // Fallback to current user
        std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "root".into())
    };

    let use_openssh =
        config.auth_type == "agent" || (config.auth_type == "key" && config.passphrase.is_none());

    if use_openssh {
        let attempts: u32 = if ssh_config.proxy_command.is_some() {
            3
        } else {
            1
        };
        let deadline = std::time::Instant::now() + Duration::from_millis(HANDSHAKE_TIMEOUT_MS);
        let mut last_err: Option<SshError> = None;

        for attempt in 1..=attempts {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            if remaining == 0 {
                break;
            }
            let started = std::time::Instant::now();
            match open_openssh_tunnel(&config, &ssh_config, &username, remaining, on_fatal.clone())
                .await
            {
                Ok(t) => return Ok(t),
                Err(e) => {
                    if e.is_auth() {
                        return Err(e);
                    }
                    let failed_fast = started.elapsed().as_millis() < FAST_RETRY_WINDOW_MS as u128;
                    let is_readiness_timeout = matches!(e, SshError::Timeout(_));
                    if attempt < attempts && failed_fast && !is_readiness_timeout {
                        eprintln!(
                            "[pluk] OpenSSH tunnel attempt {attempt} failed: {e}. Retrying in 2s…"
                        );
                        tokio::time::sleep(Duration::from_millis(2000)).await;
                        last_err = Some(e);
                    } else {
                        return Err(e);
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            SshError::Tunnel("SSH tunnel did not become ready before connect deadline".into())
        }))
    } else {
        Err(SshError::Tunnel(
            "password/encrypted-key tunnel requires russh feature".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_path_under_104_bytes() {
        let cp = control_path();
        // Template with %C is short; real expanded path must also be checked at runtime
        assert!(
            cp.len() < 104,
            "control path template too long: {cp} ({} bytes)",
            cp.len()
        );
        let dir = control_dir();
        // ssh_control_dir must be short enough that dir + "/cm-" + 40-char hash < 104
        let expanded_len = format!("{}/cm-{}", dir.display(), "a".repeat(40)).len();
        assert!(
            expanded_len < 104,
            "expanded control path would be {expanded_len} bytes: {}/cm-<40 hash>",
            dir.display()
        );
    }

    #[test]
    fn master_target_includes_control_path_and_port() {
        let cfg = SshTunnelConfig {
            host: "bastion".into(),
            port: 2222,
            user: "alice".into(),
            auth_type: "agent".into(),
            key_path: None,
            passphrase: None,
            remote_host: "db.internal".into(),
            remote_port: 5432,
        };
        let ssh_cfg = SshConfigEntry {
            port: Some(2200),
            ..Default::default()
        };
        let t = master_target(&cfg, &ssh_cfg, "alice");
        assert!(t.contains(&"-o".to_string()));
        assert!(t.contains(&"alice".to_string()));
        assert!(t.contains(&"2200".to_string()));
    }
}
