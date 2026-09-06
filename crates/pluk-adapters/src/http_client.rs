//! The HTTP client the API adapters share.
//!
//! A `reqwest::Client` owns a connection pool, so building one per call throws
//! away every kept-alive TLS connection and pays a fresh handshake on the next
//! request. One client, built once, keeps those connections warm across tool
//! calls; per-call deadlines go on the request instead.

use std::sync::OnceLock;

use crate::error::AdapterError;

/// The shared client. Cloning one shares the underlying connection pool, so
/// every caller gets the same connections.
pub fn shared_client() -> Result<reqwest::Client, AdapterError> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| reqwest::Client::builder().build().map_err(|e| e.to_string()))
        .clone()
        .map_err(AdapterError::new)
}
