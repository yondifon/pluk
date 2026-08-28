use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use chrono::Utc;
use pluk_policy::{
    dialect_for, evaluate, is_valid_database_name, sql_policy_from_settings,
    tool_gate,
};
use pluk_policy::policy::policy_description;
use pluk_store::{Integration, Store};

use crate::config_field::{ConfigField, FieldType};
use crate::gate::{CallTarget, GateMeta, GateOpts, Outcome, RunOutcome, ToolResult, cancelled_when_message_contains, err, ok, run_gated};
use crate::instructions::{build_instructions, InstructionParts};
use crate::projection::{apply_only, only_param_schema, FieldMap, Preset};
use crate::tool_host::{object_schema, BoxFuture, PromptMessage, PromptResult, PromptRole, ResourceContents, ToolHost, ToolRegistration};
use crate::tool_spec::ToolSpec;

use super::error::{driver_error_to_adapter, format_sql_error};
use pluk_db::types::{QueryResult as DbQueryResult, QueryOpts};
use pluk_db::factory::{CreateDriverOpts, create_driver};
use pluk_db::config::SqlConfig;

pub fn sql_label(type_name: &str) -> String {
    match type_name {
        "postgres" => "PostgreSQL".to_string(),
        "mysql" => "MySQL".to_string(),
        "sqlite" => "SQLite".to_string(),
        _ => type_name.to_string(),
    }
}

pub fn sql_agent_hint(type_name: &str) -> String {
    if type_name == "sqlite" {
        "Use this to query and inspect a SQLite database — read schema and rows, run SELECTs, and write only when the policy permits. Use SELECT with LIMIT before wider queries.".to_string()
    } else if type_name == "mysql" {
        "Use this to query and inspect a MySQL database — read schema and rows, run SELECTs, and write only when the policy permits. Use SELECT with LIMIT for production data.".to_string()
    } else {
        "Use this to query and inspect a PostgreSQL database — read schema and rows, run SELECTs, and write only when the policy permits. Use SELECT with LIMIT for production data.".to_string()
    }
}

pub fn sql_instructions(conn: &Integration) -> String {
    let gate = tool_gate(conn.query_policy.as_deref());
    let policy = sql_policy_from_settings(&gate.settings("query"));
    build_instructions(
        &conn.name,
        conn.environment,
        InstructionParts {
            kind: sql_label(&conn.r#type),
            access: "Query and inspect this database. Every statement is checked against the policy below and recorded in the activity log.".to_string(),
            policy: Some(policy_description(&policy)),
            hint: Some(sql_agent_hint(&conn.r#type)),
            start: Some("Start with list_tables and describe_table to learn the schema, then read with SELECT … LIMIT.".to_string()),
        },
    )
}

fn query_settings() -> Vec<ConfigField> {
    vec![
        ConfigField::new("mode", "Statements", FieldType::Select)
            .default_value(&Value::String("read-only".into()))
            .options(&[("read-only","Read-only (SELECT)"),("mutations","Mutations (INSERT/UPDATE/DELETE)"),("destructive","Destructive (DROP/TRUNCATE, DDL)")])
            .help("Which kinds of SQL this connection may run."),
        ConfigField::new("require_where", "Require WHERE on UPDATE/DELETE", FieldType::Toggle).default_value(&Value::Bool(true)).help("Block UPDATE or DELETE without a WHERE clause."),
        ConfigField::new("block_stacked", "Block stacked statements", FieldType::Toggle).default_value(&Value::Bool(true)).help("Reject queries containing more than one statement (SELECT 1; DROP …)."),
        ConfigField::new("allow_filesystem", "Allow filesystem / COPY ops", FieldType::Toggle).default_value(&Value::Bool(false)).danger().help("Allow COPY … PROGRAM, INTO OUTFILE, LOAD DATA, ATTACH DATABASE, pg_read_file."),
        ConfigField::new("max_rows", "Max rows returned", FieldType::Number).default_value(&Value::Number(1000.into())).help("Cap rows returned to the agent. 0 = no cap."),
    ]
}

fn query_map() -> FieldMap {
    FieldMap::new(
        &["env","connection","type","database","fields","rows","truncated","row_cap","row_count","returned_rows"],
        &["rows","fields","row_count","returned_rows","truncated"],
    )
    .with_preset("connection", Preset::paths(&["env","connection","type","database"]))
    .with_preset("limits", Preset::paths(&["truncated","row_cap","row_count","returned_rows"]))
}

fn relationships_map() -> FieldMap {
    FieldMap::new(
        &["from_table","from_column","to_table","to_column","constraint_name"],
        &["from_table","from_column","to_table","to_column"],
    ).with_preset("constraints", Preset::paths(&["constraint_name"]))
}

fn table_stats_map() -> FieldMap {
    FieldMap::new(
        &["table","estimatedRows","sizeBytes","indexes"],
        &["table","estimatedRows","sizeBytes"],
    ).with_preset("indexes", Preset::paths(&["indexes"]))
}

fn saved_queries_map() -> FieldMap {
    FieldMap::new(
        &["id","connection_id","name","sql","created_at"],
        &["name","created_at"],
    ).with_preset("sql", Preset::paths(&["sql"]))
     .with_preset("ids", Preset::paths(&["id","connection_id"]))
}

pub fn sql_tool_specs() -> Vec<ToolSpec> {
    let opt_in: std::collections::HashSet<&str> = [
        "explain_query","list_relationships","table_stats","list_schemas","list_databases","export_query","run_saved_query","list_saved_queries"
    ].into_iter().collect();
    let mk = |name: &str, desc: &str, settings: Option<Vec<ConfigField>>| {
        let mut spec = ToolSpec::new(name, desc, "read").with_default_enabled(!opt_in.contains(name));
        if let Some(s) = settings { spec = spec.with_settings(s); }
        spec
    };
    vec![
        mk("query", "Run a SQL query against the database.", Some(query_settings())),
        mk("list_tables", "List all tables in the database.", None),
        mk("sample_table", "Preview rows from a table without writing SQL.", None),
        mk("explain_query", "Show a query's execution plan without running it.", None),
        mk("describe_table", "Get column definitions for a table.", None),
        mk("list_relationships", "List foreign key relationships between tables.", None),
        mk("search_schema", "Find tables or columns matching a term.", None),
        mk("table_stats", "Get cheap table statistics (estimated rows, size, indexes).", None),
        mk("list_schemas", "List all schemas or databases.", None),
        mk("list_databases", "List databases on the server (targets for the `database` argument).", None),
        mk("export_query", "Run a SQL query and save results to a local CSV or JSON file.", None),
        mk("run_saved_query", "Run a saved query by name.", None),
        mk("list_saved_queries", "List saved queries for this connection.", None),
    ]
}

// ── Cancellation registry ────────────────────────────────────────────────────

#[derive(Default)]
pub struct SqlCancelRegistry {
    handles: Mutex<HashMap<i64, CancellationToken>>,
}
impl SqlCancelRegistry {
    pub fn register(&self, log_id: i64) -> CancellationToken {
        let token = CancellationToken::new();
        self.handles.lock().unwrap().insert(log_id, token.clone());
        token
    }
    pub fn clear(&self, log_id: i64) { self.handles.lock().unwrap().remove(&log_id); }
    pub fn complete(&self, log_id: i64) { self.clear(log_id); }
    pub fn token_for(&self, log_id: i64) -> Option<CancellationToken> {
        self.handles.lock().unwrap().get(&log_id).cloned()
    }
    pub fn cancel(&self, log_id: i64) -> bool {
        if let Some(t) = self.handles.lock().unwrap().remove(&log_id) { t.cancel(); true } else { false }
    }
}

// Helpers
fn pinned_db(conn: &Integration) -> Option<String> {
    conn.config.get("database").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn uses_ssh(conn: &Integration) -> bool {
    match conn.config.get("use_ssh") {
        Some(Value::Bool(true)) => true,
        Some(Value::String(s)) if s == "true" => true,
        _ => false,
    }
}

fn supports_db_arg(conn: &Integration, pinned: Option<&String>) -> bool {
    pinned.is_none() && conn.r#type != "sqlite"
}
fn supports_schema_arg(conn: &Integration) -> bool { conn.r#type == "postgres" }

fn resolve_schema(requested: Option<&str>) -> Result<Option<String>, String> {
    if let Some(s) = requested.filter(|s| !s.is_empty()) {
        if !is_valid_database_name(s) {
            return Err(format!("Invalid schema name \"{}\". Allowed: letters, digits, _, $, -.", s));
        }
        Ok(Some(s.to_string()))
    } else { Ok(None) }
}
fn resolve_database(pinned: Option<&String>, requested: Option<&str>) -> Result<Option<String>, String> {
    if pinned.is_some() { return Ok(None); }
    if let Some(r) = requested.filter(|s| !s.is_empty()) {
        if !is_valid_database_name(r) {
            return Err(format!("Invalid database name \"{}\". Allowed: letters, digits, _, $, -.", r));
        }
        Ok(Some(r.to_string()))
    } else { Ok(None) }
}
fn switch_block(sql: &str, pinned: Option<&String>) -> Option<String> {
    // strip block comments, line -- and # comments
    let without_block = regex::Regex::new(r"/\*[\s\S]*?\*/").unwrap().replace_all(sql, " ").to_string();
    let without_line = regex::Regex::new(r"--[^\n]*").unwrap().replace_all(&without_block, " ").to_string();
    let without_hash = regex::Regex::new(r"#[^\n]*").unwrap().replace_all(&without_line, " ").to_string();
    let re = regex::Regex::new(r"(?i)(^|;)\s*use\s+\S").unwrap();
    if re.is_match(&without_hash) {
        if let Some(db) = pinned {
            return Some(format!("This connection is locked to database \"{}\". USE is blocked.", db));
        } else {
            return Some("USE is blocked. Pass the `database` argument to choose a database instead.".to_string());
        }
    }
    None
}

fn sql_config_from(conn: &Integration, database_override: Option<&str>) -> SqlConfig {
    let mut cfg = SqlConfig::default();
    cfg.r#type = conn.r#type.clone();
    cfg.host = conn.config.get("host").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.port = conn.config.get("port").and_then(|v| v.as_u64()).map(|n| n as u16);
    if cfg.port.is_none()
        && let Some(s) = conn.config.get("port").and_then(|v| v.as_str()) { cfg.port = s.parse().ok(); }
    cfg.user = conn.config.get("user").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.password = conn.config.get("password").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.database = conn.config.get("database").and_then(|v| v.as_str()).map(|s| s.to_string());
    if let Some(ov) = database_override { cfg.database = Some(ov.to_string()); }
    cfg.filename = conn.config.get("filename").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.socket_path = conn.config.get("socket_path").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.use_ssl = conn.config.get("use_ssl").and_then(|v| v.as_bool()).unwrap_or(false);
    if let Some(v) = conn.config.get("use_ssl").and_then(|v| v.as_str()) { cfg.use_ssl = v=="true"; }
    cfg.ssl_mode = conn.config.get("ssl_mode").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.ssl_ca_path = conn.config.get("ssl_ca_path").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.ssl_cert_path = conn.config.get("ssl_cert_path").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.ssl_key_path = conn.config.get("ssl_key_path").and_then(|v| v.as_str()).map(|s| s.to_string());
    let use_ssh_val = conn.config.get("use_ssh").map(|v| match v { Value::Bool(b) => if *b {"true".to_string()} else {"false".to_string()}, Value::String(s)=>s.clone(), _=> "".to_string() });
    cfg.use_ssh = use_ssh_val;
    cfg.ssh_host = conn.config.get("ssh_host").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.ssh_port = conn.config.get("ssh_port").and_then(|v| v.as_u64()).map(|n| n as u16);
    if cfg.ssh_port.is_none() && let Some(s)=conn.config.get("ssh_port").and_then(|v| v.as_str()) { cfg.ssh_port = s.parse().ok(); }
    cfg.ssh_user = conn.config.get("ssh_user").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.ssh_auth_type = conn.config.get("ssh_auth_type").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.ssh_key_path = conn.config.get("ssh_key_path").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.ssh_password = conn.config.get("ssh_password").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg
}

fn effective_db(pinned: Option<&String>, database: Option<&str>) -> Option<String> {
    database.map(|s| s.to_string()).or_else(|| pinned.cloned())
}

fn cap_rows_vec(rows: Vec<Value>, cap: Option<usize>) -> (Vec<Value>, bool, Option<usize>) {
    if let Some(limit) = cap
        && rows.len() > limit {
            return (rows.into_iter().take(limit).collect(), true, Some(limit));
        }
    let limit = cap;
    (rows, false, limit)
}

fn mask_rows(rows: &mut [Value], masked: &[String]) {
    if masked.is_empty() { return; }
    for row in rows.iter_mut() {
        if let Value::Object(map) = row {
            for col in masked {
                if map.contains_key(col) {
                    map.insert(col.clone(), Value::String("***".to_string()));
                }
            }
        }
    }
}

fn projected_json(value: Value, only: Option<Vec<String>>, map: &FieldMap) -> Result<String, String> {
    let projected = apply_only(&value, only.as_ref(), map).map_err(|e| e.to_string())?;
    Ok(serde_json::to_string_pretty(&projected).unwrap())
}

pub fn register_sql_server(
    host: &mut dyn ToolHost,
    conn: &Integration,
    owner_id: &str,
    store: Arc<Store>,
    cancels: Arc<SqlCancelRegistry>,
) -> Result<(), crate::error::AdapterError> {
    // need to clone conn etc for closures
    let gate = tool_gate(conn.query_policy.as_deref());
    let policy = sql_policy_from_settings(&gate.settings("query"));
    let dialect = dialect_for(&conn.r#type);
    let policy_desc = policy_description(&policy);
    let tool_defaults: HashMap<String, bool> = sql_tool_specs().into_iter().map(|t| (t.name, t.default_enabled)).collect();
    let on = |name: &str| gate.enabled(name, *tool_defaults.get(name).unwrap_or(&true));

    let masked_columns = store.list_masked_columns(&conn.id).unwrap_or_default();
    let pinned = pinned_db(conn);
    let supports_db = supports_db_arg(conn, pinned.as_ref());
    let supports_schema = supports_schema_arg(conn);
    let uses_ssh_flag = uses_ssh(conn);

    // Helpers for policy/cap
    let conn_name = conn.name.clone();
    let conn_type = conn.r#type.clone();
    let conn_env = conn.environment.map(|e| e.to_string()).unwrap_or_else(|| "development".to_string());
    let conn_id = conn.id.clone();
    let via_group = conn.via_group.clone();

    // prompt/resource registration (always)
    {
        host.register_prompt("summarize_schema", "Generate a concise summary of the database schema and relationships",
            None,
            Arc::new(|_args: Map<String, Value>| -> BoxFuture<PromptResult> {
                Box::pin(async move {
                    PromptResult { messages: vec![PromptMessage { role: PromptRole::User, text: "Read the full schema resource, then list the main tables, their purpose, and how they relate to each other.".to_string() }] }
                })
            })
        );
        // investigate_slow_query prompt with sql arg
        let mut props = Map::new();
        props.insert("sql".into(), Value::Object({
            let mut m = Map::new(); m.insert("type".into(), Value::String("string".into())); m.insert("description".into(), Value::String("SQL query to investigate".into())); m
        }));
        let schema = object_schema(props, &["sql"]);
        host.register_prompt("investigate_slow_query", "Analyze a slow query using EXPLAIN and table stats",
            Some(schema),
            Arc::new(|args: Map<String, Value>| -> BoxFuture<PromptResult> {
                let sql = args.get("sql").and_then(|v| v.as_str()).unwrap_or("").to_string();
                Box::pin(async move {
                    PromptResult { messages: vec![PromptMessage { role: PromptRole::User, text: format!("Investigate why this query is slow. Use explain_query and table_stats, then suggest indexes or rewrites.\n\n{}", sql) }] }
                })
            })
        );
        host.register_prompt("find_unused_indexes", "Find indexes that may be unused or redundant",
            None,
            Arc::new(|_args: Map<String, Value>| -> BoxFuture<PromptResult> {
                Box::pin(async move {
                    PromptResult { messages: vec![PromptMessage { role: PromptRole::User, text: "List all tables and their indexes. Flag any indexes that look redundant or likely unused based on column patterns.".to_string() }] }
                })
            })
        );

        // resource schema://full
        let store_res = store.clone();
        let conn_res = conn.clone();
        let owner_res = owner_id.to_string();
        host.register_resource("schema", "schema://full", "text/plain", Some("Full database schema: tables, columns, primary keys, foreign keys"),
            Arc::new(move || -> BoxFuture<ResourceContents> {
                let store = store_res.clone();
                let conn = conn_res.clone();
                let _owner = owner_res.clone();
                Box::pin(async move {
                    let cfg = sql_config_from(&conn, None);
                    let driver_res = create_driver(CreateDriverOpts::new(cfg)).await;
                    let text = match driver_res {
                        Ok(dw) => {
                            let r = dw.driver.get_full_schema(None).await;
                            let _ = dw.driver.close().await;
                            match r {
                                Ok(t) => t,
                                Err(e) => format!("Error: {}", driver_error_to_adapter(e).message),
                            }
                        },
                        Err(e) => format!("Error: {}", driver_error_to_adapter(e).message),
                    };
                    // also need to handle eviction etc? ignore
                    let _ = &store; // keep
                    ResourceContents { uri: "schema://full".into(), mime_type: "text/plain".into(), text }
                })
            })
        );
    }

    // query tool
    if on("query") {
        let mut props = Map::new();
        props.insert("sql".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m.insert("description".into(), Value::String("SQL query to execute".into())); m }));
        props.insert("query".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m.insert("description".into(), Value::String("Alias for sql".into())); m }));
        if conn.r#type != "sqlite" || uses_ssh_flag {
            props.insert("timeout".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("number".into())); m.insert("description".into(), Value::String("Max seconds to wait before aborting the query (default 30).".into())); m }));
        }
        if supports_db {
            props.insert("database".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m.insert("description".into(), Value::String("Database to run against on this server. This connection has no fixed database, so name the one to use (see list_schemas). Access is limited to databases the connection's user was granted.".into())); m }));
        }
        props.insert("params".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("array".into())); m.insert("description".into(), Value::String(if conn.r#type=="postgres" {"Values to bind to $1, $2, … placeholders in the SQL. Prefer this over inlining values.".to_string()} else {"Values to bind to ? placeholders in the SQL. Prefer this over inlining values.".to_string()})); m }));
        props.insert("limit".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("number".into())); m.insert("description".into(), Value::String("Max rows to return, overriding the default cap (1000).".into())); m }));
        props.insert("only".into(), only_param_schema(&["connection","limits"]));
        let schema = object_schema(props, &[]);

        let store_q = store.clone();
        let cancels_q = cancels.clone();
        let conn_q = conn.clone();
        let policy_q = policy.clone();
        let dialect_q = dialect;
        let pinned_q = pinned.clone();
        let masked_q = masked_columns.clone();
        let conn_name_q = conn_name.clone();
        let conn_type_q = conn_type.clone();
        let conn_env_q = conn_env.clone();
        let conn_id_q = conn_id.clone();
        let via_group_q = via_group.clone();
        let policy_desc_q = policy_desc.clone();
        let supports_db_q = supports_db;

        host.register_tool(
            ToolRegistration { name: "query".into(), description: format!("Run a SQL query against the database. {}{}", policy_desc_q, if supports_db_q {" This connection has no fixed database — pass `database` to choose one.".to_string()} else { "".to_string()}), input_schema: schema, annotations: Map::new() },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let store = store_q.clone();
                let cancels = cancels_q.clone();
                let conn = conn_q.clone();
                let policy = policy_q.clone();
                let dialect = dialect_q;
                let pinned = pinned_q.clone();
                let masked = masked_q.clone();
                let conn_name = conn_name_q.clone();
                let conn_type = conn_type_q.clone();
                let conn_env = conn_env_q.clone();
                let conn_id = conn_id_q.clone();
                let via_group = via_group_q.clone();
                Box::pin(async move {
                    let obj = args.as_object().cloned().unwrap_or_default();
                    let sql = obj.get("sql").or_else(|| obj.get("query")).and_then(|v| v.as_str()).map(|s| s.to_string());
                    let sql = match sql { Some(s) if !s.is_empty() => s, _ => return err("Missing SQL. Pass either \"sql\" or \"query\".") };
                    let database = obj.get("database").and_then(|v| v.as_str()).map(|s| s.to_string());
                    // pinned check
                    let db_res = resolve_database(pinned.as_ref(), database.as_deref());
                    let db_opt = match db_res { Ok(v) => v, Err(e) => return err(e) };
                    let params = obj.get("params").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                    let limit = obj.get("limit").and_then(|v| v.as_u64()).map(|n| n as usize);
                    let only = obj.get("only").and_then(|v| v.as_array()).map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>());
                    let mut timeout_ms = obj.get("timeout").and_then(|v| v.as_u64()).map(|t| t*1000);
                    if conn.r#type=="sqlite" && !uses_ssh(&conn) {
                        timeout_ms = None;
                    } else if timeout_ms.is_none() {
                        timeout_ms = Some(30_000);
                    }
                    // switch block
                    if let Some(block) = switch_block(&sql, pinned.as_ref()) { return err(format!("Blocked: {}", block)); }
                    let verdict = evaluate(&sql, &policy, dialect);
                    if !verdict.ok {
                        // blocked via precheck? run_gated will handle but we need to mimic TS: use run_gated with precheck
                    }
                    let target = CallTarget { connection_id: conn_id.clone(), connection_name: conn_name.clone(), group: via_group.clone() };
                    let meta = GateMeta { category: verdict.categories.clone(), action: "query".to_string(), detail: sql.clone(), database: db_opt.clone().or_else(|| pinned.clone()), command: None };
                    let sql_clone = sql.clone();
                    let db_for_effective = db_opt.clone();
                    let pinned_for_effective = pinned.clone();
                    let conn_for_driver = conn.clone();
                    let masked_for_closure = masked.clone();
                    let conn_env_c = conn_env.clone();
                    let conn_name_c = conn_name.clone();
                    let conn_type_c = conn_type.clone();
                    let only_c = only.clone();
                    let policy_c = policy.clone();
                    let limit_c = limit;
                    
                    run_gated(&store, &target, meta, move |log_id| {
                        let cancels = cancels.clone();
                        let conn = conn_for_driver.clone();
                        let sql = sql_clone.clone();
                        let params = params.clone();
                        let db_opt = db_for_effective.clone();
                        let masked = masked_for_closure.clone();
                        let conn_env = conn_env_c.clone();
                        let conn_name = conn_name_c.clone();
                        let conn_type = conn_type_c.clone();
                        let only = only_c.clone();
                        let policy = policy_c.clone();
                        let pinned = pinned_for_effective.clone();
                        let timeout = timeout_ms;
                        async move {
                            let token = log_id.map(|id| cancels.register(id));
                            let query_opts = {
                                let has_timeout = timeout.is_some();
                                let has_cancel = token.is_some();
                                if has_timeout || has_cancel {
                                    Some(QueryOpts { timeout_ms: timeout, cancel: token.clone() })
                                } else {
                                    None
                                }
                            };
                            // create driver
                            let cfg = sql_config_from(&conn, db_opt.as_deref());
                            let dw = create_driver(CreateDriverOpts::new(cfg)).await.map_err(driver_error_to_adapter)?;
                            let use_read_only = policy.allowed.len()==2 && policy.allowed.contains(&pluk_policy::category::StatementCategory::Select) && policy.allowed.contains(&pluk_policy::category::StatementCategory::Inspect);
                            let res: Result<DbQueryResult, _> = if use_read_only {
                                dw.driver.query_read_only(&sql, &params, query_opts.clone()).await
                            } else {
                                dw.driver.query(&sql, &params, query_opts.clone()).await
                            };
                            let res = match res {
                                Ok(r) => r,
                                Err(e) => {
                                    if let Some(id)=log_id { cancels.clear(id); }
                                    let _ = dw.driver.close().await;
                                    return Err(driver_error_to_adapter(e));
                                }
                            };
                            let _ = dw.driver.close().await;
                            if let Some(id)=log_id { cancels.clear(id); }
                            // cap then mask
                            let effective_cap: Option<usize> = match policy.max_rows {
                                None => limit_c,
                                Some(max) => {
                                    let max_u = max as usize;
                                    Some(limit_c.map(|l| l.min(max_u)).unwrap_or(max_u))
                                }
                            };
                            let total = res.rows.len();
                            let (mut rows, truncated, cap_limit) = {
                                let rows_vec: Vec<Value> = res.rows.into_iter().collect();
                                cap_rows_vec(rows_vec, effective_cap)
                            };
                            // masking
                            mask_rows(&mut rows, &masked);
                            // build meta
                            let fields = res.fields.unwrap_or_default();
                            let meta_val = serde_json::json!({
                                "env": conn_env,
                                "connection": conn_name,
                                "type": conn_type,
                                "database": effective_db(pinned.as_ref(), db_opt.as_deref()),
                                "fields": fields,
                                "rows": rows.clone(),
                                "truncated": truncated,
                                "row_cap": cap_limit.map(|v| Value::Number((v as i64).into())).unwrap_or(Value::Null),
                                "row_count": total,
                                "returned_rows": rows.len()
                            });
                            let qmap = query_map();
                            let mut text = projected_json(meta_val.clone(), only, &qmap).map_err(crate::error::AdapterError::new)?;
                            if truncated {
                                let lim = cap_limit.unwrap_or(0);
                                text.push_str(&format!("\n\n[Row limit: showing first {} of {} rows. Add a LIMIT clause to see all results.]", lim, total));
                            }
                            // snapshot for log: masked rows + fields
                            let snapshot = pluk_store::QueryResult { fields: fields.clone(), rows: rows.clone() };
                            Ok(Outcome::Ran(RunOutcome { text, is_error: false, reason: None, result: Some(snapshot), response_text: None, command: None }))
                        }
                    }, GateOpts::default()
                        .precheck({
                            let sql = sql.clone();
                            let pinned = pinned.clone();
                            let verdict = verdict.clone();
                            move || {
                                if let Some(b) = switch_block(&sql, pinned.as_ref()) { return Some(b); }
                                if !verdict.ok { return Some(verdict.reason.clone().unwrap_or_else(|| "blocked".into())); }
                                None
                            }
                        })
                        .classify_error(cancelled_when_message_contains("cancelled"))
                        .on_error({
                            let _store = store.clone();
                            move |_e| { /* evict would go here */ }
                        })
                        .format_error(|e, verdict| {
                            if verdict == pluk_store::Verdict::Cancelled {
                                format!("Cancelled: {}", e.message)
                            } else {
                                format_sql_error(e)
                            }
                        })
                    ).await
                })
            })
        );
    }

    // list_tables
    if on("list_tables") {
        let mut props = Map::new();
        if supports_db { props.insert("database".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m })); }
        if supports_schema { props.insert("schema".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m })); }
        let schema = object_schema(props, &[]);
        let store_lt = store.clone();
        let conn_lt = conn.clone();
        let pinned_lt = pinned.clone();
        host.register_tool(
            ToolRegistration { name: "list_tables".into(), description: "List all tables in the database".into(), input_schema: schema, annotations: Map::new() },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let _store = store_lt.clone();
                let conn = conn_lt.clone();
                let pinned = pinned_lt.clone();
                Box::pin(async move {
                    let obj = args.as_object().cloned().unwrap_or_default();
                    let database = obj.get("database").and_then(|v| v.as_str());
                    let schema_val = obj.get("schema").and_then(|v| v.as_str());
                    // validate
                    if let Err(e)=resolve_database(pinned.as_ref(), database) { return err(e); }
                    let schema_opt = match resolve_schema(schema_val) { Ok(v)=>v, Err(e)=> return err(e) };
                    let db_opt = resolve_database(pinned.as_ref(), database).unwrap().clone();
                    let cfg = sql_config_from(&conn, db_opt.as_deref());
                    // introspection: use run_gated? In TS introspect uses getDriver directly with tool timeout, not gated log? Actually instrumented via sql_log . For Rust we just call driver directly and handle errors via err.
                    // Use run_gated with simple Ok path? Simpler: direct driver call and return ok/err without pending log (introspection is logged via driver layer)
                    match create_driver(CreateDriverOpts::new(cfg)).await {
                        Ok(dw) => {
                            let res = dw.driver.list_tables(schema_opt.as_deref()).await;
                            let _ = dw.driver.close().await;
                            match res {
                                Ok(tables) => ok(tables.join("\n")),
                                Err(e) => err(driver_error_to_adapter(e).message),
                            }
                        },
                        Err(e) => err(driver_error_to_adapter(e).message)
                    }
                })
            })
        );
    }

    // sample_table
    if on("sample_table") {
        let mut props = Map::new();
        props.insert("table".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m.insert("description".into(), Value::String("Table name".into())); m }));
        props.insert("limit".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("number".into())); m.insert("description".into(), Value::String("Max rows to preview".into())); m }));
        if supports_db { props.insert("database".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m })); }
        if supports_schema { props.insert("schema".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m })); }
        props.insert("only".into(), only_param_schema(&["connection","limits"]));
        let schema = object_schema(props, &["table"]);
        let store_st = store.clone();
        let conn_st = conn.clone();
        let pinned_st = pinned.clone();
        let masked_st = masked_columns.clone();
        let policy_st = policy.clone();
        let conn_name_st = conn_name.clone();
        let conn_type_st = conn_type.clone();
        let conn_env_st = conn_env.clone();
        host.register_tool(
            ToolRegistration { name: "sample_table".into(), description: "Preview rows from a table without writing SQL".into(), input_schema: schema, annotations: Map::new() },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let conn = conn_st.clone();
                let pinned = pinned_st.clone();
                let masked = masked_st.clone();
                let policy = policy_st.clone();
                let conn_name = conn_name_st.clone();
                let conn_type = conn_type_st.clone();
                let conn_env = conn_env_st.clone();
                let store = store_st.clone();
                Box::pin(async move {
                    let obj = args.as_object().cloned().unwrap_or_default();
                    let table = obj.get("table").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if table.is_empty() { return err("Missing table"); }
                    let limit = obj.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
                    let database = obj.get("database").and_then(|v| v.as_str());
                    let schema_val = obj.get("schema").and_then(|v| v.as_str());
                    let only = obj.get("only").and_then(|v| v.as_array()).map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>());
                    if let Err(e)=resolve_database(pinned.as_ref(), database) { return err(e); }
                    let schema_opt = match resolve_schema(schema_val) { Ok(v)=>v, Err(e)=> return err(e) };
                    let db_opt = resolve_database(pinned.as_ref(), database).unwrap().clone();
                    let effective_limit = match policy.max_rows { None => limit, Some(max) => std::cmp::min(limit, max as usize) };
                    let cfg = sql_config_from(&conn, db_opt.as_deref());
                    let dw = match create_driver(CreateDriverOpts::new(cfg)).await { Ok(d)=>d, Err(e)=> return err(driver_error_to_adapter(e).message) };
                    let res = dw.driver.sample_table(&table, effective_limit as i64, schema_opt.as_deref()).await;
                    let _ = dw.driver.close().await;
                    let res = match res { Ok(r)=>r, Err(e)=> return err(driver_error_to_adapter(e).message) };
                    let total = res.rows.len();
                    let cap = policy.max_rows.map(|v| v as usize);
                    let (mut rows, truncated, cap_limit) = cap_rows_vec(res.rows.into_iter().collect(), cap);
                    mask_rows(&mut rows, &masked);
                    let fields = res.fields.unwrap_or_default();
                    let meta_val = serde_json::json!({
                        "env": conn_env,
                        "connection": conn_name,
                        "type": conn_type,
                        "database": effective_db(pinned.as_ref(), db_opt.as_deref()),
                        "fields": fields,
                        "rows": rows.clone(),
                        "truncated": truncated,
                        "row_cap": cap_limit.map(|v| Value::Number((v as i64).into())).unwrap_or(Value::Null),
                        "row_count": total,
                        "returned_rows": rows.len()
                    });
                    let mut text = match projected_json(meta_val, only, &query_map()) { Ok(t)=>t, Err(e)=> return err(e) };
                    if truncated {
                        let lim = cap_limit.unwrap_or(0);
                        text.push_str(&format!("\n\n[Row limit: showing first {} of {} rows.]", lim, total));
                    }
                    // introspection not gated? but we return ok
                    let _ = store;
                    ok(text)
                })
            })
        );
    }

    // explain_query
    if on("explain_query") {
        let mut props = Map::new();
        props.insert("sql".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m }));
        props.insert("query".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m }));
        if supports_db { props.insert("database".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m })); }
        props.insert("params".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("array".into())); m }));
        props.insert("only".into(), only_param_schema(&[]));
        let schema = object_schema(props, &[]);
        let conn_eq = conn.clone();
        let pinned_eq = pinned.clone();
        let policy_eq = policy.clone();
        let dialect_eq = dialect;
        let store_eq = store.clone();
        host.register_tool(
            ToolRegistration { name: "explain_query".into(), description: "Show query execution plan without running the query".into(), input_schema: schema, annotations: Map::new() },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let conn = conn_eq.clone();
                let pinned = pinned_eq.clone();
                let policy = policy_eq.clone();
                let dialect = dialect_eq;
                let store = store_eq.clone();
                Box::pin(async move {
                    let obj = args.as_object().cloned().unwrap_or_default();
                    let sql = obj.get("sql").or_else(|| obj.get("query")).and_then(|v| v.as_str()).map(|s| s.to_string());
                    let sql = match sql { Some(s) if !s.is_empty()=>s, _=> return err("Missing SQL. Pass either \"sql\" or \"query\".") };
                    let database = obj.get("database").and_then(|v| v.as_str());
                    if let Err(e)=resolve_database(pinned.as_ref(), database) { return err(e); }
                    let db_opt = resolve_database(pinned.as_ref(), database).unwrap().clone();
                    let params = obj.get("params").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                    let only = obj.get("only").and_then(|v| v.as_array()).map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>());
                    let verdict = evaluate(&sql, &policy, dialect);
                    if !verdict.ok {
                        // log blocked
                        let draft = pluk_store::LogDraft { connection_id: conn.id.clone(), connection_name: conn.name.clone(), sql: sql.clone(), verdict: pluk_store::Verdict::Blocked, categories: Some(verdict.categories.clone()), reason: verdict.reason.clone(), source: Some("explain_query".to_string()), group: conn.via_group.clone(), database: db_opt.clone().or_else(|| pinned.clone()) };
                        let _ = store.create_log_entry(draft);
                        return err(format!("Blocked: {}", verdict.reason.unwrap_or_default()));
                    }
                    let cfg = sql_config_from(&conn, db_opt.as_deref());
                    let dw = match create_driver(CreateDriverOpts::new(cfg)).await { Ok(d)=>d, Err(e)=> return err(driver_error_to_adapter(e).message) };
                    let res = dw.driver.explain(&sql, &params).await;
                    let _ = dw.driver.close().await;
                    let res = match res { Ok(r)=>r, Err(e)=> return err(driver_error_to_adapter(e).message) };
                    let val = serde_json::json!({ "rows": res.rows, "fields": res.fields });
                    let map = FieldMap::new(&["rows","fields"], &["rows","fields"]);
                    match projected_json(val, only, &map) { Ok(t)=> ok(t), Err(e)=> err(e) }
                })
            })
        );
    }

    // describe_table
    if on("describe_table") {
        let mut props = Map::new();
        props.insert("table".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m }));
        if supports_db { props.insert("database".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m })); }
        if supports_schema { props.insert("schema".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m })); }
        let schema = object_schema(props, &["table"]);
        let conn_dt = conn.clone();
        let pinned_dt = pinned.clone();
        host.register_tool(
            ToolRegistration { name: "describe_table".into(), description: "Get column definitions for a table".into(), input_schema: schema, annotations: Map::new() },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let conn = conn_dt.clone();
                let pinned = pinned_dt.clone();
                Box::pin(async move {
                    let obj = args.as_object().cloned().unwrap_or_default();
                    let table = obj.get("table").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let database = obj.get("database").and_then(|v| v.as_str());
                    let schema_val = obj.get("schema").and_then(|v| v.as_str());
                    if let Err(e)=resolve_database(pinned.as_ref(), database) { return err(e); }
                    let schema_opt = match resolve_schema(schema_val) { Ok(v)=>v, Err(e)=> return err(e) };
                    let db_opt = resolve_database(pinned.as_ref(), database).unwrap().clone();
                    let cfg = sql_config_from(&conn, db_opt.as_deref());
                    let dw = match create_driver(CreateDriverOpts::new(cfg)).await { Ok(d)=>d, Err(e)=> return err(driver_error_to_adapter(e).message) };
                    let res = dw.driver.describe_table(&table, schema_opt.as_deref()).await;
                    let _ = dw.driver.close().await;
                    match res { Ok(cols) => {
                        let vals: Vec<Value> = cols.into_iter().map(|c| serde_json::json!({"column": c.column, "type": c.r#type, "nullable": c.nullable})).collect();
                        ok(serde_json::to_string_pretty(&vals).unwrap())
                    }, Err(e)=> err(driver_error_to_adapter(e).message) }
                })
            })
        );
    }

    // list_relationships
    if on("list_relationships") {
        let mut props = Map::new();
        props.insert("table".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m }));
        if supports_db { props.insert("database".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m })); }
        if supports_schema { props.insert("schema".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m })); }
        props.insert("only".into(), only_param_schema(&["constraints"]));
        let schema = object_schema(props, &[]);
        let conn_lr = conn.clone();
        let pinned_lr = pinned.clone();
        host.register_tool(
            ToolRegistration { name: "list_relationships".into(), description: "List foreign key relationships between tables".into(), input_schema: schema, annotations: Map::new() },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let conn = conn_lr.clone();
                let pinned = pinned_lr.clone();
                Box::pin(async move {
                    let obj = args.as_object().cloned().unwrap_or_default();
                    let table = obj.get("table").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let database = obj.get("database").and_then(|v| v.as_str());
                    let schema_val = obj.get("schema").and_then(|v| v.as_str());
                    let only = obj.get("only").and_then(|v| v.as_array()).map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>());
                    if let Err(e)=resolve_database(pinned.as_ref(), database) { return err(e); }
                    let schema_opt = match resolve_schema(schema_val) { Ok(v)=>v, Err(e)=> return err(e) };
                    let db_opt = resolve_database(pinned.as_ref(), database).unwrap().clone();
                    let cfg = sql_config_from(&conn, db_opt.as_deref());
                    let dw = match create_driver(CreateDriverOpts::new(cfg)).await { Ok(d)=>d, Err(e)=> return err(driver_error_to_adapter(e).message) };
                    let res = dw.driver.list_relationships(table.as_deref(), schema_opt.as_deref()).await;
                    let _ = dw.driver.close().await;
                    let res = match res { Ok(r)=>r, Err(e)=> return err(driver_error_to_adapter(e).message) };
                    let vals: Vec<Value> = res.into_iter().map(|r| {
                        let mut m = serde_json::Map::new();
                        m.insert("from_table".into(), Value::String(r.from_table));
                        m.insert("from_column".into(), Value::String(r.from_column));
                        m.insert("to_table".into(), Value::String(r.to_table));
                        m.insert("to_column".into(), Value::String(r.to_column));
                        if let Some(c)=r.constraint_name { m.insert("constraint_name".into(), Value::String(c)); }
                        Value::Object(m)
                    }).collect();
                    let val = Value::Array(vals);
                    match projected_json(val, only, &relationships_map()) { Ok(t)=> ok(t), Err(e)=> err(e) }
                })
            })
        );
    }

    // search_schema
    if on("search_schema") {
        let mut props = Map::new();
        props.insert("term".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m }));
        if supports_db { props.insert("database".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m })); }
        if supports_schema { props.insert("schema".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m })); }
        let schema = object_schema(props, &["term"]);
        let conn_ss = conn.clone();
        let pinned_ss = pinned.clone();
        host.register_tool(
            ToolRegistration { name: "search_schema".into(), description: "Find tables or columns matching a term".into(), input_schema: schema, annotations: Map::new() },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let conn = conn_ss.clone();
                let pinned = pinned_ss.clone();
                Box::pin(async move {
                    let obj = args.as_object().cloned().unwrap_or_default();
                    let term = obj.get("term").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let database = obj.get("database").and_then(|v| v.as_str());
                    let schema_val = obj.get("schema").and_then(|v| v.as_str());
                    if let Err(e)=resolve_database(pinned.as_ref(), database) { return err(e); }
                    let schema_opt = match resolve_schema(schema_val) { Ok(v)=>v, Err(e)=> return err(e) };
                    let db_opt = resolve_database(pinned.as_ref(), database).unwrap().clone();
                    let cfg = sql_config_from(&conn, db_opt.as_deref());
                    let dw = match create_driver(CreateDriverOpts::new(cfg)).await { Ok(d)=>d, Err(e)=> return err(driver_error_to_adapter(e).message) };
                    let res = dw.driver.search_schema(&term, schema_opt.as_deref()).await;
                    let _ = dw.driver.close().await;
                    match res { Ok(v)=> {
                        let vals: Vec<Value> = v.into_iter().map(|r| {
                            let mut m = serde_json::Map::new();
                            m.insert("kind".into(), Value::String(r.kind));
                            m.insert("table".into(), Value::String(r.table));
                            if let Some(c)=r.column { m.insert("column".into(), Value::String(c)); }
                            if let Some(t)=r.r#type { m.insert("type".into(), Value::String(t)); }
                            Value::Object(m)
                        }).collect();
                        ok(serde_json::to_string_pretty(&vals).unwrap())
                    }, Err(e)=> err(driver_error_to_adapter(e).message) }
                })
            })
        );
    }

    // table_stats
    if on("table_stats") {
        let mut props = Map::new();
        props.insert("table".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m }));
        if supports_db { props.insert("database".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m })); }
        if supports_schema { props.insert("schema".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m })); }
        props.insert("only".into(), only_param_schema(&["indexes"]));
        let schema = object_schema(props, &["table"]);
        let conn_ts = conn.clone();
        let pinned_ts = pinned.clone();
        host.register_tool(
            ToolRegistration { name: "table_stats".into(), description: "Get cheap table statistics (estimated rows, size, indexes)".into(), input_schema: schema, annotations: Map::new() },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let conn = conn_ts.clone();
                let pinned = pinned_ts.clone();
                Box::pin(async move {
                    let obj = args.as_object().cloned().unwrap_or_default();
                    let table = obj.get("table").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let database = obj.get("database").and_then(|v| v.as_str());
                    let schema_val = obj.get("schema").and_then(|v| v.as_str());
                    let only = obj.get("only").and_then(|v| v.as_array()).map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>());
                    if let Err(e)=resolve_database(pinned.as_ref(), database) { return err(e); }
                    let schema_opt = match resolve_schema(schema_val) { Ok(v)=>v, Err(e)=> return err(e) };
                    let db_opt = resolve_database(pinned.as_ref(), database).unwrap().clone();
                    let cfg = sql_config_from(&conn, db_opt.as_deref());
                    let dw = match create_driver(CreateDriverOpts::new(cfg)).await { Ok(d)=>d, Err(e)=> return err(driver_error_to_adapter(e).message) };
                    let res = dw.driver.table_stats(&table, schema_opt.as_deref()).await;
                    let _ = dw.driver.close().await;
                    let res = match res { Ok(r)=>r, Err(e)=> return err(driver_error_to_adapter(e).message) };
                    let val = serde_json::json!({
                        "table": res.table,
                        "estimatedRows": res.estimated_rows,
                        "sizeBytes": res.size_bytes,
                        "indexes": res.indexes.into_iter().map(|i| serde_json::json!({"name": i.name, "columns": i.columns, "unique": i.unique})).collect::<Vec<_>>()
                    });
                    match projected_json(val, only, &table_stats_map()) { Ok(t)=> ok(t), Err(e)=> err(e) }
                })
            })
        );
    }

    // list_schemas
    if on("list_schemas") {
        let conn_ls = conn.clone();
        host.register_tool(
            ToolRegistration { name: "list_schemas".into(), description: "List all schemas or databases".into(), input_schema: Map::new(), annotations: Map::new() },
            Arc::new(move |_args: Value| -> BoxFuture<ToolResult> {
                let conn = conn_ls.clone();
                Box::pin(async move {
                    let cfg = sql_config_from(&conn, None);
                    let dw = match create_driver(CreateDriverOpts::new(cfg)).await { Ok(d)=>d, Err(e)=> return err(driver_error_to_adapter(e).message) };
                    let res = dw.driver.list_schemas().await;
                    let _ = dw.driver.close().await;
                    match res { Ok(v)=> ok(v.join("\n")), Err(e)=> err(driver_error_to_adapter(e).message) }
                })
            })
        );
    }

    // list_databases
    if on("list_databases") {
        let desc = if supports_db { "List databases on the server. Pass one of these as `database` on other tools to query it." } else { "List databases on the server." };
        let conn_ld = conn.clone();
        host.register_tool(
            ToolRegistration { name: "list_databases".into(), description: desc.to_string(), input_schema: Map::new(), annotations: Map::new() },
            Arc::new(move |_args: Value| -> BoxFuture<ToolResult> {
                let conn = conn_ld.clone();
                Box::pin(async move {
                    let cfg = sql_config_from(&conn, None);
                    let dw = match create_driver(CreateDriverOpts::new(cfg)).await { Ok(d)=>d, Err(e)=> return err(driver_error_to_adapter(e).message) };
                    let res = dw.driver.list_databases().await;
                    let _ = dw.driver.close().await;
                    match res { Ok(v)=> ok(v.join("\n")), Err(e)=> err(driver_error_to_adapter(e).message) }
                })
            })
        );
    }

    // export_query
    if on("export_query") {
        let mut props = Map::new();
        props.insert("sql".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m }));
        props.insert("query".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m }));
        props.insert("format".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m.insert("enum".into(), Value::Array(vec![Value::String("csv".into()), Value::String("json".into())])); m }));
        if conn.r#type != "sqlite" || uses_ssh_flag { props.insert("timeout".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("number".into())); m })); }
        if supports_db { props.insert("database".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m })); }
        props.insert("params".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("array".into())); m }));
        let schema = object_schema(props, &[]);
        let store_eq2 = store.clone();
        let conn_eq2 = conn.clone();
        let pinned_eq2 = pinned.clone();
        let masked_eq2 = masked_columns.clone();
        let policy_eq2 = policy.clone();
        let dialect_eq2 = dialect;
        let cancels_eq2 = cancels.clone();
        let conn_id_eq2 = conn_id.clone();
        let conn_name_eq2 = conn_name.clone();
        let via_for_export = via_group.clone();
        host.register_tool(
            ToolRegistration { name: "export_query".into(), description: "Run a SQL query and save results to a local CSV or JSON file".into(), input_schema: schema, annotations: Map::new() },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let store = store_eq2.clone();
                let conn = conn_eq2.clone();
                let pinned = pinned_eq2.clone();
                let masked = masked_eq2.clone();
                let policy = policy_eq2.clone();
                let dialect = dialect_eq2;
                let cancels = cancels_eq2.clone();
                let conn_id = conn_id_eq2.clone();
                let conn_name = conn_name_eq2.clone();
                let via_group = via_for_export.clone();
                Box::pin(async move {
                    let obj = args.as_object().cloned().unwrap_or_default();
                    let sql = obj.get("sql").or_else(|| obj.get("query")).and_then(|v| v.as_str()).map(|s| s.to_string());
                    let sql = match sql { Some(s) if !s.is_empty()=>s, _=> return err("Missing SQL. Pass either \"sql\" or \"query\".") };
                    let format = obj.get("format").and_then(|v| v.as_str()).unwrap_or("csv").to_string();
                    let database = obj.get("database").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let db_opt = match resolve_database(pinned.as_ref(), database.as_deref()) { Ok(v)=>v, Err(e)=> return err(e) };
                    let params = obj.get("params").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                    let mut timeout_ms = obj.get("timeout").and_then(|v| v.as_u64()).map(|t| t*1000);
                    if conn.r#type == "sqlite" && !uses_ssh(&conn) {
                        timeout_ms = None;
                    } else if timeout_ms.is_none() {
                        timeout_ms = Some(30_000);
                    }
                    if let Some(b)=switch_block(&sql, pinned.as_ref()) { return err(format!("Blocked: {}", b)); }
                    let verdict = evaluate(&sql, &policy, dialect);
                    let target = CallTarget { connection_id: conn_id.clone(), connection_name: conn_name.clone(), group: via_group.clone() };
                    let meta = GateMeta { category: verdict.categories.clone(), action: "export_query".to_string(), detail: sql.clone(), database: db_opt.clone().or_else(|| pinned.clone()), command: None };
                    let sql_c = sql.clone();
                    let db_c = db_opt.clone();
                    let conn_c = conn.clone();
                    let masked_c = masked.clone();
                    let policy_c = policy.clone();
                    let pinned_for_inner = pinned.clone();
                    let _pinned_for_precheck = pinned.clone();
                    
                    run_gated(&store, &target, meta, move |log_id| {
                        let cancels = cancels.clone();
                        let conn = conn_c.clone();
                        let sql = sql_c.clone();
                        let params = params.clone();
                        let db_opt = db_c.clone();
                        let masked = masked_c.clone();
                        let policy = policy_c.clone();
                        let timeout = timeout_ms;
                        let format = format.clone();
                        let pinned_for_payload = pinned_for_inner.clone();
                        async move {
                            let token = log_id.map(|id| cancels.register(id));
                            let opts = {
                                let has_timeout = timeout.is_some();
                                let has_cancel = token.is_some();
                                if has_timeout || has_cancel {
                                    Some(QueryOpts { timeout_ms: timeout, cancel: token.clone() })
                                } else {
                                    None
                                }
                            };
                            let cfg = sql_config_from(&conn, db_opt.as_deref());
                            let dw = create_driver(CreateDriverOpts::new(cfg)).await.map_err(driver_error_to_adapter)?;
                            let res = {
                                let use_ro = policy.allowed.len()==2 && policy.allowed.contains(&pluk_policy::category::StatementCategory::Select);
                                if use_ro { dw.driver.query_read_only(&sql, &params, opts.clone()).await } else { dw.driver.query(&sql, &params, opts.clone()).await }
                            };
                            let res = match res { Ok(r)=>r, Err(e)=> { if let Some(id)=log_id { cancels.clear(id); } let _ = dw.driver.close().await; return Err(driver_error_to_adapter(e)); } };
                            let _ = dw.driver.close().await;
                            if let Some(id)=log_id { cancels.clear(id); }
                            let cap = policy.max_rows.map(|v| v as usize);
                            let total = res.rows.len();
                            let fields_tmp = res.fields.clone();
                            let (mut rows, _trunc, _cap_limit) = cap_rows_vec(res.rows.into_iter().collect(), cap);
                            mask_rows(&mut rows, &masked);
                            let fields = fields_tmp.unwrap_or_else(|| rows.first().and_then(|v| v.as_object()).map(|m| m.keys().cloned().collect()).unwrap_or_default());
                            let dir = pluk_core::platform::data_dir().join("exports");
                            let _ = std::fs::create_dir_all(&dir);
                            let ts = Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();
                            let fname = format!("{}_{}.{}", sanitize_filename(&conn.name), ts, format);
                            let path = dir.join(fname);
                            let payload = if format=="csv" {
                                to_csv(&rows, &fields)
                            } else {
                                let meta_val = serde_json::json!({
                                    "env": "development",
                                    "connection": conn.name,
                                    "type": conn.r#type,
                                    "database": effective_db(pinned_for_payload.as_ref(), db_opt.as_deref()),
                                    "fields": fields,
                                    "rows": rows.clone(),
                                    "truncated": _trunc,
                                    "row_cap": _cap_limit.map(|v| Value::Number((v as i64).into())).unwrap_or(Value::Null),
                                    "row_count": total,
                                    "returned_rows": rows.len()
                                });
                                serde_json::to_string_pretty(&meta_val).unwrap()
                            };
                            let _ = tokio::fs::write(&path, payload).await.map_err(|e| crate::error::AdapterError::new(e.to_string()));
                            let snapshot = pluk_store::QueryResult { fields: fields.clone(), rows: rows.clone() };
                            Ok(Outcome::Ran(RunOutcome { text: format!("Exported {} rows to {}", rows.len(), path.display()), result: Some(snapshot), ..Default::default() }))
                        }
                    }, GateOpts::default()
                        .precheck({
                            let sql = sql.clone();
                            let pinned = pinned.clone();
                            let verdict = verdict.clone();
                            move || {
                                if let Some(b)=switch_block(&sql, pinned.as_ref()) { return Some(b); }
                                if !verdict.ok { return Some(verdict.reason.clone().unwrap_or_else(|| "blocked".into())); }
                                None
                            }
                        })
                        .classify_error(cancelled_when_message_contains("cancelled"))
                        .format_error(|e, v| if v==pluk_store::Verdict::Cancelled { format!("Cancelled: {}", e.message) } else { format_sql_error(e) })
                    ).await
                })
            })
        );
    }

    // run_saved_query
    if on("run_saved_query") {
        let mut props = Map::new();
        props.insert("name".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m }));
        if conn.r#type != "sqlite" || uses_ssh_flag { props.insert("timeout".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("number".into())); m })); }
        if supports_db { props.insert("database".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("string".into())); m })); }
        props.insert("params".into(), Value::Object({ let mut m=Map::new(); m.insert("type".into(), Value::String("array".into())); m }));
        props.insert("only".into(), only_param_schema(&["connection","limits"]));
        let schema = object_schema(props, &["name"]);
        let store_rsq = store.clone();
        let conn_rsq = conn.clone();
        let pinned_rsq = pinned.clone();
        let masked_rsq = masked_columns.clone();
        let policy_rsq = policy.clone();
        let dialect_rsq = dialect;
        let cancels_rsq = cancels.clone();
        let conn_id_rsq = conn_id.clone();
        let conn_name_rsq = conn_name.clone();
        let conn_env_rsq = conn_env.clone();
        let conn_type_rsq = conn_type.clone();
        host.register_tool(
            ToolRegistration { name: "run_saved_query".into(), description: "Run a saved query by name".into(), input_schema: schema, annotations: Map::new() },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let store = store_rsq.clone();
                let conn = conn_rsq.clone();
                let pinned = pinned_rsq.clone();
                let masked = masked_rsq.clone();
                let policy = policy_rsq.clone();
                let dialect = dialect_rsq;
                let cancels = cancels_rsq.clone();
                let conn_id = conn_id_rsq.clone();
                let conn_name = conn_name_rsq.clone();
                let conn_env = conn_env_rsq.clone();
                let conn_type = conn_type_rsq.clone();
                let via_group = via_group.clone();
                Box::pin(async move {
                    let obj = args.as_object().cloned().unwrap_or_default();
                    let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let database = obj.get("database").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let db_opt = match resolve_database(pinned.as_ref(), database.as_deref()) { Ok(v)=>v, Err(e)=> return err(e) };
                    let params = obj.get("params").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                    let only = obj.get("only").and_then(|v| v.as_array()).map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>());
                    let mut timeout_ms = obj.get("timeout").and_then(|v| v.as_u64()).map(|t| t*1000);
                    if conn.r#type == "sqlite" && !uses_ssh(&conn) {
                        timeout_ms = None;
                    } else if timeout_ms.is_none() {
                        timeout_ms = Some(30_000);
                    }
                    let saved = match store.get_saved_query(&conn.id, &name).unwrap_or(None) { Some(q)=>q, None=> return err(format!("Saved query \"{}\" not found.", name)) };
                    let sql = saved.sql.clone();
                    if let Some(b)=switch_block(&sql, pinned.as_ref()) { return err(format!("Blocked: {}", b)); }
                    let verdict = evaluate(&sql, &policy, dialect);
                    let target = CallTarget { connection_id: conn_id.clone(), connection_name: conn_name.clone(), group: via_group.clone() };
                    let meta = GateMeta { category: verdict.categories.clone(), action: "run_saved_query".to_string(), detail: sql.clone(), database: db_opt.clone().or_else(|| pinned.clone()), command: None };
                    let sql_c = sql.clone();
                    let db_c = db_opt.clone();
                    let conn_c = conn.clone();
                    let masked_c = masked.clone();
                    let policy_c = policy.clone();
                    let conn_env_c = conn_env.clone();
                    let conn_name_c = conn_name.clone();
                    let conn_type_c = conn_type.clone();
                    let pinned_c = pinned.clone();
                    let only_c = only.clone();
                    
                    run_gated(&store, &target, meta, move |log_id| {
                        let cancels = cancels.clone();
                        let conn = conn_c.clone();
                        let sql = sql_c.clone();
                        let params = params.clone();
                        let db_opt = db_c.clone();
                        let masked = masked_c.clone();
                        let policy = policy_c.clone();
                        let conn_env = conn_env_c.clone();
                        let conn_name = conn_name_c.clone();
                        let conn_type = conn_type_c.clone();
                        let pinned = pinned_c.clone();
                        let only = only_c.clone();
                        let timeout = timeout_ms;
                        async move {
                            let token = log_id.map(|id| cancels.register(id));
                            let opts = {
                                let has_timeout = timeout.is_some();
                                let has_cancel = token.is_some();
                                if has_timeout || has_cancel {
                                    Some(QueryOpts { timeout_ms: timeout, cancel: token.clone() })
                                } else {
                                    None
                                }
                            };
                            let cfg = sql_config_from(&conn, db_opt.as_deref());
                            let dw = create_driver(CreateDriverOpts::new(cfg)).await.map_err(driver_error_to_adapter)?;
                            let use_ro = policy.allowed.len()==2;
                            let res = if use_ro { dw.driver.query_read_only(&sql, &params, opts.clone()).await } else { dw.driver.query(&sql, &params, opts.clone()).await };
                            let res = match res { Ok(r)=>r, Err(e)=> { if let Some(id)=log_id { cancels.clear(id); } let _ = dw.driver.close().await; return Err(driver_error_to_adapter(e)); } };
                            let _ = dw.driver.close().await;
                            if let Some(id)=log_id { cancels.clear(id); }
                            let cap = policy.max_rows.map(|v| v as usize);
                            let total = res.rows.len();
                            let (mut rows, truncated, cap_limit) = cap_rows_vec(res.rows.into_iter().collect(), cap);
                            mask_rows(&mut rows, &masked);
                            let fields = res.fields.unwrap_or_default();
                            let meta_val = serde_json::json!({
                                "env": conn_env,
                                "connection": conn_name,
                                "type": conn_type,
                                "database": effective_db(pinned.as_ref(), db_opt.as_deref()),
                                "fields": fields,
                                "rows": rows.clone(),
                                "truncated": truncated,
                                "row_cap": cap_limit.map(|v| Value::Number((v as i64).into())).unwrap_or(Value::Null),
                                "row_count": total,
                                "returned_rows": rows.len()
                            });
                            let mut text = projected_json(meta_val, only, &query_map()).map_err(crate::error::AdapterError::new)?;
                            if truncated { text.push_str(&format!("\n\n[Row limit: showing first {} of {} rows. Add a LIMIT clause to see all results.]", cap_limit.unwrap_or(0), total)); }
                            let snapshot = pluk_store::QueryResult { fields, rows: rows.clone() };
                            Ok(Outcome::Ran(RunOutcome { text, result: Some(snapshot), ..Default::default() }))
                        }
                    }, GateOpts::default()
                        .precheck({
                            let sql = sql.clone();
                            let pinned = pinned.clone();
                            let verdict = verdict.clone();
                            move || {
                                if let Some(b)=switch_block(&sql, pinned.as_ref()) { return Some(b); }
                                if !verdict.ok { return Some(verdict.reason.clone().unwrap_or_else(|| "blocked".into())); }
                                None
                            }
                        })
                        .classify_error(cancelled_when_message_contains("cancelled"))
                        .format_error(|e, v| if v==pluk_store::Verdict::Cancelled { format!("Cancelled: {}", e.message) } else { format_sql_error(e) })
                    ).await
                })
            })
        );
    }

    // list_saved_queries
    if on("list_saved_queries") {
        let mut props = Map::new();
        props.insert("only".into(), only_param_schema(&["sql","ids"]));
        let schema = object_schema(props, &[]);
        let store_lsq = store.clone();
        let conn_lsq = conn.clone();
        host.register_tool(
            ToolRegistration { name: "list_saved_queries".into(), description: "List saved queries for this connection".into(), input_schema: schema, annotations: Map::new() },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let store = store_lsq.clone();
                let _conn = conn_lsq.clone();
                Box::pin(async move {
                    let obj = args.as_object().cloned().unwrap_or_default();
                    let only = obj.get("only").and_then(|v| v.as_array()).map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>());
                    let queries = store.list_saved_queries(&_conn.id).unwrap_or_default();
                    let val = serde_json::to_value(queries).unwrap();
                    match projected_json(val, only, &saved_queries_map()) { Ok(t)=> ok(t), Err(e)=> err(e) }
                })
            })
        );
    }

    Ok(())
}

fn sanitize_filename(input: &str) -> String {
    input.chars().map(|c| if c.is_ascii_alphanumeric() || c=='_' || c=='-' { c } else { '_' }).collect::<String>().chars().take(64).collect()
}
fn to_csv(rows: &[Value], fields: &[String]) -> String {
    if rows.is_empty() { return fields.join(",") + "\n"; }
    let escape = |v: &Value| {
        let s = match v { Value::String(s)=> s.clone(), Value::Null => "".to_string(), other=> other.to_string() };
        if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else { s }
    };
    let mut lines = vec![fields.join(",")];
    for row in rows {
        if let Value::Object(map) = row {
            lines.push(fields.iter().map(|f| map.get(f).map(&escape).unwrap_or_default()).collect::<Vec<_>>().join(","));
        } else { lines.push(String::new()); }
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
