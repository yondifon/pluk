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
//! is one queue, one activity log, and one place a result is validated. A
//! post or reply starts no job at all: it writes a draft that waits in Pluk,
//! and the call reads back what the user decided about it.

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

/// The one tool that is this adapter's own rather than the catalog's: how to
/// collect a call that outlived its wait. It takes the id the call handed
/// back, whether that was a browser job or a post waiting on the user.
const GET_JOB: &str = "get_job";

/// The tool class the catalog tags a read-only action with; anything else
/// drives the page and is annotated as such.
const READ: &str = "read";

/// How long a tool call waits for the page, or for the person answering
/// about a post, before handing back the job id. A post stays answerable
/// far longer than this; the cap sits under the minute MCP clients give a
/// call, so the caller gets a "still going" instead of a dead socket.
const MAX_WAIT: Duration = Duration::from_secs(45);
const POLL_INTERVAL: Duration = Duration::from_millis(500);

const AGENT_HINT: &str = "Use this to read and post on the sites the user is signed in to, in their own Chrome window. Each read tool drives one real page and hands back what it read. x_post and x_reply touch no page: the exact text is handed to the user in Pluk, who sends it now, queues it for later, or discards it. One call covers writing and asking, and you get back what the user decided. Only their decision fills the composer and submits. X allows 280 weighted characters per post and a link counts 23; longer text is cut into a thread at sentence ends, or pass thread for exact parts. You cannot publish anything yourself and there is no tool that does; if the user says no, that is the answer. A call waits up to 60 seconds; past that you get a jobId, and get_job returns the outcome once it lands.";

const ACCESS: &str = "Reads and posts through a Chrome window the user is signed in to, one page at a time. Every post is shown to the user in full inside Pluk and goes out only if they say so.";

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
                    "Start with x_read_feed or x_read_profile to see what is there. x_post hands a post to the user in Pluk, who decides whether it goes out."
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
                name: GET_JOB.to_owned(),
                description: "Read a call's outcome by the jobId it handed back, including what the user decided about a post it wrote.".to_owned(),
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
        GET_JOB,
        "Read the outcome of a call that was still going.",
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

fn get_job_schema() -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert(
        "jobId".into(),
        json!({
            "type": "string",
            "description": "The id a call handed back when it was still going.",
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
    let deadline = Instant::now() + MAX_WAIT;
    // A post is not browser work yet: the user still has to say whether it
    // goes out, and that answer is this call's real result.
    if let Some(draft_id) = started["draft"]["id"].as_str() {
        return match settle_post(store, draft_id, deadline).await {
            Ok(Some(draft)) => report_post(&draft),
            Ok(None) => handed_off(draft_id),
            Err(message) => err(message),
        };
    }
    let Some(job_id) = started["job"]["id"].as_str() else {
        return err(NOT_STARTED);
    };
    match settle_job(store, job_id, deadline).await {
        Ok(Some(job)) => report_job(&job),
        Ok(None) => handed_off(job_id),
        Err(message) => err(message),
    }
}

/// The id a call handed back names a job or a post waiting on the user; a
/// finished submission job is reported by the post it sent.
async fn get_job(store: &Store, args: Value) -> ToolResult {
    let Some(id) = identifier(&args, "jobId") else {
        return err("Pass the jobId a call handed back.");
    };
    let job = match fetch_job(store, id).await {
        Ok(job) => job,
        Err(job_missing) => {
            return match fetch_draft(store, id).await {
                Ok(draft) => report_post(&draft),
                Err(_) => err(job_missing),
            };
        }
    };
    let Some(draft_id) = job["draftId"].as_str() else {
        return report_job(&job);
    };
    match fetch_draft(store, draft_id).await {
        Ok(draft) => report_post(&draft),
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

async fn fetch_draft(store: &Store, draft_id: &str) -> Result<Value, String> {
    let value = send(
        store,
        reqwest::Method::GET,
        &format!("/drafts/{draft_id}"),
        None,
    )
    .await?;
    Ok(value["draft"].clone())
}

/// Poll the job the same way an HTTP caller would. `None` when the wait ran
/// out first — the job itself keeps going.
async fn settle_job(
    store: &Store,
    job_id: &str,
    deadline: Instant,
) -> Result<Option<Value>, String> {
    loop {
        let job = fetch_job(store, job_id).await?;
        if !matches!(job["status"].as_str(), Some("queued" | "running")) {
            return Ok(Some(job));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        sleep(POLL_INTERVAL).await;
    }
}

/// Wait for the user's answer about a requested post, and for the post to go
/// out once they have given it. `None` when the wait ran out first; the post
/// is still theirs to send from Pluk.
async fn settle_post(
    store: &Store,
    draft_id: &str,
    deadline: Instant,
) -> Result<Option<Value>, String> {
    loop {
        let draft = fetch_draft(store, draft_id).await?;
        if post_outcome(&draft).is_some() {
            return Ok(Some(draft));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        sleep(POLL_INTERVAL).await;
    }
}

/// What became of a requested post, or `None` while it is still in motion:
/// unanswered, or answered and on its way into the page.
fn post_outcome(draft: &Value) -> Option<&'static str> {
    match draft["status"].as_str()? {
        "pending" => None,
        "confirmed" if draft["scheduledAt"].is_null() => None,
        "confirmed" => Some("queued"),
        "submitted" => Some("posted"),
        "cancelled" => Some("discarded"),
        "expired" => Some("expired"),
        _ => Some("failed"),
    }
}

fn handed_off(id: &str) -> ToolResult {
    ok(format!(
        "Still going. Call {GET_JOB} with jobId {id} to pick up what happened."
    ))
}

fn report_job(job: &Value) -> ToolResult {
    let text = pretty(job);
    if job["status"] == "succeeded" {
        return ok(text);
    }
    let reason = job["error"]["message"]
        .as_str()
        .unwrap_or("This did not finish.");
    err(format!("{reason}\n\n{text}"))
}

/// A post is reported by what the user decided about it, never by the
/// request having been taken.
fn report_post(draft: &Value) -> ToolResult {
    let detail = pretty(draft);
    match post_outcome(draft) {
        Some("posted") => ok(format!("The user sent this. It is posted.\n\n{detail}")),
        Some("queued") => ok(format!(
            "The user chose to send this later. It goes out on its own {}.\n\n{detail}",
            slot_label(draft)
        )),
        Some("discarded") => ok(format!(
            "The user chose not to send this. Nothing was posted. Do not write it again unless they ask.\n\n{detail}"
        )),
        Some("expired") => ok(format!(
            "Nobody answered, so this was not posted. It is still in Pluk for the user to send.\n\n{detail}"
        )),
        Some(_) => err(format!(
            "The user sent this, but the page did not take it. Nothing was posted.\n\n{detail}"
        )),
        None => ok(format!(
            "Waiting on the user in Pluk to say whether it goes out. Nothing is posted yet.\n\n{detail}"
        )),
    }
}

/// When a queued post goes out, in the user's clock.
fn slot_label(draft: &Value) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or(0);
    draft["scheduledAt"]
        .as_i64()
        .map(|at| pluk_store::browser::schedule::describe_slot(at, now))
        .unwrap_or_else(|| "at the next free time".to_owned())
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
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

    /// Every tool on, which is the worst case for the question that matters:
    /// what can an agent reach when nothing is holding it back?
    fn everything_on() -> String {
        let toggles: Vec<String> = tool_specs()
            .into_iter()
            .map(|spec| format!("\"{}\":{{\"enabled\":true}}", spec.name))
            .collect();
        format!("{{\"tools\":{{{}}}}}", toggles.join(","))
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
    fn every_catalog_tool_is_published_plus_the_one_this_adapter_owns() {
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
                "x_reply",
                "x_post",
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

    /// The line the whole design rests on: with every switch turned on, an
    /// agent still has no tool that sends a post. Asking for one hands it to
    /// the user in Pluk, and only their answer sends it.
    #[test]
    fn nothing_an_agent_can_reach_publishes_on_its_own() {
        let names = registered(Some(&everything_on()));
        assert!(names.contains(&"x_post".to_string()));
        for name in &names {
            assert!(
                !["confirm", "publish", "submit", "send", "approve"]
                    .iter()
                    .any(|word| name.contains(word)),
                "{name} looks like a way to publish without the user"
            );
        }
    }

    /// The same question asked of the loopback routes: the Pluk ID an agent
    /// carries must not open a door the tools do not.
    #[test]
    fn no_loopback_route_publishes_either() {
        let routes = include_str!("../../../pluk-browser/src/service.rs");
        assert!(
            !routes.contains("/drafts/{draft_id}/confirm"),
            "the confirm route is reachable with the Pluk ID again"
        );
        assert!(
            routes.contains("/drafts/{draft_id}/cancel"),
            "discarding a post must stay reachable"
        );
    }

    #[test]
    fn required_properties_move_into_the_schema_array() {
        let compose = pluk_browser::catalog_tools()
            .into_iter()
            .find(|tool| tool.id == "x.post")
            .expect("post is in the catalog");
        let schema = input_schema(&compose.args_schema);
        assert_eq!(schema["required"], json!(["payload"]));
        let payload = &schema["properties"]["payload"];
        // text and thread are alternatives, so neither is required on its own.
        assert!(payload.get("required").is_none());
        assert!(payload["properties"]["text"].get("required").is_none());
        assert!(payload["properties"]["thread"].get("required").is_none());
        assert!(schema["properties"]["ttlMs"].get("required").is_none());
    }

    /// Registration is the enable switch, so a fresh integration must not
    /// expose anything that writes, and turning one off must unregister it.
    #[test]
    fn the_toggle_decides_what_reaches_the_agent() {
        let fresh = registered(None);
        assert!(fresh.contains(&"x_read_feed".to_string()));
        assert!(!fresh.contains(&"x_post".to_string()));

        let composing_on = r#"{"tools":{"x_post":{"enabled":true}}}"#;
        assert!(registered(Some(composing_on)).contains(&"x_post".to_string()));

        let feed_off = r#"{"tools":{"x_read_feed":{"enabled":false}}}"#;
        assert!(!registered(Some(feed_off)).contains(&"x_read_feed".to_string()));
    }

    /// A post nobody has answered yet is not a result. The call says so, and
    /// keeps saying so rather than reporting the write as a success.
    #[test]
    fn an_unanswered_post_is_never_reported_as_posted() {
        let waiting = json!({ "status": "pending", "text": "hello", "scheduledAt": null });
        assert_eq!(post_outcome(&waiting), None);
        let result = report_post(&waiting);
        assert!(!result.is_error);
        assert!(
            result.content[0]
                .text
                .starts_with("Waiting on the user in Pluk")
        );
        assert!(!result.content[0].text.contains("posted."));

        // Confirmed but still on its way into the page is not an answer yet.
        let sending = json!({ "status": "confirmed", "scheduledAt": null });
        assert_eq!(post_outcome(&sending), None);
    }

    #[test]
    fn each_answer_reads_as_what_the_user_chose() {
        for (status, scheduled, outcome, opening) in [
            ("submitted", Value::Null, "posted", "The user sent this."),
            (
                "confirmed",
                json!(1_000),
                "queued",
                "The user chose to send this later",
            ),
            (
                "cancelled",
                Value::Null,
                "discarded",
                "The user chose not to send this.",
            ),
            (
                "expired",
                Value::Null,
                "expired",
                "Nobody answered, so this was not posted.",
            ),
        ] {
            let draft = json!({ "status": status, "scheduledAt": scheduled });
            assert_eq!(post_outcome(&draft), Some(outcome));
            let result = report_post(&draft);
            assert!(!result.is_error, "{status} should not read as a failure");
            assert!(
                result.content[0].text.starts_with(opening),
                "{status} reads as: {}",
                result.content[0].text
            );
        }
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
