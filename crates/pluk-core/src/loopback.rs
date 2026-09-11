//! The loopback port Pluk serves on.
//!
//! One number, read the same way everywhere: the HTTP server binds it, and
//! the adapter that tests the browser surface dials it. `PORT` overrides it;
//! the interface is always `127.0.0.1`.

const DEFAULT_PORT: u16 = 4242;

/// `PORT` when parseable, else 4242.
pub fn port() -> u16 {
    std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}
