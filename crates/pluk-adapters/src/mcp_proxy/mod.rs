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
mod child;
pub mod client;
mod discovery;
pub mod local;
pub mod oauth;
pub mod probe;
pub mod transport;

use std::borrow::Cow;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde_json::{Map, Value, json};

use pluk_policy::tool_gate;
use pluk_store::{Config, Integration, ProxyTool, SecretKind, Store, ToolState, Verdict};

use crate::adapter::{Adapter, ApiRequest, ApiResponse, PolicyKind};
use crate::config_field::{ConfigField, FieldType};
use crate::error::AdapterError;
use crate::gate::{CallTarget, GateMeta, GateOpts, Outcome, RunOutcome, run_gated};
use crate::instructions::{InstructionParts, build_instructions};
use crate::key_value::{self, ConfigProblem};
use crate::tool_host::{ToolHandler, ToolHost, ToolRegistration};
use crate::tool_spec::ToolSpec;

use client::{UpstreamAuth, UpstreamCall};
use probe::SignInRequired;
use transport::{HEADERS_KEY, SignIn};

pub const ADAPTER_ID: &str = "mcp";

/// The tool upstream now describes is not the one that was approved.
pub const TOOL_CHANGED_CODE: &str = "MCP_PROXY_TOOL_CHANGED";
/// The stored token no longer signs in to the upstream server.
pub const TOKEN_REJECTED_CODE: &str = "MCP_PROXY_TOKEN_REJECTED";
/// The server only answers people who signed in, and nobody has.
pub const SIGN_IN_NEEDED_CODE: &str = "MCP_PROXY_SIGN_IN_NEEDED";
/// The server only answers a token, and none is saved.
pub const TOKEN_NEEDED_CODE: &str = "MCP_PROXY_TOKEN_NEEDED";

const TOOL_CHANGED: &str = "This tool changed. Turn it on again in Pluk.";
const TOKEN_REJECTED: &str = "Pluk could not sign in to this MCP server. Check the token in Pluk.";
const HEADERS_REJECTED: &str =
    "Pluk could not sign in to this MCP server. Check the headers in Pluk.";
const PERMISSION_DENIED: &str =
    "This MCP server refused the request. The account signed in to Pluk may not have permission.";
const SIGN_IN_NEEDED: &str = "This server needs you to sign in. Open the Tools tab and sign in.";
const TOKEN_NEEDED: &str = "This server needs a token. Add one in this integration's settings.";

const AGENT_HINT: &str = "Use this to reach the tools of another MCP server the owner connected in Pluk. Each tool is that server's own: call it by name with the arguments its schema describes.";

/// How much of a call's arguments the log line keeps. The response itself is
/// capped by the store; the arguments share one line with the tool name.
const MAX_LOGGED_ARGS: usize = 4_000;
/// How much of an upstream failure the row's reason keeps. The full text is
/// still stored as the response.
const MAX_REASON_CHARS: usize = 200;
/// How a cut line says it was cut, matching the store's own marker.
const TRUNCATED: &str = "…[truncated]";

fn mcp_fields() -> Vec<ConfigField> {
    let when_local = || json!(local::LOCAL);
    vec![
        ConfigField::new(local::CONNECTION_KEY, "Connection", FieldType::Select)
            .group("Connection")
            .options(&[(local::REMOTE, "Remote server"), (local::LOCAL, "Local server")])
            .default_value(&json!(local::REMOTE))
            .help("A remote server has a URL. A local server is a command Pluk starts on this Mac."),
        ConfigField::new(local::COMMAND_KEY, "Command", FieldType::Text)
            .group("Connection")
            .show_if_eq(local::CONNECTION_KEY, &when_local())
            .placeholder("npx")
            .help("The program that starts the server, such as npx, node or uvx."),
        ConfigField::new(local::ARGS_KEY, "Arguments", FieldType::List)
            .group("Connection")
            .show_if_eq(local::CONNECTION_KEY, &when_local())
            .help("Passed to the command one by one, exactly as written."),
        ConfigField::new(local::CWD_KEY, "Working folder", FieldType::Text)
            .group("Connection")
            .show_if_eq(local::CONNECTION_KEY, &when_local())
            .placeholder("~")
            .help("Where the command runs. Leave this empty to use your home folder."),
        ConfigField::key_value(local::ENV_KEY, "Environment variables", SecretKind::Env)
            .group("Connection")
            .show_if_eq(local::CONNECTION_KEY, &when_local())
            .default_not_secret()
            .help("Set for this server only. Turn Secret on for anything like a token or key."),
        ConfigField::new("url", "Server URL", FieldType::Text)
            .group("Connection")
            .required()
            .show_unless_eq(local::CONNECTION_KEY, &when_local())
            .placeholder("https://example.com/mcp")
            .help("Pluk works out what this server needs. The other fields are only for the few servers that ask for more."),
        ConfigField::key_value(HEADERS_KEY, "Headers", SecretKind::Header)
            .group("Connection")
            .show_unless_eq(local::CONNECTION_KEY, &when_local())
            .help("Sent with every request, exactly as written. Most servers do not need any. Add them if the server asked for them."),
        ConfigField::new("token", "Token", FieldType::Password)
            .group("Sign-in")
            .secret()
            .show_unless_eq(local::CONNECTION_KEY, &when_local())
            .placeholder("leave empty")
            .help("Most servers do not need one. Add the token if the server gave you one."),
        ConfigField::new("header_name", "Header name", FieldType::Text)
            .group("Sign-in")
            .show_unless_eq(local::CONNECTION_KEY, &when_local())
            .default_value(&json!(client::DEFAULT_AUTH_HEADER))
            .help("Leave this as it is unless the server asked for the token under another name."),
        ConfigField::new("client_id", "Client ID", FieldType::Text)
            .group("Sign-in")
            .show_unless_eq(local::CONNECTION_KEY, &when_local())
            .placeholder("leave empty")
            .help("Most servers do not need one. Add it if the server asked you to register Pluk first."),
        ConfigField::new("client_secret", "Client secret", FieldType::Password)
            .group("Sign-in")
            .secret()
            .show_unless_eq(local::CONNECTION_KEY, &when_local())
            .placeholder("leave empty")
            .help("Add this only if the server gave you one with the client ID."),
    ]
}

pub struct McpProxyAdapter {
    store: Arc<Store>,
}

impl McpProxyAdapter {
    pub fn new(store: Arc<Store>) -> Arc<Self> {
        Arc::new(McpProxyAdapter { store })
    }

    /// Whether the user signed in to this integration's server through Pluk.
    /// A read that fails counts as signed in, so the check errs on refusing.
    fn has_sign_in(&self, integration_id: Option<&str>) -> bool {
        integration_id.is_some_and(|id| !matches!(self.store.get_proxy_auth(id), Ok(None)))
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

    /// Reaching the server is the test. A server that will only answer someone
    /// who signed in is working as built, so the failure it reports is the step
    /// the user still owes it rather than the refusal it gave Pluk.
    /// A local server is started for the test, once the user approved its
    /// command.
    async fn test_connection(&self, conn: &Integration) -> Result<(), AdapterError> {
        credentials_in_hand(&self.store, conn).await?;
        catalog::discover(&self.store, conn).await.map(|_| ())
    }

    /// For a remote server, header rows the transport would refuse, or that
    /// clash with the sign-in this integration already sends. For a local
    /// one, a command, args, folder or variable Pluk could not start it with.
    fn check_config(
        &self,
        integration_id: Option<&str>,
        config: &Config,
    ) -> Result<(), ConfigProblem> {
        if local::is_local_config(config) {
            return local::check_config(config);
        }
        let token = config
            .get("token")
            .and_then(Value::as_str)
            .is_some_and(|token| !token.trim().is_empty());
        let sign_in = if token {
            let header = config
                .get("header_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(client::DEFAULT_AUTH_HEADER);
            SignIn::Token {
                header: header.to_string(),
            }
        } else if self.has_sign_in(integration_id) {
            SignIn::Oauth
        } else {
            SignIn::None
        };
        transport::check_rows(&key_value::rows(config, HEADERS_KEY), &sign_in)
            .map_err(|(row, message)| ConfigProblem::at_row(HEADERS_KEY, row, message))
    }

    fn humanize_error(&self, error: &AdapterError) -> Option<String> {
        if error.has_code(SIGN_IN_NEEDED_CODE) {
            return Some(SIGN_IN_NEEDED.to_string());
        }
        if error.has_code(local::LAUNCH_NOT_APPROVED_CODE) {
            return Some(local::LAUNCH_NOT_APPROVED.to_string());
        }
        error
            .has_code(TOKEN_NEEDED_CODE)
            .then(|| TOKEN_NEEDED.to_string())
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

/// Whether Pluk holds what this server asks for. A saved secret header
/// counts: it is how a server that takes keys in headers is signed in to.
/// The failure names the step the user still owes it.
async fn credentials_in_hand(store: &Store, conn: &Integration) -> Result<(), AdapterError> {
    let required = probe::required(conn).await?;
    if required == SignInRequired::None
        || catalog::upstream_auth(store, conn).await? != UpstreamAuth::None
        || transport::has_secret_headers(store, conn)?
    {
        return Ok(());
    }
    Err(match required {
        SignInRequired::Token => AdapterError::new(TOKEN_NEEDED).with_code(TOKEN_NEEDED_CODE),
        _ => AdapterError::new(SIGN_IN_NEEDED).with_code(SIGN_IN_NEEDED_CODE),
    })
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
            let meta = GateMeta::new(category, name.clone(), detail_of(&name, &args));
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

/// The log line for one call: the tool name, and the arguments it was given.
fn detail_of(name: &str, args: &Value) -> String {
    let arguments = match args {
        Value::Object(arguments) if !arguments.is_empty() => arguments,
        _ => return name.to_string(),
    };
    let rendered = serde_json::to_string(arguments).unwrap_or_default();
    format!("{name} {}", cut_to(&rendered, MAX_LOGGED_ARGS))
}

fn cut_to(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}{TRUNCATED}")
}

/// One proxied call.
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
        Ok(call) => Ok(outcome_of(call)),
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
/// access token can run out mid-call. A token or secret header the user typed
/// in is theirs to fix, and an open server refusing us is its own failure to
/// report.
async fn after_refusal(
    store: &Store,
    conn: &Integration,
    name: &str,
    arguments: Option<Map<String, Value>>,
    error: AdapterError,
) -> Result<Outcome, AdapterError> {
    if !oauth::renew(store, conn).await? {
        return match catalog::static_auth(conn) {
            UpstreamAuth::None if transport::has_secret_headers(store, conn)? => {
                Err(AdapterError::new(HEADERS_REJECTED).with_code(TOKEN_REJECTED_CODE))
            }
            UpstreamAuth::None => Err(error),
            _ => Err(AdapterError::new(TOKEN_REJECTED).with_code(TOKEN_REJECTED_CODE)),
        };
    }
    let client = catalog::client_for(store, conn).await?;
    match client.call_tool(name, arguments).await {
        Ok(call) => Ok(outcome_of(call)),
        // A token minted seconds ago and still refused is a grant the server
        // no longer honors, whatever it says.
        Err(again) if again.has_code(client::AUTH_REJECTED_CODE) => {
            Err(oauth::require_sign_in(store, &conn.id))
        }
        Err(again) => Err(again),
    }
}

fn outcome_of(call: UpstreamCall) -> Outcome {
    let text = call
        .result
        .content
        .iter()
        .map(|part| part.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    Outcome::Ran(RunOutcome {
        text,
        is_error: call.result.is_error,
        reason: call.result.is_error.then(|| reason_of(&call.logged)),
        response_text: Some(call.logged),
        ..Default::default()
    })
}

/// The one line the log shows for a failure: what upstream said, cut to fit.
fn reason_of(logged: &str) -> String {
    let line = logged.lines().next().unwrap_or_default().trim();
    if line.is_empty() {
        return "the tool reported a failure".to_string();
    }
    cut_to(line, MAX_REASON_CHARS)
}

/// Pluk's own refusals are finished sentences; an upstream failure keeps the
/// runner's prefix so the agent can tell the two apart.
fn agent_text(error: &AdapterError, verdict: Verdict) -> String {
    if error.has_code(TOOL_CHANGED_CODE)
        || error.has_code(TOKEN_REJECTED_CODE)
        || error.has_code(client::PERMISSION_DENIED_CODE)
        || error.has_code(oauth::RECONNECT_NEEDED_CODE)
        || error.has_code(local::LAUNCH_NOT_APPROVED_CODE)
        || error.has_code(client::SERVER_CRASHED_CODE)
        || error.has_code(client::SERVER_STOPPED_CODE)
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
    use axum::response::IntoResponse;
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

    use pluk_store::{Environment, IntegrationInput, LOG_RESPONSE_LIMIT, LogEntry};

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
            let result = match &*request.name {
                "flood" => CallToolResult::success(vec![ContentBlock::text(
                    "y".repeat(LOG_RESPONSE_LIMIT + 50),
                )]),
                "break" => CallToolResult::error(vec![ContentBlock::text(
                    "the document is locked".to_string(),
                )]),
                _ => CallToolResult::success(vec![ContentBlock::text(format!("found {query}"))]),
            };
            Ok(CallToolResponse::Complete(result))
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

    /// Every request's headers, as the upstream server received them.
    type Received = Arc<Mutex<Vec<axum::http::HeaderMap>>>;

    /// An upstream that answers only a request carrying `key` in
    /// `DD_API_KEY`, and otherwise refuses with a bare 401 while publishing a
    /// sign-in it would never honor, the way Datadog does.
    async fn keyed_upstream(key: &'static str) -> (String, Received) {
        let received: Received = Arc::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let handler = TestServer {
            tools: Arc::new(Mutex::new(vec![tool("search", "Search the docs")])),
        };
        let service = StreamableHttpService::new(
            move || Ok(handler.clone()),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default(),
        );
        let seen = received.clone();
        let issuer = base.clone();
        let router = Router::new()
            .route(
                "/.well-known/oauth-authorization-server",
                any(move || {
                    let issuer = issuer.clone();
                    async move {
                        axum::Json(json!({
                            "issuer": issuer,
                            "authorization_endpoint": format!("{issuer}/authorize"),
                            "token_endpoint": format!("{issuer}/token"),
                            "registration_endpoint": format!("{issuer}/register"),
                        }))
                    }
                }),
            )
            .route(
                "/mcp",
                any(move |request: axum::extract::Request| {
                    let service = service.clone();
                    let seen = seen.clone();
                    async move {
                        seen.lock()
                            .expect("received")
                            .push(request.headers().clone());
                        let keyed = request
                            .headers()
                            .get("DD_API_KEY")
                            .is_some_and(|value| value == key);
                        if !keyed {
                            return axum::http::StatusCode::UNAUTHORIZED.into_response();
                        }
                        match service.oneshot(request).await {
                            Ok(response) => response.map(axum::body::Body::new).into_response(),
                            Err(never) => match never {},
                        }
                    }
                }),
            );
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        (format!("{base}/mcp"), received)
    }

    fn save_header(store: &Store, id: &str, name: &str, value: &str) {
        store
            .write_proxy_secrets(
                id,
                &[pluk_store::SecretWrite::Set {
                    kind: SecretKind::Header,
                    name: name.to_string(),
                    value: value.to_string(),
                }],
            )
            .expect("save header");
    }

    fn header_of(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    #[tokio::test]
    async fn key_headers_reach_a_server_that_takes_no_other_sign_in() {
        let (endpoint, received) = keyed_upstream("dd-key-1").await;
        let (_dir, store) = store();
        let adapter = McpProxyAdapter::new(store.clone());
        let conn = integration(
            "datadog-headers",
            json!({ "url": endpoint, "headers": [
                {"name": "DD_API_KEY", "secret": true},
                {"name": "DD_APPLICATION_KEY", "secret": true},
                {"name": "X-Org", "value": "acme", "secret": false},
            ]}),
        );
        save_header(&store, &conn.id, "DD_API_KEY", "dd-key-1");
        save_header(&store, &conn.id, "DD_APPLICATION_KEY", "dd-app-1");

        adapter
            .test_connection(&conn)
            .await
            .expect("a saved key header counts as signed in");
        assert_eq!(
            states(&store, &conn.id),
            [("search".to_string(), ToolState::New)]
        );

        // The session asks for either reply format, SSE first; the probe and
        // its sign-in discovery ask otherwise.
        let received = received.lock().expect("received").clone();
        let (session, probe): (Vec<_>, Vec<_>) = received.iter().partition(|headers| {
            header_of(headers, "accept").as_deref() == Some("text/event-stream, application/json")
        });
        assert_eq!(header_of(probe[0], "x-org").as_deref(), Some("acme"));
        for headers in &probe {
            assert_eq!(
                header_of(headers, "dd_api_key"),
                None,
                "the probe carries no secret"
            );
            assert_eq!(header_of(headers, "dd_application_key"), None);
        }
        assert!(!session.is_empty());
        for headers in &session {
            assert_eq!(
                header_of(headers, "dd_api_key").as_deref(),
                Some("dd-key-1")
            );
            assert_eq!(
                header_of(headers, "dd_application_key").as_deref(),
                Some("dd-app-1")
            );
            assert_eq!(header_of(headers, "x-org").as_deref(), Some("acme"));
        }

        client::shutdown(&conn.id);
    }

    #[tokio::test]
    async fn an_authorization_header_goes_out_exactly_as_written() {
        let (endpoint, received) = recording_upstream().await;
        let (_dir, store) = store();
        let adapter = McpProxyAdapter::new(store.clone());
        let conn = integration(
            "wix-headers",
            json!({ "url": endpoint, "headers": [
                {"name": "Authorization", "secret": true},
                {"name": "wix-account-id", "value": "account-1", "secret": false},
            ]}),
        );
        save_header(&store, &conn.id, "Authorization", "IST.wix-key");

        adapter.test_connection(&conn).await.expect("discover");

        let received = received.lock().expect("received").clone();
        let session = received.last().expect("requests");
        assert_eq!(
            header_of(session, "authorization").as_deref(),
            Some("IST.wix-key")
        );
        assert_eq!(
            header_of(session, "wix-account-id").as_deref(),
            Some("account-1")
        );

        client::shutdown(&conn.id);
    }

    #[tokio::test]
    async fn a_sign_in_wins_over_an_authorization_header_and_other_headers_still_go() {
        let (endpoint, received) = recording_upstream().await;
        let (_dir, store) = store();
        let conn = integration(
            "grafana-headers",
            json!({ "url": endpoint, "headers": [
                {"name": "Authorization", "secret": true},
                {"name": "X-Grafana-URL", "value": "https://grafana.example.com", "secret": false},
            ]}),
        );
        save_header(&store, &conn.id, "Authorization", "stale-key");
        let headers = transport::static_headers(&store, &conn).expect("headers");
        let client =
            client::McpProxyClient::new(&conn.id, endpoint, UpstreamAuth::bearer("oauth-1"))
                .with_headers(headers);

        client.list_tools().await.expect("list");

        let received = received.lock().expect("received").clone();
        for headers in &received {
            let sent: Vec<_> = headers.get_all("authorization").iter().collect();
            assert_eq!(sent, ["Bearer oauth-1"]);
            assert_eq!(
                header_of(headers, "x-grafana-url").as_deref(),
                Some("https://grafana.example.com")
            );
        }

        client::shutdown(&conn.id);
    }

    #[tokio::test]
    async fn changing_a_header_reconnects_the_session() {
        let (endpoint, received) = recording_upstream().await;
        let (_dir, store) = store();
        let adapter = McpProxyAdapter::new(store.clone());
        let rows = json!([{"name": "X-Key", "secret": true}]);
        let conn = integration("header-edit", json!({ "url": endpoint, "headers": rows }));
        save_header(&store, &conn.id, "X-Key", "first");
        adapter.test_connection(&conn).await.expect("discover");

        save_header(&store, &conn.id, "X-Key", "second");
        adapter.test_connection(&conn).await.expect("rediscover");

        let received = received.lock().expect("received").clone();
        let last = received.last().expect("requests");
        assert_eq!(header_of(last, "x-key").as_deref(), Some("second"));

        client::shutdown(&conn.id);
    }

    #[tokio::test]
    async fn a_refused_key_header_is_named_without_being_shown() {
        let (endpoint, _tools) = upstream().await;
        let (_dir, store) = store();
        let adapter = McpProxyAdapter::new(store.clone());
        let rows = json!([{"name": "DD_API_KEY", "secret": true}]);
        let mut conn = integration(
            "header-refused",
            json!({ "url": endpoint, "headers": rows }),
        );
        conn.query_policy = Some(enabling("search"));
        save_header(&store, &conn.id, "DD_API_KEY", TOKEN);
        adapter.test_connection(&conn).await.expect("discover");
        store
            .approve_proxy_tools(&conn.id, &["search".to_string()])
            .expect("approve");

        let mut refused = conn.clone();
        refused
            .config
            .insert("url".to_string(), Value::String(rejecting_upstream().await));
        let mut host = RecordingHost::default();
        adapter
            .register(&mut host, &refused, "owner")
            .expect("register");
        let result = host.handlers["search"](json!({ "q": "onboarding" })).await;

        assert!(result.is_error);
        assert_eq!(result.text(), HEADERS_REJECTED);
        assert!(!format!("{:?}{result:?}", logs(&store)).contains(TOKEN));

        client::shutdown(&conn.id);
    }

    #[test]
    fn a_save_is_refused_at_the_header_row_that_breaks_a_rule() {
        let (_dir, store) = store();
        let adapter = McpProxyAdapter::new(store.clone());
        let config = |value: Value| match value {
            Value::Object(map) => map,
            _ => unreachable!(),
        };
        let rows = json!([
            {"name": "X-Org", "value": "acme", "secret": false},
            {"name": "Authorization", "value": "IST.key-1"},
        ]);

        assert_eq!(
            adapter.check_config(None, &config(json!({ "headers": rows }))),
            Ok(())
        );
        let with_token = adapter
            .check_config(None, &config(json!({ "headers": rows, "token": "t0ken" })))
            .expect_err("a token is saved");
        assert_eq!(with_token.field, HEADERS_KEY);
        assert_eq!(with_token.row, Some(1));
        assert!(!with_token.message.contains("IST.key-1"));

        let signed_in = store
            .create_integration(&IntegrationInput::new("Signed in", ADAPTER_ID))
            .expect("create");
        store
            .set_proxy_auth(&pluk_store::ProxyAuthInput {
                integration_id: signed_in.id.clone(),
                kind: "oauth".to_string(),
                access_token: "access-1".to_string(),
                refresh_token: None,
                expires_at: None,
                client_id: None,
                client_secret: None,
                metadata_json: None,
            })
            .expect("sign in");
        let problem = adapter
            .check_config(Some(&signed_in.id), &config(json!({ "headers": rows })))
            .expect_err("signed in");
        assert_eq!(
            problem.message,
            "Authorization is already sent by the sign-in. Remove this header."
        );

        let reserved = adapter
            .check_config(
                None,
                &config(json!({ "headers": [{"name": "Mcp-Session-Id", "value": "x"}] })),
            )
            .expect_err("reserved");
        assert_eq!(reserved.row, Some(0));
    }

    /// An open upstream that records the headers of every request.
    async fn recording_upstream() -> (String, Received) {
        let received: Received = Arc::default();
        let handler = TestServer {
            tools: Arc::new(Mutex::new(vec![tool("search", "Search the docs")])),
        };
        let service = StreamableHttpService::new(
            move || Ok(handler.clone()),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default(),
        );
        let seen = received.clone();
        let router = Router::new().route(
            "/mcp",
            any(move |request: axum::extract::Request| {
                let service = service.clone();
                let seen = seen.clone();
                async move {
                    seen.lock()
                        .expect("received")
                        .push(request.headers().clone());
                    match service.oneshot(request).await {
                        Ok(response) => response.map(axum::body::Body::new),
                        Err(never) => match never {},
                    }
                }
            }),
        );
        (serve(router).await, received)
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

        client::shutdown(&conn.id);
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

        client::shutdown(&conn.id);
    }

    #[tokio::test]
    async fn a_proxied_call_logs_its_arguments_and_the_response_but_no_credential() {
        let (endpoint, _tools) = upstream().await;
        let (_dir, store) = store();
        let adapter = McpProxyAdapter::new(store.clone());
        let mut conn = integration("proxied-call", json!({ "url": endpoint, "token": TOKEN }));
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
        assert_eq!(entries[0].sql, r#"search {"q":"onboarding"}"#);
        assert_eq!(entries[0].source.as_deref(), Some("search"));
        assert_eq!(entries[0].verdict, "allowed");
        assert_eq!(
            entries[0].response_text.as_deref(),
            Some("found onboarding")
        );
        assert!(
            !format!("{entries:?}").contains(TOKEN),
            "no credential reaches the log"
        );

        client::shutdown(&conn.id);
    }

    #[tokio::test]
    async fn an_oversized_response_is_stored_up_to_the_log_cap() {
        let (endpoint, tools) = upstream().await;
        *tools.lock().expect("tools") = vec![tool("flood", "Return a lot")];
        let (_dir, store) = store();
        let adapter = McpProxyAdapter::new(store.clone());
        let mut conn = integration("oversized-response", json!({ "url": endpoint }));
        conn.query_policy = Some(enabling("flood"));

        adapter.test_connection(&conn).await.expect("discover");
        store
            .approve_proxy_tools(&conn.id, &["flood".to_string()])
            .expect("approve");
        let host = registered(&adapter, &conn);

        host.handlers["flood"](json!({})).await;

        let stored = logs(&store)[0]
            .response_text
            .clone()
            .expect("response stored");
        assert!(stored.ends_with(TRUNCATED), "{}", &stored[..40]);
        assert_eq!(
            stored.chars().filter(|char| *char == 'y').count(),
            LOG_RESPONSE_LIMIT
        );

        client::shutdown(&conn.id);
    }

    #[tokio::test]
    async fn an_upstream_tool_error_is_logged_as_a_failed_call_with_its_message() {
        let (endpoint, tools) = upstream().await;
        *tools.lock().expect("tools") = vec![tool("break", "Fail on purpose")];
        let (_dir, store) = store();
        let adapter = McpProxyAdapter::new(store.clone());
        let mut conn = integration("upstream-error", json!({ "url": endpoint }));
        conn.query_policy = Some(enabling("break"));

        adapter.test_connection(&conn).await.expect("discover");
        store
            .approve_proxy_tools(&conn.id, &["break".to_string()])
            .expect("approve");
        let host = registered(&adapter, &conn);

        let result = host.handlers["break"](json!({ "q": "budget" })).await;
        assert!(result.is_error);
        assert_eq!(result.text(), "the document is locked");

        let entries = logs(&store);
        assert_eq!(entries[0].verdict, "error");
        assert_eq!(entries[0].reason.as_deref(), Some("the document is locked"));
        assert_eq!(
            entries[0].response_text.as_deref(),
            Some("the document is locked")
        );

        client::shutdown(&conn.id);
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

        client::shutdown(&conn.id);
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

        client::shutdown(&conn.id);
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

        client::shutdown(&conn.id);
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
            json!({ "kind": "none", "status": "not_connected", "required": "none" }),
            "an open server asks for nothing, so the screen offers no sign-in"
        );

        client::shutdown(&conn.id);
    }

    #[tokio::test]
    async fn turning_a_tool_on_is_one_step_and_turning_it_off_keeps_it_ready() {
        let (endpoint, tools) = upstream().await;
        let (_dir, store) = store();
        let adapter = McpProxyAdapter::new(store.clone());
        let mut input = IntegrationInput::new("Docs server", ADAPTER_ID);
        input
            .config
            .insert("url".to_string(), Value::String(endpoint));
        let conn = store.create_integration(&input).expect("create");
        let stored = || {
            store
                .integration_by_id(&conn.id)
                .expect("read")
                .expect("row")
        };
        let switch = |on: bool| Some(json!({ "names": ["search"], "enabled": on }).to_string());

        call_api(&adapter, &conn, "POST", "/proxy/refresh", None).await;
        assert!(registered(&adapter, &stored()).names().is_empty());

        let on = call_api(&adapter, &conn, "POST", "/proxy/enable", switch(true)).await;
        assert_eq!(on["tools"][0]["state"], json!("approved"));
        assert_eq!(
            registered(&adapter, &stored()).names(),
            ["search".to_string()]
        );

        call_api(&adapter, &conn, "POST", "/proxy/enable", switch(false)).await;
        assert!(registered(&adapter, &stored()).names().is_empty());
        assert_eq!(
            states(&store, &conn.id),
            [("search".to_string(), ToolState::Approved)],
            "turning a tool off leaves it pinned, so it goes back on in one step"
        );

        // A tool the server now describes differently is held back until the
        // user turns it on against that new description.
        *tools.lock().expect("tools") = vec![tool("search", "Search everything")];
        call_api(&adapter, &conn, "POST", "/proxy/enable", switch(true)).await;
        let changed = call_api(&adapter, &conn, "POST", "/proxy/refresh", None).await;
        assert_eq!(changed["tools"][0]["state"], json!("changed"));
        assert!(registered(&adapter, &stored()).names().is_empty());

        let again = call_api(&adapter, &conn, "POST", "/proxy/enable", switch(true)).await;
        assert_eq!(again["tools"][0]["state"], json!("approved"));
        assert_eq!(
            registered(&adapter, &stored()).names(),
            ["search".to_string()]
        );

        client::shutdown(&conn.id);
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

        client::shutdown(&conn.id);
    }
}
