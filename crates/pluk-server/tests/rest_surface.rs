//! The REST surface: adapter catalog, health, log paging.

mod common;

use serde_json::Value;

use common::{TestApp, spawn_app};

use pluk_server::HealthStatus;
use pluk_store::LOG_PAGE_SIZE;

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

#[tokio::test]
async fn the_adapter_catalog_serves_definitions_never_secret_values() {
    let app = spawn_app().await;
    let (status, body) = app.get_json("/api/adapters").await;
    assert_eq!(status, 200);

    let adapters = body["adapters"].as_array().expect("adapters array");
    assert_eq!(adapters.len(), 1);
    let stub = &adapters[0];
    assert_eq!(stub["id"], "stub");
    assert_eq!(stub["label"], "Stub Service");
    assert_eq!(stub["policyKind"], "none");
    assert_eq!(stub["agentHint"], "Use echo first.");
    assert_eq!(stub["offeredForSetup"], true);

    let fields = stub["configFields"].as_array().unwrap();
    assert_eq!(fields.len(), 4);
    // Definitions only: a field describes its input, it never carries a value.
    for field in fields {
        assert!(
            field.get("value").is_none(),
            "catalog leaked a value: {field}"
        );
    }
    assert_eq!(fields[2]["type"], "password");
    assert_eq!(fields[2]["secret"], true);

    let tools = stub["tools"].as_array().unwrap();
    assert_eq!(tools[0]["name"], "echo");
    assert_eq!(tools[0]["defaultEnabled"], true);
}

#[tokio::test]
async fn health_reports_not_checked_ok_and_error_as_three_states() {
    let app = spawn_app().await;
    let (ok_id, _) = integration(&app, "Healthy");
    let (bad_id, _) = integration(&app, "Broken");

    // Nothing tested yet: absent is its own state.
    let (status, body) = app.get_json("/api/health").await;
    assert_eq!(status, 200);
    assert!(
        body["health"].get(&ok_id).is_none(),
        "untested connections stay absent"
    );
    assert!(body["health"].get(&bad_id).is_none());

    app.health.record(&ok_id, HealthStatus::Ok, None);
    app.health.record(
        &bad_id,
        HealthStatus::Error,
        Some("stub refuses connections".into()),
    );

    let (_, body) = app.get_json("/api/health").await;
    assert_eq!(body["health"][&ok_id]["status"], "ok");
    assert_eq!(body["health"][&ok_id].get("error"), None);
    assert_eq!(body["health"][&bad_id]["status"], "error");
    assert_eq!(body["health"][&bad_id]["error"], "stub refuses connections");
}

#[tokio::test]
async fn a_connection_test_is_not_run_over_loopback() {
    let app = spawn_app().await;
    let (id, _) = integration(&app, "Untested");
    for path in [
        format!("/api/integrations/{id}/test"),
        "/api/integrations/ghost/test".to_string(),
    ] {
        let response = reqwest::Client::new()
            .post(format!("{}{path}", app.base_url))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403, "{path}");
    }
    let (_, body) = app.get_json("/api/health").await;
    assert!(body["health"].get(&id).is_none(), "nothing was tested");
}

#[tokio::test]
async fn logs_page_across_a_keyset_boundary_without_gaps_or_duplicates() {
    let app = spawn_app().await;
    let (conn_id, _) = integration(&app, "Paged");
    let total = LOG_PAGE_SIZE + 3;
    let ids = app.insert_logs(total, &conn_id);

    let first = reqwest::get(format!(
        "{base}/api/logs?connectionId={conn_id}&range=all",
        base = app.base_url
    ))
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap();
    assert_eq!(first["entries"].as_array().unwrap().len(), LOG_PAGE_SIZE);
    assert_eq!(first["hasMore"], true);
    let cursor = first["nextCursor"].as_object().expect("cursor object");
    assert!(cursor.contains_key("createdAt") && cursor.contains_key("id"));

    let next_cursor_time = first["nextCursor"]["createdAt"]
        .as_str()
        .unwrap()
        .to_string();
    let next_cursor_id = first["nextCursor"]["id"].as_i64().unwrap();
    let second = reqwest::get(format!(
        "{base}/api/logs?connectionId={conn_id}&range=all&cursorTime={t}&cursorId={i}",
        base = app.base_url,
        t = urlencoding_encode(&next_cursor_time),
        i = next_cursor_id,
    ))
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap();

    let mut seen: Vec<i64> = first["entries"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["entries"].as_array().unwrap())
        .map(|e| e["id"].as_i64().unwrap())
        .collect();
    seen.sort_unstable();
    assert_eq!(seen.len(), total, "no duplicates");
    seen.sort_unstable();
    let mut expected = ids.clone();
    expected.sort_unstable();
    assert_eq!(seen, expected, "no gaps");
    assert_eq!(second["hasMore"], false);
    assert!(!second["entries"].as_array().unwrap().is_empty());
}

fn urlencoding_encode(raw: &str) -> String {
    raw.replace(' ', "%20")
}

#[tokio::test]
async fn group_scoped_reads_return_group_rows_in_camel_case() {
    let app = spawn_app().await;
    let (member, _) = integration(&app, "Member");
    app.store
        .create_log_entry(
            pluk_store::LogDraft::new(member.clone(), "Member", "select 1").with_group(
                pluk_store::LogGroup {
                    id: "group-a".into(),
                    name: "Group A".into(),
                },
            ),
        )
        .unwrap();

    let response = reqwest::get(format!("{}/api/logs?groupId=group-a", app.base_url))
        .await
        .unwrap();
    let body: Value = response.json().await.unwrap();
    let entry = &body["entries"][0];
    assert_eq!(entry["groupId"], "group-a");
    assert_eq!(entry["groupName"], "Group A");
    assert_eq!(entry["connectionName"], "Member");
    // camelCase wire shape throughout.
    assert!(entry.get("connection_id").is_none());
    assert!(entry.get("rowCount").is_some());
    assert_eq!(body["hasMore"], false);
    assert!(body["nextCursor"].is_null());
}

#[tokio::test]
async fn log_read_validation_rejects_bad_scopes_ranges_and_cursors() {
    let app = spawn_app().await;
    let cases = [
        ("/api/logs", 400),                                           // no scope
        ("/api/logs?connectionId=&range=all", 400),                   // empty scope
        ("/api/logs?connectionId=a&groupId=b", 400),                  // two scopes
        ("/api/logs?connectionId=a&range=tomorrow", 400),             // bad range
        ("/api/logs?connectionId=a&cursorTime=nope&cursorId=1", 400), // bad time
        (
            "/api/logs?connectionId=a&cursorTime=2026-01-01%2000%3A00%3A00",
            400,
        ), // half cursor
        (
            "/api/logs?connectionId=a&cursorTime=2026-01-01 00:00:00&cursorId=0",
            400,
        ), // zero id
    ];
    for (path, expected) in cases {
        let status = app.get_status(path).await;
        assert_eq!(status, expected, "{path}");
    }
}

#[tokio::test]
async fn per_integration_adapter_routes_are_refused_and_global_ones_answer() {
    let app = spawn_app().await;
    let (conn_id, _) = integration(&app, "Routed");

    // The adapter would answer `/ping`, but only the window may reach it.
    for tail in ["ping", "unknown"] {
        let response = reqwest::Client::new()
            .post(format!(
                "{}/api/integrations/{conn_id}/{tail}",
                app.base_url
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403, "{tail}");
        assert_eq!(
            response.text().await.unwrap(),
            "Only the Pluk window can change an integration's settings."
        );
    }

    // Global handlers still answer unmatched paths.
    let body = reqwest::get(format!("{}/api/stub-global", app.base_url))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "global");
}

#[tokio::test]
async fn plain_health_and_fallbacks_behave_like_the_typescript_server() {
    let app = spawn_app().await;
    assert_eq!(
        reqwest::get(format!("{}/health", app.base_url))
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "ok"
    );
    let response = reqwest::get(format!("{}/nothing/here", app.base_url))
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    assert_eq!(response.text().await.unwrap(), "Not found");
}
