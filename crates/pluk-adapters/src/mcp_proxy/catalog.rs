//! The snapshot of what an upstream server offers, and how Pluk reaches it.
//!
//! Discovery is the only writer of the snapshot: it asks upstream what it has,
//! hashes each tool, and replaces the rows. An approval is bound to that hash,
//! so a tool whose description, schema or hints moved reads as changed and
//! stops being exposed until the owner approves it again.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use upstream_http::Url;

use pluk_store::{DiscoveredTool, Integration, ProxyTool, Store, StoreError, ToolState};

use crate::error::AdapterError;
use crate::tool_spec::ToolSpec;

use super::client::{self, DEFAULT_AUTH_HEADER, McpProxyClient, UpstreamAuth, UpstreamTool};
use super::import;
use super::local;
use super::oauth;
use super::transport::{self, StaticHeader};

/// The address would carry a credential in the clear to somewhere else.
pub const INSECURE_ADDRESS_CODE: &str = "MCP_PROXY_INSECURE_ADDRESS";

const INSECURE_ADDRESS: &str =
    "Use https:// for this server. http:// only works for a server on this computer.";
const NOT_AN_ADDRESS: &str = "The server URL has to start with http:// or https://.";
const NO_ADDRESS: &str = "Add the URL of the MCP server.";

/// The upstream address, rejected before anything tries to open a session.
///
/// Plain `http` reaches loopback and nothing else. Everything Pluk sends a
/// server travels with whatever the user signed in with, and off this machine
/// that is a wire anyone on the path can read.
pub fn endpoint(conn: &Integration) -> Result<String, AdapterError> {
    let Some(url) = config_str(conn, "url") else {
        return Err(AdapterError::new(NO_ADDRESS));
    };
    let Ok(parsed) = Url::parse(&url) else {
        return Err(AdapterError::new(NOT_AN_ADDRESS));
    };
    match parsed.scheme() {
        "https" => Ok(url),
        "http" if is_loopback(&parsed) => Ok(url),
        "http" => Err(AdapterError::new(INSECURE_ADDRESS).with_code(INSECURE_ADDRESS_CODE)),
        _ => Err(AdapterError::new(NOT_AN_ADDRESS)),
    }
}

/// Whether an address may be reached at all, by the same rule [`endpoint`]
/// holds the upstream to: off this machine, nothing travels in the clear.
pub(super) fn is_secure_address(url: &Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => is_loopback(url),
        _ => false,
    }
}

/// Whether the address names this machine, the one place cleartext stays on.
fn is_loopback(url: &Url) -> bool {
    match url.host_str() {
        Some(host) => {
            let host = host.trim_start_matches('[').trim_end_matches(']');
            host.eq_ignore_ascii_case("localhost")
                || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
        }
        None => false,
    }
}

/// What Pluk presents to one upstream server.
///
/// Every caller resolves credentials here and nowhere else, so a new sign-in
/// style reaches discovery, the refresh endpoint and every proxied call at
/// once. A sign-in the user completed in Pluk wins over a token typed into the
/// config, because it is the one Pluk can keep current.
pub async fn upstream_auth(
    store: &Store,
    conn: &Integration,
) -> Result<UpstreamAuth, AdapterError> {
    match oauth::bearer(store, conn).await? {
        Some(access_token) => Ok(UpstreamAuth::bearer(access_token)),
        None => Ok(static_auth(conn)),
    }
}

/// The credentials the integration's own config carries, with no store behind
/// them.
pub fn static_auth(conn: &Integration) -> UpstreamAuth {
    match config_str(conn, "token") {
        None => UpstreamAuth::None,
        Some(token) => UpstreamAuth::header(
            &config_str(conn, "header_name").unwrap_or_else(|| DEFAULT_AUTH_HEADER.to_string()),
            &token,
        ),
    }
}

/// A client for one integration, with the pooled session dropped when the
/// address, the credentials or the headers moved since it was opened.
///
/// A local server's session is dropped, and the server stopped, when its
/// launch moved; the new launch starts only once the user approved it.
pub async fn client_for(store: &Store, conn: &Integration) -> Result<McpProxyClient, AdapterError> {
    if local::is_local(conn) {
        let spec = local::launch_spec(store, conn).await?;
        keep_session_if_unchanged(&conn.id, spec.launch_hash());
        return local::client_for(store, conn, spec).await;
    }
    let endpoint = endpoint(conn)?;
    let auth = upstream_auth(store, conn).await?;
    let headers = transport::static_headers(store, conn)?;
    keep_session_if_unchanged(&conn.id, fingerprint(&endpoint, &auth, &headers));
    Ok(McpProxyClient::new(&conn.id, endpoint, auth).with_headers(headers))
}

/// Shut the pooled session down when what it was opened with moved.
fn keep_session_if_unchanged(integration_id: &str, fingerprint: String) {
    let previous = session_fingerprints()
        .lock()
        .expect("mcp proxy fingerprints")
        .insert(integration_id.to_string(), fingerprint.clone());
    if previous.is_some_and(|previous| previous != fingerprint) {
        client::shutdown(integration_id);
    }
}

/// Ask upstream what it offers now and replace the snapshot with the answer.
/// Tools an imported config turned off are switched off once they are found.
pub async fn discover(store: &Store, conn: &Integration) -> Result<Vec<ProxyTool>, AdapterError> {
    let tools = client_for(store, conn).await?.list_tools().await?;
    let discovered: Vec<DiscoveredTool> = tools.iter().map(discovered_from).collect();
    store
        .replace_proxy_tools(&conn.id, &discovered)
        .map_err(store_failure)?;
    let found: Vec<String> = tools.iter().map(|tool| tool.name.clone()).collect();
    import::apply_pending_off(store, &conn.id, &found).map_err(store_failure)?;
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
///
/// Nothing an MCP server offers is on until the owner turns it on. A server
/// describes its own tools, so letting that description decide would let a
/// server that describes itself generously ship enabled.
pub fn spec_for(tool: &ProxyTool) -> ToolSpec {
    ToolSpec::new(tool.name.clone(), tool.description.clone(), category(tool))
        .with_default_enabled(false)
}

/// The policy category upstream's hints put a tool in.
///
/// `readOnlyHint` and `destructiveHint` come from the upstream server and are
/// hints, not guarantees: they label the row and decide nothing.
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

pub(super) fn config_str(conn: &Integration, key: &str) -> Option<String> {
    conn.config
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// What an open session was opened with, without keeping the secret around.
fn fingerprint(endpoint: &str, auth: &UpstreamAuth, headers: &[StaticHeader]) -> String {
    let credential = match auth {
        UpstreamAuth::None => String::new(),
        UpstreamAuth::Header { name, value } => format!("{name}\u{0}{value}"),
        UpstreamAuth::Bearer { access_token } => format!("bearer\u{0}{access_token}"),
    };
    let headers = transport::digest(headers);
    digest(&format!("{endpoint}\u{0}{credential}\u{0}{headers}"))
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

    fn proxy_tool(annotations: Option<Value>) -> ProxyTool {
        ProxyTool {
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
        }
    }

    #[test]
    fn upstream_hints_pick_the_category() {
        let of = |annotations: Option<Value>| category(&proxy_tool(annotations));
        assert_eq!(of(Some(json!({"readOnlyHint": true}))), "read");
        assert_eq!(of(Some(json!({"destructiveHint": true}))), "delete");
        assert_eq!(of(Some(json!({"title": "Search"}))), "write");
        assert_eq!(of(None), "write");
    }

    #[test]
    fn a_tool_that_calls_itself_read_only_is_labelled_but_still_ships_off() {
        let spec = spec_for(&proxy_tool(Some(json!({"readOnlyHint": true}))));
        assert_eq!(spec.category, "read");
        assert!(!spec.default_enabled);
        assert!(!spec_for(&proxy_tool(None)).default_enabled);
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
    fn cleartext_reaches_this_computer_and_nowhere_else() {
        let refused = endpoint(&integration(
            "int-1",
            json!({"url": "http://example.com/mcp"}),
        ))
        .expect_err("cleartext to another host");
        assert!(refused.has_code(INSECURE_ADDRESS_CODE), "{refused:?}");
        assert_eq!(refused.message, INSECURE_ADDRESS);

        for allowed in [
            "http://127.0.0.1:9000/mcp",
            "http://localhost:9000/mcp",
            "http://[::1]:9000/mcp",
            "https://example.com/mcp",
        ] {
            assert_eq!(
                endpoint(&integration("int-1", json!({ "url": allowed }))).expect(allowed),
                allowed
            );
        }
    }

    #[test]
    fn a_bare_token_signs_in_as_a_bearer_header_and_a_named_one_goes_raw() {
        assert_eq!(
            static_auth(&integration("int-1", json!({"url": "https://up/mcp"}))),
            UpstreamAuth::None
        );
        assert_eq!(
            static_auth(&integration(
                "int-1",
                json!({"url": "https://up/mcp", "token": "t0ken"})
            )),
            UpstreamAuth::Header {
                name: "Authorization".to_string(),
                value: "Bearer t0ken".to_string(),
            }
        );
        assert_eq!(
            static_auth(&integration(
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
