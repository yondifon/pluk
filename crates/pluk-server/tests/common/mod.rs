//! Shared harness: a real loopback server over a throwaway database, driven
//! by an HTTP client like an agent or the frontend would.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use pluk_adapters::{
    Adapter, AdapterError, AdapterRegistry, ApiRequest, ApiResponse, CallTarget, ConfigField,
    FieldType, GateMeta, PolicyKind, PromptMessage, PromptResult, PromptRole, ResourceContents,
    ToolRegistration, ToolSpec, object_schema, ok, run_gated,
};
use pluk_server::{AppState, EventHub, HealthMap, OwnerPool, router};
use pluk_store::{Integration, Store};

/// An adapter whose whole surface exists to be observed by tests.
pub struct StubAdapter {
    pub store: Arc<Store>,
    /// What `test_connection` reports.
    pub healthy: AtomicBool,
}

impl StubAdapter {
    pub fn new(store: Arc<Store>) -> Arc<Self> {
        Arc::new(Self {
            store,
            healthy: AtomicBool::new(true),
        })
    }

    pub fn set_healthy(&self, healthy: bool) {
        self.healthy.store(healthy, Ordering::SeqCst);
    }
}

#[async_trait]
impl Adapter for StubAdapter {
    fn id(&self) -> &str {
        "stub"
    }

    fn label(&self) -> &str {
        "Stub Service"
    }

    fn category(&self) -> &str {
        "misc"
    }

    fn policy_kind(&self) -> PolicyKind {
        PolicyKind::None
    }

    fn agent_hint(&self) -> &str {
        "Use echo first."
    }

    fn tool_specs(&self) -> &[ToolSpec] {
        static SPECS: std::sync::OnceLock<Vec<ToolSpec>> = std::sync::OnceLock::new();
        SPECS.get_or_init(|| vec![ToolSpec::new("echo", "Echo a value back", "read")])
    }

    fn config_fields(&self) -> &[ConfigField] {
        static FIELDS: std::sync::OnceLock<Vec<ConfigField>> = std::sync::OnceLock::new();
        FIELDS.get_or_init(|| {
            vec![
                ConfigField::new("endpoint", "Endpoint", FieldType::Text).required(),
                ConfigField::new("retries", "Retries", FieldType::Number),
                ConfigField::new("token", "Token", FieldType::Password).secret(),
                ConfigField::new("verbose", "Verbose", FieldType::Toggle),
            ]
        })
    }

    async fn test_connection(&self, conn: &Integration) -> Result<(), AdapterError> {
        if self.healthy.load(Ordering::SeqCst) {
            Ok(())
        } else if conn.config.get("verbose").and_then(Value::as_bool) == Some(true) {
            Err(AdapterError::new("special"))
        } else {
            Err(AdapterError::new("stub refuses connections"))
        }
    }

    fn humanize_error(&self, error: &AdapterError) -> Option<String> {
        (error.message == "special").then(|| "translated failure".to_string())
    }

    async fn handle_api(
        &self,
        conn: &Integration,
        _request: ApiRequest,
        subpath: &str,
    ) -> Option<ApiResponse> {
        (subpath == "/ping")
            .then(|| ApiResponse::json(200, &json!({ "ok": true, "from": conn.id })))
    }

    async fn handle_global_api(&self, _request: ApiRequest, path: &str) -> Option<ApiResponse> {
        (path == "/api/stub-global").then(|| ApiResponse::text(200, "global"))
    }

    fn instructions(&self, conn: &Integration) -> String {
        format!(
            "Stub integration \"{}\".\nEcho things.\nEndpoint: {}",
            conn.name,
            conn.config
                .get("endpoint")
                .and_then(Value::as_str)
                .unwrap_or("(unset)")
        )
    }

    fn register(
        &self,
        host: &mut dyn pluk_adapters::ToolHost,
        conn: &Integration,
        _owner_id: &str,
    ) -> Result<(), AdapterError> {
        let mut properties = Map::new();
        properties.insert(
            "value".into(),
            json!({ "type": "string", "description": "What to echo" }),
        );

        let store = self.store.clone();
        let target = CallTarget {
            connection_id: conn.id.clone(),
            connection_name: conn.name.clone(),
            group: conn.via_group.clone(),
        };
        host.register_tool(
            ToolRegistration {
                name: "echo".into(),
                description: "Echo a value back".into(),
                input_schema: object_schema(properties, &["value"]),
                annotations: Map::new(),
            },
            Arc::new(move |args: Value| {
                let store = store.clone();
                let target = target.clone();
                Box::pin(async move {
                    let value = args
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    run_gated(
                        &store,
                        &target,
                        GateMeta::new("read", "echo", format!("echo {value}")),
                        move |_| async move { Ok(pluk_adapters::Outcome::ran(value)) },
                        Default::default(),
                    )
                    .await
                })
            }),
        );
        host.register_tool(
            ToolRegistration::no_args("ping", "Take no arguments").with_annotations(
                serde_json::from_value::<Map<String, Value>>(json!({ "readOnlyHint": true }))
                    .expect("annotations"),
            ),
            Arc::new(
                |_args: Value| -> pluk_adapters::BoxFuture<pluk_adapters::ToolResult> {
                    Box::pin(async move { ok("pong") })
                },
            ),
        );
        host.register_prompt(
            "greet",
            "Greet someone",
            None,
            Arc::new(
                |args: Map<String, Value>| -> pluk_adapters::BoxFuture<PromptResult> {
                    Box::pin(async move {
                        PromptResult {
                            messages: vec![PromptMessage {
                                role: PromptRole::User,
                                text: format!(
                                    "hello {}",
                                    args.get("who").and_then(Value::as_str).unwrap_or("world")
                                ),
                            }],
                        }
                    })
                },
            ),
        );
        host.register_resource(
            "schema",
            "schema://full",
            "text/plain",
            Some("The full schema"),
            Arc::new(|| -> pluk_adapters::BoxFuture<ResourceContents> {
                Box::pin(async move {
                    ResourceContents {
                        uri: "schema://full".into(),
                        mime_type: "text/plain".into(),
                        text: "everything".into(),
                    }
                })
            }),
        );
        Ok(())
    }
}

pub struct TestApp {
    pub base_url: String,
    pub store: Arc<Store>,
    pub adapter: Arc<StubAdapter>,
    pub health: Arc<HealthMap>,
    pub owners: Arc<OwnerPool>,
    /// The hub behind `/api/events` (for subscriber-count assertions).
    pub events: Arc<EventHub>,
    /// Owner ids seen by the close hook registered at startup.
    pub closed_owners: Arc<Mutex<Vec<String>>>,
    db_path: std::path::PathBuf,
    _dir: tempfile::TempDir,
    shutdown: CancellationToken,
}

impl Drop for TestApp {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// Spawn the real router on an OS-assigned loopback port.
pub async fn spawn_app() -> TestApp {
    // Short keepalive + small buffers so timing-sensitive behaviour is
    // observable quickly.
    spawn_app_with_events(Duration::from_millis(60), 64).await
}

pub async fn spawn_app_with_events(keepalive: Duration, capacity: usize) -> TestApp {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("pluk.db");
    let store = Arc::new(Store::open(&db_path).expect("open store"));
    let adapter = StubAdapter::new(store.clone());

    let mut registry = AdapterRegistry::new();
    registry.register(adapter.clone()).expect("register stub");

    let owners = Arc::new(OwnerPool::default());
    let health = Arc::new(HealthMap::default());
    let closed = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = closed.clone();
    owners.on_owner_close(Arc::new(move |owner| {
        sink.lock().unwrap().push(owner.to_string())
    }));

    let events = Arc::new(EventHub::with_options(store.clone(), keepalive, capacity));
    let state = AppState::with_event_hub(
        store.clone(),
        Arc::new(registry),
        owners.clone(),
        health.clone(),
        events.clone(),
    );
    finish_spawn(
        state, dir, db_path, store, adapter, owners, health, closed, events,
    )
    .await
}

/// Spawn with a caller-built state (full control over every shared handle).
pub async fn spawn_app_with(
    build: impl Fn(Arc<Store>, Arc<AdapterRegistry>, Arc<OwnerPool>, Arc<HealthMap>) -> AppState,
) -> TestApp {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("pluk.db");
    let store = Arc::new(Store::open(&db_path).expect("open store"));
    let adapter = StubAdapter::new(store.clone());

    let mut registry = AdapterRegistry::new();
    registry.register(adapter.clone()).expect("register stub");

    let owners = Arc::new(OwnerPool::default());
    let health = Arc::new(HealthMap::default());
    let closed = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = closed.clone();
    owners.on_owner_close(Arc::new(move |owner| {
        sink.lock().unwrap().push(owner.to_string())
    }));

    let events = Arc::new(EventHub::with_options(
        store.clone(),
        Duration::from_millis(60),
        64,
    ));
    let state = build(
        store.clone(),
        Arc::new(registry),
        owners.clone(),
        health.clone(),
    );
    finish_spawn(
        state, dir, db_path, store, adapter, owners, health, closed, events,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn finish_spawn(
    state: AppState,
    dir: tempfile::TempDir,
    db_path: std::path::PathBuf,
    store: Arc<Store>,
    adapter: Arc<StubAdapter>,
    owners: Arc<OwnerPool>,
    health: Arc<HealthMap>,
    closed_owners: Arc<Mutex<Vec<String>>>,
    events: Arc<EventHub>,
) -> TestApp {
    let app = router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().unwrap().port();
    let shutdown = tokio_util::sync::CancellationToken::new();
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .expect("serve");
        });
    }

    TestApp {
        base_url: format!("http://127.0.0.1:{port}"),
        store,
        adapter,
        health,
        owners,
        events,
        closed_owners,
        db_path,
        _dir: dir,
        shutdown,
    }
}

impl TestApp {
    /// Insert `count` log rows for a fake connection; returns their ids.
    pub fn insert_logs(&self, count: usize, connection_id: &str) -> Vec<i64> {
        (0..count)
            .map(|i| {
                self.store
                    .create_log_entry(pluk_store::LogDraft::new(
                        connection_id,
                        connection_id,
                        format!("select {i}"),
                    ))
                    .expect("insert")
            })
            .collect()
    }

    /// Overwrite a row's created_at, like the TypeScript tests did via SQL.
    pub fn set_created_at(&self, ids: &[i64], created_at: &str) {
        let connection = rusqlite::Connection::open(&self.db_path).expect("open db");
        for id in ids {
            connection
                .execute(
                    "UPDATE query_log SET created_at = ? WHERE id = ?",
                    rusqlite::params![created_at, id],
                )
                .expect("update");
        }
    }

    /// Raw MCP POST returning (status, content-type, parsed JSON body).
    pub async fn mcp_post(&self, token: &str, body: Value) -> (u16, String, Value) {
        let response = self.mcp_request(token, body.to_string()).await;
        let status = response.status().as_u16();
        let content_type = header(&response, "content-type");
        let payload = response.json::<Value>().await.unwrap_or(Value::Null);
        (status, content_type, payload)
    }

    /// POST a notification (no id ⇒ no response body).
    pub async fn mcp_notify(&self, token: &str, method: &str) -> u16 {
        let response = self
            .mcp_request(
                token,
                json!({ "jsonrpc": "2.0", "method": method }).to_string(),
            )
            .await;
        response.status().as_u16()
    }

    async fn mcp_request(&self, token: &str, body: String) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("{}/mcp/{token}", self.base_url))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(body)
            .send()
            .await
            .expect("mcp post")
    }

    pub async fn get_status(&self, path: &str) -> u16 {
        reqwest::get(format!("{}{path}", self.base_url))
            .await
            .expect("get")
            .status()
            .as_u16()
    }

    pub async fn get_json(&self, path: &str) -> (u16, Value) {
        let response = reqwest::get(format!("{}{path}", self.base_url))
            .await
            .expect("get");
        let status = response.status().as_u16();
        (
            status,
            response.json::<Value>().await.unwrap_or(Value::Null),
        )
    }

    /// Open an SSE stream and read frames until `ready` arrived plus any
    /// replayed frames before it.
    pub async fn sse_connect(&self, query: &str) -> SseReader {
        let response = reqwest::Client::new()
            .get(format!("{base}/api/events{query}", base = self.base_url))
            .send()
            .await
            .expect("sse connect");
        assert_eq!(response.status(), 200, "/api/events must open");
        SseReader {
            response,
            buffer: String::new(),
        }
    }
}

fn header(response: &reqwest::Response, name: &str) -> String {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// A frame-by-frame SSE reader over a live response.
pub struct SseReader {
    response: reqwest::Response,
    buffer: String,
}

pub struct Frame {
    pub event: String,
    pub data: Value,
}

impl SseReader {
    /// Wait for and parse the next complete frame.
    pub async fn next_frame(&mut self) -> Frame {
        loop {
            if let Some(end) = self.buffer.find("\n\n") {
                let raw = self.buffer.drain(..end + 2).collect::<String>();
                let mut event = String::new();
                let mut data = String::new();
                for line in raw.trim_end_matches('\n').split('\n') {
                    if let Some(name) = line.strip_prefix("event: ") {
                        event = name.trim().to_string();
                    } else if let Some(payload) = line.strip_prefix("data: ") {
                        data.push_str(payload.trim());
                    }
                }
                return Frame {
                    event,
                    data: serde_json::from_str(&data).unwrap_or(Value::Null),
                };
            }
            let chunk = self
                .response
                .chunk()
                .await
                .expect("sse chunk")
                .expect("stream closed before frame");
            self.buffer.push_str(&String::from_utf8_lossy(&chunk));
        }
    }

    /// End the stream server-side (client disconnects).
    pub async fn close(self) {
        drop(self.response)
    }
}
