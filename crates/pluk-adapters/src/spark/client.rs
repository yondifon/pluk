use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use pluk_core::process::RunError;
use pluk_store::Integration;

use crate::error::AdapterError;

const DEFAULT_BIN: &str = "/usr/local/bin/spark";
const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_MAX_PAGE: u64 = 25;

fn expand_home(p: &str) -> String {
    if let Some(path) = p.strip_prefix('~') {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{}{}", home, path);
        }
        #[allow(deprecated)]
        if let Some(home) = std::env::home_dir() {
            return format!("{}{}", home.to_string_lossy(), path);
        }
    }
    p.to_string()
}

fn str_val(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.trim().to_string(),
        serde_json::Value::Null => String::new(),
        other => other.as_str().unwrap_or("").trim().to_string(),
    }
}

fn positive_u64(v: Option<&serde_json::Value>, fallback: u64) -> u64 {
    let Some(val) = v else { return fallback };
    let n = if let Some(n) = val.as_u64() {
        n as i64
    } else if let Some(n) = val.as_i64() {
        n
    } else if let Some(s) = val.as_str().and_then(|s| s.parse::<i64>().ok()) {
        s
    } else if let Some(f) = val.as_f64() {
        f.floor() as i64
    } else {
        return fallback;
    };
    if n > 0 { n as u64 } else { fallback }
}

#[derive(Debug, Clone)]
pub struct SparkCfg {
    pub bin: String,
    pub account: String,
    pub folder: String,
    pub team: String,
    pub max_page_size: u64,
    pub timeout_ms: u64,
}

pub fn spark_config(conn: &Integration) -> SparkCfg {
    let c = &conn.config;
    let bin_raw = c.get("spark_bin").map(str_val).unwrap_or_default();
    let bin = if bin_raw.is_empty() {
        DEFAULT_BIN.to_string()
    } else {
        expand_home(&bin_raw)
    };
    let account = c.get("default_account").map(str_val).unwrap_or_default();
    let folder = c.get("default_folder").map(str_val).unwrap_or_default();
    let team = c.get("default_team").map(str_val).unwrap_or_default();
    let max_page_size = positive_u64(c.get("max_page_size"), DEFAULT_MAX_PAGE);
    let timeout_ms = positive_u64(c.get("timeout_seconds"), 30) * 1000;
    let timeout_ms = if timeout_ms == 0 {
        DEFAULT_TIMEOUT_MS
    } else {
        timeout_ms
    };
    SparkCfg {
        bin,
        account,
        folder,
        team,
        max_page_size,
        timeout_ms,
    }
}

// ── helpers ──────────────────────────────────────────────────────────────

pub fn assert_positional(value: &str, what: &str) -> Result<String, AdapterError> {
    let v = value.trim().to_string();
    if v.is_empty() {
        return Err(AdapterError::new(format!("{what} is required.")));
    }
    if v.starts_with('-') {
        return Err(AdapterError::new(format!(
            "Invalid {what} \"{value}\" — it must not start with \"-\"."
        )));
    }
    Ok(v)
}

pub fn assert_message_id(value: &str, what: &str) -> Result<String, AdapterError> {
    let v = value.trim().to_string();
    if v.is_empty() {
        return Err(AdapterError::new(format!("{what} is required.")));
    }
    let is_numeric = v.chars().all(|c| c.is_ascii_digit());
    let is_link = v.starts_with("https://sparkmailapp.com/")
        || v.starts_with("readdle-spark://")
        || v.starts_with("readdlespark://");
    if !is_numeric && !is_link {
        return Err(AdapterError::new(format!(
            "Invalid {what} \"{v}\" — pass a numeric id from list_emails or a Spark deep link."
        )));
    }
    Ok(v)
}

fn mailbox_of(id: &str) -> &str {
    match id.find(':') {
        Some(idx) => &id[..idx],
        None => id,
    }
}

fn out_of_scope(account: &str, what: &str, value: &str) -> AdapterError {
    AdapterError::new(format!(
        "This integration is scoped to {account}; {what} \"{value}\" is another mailbox. Omit it to use {account}, or clear the integration's Account setting to reach every mailbox."
    ))
}

pub fn scoped(cfg: &SparkCfg, value: Option<&str>, what: &str) -> Result<String, AdapterError> {
    let v = value.unwrap_or("").trim().to_string();
    if v.is_empty() || cfg.account.is_empty() {
        return Ok(v);
    }
    if mailbox_of(&v).eq_ignore_ascii_case(&cfg.account) {
        return Ok(v);
    }
    if !v.contains(':') && !v.contains('@') {
        return Ok(format!("{}:{}", cfg.account, v));
    }
    Err(out_of_scope(&cfg.account, what, &v))
}

pub fn same_account(
    cfg: &SparkCfg,
    value: Option<&str>,
    what: &str,
) -> Result<String, AdapterError> {
    let v = value.unwrap_or("").trim().to_string();
    if cfg.account.is_empty() {
        return Ok(v);
    }
    if v.is_empty() {
        return Ok(cfg.account.clone());
    }
    if !v.eq_ignore_ascii_case(&cfg.account) {
        return Err(out_of_scope(&cfg.account, what, &v));
    }
    Ok(v)
}

pub fn list_values(value: Option<&serde_json::Value>) -> Vec<String> {
    match value {
        None => vec![],
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => vec![s.trim().to_string()],
        Some(other) => {
            let s = str_val(other);
            if s.is_empty() { vec![] } else { vec![s] }
        }
    }
}

/// Append `--flag value` when value is present.
pub fn flag(args: &mut Vec<String>, name: &str, value: Option<&str>) {
    if let Some(v) = value {
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            args.push(name.to_string());
            args.push(trimmed.to_string());
        }
    }
}

/// Append `--flag value` once per item.
pub fn flag_each(args: &mut Vec<String>, name: &str, values: &[String]) {
    for item in values {
        if !item.trim().is_empty() {
            args.push(name.to_string());
            args.push(item.trim().to_string());
        }
    }
}

/// Append bare `--flag` when true.
pub fn toggle(args: &mut Vec<String>, name: &str, value: bool) {
    if value {
        args.push(name.to_string());
    }
}

pub fn paging(args: &mut Vec<String>, cfg: &SparkCfg, page: Option<i64>, page_size: Option<i64>) {
    if let Some(p) = page
        && p > 1
    {
        args.push("--page".to_string());
        args.push(p.to_string());
    }
    let size = match page_size {
        Some(n) if n > 0 => n as u64,
        _ => cfg.max_page_size,
    };
    let capped = std::cmp::min(size, cfg.max_page_size);
    args.push("--page-size".to_string());
    args.push(capped.to_string());
}

pub fn range_args(
    args: &mut Vec<String>,
    start: Option<&str>,
    end: Option<&str>,
    range: Option<&str>,
) {
    let s = start.unwrap_or("").trim().to_string();
    let e = end.unwrap_or("").trim().to_string();
    if !s.is_empty() || !e.is_empty() {
        flag(args, "--start", if s.is_empty() { None } else { Some(&s) });
        flag(args, "--end", if e.is_empty() { None } else { Some(&e) });
        return;
    }
    if let Some(r) = range {
        let t = r.trim();
        if !t.is_empty() {
            args.push(format!("--{t}"));
        }
    }
}

// ── process ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SparkRunResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub type SparkRunner = Arc<
    dyn Fn(
            String,
            Vec<String>,
            Duration,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<SparkRunResult, AdapterError>> + Send>,
        > + Send
        + Sync,
>;

static GLOBAL_RUNNER: OnceLock<Mutex<Option<SparkRunner>>> = OnceLock::new();

fn global_runner_slot() -> &'static Mutex<Option<SparkRunner>> {
    GLOBAL_RUNNER.get_or_init(|| Mutex::new(None))
}

pub fn set_spark_runner(runner: Option<SparkRunner>) {
    *global_runner_slot().lock().unwrap() = runner;
}

async fn spawn_spark(
    bin: &str,
    args: &[String],
    timeout: Duration,
) -> Result<SparkRunResult, AdapterError> {
    if bin.contains('/') && !Path::new(bin).exists() {
        return Err(AdapterError::new(format!(
            "Spark CLI not found: {bin}. Install Spark Desktop, or set the CLI path on this integration."
        )));
    }
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args);
    match pluk_core::process::run_capture(&mut cmd, timeout).await {
        Ok(output) => Ok(SparkRunResult {
            code: output.code.unwrap_or(0),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        }),
        Err(RunError::Spawn(e)) => {
            let msg = e.to_string();
            if msg.contains("No such file") || msg.contains("ENOENT") || msg.contains("not found") {
                Err(AdapterError::new(format!(
                    "Spark CLI not found: {bin}. Install Spark Desktop, or set the CLI path on this integration."
                )))
            } else {
                Err(AdapterError::new(format!("Could not start spark: {msg}")))
            }
        }
        Err(RunError::Io(e)) => Err(AdapterError::new(e.to_string())),
        Err(RunError::TimedOut) => Err(AdapterError::new(format!(
            "spark {} timed out after {}s and was stopped.",
            args.first().map(|s| s.as_str()).unwrap_or(""),
            timeout.as_secs()
        ))),
    }
}

pub async fn run_spark(cfg: &SparkCfg, args: Vec<String>) -> Result<String, AdapterError> {
    let timeout = Duration::from_millis(cfg.timeout_ms);
    let runner_opt = { global_runner_slot().lock().unwrap().clone() };
    let result = if let Some(runner) = runner_opt {
        runner(cfg.bin.clone(), args.clone(), timeout).await?
    } else {
        spawn_spark(&cfg.bin, &args, timeout).await?
    };
    if result.code != 0 {
        let msg = if !result.stderr.trim().is_empty() {
            result.stderr.trim().to_string()
        } else if !result.stdout.trim().is_empty() {
            result.stdout.trim().to_string()
        } else {
            format!(
                "spark {} failed (exit {}).",
                args.first().map(|s| s.as_str()).unwrap_or(""),
                result.code
            )
        };
        return Err(AdapterError::new(msg));
    }
    let out = result.stdout.trim().to_string();
    if out.is_empty() {
        Ok("(no output)".to_string())
    } else {
        Ok(out)
    }
}

pub fn spark_command(cfg: &SparkCfg, args: &[String]) -> String {
    let quote = |value: &str| -> String {
        if value.is_empty() {
            "''".to_string()
        } else if value.chars().all(|c| matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '.' | '/' | ':' | '@' | '%' | '+' | '=' | ',' | '-')) {
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

pub fn humanize_spark_error(error: &AdapterError) -> String {
    let msg = &error.message;
    let lower = msg.to_lowercase();
    if lower.contains("spark desktop running")
        || lower.contains("connection refused")
        || lower.contains("connect")
    {
        return format!(
            "{msg}\n\nSpark Desktop must be running with its CLI server enabled (Settings → AI Agents)."
        );
    }
    if lower.contains("access level")
        || lower.contains("read-only")
        || lower.contains("triage")
        || lower.contains("send access")
    {
        return format!(
            "{msg}\n\nRaise the account's access level in Spark Desktop → Settings → AI Agents."
        );
    }
    msg.clone()
}

pub async fn test_spark(conn: &Integration) -> Result<(), AdapterError> {
    let cfg = spark_config(conn);
    run_spark(&cfg, vec!["--version".to_string()]).await?;
    run_spark(&cfg, vec!["accounts".to_string()]).await?;
    Ok(())
}

/// The runner seam is process-global, so the tests that install one run in
/// sequence.
#[cfg(test)]
pub(crate) static RUNNER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{recorded_pid, tree_script, wait_until_gone};
    use serde_json::json;

    fn cfg_with_account(account: &str) -> SparkCfg {
        SparkCfg {
            bin: "/usr/local/bin/spark".to_string(),
            account: account.to_string(),
            folder: String::new(),
            team: String::new(),
            max_page_size: 25,
            timeout_ms: 5000,
        }
    }

    #[test]
    fn paging_adds_page_and_caps_size() {
        let cfg = cfg_with_account("");
        let mut a = vec!["emails".to_string()];
        paging(&mut a, &cfg, Some(1), Some(100));
        assert_eq!(a, vec!["emails", "--page-size", "25"]);
        let mut b = vec!["emails".to_string()];
        paging(&mut b, &cfg, Some(2), Some(10));
        assert_eq!(b, vec!["emails", "--page", "2", "--page-size", "10"]);
        let mut c = vec!["emails".to_string()];
        paging(&mut c, &cfg, None, None);
        assert_eq!(c, vec!["emails", "--page-size", "25"]);
    }

    #[test]
    fn range_prefers_start_end_over_shortcut() {
        let mut a = Vec::new();
        range_args(
            &mut a,
            Some("2026-01-01"),
            Some("2026-01-02"),
            Some("today"),
        );
        assert_eq!(a, vec!["--start", "2026-01-01", "--end", "2026-01-02"]);
        let mut b = Vec::new();
        range_args(&mut b, None, None, Some("week"));
        assert_eq!(b, vec!["--week"]);
        let mut c = Vec::new();
        range_args(&mut c, None, None, None);
        assert!(c.is_empty());
    }

    #[test]
    fn repeated_flag_appends_each() {
        let mut a = Vec::new();
        flag_each(
            &mut a,
            "--to",
            &["a@b.com".to_string(), "c@d.com".to_string()],
        );
        assert_eq!(a, vec!["--to", "a@b.com", "--to", "c@d.com"]);
        let mut b = Vec::new();
        flag(&mut b, "--filter", Some("from:joe"));
        assert_eq!(b, vec!["--filter", "from:joe"]);
        let mut c = Vec::new();
        flag(&mut c, "--filter", Some(""));
        assert!(c.is_empty());
        let mut d = Vec::new();
        toggle(&mut d, "--personal", true);
        assert_eq!(d, vec!["--personal"]);
        let mut e = Vec::new();
        toggle(&mut e, "--personal", false);
        assert!(e.is_empty());
    }

    #[test]
    fn assert_positional_rejects_flag_like() {
        assert!(assert_positional("-bad", "query").is_err());
        assert!(assert_positional("  ", "query").is_err());
        assert_eq!(assert_positional("hello", "query").unwrap(), "hello");
    }

    #[test]
    fn assert_message_id_accepts_numeric_and_links() {
        assert!(assert_message_id("123", "message id").is_ok());
        assert!(assert_message_id("https://sparkmailapp.com/abc", "message id").is_ok());
        assert!(assert_message_id("readdle-spark://x", "message id").is_ok());
        assert!(assert_message_id("readdlespark://x", "message id").is_ok());
        assert!(assert_message_id("bad-id", "message id").is_err());
        assert!(assert_message_id("", "message id").is_err());
    }

    #[test]
    fn scoped_qualifies_bare_and_refuses_other() {
        let cfg = cfg_with_account("you@co.com");
        assert_eq!(
            scoped(&cfg, Some("Inbox"), "folder").unwrap(),
            "you@co.com:Inbox"
        );
        assert_eq!(
            scoped(&cfg, Some("you@co.com:Archive"), "folder").unwrap(),
            "you@co.com:Archive"
        );
        assert!(scoped(&cfg, Some("other@co.com:Inbox"), "folder").is_err());
        assert!(scoped(&cfg, Some("Team Name:Folder"), "folder").is_err());
        let cfg2 = cfg_with_account("");
        assert_eq!(scoped(&cfg2, Some("Inbox"), "folder").unwrap(), "Inbox");
    }

    #[test]
    fn same_account_refuses_other() {
        let cfg = cfg_with_account("you@co.com");
        assert_eq!(
            same_account(&cfg, Some("you@co.com"), "account").unwrap(),
            "you@co.com"
        );
        assert!(same_account(&cfg, Some("other@co.com"), "account").is_err());
        assert_eq!(same_account(&cfg, None, "account").unwrap(), "you@co.com");
        let cfg2 = cfg_with_account("");
        assert_eq!(
            same_account(&cfg2, Some("other@co.com"), "account").unwrap(),
            "other@co.com"
        );
    }

    #[test]
    fn humanize_maps_desktop_and_access() {
        let e = AdapterError::new("Spark Desktop running is required");
        assert!(humanize_spark_error(&e).contains("Spark Desktop must be running"));
        let e2 = AdapterError::new("Connection refused");
        assert!(humanize_spark_error(&e2).contains("Spark Desktop must be running"));
        let e3 = AdapterError::new("read-only access level insufficient");
        assert!(humanize_spark_error(&e3).contains("Raise the account"));
        let e4 = AdapterError::new("something else");
        assert_eq!(humanize_spark_error(&e4), "something else");
    }

    #[tokio::test]
    async fn missing_binary_produces_clear_error() {
        let _g = crate::spark::client::RUNNER_LOCK.lock().await;
        set_spark_runner(None);
        let cfg = SparkCfg {
            bin: "/nonexistent/spark-binary-xyz".to_string(),
            account: String::new(),
            folder: String::new(),
            team: String::new(),
            max_page_size: 25,
            timeout_ms: 1000,
        };
        let err = run_spark(&cfg, vec!["accounts".to_string()])
            .await
            .unwrap_err();
        assert!(
            err.message.contains("Spark CLI not found"),
            "got: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn timed_out_command_leaves_nothing_running() {
        let _g = crate::spark::client::RUNNER_LOCK.lock().await;
        set_spark_runner(None);
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("grandchild.pid");
        let cfg = SparkCfg {
            bin: "/bin/sh".to_string(),
            account: String::new(),
            folder: String::new(),
            team: String::new(),
            max_page_size: 25,
            timeout_ms: 400,
        };
        let err = run_spark(&cfg, vec!["-c".to_string(), tree_script(&pid_file)])
            .await
            .unwrap_err();
        assert!(err.message.contains("timed out"), "got: {}", err.message);
        let grandchild = recorded_pid(&pid_file).await;
        assert!(
            wait_until_gone(grandchild).await,
            "a timed-out spark call left {grandchild} running"
        );
    }

    #[tokio::test]
    async fn verbatim_passthrough_trims_output() {
        let _g = crate::spark::client::RUNNER_LOCK.lock().await;
        let cfg = SparkCfg {
            bin: "/usr/local/bin/spark".to_string(),
            account: String::new(),
            folder: String::new(),
            team: String::new(),
            max_page_size: 25,
            timeout_ms: 5000,
        };
        let runner: crate::spark::client::SparkRunner = std::sync::Arc::new(|_, _, _| {
            Box::pin(async {
                Ok(SparkRunResult {
                    code: 0,
                    stdout: "  hello world  \n".to_string(),
                    stderr: String::new(),
                })
            })
        });
        set_spark_runner(Some(runner));
        let out = run_spark(&cfg, vec!["accounts".to_string()]).await.unwrap();
        assert_eq!(out, "hello world");
        set_spark_runner(None);
    }

    #[test]
    fn spark_command_never_builds_shell_string_with_injection() {
        let cfg = cfg_with_account("");
        let cmd = spark_command(&cfg, &["search".to_string(), "hello; rm -rf /".to_string()]);
        assert!(cmd.contains("'hello; rm -rf /'"), "got: {cmd}");
    }

    #[test]
    fn spark_config_reads_integration() {
        let conn = pluk_store::Integration {
            id: "1".into(),
            name: "x".into(),
            r#type: "spark".into(),
            config: {
                let mut m = serde_json::Map::new();
                m.insert("spark_bin".into(), json!("/tmp/spark"));
                m.insert("default_account".into(), json!("you@co.com"));
                m.insert("max_page_size".into(), json!(10));
                m
            },
            environment: None,
            read_only: 0,
            query_policy: None,
            token: "tok".into(),
            created_at: String::new(),
            via_group: None,
        };
        let c = spark_config(&conn);
        assert_eq!(c.bin, "/tmp/spark");
        assert_eq!(c.account, "you@co.com");
        assert_eq!(c.max_page_size, 10);
    }
}
