//! The adapter contract: one trait every integration implements.
//!
//! Ported from `pluk/src/adapters/types.ts`. Adding a service means adding
//! one type that implements [`Adapter`] and registering it in the
//! [`AdapterRegistry`](crate::AdapterRegistry) — no edits to the store, MCP
//! transport, or REST layer.

use async_trait::async_trait;

use pluk_store::Integration;

use crate::config_field::ConfigField;
use crate::error::AdapterError;
use crate::tool_host::ToolHost;
use crate::tool_spec::ToolSpec;

/// How the policy/audit layer interprets an adapter.
///
/// - [`PolicyKind::Sql`]: statement-category policy (SELECT/INSERT/…) + SQL
///   guards.
/// - [`PolicyKind::Action`]: read/write action policy.
/// - [`PolicyKind::None`]: no policy gate; every call is confirmed by the
///   client instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyKind {
    Sql,
    Action,
    None,
}

impl PolicyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PolicyKind::Sql => "sql",
            PolicyKind::Action => "action",
            PolicyKind::None => "none",
        }
    }
}

/// A REST request handed to an adapter's optional API handlers. Deliberately
/// minimal — the server crate maps its real HTTP types onto this.
#[derive(Debug, Clone, Default)]
pub struct ApiRequest {
    /// `GET`, `POST`, `DELETE`, …
    pub method: String,
    /// Path + query string, as received (e.g. `/api/integrations/<id>/saved`).
    pub url: String,
    /// Raw request body, when present.
    pub body: Option<String>,
}

/// A REST response an adapter's API handler produced; `None` means "not mine"
/// and lets the server keep routing.
#[derive(Debug, Clone)]
pub struct ApiResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

impl ApiResponse {
    pub fn text(status: u16, body: impl Into<String>) -> Self {
        ApiResponse {
            status,
            content_type: Some("text/plain; charset=utf-8".into()),
            body: body.into().into_bytes(),
        }
    }

    pub fn json(status: u16, value: &serde_json::Value) -> Self {
        let mut response = ApiResponse {
            status,
            content_type: Some("application/json".into()),
            body: serde_json::to_vec(value).unwrap_or_default(),
        };
        // Response.json always serializes; an unwritable value degrades to {}.
        if response.body.is_empty() {
            response.body = b"{}".to_vec();
        }
        response
    }
}

/// The adapter contract.
///
/// Object-safe on purpose: the registry stores `Arc<dyn Adapter>`. Async
/// methods go through `async_trait`, which boxes their futures — adapters are
/// I/O-bound and called at human rates, so the allocation is noise next to
/// the network round trip it fronts.
#[async_trait]
pub trait Adapter: Send + Sync {
    /// Matches `Integration.r#type`.
    fn id(&self) -> &str;
    fn label(&self) -> &str;
    /// Coarse grouping for the UI (`database`, `issue-tracker`, …).
    fn category(&self) -> &str;
    fn policy_kind(&self) -> PolicyKind;
    /// Shown in the UI beside the MCP URL.
    fn agent_hint(&self) -> &str;
    /// The fixed tool set, published once for the catalog/UI. Each tool is
    /// individually toggled on/off and may carry its own settings.
    fn tool_specs(&self) -> &[ToolSpec];
    /// The form schema served verbatim to the frontend (definitions only —
    /// never secret values).
    fn config_fields(&self) -> &[ConfigField];

    /// Verify the config can reach the service. `Err` on failure.
    async fn test_connection(&self, conn: &Integration) -> Result<(), AdapterError>;

    /// Turn a raw failure into something the user can act on (shown by the UI
    /// after a failed connection test). `None` when no translation applies.
    fn humanize_error(&self, error: &AdapterError) -> Option<String> {
        let _ = error;
        None
    }

    /// Per-integration REST API, routed under `/api/integrations/<id>/…`.
    /// Return `None` to decline the request.
    async fn handle_api(
        &self,
        _conn: &Integration,
        _request: ApiRequest,
        _subpath: &str,
    ) -> Option<ApiResponse> {
        None
    }

    /// Global REST API, tried before per-integration routes. Return `None` to
    /// decline the request.
    async fn handle_global_api(&self, request: ApiRequest, path: &str) -> Option<ApiResponse> {
        let _ = (request, path);
        None
    }

    /// Agent-facing guidance for this integration, built per request from
    /// live config + policy. Returned in discovery results and embedded per
    /// member by group endpoints.
    fn instructions(&self, conn: &Integration) -> String;

    /// Register this integration's tools/prompts/resources onto a host.
    ///
    /// A disabled tool must not be registered at all — the agent never sees
    /// it. This is how an integration shrinks its surface (and locks out
    /// write/delete): enable/disable takes effect by rebuilding registration,
    /// never by checking a flag inside the call.
    fn register(
        &self,
        host: &mut dyn ToolHost,
        conn: &Integration,
        owner_id: &str,
    ) -> Result<(), AdapterError>;
}
