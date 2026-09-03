use thiserror::Error;

#[derive(Debug, Error)]
pub enum DriverError {
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("query failed: {0}")]
    Query(String),
    #[error("cancelled")]
    Cancelled,
    #[error("timeout after {0}ms")]
    Timeout(u64),
    #[error("SSL error: {0}")]
    Ssl(String),
    #[error("invalid database name: {0}")]
    InvalidDatabaseName(String),
    #[error("connection is locked to database \"{0}\"")]
    DatabasePinned(String),
    #[error("unsupported database type: {0}")]
    UnsupportedType(String),
    #[error("pool error: {0}")]
    Pool(String),
    #[error("{0}")]
    Other(String),
}

impl From<pluk_policy::PolicyError> for DriverError {
    fn from(e: pluk_policy::PolicyError) -> Self {
        match e {
            pluk_policy::PolicyError::InvalidDatabaseName(n) => Self::InvalidDatabaseName(n),
            pluk_policy::PolicyError::DatabasePinned(n) => Self::DatabasePinned(n),
        }
    }
}
