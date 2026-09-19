//! Signing in to an upstream MCP server on the user's behalf.
//!
//! The user starts the sign-in in Pluk, approves it in their own browser, and
//! the browser lands back on Pluk's loopback server. Tokens live in
//! `proxy_auth` and never leave this module: a caller gets a bearer string and
//! nothing else.
//!
//! rmcp's `auth` module drives the protocol — metadata discovery, client
//! registration, PKCE, the code exchange and the refresh. The redirect, the
//! authorizations waiting on the browser, and the stored tokens stay here,
//! because the store is what decides when a sign-in is spent.
//!
//! Agents connecting to Pluk see none of this. They call a proxied tool and
//! either reach upstream or are told, in one sentence, to sign in again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rmcp::transport::auth::{
    AuthError, AuthorizationManager, AuthorizationMetadata, AuthorizationRequest,
    AuthorizationSession, CredentialStore, InMemoryCredentialStore, OAuthClientConfig,
    OAuthTokenResponse, StoredCredentials,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex as AsyncMutex;
use upstream_http::Url;
use upstream_http::header::WWW_AUTHENTICATE;

use pluk_store::{
    AuthStatus, Integration, ProxyAuth, ProxyAuthInput, RefreshedTokens, Store, StoreError,
};

use crate::error::AdapterError;

use super::catalog;
use super::client::{self, UPSTREAM_UNREACHABLE_CODE, UpstreamAuth};

/// The stored sign-in is spent and only the user can renew it.
pub const RECONNECT_NEEDED_CODE: &str = "MCP_PROXY_RECONNECT_NEEDED";

const RECONNECT_NEEDED: &str =
    "Pluk needs you to sign in to this MCP server again. Open Pluk to reconnect.";
const NO_SIGN_IN: &str = "This server does not offer sign-in. Add a token instead.";
const NEEDS_CLIENT_ID: &str = "This server needs a client ID. Add the one it gave you.";
const SIGN_IN_FAILED: &str = "Pluk could not finish signing in to this server. Try again.";
const SIGN_IN_UNREACHABLE: &str =
    "Pluk could not reach the sign-in for this server. Check your connection and try again.";

/// Where the browser lands once the user approves.
pub const CALLBACK_PATH: &str = "/oauth/mcp/callback";

/// The name Pluk registers itself under when a server hands out client IDs.
const CLIENT_NAME: &str = "Pluk";

/// The authentication style a stored row records.
const KIND: &str = "oauth";

/// How long an authorization waiting on the browser stays usable.
const PENDING_TTL: Duration = Duration::from_secs(10 * 60);

/// How far ahead of expiry a token is renewed, so a call in flight does not
/// land on one that just ran out.
const EXPIRY_SKEW_MS: i64 = 60_000;

/// How this integration signs in to its upstream server, and whether that
/// sign-in still works. Carries no credential.
pub struct SignInState {
    pub kind: &'static str,
    pub status: &'static str,
}

/// The address the browser is sent back to. Pluk serves it itself, on the same
/// loopback port everything else is reached on.
pub fn redirect_uri() -> String {
    format!(
        "http://127.0.0.1:{}{CALLBACK_PATH}",
        pluk_core::loopback::port()
    )
}

/// Begin a sign-in and hand back the address the user's browser has to open.
///
/// Nothing is stored until the browser comes back: what this leaves behind is
/// one pending authorization, in memory, good for ten minutes.
pub async fn start(conn: &Integration) -> Result<String, AdapterError> {
    let endpoint = catalog::endpoint(conn)?;
    let redirect_uri = redirect_uri();

    let mut manager = AuthorizationManager::new(&endpoint)
        .await
        .map_err(|_| unreachable_error())?;
    let resolution = manager
        .resolve_metadata_from_challenge(challenge(&endpoint).await.as_deref())
        .await
        .map_err(|_| unreachable_error())?;
    // Endpoints the server never published are guesses, and sending the user's
    // browser at a guess is worse than saying the server does not do this.
    if !resolution.source.is_discovered() {
        return Err(AdapterError::new(NO_SIGN_IN));
    }
    let metadata = resolution.metadata.clone();
    manager.set_metadata(resolution.metadata);

    let scopes = manager.select_scopes(None, &[]);
    let (client_id, client_secret) =
        client_identity(conn, &mut manager, &metadata, &redirect_uri, &scopes).await?;

    let mut request = AuthorizationRequest::new(&redirect_uri)
        .with_scopes(scopes)
        .with_preregistered_client(&client_id);
    if let Some(secret) = &client_secret {
        request = request.with_client_secret(secret);
    }
    let session = AuthorizationSession::new(manager, request)
        .await
        .map_err(|_| sign_in_failed())?;

    let authorize_url = session.get_authorization_url().to_string();
    let (state, resource) = authorize_params(&authorize_url, &endpoint)?;
    remember(
        state,
        Pending {
            integration_id: conn.id.clone(),
            session,
            client_id,
            client_secret,
            context: AuthContext { metadata, resource },
            started: Instant::now(),
        },
    );
    Ok(authorize_url)
}

/// Finish a sign-in from the address the browser was redirected to.
///
/// A redirect whose state is unknown, already spent, or older than ten minutes
/// is refused and changes nothing.
pub async fn complete(store: &Store, callback_url: &str) -> Result<(), AdapterError> {
    let (code, state, issuer) = callback_params(callback_url)?;
    let pending = claim(&state).ok_or_else(sign_in_failed)?;
    let tokens = pending
        .session
        .handle_callback_with_issuer(&code, &state, issuer.as_deref())
        .await
        .map_err(|_| sign_in_failed())?;

    store
        .set_proxy_auth(&stored_from(&pending, &tokens)?)
        .map_err(store_failure)?;
    client::invalidate(&pending.integration_id);

    // The tool list is read now rather than on whatever call comes first. A
    // server that answers the token endpoint but not this one is still signed
    // in, and the settings screen refreshes the list on its own.
    if let Ok(Some(conn)) = store.integration_by_id(&pending.integration_id) {
        let _ = catalog::discover(store, &conn).await;
    }
    Ok(())
}

/// The access token for a stored sign-in, renewed when it is about to run out.
/// `None` when the user has not signed in to the server this integration now
/// points at.
pub async fn bearer(store: &Store, conn: &Integration) -> Result<Option<String>, AdapterError> {
    let Some(stored) = signed_in_here(store, conn)? else {
        return Ok(None);
    };
    if stored.status == AuthStatus::ReconnectNeeded {
        return Err(reconnect_error());
    }
    if !is_expiring(&stored) {
        return Ok(Some(stored.access_token));
    }
    renewed_token(store, conn, &stored.access_token)
        .await
        .map(Some)
}

/// Renew a stored sign-in after upstream refused the token Pluk sent.
/// `false` when there is no stored sign-in to renew.
pub async fn renew(store: &Store, conn: &Integration) -> Result<bool, AdapterError> {
    let Some(stored) = signed_in_here(store, conn)? else {
        return Ok(false);
    };
    renewed_token(store, conn, &stored.access_token).await?;
    Ok(true)
}

/// Forget a stored sign-in.
pub fn disconnect(store: &Store, integration_id: &str) -> Result<(), AdapterError> {
    store
        .delete_proxy_auth(integration_id)
        .map_err(store_failure)?;
    client::invalidate(integration_id);
    Ok(())
}

/// Mark a stored sign-in as spent and say so. Called when the server will not
/// renew it, which no retry can fix.
pub fn require_sign_in(store: &Store, integration_id: &str) -> AdapterError {
    match store.set_proxy_auth_status(integration_id, AuthStatus::ReconnectNeeded) {
        Ok(_) => {
            client::invalidate(integration_id);
            reconnect_error()
        }
        Err(error) => store_failure(error),
    }
}

/// What the settings screen renders for one integration's sign-in.
pub fn sign_in_state(store: &Store, conn: &Integration) -> Result<SignInState, AdapterError> {
    if let Some(stored) = signed_in_here(store, conn)? {
        return Ok(SignInState {
            kind: KIND,
            status: stored.status.as_str(),
        });
    }
    Ok(match catalog::static_auth(conn) {
        UpstreamAuth::None => SignInState {
            kind: "none",
            status: "not_connected",
        },
        _ => SignInState {
            kind: "token",
            status: "connected",
        },
    })
}

/// One refresh at a time per integration. Whoever loses the race re-reads the
/// row and uses the token the winner stored.
async fn renewed_token(
    store: &Store,
    conn: &Integration,
    spent: &str,
) -> Result<String, AdapterError> {
    let integration_id = conn.id.as_str();
    let lock = refresh_lock(integration_id);
    let _held = lock.lock().await;

    let Some(stored) = signed_in_here(store, conn)? else {
        return Err(reconnect_error());
    };
    if stored.status == AuthStatus::ReconnectNeeded {
        return Err(reconnect_error());
    }
    if stored.access_token != spent {
        return Ok(stored.access_token);
    }
    let Some(refresh_token) = stored.refresh_token.clone() else {
        return Err(require_sign_in(store, integration_id));
    };

    let context = context_of(&stored)?;
    let manager = refresh_manager(&stored, &context, &refresh_token).await?;
    let response = match manager.refresh_token().await {
        Ok(response) => response,
        Err(AuthError::TokenRefreshRejected(_) | AuthError::AuthorizationRequired) => {
            return Err(require_sign_in(store, integration_id));
        }
        Err(_) => return Err(unreachable_error()),
    };

    let fresh = Tokens::read(&response)?;
    let landed = store
        .update_proxy_tokens(
            integration_id,
            stored.version,
            &RefreshedTokens {
                access_token: fresh.access_token.clone(),
                refresh_token: fresh.refresh_token,
                expires_at: fresh.expires_at,
            },
        )
        .map_err(store_failure)?;
    client::invalidate(integration_id);
    if landed {
        return Ok(fresh.access_token);
    }
    match stored_auth(store, integration_id)? {
        Some(winner) => Ok(winner.access_token),
        None => Err(reconnect_error()),
    }
}

/// The stored sign-in, but only when it was made for the address this
/// integration points at now.
///
/// A sign-in is issued for one server. Editing the URL afterwards must not
/// hand the next server what the last one granted, and that holds for the
/// refresh token too. The row is left alone: putting the old address back
/// signs the user straight back in.
fn signed_in_here(store: &Store, conn: &Integration) -> Result<Option<ProxyAuth>, AdapterError> {
    let Some(stored) = stored_auth(store, &conn.id)? else {
        return Ok(None);
    };
    let issued_for = context_of(&stored)
        .ok()
        .and_then(|context| Url::parse(&context.resource).ok());
    let address = catalog::endpoint(conn)
        .ok()
        .and_then(|address| Url::parse(&address).ok());
    Ok(match (issued_for, address) {
        (Some(issued_for), Some(address)) if covers(&issued_for, &address) => Some(stored),
        _ => None,
    })
}

/// Whether a resource indicator names the address in question.
///
/// This is the rule the indicator was accepted by when the sign-in discovered
/// it: same origin, and a path the address sits under, because a server may
/// publish one resource for a whole tree of endpoints.
fn covers(resource: &Url, address: &Url) -> bool {
    if resource.scheme() != address.scheme()
        || resource.host_str() != address.host_str()
        || resource.port_or_known_default() != address.port_or_known_default()
    {
        return false;
    }
    let (held, wanted) = (resource.path(), address.path());
    wanted == held
        || (wanted.starts_with(held)
            && (held.ends_with('/') || wanted.as_bytes().get(held.len()) == Some(&b'/')))
}

/// A manager built from the stored row alone, so a refresh costs no discovery.
///
/// Its base URL is the resource the tokens were issued for, which is what rmcp
/// sends as the RFC 8707 `resource` indicator when nothing was discovered — so
/// the refresh carries exactly the indicator the sign-in did.
async fn refresh_manager(
    stored: &ProxyAuth,
    context: &AuthContext,
    refresh_token: &str,
) -> Result<AuthorizationManager, AdapterError> {
    let client_id = stored.client_id.clone().ok_or_else(sign_in_failed)?;
    let mut manager = AuthorizationManager::new(&context.resource)
        .await
        .map_err(|_| sign_in_failed())?;
    manager.set_metadata(context.metadata.clone());

    let mut config = OAuthClientConfig::new(client_id.clone(), redirect_uri());
    if let Some(secret) = &stored.client_secret {
        config = config.with_client_secret(secret.clone());
    }
    manager
        .configure_client(config)
        .map_err(|_| sign_in_failed())?;

    let credentials = InMemoryCredentialStore::new();
    credentials
        .save(StoredCredentials::new(
            client_id,
            Some(seed_response(&stored.access_token, refresh_token)?),
            Vec::new(),
            None,
        ))
        .await
        .map_err(|_| sign_in_failed())?;
    manager.set_credential_store(credentials);
    Ok(manager)
}

/// The client Pluk presents itself as: the one the user configured, or one the
/// server hands out.
///
/// Registering here rather than leaving it to [`AuthorizationSession::new`] is
/// what keeps the secret a server may issue, which the next refresh needs and
/// the session does not expose.
async fn client_identity(
    conn: &Integration,
    manager: &mut AuthorizationManager,
    metadata: &AuthorizationMetadata,
    redirect_uri: &str,
    scopes: &[String],
) -> Result<(String, Option<String>), AdapterError> {
    if let Some(client_id) = catalog::config_str(conn, "client_id") {
        return Ok((client_id, catalog::config_str(conn, "client_secret")));
    }
    if metadata.registration_endpoint.is_none() {
        return Err(AdapterError::new(NEEDS_CLIENT_ID));
    }
    let scope_refs: Vec<&str> = scopes.iter().map(String::as_str).collect();
    let registered = manager
        .register_client(CLIENT_NAME, redirect_uri, &scope_refs)
        .await
        .map_err(|_| sign_in_failed())?;
    Ok((registered.client_id, registered.client_secret))
}

/// The `WWW-Authenticate` an unauthenticated request draws. It points
/// discovery straight at the server's own metadata and carries the scopes it
/// wants, so it is worth one request; a server that answers anything else just
/// leaves discovery to probe the well-known paths.
async fn challenge(endpoint: &str) -> Option<String> {
    let response = client::upstream_client()
        .ok()?
        .get(endpoint)
        .send()
        .await
        .ok()?;
    response
        .headers()
        .get(WWW_AUTHENTICATE)?
        .to_str()
        .ok()
        .map(str::to_string)
}

/// What a later refresh needs and cannot rebuild from the row alone.
#[derive(Serialize, Deserialize)]
struct AuthContext {
    metadata: AuthorizationMetadata,
    /// The RFC 8707 resource indicator the tokens were issued for.
    resource: String,
}

/// A sign-in waiting on the user's browser. Single use, and gone ten minutes
/// after it started.
struct Pending {
    integration_id: String,
    session: AuthorizationSession,
    client_id: String,
    client_secret: Option<String>,
    context: AuthContext,
    started: Instant,
}

/// The parts of a token response Pluk keeps.
///
/// rmcp answers with oauth2's response type, whose fields are only reachable
/// through a trait of oauth2's own, so the wire form is what gets read.
struct Tokens {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<i64>,
}

impl Tokens {
    fn read(response: &OAuthTokenResponse) -> Result<Self, AdapterError> {
        let wire = serde_json::to_value(response).map_err(|_| sign_in_failed())?;
        let access_token = wire
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(sign_in_failed)?
            .to_string();
        Ok(Tokens {
            access_token,
            refresh_token: wire
                .get("refresh_token")
                .and_then(Value::as_str)
                .map(str::to_string),
            expires_at: wire
                .get("expires_in")
                .and_then(Value::as_i64)
                .map(|seconds| now_ms() + seconds * 1_000),
        })
    }
}

fn stored_from(
    pending: &Pending,
    response: &OAuthTokenResponse,
) -> Result<ProxyAuthInput, AdapterError> {
    let tokens = Tokens::read(response)?;
    Ok(ProxyAuthInput {
        integration_id: pending.integration_id.clone(),
        kind: KIND.to_string(),
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at: tokens.expires_at,
        client_id: Some(pending.client_id.clone()),
        client_secret: pending.client_secret.clone(),
        metadata_json: serde_json::to_string(&pending.context).ok(),
    })
}

fn seed_response(
    access_token: &str,
    refresh_token: &str,
) -> Result<OAuthTokenResponse, AdapterError> {
    serde_json::from_value(json!({
        "access_token": access_token,
        "token_type": "bearer",
        "refresh_token": refresh_token,
    }))
    .map_err(|_| sign_in_failed())
}

fn context_of(stored: &ProxyAuth) -> Result<AuthContext, AdapterError> {
    stored
        .metadata_json
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .ok_or_else(sign_in_failed)
}

/// The state and the resource indicator rmcp put in the authorization URL.
/// Both are wanted back: the state keys the pending authorization, and the
/// resource is what the refresh has to repeat.
fn authorize_params(authorize_url: &str, endpoint: &str) -> Result<(String, String), AdapterError> {
    let parsed = Url::parse(authorize_url).map_err(|_| sign_in_failed())?;
    let mut state = None;
    let mut resource = None;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "state" => state = Some(value.into_owned()),
            "resource" => resource = Some(value.into_owned()),
            _ => {}
        }
    }
    Ok((
        state.ok_or_else(sign_in_failed)?,
        resource
            .filter(|resource| !resource.is_empty())
            .unwrap_or_else(|| endpoint.to_string()),
    ))
}

fn callback_params(callback_url: &str) -> Result<(String, String, Option<String>), AdapterError> {
    let absolute = match Url::parse(callback_url) {
        Ok(url) => url,
        Err(_) => {
            Url::parse(&format!("http://127.0.0.1{callback_url}")).map_err(|_| sign_in_failed())?
        }
    };
    let mut code = None;
    let mut state = None;
    let mut issuer = None;
    for (key, value) in absolute.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "iss" => issuer = Some(value.into_owned()),
            _ => {}
        }
    }
    match (code, state) {
        (Some(code), Some(state)) => Ok((code, state, issuer)),
        _ => Err(sign_in_failed()),
    }
}

fn remember(state: String, pending: Pending) {
    let mut held = waiting().lock().expect("mcp oauth pending");
    held.retain(|_, waiting| waiting.started.elapsed() < PENDING_TTL);
    held.insert(state, pending);
}

/// The authorization this state belongs to, spent by the reading.
fn claim(state: &str) -> Option<Pending> {
    let mut held = waiting().lock().expect("mcp oauth pending");
    let pending = held.remove(state)?;
    (pending.started.elapsed() < PENDING_TTL).then_some(pending)
}

fn waiting() -> &'static Mutex<HashMap<String, Pending>> {
    static WAITING: OnceLock<Mutex<HashMap<String, Pending>>> = OnceLock::new();
    WAITING.get_or_init(Default::default)
}

fn refresh_lock(integration_id: &str) -> Arc<AsyncMutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>> = OnceLock::new();
    LOCKS
        .get_or_init(Default::default)
        .lock()
        .expect("mcp oauth refresh locks")
        .entry(integration_id.to_string())
        .or_default()
        .clone()
}

fn stored_auth(store: &Store, integration_id: &str) -> Result<Option<ProxyAuth>, AdapterError> {
    store.get_proxy_auth(integration_id).map_err(store_failure)
}

fn is_expiring(stored: &ProxyAuth) -> bool {
    stored
        .expires_at
        .is_some_and(|expires_at| expires_at - EXPIRY_SKEW_MS <= now_ms())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn reconnect_error() -> AdapterError {
    AdapterError::new(RECONNECT_NEEDED).with_code(RECONNECT_NEEDED_CODE)
}

fn unreachable_error() -> AdapterError {
    AdapterError::new(SIGN_IN_UNREACHABLE).with_code(UPSTREAM_UNREACHABLE_CODE)
}

fn sign_in_failed() -> AdapterError {
    AdapterError::new(SIGN_IN_FAILED)
}

fn store_failure(error: StoreError) -> AdapterError {
    AdapterError::new(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::extract::State as AxumState;
    use axum::http::{HeaderMap, StatusCode, Uri, header};
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

    use pluk_store::{IntegrationInput, ToolState};

    use crate::adapter::{Adapter, ApiRequest, ApiResponse};
    use crate::mcp_proxy::api::{SIGN_IN_LAPSED, SIGNED_IN};
    use crate::mcp_proxy::tests::RecordingHost;
    use crate::mcp_proxy::{McpProxyAdapter, PERMISSION_DENIED};

    use super::*;

    const FIRST_ACCESS: &str = "access-token-one-abcdef";
    const FIRST_REFRESH: &str = "refresh-token-one-abcdef";
    const SECOND_ACCESS: &str = "access-token-two-ghijkl";
    const SECOND_REFRESH: &str = "refresh-token-two-ghijkl";

    #[test]
    fn the_state_and_the_resource_come_back_off_the_authorization_url() {
        let (state, resource) = authorize_params(
            "https://as.example.com/authorize?response_type=code&state=abc123&resource=https%3A%2F%2Fup%2F",
            "https://up/mcp",
        )
        .expect("params");
        assert_eq!(state, "abc123");
        assert_eq!(resource, "https://up/");
    }

    #[test]
    fn an_authorization_url_without_a_resource_falls_back_to_the_server_address() {
        let (_, resource) = authorize_params(
            "https://as.example.com/authorize?state=abc",
            "https://up/mcp",
        )
        .expect("params");
        assert_eq!(resource, "https://up/mcp");
        assert!(authorize_params("https://as.example.com/authorize", "https://up/mcp").is_err());
    }

    #[test]
    fn a_redirect_without_a_code_or_a_state_is_refused() {
        assert!(callback_params("/oauth/mcp/callback?code=c&state=s").is_ok());
        assert!(callback_params("/oauth/mcp/callback?code=c").is_err());
        assert!(callback_params("/oauth/mcp/callback?state=s").is_err());
        assert!(callback_params("/oauth/mcp/callback?error=access_denied").is_err());
    }

    #[test]
    fn a_redirect_carries_the_issuer_when_the_server_sent_one() {
        let (code, state, issuer) =
            callback_params("/oauth/mcp/callback?code=c&state=s&iss=https%3A%2F%2Fas")
                .expect("params");
        assert_eq!((code.as_str(), state.as_str()), ("c", "s"));
        assert_eq!(issuer.as_deref(), Some("https://as"));
    }

    #[test]
    fn a_token_response_is_read_off_its_wire_form() {
        let response = seed_response("access-1", "refresh-1").expect("seed");
        let tokens = Tokens::read(&response).expect("read");
        assert_eq!(tokens.access_token, "access-1");
        assert_eq!(tokens.refresh_token.as_deref(), Some("refresh-1"));
        assert_eq!(tokens.expires_at, None);

        let with_expiry: OAuthTokenResponse = serde_json::from_value(json!({
            "access_token": "access-2",
            "token_type": "bearer",
            "expires_in": 3_600,
        }))
        .expect("parse");
        let tokens = Tokens::read(&with_expiry).expect("read");
        assert_eq!(tokens.refresh_token, None);
        let expires_at = tokens.expires_at.expect("expiry");
        assert!(expires_at > now_ms() + 3_500_000, "{expires_at}");
    }

    #[test]
    fn the_redirect_address_is_pluks_own_loopback_port() {
        assert_eq!(
            redirect_uri(),
            format!(
                "http://127.0.0.1:{}/oauth/mcp/callback",
                pluk_core::loopback::port()
            )
        );
    }

    /// AC-1: the whole trip. Pluk starts the sign-in, the browser approves,
    /// the redirect lands, and the call that follows carries the bearer.
    #[tokio::test]
    async fn signing_in_stores_the_tokens_and_the_next_call_carries_them() {
        let world = World::new("oauth-happy-path").await;

        let authorize_url = start(&world.conn).await.expect("start");
        assert!(
            authorize_url.starts_with(&format!("{}/authorize", world.authority.base)),
            "{authorize_url}"
        );
        let redirect = world.approve(&authorize_url).await;
        complete(&world.store, &redirect).await.expect("callback");

        let stored = world.row().expect("row");
        assert_eq!(stored.access_token, FIRST_ACCESS);
        assert_eq!(stored.refresh_token.as_deref(), Some(FIRST_REFRESH));
        assert_eq!(stored.status, AuthStatus::Connected);
        assert_eq!(stored.version, 1);
        assert_eq!(stored.client_id.as_deref(), Some("client-1"));

        // Signing in reads the tool list, so there is something to approve.
        assert_eq!(
            world.catalog(),
            [("search".to_string(), ToolState::New)],
            "the sign-in discovered what the server offers"
        );

        world.approve_tool("search").await;
        assert_eq!(world.call("search").await, "found onboarding");
        assert_eq!(
            world.upstream.last_bearer(),
            Some(format!("Bearer {FIRST_ACCESS}"))
        );
    }

    /// AC-2: a redirect Pluk did not start, or already spent, changes nothing.
    #[tokio::test]
    async fn a_redirect_with_an_unknown_or_spent_state_stores_nothing() {
        let world = World::new("oauth-bad-state").await;

        complete(
            &world.store,
            "/oauth/mcp/callback?code=stolen&state=never-issued",
        )
        .await
        .expect_err("unknown state");
        assert!(world.row().is_none(), "nothing was stored");

        let authorize_url = start(&world.conn).await.expect("start");
        let redirect = world.approve(&authorize_url).await;
        complete(&world.store, &redirect).await.expect("callback");
        let first = world.row().expect("row");

        complete(&world.store, &redirect)
            .await
            .expect_err("already spent");
        let after = world.row().expect("row");
        assert_eq!(after.access_token, first.access_token);
        assert_eq!(after.version, first.version);
    }

    #[tokio::test]
    async fn an_authorization_older_than_its_window_is_gone() {
        let state = "state-that-timed-out";
        remember(state.to_string(), expired_pending().await);
        assert!(claim(state).is_none());
    }

    /// AC-3: a token about to run out is renewed before the call, and two
    /// callers racing produce one renewal, not two.
    #[tokio::test]
    async fn an_expired_token_is_renewed_once_however_many_callers_want_it() {
        let world = World::new("oauth-renew-race").await;
        world.seed(Some(now_ms() - 1_000));
        world.upstream.accept_only(SECOND_ACCESS);

        let (one, two) = tokio::join!(
            bearer(&world.store, &world.conn),
            bearer(&world.store, &world.conn)
        );
        assert_eq!(one.expect("first").as_deref(), Some(SECOND_ACCESS));
        assert_eq!(two.expect("second").as_deref(), Some(SECOND_ACCESS));
        assert_eq!(world.authority.refreshes(), 1);

        let stored = world.row().expect("row");
        assert_eq!(stored.access_token, SECOND_ACCESS);
        assert_eq!(
            stored.refresh_token.as_deref(),
            Some(SECOND_REFRESH),
            "a rotated refresh token replaces the spent one"
        );
        assert_eq!(stored.version, 2);

        world.approve_tool("search").await;
        assert_eq!(world.call("search").await, "found onboarding");
        assert_eq!(
            world.upstream.last_bearer(),
            Some(format!("Bearer {SECOND_ACCESS}"))
        );
    }

    #[tokio::test]
    async fn a_token_still_in_date_is_used_as_it_is() {
        let world = World::new("oauth-still-valid").await;
        world.seed(Some(now_ms() + 3_600_000));

        assert_eq!(
            bearer(&world.store, &world.conn)
                .await
                .expect("bearer")
                .as_deref(),
            Some(FIRST_ACCESS)
        );
        assert_eq!(world.authority.refreshes(), 0);
    }

    /// AC-4: a refusal to renew is the user's to fix; a server Pluk cannot
    /// reach is not.
    #[tokio::test]
    async fn a_refused_renewal_asks_the_user_to_sign_in_again() {
        let world = World::new("oauth-refused-renewal").await;
        world.authority.refuse_renewals();
        world.seed(Some(now_ms() - 1_000));

        let error = bearer(&world.store, &world.conn)
            .await
            .expect_err("refused");
        assert!(error.has_code(RECONNECT_NEEDED_CODE), "{error:?}");
        assert_eq!(error.message, RECONNECT_NEEDED);
        assert_eq!(
            world.row().expect("row").status,
            AuthStatus::ReconnectNeeded
        );

        // Every later call says the same thing without asking the server again.
        let again = bearer(&world.store, &world.conn)
            .await
            .expect_err("still refused");
        assert!(again.has_code(RECONNECT_NEEDED_CODE));
        assert_eq!(world.authority.refreshes(), 1);
    }

    #[tokio::test]
    async fn a_sign_in_server_pluk_cannot_reach_is_not_the_users_fault() {
        let world = World::new("oauth-unreachable-renewal").await;
        world.seed_against(&closed_authority().await, Some(now_ms() - 1_000));

        let error = bearer(&world.store, &world.conn)
            .await
            .expect_err("unreachable");
        assert!(error.has_code(UPSTREAM_UNREACHABLE_CODE), "{error:?}");
        assert!(!error.has_code(RECONNECT_NEEDED_CODE));
        assert_eq!(
            world.row().expect("row").status,
            AuthStatus::Connected,
            "a server that is down does not spend the sign-in"
        );
    }

    #[tokio::test]
    async fn a_stored_sign_in_without_a_refresh_token_asks_for_a_new_one() {
        let world = World::new("oauth-no-refresh-token").await;
        world
            .store
            .set_proxy_auth(&ProxyAuthInput {
                integration_id: world.conn.id.clone(),
                kind: KIND.to_string(),
                access_token: FIRST_ACCESS.to_string(),
                refresh_token: None,
                expires_at: Some(now_ms() - 1_000),
                client_id: Some("client-1".to_string()),
                client_secret: None,
                metadata_json: Some(world.context_json(&world.authority)),
            })
            .expect("seed");

        let error = bearer(&world.store, &world.conn)
            .await
            .expect_err("nothing to renew");
        assert!(error.has_code(RECONNECT_NEEDED_CODE), "{error:?}");
        assert_eq!(
            world.row().expect("row").status,
            AuthStatus::ReconnectNeeded
        );
        assert_eq!(world.authority.refreshes(), 0);
    }

    /// A call whose token upstream refuses is retried once behind a renewal,
    /// which is what an access token running out mid-call looks like.
    #[tokio::test]
    async fn a_call_upstream_refuses_is_retried_behind_a_renewal() {
        let world = World::new("oauth-retry-after-renewal").await;
        world.seed(Some(now_ms() + 3_600_000));
        world.approve_tool("search").await;
        world.upstream.accept_only(SECOND_ACCESS);

        assert_eq!(world.call("search").await, "found onboarding");
        assert_eq!(world.authority.refreshes(), 1);
        assert_eq!(world.row().expect("row").access_token, SECOND_ACCESS);
    }

    #[tokio::test]
    async fn a_call_still_refused_after_a_renewal_asks_the_user_to_sign_in() {
        let world = World::new("oauth-retry-exhausted").await;
        world.seed(Some(now_ms() + 3_600_000));
        world.approve_tool("search").await;
        world.upstream.accept_only("a-token-no-renewal-will-mint");

        assert_eq!(world.call("search").await, RECONNECT_NEEDED);
        assert_eq!(
            world.row().expect("row").status,
            AuthStatus::ReconnectNeeded
        );
    }

    /// A server that will not do this for the account Pluk signed in with is
    /// answering the request, not the sign-in. Sending the user round the
    /// browser again would change nothing.
    #[tokio::test]
    async fn a_server_refusing_on_permission_is_not_a_sign_in_to_redo() {
        let world = World::new("oauth-permission-denied").await;
        world.seed(Some(now_ms() + 3_600_000));
        world.approve_tool("search").await;
        world.upstream.refuse_the_request();

        assert_eq!(world.call("search").await, PERMISSION_DENIED);
        assert_eq!(world.authority.refreshes(), 0);
        assert_eq!(world.row().expect("row").status, AuthStatus::Connected);
    }

    /// A sign-in is granted by one server for one server. Point the
    /// integration somewhere else and it stops counting, without being thrown
    /// away.
    #[tokio::test]
    async fn a_sign_in_does_not_follow_the_integration_to_another_address() {
        let mut world = World::new("oauth-address-moved").await;
        world.seed(Some(now_ms() + 3_600_000));
        world.approve_tool("search").await;

        let elsewhere = upstream(&world.authority.base).await;
        world.point_at(&elsewhere.endpoint);

        assert!(world.call("search").await.starts_with("Error:"));
        assert_eq!(
            elsewhere.last_bearer(),
            None,
            "the other server is offered nothing the first one granted"
        );
        assert_eq!(world.authority.refreshes(), 0);
        assert_eq!(
            world.rest("GET", "/proxy/auth", None).await["auth"],
            json!({ "kind": "none", "status": "not_connected" })
        );
        assert!(world.row().is_some(), "the sign-in is kept, not discarded");
    }

    /// AC-5: none of the REST surface, and none of the failures it produces,
    /// carry anything the user signed in with.
    #[tokio::test]
    async fn nothing_the_rest_surface_says_carries_a_secret() {
        let world = World::new("oauth-nothing-leaks").await;

        let started = world.rest("POST", "/proxy/oauth/start", None).await;
        let authorize_url = started["authorizeUrl"].as_str().expect("url").to_string();
        let redirect = world.approve(&authorize_url).await;
        world.page(&redirect).await;

        let code = query_of(&redirect)
            .remove("code")
            .expect("the browser carried a code");
        let stored = world.row().expect("row");
        let refused = world.refuse_the_next_renewal().await;

        let seen = format!(
            "{}{}{}{:?}{:?}{:?}",
            world.rest_body("GET", "/proxy/auth", None).await,
            world.rest_body("GET", "/proxy/tools", None).await,
            authorize_url,
            started,
            refused,
            stored,
        );
        for secret in [
            FIRST_ACCESS,
            FIRST_REFRESH,
            SECOND_ACCESS,
            SECOND_REFRESH,
            code.as_str(),
        ] {
            assert!(!seen.contains(secret), "{secret} reached a caller");
        }
    }

    #[tokio::test]
    async fn the_sign_in_route_says_how_a_server_is_reached() {
        let world = World::new("oauth-sign-in-shape").await;
        assert_eq!(
            world.rest("GET", "/proxy/auth", None).await["auth"],
            json!({ "kind": "none", "status": "not_connected" })
        );

        let mut with_token = world.conn.clone();
        with_token
            .config
            .insert("token".to_string(), Value::String("t0ken".to_string()));
        let typed_in = sign_in_state(&world.store, &with_token).expect("sign in");
        assert_eq!((typed_in.kind, typed_in.status), ("token", "connected"));

        world.seed(Some(now_ms() + 3_600_000));
        assert_eq!(
            world.rest("GET", "/proxy/auth", None).await["auth"],
            json!({ "kind": "oauth", "status": "connected" })
        );

        world.rest("POST", "/proxy/disconnect", None).await;
        assert!(world.row().is_none(), "the sign-in is forgotten");
        assert_eq!(
            world.rest("GET", "/proxy/auth", None).await["auth"],
            json!({ "kind": "none", "status": "not_connected" })
        );
    }

    #[tokio::test]
    async fn the_landing_page_says_one_thing_and_never_the_code() {
        let world = World::new("oauth-landing-page").await;
        let authorize_url = start(&world.conn).await.expect("start");
        let redirect = world.approve(&authorize_url).await;

        let done = world.page(&redirect).await;
        assert_eq!(done.status, 200);
        let body = String::from_utf8_lossy(&done.body).to_string();
        assert!(body.contains(SIGNED_IN), "{body}");
        assert!(!body.contains("code="), "{body}");

        let lapsed = world.page(&redirect).await;
        assert_eq!(lapsed.status, 400);
        assert!(
            String::from_utf8_lossy(&lapsed.body).contains(SIGN_IN_LAPSED),
            "the second trip says the link is spent"
        );
    }

    /// One integration, one upstream server and one authorization server, all
    /// on loopback, with the store they share.
    struct World {
        store: Arc<Store>,
        conn: Integration,
        upstream: Arc<UpstreamState>,
        authority: Arc<AuthorityState>,
        adapter: Arc<McpProxyAdapter>,
        browser: upstream_http::Client,
        _dir: tempfile::TempDir,
    }

    impl World {
        async fn new(name: &str) -> Self {
            let authority = authority().await;
            let upstream = upstream(&authority.base).await;
            let dir = tempfile::tempdir().expect("tempdir");
            let store = Arc::new(Store::open(&dir.path().join("pluk.db")).expect("open"));
            // A row the store really holds: signing in reads the integration
            // back to discover what the server offers.
            let mut input = IntegrationInput::new(name, crate::mcp_proxy::ADAPTER_ID);
            input
                .config
                .insert("url".to_string(), Value::String(upstream.endpoint.clone()));
            input.query_policy =
                Some(json!({ "tools": { "search": { "enabled": true } } }).to_string());
            let conn = store.create_integration(&input).expect("integration");
            World {
                adapter: McpProxyAdapter::new(store.clone()),
                store,
                conn,
                upstream,
                authority,
                browser: upstream_http::Client::builder()
                    .redirect(upstream_http::redirect::Policy::none())
                    .build()
                    .expect("browser"),
                _dir: dir,
            }
        }

        /// What the user's browser does with the address Pluk hands it.
        async fn approve(&self, authorize_url: &str) -> String {
            let response = self
                .browser
                .get(authorize_url)
                .send()
                .await
                .expect("authorize");
            assert_eq!(response.status().as_u16(), 302, "the server redirects back");
            let location = response
                .headers()
                .get("location")
                .expect("location")
                .to_str()
                .expect("ascii")
                .to_string();
            assert!(location.starts_with(&redirect_uri()), "{location}");
            location
        }

        /// The redirect as Pluk's own loopback server hands it over: the
        /// adapter's global route, landing page and all.
        async fn page(&self, redirect: &str) -> ApiResponse {
            let path_and_query = redirect
                .strip_prefix(&format!("http://127.0.0.1:{}", pluk_core::loopback::port()))
                .expect("a loopback redirect")
                .to_string();
            self.adapter
                .handle_global_api(
                    ApiRequest {
                        method: "GET".to_string(),
                        url: path_and_query,
                        body: None,
                    },
                    CALLBACK_PATH,
                )
                .await
                .expect("the callback route is the proxy adapter's")
        }

        fn row(&self) -> Option<ProxyAuth> {
            self.store.get_proxy_auth(&self.conn.id).expect("read")
        }

        fn catalog(&self) -> Vec<(String, ToolState)> {
            catalog::snapshot(&self.store, &self.conn.id)
                .expect("snapshot")
                .iter()
                .map(|tool| (tool.name.clone(), tool.state()))
                .collect()
        }

        fn seed(&self, expires_at: Option<i64>) {
            self.seed_against(&self.authority, expires_at);
        }

        /// The same integration, pointed at another server.
        fn point_at(&mut self, endpoint: &str) {
            self.conn
                .config
                .insert("url".to_string(), Value::String(endpoint.to_string()));
            client::invalidate(&self.conn.id);
        }

        /// A sign-in that already happened, pointed at one authorization
        /// server, so a renewal can be exercised without a browser.
        fn seed_against(&self, authority: &AuthorityState, expires_at: Option<i64>) {
            self.store
                .set_proxy_auth(&ProxyAuthInput {
                    integration_id: self.conn.id.clone(),
                    kind: KIND.to_string(),
                    access_token: FIRST_ACCESS.to_string(),
                    refresh_token: Some(FIRST_REFRESH.to_string()),
                    expires_at,
                    client_id: Some("client-1".to_string()),
                    client_secret: None,
                    metadata_json: Some(self.context_json(authority)),
                })
                .expect("seed");
            client::invalidate(&self.conn.id);
        }

        fn context_json(&self, authority: &AuthorityState) -> String {
            serde_json::to_string(&AuthContext {
                metadata: serde_json::from_value(authority.metadata()).expect("metadata"),
                resource: self.upstream.endpoint.clone(),
            })
            .expect("context")
        }

        async fn approve_tool(&self, name: &str) {
            catalog::discover(&self.store, &self.conn)
                .await
                .expect("discover");
            self.store
                .approve_proxy_tools(&self.conn.id, &[name.to_string()])
                .expect("approve");
        }

        /// One proxied call, through the gate an agent reaches it by.
        async fn call(&self, name: &str) -> String {
            let mut host = RecordingHost::default();
            self.adapter
                .register(&mut host, &self.conn, "owner")
                .expect("register");
            let handler = host.handlers.get(name).expect("registered").clone();
            handler(json!({ "q": "onboarding" }))
                .await
                .text()
                .to_string()
        }

        /// Force the next renewal to be refused, and hand back what a caller
        /// is told when it is.
        async fn refuse_the_next_renewal(&self) -> AdapterError {
            self.authority.refuse_renewals();
            let stored = self.row().expect("row");
            self.store
                .update_proxy_tokens(
                    &self.conn.id,
                    stored.version,
                    &RefreshedTokens {
                        access_token: stored.access_token,
                        refresh_token: stored.refresh_token,
                        expires_at: Some(now_ms() - 1_000),
                    },
                )
                .expect("expire");
            bearer(&self.store, &self.conn).await.expect_err("refused")
        }

        async fn rest(&self, method: &str, subpath: &str, body: Option<String>) -> Value {
            serde_json::from_str(&self.rest_body(method, subpath, body).await).expect("json")
        }

        async fn rest_body(&self, method: &str, subpath: &str, body: Option<String>) -> String {
            let response = self
                .adapter
                .handle_api(
                    &self.conn,
                    ApiRequest {
                        method: method.to_string(),
                        url: format!("/api/integrations/{}{subpath}", self.conn.id),
                        body,
                    },
                    subpath,
                )
                .await
                .expect("route");
            String::from_utf8_lossy(&response.body).to_string()
        }
    }

    impl Drop for World {
        fn drop(&mut self) {
            client::invalidate(&self.conn.id);
        }
    }

    /// A pending authorization that started long enough ago to be gone.
    async fn expired_pending() -> Pending {
        let base = "http://127.0.0.1:1/mcp";
        let mut manager = AuthorizationManager::new(base).await.expect("manager");
        let metadata: AuthorizationMetadata = serde_json::from_value(json!({
            "issuer": "http://127.0.0.1:1",
            "authorization_endpoint": "http://127.0.0.1:1/authorize",
            "token_endpoint": "http://127.0.0.1:1/token",
        }))
        .expect("metadata");
        manager.set_metadata(metadata.clone());
        manager
            .configure_client(OAuthClientConfig::new("client-1", redirect_uri()))
            .expect("client");
        Pending {
            integration_id: "oauth-stale".to_string(),
            session: AuthorizationSession::for_scope_upgrade(
                manager,
                "http://127.0.0.1:1/authorize".to_string(),
                &redirect_uri(),
            ),
            client_id: "client-1".to_string(),
            client_secret: None,
            context: AuthContext {
                metadata,
                resource: base.to_string(),
            },
            started: Instant::now() - PENDING_TTL - Duration::from_secs(1),
        }
    }

    /// axum's own extractors are behind features this crate does not build, so
    /// the fakes read their own query strings and form bodies.
    fn params(raw: &str) -> HashMap<String, String> {
        raw.split('&')
            .filter(|pair| !pair.is_empty())
            .filter_map(|pair| pair.split_once('='))
            .map(|(key, value)| (decode(key), decode(value)))
            .collect()
    }

    fn decode(raw: &str) -> String {
        urlencoding::decode(&raw.replace('+', " "))
            .map(|decoded| decoded.into_owned())
            .unwrap_or_else(|_| raw.to_string())
    }

    fn query_of(url: &str) -> HashMap<String, String> {
        params(url.split_once('?').map_or("", |(_, query)| query))
    }

    fn as_json(value: Value) -> Response {
        (
            [(header::CONTENT_TYPE, "application/json")],
            value.to_string(),
        )
            .into_response()
    }

    // ---- the authorization server -------------------------------------

    struct AuthorityState {
        base: String,
        refreshes: AtomicUsize,
        refuses: Mutex<bool>,
        issued: Mutex<HashMap<String, String>>,
    }

    impl AuthorityState {
        fn refreshes(&self) -> usize {
            self.refreshes.load(Ordering::SeqCst)
        }

        fn refuse_renewals(&self) {
            *self.refuses.lock().expect("refuses") = true;
        }

        fn metadata(&self) -> Value {
            let base = &self.base;
            json!({
                "issuer": base,
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "registration_endpoint": format!("{base}/register"),
                "response_types_supported": ["code"],
                "code_challenge_methods_supported": ["S256"],
                "grant_types_supported": ["authorization_code", "refresh_token"],
            })
        }
    }

    async fn authority() -> Arc<AuthorityState> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let state = Arc::new(AuthorityState {
            base,
            refreshes: AtomicUsize::new(0),
            refuses: Mutex::new(false),
            issued: Mutex::new(HashMap::new()),
        });
        let router = Router::new()
            .route(
                "/.well-known/oauth-authorization-server",
                get(
                    |AxumState(state): AxumState<Arc<AuthorityState>>| async move {
                        as_json(state.metadata())
                    },
                ),
            )
            .route(
                "/register",
                post(|| async {
                    as_json(json!({
                        "client_id": "client-1",
                        "redirect_uris": [redirect_uri()],
                    }))
                }),
            )
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .with_state(state.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        state
    }

    async fn authorize(AxumState(state): AxumState<Arc<AuthorityState>>, uri: Uri) -> Response {
        let query = params(uri.query().unwrap_or_default());
        assert_eq!(
            query.get("code_challenge_method").map(String::as_str),
            Some("S256"),
            "the flow proves possession with S256"
        );
        assert!(query.contains_key("code_challenge"));
        assert!(
            query.get("resource").is_some_and(|it| !it.is_empty()),
            "the resource indicator is sent"
        );
        let csrf = query.get("state").expect("state").clone();
        let code = format!("code-for-{csrf}");
        state
            .issued
            .lock()
            .expect("issued")
            .insert(code.clone(), query["code_challenge"].clone());
        let location = format!(
            "{}?code={}&state={}&iss={}",
            query.get("redirect_uri").expect("redirect_uri"),
            urlencoding::encode(&code),
            urlencoding::encode(&csrf),
            urlencoding::encode(&state.base),
        );
        (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
    }

    async fn token(AxumState(state): AxumState<Arc<AuthorityState>>, body: String) -> Response {
        let form = params(&body);
        assert!(
            form.get("resource").is_some_and(|it| !it.is_empty()),
            "the resource indicator is sent on every token request"
        );
        match form.get("grant_type").map(String::as_str) {
            Some("authorization_code") => {
                let code = form.get("code").expect("code");
                assert!(
                    state.issued.lock().expect("issued").remove(code).is_some(),
                    "the code was issued here and is spent once"
                );
                assert!(
                    form.get("code_verifier").is_some_and(|it| !it.is_empty()),
                    "the verifier is sent"
                );
                as_json(json!({
                    "access_token": FIRST_ACCESS,
                    "token_type": "bearer",
                    "expires_in": 3_600,
                    "refresh_token": FIRST_REFRESH,
                }))
            }
            Some("refresh_token") => {
                state.refreshes.fetch_add(1, Ordering::SeqCst);
                if *state.refuses.lock().expect("refuses") {
                    return (
                        StatusCode::BAD_REQUEST,
                        [(header::CONTENT_TYPE, "application/json")],
                        json!({ "error": "invalid_grant" }).to_string(),
                    )
                        .into_response();
                }
                assert_eq!(
                    form.get("refresh_token").map(String::as_str),
                    Some(FIRST_REFRESH)
                );
                as_json(json!({
                    "access_token": SECOND_ACCESS,
                    "token_type": "bearer",
                    "expires_in": 3_600,
                    "refresh_token": SECOND_REFRESH,
                }))
            }
            other => panic!("unexpected grant type {other:?}"),
        }
    }

    /// An authorization server at an address nothing answers on.
    async fn closed_authority() -> AuthorityState {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        drop(listener);
        AuthorityState {
            base,
            refreshes: AtomicUsize::new(0),
            refuses: Mutex::new(false),
            issued: Mutex::new(HashMap::new()),
        }
    }

    // ---- the upstream MCP server --------------------------------------

    struct UpstreamState {
        endpoint: String,
        metadata_url: String,
        accepted: Mutex<Option<String>>,
        last_bearer: Mutex<Option<String>>,
        forbids: Mutex<bool>,
    }

    impl UpstreamState {
        fn last_bearer(&self) -> Option<String> {
            self.last_bearer.lock().expect("bearer").clone()
        }

        /// Take the credentials and refuse the request anyway, the way a
        /// server behaves when the account is not allowed to do this.
        fn refuse_the_request(&self) {
            *self.forbids.lock().expect("forbids") = true;
        }

        /// Take this token and nothing else, the way a server behaves once the
        /// one Pluk holds has run out.
        fn accept_only(&self, access_token: &str) {
            *self.accepted.lock().expect("accepted") = Some(access_token.to_string());
        }

        fn challenge(&self) -> String {
            format!("Bearer resource_metadata=\"{}\"", self.metadata_url)
        }
    }

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

    async fn upstream(authority_base: &str) -> Arc<UpstreamState> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let state = Arc::new(UpstreamState {
            endpoint: format!("{base}/mcp"),
            metadata_url: format!("{base}/.well-known/oauth-protected-resource"),
            accepted: Mutex::new(Some(FIRST_ACCESS.to_string())),
            last_bearer: Mutex::new(None),
            forbids: Mutex::new(false),
        });
        let resource = state.endpoint.clone();
        let authority_base = authority_base.to_string();
        let mcp = StreamableHttpService::new(
            || Ok(TestServer),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default(),
        );

        let guarded = state.clone();
        let router = Router::new()
            .route(
                "/.well-known/oauth-protected-resource",
                get(move || {
                    let (resource, authority_base) = (resource.clone(), authority_base.clone());
                    async move {
                        as_json(json!({
                            "resource": resource,
                            "authorization_servers": [authority_base],
                        }))
                    }
                }),
            )
            .route(
                "/mcp",
                any(move |headers: HeaderMap, request: axum::extract::Request| {
                    let (state, mcp) = (guarded.clone(), mcp.clone());
                    async move {
                        let presented = headers
                            .get(header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_string);
                        *state.last_bearer.lock().expect("bearer") = presented.clone();
                        let wanted = state
                            .accepted
                            .lock()
                            .expect("accepted")
                            .clone()
                            .map(|token| format!("Bearer {token}"));
                        if presented != wanted {
                            return (
                                StatusCode::UNAUTHORIZED,
                                [(header::WWW_AUTHENTICATE, state.challenge())],
                                "unauthorized",
                            )
                                .into_response();
                        }
                        if *state.forbids.lock().expect("forbids") {
                            return (StatusCode::FORBIDDEN, "forbidden").into_response();
                        }
                        match mcp.oneshot(request).await {
                            Ok(response) => response.map(axum::body::Body::new).into_response(),
                            Err(never) => match never {},
                        }
                    }
                }),
            );
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        state
    }
}
