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
pub mod github_cli;
mod instructions;
pub mod linear;
mod projection;
pub mod redis;
mod registry;
pub mod sentry;
pub mod slack;
pub mod spark;
pub mod sql;
pub mod ssh;
mod ssh_fields;
#[cfg(test)]
mod test_support;
mod tool_host;
mod tool_spec;

pub use action::{
    ActionAdapter, ActionAdapterSpec, ActionOutput, ActionTool, ClientFn, HumanizeFn,
    TestConnectionFn, ToolErrorHook, ToolsFn, action_adapter,
};
pub use adapter::{Adapter, ApiRequest, ApiResponse, PolicyKind};
pub use config_field::{ConfigField, FieldType, SelectOption, ShowIf, normalize_scalar};
pub use error::{AdapterError, SSH_CONNECT_PENDING_CODE};
pub use gate::{
    CallTarget, GateMeta, GateOpts, Outcome, RunOutcome, TextContent, ToolResult,
    cancelled_when_message_contains, err, ok, run_gated,
};
pub use instructions::{InstructionParts, build_instructions};
pub use projection::{
    FieldMap, OnlyError, Preset, ReduceFn, apply_only, only_param_description, only_param_schema,
    only_value, pick_paths,
};
pub use registry::{AdapterRegistry, default_registry};
pub use ssh_fields::ssh_auth_fields;
pub use tool_host::{
    BoxFuture, PromptHandler, PromptMessage, PromptResult, PromptRole, ResourceContents,
    ResourceHandler, ToolHandler, ToolHost, ToolRegistration, object_schema,
};
pub use tool_spec::ToolSpec;
