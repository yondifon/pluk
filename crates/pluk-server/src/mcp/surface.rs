//! The MCP surface one request sees: the registrations adapters pushed onto a
//! [`SurfaceBuilder`], served through `rmcp`'s [`ServerHandler`].
//!
//! Adapters never touch the MCP SDK — they push tools, prompts and resources
//! onto the [`pluk_adapters::ToolHost`] trait (see R04). This module is that
//! trait's server-side implementation, plus the wire translation.

use std::sync::Arc;

use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
    GetPromptRequestParams, GetPromptResponse, GetPromptResult, Implementation, ListPromptsResult,
    ListResourcesResult, ListToolsResult, Prompt, PromptArgument, PromptMessage,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, ResourceContents, Role,
    ServerCapabilities, ServerInfo, ToolsCapability,
};
use rmcp::{ErrorData as McpError, ServerHandler};
use serde_json::{Map, Value};

use pluk_adapters::{PromptHandler, PromptRole, ResourceHandler, ToolHandler, ToolRegistration};

/// Protocol revision 2026-07-28 requires `ttlMs` and `cacheScope` on every
/// list and read result (SEP-2549); strict clients reject a response without
/// them. A surface is derived from the token owner's integrations and changes
/// whenever those do, so it is never shareable and never fresh.
const SURFACE_TTL_MS: u64 = 0;

/// One registered tool.
struct RegisteredTool {
    description: String,
    input_schema: Map<String, Value>,
    annotations: Map<String, Value>,
    handler: ToolHandler,
}

/// One registered prompt.
struct RegisteredPrompt {
    description: String,
    args_schema: Option<Map<String, Value>>,
    handler: PromptHandler,
}

/// One registered resource.
struct RegisteredResource {
    name: String,
    mime_type: String,
    description: Option<String>,
    handler: ResourceHandler,
}

/// Collects adapter registrations in order, plus the discovery identity.
/// Duplicate names are recorded, not thrown: the TypeScript SDK raised at
/// registration time, which would abort the request mid-build; here the
/// duplicate fails the built surface instead, surfacing as a protocol error
/// for every request until fixed.
#[derive(Default)]
pub struct SurfaceBuilder {
    server_name: String,
    instructions: Option<String>,
    tools: Vec<(String, RegisteredTool)>,
    prompts: Vec<(String, RegisteredPrompt)>,
    resources: Vec<(String, RegisteredResource)>,
    duplicate: Option<String>,
}

impl SurfaceBuilder {
    /// The `serverInfo.name` returned by initialize/discovery. Defaults to
    /// the empty string; callers set it to the integration or group name.
    pub fn set_server_name(&mut self, name: impl Into<String>) -> &mut Self {
        self.server_name = name.into();
        self
    }

    /// The agent-facing guidance block returned with discovery results.
    pub fn set_instructions(&mut self, instructions: impl Into<Option<String>>) -> &mut Self {
        self.instructions = instructions.into();
        self
    }
}

impl pluk_adapters::ToolHost for SurfaceBuilder {
    fn register_tool(&mut self, registration: ToolRegistration, handler: ToolHandler) {
        if self
            .tools
            .iter()
            .any(|(name, _)| *name == registration.name)
        {
            self.duplicate = Some(format!("tool {}", registration.name));
            return;
        }
        self.tools.push((
            registration.name,
            RegisteredTool {
                description: registration.description,
                input_schema: registration.input_schema,
                annotations: registration.annotations,
                handler,
            },
        ));
    }

    fn register_prompt(
        &mut self,
        name: &str,
        description: &str,
        args_schema: Option<Map<String, Value>>,
        handler: PromptHandler,
    ) {
        if self.prompts.iter().any(|(known, _)| known == name) {
            self.duplicate = Some(format!("prompt {name}"));
            return;
        }
        self.prompts.push((
            name.to_string(),
            RegisteredPrompt {
                description: description.to_string(),
                args_schema,
                handler,
            },
        ));
    }

    fn register_resource(
        &mut self,
        name: &str,
        uri: &str,
        mime_type: &str,
        description: Option<&str>,
        handler: ResourceHandler,
    ) {
        if self.resources.iter().any(|(known, _)| known == uri) {
            self.duplicate = Some(format!("resource {uri}"));
            return;
        }
        self.resources.push((
            uri.to_string(),
            RegisteredResource {
                name: name.to_string(),
                mime_type: mime_type.to_string(),
                description: description.map(str::to_string),
                handler,
            },
        ));
    }
}

impl SurfaceBuilder {
    /// Freeze the collected registrations into a servable surface. Fails when
    /// an adapter registered the same tool/prompt/resource twice.
    pub fn build(self) -> Result<Surface, String> {
        if let Some(duplicate) = self.duplicate {
            return Err(format!("already registered: {duplicate}"));
        }
        Ok(Surface {
            server_name: self.server_name,
            instructions: self.instructions,
            tools: self.tools,
            prompts: self.prompts,
            resources: self.resources,
        })
    }
}

/// The immutable result of one build. Cheap to serve from: lookups are linear
/// scans over at most dozens of entries.
pub struct Surface {
    server_name: String,
    instructions: Option<String>,
    tools: Vec<(String, RegisteredTool)>,
    prompts: Vec<(String, RegisteredPrompt)>,
    resources: Vec<(String, RegisteredResource)>,
}

/// A tool with no schema is advertised as a plain object, matching what the
/// TypeScript SDK produced for zero-argument tools.
fn normalized_input_schema(schema: &Map<String, Value>) -> Arc<Map<String, Value>> {
    let mut schema = schema.clone();
    if schema.is_empty() {
        schema.insert("type".into(), Value::String("object".into()));
        schema.insert("properties".into(), Value::Object(Map::new()));
    }
    Arc::new(schema)
}

/// Adapter-declared annotation keys are camelCase JSON (`readOnlyHint`);
/// deserialize them straight into the matching camelCase-tolerant model type.
fn tool_annotations(annotations: &Map<String, Value>) -> Option<rmcp::model::ToolAnnotations> {
    (!annotations.is_empty())
        .then(|| serde_json::from_value(Value::Object(annotations.clone())).ok())
        .flatten()
}

/// The prompt arguments advertised by `prompts/list`, derived from the
/// registered JSON-schema argument object: one entry per property, required
/// per the schema's `required` list.
fn prompt_arguments(args_schema: &Option<Map<String, Value>>) -> Option<Vec<PromptArgument>> {
    let schema = args_schema.as_ref()?;
    let properties = schema.get("properties")?.as_object()?;
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let arguments: Vec<PromptArgument> = properties
        .iter()
        .map(|(name, spec)| {
            let mut argument = PromptArgument::new(name.clone());
            if let Some(description) = spec.get("description").and_then(Value::as_str) {
                argument.description = Some(description.to_string());
            }
            argument.required = Some(required.contains(&name.as_str()));
            argument
        })
        .collect();
    (!arguments.is_empty()).then_some(arguments)
}

fn prompt_role(role: PromptRole) -> Role {
    match role {
        PromptRole::User => Role::User,
        PromptRole::Assistant => Role::Assistant,
    }
}

impl ServerHandler for Surface {
    fn get_info(&self) -> ServerInfo {
        let mut capabilities = ServerCapabilities::default();
        if !self.prompts.is_empty() {
            capabilities.prompts = Some(rmcp::model::PromptsCapability::default());
        }
        if !self.resources.is_empty() {
            capabilities.resources = Some(rmcp::model::ResourcesCapability::default());
        }
        if !self.tools.is_empty() {
            capabilities.tools = Some(ToolsCapability::default());
        }
        // The negotiated revision is filled in per initialize request; this
        // default advertises the SDK's latest supported version otherwise.
        let mut info = ServerInfo::new(capabilities)
            .with_server_info(Implementation::new(self.server_name.clone(), "1.0.0"));
        info.instructions = self.instructions.clone();
        info
    }

    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let tools = self
            .tools
            .iter()
            .map(|(name, tool)| {
                let mut entry = rmcp::model::Tool::new(
                    name.clone(),
                    tool.description.clone(),
                    normalized_input_schema(&tool.input_schema),
                );
                entry.annotations = tool_annotations(&tool.annotations);
                entry
            })
            .collect();
        Ok(ListToolsResult::with_all_items(tools)
            .with_ttl_ms(SURFACE_TTL_MS)
            .with_cache_scope(CacheScope::Private))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let (_, tool) = self
            .tools
            .iter()
            .find(|(name, _)| *name == request.name.as_ref())
            .ok_or_else(|| {
                McpError::invalid_params(format!("Unknown tool: {}", request.name), None)
            })?;

        let arguments = Value::Object(request.arguments.unwrap_or_default());
        let result = (tool.handler)(arguments).await;

        let content = result
            .content
            .iter()
            .map(|block| ContentBlock::text(block.text.clone()))
            .collect();
        let mut call = CallToolResult::success(content);
        call.is_error = Some(result.is_error);
        Ok(CallToolResponse::Complete(call))
    }

    async fn list_prompts(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        let prompts = self
            .prompts
            .iter()
            .map(|(name, prompt)| {
                Prompt::new(
                    name.clone(),
                    Some(prompt.description.clone()),
                    prompt_arguments(&prompt.args_schema),
                )
            })
            .collect();
        Ok(ListPromptsResult::with_all_items(prompts)
            .with_ttl_ms(SURFACE_TTL_MS)
            .with_cache_scope(CacheScope::Private))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        let (_, prompt) = self
            .prompts
            .iter()
            .find(|(name, _)| *name == request.name.as_ref())
            .ok_or_else(|| {
                McpError::invalid_params(format!("Unknown prompt: {}", request.name), None)
            })?;

        let result = (prompt.handler)(request.arguments.unwrap_or_default()).await;
        let messages = result
            .messages
            .iter()
            .map(|message| {
                PromptMessage::new(
                    prompt_role(message.role),
                    ContentBlock::text(message.text.clone()),
                )
            })
            .collect();
        Ok(GetPromptResponse::Complete(GetPromptResult::new(messages)))
    }

    async fn list_resources(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let resources = self
            .resources
            .iter()
            .map(|(uri, resource)| {
                let mut entry = rmcp::model::Resource::new(uri.clone(), resource.name.clone());
                entry.description = resource.description.clone();
                entry.mime_type = Some(resource.mime_type.clone());
                entry
            })
            .collect();
        Ok(ListResourcesResult::with_all_items(resources)
            .with_ttl_ms(SURFACE_TTL_MS)
            .with_cache_scope(CacheScope::Private))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let (_, resource) = self
            .resources
            .iter()
            .find(|(uri, _)| uri.as_str() == request.uri.as_str())
            .ok_or_else(|| {
                McpError::invalid_params(format!("Unknown resource: {}", request.uri), None)
            })?;

        let contents = (resource.handler)().await;
        let contents =
            ResourceContents::text(contents.text, contents.uri).with_mime_type(contents.mime_type);
        Ok(ReadResourceResponse::Complete(
            ReadResourceResult::new(vec![contents])
                .with_ttl_ms(SURFACE_TTL_MS)
                .with_cache_scope(CacheScope::Private),
        ))
    }
}
