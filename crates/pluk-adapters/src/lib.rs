//! Adapter framework for Pluk: the [`Adapter`] contract, the audited gated
//! call lifecycle, config-field schemas, `only` field projection, and the
//! action-adapter factory. Per-service adapters (databases, SSH hosts, APIs,
//! CLIs) build on this crate — see tasks R09–R14.
//!
//! Ported from `pluk/src/adapters/{types,kit,onlyProjection,index}.ts` and
//! the shared helpers they carry.

mod action;
mod adapter;
mod config_field;
mod error;
mod gate;
mod instructions;
mod projection;
mod registry;
mod ssh_fields;
mod tool_host;
mod tool_spec;
pub mod github_cli;
pub mod sql;
pub mod ssh;

pub use action::{
    action_adapter, ActionAdapter, ActionAdapterSpec, ActionOutput, ActionTool, ClientFn,
    HumanizeFn, TestConnectionFn, ToolErrorHook, ToolsFn,
};
pub use adapter::{Adapter, ApiRequest, ApiResponse, PolicyKind};
pub use config_field::{normalize_scalar, ConfigField, FieldType, SelectOption, ShowIf};
pub use error::{AdapterError, SSH_CONNECT_PENDING_CODE};
pub use gate::{
    cancelled_when_message_contains, err, ok, run_gated, CallTarget, GateMeta, GateOpts, Outcome,
    RunOutcome, TextContent, ToolResult,
};
pub use instructions::{build_instructions, InstructionParts};
pub use projection::{
    apply_only, only_param_description, only_param_schema, only_value, pick_paths, FieldMap,
    OnlyError, Preset, ReduceFn,
};
pub use registry::AdapterRegistry;
pub use ssh_fields::ssh_auth_fields;
pub use tool_host::{
    object_schema, BoxFuture, PromptHandler, PromptMessage, PromptResult, PromptRole,
    ResourceContents, ResourceHandler, ToolHandler, ToolHost, ToolRegistration,
};
pub use tool_spec::ToolSpec;
