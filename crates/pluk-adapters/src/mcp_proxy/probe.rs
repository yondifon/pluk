//! What an upstream MCP server asks for before it answers.
//!
//! One request with no credentials on it settles the question. A server that
//! answers asks for nothing; a server that refuses and publishes where to sign
//! in can be signed in to; a server that refuses and publishes nothing wants a
//! token only the user can get.
//!
//! Nothing the user stored is presented here. The answer has to describe the
//! server rather than what Pluk already holds, and a credential sent to find
//! that out would be sent before anyone decided it belonged there.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use rmcp::transport::auth::AuthorizationManager;
use serde_json::{Value, json};
use tokio::time::timeout;
use upstream_http::StatusCode;
use upstream_http::header::{ACCEPT, CONTENT_TYPE, WWW_AUTHENTICATE};

use pluk_store::Integration;

use crate::error::AdapterError;

use super::catalog;
use super::client::{self, UPSTREAM_UNREACHABLE_CODE};
use super::discovery;

/// How long the probe waits before calling the server unreachable.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

const UNREACHABLE: &str = "Pluk could not reach this server. Check the URL and try again.";
const NOT_AN_MCP_SERVER: &str =
    "This server did not answer the way an MCP server does. Check the URL and try again.";

/// What a server asks of a caller before it will answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignInRequired {
    /// Nothing: it answered a request that carried no credentials.
    None,
    /// A sign-in Pluk runs in the user's browser. `registers_clients` is
    /// whether the server hands Pluk a client ID of its own.
    Oauth { registers_clients: bool },
    /// A token only the user can get.
    Token,
}

impl SignInRequired {
    pub fn as_str(self) -> &'static str {
        match self {
            SignInRequired::None => "none",
            SignInRequired::Oauth { .. } => "oauth",
            SignInRequired::Token => "token",
        }
    }
}

/// What the server this integration points at asks for.
///
/// The answer is kept per integration against the address it was found at, so
/// pointing the integration somewhere else asks the new server for itself.
pub async fn required(conn: &Integration) -> Result<SignInRequired, AdapterError> {
    let endpoint = catalog::endpoint(conn)?;
    if let Some(known) = remembered(&conn.id, &endpoint) {
        return Ok(known);
    }
    let detected = detect(&endpoint).await?;
    remember(&conn.id, &endpoint, detected);
    Ok(detected)
}

/// Ask the server again the next time anyone wants to know.
pub fn forget(integration_id: &str) {
    detected().lock().expect("mcp probe").remove(integration_id);
}

/// Whether the user has to hand this server a client ID before a sign-in can
/// start: it does not register Pluk itself, and none was configured.
pub fn needs_client_id(conn: &Integration, required: SignInRequired) -> bool {
    matches!(
        required,
        SignInRequired::Oauth {
            registers_clients: false
        }
    ) && catalog::config_str(conn, "client_id").is_none()
}

async fn detect(endpoint: &str) -> Result<SignInRequired, AdapterError> {
    let sent = client::upstream_client()?
        .post(endpoint)
        .header(ACCEPT, "application/json, text/event-stream")
        .header(CONTENT_TYPE, "application/json")
        .body(initialize().to_string())
        .send();
    let Ok(Ok(response)) = timeout(PROBE_TIMEOUT, sent).await else {
        return Err(AdapterError::new(UNREACHABLE).with_code(UPSTREAM_UNREACHABLE_CODE));
    };
    if response.status().is_success() {
        return Ok(SignInRequired::None);
    }
    if response.status() != StatusCode::UNAUTHORIZED {
        return Err(AdapterError::new(NOT_AN_MCP_SERVER));
    }
    let challenge = response
        .headers()
        .get(WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    Ok(
        match published_sign_in(endpoint, challenge.as_deref()).await {
            Some(registers_clients) => SignInRequired::Oauth { registers_clients },
            None => SignInRequired::Token,
        },
    )
}

/// Whether the server published where to sign in, and if so whether it hands
/// out client IDs. `None` when it published nothing, which is the same rule a
/// real sign-in is started by.
async fn published_sign_in(endpoint: &str, challenge: Option<&str>) -> Option<bool> {
    let manager = AuthorizationManager::new(endpoint).await.ok()?;
    let metadata = discovery::published(&manager, endpoint, challenge).await?;
    Some(metadata.registration_endpoint.is_some())
}

/// The handshake every MCP session opens with, and the cheapest request that
/// an open server answers and a guarded one refuses.
fn initialize() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": client::client_info(),
    })
}

fn remembered(integration_id: &str, endpoint: &str) -> Option<SignInRequired> {
    let held = detected().lock().expect("mcp probe");
    let (address, required) = held.get(integration_id)?;
    (address == endpoint).then_some(*required)
}

fn remember(integration_id: &str, endpoint: &str, required: SignInRequired) {
    detected()
        .lock()
        .expect("mcp probe")
        .insert(integration_id.to_string(), (endpoint.to_string(), required));
}

fn detected() -> &'static Mutex<HashMap<String, (String, SignInRequired)>> {
    static DETECTED: OnceLock<Mutex<HashMap<String, (String, SignInRequired)>>> = OnceLock::new();
    DETECTED.get_or_init(Default::default)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::Router;
    use axum::extract::State as AxumState;
    use axum::http::{HeaderMap, StatusCode as AxumStatus, header};
    use axum::response::{IntoResponse, Response};
    use axum::routing::{any, get, post};
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

    use pluk_store::{ProxyAuthInput, Store};

    use crate::adapter::Adapter;
    use crate::mcp_proxy::tests::integration;
    use crate::mcp_proxy::{McpProxyAdapter, SIGN_IN_NEEDED, TOKEN_NEEDED, oauth};

    use super::*;

    const TOKEN: &str = "super-secret-upstream-token";

    #[derive(Clone)]
    struct TestServer;

    fn schema() -> JsonObject {
        match json!({"type": "object", "properties": {"q": {"type": "string"}}}) {
            Value::Object(map) => map,
            _ => unreachable!(),
        }
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
            Ok(ListToolsResult::with_all_items(vec![Tool::new(
                "search".to_string(),
                "Search the docs".to_string(),
                Arc::new(schema()),
            )]))
        }

        async fn call_tool(
            &self,
            _request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, McpError> {
            Ok(CallToolResponse::Complete(CallToolResult::success(vec![
                ContentBlock::text("found"),
            ])))
        }
    }

    async fn serve(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = format!("http://{}", listener.local_addr().expect("addr"));
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        address
    }

    /// A server that answers anyone.
    async fn open_server() -> String {
        let service = StreamableHttpService::new(
            || Ok(TestServer),
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
        format!("{}/mcp", serve(router).await)
    }

    /// What every request a test server answered was carrying.
    #[derive(Default)]
    struct Presented {
        headers: Mutex<Vec<Option<String>>>,
    }

    impl Presented {
        fn record(&self, headers: &HeaderMap) {
            self.headers.lock().expect("presented").push(
                headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string),
            );
        }

        fn carried(&self) -> Vec<Option<String>> {
            self.headers.lock().expect("presented").clone()
        }
    }

    /// What an authorization server publishes about itself.
    fn authority_metadata(base: &str, registers_clients: bool) -> Value {
        let mut metadata = json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "response_types_supported": ["code"],
            "code_challenge_methods_supported": ["S256"],
        });
        if registers_clients {
            metadata["registration_endpoint"] = json!(format!("{base}/register"));
        }
        metadata
    }

    /// A server that refuses everyone and points at an authorization server of
    /// its own, which may or may not hand out client IDs.
    async fn guarded_server(registers_clients: bool) -> (String, Arc<Presented>) {
        let presented = Arc::new(Presented::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let resource = format!("{base}/mcp");
        let authority = base.clone();
        let router = Router::new()
            .route(
                "/.well-known/oauth-protected-resource",
                get(move || {
                    let (resource, authority) = (resource.clone(), authority.clone());
                    async move {
                        as_json(json!({
                            "resource": resource,
                            "authorization_servers": [authority],
                        }))
                    }
                }),
            )
            .route(
                "/.well-known/oauth-authorization-server",
                get(move |AxumState(base): AxumState<String>| async move {
                    as_json(authority_metadata(&base, registers_clients))
                }),
            )
            .route("/mcp", any(refuse))
            .layer(axum::Extension(presented.clone()))
            .with_state(base.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        (format!("{base}/mcp"), presented)
    }

    /// A server that refuses everyone and publishes nothing about how to get in.
    async fn closed_server() -> String {
        let router = Router::new().route(
            "/mcp",
            any(|| async {
                (
                    AxumStatus::UNAUTHORIZED,
                    [(header::WWW_AUTHENTICATE, "Bearer realm=\"docs\"")],
                    "unauthorized",
                )
            }),
        );
        format!("{}/mcp", serve(router).await)
    }

    /// A server that refuses without saying why: a 401 with no challenge on it
    /// at all, and its authorization server published where the spec says to
    /// look for one.
    async fn silent_server(registers_clients: bool) -> (String, Arc<Presented>) {
        let presented = Arc::new(Presented::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let router = Router::new()
            .route(
                "/.well-known/oauth-authorization-server",
                get(
                    move |axum::Extension(presented): axum::Extension<Arc<Presented>>,
                          headers: HeaderMap,
                          AxumState(base): AxumState<String>| async move {
                        presented.record(&headers);
                        as_json(authority_metadata(&base, registers_clients))
                    },
                ),
            )
            .route(
                "/register",
                post(|| async {
                    as_json(json!({
                        "client_id": "client-1",
                        "redirect_uris": [oauth::redirect_uri()],
                    }))
                }),
            )
            .route(
                "/mcp",
                any(
                    |axum::Extension(presented): axum::Extension<Arc<Presented>>,
                     headers: HeaderMap| async move {
                        presented.record(&headers);
                        (AxumStatus::UNAUTHORIZED, "unauthorized")
                    },
                ),
            )
            .layer(axum::Extension(presented.clone()))
            .with_state(base.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        (format!("{base}/mcp"), presented)
    }

    /// A server with no protected resource metadata to read — the path it
    /// would live at fails outright — and a working authorization server.
    async fn no_resource_metadata_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let router = Router::new()
            .route(
                "/.well-known/oauth-protected-resource/mcp",
                get(|| async { AxumStatus::INTERNAL_SERVER_ERROR }),
            )
            .route(
                "/.well-known/oauth-authorization-server",
                get(|AxumState(base): AxumState<String>| async move {
                    as_json(authority_metadata(&base, true))
                }),
            )
            .route("/mcp", any(refuse_plainly))
            .with_state(base.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        format!("{base}/mcp")
    }

    /// A server whose published token endpoint would carry the authorization
    /// code in the clear to another machine.
    async fn insecure_authority_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let router = Router::new()
            .route(
                "/.well-known/oauth-authorization-server",
                get(|AxumState(base): AxumState<String>| async move {
                    as_json(json!({
                        "issuer": base,
                        "authorization_endpoint": format!("{base}/authorize"),
                        "token_endpoint": "http://tokens.example.com/token",
                    }))
                }),
            )
            .route("/mcp", any(refuse_plainly))
            .with_state(base.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        format!("{base}/mcp")
    }

    async fn refuse_plainly() -> Response {
        (
            AxumStatus::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer realm=\"docs\"")],
            "unauthorized",
        )
            .into_response()
    }

    fn as_json(value: Value) -> Response {
        (
            [(header::CONTENT_TYPE, "application/json")],
            value.to_string(),
        )
            .into_response()
    }

    async fn refuse(
        axum::Extension(presented): axum::Extension<Arc<Presented>>,
        AxumState(base): AxumState<String>,
        headers: HeaderMap,
    ) -> Response {
        presented.record(&headers);
        (
            AxumStatus::UNAUTHORIZED,
            [(
                header::WWW_AUTHENTICATE,
                format!("Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource\""),
            )],
            "unauthorized",
        )
            .into_response()
    }

    fn store() -> (tempfile::TempDir, Arc<Store>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("pluk.db")).expect("open");
        (dir, Arc::new(store))
    }

    #[tokio::test]
    async fn a_server_that_answers_anyone_asks_for_nothing() {
        let conn = integration("probe-open", json!({ "url": open_server().await }));
        assert_eq!(required(&conn).await.expect("probe"), SignInRequired::None);
    }

    #[tokio::test]
    async fn a_server_that_publishes_its_sign_in_can_be_signed_in_to() {
        let (endpoint, _) = guarded_server(true).await;
        let conn = integration("probe-registers", json!({ "url": endpoint }));
        assert_eq!(
            required(&conn).await.expect("probe"),
            SignInRequired::Oauth {
                registers_clients: true
            }
        );
        assert!(!needs_client_id(
            &conn,
            required(&conn).await.expect("probe")
        ));
    }

    #[tokio::test]
    async fn a_sign_in_server_that_registers_nobody_needs_a_client_id_first() {
        let (endpoint, _) = guarded_server(false).await;
        let conn = integration("probe-no-registration", json!({ "url": endpoint.clone() }));
        let found = required(&conn).await.expect("probe");
        assert_eq!(
            found,
            SignInRequired::Oauth {
                registers_clients: false
            }
        );
        assert!(needs_client_id(&conn, found));

        let configured = integration(
            "probe-client-id-configured",
            json!({ "url": endpoint, "client_id": "client-1" }),
        );
        assert!(!needs_client_id(&configured, found));
    }

    #[tokio::test]
    async fn a_server_that_refuses_without_saying_why_can_still_be_signed_in_to() {
        let (registering, _) = silent_server(true).await;
        let conn = integration("probe-silent-registers", json!({ "url": registering }));
        assert_eq!(
            required(&conn).await.expect("probe"),
            SignInRequired::Oauth {
                registers_clients: true
            }
        );

        let (bare, _) = silent_server(false).await;
        let conn = integration("probe-silent-no-registration", json!({ "url": bare }));
        assert_eq!(
            required(&conn).await.expect("probe"),
            SignInRequired::Oauth {
                registers_clients: false
            }
        );
    }

    #[tokio::test]
    async fn a_server_with_no_protected_resource_metadata_can_still_be_signed_in_to() {
        let conn = integration(
            "probe-no-resource-metadata",
            json!({ "url": no_resource_metadata_server().await }),
        );
        assert_eq!(
            required(&conn).await.expect("probe"),
            SignInRequired::Oauth {
                registers_clients: true
            }
        );
    }

    #[tokio::test]
    async fn a_sign_in_that_would_travel_in_the_clear_is_not_offered() {
        let conn = integration(
            "probe-insecure-authority",
            json!({ "url": insecure_authority_server().await }),
        );
        assert_eq!(required(&conn).await.expect("probe"), SignInRequired::Token);
    }

    #[tokio::test]
    async fn the_well_known_paths_are_asked_as_a_stranger() {
        let (endpoint, presented) = silent_server(true).await;
        let conn = integration(
            "probe-silent-carries-nothing",
            json!({ "url": endpoint, "token": TOKEN }),
        );

        required(&conn).await.expect("probe");
        let carried = presented.carried();
        assert!(carried.len() > 1, "the well-known paths were asked too");
        assert!(
            carried.iter().all(Option::is_none),
            "every request presented nothing: {carried:?}"
        );
    }

    #[tokio::test]
    async fn a_sign_in_starts_against_a_server_that_refuses_without_saying_why() {
        let (endpoint, _) = silent_server(true).await;
        let conn = integration("probe-silent-sign-in", json!({ "url": endpoint.clone() }));
        let authorize_url = oauth::start(&conn).await.expect("sign-in started");
        let published = format!("{}/authorize?", endpoint.trim_end_matches("/mcp"));
        assert!(
            authorize_url.starts_with(&published),
            "the browser is sent at the published endpoint: {authorize_url}"
        );
    }

    #[tokio::test]
    async fn a_server_that_publishes_nothing_wants_a_token() {
        let conn = integration("probe-closed", json!({ "url": closed_server().await }));
        assert_eq!(required(&conn).await.expect("probe"), SignInRequired::Token);
    }

    #[tokio::test]
    async fn the_probe_carries_nothing_the_user_saved() {
        let (endpoint, presented) = guarded_server(true).await;
        let (_dir, store) = store();
        let conn = integration(
            "probe-carries-nothing",
            json!({ "url": endpoint, "token": TOKEN }),
        );
        store
            .set_proxy_auth(&ProxyAuthInput {
                integration_id: conn.id.clone(),
                kind: "oauth".to_string(),
                access_token: "access-token-abcdef".to_string(),
                refresh_token: None,
                expires_at: None,
                client_id: Some("client-1".to_string()),
                client_secret: None,
                metadata_json: None,
            })
            .expect("seed");

        required(&conn).await.expect("probe");
        let carried = presented.carried();
        assert!(!carried.is_empty(), "the server was asked");
        assert!(
            carried.iter().all(Option::is_none),
            "the probe presented nothing: {carried:?}"
        );
    }

    #[tokio::test]
    async fn the_answer_is_kept_until_it_is_asked_for_again() {
        let (endpoint, presented) = guarded_server(true).await;
        let conn = integration("probe-remembered", json!({ "url": endpoint }));

        required(&conn).await.expect("probe");
        required(&conn).await.expect("probe again");
        assert_eq!(presented.headers.lock().expect("presented").len(), 1);

        forget(&conn.id);
        required(&conn).await.expect("probe after forgetting");
        assert_eq!(presented.headers.lock().expect("presented").len(), 2);
    }

    #[tokio::test]
    async fn a_connection_test_says_what_the_server_needs() {
        let (_dir, store) = store();
        let adapter = McpProxyAdapter::new(store);
        let (guarded, _) = guarded_server(true).await;

        let sign_in = integration("probe-test-sign-in", json!({ "url": guarded }));
        let error = adapter
            .test_connection(&sign_in)
            .await
            .expect_err("nobody signed in");
        assert_eq!(error.message, SIGN_IN_NEEDED);
        assert_eq!(
            adapter.humanize_error(&error).as_deref(),
            Some(SIGN_IN_NEEDED)
        );

        let token = integration("probe-test-token", json!({ "url": closed_server().await }));
        let error = adapter
            .test_connection(&token)
            .await
            .expect_err("no token saved");
        assert_eq!(error.message, TOKEN_NEEDED);
        assert_eq!(
            adapter.humanize_error(&error).as_deref(),
            Some(TOKEN_NEEDED)
        );

        let open = integration("probe-test-open", json!({ "url": open_server().await }));
        adapter.test_connection(&open).await.expect("open server");
        client::invalidate(&open.id);
    }
}
