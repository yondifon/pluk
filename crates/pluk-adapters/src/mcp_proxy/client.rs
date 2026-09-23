//! The client Pluk points at an upstream MCP server.
//!
//! Pluk's own MCP surface is stateless: it is rebuilt from the store on every
//! protocol request. Tying an upstream session to one of those requests would
//! re-handshake with the upstream server on every tool call, so the session
//! lives here instead — one per integration, connected on first use, shared by
//! every handler, and dropped only when the integration's config or
//! credentials change.
//!
//! A local server lives in the same place: its session is the process, so
//! the pool starts it on first use, notices when it exits, holds it after
//! [`MAX_STARTS`] starts within [`CRASH_WINDOW`], and stops it for good on
//! [`shutdown`]. Listing an integration never starts one.
//!
//! Credentials are passed in by the caller. Nothing here reads the store, and
//! no token reaches a log line, an error message, or a `Debug` rendering.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

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
use serde::Serialize;
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::timeout;

use crate::error::AdapterError;
use crate::gate::{TextContent, ToolResult};

use super::child;
use super::transport::{LaunchSpec, StaticHeader, UpstreamTransport};

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
/// The local server exited too often, and waits for the user to restart it.
pub const SERVER_CRASHED_CODE: &str = "MCP_PROXY_SERVER_CRASHED";
/// The user stopped the local server, and it waits for them to start it.
pub const SERVER_STOPPED_CODE: &str = "MCP_PROXY_SERVER_STOPPED";

const SERVER_CRASHED: &str =
    "This server keeps stopping. Check its output in Pluk, then restart it.";
const SERVER_STOPPED: &str = "This server is stopped. Start it again in Pluk.";

/// Header a bare token is sent in when the integration names no other.
pub const DEFAULT_AUTH_HEADER: &str = "Authorization";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// A local server's first start can download its packages, as `npx -y` does.
const STDIO_CONNECT_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a local server gets to exit once its stdin closes. rmcp waits as
/// long before it kills the server itself.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);
const CLOSE_GRACE: Duration = Duration::from_millis(500);
const CLOSE_POLL: Duration = Duration::from_millis(25);
/// A local server started this many times within the window is held until
/// the user restarts it.
const MAX_STARTS: usize = 3;
const CRASH_WINDOW: Duration = Duration::from_secs(60);
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
/// integration id reuses it. For a local server, that first call is also what
/// starts it.
#[derive(Debug, Clone)]
pub struct McpProxyClient {
    integration_id: String,
    transport: UpstreamTransport,
}

impl McpProxyClient {
    pub fn new(
        integration_id: impl Into<String>,
        endpoint: impl Into<String>,
        auth: UpstreamAuth,
    ) -> Self {
        McpProxyClient {
            integration_id: integration_id.into(),
            transport: UpstreamTransport::Http {
                endpoint: endpoint.into(),
                auth,
                headers: Vec::new(),
            },
        }
    }

    /// A client for a local server Pluk starts from `spec`. The caller has
    /// already checked the user approved this exact launch.
    pub fn stdio(integration_id: impl Into<String>, spec: LaunchSpec) -> Self {
        McpProxyClient {
            integration_id: integration_id.into(),
            transport: UpstreamTransport::Stdio(spec),
        }
    }

    /// Headers sent on every request on top of the sign-in. Where one names
    /// the header the sign-in uses, the sign-in wins. A local server takes
    /// none.
    pub fn with_headers(mut self, headers: Vec<StaticHeader>) -> Self {
        if let UpstreamTransport::Http { headers: held, .. } = &mut self.transport {
            *held = headers;
        }
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

    /// The open session, or a new one. A session whose transport closed under
    /// it, such as a local server that exited, is not reused.
    async fn session(&self) -> Result<Arc<Upstream>, AdapterError> {
        let slot = slot(&self.integration_id);
        let mut held = slot.session.lock().await;
        if let Some(upstream) = held.as_ref() {
            if !upstream.is_closed() && !upstream.is_transport_closed() {
                return Ok(upstream.clone());
            }
            *held = None;
            slot.lost_server();
        }
        let upstream = Arc::new(self.connect(&slot).await?);
        *held = Some(upstream.clone());
        Ok(upstream)
    }

    async fn drop_session(&self) {
        let slot = slot(&self.integration_id);
        if slot.session.lock().await.take().is_some() {
            slot.lost_server();
        }
    }

    async fn connect(&self, slot: &Slot) -> Result<Upstream, AdapterError> {
        let handler = ProxyHandler {
            tools_changed: slot.tools_changed.clone(),
        };
        match &self.transport {
            UpstreamTransport::Http {
                endpoint,
                auth,
                headers,
            } => connect_http(endpoint, auth, headers, handler).await,
            UpstreamTransport::Stdio(spec) => connect_stdio(spec, slot, handler).await,
        }
    }

    /// How an error names the server: the address it is reached at, or the
    /// file name of the program that runs it.
    fn server(&self) -> String {
        match &self.transport {
            UpstreamTransport::Http { endpoint, .. } => format!("the MCP server at {endpoint}"),
            UpstreamTransport::Stdio(spec) => format!("the local MCP server ({})", spec.label()),
        }
    }

    fn timed_out(&self) -> AdapterError {
        AdapterError::new(format!("{} did not answer", self.server()))
            .with_code(UPSTREAM_TIMEOUT_CODE)
    }

    fn failed(&self, error: &ServiceError) -> AdapterError {
        let message = format!("{} failed: {error}", self.server());
        match failure_code(error) {
            Some(code) => AdapterError::new(message).with_code(code),
            None => AdapterError::new(message),
        }
    }
}

async fn connect_http(
    endpoint: &str,
    auth: &UpstreamAuth,
    headers: &[StaticHeader],
    handler: ProxyHandler,
) -> Result<Upstream, AdapterError> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(endpoint.to_string());
    let oauth = matches!(auth, UpstreamAuth::Bearer { .. });
    for header in headers {
        // rmcp adds the OAuth token on its own, next to these rather than
        // in place of them.
        if oauth && header.name == AUTHORIZATION {
            continue;
        }
        config
            .custom_headers
            .insert(header.name.clone(), header.header_value()?);
    }
    match auth {
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
    match timeout(CONNECT_TIMEOUT, serve_client(handler, transport)).await {
        Err(_) => Err(
            AdapterError::new(format!("the MCP server at {endpoint} did not answer"))
                .with_code(UPSTREAM_TIMEOUT_CODE),
        ),
        Ok(Ok(upstream)) => Ok(upstream),
        Ok(Err(error)) => Err(AdapterError::new(format!(
            "could not connect to the MCP server at {endpoint}: {error}"
        ))
        .with_code(failure_code(&error).unwrap_or(UPSTREAM_UNREACHABLE_CODE))),
    }
}

/// Start the local server and open a session on it. Every start counts
/// toward the crash limit, and a server that does not get as far as a
/// session is stopped and counts as crashed.
async fn connect_stdio(
    spec: &LaunchSpec,
    slot: &Slot,
    handler: ProxyHandler,
) -> Result<Upstream, AdapterError> {
    slot.claim_start()?;
    let label = spec.label();
    let (transport, pgid, stderr) = child::spawn(spec).map_err(|error| {
        slot.local.lock().expect("local server").crashed = true;
        AdapterError::new(format!("Pluk could not start {label}: {error}"))
            .with_code(UPSTREAM_UNREACHABLE_CODE)
    })?;
    child::drain(stderr, spec.secret_values(), slot.output.clone());
    slot.local.lock().expect("local server").pgid = Some(pgid);
    match timeout(STDIO_CONNECT_TIMEOUT, serve_client(handler, transport)).await {
        Ok(Ok(upstream)) => Ok(upstream),
        Err(_) => {
            slot.lost_server();
            Err(
                AdapterError::new(format!("the local MCP server ({label}) did not answer"))
                    .with_code(UPSTREAM_TIMEOUT_CODE),
            )
        }
        Ok(Err(_)) => {
            slot.lost_server();
            Err(AdapterError::new(format!(
                "the local MCP server ({label}) stopped before it was ready"
            ))
            .with_code(UPSTREAM_UNREACHABLE_CODE))
        }
    }
}

/// Close one integration's session and forget everything held for it. A
/// local server is stopped: closed, given [`CLOSE_TIMEOUT`] to exit, then its
/// whole process group is killed. The next call starts over with whatever
/// config and credentials are current by then.
///
/// Returns at once. The server is stopped on the runtime when there is one,
/// and killed on the spot when there is not.
pub fn shutdown(integration_id: &str) {
    let Some(slot) = pool()
        .lock()
        .expect("mcp proxy pool")
        .remove(integration_id)
    else {
        return;
    };
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => {
            runtime.spawn(async move { slot.close().await });
        }
        Err(_) => slot.kill(),
    }
}

/// Stop every local server and close every session, for when Pluk quits.
/// Whatever is still alive afterwards is killed synchronously, so nothing
/// outlives Pluk waiting on a runtime that is gone.
pub async fn shutdown_all() {
    let slots: Vec<Arc<Slot>> = pool()
        .lock()
        .expect("mcp proxy pool")
        .drain()
        .map(|(_, slot)| slot)
        .collect();
    let closing = slots.iter().map(|slot| slot.close());
    let _ = timeout(
        CLOSE_TIMEOUT + CLOSE_GRACE,
        futures::future::join_all(closing),
    )
    .await;
    for slot in &slots {
        slot.kill();
    }
}

/// Stop the local server and keep it stopped: calls are refused until the
/// user starts it again with [`restart`], or its config changes.
pub async fn stop(integration_id: &str) {
    let slot = slot(integration_id);
    slot.local.lock().expect("local server").held = Some(Held::Stopped);
    slot.close().await;
}

/// Stop the local server if it runs and clear what kept it from starting:
/// a stop, or the crash limit. The next call starts it.
pub async fn restart(integration_id: &str) {
    let slot = slot(integration_id);
    slot.close().await;
    let mut local = slot.local.lock().expect("local server");
    local.held = None;
    local.starts.clear();
    local.crashed = false;
}

/// Whether the local server runs, and its pid while it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub state: ServerState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerState {
    /// Started, and not yet ready for a session.
    Starting,
    Running,
    /// Not started yet, or stopped by the user or by Pluk.
    Stopped,
    /// It exited without Pluk stopping it, or kept doing so and is held.
    Crashed,
}

pub fn status(integration_id: &str) -> ServerStatus {
    let stopped = ServerStatus {
        state: ServerState::Stopped,
        pid: None,
    };
    let Some(slot) = existing_slot(integration_id) else {
        return stopped;
    };
    let local = slot.local.lock().expect("local server");
    let session = match slot.session.try_lock() {
        Ok(session) => session,
        Err(_) => {
            return ServerStatus {
                state: ServerState::Starting,
                pid: local.pgid,
            };
        }
    };
    match session.as_ref() {
        Some(upstream) if !upstream.is_transport_closed() => ServerStatus {
            state: ServerState::Running,
            pid: local.pgid,
        },
        Some(_) => ServerStatus {
            state: ServerState::Crashed,
            pid: None,
        },
        None if local.crashed || local.held == Some(Held::CrashLoop) => ServerStatus {
            state: ServerState::Crashed,
            pid: None,
        },
        None => stopped,
    }
}

/// The last lines the local server printed to stderr, secrets scrubbed.
pub fn output(integration_id: &str) -> Vec<String> {
    existing_slot(integration_id)
        .map(|slot| slot.output.lock().expect("server output").lines())
        .unwrap_or_default()
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
/// reconnect does not swallow an announcement that arrived just before it,
/// and the output and crash count outlive it so a server that keeps exiting
/// can be seen and held.
struct Slot {
    tools_changed: Arc<AtomicBool>,
    session: AsyncMutex<Option<Arc<Upstream>>>,
    local: Mutex<Local>,
    output: Arc<Mutex<child::Output>>,
}

/// What the pool knows about a local server beyond its session.
#[derive(Default)]
struct Local {
    /// The server's process group while Pluk may still have to kill it.
    pgid: Option<u32>,
    /// When the server was started, within the crash window.
    starts: VecDeque<Instant>,
    /// Why no call may start the server until the user restarts it.
    held: Option<Held>,
    /// The last server exited, or failed to start, without Pluk stopping it.
    crashed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    Stopped,
    CrashLoop,
}

impl Slot {
    fn new() -> Self {
        Slot {
            tools_changed: Arc::new(AtomicBool::new(false)),
            session: AsyncMutex::new(None),
            local: Mutex::new(Local::default()),
            output: Arc::default(),
        }
    }

    /// Count one start of the local server, or refuse it: the user stopped
    /// it, or it already started [`MAX_STARTS`] times within [`CRASH_WINDOW`].
    fn claim_start(&self) -> Result<(), AdapterError> {
        let mut local = self.local.lock().expect("local server");
        match local.held {
            Some(Held::Stopped) => {
                return Err(AdapterError::new(SERVER_STOPPED).with_code(SERVER_STOPPED_CODE));
            }
            Some(Held::CrashLoop) => {
                return Err(AdapterError::new(SERVER_CRASHED).with_code(SERVER_CRASHED_CODE));
            }
            None => {}
        }
        let now = Instant::now();
        while local
            .starts
            .front()
            .is_some_and(|start| now.duration_since(*start) >= CRASH_WINDOW)
        {
            local.starts.pop_front();
        }
        if local.starts.len() >= MAX_STARTS {
            local.held = Some(Held::CrashLoop);
            return Err(AdapterError::new(SERVER_CRASHED).with_code(SERVER_CRASHED_CODE));
        }
        local.starts.push_back(now);
        Ok(())
    }

    /// The session ended without Pluk ending it. Whatever the server left
    /// running in its group is killed.
    fn lost_server(&self) {
        let mut local = self.local.lock().expect("local server");
        if let Some(pgid) = local.pgid.take() {
            local.crashed = true;
            pluk_core::platform::kill_process_group(pgid);
        }
    }

    /// Close the session. A local server is closed, which ends its stdin and
    /// gives it [`CLOSE_TIMEOUT`] to exit, and then its group is killed.
    async fn close(&self) {
        let upstream = self.session.lock().await.take();
        if let Some(upstream) = upstream {
            upstream.cancellation_token().cancel();
            let closed = async {
                while !upstream.is_transport_closed() {
                    tokio::time::sleep(CLOSE_POLL).await;
                }
            };
            let _ = timeout(CLOSE_TIMEOUT + CLOSE_GRACE, closed).await;
        }
        self.local.lock().expect("local server").crashed = false;
        self.kill();
    }

    /// Kill the local server's group on the spot.
    fn kill(&self) {
        if let Some(pgid) = self.local.lock().expect("local server").pgid.take() {
            pluk_core::platform::kill_process_group(pgid);
        }
    }
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
        .or_insert_with(|| Arc::new(Slot::new()))
        .clone()
}

fn existing_slot(integration_id: &str) -> Option<Arc<Slot>> {
    pool()
        .lock()
        .expect("mcp proxy pool")
        .get(integration_id)
        .cloned()
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
pub(super) mod tests {
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
    use crate::mcp_proxy::transport::{RowText, Secret};

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

        shutdown("lists-and-calls");
    }

    #[tokio::test]
    async fn reuses_one_session_until_it_is_shut_down() {
        let (endpoint, sessions) = upstream().await;
        let client = McpProxyClient::new("reuses-session", endpoint, UpstreamAuth::None);

        client.list_tools().await.expect("first list");
        client.list_tools().await.expect("second list");
        assert_eq!(sessions.load(Ordering::SeqCst), 1);

        shutdown("reuses-session");
        client.list_tools().await.expect("list after shutdown");
        assert_eq!(sessions.load(Ordering::SeqCst), 2);

        shutdown("reuses-session");
    }

    #[tokio::test]
    async fn reports_a_changed_tool_list_once() {
        let (endpoint, _) = upstream().await;
        let client = McpProxyClient::new("tools-changed", endpoint, UpstreamAuth::None);

        client.call_tool("announce", None).await.expect("announce");
        assert!(client.take_tools_changed());
        assert!(!client.take_tools_changed());

        shutdown("tools-changed");
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

        shutdown("auth-rejected");
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

        shutdown("no-redirects");
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
    /// A local MCP server in `/bin/sh`: it answers `initialize` with the
    /// version it was asked for and lists one tool. It records its pid and
    /// environment under `$STUB_STATE`, prints `$STUB_TOKEN` to stderr, and
    /// leaves a `sleep` running in its group whose pid it records too.
    pub(in crate::mcp_proxy) const STUB: &str = r#"
echo $$ > "$STUB_STATE/pid"
env > "$STUB_STATE/env"
sleep 300 </dev/null >/dev/null 2>&1 &
echo $! > "$STUB_STATE/grandchild"
echo "starting with token $STUB_TOKEN" >&2
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      version=$(printf '%s' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"stub","version":"1"}}}\n' "$id" "$version"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"Echo","inputSchema":{"type":"object"}}]}}\n' "$id"
      ;;
  esac
done
"#;

    const STUB_TOKEN: &str = "stub-secret-token-1";

    /// A server that stops as soon as it starts, the way one missing a module
    /// does.
    const EXITING: &str = "echo 'Cannot find module' >&2\nexit 1\n";

    fn launch(dir: &std::path::Path, body: &str) -> LaunchSpec {
        let script = dir.join("server.sh");
        std::fs::write(&script, body).unwrap();
        LaunchSpec {
            program: "/bin/sh".into(),
            args: vec![script.to_string_lossy().into_owned()],
            cwd: dir.to_path_buf(),
            env: vec![
                (
                    "STUB_STATE".to_string(),
                    RowText::Plain(dir.to_string_lossy().into_owned()),
                ),
                (
                    "STUB_TOKEN".to_string(),
                    RowText::Secret(Secret::new(STUB_TOKEN)),
                ),
            ],
            path: pluk_core::shell_env::fallback_path(),
        }
    }

    pub(in crate::mcp_proxy) async fn recorded(dir: &std::path::Path, name: &str) -> u32 {
        for _ in 0..200 {
            if let Ok(text) = std::fs::read_to_string(dir.join(name))
                && let Ok(pid) = text.trim().parse()
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the server never recorded {name}");
    }

    pub(in crate::mcp_proxy) fn alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    pub(in crate::mcp_proxy) async fn gone(pid: u32) -> bool {
        for _ in 0..200 {
            if !alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        false
    }

    #[tokio::test]
    async fn a_local_server_starts_on_first_use_with_only_the_environment_it_was_given() {
        let dir = tempfile::tempdir().unwrap();
        let id = "local-starts";
        let client = McpProxyClient::stdio(id, launch(dir.path(), STUB));
        assert_eq!(status(id).state, ServerState::Stopped);

        let tools = client.list_tools().await.expect("list tools");
        assert_eq!(tools[0].name, "echo");
        let pid = recorded(dir.path(), "pid").await;
        assert_eq!(
            status(id),
            ServerStatus {
                state: ServerState::Running,
                pid: Some(pid),
            }
        );

        let env = std::fs::read_to_string(dir.path().join("env")).unwrap();
        let names: Vec<&str> = env
            .lines()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name))
            .collect();
        assert!(names.contains(&"HOME"), "{names:?}");
        assert!(names.contains(&"PATH"), "{names:?}");
        assert!(names.contains(&"STUB_TOKEN"), "{names:?}");
        assert!(
            !names.iter().any(|name| name.starts_with("CARGO")),
            "Pluk's own environment reached the server: {names:?}"
        );

        shutdown(id);
        assert!(gone(pid).await, "the server outlived shutdown");
    }

    #[tokio::test]
    async fn stopping_a_local_server_kills_what_it_started_and_keeps_it_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let id = "local-stops";
        let client = McpProxyClient::stdio(id, launch(dir.path(), STUB));
        client.list_tools().await.expect("start");
        let pid = recorded(dir.path(), "pid").await;
        let grandchild = recorded(dir.path(), "grandchild").await;
        assert!(alive(grandchild));

        stop(id).await;
        assert!(gone(pid).await, "the server survived a stop");
        assert!(gone(grandchild).await, "what the server started survived");
        assert_eq!(status(id).state, ServerState::Stopped);
        let refused = client.list_tools().await.expect_err("stopped");
        assert!(refused.has_code(SERVER_STOPPED_CODE), "{refused:?}");

        restart(id).await;
        client.list_tools().await.expect("started again");
        shutdown(id);
        let restarted = recorded(dir.path(), "pid").await;
        assert!(gone(restarted).await, "the server survived shutdown");
    }

    #[tokio::test]
    async fn a_server_that_keeps_exiting_is_held_until_it_is_restarted() {
        let dir = tempfile::tempdir().unwrap();
        let id = "local-crashes";
        let spec = launch(dir.path(), EXITING);
        let script = spec.args[0].clone();
        let client = McpProxyClient::stdio(id, spec);

        for _ in 0..MAX_STARTS {
            let error = client.list_tools().await.expect_err("exits at once");
            assert!(error.has_code(UPSTREAM_UNREACHABLE_CODE), "{error:?}");
            assert!(error.message.contains("(sh)"), "{}", error.message);
            assert!(!error.message.contains(&script), "{}", error.message);
        }
        assert_eq!(status(id).state, ServerState::Crashed);
        let held = client.list_tools().await.expect_err("held");
        assert!(held.has_code(SERVER_CRASHED_CODE), "{held:?}");
        assert!(output(id).iter().any(|line| line == "Cannot find module"));

        restart(id).await;
        let again = client.list_tools().await.expect_err("starts, and exits");
        assert!(again.has_code(UPSTREAM_UNREACHABLE_CODE), "{again:?}");
        shutdown(id);
    }

    #[tokio::test]
    async fn the_output_keeps_what_the_server_printed_with_secrets_scrubbed() {
        let dir = tempfile::tempdir().unwrap();
        let id = "local-output";
        let client = McpProxyClient::stdio(id, launch(dir.path(), STUB));
        client.list_tools().await.expect("start");

        let mut lines = Vec::new();
        for _ in 0..100 {
            lines = output(id);
            if !lines.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(lines, ["starting with token <redacted>"]);
        assert!(!format!("{client:?}").contains(STUB_TOKEN));
        assert!(!format!("{client:?}").contains("server.sh"));
        shutdown(id);
    }
}
