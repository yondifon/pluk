//! The client Pluk points at an upstream MCP server.
//!
//! Pluk's own MCP surface is stateless: it is rebuilt from the store on every
//! protocol request. Tying an upstream session to one of those requests would
//! re-handshake with the upstream server on every tool call, so the session
//! lives here instead — one per integration, connected on first use, shared by
//! every handler, and dropped only when the integration's config or
//! credentials change.
//!
//! Credentials are passed in by the caller. Nothing here reads the store, and
//! no token reaches a log line, an error message, or a `Debug` rendering.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderName, HeaderValue};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientInfo, ContentBlock, JsonObject, Tool,
};
use rmcp::service::{ClientInitializeError, NotificationContext, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpError,
};
use rmcp::{ClientHandler, RoleClient, ServiceError, serve_client};
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::timeout;

use crate::error::AdapterError;
use crate::gate::{TextContent, ToolResult};

use super::transport::StaticHeader;

/// Upstream refused the credentials we presented. The caller may refresh the
/// token and retry once.
pub const AUTH_REJECTED_CODE: &str = "MCP_PROXY_AUTH_REJECTED";
/// Upstream knows who we are and will not do this. No credential fixes it, so
/// nothing about it is worth a retry.
pub const PERMISSION_DENIED_CODE: &str = "MCP_PROXY_PERMISSION_DENIED";
/// The upstream server could not be reached at all.
pub const UPSTREAM_UNREACHABLE_CODE: &str = "MCP_PROXY_UNREACHABLE";
/// The upstream server accepted the request but did not answer in time.
pub const UPSTREAM_TIMEOUT_CODE: &str = "MCP_PROXY_TIMEOUT";

/// Header a bare token is sent in when the integration names no other.
pub const DEFAULT_AUTH_HEADER: &str = "Authorization";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// How much tool-result text is handed back before it is cut.
pub const MAX_RESULT_BYTES: usize = 1_000_000;
const TRUNCATION_MARKER: &str = "[upstream result truncated at 1 MB]";

/// What Pluk presents to the upstream server to prove who it is.
///
/// `Debug` never renders a secret: an accidental `{:?}` on an adapter, a
/// config struct, or an error chain cannot leak a token into the log.
#[derive(Clone, PartialEq, Eq, Default)]
pub enum UpstreamAuth {
    /// The server is open, or authenticates by other means.
    #[default]
    None,
    /// A fixed header, the same on every request.
    Header { name: String, value: String },
    /// An OAuth access token, sent as `Authorization: Bearer <token>`.
    Bearer { access_token: String },
}

impl UpstreamAuth {
    /// A static header. An empty name falls back to `Authorization`, and a
    /// bare token in that header is sent as `Bearer <token>`.
    pub fn header(name: &str, value: &str) -> Self {
        let name = match name.trim() {
            "" => DEFAULT_AUTH_HEADER,
            named => named,
        };
        let value = value.trim();
        let value = if name.eq_ignore_ascii_case(DEFAULT_AUTH_HEADER) && !value.contains(' ') {
            format!("Bearer {value}")
        } else {
            value.to_string()
        };
        UpstreamAuth::Header {
            name: name.to_string(),
            value,
        }
    }

    pub fn bearer(access_token: impl Into<String>) -> Self {
        UpstreamAuth::Bearer {
            access_token: access_token.into(),
        }
    }
}

impl std::fmt::Debug for UpstreamAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpstreamAuth::None => f.write_str("UpstreamAuth::None"),
            UpstreamAuth::Header { name, .. } => {
                write!(
                    f,
                    "UpstreamAuth::Header {{ name: {name:?}, value: <redacted> }}"
                )
            }
            UpstreamAuth::Bearer { .. } => {
                f.write_str("UpstreamAuth::Bearer { access_token: <redacted> }")
            }
        }
    }
}

/// One tool as the upstream server just described it.
#[derive(Debug, Clone, PartialEq)]
pub struct UpstreamTool {
    pub name: String,
    /// Empty when the server published none.
    pub description: String,
    /// The tool's JSON Schema, as received.
    pub input_schema: Value,
    pub annotations: Option<Value>,
}

impl From<Tool> for UpstreamTool {
    fn from(tool: Tool) -> Self {
        UpstreamTool {
            name: tool.name.into_owned(),
            description: tool.description.map(|d| d.into_owned()).unwrap_or_default(),
            input_schema: Value::Object((*tool.input_schema).clone()),
            annotations: tool
                .annotations
                .and_then(|a| serde_json::to_value(a).ok())
                .filter(|a| !a.is_null()),
        }
    }
}

/// A handle onto the pooled session for one integration.
///
/// Building one costs nothing and connects nothing; the first call that needs
/// the upstream server opens the session, and every later handle for the same
/// integration id reuses it.
#[derive(Debug, Clone)]
pub struct McpProxyClient {
    integration_id: String,
    endpoint: String,
    auth: UpstreamAuth,
    headers: Vec<StaticHeader>,
}

impl McpProxyClient {
    pub fn new(
        integration_id: impl Into<String>,
        endpoint: impl Into<String>,
        auth: UpstreamAuth,
    ) -> Self {
        McpProxyClient {
            integration_id: integration_id.into(),
            endpoint: endpoint.into(),
            auth,
            headers: Vec::new(),
        }
    }

    /// Headers sent on every request on top of the sign-in. Where one names
    /// the header the sign-in uses, the sign-in wins.
    pub fn with_headers(mut self, headers: Vec<StaticHeader>) -> Self {
        self.headers = headers;
        self
    }

    /// Every tool the upstream server offers, following pagination to the end.
    pub async fn list_tools(&self) -> Result<Vec<UpstreamTool>, AdapterError> {
        let tools = self
            .with_session(|upstream| async move { upstream.list_all_tools().await })
            .await?;
        Ok(tools.into_iter().map(UpstreamTool::from).collect())
    }

    /// Call one upstream tool. An upstream tool that reports failure comes
    /// back as a result with `is_error` set, not as an [`AdapterError`] —
    /// only the connection itself errors.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Option<JsonObject>,
    ) -> Result<UpstreamCall, AdapterError> {
        let mut params = CallToolRequestParams::new(name.to_string());
        params.arguments = arguments;
        let result = self
            .with_session(|upstream| {
                let params = params.clone();
                async move { upstream.call_tool(params).await }
            })
            .await?;
        Ok(shape_result(result))
    }

    /// Whether upstream announced a changed tool list since this was last
    /// asked. Reading it clears it.
    pub fn take_tools_changed(&self) -> bool {
        slot(&self.integration_id)
            .tools_changed
            .swap(false, Ordering::SeqCst)
    }

    async fn with_session<T, F, Fut>(&self, call: F) -> Result<T, AdapterError>
    where
        F: Fn(Arc<Upstream>) -> Fut,
        Fut: Future<Output = Result<T, ServiceError>>,
    {
        let session = self.session().await?;
        match timeout(REQUEST_TIMEOUT, call(session)).await {
            Err(_) => return Err(self.timed_out()),
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(error)) if !is_disconnected(&error) => return Err(self.failed(&error)),
            Ok(Err(_)) => {}
        }
        self.drop_session().await;
        let session = self.session().await?;
        match timeout(REQUEST_TIMEOUT, call(session)).await {
            Err(_) => Err(self.timed_out()),
            Ok(result) => result.map_err(|error| self.failed(&error)),
        }
    }

    async fn session(&self) -> Result<Arc<Upstream>, AdapterError> {
        let slot = slot(&self.integration_id);
        let mut held = slot.session.lock().await;
        if let Some(upstream) = held.as_ref() {
            if !upstream.is_closed() {
                return Ok(upstream.clone());
            }
            *held = None;
        }
        let upstream = Arc::new(self.connect(slot.tools_changed.clone()).await?);
        *held = Some(upstream.clone());
        Ok(upstream)
    }

    async fn drop_session(&self) {
        slot(&self.integration_id).session.lock().await.take();
    }

    async fn connect(&self, tools_changed: Arc<AtomicBool>) -> Result<Upstream, AdapterError> {
        let mut config = StreamableHttpClientTransportConfig::with_uri(self.endpoint.clone());
        let oauth = matches!(self.auth, UpstreamAuth::Bearer { .. });
        for header in &self.headers {
            // rmcp adds the OAuth token on its own, next to these rather than
            // in place of them.
            if oauth && header.name == AUTHORIZATION {
                continue;
            }
            config
                .custom_headers
                .insert(header.name.clone(), header.header_value()?);
        }
        match &self.auth {
            UpstreamAuth::None => {}
            UpstreamAuth::Bearer { access_token } => {
                config.auth_header = Some(access_token.clone());
            }
            UpstreamAuth::Header { name, value } => {
                let name = HeaderName::try_from(name.as_str()).map_err(|_| {
                    AdapterError::new(format!("`{name}` is not a valid HTTP header name"))
                })?;
                let value = HeaderValue::from_str(value).map_err(|_| {
                    AdapterError::new(format!(
                        "the value set for `{name}` is not a valid HTTP header value"
                    ))
                })?;
                config.custom_headers.insert(name, value);
            }
        }
        let transport = StreamableHttpClientTransport::with_client(session_client()?, config);
        let handler = ProxyHandler { tools_changed };
        match timeout(CONNECT_TIMEOUT, serve_client(handler, transport)).await {
            Err(_) => Err(self.timed_out()),
            Ok(Ok(upstream)) => Ok(upstream),
            Ok(Err(error)) => Err(AdapterError::new(format!(
                "could not connect to the MCP server at {}: {error}",
                self.endpoint
            ))
            .with_code(failure_code(&error).unwrap_or(UPSTREAM_UNREACHABLE_CODE))),
        }
    }

    fn timed_out(&self) -> AdapterError {
        AdapterError::new(format!(
            "the MCP server at {} did not answer",
            self.endpoint
        ))
        .with_code(UPSTREAM_TIMEOUT_CODE)
    }

    fn failed(&self, error: &ServiceError) -> AdapterError {
        let message = format!("the MCP server at {} failed: {error}", self.endpoint);
        match failure_code(error) {
            Some(code) => AdapterError::new(message).with_code(code),
            None => AdapterError::new(message),
        }
    }
}

/// Drop the pooled session for one integration. The next call reconnects with
/// whatever config and credentials are current by then.
pub fn invalidate(integration_id: &str) {
    pool()
        .lock()
        .expect("mcp proxy pool")
        .remove(integration_id);
}

/// The HTTP client the probe and sign-in discovery reach an upstream server
/// through. It follows redirects, since metadata documents are often moved,
/// and it never carries a credential. It is not the one
/// [`crate::http_client`] hands the API adapters: rmcp's transport is built on
/// the next major of reqwest, so the two cannot be the same value.
pub(super) fn upstream_client() -> Result<upstream_http::Client, AdapterError> {
    static CLIENT: OnceLock<Result<upstream_http::Client, String>> = OnceLock::new();
    shared(&CLIENT, upstream_http::Client::builder())
}

/// The HTTP client a session is carried on. It never follows a redirect: on a
/// hop to another host reqwest drops only `Authorization` and cookies, so any
/// other header holding a credential would go along to the new host.
fn session_client() -> Result<upstream_http::Client, AdapterError> {
    static CLIENT: OnceLock<Result<upstream_http::Client, String>> = OnceLock::new();
    shared(
        &CLIENT,
        upstream_http::Client::builder().redirect(upstream_http::redirect::Policy::none()),
    )
}

fn shared(
    cell: &'static OnceLock<Result<upstream_http::Client, String>>,
    builder: upstream_http::ClientBuilder,
) -> Result<upstream_http::Client, AdapterError> {
    cell.get_or_init(|| builder.build().map_err(|e| e.to_string()))
        .clone()
        .map_err(AdapterError::new)
}

type Upstream = RunningService<RoleClient, ProxyHandler>;

/// One integration's place in the pool. The flag outlives the session so a
/// reconnect does not swallow an announcement that arrived just before it.
struct Slot {
    tools_changed: Arc<AtomicBool>,
    session: AsyncMutex<Option<Arc<Upstream>>>,
}

fn pool() -> &'static Mutex<HashMap<String, Arc<Slot>>> {
    static POOL: OnceLock<Mutex<HashMap<String, Arc<Slot>>>> = OnceLock::new();
    POOL.get_or_init(Default::default)
}

fn slot(integration_id: &str) -> Arc<Slot> {
    pool()
        .lock()
        .expect("mcp proxy pool")
        .entry(integration_id.to_string())
        .or_insert_with(|| {
            Arc::new(Slot {
                tools_changed: Arc::new(AtomicBool::new(false)),
                session: AsyncMutex::new(None),
            })
        })
        .clone()
}

struct ProxyHandler {
    tools_changed: Arc<AtomicBool>,
}

impl ClientHandler for ProxyHandler {
    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.tools_changed.store(true, Ordering::SeqCst);
    }

    fn get_info(&self) -> ClientInfo {
        client_info()
    }
}

/// How Pluk introduces itself to an upstream server, on a session it opens and
/// on a bare request it sends without one.
pub(super) fn client_info() -> ClientInfo {
    let mut info = ClientInfo::default();
    info.client_info.name = "pluk".to_string();
    info.client_info.version = env!("CARGO_PKG_VERSION").to_string();
    info
}

/// Whether the session is gone rather than the request being refused: worth
/// one reconnect, where a refusal is not.
fn is_disconnected(error: &ServiceError) -> bool {
    match error {
        ServiceError::TransportClosed => true,
        ServiceError::TransportSend(_) => !matches!(
            failure_code(error),
            Some(AUTH_REJECTED_CODE | PERMISSION_DENIED_CODE)
        ),
        _ => false,
    }
}

/// The stable code for a failure, read off the error chain the transport
/// produced. `None` means the upstream server answered and the failure is its
/// own, not the connection's.
fn failure_code(error: &(dyn std::error::Error + 'static)) -> Option<&'static str> {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(code) = code_of(error) {
            return Some(code);
        }
        current = cause_of(error);
    }
    None
}

fn code_of(error: &(dyn std::error::Error + 'static)) -> Option<&'static str> {
    if let Some(http) = error.downcast_ref::<StreamableHttpError<upstream_http::Error>>() {
        match http {
            StreamableHttpError::AuthRequired(_) => return Some(AUTH_REJECTED_CODE),
            StreamableHttpError::InsufficientScope(_) => return Some(PERMISSION_DENIED_CODE),
            StreamableHttpError::UnexpectedServerResponse(body) => {
                if let Some(code) = refusal_in(body) {
                    return Some(code);
                }
            }
            _ => {}
        }
    }
    if let Some(request) = error.downcast_ref::<upstream_http::Error>() {
        match request.status() {
            Some(upstream_http::StatusCode::UNAUTHORIZED) => return Some(AUTH_REJECTED_CODE),
            Some(upstream_http::StatusCode::FORBIDDEN) => return Some(PERMISSION_DENIED_CODE),
            _ => {}
        }
        if request.is_timeout() {
            return Some(UPSTREAM_TIMEOUT_CODE);
        }
        if request.is_connect() || request.is_request() {
            return Some(UPSTREAM_UNREACHABLE_CODE);
        }
    }
    match error.downcast_ref::<ServiceError>() {
        Some(ServiceError::Timeout { .. }) => Some(UPSTREAM_TIMEOUT_CODE),
        Some(ServiceError::TransportClosed) => Some(UPSTREAM_UNREACHABLE_CODE),
        _ => None,
    }
}

/// The next error down. Both error types that wrap a transport failure hold it
/// in a plain field rather than a `source`, so walking `source()` alone stops
/// before it ever reaches the HTTP status that explains the failure.
fn cause_of<'a>(
    error: &'a (dyn std::error::Error + 'static),
) -> Option<&'a (dyn std::error::Error + 'static)> {
    if let Some(ClientInitializeError::TransportError { error, .. }) =
        error.downcast_ref::<ClientInitializeError>()
    {
        return Some(error);
    }
    if let Some(ServiceError::TransportSend(error)) = error.downcast_ref::<ServiceError>() {
        return Some(error);
    }
    error.source()
}

/// The code for a 401 or 403 the transport passed through as a plain HTTP
/// failure, because the server sent no `WWW-Authenticate` header to go with it.
fn refusal_in(body: &str) -> Option<&'static str> {
    if body.starts_with("HTTP 401") {
        return Some(AUTH_REJECTED_CODE);
    }
    body.starts_with("HTTP 403")
        .then_some(PERMISSION_DENIED_CODE)
}

/// One upstream call: what the agent gets back, and what the activity log
/// records for it.
pub struct UpstreamCall {
    pub result: ToolResult,
    pub logged: String,
}

fn shape_result(result: CallToolResult) -> UpstreamCall {
    let mut texts: Vec<String> = Vec::new();
    let mut logged: Vec<String> = Vec::new();
    for block in &result.content {
        let (text, line) = render_block(block);
        texts.push(text);
        logged.push(line);
    }
    if texts.is_empty()
        && let Some(structured) = &result.structured_content
    {
        texts.push(structured.to_string());
        logged.push(structured.to_string());
    }
    let mut content = Vec::new();
    let mut budget = MAX_RESULT_BYTES;
    let mut truncated = false;
    for mut text in texts {
        if text.len() > budget {
            truncate_chars(&mut text, budget);
            truncated = true;
        }
        budget -= text.len();
        content.push(TextContent {
            content_type: "text",
            text,
        });
        if truncated {
            break;
        }
    }
    if truncated {
        content.push(TextContent {
            content_type: "text",
            text: TRUNCATION_MARKER.to_string(),
        });
    }
    UpstreamCall {
        result: ToolResult {
            content,
            is_error: result.is_error.unwrap_or(false),
        },
        logged: logged.join("\n"),
    }
}

/// What one content block becomes: the text the agent gets, and the line the
/// log keeps. Images, audio and embedded resources reach the agent as their
/// own JSON so nothing upstream returned is silently dropped; the log only
/// names them, because base64 bytes are unreadable there.
fn render_block(block: &ContentBlock) -> (String, String) {
    match block.as_text() {
        Some(text) => (text.text.clone(), text.text.clone()),
        None => (
            serde_json::to_string(block).unwrap_or_default(),
            block_label(block),
        ),
    }
}

fn block_label(block: &ContentBlock) -> String {
    let json = serde_json::to_value(block).unwrap_or_default();
    match json["type"].as_str().unwrap_or("content") {
        "resource" => "[embedded resource]".to_string(),
        "resource_link" => "[resource link]".to_string(),
        other => format!("[{other}]"),
    }
}

fn truncate_chars(s: &mut String, max: usize) {
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use axum::Router;
    use axum::routing::any;
    use rmcp::ErrorData as McpError;
    use rmcp::model::{
        CallToolResponse, CallToolResult, ContentBlock, ListToolsResult, PaginatedRequestParams,
        ServerCapabilities, ServerInfo, Tool,
    };
    use rmcp::service::RequestContext;
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
    use rmcp::{RoleServer, ServerHandler};
    use serde_json::json;
    use tower::ServiceExt as _;

    use super::*;

    const TOKEN: &str = "super-secret-upstream-token";

    /// A two-page tool list, an echo tool, and a tool that announces a changed
    /// list before it answers.
    #[derive(Clone)]
    struct TestServer {
        sessions: Arc<AtomicUsize>,
    }

    fn schema() -> JsonObject {
        match json!({"type": "object", "properties": {"text": {"type": "string"}}}) {
            Value::Object(map) => map,
            _ => unreachable!(),
        }
    }

    impl ServerHandler for TestServer {
        fn get_info(&self) -> ServerInfo {
            let mut info = ServerInfo::default();
            info.capabilities = ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build();
            info
        }

        async fn list_tools(
            &self,
            request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, McpError> {
            let cursor = request.and_then(|request| request.cursor);
            Ok(match cursor {
                None => {
                    let mut page = ListToolsResult::with_all_items(vec![Tool::new(
                        "echo",
                        "Echo the text back",
                        Arc::new(schema()),
                    )]);
                    page.next_cursor = Some("page-two".to_string());
                    page
                }
                Some(_) => ListToolsResult::with_all_items(vec![Tool::new(
                    "announce",
                    "Announce a changed tool list",
                    Arc::new(schema()),
                )]),
            })
        }

        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, McpError> {
            let result = match request.name.as_ref() {
                "announce" => {
                    let _ = context.peer.notify_tool_list_changed().await;
                    CallToolResult::success(vec![ContentBlock::text("announced")])
                }
                _ => {
                    let text = request
                        .arguments
                        .as_ref()
                        .and_then(|args| args.get("text"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    CallToolResult::success(vec![ContentBlock::text(text)])
                }
            };
            Ok(CallToolResponse::Complete(result))
        }
    }

    async fn serve(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        format!("http://{addr}/mcp")
    }

    /// The endpoint, plus a counter of how many upstream sessions were opened.
    async fn upstream() -> (String, Arc<AtomicUsize>) {
        let sessions = Arc::new(AtomicUsize::new(0));
        let handler = TestServer {
            sessions: sessions.clone(),
        };
        let service = StreamableHttpService::new(
            move || {
                handler.sessions.fetch_add(1, Ordering::SeqCst);
                Ok(handler.clone())
            },
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
        (serve(router).await, sessions)
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

    #[tokio::test]
    async fn lists_every_page_of_upstream_tools_and_calls_one() {
        let (endpoint, _) = upstream().await;
        let client = McpProxyClient::new("lists-and-calls", endpoint, UpstreamAuth::None);

        let tools = client.list_tools().await.expect("list tools");
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
        assert_eq!(names, vec!["echo", "announce"]);
        assert_eq!(tools[0].description, "Echo the text back");
        assert_eq!(tools[0].input_schema["type"], json!("object"));

        let mut arguments = JsonObject::new();
        arguments.insert("text".to_string(), json!("hello upstream"));
        let result = client
            .call_tool("echo", Some(arguments))
            .await
            .expect("call tool");
        assert!(!result.result.is_error);
        assert_eq!(result.result.text(), "hello upstream");

        invalidate("lists-and-calls");
    }

    #[tokio::test]
    async fn reuses_one_session_until_it_is_invalidated() {
        let (endpoint, sessions) = upstream().await;
        let client = McpProxyClient::new("reuses-session", endpoint, UpstreamAuth::None);

        client.list_tools().await.expect("first list");
        client.list_tools().await.expect("second list");
        assert_eq!(sessions.load(Ordering::SeqCst), 1);

        invalidate("reuses-session");
        client.list_tools().await.expect("list after invalidate");
        assert_eq!(sessions.load(Ordering::SeqCst), 2);

        invalidate("reuses-session");
    }

    #[tokio::test]
    async fn reports_a_changed_tool_list_once() {
        let (endpoint, _) = upstream().await;
        let client = McpProxyClient::new("tools-changed", endpoint, UpstreamAuth::None);

        client.call_tool("announce", None).await.expect("announce");
        assert!(client.take_tools_changed());
        assert!(!client.take_tools_changed());

        invalidate("tools-changed");
    }

    #[tokio::test]
    async fn rejected_credentials_carry_their_own_code_and_never_the_token() {
        let endpoint = rejecting_upstream().await;
        let client = McpProxyClient::new(
            "auth-rejected",
            endpoint,
            UpstreamAuth::bearer(TOKEN.to_string()),
        );

        let error = client.list_tools().await.expect_err("upstream rejects us");
        assert!(error.has_code(AUTH_REJECTED_CODE), "{error:?}");
        assert!(!error.message.contains(TOKEN));
        assert!(!format!("{error:?}").contains(TOKEN));
        assert!(!format!("{client:?}").contains(TOKEN));

        invalidate("auth-rejected");
    }

    /// A server at another host that counts the requests reaching it, and one
    /// at the endpoint that sends every request there.
    async fn redirecting_upstream() -> (String, Arc<AtomicUsize>) {
        let reached = Arc::new(AtomicUsize::new(0));
        let counter = reached.clone();
        let elsewhere = serve(Router::new().route(
            "/mcp",
            any(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                async { "reached" }
            }),
        ))
        .await
        .replace("127.0.0.1", "localhost");
        let endpoint = serve(Router::new().route(
            "/mcp",
            any(move || {
                let elsewhere = elsewhere.clone();
                async move {
                    (
                        axum::http::StatusCode::TEMPORARY_REDIRECT,
                        [(axum::http::header::LOCATION, elsewhere)],
                    )
                }
            }),
        ))
        .await;
        (endpoint, reached)
    }

    #[tokio::test]
    async fn a_session_does_not_follow_a_redirect_with_its_credentials() {
        let (endpoint, reached) = redirecting_upstream().await;
        let client = McpProxyClient::new(
            "no-redirects",
            endpoint,
            UpstreamAuth::header("X-Api-Key", TOKEN),
        );

        client
            .list_tools()
            .await
            .expect_err("a redirect is not a session");
        assert_eq!(reached.load(Ordering::SeqCst), 0);

        invalidate("no-redirects");
    }

    #[test]
    fn a_bare_token_goes_out_as_a_bearer_header() {
        assert_eq!(
            UpstreamAuth::header("", TOKEN),
            UpstreamAuth::Header {
                name: DEFAULT_AUTH_HEADER.to_string(),
                value: format!("Bearer {TOKEN}"),
            }
        );
        assert_eq!(
            UpstreamAuth::header("X-Api-Key", TOKEN),
            UpstreamAuth::Header {
                name: "X-Api-Key".to_string(),
                value: TOKEN.to_string(),
            }
        );
    }

    #[test]
    fn an_oversized_result_is_cut_with_a_marker() {
        let long = "x".repeat(MAX_RESULT_BYTES + 10);
        let shaped = shape_result(CallToolResult::success(vec![ContentBlock::text(long)])).result;
        assert_eq!(shaped.content.len(), 2);
        assert_eq!(shaped.content[0].text.len(), MAX_RESULT_BYTES);
        assert_eq!(shaped.content[1].text, TRUNCATION_MARKER);
    }

    #[test]
    fn the_log_names_content_it_does_not_store() {
        let image: ContentBlock = serde_json::from_value(json!({
            "type": "image",
            "data": "iVBORw0KGgo=",
            "mimeType": "image/png",
        }))
        .expect("image block");
        let call = shape_result(CallToolResult::success(vec![
            ContentBlock::text("the chart"),
            image,
        ]));

        assert_eq!(call.logged, "the chart\n[image]");
        assert!(!call.logged.contains("iVBORw0KGgo="));
        assert!(call.result.content[1].text.contains("iVBORw0KGgo="));
    }
}
