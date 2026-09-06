//! Signing in to an MCP server that asks for it.
//!
//! Everything protocol-shaped comes from `rmcp`'s `transport::auth`: protected
//! resource + authorization server discovery, dynamic client registration,
//! PKCE, the `state` check, the code exchange and the refresh — including
//! writing a refreshed token back through the credential store. What lives
//! here is only what `rmcp` leaves to the application: where the tokens are
//! kept, where the browser comes back to, and how a half-finished sign-in is
//! held while the person is away in their browser.
//!
//! Tokens are kept in the integration's own config blob, next to every other
//! credential Pluk stores, under one key holding a JSON document keyed by
//! server name. They are as readable on disk as a pasted token is — the same
//! posture, no better.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rmcp::transport::auth::{
    AuthError, AuthorizationManager, AuthorizationRequest, AuthorizationSession, CredentialStore,
    StoredCredentials,
};
use serde_json::Value;

use pluk_store::{Integration, IntegrationUpdate, Store};

use crate::error::AdapterError;

/// Config key holding every signed-in server's tokens, as a JSON document.
/// Public so the window's own copy of a config can leave it out and put it
/// back: the person editing a connection never handles these.
pub const CREDENTIALS_KEY: &str = "signed_in";

/// How long a sign-in that was started but never came back is held.
const FLOW_LIFETIME: Duration = Duration::from_secs(600);

/// The path the browser is sent back to once the server has asked the person.
pub const CALLBACK_PATH: &str = "/oauth/callback";

/// Marks the one failure the person can fix themselves: this server will not
/// answer until they sign in. Distinct from a server that cannot be reached.
pub const NEEDS_SIGN_IN_CODE: &str = "MCP_NEEDS_SIGN_IN";

/// Where the browser returns to. The loopback server's port is read the same
/// way `pluk-server` reads it; this crate cannot depend on that one.
pub fn redirect_uri() -> String {
    let port = std::env::var("PORT")
        .ok()
        .and_then(|port| port.parse::<u16>().ok())
        .unwrap_or(4242);
    format!("http://127.0.0.1:{port}{CALLBACK_PATH}")
}

fn documents(conn: &Integration) -> HashMap<String, StoredCredentials> {
    conn.config
        .get(CREDENTIALS_KEY)
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default()
}

/// Whether this server has been signed in to.
pub fn is_signed_in(conn: &Integration, server: &str) -> bool {
    documents(conn)
        .get(server)
        .is_some_and(|stored| stored.token_response.is_some())
}

/// One server's tokens inside one integration's config.
struct StoredIn {
    store: Arc<Store>,
    integration_id: String,
    server: String,
}

impl StoredIn {
    fn read(&self) -> Result<Option<StoredCredentials>, AdapterError> {
        let conn = integration(&self.store, &self.integration_id)?;
        Ok(documents(&conn).remove(&self.server))
    }

    /// Replace this server's entry, leaving every other config key alone.
    fn write(&self, credentials: Option<StoredCredentials>) -> Result<(), AdapterError> {
        let conn = integration(&self.store, &self.integration_id)?;
        let mut all = documents(&conn);
        match credentials {
            Some(credentials) => all.insert(self.server.clone(), credentials),
            None => all.remove(&self.server),
        };
        let mut config = conn.config.clone();
        let encoded = serde_json::to_string(&all).map_err(|error| {
            AdapterError::new(format!("the sign-in could not be saved: {error}"))
        })?;
        config.insert(CREDENTIALS_KEY.to_string(), Value::String(encoded));
        self.store
            .update_integration(
                &self.integration_id,
                &IntegrationUpdate {
                    config: Some(config),
                    ..IntegrationUpdate::default()
                },
            )
            .map_err(|error| {
                AdapterError::new(format!("the sign-in could not be saved: {error}"))
            })?;
        Ok(())
    }
}

fn store_error(error: AdapterError) -> AuthError {
    AuthError::InternalError(error.message)
}

#[async_trait::async_trait]
impl CredentialStore for StoredIn {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        self.read().map_err(store_error)
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        self.write(Some(credentials)).map_err(store_error)
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.write(None).map_err(store_error)
    }
}

/// A handle to one server's stored tokens, carried by an upstream that signs
/// in. Cloning shares the same entry.
#[derive(Clone)]
pub struct Credentials(Arc<StoredIn>);

impl Credentials {
    pub fn new(store: Arc<Store>, integration_id: &str, server: &str) -> Self {
        Credentials(Arc::new(StoredIn {
            store,
            integration_id: integration_id.to_string(),
            server: server.to_string(),
        }))
    }

    fn shared(&self) -> SharedStore {
        SharedStore(self.0.clone())
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("server", &self.0.server)
            .finish()
    }
}

/// `rmcp` takes ownership of the credential store it is given; this hands it a
/// second holder of the same entry.
struct SharedStore(Arc<StoredIn>);

#[async_trait::async_trait]
impl CredentialStore for SharedStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        self.0.load().await
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        self.0.save(credentials).await
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.0.clear().await
    }
}

fn integration(store: &Store, id: &str) -> Result<Integration, AdapterError> {
    store
        .integration_by_id(id)
        .map_err(|error| AdapterError::new(error.to_string()))?
        .ok_or_else(|| AdapterError::new("this connection no longer exists"))
}

/// An [`AuthorizationManager`] already pointed at `url` and reading and writing
/// this server's stored tokens.
async fn manager(url: &str, credentials: &Credentials) -> Result<AuthorizationManager, AuthError> {
    let mut manager = AuthorizationManager::new(url).await?;
    manager.set_credential_store(credentials.shared());
    Ok(manager)
}

/// A manager holding a usable sign-in, or `None` when the person has to sign
/// in before the server will answer.
pub async fn authorized(
    url: &str,
    credentials: &Credentials,
) -> Result<Option<AuthorizationManager>, AdapterError> {
    let mut manager = manager(url, credentials).await.map_err(auth_error)?;
    match manager.initialize_from_store().await {
        Ok(true) => Ok(Some(manager)),
        Ok(false) => Ok(None),
        Err(AuthError::AuthorizationRequired) => Ok(None),
        Err(error) => Err(auth_error(error)),
    }
}

/// A sign-in the person is away completing, held until their browser returns.
struct Pending {
    session: AuthorizationSession,
    integration_id: String,
    server: String,
    started: Instant,
}

fn pending() -> &'static Mutex<HashMap<String, Pending>> {
    static PENDING: OnceLock<Mutex<HashMap<String, Pending>>> = OnceLock::new();
    PENDING.get_or_init(Mutex::default)
}

/// The `state` value the authorization URL carries, which is what the browser
/// comes back with and how the returning call finds its own sign-in.
fn state_of(url: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == "state")
        .map(|(_, value)| value.to_string())
}

/// Start a sign-in and return the address to open in the browser.
pub async fn start(
    store: Arc<Store>,
    integration_id: &str,
    server: &str,
    url: &str,
) -> Result<String, AdapterError> {
    let credentials = Credentials::new(store, integration_id, server);
    let mut manager = manager(url, &credentials).await.map_err(auth_error)?;
    let resolved = manager
        .resolve_metadata_from_challenge(None)
        .await
        .map_err(auth_error)?;
    manager.set_metadata(resolved.metadata);

    let request = AuthorizationRequest::new(redirect_uri()).with_client_name("Pluk");
    let session = AuthorizationSession::new(manager, request)
        .await
        .map_err(|(_, error)| auth_error(error))?;
    let authorize_url = session.get_authorization_url().to_string();
    let state = state_of(&authorize_url)
        .ok_or_else(|| AdapterError::new("this server did not offer a usable sign-in"))?;

    let mut held = pending().lock().expect("mcp sign-in");
    held.retain(|_, flow| flow.started.elapsed() < FLOW_LIFETIME);
    held.insert(
        state,
        Pending {
            session,
            integration_id: integration_id.to_string(),
            server: server.to_string(),
            started: Instant::now(),
        },
    );
    Ok(authorize_url)
}

/// The integration and server one returning browser belongs to.
pub struct Completed {
    pub integration_id: String,
    pub server: String,
}

/// Finish the sign-in a browser has come back from, given the path and query
/// it arrived on. The callback is matched to a waiting sign-in by its `state`,
/// and a sign-in is only ever used once.
pub async fn complete(callback: &str) -> Result<Completed, AdapterError> {
    let callback_url = format!("http://127.0.0.1{callback}");
    let state = state_of(&callback_url)
        .ok_or_else(|| AdapterError::new("this sign-in could not be matched to Pluk"))?;
    let flow = {
        let mut held = pending().lock().expect("mcp sign-in");
        held.retain(|_, flow| flow.started.elapsed() < FLOW_LIFETIME);
        held.remove(&state)
    };
    let flow = flow.ok_or_else(|| {
        AdapterError::new("this sign-in has already finished or was left too long")
    })?;
    flow.session
        .handle_callback_url(&callback_url)
        .await
        .map_err(auth_error)?;
    Ok(Completed {
        integration_id: flow.integration_id,
        server: flow.server,
    })
}

/// Drop this server's tokens.
pub fn forget(store: Arc<Store>, integration_id: &str, server: &str) -> Result<(), AdapterError> {
    Credentials::new(store, integration_id, server)
        .0
        .write(None)
}

/// Wording for a failure the person can act on. Nothing here can carry a token:
/// `rmcp`'s error text names what step failed, never what was sent.
fn auth_error(error: AuthError) -> AdapterError {
    match error {
        AuthError::AuthorizationRequired => AdapterError::new("this server needs you to sign in"),
        AuthError::NoAuthorizationSupport => {
            AdapterError::new("this server does not offer signing in")
        }
        other => AdapterError::new(format!("signing in did not finish: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_is_read_back_out_of_the_authorization_url() {
        assert_eq!(
            state_of("https://example.com/authorize?client_id=a&state=xyz&scope=b"),
            Some("xyz".into())
        );
        assert_eq!(state_of("https://example.com/authorize"), None);
    }

    fn integration() -> Integration {
        Integration {
            id: "proxy".into(),
            name: "My servers".into(),
            r#type: "mcp-proxy".into(),
            config: serde_json::Map::new(),
            environment: None,
            read_only: 0,
            query_policy: None,
            token: "t".into(),
            created_at: String::new(),
            via_group: None,
        }
    }

    #[test]
    fn a_server_with_no_stored_token_is_not_signed_in() {
        let mut conn = integration();
        assert!(!is_signed_in(&conn, "Docs"));
        conn.config.insert(
            CREDENTIALS_KEY.into(),
            Value::String(r#"{"Docs":{"client_id":"c","token_response":null}}"#.into()),
        );
        assert!(!is_signed_in(&conn, "Docs"));
    }
}
