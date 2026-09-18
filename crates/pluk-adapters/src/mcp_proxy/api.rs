//! The settings screen's view of one proxied server, under
//! `/api/integrations/<id>/proxy/…`.
//!
//! Nothing here echoes a credential: the snapshot rows carry no secret, and
//! the auth route answers with how the server is signed in to, never with
//! what was stored.

use serde_json::{Value, json};

use pluk_store::{Integration, ProxyTool, Store};

use crate::adapter::{ApiRequest, ApiResponse};
use crate::error::AdapterError;

use super::catalog;
use super::client::UpstreamAuth;

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
        ("GET", "auth") => Some(auth(conn)),
        _ => None,
    }
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

fn auth(conn: &Integration) -> ApiResponse {
    let kind = match catalog::upstream_auth(conn) {
        UpstreamAuth::None => "none",
        _ => "token",
    };
    ApiResponse::json(
        200,
        &json!({ "ok": true, "auth": { "kind": kind, "status": "connected" } }),
    )
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

fn failed(status: u16, error: &str) -> ApiResponse {
    ApiResponse::json(status, &json!({ "ok": false, "error": error }))
}
