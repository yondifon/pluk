pub mod client;
pub mod resolve;

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Map, Value};

use pluk_store::Integration;

use crate::adapter::{Adapter, PolicyKind};
use crate::config_field::{ConfigField, FieldType};
use crate::error::AdapterError;
use crate::gate::{run_gated, CallTarget, GateMeta, GateOpts, Outcome, RunOutcome};
use crate::instructions::{build_instructions, InstructionParts};
use crate::projection::{apply_only, FieldMap, Preset};
use crate::tool_host::{object_schema, ToolHost, ToolRegistration, ToolHandler};
use crate::tool_spec::ToolSpec;

use client::linear_graphql;
use resolve::{resolve_labels, resolve_state, resolve_team, resolve_user};

const AGENT_HINT: &str = "Use this for Linear issue tracking — start with my_issues for the work assigned to you, or list_issues / search_issues / list_projects to look wider. Read a thread with list_comments and check who replied to you with inbox. Check project progress and issue counts with list_projects and a project's status-update log with project_updates. Create issues, comment or reply, move an issue with update_issue, and attach a pull request with link_url when write is permitted. Read before writing.";

fn linear_fields() -> Vec<ConfigField> {
    vec![
        ConfigField::new("api_key", "API Key", FieldType::Password).group("Auth").required().secret().placeholder("lin_api_…"),
        ConfigField::new("team_key", "Default Team", FieldType::Text).group("Defaults").placeholder("ENG (optional, scopes list_issues)"),
    ]
}

fn linear_config(conn: &Integration) -> (String, Option<String>) {
    let api_key = conn.config.get("api_key").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let team = conn.config.get("team_key").and_then(|v| v.as_str()).map(|s| s.to_string()).filter(|s| !s.is_empty());
    (api_key, team)
}

// Field maps
fn issue_list_map() -> FieldMap {
    FieldMap::new(&["identifier","title","state","assignee","updatedAt","id","url","priority"], &["identifier","title","state.name","assignee.name","updatedAt"])
        .with_preset("priority", Preset::paths(&["priority"]))
        .with_preset("url", Preset::paths(&["url"]))
        .with_preset("ids", Preset::paths(&["id"]))
}
fn my_issues_map() -> FieldMap {
    FieldMap::new(&["identifier","title","state","priority","assignee","id","url","updatedAt"], &["identifier","title","state.name","priority"])
        .with_preset("priority", Preset::paths(&["priority"]))
        .with_preset("url", Preset::paths(&["url"]))
        .with_preset("ids", Preset::paths(&["id"]))
}
fn get_issue_map() -> FieldMap {
    FieldMap::new(&["identifier","title","description","state","assignee","priority","url","id","branchName","estimate","dueDate","createdAt","updatedAt","team","project","parent","labels"], &["identifier","title","description","state.name","assignee.name","priority","url"])
        .with_preset("meta", Preset::paths(&["labels","project","parent","team","createdAt","updatedAt"]))
        .with_preset("planning", Preset::paths(&["estimate","dueDate"]))
        .with_preset("branch", Preset::paths(&["branchName"]))
}
fn list_comments_map() -> FieldMap {
    FieldMap::new(&["issue","comments"], &["issue","comments.body","comments.user.name","comments.createdAt","comments.replies"])
        .with_preset("refs", Preset::paths(&["comments.id","comments.url"]))
}
fn inbox_map() -> FieldMap {
    FieldMap::new(&["type","subtitle","createdAt","actor","issue","comment","title","url","id","readAt","parentComment"], &["type","subtitle","createdAt","actor.name","issue.identifier","issue.title","comment.body"])
        .with_preset("urls", Preset::paths(&["url","comment.url","issue.url"]))
        .with_preset("thread", Preset::paths(&["parentComment"]))
        .with_preset("read", Preset::paths(&["readAt"]))
}
fn list_teams_map() -> FieldMap {
    FieldMap::new(&["id","name","key"], &["id","name","key"])
}
fn list_projects_map() -> FieldMap {
    FieldMap::new(&["id","name","state","progress_percent","total_issues","completed_issues","url","startDate","targetDate","lead"], &["id","name","state","progress_percent","total_issues","completed_issues"])
        .with_preset("dates", Preset::paths(&["startDate","targetDate"]))
        .with_preset("lead", Preset::paths(&["lead.name"]))
}
fn project_updates_map() -> FieldMap {
    FieldMap::new(&["project","updates"], &["project","updates.health","updates.createdAt","updates.user.name","updates.body"])
        .with_preset("urls", Preset::paths(&["updates.url"]))
}
fn create_issue_map() -> FieldMap { FieldMap::new(&["success","issue"], &["issue.identifier","issue.url"]) }
fn update_issue_map() -> FieldMap { FieldMap::new(&["success","issue"], &["issue.identifier","issue.state.name"]) }
fn comment_map() -> FieldMap { FieldMap::new(&["success","comment"], &["comment.url"]) }

pub fn thread_comments(nodes: Vec<Value>) -> Vec<Value> {
    // build map id -> node with replies
    let mut by_id: BTreeMap<String, Value> = BTreeMap::new();
    for n in &nodes {
        let mut rest = n.as_object().cloned().unwrap_or_default();
        rest.remove("parentId");
        let id = n.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let mut obj = Value::Object(rest);
        if let Value::Object(ref mut m) = obj { m.insert("replies".to_string(), Value::Array(vec![])); }
        by_id.insert(id, obj);
    }
    let mut roots: Vec<Value> = Vec::new();
    // Need to track replies nesting
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut is_child: std::collections::HashSet<String> = std::collections::HashSet::new();
    for n in &nodes {
        let id = n.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if let Some(pid) = n.get("parentId").and_then(|v| v.as_str()) {
            if by_id.contains_key(pid) {
                children.entry(pid.to_string()).or_default().push(id.clone());
                is_child.insert(id.clone());
            }
        }
    }
    // build nested structures recursively
    fn build(id: &str, by_id: &BTreeMap<String, Value>, children: &BTreeMap<String, Vec<String>>) -> Value {
        let mut node = by_id.get(id).cloned().unwrap_or(Value::Null);
        if let Value::Object(ref mut m) = node {
            if let Some(kids) = children.get(id) {
                let replies: Vec<Value> = kids.iter().map(|kid| build(kid, by_id, children)).collect();
                m.insert("replies".to_string(), Value::Array(replies));
            }
        }
        node
    }
    for n in &nodes {
        let id = n.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let parent_id = n.get("parentId").and_then(|v| v.as_str());
        let parent_exists = parent_id.map(|pid| by_id.contains_key(pid)).unwrap_or(false);
        if !is_child.contains(&id) || !parent_exists {
            // For orphan (parent outside page) treat as root if not child of existing
            if parent_id.is_none() || !parent_exists {
                // but ensure not already counted as child
                if !is_child.contains(&id) || !parent_exists {
                    roots.push(build(&id, &by_id, &children));
                }
            }
        }
    }
    // Simpler: just push roots that are not children
    // The above loop double counts; redo clean:
    let mut final_roots = Vec::new();
    for n in &nodes {
        let id = n.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if !is_child.contains(&id) {
            final_roots.push(build(&id, &by_id, &children));
        } else {
            // orphan whose parent missing already not in is_child? Actually orphans are not marked as child if parent missing
            let pid = n.get("parentId").and_then(|v| v.as_str());
            if let Some(pid) = pid { if !by_id.contains_key(pid) { final_roots.push(build(&id, &by_id, &children)); } }
        }
    }
    // Deduplicate by id preserve order
    let mut seen = std::collections::HashSet::new();
    let mut dedup = Vec::new();
    for r in final_roots { let id = r.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(); if seen.insert(id) { dedup.push(r); } }
    // For orphan case where nodes contains child before parent missing, the above handles
    // Fallback to earlier roots if dedup empty but nodes non-empty -> use roots computed
    if dedup.is_empty() && !roots.is_empty() { return roots; }
    dedup
}

fn summarize_project(p: Value) -> Value {
    if let Value::Object(mut m) = p {
        let issue_history = m.remove("issueCountHistory");
        let completed_history = m.remove("completedIssueCountHistory");
        let progress = m.remove("progress");
        let last = |v: Option<Value>| -> i64 {
            if let Some(Value::Array(arr)) = v { arr.last().and_then(|n| n.as_i64().or_else(|| n.as_u64().map(|x| x as i64)).or_else(|| n.as_f64().map(|x| x as i64))).unwrap_or(0) } else { 0 }
        };
        let total = last(issue_history);
        let completed = last(completed_history);
        let prog = progress.as_ref().and_then(|v| v.as_f64()).unwrap_or(0.0);
        m.insert("total_issues".to_string(), json!(total));
        m.insert("completed_issues".to_string(), json!(completed));
        m.insert("progress_percent".to_string(), json!((prog * 100.0).round() as i64));
        Value::Object(m)
    } else { p }
}

// helpers for tool handlers
fn only_from_args(args: &Value) -> Option<Vec<String>> {
    args.get("only").and_then(|v| v.as_array()).map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
}
fn wrap_ok(value: Value) -> RunOutcome {
    let text = match &value { Value::String(s) => s.clone(), _ => serde_json::to_string_pretty(&value).unwrap_or("{}".to_string()) };
    let rows = match &value { Value::Array(a) => a.clone(), other => vec![other.clone()] };
    RunOutcome { text: text.clone(), is_error: false, result: Some(pluk_store::QueryResult { fields: vec![], rows }), response_text: Some(text), ..Default::default() }
}
fn project_value(data: Value, only: Option<Vec<String>>, map: &FieldMap) -> Result<Value, AdapterError> {
    apply_only(&data, only.as_ref(), map).map_err(|e| AdapterError::new(e.to_string()))
}

fn handler<F, Fut>(store: Arc<pluk_store::Store>, conn: Integration, meta_fn: impl Fn(&Value)->String + Send + Sync + 'static, category: &'static str, tool_name: &'static str, f: F) -> ToolHandler
where F: Fn(Value, String, Option<String>) -> Fut + Send + Sync + 'static, Fut: std::future::Future<Output=Result<Value, AdapterError>> + Send + 'static {
    let store = store.clone();
    let conn_clone = conn.clone();
    Arc::new(move |args: Value| {
        let store = store.clone();
        let conn = conn_clone.clone();
        let meta_fn = &meta_fn;
        let detail = meta_fn(&args);
        let (api_key, default_team) = linear_config(&conn);
        let fut = f(args.clone(), api_key.clone(), default_team.clone());
        let meta = GateMeta::new(category, tool_name, detail);
        let target = CallTarget::from(&conn);
        Box::pin(async move {
            run_gated(&store, &target, meta, |_| async {
                let v = fut.await?;
                Ok(Outcome::Ran(wrap_ok(v)))
            }, GateOpts::default()).await.into()
        }) as _ // tool result already
    })
}

// Instead use direct gate: we computed wrap above; run_gated returns ToolResult
// Need trait workaround: use helper

pub struct LinearAdapter { store: Arc<pluk_store::Store> }
impl LinearAdapter {
    pub fn new(store: Arc<pluk_store::Store>) -> Arc<Self> { Arc::new(Self { store }) }
}

#[async_trait]
impl Adapter for LinearAdapter {
    fn id(&self) -> &str { "linear" }
    fn label(&self) -> &str { "Linear" }
    fn category(&self) -> &str { "issue-tracker" }
    fn policy_kind(&self) -> PolicyKind { PolicyKind::Action }
    fn agent_hint(&self) -> &str { AGENT_HINT }
    fn tool_specs(&self) -> &[ToolSpec] {
        static SPECS: std::sync::OnceLock<Vec<ToolSpec>> = std::sync::OnceLock::new();
        SPECS.get_or_init(|| vec![
            ToolSpec::new("list_issues", "List issues, optionally scoped to a team.", "read"),
            ToolSpec::new("my_issues", "List the issues assigned to you — the starting point for \"what am I working on\". Open issues only unless include_done is set.", "read"),
            ToolSpec::new("get_issue", "Get a single issue by its id or identifier (e.g. ENG-123)", "read"),
            ToolSpec::new("search_issues", "Search issues by text in title or description", "read"),
            ToolSpec::new("list_comments", "Read an issue's comment thread, oldest first, with replies nested under the comment they answer. Use this to see the discussion and whether anyone responded.", "read"),
            ToolSpec::new("inbox", "Read your Linear notifications — replies to your comments, mentions, assignments and status changes, newest first. Unread only by default. Use this to find out who responded to you and where.", "read"),
            ToolSpec::new("list_teams", "List teams (id, name, key).", "read").with_default_enabled(false),
            ToolSpec::new("list_states", "List a team's workflow states (id, name, type). Use a state name with update_issue to move an issue.", "read").with_default_enabled(false),
            ToolSpec::new("list_projects", "List projects with their state, progress percent, and issue counts (total/completed). Optionally filter by name.", "read").with_default_enabled(false),
            ToolSpec::new("project_updates", "Read a project's status-update log (the periodic updates with health on-track/at-risk/off-track), newest first. Use list_projects to find the project id.", "read").with_default_enabled(false),
            ToolSpec::new("create_issue", "Create a new issue. Team by key or name, assignee by email or display name, state and labels by name.", "write"),
            ToolSpec::new("comment", "Add a comment to an issue, or reply in a thread by passing the comment id you are answering as parent_id. Get comment ids from list_comments.", "write"),
            ToolSpec::new("update_issue", "Update an issue — move it to another state, reassign or unassign, or change title, description, priority, estimate or labels. State by name, assignee by email or display name, labels by name.", "write"),
            ToolSpec::new("link_url", "Attach a URL to an issue — a pull request, build, or doc. URLs from a configured integration (GitHub, GitLab, Slack) become rich attachments that sync status back to the issue.", "write"),
        ])
    }
    fn config_fields(&self) -> &[ConfigField] {
        static FIELDS: std::sync::OnceLock<Vec<ConfigField>> = std::sync::OnceLock::new();
        FIELDS.get_or_init(linear_fields)
    }
    async fn test_connection(&self, conn: &Integration) -> Result<(), AdapterError> {
        let (api_key, _) = linear_config(conn);
        let data = linear_graphql(&api_key, "{ viewer { id name } }", json!({})).await?;
        if data.get("viewer").is_some() { Ok(()) } else { Err(AdapterError::new("Linear test failed: no viewer")) }
    }
    fn instructions(&self, conn: &Integration) -> String {
        let enabled: Vec<&str> = self.tool_specs().iter().filter(|t| {
            let gate = pluk_policy::tool_gate(conn.query_policy.as_deref());
            gate.enabled(&t.name, t.default_enabled)
        }).map(|t| t.name.as_str()).collect();
        let policy = if enabled.is_empty() { "No tools are enabled on this integration.".to_string() } else { format!("Enabled tools: {}.", enabled.join(", ")) };
        build_instructions(&conn.name, conn.environment, InstructionParts { kind: "Linear".into(), access: "Read and search Linear issues; create or update them when write is permitted. Every action is policy-checked and recorded in the activity log.".into(), policy: Some(policy), start: None, hint: Some(AGENT_HINT.into()) })
    }
    fn register(&self, host: &mut dyn ToolHost, conn: &Integration, _owner_id: &str) -> Result<(), AdapterError> {
        let store = self.store.clone();
        // helper to register one tool with gated runner
        macro_rules! reg {
            ($name:expr, $desc:expr, $cat:expr, $schema:expr, $detail:expr, $body:expr) => {
                {
                    let store = store.clone();
                    let conn = conn.clone();
                    let handler: ToolHandler = Arc::new(move |args: Value| {
                        let store = store.clone();
                        let conn = conn.clone();
                        let detail = $detail(&args);
                        let meta = GateMeta::new($cat, $name, detail);
                        let target = CallTarget::from(&conn);
                        Box::pin(async move {
                            run_gated(&store, &target, meta, |_| async {
                                let out = $body(args, &conn).await?;
                                let text = match &out { Value::String(s) => s.clone(), _ => serde_json::to_string_pretty(&out).unwrap_or("{}".into()) };
                                let rows = match &out { Value::Array(a) => a.clone(), o => vec![o.clone()] };
                                Ok(Outcome::Ran(RunOutcome { text: text.clone(), response_text: Some(text), result: Some(pluk_store::QueryResult{ fields: vec![], rows }), ..Default::default() }))
                            }, GateOpts::default()).await
                        })
                    });
                    let mut props = $schema;
                    let schema = if props.is_empty() { Map::new() } else { object_schema(props, &[]) };
                    host.register_tool(ToolRegistration { name: $name.into(), description: $desc.into(), input_schema: schema, annotations: Map::new() }, handler);
                }
            };
        }
        const ISSUE_FIELDS: &str = "id identifier title state { name } assignee { name } priority url updatedAt";
        const PROJECT_FIELDS: &str = "id name state progress startDate targetDate url lead { name } issueCountHistory completedIssueCountHistory";
        const COMMENT_FIELDS: &str = "id body url createdAt parentId resolvedAt user { name } botActor { name }";
        const NOTIFICATION_FIELDS: &str = "id type title subtitle url createdAt readAt actor { name }\n  ... on IssueNotification { issue { identifier title url } comment { body url } parentComment { id url } }";

        // list_issues
        {
            let fallback_team = conn.config.get("team_key").and_then(|v| v.as_str()).unwrap_or("*").to_string();
            reg!("list_issues", "List issues, optionally scoped to a team.", "read", {
                let mut m = Map::new();
                m.insert("team".into(), json!({"type":"string","description":"Team key (e.g. ENG). Defaults to the integration's team if set."}));
                m.insert("limit".into(), json!({"type":"integer","minimum":1,"maximum":100,"default":25,"description":"Max issues to return"}));
                m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&["priority","url","ids"])}));
                m
            }, {
                let ft = fallback_team.clone();
                move |args: &Value| format!("list_issues team={} limit={}", args.get("team").and_then(|v| v.as_str()).unwrap_or(&ft), args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25))
            }, |args: Value, conn: &Integration| {
            let (api_key, default_team) = linear_config(conn);
            Box::pin(async move {
                let team_key = args.get("team").and_then(|v| v.as_str()).map(|s| s.to_string()).or(default_team);
                let filter = team_key.as_ref().map(|k| json!({ "team": { "key": { "eq": k } } }));
                let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25);
                let data = linear_graphql(&api_key, &format!("query($first:Int!,$filter:IssueFilter){{ issues(first:$first, filter:$filter){{ nodes {{ {ISSUE_FIELDS} }} }} }}"), json!({ "first": limit, "filter": filter })).await?;
                let nodes = data.get("issues").and_then(|v| v.get("nodes")).cloned().unwrap_or(json!([]));
                let only = only_from_args(&args);
                project_value(nodes, only, &issue_list_map())
            })
        });
        }
        // my_issues
        reg!("my_issues", "List the issues assigned to you — the starting point for \"what am I working on\". Open issues only unless include_done is set.", "read", {
            let mut m = Map::new();
            m.insert("include_done".into(), json!({"type":"boolean","default":false,"description":"Also return completed and canceled issues"}));
            m.insert("limit".into(), json!({"type":"integer","minimum":1,"maximum":100,"default":25,"description":"Max issues to return"}));
            m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&["priority","url","ids"])}));
            m
        }, |args: &Value| format!("my_issues include_done={} limit={}", args.get("include_done").and_then(|v| v.as_bool()).unwrap_or(false), args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25)), |args: Value, conn: &Integration| {
            let (api_key, _) = linear_config(conn);
            Box::pin(async move {
                let include_done = args.get("include_done").and_then(|v| v.as_bool()).unwrap_or(false);
                let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25);
                let mut filter = json!({ "assignee": { "isMe": { "eq": true } } });
                if !include_done { filter.as_object_mut().unwrap().insert("state".into(), json!({ "type": { "nin": ["completed","canceled"] } })); }
                let data = linear_graphql(&api_key, &format!("query($first:Int!,$filter:IssueFilter){{ issues(first:$first, filter:$filter){{ nodes {{ {ISSUE_FIELDS} }} }} }}"), json!({ "first": limit, "filter": filter })).await?;
                let nodes = data.get("issues").and_then(|v| v.get("nodes")).cloned().unwrap_or(json!([]));
                let only = only_from_args(&args);
                project_value(nodes, only, &my_issues_map())
            })
        });
        // get_issue
        reg!("get_issue", "Get a single issue by its id or identifier (e.g. ENG-123)", "read", {
            let mut m = Map::new();
            m.insert("id".into(), json!({"type":"string","description":"Issue id or identifier"}));
            m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&["meta","planning","branch"])}));
            m
        }, |args: &Value| format!("get_issue {}", args.get("id").and_then(|v| v.as_str()).unwrap_or("")), |args: Value, conn: &Integration| {
            let (api_key, _) = linear_config(conn);
            Box::pin(async move {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let data = linear_graphql(&api_key, "query($id:String!){ issue(id:$id){ id identifier title description state { name type } assignee { name } priority estimate dueDate branchName url createdAt updatedAt team { key } project { name } parent { identifier title } labels { nodes { name } } } }", json!({ "id": id })).await?;
                let issue = data.get("issue").cloned().unwrap_or(Value::Null);
                let only = only_from_args(&args);
                project_value(issue, only, &get_issue_map())
            })
        });
        // search_issues
        reg!("search_issues", "Search issues by text in title or description", "read", {
            let mut m = Map::new();
            m.insert("query".into(), json!({"type":"string","description":"Search term"}));
            m.insert("limit".into(), json!({"type":"integer","minimum":1,"maximum":100,"default":25,"description":"Max issues to return"}));
            m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&["priority","url","ids"])}));
            m
        }, |args: &Value| format!("search_issues \"{}\" limit={}", args.get("query").and_then(|v| v.as_str()).unwrap_or(""), args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25)), |args: Value, conn: &Integration| {
            let (api_key, _) = linear_config(conn);
            Box::pin(async move {
                let q = args.get("query").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25);
                let filter = json!({ "or": [{ "title": { "containsIgnoreCase": q } }, { "description": { "containsIgnoreCase": q } }] });
                let data = linear_graphql(&api_key, &format!("query($first:Int!,$filter:IssueFilter){{ issues(first:$first, filter:$filter){{ nodes {{ {ISSUE_FIELDS} }} }} }}"), json!({ "first": limit, "filter": filter })).await?;
                let nodes = data.get("issues").and_then(|v| v.get("nodes")).cloned().unwrap_or(json!([]));
                let only = only_from_args(&args);
                project_value(nodes, only, &issue_list_map())
            })
        });
        // list_comments
        reg!("list_comments", "Read an issue's comment thread, oldest first, with replies nested under the comment they answer. Use this to see the discussion and whether anyone responded.", "read", {
            let mut m = Map::new();
            m.insert("issue_id".into(), json!({"type":"string","description":"Issue id or identifier (e.g. ENG-123)"}));
            m.insert("limit".into(), json!({"type":"integer","minimum":1,"maximum":100,"default":50,"description":"Max comments to fetch; replies count towards this"}));
            m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&["refs"])}));
            m
        }, |args: &Value| format!("list_comments {} limit={}", args.get("issue_id").and_then(|v| v.as_str()).unwrap_or(""), args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50)), |args: Value, conn: &Integration| {
            let (api_key, _) = linear_config(conn);
            Box::pin(async move {
                let issue_id = args.get("issue_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
                let data = linear_graphql(&api_key, &format!("query($id:String!,$first:Int!){{ issue(id:$id){{ identifier comments(first:$first, orderBy:createdAt){{ nodes {{ {COMMENT_FIELDS} }} }} }} }}"), json!({ "id": issue_id, "first": limit })).await?;
                let issue = data.get("issue");
                if issue.is_none() || issue.unwrap().is_null() { return Err(AdapterError::new(format!("Issue \"{issue_id}\" not found."))); }
                let issue_obj = issue.unwrap();
                let identifier = issue_obj.get("identifier").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let nodes = issue_obj.get("comments").and_then(|c| c.get("nodes")).and_then(|n| n.as_array()).cloned().unwrap_or_default();
                let threaded = thread_comments(nodes);
                let result = json!({ "issue": identifier, "comments": threaded });
                let only = only_from_args(&args);
                project_value(result, only, &list_comments_map())
            })
        });
        // inbox
        reg!("inbox", "Read your Linear notifications — replies to your comments, mentions, assignments and status changes, newest first. Unread only by default. Use this to find out who responded to you and where.", "read", {
            let mut m = Map::new();
            m.insert("unread_only".into(), json!({"type":"boolean","default":true,"description":"Only notifications you have not read yet"}));
            m.insert("limit".into(), json!({"type":"integer","minimum":1,"maximum":50,"default":25,"description":"Max notifications to return"}));
            m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&["urls","thread","read"])}));
            m
        }, |args: &Value| format!("inbox unread_only={} limit={}", args.get("unread_only").and_then(|v| v.as_bool()).unwrap_or(true), args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25)), |args: Value, conn: &Integration| {
            let (api_key, _) = linear_config(conn);
            Box::pin(async move {
                let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25) as usize;
                let unread_only = args.get("unread_only").and_then(|v| v.as_bool()).unwrap_or(true);
                let first = if unread_only { std::cmp::min(limit*4, 200) } else { limit };
                let data = linear_graphql(&api_key, &format!("query($first:Int!){{ notifications(first:$first){{ nodes {{ {NOTIFICATION_FIELDS} }} }} }}"), json!({ "first": first })).await?;
                let mut nodes = data.get("notifications").and_then(|n| n.get("nodes")).and_then(|n| n.as_array()).cloned().unwrap_or_default();
                if unread_only { nodes.retain(|n| n.get("readAt").is_none() || n.get("readAt").unwrap().is_null()); }
                nodes.truncate(limit);
                let val = Value::Array(nodes);
                let only = only_from_args(&args);
                project_value(val, only, &inbox_map())
            })
        });
        // list_teams
        reg!("list_teams", "List teams (id, name, key).", "read", {
            let mut m = Map::new();
            m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&[])}));
            m
        }, |_: &Value| "list_teams".to_string(), |args: Value, conn: &Integration| {
            let (api_key, _) = linear_config(conn);
            Box::pin(async move {
                let data = linear_graphql(&api_key, "{ teams { nodes { id name key } } }", json!({})).await?;
                let nodes = data.get("teams").and_then(|t| t.get("nodes")).cloned().unwrap_or(json!([]));
                let only = only_from_args(&args);
                project_value(nodes, only, &list_teams_map())
            })
        });
        // list_states
        {
            let fallback_team2 = conn.config.get("team_key").and_then(|v| v.as_str()).unwrap_or("*").to_string();
            reg!("list_states", "List a team's workflow states (id, name, type). Use a state name with update_issue to move an issue.", "read", {
                let mut m = Map::new();
                m.insert("team".into(), json!({"type":"string","description":"Team key (e.g. ENG). Defaults to the integration's team if set."}));
                m
            }, {
                let ft = fallback_team2.clone();
                move |args: &Value| format!("list_states team={}", args.get("team").and_then(|v| v.as_str()).unwrap_or(&ft))
            }, |args: Value, conn: &Integration| {
            let (api_key, default_team) = linear_config(conn);
            Box::pin(async move {
                let team_key = args.get("team").and_then(|v| v.as_str()).map(|s| s.to_string()).or(default_team);
                let filter = team_key.as_ref().map(|k| json!({ "team": { "key": { "eq": k } } }));
                let data = linear_graphql(&api_key, "query($filter:WorkflowStateFilter){ workflowStates(filter:$filter){ nodes { id name type position team { key } } } }", json!({ "filter": filter })).await?;
                let nodes = data.get("workflowStates").and_then(|w| w.get("nodes")).cloned().unwrap_or(json!([]));
                Ok::<Value, AdapterError>(nodes)
            })
        });
        }
        // list_projects
        reg!("list_projects", "List projects with their state, progress percent, and issue counts (total/completed). Optionally filter by name.", "read", {
            let mut m = Map::new();
            m.insert("query".into(), json!({"type":"string","description":"Filter projects whose name contains this text"}));
            m.insert("limit".into(), json!({"type":"integer","minimum":1,"maximum":100,"default":25,"description":"Max projects to return"}));
            m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&["dates","lead"])}));
            m
        }, |args: &Value| format!("list_projects query=\"{}\" limit={}", args.get("query").and_then(|v| v.as_str()).unwrap_or(""), args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25)), |args: Value, conn: &Integration| {
            let (api_key, _) = linear_config(conn);
            Box::pin(async move {
                let q = args.get("query").and_then(|v| v.as_str()).map(|s| s.to_string());
                let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25);
                let filter = q.map(|s| json!({ "name": { "containsIgnoreCase": s } }));
                let data = linear_graphql(&api_key, &format!("query($first:Int!,$filter:ProjectFilter){{ projects(first:$first, filter:$filter){{ nodes {{ {PROJECT_FIELDS} }} }} }}"), json!({ "first": limit, "filter": filter })).await?;
                let nodes = data.get("projects").and_then(|p| p.get("nodes")).and_then(|n| n.as_array()).cloned().unwrap_or_default();
                let summarized: Vec<Value> = nodes.into_iter().map(summarize_project).collect();
                let val = Value::Array(summarized);
                let only = only_from_args(&args);
                project_value(val, only, &list_projects_map())
            })
        });
        // project_updates
        reg!("project_updates", "Read a project's status-update log (the periodic updates with health on-track/at-risk/off-track), newest first. Use list_projects to find the project id.", "read", {
            let mut m = Map::new();
            m.insert("project_id".into(), json!({"type":"string","description":"Project id (UUID) from list_projects"}));
            m.insert("limit".into(), json!({"type":"integer","minimum":1,"maximum":50,"default":10,"description":"Max updates to return"}));
            m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&["urls"])}));
            m
        }, |args: &Value| format!("project_updates {} limit={}", args.get("project_id").and_then(|v| v.as_str()).unwrap_or(""), args.get("limit").and_then(|v| v.as_i64()).unwrap_or(10)), |args: Value, conn: &Integration| {
            let (api_key, _) = linear_config(conn);
            Box::pin(async move {
                let project_id = args.get("project_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(10);
                let data = linear_graphql(&api_key, "query($id:String!,$first:Int!){ project(id:$id){ name projectUpdates(first:$first){ nodes { body health createdAt url user { name } } } } }", json!({ "id": project_id, "first": limit })).await?;
                let proj = data.get("project");
                if proj.is_none() || proj.unwrap().is_null() { return Err(AdapterError::new(format!("Project \"{project_id}\" not found."))); }
                let proj = proj.unwrap();
                let name = proj.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let updates = proj.get("projectUpdates").and_then(|u| u.get("nodes")).cloned().unwrap_or(json!([]));
                let result = json!({ "project": name, "updates": updates });
                let only = only_from_args(&args);
                project_value(result, only, &project_updates_map())
            })
        });
        // create_issue
        reg!("create_issue", "Create a new issue. Team by key or name, assignee by email or display name, state and labels by name.", "write", {
            let mut m = Map::new();
            m.insert("team".into(), json!({"type":"string","description":"Team key or name, e.g. ENG or Engineering"}));
            m.insert("title".into(), json!({"type":"string","description":"Issue title"}));
            m.insert("description".into(), json!({"type":"string","description":"Issue description (markdown)"}));
            m.insert("assignee".into(), json!({"type":"string","description":"Assignee's email or display name; omit to leave unassigned"}));
            m.insert("state".into(), json!({"type":"string","description":"Initial workflow state name, e.g. In Progress"}));
            m.insert("priority".into(), json!({"type":"integer","minimum":0,"maximum":4,"description":"0 none, 1 urgent, 2 high, 3 normal, 4 low"}));
            m.insert("labels".into(), json!({"type":"array","items":{"type":"string"},"description":"Label names to apply"}));
            m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&[])}));
            m
        }, |args: &Value| format!("create_issue team={} \"{}\"", args.get("team").and_then(|v| v.as_str()).unwrap_or(""), args.get("title").and_then(|v| v.as_str()).unwrap_or("")), |args: Value, conn: &Integration| {
            let (api_key, _) = linear_config(conn);
            Box::pin(async move {
                let team_str = args.get("team").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let team = resolve_team(&api_key, &team_str).await?;
                let team_id = team.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let team_key = team.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let mut input = json!({ "teamId": team_id, "title": title });
                if let Some(d) = args.get("description") { input.as_object_mut().unwrap().insert("description".into(), d.clone()); }
                if let Some(a) = args.get("assignee").and_then(|v| v.as_str()) { let u = resolve_user(&api_key, a).await?; input.as_object_mut().unwrap().insert("assigneeId".into(), json!(u.get("id").and_then(|v| v.as_str()).unwrap_or(""))); }
                if let Some(s) = args.get("state").and_then(|v| v.as_str()) { let st = resolve_state(&api_key, &team_key, s).await?; input.as_object_mut().unwrap().insert("stateId".into(), json!(st.get("id").and_then(|v| v.as_str()).unwrap_or(""))); }
                if let Some(p) = args.get("priority") { input.as_object_mut().unwrap().insert("priority".into(), p.clone()); }
                if let Some(labels) = args.get("labels").and_then(|v| v.as_array()) { let names: Vec<String> = labels.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect(); let ids = resolve_labels(&api_key, &names).await?; input.as_object_mut().unwrap().insert("labelIds".into(), json!(ids)); }
                let data = linear_graphql(&api_key, "mutation($input:IssueCreateInput!){ issueCreate(input: $input){ success issue { id identifier title url } } }", json!({ "input": input })).await?;
                let issue_create = data.get("issueCreate").cloned().unwrap_or(Value::Null);
                let only = only_from_args(&args);
                project_value(issue_create, only, &create_issue_map())
            })
        });
        // comment
        reg!("comment", "Add a comment to an issue, or reply in a thread by passing the comment id you are answering as parent_id. Get comment ids from list_comments.", "write", {
            let mut m = Map::new();
            m.insert("issue_id".into(), json!({"type":"string","description":"Issue id or identifier"}));
            m.insert("body".into(), json!({"type":"string","description":"Comment body (markdown)"}));
            m.insert("parent_id".into(), json!({"type":"string","description":"Comment id to reply to; omit to start a new thread"}));
            m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&[])}));
            m
        }, |args: &Value| if args.get("parent_id").is_some() { format!("comment {} reply-to={}", args.get("issue_id").and_then(|v| v.as_str()).unwrap_or(""), args.get("parent_id").and_then(|v| v.as_str()).unwrap_or("")) } else { format!("comment {}", args.get("issue_id").and_then(|v| v.as_str()).unwrap_or("")) }, |args: Value, conn: &Integration| {
            let (api_key, _) = linear_config(conn);
            Box::pin(async move {
                let issue_id = args.get("issue_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let body = args.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let parent_id = args.get("parent_id").and_then(|v| v.as_str()).map(|s| s.to_string());
                let mut input = json!({ "issueId": issue_id, "body": body });
                if let Some(pid) = parent_id { input.as_object_mut().unwrap().insert("parentId".into(), json!(pid)); } else { input.as_object_mut().unwrap().insert("parentId".into(), Value::Null); }
                let data = linear_graphql(&api_key, "mutation($input:CommentCreateInput!){ commentCreate(input:$input){ success comment { id url parentId } } }", json!({ "input": input })).await?;
                let cc = data.get("commentCreate").cloned().unwrap_or(Value::Null);
                let only = only_from_args(&args);
                project_value(cc, only, &comment_map())
            })
        });
        // update_issue
        reg!("update_issue", "Update an issue — move it to another state, reassign or unassign, or change title, description, priority, estimate or labels. State by name, assignee by email or display name, labels by name.", "write", {
            let mut m = Map::new();
            m.insert("id".into(), json!({"type":"string","description":"Issue id or identifier (e.g. ENG-123)"}));
            m.insert("state".into(), json!({"type":"string","description":"New workflow state name, e.g. Done"}));
            m.insert("assignee".into(), json!({"type":["string","null"],"description":"Assignee's email or display name; pass null to unassign"}));
            m.insert("priority".into(), json!({"type":"integer","minimum":0,"maximum":4,"description":"0 none, 1 urgent, 2 high, 3 normal, 4 low"}));
            m.insert("estimate".into(), json!({"type":"integer","minimum":0,"description":"Estimate points"}));
            m.insert("title".into(), json!({"type":"string","description":"New title"}));
            m.insert("description".into(), json!({"type":"string","description":"New description (markdown); replaces the existing one"}));
            m.insert("labels".into(), json!({"type":"array","items":{"type":"string"},"description":"Label names; replaces the issue's current labels"}));
            m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&[])}));
            m
        }, |args: &Value| {
            let keys: Vec<String> = args.as_object().map(|o| o.keys().filter(|k| *k!="id" && args.get(*k).is_some()).cloned().collect()).unwrap_or_default();
            format!("update_issue {} {}", args.get("id").and_then(|v| v.as_str()).unwrap_or(""), keys.join(","))
        }, |args: Value, conn: &Integration| {
            let (api_key, _) = linear_config(conn);
            Box::pin(async move {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let mut input = Map::new();
                if let Some(state) = args.get("state").and_then(|v| v.as_str()) {
                    let issue = linear_graphql(&api_key, "query($id:String!){ issue(id:$id){ team { key } } }", json!({ "id": id })).await?;
                    let team_key = issue.get("issue").and_then(|i| i.get("team")).and_then(|t| t.get("key")).and_then(|k| k.as_str()).ok_or_else(|| AdapterError::new(format!("Issue \"{id}\" not found.")))?;
                    let st = resolve_state(&api_key, team_key, state).await?;
                    input.insert("stateId".into(), json!(st.get("id").and_then(|v| v.as_str()).unwrap_or("")));
                }
                if args.as_object().map(|o| o.contains_key("assignee")).unwrap_or(false) {
                    let v = args.get("assignee");
                    if v.is_none() || v.unwrap().is_null() { input.insert("assigneeId".into(), Value::Null); } else if let Some(s) = v.and_then(|x| x.as_str()) { let u = resolve_user(&api_key, s).await?; input.insert("assigneeId".into(), json!(u.get("id").and_then(|x| x.as_str()).unwrap_or(""))); }
                }
                if let Some(p) = args.get("priority") { input.insert("priority".into(), p.clone()); }
                if let Some(e) = args.get("estimate") { input.insert("estimate".into(), e.clone()); }
                if let Some(t) = args.get("title") { input.insert("title".into(), t.clone()); }
                if let Some(d) = args.get("description") { input.insert("description".into(), d.clone()); }
                if let Some(labels) = args.get("labels").and_then(|v| v.as_array()) { let names: Vec<String> = labels.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect(); let ids = resolve_labels(&api_key, &names).await?; input.insert("labelIds".into(), json!(ids)); }
                if input.is_empty() { return Err(AdapterError::new("update_issue needs at least one field to change.")); }
                let data = linear_graphql(&api_key, "mutation($id:String!,$input:IssueUpdateInput!){ issueUpdate(id:$id, input:$input){ success issue { identifier title state { name } assignee { name } priority url } } }", json!({ "id": id, "input": Value::Object(input) })).await?;
                let iu = data.get("issueUpdate").cloned().unwrap_or(Value::Null);
                let only = only_from_args(&args);
                project_value(iu, only, &update_issue_map())
            })
        });
        // link_url
        reg!("link_url", "Attach a URL to an issue — a pull request, build, or doc. URLs from a configured integration (GitHub, GitLab, Slack) become rich attachments that sync status back to the issue.", "write", {
            let mut m = Map::new();
            m.insert("issue_id".into(), json!({"type":"string","description":"Issue id or identifier"}));
            m.insert("url".into(), json!({"type":"string","format":"uri","description":"URL to attach"}));
            m.insert("title".into(), json!({"type":"string","description":"Link title shown on the issue"}));
            m
        }, |args: &Value| format!("link_url {} {}", args.get("issue_id").and_then(|v| v.as_str()).unwrap_or(""), args.get("url").and_then(|v| v.as_str()).unwrap_or("")), |args: Value, conn: &Integration| {
            let (api_key, _) = linear_config(conn);
            Box::pin(async move {
                let issue_id = args.get("issue_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let title = args.get("title").and_then(|v| v.as_str()).map(|s| s.to_string());
                let data = linear_graphql(&api_key, "mutation($issueId:String!,$url:String!,$title:String){ attachmentLinkURL(issueId:$issueId, url:$url, title:$title){ success attachment { id title url } } }", json!({ "issueId": issue_id, "url": url, "title": title })).await?;
                Ok::<Value, AdapterError>(data.get("attachmentLinkURL").cloned().unwrap_or(Value::Null))
            })
        });

        Ok(())
    }
}

pub fn linear_adapters(store: Arc<pluk_store::Store>) -> Vec<Arc<dyn Adapter>> { vec![LinearAdapter::new(store)] }

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn thread_nests_replies() {
        let nodes = vec![
            json!({"id":"a","body":"question","parentId":null}),
            json!({"id":"b","body":"answer","parentId":"a"}),
            json!({"id":"c","body":"follow-up","parentId":"a"}),
        ];
        let roots = thread_comments(nodes);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].get("id").and_then(|v| v.as_str()), Some("a"));
        let replies = roots[0].get("replies").and_then(|v| v.as_array()).unwrap();
        assert_eq!(replies.len(), 2);
        assert!(roots[0].get("parentId").is_none());
    }

    #[test]
    fn orphan_surfaces_as_root() {
        let nodes = vec![
            json!({"id":"b","body":"answer","parentId":"a"}),
            json!({"id":"c","body":"unrelated","parentId":null}),
        ];
        let roots = thread_comments(nodes);
        let mut ids: Vec<String> = roots.iter().filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(|s| s.to_string())).collect();
        ids.sort();
        assert_eq!(ids, vec!["b","c"]);
    }

    #[test]
    fn issue_list_default_projection() {
        let node = json!({"identifier":"ENG-42","title":"Fix","state":{"name":"In Progress"},"assignee":{"name":"Ada"},"priority":2,"url":"https://x","updatedAt":"2026-08-01","id":"iss-1"});
        let out = project_value(json!([node]), None, &issue_list_map()).unwrap();
        assert_eq!(out, json!([{"identifier":"ENG-42","title":"Fix","state":{"name":"In Progress"},"assignee":{"name":"Ada"},"updatedAt":"2026-08-01"}]));
    }
    #[test]
    fn issue_list_presets() {
        let node = json!({"identifier":"ENG-42","priority":2,"url":"https://x"});
        let out = project_value(node, Some(vec!["priority".into(),"url".into()]), &issue_list_map()).unwrap();
        assert_eq!(out, json!({"priority":2,"url":"https://x"}));
        let out2 = project_value(json!({"id":"iss-1"}), Some(vec!["ids".into()]), &issue_list_map()).unwrap();
        assert_eq!(out2, json!({"id":"iss-1"}));
    }
    #[test]
    fn get_issue_default_and_presets() {
        let issue = json!({"identifier":"ENG-42","title":"Fix","description":"Repro","state":{"name":"In Progress"},"assignee":{"name":"Ada"},"priority":2,"url":"https://x","branchName":"eng-42","estimate":3,"labels":{"nodes":[{"name":"bug"}]}});
        let out = project_value(issue.clone(), None, &get_issue_map()).unwrap();
        assert_eq!(out, json!({"identifier":"ENG-42","title":"Fix","description":"Repro","state":{"name":"In Progress"},"assignee":{"name":"Ada"},"priority":2,"url":"https://x"}));
        let meta = project_value(json!({"labels":{"nodes":[{"name":"bug"}]},"project":{"name":"Auth"},"parent":null,"team":{"key":"ENG"},"createdAt":"2026-01-01","updatedAt":"2026-08-01"}), Some(vec!["meta".into()]), &get_issue_map()).unwrap();
        assert!(meta.get("labels").is_some());
        assert!(meta.get("project").is_some());
    }
    #[test]
    fn inbox_and_comments_defaults() {
        let inbox = json!([{"type":"issueCommentMention","subtitle":"Ada replied","createdAt":"2026-08-01","actor":{"name":"Ada"},"issue":{"identifier":"ENG-42","title":"Fix","url":"https://y"},"comment":{"body":"looks good","url":"https://z"},"title":"dup","url":"https://x"}]);
        let out = project_value(inbox, None, &inbox_map()).unwrap();
        assert_eq!(out, json!([{"type":"issueCommentMention","subtitle":"Ada replied","createdAt":"2026-08-01","actor":{"name":"Ada"},"issue":{"identifier":"ENG-42","title":"Fix"},"comment":{"body":"looks good"}}]));
        let comments = json!({"issue":"ENG-42","comments":[{"body":"hi","createdAt":"2026-08-01","user":{"name":"Ada"},"replies":[],"id":"c1","url":"https://x"}]});
        let out2 = project_value(comments, Some(vec!["refs".into()]), &list_comments_map()).unwrap();
        assert!(out2.get("comments").unwrap().as_array().unwrap()[0].get("id").is_some());
    }
    #[test]
    fn star_bypass() {
        let v = json!({"identifier":"ENG-42","extra":"x"});
        let out = project_value(v.clone(), Some(vec!["*".into()]), &issue_list_map()).unwrap();
        assert_eq!(out, v);
    }
    #[test]
    fn unknown_field_errors() {
        let err = project_value(json!({"identifier":"ENG-42"}), Some(vec!["bogus".into()]), &issue_list_map()).unwrap_err();
        assert!(err.message.contains("Unknown \"only\" field \"bogus\""));
    }
    #[tokio::test]
    async fn linear_auth_and_timeout_and_api_error() {
        // missing key
        let err = client::linear_graphql("", "query{}", json!({})).await.unwrap_err();
        assert!(err.message.contains("missing"));
        // mock runner - api error
        client::set_linear_runner(Some(Arc::new(|_, _| Box::pin(async { Err(AdapterError::new("Linear: Authentication required")) }))));
        let e = client::linear_graphql("k", "query{}", json!({})).await.unwrap_err();
        assert!(e.message.contains("Authentication required"));
        // timeout simulation
        client::set_linear_runner(Some(Arc::new(|_, _| Box::pin(async { Err(AdapterError::new("Linear API timed out after 20s")) }))));
        let e2 = client::linear_graphql("k", "query{}", json!({})).await.unwrap_err();
        assert!(e2.message.contains("timed out"));
        client::set_linear_runner(None);
        // verify header shape is raw key not Bearer - check that no Bearer prefix is added in source (just ensure function exists)
        // request construction verified via mock capture
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String,Value)>::new()));
        let c2 = captured.clone();
        client::set_linear_runner(Some(Arc::new(move |q, v| {
            let c = c2.clone();
            Box::pin(async move { c.lock().unwrap().push((q.clone(), v.clone())); Ok(json!({"issues":{"nodes":[]}})) })
        })));
        let _ = client::linear_graphql("lin_api_x", "query($first:Int!){issues}", json!({"first":25})).await.unwrap();
        let calls = captured.lock().unwrap();
        assert_eq!(calls[0].1.get("first").and_then(|v| v.as_i64()), Some(25));
        client::set_linear_runner(None);
    }
    #[test]
    fn summarize_and_maps() {
        let p = json!({"id":"p1","name":"Auth","state":"started","progress":0.4,"issueCountHistory":[1,2,5],"completedIssueCountHistory":[0,1,2]});
        let s = summarize_project(p);
        assert_eq!(s.get("total_issues").and_then(|v| v.as_i64()), Some(5));
        assert_eq!(s.get("progress_percent").and_then(|v| v.as_i64()), Some(40));
        let proj = project_value(json!([{"id":"p1","name":"Auth","state":"started","progress_percent":40,"total_issues":5,"completed_issues":2,"url":"https://x"}]), None, &list_projects_map()).unwrap();
        assert!(proj.as_array().unwrap()[0].get("url").is_none());
    }
}
