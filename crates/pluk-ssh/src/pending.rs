use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub const SSH_PENDING_CODE: &str = "SSH_CONNECT_PENDING";
pub const SSH_STALLED_CODE: &str = "SSH_CONNECT_STALLED";
pub const SSH_CONNECT_WAIT_MS: u64 = 25_000;
pub const SSH_PENDING_MAX_REPORTS: u32 = 2;

#[derive(Debug, Clone)]
struct Episode {
    pending_reports: u32,
    attempt_seq: u64,
    last_error: Option<String>,
    last_error_code: Option<String>,
    last_error_seq: u64,
}

static EPISODES: OnceLock<Mutex<HashMap<String, Episode>>> = OnceLock::new();
static SEQ: OnceLock<Mutex<u64>> = OnceLock::new();

fn episodes() -> &'static Mutex<HashMap<String, Episode>> {
    EPISODES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn seq_lock() -> &'static Mutex<u64> {
    SEQ.get_or_init(|| Mutex::new(0))
}

fn next_seq() -> u64 {
    let mut s = seq_lock().lock().unwrap();
    *s += 1;
    *s
}

fn with_episode<F, R>(key: &str, f: F) -> R
where
    F: FnOnce(&mut Episode) -> R,
{
    let mut map = episodes().lock().unwrap();
    let ep = map.entry(key.to_string()).or_insert_with(|| Episode {
        pending_reports: 0,
        attempt_seq: 0,
        last_error: None,
        last_error_code: None,
        last_error_seq: 0,
    });
    f(ep)
}

pub fn clear_connect_episode(key: &str) {
    episodes().lock().unwrap().remove(key);
}

pub fn start_connect_attempt(key: &str) {
    let seq = next_seq();
    with_episode(key, |ep| {
        ep.attempt_seq = seq;
    });
}

pub fn record_connect_failure(key: &str, err: &dyn std::error::Error) {
    let seq = next_seq();
    let msg = err.to_string();
    // Try to extract code if error has it
    let code = None::<String>; // caller can use record_connect_failure_with_code
    with_episode(key, |ep| {
        ep.last_error = Some(msg);
        ep.last_error_code = code;
        ep.last_error_seq = seq;
    });
}

pub fn record_connect_failure_msg(key: &str, msg: String, code: Option<String>) {
    let seq = next_seq();
    with_episode(key, |ep| {
        ep.last_error = Some(msg.clone());
        ep.last_error_code = code.clone();
        ep.last_error_seq = seq;
    });
}

pub fn record_connect_failure_str(key: &str, msg: &str) {
    record_connect_failure_msg(key, msg.to_string(), None);
}

/// Auth and agent failures are deterministic — must break out immediately rather than retry.
pub fn is_ssh_auth_error(err_msg: &str) -> bool {
    let lower = err_msg.to_ascii_lowercase();
    lower.contains("permission denied")
        || lower.contains("communication with agent failed")
        || lower.contains("signing failed")
        || lower.contains("publickey")
        || lower.contains("no supported authentication")
        || lower.contains("authentication failed")
        || lower.contains("too many authentication failures")
        || lower.contains("ssh key agent")
        || lower.contains("ssh_agent_unreachable")
        || lower.contains("agent unreachable")
}

pub fn is_ssh_auth_error_obj(err: &dyn std::error::Error) -> bool {
    is_ssh_auth_error(&err.to_string())
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct CodedError {
    pub message: String,
    pub code: &'static str,
}

fn coded(code: &'static str, message: String) -> CodedError {
    CodedError { message, code }
}

pub fn ssh_pending_error() -> CodedError {
    coded(
        SSH_PENDING_CODE,
        "SSH connect is still running — authenticating, or waiting on an SSH agent or proxy approval. It continues in the background; retry in a moment. If it keeps repeating, check for a pending agent (e.g. 1Password) prompt.".into(),
    )
}

pub fn ssh_stalled_error(last_error: Option<String>) -> CodedError {
    let detail = last_error
        .map(|m| format!(" Last connect error: {m}"))
        .unwrap_or_default();
    coded(
        SSH_STALLED_CODE,
        format!(
            "SSH connection never came up and no approval landed after {SSH_PENDING_MAX_REPORTS} attempts.{detail}"
        ),
    )
}

pub fn is_ssh_pending(err_code: Option<&str>) -> bool {
    err_code == Some(SSH_PENDING_CODE)
}

pub fn is_ssh_stalled(err_code: Option<&str>) -> bool {
    err_code == Some(SSH_STALLED_CODE)
}

pub fn is_transient_ssh_error(err_code: Option<&str>) -> bool {
    matches!(err_code, Some(SSH_PENDING_CODE) | Some("SSH_AGENT_DENIED"))
}

/// Error for a caller whose bounded wait on an in-flight connect ran out.
/// This attempt dying on auth is the real story — report it, not the pending guess.
/// Otherwise: the guess while it's still plausible, then the last real failure.
pub fn connect_wait_error(key: &str) -> CodedError {
    let mut map = episodes().lock().unwrap();
    let ep = match map.get_mut(key) {
        Some(e) => e,
        None => return ssh_pending_error(),
    };
    let from_this_attempt = ep.last_error_seq >= ep.attempt_seq;
    if let Some(ref msg) = ep.last_error.clone()
        && from_this_attempt && is_ssh_auth_error(msg) {
            let err_msg = msg.clone();
            drop(map);
            // Need to remove episode before returning
            episodes().lock().unwrap().remove(key);
            return coded("SSH_AUTH_ERROR", err_msg);
        }
    ep.pending_reports += 1;
    if ep.pending_reports <= SSH_PENDING_MAX_REPORTS {
        let msg = ssh_pending_error().message.clone();
        let code = ssh_pending_error().code;
        return coded(code, msg);
    }
    let last = ep.last_error.clone();
    drop(map);
    episodes().lock().unwrap().remove(key);
    ssh_stalled_error(last)
}

// Test helper: reset global state
#[cfg(test)]
pub fn reset_for_test() {
    episodes().lock().unwrap().clear();
    *seq_lock().lock().unwrap() = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_rationing_two_then_stalled() {
        reset_for_test();
        let key = "test-rationing";
        start_connect_attempt(key);
        // First two calls return pending
        let e1 = connect_wait_error(key);
        assert_eq!(e1.code, SSH_PENDING_CODE);
        let e2 = connect_wait_error(key);
        assert_eq!(e2.code, SSH_PENDING_CODE);
        // Third returns stalled
        let e3 = connect_wait_error(key);
        assert_eq!(e3.code, SSH_STALLED_CODE);
        // After stalled, episode is cleared — next call is fresh pending
        let e4 = connect_wait_error(key);
        assert_eq!(e4.code, SSH_PENDING_CODE);
        reset_for_test();
    }

    #[test]
    fn auth_error_breaks_immediately() {
        reset_for_test();
        let key = "test-auth-break";
        start_connect_attempt(key);
        record_connect_failure_msg(key, "permission denied (publickey)".into(), None);
        let e = connect_wait_error(key);
        // Auth error from this attempt should be returned directly, not pending
        assert!(e.message.contains("permission denied"));
        assert_ne!(e.code, SSH_PENDING_CODE);
        reset_for_test();
    }

    #[test]
    fn stale_auth_not_returned() {
        reset_for_test();
        let key = "test-stale-auth";
        // Record failure before attempt starts (stale)
        record_connect_failure_msg(key, "permission denied".into(), None);
        start_connect_attempt(key);
        // Now pending should be returned, not the stale auth
        let e = connect_wait_error(key);
        assert_eq!(e.code, SSH_PENDING_CODE);
        reset_for_test();
    }

    #[test]
    fn is_ssh_auth_error_detection() {
        assert!(is_ssh_auth_error("Permission denied (publickey)"));
        assert!(is_ssh_auth_error("No supported authentication methods available"));
        assert!(is_ssh_auth_error("SSH_AGENT_UNREACHABLE"));
        assert!(!is_ssh_auth_error("connection timed out"));
        assert!(!is_ssh_auth_error("tunnel did not become ready"));
    }

    #[test]
    fn sequence_stamps_preserve_order() {
        reset_for_test();
        let key = "test-seq";
        record_connect_failure_msg(key, "old error".into(), None);
        start_connect_attempt(key);
        // old error is before attempt, so pending
        let e = connect_wait_error(key);
        assert_eq!(e.code, SSH_PENDING_CODE);
        // Now record a new failure after attempt started
        record_connect_failure_msg(key, "permission denied".into(), None);
        // Next call should surface auth error (since it's from this attempt)
        // But pending_reports is now 1, need second pending then auth?
        // Actually auth check happens first: fromThisAttempt && isAuth -> return auth
        // So even with pending_reports=1, auth takes priority
        let e2 = connect_wait_error(key);
        assert!(e2.message.contains("permission denied"));
        reset_for_test();
    }
}
