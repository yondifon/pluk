use std::cell::RefCell;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use pluk_store::Integration;

use crate::error::AdapterError;

const DEFAULT_TIMEOUT_MS: u64 = 30_000;

fn expand_home(p: &str) -> String {
    if let Some(path) = p.strip_prefix('~') {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{}{}", home, path);
        }
        if let Some(home) = dirs_home() {
            return format!("{}{}", home, path);
        }
    }
    p.to_string()
}

fn dirs_home() -> Option<String> {
    std::env::var("HOME").ok().or_else(|| {
        #[allow(deprecated)]
        std::env::home_dir().map(|p| p.to_string_lossy().to_string())
    })
}

#[derive(Debug, Clone)]
pub struct GhConfig {
    pub bin: String,
    pub default_repo: Option<String>,
    pub default_cwd: String,
    pub timeout_ms: u64,
}

pub fn gh_config(conn: &Integration) -> GhConfig {
    let c = &conn.config;
    let bin_raw = c
        .get("gh_bin")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let bin = if bin_raw.is_empty() {
        "gh".to_string()
    } else {
        expand_home(&bin_raw)
    };
    let default_repo = c
        .get("default_repo")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let default_cwd = c
        .get("default_cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf()).to_string_lossy().to_string());
    let timeout_ms = c
        .get("timeout_seconds")
        .and_then(|v| {
            if let Some(n) = v.as_u64() { Some(n) }
            else if let Some(s) = v.as_str().and_then(|s| s.parse::<i64>().ok()) { if s > 0 { Some(s as u64) } else { None } }
            else if let Some(n) = v.as_i64() { if n > 0 { Some(n as u64) } else { None } }
            else { None }
        })
        .map(|s| s * 1000)
        .filter(|ms| *ms > 0)
        .unwrap_or(DEFAULT_TIMEOUT_MS);
    // also handle f64 via serde_json Number
    let timeout_ms = if timeout_ms == DEFAULT_TIMEOUT_MS {
        if let Some(v) = c.get("timeout_seconds")
            && let Some(f) = v.as_f64() {
                let ms = (f * 1000.0).floor() as i64;
                if ms > 0 {
                    return GhConfig { bin, default_repo, default_cwd, timeout_ms: ms as u64 };
                }
            }
        timeout_ms
    } else {
        timeout_ms
    };
    // reject nonsense
    let timeout_ms = if timeout_ms == 0 { DEFAULT_TIMEOUT_MS } else { timeout_ms };
    GhConfig { bin, default_repo, default_cwd, timeout_ms }
}

pub fn gh_cwd(cfg: &GhConfig, arg: Option<&str>) -> String {
    match arg {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => cfg.default_cwd.clone(),
    }
}

pub fn repo_flag(cfg: &GhConfig, arg: Option<&str>) -> Vec<String> {
    let spec = arg
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| cfg.default_repo.clone())
        .unwrap_or_default();
    if spec.is_empty() {
        vec![]
    } else {
        vec!["--repo".to_string(), spec]
    }
}

pub fn gh_command(cfg: &GhConfig, args: &[String]) -> String {
    let quote = |value: &str| -> String {
        let safe = value.chars().all(|c| matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '.' | '/' | ':' | '@' | '%' | '+' | '=' | ',' | '-'));
        if safe && !value.is_empty() {
            value.to_string()
        } else {
            format!("'{}'", value.replace('\'', "'\\''"))
        }
    };
    let mut parts = vec![quote(&cfg.bin)];
    for a in args {
        parts.push(quote(a));
    }
    parts.join(" ")
}

pub fn positional(value: &str, what: &str) -> Result<String, AdapterError> {
    let v = value.trim().to_string();
    if v.is_empty() {
        return Err(AdapterError::new(format!("{what} is required.")));
    }
    if v.starts_with('-') {
        return Err(AdapterError::new(format!("Invalid {what} \"{value}\" — it must not start with \"-\".")));
    }
    Ok(v)
}

pub fn resolve_repo(cfg: &GhConfig, arg: Option<&str>) -> Result<(String, String), AdapterError> {
    let spec = arg
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| cfg.default_repo.clone())
        .unwrap_or_default();
    if spec.is_empty() {
        return Err(AdapterError::new("No repo given. Pass repo as owner/repo or set a default repo in the integration config."));
    }
    let mut parts = spec.splitn(2, '/');
    let owner = parts.next().unwrap_or("").to_string();
    let repo = parts.next().unwrap_or("").to_string();
    if owner.is_empty() || repo.is_empty() {
        return Err(AdapterError::new(format!("Invalid repo \"{spec}\". Use the form owner/repo.")));
    }
    Ok((owner, repo))
}

#[derive(Debug, Clone)]
pub struct GhRunResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub type GhRunner = Arc<dyn Fn(String, Vec<String>, String, Duration) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<GhRunResult, AdapterError>> + Send>> + Send + Sync>;

thread_local! {
    static TL_RUNNER: RefCell<Option<GhRunner>> = RefCell::new(None);
}
static GLOBAL_RUNNER: OnceLock<Mutex<Option<GhRunner>>> = OnceLock::new();

fn global_runner_slot() -> &'static Mutex<Option<GhRunner>> {
    GLOBAL_RUNNER.get_or_init(|| Mutex::new(None))
}

pub fn set_gh_runner(runner: Option<GhRunner>) {
    TL_RUNNER.with(|c| *c.borrow_mut() = runner.clone());
    *global_runner_slot().lock().unwrap() = runner;
}

async fn spawn_gh(bin: &str, args: &[String], cwd: &str, timeout: Duration) -> Result<GhRunResult, AdapterError> {
    if bin.contains('/') && !Path::new(bin).exists() {
        return Err(AdapterError::new(format!("gh executable not found: {bin}. Install GitHub CLI or set gh_bin on this integration.")));
    }
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args);
    cmd.current_dir(cwd);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.stdin(std::process::Stdio::null());
    let child = cmd.spawn().map_err(|e| {
        let msg = e.to_string();
        if msg.contains("No such file") || msg.contains("ENOENT") || msg.contains("not found") {
            AdapterError::new(format!("gh executable not found (\"{bin}\"). Install GitHub CLI and make sure it is on PATH, or set gh_bin on this integration."))
        } else {
            AdapterError::new(format!("Could not start gh: {msg}"))
        }
    })?;
    let res = tokio::time::timeout(timeout, child.wait_with_output()).await;
    match res {
        Ok(Ok(output)) => {
            let code = output.status.code().unwrap_or(0);
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Ok(GhRunResult { code, stdout, stderr })
        }
        Ok(Err(e)) => Err(AdapterError::new(e.to_string())),
        Err(_) => {
            Err(AdapterError::new(format!("gh {} timed out after {}s.", args.join(" "), timeout.as_secs())))
        }
    }
}

pub async fn run_gh(cfg: &GhConfig, args: Vec<String>, cwd_arg: Option<&str>) -> Result<GhRunResult, AdapterError> {
    let cwd = gh_cwd(cfg, cwd_arg);
    let timeout = Duration::from_millis(cfg.timeout_ms);
    let tl_runner = TL_RUNNER.with(|c| c.borrow().clone());
    if let Some(runner) = tl_runner {
        return runner(cfg.bin.clone(), args, cwd, timeout).await;
    }
    let global = { global_runner_slot().lock().unwrap().clone() };
    if let Some(runner) = global {
        return runner(cfg.bin.clone(), args, cwd, timeout).await;
    }
    spawn_gh(&cfg.bin, &args, &cwd, timeout).await
}

fn gh_error(op: &str, res: &GhRunResult) -> AdapterError {
    let msg = if !res.stderr.trim().is_empty() { res.stderr.trim() } else if !res.stdout.trim().is_empty() { res.stdout.trim() } else { "no output" };
    AdapterError::new(format!("gh {op} failed (exit {}): {msg}", res.code))
}

pub async fn gh_json(cfg: &GhConfig, args: Vec<String>, cwd_arg: Option<&str>) -> Result<serde_json::Value, AdapterError> {
    let op = args.join(" ");
    let res = run_gh(cfg, args, cwd_arg).await?;
    if res.code != 0 {
        return Err(gh_error(&op, &res));
    }
    let text = res.stdout.trim();
    if text.is_empty() {
        return Ok(serde_json::Value::Null);
    }
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(v) => Ok(v),
        Err(_) => Ok(serde_json::Value::String(text.to_string())),
    }
}

pub async fn gh_text(cfg: &GhConfig, args: Vec<String>, cwd_arg: Option<&str>) -> Result<String, AdapterError> {
    let op = args.join(" ");
    let res = run_gh(cfg, args, cwd_arg).await?;
    if res.code != 0 {
        return Err(gh_error(&op, &res));
    }
    Ok(res.stdout.trim().to_string())
}

pub fn humanize_gh_error(error: &AdapterError) -> String {
    let msg = &error.message;
    if msg.contains("executable not found") {
        return format!("{msg}\n\nInstall GitHub CLI (https://cli.github.com) and sign in with `gh auth login`.");
    }
    let lower = msg.to_lowercase();
    if lower.contains("not authenticated") || lower.contains("auth login") || lower.contains("not logged") || lower.contains("please log in") || lower.contains("auth:") {
        return format!("{msg}\n\nRun `gh auth login` in a terminal, then test again.");
    }
    msg.clone()
}

pub async fn test_gh(conn: &Integration) -> Result<(), AdapterError> {
    let cfg = gh_config(conn);
    let res = run_gh(&cfg, vec!["auth".to_string(), "status".to_string()], None).await?;
    if res.code != 0 {
        let msg = if !res.stderr.trim().is_empty() { res.stderr.trim() } else if !res.stdout.trim().is_empty() { res.stdout.trim() } else { &format!("exit {}", res.code) };
        let lower = msg.to_lowercase();
        if lower.contains("not logged") || lower.contains("auth:") || lower.contains("please log in") {
            return Err(AdapterError::new(format!("gh is not authenticated: {msg}. Run `gh auth login`.")));
        }
        return Err(AdapterError::new(format!("gh auth status failed: {msg}")));
    }
    Ok(())
}
