//! Where an upstream MCP server says a sign-in can be had.
//!
//! rmcp walks the order the MCP authorization spec lays out — protected
//! resource metadata, then authorization server metadata — and on a server
//! that answers every step it is the whole answer. It stops at the first step
//! that fails, though, and plenty of servers fail one: a well-known path that
//! answers a server error, an authorization server naming an issuer that is
//! not the host it was found on. One such step reads as a server that does not
//! offer sign-in at all. The walk here picks up from there: every standard
//! location is tried, a location that fails is only that location, and the
//! first document that is actually usable wins.
//!
//! Only a document a server served counts. Endpoints built out of the address
//! by convention are guesses, and a guess is not somewhere to send a browser.

use std::time::Duration;

use rmcp::transport::auth::{AuthorizationManager, AuthorizationMetadata};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::time::timeout;
use upstream_http::Url;

use super::catalog;
use super::client;

/// How long the walk over the well-known locations may take in total. It runs
/// inside the probe, where a user is waiting on an answer about one server.
const WALK_BUDGET: Duration = Duration::from_secs(10);

/// The sign-in this server published, or `None` when it published none.
///
/// Carries nothing the user stored: whatever this asks a server is asked as a
/// stranger, before anyone has decided a credential belongs there.
pub(super) async fn published(
    manager: &AuthorizationManager,
    endpoint: &str,
    challenge: Option<&str>,
) -> Option<AuthorizationMetadata> {
    if let Ok(resolution) = manager.resolve_metadata_from_challenge(challenge).await
        && resolution.source.is_discovered()
        && is_usable(&resolution.metadata)
    {
        return Some(resolution.metadata);
    }
    let base = Url::parse(endpoint).ok()?;
    timeout(WALK_BUDGET, walk(&base)).await.ok().flatten()
}

async fn walk(base: &Url) -> Option<AuthorizationMetadata> {
    for url in well_known(base, "oauth-protected-resource") {
        if let Some(metadata) = from_resource_metadata(&url).await {
            return Some(metadata);
        }
    }
    from_authority(base).await
}

/// The authorization servers a protected resource names, asked in turn for
/// metadata of their own.
async fn from_resource_metadata(url: &Url) -> Option<AuthorizationMetadata> {
    let resource: ResourceMetadata = fetched(url).await?;
    for authority in resource.authorization_servers {
        let Ok(authority) = Url::parse(&authority) else {
            continue;
        };
        if let Some(metadata) = from_authority(&authority).await {
            return Some(metadata);
        }
    }
    None
}

async fn from_authority(base: &Url) -> Option<AuthorizationMetadata> {
    for resource in ["oauth-authorization-server", "openid-configuration"] {
        for url in well_known(base, resource) {
            if let Some(metadata) = fetched::<AuthorizationMetadata>(&url).await
                && is_usable(&metadata)
            {
                return Some(metadata);
            }
        }
    }
    None
}

/// Where RFC 9728 and RFC 8414 say a document about `base` is published: under
/// the path the endpoint lives at, either way round, then at the host root.
fn well_known(base: &Url, resource: &str) -> Vec<Url> {
    let at = |path: String| {
        let mut url = base.clone();
        url.set_query(None);
        url.set_fragment(None);
        url.set_path(&path);
        url
    };
    let canonical = format!("/.well-known/{resource}");
    let path = base.path().trim_matches('/');
    if path.is_empty() {
        return vec![at(canonical)];
    }
    vec![
        at(format!("{canonical}/{path}")),
        at(format!("/{path}/.well-known/{resource}")),
        at(canonical),
    ]
}

/// Whether a document names both endpoints a sign-in needs, at addresses an
/// authorization code may travel to.
fn is_usable(metadata: &AuthorizationMetadata) -> bool {
    [&metadata.authorization_endpoint, &metadata.token_endpoint]
        .into_iter()
        .all(|address| Url::parse(address).is_ok_and(|url| catalog::is_secure_address(&url)))
}

async fn fetched<T: DeserializeOwned>(url: &Url) -> Option<T> {
    if !catalog::is_secure_address(url) {
        return None;
    }
    let response = client::upstream_client()
        .ok()?
        .get(url.clone())
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    serde_json::from_slice(&response.bytes().await.ok()?).ok()
}

/// RFC 9728 protected resource metadata. Only the authorization servers it
/// names lead anywhere from here.
#[derive(Deserialize)]
struct ResourceMetadata {
    #[serde(default)]
    authorization_servers: Vec<String>,
}
