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
        ToolRegistration { name: name.into(), description: description.into(), input_schema: Map::new(), annotations: Map::new() }
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
            Value::Array(required.iter().map(|r| Value::String((*r).to_string())).collect()),
        );
    }
    schema
}

/// Where adapters register their surface. Implemented by the server crate.
pub trait ToolHost {
    fn register_tool(&mut self, registration: ToolRegistration, handler: ToolHandler);
    fn register_prompt(&mut self, name: &str, description: &str, args_schema: Option<Map<String, Value>>, handler: PromptHandler);
    fn register_resource(&mut self, name: &str, uri: &str, mime_type: &str, description: Option<&str>, handler: ResourceHandler);
}
