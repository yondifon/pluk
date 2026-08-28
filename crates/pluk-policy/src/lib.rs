//! SQL policy engine for Pluk.
//!
//! Decides whether a SQL statement an AI agent wants to run is allowed:
//! statement classification (AST via `sqlparser`, keyword fallback),
//! dangerous-construct scanning, policy evaluation, and the database pin
//! rule. This crate is a security boundary — every path that cannot identify
//! a statement denies it (fail closed).
//!
//! Ported from `pluk/src/mcp/policy.ts`, `toolConfig.ts`, `actionPolicy.ts`
//! and `pluk/src/db/dbName.ts`.

pub mod action;
pub mod ast;
pub mod category;
pub mod classify;
pub mod dangerous;
pub mod db_name;
pub mod dialect;
pub mod error;
pub mod keywords;
pub mod policy;
pub mod tool_config;

pub use action::{
    ActionCategory, ActionPolicy, action_allowed, action_policy_description, parse_action_policy,
};
pub use category::{ALL_CATEGORIES, CATEGORY_GROUPS, CategoryGroup, StatementCategory};
pub use classify::{ClassifyResult, classify};
pub use dangerous::{DangerousConstruct, scan_dangerous};
pub use db_name::{is_valid_database_name, resolve_override_database};
pub use dialect::{Dialect, dialect_for};
pub use error::PolicyError;
pub use policy::{
    CapResult, CostEstimate, EvalResult, PresetName, QueryPolicy, cap_rows, default_policy_for,
    evaluate, parse_policy, parse_postgres_cost, sql_policy_from_settings,
};
pub use tool_config::{
    StoredToolState, ToolConfig, ToolGate, default_enabled_for_category, parse_tool_config,
    setting_bool, setting_number_or_null, setting_string, tool_gate,
};
