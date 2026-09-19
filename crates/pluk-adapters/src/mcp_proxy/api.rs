//! The settings screen's view of one proxied server, under
//! `/api/integrations/<id>/proxy/…`, plus the page the browser lands on when
//! the user finishes signing in.
//!
//! Nothing here echoes a credential: the snapshot rows carry no secret, and
//! the auth route answers with how the server is signed in to, never with
//! what was stored.

use serde_json::{Map, Value, json};

use pluk_store::{
    Integration, IntegrationUpdate, ProxyTool, Store, ToolPolicy, parse_query_policy,
    serialize_query_policy,
};

use crate::adapter::{ApiRequest, ApiResponse};
use crate::error::AdapterError;

use super::catalog;
use super::oauth;
use super::probe::{self, SignInRequired};

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
        ("POST", "refresh") => {
            probe::forget(&conn.id);
            Some(tool_list(catalog::discover(store, conn).await))
        }
        ("POST", "approve") => Some(approve(store, conn, request.body.as_deref())),
        ("POST", "enable") => Some(set_enabled(store, conn, request.body.as_deref())),
        ("GET", "auth") => Some(auth(store, conn).await),
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
    let names = requested_names(&parse_body(body));
    if names.is_empty() {
        return failed(400, "Choose at least one tool.");
    }
    match store.approve_proxy_tools(&conn.id, &names) {
        Ok(_) => tool_list(catalog::snapshot(store, &conn.id)),
        Err(error) => failed(500, &error.to_string()),
    }
}

/// Turning a tool on is one step for the user and two writes here: the tool is
/// pinned to the definition the server offers right now, then its switch goes
/// on. Turning it off writes the switch alone, so a tool that comes back on
/// asks nothing of the user.
///
/// The pin comes first. A write that stops half way leaves a tool pinned and
/// off, which reaches no agent; the other order would leave one switched on
/// that the user never saw the current definition of.
fn set_enabled(store: &Store, conn: &Integration, body: Option<&str>) -> ApiResponse {
    let body = parse_body(body);
    let names = requested_names(&body);
    if names.is_empty() {
        return failed(400, "Choose at least one tool.");
    }
    let on = body
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if on && let Err(error) = store.approve_proxy_tools(&conn.id, &names) {
        return failed(500, &error.to_string());
    }
    match switch_tools(store, &conn.id, &names, on) {
        Ok(()) => tool_list(catalog::snapshot(store, &conn.id)),
        Err(error) => failed(500, &error),
    }
}

/// Write the named switches into the integration's policy, leaving every
/// sibling key and every other tool's settings as they were.
fn switch_tools(store: &Store, id: &str, names: &[String], on: bool) -> Result<(), String> {
    let Some(stored) = store.integration_by_id(id).map_err(|e| e.to_string())? else {
        return Err("This integration is no longer in Pluk.".to_string());
    };
    let mut policy = parse_query_policy(stored.query_policy.as_deref()).unwrap_or_default();
    for name in names {
        match policy.tools.get_mut(name) {
            Some(tool) => tool.enabled = on,
            None => {
                policy.tools.insert(
                    name.clone(),
                    ToolPolicy {
                        enabled: on,
                        settings: Map::new(),
                        extra: Map::new(),
                    },
                );
            }
        }
    }
    let update = IntegrationUpdate {
        query_policy: Some(Some(serialize_query_policy(&policy))),
        ..Default::default()
    };
    store
        .update_integration(id, &update)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// How this server is signed in to, and what it asks for in the first place.
/// The screen needs both: the stored sign-in says where the user got to, and
/// the server's own answer says which step it was.
async fn auth(store: &Store, conn: &Integration) -> ApiResponse {
    let state = match oauth::sign_in_state(store, conn) {
        Ok(state) => state,
        Err(error) => return failed(500, &error.message),
    };
    let required = match probe::required(conn).await {
        Ok(required) => required,
        Err(error) => return failed(502, &error.message),
    };
    let mut auth = json!({
        "kind": state.kind,
        "status": state.status,
        "required": required.as_str(),
    });
    if matches!(required, SignInRequired::Oauth { .. }) {
        auth["needsClientId"] = json!(probe::needs_client_id(conn, required));
    }
    ApiResponse::json(200, &json!({ "ok": true, "auth": auth }))
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

fn parse_body(body: Option<&str>) -> Value {
    body.and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .unwrap_or(Value::Null)
}

fn requested_names(body: &Value) -> Vec<String> {
    body.get("names")
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
