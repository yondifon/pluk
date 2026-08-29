use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use pluk_core::process::RunError;
use pluk_ssh::SshTunnelConfig;

use crate::error::AdapterError;

pub const DEFAULT_EXEC_TIMEOUT_MS: u64 = 60_000;
pub const MAX_COMMAND_TIMEOUT_S: u64 = 600;
pub const MAX_OUTPUT_BYTES: usize = 1_000_000;

#[derive(Debug, Clone)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i32>,
    pub truncated: bool,
}

// Trait for SSH execution, stubable
#[async_trait::async_trait]
pub trait SshExecutor: Send + Sync {
    async fn exec(
        &self,
        host: &str,
        port: u16,
        user: &str,
        auth_type: &str,
        key_path: Option<String>,
        password: Option<String>,
        command: &str,
        timeout_ms: u64,
    ) -> Result<ExecResult, AdapterError>;
}

// Real executor via OpenSSH
pub struct RealSshExecutor;
#[async_trait::async_trait]
impl SshExecutor for RealSshExecutor {
    async fn exec(
        &self,
        host: &str,
        port: u16,
        user: &str,
        auth_type: &str,
        key_path: Option<String>,
        _password: Option<String>,
        command: &str,
        timeout_ms: u64,
    ) -> Result<ExecResult, AdapterError> {
        use tokio::process::Command as TokioCommand;
        let control_path = pluk_ssh::openssh::control_path();
        let ssh_config = pluk_ssh::config::parse_ssh_config(host);
        let effective_host = ssh_config.host_name.as_deref().unwrap_or(host);
        let effective_port = ssh_config.port.unwrap_or(port);
        let effective_user = if !user.is_empty() {
            user.to_string()
        } else if let Some(u) = ssh_config.user.clone() {
            u
        } else {
            std::env::var("USER").unwrap_or_else(|_| "root".into())
        };

        let mut args: Vec<String> = vec![
            "-o".to_string(),
            format!("ControlPath={}", control_path),
            "-o".to_string(),
            "ControlMaster=auto".to_string(),
            "-o".to_string(),
            "ControlPersist=10m".to_string(),
            "-o".to_string(),
            "ServerAliveInterval=30".to_string(),
            "-o".to_string(),
            "StrictHostKeyChecking=accept-new".to_string(),
            "-p".to_string(),
            effective_port.to_string(),
        ];
        if !effective_user.is_empty() {
            args.push("-l".to_string());
            args.push(effective_user.clone());
        }
        if auth_type == "key" {
            if let Some(kp) = key_path.clone() {
                args.push("-i".to_string());
                args.push(pluk_ssh::config::expand_home(&kp));
                args.push("-o".to_string());
                args.push("IdentitiesOnly=yes".to_string());
                args.push("-o".to_string());
                args.push("IdentityAgent=none".to_string());
            }
        } else if auth_type == "agent" {
            if let Some(agent) = pluk_ssh::agent::resolve_live_agent(host).await {
                args.push("-o".to_string());
                args.push(format!("IdentityAgent=\"{}\"", agent.socket));
            } else {
                return Err(
                    AdapterError::new(pluk_ssh::agent::agent_unreachable_error().message)
                        .with_code(pluk_ssh::agent::SSH_AGENT_UNREACHABLE_CODE),
                );
            }
        }
        // password auth not supported via openssh non-interactively; fallback to error
        if auth_type == "password" {
            return Err(AdapterError::new(
                "password auth not supported via OpenSSH transport; use key or agent",
            ));
        }
        args.push(effective_host.to_string());
        args.push(command.to_string());

        let mut cmd = TokioCommand::new("ssh");
        cmd.args(&args);

        let timeout = Duration::from_millis(timeout_ms);
        match pluk_core::process::run_capture(&mut cmd, timeout).await {
            Ok(output) => {
                let mut stdout_str = String::from_utf8_lossy(&output.stdout).to_string();
                let mut stderr_str = String::from_utf8_lossy(&output.stderr).to_string();
                let truncated = stdout_str.len() + stderr_str.len() > MAX_OUTPUT_BYTES;
                if truncated {
                    if stdout_str.len() > MAX_OUTPUT_BYTES {
                        truncate_chars(&mut stdout_str, MAX_OUTPUT_BYTES);
                        stderr_str.clear();
                    } else {
                        truncate_chars(&mut stderr_str, MAX_OUTPUT_BYTES - stdout_str.len());
                    }
                }
                Ok(ExecResult {
                    stdout: stdout_str,
                    stderr: stderr_str,
                    code: output.code,
                    truncated,
                })
            }
            Err(RunError::TimedOut) => Err(AdapterError::new(format!(
                "Command timed out after {}s and was stopped",
                timeout_ms / 1000
            ))),
            Err(e) => Err(AdapterError::new(e.to_string())),
        }
    }
}

/// Cut `s` down to at most `max` bytes without splitting a character.
fn truncate_chars(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
}

static EXECUTOR: OnceLock<Arc<dyn SshExecutor>> = OnceLock::new();
static TEST_EXECUTOR: OnceLock<Mutex<Option<Arc<dyn SshExecutor>>>> = OnceLock::new();

fn test_executor_slot() -> &'static Mutex<Option<Arc<dyn SshExecutor>>> {
    TEST_EXECUTOR.get_or_init(|| Mutex::new(None))
}

pub fn set_test_executor(exec: Arc<dyn SshExecutor>) {
    *test_executor_slot().lock().unwrap() = Some(exec);
}
pub fn clear_test_executor() {
    *test_executor_slot().lock().unwrap() = None;
}

fn executor() -> Arc<dyn SshExecutor> {
    if let Some(test) = test_executor_slot().lock().unwrap().clone() {
        return test;
    }
    EXECUTOR
        .get_or_init(|| Arc::new(RealSshExecutor) as Arc<dyn SshExecutor>)
        .clone()
}

// Helper to extract ssh params from Integration
fn ssh_params(
    conn: &pluk_store::Integration,
) -> (String, u16, String, String, Option<String>, Option<String>) {
    let host = conn
        .config
        .get("host")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let port = conn
        .config
        .get("port")
        .and_then(|v| v.as_u64())
        .map(|n| n as u16)
        .or_else(|| {
            conn.config
                .get("port")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(22);
    let user = conn
        .config
        .get("user")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let auth_type = conn
        .config
        .get("auth_type")
        .and_then(|v| v.as_str())
        .unwrap_or("agent")
        .to_string();
    let key_path = conn
        .config
        .get("key_path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let password = conn
        .config
        .get("password")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    (host, port, user, auth_type, key_path, password)
}

pub async fn run_command(
    conn: &pluk_store::Integration,
    command: &str,
    timeout_ms: Option<u64>,
) -> Result<ExecResult, AdapterError> {
    let timeout = timeout_ms.unwrap_or(DEFAULT_EXEC_TIMEOUT_MS);
    let clamped = std::cmp::min(timeout, MAX_COMMAND_TIMEOUT_S * 1000);
    let (host, port, user, auth_type, key_path, password) = ssh_params(conn);
    let exec = executor();
    // Check for pending simulation via global stub? executor may return pending error with code
    exec.exec(
        &host, port, &user, &auth_type, key_path, password, command, clamped,
    )
    .await
}

// ── forwards ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ForwardInfo {
    pub id: String,
    pub remote_host: String,
    pub remote_port: u16,
    pub local_port: u16,
}

/// A live local port forward. Dropping it closes the forward.
pub trait Forward: Send + Sync {
    fn local_port(&self) -> u16;
}

impl Forward for pluk_ssh::Tunnel {
    fn local_port(&self) -> u16 {
        self.local_port
    }
}

/// What actually opens the forward — the SSH transport in production, a stub in
/// tests, mirroring the [`SshExecutor`] seam.
#[async_trait::async_trait]
pub trait ForwardOpener: Send + Sync {
    async fn open(&self, config: SshTunnelConfig) -> Result<Arc<dyn Forward>, AdapterError>;
}

struct SshForwardOpener;

#[async_trait::async_trait]
impl ForwardOpener for SshForwardOpener {
    async fn open(&self, config: SshTunnelConfig) -> Result<Arc<dyn Forward>, AdapterError> {
        let tunnel = pluk_ssh::open_ssh_tunnel(config, None)
            .await
            .map_err(|e| AdapterError::new(e.to_string()))?;
        Ok(Arc::new(tunnel))
    }
}

struct ForwardEntry {
    info: ForwardInfo,
    /// Held so the forward stays open until it is closed or the entry is dropped.
    _handle: Arc<dyn Forward>,
}

static FORWARDS: OnceLock<Mutex<HashMap<String, HashMap<String, ForwardEntry>>>> = OnceLock::new();
static USED_PORTS: OnceLock<Mutex<HashSet<u16>>> = OnceLock::new();
static OPENER: OnceLock<Arc<dyn ForwardOpener>> = OnceLock::new();
static TEST_OPENER: OnceLock<Mutex<Option<Arc<dyn ForwardOpener>>>> = OnceLock::new();

fn forwards_map() -> &'static Mutex<HashMap<String, HashMap<String, ForwardEntry>>> {
    FORWARDS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn used_ports() -> &'static Mutex<HashSet<u16>> {
    USED_PORTS.get_or_init(|| Mutex::new(HashSet::new()))
}
fn test_opener_slot() -> &'static Mutex<Option<Arc<dyn ForwardOpener>>> {
    TEST_OPENER.get_or_init(|| Mutex::new(None))
}

pub fn set_test_forward_opener(opener: Arc<dyn ForwardOpener>) {
    *test_opener_slot().lock().unwrap() = Some(opener);
}
pub fn clear_test_forward_opener() {
    *test_opener_slot().lock().unwrap() = None;
}

fn opener() -> Arc<dyn ForwardOpener> {
    if let Some(test) = test_opener_slot().lock().unwrap().clone() {
        return test;
    }
    OPENER
        .get_or_init(|| Arc::new(SshForwardOpener) as Arc<dyn ForwardOpener>)
        .clone()
}

fn owner_key(owner_id: &str, integration_id: &str) -> String {
    format!("{}::{}", owner_id, integration_id)
}

fn port_in_use(port: u16) -> AdapterError {
    AdapterError::new(format!(
        "Local port {} is already in use. Pick another local_port or omit it to auto-assign.",
        port
    ))
}

/// Reject a requested port that is already forwarded or otherwise listening,
/// before the transport tries to bind it and fails with a cryptic message.
fn check_requested_port(port: u16) -> Result<(), AdapterError> {
    if used_ports().lock().unwrap().contains(&port) {
        return Err(port_in_use(port));
    }
    match std::net::TcpListener::bind(format!("127.0.0.1:{}", port)) {
        Ok(listener) => {
            drop(listener);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => Err(port_in_use(port)),
        Err(e) => Err(AdapterError::new(e.to_string())),
    }
}

pub async fn open_forward(
    owner_id: &str,
    conn: &pluk_store::Integration,
    remote_host: &str,
    remote_port: u16,
    requested_local_port: Option<u16>,
) -> Result<ForwardInfo, AdapterError> {
    let rh = if remote_host.trim().is_empty() {
        "localhost".to_string()
    } else {
        remote_host.trim().to_string()
    };
    let id = format!("{}:{}", rh, remote_port);
    let key = owner_key(owner_id, &conn.id);
    {
        let map = forwards_map().lock().unwrap();
        if let Some(inner) = map.get(&key)
            && let Some(entry) = inner.get(&id)
        {
            return Ok(entry.info.clone());
        }
    }
    if let Some(port) = requested_local_port {
        check_requested_port(port)?;
    }

    let (host, port, user, auth_type, key_path, password) = ssh_params(conn);
    let handle = opener()
        .open(SshTunnelConfig {
            host,
            port,
            user,
            auth_type,
            key_path,
            passphrase: password,
            remote_host: rh.clone(),
            remote_port,
            local_port: requested_local_port,
        })
        .await?;

    let info = ForwardInfo {
        id: id.clone(),
        remote_host: rh,
        remote_port,
        local_port: handle.local_port(),
    };
    used_ports().lock().unwrap().insert(info.local_port);
    let mut map = forwards_map().lock().unwrap();
    map.entry(key).or_default().insert(
        id,
        ForwardEntry {
            info: info.clone(),
            _handle: handle,
        },
    );
    Ok(info)
}

pub fn list_forwards(owner_id: &str, conn: &pluk_store::Integration) -> Vec<ForwardInfo> {
    let key = owner_key(owner_id, &conn.id);
    let map = forwards_map().lock().unwrap();
    if let Some(inner) = map.get(&key) {
        let mut v: Vec<ForwardInfo> = inner.values().map(|e| e.info.clone()).collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    } else {
        vec![]
    }
}

pub fn close_forward(owner_id: &str, conn: &pluk_store::Integration, id: &str) -> bool {
    let key = owner_key(owner_id, &conn.id);
    let mut map = forwards_map().lock().unwrap();
    let Some(inner) = map.get_mut(&key) else {
        return false;
    };
    // Dropping the entry closes the forward.
    let Some(entry) = inner.remove(id) else {
        return false;
    };
    used_ports().lock().unwrap().remove(&entry.info.local_port);
    if inner.is_empty() {
        map.remove(&key);
    }
    true
}

#[cfg(test)]
pub fn reset_forwards_for_test() {
    forwards_map().lock().unwrap().clear();
    used_ports().lock().unwrap().clear();
    clear_test_executor();
    clear_test_forward_opener();
}

// Test stub executor
#[cfg(test)]
pub struct StubExecutor {
    pub handler:
        std::sync::Arc<dyn Fn(&str, u64) -> Result<ExecResult, AdapterError> + Send + Sync>,
}
#[cfg(test)]
#[async_trait::async_trait]
impl SshExecutor for StubExecutor {
    async fn exec(
        &self,
        _host: &str,
        _port: u16,
        _user: &str,
        _auth_type: &str,
        _key_path: Option<String>,
        _password: Option<String>,
        command: &str,
        timeout_ms: u64,
    ) -> Result<ExecResult, AdapterError> {
        (self.handler)(command, timeout_ms)
    }
}
