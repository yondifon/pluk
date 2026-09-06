//! One Pluk endpoint in front of several other MCP servers.
//!
//! An agent config that would list seven servers lists Pluk once instead. Each
//! configured server is connected on demand, its tools are discovered, and they
//! are exposed under a prefix taken from the name the user gave it, so two
//! servers offering `search` never collide.
//!
//! Discovered tools carry no category, so their default-on state comes from the
//! `readOnlyHint` annotation: only a tool that claims to be read-only ships on,
//! everything else waits for its toggle. Annotations are hints from someone
//! else's server, so anything unclear counts as "not read-only".
//!
//! Sessions live in [`crate::mcp_pool`] and are keyed by integration, not by
//! endpoint: discovery runs before the endpoint owner is known, and two
//! endpoints fronting the same integration can share one child process.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::future::join_all;
use rmcp::model::{CallToolResult, ContentBlock, JsonObject, Tool};
use serde_json::Value;

use pluk_store::{Integration, Store};

use crate::adapter::{Adapter, ApiRequest, ApiResponse, PolicyKind};
use crate::config_field::{ConfigField, FieldType, ShowIf};
use crate::error::AdapterError;
use crate::gate::{CallTarget, GateMeta, GateOpts, Outcome, RunOutcome, run_gated};
use crate::instructions::{InstructionParts, build_instructions};
use crate::mcp_auth::{self, Credentials};
use crate::mcp_client::McpUpstream;
use crate::mcp_pool::{UpstreamSession, mcp_sessions};
use crate::namespace::{NamespacedHost, slug};
use crate::tool_host::{ToolHandler, ToolHost, ToolRegistration};
use crate::tool_spec::ToolSpec;

/// Bounds the handshake and every later call on a session. Long enough for a
/// program that has to be downloaded before it can start, short enough that one
/// unreachable server cannot hold a request open indefinitely.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

const AGENT_HINT: &str = "Every tool here comes from another MCP server. A tool's name starts with the server it belongs to, so pick by prefix when two servers offer the same thing. Arguments and results are passed through unchanged.";

pub fn mcp_proxy_fields() -> Vec<ConfigField> {
    vec![
        ConfigField::new("servers", "Servers", FieldType::List)
            .required()
            .help("Each server's tools appear here with its name in front of them.")
            .entries(
                "Server",
                vec![
                    ConfigField::new("name", "Name", FieldType::Text)
                        .required()
                        .placeholder("GitHub")
                        .help("Goes in front of every tool name this server offers."),
                    ConfigField::new("kind", "Reach it through", FieldType::Select)
                        .options(&[
                            ("command", "A program on this computer"),
                            ("url", "A web address"),
                        ])
                        .default_value(&Value::String("command".into())),
                    ConfigField::new("command", "Program", FieldType::Text)
                        .required()
                        .placeholder("npx")
                        .show_if(ShowIf::eq_str("kind", "command")),
                    ConfigField::new("args", "Arguments", FieldType::Text)
                        .placeholder("-y @modelcontextprotocol/server-github")
                        .help("Separated by spaces.")
                        .show_if(ShowIf::eq_str("kind", "command")),
                    ConfigField::new("env", "Variables", FieldType::Password)
                        .placeholder("GITHUB_TOKEN=ghp_example")
                        .secret()
                        .help("NAME=value, separated by spaces.")
                        .show_if(ShowIf::eq_str("kind", "command")),
                    ConfigField::new("url", "Address", FieldType::Text)
                        .required()
                        .placeholder("https://example.com/mcp")
                        .show_if(ShowIf::eq_str("kind", "url")),
                    ConfigField::new("access", "Access", FieldType::Select)
                        .options(&[
                            ("headers", "A token I paste"),
                            ("signin", "Sign in to the server"),
                        ])
                        .default_value(&Value::String("headers".into()))
                        .show_if(ShowIf::eq_str("kind", "url")),
                    ConfigField::new("headers", "Headers", FieldType::Password)
                        .placeholder("Authorization: Bearer example")
                        .secret()
                        .help("Name: value, separated by commas.")
                        .show_if(ShowIf::eq_str("access", "headers")),
                    ConfigField::new("signin", "Account", FieldType::SignIn)
                        .help("Opens the server's sign-in page in your browser.")
                        .show_if(ShowIf::eq_str("access", "signin")),
                ],
            ),
    ]
}

/// Whether each server the person signs in to is connected right now, by name.
pub fn sign_in_states(conn: &Integration) -> HashMap<String, bool> {
    signing_in(conn)
        .map(|(name, _)| {
            let connected = mcp_auth::is_signed_in(conn, &name);
            (name, connected)
        })
        .collect()
}

/// The configured servers that are reached by signing in, with their address.
fn signing_in(conn: &Integration) -> impl Iterator<Item = (String, String)> + '_ {
    conn.config
        .get("servers")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|entry| entry_text(entry, "access").as_deref() == Some("signin"))
        .filter_map(|entry| Some((entry_text(entry, "name")?, entry_text(entry, "url")?)))
}

/// Start signing in to one server, returning the address to open in a browser.
pub async fn begin_sign_in(
    store: Arc<Store>,
    conn: &Integration,
    server: &str,
) -> Result<String, AdapterError> {
    let url = signing_in(conn)
        .find(|(name, _)| name == server)
        .map(|(_, url)| url)
        .ok_or_else(|| AdapterError::new("this server is not set up to be signed in to"))?;
    mcp_auth::start(store, &conn.id, server, &url).await
}

/// Forget one server's sign-in, and drop the session it was holding open.
pub fn sign_out(store: Arc<Store>, conn: &Integration, server: &str) -> Result<(), AdapterError> {
    mcp_auth::forget(store, &conn.id, server)?;
    mcp_sessions().evict_owner(&conn.id);
    Ok(())
}

/// What the browser shows once it comes back from the server's sign-in page.
fn callback_page(message: &str) -> ApiResponse {
    let escaped = message.replace('&', "&amp;").replace('<', "&lt;");
    ApiResponse {
        status: 200,
        content_type: Some("text/html; charset=utf-8".into()),
        body: format!(
            "<!doctype html><meta charset=\"utf-8\"><title>Pluk</title>\
             <body style=\"font:16px system-ui;margin:15vh auto;max-width:28rem;text-align:center\">\
             <p>{escaped}</p><p>You can close this tab and go back to Pluk.</p>"
        )
        .into_bytes(),
    }
}

/// One configured server: what the user called it, the prefix its tools carry,
/// and how to reach it — or why its settings cannot be reached at all.
struct Server {
    name: String,
    prefix: String,
    upstream: Result<McpUpstream, AdapterError>,
}

fn entry_text(entry: &Value, key: &str) -> Option<String> {
    entry
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Split `NAME=value` / `Name: value` settings written on one line.
fn pairs(raw: Option<String>, separator: char, assign: char) -> HashMap<String, String> {
    raw.unwrap_or_default()
        .split(separator)
        .filter_map(|item| item.split_once(assign))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .filter(|(name, _)| !name.is_empty())
        .collect()
}

fn upstream_from(
    entry: &Value,
    store: &Arc<Store>,
    conn: &Integration,
    name: &str,
) -> Result<McpUpstream, AdapterError> {
    if entry_text(entry, "kind").as_deref() == Some("url") {
        let url = entry_text(entry, "url")
            .ok_or_else(|| AdapterError::new("no address is set for it"))?;
        if entry_text(entry, "access").as_deref() == Some("signin") {
            return Ok(McpUpstream::SignedInHttp {
                url,
                credentials: Credentials::new(store.clone(), &conn.id, name),
            });
        }
        return Ok(McpUpstream::Http {
            url,
            headers: pairs(entry_text(entry, "headers"), ',', ':'),
        });
    }
    Ok(McpUpstream::Stdio {
        command: entry_text(entry, "command")
            .ok_or_else(|| AdapterError::new("no program is set for it"))?,
        args: entry_text(entry, "args")
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        env: pairs(entry_text(entry, "env"), ' ', '='),
    })
}

/// The configured servers, each with a prefix unique within the integration.
fn servers(store: &Arc<Store>, conn: &Integration) -> Vec<Server> {
    let entries = conn
        .config
        .get("servers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut used: HashMap<String, usize> = HashMap::new();
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let name = entry_text(entry, "name").unwrap_or_else(|| format!("Server {}", index + 1));
            let base = slug(&name);
            let seen = used.entry(base.clone()).or_insert(0);
            *seen += 1;
            let prefix = if *seen > 1 {
                format!("{base}_{seen}")
            } else {
                base
            };
            let upstream = upstream_from(entry, store, conn, &name);
            Server {
                name,
                prefix,
                upstream,
            }
        })
        .collect()
}

/// Connect to every configured server at once, so one slow server delays only
/// itself. A live session is served from the pool without reconnecting.
async fn connect_all(
    store: &Arc<Store>,
    conn: &Integration,
) -> Vec<(Server, Result<Arc<UpstreamSession>, AdapterError>)> {
    let servers = servers(store, conn);
    let sessions = join_all(servers.iter().map(|server| async {
        let upstream = server.upstream.as_ref().map_err(Clone::clone)?;
        mcp_sessions()
            .session(&conn.id, &conn.id, &server.name, upstream, CALL_TIMEOUT)
            .await
    }))
    .await;
    servers.into_iter().zip(sessions).collect()
}

/// Whether a discovered tool is on for a fresh integration. An explicit
/// read-only claim is the only thing that ships a tool on.
fn category_of(tool: &Tool) -> &'static str {
    match tool.annotations.as_ref().and_then(|a| a.read_only_hint) {
        Some(true) => "read",
        _ => "write",
    }
}

fn spec_for(prefix: &str, tool: &Tool) -> ToolSpec {
    ToolSpec::new(
        format!("{prefix}__{}", tool.name),
        tool.description.clone().unwrap_or_default(),
        category_of(tool),
    )
}

/// The agent-visible text of an upstream result. Text blocks pass through
/// verbatim; anything else keeps its wire form so nothing is silently dropped.
fn result_text(result: &CallToolResult) -> String {
    let mut blocks: Vec<String> = result
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text(text) => text.text.clone(),
            other => serde_json::to_string(other).unwrap_or_default(),
        })
        .collect();
    if let Some(structured) = &result.structured_content {
        blocks.push(serde_json::to_string_pretty(structured).unwrap_or_default());
    }
    blocks.retain(|block| !block.is_empty());
    blocks.join("\n")
}

fn outcome_of(result: CallToolResult) -> Outcome {
    let text = result_text(&result);
    if result.is_error == Some(true) {
        return Outcome::failed(text, "the server reported the call failed");
    }
    Outcome::Ran(RunOutcome {
        text: text.clone(),
        response_text: Some(text),
        ..Default::default()
    })
}

fn arguments_of(args: Value) -> Option<JsonObject> {
    match args {
        Value::Object(object) if !object.is_empty() => Some(object),
        _ => None,
    }
}

fn tool_handler(
    store: Arc<pluk_store::Store>,
    conn: &Integration,
    server_name: String,
    upstream: McpUpstream,
    tool_name: String,
    category: &'static str,
) -> ToolHandler {
    let target = CallTarget::from(conn);
    let integration_id = conn.id.clone();

    Arc::new(move |args: Value| {
        let store = store.clone();
        let target = target.clone();
        let integration_id = integration_id.clone();
        let server_name = server_name.clone();
        let upstream = upstream.clone();
        let tool_name = tool_name.clone();
        let meta = GateMeta::new(category, &tool_name, format!("{server_name}: {tool_name}"));
        Box::pin(async move {
            run_gated(
                &store,
                &target,
                meta,
                |_| async move {
                    let session = mcp_sessions()
                        .session(
                            &integration_id,
                            &integration_id,
                            &server_name,
                            &upstream,
                            CALL_TIMEOUT,
                        )
                        .await?;
                    Ok(outcome_of(
                        session.call_tool(&tool_name, arguments_of(args)).await?,
                    ))
                },
                GateOpts::default(),
            )
            .await
        })
    })
}

pub struct McpProxyAdapter {
    store: Arc<pluk_store::Store>,
}

impl McpProxyAdapter {
    pub fn new(store: Arc<pluk_store::Store>) -> Arc<Self> {
        Arc::new(Self { store })
    }
}

#[async_trait]
impl Adapter for McpProxyAdapter {
    fn id(&self) -> &str {
        "mcp-proxy"
    }
    fn label(&self) -> &str {
        "MCP Servers"
    }
    fn category(&self) -> &str {
        "tools"
    }
    fn policy_kind(&self) -> PolicyKind {
        PolicyKind::Action
    }
    fn agent_hint(&self) -> &str {
        AGENT_HINT
    }
    /// Nothing is known before connecting: the real list is per integration.
    fn tool_specs(&self) -> &[ToolSpec] {
        &[]
    }
    async fn tool_specs_for(&self, conn: &Integration) -> Result<Vec<ToolSpec>, AdapterError> {
        // A server that cannot be reached contributes no tools; the rest still
        // list theirs, so one failure never empties the endpoint.
        let mut specs = Vec::new();
        for (server, session) in connect_all(&self.store, conn).await {
            let Ok(session) = session else { continue };
            specs.extend(
                session
                    .tools()
                    .iter()
                    .map(|tool| spec_for(&server.prefix, tool)),
            );
        }
        Ok(specs)
    }
    fn config_fields(&self) -> &[ConfigField] {
        static FIELDS: std::sync::OnceLock<Vec<ConfigField>> = std::sync::OnceLock::new();
        FIELDS.get_or_init(mcp_proxy_fields)
    }
    async fn test_connection(&self, conn: &Integration) -> Result<(), AdapterError> {
        let results = connect_all(&self.store, conn).await;
        if results.is_empty() {
            return Err(AdapterError::new("Add a server to connect to."));
        }
        let mut report = Vec::new();
        let mut failed = false;
        for (server, session) in results {
            report.push(match session {
                Ok(session) => format!(
                    "{} answered with {} tools.",
                    server.name,
                    session.tools().len()
                ),
                Err(error) if error.has_code(mcp_auth::NEEDS_SIGN_IN_CODE) => {
                    failed = true;
                    format!("{} is waiting for you to sign in.", server.name)
                }
                Err(error) => {
                    failed = true;
                    format!("{} did not answer: {}", server.name, error.message)
                }
            });
        }
        if failed {
            return Err(AdapterError::new(report.join(" ")));
        }
        Ok(())
    }
    /// The browser comes back here after the person has answered the server.
    async fn handle_global_api(&self, request: ApiRequest, path: &str) -> Option<ApiResponse> {
        if path != mcp_auth::CALLBACK_PATH {
            return None;
        }
        Some(match mcp_auth::complete(&request.url).await {
            Ok(done) => {
                mcp_sessions().evict_owner(&done.integration_id);
                callback_page(&format!("You're signed in to {}.", done.server))
            }
            Err(error) => callback_page(&error.message),
        })
    }
    fn instructions(&self, conn: &Integration) -> String {
        let servers = servers(&self.store, conn);
        let prefixes: Vec<String> = servers
            .iter()
            .map(|server| format!("{}__ for {}", server.prefix, server.name))
            .collect();
        let start = match prefixes.is_empty() {
            true => "No servers are set up yet, so there are no tools to call.".to_string(),
            false => format!(
                "Tool names start with the server they came from: {}.",
                prefixes.join(", ")
            ),
        };
        build_instructions(
            &conn.name,
            conn.environment,
            InstructionParts {
                kind: "MCP".into(),
                access: "The tools here belong to other MCP servers. Each is individually permitted, and every call is recorded in the activity log.".into(),
                policy: None,
                start: Some(start),
                hint: Some(AGENT_HINT.into()),
            },
        )
    }
    fn register(
        &self,
        _host: &mut dyn ToolHost,
        _conn: &Integration,
        _owner_id: &str,
    ) -> Result<(), AdapterError> {
        Ok(())
    }
    async fn register_surface(
        &self,
        host: &mut dyn ToolHost,
        conn: &Integration,
        _owner_id: &str,
    ) -> Result<(), AdapterError> {
        for (server, session) in connect_all(&self.store, conn).await {
            let (Ok(upstream), Ok(session)) = (&server.upstream, &session) else {
                continue;
            };
            let mut namespaced = NamespacedHost::new(host, server.prefix.clone());
            for tool in session.tools() {
                let handler = tool_handler(
                    self.store.clone(),
                    conn,
                    server.name.clone(),
                    upstream.clone(),
                    tool.name.to_string(),
                    category_of(tool),
                );
                namespaced.register_tool(
                    ToolRegistration {
                        name: tool.name.to_string(),
                        description: tool.description.clone().unwrap_or_default().to_string(),
                        input_schema: (*tool.input_schema).clone(),
                        annotations: annotations_of(tool),
                    },
                    handler,
                );
            }
        }
        Ok(())
    }
}

/// The upstream's own annotation hints, in the camelCase shape the surface
/// serves them back in.
fn annotations_of(tool: &Tool) -> serde_json::Map<String, Value> {
    tool.annotations
        .as_ref()
        .and_then(|annotations| serde_json::to_value(annotations).ok())
        .and_then(|value| match value {
            Value::Object(object) => Some(object),
            _ => None,
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The proxy reads sign-ins out of the store, so its tests need a real one.
    fn store() -> (tempfile::TempDir, Arc<Store>) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Arc::new(Store::open(&dir.path().join("pluk.db")).expect("open"));
        (dir, store)
    }

    fn conn(config: Value) -> Integration {
        Integration {
            id: "proxy".into(),
            name: "My servers".into(),
            r#type: "mcp-proxy".into(),
            config: config.as_object().cloned().expect("config"),
            environment: None,
            read_only: 0,
            query_policy: None,
            token: "t".into(),
            created_at: String::new(),
            via_group: None,
        }
    }

    fn tool(name: &str, read_only: Option<bool>) -> Tool {
        let mut tool = Tool::new(name.to_string(), "…", Arc::new(serde_json::Map::new()));
        tool.annotations = read_only.map(|read_only| {
            let mut annotations = rmcp::model::ToolAnnotations::default();
            annotations.read_only_hint = Some(read_only);
            annotations
        });
        tool
    }

    #[test]
    fn same_named_servers_get_distinct_prefixes() {
        let (_dir, store) = store();
        let servers = servers(
            &store,
            &conn(json!({ "servers": [
                { "name": "Docs", "command": "docs-mcp" },
                { "name": "docs", "command": "other-mcp" },
            ]})),
        );
        let prefixes: Vec<&str> = servers.iter().map(|s| s.prefix.as_str()).collect();
        assert_eq!(prefixes, ["docs", "docs_2"]);
    }

    #[test]
    fn a_server_missing_its_target_carries_the_reason() {
        let (_dir, store) = store();
        let servers = servers(
            &store,
            &conn(json!({ "servers": [
                { "name": "Docs", "kind": "url" },
                { "name": "Local", "kind": "command", "command": "docs-mcp", "args": "--stdio -v",
                  "env": "TOKEN=abc OTHER=1" },
            ]})),
        );
        assert_eq!(
            servers[0].upstream.as_ref().unwrap_err().message,
            "no address is set for it"
        );
        let McpUpstream::Stdio { args, env, .. } = servers[1].upstream.as_ref().expect("stdio")
        else {
            panic!("expected a local program");
        };
        assert_eq!(args, &["--stdio", "-v"]);
        assert_eq!(env["TOKEN"], "abc");
        assert_eq!(env["OTHER"], "1");
    }

    #[test]
    fn headers_are_read_as_name_value_pairs() {
        let (_dir, store) = store();
        let servers = servers(
            &store,
            &conn(json!({ "servers": [
                { "name": "Docs", "kind": "url", "url": "https://example.com/mcp",
                  "headers": "Authorization: Bearer abc, X-Team: eng" },
            ]})),
        );
        let McpUpstream::Http { headers, .. } = servers[0].upstream.as_ref().expect("http") else {
            panic!("expected a web address");
        };
        assert_eq!(headers["Authorization"], "Bearer abc");
        assert_eq!(headers["X-Team"], "eng");
    }

    #[test]
    fn pasted_headers_and_signing_in_pick_different_ways_in() {
        let (_dir, store) = store();
        let servers = servers(&store, &conn(json!({ "servers": [
            { "name": "Docs", "kind": "url", "url": "https://example.com/mcp",
              "access": "headers", "headers": "Authorization: Bearer abc" },
            { "name": "Mail", "kind": "url", "url": "https://mail.example.com/mcp",
              "access": "signin" },
        ]})));
        assert!(matches!(
            servers[0].upstream.as_ref().expect("http"),
            McpUpstream::Http { .. }
        ));
        assert!(matches!(
            servers[1].upstream.as_ref().expect("signed in"),
            McpUpstream::SignedInHttp { .. }
        ));
    }

    #[test]
    fn only_a_read_only_claim_ships_a_tool_on() {
        assert!(spec_for("docs", &tool("search", Some(true))).default_enabled);
        for unclear in [None, Some(false)] {
            let spec = spec_for("docs", &tool("deploy", unclear));
            assert_eq!(spec.name, "docs__deploy");
            assert!(!spec.default_enabled, "{unclear:?} must not ship on");
        }
    }

    #[test]
    fn an_error_result_is_logged_as_a_failure_and_still_returns_its_text() {
        let mut result = CallToolResult::success(vec![ContentBlock::text("boom")]);
        result.is_error = Some(true);
        let Outcome::Ran(ran) = outcome_of(result) else {
            panic!("expected a run");
        };
        assert!(ran.is_error);
        assert_eq!(ran.text, "boom");
    }
}
