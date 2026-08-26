//! The adapter error type.
//!
//! The TypeScript port throws plain `Error` objects that sometimes carry a
//! stable `code` (e.g. `SSH_CONNECT_PENDING`). Both halves matter: the message
//! is what agents and log rows show, the code is what classifiers and
//! suppression rules branch on.

/// Well-known SSH code: the connection is waiting on an agent approval (e.g. a
/// 1Password prompt). Produced by the SSH client layer; the gated runner
/// suppresses its error hook for it, and SQL error formatting special-cases it.
pub const SSH_CONNECT_PENDING_CODE: &str = "SSH_CONNECT_PENDING";

/// An adapter failure: a human-readable message plus an optional stable code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterError {
    pub message: String,
    pub code: Option<String>,
}

impl AdapterError {
    pub fn new(message: impl Into<String>) -> Self {
        AdapterError { message: message.into(), code: None }
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// Whether this error carries the given stable code.
    pub fn has_code(&self, code: &str) -> bool {
        self.code.as_deref() == Some(code)
    }

    /// Whether this is an SSH pending-approval error (the agent answered but
    /// no approval landed). Such errors never trigger eviction-style cleanup.
    pub fn is_ssh_pending(&self) -> bool {
        self.has_code(SSH_CONNECT_PENDING_CODE)
    }
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AdapterError {}

impl From<String> for AdapterError {
    fn from(message: String) -> Self {
        AdapterError::new(message)
    }
}

impl From<&str> for AdapterError {
    fn from(message: &str) -> Self {
        AdapterError::new(message)
    }
}
