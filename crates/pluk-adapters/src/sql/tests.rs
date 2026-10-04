use super::{SqlCancelRegistry, register_sql_server};
use crate::adapter::Adapter;
use crate::tool_host::{PromptHandler, ResourceHandler, ToolHandler, ToolHost, ToolRegistration};
use pluk_store::{Integration, LogRange, LogScope, Store};
use serde_json::{Map, Value, json};
use std::sync::Arc;

fn temp_store() -> (tempfile::TempDir, Arc<Store>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&dir.path().join("pluk.db")).unwrap());
    (dir, store)
}

fn make_integration(
    id: &str,
    type_name: &str,
    config: Value,
    query_policy: Option<&str>,
) -> Integration {
    Integration {
        id: id.to_string(),
        name: format!("test-{}", id),
        r#type: type_name.to_string(),
        config: config.as_object().cloned().unwrap_or_default(),
        environment: None,
        read_only: 0,
        query_policy: query_policy.map(|s| s.to_string()),
        token: "tok".to_string(),
        created_at: "2026-01-01".to_string(),
        via_group: None,
    }
}

struct CaptureHost {
    tools: std::collections::HashMap<String, ToolHandler>,
    tools_meta: std::collections::HashMap<String, ToolRegistration>,
    prompts: std::collections::HashMap<String, (String, Option<Map<String, Value>>)>,
    resources: std::collections::HashMap<String, (String, String)>,
    resource_handlers: std::collections::HashMap<String, ResourceHandler>,
}
impl CaptureHost {
    fn new() -> Self {
        Self {
            tools: std::collections::HashMap::new(),
            tools_meta: std::collections::HashMap::new(),
            prompts: std::collections::HashMap::new(),
            resources: std::collections::HashMap::new(),
            resource_handlers: std::collections::HashMap::new(),
        }
    }
}
impl ToolHost for CaptureHost {
    fn register_tool(&mut self, reg: ToolRegistration, handler: ToolHandler) {
        self.tools_meta.insert(reg.name.clone(), reg.clone());
        self.tools.insert(reg.name, handler);
    }
    fn register_prompt(
        &mut self,
        name: &str,
        desc: &str,
        args: Option<Map<String, Value>>,
        _h: PromptHandler,
    ) {
        self.prompts
            .insert(name.to_string(), (desc.to_string(), args));
    }
    fn register_resource(
        &mut self,
        name: &str,
        uri: &str,
        mime: &str,
        _desc: Option<&str>,
        handler: ResourceHandler,
    ) {
        self.resources
            .insert(uri.to_string(), (name.to_string(), mime.to_string()));
        self.resource_handlers.insert(uri.to_string(), handler);
    }
}

fn capture_for(conn: &Integration, store: Arc<Store>) -> CaptureHost {
    let mut host = CaptureHost::new();
    let cancels = Arc::new(SqlCancelRegistry::default());
    register_sql_server(&mut host, conn, "owner1", store, cancels).unwrap();
    host
}

#[tokio::test]
async fn list_tables_recovers_when_the_first_connection_is_dead() {
    use super::{ConnectStep, TEST_CONNECT};
    let (_dir, store) = temp_store();
    let conn = make_integration("retry", "postgres", json!({}), None);
    let host = capture_for(&conn, store);
    let steps = std::sync::Mutex::new(std::collections::VecDeque::from([
        ConnectStep::Fail(pluk_db::DriverError::Connection("connection failed".into())),
        ConnectStep::Ready(pluk_db::fake::FakeDriver::new_postgres()),
    ]));
    TEST_CONNECT.scope(steps, async {
        let result = host.tools["list_tables"](json!({})).await;
        assert!(!result.is_error, "{}", result.text());
        assert!(result.text().contains("users"));
    }).await;
}

#[tokio::test]
async fn a_write_with_an_unknown_outcome_is_not_sent_twice() {
    use super::{ConnectStep, TEST_CONNECT};
    let (_dir, store) = temp_store();
    let conn = make_integration("write", "postgres", json!({}), Some(r#"{"tools":{"query":{"settings":{"mode":"mutations"}}}}"#));
    let host = capture_for(&conn, store);
    let driver = pluk_db::fake::FakeDriver::new_postgres();
    driver.fail_next_query(pluk_db::DriverError::Query("connection closed".into()));
    let steps = std::sync::Mutex::new(std::collections::VecDeque::from([
        ConnectStep::Ready(driver.clone()),
        ConnectStep::Ready(driver.clone()),
    ]));
    TEST_CONNECT.scope(steps, async {
        let result = host.tools["query"](json!({"sql":"INSERT INTO t VALUES (1)"})).await;
        assert!(result.is_error);
        assert!(result.text().contains("Do not retry"), "{}", result.text());
        assert_eq!(driver.queries(), vec!["INSERT INTO t VALUES (1)"]);
    }).await;
}

#[tokio::test(start_paused = true)]
async fn a_connection_that_never_answers_fails_before_the_client_timeout() {
    use super::{ConnectStep, TEST_CONNECT};
    let (_dir, store) = temp_store();
    let conn = make_integration("timeout", "postgres", json!({"use_ssh":true,"ssh_host":"app4-ssh-infra"}), None);
    let host = capture_for(&conn, store);
    let steps = std::sync::Mutex::new(std::collections::VecDeque::from([
        ConnectStep::Hang,
        ConnectStep::Hang,
    ]));
    TEST_CONNECT.scope(steps, async {
        let start = tokio::time::Instant::now();
        let result = host.tools["list_tables"](json!({})).await;
        assert!(result.is_error);
        assert!(start.elapsed() <= std::time::Duration::from_secs(45));
        assert!(result.text().contains("SSH tunnel"), "{}", result.text());
        assert!(result.text().contains("app4-ssh-infra"));
        assert!(result.text().contains("Retrying is safe"));
    }).await;
}

#[tokio::test]
async fn a_read_only_query_recovers_when_the_connection_drops_during_the_statement() {
    use super::{ConnectStep, TEST_CONNECT};
    let (_dir, store) = temp_store();
    let conn = make_integration("read-drop", "postgres", json!({}), None);
    let host = capture_for(&conn, store);
    let driver = pluk_db::fake::FakeDriver::new_postgres();
    driver.fail_next_query(pluk_db::DriverError::Query("connection closed".into()));
    let steps = std::sync::Mutex::new(std::collections::VecDeque::from([
        ConnectStep::Ready(driver.clone()),
        ConnectStep::Ready(driver),
    ]));
    TEST_CONNECT.scope(steps, async {
        let result = host.tools["query"](json!({"sql":"SELECT 1"})).await;
        assert!(!result.is_error, "{}", result.text());
        let payload: Value = serde_json::from_str(result.text()).unwrap();
        assert_eq!(payload["rows"][0]["ok"], 1);
    }).await;
}


#[tokio::test(start_paused = true)]
async fn a_failed_call_records_how_long_it_waited_and_keeps_it_after_reopen() {
    use super::{ConnectStep, TEST_CONNECT};
    let (dir, store) = temp_store();
    let conn = make_integration("duration", "postgres", json!({}), None);
    let host = capture_for(&conn, store.clone());
    let steps = std::sync::Mutex::new(std::collections::VecDeque::from([ConnectStep::Hang]));
    TEST_CONNECT.scope(steps, async {
        let result = host.tools["list_tables"](json!({})).await;
        assert!(result.is_error);
        let page = store.read_log_page(&LogScope::Connection("duration".into()), LogRange::All, None).unwrap();
        assert_eq!(page.entries[0].verdict, "error");
        assert_eq!(page.entries[0].duration_ms, Some(45_000));
    }).await;
    drop(host);
    drop(store);
    let reopened = Store::open(&dir.path().join("pluk.db")).unwrap();
    assert_eq!(reopened.read_log_page(&LogScope::Connection("duration".into()), LogRange::All, None).unwrap().entries[0].duration_ms, Some(45_000));
}

#[tokio::test]
async fn query_happy_path_returns_rows() {
    let (_dir, store) = temp_store();
    let conn = make_integration("pg1", "postgres", json!({"host":"localhost"}), None);
    let host = capture_for(&conn, store.clone());
    let handler = host.tools.get("query").expect("query tool");
    let res = handler(json!({"sql":"SELECT 1"})).await;
    if res.is_error && res.text().contains("connection failed") {
        eprintln!(
            "skip: no postgres reachable for query_happy_path: {}",
            res.text()
        );
        return;
    }
    assert!(!res.is_error, "query should succeed: {}", res.text());
    let v: Value = serde_json::from_str(res.text()).unwrap();
    assert!(v.get("rows").is_some());
}

#[tokio::test]
async fn each_tool_happy_path() {
    let (_dir, store) = temp_store();
    // needs a saved query for run_saved_query
    let conn = make_integration("pg1", "postgres", json!({"host":"localhost"}), None);
    store
        .create_saved_query(&pluk_store::SavedQueryInput {
            connection_id: "pg1".into(),
            name: "myq".into(),
            sql: "SELECT 1".into(),
        })
        .unwrap();
    let host = capture_for(&conn, store.clone());
    let cases: Vec<(&str, Value)> = vec![
        ("query", json!({"sql":"SELECT 1"})),
        ("list_tables", json!({})),
        ("sample_table", json!({"table":"users"})),
        ("describe_table", json!({"table":"users"})),
        ("search_schema", json!({"term":"user"})),
    ];
    for (name, args) in cases {
        let h = host
            .tools
            .get(name)
            .unwrap_or_else(|| panic!("missing {}", name));
        let r = h(args).await;
        if r.is_error && r.text().contains("connection failed") {
            eprintln!(
                "skip each_tool_happy_path: no postgres reachable for {name}: {}",
                r.text()
            );
            return;
        }
        assert!(!r.is_error, "{} failed: {}", name, r.text());
    }
    // tools default off should not be present
    assert!(host.tools.get("explain_query").is_none());
    assert!(host.tools.get("table_stats").is_none());
    assert!(host.tools.get("list_schemas").is_none());
    assert!(host.tools.get("list_databases").is_none());
    // enable them via policy
    let policy = r#"{"tools":{"explain_query":{"enabled":true},"list_relationships":{"enabled":true},"table_stats":{"enabled":true},"list_schemas":{"enabled":true},"list_databases":{"enabled":true},"export_query":{"enabled":true},"run_saved_query":{"enabled":true},"list_saved_queries":{"enabled":true}}}"#;
    let conn2 = make_integration("pg2", "postgres", json!({"host":"localhost"}), Some(policy));
    store
        .create_saved_query(&pluk_store::SavedQueryInput {
            connection_id: "pg2".into(),
            name: "myq".into(),
            sql: "SELECT 1".into(),
        })
        .unwrap();
    let host2 = capture_for(&conn2, store.clone());
    let extra: Vec<(&str, Value)> = vec![
        ("explain_query", json!({"sql":"SELECT 1"})),
        ("list_relationships", json!({})),
        ("table_stats", json!({"table":"users"})),
        ("list_schemas", json!({})),
        ("list_databases", json!({})),
        ("export_query", json!({"sql":"SELECT 1","format":"csv"})),
        ("run_saved_query", json!({"name":"myq"})),
        ("list_saved_queries", json!({})),
    ];
    for (name, args) in extra {
        let h = host2
            .tools
            .get(name)
            .unwrap_or_else(|| panic!("missing opt-in {}", name));
        let r = h(args).await;
        if r.is_error && r.text().contains("connection failed") {
            eprintln!(
                "skip each_tool_happy_path extra: no postgres reachable for {name}: {}",
                r.text()
            );
            return;
        }
        assert!(!r.is_error, "{} failed: {}", name, r.text());
    }
}

#[tokio::test]
async fn successful_query_logs_result_json_without_response_text() {
    let (_dir, store) = temp_store();
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("regress.sqlite");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE t (id INTEGER, name TEXT)", [])
            .unwrap();
        conn.execute("INSERT INTO t VALUES (1, 'a'), (2, 'b')", [])
            .unwrap();
    }
    let conn = make_integration(
        "sq1",
        "sqlite",
        json!({"filename": db_path.to_str().unwrap()}),
        None,
    );
    let host = capture_for(&conn, store.clone());
    let h = host.tools.get("query").unwrap();
    let r = h(json!({"sql": "SELECT * FROM t"})).await;
    assert!(!r.is_error, "query should succeed: {}", r.text());

    let page = store
        .read_log_page(&LogScope::Connection("sq1".into()), LogRange::All, None)
        .unwrap();
    let entry = page
        .entries
        .iter()
        .find(|e| e.source.as_deref() == Some("query"))
        .expect("query log entry");
    assert_eq!(entry.verdict, "allowed");
    assert!(
        entry.result_json.is_some(),
        "result_json must be set so the activity log can render a table"
    );
    assert!(
        entry.response_text.is_none(),
        "response_text must stay unset on a successful query — setting it makes the UI fall back to the raw-JSON preview instead of the table"
    );
    let result: Value = serde_json::from_str(entry.result_json.as_deref().unwrap()).unwrap();
    assert_eq!(result["rows"].as_array().unwrap().len(), 2);
}


#[tokio::test]
async fn use_is_blocked() {
    let (_dir, store) = temp_store();
    let conn = make_integration("pg1", "postgres", json!({"host":"localhost"}), None);
    let host = capture_for(&conn, store.clone());
    let h = host.tools.get("query").unwrap();
    let r = h(json!({"sql":"USE otherdb"})).await;
    assert!(r.is_error);
    assert!(r.text().contains("USE is blocked"));
    // pinned variant
    let conn_pinned = make_integration(
        "pg2",
        "postgres",
        json!({"host":"localhost","database":"app"}),
        None,
    );
    let host2 = capture_for(&conn_pinned, store);
    let h2 = host2.tools.get("query").unwrap();
    let r2 = h2(json!({"sql":"USE otherdb"})).await;
    assert!(r2.text().contains("locked to database"));
}


#[tokio::test]
async fn blocked_statement_produces_no_pending_row() {
    let (_dir, store) = temp_store();
    // policy read-only, try insert
    let conn = make_integration("pg1", "postgres", json!({"host":"localhost"}), None);
    let host = capture_for(&conn, store.clone());
    let h = host.tools.get("query").unwrap();
    let r = h(json!({"sql":"INSERT INTO t VALUES (1)"})).await;
    assert!(r.is_error);
    assert!(r.text().contains("Blocked"));
    let page = store
        .read_log_page(&LogScope::Connection("pg1".into()), LogRange::All, None)
        .unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].verdict, "blocked");
    // ensure no pending
    for e in page.entries {
        assert_ne!(e.verdict, "pending");
    }
}

#[tokio::test]
async fn cancelled_query_recorded_as_cancelled() {
    let (_dir, store) = temp_store();
    let _conn = make_integration("pg1", "postgres", json!({"host":"localhost"}), None);
    // Use gate directly to simulate cancellation via driver error
    use crate::error::AdapterError;
    use crate::gate::{CallTarget, GateMeta, GateOpts, cancelled_when_message_contains, run_gated};

    let target = CallTarget::new("pg1", "test-pg1");
    let meta = GateMeta::new("read", "query", "SELECT pg_sleep(10)");
    let res = run_gated(
        &store,
        &target,
        meta,
        |_| async { Err(AdapterError::new("Query cancelled")) },
        GateOpts::default().classify_error(cancelled_when_message_contains("cancelled")),
    )
    .await;
    assert!(res.is_error);
    assert!(res.text().contains("Cancelled"));
    let page = store
        .read_log_page(&LogScope::Connection("pg1".into()), LogRange::All, None)
        .unwrap();
    assert_eq!(page.entries[0].verdict, "cancelled");
}

#[tokio::test]
async fn param_rejection_on_remote_sqlite() {
    let (_dir, store) = temp_store();
    let conn = make_integration(
        "sq1",
        "sqlite",
        json!({"filename":"/tmp/x.db","use_ssh":"true","ssh_host":"bastion"}),
        None,
    );
    let host = capture_for(&conn, store);
    let h = host.tools.get("query").unwrap();
    let r = h(json!({"sql":"SELECT 1","params":[1]})).await;
    assert!(r.is_error);
    assert!(
        r.text().contains("Bind parameters are not supported"),
        "got {}",
        r.text()
    );
}




#[test]
fn error_humanising_cancel_vs_failure() {
    use crate::error::AdapterError;
    use crate::sql::error::{classify_sql_error, humanize_sql_error};
    let cancelled = AdapterError::new("Query cancelled");
    let _info = classify_sql_error(&cancelled);
    // not pending, not auth, should be query_failed with message "Query cancelled"
    assert!(
        humanize_sql_error(&cancelled).contains("Query cancelled")
            || humanize_sql_error(&cancelled).contains("cancelled")
    );
    let auth = AdapterError::new("SASL authentication failed").with_code("28P01");
    let info2 = classify_sql_error(&auth);
    assert_eq!(
        info2.category,
        crate::sql::error::SqlErrorCategory::AuthFailed
    );
}


#[tokio::test]
async fn bind_params_postgres_and_mysql() {
    let (_dir, store) = temp_store();
    let conn_pg = make_integration("pg1", "postgres", json!({"host":"localhost"}), None);
    let host_pg = capture_for(&conn_pg, store.clone());
    let h = host_pg.tools.get("query").unwrap();
    let r = h(json!({"sql":"SELECT $1::int + $2::int","params":[1,2]})).await;
    if r.is_error && r.text().to_lowercase().contains("connection") {
        eprintln!("skip bind_params postgres: no pg reachable: {}", r.text());
    } else {
        assert!(
            !r.is_error,
            "postgres $1 params should succeed: {}",
            r.text()
        );
    }

    let conn_my = make_integration("my1", "mysql", json!({"host":"localhost"}), None);
    let host_my = capture_for(&conn_my, store);
    let h2 = host_my.tools.get("query").unwrap();
    let r2 = h2(json!({"sql":"SELECT ? + ?","params":[1,2]})).await;
    if r2.is_error && r2.text().to_lowercase().contains("connection") {
        eprintln!("skip bind_params mysql: no mysql reachable: {}", r2.text());
        return;
    }
    assert!(!r2.is_error, "mysql ? params should succeed: {}", r2.text());
}

#[tokio::test]
async fn api_saved_query_and_masked_column_crud() {
    let (_dir, store) = temp_store();
    let conn = make_integration("pg1", "postgres", json!({"host":"localhost"}), None);
    let cancels = Arc::new(SqlCancelRegistry::default());
    let adapter = crate::sql::SqlAdapter::postgres(store.clone(), cancels);
    // saved query CRUD via handle_api
    let req_create = crate::adapter::ApiRequest {
        method: "POST".into(),
        url: "/api/integrations/pg1/saved_queries".into(),
        body: Some(r#"{"name":"q1","sql":"SELECT 1"}"#.into()),
    };
    let resp = adapter
        .as_ref()
        .handle_api(&conn, req_create, "/saved_queries")
        .await
        .unwrap();
    assert_eq!(resp.status, 200);
    let body: Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(body["ok"], true);
    let req_list = crate::adapter::ApiRequest {
        method: "GET".into(),
        url: "/api/integrations/pg1/saved_queries".into(),
        body: None,
    };
    let resp2 = adapter
        .as_ref()
        .handle_api(&conn, req_list, "/saved_queries")
        .await
        .unwrap();
    let body2: Value = serde_json::from_slice(&resp2.body).unwrap();
    assert_eq!(body2["queries"].as_array().unwrap().len(), 1);
    // masked column CRUD
    let req_add = crate::adapter::ApiRequest {
        method: "POST".into(),
        url: "/api/integrations/pg1/masked_columns".into(),
        body: Some(r#"{"column_name":"secret"}"#.into()),
    };
    let resp3 = adapter
        .as_ref()
        .handle_api(&conn, req_add, "/masked_columns")
        .await
        .unwrap();
    assert_eq!(resp3.status, 200);
    let req_get = crate::adapter::ApiRequest {
        method: "GET".into(),
        url: "/api/integrations/pg1/masked_columns".into(),
        body: None,
    };
    let resp4 = adapter
        .as_ref()
        .handle_api(&conn, req_get, "/masked_columns")
        .await
        .unwrap();
    let body4: Value = serde_json::from_slice(&resp4.body).unwrap();
    assert!(!body4["columns"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn connection_testing_opens_and_closes() {
    let (_dir, store) = temp_store();
    let conn = make_integration("pg1", "postgres", json!({"host":"localhost"}), None);
    let cancels = Arc::new(SqlCancelRegistry::default());
    let adapter = crate::sql::SqlAdapter::postgres(store, cancels);
    let res = adapter.as_ref().test_connection(&conn).await;
    if let Err(e) = &res
        && e.to_string().contains("connection failed")
    {
        eprintln!("skip connection_testing: no postgres reachable: {e}");
        return;
    }
    assert!(
        res.is_ok(),
        "test_connection should succeed: {:?}",
        res.err()
    );
}

/// A SQLite file with one table, so introspection has something real to read.
fn sqlite_fixture() -> (tempfile::TempDir, Integration) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("CREATE TABLE users (id INTEGER, email TEXT)", [])
        .unwrap();
    db.execute("INSERT INTO users VALUES (1, 'a@b.c')", [])
        .unwrap();
    // The opt-in introspection tools are off by default; this fixture turns
    // them on so the audit covers every one of them.
    let policy = r#"{"tools":{"list_schemas":{"enabled":true},"table_stats":{"enabled":true},"list_relationships":{"enabled":true}}}"#;
    let conn = make_integration(
        "sq1",
        "sqlite",
        json!({ "filename": path.to_str().unwrap() }),
        Some(policy),
    );
    (dir, conn)
}

fn log_rows(store: &Store) -> Vec<pluk_store::LogEntry> {
    store
        .read_log_page(&LogScope::Connection("sq1".into()), LogRange::All, None)
        .unwrap()
        .entries
}

#[tokio::test]
async fn every_call_that_reaches_the_database_leaves_a_log_row() {
    let (_dir, store) = temp_store();
    let (_db, conn) = sqlite_fixture();
    let host = capture_for(&conn, store.clone());

    let calls: Vec<(&str, Value)> = vec![
        ("list_tables", json!({})),
        ("sample_table", json!({ "table": "users" })),
        ("describe_table", json!({ "table": "users" })),
        ("search_schema", json!({ "term": "user" })),
        ("list_schemas", json!({})),
        ("table_stats", json!({ "table": "users" })),
        ("list_relationships", json!({})),
    ];
    for (name, args) in &calls {
        let handler = host.tools.get(*name).unwrap_or_else(|| panic!("{name}"));
        let res = handler(args.clone()).await;
        assert!(!res.is_error, "{name} failed: {}", res.text());
    }

    let schema = host.resource_handlers.get("schema://full").unwrap();
    assert!(schema().await.text.contains("users"));

    let sources: std::collections::HashSet<String> = log_rows(&store)
        .iter()
        .filter_map(|e| e.source.clone())
        .collect();
    for (name, _) in &calls {
        assert!(
            sources.contains(*name),
            "{name} left no log row: {sources:?}"
        );
    }
    assert!(
        sources.contains("schema"),
        "the schema resource left no log row: {sources:?}"
    );
    assert!(
        log_rows(&store).iter().all(|e| e.verdict == "allowed"),
        "every recorded call ran"
    );
}

#[tokio::test]
async fn a_sampled_table_records_the_rows_it_returned() {
    let (_dir, store) = temp_store();
    let (_db, conn) = sqlite_fixture();
    let host = capture_for(&conn, store.clone());

    let res = host.tools.get("sample_table").unwrap()(json!({ "table": "users" })).await;
    assert!(!res.is_error, "{}", res.text());

    let row = log_rows(&store)
        .into_iter()
        .find(|e| e.source.as_deref() == Some("sample_table"))
        .expect("sample_table row");
    assert_eq!(row.sql, "sample_table users");
    let snapshot: Value = serde_json::from_str(row.result_json.as_deref().unwrap()).unwrap();
    assert_eq!(snapshot["rows"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_failed_introspection_call_is_recorded_as_an_error() {
    let (_dir, store) = temp_store();
    let (_db, conn) = sqlite_fixture();
    let host = capture_for(&conn, store.clone());

    let res = host.tools.get("sample_table").unwrap()(json!({ "table": "nope" })).await;
    assert!(res.is_error);

    let row = log_rows(&store)
        .into_iter()
        .find(|e| e.source.as_deref() == Some("sample_table"))
        .expect("sample_table row");
    assert_eq!(row.verdict, "error");
}

#[tokio::test]
async fn mysql_gates_the_statement_it_will_run_not_the_placeholder_form() {
    let (_dir, store) = temp_store();
    // Read-only by default, so the rendered UPDATE is refused at the gate and
    // no connection is attempted.
    let conn = make_integration("my1", "mysql", json!({ "host": "127.0.0.1" }), None);
    let host = capture_for(&conn, store.clone());

    let res = host.tools.get("query").unwrap()(json!({
        "sql": "UPDATE users SET email = ? WHERE id = ?",
        "params": ["a'; DROP TABLE users; --", 7],
    }))
    .await;
    assert!(
        res.is_error,
        "read-only must refuse an UPDATE: {}",
        res.text()
    );

    let row = mysql_row(&store);
    assert_eq!(row.verdict, "blocked");
    // MySQL inlines its parameters, so the placeholder form is not what runs:
    // the row records the statement the gate actually judged.
    assert_eq!(
        row.sql,
        "UPDATE users SET email = 'a\\'; DROP TABLE users; --' WHERE id = 7"
    );
}

#[tokio::test]
async fn a_parameter_cannot_smuggle_a_statement_past_the_gate() {
    let (_dir, store) = temp_store();
    let conn = make_integration("my1", "mysql", json!({ "host": "127.0.0.1" }), None);
    let host = capture_for(&conn, store.clone());

    // The `?` sits inside a literal, so it is not a placeholder: substituting
    // there would close the quote and hand the rest back as SQL.
    let res = host.tools.get("query").unwrap()(json!({
        "sql": "UPDATE users SET email = '?'",
        "params": ["x'; DROP TABLE users; --"],
    }))
    .await;
    assert!(res.is_error);

    let row = mysql_row(&store);
    assert_eq!(row.verdict, "blocked");
    assert_eq!(row.sql, "UPDATE users SET email = '?'");
}

fn mysql_row(store: &Store) -> pluk_store::LogEntry {
    store
        .read_log_page(&LogScope::Connection("my1".into()), LogRange::All, None)
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.source.as_deref() == Some("query"))
        .expect("query row")
}
