//! MCP endpoint behaviour: token routing, protocol negotiation, stateless
//! operation, group namespacing, log attribution, reload.

mod common;

use common::{spawn_app, TestApp};
use serde_json::{json, Value};

/// Create one integration and return (id, token).
fn integration(app: &TestApp, name: &str) -> (String, String) {
    let created = app
        .store
        .create_integration(&pluk_store::IntegrationInput::new(name.to_string(), "stub".to_string()))
        .expect("create integration");
    (created.id.clone(), created.token.clone())
}

fn group(app: &TestApp, name: &str, member_ids: &[String]) -> String {
    let members = member_ids
        .iter()
        .map(|id| pluk_store::GroupMember { id: id.clone(), overrides: Default::default() })
        .collect();
    let created = app
        .store
        .create_group(&pluk_store::GroupInput { name: name.to_string(), environment: None, members })
        .expect("create group");
    created.token
}

#[tokio::test]
async fn a_token_serves_its_integration_statelessly() {
    let app = spawn_app().await;
    let (_id, token) = integration(&app, "Main DB");

    // No initialize handshake is required: tools/list answers on its own.
    let (status, content_type, body) = app
        .mcp_post(
            &token,
            json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/list", "params": {} }),
        )
        .await;
    assert_eq!(status, 200);
    assert!(content_type.starts_with("application/json"), "stateless JSON replies");
    let names = body["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.contains(&"echo".to_string()));

    // The echo tool registered through ToolHost forwards description + schema.
    let echo = body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "echo")
        .cloned()
        .unwrap();
    assert_eq!(echo["description"], "Echo a value back");
    assert_eq!(echo["inputSchema"]["properties"]["value"]["type"], "string");

    let ping = body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "ping")
        .cloned()
        .unwrap();
    assert_eq!(ping["annotations"]["readOnlyHint"], true);

    // And the tool is callable.
    let (_, _, call) = app
        .mcp_post(
            &token,
            json!({ "jsonrpc": "2.0", "id": 8, "method": "tools/call", "params": {
                "name": "echo", "arguments": { "value": "hi there" } } }),
        )
        .await;
    assert_eq!(call["result"]["content"][0]["text"], "hi there");
    assert_eq!(call["result"]["isError"], false);

    // Prompts and resources ride along.
    let (_, _, prompts) = app
        .mcp_post(&token, json!({ "jsonrpc": "2.0", "id": 9, "method": "prompts/list" }))
        .await;
    assert_eq!(prompts["result"]["prompts"][0]["name"], "greet");

    let (_, _, read) = app
        .mcp_post(
            &token,
            json!({ "jsonrpc": "2.0", "id": 10, "method": "resources/read",
                    "params": { "uri": "schema://full" } }),
        )
        .await;
    assert_eq!(read["result"]["contents"][0]["text"], "everything");

    // Unknown tools are invalid params, not transport errors.
    let (_, _, unknown) = app
        .mcp_post(
            &token,
            json!({ "jsonrpc": "2.0", "id": 11, "method": "tools/call",
                    "params": { "name": "nope" } }),
        )
        .await;
    assert_eq!(unknown["error"]["code"], -32602);
    assert!(
        unknown["error"]["message"].as_str().unwrap_or_default().contains("Unknown tool"),
        "{}",
        unknown["error"]["message"]
    );
}

#[tokio::test]
async fn protocol_negotiation_serves_modern_and_legacy_clients() {
    let app = spawn_app().await;
    let (_id, token) = integration(&app, "Negotiator");

    let initialize = |version: &str, id: i64| {
        json!({
            "jsonrpc": "2.0", "id": id, "method": "initialize",
            "params": {
                "protocolVersion": version,
                "capabilities": {},
                "clientInfo": { "name": "test-client", "version": "1.0.0" }
            }
        })
    };

    // A modern client negotiates the current revision.
    let (_, _, modern) = app.mcp_post(&token, initialize("2026-07-28", 1)).await;
    assert_eq!(modern["result"]["protocolVersion"], "2026-07-28");

    // A Claude-Code-era client keeps its own revision.
    let (_, _, claude) = app.mcp_post(&token, initialize("2025-11-25", 2)).await;
    assert_eq!(claude["result"]["protocolVersion"], "2025-11-25");

    let (_, _, older) = app.mcp_post(&token, initialize("2025-06-18", 3)).await;
    assert_eq!(older["result"]["protocolVersion"], "2025-06-18");

    // Discovery carries server identity and instructions.
    assert_eq!(modern["result"]["serverInfo"]["name"], "Negotiator");
    assert!(modern["result"]["instructions"].as_str().unwrap_or_default().contains("Stub integration"));

    // Notifications are accepted with no body.
    assert_eq!(app.mcp_notify(&token, "notifications/initialized").await, 202);
}

#[tokio::test]
async fn an_unknown_token_is_not_found() {
    let app = spawn_app().await;
    let response = reqwest::get(format!("{}/mcp/not-a-real-token", app.base_url))
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    assert_eq!(response.text().await.unwrap(), "Integration not found");
}

#[tokio::test]
async fn an_integration_whose_adapter_is_missing_is_a_bad_request() {
    let app = spawn_app().await;
    let created = app
        .store
        .create_integration(&pluk_store::IntegrationInput::new("Ghost".to_string(), "no-such-adapter".to_string()))
        .unwrap();
    let (status, _, _) = app
        .mcp_post(&created.token, json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn group_members_get_collision_free_namespaces_and_their_tools_work() {
    let app = spawn_app().await;
    // Two same-named members: their slugs collide, so the second gets _2.
    let (first_id, _) = integration(&app, "Metrics DB");
    let (second_id, _) = integration(&app, "Metrics DB");
    let token = group(&app, "Warehouse", &[first_id.clone(), second_id.clone()]);

    let (_, _, listed) = app
        .mcp_post(&token, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
        .await;
    let mut names: Vec<String> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    let mut expected = [
        "metrics_db__echo",
        "metrics_db__ping",
        "metrics_db_2__echo",
        "metrics_db_2__ping",
    ];
    expected.sort_unstable();
    assert_eq!(names, expected, "collision gets a _2 suffix");

    // Resources are namespaced by URI, not just name.
    let (_, _, resources) = app
        .mcp_post(&token, json!({ "jsonrpc": "2.0", "id": 2, "method": "resources/list" }))
        .await;
    let uris: Vec<&str> = resources["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["uri"].as_str().unwrap())
        .collect();
    assert!(uris.contains(&"schema://metrics_db/full"));
    assert!(uris.contains(&"schema://metrics_db_2/full"));

    // Group instructions embed each member's own block under its prefix.
    let (_, _, init) = app
        .mcp_post(
            &token,
            json!({
                "jsonrpc": "2.0", "id": 3, "method": "initialize",
                "params": { "protocolVersion": "2025-11-25", "capabilities": {},
                            "clientInfo": { "name": "c", "version": "1" } }
            }),
        )
        .await;
    let instructions = init["result"]["instructions"].as_str().unwrap();
    assert!(instructions.contains("Group \"Warehouse\" fronts 2 integrations."), "{instructions}");
    assert!(instructions.contains("metrics_db__<tool>"));
    assert!(instructions.contains("Member tools prefixed \"metrics_db_2__\":"));
}

#[tokio::test]
async fn a_call_through_a_group_is_attributed_to_it_in_the_log() {
    let app = spawn_app().await;
    let (member_id, _) = integration(&app, "Attribution DB");
    let token = group(&app, "Front Group", &[member_id.clone()]);
    let group_row = app.store.list_groups().unwrap().into_iter().next().unwrap();

    app.mcp_post(
        &token,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
            "name": "attribution_db__echo", "arguments": { "value": "logged" } } }),
    )
    .await;
    let page = app
        .store
        .read_log_page(&pluk_store::LogScope::Group(group_row.id.clone()), pluk_store::LogRange::All, None)
        .unwrap();
    assert_eq!(page.entries.len(), 1, "the call lands in the group's view");
    assert_eq!(page.entries[0].group_name.as_deref(), Some("Front Group"));
    assert_eq!(page.entries[0].connection_id, member_id);
    assert_eq!(page.entries[0].verdict, "allowed");

    // The standalone view sees it too (same row, attributed to the member).
    let standalone = app
        .store
        .read_log_page(&pluk_store::LogScope::Connection(member_id.clone()), pluk_store::LogRange::All, None)
        .unwrap();
    assert_eq!(standalone.entries.len(), 1);
}

#[tokio::test]
async fn per_member_overrides_are_coerced_before_registration() {
    let app = spawn_app().await;
    let (member_id, _) = integration(&app, "Override DB");
    // Override `endpoint` for this membership only; blank retries inherits.
    let token = {
        let members = vec![pluk_store::GroupMember {
            id: member_id.clone(),
            overrides: serde_json::from_str::<serde_json::Map<String, Value>>(
                r#"{"endpoint":"overridden-host","retries":"","verbose":"true"}"#,
            )
            .unwrap(),
        }];
        app.store
            .create_group(&pluk_store::GroupInput { name: "Scoped".into(), environment: None, members })
            .unwrap()
            .token
    };

    let (_, _, init) = app
        .mcp_post(
            &token,
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2025-11-25", "capabilities": {},
                            "clientInfo": { "name": "c", "version": "1" } }
            }),
        )
        .await;
    let instructions = init["result"]["instructions"].as_str().unwrap();
    assert!(
        instructions.contains("Endpoint: overridden-host"),
        "override must reach registration: {instructions}"
    );
    assert!(!instructions.contains("retries"), "blank override must not surface");
}

#[tokio::test]
async fn reload_closes_an_owners_resources_and_reports_the_count() {
    let app = spawn_app().await;
    let (owner_a, token_a) = integration(&app, "Reloadable");
    let (owner_b, token_b) = integration(&app, "Untouched");

    // Opening both owners through real requests registers their scopes.
    app.mcp_post(&token_a, json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })).await;
    app.mcp_post(&token_b, json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })).await;
    assert!(app.owners.owner_token(&owner_a).is_some());
    assert!(app.owners.owner_token(&owner_b).is_some());

    // Scoped reload closes exactly that owner.
    let response = reqwest::Client::new()
        .post(format!("{}/api/reload?id={owner_a}", app.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let payload: Value = response.json().await.unwrap();
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["count"], 1);
    assert!(app.owners.owner_token(&owner_a).is_none(), "closed scope must be gone");
    assert!(app.owners.owner_token(&owner_b).is_some(), "other owners stay open");

    let closed = app.closed_owners.lock().unwrap().clone();
    assert!(closed.contains(&owner_a), "close hooks observe the owner");
    assert!(!closed.contains(&owner_b));

    // Unscoped reload closes everything left; an unknown owner counts zero.
    let payload: Value = reqwest::Client::new()
        .post(format!("{}/api/reload?id=who-is-this", app.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(payload["count"], 0);
    let payload: Value = reqwest::Client::new()
        .post(format!("{}/api/reload", app.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(payload["count"], 1, "only owner B remains");
}

#[tokio::test]
async fn get_on_the_mcp_endpoint_has_no_session_stream() {
    let app = spawn_app().await;
    let (_id, token) = integration(&app, "Streamless");
    let status = reqwest::get(format!("{}/mcp/{token}", app.base_url))
        .await
        .unwrap()
        .status()
        .as_u16();
    assert_eq!(status, 405, "stateless serving has no GET stream");
}
