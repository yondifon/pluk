//! Errors produced by the policy layer.

use std::fmt;

/// Fail-closed errors from the database pin rule.
#[derive(Debug, Clone, PartialEq)]
pub enum PolicyError {
    /// The override identifier failed validation.
    InvalidDatabaseName(String),
    /// The connection is pinned to another database.
    DatabasePinned(String),
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PolicyError::InvalidDatabaseName(name) => write!(f, "Invalid database name: {name}"),
            PolicyError::DatabasePinned(configured) => {
                write!(f, "Connection is locked to database \"{configured}\".")
            }
        }
    }
}

impl std::error::Error for PolicyError {}
