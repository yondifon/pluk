//! The settings screen's view of one proxied server, under
//! `/api/integrations/<id>/proxy/…`, plus the page the browser lands on when
//! the user finishes signing in.
//!
//! Nothing here echoes a credential: the snapshot rows carry no secret, and
//! the auth route answers with how the server is signed in to, never with
//! what was stored.

use serde_json::{Value, json};

use pluk_store::{Integration, ProxyTool, Store};

use crate::adapter::{ApiRequest, ApiResponse};
use crate::error::AdapterError;

use super::catalog;
use super::oauth;

pub(super) const SIGNED_IN: &str = "You're signed in. Close this tab and go back to Pluk.";
pub(super) const SIGN_IN_LAPSED: &str =
    "That sign-in did not go through. Go back to Pluk and start again.";

pub async fn handle_proxy_api(
    store: &Store,
    conn: &Integration,
    request: ApiRequest,
    subpath: &str,
) -> Option<ApiResponse> {
    let route = subpath.strip_prefix("/proxy/")?;
    match (request.method.as_str(), route) {
        ("GET", "tools") => Some(tool_list(catalog::snapshot(store, &conn.id))),
        ("POST", "refresh") => Some(tool_list(catalog::discover(store, conn).await)),
        ("POST", "approve") => Some(approve(store, conn, request.body.as_deref())),
        ("GET", "auth") => Some(auth(store, conn)),
        ("POST", "oauth/start") => Some(start_sign_in(conn).await),
        ("POST", "disconnect") => Some(disconnect(store, conn)),
        _ => None,
    }
}

/// The browser's landing page after the user approves a sign-in. It is reached
/// by a redirect the authorization server controls, so it claims one fixed
/// path and declines everything else.
pub async fn handle_callback(
    store: &Store,
    request: ApiRequest,
    path: &str,
) -> Option<ApiResponse> {
    if path != oauth::CALLBACK_PATH || request.method != "GET" {
        return None;
    }
    Some(match oauth::complete(store, &request.url).await {
        Ok(()) => page(200, SIGNED_IN),
        Err(_) => page(400, SIGN_IN_LAPSED),
    })
}

fn approve(store: &Store, conn: &Integration, body: Option<&str>) -> ApiResponse {
    let names = requested_names(body);
    if names.is_empty() {
        return failed(400, "Choose at least one tool to approve.");
    }
    match store.approve_proxy_tools(&conn.id, &names) {
        Ok(_) => tool_list(catalog::snapshot(store, &conn.id)),
        Err(error) => failed(500, &error.to_string()),
    }
}

fn auth(store: &Store, conn: &Integration) -> ApiResponse {
    match oauth::sign_in_state(store, conn) {
        Ok(state) => ApiResponse::json(
            200,
            &json!({ "ok": true, "auth": { "kind": state.kind, "status": state.status } }),
        ),
        Err(error) => failed(500, &error.message),
    }
}

async fn start_sign_in(conn: &Integration) -> ApiResponse {
    match oauth::start(conn).await {
        Ok(authorize_url) => {
            ApiResponse::json(200, &json!({ "ok": true, "authorizeUrl": authorize_url }))
        }
        Err(error) => failed(502, &error.message),
    }
}

fn disconnect(store: &Store, conn: &Integration) -> ApiResponse {
    match oauth::disconnect(store, &conn.id) {
        Ok(()) => ApiResponse::json(200, &json!({ "ok": true })),
        Err(error) => failed(500, &error.message),
    }
}

fn requested_names(body: Option<&str>) -> Vec<String> {
    body.and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .as_ref()
        .and_then(|body| body.get("names"))
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn tool_list(tools: Result<Vec<ProxyTool>, AdapterError>) -> ApiResponse {
    match tools {
        Ok(tools) => {
            let tools: Vec<Value> = tools.iter().map(listed_tool).collect();
            ApiResponse::json(200, &json!({ "ok": true, "tools": tools }))
        }
        Err(error) => failed(502, &error.message),
    }
}

fn listed_tool(tool: &ProxyTool) -> Value {
    let spec = catalog::spec_for(tool);
    json!({
        "name": spec.name,
        "label": spec.label,
        "description": spec.description,
        "category": spec.category,
        "state": tool.state(),
        "present": tool.present,
        "updatedAt": tool.updated_at,
    })
}

/// One line, in the browser's own font. This page exists to end the trip, not
/// to be an interface.
fn page(status: u16, line: &str) -> ApiResponse {
    ApiResponse {
        status,
        content_type: Some("text/html; charset=utf-8".to_string()),
        body: format!(
            "<!doctype html><meta charset=\"utf-8\"><title>Pluk</title>\
             <body style=\"font:16px system-ui,sans-serif;margin:4rem auto;max-width:28rem;padding:0 1rem\">{line}"
        )
        .into_bytes(),
    }
}

fn failed(status: u16, error: &str) -> ApiResponse {
    ApiResponse::json(status, &json!({ "ok": false, "error": error }))
}
