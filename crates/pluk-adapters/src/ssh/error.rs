use crate::error::{AdapterError, SSH_CONNECT_PENDING_CODE};
use crate::sql::error::classify_sql_error;

pub const MAX_COMMAND_TIMEOUT_S: u64 = 600;

pub fn humanize_ssh_error(err: &AdapterError) -> String {
    // timeout case: message contains "timed out"
    if err.message.to_ascii_lowercase().contains("timed out") {
        return format!(
            "{} The command exceeded the timeout — retry with a higher `timeout` (up to {} seconds).",
            err.message, MAX_COMMAND_TIMEOUT_S
        );
    }
    let info = classify_sql_error(err);
    // pending already handled in classify as PendingApproval
    if info.category == crate::sql::error::SqlErrorCategory::PendingApproval {
        return format!("{} {}", info.message, info.hint.unwrap_or_default());
    }
    // connection_failed or query_failed with hint
    if let Some(hint) = info.hint {
        return format!("{} {}", info.message, hint);
    }
    info.message
}

// Helper to detect pending code
pub fn is_pending(err: &AdapterError) -> bool {
    err.has_code(SSH_CONNECT_PENDING_CODE)
}
