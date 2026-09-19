//! Proxying a third-party MCP server: Pluk connects to it as a client,
//! discovers the tools it offers, and re-exposes the ones the user approved.
//!
//! [`client`] owns that upstream connection and nothing else — it reads no
//! store and decides no policy. [`catalog`] is the snapshot in between, and
//! this module is the adapter that turns it into tools.
//!
//! Three things have to hold before an agent can reach an upstream tool: the
//! owner approved that exact definition, upstream still lists it, and the
//! tool's toggle is on. The first two are checked here because the policy gate
//! only knows the third. Discovery alone exposes nothing.

pub mod api;
pub mod catalog;
pub mod client;
pub mod oauth;

use std::borrow::Cow;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde_json::{Map, Value, json};

use pluk_policy::tool_gate;
use pluk_store::{Integration, ProxyTool, Store, ToolState, Verdict};

use crate::adapter::{Adapter, ApiRequest, ApiResponse, PolicyKind};
use crate::config_field::{ConfigField, FieldType};
use crate::error::AdapterError;
use crate::gate::{CallTarget, GateMeta, GateOpts, Outcome, ToolResult, run_gated};
use crate::instructions::{InstructionParts, build_instructions};
use crate::tool_host::{ToolHandler, ToolHost, ToolRegistration};
use crate::tool_spec::ToolSpec;

use client::UpstreamAuth;

pub const ADAPTER_ID: &str = "mcp";

/// The tool upstream now describes is not the one that was approved.
pub const TOOL_CHANGED_CODE: &str = "MCP_PROXY_TOOL_CHANGED";
/// The stored token no longer signs in to the upstream server.
pub const TOKEN_REJECTED_CODE: &str = "MCP_PROXY_TOKEN_REJECTED";

const TOOL_CHANGED: &str = "This tool changed. Approve it again in Pluk.";
const TOKEN_REJECTED: &str = "Pluk could not sign in to this MCP server. Check the token in Pluk.";
const PERMISSION_DENIED: &str =
    "This MCP server refused the request. The account signed in to Pluk may not have permission.";

const AGENT_HINT: &str = "Use this to reach the tools of another MCP server the owner connected in Pluk. Each tool is that server's own: call it by name with the arguments its schema describes.";

fn mcp_fields() -> Vec<ConfigField> {
    vec![
        ConfigField::new("url", "Server URL", FieldType::Text)
            .group("Connection")
            .required()
            .placeholder("https://example.com/mcp"),
        ConfigField::new("token", "Token", FieldType::Password)
            .group("Auth")
            .secret()
            .placeholder("leave empty if the server does not need one"),
        ConfigField::new("header_name", "Header name", FieldType::Text)
            .group("Auth")
            .default_value(&json!(client::DEFAULT_AUTH_HEADER))
            .help("Authorization sends the token as a bearer token. Any other header sends it as written."),
        ConfigField::new("client_id", "Client ID", FieldType::Text)
            .group("Auth")
            .placeholder("leave empty if the server hands one out")
            .help("Some servers ask you to register Pluk with them first, then give you this."),
        ConfigField::new("client_secret", "Client secret", FieldType::Password)
            .group("Auth")
            .secret()
            .placeholder("leave empty if the server gave none"),
    ]
}

pub struct McpProxyAdapter {
    store: Arc<Store>,
}

impl McpProxyAdapter {
    pub fn new(store: Arc<Store>) -> Arc<Self> {
        Arc::new(McpProxyAdapter { store })
    }

    /// The tools an agent can actually reach right now: approved, still
    /// offered, and toggled on.
    fn live_tools(&self, conn: &Integration) -> Vec<ToolSpec> {
        let gate = tool_gate(conn.query_policy.as_deref());
        catalog::snapshot(&self.store, &conn.id)
            .unwrap_or_default()
            .iter()
            .filter(|tool| tool.state() == ToolState::Approved)
            .map(catalog::spec_for)
            .filter(|spec| gate.enabled(&spec.name, spec.default_enabled))
            .collect()
    }
}

#[async_trait]
impl Adapter for McpProxyAdapter {
    fn id(&self) -> &str {
        ADAPTER_ID
    }

    fn label(&self) -> &str {
        "MCP server"
    }

    fn category(&self) -> &str {
        "mcp"
    }

    fn policy_kind(&self) -> PolicyKind {
        PolicyKind::Action
    }

    fn agent_hint(&self) -> &str {
        AGENT_HINT
    }

    /// Empty by design: an MCP server's tools are its own, so the catalog only
    /// exists per integration.
    fn tool_specs(&self) -> &[ToolSpec] {
        &[]
    }

    fn tool_specs_for(&self, conn: &Integration) -> Cow<'_, [ToolSpec]> {
        Cow::Owned(
            catalog::snapshot(&self.store, &conn.id)
                .unwrap_or_default()
                .iter()
                .filter(|tool| tool.present)
                .map(catalog::spec_for)
                .collect(),
        )
    }

    fn config_fields(&self) -> &[ConfigField] {
        static FIELDS: OnceLock<Vec<ConfigField>> = OnceLock::new();
        FIELDS.get_or_init(mcp_fields)
    }

    async fn test_connection(&self, conn: &Integration) -> Result<(), AdapterError> {
        catalog::discover(&self.store, conn).await.map(|_| ())
    }

    async fn handle_api(
        &self,
        conn: &Integration,
        request: ApiRequest,
        subpath: &str,
    ) -> Option<ApiResponse> {
        api::handle_proxy_api(&self.store, conn, request, subpath).await
    }

    /// The page the user's browser lands on after they approve a sign-in.
    async fn handle_global_api(&self, request: ApiRequest, path: &str) -> Option<ApiResponse> {
        api::handle_callback(&self.store, request, path).await
    }

    fn instructions(&self, conn: &Integration) -> String {
        let live = self.live_tools(conn);
        let policy = if live.is_empty() {
            "No tools from this server are turned on.".to_string()
        } else {
            let names: Vec<&str> = live.iter().map(|spec| spec.name.as_str()).collect();
            format!("Enabled tools: {}.", names.join(", "))
        };
        build_instructions(
            &conn.name,
            conn.environment,
            InstructionParts {
                kind: "MCP server".to_string(),
                access: "These tools come from an MCP server the owner connected in Pluk."
                    .to_string(),
                policy: Some(policy),
                start: None,
                hint: Some(
                    "Only tools the owner approved are listed. A tool you expected and cannot find is one they have not turned on."
                        .to_string(),
                ),
            },
        )
    }

    fn register(
        &self,
        host: &mut dyn ToolHost,
        conn: &Integration,
        _owner_id: &str,
    ) -> Result<(), AdapterError> {
        for tool in catalog::snapshot(&self.store, &conn.id)? {
            if tool.state() != ToolState::Approved {
                continue;
            }
            let handler = handler_for(self.store.clone(), conn, &tool);
            host.register_tool(registration_for(&tool), handler);
        }
        Ok(())
    }
}

/// The upstream definition, passed through as it was approved.
fn registration_for(tool: &ProxyTool) -> ToolRegistration {
    ToolRegistration {
        name: tool.name.clone(),
        description: tool.description.clone(),
        input_schema: object_of(&tool.schema_json),
        annotations: tool
            .annotations_json
            .as_deref()
            .map(object_of)
            .unwrap_or_default(),
    }
}

fn object_of(raw: &str) -> Map<String, Value> {
    serde_json::from_str(raw).unwrap_or_default()
}

fn handler_for(store: Arc<Store>, conn: &Integration, tool: &ProxyTool) -> ToolHandler {
    let conn = conn.clone();
    let name = tool.name.clone();
    let category = catalog::category(tool).to_string();

    Arc::new(move |args: Value| {
        let store = store.clone();
        let conn = conn.clone();
        let name = name.clone();
        let category = category.clone();

        Box::pin(async move {
            let target = CallTarget::from(&conn);
            let meta = GateMeta::new(category, name.clone(), name.clone());
            let upstream_store = store.clone();
            run_gated(
                &store,
                &target,
                meta,
                move |_log_id| async move { proxy_call(&upstream_store, &conn, &name, args).await },
                GateOpts::default().format_error(agent_text),
            )
            .await
        })
    })
}

/// One proxied call. Arguments and results reach the upstream server and the
/// agent, never the log.
async fn proxy_call(
    store: &Store,
    conn: &Integration,
    name: &str,
    args: Value,
) -> Result<Outcome, AdapterError> {
    let client = catalog::client_for(store, conn).await?;
    if client.take_tools_changed() {
        catalog::discover(store, conn).await?;
    }
    if !catalog::is_approved(store, &conn.id, name) {
        return Err(AdapterError::new(TOOL_CHANGED).with_code(TOOL_CHANGED_CODE));
    }
    let arguments = match args {
        Value::Object(arguments) => Some(arguments),
        _ => None,
    };
    match client.call_tool(name, arguments.clone()).await {
        Ok(result) => Ok(outcome_of(result)),
        // Upstream took the credentials and refused anyway. Renewing them
        // answers a question nobody asked.
        Err(error) if error.has_code(client::PERMISSION_DENIED_CODE) => {
            Err(AdapterError::new(PERMISSION_DENIED).with_code(client::PERMISSION_DENIED_CODE))
        }
        Err(error) if error.has_code(client::AUTH_REJECTED_CODE) => {
            after_refusal(store, conn, name, arguments, error).await
        }
        Err(error) => Err(error),
    }
}

/// Upstream would not take the credentials Pluk presented.
///
/// A sign-in Pluk owns is renewed once and the call repeated, because an
/// access token can run out mid-call. A token the user typed in is theirs to
/// fix, and an open server refusing us is its own failure to report.
async fn after_refusal(
    store: &Store,
    conn: &Integration,
    name: &str,
    arguments: Option<Map<String, Value>>,
    error: AdapterError,
) -> Result<Outcome, AdapterError> {
    if !oauth::renew(store, conn).await? {
        return match catalog::static_auth(conn) {
            UpstreamAuth::None => Err(error),
            _ => Err(AdapterError::new(TOKEN_REJECTED).with_code(TOKEN_REJECTED_CODE)),
        };
    }
    let client = catalog::client_for(store, conn).await?;
    match client.call_tool(name, arguments).await {
        Ok(result) => Ok(outcome_of(result)),
        // A token minted seconds ago and still refused is a grant the server
        // no longer honors, whatever it says.
        Err(again) if again.has_code(client::AUTH_REJECTED_CODE) => {
            Err(oauth::require_sign_in(store, &conn.id))
        }
        Err(again) => Err(again),
    }
}

fn outcome_of(result: ToolResult) -> Outcome {
    let text = result
        .content
        .iter()
        .map(|part| part.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if result.is_error {
        Outcome::failed(text, "the tool reported a failure")
    } else {
        Outcome::ran(text)
    }
}

/// Pluk's own refusals are finished sentences; an upstream failure keeps the
/// runner's prefix so the agent can tell the two apart.
fn agent_text(error: &AdapterError, verdict: Verdict) -> String {
    if error.has_code(TOOL_CHANGED_CODE)
        || error.has_code(TOKEN_REJECTED_CODE)
        || error.has_code(client::PERMISSION_DENIED_CODE)
        || error.has_code(oauth::RECONNECT_NEEDED_CODE)
    {
        return error.message.clone();
    }
    let label = if verdict == Verdict::Cancelled {
        "Cancelled"
    } else {
        "Error"
    };
    format!("{label}: {}", error.message)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use axum::Router;
    use axum::routing::any;
    use rmcp::ErrorData as McpError;
    use rmcp::model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, JsonObject,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
    };
    use rmcp::service::RequestContext;
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
    use rmcp::{RoleServer, ServerHandler};
    use tower::ServiceExt as _;

    use pluk_store::{Environment, LogEntry};

    use super::*;

    const TOKEN: &str = "super-secret-upstream-token";

    pub(super) fn integration(id: &str, config: Value) -> Integration {
        let config = match config {
            Value::Object(config) => config,
            _ => Map::new(),
        };
        Integration {
            id: id.to_string(),
            name: "Docs server".to_string(),
            r#type: ADAPTER_ID.to_string(),
            config,
            environment: Some(Environment::Development),
            read_only: 0,
            query_policy: None,
            token: "pluk-token".to_string(),
            created_at: String::new(),
            via_group: None,
        }
    }

    /// A host that keeps the handlers so a test can call one directly.
    #[derive(Default)]
    pub(super) struct RecordingHost {
        pub(super) tools: Vec<ToolRegistration>,
        pub(super) handlers: HashMap<String, ToolHandler>,
    }

    impl RecordingHost {
        fn names(&self) -> Vec<String> {
            self.tools.iter().map(|tool| tool.name.clone()).collect()
        }
    }

    impl ToolHost for RecordingHost {
        fn register_tool(&mut self, registration: ToolRegistration, handler: ToolHandler) {
            self.handlers.insert(registration.name.clone(), handler);
            self.tools.push(registration);
        }
        fn register_prompt(
            &mut self,
            _name: &str,
            _description: &str,
            _args_schema: Option<Map<String, Value>>,
            _handler: crate::tool_host::PromptHandler,
        ) {
        }
        fn register_resource(
            &mut self,
            _name: &str,
            _uri: &str,
            _mime_type: &str,
            _description: Option<&str>,
            _handler: crate::tool_host::ResourceHandler,
        ) {
        }
    }

    /// An upstream server whose tool list a test can rewrite between calls.
    #[derive(Clone)]
    struct TestServer {
        tools: Arc<Mutex<Vec<Tool>>>,
    }

    fn schema() -> JsonObject {
        match json!({"type": "object", "properties": {"q": {"type": "string"}}}) {
            Value::Object(map) => map,
            _ => unreachable!(),
        }
    }

    fn tool(name: &str, description: &str) -> Tool {
        Tool::new(
            name.to_string(),
            description.to_string(),
            Arc::new(schema()),
        )
    }

    impl ServerHandler for TestServer {
        fn get_info(&self) -> ServerInfo {
            let mut info = ServerInfo::default();
            info.capabilities = ServerCapabilities::builder().enable_tools().build();
            info
        }

        async fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, McpError> {
            let tools = self.tools.lock().expect("tools").clone();
            Ok(ListToolsResult::with_all_items(tools))
        }

        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, McpError> {
            let query = request
                .arguments
                .as_ref()
                .and_then(|arguments| arguments.get("q"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            Ok(CallToolResponse::Complete(CallToolResult::success(vec![
                ContentBlock::text(format!("found {query}")),
            ])))
        }
    }

    async fn serve(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        format!("http://{addr}/mcp")
    }

    /// The endpoint, plus the handle a test rewrites the tool list through.
    async fn upstream() -> (String, Arc<Mutex<Vec<Tool>>>) {
        let tools = Arc::new(Mutex::new(vec![tool("search", "Search the docs")]));
        let handler = TestServer {
            tools: tools.clone(),
        };
        let service = StreamableHttpService::new(
            move || Ok(handler.clone()),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default(),
        );
        let router = Router::new().route(
            "/mcp",
            any(move |request: axum::extract::Request| {
                let service = service.clone();
                async move {
                    match service.oneshot(request).await {
                        Ok(response) => response.map(axum::body::Body::new),
                        Err(never) => match never {},
                    }
                }
            }),
        );
        (serve(router).await, tools)
    }

    async fn rejecting_upstream() -> String {
        let router = Router::new().route(
            "/mcp",
            any(|| async {
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    [(
                        axum::http::header::WWW_AUTHENTICATE,
                        "Bearer realm=\"upstream\"",
                    )],
                    "unauthorized",
                )
            }),
        );
        serve(router).await
    }

    fn store() -> (tempfile::TempDir, Arc<Store>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("pluk.db")).expect("open");
        (dir, Arc::new(store))
    }

    /// The tool toggled on, so only approval decides what registers.
    fn enabling(name: &str) -> String {
        json!({ "tools": { name: { "enabled": true } } }).to_string()
    }

    fn registered(adapter: &McpProxyAdapter, conn: &Integration) -> RecordingHost {
        let mut host = RecordingHost::default();
        crate::tool_host::register_gated(adapter, &mut host, conn, "owner").expect("register");
        host
    }

    fn states(store: &Store, id: &str) -> Vec<(String, ToolState)> {
        catalog::snapshot(store, id)
            .expect("snapshot")
            .iter()
            .map(|tool| (tool.name.clone(), tool.state()))
            .collect()
    }

    fn logs(store: &Store) -> Vec<LogEntry> {
        store.log_rows_after(0).expect("logs")
    }

    #[tokio::test]
    async fn discovery_exposes_nothing_until_the_owner_approves() {
        let (endpoint, _tools) = upstream().await;
        let (_dir, store) = store();
        let adapter = McpProxyAdapter::new(store.clone());
        let mut conn = integration("approve-first", json!({ "url": endpoint }));
        conn.query_policy = Some(enabling("search"));

        adapter.test_connection(&conn).await.expect("discover");
        assert_eq!(
            states(&store, &conn.id),
            [("search".to_string(), ToolState::New)]
        );
        assert!(registered(&adapter, &conn).names().is_empty());

        store
            .approve_proxy_tools(&conn.id, &["search".to_string()])
            .expect("approve");
        let host = registered(&adapter, &conn);
        assert_eq!(host.names(), ["search".to_string()]);
        assert_eq!(host.tools[0].description, "Search the docs");
        assert_eq!(
            Value::Object(host.tools[0].input_schema.clone()),
            json!({"type": "object", "properties": {"q": {"type": "string"}}})
        );

        client::invalidate(&conn.id);
    }

    #[tokio::test]
    async fn a_disabled_tool_stays_off_however_it_was_approved() {
        let (endpoint, _tools) = upstream().await;
        let (_dir, store) = store();
        let adapter = McpProxyAdapter::new(store.clone());
        let conn = integration("policy-off", json!({ "url": endpoint }));

        adapter.test_connection(&conn).await.expect("discover");
        store
            .approve_proxy_tools(&conn.id, &["search".to_string()])
            .expect("approve");

        // Nothing a proxied server offers is on until the owner turns it on,
        // so an approved tool with no toggle stays unreachable.
        assert!(registered(&adapter, &conn).names().is_empty());

        client::invalidate(&conn.id);
    }

    #[tokio::test]
    async fn a_proxied_call_reaches_upstream_and_logs_only_the_tool_name() {
        let (endpoint, _tools) = upstream().await;
        let (_dir, store) = store();
        let adapter = McpProxyAdapter::new(store.clone());
        let mut conn = integration("proxied-call", json!({ "url": endpoint }));
        conn.query_policy = Some(enabling("search"));

        adapter.test_connection(&conn).await.expect("discover");
        store
            .approve_proxy_tools(&conn.id, &["search".to_string()])
            .expect("approve");
        let host = registered(&adapter, &conn);

        let result = host.handlers["search"](json!({ "q": "onboarding" })).await;
        assert!(!result.is_error, "{result:?}");
        assert_eq!(result.text(), "found onboarding");

        let entries = logs(&store);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].sql, "search");
        assert_eq!(entries[0].source.as_deref(), Some("search"));
        assert_eq!(entries[0].verdict, "allowed");
        assert!(!entries[0].sql.contains("onboarding"));
        assert!(
            !format!("{entries:?}").contains("onboarding"),
            "no argument value reaches the log"
        );

        client::invalidate(&conn.id);
    }

    #[tokio::test]
    async fn an_edited_tool_stops_being_exposed_and_refuses_the_call() {
        let (endpoint, tools) = upstream().await;
        let (_dir, store) = store();
        let adapter = McpProxyAdapter::new(store.clone());
        let mut conn = integration("edited-upstream", json!({ "url": endpoint }));
        conn.query_policy = Some(enabling("search"));

        adapter.test_connection(&conn).await.expect("discover");
        store
            .approve_proxy_tools(&conn.id, &["search".to_string()])
            .expect("approve");
        let host = registered(&adapter, &conn);

        *tools.lock().expect("tools") = vec![tool("search", "Search everything, including drafts")];
        adapter.test_connection(&conn).await.expect("rediscover");

        assert_eq!(
            states(&store, &conn.id),
            [("search".to_string(), ToolState::Changed)]
        );
        assert!(registered(&adapter, &conn).names().is_empty());

        let result = host.handlers["search"](json!({ "q": "onboarding" })).await;
        assert!(result.is_error);
        assert_eq!(result.text(), TOOL_CHANGED);

        client::invalidate(&conn.id);
    }

    #[tokio::test]
    async fn a_new_tool_arrives_unapproved_and_a_vanished_one_stops_being_exposed() {
        let (endpoint, tools) = upstream().await;
        let (_dir, store) = store();
        let adapter = McpProxyAdapter::new(store.clone());
        let mut conn = integration("moving-catalog", json!({ "url": endpoint }));
        conn.query_policy = Some(
            json!({
                "tools": { "search": { "enabled": true }, "publish": { "enabled": true } }
            })
            .to_string(),
        );

        adapter.test_connection(&conn).await.expect("discover");
        store
            .approve_proxy_tools(&conn.id, &["search".to_string()])
            .expect("approve");
        assert_eq!(registered(&adapter, &conn).names(), ["search".to_string()]);

        *tools.lock().expect("tools") = vec![tool("publish", "Publish a page")];
        adapter.test_connection(&conn).await.expect("rediscover");

        assert_eq!(
            states(&store, &conn.id),
            [
                ("publish".to_string(), ToolState::New),
                ("search".to_string(), ToolState::Missing),
            ]
        );
        assert!(registered(&adapter, &conn).names().is_empty());
        // A missing tool keeps no toggle either: the catalog is what upstream
        // still offers.
        assert_eq!(
            adapter
                .tool_specs_for(&conn)
                .iter()
                .map(|spec| spec.name.clone())
                .collect::<Vec<_>>(),
            ["publish".to_string()]
        );

        client::invalidate(&conn.id);
    }

    #[tokio::test]
    async fn a_refused_token_is_named_without_being_shown() {
        let (endpoint, _tools) = upstream().await;
        let (_dir, store) = store();
        let adapter = McpProxyAdapter::new(store.clone());
        let mut conn = integration("token-refused", json!({ "url": endpoint, "token": TOKEN }));
        conn.query_policy = Some(enabling("search"));

        adapter.test_connection(&conn).await.expect("discover");
        store
            .approve_proxy_tools(&conn.id, &["search".to_string()])
            .expect("approve");

        // The same integration, now pointed at a server that refuses it: the
        // pooled session is dropped because the address moved.
        let mut refused = conn.clone();
        refused
            .config
            .insert("url".to_string(), Value::String(rejecting_upstream().await));
        let refused_host = {
            let mut host = RecordingHost::default();
            adapter
                .register(&mut host, &refused, "owner")
                .expect("register");
            host
        };
        let result = refused_host.handlers["search"](json!({ "q": "onboarding" })).await;
        assert!(result.is_error);
        assert_eq!(result.text(), TOKEN_REJECTED);

        let seen = format!("{:?}{:?}", logs(&store), result);
        assert!(!seen.contains(TOKEN), "the token never reaches a log row");

        let listed = api::handle_proxy_api(
            &store,
            &refused,
            ApiRequest {
                method: "GET".to_string(),
                url: format!("/api/integrations/{}/proxy/tools", refused.id),
                body: None,
            },
            "/proxy/tools",
        )
        .await
        .expect("tools route");
        assert!(!String::from_utf8_lossy(&listed.body).contains(TOKEN));

        client::invalidate(&conn.id);
    }

    #[tokio::test]
    async fn the_rest_surface_lists_approves_and_names_the_sign_in() {
        let (endpoint, tools) = upstream().await;
        let (_dir, store) = store();
        let adapter = McpProxyAdapter::new(store.clone());
        let conn = integration("rest-surface", json!({ "url": endpoint }));

        let refreshed = call_api(&adapter, &conn, "POST", "/proxy/refresh", None).await;
        assert_eq!(refreshed["tools"][0]["name"], json!("search"));
        assert_eq!(refreshed["tools"][0]["label"], json!("Search"));
        assert_eq!(refreshed["tools"][0]["state"], json!("new"));
        assert_eq!(refreshed["tools"][0]["category"], json!("write"));

        let approved = call_api(
            &adapter,
            &conn,
            "POST",
            "/proxy/approve",
            Some(json!({ "names": ["search"] }).to_string()),
        )
        .await;
        assert_eq!(approved["tools"][0]["state"], json!("approved"));

        *tools.lock().expect("tools") = vec![tool("search", "Search everything")];
        let rediscovered = call_api(&adapter, &conn, "POST", "/proxy/refresh", None).await;
        assert_eq!(rediscovered["tools"][0]["state"], json!("changed"));

        let listed = call_api(&adapter, &conn, "GET", "/proxy/tools", None).await;
        assert_eq!(listed["tools"][0]["state"], json!("changed"));

        let auth = call_api(&adapter, &conn, "GET", "/proxy/auth", None).await;
        assert_eq!(
            auth["auth"],
            json!({ "kind": "none", "status": "not_connected" })
        );

        client::invalidate(&conn.id);
    }

    async fn call_api(
        adapter: &McpProxyAdapter,
        conn: &Integration,
        method: &str,
        subpath: &str,
        body: Option<String>,
    ) -> Value {
        let response = adapter
            .handle_api(
                conn,
                ApiRequest {
                    method: method.to_string(),
                    url: format!("/api/integrations/{}{subpath}", conn.id),
                    body,
                },
                subpath,
            )
            .await
            .expect("route");
        assert_eq!(
            response.status,
            200,
            "{}",
            String::from_utf8_lossy(&response.body)
        );
        serde_json::from_slice(&response.body).expect("json")
    }

    #[tokio::test]
    async fn instructions_name_the_tools_an_agent_can_reach() {
        let (endpoint, _tools) = upstream().await;
        let (_dir, store) = store();
        let adapter = McpProxyAdapter::new(store.clone());
        let mut conn = integration("instructions", json!({ "url": endpoint }));
        conn.query_policy = Some(enabling("search"));

        adapter.test_connection(&conn).await.expect("discover");
        let none = adapter.instructions(&conn);
        assert!(
            none.contains("Current policy: No tools from this server are turned on."),
            "{none}"
        );

        store
            .approve_proxy_tools(&conn.id, &["search".to_string()])
            .expect("approve");
        let some = adapter.instructions(&conn);
        assert!(
            some.contains("Current policy: Enabled tools: search."),
            "{some}"
        );
        assert!(
            some.starts_with("MCP server integration \"Docs server\" — development environment."),
            "{some}"
        );

        client::invalidate(&conn.id);
    }
}
