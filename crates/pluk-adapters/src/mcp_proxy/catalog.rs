//! The snapshot of what an upstream server offers, and how Pluk reaches it.
//!
//! Discovery is the only writer of the snapshot: it asks upstream what it has,
//! hashes each tool, and replaces the rows. An approval is bound to that hash,
//! so a tool whose description, schema or hints moved reads as changed and
//! stops being exposed until the owner approves it again.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use pluk_store::{DiscoveredTool, Integration, ProxyTool, Store, StoreError, ToolState};

use crate::error::AdapterError;
use crate::tool_spec::ToolSpec;

use super::client::{self, DEFAULT_AUTH_HEADER, McpProxyClient, UpstreamAuth, UpstreamTool};

/// The upstream address, rejected before anything tries to open a session.
pub fn endpoint(conn: &Integration) -> Result<String, AdapterError> {
    match config_str(conn, "url") {
        Some(url) if url.starts_with("http://") || url.starts_with("https://") => Ok(url),
        Some(_) => Err(AdapterError::new(
            "The server URL has to start with http:// or https://.",
        )),
        None => Err(AdapterError::new("Add the URL of the MCP server.")),
    }
}

/// What Pluk presents to one upstream server.
///
/// Every caller resolves credentials here and nowhere else, so a new sign-in
/// style reaches discovery, the refresh endpoint and every proxied call at
/// once.
pub fn upstream_auth(conn: &Integration) -> UpstreamAuth {
    match config_str(conn, "token") {
        None => UpstreamAuth::None,
        Some(token) => UpstreamAuth::header(
            &config_str(conn, "header_name").unwrap_or_else(|| DEFAULT_AUTH_HEADER.to_string()),
            &token,
        ),
    }
}

/// A client for one integration, with the pooled session dropped when the
/// address or the credentials moved since it was opened.
pub fn client_for(conn: &Integration) -> Result<McpProxyClient, AdapterError> {
    let endpoint = endpoint(conn)?;
    let auth = upstream_auth(conn);
    let fingerprint = fingerprint(&endpoint, &auth);
    let previous = session_fingerprints()
        .lock()
        .expect("mcp proxy fingerprints")
        .insert(conn.id.clone(), fingerprint.clone());
    if previous.is_some_and(|previous| previous != fingerprint) {
        client::invalidate(&conn.id);
    }
    Ok(McpProxyClient::new(&conn.id, endpoint, auth))
}

/// Ask upstream what it offers now and replace the snapshot with the answer.
pub async fn discover(store: &Store, conn: &Integration) -> Result<Vec<ProxyTool>, AdapterError> {
    let tools = client_for(conn)?.list_tools().await?;
    let discovered: Vec<DiscoveredTool> = tools.iter().map(discovered_from).collect();
    store
        .replace_proxy_tools(&conn.id, &discovered)
        .map_err(store_failure)?;
    snapshot(store, &conn.id)
}

pub fn snapshot(store: &Store, integration_id: &str) -> Result<Vec<ProxyTool>, AdapterError> {
    store
        .list_proxy_tools(integration_id)
        .map_err(store_failure)
}

/// Whether the owner approved this exact tool as upstream describes it now.
pub fn is_approved(store: &Store, integration_id: &str, name: &str) -> bool {
    snapshot(store, integration_id)
        .unwrap_or_default()
        .iter()
        .any(|tool| tool.name == name && tool.state() == ToolState::Approved)
}

/// The catalog entry for one snapshot row: what the settings screen renders a
/// toggle for, and what the policy gate reads its default from.
pub fn spec_for(tool: &ProxyTool) -> ToolSpec {
    ToolSpec::new(tool.name.clone(), tool.description.clone(), category(tool))
}

/// The policy category upstream's hints put a tool in.
///
/// `readOnlyHint` and `destructiveHint` come from the upstream server and are
/// hints, not guarantees: they pick the default toggle, and nothing more.
pub fn category(tool: &ProxyTool) -> &'static str {
    let annotations: Option<Value> = tool
        .annotations_json
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok());
    let hint = |key: &str| {
        annotations
            .as_ref()
            .and_then(|value| value.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    if hint("readOnlyHint") {
        "read"
    } else if hint("destructiveHint") {
        "delete"
    } else {
        "write"
    }
}

/// Everything an approval is bound to, hashed. Object keys serialize in sorted
/// order, so a server that re-emits the same schema in another order hashes to
/// the same value.
fn content_hash(tool: &UpstreamTool) -> String {
    digest(
        &json!({
            "name": tool.name,
            "description": tool.description,
            "inputSchema": tool.input_schema,
            "annotations": tool.annotations,
        })
        .to_string(),
    )
}

fn discovered_from(tool: &UpstreamTool) -> DiscoveredTool {
    DiscoveredTool {
        name: tool.name.clone(),
        description: tool.description.clone(),
        schema_json: tool.input_schema.to_string(),
        annotations_json: tool.annotations.as_ref().map(Value::to_string),
        content_hash: content_hash(tool),
    }
}

fn config_str(conn: &Integration, key: &str) -> Option<String> {
    conn.config
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// What an open session was opened with, without keeping the secret around.
fn fingerprint(endpoint: &str, auth: &UpstreamAuth) -> String {
    let credential = match auth {
        UpstreamAuth::None => String::new(),
        UpstreamAuth::Header { name, value } => format!("{name}\u{0}{value}"),
        UpstreamAuth::Bearer { access_token } => format!("bearer\u{0}{access_token}"),
    };
    digest(&format!("{endpoint}\u{0}{credential}"))
}

fn session_fingerprints() -> &'static Mutex<HashMap<String, String>> {
    static FINGERPRINTS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    FINGERPRINTS.get_or_init(Default::default)
}

fn digest(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn store_failure(error: StoreError) -> AdapterError {
    AdapterError::new(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_proxy::tests::integration;

    fn upstream_tool(description: &str, annotations: Option<Value>) -> UpstreamTool {
        UpstreamTool {
            name: "search".to_string(),
            description: description.to_string(),
            input_schema: json!({"type": "object", "properties": {"q": {"type": "string"}}}),
            annotations,
        }
    }

    #[test]
    fn key_order_does_not_change_the_hash_but_a_description_does() {
        let one = UpstreamTool {
            input_schema: json!({"properties": {"q": {"type": "string"}}, "type": "object"}),
            ..upstream_tool("Search the docs", None)
        };
        assert_eq!(
            content_hash(&upstream_tool("Search the docs", None)),
            content_hash(&one)
        );
        assert_ne!(
            content_hash(&upstream_tool("Search the docs", None)),
            content_hash(&upstream_tool("Search everything", None))
        );
    }

    #[test]
    fn annotations_are_part_of_what_was_approved() {
        assert_ne!(
            content_hash(&upstream_tool("Search", None)),
            content_hash(&upstream_tool(
                "Search",
                Some(json!({"readOnlyHint": true}))
            ))
        );
    }

    #[test]
    fn upstream_hints_pick_the_category() {
        let of = |annotations: Option<Value>| {
            category(&ProxyTool {
                integration_id: "int-1".to_string(),
                name: "search".to_string(),
                description: String::new(),
                schema_json: "{}".to_string(),
                annotations_json: annotations.as_ref().map(Value::to_string),
                content_hash: String::new(),
                approved_hash: None,
                present: true,
                discovered_at: String::new(),
                updated_at: String::new(),
            })
        };
        assert_eq!(of(Some(json!({"readOnlyHint": true}))), "read");
        assert_eq!(of(Some(json!({"destructiveHint": true}))), "delete");
        assert_eq!(of(Some(json!({"title": "Search"}))), "write");
        assert_eq!(of(None), "write");
    }

    #[test]
    fn an_address_that_is_not_http_is_refused() {
        let conn = integration("int-1", json!({"url": "ftp://files.example.com"}));
        assert!(endpoint(&conn).is_err());
        assert!(endpoint(&integration("int-1", json!({}))).is_err());
        assert_eq!(
            endpoint(&integration(
                "int-1",
                json!({"url": "https://example.com/mcp"})
            ))
            .expect("address"),
            "https://example.com/mcp"
        );
    }

    #[test]
    fn a_bare_token_signs_in_as_a_bearer_header_and_a_named_one_goes_raw() {
        assert_eq!(
            upstream_auth(&integration("int-1", json!({"url": "https://up/mcp"}))),
            UpstreamAuth::None
        );
        assert_eq!(
            upstream_auth(&integration(
                "int-1",
                json!({"url": "https://up/mcp", "token": "t0ken"})
            )),
            UpstreamAuth::Header {
                name: "Authorization".to_string(),
                value: "Bearer t0ken".to_string(),
            }
        );
        assert_eq!(
            upstream_auth(&integration(
                "int-1",
                json!({"url": "https://up/mcp", "token": "t0ken", "header_name": "X-Api-Key"})
            )),
            UpstreamAuth::Header {
                name: "X-Api-Key".to_string(),
                value: "t0ken".to_string(),
            }
        );
    }
}
