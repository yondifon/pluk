//! Wande: browser control as an integration.
//!
//! The surface itself lives in [`pluk_browser`]: a Chrome extension signed in
//! as the user pairs to Pluk's `/wande` routes and drives the sites they are
//! signed in to. This adapter is the row behind it — what the user picks in
//! the add flow, what browser activity is logged against, and what the Test
//! button asks.
//!
//! One integration covers every platform. Which platforms and tools exist is
//! `pluk_browser`'s catalog to answer, so nothing here names a platform or an
//! action and adding one needs no edit in this file.
//!
//! The tools published at the MCP endpoint are the catalog's, one for one.
//! A call does not run the browser itself: it starts a job on `/wande` and
//! reads the job back, the same two requests an HTTP caller makes, so there
//! is one queue, one activity log, and one place a result is validated.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::time::{Instant, sleep};

use pluk_store::{Integration, Store};

use crate::adapter::{Adapter, PolicyKind};
use crate::config_field::ConfigField;
use crate::error::AdapterError;
use crate::gate::{ToolResult, err, ok};
use crate::http_client;
use crate::instructions::{InstructionParts, build_instructions};
use crate::tool_host::{BoxFuture, ToolHost, ToolRegistration, object_schema};
use crate::tool_spec::ToolSpec;

const LABEL: &str = "Wande";

/// Tools that are this adapter's own rather than the catalog's: the second
/// half of the two-step publish, and the way to collect a slow job.
const CONFIRM_DRAFT: &str = "confirm_draft";
const GET_JOB: &str = "get_job";

/// Tool classes, matching the ones the catalog tags its own actions with.
const READ: &str = "read";
const WRITE: &str = "write";

/// How long a tool call waits for its job before handing back the job id.
/// Well under the pipeline's own two-minute expiry, because MCP clients give
/// up long before that.
const MAX_WAIT: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(500);

const AGENT_HINT: &str = "Use this to read and post on the sites the user is signed in to, in their own Chrome window. Each tool drives one real page and hands back what it read. Publishing is always two steps: x_compose_post and x_prepare_reply only produce a draft and never publish, and confirm_draft with that draftId is what publishes it. A call waits up to 60 seconds; if the page is still working you get a jobId back, and get_job returns the result once it lands.";

const ACCESS: &str = "Reads and posts through a Chrome window the user is signed in to, one page at a time. Posting always takes two steps: a draft, then a confirmation.";

/// The one failure the user can do something about, and the marker
/// [`WandeAdapter::humanize_error`] recognises it by.
const NOT_PAIRED: &str = "Chrome is not connected.";

/// What to tell whoever hit that failure. Shared so a failed connection test
/// and a refused tool call say the same thing.
const NOT_PAIRED_HELP: &str =
    "Chrome isn’t connected. Paste this Pluk ID into Wande in Chrome, then try again.";

const SERVICE_DOWN: &str = "Pluk isn’t answering. Restart Pluk and try again.";
const NOT_STARTED: &str = "Pluk did not start this. Try again.";

/// Wande, as the user's integration.
pub struct WandeAdapter {
    store: Arc<Store>,
    tool_specs: Vec<ToolSpec>,
}

impl WandeAdapter {
    pub fn new(store: Arc<Store>) -> Arc<Self> {
        Arc::new(WandeAdapter {
            store,
            tool_specs: tool_specs(),
        })
    }
}

#[async_trait::async_trait]
impl Adapter for WandeAdapter {
    fn id(&self) -> &str {
        pluk_browser::INTEGRATION_TYPE
    }

    fn label(&self) -> &str {
        LABEL
    }

    fn category(&self) -> &str {
        "social"
    }

    fn policy_kind(&self) -> PolicyKind {
        PolicyKind::None
    }

    fn agent_hint(&self) -> &str {
        AGENT_HINT
    }

    fn tool_specs(&self) -> &[ToolSpec] {
        &self.tool_specs
    }

    fn config_fields(&self) -> &[ConfigField] {
        &[]
    }

    /// Ask the running surface whether Chrome is paired right now — the one
    /// thing standing between a saved integration and a working browser.
    ///
    /// Which tools that browser can then run is the catalog's answer, not
    /// this check's.
    async fn test_connection(&self, _conn: &Integration) -> Result<(), AdapterError> {
        match chrome_is_connected(&self.store).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(AdapterError::new(NOT_PAIRED)),
            Err(message) => Err(AdapterError::new(message)),
        }
    }

    fn humanize_error(&self, error: &AdapterError) -> Option<String> {
        (error.message == NOT_PAIRED).then(|| NOT_PAIRED_HELP.to_owned())
    }

    fn instructions(&self, conn: &Integration) -> String {
        build_instructions(
            &conn.name,
            conn.environment,
            InstructionParts {
                kind: LABEL.to_owned(),
                access: ACCESS.to_owned(),
                policy: None,
                hint: Some(AGENT_HINT.to_owned()),
                start: Some(
                    "Start with x_read_feed or x_read_profile to see what is there. Compose with x_compose_post, then publish it with confirm_draft."
                        .to_owned(),
                ),
            },
        )
    }

    fn register(
        &self,
        host: &mut dyn ToolHost,
        _conn: &Integration,
        _owner_id: &str,
    ) -> Result<(), AdapterError> {
        for tool in pluk_browser::catalog_tools() {
            let store = self.store.clone();
            let tool_id = tool.id.clone();
            host.register_tool(
                ToolRegistration {
                    name: mcp_name(&tool.id),
                    description: tool.summary.to_owned(),
                    input_schema: input_schema(&tool.args_schema),
                    annotations: annotations(tool.category),
                },
                Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                    let store = store.clone();
                    let tool_id = tool_id.clone();
                    Box::pin(async move { run_tool(&store, &tool_id, args).await })
                }),
            );
        }

        let store = self.store.clone();
        host.register_tool(
            ToolRegistration {
                name: CONFIRM_DRAFT.to_owned(),
                description: "Publish a draft that x_compose_post or x_prepare_reply prepared. This is what posts; nothing before it is public. Pass schedule to take the next queued slot instead of posting now.".to_owned(),
                input_schema: confirm_schema(),
                annotations: annotations(WRITE),
            },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let store = store.clone();
                Box::pin(async move { confirm_draft(&store, args).await })
            }),
        );

        let store = self.store.clone();
        host.register_tool(
            ToolRegistration {
                name: GET_JOB.to_owned(),
                description:
                    "Read a job by id, for when a call handed back a jobId instead of a result."
                        .to_owned(),
                input_schema: get_job_schema(),
                annotations: annotations(READ),
            },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let store = store.clone();
                Box::pin(async move { get_job(&store, args).await })
            }),
        );
        Ok(())
    }
}

/// The catalog's ids carry a dot (`x.read_feed`). MCP tool names are safest
/// as word characters, so the published name swaps it for an underscore; the
/// id itself still goes on the wire to `/wande`.
fn mcp_name(tool_id: &str) -> String {
    tool_id.replace('.', "_")
}

fn tool_specs() -> Vec<ToolSpec> {
    let mut specs: Vec<ToolSpec> = pluk_browser::catalog_tools()
        .iter()
        .map(|tool| ToolSpec::new(mcp_name(&tool.id), tool.summary, tool.category))
        .collect();
    specs.push(ToolSpec::new(
        CONFIRM_DRAFT,
        "Publish a prepared draft. This is the step that posts.",
        WRITE,
    ));
    specs.push(ToolSpec::new(
        GET_JOB,
        "Read the result of a job that was still running.",
        READ,
    ));
    specs
}

fn annotations(category: &str) -> Map<String, Value> {
    let read_only = category == READ;
    let mut map = Map::new();
    map.insert("readOnlyHint".into(), Value::Bool(read_only));
    map.insert("destructiveHint".into(), Value::Bool(!read_only));
    map.insert("openWorldHint".into(), Value::Bool(true));
    map
}

/// The catalog marks each property required inline (`"required": true`); MCP
/// wants one `required` array per object level, so lift them as we copy.
fn input_schema(args_schema: &Value) -> Map<String, Value> {
    let (properties, required) = lift_required(args_schema);
    object_schema(
        properties,
        &required.iter().map(String::as_str).collect::<Vec<_>>(),
    )
}

fn lift_required(properties: &Value) -> (Map<String, Value>, Vec<String>) {
    let mut lifted = Map::new();
    let mut required = Vec::new();
    let Some(object) = properties.as_object() else {
        return (lifted, required);
    };
    for (name, spec) in object {
        let Some(spec) = spec.as_object() else {
            lifted.insert(name.clone(), spec.clone());
            continue;
        };
        if spec.get("required").and_then(Value::as_bool) == Some(true) {
            required.push(name.clone());
        }
        let mut copy = Map::new();
        for (key, value) in spec {
            match key.as_str() {
                "required" => {}
                "properties" => {
                    let (nested, nested_required) = lift_required(value);
                    copy.insert("properties".into(), Value::Object(nested));
                    if !nested_required.is_empty() {
                        copy.insert("required".into(), json!(nested_required));
                    }
                }
                _ => {
                    copy.insert(key.clone(), value.clone());
                }
            }
        }
        lifted.insert(name.clone(), Value::Object(copy));
    }
    (lifted, required)
}

fn confirm_schema() -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert(
        "draftId".into(),
        json!({
            "type": "string",
            "description": "The draftId the compose or prepare-reply call returned.",
        }),
    );
    properties.insert(
        "schedule".into(),
        json!({
            "type": "boolean",
            "description": "Take the next queued slot instead of posting now. Defaults to false.",
        }),
    );
    object_schema(properties, &["draftId"])
}

fn get_job_schema() -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert(
        "jobId".into(),
        json!({
            "type": "string",
            "description": "The jobId a call handed back when it was still running.",
        }),
    );
    object_schema(properties, &["jobId"])
}

async fn run_tool(store: &Store, tool_id: &str, args: Value) -> ToolResult {
    if let Err(message) = require_chrome(store).await {
        return err(message);
    }
    let started = match send(
        store,
        reqwest::Method::POST,
        &format!("/tools/{tool_id}"),
        Some(args),
    )
    .await
    {
        Ok(value) => value,
        Err(message) => return err(message),
    };
    let Some(job_id) = started["job"]["id"].as_str() else {
        return err(NOT_STARTED);
    };
    await_job(store, job_id).await
}

async fn confirm_draft(store: &Store, args: Value) -> ToolResult {
    let Some(draft_id) = identifier(&args, "draftId") else {
        return err("Pass the draftId the compose or prepare-reply call returned.");
    };
    if let Err(message) = require_chrome(store).await {
        return err(message);
    }
    let schedule = args["schedule"].as_bool().unwrap_or(false);
    let body = if schedule {
        json!({ "schedule": true })
    } else {
        json!({})
    };
    let confirmed = match send(
        store,
        reqwest::Method::POST,
        &format!("/drafts/{draft_id}/confirm"),
        Some(body),
    )
    .await
    {
        Ok(value) => value,
        Err(message) => return err(message),
    };
    let Some(job_id) = confirmed["job"]["id"].as_str() else {
        return err(NOT_STARTED);
    };
    await_job(store, job_id).await
}

async fn get_job(store: &Store, args: Value) -> ToolResult {
    let Some(job_id) = identifier(&args, "jobId") else {
        return err("Pass the jobId a call handed back.");
    };
    match fetch_job(store, job_id).await {
        Ok(job) => job_result(&job),
        Err(message) => err(message),
    }
}

async fn fetch_job(store: &Store, job_id: &str) -> Result<Value, String> {
    let value = send(
        store,
        reqwest::Method::GET,
        &format!("/jobs/{job_id}"),
        None,
    )
    .await?;
    Ok(value["job"].clone())
}

/// Poll the job the same way an HTTP caller would, and stop well short of the
/// job's own expiry so the caller gets an id rather than a dropped connection.
async fn await_job(store: &Store, job_id: &str) -> ToolResult {
    let deadline = Instant::now() + MAX_WAIT;
    loop {
        let job = match fetch_job(store, job_id).await {
            Ok(job) => job,
            Err(message) => return err(message),
        };
        if !matches!(job["status"].as_str(), Some("queued" | "running")) {
            return job_result(&job);
        }
        if Instant::now() >= deadline {
            return ok(format!(
                "Still working on this page. Call {GET_JOB} with jobId {job_id} to pick up the result."
            ));
        }
        sleep(POLL_INTERVAL).await;
    }
}

fn job_result(job: &Value) -> ToolResult {
    let text = serde_json::to_string_pretty(job).unwrap_or_else(|_| job.to_string());
    if job["status"] == "succeeded" {
        return ok(text);
    }
    let reason = job["error"]["message"]
        .as_str()
        .unwrap_or("This did not finish.");
    err(format!("{reason}\n\n{text}"))
}

async fn require_chrome(store: &Store) -> Result<(), String> {
    match chrome_is_connected(store).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(NOT_PAIRED_HELP.to_owned()),
        Err(message) => Err(message),
    }
}

async fn chrome_is_connected(store: &Store) -> Result<bool, String> {
    let status = send(store, reqwest::Method::GET, "/status", None).await?;
    Ok(status["extension"]["connected"].as_bool().unwrap_or(false))
}

/// Ids reach the URL path, so anything that is not one is refused here rather
/// than steered into another route.
fn identifier<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    let value = args[key].as_str()?;
    let plain = !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    plain.then_some(value)
}

/// One request to the `/wande` routes on Pluk's own loopback server, carrying
/// the Pluk ID the extension pairs with.
async fn send(
    store: &Store,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let key = pluk_browser::pairing_key(store)?;
    let client = http_client::shared().map_err(|error| error.message)?;
    let mut request = client
        .request(
            method,
            format!(
                "http://127.0.0.1:{}/wande{path}",
                pluk_core::loopback::port()
            ),
        )
        .bearer_auth(key);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.map_err(|_| SERVICE_DOWN.to_owned())?;
    let status = response.status();
    let value: Value = response
        .json()
        .await
        .map_err(|_| "Pluk sent back something unreadable. Try again.".to_owned())?;
    if status.is_success() {
        return Ok(value);
    }
    Err(value["error"]["message"]
        .as_str()
        .unwrap_or("Pluk refused this.")
        .to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_host::{PromptHandler, ResourceHandler, ToolHandler, register_gated};

    #[derive(Default)]
    struct RecordingHost {
        tools: Vec<String>,
    }

    impl ToolHost for RecordingHost {
        fn register_tool(&mut self, registration: ToolRegistration, _handler: ToolHandler) {
            self.tools.push(registration.name);
        }
        fn register_prompt(
            &mut self,
            _name: &str,
            _description: &str,
            _args_schema: Option<Map<String, Value>>,
            _handler: PromptHandler,
        ) {
        }
        fn register_resource(
            &mut self,
            _name: &str,
            _uri: &str,
            _mime_type: &str,
            _description: Option<&str>,
            _handler: ResourceHandler,
        ) {
        }
    }

    fn registered(query_policy: Option<&str>) -> Vec<String> {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(Store::open(&dir.path().join("pluk.db")).expect("open"));
        let integration = Integration {
            id: "i1".into(),
            name: "Wande".into(),
            r#type: pluk_browser::INTEGRATION_TYPE.into(),
            config: Map::new(),
            environment: None,
            read_only: 0,
            query_policy: query_policy.map(Into::into),
            token: "t".into(),
            created_at: String::new(),
            via_group: None,
        };
        let mut host = RecordingHost::default();
        register_gated(&*WandeAdapter::new(store), &mut host, &integration, "").expect("register");
        host.tools
    }

    #[test]
    fn every_catalog_tool_is_published_plus_the_two_this_adapter_owns() {
        let names: Vec<String> = tool_specs().into_iter().map(|spec| spec.name).collect();
        assert_eq!(
            names,
            vec![
                "x_inspect",
                "x_read_profile",
                "x_read_post",
                "x_read_feed",
                "x_read_trends",
                "x_refresh",
                "x_capture",
                "x_prepare_reply",
                "x_compose_post",
                CONFIRM_DRAFT,
                GET_JOB,
            ]
        );
    }

    #[test]
    fn reading_ships_on_and_anything_that_writes_ships_off() {
        for spec in tool_specs() {
            assert_eq!(
                spec.default_enabled,
                spec.category == READ,
                "{} defaults wrong",
                spec.name
            );
        }
    }

    /// Nothing may publish in one step: the only tool that posts is the
    /// confirmation, and it takes a draft someone else prepared.
    #[test]
    fn no_tool_submits_a_post_directly() {
        let names: Vec<String> = tool_specs().into_iter().map(|spec| spec.name).collect();
        assert!(!names.iter().any(|name| name.contains("submit")));
    }

    #[test]
    fn required_properties_move_into_the_schema_array() {
        let compose = pluk_browser::catalog_tools()
            .into_iter()
            .find(|tool| tool.id == "x.compose_post")
            .expect("compose_post is in the catalog");
        let schema = input_schema(&compose.args_schema);
        assert_eq!(schema["required"], json!(["payload"]));
        let payload = &schema["properties"]["payload"];
        assert_eq!(payload["required"], json!(["text"]));
        assert!(payload["properties"]["text"].get("required").is_none());
        assert!(schema["properties"]["ttlMs"].get("required").is_none());
    }

    /// Registration is the enable switch, so a fresh integration must not
    /// expose anything that writes, and turning one off must unregister it.
    #[test]
    fn the_toggle_decides_what_reaches_the_agent() {
        let fresh = registered(None);
        assert!(fresh.contains(&"x_read_feed".to_string()));
        assert!(!fresh.contains(&"x_compose_post".to_string()));
        assert!(!fresh.contains(&CONFIRM_DRAFT.to_string()));

        let composing_on = r#"{"tools":{"x_compose_post":{"enabled":true}}}"#;
        assert!(registered(Some(composing_on)).contains(&"x_compose_post".to_string()));

        let feed_off = r#"{"tools":{"x_read_feed":{"enabled":false}}}"#;
        assert!(!registered(Some(feed_off)).contains(&"x_read_feed".to_string()));
    }

    #[test]
    fn an_id_that_could_leave_its_route_is_refused() {
        assert_eq!(
            identifier(&json!({ "jobId": "7f3a-41" }), "jobId"),
            Some("7f3a-41")
        );
        for bad in ["../status", "a/b", "", "a b", "x".repeat(65).as_str()] {
            assert!(
                identifier(&json!({ "jobId": bad }), "jobId").is_none(),
                "{bad} should be refused"
            );
        }
    }
}
