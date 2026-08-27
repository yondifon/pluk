//! The loopback HTTP surface.
//!
//! Route order mirrors `pluk/src/server.ts`: the fixed REST routes, then the
//! MCP endpoint (token → integration or group), then health. Adapter-supplied
//! APIs are probed from the fallback handler in the TypeScript order — global
//! handlers first, then per-integration subpaths.

use std::collections::BTreeMap;

use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::{Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use futures::StreamExt;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use tower::ServiceExt;

use pluk_adapters::ApiRequest;

use crate::events::parse_after;
use crate::health::HealthStatus;
use crate::logging;
use crate::mcp::{build_owner_surface, resolve_owner};
use crate::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/adapters", get(adapters_catalog))
        .route("/api/integrations/{id}/test", post(test_integration))
        .route("/api/reload", post(reload))
        .route("/api/events", get(events))
        .route("/api/logs", get(logs).delete(clear_logs))
        .route("/api/log/{id}/cancel", post(cancel_log))
        .route("/api/retention", get(get_retention).put(set_retention))
        .route("/mcp/{token}", any(mcp))
        .route("/health", get(|| async { "ok" }))
        .route("/api/health", get(health_report))
        .fallback(adapter_apis_or_not_found)
        .with_state(state)
}

fn json_response(status: StatusCode, value: serde_json::Value) -> Response {
    (status, Json(value)).into_response()
}

// ── Fixed REST routes ────────────────────────────────────────────────────────

/// GET /api/adapters — the adapter catalog for the UI to render forms
/// dynamically. Definitions only; never stored secret values.
async fn adapters_catalog(State(state): State<AppState>) -> Response {
    let adapters: Vec<serde_json::Value> = state
        .registry
        .list()
        .iter()
        .map(|adapter| {
            serde_json::json!({
                "id": adapter.id(),
                "label": adapter.label(),
                "category": adapter.category(),
                "policyKind": adapter.policy_kind(),
                "agentHint": adapter.agent_hint(),
                "tools": adapter.tool_specs(),
                "configFields": adapter.config_fields(),
            })
        })
        .collect();
    json_response(StatusCode::OK, serde_json::json!({ "adapters": adapters }))
}

/// POST /api/integrations/:id/test — run the integration's connection test and
/// record the outcome as its health.
async fn test_integration(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(integration) = state.store.integration_by_id(&id).ok().flatten() else {
        return json_response(StatusCode::NOT_FOUND, serde_json::json!({ "ok": false, "error": "Not found" }));
    };
    let Some(adapter) = state.registry.get(&integration.r#type) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "ok": false, "error": format!("No adapter for type: {}", integration.r#type) }),
        );
    };

    match adapter.test_connection(&integration).await {
        Ok(()) => {
            logging::log_info(&format!("connection test ok: {} ({})", integration.name, integration.id));
            state.health.record(&integration.id, HealthStatus::Ok, None);
            json_response(StatusCode::OK, serde_json::json!({ "ok": true }))
        }
        Err(error) => {
            logging::log_error("connection test failed", &error.message, None);
            let reason = adapter.humanize_error(&error).unwrap_or_else(|| error.message.clone());
            state.health.record(&integration.id, HealthStatus::Error, Some(reason.clone()));
            // A failed test is a valid answer, not a transport error.
            json_response(StatusCode::OK, serde_json::json!({ "ok": false, "error": reason }))
        }
    }
}

/// POST /api/reload?id=<owner> — drop an owner's pooled drivers, tunnels and
/// forwards so credential/override edits take effect on the next agent
/// request. Scoped to one owner when given, all owners otherwise.
async fn reload(State(state): State<AppState>, RawQuery(query): RawQuery) -> Response {
    let id = query_param(query.as_deref(), "id").filter(|v| !v.is_empty());
    let count = state.owners.reset_owners(id.as_deref());
    logging::log_info(&format!(
        "reloaded MCP owners ({count}){}",
        id.map(|i| format!(" for {i}")).unwrap_or_default()
    ));
    json_response(StatusCode::OK, serde_json::json!({ "ok": true, "count": count }))
}

/// GET /api/events?after=<cursor> — held-open SSE stream for the activity log.
async fn events(State(state): State<AppState>, RawQuery(query): RawQuery) -> Response {
    let after = query_param(query.as_deref(), "after");
    match parse_after(after.as_deref()) {
        Some(cursor) => {
            let (replay, rx) = state.events.attach(cursor);
            let frames =
                futures::stream::iter(replay).chain(tokio_stream::wrappers::ReceiverStream::new(rx));
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-cache")
                .header("connection", "keep-alive")
                .body(axum::body::Body::from_stream(frames.map(|frame| {
                    Ok::<_, std::convert::Infallible>(frame.wire())
                })))
                .expect("static SSE response parts")
        }
        None => json_response(StatusCode::BAD_REQUEST, serde_json::json!({ "ok": false, "error": "Invalid cursor" })),
    }
}

/// GET /api/logs — keyset-paged audit-log reads.
async fn logs(State(state): State<AppState>, RawQuery(query): RawQuery) -> Response {
    crate::logs_api::handle(
        &state.store,
        "/api/logs",
        "GET",
        query.as_deref().unwrap_or_default(),
    )
    .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

/// POST /api/log/:id/cancel — abort a single in-flight query by log row id.
async fn cancel_log(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Ok(log_id) = id.parse::<i64>() else {
        return json_response(StatusCode::BAD_REQUEST, serde_json::json!({ "ok": false, "error": "Invalid log id" }));
    };
    let ok = state.cancels.cancel(log_id);
    json_response(StatusCode::OK, serde_json::json!({ "ok": ok }))
}

/// GET /api/retention — current log retention window in days (0 = forever).
async fn get_retention(State(state): State<AppState>) -> Response {
    let days = state.store.retention_days().unwrap_or(30);
    json_response(StatusCode::OK, serde_json::json!({ "days": days }))
}

/// PUT /api/retention — set log retention window. Body: { days: number }.
async fn set_retention(State(state): State<AppState>, bytes: Bytes) -> Response {
    let days: Option<i64> = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| v.get("days").and_then(|d| d.as_i64()));
    let Some(days) = days else {
        return json_response(StatusCode::BAD_REQUEST, serde_json::json!({ "ok": false, "error": "days is required" }));
    };
    let allowed = [0i64, 7, 14, 30, 60, 90];
    if !allowed.contains(&days) {
        return json_response(StatusCode::BAD_REQUEST, serde_json::json!({ "ok": false, "error": "Invalid retention days" }));
    }
    match state.store.set_retention_days(days) {
        Ok(()) => {
            // Purge immediately after changing window so the UI reflects the new limit.
            let _ = state.store.purge_old_logs();
            json_response(StatusCode::OK, serde_json::json!({ "ok": true, "days": days }))
        }
        Err(e) => json_response(StatusCode::INTERNAL_SERVER_ERROR, serde_json::json!({ "ok": false, "error": e.to_string() })),
    }
}

/// DELETE /api/logs?connectionId=… or ?groupId=… — clear all logs for one entity.
async fn clear_logs(State(state): State<AppState>, RawQuery(query): RawQuery) -> Response {
    let params = crate::logs_api::parse_query(query.as_deref().unwrap_or_default());
    let get = |key: &str| params.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str());
    let scope = match (get("connectionId"), get("groupId")) {
        (Some(c), None) if !c.is_empty() => pluk_store::LogScope::Connection(c.to_string()),
        (None, Some(g)) if !g.is_empty() => pluk_store::LogScope::Group(g.to_string()),
        _ => return json_response(StatusCode::BAD_REQUEST, serde_json::json!({ "ok": false, "error": "Exactly one scope required" })),
    };
    match state.store.clear_logs(&scope) {
        Ok(deleted) => json_response(StatusCode::OK, serde_json::json!({ "ok": true, "deleted": deleted })),
        Err(e) => json_response(StatusCode::INTERNAL_SERVER_ERROR, serde_json::json!({ "ok": false, "error": e.to_string() })),
    }
}

/// GET /api/health — per-connection health for the UI.
async fn health_report(State(state): State<AppState>) -> Response {
    let report: BTreeMap<String, crate::health::ConnHealth> = state.health.all();
    json_response(StatusCode::OK, serde_json::json!({ "health": report }))
}

// ── Adapter-supplied APIs ────────────────────────────────────────────────────

async fn adapter_apis_or_not_found(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    bytes: Bytes,
) -> Response {
    let path = uri.path().to_string();
    let request = ApiRequest {
        method: method.to_string(),
        url: build_url(&path, uri.query()),
        // An empty body stands in for an absent one; adapter APIs read JSON.
        body: (!bytes.is_empty()).then(|| String::from_utf8_lossy(&bytes).into_owned()),
    };

    // Global handlers first: any adapter may claim any unmatched path.
    for adapter in state.registry.list() {
        if let Some(response) = adapter.handle_global_api(request.clone(), &path).await {
            return api_response(response);
        }
    }

    // Then per-integration subpaths: /api/integrations/<id>/<subpath>.
    let Some(rest) = path.strip_prefix("/api/integrations/") else {
        return not_found();
    };
    let Some((id, tail)) = rest.split_once('/') else {
        return not_found();
    };
    if id.is_empty() || tail.is_empty() {
        return not_found();
    }
    let Ok(Some(conn)) = state.store.integration_by_id(id) else {
        return json_response(StatusCode::NOT_FOUND, serde_json::json!({ "ok": false, "error": "Not found" }));
    };
    let Some(adapter) = state.registry.get(&conn.r#type) else {
        return not_found();
    };
    let subpath = format!("/{tail}");
    match adapter.handle_api(&conn, request, &subpath).await {
        Some(response) => api_response(response),
        None => not_found(),
    }
}

fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    crate::logs_api::parse_query(query.unwrap_or_default())
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}

fn build_url(path: &str, query: Option<&str>) -> String {
    match query {
        Some(query) if !query.is_empty() => format!("{path}?{query}"),
        _ => path.to_string(),
    }
}

fn api_response(response: pluk_adapters::ApiResponse) -> Response {
    let mut builder =
        Response::builder().status(StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR));
    if let Some(content_type) = response.content_type {
        builder = builder.header("content-type", content_type);
    }
    builder.body(axum::body::Body::from(response.body)).expect("valid response parts")
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "Not found").into_response()
}

// ── MCP endpoint ─────────────────────────────────────────────────────────────

/// /mcp/:token — MCP streamable HTTP for AI agents. The token resolves to a
/// single integration or a group; everything long-lived keys on that owner.
async fn mcp(State(state): State<AppState>, Path(token): Path<String>, request: axum::extract::Request) -> Response {
    let owner = match resolve_owner(&state.store, &state.registry, &token) {
        Ok(Some(owner)) => owner,
        Ok(None) => return (StatusCode::NOT_FOUND, "Integration not found").into_response(),
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };
    let owner_id = owner.owner_id().to_string();
    state.owners.open_owner(&owner_id);

    let app_state = state.clone();
    let token_for_factory = token;
    // Stateless serving: the factory runs per protocol request, so the surface
    // always reflects current config (tool toggles included).
    let service = StreamableHttpService::new(
        move || {
            let owner = resolve_owner(&app_state.store, &app_state.registry, &token_for_factory)
                .map_err(std::io::Error::other)?
                .ok_or_else(|| std::io::Error::other("owner vanished"))?;
            build_owner_surface(&owner, &app_state.store, &app_state.registry).map_err(std::io::Error::other)
        },
        state.sessions.clone(),
        crate::ServerConfig::mcp_transport_config(),
    );

    match service.oneshot(request).await {
        Ok(response) => response.map(axum::body::Body::new),
        Err(infallible) => match infallible {},
    }
}
