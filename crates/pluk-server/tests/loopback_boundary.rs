//! The loopback boundary: which callers reach the HTTP surface at all.

mod common;

use common::{TestApp, spawn_app, spawn_app_with_browser};
use reqwest::{Client, Method, Response};
use serde_json::json;

/// A Chrome extension origin: 32 letters in a–p, as Chrome mints them.
const EXTENSION_ORIGIN: &str = "chrome-extension://abcdefghijklmnopabcdefghijklmnop";

/// The headers a browser attaches when it follows a redirect into a new page.
const TOP_LEVEL_NAVIGATION: [(&str, &str); 3] = [
    ("sec-fetch-site", "cross-site"),
    ("sec-fetch-mode", "navigate"),
    ("sec-fetch-dest", "document"),
];

fn integration(app: &TestApp, name: &str) -> (String, String) {
    let created = app
        .store
        .create_integration(&pluk_store::IntegrationInput::new(
            name.to_string(),
            "stub".to_string(),
        ))
        .expect("create integration");
    (created.id.clone(), created.token.clone())
}

async fn send(app: &TestApp, method: Method, path: &str, headers: &[(&str, &str)]) -> Response {
    let mut request = Client::new().request(method, format!("{}{path}", app.base_url));
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    request.send().await.expect("request")
}

#[tokio::test]
async fn a_request_arriving_under_another_host_is_refused_on_every_route() {
    let app = spawn_app().await;
    let (_, token) = integration(&app, "Rebound");
    let mcp = format!("/mcp/{token}");

    for path in ["/api/adapters", &mcp, "/oauth/mcp/callback?code=c&state=s"] {
        let response = send(&app, Method::GET, path, &[("host", "evil.example")]).await;
        assert_eq!(response.status(), 403, "{path} answered a rebound host");
    }
}

#[tokio::test]
async fn a_web_page_cannot_post_to_the_api_but_a_local_client_can() {
    let app = spawn_app().await;
    let (id, _) = integration(&app, "Proxied");
    let approve = format!("/api/integrations/{id}/proxy/approve");

    let refused = send(
        &app,
        Method::POST,
        &approve,
        &[("origin", "https://evil.example")],
    )
    .await;
    assert_eq!(refused.status(), 403);
    assert_eq!(
        refused.text().await.expect("body"),
        "Pluk does not answer requests made by a web page."
    );

    // No Origin at all is how an agent, the CLI and the desktop app arrive.
    let served = send(&app, Method::POST, &approve, &[]).await;
    assert_ne!(
        served.status(),
        403,
        "a local client must reach the adapter"
    );
    let ping = send(
        &app,
        Method::POST,
        &format!("/api/integrations/{id}/ping"),
        &[],
    )
    .await;
    assert_eq!(ping.status(), 200);
}

#[tokio::test]
async fn an_agent_speaking_mcp_over_loopback_is_untouched() {
    let app = spawn_app().await;
    let (_, token) = integration(&app, "Agent reachable");

    let (status, _, body) = app
        .mcp_post(
            &token,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["result"]["tools"][0]["name"], "echo");
}

#[tokio::test]
async fn the_sign_in_redirect_lands_while_a_cross_site_post_to_it_does_not() {
    let app = spawn_app().await;

    let landed = send(
        &app,
        Method::GET,
        "/oauth/mcp/callback?code=c&state=s",
        &TOP_LEVEL_NAVIGATION,
    )
    .await;
    assert_eq!(landed.status(), 200);
    assert_eq!(landed.text().await.expect("body"), "signed in");

    let submitted = send(
        &app,
        Method::POST,
        "/oauth/mcp/callback?code=c&state=s",
        &TOP_LEVEL_NAVIGATION,
    )
    .await;
    assert_eq!(submitted.status(), 403);
}

#[tokio::test]
async fn the_extension_reaches_wande_from_its_own_origin_and_nothing_else() {
    let app = spawn_app_with_browser().await;

    let paired = send(
        &app,
        Method::GET,
        "/wande/healthz",
        &[("origin", EXTENSION_ORIGIN)],
    )
    .await;
    assert_eq!(paired.status(), 200);

    let elsewhere = send(
        &app,
        Method::GET,
        "/api/adapters",
        &[("origin", EXTENSION_ORIGIN)],
    )
    .await;
    assert_eq!(elsewhere.status(), 403);

    let page = send(
        &app,
        Method::GET,
        "/wande/healthz",
        &[("origin", "https://evil.example")],
    )
    .await;
    assert_eq!(page.status(), 403);
}
