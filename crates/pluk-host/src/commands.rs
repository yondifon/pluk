//! Typed Tauri command surface — the boundary the webview calls.
//!
//! Every command here round-trips through serde, so mismatches fail fast.
//! The surface covers: integrations, groups, adapter catalog, test connection,
//! health, log paging, cancel, reload (dropping owner sessions), zoom and
//! frame.

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::frame::{self, Frame};
use crate::server::ServerHandle;
use crate::zoom::ZoomState;

type CmdResult<T> = Result<T, String>;

// ── Shared state managed by Tauri ─────────────────────────────────────

pub struct HostState {
    pub store: std::sync::Arc<pluk_store::Store>,
    pub server: tokio::sync::Mutex<ServerHandle>,
    /// The server's shared state, held directly so commands read it without
    /// taking the async lock — locking it from a command panics the runtime.
    pub shared: crate::server::SharedState,
    pub zoom: std::sync::Mutex<crate::zoom::PersistedZoom>,
}

// ── Zoom ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ZoomInfo {
    pub scale: f64,
    pub index: usize,
    pub can_zoom_in: bool,
    pub can_zoom_out: bool,
    pub is_default: bool,
    pub label: String,
    pub reset_title: String,
}

impl From<&ZoomState> for ZoomInfo {
    fn from(z: &ZoomState) -> Self {
        Self {
            scale: z.scale(),
            index: z.index(),
            can_zoom_in: z.can_zoom_in(),
            can_zoom_out: z.can_zoom_out(),
            is_default: z.is_default(),
            label: z.label(),
            reset_title: z.reset_title(),
        }
    }
}

#[tauri::command]
pub fn get_zoom(state: State<'_, HostState>) -> ZoomInfo {
    let zoom = state.zoom.lock().expect("zoom lock");
    ZoomInfo::from(zoom.state())
}

#[tauri::command]
pub fn zoom_in(state: State<'_, HostState>) -> ZoomInfo {
    let mut zoom = state.zoom.lock().expect("zoom lock");
    zoom.state_mut().zoom_in();
    let _ = zoom.save(Some(&state.store));
    ZoomInfo::from(zoom.state())
}

#[tauri::command]
pub fn zoom_out(state: State<'_, HostState>) -> ZoomInfo {
    let mut zoom = state.zoom.lock().expect("zoom lock");
    zoom.state_mut().zoom_out();
    let _ = zoom.save(Some(&state.store));
    ZoomInfo::from(zoom.state())
}

#[tauri::command]
pub fn zoom_reset(state: State<'_, HostState>) -> ZoomInfo {
    let mut zoom = state.zoom.lock().expect("zoom lock");
    zoom.state_mut().reset();
    let _ = zoom.save(Some(&state.store));
    ZoomInfo::from(zoom.state())
}

// ── Frame ───────────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_frame() -> Frame {
    frame::load(&frame::default_file_path())
}

#[tauri::command]
pub fn set_frame(frame: Frame) -> CmdResult<Frame> {
    let clamped = frame.clamped();
    frame::save(&frame::default_file_path(), &clamped).map_err(|e| e.to_string())?;
    Ok(clamped)
}

// ── Integrations ───────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationJson {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub config: serde_json::Map<String, serde_json::Value>,
    pub environment: Option<String>,
    /// Per-tool enablement and settings, lifted out of the `query_policy` blob.
    pub tool_config: std::collections::BTreeMap<String, pluk_store::ToolPolicy>,
    pub token: String,
    pub created_at: String,
}

impl From<pluk_store::Integration> for IntegrationJson {
    fn from(i: pluk_store::Integration) -> Self {
        Self {
            id: i.id,
            name: i.name,
            r#type: i.r#type,
            config: i.config,
            environment: i.environment.map(|e| e.as_str().to_string()),
            tool_config: pluk_store::parse_query_policy(i.query_policy.as_deref())
                .map(|p| p.tools)
                .unwrap_or_default(),
            token: i.token,
            created_at: i.created_at,
        }
    }
}

#[tauri::command]
pub fn list_integrations(state: State<'_, HostState>) -> CmdResult<Vec<IntegrationJson>> {
    state
        .store
        .list_integrations()
        .map(|v| v.into_iter().map(IntegrationJson::from).collect())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_integration(state: State<'_, HostState>, id: String) -> CmdResult<Option<IntegrationJson>> {
    state
        .store
        .integration_by_id(&id)
        .map(|o| o.map(IntegrationJson::from))
        .map_err(|e| e.to_string())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateIntegrationPayload {
    pub name: String,
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(default)]
    pub config: serde_json::Map<String, serde_json::Value>,
    pub environment: Option<String>,
}

#[tauri::command]
pub fn create_integration(state: State<'_, HostState>, payload: CreateIntegrationPayload) -> CmdResult<IntegrationJson> {
    let mut input = pluk_store::IntegrationInput::new(payload.name, payload.r#type);
    input.config = payload.config;
    input.environment = payload.environment.as_deref().and_then(pluk_store::Environment::parse);
    state
        .store
        .create_integration(&input)
        .map(IntegrationJson::from)
        .map_err(|e| e.to_string())
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateIntegrationPayload {
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub r#type: Option<String>,
    pub config: Option<serde_json::Map<String, serde_json::Value>>,
    pub environment: Option<String>,
    /// Per-tool enablement; absent leaves the stored policy untouched.
    pub tool_config: Option<std::collections::BTreeMap<String, pluk_store::ToolPolicy>>,
}

#[tauri::command]
pub fn update_integration(
    state: State<'_, HostState>,
    id: String,
    payload: UpdateIntegrationPayload,
) -> CmdResult<Option<IntegrationJson>> {
    // Fold tool settings back into the policy blob, keeping the sibling keys
    // the other writers store there.
    let query_policy = match payload.tool_config {
        Some(tools) => {
            let stored = state.store.integration_by_id(&id).map_err(|e| e.to_string())?;
            let mut policy = stored
                .and_then(|i| pluk_store::parse_query_policy(i.query_policy.as_deref()))
                .unwrap_or_default();
            policy.tools = tools;
            Some(Some(pluk_store::serialize_query_policy(&policy)))
        }
        None => None,
    };
    let update = pluk_store::IntegrationUpdate {
        name: payload.name,
        r#type: payload.r#type,
        config: payload.config,
        environment: payload.environment.as_deref().and_then(pluk_store::Environment::parse),
        read_only: None,
        query_policy,
    };
    state
        .store
        .update_integration(&id, &update)
        .map(|o| o.map(IntegrationJson::from))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_integration(state: State<'_, HostState>, id: String) -> CmdResult<bool> {
    let did = state.store.delete_integration(&id).map_err(|e| e.to_string())?;
    if did {
        // Drop owner's pooled resources so stale creds/tunnels are gone.
        let owners = state.shared.owners.clone();
        owners.reset_owners(Some(&id));
    }
    Ok(did)
}

// ── Groups ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupJson {
    pub id: String,
    pub name: String,
    pub environment: Option<String>,
    pub members: Vec<pluk_store::GroupMember>,
    pub token: String,
    pub created_at: String,
}

impl From<pluk_store::Group> for GroupJson {
    fn from(g: pluk_store::Group) -> Self {
        Self {
            id: g.id,
            name: g.name,
            environment: g.environment.map(|e| e.as_str().to_string()),
            members: g.members,
            token: g.token,
            created_at: g.created_at,
        }
    }
}

#[tauri::command]
pub fn list_groups(state: State<'_, HostState>) -> CmdResult<Vec<GroupJson>> {
    state.store.list_groups().map(|v| v.into_iter().map(GroupJson::from).collect()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_group(state: State<'_, HostState>, id: String) -> CmdResult<Option<GroupJson>> {
    state.store.group_by_id(&id).map(|o| o.map(GroupJson::from)).map_err(|e| e.to_string())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateGroupPayload {
    pub name: String,
    pub environment: Option<String>,
    pub members: Vec<pluk_store::GroupMember>,
}

#[tauri::command]
pub fn create_group(state: State<'_, HostState>, payload: CreateGroupPayload) -> CmdResult<GroupJson> {
    let input = pluk_store::GroupInput {
        name: payload.name,
        environment: payload.environment.as_deref().and_then(pluk_store::Environment::parse),
        members: payload.members,
    };
    state.store.create_group(&input).map(GroupJson::from).map_err(|e| e.to_string())
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct UpdateGroupPayload {
    pub name: Option<String>,
    pub environment: Option<Option<String>>,
    pub members: Option<Vec<pluk_store::GroupMember>>,
}

#[tauri::command]
pub fn update_group(state: State<'_, HostState>, id: String, payload: UpdateGroupPayload) -> CmdResult<Option<GroupJson>> {
    let env = payload
        .environment
        .map(|inner| inner.as_deref().and_then(pluk_store::Environment::parse));
    let update = pluk_store::GroupUpdate { name: payload.name, environment: env, members: payload.members };
    let result = state.store.update_group(&id, &update).map(|o| o.map(GroupJson::from)).map_err(|e| e.to_string())?;
    if result.is_some() {
        let owners = state.shared.owners.clone();
        owners.reset_owners(Some(&id));
    }
    Ok(result)
}

#[tauri::command]
pub fn delete_group(state: State<'_, HostState>, id: String) -> CmdResult<bool> {
    let did = state.store.delete_group(&id).map_err(|e| e.to_string())?;
    if did {
        let owners = state.shared.owners.clone();
        owners.reset_owners(Some(&id));
    }
    Ok(did)
}

// ── Adapter catalog ────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterInfo {
    pub id: String,
    pub label: String,
    pub category: String,
    pub policy_kind: String,
    pub agent_hint: String,
    pub tools: Vec<pluk_adapters::ToolSpec>,
    pub config_fields: Vec<pluk_adapters::ConfigField>,
}

// Note: we expose via HTTP fallback too, but commands are the default for the host UI.
#[tauri::command]
pub fn list_adapters(state: State<'_, HostState>) -> Vec<AdapterInfo> {
    let registry = state.shared.registry.clone();
    registry
        .list()
        .iter()
        .map(|a| AdapterInfo {
            id: a.id().to_string(),
            label: a.label().to_string(),
            category: format!("{:?}", a.category()),
            policy_kind: format!("{:?}", a.policy_kind()),
            agent_hint: a.agent_hint().to_string(),
            tools: a.tool_specs().to_vec(),
            config_fields: a.config_fields().to_vec(),
        })
        .collect()
}

// ── Health ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct HealthEntry {
    pub status: String,
    pub error: Option<String>,
    pub at: i64,
}

#[tauri::command]
pub fn get_health(state: State<'_, HostState>) -> std::collections::BTreeMap<String, HealthEntry> {
    let map = state.shared.health.all();
    map.into_iter()
        .map(|(k, v)| {
            let status = match v.status {
                pluk_server::HealthStatus::Ok => "ok",
                pluk_server::HealthStatus::Error => "error",
            }
            .to_string();
            (k, HealthEntry { status, error: v.error, at: v.at })
        })
        .collect()
}

#[tauri::command]
pub async fn test_connection(state: State<'_, HostState>, id: String) -> CmdResult<serde_json::Value> {
    let store = state.store.clone();
    let registry = state.shared.registry.clone();
    let health = state.shared.health.clone();

    let integration = store.integration_by_id(&id).map_err(|e| e.to_string())?.ok_or_else(|| "Not found".to_string())?;
    let adapter = registry.get(&integration.r#type).ok_or_else(|| format!("No adapter for type: {}", integration.r#type))?;

    match adapter.test_connection(&integration).await {
        Ok(()) => {
            health.record(&integration.id, pluk_server::HealthStatus::Ok, None);
            Ok(serde_json::json!({ "ok": true }))
        }
        Err(e) => {
            let msg = adapter.humanize_error(&e).unwrap_or_else(|| e.message.clone());
            health.record(&integration.id, pluk_server::HealthStatus::Error, Some(msg.clone()));
            Ok(serde_json::json!({ "ok": false, "error": msg }))
        }
    }
}

// ── Logs ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPageJson {
    pub entries: Vec<LogEntryJson>,
    pub next_cursor: Option<CursorJson>,
    pub has_more: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntryJson {
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

impl From<pluk_store::LogEntry> for LogEntryJson {
    fn from(e: pluk_store::LogEntry) -> Self {
        Self {
            id: e.id,
            connection_id: e.connection_id,
            connection_name: e.connection_name,
            sql: e.sql,
            verdict: e.verdict,
            reason: e.reason,
            categories: e.categories,
            source: e.source,
            result_json: e.result_json,
            row_count: e.row_count,
            response_text: e.response_text,
            group_id: e.group_id,
            group_name: e.group_name,
            database: e.database,
            created_at: e.created_at,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorJson {
    pub created_at: String,
    pub id: i64,
}

#[tauri::command]
pub fn get_retention(state: State<'_, HostState>) -> CmdResult<i64> {
    state.store.retention_days().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_retention(state: State<'_, HostState>, days: i64) -> CmdResult<()> {
    state.store.set_retention_days(days).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_logs(state: State<'_, HostState>, scope: String, scope_id: String) -> CmdResult<usize> {
    let log_scope = if scope == "group" {
        pluk_store::LogScope::Group(scope_id)
    } else {
        pluk_store::LogScope::Connection(scope_id)
    };
    state.store.clear_logs(&log_scope).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_logs(
    state: State<'_, HostState>,
    scope: String,
    scope_id: String,
    range: Option<String>,
    cursor_time: Option<String>,
    cursor_id: Option<i64>,
) -> CmdResult<LogPageJson> {
    let log_scope = if scope == "group" {
        pluk_store::LogScope::Group(scope_id)
    } else {
        pluk_store::LogScope::Connection(scope_id)
    };
    let log_range = match range.as_deref() {
        Some("hour") => pluk_store::LogRange::Hour,
        Some("today") => pluk_store::LogRange::Today,
        Some("7d") => pluk_store::LogRange::SevenDays,
        Some("30d") => pluk_store::LogRange::ThirtyDays,
        _ => pluk_store::LogRange::All,
    };
    let cursor = match (cursor_time, cursor_id) {
        (Some(t), Some(id)) => Some(pluk_store::LogCursor { created_at: t, id }),
        _ => None,
    };
    let page = state
        .store
        .read_log_page(&log_scope, log_range, cursor.as_ref())
        .map_err(|e| e.to_string())?;
    Ok(LogPageJson {
        entries: page.entries.into_iter().map(LogEntryJson::from).collect(),
        next_cursor: page.next_cursor.map(|c| CursorJson { created_at: c.created_at, id: c.id }),
        has_more: page.has_more,
    })
}

#[tauri::command]
pub fn cancel_query(state: State<'_, HostState>, log_id: i64) -> bool {
    state.shared.cancels.cancel(log_id)
}

// ── MCP client config ──────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct InjectResultJson {
    pub status: String,
    pub path: String,
}

fn parse_mcp_client(raw: &str) -> Option<pluk_core::platform::McpClient> {
    match raw {
        "opencode" => Some(pluk_core::platform::McpClient::Opencode),
        "codex" => Some(pluk_core::platform::McpClient::Codex),
        "claudeCode" | "claude-code" | "claude_code" => Some(pluk_core::platform::McpClient::ClaudeCode),
        "cursor" => Some(pluk_core::platform::McpClient::Cursor),
        "windsurf" => Some(pluk_core::platform::McpClient::Windsurf),
        "antigravity" => Some(pluk_core::platform::McpClient::Antigravity),
        _ => None,
    }
}

#[tauri::command]
pub fn inject_mcp_config(
    client: String,
    scope: String,
    project_dir: Option<String>,
    key: String,
    url: String,
) -> CmdResult<InjectResultJson> {
    let mcp_client = parse_mcp_client(&client).ok_or_else(|| format!("Unknown client “{client}”. Choose a supported client and try again."))?;
    let config_scope = match scope.as_str() {
        "project" => {
            let dir = project_dir
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| "Choose a project folder and try again.".to_string())?;
            pluk_core::platform::ConfigScope::Project {
                root: std::path::PathBuf::from(dir),
            }
        }
        "global" => pluk_core::platform::ConfigScope::Global,
        _ => return Err("Unknown scope. Use Project or Global and try again.".to_string()),
    };
    if key.trim().is_empty() {
        return Err("Missing integration key. Try again.".to_string());
    }
    if url.trim().is_empty() {
        return Err("Missing endpoint URL. Try again.".to_string());
    }
    match pluk_core::mcp_config::inject(mcp_client, &config_scope, &key, &url) {
        Ok(pluk_core::mcp_config::InjectResult::Added { path }) => Ok(InjectResultJson {
            status: "added".to_string(),
            path: path.display().to_string(),
        }),
        Ok(pluk_core::mcp_config::InjectResult::Skipped { path }) => Ok(InjectResultJson {
            status: "skipped".to_string(),
            path: path.display().to_string(),
        }),
        Err(e) => Err(format!("{e} Check the file and try again, or copy the snippet manually.")),
    }
}

#[tauri::command]
pub fn list_installed_mcp_clients() -> Vec<String> {
    pluk_core::platform::McpClient::ALL
        .iter()
        .filter(|c| c.is_installed())
        .map(|c| match c {
            pluk_core::platform::McpClient::Opencode => "opencode".to_string(),
            pluk_core::platform::McpClient::Codex => "codex".to_string(),
            pluk_core::platform::McpClient::ClaudeCode => "claudeCode".to_string(),
            pluk_core::platform::McpClient::Cursor => "cursor".to_string(),
            pluk_core::platform::McpClient::Windsurf => "windsurf".to_string(),
            pluk_core::platform::McpClient::Antigravity => "antigravity".to_string(),
        })
        .collect()
}

// ── Reload ─────────────────────────────────────────────────────────────

#[tauri::command]
pub fn reload(state: State<'_, HostState>, owner_id: Option<String>) -> usize {
    let owners = state.shared.owners.clone();
    owners.reset_owners(owner_id.as_deref())
}

// ── Helpers for serialization tests ────────────────────────────────────

/// Verify STEPS serializes stably as JSON numbers.
#[cfg(test)]
pub fn steps_json() -> serde_json::Value {
    serde_json::json!(crate::zoom::STEPS)
}
