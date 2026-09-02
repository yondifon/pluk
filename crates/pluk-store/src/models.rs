//! Typed rows for the `pluk.db` schema.
//!
//! Column names are kept verbatim (including legacy ones like
//! `groups.member_ids` and `integrations.read_only`) for compatibility.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Map;

use crate::query_log::LogGroup;

/// A config blob: string-keyed, per-adapter values. Holds secrets; never log it.
pub type Config = Map<String, serde_json::Value>;

/// The four environments a connection or group can be scoped to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Environment {
    Production,
    Staging,
    Development,
    Local,
}

impl Environment {
    pub fn as_str(self) -> &'static str {
        match self {
            Environment::Production => "production",
            Environment::Staging => "staging",
            Environment::Development => "development",
            Environment::Local => "local",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "production" => Environment::Production,
            "staging" => Environment::Staging,
            "development" => Environment::Development,
            "local" => Environment::Local,
            _ => return None,
        })
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A configured service — a database, Linear, Sentry, … resolved to an adapter
/// by its `type` field (the adapter id).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Integration {
    pub id: String,
    pub name: String,
    /// Adapter id, e.g. `postgres`, `linear`, `github-cli`.
    pub r#type: String,
    pub config: Config,
    /// `None` only for rows written before environments existed; new rows
    /// always carry one (the column default is `development`).
    pub environment: Option<Environment>,
    /// Legacy column; kept for schema compatibility.
    pub read_only: i64,
    /// Serialized [`QueryPolicy`](crate::codec::QueryPolicy) blob, passed
    /// through opaquely so unknown fields survive round trips untouched.
    pub query_policy: Option<String>,
    pub token: String,
    pub created_at: String,
    /// Transient, not persisted: set only when this integration is registered
    /// as a member of a group, so the gated runner attributes its log rows to
    /// the group endpoint that fronted the call.
    #[serde(skip)]
    pub via_group: Option<LogGroup>,
}

/// One group member: an integration id plus optional per-group config overrides.
///
/// Rows may hold either the current form (`{"id": …, "overrides": {…}}`) or the
/// legacy form (a bare id string); parsing accepts both. Serialization always
/// emits the current form, omitting empty overrides exactly like both existing
/// writers do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupMember {
    pub id: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub overrides: Map<String, serde_json::Value>,
}

/// Several integrations fronted by one MCP token/endpoint.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    /// `None` means the group spans all environments.
    pub environment: Option<Environment>,
    pub members: Vec<GroupMember>,
    pub token: String,
    pub created_at: String,
}

/// A group member resolved to its live integration, with the group's overrides.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMember {
    pub integration: Integration,
    pub overrides: Map<String, serde_json::Value>,
}

/// Verdict recorded on a query-log row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    #[default]
    Pending,
    Allowed,
    Blocked,
    Cancelled,
    Error,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Pending => "pending",
            Verdict::Allowed => "allowed",
            Verdict::Blocked => "blocked",
            Verdict::Cancelled => "cancelled",
            Verdict::Error => "error",
        }
    }
}

/// One audit-log row, carrying every `query_log` column.
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub id: i64,
    pub connection_id: String,
    pub connection_name: String,
    pub sql: String,
    pub verdict: String,
    pub reason: Option<String>,
    pub categories: Option<String>,
    pub source: Option<String>,
    pub result_json: Option<String>,
    pub row_count: Option<i64>,
    pub response_text: Option<String>,
    pub group_id: Option<String>,
    pub group_name: Option<String>,
    pub database: Option<String>,
    pub created_at: String,
}

/// One masked column: a result column whose values never reach agents raw.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MaskedColumn {
    pub id: String,
    pub connection_id: String,
    pub column_name: String,
    pub created_at: String,
}

/// A user-saved SQL statement for one connection.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SavedQuery {
    pub id: String,
    pub connection_id: String,
    pub name: String,
    pub sql: String,
    pub created_at: String,
}

/// A user-curated shell command for an SSH integration. Agents run these by
/// name through `run_saved_command`; there is no allowlist beyond this table.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SavedCommand {
    pub id: String,
    pub connection_id: String,
    pub name: String,
    pub command: String,
    pub working_dir: Option<String>,
    pub created_at: String,
}
