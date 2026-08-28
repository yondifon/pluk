use pluk_db::error::DriverError;

use crate::error::{AdapterError, SSH_CONNECT_PENDING_CODE};

pub const SSH_AGENT_DENIED_CODE: &str = "SSH_AGENT_DENIED";
pub const SSH_AGENT_UNREACHABLE_CODE: &str = "SSH_AGENT_UNREACHABLE";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlErrorCategory {
    AuthFailed,
    TunnelFailed,
    QueryFailed,
    ConnectionFailed,
    PendingApproval,
}

impl SqlErrorCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            SqlErrorCategory::AuthFailed => "auth_failed",
            SqlErrorCategory::TunnelFailed => "tunnel_failed",
            SqlErrorCategory::QueryFailed => "query_failed",
            SqlErrorCategory::ConnectionFailed => "connection_failed",
            SqlErrorCategory::PendingApproval => "pending_approval",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SqlErrorInfo {
    pub category: SqlErrorCategory,
    pub message: String,
    pub hint: Option<String>,
    pub code: String,
}

fn contains(haystack: &str, needle: &str) -> bool {
    haystack.contains(needle)
}

pub fn classify_sql_error(err: &AdapterError) -> SqlErrorInfo {
    let msg = &err.message;
    let code = err.code.clone();

    if err.is_ssh_pending() {
        return SqlErrorInfo {
            category: SqlErrorCategory::PendingApproval,
            message: "SSH connection is waiting on an approval.".to_string(),
            hint: Some("Approve the 1Password or proxy sign-in prompt, then retry. If none is visible, click Test in Pluk to start a fresh connection.".to_string()),
            code: code.unwrap_or_else(|| "SSH_CONNECT_PENDING".to_string()),
        };
    }

    // stalled detection via code or message? Use simple check
    if code.as_deref() == Some("SSH_CONNECT_STALLED") || contains(msg, "stalled") {
        return SqlErrorInfo {
            category: SqlErrorCategory::TunnelFailed,
            message: msg.clone(),
            hint: Some("The stuck attempt was dropped — retry to open a brand-new SSH connection. If it keeps failing, check the host/proxy is reachable and your SSH agent is unlocked.".to_string()),
            code: code.unwrap_or_else(|| "SSH_CONNECT_STALLED".to_string()),
        };
    }

    if code.as_deref() == Some(SSH_AGENT_DENIED_CODE) || regex::Regex::new(r"agent refused operation|signing failed .* agent").unwrap().is_match(msg) {
        return SqlErrorInfo {
            category: SqlErrorCategory::AuthFailed,
            message: "Your SSH agent refused to sign.".to_string(),
            hint: Some("Check 1Password for a pending approval, or unlock it, then retry.".to_string()),
            code: SSH_AGENT_DENIED_CODE.to_string(),
        };
    }

    if code.as_deref() == Some(SSH_AGENT_UNREACHABLE_CODE) || regex::Regex::new(r"communication with agent failed|SSH_AUTH_SOCK|open agent|could not open a connection to your authentication agent|No reply from server").unwrap().is_match(msg) {
        return SqlErrorInfo {
            category: SqlErrorCategory::AuthFailed,
            message: "Can't reach your SSH key agent.".to_string(),
            hint: Some("Open and unlock 1Password (with its SSH agent enabled), or load the key into ssh-agent, then retry.".to_string()),
            code: SSH_AGENT_UNREACHABLE_CODE.to_string(),
        };
    }

    if regex::Regex::new(r"Permission denied \(publickey\)|no matching (?:host )?key|no mutual signature|All configured authentication methods failed").unwrap().is_match(msg) {
        return SqlErrorInfo {
            category: SqlErrorCategory::AuthFailed,
            message: "SSH rejected the key.".to_string(),
            hint: Some("Check the SSH user and make sure the agent has a key this host accepts.".to_string()),
            code: code.unwrap_or_else(|| "SSH_KEY_REJECTED".to_string()),
        };
    }

    if regex::Regex::new(r"connection reset by peer|cloudflared|ProxyCommand exited|did not become ready|unexpected EOF|process exited before tunnel").unwrap().is_match(msg) {
        return SqlErrorInfo {
            category: SqlErrorCategory::TunnelFailed,
            message: "SSH proxy connection dropped.".to_string(),
            hint: Some("Retry to re-authenticate the proxy session, especially for Cloudflare Access.".to_string()),
            code: code.unwrap_or_else(|| "SSH_TUNNEL_DROPPED".to_string()),
        };
    }

    if code.as_deref() == Some("28P01") || code.as_deref() == Some("28000") || regex::Regex::new(r"password authentication failed|SASL authentication failed").unwrap().is_match(msg) {
        return SqlErrorInfo { category: SqlErrorCategory::AuthFailed, message: "Database authentication failed.".to_string(), hint: Some("Check username and password.".to_string()), code: code.unwrap_or_else(|| "DB_AUTH_FAILED".to_string()) };
    }

    if code.as_deref() == Some("3D000") || regex::Regex::new(r"database .* does not exist").unwrap().is_match(msg) {
        return SqlErrorInfo { category: SqlErrorCategory::ConnectionFailed, message: "Database not found.".to_string(), hint: Some("Check the database name.".to_string()), code: code.unwrap_or_else(|| "DB_NOT_FOUND".to_string()) };
    }

    if code.as_deref() == Some("ECONNREFUSED") || contains(msg, "ECONNREFUSED") {
        return SqlErrorInfo { category: SqlErrorCategory::ConnectionFailed, message: "Connection refused.".to_string(), hint: Some("Check host, port, firewall, and SSH tunnel config.".to_string()), code: code.unwrap_or_else(|| "ECONNREFUSED".to_string()) };
    }

    if code.as_deref() == Some("ENOTFOUND") || regex::Regex::new(r"no such host|name or service not known").unwrap().is_match(msg) {
        return SqlErrorInfo { category: SqlErrorCategory::ConnectionFailed, message: "Host not found.".to_string(), hint: Some("Check the host name.".to_string()), code: code.unwrap_or_else(|| "ENOTFOUND".to_string()) };
    }

    if regex::Regex::new(r"self.signed|certificate|\bssl\b|\btls\b").unwrap().is_match(&msg.to_lowercase()) {
        return SqlErrorInfo { category: SqlErrorCategory::ConnectionFailed, message: "SSL error.".to_string(), hint: Some("Check SSL mode and certificates.".to_string()), code: code.unwrap_or_else(|| "SSL_ERROR".to_string()) };
    }

    if regex::Regex::new(r"timed out|connection timeout|timeout expired").unwrap().is_match(&msg.to_lowercase()) {
        return SqlErrorInfo { category: SqlErrorCategory::ConnectionFailed, message: "Timed out.".to_string(), hint: Some("Check host, port, SSH tunnel, and firewall/VPC rules.".to_string()), code: code.unwrap_or_else(|| "TIMEOUT".to_string()) };
    }

    if regex::Regex::new(r"no usable private key|cannot parse privatekey|encrypted.*passphrase|bad passphrase").unwrap().is_match(msg) {
        return SqlErrorInfo { category: SqlErrorCategory::AuthFailed, message: "SSH key problem.".to_string(), hint: Some("Check key path and passphrase.".to_string()), code: code.unwrap_or_else(|| "SSH_KEY_INVALID".to_string()) };
    }

    if regex::Regex::new(r"host key|hostkey").unwrap().is_match(&msg.to_lowercase()) {
        return SqlErrorInfo { category: SqlErrorCategory::AuthFailed, message: "SSH host key was rejected.".to_string(), hint: None, code: code.unwrap_or_else(|| "SSH_HOST_KEY_REJECTED".to_string()) };
    }

    SqlErrorInfo { category: SqlErrorCategory::QueryFailed, message: msg.clone(), hint: None, code: code.unwrap_or_else(|| "QUERY_FAILED".to_string()) }
}

pub fn humanize_sql_error(err: &AdapterError) -> String {
    let info = classify_sql_error(err);
    if let Some(hint) = info.hint {
        format!("{} {}", info.message, hint)
    } else {
        format!("{} (see Logs for details)", info.message)
    }
}

pub fn format_sql_error(err: &AdapterError) -> String {
    let info = classify_sql_error(err);
    let mut obj = serde_json::Map::new();
    let mut inner = serde_json::Map::new();
    inner.insert("category".into(), serde_json::Value::String(info.category.as_str().to_string()));
    inner.insert("message".into(), serde_json::Value::String(info.message));
    if let Some(hint) = info.hint {
        inner.insert("hint".into(), serde_json::Value::String(hint));
    }
    inner.insert("code".into(), serde_json::Value::String(info.code));
    obj.insert("error".into(), serde_json::Value::Object(inner));
    format!("Error: {}", serde_json::to_string_pretty(&serde_json::Value::Object(obj)).unwrap())
}

pub fn driver_error_to_adapter(err: DriverError) -> AdapterError {
    match err {
        DriverError::Cancelled => AdapterError::new("Query cancelled"),
        DriverError::Timeout(ms) => AdapterError::new(format!("Timed out after {}ms", ms)),
        DriverError::InvalidDatabaseName(n) => AdapterError::new(format!("Invalid database name \"{}\". Allowed: letters, digits, _, $, -.", n)),
        DriverError::DatabasePinned(db) => AdapterError::new(format!("This connection is locked to database \"{}\". USE is blocked.", db)).with_code("DB_PINNED"),
        DriverError::Connection(m) => AdapterError::new(m),
        DriverError::Query(m) => AdapterError::new(m),
        DriverError::Ssl(m) => AdapterError::new(m),
        DriverError::UnsupportedType(t) => AdapterError::new(format!("Unsupported DB type: {}", t)),
        DriverError::Pool(m) => AdapterError::new(m),
        DriverError::Other(m) => {
            if m.contains(SSH_CONNECT_PENDING_CODE) {
                AdapterError::new(m).with_code(SSH_CONNECT_PENDING_CODE)
            } else {
                AdapterError::new(m)
            }
        }
    }
}
