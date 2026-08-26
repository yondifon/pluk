//! MCP server library: HTTP routes, SSE transport, tool registration.
//! Filled in by task R05.
//!
//! The server runs inside the Tauri host process ([`pluk-host`]). The
//! [`pluk-serverd`](src/bin/pluk-serverd.rs) binary is a thin target kept so a
//! future headless deployment stays possible without spawning one today.
//!
//! [`pluk-host`]: ../pluk_host/index.html
