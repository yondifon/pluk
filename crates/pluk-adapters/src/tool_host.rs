//! The registration surface adapters build against.
//!
//! Adapters never touch the MCP SDK: they push tools, prompts and resources
//! onto a [`ToolHost`], and the server crate (R05) implements that host over
//! the real SDK. A disabled tool is never registered at all — registration
//! *is* the enable switch, so the agent's tool list only ever shows what is
//! actually callable.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value};

/// A boxed, sendable future — the currency of async registration.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// One registered tool's call handler. Receives the raw argument object and
/// returns a shaped MCP result.
pub type ToolHandler = Arc<dyn Fn(Value) -> BoxFuture<crate::gate::ToolResult> + Send + Sync>;

/// The role line of a prompt message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptRole {
    User,
    Assistant,
}

/// One message of a prompt result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PromptMessage {
    pub role: PromptRole,
    /// Rendered as `{ type: "text", text }` content on the wire.
    pub text: String,
}

/// What a prompt handler returns (MCP `GetPromptResult`, text messages).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PromptResult {
    pub messages: Vec<PromptMessage>,
}

/// A prompt handler receives the prompt's filled-in arguments.
pub type PromptHandler = Arc<dyn Fn(Map<String, Value>) -> BoxFuture<PromptResult> + Send + Sync>;

/// Text contents of a resource read (MCP `ReadResourceResult`, one text part).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResourceContents {
    pub uri: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    pub text: String,
}

pub type ResourceHandler = Arc<dyn Fn() -> BoxFuture<ResourceContents> + Send + Sync>;

/// A tool about to be registered onto a host.
#[derive(Debug, Clone)]
pub struct ToolRegistration {
    pub name: String,
    pub description: String,
    /// Full JSON Schema object for the tool's input
    /// (`{"type":"object","properties":{…}}`). Empty when the tool takes no
    /// arguments; use [`object_schema`] to build one from properties.
    pub input_schema: Map<String, Value>,
    /// MCP tool-annotation hints (`readOnlyHint`, `destructiveHint`, …),
    /// camelCase like the TypeScript server sent them. Hints only — never
    /// enforced here.
    pub annotations: Map<String, Value>,
}

impl ToolRegistration {
    pub fn no_args(name: impl Into<String>, description: impl Into<String>) -> Self {
        ToolRegistration {
            name: name.into(),
            description: description.into(),
            input_schema: Map::new(),
            annotations: Map::new(),
        }
    }

    /// Attach annotation hints (camelCase keys).
    pub fn with_annotations(mut self, annotations: Map<String, Value>) -> Self {
        self.annotations = annotations;
        self
    }
}

/// Build an `input_schema` object from property definitions plus required keys.
pub fn object_schema(properties: Map<String, Value>, required: &[&str]) -> Map<String, Value> {
    let mut schema = Map::new();
    schema.insert("type".into(), Value::String("object".into()));
    schema.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert(
            "required".into(),
            Value::Array(
                required
                    .iter()
                    .map(|r| Value::String((*r).to_string()))
                    .collect(),
            ),
        );
    }
    schema
}

/// Where adapters register their surface. Implemented by the server crate.
pub trait ToolHost {
    fn register_tool(&mut self, registration: ToolRegistration, handler: ToolHandler);
    fn register_prompt(
        &mut self,
        name: &str,
        description: &str,
        args_schema: Option<Map<String, Value>>,
        handler: PromptHandler,
    );
    fn register_resource(
        &mut self,
        name: &str,
        uri: &str,
        mime_type: &str,
        description: Option<&str>,
        handler: ResourceHandler,
    );
}

/// A [`ToolHost`] that drops every tool the integration's policy disables.
///
/// Registration is the enable switch, so the filter belongs *between* the
/// adapter and the host rather than inside each adapter: an adapter that
/// forgets to consult the policy still cannot expose a disabled tool.
///
/// A tool absent from the adapter's catalog is dropped. The catalog is what
/// the settings UI renders, so a name missing from it has no switch a user
/// could ever turn off.
pub struct PolicyGatedHost<'a> {
    inner: &'a mut dyn ToolHost,
    gate: pluk_policy::ToolGate,
    defaults: std::collections::HashMap<String, bool>,
}

impl<'a> PolicyGatedHost<'a> {
    pub fn new(
        inner: &'a mut dyn ToolHost,
        specs: &[crate::tool_spec::ToolSpec],
        query_policy: Option<&str>,
    ) -> Self {
        PolicyGatedHost {
            inner,
            gate: pluk_policy::tool_gate(query_policy),
            defaults: specs
                .iter()
                .map(|spec| (spec.name.clone(), spec.default_enabled))
                .collect(),
        }
    }

    fn allows(&self, name: &str) -> bool {
        match self.defaults.get(name) {
            Some(default) => self.gate.enabled(name, *default),
            None => false,
        }
    }
}

impl ToolHost for PolicyGatedHost<'_> {
    fn register_tool(&mut self, registration: ToolRegistration, handler: ToolHandler) {
        if self.allows(&registration.name) {
            self.inner.register_tool(registration, handler);
        }
    }

    fn register_prompt(
        &mut self,
        name: &str,
        description: &str,
        args_schema: Option<Map<String, Value>>,
        handler: PromptHandler,
    ) {
        self.inner
            .register_prompt(name, description, args_schema, handler);
    }

    fn register_resource(
        &mut self,
        name: &str,
        uri: &str,
        mime_type: &str,
        description: Option<&str>,
        handler: ResourceHandler,
    ) {
        self.inner
            .register_resource(name, uri, mime_type, description, handler);
    }
}

/// Register an adapter's surface with its integration's tool policy enforced.
/// Every endpoint builds its surface through here.
pub fn register_gated(
    adapter: &dyn crate::adapter::Adapter,
    host: &mut dyn ToolHost,
    conn: &pluk_store::Integration,
    owner_id: &str,
) -> Result<(), crate::error::AdapterError> {
    let mut gated = PolicyGatedHost::new(host, adapter.tool_specs(), conn.query_policy.as_deref());
    adapter.register(&mut gated, conn, owner_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{Adapter, PolicyKind};
    use crate::config_field::ConfigField;
    use crate::error::AdapterError;
    use crate::tool_spec::ToolSpec;
    use async_trait::async_trait;
    use pluk_store::Integration;

    /// An adapter that registers its whole surface without consulting the
    /// policy — the shape every hand-written `register` had.
    struct UngatedAdapter {
        specs: Vec<ToolSpec>,
        registers: Vec<String>,
    }

    #[async_trait]
    impl Adapter for UngatedAdapter {
        fn id(&self) -> &str {
            "ungated"
        }
        fn label(&self) -> &str {
            "Ungated"
        }
        fn category(&self) -> &str {
            "misc"
        }
        fn policy_kind(&self) -> PolicyKind {
            PolicyKind::Action
        }
        fn agent_hint(&self) -> &str {
            ""
        }
        fn tool_specs(&self) -> &[ToolSpec] {
            &self.specs
        }
        fn config_fields(&self) -> &[ConfigField] {
            &[]
        }
        async fn test_connection(&self, _conn: &Integration) -> Result<(), AdapterError> {
            Ok(())
        }
        fn instructions(&self, _conn: &Integration) -> String {
            String::new()
        }
        fn register(
            &self,
            host: &mut dyn ToolHost,
            _conn: &Integration,
            _owner_id: &str,
        ) -> Result<(), AdapterError> {
            for name in &self.registers {
                host.register_tool(
                    ToolRegistration::no_args(name, "…"),
                    Arc::new(|_| Box::pin(async { crate::gate::ok("ran") })),
                );
            }
            Ok(())
        }
    }

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
        fn register_prompt(
            &mut self,
            name: &str,
            _description: &str,
            _args_schema: Option<Map<String, Value>>,
            _handler: PromptHandler,
        ) {
            self.prompts.push(name.to_string());
        }
        fn register_resource(
            &mut self,
            name: &str,
            _uri: &str,
            _mime_type: &str,
            _description: Option<&str>,
            _handler: ResourceHandler,
        ) {
            self.resources.push(name.to_string());
        }
    }

    fn adapter() -> UngatedAdapter {
        UngatedAdapter {
            specs: vec![
                ToolSpec::new("list", "List", "read"),
                ToolSpec::new("post_message", "Post", "write"),
                ToolSpec::new("del", "Delete", "delete"),
            ],
            registers: vec!["list".into(), "post_message".into(), "del".into()],
        }
    }

    fn integration(query_policy: Option<&str>) -> Integration {
        Integration {
            id: "i1".into(),
            name: "Test".into(),
            r#type: "ungated".into(),
            config: Map::new(),
            environment: None,
            read_only: 0,
            query_policy: query_policy.map(Into::into),
            token: "t".into(),
            created_at: String::new(),
            via_group: None,
        }
    }

    fn registered(conn: &Integration) -> Vec<String> {
        let mut host = RecordingHost::default();
        register_gated(&adapter(), &mut host, conn, "").expect("register");
        host.tools
    }

    #[test]
    fn a_fresh_integration_exposes_no_write_or_delete_tool() {
        assert_eq!(registered(&integration(None)), vec!["list".to_string()]);
    }

    #[test]
    fn the_toggle_decides_what_the_agent_can_reach() {
        let on = r#"{"tools":{"post_message":{"enabled":true}}}"#;
        assert_eq!(
            registered(&integration(Some(on))),
            vec!["list".to_string(), "post_message".to_string()]
        );

        let off = r#"{"tools":{"list":{"enabled":false}}}"#;
        assert!(registered(&integration(Some(off))).is_empty());
    }

    #[test]
    fn a_tool_missing_from_the_catalog_is_dropped() {
        let mut host = RecordingHost::default();
        let undeclared = UngatedAdapter {
            specs: vec![ToolSpec::new("list", "List", "read")],
            registers: vec!["list".into(), "secret_admin".into()],
        };
        register_gated(&undeclared, &mut host, &integration(None), "").expect("register");
        // No catalog entry means no toggle a user could ever switch off.
        assert_eq!(host.tools, vec!["list".to_string()]);
    }

    #[test]
    fn prompts_and_resources_pass_through() {
        let mut inner = RecordingHost::default();
        let mut gated = PolicyGatedHost::new(&mut inner, &[], None);
        gated.register_prompt("summarize", "…", None, Arc::new(|_| {
            Box::pin(async { PromptResult { messages: Vec::new() } })
        }));
        gated.register_resource("schema", "schema://full", "text/plain", None, Arc::new(|| {
            Box::pin(async {
                ResourceContents {
                    uri: "schema://full".into(),
                    mime_type: "text/plain".into(),
                    text: String::new(),
                }
            })
        }));
        assert_eq!(inner.prompts, vec!["summarize".to_string()]);
        assert_eq!(inner.resources, vec!["schema".to_string()]);
    }
}
