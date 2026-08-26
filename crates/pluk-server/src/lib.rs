//! MCP serving for Pluk: the loopback HTTP surface, the MCP protocol
//! endpoint, token routing, group namespacing, the live event stream and log
//! paging.
//!
//! A library crate by design: the Tauri host ([`pluk-host`]) runs the server
//! in-process. [`serve`] binds `127.0.0.1` on port 4242 (overridable by
//! `PORT`, never by any other configuration — loopback-only is a product
//! promise) and drains in-flight requests when the shutdown token fires. The
//! [`pluk-serverd`](src/bin/pluk-serverd.rs) binary is a thin target kept so
//! a future headless deployment stays possible without spawning one today.
//!
//! The MCP endpoint is served statelessly (no initialize handshake required,
//! no session id): the protocol revision is negotiated per request, and the
//! server instance behind an endpoint token is rebuilt on every request so
//! config edits and tool enable/disable take effect immediately. Long-lived
//! resources (driver pools, tunnels) are keyed by the *owner* — the
//! integration or group a `/mcp/<token>` path resolves to.
//!
//! Ported from `pluk/src/server.ts`, `pluk/src/mcp/*`, `pluk/src/events.ts`
//! and `pluk/src/logs.ts`.
//!
//! [`pluk-host`]: ../pluk_host/index.html

mod cancel;
mod events;
mod health;
mod http;
mod logging;
mod logs_api;
pub mod mcp;

use std::net::SocketAddr;
use std::sync::Arc;

use pluk_adapters::AdapterRegistry;
use pluk_store::Store;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig,
};

pub use cancel::CancelRegistry;
pub use events::{parse_after, EventHub};
pub use health::{ConnHealth, HealthMap, HealthStatus};
pub use http::router;
pub use mcp::owner::OwnerPool;

/// Everything the HTTP surface needs. Cheap to clone; hand it to adapters and
/// background tasks freely.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub registry: Arc<AdapterRegistry>,
    /// Owner-scoped abort handles + close hooks (reload target).
    pub owners: Arc<OwnerPool>,
    /// Per-integration health surfaced at `GET /api/health`.
    pub health: Arc<HealthMap>,
    /// Per-query abort handles (`POST /api/log/:id/cancel`).
    pub cancels: Arc<CancelRegistry>,
    pub(crate) events: Arc<EventHub>,
    pub(crate) sessions: Arc<LocalSessionManager>,
}

impl AppState {
    pub fn new(
        store: Arc<Store>,
        registry: Arc<AdapterRegistry>,
        owners: Arc<OwnerPool>,
        health: Arc<HealthMap>,
    ) -> Self {
        let events = Arc::new(EventHub::new(store.clone()));
        AppState::with_event_hub(store, registry, owners, health, events)
    }

    /// Like [`AppState::new`] with an explicit event hub (tests tune the
    /// keepalive interval and buffer size through this).
    pub fn with_event_hub(
        store: Arc<Store>,
        registry: Arc<AdapterRegistry>,
        owners: Arc<OwnerPool>,
        health: Arc<HealthMap>,
        events: Arc<EventHub>,
    ) -> Self {
        AppState {
            store,
            registry,
            owners,
            health,
            cancels: Arc::new(CancelRegistry::default()),
            events,
            sessions: Arc::new(LocalSessionManager::default()),
        }
    }
}

/// Server construction options.
pub struct ServerConfig {
    pub store: Arc<Store>,
    pub registry: Arc<AdapterRegistry>,
    pub owners: Arc<OwnerPool>,
    pub health: Arc<HealthMap>,
    pub cancels: Arc<CancelRegistry>,
    /// Loopback port to bind. Defaults from `PORT` (else 4242); the interface
    /// is always `127.0.0.1`.
    pub port: Option<u16>,
}

impl ServerConfig {
    pub fn new(store: Arc<Store>, registry: Arc<AdapterRegistry>) -> Self {
        ServerConfig {
            store,
            registry,
            owners: Arc::new(OwnerPool::default()),
            health: Arc::new(HealthMap::default()),
            cancels: Arc::new(CancelRegistry::default()),
            port: None,
        }
    }

    fn bind_addr(&self) -> SocketAddr {
        let port = self.port.unwrap_or_else(Self::default_port);
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    /// `PORT` when parseable, else 4242.
    pub fn default_port() -> u16 {
        std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(4242)
    }

    fn into_state(self) -> AppState {
        let ServerConfig { store, registry, owners, health, cancels, .. } = self;
        let mut state = AppState::new(store, registry, owners, health);
        state.cancels = cancels;
        state
    }

    /// The streamable-HTTP transport config every MCP endpoint shares:
    /// stateless for modern and legacy revisions alike, plain JSON replies,
    /// loopback hosts only.
    pub(crate) fn mcp_transport_config() -> StreamableHttpServerConfig {
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true)
    }
}

/// Bind the loopback HTTP surface and serve until `shutdown` is cancelled.
///
/// Cancelling the token stops accepting connections, ends held-open event
/// streams (so they cannot stall the drain), and waits for in-flight requests
/// before returning.
pub async fn serve(config: ServerConfig, shutdown: tokio_util::sync::CancellationToken) -> std::io::Result<()> {
    let addr = config.bind_addr();
    let state = config.into_state();
    let app = http::router(state.clone());

    let listener = tokio::net::TcpListener::bind(addr).await?;
    logging::log_info(&format!("pluk MCP server on http://localhost:{addr}"));

    axum::serve(listener, app)
        .with_graceful_shutdown({
            let events = state.events.clone();
            async move {
                shutdown.cancelled().await;
                events.shutdown().await;
            }
        })
        .await
}
