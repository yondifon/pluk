//! The HTTP client the API adapters share.
//!
//! A `reqwest::Client` owns a connection pool, so one client per call keeps no
//! connection warm and pays a TLS handshake on every request. Adapters take
//! this one instead and put their deadline on the request.

use std::sync::OnceLock;

use crate::error::AdapterError;

/// The process-wide client. Cloning it shares the connection pool, so every
/// caller reuses the same connections.
pub fn shared() -> Result<reqwest::Client, AdapterError> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .build()
                .map_err(|e| e.to_string())
        })
        .clone()
        .map_err(AdapterError::new)
}
