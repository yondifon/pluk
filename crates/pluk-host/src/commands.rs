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

pub struct HostState {
    pub store: std::sync::Arc<pluk_store::Store>,
    pub server: tokio::sync::Mutex<ServerHandle>,
    /// The server's shared state, held directly so commands read it without
    /// taking the async lock — locking it from a command panics the runtime.
    pub shared: crate::server::SharedState,
    pub zoom: std::sync::Mutex<crate::zoom::PersistedZoom>,
}

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

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationJson {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub r#type: String,
    /// The stored config without its secret values, which the window never
    /// reads back.
    pub config: serde_json::Map<String, serde_json::Value>,
    /// The secret fields that hold a saved value.
    pub secrets_set: Vec<String>,
    pub environment: Option<String>,
    /// Per-tool enablement and settings, lifted out of the `query_policy` blob.
    pub tool_config: std::collections::BTreeMap<String, pluk_store::ToolPolicy>,
    /// The rules deciding what runs without asking, and whether to ask at all.
    pub approvals: pluk_store::Approvals,
    /// Tools an imported config turned off that the server has not listed yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_tools_off: Vec<String>,
    pub token: String,
    pub created_at: String,
    /// This integration's own tool catalog, which for some adapters is only
    /// known once it has reached the service. Absent when no adapter matches
    /// the type, and the UI falls back to the per-type catalog.
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<pluk_adapters::ToolSpec>>,
}

impl IntegrationJson {
    /// The wire shape: secrets withheld, plus the tools this integration
    /// itself publishes. Every integration the window reads is built here.
    fn from_integration(
        i: pluk_store::Integration,
        registry: &pluk_adapters::AdapterRegistry,
        store: &pluk_store::Store,
    ) -> Self {
        let adapter = registry.get(&i.r#type);
        let tools = match adapter.as_deref() {
            Some(adapter) => Some(adapter.tool_specs_for(&i).into_owned()),
            None => {
                pluk_server::logging::log_info(&format!(
                    "skipping integration with unknown adapter kind: {} ({})",
                    i.r#type, i.id
                ));
                None
            }
        };
        let mut config = i.config;
        let secrets_set = adapter
            .as_deref()
            .map(|adapter| {
                let fields = adapter.config_fields();
                if pluk_adapters::key_value::has_secret_rows(fields) {
                    // A read that fails shows every secret row as not saved,
                    // which never shows more than is there.
                    let saved = store.list_proxy_secrets(&i.id).unwrap_or_default();
                    pluk_adapters::show_secret_rows(&mut config, fields, &saved);
                }
                pluk_adapters::withhold_secrets(&mut config, fields)
            })
            .unwrap_or_default();
        let policy = pluk_store::parse_query_policy(i.query_policy.as_deref());
        IntegrationJson {
            id: i.id,
            name: i.name,
            r#type: i.r#type,
            config,
            secrets_set,
            environment: i.environment.map(|e| e.as_str().to_string()),
            pending_tools_off: pluk_adapters::mcp_proxy::import::pending_off(policy.as_ref()),
            tool_config: policy.clone().map(|p| p.tools).unwrap_or_default(),
            approvals: policy.map(|p| p.approvals).unwrap_or_default(),
            token: i.token,
            created_at: i.created_at,
            tools,
        }
    }
}

/// What a save stores: the config with the secrets the window left untouched
/// folded back in and secret row values taken out, and the writes that save
/// those values.
struct PreparedConfig {
    config: pluk_store::Config,
    secrets: Vec<pluk_store::SecretWrite>,
}

/// Why a config cannot be saved: a problem the window shows at its field, or
/// a failure reading what is stored.
enum SaveRefused {
    Problem(pluk_adapters::ConfigProblem),
    Failed(String),
}

impl From<SaveRefused> for String {
    fn from(refused: SaveRefused) -> Self {
        match refused {
            SaveRefused::Problem(problem) => problem.message,
            SaveRefused::Failed(message) => message,
        }
    }
}

/// Check a config the window sent and fold it over what is saved. `source`
/// holds the secrets a blank value keeps: the integration being edited
/// (`editing`), or the one a new integration copies. An unknown type has no
/// fields to go by, so the config is taken as sent.
fn prepare_config(
    store: &pluk_store::Store,
    registry: &pluk_adapters::AdapterRegistry,
    r#type: &str,
    editing: Option<&str>,
    source: Option<&pluk_store::Integration>,
    sent: pluk_store::Config,
) -> Result<PreparedConfig, SaveRefused> {
    let Some(adapter) = registry.get(r#type) else {
        return Ok(PreparedConfig {
            config: sent,
            secrets: Vec::new(),
        });
    };
    let fields = adapter.config_fields();
    let stored = source.map(|i| i.config.clone()).unwrap_or_default();
    let mut config = pluk_adapters::keep_secrets(&stored, sent, fields);
    adapter
        .check_config(editing, &config)
        .map_err(SaveRefused::Problem)?;
    let saved = match source {
        Some(source) if pluk_adapters::key_value::has_secret_rows(fields) => store
            .list_proxy_secrets(&source.id)
            .map_err(|e| SaveRefused::Failed(e.to_string()))?,
        _ => Vec::new(),
    };
    let kept = match editing {
        Some(_) => pluk_adapters::KeptFrom::Itself(&saved),
        None => pluk_adapters::KeptFrom::Copy(&saved),
    };
    let secrets =
        pluk_adapters::fold_secret_rows(fields, &mut config, kept).map_err(SaveRefused::Problem)?;
    Ok(PreparedConfig { config, secrets })
}

#[tauri::command]
pub fn list_integrations(state: State<'_, HostState>) -> CmdResult<Vec<IntegrationJson>> {
    let registry = state.shared.registry.clone();
    state
        .store
        .list_integrations()
        .map(|v| {
            v.into_iter()
                .map(|i| IntegrationJson::from_integration(i, &registry, &state.store))
                .collect()
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_integration(
    state: State<'_, HostState>,
    id: String,
) -> CmdResult<Option<IntegrationJson>> {
    let registry = state.shared.registry.clone();
    state
        .store
        .integration_by_id(&id)
        .map(|o| o.map(|i| IntegrationJson::from_integration(i, &registry, &state.store)))
        .map_err(|e| e.to_string())
}

/// What an adapter's own API answered: the HTTP status it chose, and its
/// body. Anything the route itself reports, including a refusal, arrives here
/// rather than as an error, so the window can show the adapter's own wording.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiAnswer {
    pub status: u16,
    pub body: serde_json::Value,
}

/// The window's one way into an integration's own REST API, the routes its
/// adapter serves under `/api/integrations/<id>/…`. The loopback server is
/// closed to browser origins, so the request is resolved here instead.
#[tauri::command]
pub async fn integration_api(
    state: State<'_, HostState>,
    integration_id: String,
    method: String,
    subpath: String,
    body: Option<String>,
) -> CmdResult<ApiAnswer> {
    integration_api_request(
        &state.store,
        &state.shared.registry,
        integration_id,
        method,
        subpath,
        body,
    )
    .await
}

async fn integration_api_request(
    store: &pluk_store::Store,
    registry: &pluk_adapters::AdapterRegistry,
    integration_id: String,
    method: String,
    subpath: String,
    body: Option<String>,
) -> CmdResult<ApiAnswer> {
    let conn = store
        .integration_by_id(&integration_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Integration not found".to_string())?;
    let adapter = registry
        .get(&conn.r#type)
        .ok_or_else(|| "Adapter not found".to_string())?;
    let request = pluk_adapters::ApiRequest {
        method,
        url: format!("/api/integrations/{integration_id}{subpath}"),
        body,
    };
    let response = adapter
        .handle_api(&conn, request, &subpath)
        .await
        .ok_or_else(|| format!("No route for {subpath}"))?;
    Ok(ApiAnswer {
        status: response.status,
        body: serde_json::from_slice(&response.body).map_err(|e| e.to_string())?,
    })
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
    /// An integration whose saved secrets fill the ones this config leaves
    /// untouched, so a copy keeps secrets the window never saw.
    #[serde(default)]
    pub secrets_from: Option<String>,
}

#[tauri::command]
pub fn create_integration(
    state: State<'_, HostState>,
    payload: CreateIntegrationPayload,
) -> CmdResult<IntegrationJson> {
    create_integration_in(&state.store, &state.shared.registry, payload)
}

fn create_integration_in(
    store: &pluk_store::Store,
    registry: &pluk_adapters::AdapterRegistry,
    payload: CreateIntegrationPayload,
) -> CmdResult<IntegrationJson> {
    create_with_policy(store, registry, payload, None)
}

/// The one create path, with the policy blob the new integration starts with.
fn create_with_policy(
    store: &pluk_store::Store,
    registry: &pluk_adapters::AdapterRegistry,
    payload: CreateIntegrationPayload,
    query_policy: Option<String>,
) -> CmdResult<IntegrationJson> {
    let source = copy_source(store, payload.secrets_from.as_deref())?;
    let prepared = prepare_config(
        store,
        registry,
        &payload.r#type,
        None,
        source.as_ref(),
        payload.config,
    )?;
    let mut input = pluk_store::IntegrationInput::new(payload.name, payload.r#type);
    input.config = prepared.config;
    input.environment = payload
        .environment
        .as_deref()
        .and_then(pluk_store::Environment::parse);
    input.query_policy = query_policy;
    let created = store
        .create_integration(&input)
        .map_err(|e| e.to_string())?;
    store
        .write_proxy_secrets(&created.id, &prepared.secrets)
        .map_err(|e| e.to_string())?;
    Ok(IntegrationJson::from_integration(created, registry, store))
}

/// The integration a new one copies its saved secrets from.
fn copy_source(
    store: &pluk_store::Store,
    secrets_from: Option<&str>,
) -> CmdResult<Option<pluk_store::Integration>> {
    let Some(id) = secrets_from else {
        return Ok(None);
    };
    store
        .integration_by_id(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Integration not found".to_string())
        .map(Some)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckConfigPayload {
    #[serde(rename = "type")]
    pub r#type: String,
    /// The integration being edited; absent for a new one.
    #[serde(default)]
    pub id: Option<String>,
    /// For a new integration, the one whose saved secrets it copies.
    #[serde(default)]
    pub secrets_from: Option<String>,
    #[serde(default)]
    pub config: pluk_store::Config,
}

/// The first thing in a config that saving it would refuse, so the form can
/// show it beside the field and row that hold it.
#[tauri::command]
pub fn check_integration_config(
    state: State<'_, HostState>,
    payload: CheckConfigPayload,
) -> CmdResult<Option<pluk_adapters::ConfigProblem>> {
    check_integration_config_in(&state.store, &state.shared.registry, payload)
}

fn check_integration_config_in(
    store: &pluk_store::Store,
    registry: &pluk_adapters::AdapterRegistry,
    payload: CheckConfigPayload,
) -> CmdResult<Option<pluk_adapters::ConfigProblem>> {
    let source = match payload.id.as_deref() {
        Some(id) => store.integration_by_id(id).map_err(|e| e.to_string())?,
        None => copy_source(store, payload.secrets_from.as_deref())?,
    };
    let prepared = prepare_config(
        store,
        registry,
        &payload.r#type,
        payload.id.as_deref(),
        source.as_ref(),
        payload.config,
    );
    match prepared {
        Ok(_) => Ok(None),
        Err(SaveRefused::Problem(problem)) => Ok(Some(problem)),
        Err(SaveRefused::Failed(message)) => Err(message),
    }
}

/// The MCP servers a config copied from another client lists, for the window
/// to review before anything is saved.
#[tauri::command]
pub fn parse_mcp_config(
    text: String,
) -> Result<pluk_core::mcp_import::ParsedImport, pluk_core::mcp_import::ImportError> {
    pluk_core::mcp_import::parse(&text)
}

/// How saving one reviewed server went: the integration it became, or why
/// it was not added.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedServer {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integration: Option<IntegrationJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Add each reviewed server as its own MCP integration through the create
/// path the settings form uses. One that cannot be added leaves the others
/// saved. A local server is saved like any other and still waits for the
/// user to approve its command.
#[tauri::command]
pub fn import_mcp_servers(
    state: State<'_, HostState>,
    servers: Vec<pluk_core::mcp_import::ServerDraft>,
) -> CmdResult<Vec<ImportedServer>> {
    import_mcp_servers_in(&state.store, &state.shared.registry, servers)
}

fn import_mcp_servers_in(
    store: &pluk_store::Store,
    registry: &pluk_adapters::AdapterRegistry,
    servers: Vec<pluk_core::mcp_import::ServerDraft>,
) -> CmdResult<Vec<ImportedServer>> {
    use pluk_adapters::mcp_proxy::{ADAPTER_ID, import};

    let mut taken: Vec<String> = store
        .list_integrations()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|i| i.name.trim().to_lowercase())
        .collect();
    let mut outcomes = Vec::new();
    for server in servers {
        let name = server.name.trim().to_string();
        let result = if name.is_empty() {
            Err("Add a name for this server.".to_string())
        } else if taken.contains(&name.to_lowercase()) {
            Err(format!("{name} is already in Pluk. Choose another name."))
        } else {
            create_with_policy(
                store,
                registry,
                CreateIntegrationPayload {
                    name: name.clone(),
                    r#type: ADAPTER_ID.to_string(),
                    config: import::config_for(&server),
                    environment: None,
                    secrets_from: None,
                },
                import::policy_with_pending_off(&server.disabled_tools),
            )
        };
        let (integration, error) = match result {
            Ok(created) => {
                taken.push(name.to_lowercase());
                (Some(created), None)
            }
            Err(message) => (None, Some(message)),
        };
        outcomes.push(ImportedServer {
            name,
            integration,
            error,
        });
    }
    Ok(outcomes)
}

/// Read a field so an explicit `null` keeps its own meaning: an absent field
/// stays the outer `None` ("leave as stored"), `null` becomes `Some(None)`
/// ("clear it"). Plain `Option` collapses both to `None`.
fn nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer).map(Some)
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateIntegrationPayload {
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub r#type: Option<String>,
    /// Replaces the stored config, except that a secret field left out or
    /// empty keeps its stored value and one sent as `null` is removed. Key/value
    /// rows follow `pluk_adapters::key_value`: a secret row sent without a
    /// value keeps the one saved for it.
    pub config: Option<serde_json::Map<String, serde_json::Value>>,
    /// Absent leaves the stored environment; `null` clears it.
    #[serde(default, deserialize_with = "nullable")]
    pub environment: Option<Option<String>>,
    /// Per-tool enablement; absent leaves the stored policy untouched.
    pub tool_config: Option<std::collections::BTreeMap<String, pluk_store::ToolPolicy>>,
    /// Allow and deny rules; absent leaves the stored ones untouched.
    pub approvals: Option<pluk_store::Approvals>,
}

/// Fold tool settings and approval rules back into the policy blob, keeping the
/// sibling keys the other writers store there. Absent means "leave as stored";
/// the outer `None` leaves the whole column alone.
///
/// A rule that is not a pattern Pluk can match stops the save: stored, it would
/// sit in the list looking active while matching nothing.
fn merged_policy(
    stored: Option<&str>,
    tool_config: Option<std::collections::BTreeMap<String, pluk_store::ToolPolicy>>,
    approvals: Option<pluk_store::Approvals>,
) -> CmdResult<Option<Option<String>>> {
    if tool_config.is_none() && approvals.is_none() {
        return Ok(None);
    }
    let mut policy = pluk_store::parse_query_policy(stored).unwrap_or_default();
    if let Some(tools) = tool_config {
        policy.tools = tools;
    }
    if let Some(approvals) = approvals {
        approvals.validate().map_err(|problem| problem.message)?;
        policy.approvals = approvals;
    }
    Ok(Some(Some(pluk_store::serialize_query_policy(&policy))))
}

/// The first rule the Edit screen cannot save, so it can be shown beside the
/// list that holds it.
#[tauri::command]
pub fn check_approval_rules(approvals: pluk_store::Approvals) -> Option<pluk_policy::RuleProblem> {
    approvals.validate().err()
}

/// Async so the runtime is there to stop a local MCP server the edit
/// changed, gracefully and in the background.
#[tauri::command]
pub async fn update_integration(
    state: State<'_, HostState>,
    id: String,
    payload: UpdateIntegrationPayload,
) -> CmdResult<Option<IntegrationJson>> {
    update_integration_in(&state.store, &state.shared.registry, &id, payload)
}

fn update_integration_in(
    store: &pluk_store::Store,
    registry: &pluk_adapters::AdapterRegistry,
    id: &str,
    payload: UpdateIntegrationPayload,
) -> CmdResult<Option<IntegrationJson>> {
    let Some(stored) = store.integration_by_id(id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let query_policy = merged_policy(
        stored.query_policy.as_deref(),
        payload.tool_config,
        payload.approvals,
    )?;
    let r#type = payload.r#type.as_deref().unwrap_or(&stored.r#type);
    let reconnects = payload.config.is_some() || payload.r#type.is_some();
    let prepared = payload
        .config
        .map(|sent| prepare_config(store, registry, r#type, Some(id), Some(&stored), sent))
        .transpose()?;
    let (config, secrets) = match prepared {
        Some(prepared) => (Some(prepared.config), prepared.secrets),
        None => (None, Vec::new()),
    };
    let update = pluk_store::IntegrationUpdate {
        name: payload.name,
        r#type: payload.r#type,
        config,
        environment: payload
            .environment
            .map(|env| env.as_deref().and_then(pluk_store::Environment::parse)),
        read_only: None,
        query_policy,
    };
    let Some(updated) = store
        .update_integration(id, &update)
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    store
        .write_proxy_secrets(id, &secrets)
        .map_err(|e| e.to_string())?;
    if reconnects {
        // A local MCP server keeps running on the launch it started with, so
        // it is stopped now rather than on its next call.
        pluk_adapters::mcp_proxy::client::shutdown(id);
    }
    Ok(Some(IntegrationJson::from_integration(
        updated, registry, store,
    )))
}

/// Async for the same reason as [`update_integration`].
#[tauri::command]
pub async fn delete_integration(state: State<'_, HostState>, id: String) -> CmdResult<bool> {
    if let Some(browser) = state.shared.browser.as_deref() {
        browser
            .disconnect_integration(&id)
            .map_err(|error| error.message)?;
    }
    let did = delete_integration_in(&state.store, &id)?;
    if did {
        // Drop owner's pooled resources so stale creds/tunnels are gone.
        let owners = state.shared.owners.clone();
        owners.reset_owners(Some(&id));
    }
    Ok(did)
}

fn delete_integration_in(store: &pluk_store::Store, id: &str) -> CmdResult<bool> {
    let did = store.delete_integration(id).map_err(|e| e.to_string())?;
    if did {
        pluk_adapters::mcp_proxy::client::shutdown(id);
    }
    Ok(did)
}

/// The local MCP server integration `id`, or why it is not one.
fn local_mcp(store: &pluk_store::Store, id: &str) -> CmdResult<pluk_store::Integration> {
    let integration = store
        .integration_by_id(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Integration not found".to_string())?;
    if integration.r#type != pluk_adapters::mcp_proxy::ADAPTER_ID
        || !pluk_adapters::mcp_proxy::local::is_local(&integration)
    {
        return Err("This integration is not a local MCP server.".to_string());
    }
    Ok(integration)
}

/// The exact command a local MCP server would start with, for the user to
/// review before approving it. Secret values are never part of it.
#[tauri::command]
pub async fn mcp_launch_preview(
    state: State<'_, HostState>,
    id: String,
) -> CmdResult<pluk_adapters::mcp_proxy::local::LaunchPreview> {
    let integration = local_mcp(&state.store, &id)?;
    pluk_adapters::mcp_proxy::local::preview(&state.store, &integration)
        .await
        .map_err(|error| error.message)
}

/// Approve the launch the preview showed as `launch_hash`, so Pluk may start
/// it. A Tauri command and nothing else: an adapter route would let any
/// process on this machine approve a command.
#[tauri::command]
pub async fn approve_mcp_launch(
    state: State<'_, HostState>,
    id: String,
    launch_hash: String,
) -> CmdResult<()> {
    approve_mcp_launch_in(&state.store, &id, &launch_hash).await
}

async fn approve_mcp_launch_in(
    store: &pluk_store::Store,
    id: &str,
    launch_hash: &str,
) -> CmdResult<()> {
    let integration = local_mcp(store, id)?;
    pluk_adapters::mcp_proxy::local::approve(store, &integration, launch_hash)
        .await
        .map_err(|error| error.message)
}

#[tauri::command]
pub fn mcp_server_status(
    state: State<'_, HostState>,
    id: String,
) -> CmdResult<pluk_adapters::mcp_proxy::client::ServerStatus> {
    local_mcp(&state.store, &id)?;
    Ok(pluk_adapters::mcp_proxy::client::status(&id))
}

/// The last lines the local MCP server printed, secrets scrubbed.
#[tauri::command]
pub fn mcp_server_output(state: State<'_, HostState>, id: String) -> CmdResult<Vec<String>> {
    local_mcp(&state.store, &id)?;
    Ok(pluk_adapters::mcp_proxy::client::output(&id))
}

/// Stop the local MCP server and keep it stopped until it is restarted.
#[tauri::command]
pub async fn stop_mcp_server(state: State<'_, HostState>, id: String) -> CmdResult<()> {
    local_mcp(&state.store, &id)?;
    pluk_adapters::mcp_proxy::client::stop(&id).await;
    Ok(())
}

/// Start the local MCP server again, clearing a stop or the crash limit, and
/// refresh its tools.
#[tauri::command]
pub async fn restart_mcp_server(state: State<'_, HostState>, id: String) -> CmdResult<()> {
    let integration = local_mcp(&state.store, &id)?;
    let registry = state.shared.registry.clone();
    pluk_adapters::mcp_proxy::local::restart(&state.store, &integration)
        .await
        .map_err(|error| {
            registry
                .get(&integration.r#type)
                .and_then(|adapter| adapter.humanize_error(&error))
                .unwrap_or(error.message)
        })
}

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
    state
        .store
        .list_groups()
        .map(|v| v.into_iter().map(GroupJson::from).collect())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_group(state: State<'_, HostState>, id: String) -> CmdResult<Option<GroupJson>> {
    state
        .store
        .group_by_id(&id)
        .map(|o| o.map(GroupJson::from))
        .map_err(|e| e.to_string())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateGroupPayload {
    pub name: String,
    pub environment: Option<String>,
    pub members: Vec<pluk_store::GroupMember>,
}

#[tauri::command]
pub fn create_group(
    state: State<'_, HostState>,
    payload: CreateGroupPayload,
) -> CmdResult<GroupJson> {
    let input = pluk_store::GroupInput {
        name: payload.name,
        environment: payload
            .environment
            .as_deref()
            .and_then(pluk_store::Environment::parse),
        members: payload.members,
    };
    state
        .store
        .create_group(&input)
        .map(GroupJson::from)
        .map_err(|e| e.to_string())
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct UpdateGroupPayload {
    pub name: Option<String>,
    /// Absent leaves the stored environment; `null` clears it.
    #[serde(default, deserialize_with = "nullable")]
    pub environment: Option<Option<String>>,
    pub members: Option<Vec<pluk_store::GroupMember>>,
}

#[tauri::command]
pub fn update_group(
    state: State<'_, HostState>,
    id: String,
    payload: UpdateGroupPayload,
) -> CmdResult<Option<GroupJson>> {
    let env = payload
        .environment
        .map(|inner| inner.as_deref().and_then(pluk_store::Environment::parse));
    let update = pluk_store::GroupUpdate {
        name: payload.name,
        environment: env,
        members: payload.members,
    };
    let result = state
        .store
        .update_group(&id, &update)
        .map(|o| o.map(GroupJson::from))
        .map_err(|e| e.to_string())?;
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterInfo {
    pub id: String,
    pub label: String,
    pub category: String,
    pub policy_kind: String,
    pub agent_hint: String,
    pub runs_commands: bool,
    pub offered_for_setup: bool,
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
            category: a.category().to_string(),
            policy_kind: a.policy_kind().as_str().to_string(),
            agent_hint: a.agent_hint().to_string(),
            runs_commands: a.runs_commands(),
            offered_for_setup: registry.offered_for_setup(a.id()),
            tools: a.tool_specs().to_vec(),
            config_fields: a.config_fields().to_vec(),
        })
        .collect()
}

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
            (
                k,
                HealthEntry {
                    status,
                    error: v.error,
                    at: v.at,
                },
            )
        })
        .collect()
}

#[tauri::command]
pub async fn test_connection(
    state: State<'_, HostState>,
    id: String,
) -> CmdResult<serde_json::Value> {
    let store = state.store.clone();
    let registry = state.shared.registry.clone();
    let health = state.shared.health.clone();

    let integration = store
        .integration_by_id(&id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Not found".to_string())?;
    let adapter = registry
        .get(&integration.r#type)
        .ok_or_else(|| format!("No adapter for type: {}", integration.r#type))?;

    match adapter.test_connection(&integration).await {
        Ok(()) => {
            health.record(&integration.id, pluk_server::HealthStatus::Ok, None);
            Ok(serde_json::json!({ "ok": true }))
        }
        Err(e) => {
            let msg = adapter
                .humanize_error(&e)
                .unwrap_or_else(|| e.message.clone());
            health.record(
                &integration.id,
                pluk_server::HealthStatus::Error,
                Some(msg.clone()),
            );
            Ok(serde_json::json!({ "ok": false, "error": msg }))
        }
    }
}

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
    state
        .store
        .set_retention_days(days)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_logs(
    state: State<'_, HostState>,
    scope: String,
    scope_id: String,
) -> CmdResult<usize> {
    let log_scope = if scope == "group" {
        pluk_store::LogScope::Group(scope_id)
    } else {
        pluk_store::LogScope::Connection(scope_id)
    };
    state
        .store
        .clear_logs(&log_scope)
        .map_err(|e| e.to_string())
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
        next_cursor: page.next_cursor.map(|c| CursorJson {
            created_at: c.created_at,
            id: c.id,
        }),
        has_more: page.has_more,
    })
}

#[tauri::command]
pub fn cancel_query(state: State<'_, HostState>, log_id: i64) -> bool {
    state.shared.cancels.cancel(log_id)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InjectResultJson {
    pub status: String,
    pub path: String,
}

fn parse_mcp_client(raw: &str) -> Option<pluk_core::platform::McpClient> {
    match raw {
        "opencode" => Some(pluk_core::platform::McpClient::Opencode),
        "codex" => Some(pluk_core::platform::McpClient::Codex),
        "claudeCode" | "claude-code" | "claude_code" => {
            Some(pluk_core::platform::McpClient::ClaudeCode)
        }
        "cursor" => Some(pluk_core::platform::McpClient::Cursor),
        "windsurf" => Some(pluk_core::platform::McpClient::Windsurf),
        "antigravity" => Some(pluk_core::platform::McpClient::Antigravity),
        _ => None,
    }
}

/// Collapse the home directory back to `~` so the panel can show the file it
/// touched without a machine-specific prefix.
fn display_path(path: &std::path::Path) -> String {
    let full = path.display().to_string();
    match pluk_core::platform::home_dir() {
        Some(home) => {
            let home = home.display().to_string();
            match full.strip_prefix(&home) {
                Some(rest) => format!("~{rest}"),
                None => full,
            }
        }
        None => full,
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
    let mcp_client = parse_mcp_client(&client).ok_or_else(|| {
        format!("Unknown client “{client}”. Choose a supported client and try again.")
    })?;
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
            path: display_path(&path),
        }),
        Ok(pluk_core::mcp_config::InjectResult::Skipped { path }) => Ok(InjectResultJson {
            status: "skipped".to_string(),
            path: display_path(&path),
        }),
        Err(e) => Err(format!(
            "{e} Check the file and try again, or copy the snippet manually."
        )),
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

#[tauri::command]
pub fn reload(state: State<'_, HostState>, owner_id: Option<String>) -> usize {
    let owners = state.shared.owners.clone();
    owners.reset_owners(owner_id.as_deref())
}

/// Hand a web address to the browser the user already has open.
///
/// Only `http` and `https` get through. Everything else the system opener
/// accepts is a file, an application, or a scheme some other program claims,
/// and the window has no business reaching those through here.
#[tauri::command]
pub fn open_external(url: String) -> CmdResult<()> {
    tauri_plugin_opener::open_url(web_url(&url)?, None::<&str>).map_err(|e| e.to_string())
}

fn web_url(raw: &str) -> CmdResult<&str> {
    let url = raw.trim();
    let scheme = url.split_once("://").map_or("", |(scheme, _)| scheme);
    if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
        return Ok(url);
    }
    Err("Only web addresses can be opened.".to_string())
}

#[cfg(test)]
mod external_url_tests {
    use super::web_url;

    #[test]
    fn only_web_addresses_are_opened() {
        assert_eq!(
            web_url("https://example.com/a?b=c"),
            Ok("https://example.com/a?b=c")
        );
        assert_eq!(
            web_url("  http://127.0.0.1:4242/x  "),
            Ok("http://127.0.0.1:4242/x")
        );
        assert_eq!(web_url("HTTPS://example.com"), Ok("HTTPS://example.com"));
        for refused in [
            "file:///etc/passwd",
            "javascript:alert('http://x')",
            "ftp://files.example.com",
            "mailto:someone@example.com",
            "example.com",
            "",
        ] {
            assert!(web_url(refused).is_err(), "{refused}");
        }
    }
}

/// Verify STEPS serializes stably as JSON numbers.
#[cfg(test)]
pub fn steps_json() -> serde_json::Value {
    serde_json::json!(crate::zoom::STEPS)
}

/// Covers the command the Install button reaches, not a helper beneath it:
/// every case here calls `inject_mcp_config` with the same argument shape the
/// window sends, and asserts the file on disk afterwards.
#[cfg(test)]
mod inject_command_tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;
    use serde_json::Value;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const URL: &str = "http://localhost:4242/mcp/tok";

    fn read_json(path: &std::path::Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).expect("config written")).expect("valid json")
    }

    #[test]
    fn project_scope_writes_the_repo_file() {
        let repo = tempfile::tempdir().unwrap();
        let res = inject_mcp_config(
            "cursor".to_string(),
            "project".to_string(),
            Some(repo.path().display().to_string()),
            "marketing-db-production".to_string(),
            URL.to_string(),
        )
        .expect("install succeeds");

        assert_eq!(res.status, "added");
        let written = read_json(&repo.path().join(".cursor/mcp.json"));
        assert_eq!(
            written["mcpServers"]["marketing-db-production"],
            serde_json::json!({"command": "bunx", "args": ["mcp-remote", URL]})
        );
    }

    #[test]
    fn project_scope_keeps_servers_already_in_the_file() {
        let repo = tempfile::tempdir().unwrap();
        let path = repo.path().join("opencode.json");
        fs::write(&path, r#"{"theme":"dark","mcp":{"other":{"type":"local"}}}"#).unwrap();

        inject_mcp_config(
            "opencode".to_string(),
            "project".to_string(),
            Some(repo.path().display().to_string()),
            "my-db".to_string(),
            URL.to_string(),
        )
        .expect("install succeeds");

        let written = read_json(&path);
        assert_eq!(written["mcp"]["other"]["type"], "local");
        assert_eq!(written["theme"], "dark");
        assert_eq!(written["mcp"]["my-db"]["url"], URL);
    }

    #[test]
    fn global_scope_writes_the_user_file_and_reports_a_tilde_path() {
        let _lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let orig = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", home.path()) };

        let res = inject_mcp_config(
            "claudeCode".to_string(),
            "global".to_string(),
            None,
            "my-db".to_string(),
            URL.to_string(),
        )
        .expect("install succeeds");

        assert_eq!(res.path, "~/.mcp.json");
        let written = read_json(&home.path().join(".mcp.json"));
        assert_eq!(
            written["mcpServers"]["my-db"],
            serde_json::json!({"type": "http", "url": URL})
        );

        // A second install leaves the entry alone and says so.
        let again = inject_mcp_config(
            "claudeCode".to_string(),
            "global".to_string(),
            None,
            "my-db".to_string(),
            URL.to_string(),
        )
        .expect("second install succeeds");
        assert_eq!(again.status, "skipped");

        match orig {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn project_scope_without_a_folder_is_an_error_the_user_can_act_on() {
        let err = inject_mcp_config(
            "cursor".to_string(),
            "project".to_string(),
            None,
            "my-db".to_string(),
            URL.to_string(),
        )
        .unwrap_err();
        assert_eq!(err, "Choose a project folder and try again.");
    }

    #[test]
    fn a_config_that_cannot_be_parsed_is_reported_and_left_alone() {
        let repo = tempfile::tempdir().unwrap();
        let path = repo.path().join(".mcp.json");
        fs::write(&path, "{ not json").unwrap();

        let err = inject_mcp_config(
            "claudeCode".to_string(),
            "project".to_string(),
            Some(repo.path().display().to_string()),
            "my-db".to_string(),
            URL.to_string(),
        )
        .unwrap_err();

        assert!(err.contains("Couldn't parse the existing config"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
    }
}

/// The edit payload has to tell "the window left this out" from "the window
/// chose no environment"; a plain `Option` reads both as absent.
#[cfg(test)]
mod update_payload_tests {
    use super::*;

    fn environment(json: serde_json::Value) -> Option<Option<String>> {
        serde_json::from_value::<UpdateIntegrationPayload>(json)
            .expect("payload")
            .environment
    }

    fn group_environment(json: serde_json::Value) -> Option<Option<String>> {
        serde_json::from_value::<UpdateGroupPayload>(json)
            .expect("payload")
            .environment
    }

    #[test]
    fn an_absent_environment_is_not_a_cleared_one() {
        assert_eq!(environment(serde_json::json!({})), None);
        assert_eq!(
            environment(serde_json::json!({ "environment": null })),
            Some(None)
        );
        assert_eq!(
            environment(serde_json::json!({ "environment": "local" })),
            Some(Some("local".to_string()))
        );
    }

    #[test]
    fn a_group_can_go_back_to_spanning_every_environment() {
        assert_eq!(group_environment(serde_json::json!({})), None);
        assert_eq!(
            group_environment(serde_json::json!({ "environment": null })),
            Some(None)
        );
        assert_eq!(
            group_environment(serde_json::json!({ "environment": "local" })),
            Some(Some("local".to_string()))
        );
    }

    #[test]
    fn a_members_picked_tools_reach_the_store_unchanged() {
        let payload = serde_json::from_value::<UpdateGroupPayload>(serde_json::json!({
            "members": [
                { "id": "a", "tools": ["echo"] },
                { "id": "b" },
            ]
        }))
        .expect("payload");
        let members = payload.members.expect("members");
        assert_eq!(members[0].tools, Some(vec!["echo".to_string()]));
        assert_eq!(members[1].tools, None);
    }
}

#[cfg(test)]
mod approval_rule_tests {
    use super::*;
    use pluk_store::Approvals;

    fn rules(allow: &[&str], deny: &[&str]) -> Approvals {
        Approvals {
            ask: true,
            allow: allow.iter().map(|s| s.to_string()).collect(),
            deny: deny.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn valid_rules_are_folded_in_beside_the_tool_switches() {
        let stored = r#"{"tools":{"query":{"enabled":true}}}"#;
        let saved = merged_policy(Some(stored), None, Some(rules(&["git pull*"], &["rm *"])))
            .expect("saves")
            .expect("policy set")
            .expect("policy text");
        let policy = pluk_store::parse_query_policy(Some(&saved)).expect("parses");
        assert_eq!(policy.approvals.allow, vec!["git pull*".to_string()]);
        assert_eq!(policy.approvals.deny, vec!["rm *".to_string()]);
        assert!(policy.tools["query"].enabled, "tool switches survive");
    }

    #[test]
    fn a_rule_that_does_not_compile_stops_the_save() {
        let stored = r#"{"tools":{},"approvals":{"ask":true,"allow":[],"deny":["rm -rf *"]}}"#;
        let error = merged_policy(Some(stored), None, Some(rules(&[], &["rm [a-"])))
            .expect_err("refused");
        assert!(error.contains("Never allow"), "{error}");
        assert!(error.contains("rm [a-"), "{error}");
        assert!(error.contains("[ … ] group is not closed"), "{error}");
    }

    #[test]
    fn a_save_that_touches_neither_leaves_the_column_alone() {
        assert_eq!(merged_policy(Some("{}"), None, None).expect("saves"), None);
    }

    #[test]
    fn the_edit_screen_is_told_which_list_holds_the_bad_rule() {
        assert_eq!(check_approval_rules(rules(&["ok*"], &["also-ok*"])), None);
        let problem = check_approval_rules(rules(&["x[]y"], &[])).expect("a problem");
        assert_eq!(problem.list, pluk_policy::RuleList::Allow);
        assert!(problem.message.contains("Always allow"), "{}", problem.message);
    }
}

#[cfg(test)]
mod integration_api_tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn proxy_tools_round_trip_through_the_command() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(pluk_store::Store::open(&directory.path().join("pluk.db")).unwrap());
        let registry = Arc::new(
            pluk_adapters::default_registry(
                store.clone(),
                Arc::new(pluk_adapters::sql::SqlCancelRegistry::default()),
            )
            .unwrap(),
        );
        let integration = store
            .create_integration(&pluk_store::IntegrationInput::new("Proxy", "mcp"))
            .unwrap();
        let response = integration_api_request(
            &store,
            &registry,
            integration.id,
            "GET".to_string(),
            "/proxy/tools".to_string(),
            None,
        )
        .await
        .unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(response.body["ok"], true);
        assert!(response.body["tools"].is_array());
    }
}

#[cfg(test)]
mod secret_tests {
    use super::*;
    use std::sync::Arc;

    use axum::http::{HeaderMap, StatusCode, header};
    use axum::response::{IntoResponse, Response};
    use serde_json::{Value, json};

    const TOKEN: &str = "saved-upstream-token";

    struct World {
        _directory: tempfile::TempDir,
        store: Arc<pluk_store::Store>,
        registry: Arc<pluk_adapters::AdapterRegistry>,
    }

    fn world() -> World {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(pluk_store::Store::open(&directory.path().join("pluk.db")).unwrap());
        let registry = Arc::new(
            pluk_adapters::default_registry(
                store.clone(),
                Arc::new(pluk_adapters::sql::SqlCancelRegistry::default()),
            )
            .unwrap(),
        );
        World {
            _directory: directory,
            store,
            registry,
        }
    }

    fn config(value: Value) -> pluk_store::Config {
        match value {
            Value::Object(map) => map,
            _ => unreachable!(),
        }
    }

    fn create(world: &World, r#type: &str, config: pluk_store::Config) -> IntegrationJson {
        create_integration_in(
            &world.store,
            &world.registry,
            CreateIntegrationPayload {
                name: format!("{type} integration"),
                r#type: r#type.to_string(),
                config,
                environment: None,
                secrets_from: None,
            },
        )
        .unwrap()
    }

    /// An edit as the window sends it: the config it was given, back again.
    fn resave(
        world: &World,
        shown: &IntegrationJson,
        config: pluk_store::Config,
    ) -> IntegrationJson {
        let payload = UpdateIntegrationPayload {
            name: Some(format!("{} renamed", shown.name)),
            config: Some(config),
            ..Default::default()
        };
        update_integration_in(&world.store, &world.registry, &shown.id, payload)
            .unwrap()
            .unwrap()
    }

    fn stored(world: &World, id: &str) -> pluk_store::Config {
        world.store.integration_by_id(id).unwrap().unwrap().config
    }

    #[test]
    fn every_adapters_secrets_stay_out_of_the_window_and_survive_an_edit() {
        let world = world();
        let mut checked = 0;
        for adapter in world.registry.list() {
            let secrets: Vec<&str> = adapter
                .config_fields()
                .iter()
                .filter(|field| field.secret)
                .map(|field| field.key.as_str())
                .collect();
            if secrets.is_empty() {
                continue;
            }
            let mut saved = config(json!({"host": "db.internal"}));
            for key in &secrets {
                saved.insert(key.to_string(), json!(format!("{key}-value")));
            }
            let shown = create(&world, adapter.id(), saved.clone());
            for key in &secrets {
                assert!(!shown.config.contains_key(*key), "{} {key}", adapter.id());
            }
            let mut set = shown.secrets_set.clone();
            set.sort();
            let mut expected: Vec<String> = secrets.iter().map(|key| key.to_string()).collect();
            expected.sort();
            expected.dedup();
            assert_eq!(set, expected, "{}", adapter.id());

            let edited = resave(&world, &shown, shown.config.clone());
            assert_eq!(stored(&world, &shown.id), saved, "{}", adapter.id());
            assert_eq!(edited.secrets_set.len(), expected.len(), "{}", adapter.id());
            checked += 1;
        }
        assert!(checked >= 2, "only {checked} adapters declare secrets");
    }

    #[test]
    fn a_secret_can_be_cleared_or_replaced_from_the_window() {
        let world = world();
        let shown = create(
            &world,
            "mcp",
            config(json!({"url": "https://example.com/mcp", "token": TOKEN, "client_secret": "s"})),
        );

        let mut sent = shown.config.clone();
        sent.insert("token".into(), json!("replaced"));
        sent.insert("client_secret".into(), Value::Null);
        let edited = resave(&world, &shown, sent);

        assert_eq!(stored(&world, &shown.id)["token"], json!("replaced"));
        assert!(!stored(&world, &shown.id).contains_key("client_secret"));
        assert_eq!(edited.secrets_set, vec!["token".to_string()]);
    }

    #[test]
    fn a_copy_keeps_the_secrets_of_the_integration_it_copies() {
        let world = world();
        let shown = create(
            &world,
            "mcp",
            config(json!({"url": "https://example.com/mcp", "token": TOKEN})),
        );
        let copy = create_integration_in(
            &world.store,
            &world.registry,
            CreateIntegrationPayload {
                name: "copy".into(),
                r#type: shown.r#type.clone(),
                config: shown.config.clone(),
                environment: None,
                secrets_from: Some(shown.id.clone()),
            },
        )
        .unwrap();

        assert_eq!(stored(&world, &copy.id)["token"], json!(TOKEN));
        assert!(!copy.config.contains_key("token"));
    }

    #[test]
    fn a_pasted_config_saves_each_server_with_its_secrets_in_the_secret_store() {
        let world = world();
        create(
            &world,
            "mcp",
            config(json!({"url": "https://taken.example/mcp"})),
        );
        let parsed = parse_mcp_config(
            r#"{"mcpServers":{
              "sentry-selfhosted":{"command":"node","args":["/path/to/sentry-mcp/build/index.js"],
                "env":{"SENTRY_URL":"https://sentry.internal.domain","SENTRY_AUTH_TOKEN":"sntrys_secret","SENTRY_ORG_SLUG":"my-org"},
                "disabledTools":["create_sentry_issue_comment","update_sentry_issue_status"]},
              "datadog":{"type":"http","url":"https://mcp.datadoghq.com/mcp","headers":{"DD-API-KEY":"dd_secret"}},
              "clash":{"url":"https://other.example/mcp"}}}"#
                .to_string(),
        )
        .unwrap();
        let mut servers = parsed.servers;
        let clash = servers.iter_mut().find(|s| s.name == "clash").unwrap();
        clash.name = "MCP integration".into();

        let outcomes = import_mcp_servers_in(&world.store, &world.registry, servers).unwrap();
        let by_name = |name: &str| outcomes.iter().find(|o| o.name == name).unwrap();
        assert_eq!(
            by_name("MCP integration").error.as_deref(),
            Some("MCP integration is already in Pluk. Choose another name.")
        );

        let sentry = by_name("sentry-selfhosted").integration.as_ref().unwrap();
        let config = stored(&world, &sentry.id);
        assert_eq!(config["connection"], json!("local"));
        assert_eq!(config["command"], json!("node"));
        assert!(
            !serde_json::to_string(&config)
                .unwrap()
                .contains("sntrys_secret")
        );
        assert!(
            !serde_json::to_string(&sentry.config)
                .unwrap()
                .contains("sntrys_secret")
        );
        let saved = world.store.list_proxy_secrets(&sentry.id).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, "SENTRY_AUTH_TOKEN");
        assert_eq!(saved[0].value, "sntrys_secret");
        assert!(
            config["env"]
                .as_array()
                .unwrap()
                .contains(&json!({"name": "SENTRY_ORG_SLUG", "value": "my-org", "secret": false}))
        );
        assert_eq!(
            sentry.pending_tools_off,
            ["create_sentry_issue_comment", "update_sentry_issue_status"]
        );
        assert!(sentry.tool_config.is_empty());
        assert_eq!(world.store.approved_launch(&sentry.id).unwrap(), None);

        let datadog = by_name("datadog").integration.as_ref().unwrap();
        assert!(
            !serde_json::to_string(&stored(&world, &datadog.id))
                .unwrap()
                .contains("dd_secret")
        );
        let saved = world.store.list_proxy_secrets(&datadog.id).unwrap();
        assert_eq!(
            (saved[0].name.as_str(), saved[0].value.as_str()),
            ("DD-API-KEY", "dd_secret")
        );
    }

    /// Just enough of an MCP server to open a session and list one tool, and
    /// only for a caller holding the token.
    async fn guarded_mcp(headers: HeaderMap, body: axum::Json<Value>) -> Response {
        let bearer = format!("Bearer {TOKEN}");
        if headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            != Some(&bearer)
        {
            return (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, "Bearer")],
            )
                .into_response();
        }
        let Some(id) = body.get("id").cloned() else {
            return StatusCode::ACCEPTED.into_response();
        };
        let result = match body["method"].as_str() {
            Some("initialize") => json!({
                "protocolVersion": body["params"]["protocolVersion"],
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "guarded", "version": "1"},
            }),
            Some("tools/list") => json!({
                "tools": [{"name": "echo", "inputSchema": {"type": "object"}}],
            }),
            _ => json!({}),
        };
        axum::Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
    }

    /// The same server, answering only a caller holding the token in the
    /// `DD_API_KEY` header.
    async fn keyed_mcp(mut headers: HeaderMap, body: axum::Json<Value>) -> Response {
        match headers.remove("DD_API_KEY") {
            Some(key) if key == TOKEN => {
                headers.insert(
                    header::AUTHORIZATION,
                    format!("Bearer {TOKEN}").parse().unwrap(),
                );
                guarded_mcp(headers, body).await
            }
            _ => StatusCode::UNAUTHORIZED.into_response(),
        }
    }

    fn header_rows(rows: Value) -> pluk_store::Config {
        config(json!({"url": "https://example.com/mcp", "headers": rows}))
    }

    #[test]
    fn secret_header_values_never_reach_the_window() {
        let world = world();
        let shown = create(
            &world,
            "mcp",
            header_rows(json!([
                {"name": "DD_API_KEY", "value": TOKEN, "secret": true},
                {"name": "X-Org", "value": "acme", "secret": false},
            ])),
        );
        let listed = list_of(&world);
        let read = serde_json::to_string(&(&shown, &listed)).unwrap();

        assert!(!read.contains(TOKEN), "{read}");
        assert_eq!(
            shown.config["headers"],
            json!([
                {"name": "DD_API_KEY", "secret": true, "set": true},
                {"name": "X-Org", "value": "acme", "secret": false},
            ])
        );
        assert!(
            !serde_json::to_string(&stored(&world, &shown.id))
                .unwrap()
                .contains(TOKEN)
        );
        let saved = world.store.list_proxy_secrets(&shown.id).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].value, TOKEN);
    }

    fn list_of(world: &World) -> Vec<IntegrationJson> {
        world
            .store
            .list_integrations()
            .unwrap()
            .into_iter()
            .map(|i| IntegrationJson::from_integration(i, &world.registry, &world.store))
            .collect()
    }

    #[test]
    fn a_renamed_secret_header_keeps_its_value_and_a_removed_one_is_cleared() {
        let world = world();
        let shown = create(
            &world,
            "mcp",
            header_rows(json!([
                {"name": "DD_API_KEY", "value": "api-1"},
                {"name": "DD_APPLICATION_KEY", "value": "app-1"},
            ])),
        );

        let edited = resave(
            &world,
            &shown,
            header_rows(json!([{"name": "X-Api-Key", "savedName": "DD_API_KEY", "secret": true}])),
        );

        let saved = world.store.list_proxy_secrets(&shown.id).unwrap();
        let saved: Vec<(&str, &str)> = saved
            .iter()
            .map(|s| (s.name.as_str(), s.value.as_str()))
            .collect();
        assert_eq!(saved, [("X-Api-Key", "api-1")]);
        assert_eq!(
            edited.config["headers"],
            json!([{"name": "X-Api-Key", "secret": true, "set": true}])
        );
    }

    #[test]
    fn a_header_the_save_would_refuse_is_named_at_its_row() {
        let world = world();
        let check = |id: Option<String>, rows: Value| {
            check_integration_config_in(
                &world.store,
                &world.registry,
                CheckConfigPayload {
                    r#type: "mcp".into(),
                    id,
                    secrets_from: None,
                    config: header_rows(rows),
                },
            )
            .unwrap()
        };

        assert_eq!(
            check(None, json!([{"name": "X-Org", "value": "acme"}])),
            None
        );
        let problem = check(
            None,
            json!([{"name": "X-Org", "value": "acme"}, {"name": "Content-Type", "value": "text/plain"}]),
        )
        .expect("refused");
        assert_eq!((problem.field.as_str(), problem.row), ("headers", Some(1)));

        let shown = create(
            &world,
            "mcp",
            header_rows(json!([{"name": "X-Key", "value": "k"}])),
        );
        assert_eq!(
            check(
                Some(shown.id.clone()),
                json!([{"name": "X-Key", "savedName": "X-Key"}])
            ),
            None,
            "a saved value counts as given"
        );
        let missing =
            check(None, json!([{"name": "X-Key", "savedName": "X-Key"}])).expect("refused");
        assert_eq!(missing.message, "Add a value for X-Key.");

        let refused = create_integration_in(
            &world.store,
            &world.registry,
            CreateIntegrationPayload {
                name: "bad".into(),
                r#type: "mcp".into(),
                config: header_rows(json!([{"name": "Host", "value": "evil.example.com"}])),
                environment: None,
                secrets_from: None,
            },
        )
        .expect_err("refused");
        assert_eq!(refused, "Pluk cannot send Host. Remove this header.");
    }

    #[test]
    fn a_copy_keeps_the_secret_headers_of_the_integration_it_copies() {
        let world = world();
        let shown = create(
            &world,
            "mcp",
            header_rows(json!([{"name": "DD_API_KEY", "value": TOKEN}])),
        );
        let copy = create_integration_in(
            &world.store,
            &world.registry,
            CreateIntegrationPayload {
                name: "copy".into(),
                r#type: "mcp".into(),
                config: shown.config.clone(),
                environment: None,
                secrets_from: Some(shown.id.clone()),
            },
        )
        .unwrap();

        let saved = world.store.list_proxy_secrets(&copy.id).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(
            (saved[0].name.as_str(), saved[0].value.as_str()),
            ("DD_API_KEY", TOKEN)
        );
    }

    #[tokio::test]
    async fn an_mcp_integration_still_connects_after_an_edit_that_skips_its_secret_header() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let router = axum::Router::new().route("/mcp", axum::routing::post(keyed_mcp));
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let world = world();
        let shown = create(
            &world,
            "mcp",
            config(json!({"url": url, "headers": [{"name": "DD_API_KEY", "value": TOKEN}]})),
        );

        resave(&world, &shown, shown.config.clone());

        let integration = world.store.integration_by_id(&shown.id).unwrap().unwrap();
        let adapter = world.registry.get("mcp").unwrap();
        adapter
            .test_connection(&integration)
            .await
            .expect("connects with the kept header");
        pluk_adapters::mcp_proxy::client::shutdown(&shown.id);
    }

    #[tokio::test]
    async fn an_mcp_integration_still_connects_after_an_edit_that_skips_its_token() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let router = axum::Router::new().route("/mcp", axum::routing::post(guarded_mcp));
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let world = world();
        let shown = create(&world, "mcp", config(json!({"url": url, "token": TOKEN})));

        resave(&world, &shown, shown.config.clone());

        let integration = world.store.integration_by_id(&shown.id).unwrap().unwrap();
        let adapter = world.registry.get("mcp").unwrap();
        adapter
            .test_connection(&integration)
            .await
            .expect("connects with the kept token");
        pluk_adapters::mcp_proxy::client::shutdown(&shown.id);
    }
}

#[cfg(test)]
mod local_server_tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::{Value, json};

    /// A local MCP server in `/bin/sh` that records its pid under
    /// `$STUB_STATE`, answers `initialize`, and lists no tools.
    const STUB: &str = r#"
echo $$ > "$STUB_STATE/pid"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      version=$(printf '%s' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"stub","version":"1"}}}\n' "$id" "$version"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[]}}\n' "$id"
      ;;
  esac
done
"#;

    struct World {
        _directory: tempfile::TempDir,
        store: Arc<pluk_store::Store>,
        registry: Arc<pluk_adapters::AdapterRegistry>,
    }

    fn world() -> World {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(pluk_store::Store::open(&directory.path().join("pluk.db")).unwrap());
        let registry = Arc::new(
            pluk_adapters::default_registry(
                store.clone(),
                Arc::new(pluk_adapters::sql::SqlCancelRegistry::default()),
            )
            .unwrap(),
        );
        World {
            _directory: directory,
            store,
            registry,
        }
    }

    fn local_config(dir: &std::path::Path, extra_arg: Option<&str>) -> pluk_store::Config {
        let script = dir.join("server.sh");
        std::fs::write(&script, STUB).unwrap();
        let mut args = vec![script.to_string_lossy().into_owned()];
        args.extend(extra_arg.map(str::to_string));
        match json!({
            "connection": "local",
            "command": "/bin/sh",
            "args": args,
            "cwd": dir,
            "env": [{"name": "STUB_STATE", "value": dir, "secret": false}],
        }) {
            Value::Object(config) => config,
            _ => unreachable!(),
        }
    }

    /// Approve the launch as it stands, through the command, and start it.
    async fn approve_and_start(world: &World, id: &str, dir: &std::path::Path) -> u32 {
        let integration = world.store.integration_by_id(id).unwrap().unwrap();
        let shown = pluk_adapters::mcp_proxy::local::preview(&world.store, &integration)
            .await
            .unwrap();
        approve_mcp_launch_in(&world.store, id, &shown.launch_hash)
            .await
            .unwrap();
        let _ = std::fs::remove_file(dir.join("pid"));
        world
            .registry
            .get("mcp")
            .unwrap()
            .test_connection(&integration)
            .await
            .expect("starts once approved");
        std::fs::read_to_string(dir.join("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    async fn gone(pid: u32) -> bool {
        for _ in 0..200 {
            let alive = std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if !alive {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        false
    }

    #[tokio::test]
    async fn editing_or_deleting_a_local_server_stops_it() {
        let world = world();
        let dir = tempfile::tempdir().unwrap();
        let created = create_integration_in(
            &world.store,
            &world.registry,
            CreateIntegrationPayload {
                name: "Local".into(),
                r#type: "mcp".into(),
                config: local_config(dir.path(), None),
                environment: None,
                secrets_from: None,
            },
        )
        .unwrap();

        let first = approve_and_start(&world, &created.id, dir.path()).await;
        let edit = UpdateIntegrationPayload {
            config: Some(local_config(dir.path(), Some("--verbose"))),
            ..Default::default()
        };
        update_integration_in(&world.store, &world.registry, &created.id, edit).unwrap();
        assert!(gone(first).await, "the server outlived an edit");

        let second = approve_and_start(&world, &created.id, dir.path()).await;
        assert!(delete_integration_in(&world.store, &created.id).unwrap());
        assert!(gone(second).await, "the server outlived its integration");
    }

    #[tokio::test]
    async fn only_a_local_server_can_be_approved_or_controlled() {
        let world = world();
        let remote = world
            .store
            .create_integration(&pluk_store::IntegrationInput::new("Remote", "mcp"))
            .unwrap();
        let refused = approve_mcp_launch_in(&world.store, &remote.id, "hash")
            .await
            .expect_err("not local");
        assert_eq!(refused, "This integration is not a local MCP server.");
        assert!(world.store.approved_launch(&remote.id).unwrap().is_none());
    }
}
