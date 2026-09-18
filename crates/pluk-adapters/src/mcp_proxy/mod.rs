//! Proxying a third-party MCP server: Pluk connects to it as a client,
//! discovers the tools it offers, and re-exposes the ones the user approved.
//!
//! [`client`] owns that upstream connection and nothing else — it reads no
//! store and decides no policy.

pub mod client;
