//! The action-adapter factory: declarative REST/CLI integrations.
//!
//! Ported from `pluk/src/adapters/kit.ts`. Linear, Sentry, GitHub CLI, Redis,
//! Slack and friends declare their tools and client in an
//! [`ActionAdapterSpec`]; gating against the integration's per-tool config,
//! logging through [`run_gated`](crate::gate::run_gated), instructions, and
//! server construction are all supplied here.
//!
//! Catalog safety: tool definitions never depend on a live connection, so the
//! spec set is enumerated once at startup against a throwaway config. A
//! client or tool builder that panics there degrades to an empty catalog —
//! it can never take the process down.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use serde_json::{Map, Value};

use pluk_policy::{default_enabled_for_category, tool_gate, ActionCategory};
use pluk_store::{Integration, QueryResult, Store};

use crate::adapter::{Adapter, PolicyKind};
use crate::config_field::ConfigField;
use crate::error::AdapterError;
use crate::gate::{run_gated, CallTarget, GateMeta, GateOpts, Outcome, RunOutcome};
use crate::instructions::{build_instructions, InstructionParts};
use crate::tool_host::{object_schema, BoxFuture, ToolHandler, ToolHost, ToolRegistration};
use crate::tool_spec::ToolSpec;

// ── Tool outputs ─────────────────────────────────────────────────────────────

/// What a tool's run function produced.
///
/// A plain string passes through to the agent verbatim — this is how
/// CLI-wrapping adapters work; anything else is rendered as pretty JSON.
/// [`ActionOutput::WithCommand`] additionally carries the exact shell line
/// that was executed, so the log can show it.
#[derive(Debug, Clone)]
pub enum ActionOutput {
    Value(Value),
    WithCommand { value: Value, command: String },
}

impl ActionOutput {
    /// A structured payload.
    pub fn json(value: Value) -> Self {
        ActionOutput::Value(value)
    }

    /// CLI text, passed through verbatim.
    pub fn text(text: impl Into<String>) -> Self {
        ActionOutput::Value(Value::String(text.into()))
    }

    /// Structured/text output plus the equivalent shell command.
    pub fn with_command(value: Value, command: impl Into<String>) -> Self {
        ActionOutput::WithCommand { value, command: command.into() }
    }

    fn into_parts(self) -> (Value, Option<String>) {
        match self {
            ActionOutput::Value(value) => (value, None),
            ActionOutput::WithCommand { value, command } => (value, Some(command)),
        }
    }
}

/// Shape a run output into what the gated runner records and returns.
/// A string value passes through verbatim; anything else renders as pretty
/// JSON. Both land in the log: as the response text and as a row snapshot.
fn shape_output(output: ActionOutput) -> RunOutcome {
    let (data, command) = output.into_parts();
    let rows = match &data {
        Value::Array(items) => items.clone(),
        other => vec![other.clone()],
    };
    let text = match &data {
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| "{}".to_string()),
    };
    RunOutcome {
        text: text.clone(),
        is_error: false,
        reason: None,
        result: Some(QueryResult { fields: Vec::new(), rows }),
        response_text: Some(text),
        command,
    }
}

// ── One tool ─────────────────────────────────────────────────────────────────

type DetailFn = Arc<dyn Fn(&Value) -> String + Send + Sync>;
type CommandFn = Arc<dyn Fn(&Value, &Map<String, Value>) -> String + Send + Sync>;
type RunFn = Arc<dyn Fn(Value, Map<String, Value>) -> BoxFuture<Result<ActionOutput, AdapterError>> + Send + Sync>;

/// One tool on a REST/action service: its data fetch plus coarse category;
/// the platform handles enable-gating, logging, and response shaping.
#[derive(Clone)]
pub struct ActionTool {
    pub name: String,
    pub description: String,
    /// JSON-schema property definitions for the tool's arguments
    /// (the zod-shape equivalent). Absent → the tool takes no arguments.
    pub schema: Option<Map<String, Value>>,
    /// Drives the default-on state and the log category.
    pub category: ActionCategory,
    /// Override the derived default-on state. `None` derives from the
    /// category (read/inspect on, write/delete/admin off). Set `false` on a
    /// niche/heavy read so it ships opt-in; never `true` on a state-changing
    /// tool.
    pub default_enabled: Option<bool>,
    /// This tool's own settings, resolved from the integration's per-tool
    /// config and passed to `run` at call time.
    pub settings: Vec<ConfigField>,
    /// Log line for this call. Defaults to the tool name.
    pub detail: Option<DetailFn>,
    /// Equivalent shell command, recorded in the log.
    pub command: Option<CommandFn>,
    pub run: RunFn,
}

impl ActionTool {
    pub fn new(name: impl Into<String>, description: impl Into<String>, category: ActionCategory) -> Self {
        ActionTool {
            name: name.into(),
            description: description.into(),
            schema: None,
            category,
            default_enabled: None,
            settings: Vec::new(),
            detail: None,
            command: None,
            run: Arc::new(|_, _| Box::pin(async { Err(AdapterError::new("not implemented")) })),
        }
    }

    pub fn default_enabled(mut self, enabled: bool) -> Self {
        self.default_enabled = Some(enabled);
        self
    }

    pub fn schema(mut self, properties: Map<String, Value>) -> Self {
        self.schema = Some(properties);
        self
    }

    pub fn settings(mut self, settings: Vec<ConfigField>) -> Self {
        self.settings = settings;
        self
    }

    /// Log line for a call, computed from the call's arguments.
    pub fn detail_fn(mut self, f: impl Fn(&Value) -> String + Send + Sync + 'static) -> Self {
        self.detail = Some(Arc::new(f));
        self
    }

    /// Equivalent shell command, computed from arguments + resolved settings.
    pub fn command_fn(
        mut self,
        f: impl Fn(&Value, &Map<String, Value>) -> String + Send + Sync + 'static,
    ) -> Self {
        self.command = Some(Arc::new(f));
        self
    }

    /// The tool's data fetch: arguments + resolved settings in, output out.
    pub fn run<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(Value, Map<String, Value>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<ActionOutput, AdapterError>> + Send + 'static,
    {
        self.run = Arc::new(move |args, settings| Box::pin(f(args, settings)));
        self
    }

    /// The static catalog descriptor for this tool.
    fn to_spec(&self) -> ToolSpec {
        let mut spec = ToolSpec::new(&self.name, &self.description, self.category.as_str())
            .with_settings_if_any(&self.settings);
        if let Some(default_enabled) = self.default_enabled {
            spec = spec.with_default_enabled(default_enabled);
        }
        spec
    }
}

impl ToolSpec {
    fn with_settings_if_any(self, settings: &[ConfigField]) -> Self {
        if settings.is_empty() {
            self
        } else {
            self.with_settings(settings.to_vec())
        }
    }
}

// ── The spec ─────────────────────────────────────────────────────────────────

pub type ClientFn<C> = Box<dyn Fn(&Integration, &str) -> Result<C, AdapterError> + Send + Sync>;
pub type TestConnectionFn = Arc<dyn Fn(&Integration) -> BoxFuture<Result<(), AdapterError>> + Send + Sync>;
pub type ToolsFn<C> = Box<dyn Fn(&Integration, &C) -> Vec<ActionTool> + Send + Sync>;
pub type HumanizeFn = Arc<dyn Fn(&AdapterError) -> String + Send + Sync>;
/// Receives the failing tool's name and the error; installed once per spec so
/// every tool of an adapter reports failures the same way.
pub type ToolErrorHook = Arc<dyn Fn(&str, &AdapterError) + Send + Sync>;

/// Everything an action integration declares; [`action_adapter`] supplies the rest.
pub struct ActionAdapterSpec<C> {
    pub id: String,
    pub label: String,
    pub category: String,
    pub agent_hint: String,
    /// One line on the access / safety model, shown to connecting agents.
    pub access: String,
    /// Optional discovery hint: which tools to reach for first.
    pub start: Option<String>,
    pub config_fields: Vec<ConfigField>,
    /// Build the per-connection client/config once per endpoint, reused
    /// across tools. Receives the `owner_id` (the integration or group the
    /// endpoint fronts) so a client can scope long-lived resources to it.
    pub client: ClientFn<C>,
    pub test_connection: TestConnectionFn,
    pub tools: ToolsFn<C>,
    /// Turn a raw failure into something the user can act on (shown by the UI
    /// after a failed connection test).
    pub humanize_error: Option<HumanizeFn>,
    /// Failure reporter for tool calls (the TypeScript factory wired
    /// `logError` here); the audit row itself is always written either way.
    pub on_tool_error: Option<ToolErrorHook>,
}

impl<C> ActionAdapterSpec<C> {
    pub fn new(id: impl Into<String>, label: impl Into<String>, category: impl Into<String>) -> Self {
        ActionAdapterSpec {
            id: id.into(),
            label: label.into(),
            category: category.into(),
            agent_hint: String::new(),
            access: String::new(),
            start: None,
            config_fields: Vec::new(),
            client: Box::new(|_, _| Err(AdapterError::new("no client configured"))),
            test_connection: Arc::new(|_| Box::pin(async { Err(AdapterError::new("no test configured")) })),
            tools: Box::new(|_, _| Vec::new()),
            humanize_error: None,
            on_tool_error: None,
        }
    }

    pub fn agent_hint(mut self, hint: impl Into<String>) -> Self {
        self.agent_hint = hint.into();
        self
    }

    pub fn access(mut self, access: impl Into<String>) -> Self {
        self.access = access.into();
        self
    }

    pub fn start(mut self, start: impl Into<String>) -> Self {
        self.start = Some(start.into());
        self
    }

    pub fn config_fields(mut self, fields: Vec<ConfigField>) -> Self {
        self.config_fields = fields;
        self
    }

    pub fn client<F>(mut self, f: F) -> Self
    where
        F: Fn(&Integration, &str) -> Result<C, AdapterError> + Send + Sync + 'static,
    {
        self.client = Box::new(f);
        self
    }

    pub fn test_connection<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(&Integration) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), AdapterError>> + Send + 'static,
    {
        self.test_connection = Arc::new(move |conn| Box::pin(f(conn)));
        self
    }

    pub fn tools<F>(mut self, f: F) -> Self
    where
        F: Fn(&Integration, &C) -> Vec<ActionTool> + Send + Sync + 'static,
    {
        self.tools = Box::new(f);
        self
    }

    pub fn humanize_error(mut self, f: impl Fn(&AdapterError) -> String + Send + Sync + 'static) -> Self {
        self.humanize_error = Some(Arc::new(f));
        self
    }

    pub fn on_tool_error(mut self, f: impl Fn(&str, &AdapterError) + Send + Sync + 'static) -> Self {
        self.on_tool_error = Some(Arc::new(f));
        self
    }
}

// ── The built adapter ────────────────────────────────────────────────────────

/// A complete action adapter: gating, logging, instructions and registration
/// supplied by the factory; only the spec's declarations vary per service.
pub struct ActionAdapter<C> {
    spec: ActionAdapterSpec<C>,
    store: Arc<Store>,
    tool_specs: Vec<ToolSpec>,
    default_enabled_by_name: HashMap<String, bool>,
}

/// Build a complete action `Adapter` from a declarative spec.
pub fn action_adapter<C>(spec: ActionAdapterSpec<C>, store: Arc<Store>) -> ActionAdapter<C> {
    let tool_specs = enumerate_specs(&spec);
    let default_enabled_by_name =
        tool_specs.iter().map(|t| (t.name.clone(), t.default_enabled)).collect();
    ActionAdapter { spec, store, tool_specs, default_enabled_by_name }
}

fn dummy_integration(adapter_id: &str) -> Integration {
    Integration {
        id: String::new(),
        name: String::new(),
        r#type: adapter_id.to_string(),
        config: Map::new(),
        environment: None,
        read_only: 0,
        query_policy: None,
        token: String::new(),
        created_at: String::new(),
         via_group: None,
    }
}

/// Enumerate the static catalog defensively: some client builders read config
/// and may throw on blanks, some tool builders read the client — any panic
/// falls back to an empty list so metadata can never take the process down.
fn enumerate_specs<C>(spec: &ActionAdapterSpec<C>) -> Vec<ToolSpec> {
    let dummy_conn = dummy_integration(&spec.id);
    let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let client = (spec.client)(&dummy_conn, "").ok()?;
        let tools = (spec.tools)(&dummy_conn, &client);
        Some(tools.iter().map(ActionTool::to_spec).collect::<Vec<_>>())
    }));
    attempt.ok().flatten().unwrap_or_default()
}

#[async_trait::async_trait]
impl<C> Adapter for ActionAdapter<C> {
    fn id(&self) -> &str {
        &self.spec.id
    }

    fn label(&self) -> &str {
        &self.spec.label
    }

    fn category(&self) -> &str {
        &self.spec.category
    }

    fn policy_kind(&self) -> PolicyKind {
        PolicyKind::Action
    }

    fn agent_hint(&self) -> &str {
        &self.spec.agent_hint
    }

    fn tool_specs(&self) -> &[ToolSpec] {
        &self.tool_specs
    }

    fn config_fields(&self) -> &[ConfigField] {
        &self.spec.config_fields
    }

    async fn test_connection(&self, conn: &Integration) -> Result<(), AdapterError> {
        (self.spec.test_connection)(conn).await
    }

    fn humanize_error(&self, error: &AdapterError) -> Option<String> {
        self.spec.humanize_error.as_ref().map(|humanize| humanize(error))
    }

    fn instructions(&self, conn: &Integration) -> String {
        let gate = tool_gate(conn.query_policy.as_deref());
        let enabled: Vec<&str> = self
            .tool_specs
            .iter()
            .filter(|t| gate.enabled(&t.name, t.default_enabled))
            .map(|t| t.name.as_str())
            .collect();
        let policy = if enabled.is_empty() {
            "No tools are enabled on this integration.".to_string()
        } else {
            format!("Enabled tools: {}.", enabled.join(", "))
        };
        build_instructions(
            &conn.name,
            conn.environment,
            InstructionParts {
                kind: self.spec.label.clone(),
                access: self.spec.access.clone(),
                policy: Some(policy),
                start: self.spec.start.clone(),
                hint: Some(self.spec.agent_hint.clone()),
            },
        )
    }

    fn register(&self, host: &mut dyn ToolHost, conn: &Integration, owner_id: &str) -> Result<(), AdapterError> {
        let client = (self.spec.client)(conn, owner_id)?;
        let gate = tool_gate(conn.query_policy.as_deref());

        for tool in (self.spec.tools)(conn, &client) {
            // A disabled tool is not registered at all — the agent never sees
            // it. This is how an integration shrinks its surface (and locks
            // out write/delete).
            let fallback = self
                .default_enabled_by_name
                .get(&tool.name)
                .copied()
                .unwrap_or_else(|| default_enabled_for_category(tool.category.as_str()));
            if !gate.enabled(&tool.name, fallback) {
                continue;
            }

            let settings = gate.settings(&tool.name);
            let registration = ToolRegistration {
                name: tool.name.clone(),
                description: tool.description.clone(),
                input_schema: tool
                    .schema
                    .clone()
                    .map(|properties| object_schema(properties, &[]))
                    .unwrap_or_default(),
                annotations: Map::new(),
            };
            let handler = make_handler(
                self.store.clone(),
                conn,
                tool,
                settings,
                self.spec.on_tool_error.clone(),
            );
            host.register_tool(registration, handler);
        }
        Ok(())
    }
}

/// Bind one tool into a callable MCP handler carrying the audited lifecycle.
fn make_handler(
    store: Arc<Store>,
    conn: &Integration,
    tool: ActionTool,
    settings: Map<String, Value>,
    on_tool_error: Option<ToolErrorHook>,
) -> ToolHandler {
    let category = tool.category.as_str().to_string();
    let name = tool.name.clone();
    let fallback_detail = tool.name.clone();
    let detail_fn = tool.detail.clone();
    let command_fn = tool.command.clone();
    let run_fn = tool.run.clone();
    let conn = conn.clone();

    Arc::new(move |args: Value| {
        let store = store.clone();
        let conn = conn.clone();
        let name = name.clone();
        let category = category.clone();
        let fallback_detail = fallback_detail.clone();
        let detail_fn = detail_fn.clone();
        let command_fn = command_fn.clone();
        let run_fn = run_fn.clone();
        let settings = settings.clone();
        let on_tool_error = on_tool_error.clone();

        Box::pin(async move {
            let detail = detail_fn.as_ref().map(|f| f(&args)).unwrap_or_else(|| fallback_detail);
            let mut meta = GateMeta::new(category, &name, detail);
            if let Some(command_fn) = command_fn.as_ref() {
                meta = meta.with_command(command_fn(&args, &settings));
            }

            run_gated(
                &store,
                &CallTarget::from(&conn),
                meta,
                move |_log_id| {
                    let run_fn = run_fn.clone();
                    let args = args.clone();
                    let settings = settings.clone();
                    async move { Ok::<Outcome, AdapterError>(Outcome::Ran(shape_output(run_fn(args, settings).await?))) }
                },
                {
                    let name = name.clone();
                    let mut opts = GateOpts::default();
                    if let Some(hook) = on_tool_error {
                        opts = opts.on_error(move |error| hook(&name, error));
                    }
                    opts
                },
            )
            .await
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::ApiResponse;
    use crate::error::SSH_CONNECT_PENDING_CODE;
    use crate::tool_host::ResourceContents;
    use pluk_store::Environment;
    use serde_json::json;
    use std::sync::Mutex;

    const ADAPTER_ID: &str = "test-api";

    /// A host that records registrations instead of serving them.
    #[derive(Default)]
    struct RecordingHost {
        tools: Vec<String>,
        prompts: Vec<String>,
        resources: Vec<String>,
    }

    impl ToolHost for RecordingHost {
        fn register_tool(&mut self, registration: ToolRegistration, _handler: ToolHandler) {
            self.tools.push(registration.name);
        }

        fn register_prompt(&mut self, name: &str, _description: &str, _schema: Option<Map<String, Value>>, _handler: crate::tool_host::PromptHandler) {
            self.prompts.push(name.to_string());
        }

        fn register_resource(&mut self, name: &str, uri: &str, _mime_type: &str, _description: Option<&str>, _handler: Arc<dyn Fn() -> BoxFuture<ResourceContents> + Send + Sync>) {
            self.resources.push(format!("{name}:{uri}"));
        }
    }

    fn temp_store() -> (tempfile::TempDir, Arc<Store>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("pluk.db");
        let store = Arc::new(Store::open(&path).expect("open"));
        (dir, store)
    }

    fn integration(query_policy: Option<&str>) -> Integration {
        Integration {
            id: "r1".into(),
            name: "Test API".into(),
            r#type: ADAPTER_ID.into(),
            config: Map::new(),
            environment: Some(Environment::Development),
            read_only: 0,
            query_policy: query_policy.map(Into::into),
            token: "tok".into(),
            created_at: String::new(),
            via_group: None,
        }
    }

    fn tool_config(names: &[&str]) -> String {
        let entries: Vec<String> = names.iter().map(|n| format!("\"{n}\": {{\"enabled\": true}}")).collect();
        format!("{{\"tools\":{{{}}}}}", entries.join(","))
    }

    fn spec() -> ActionAdapterSpec<()> {
        ActionAdapterSpec::<()>::new(ADAPTER_ID, "Test API", "issue-tracker")
            .access("Read-mostly.")
            .agent_hint("Start with list.")
            .client(|_, _| Ok(()))
            .tools(|_, _| {
                vec![
                    ActionTool::new("list", "List items", ActionCategory::Read)
                        .default_enabled(false)
                        .run(|_, _| async { Ok(ActionOutput::text("items")) }),
                    ActionTool::new("get", "Get one item", ActionCategory::Read)
                        .run(|_, _| async { Ok(ActionOutput::text("item")) }),
                    ActionTool::new("create", "Create an item", ActionCategory::Write)
                        .run(|_, _| async { Ok(ActionOutput::text("created")) }),
                    ActionTool::new("del", "Delete an item", ActionCategory::Delete)
                        .run(|_, _| async { Ok(ActionOutput::text("deleted")) }),
                ]
            })
    }

    fn registered_names(adapter: &dyn Adapter, conn: &Integration) -> Vec<String> {
        let mut host = RecordingHost::default();
        adapter.register(&mut host, conn, "").expect("register");
        host.tools.sort();
        host.tools
    }

    #[test]
    fn unconfigured_integrations_expose_read_tools_but_not_writes_fail_safe() {
        let (_dir, store) = temp_store();
        let adapter = action_adapter(spec(), store);
        let names = registered_names(&adapter, &integration(None));
        // Hidden, not merely blocked — nothing registers them at all.
        assert_eq!(names, vec!["get".to_string()]);
    }

    #[test]
    fn enabling_tools_exposes_exactly_those_tools() {
        let (_dir, store) = temp_store();
        let adapter = action_adapter(spec(), store);
        // `get` stays on through its read default; only `expire`-style tools
        // that are neither enabled nor default-on stay hidden.
        let names = registered_names(&adapter, &integration(Some(&tool_config(&["list", "del"]))));
        assert_eq!(names, vec!["del".to_string(), "get".to_string(), "list".to_string()]);
    }

    #[test]
    fn disabling_a_default_on_read_tool_removes_it() {
        let (_dir, store) = temp_store();
        let adapter = action_adapter(spec(), store);
        let policy = "{\"tools\":{\"get\":{\"enabled\":false},\"list\":{\"enabled\":true}}}";
        let names = registered_names(&adapter, &integration(Some(policy)));
        assert_eq!(names, vec!["list".to_string()]);
    }

    #[test]
    fn the_catalog_derives_defaults_from_categories_with_overrides() {
        let (_dir, store) = temp_store();
        let adapter = action_adapter(spec(), store);
        let find = |name: &str| adapter.tool_specs().iter().find(|t| t.name == name).unwrap();
        assert!(find("get").default_enabled, "plain read ships on");
        assert!(!find("list").default_enabled, "explicit override wins");
        assert!(!find("create").default_enabled, "write always ships off");
        assert!(!find("del").default_enabled, "delete always ships off");
        for tool in adapter.tool_specs() {
            if tool.category != "read" && tool.category != "inspect" {
                assert!(!tool.default_enabled, "no state-changing tool may default on");
            }
        }
    }

    #[test]
    fn a_panicking_client_builder_degrades_to_an_empty_catalog() {
        let broken = ActionAdapterSpec::<()>::new("broken", "Broken", "misc")
            .client(|_, _| panic!("config blank"))
            .tools(|_, _| vec![ActionTool::new("x", "X", ActionCategory::Read)]);
        let (_dir, store) = temp_store();
        let adapter = action_adapter(broken, store);
        assert!(adapter.tool_specs().is_empty());
    }

    #[test]
    fn a_panicking_tools_builder_degrades_to_an_empty_catalog() {
        let broken = ActionAdapterSpec::<u8>::new("broken-tools", "Broken", "misc")
            .client(|_, _| Ok(0u8))
            .tools(|_, _| panic!("tool builder blew up"));
        let (_dir, store) = temp_store();
        let adapter = action_adapter(broken, store);
        assert!(adapter.tool_specs().is_empty());
        // The adapter itself still constructs and describes itself.
        assert_eq!(adapter.id(), "broken-tools");
    }

    #[test]
    fn instructions_list_the_currently_enabled_tools() {
        let (_dir, store) = temp_store();
        let adapter = action_adapter(spec(), store);

        // Unconfigured: only the plain read tool is on.
        let none = adapter.instructions(&integration(None));
        assert!(none.contains("Current policy: Enabled tools: get."), "{none}");

        let some = adapter.instructions(&integration(Some(&tool_config(&["list"]))));
        assert!(some.contains("Current policy: Enabled tools: list, get."), "{some}");
        assert!(
            some.starts_with("Test API integration \"Test API\" — development environment.\nRead-mostly."),
            "{some}"
        );
    }

    #[tokio::test]
    async fn handlers_run_the_gated_lifecycle_and_resolve_settings() {
        let (_dir, store) = temp_store();
        let spec = ActionAdapterSpec::<()>::new(ADAPTER_ID, "Test API", "misc")
            .client(|_, _| Ok(()))
            .tools(|_, _| {
                vec![ActionTool::new("queryish", "Q", ActionCategory::Read)
                    .settings(vec![crate::config_field::ConfigField::new("mode", "Mode", crate::config_field::FieldType::Select)])
                    .detail_fn(|args| format!("queryish {args}"))
                    .run(|_args, settings| async move {
                        Ok(ActionOutput::json(json!({ "mode": settings.get("mode") })))
                    })]
            });
        let adapter = action_adapter(spec, store.clone());
        let policy = "{\"tools\":{\"queryish\":{\"enabled\":true,\"settings\":{\"mode\":\"read-only\"}}}}";
        let conn = integration(Some(policy));

        let mut capturing = CapturingHost::default();
        adapter.register(&mut capturing, &conn, "").expect("register");
        assert_eq!(capturing.handlers.len(), 1);
        assert_eq!(capturing.handlers[0].0, "queryish");
        let handler = capturing.handlers.remove(0).1;
        let result = handler(json!({ "q": 1 })).await;

        assert!(!result.is_error);
        assert!(result.text().contains("\"mode\": \"read-only\""), "{}", result.text());
        let page = store
            .read_log_page(&pluk_store::LogScope::Connection("r1".into()), pluk_store::LogRange::All, None)
            .expect("page");
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].verdict, "allowed");
        assert_eq!(page.entries[0].source.as_deref(), Some("queryish"));
        assert_eq!(page.entries[0].categories.as_deref(), Some("read"));
        // The detail line renders the call's arguments into the log row.
        assert_eq!(page.entries[0].sql, "queryish {\"q\":1}");
    }

    #[tokio::test]
    async fn cli_text_passes_through_verbatim_and_records_the_command() {
        let (_dir, store) = temp_store();
        let spec = ActionAdapterSpec::<()>::new(ADAPTER_ID, "CLI", "misc")
            .client(|_, _| Ok(()))
            .tools(|_, _| {
                vec![ActionTool::new("gh", "GitHub via gh", ActionCategory::Admin)
                    .command_fn(|_args, _settings| "gh pr list --limit 30".to_string())
                    .run(|_, _| async {
                        Ok(ActionOutput::with_command(Value::String("PR #1\nPR #2".into()), "gh pr list --limit 30 --state open"))
                    })]
            });
        let adapter = action_adapter(spec, store.clone());
        let conn = integration(Some(&tool_config(&["gh"])));

        let mut capturing = CapturingHost::default();
        adapter.register(&mut capturing, &conn, "").expect("register");
        let handler = capturing.handlers.remove(0).1;
        let result = handler(json!({})).await;

        // Verbatim: no JSON quoting around the text.
        assert_eq!(result.text(), "PR #1\nPR #2");
        let page = store
            .read_log_page(&pluk_store::LogScope::Connection("r1".into()), pluk_store::LogRange::All, None)
            .expect("page");
        assert_eq!(page.entries[0].sql, "gh pr list --limit 30 --state open");
        assert_eq!(page.entries[0].response_text.as_deref(), Some("PR #1\nPR #2"));
        assert_eq!(page.entries[0].row_count, Some(1));
    }

    #[tokio::test]
    async fn structured_output_renders_as_pretty_json() {
        let (_dir, store) = temp_store();
        let spec = ActionAdapterSpec::<()>::new(ADAPTER_ID, "API", "misc")
            .client(|_, _| Ok(()))
            .tools(|_, _| {
                vec![ActionTool::new("get_issue", "G", ActionCategory::Read)
                    .run(|_, _| async { Ok(ActionOutput::json(json!({ "id": 7, "title": "Bug" })))})]            });
        let adapter = action_adapter(spec, store.clone());
        let conn = integration(Some(&tool_config(&["get_issue"])));

        let mut capturing = CapturingHost::default();
        adapter.register(&mut capturing, &conn, "").expect("register");
        let handler = capturing.handlers.remove(0).1;
        let result = handler(json!({})).await;

        assert!(result.text().contains("\n  \""), "expected indented JSON: {}", result.text());
        assert_eq!(result.text(), "{\n  \"id\": 7,\n  \"title\": \"Bug\"\n}");
    }

    #[tokio::test]
    async fn tool_errors_flow_through_the_classifier_and_the_hook() {
        let (_dir, store) = temp_store();
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let capture = seen.clone();
        let spec = ActionAdapterSpec::<()>::new(ADAPTER_ID, "API", "misc")
            .client(|_, _| Ok(()))
            .on_tool_error(move |tool, error| capture.lock().unwrap().push(format!("{tool}: {}", error.message)))
            .tools(|_, _| {
                vec![
                    ActionTool::new("flaky", "F", ActionCategory::Read)
                        .run(|_, _| async { Err(AdapterError::new("boom")) }),
                    ActionTool::new("pending", "P", ActionCategory::Read)
                        .run(|_, _| async { Err(AdapterError::new("waiting").with_code(SSH_CONNECT_PENDING_CODE)) }),
                ]
            });
        let adapter = action_adapter(spec, store.clone());
        let conn = integration(Some(&tool_config(&["flaky", "pending"])));

        let mut capturing = CapturingHost::default();
        adapter.register(&mut capturing, &conn, "").expect("register");
        let handlers = capturing.handlers;
        let flaky = handlers.iter().find(|(n, _)| n == "flaky").unwrap().1.clone();
        let pending = handlers.iter().find(|(n, _)| n == "pending").unwrap().1.clone();

        let failed = flaky(json!({})).await;
        assert!(failed.is_error);
        assert_eq!(failed.text(), "Error: boom");

        let waiting = pending(json!({})).await;
        assert_eq!(waiting.text(), "Error: waiting");

        let reported = seen.lock().unwrap().clone();
        // Only the true error reaches the hook; the SSH pending approval is suppressed.
        assert_eq!(reported, vec!["flaky: boom".to_string()]);
    }

    #[tokio::test]
    async fn declined_registrations_surface_as_errors() {
        let (_dir, store) = temp_store();
        let spec = ActionAdapterSpec::<()>::new("bad-client", "Bad", "misc")
            .client(|_, _| Err(AdapterError::new("missing token")))
            .tools(|_, _| vec![ActionTool::new("x", "X", ActionCategory::Read)]);
        let adapter = action_adapter(spec, store);
        let error = adapter.register(&mut RecordingHost::default(), &integration(None), "");
        assert_eq!(error.expect_err("must fail").message, "missing token");
    }

    #[test]
    fn api_response_helpers_shape_plain_and_json_bodies() {
        let text = ApiResponse::text(405, "Method not allowed");
        assert_eq!(text.status, 405);
        assert_eq!(text.body, b"Method not allowed");

        let json_body = ApiResponse::json(200, &json!({ "ok": true }));
        assert_eq!(json_body.content_type.as_deref(), Some("application/json"));
        assert_eq!(std::str::from_utf8(&json_body.body).unwrap(), "{\"ok\":true}");
    }

    /// A host that keeps handlers reachable for direct invocation.
    #[derive(Default)]
    struct CapturingHost {
        handlers: Vec<(String, ToolHandler)>,
    }

    impl ToolHost for CapturingHost {
        fn register_tool(&mut self, registration: ToolRegistration, handler: ToolHandler) {
            self.handlers.push((registration.name, handler));
        }

        fn register_prompt(&mut self, _name: &str, _description: &str, _schema: Option<Map<String, Value>>, _handler: crate::tool_host::PromptHandler) {}

        fn register_resource(&mut self, _name: &str, _uri: &str, _mime_type: &str, _description: Option<&str>, _handler: Arc<dyn Fn() -> BoxFuture<ResourceContents> + Send + Sync>) {}
    }
}
