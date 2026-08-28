pub mod client;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value, json};

use pluk_store::Integration;

use crate::adapter::{Adapter, PolicyKind};
use crate::config_field::{ConfigField, FieldType};
use crate::error::AdapterError;
use crate::gate::{CallTarget, GateMeta, GateOpts, Outcome, RunOutcome, run_gated};
use crate::instructions::{InstructionParts, build_instructions};
use crate::projection::{FieldMap, Preset, apply_only};
use crate::tool_host::{ToolHandler, ToolHost, ToolRegistration, object_schema};
use crate::tool_spec::ToolSpec;

use client::{SentryConfig, sentry_config_from, sentry_request, sentry_request_bytes};

const AGENT_HINT: &str = "Use this for Sentry error monitoring and logs — list/read issues, pull latest issue events, download event attachments to file paths you can open, and query structured logs. Start with list_issues + latest_event for issue debugging, list_event_attachments to see what an event captured, or query_logs for log search.";

fn sentry_fields() -> Vec<ConfigField> {
    vec![
        ConfigField::new("auth_token", "Auth Token", FieldType::Password)
            .group("Auth")
            .required()
            .secret()
            .placeholder("sntrys_… or a personal token"),
        ConfigField::new("org_slug", "Organization", FieldType::Text)
            .group("Scope")
            .required()
            .placeholder("my-org"),
        ConfigField::new("project_slug", "Default Project", FieldType::Text)
            .group("Scope")
            .placeholder("backend (optional, scopes list_issues)"),
        ConfigField::new("base_url", "Base URL", FieldType::Text)
            .group("Connection")
            .default_value(&json!("https://sentry.io"))
            .placeholder("https://sentry.io (or self-hosted)"),
    ]
}

// -- attachment helpers
fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("PLUK_DATA_DIR") {
        PathBuf::from(dir)
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".pluk")
    } else {
        PathBuf::from(".pluk")
    }
}
fn attachment_cache_dir() -> PathBuf {
    data_dir().join("sentry-attachments")
}
fn safe_part(v: &str) -> String {
    let clean: String = v
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() || clean == "." || clean == ".." {
        "_".to_string()
    } else {
        clean
    }
}
fn attachment_name(name: Option<&str>, id: &str) -> String {
    safe_part(name.unwrap_or(&format!("attachment-{id}")))
}
fn attachment_size(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        if n >= 0 { Some(n) } else { None }
    } else {
        v.as_u64().map(|n| n as i64)
    }
}
fn size_warning(listed: Option<i64>, actual: i64) -> Option<String> {
    match listed {
        Some(l) if l == actual => None,
        Some(l) if l > actual => Some(format!(
            "Saved {actual} bytes, fewer than the {l} Sentry listed — the file may be incomplete."
        )),
        Some(l) => Some(format!("Saved {actual} bytes; Sentry listed {l}.")),
        None => None,
    }
}

async fn download_attachment(
    cfg: &SentryConfig,
    project: &str,
    event_id: &str,
    att_id: &str,
    name: Option<&str>,
    listed: Option<i64>,
) -> Result<(PathBuf, Option<String>), AdapterError> {
    let dir = attachment_cache_dir()
        .join(safe_part(project))
        .join(safe_part(event_id));
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AdapterError::new(e.to_string()))?;
    let path = dir.join(format!(
        "{}-{}",
        safe_part(att_id),
        attachment_name(name, att_id)
    ));
    // check cache
    if let Ok(meta) = tokio::fs::metadata(&path).await
        && meta.is_file()
        && (meta.len() > 0 || listed == Some(0))
    {
        let w = size_warning(listed, meta.len() as i64);
        return Ok((path, w));
    }
    let bytes = sentry_request_bytes(
        cfg,
        "GET",
        &format!(
            "/projects/{}/{}/events/{}/attachments/{}/",
            urlencoding::encode(&cfg.org),
            urlencoding::encode(project),
            urlencoding::encode(event_id),
            urlencoding::encode(att_id)
        ),
        Some(json!({"download":1})),
    )
    .await?;
    if bytes.bytes.is_empty() && listed != Some(0) {
        return Err(AdapterError::new(format!(
            "Attachment {att_id} downloaded empty — Sentry returned no bytes."
        )));
    }
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    tokio::fs::write(&tmp, &bytes.bytes)
        .await
        .map_err(|e| AdapterError::new(e.to_string()))?;
    tokio::fs::rename(&tmp, &path)
        .await
        .map_err(|e| AdapterError::new(e.to_string()))?;
    let _ = tokio::fs::remove_file(&tmp).await;
    let w = size_warning(listed, bytes.bytes.len() as i64);
    Ok((path, w))
}

// -- field maps
fn list_projects_map() -> FieldMap {
    let mut m = FieldMap::new(
        &[
            "slug",
            "name",
            "platform",
            "team",
            "environments",
            "access",
            "features",
            "teams",
            "id",
            "isBookmarked",
            "isMember",
            "hasAccess",
            "dateCreated",
            "firstEvent",
            "firstTransactionEvent",
            "platforms",
            "latestRelease",
            "latestDeploys",
        ],
        &["slug", "name", "platform", "team.slug", "environments"],
    );
    m = m.with_preset(
        "deploys",
        Preset::paths(&["latestRelease", "latestDeploys"]),
    );
    m = m.with_preset(
        "access",
        Preset::paths(&["access", "hasAccess", "isMember"]),
    );
    m = m.with_preset(
        "capabilities",
        Preset::reduce(|item| {
            let mut out = Map::new();
            if let Some(f) = item.get("features") {
                out.insert("features".into(), f.clone());
            }
            if let Some(obj) = item.as_object() {
                for (k, v) in obj {
                    if k.starts_with("has") {
                        out.insert(k.clone(), v.clone());
                    }
                }
            }
            out
        }),
    );
    m
}
fn list_issues_map() -> FieldMap {
    FieldMap::new(
        &[
            "shortId",
            "title",
            "culprit",
            "level",
            "status",
            "priority",
            "count",
            "userCount",
            "firstSeen",
            "lastSeen",
            "project",
            "stats",
            "lifetime",
            "metadata",
            "annotations",
            "permalink",
            "id",
            "shareId",
            "statusDetails",
            "substatus",
            "isPublic",
            "platform",
            "type",
            "numComments",
            "assignedTo",
            "isBookmarked",
            "isSubscribed",
            "subscriptionDetails",
            "hasSeen",
            "issueType",
            "issueCategory",
            "priorityLockedAt",
            "seerFixabilityScore",
            "seerAutofixLastTriggered",
            "isUnhandled",
            "filtered",
        ],
        &[
            "shortId",
            "title",
            "culprit",
            "level",
            "status",
            "priority",
            "count",
            "userCount",
            "firstSeen",
            "lastSeen",
            "project.slug",
        ],
    )
    .with_preset("stats", Preset::paths(&["stats", "lifetime"]))
    .with_preset(
        "triage",
        Preset::paths(&[
            "assignedTo",
            "isBookmarked",
            "isSubscribed",
            "hasSeen",
            "numComments",
            "annotations",
        ]),
    )
    .with_preset("links", Preset::paths(&["permalink", "id"]))
    .with_preset(
        "meta",
        Preset::paths(&["metadata", "issueType", "issueCategory", "substatus"]),
    )
}
fn get_issue_map() -> FieldMap {
    FieldMap::new(
        &[
            "shortId",
            "title",
            "culprit",
            "level",
            "status",
            "substatus",
            "priority",
            "count",
            "userCount",
            "firstSeen",
            "lastSeen",
            "isUnhandled",
            "permalink",
            "project",
            "metadata",
            "stats",
            "activity",
            "tags",
            "seenBy",
            "participants",
            "pluginActions",
            "pluginIssues",
            "pluginContexts",
            "userReportCount",
            "firstRelease",
            "lastRelease",
            "id",
            "shareId",
            "statusDetails",
            "isPublic",
            "isBookmarked",
            "isSubscribed",
            "subscriptionDetails",
            "hasSeen",
            "issueType",
            "issueCategory",
            "priorityLockedAt",
            "seerFixabilityScore",
            "seerAutofixLastTriggered",
        ],
        &[
            "shortId",
            "title",
            "culprit",
            "level",
            "status",
            "substatus",
            "priority",
            "count",
            "userCount",
            "firstSeen",
            "lastSeen",
            "isUnhandled",
            "permalink",
            "project.slug",
            "metadata.type",
            "metadata.value",
        ],
    )
    .with_preset("stats", Preset::paths(&["stats"]))
    .with_preset("tags", Preset::paths(&["tags"]))
    .with_preset(
        "activity",
        Preset::paths(&["activity", "seenBy", "participants"]),
    )
    .with_preset("releases", Preset::paths(&["firstRelease", "lastRelease"]))
}
fn list_event_attachments_map() -> FieldMap {
    FieldMap::new(
        &[
            "id",
            "name",
            "mimetype",
            "dateCreated",
            "project",
            "event_id",
            "size",
            "path",
            "warning",
            "error",
        ],
        &[
            "name", "size", "mimetype", "path", "event_id", "warning", "error",
        ],
    )
}
fn latest_event_map() -> FieldMap {
    FieldMap::new(
        &[
            "eventID",
            "dateCreated",
            "title",
            "culprit",
            "message",
            "tags",
            "contexts",
            "user",
            "packages",
            "_meta",
            "groupingConfig",
            "fingerprints",
            "breadcrumbs",
            "exception",
        ],
        &[
            "eventID",
            "dateCreated",
            "title",
            "culprit",
            "message",
            "tags",
            "contexts.runtime",
            "contexts.os",
            "contexts.trace",
            "exception",
        ],
    )
    .with_preset("breadcrumbs", Preset::paths(&["breadcrumbs"]))
    .with_preset("packages", Preset::paths(&["packages"]))
    .with_preset("request", Preset::paths(&["contexts.response", "user"]))
    .with_preset(
        "grouping",
        Preset::paths(&["groupingConfig", "fingerprints"]),
    )
    .with_preset("raw", Preset::paths(&["_meta"]))
    .with_preset("frames.all", Preset::paths(&["exception"]))
    .with_preset("frames.context", Preset::paths(&["exception"]))
    .with_preset("frames.vars", Preset::paths(&["exception"]))
    .with_preset("frames.full", Preset::paths(&["exception"]))
}

// frame reduction
fn find_entry(entries: &Value, ty: &str) -> Option<Value> {
    entries
        .as_array()?
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some(ty))
        .cloned()
}
fn reduce_frame(
    frame: &Map<String, Value>,
    all: bool,
    ctx: bool,
    vars: bool,
    full: bool,
) -> Map<String, Value> {
    let mut out = Map::new();
    for k in ["filename", "function", "lineNo", "module"] {
        if let Some(v) = frame.get(k) {
            out.insert(k.into(), v.clone());
        }
    }
    if (ctx || full)
        && let Some(v) = frame.get("context")
    {
        out.insert("context".into(), v.clone());
    }
    if (vars || full)
        && let Some(v) = frame.get("vars")
    {
        out.insert("vars".into(), v.clone());
    }
    let _ = all;
    out
}
fn reduce_exception(entries: &Value, opts: (bool, bool, bool, bool)) -> Value {
    let (all, ctx, vars, full) = opts;
    let data = find_entry(entries, "exception")
        .and_then(|e| e.get("data").cloned())
        .unwrap_or(Value::Null);
    let values = data
        .get("values")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mapped: Vec<Value> = values
        .into_iter()
        .map(|mut v| {
            if full {
                return v;
            }
            let frames = v
                .get("stacktrace")
                .and_then(|s| s.get("frames"))
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default();
            let kept: Vec<Value> = if all {
                frames.clone()
            } else {
                frames
                    .into_iter()
                    .filter(|f| f.get("inApp").and_then(|b| b.as_bool()).unwrap_or(false))
                    .collect()
            };
            let reduced: Vec<Value> = kept
                .iter()
                .map(|f| {
                    let m = f.as_object().cloned().unwrap_or_default();
                    Value::Object(reduce_frame(&m, all, ctx, vars, full))
                })
                .collect();
            if let Some(obj) = v.as_object_mut() {
                obj.insert("frames".into(), Value::Array(reduced));
                obj.remove("stacktrace");
                // keep type,value,module
                let mut out = Map::new();
                for k in ["type", "value", "module", "frames"] {
                    if let Some(val) = obj.get(k) {
                        out.insert(k.into(), val.clone());
                    }
                }
                Value::Object(out)
            } else {
                v
            }
        })
        .collect();
    Value::Array(mapped)
}
fn frame_opts(only: &Option<Vec<String>>) -> (bool, bool, bool, bool) {
    let set: std::collections::HashSet<String> =
        only.clone().unwrap_or_default().into_iter().collect();
    let full = set.contains("frames.full");
    let all = full || set.contains("frames.all");
    let ctx = full || set.contains("frames.context");
    let vars = full || set.contains("frames.vars");
    (all, ctx, vars, full)
}
fn only_from_args(args: &Value) -> Option<Vec<String>> {
    args.get("only").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect()
    })
}
fn project_value(
    data: Value,
    only: Option<Vec<String>>,
    map: &FieldMap,
) -> Result<Value, AdapterError> {
    apply_only(&data, only.as_ref(), map).map_err(|e| AdapterError::new(e.to_string()))
}
async fn resolve_issue_project(
    cfg: &SentryConfig,
    issue_id: &str,
) -> Result<Option<String>, AdapterError> {
    let v = sentry_request(
        cfg,
        "GET",
        &format!(
            "/organizations/{}/issues/{}/",
            urlencoding::encode(&cfg.org),
            urlencoding::encode(issue_id)
        ),
        None,
        None,
    )
    .await?;
    Ok(v.get("project")
        .and_then(|p| p.get("slug"))
        .and_then(|s| s.as_str())
        .map(|s| s.to_string()))
}
async fn resolve_latest_event_id(
    cfg: &SentryConfig,
    issue_id: &str,
) -> Result<String, AdapterError> {
    let v = sentry_request(
        cfg,
        "GET",
        &format!("/issues/{}/events/latest/", urlencoding::encode(issue_id)),
        None,
        None,
    )
    .await?;
    v.get("eventID")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| AdapterError::new(format!("No event found for issue {issue_id}")))
}

pub struct SentryAdapter {
    store: Arc<pluk_store::Store>,
}
impl SentryAdapter {
    pub fn new(store: Arc<pluk_store::Store>) -> Arc<Self> {
        Arc::new(Self { store })
    }
}

#[async_trait]
impl Adapter for SentryAdapter {
    fn id(&self) -> &str {
        "sentry"
    }
    fn label(&self) -> &str {
        "Sentry"
    }
    fn category(&self) -> &str {
        "observability"
    }
    fn policy_kind(&self) -> PolicyKind {
        PolicyKind::Action
    }
    fn agent_hint(&self) -> &str {
        AGENT_HINT
    }
    fn tool_specs(&self) -> &[ToolSpec] {
        static SPECS: std::sync::OnceLock<Vec<ToolSpec>> = std::sync::OnceLock::new();
        SPECS.get_or_init(|| vec![
            ToolSpec::new("list_projects","List projects in the organization (slug, name, platform).","read"),
            ToolSpec::new("list_issues","List issues, newest first. Scoped to the default project if set, else all projects.","read"),
            ToolSpec::new("get_issue","Get a single issue by its id or short id (e.g. BACKEND-1A)","read"),
            ToolSpec::new("latest_event","Get the latest event for an issue, including the stacktrace and tags","read"),
            ToolSpec::new("list_event_attachments","List an event's attachments and download each one to a local file path you can open. Defaults to the issue's latest event unless event_id is given.","read"),
            ToolSpec::new("read_event_attachment","Download one attachment and return a local file path to open. Attachment content is never returned in the tool response.","read"),
            ToolSpec::new("list_events","List recent error events for a project, optionally with full event bodies.","read").with_default_enabled(false),
            ToolSpec::new("query_logs","Query Sentry structured logs using Explore's logs dataset.","read").with_default_enabled(false),
            ToolSpec::new("update_issue","Resolve, ignore, or reopen an issue (write).","write"),
        ])
    }
    fn config_fields(&self) -> &[ConfigField] {
        static F: std::sync::OnceLock<Vec<ConfigField>> = std::sync::OnceLock::new();
        F.get_or_init(sentry_fields)
    }
    async fn test_connection(&self, conn: &Integration) -> Result<(), AdapterError> {
        let cfg = sentry_config_from(conn);
        sentry_request(
            &cfg,
            "GET",
            &format!("/organizations/{}/", urlencoding::encode(&cfg.org)),
            None,
            None,
        )
        .await
        .map(|_| ())
    }
    fn instructions(&self, conn: &Integration) -> String {
        let enabled: Vec<&str> = self
            .tool_specs()
            .iter()
            .filter(|t| {
                pluk_policy::tool_gate(conn.query_policy.as_deref())
                    .enabled(&t.name, t.default_enabled)
            })
            .map(|t| t.name.as_str())
            .collect();
        let policy = if enabled.is_empty() {
            "No tools are enabled on this integration.".into()
        } else {
            format!("Enabled tools: {}.", enabled.join(", "))
        };
        build_instructions(&conn.name, conn.environment, InstructionParts{kind:"Sentry".into(), access:"Read projects, issues, event stack traces, project error events, and structured logs; resolve or ignore issues when write is permitted. Every action is policy-checked and recorded in the activity log.".into(), policy:Some(policy), start:None, hint:Some(AGENT_HINT.into())})
    }
    fn register(
        &self,
        host: &mut dyn ToolHost,
        conn: &Integration,
        _owner: &str,
    ) -> Result<(), AdapterError> {
        let store = self.store.clone();
        macro_rules! reg {
            ($name:expr,$desc:expr,$cat:expr,$schema:expr,$detail:expr,$body:expr) => {{
                let store = store.clone();
                let conn = conn.clone();
                let handler: ToolHandler = Arc::new(move |args: Value| {
                    let store = store.clone();
                    let conn = conn.clone();
                    let detail = $detail(&args);
                    let meta = GateMeta::new($cat, $name, detail);
                    let target = CallTarget::from(&conn);
                    Box::pin(async move {
                        run_gated(
                            &store,
                            &target,
                            meta,
                            |_| async {
                                let out = $body(args, &conn).await?;
                                let text = match &out {
                                    Value::String(s) => s.clone(),
                                    _ => serde_json::to_string_pretty(&out).unwrap_or("{}".into()),
                                };
                                let rows = match &out {
                                    Value::Array(a) => a.clone(),
                                    o => vec![o.clone()],
                                };
                                Ok(Outcome::Ran(RunOutcome {
                                    text: text.clone(),
                                    response_text: Some(text),
                                    result: Some(pluk_store::QueryResult {
                                        fields: vec![],
                                        rows,
                                    }),
                                    ..Default::default()
                                }))
                            },
                            GateOpts::default(),
                        )
                        .await
                    })
                });
                let props = $schema;
                let schema = if props.is_empty() {
                    Map::new()
                } else {
                    object_schema(props, &[])
                };
                host.register_tool(
                    ToolRegistration {
                        name: $name.into(),
                        description: $desc.into(),
                        input_schema: schema,
                        annotations: Map::new(),
                    },
                    handler,
                );
            }};
        }
        // list_projects
        reg!(
            "list_projects",
            "List projects in the organization (slug, name, platform).",
            "read",
            {
                let mut m = Map::new();
                m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&["deploys","access","capabilities"])}));
                m
            },
            |_: &Value| "list_projects".to_string(),
            |args: Value, conn: &Integration| {
                let cfg = sentry_config_from(conn);
                Box::pin(async move {
                    let v = sentry_request(
                        &cfg,
                        "GET",
                        &format!("/organizations/{}/projects/", urlencoding::encode(&cfg.org)),
                        None,
                        None,
                    )
                    .await?;
                    let only = only_from_args(&args);
                    project_value(v, only, &list_projects_map())
                })
            }
        );
        // list_issues
        {
            let fallback_proj = conn
                .config
                .get("project_slug")
                .and_then(|v| v.as_str())
                .unwrap_or("*")
                .to_string();
            reg!(
                "list_issues",
                "List issues, newest first. Scoped to the default project if set, else all projects.",
                "read",
                {
                    let mut m = Map::new();
                    m.insert("query".into(), json!({"type":"string","description":"Sentry search query, e.g. \"is:unresolved level:error\""}));
                    m.insert("project".into(), json!({"type":"string","description":"Project slug. Defaults to the integration's project if set."}));
                    m.insert("period".into(), json!({"type":"string","default":"14d","description":"Stats period, e.g. 24h, 14d, 90d"}));
                    m.insert("limit".into(), json!({"type":"integer","minimum":1,"maximum":100,"default":25,"description":"Max issues to return"}));
                    m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&["stats","triage","links","meta"])}));
                    m
                },
                {
                    let fp = fallback_proj.clone();
                    move |args: &Value| {
                        format!(
                            "list_issues project={} query=\"{}\" period={} limit={}",
                            args.get("project").and_then(|v| v.as_str()).unwrap_or(&fp),
                            args.get("query").and_then(|v| v.as_str()).unwrap_or(""),
                            args.get("period").and_then(|v| v.as_str()).unwrap_or("14d"),
                            args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25)
                        )
                    }
                },
                |args: Value, conn: &Integration| {
                    let cfg = sentry_config_from(conn);
                    Box::pin(async move {
                        let proj = args
                            .get("project")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .or(cfg.project.clone());
                        let query = args
                            .get("query")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let period = args
                            .get("period")
                            .and_then(|v| v.as_str())
                            .unwrap_or("14d")
                            .to_string();
                        let limit =
                            args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25) as usize;
                        let v = if let Some(p) = proj {
                            sentry_request(
                                &cfg,
                                "GET",
                                &format!(
                                    "/projects/{}/{}/issues/",
                                    urlencoding::encode(&cfg.org),
                                    urlencoding::encode(&p)
                                ),
                                Some(json!({"query":query,"statsPeriod":period})),
                                None,
                            )
                            .await?
                        } else {
                            sentry_request(
                                &cfg,
                                "GET",
                                &format!(
                                    "/organizations/{}/issues/",
                                    urlencoding::encode(&cfg.org)
                                ),
                                Some(json!({"query":query,"statsPeriod":period,"project":"-1"})),
                                None,
                            )
                            .await?
                        };
                        let mut arr = v;
                        if let Some(a) = arr.as_array() {
                            let mut c = a.clone();
                            c.truncate(limit);
                            arr = Value::Array(c);
                        }
                        let only = only_from_args(&args);
                        project_value(arr, only, &list_issues_map())
                    })
                }
            );
        }
        // get_issue
        reg!(
            "get_issue",
            "Get a single issue by its id or short id (e.g. BACKEND-1A)",
            "read",
            {
                let mut m = Map::new();
                m.insert(
                    "id".into(),
                    json!({"type":"string","description":"Issue id (numeric) or short id"}),
                );
                m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&["stats","tags","activity","releases"])}));
                m
            },
            |args: &Value| format!(
                "get_issue {}",
                args.get("id").and_then(|v| v.as_str()).unwrap_or("")
            ),
            |args: Value, conn: &Integration| {
                let cfg = sentry_config_from(conn);
                Box::pin(async move {
                    let id = args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let v = sentry_request(
                        &cfg,
                        "GET",
                        &format!(
                            "/organizations/{}/issues/{}/",
                            urlencoding::encode(&cfg.org),
                            urlencoding::encode(&id)
                        ),
                        None,
                        None,
                    )
                    .await?;
                    let only = only_from_args(&args);
                    project_value(v, only, &get_issue_map())
                })
            }
        );
        // latest_event
        reg!(
            "latest_event",
            "Get the latest event for an issue, including the stacktrace and tags",
            "read",
            {
                let mut m = Map::new();
                m.insert(
                    "id".into(),
                    json!({"type":"string","description":"Issue id (numeric) or short id"}),
                );
                m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&["frames.all","frames.context","frames.vars","frames.full","breadcrumbs","packages","request","grouping","raw"])}));
                m
            },
            |args: &Value| format!(
                "latest_event {}",
                args.get("id").and_then(|v| v.as_str()).unwrap_or("")
            ),
            |args: Value, conn: &Integration| {
                let cfg = sentry_config_from(conn);
                Box::pin(async move {
                    let id = args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let raw = sentry_request(
                        &cfg,
                        "GET",
                        &format!("/issues/{}/events/latest/", urlencoding::encode(&id)),
                        None,
                        None,
                    )
                    .await?;
                    let only = only_from_args(&args);
                    if only
                        .as_ref()
                        .map(|o| o.iter().any(|x| x == "*"))
                        .unwrap_or(false)
                    {
                        return Ok(raw);
                    }
                    let opts = frame_opts(&only);
                    // derived object
                    let mut derived = Map::new();
                    for k in [
                        "eventID",
                        "dateCreated",
                        "title",
                        "culprit",
                        "message",
                        "tags",
                        "contexts",
                        "user",
                        "packages",
                        "_meta",
                        "groupingConfig",
                        "fingerprints",
                    ] {
                        if let Some(v) = raw.get(k) {
                            derived.insert(k.into(), v.clone());
                        }
                    }
                    // breadcrumbs
                    if let Some(b) =
                        find_entry(raw.get("entries").unwrap_or(&Value::Null), "breadcrumbs")
                            .and_then(|e| e.get("data").cloned())
                    {
                        derived.insert("breadcrumbs".into(), b);
                    }
                    let entries = raw.get("entries").cloned().unwrap_or(Value::Null);
                    derived.insert("exception".into(), reduce_exception(&entries, opts));
                    let val = Value::Object(derived);
                    project_value(val, only, &latest_event_map())
                })
            }
        );
        // list_event_attachments
        reg!(
            "list_event_attachments",
            "List an event's attachments and download each one to a local file path you can open. Defaults to the issue's latest event unless event_id is given.",
            "read",
            {
                let mut m = Map::new();
                m.insert(
                    "id".into(),
                    json!({"type":"string","description":"Issue id (numeric) or short id"}),
                );
                m.insert("event_id".into(), json!({"type":"string","description":"Event id (hex). Omit to use the issue's latest event."}));
                m.insert("project".into(), json!({"type":"string","description":"Project slug. Defaults to the integration's project, else derived from the issue."}));
                m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&[])}));
                m
            },
            |args: &Value| format!(
                "list_event_attachments {}{}",
                args.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                args.get("event_id")
                    .and_then(|v| v.as_str())
                    .map(|s| format!(" event={s}"))
                    .unwrap_or_default()
            ),
            |args: Value, conn: &Integration| {
                let cfg = sentry_config_from(conn);
                Box::pin(async move {
                    let issue_id = args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let project = args
                        .get("project")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or(cfg.project.clone())
                        .or(resolve_issue_project(&cfg, &issue_id).await?);
                    let project=project.ok_or_else(|| AdapterError::new("No project given. Pass project or set project_slug in the integration config."))?;
                    let event_id = if let Some(e) = args.get("event_id").and_then(|v| v.as_str()) {
                        e.to_string()
                    } else {
                        resolve_latest_event_id(&cfg, &issue_id).await?
                    };
                    let attachments = sentry_request(
                        &cfg,
                        "GET",
                        &format!(
                            "/projects/{}/{}/events/{}/attachments/",
                            urlencoding::encode(&cfg.org),
                            urlencoding::encode(&project),
                            urlencoding::encode(&event_id)
                        ),
                        None,
                        None,
                    )
                    .await?;
                    let arr = attachments.as_array().cloned().unwrap_or_default();
                    let mut results: Vec<Value> = Vec::new();
                    for att in arr {
                        let id = att
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = att
                            .get("name")
                            .and_then(|v| v.as_str())
                            .or(att.get("filename").and_then(|v| v.as_str()))
                            .unwrap_or(&format!("attachment-{id}"))
                            .to_string();
                        let size = att.get("size").and_then(attachment_size);
                        let mimetype = att
                            .get("mimetype")
                            .or(att.get("mime_type"))
                            .cloned()
                            .unwrap_or(Value::Null);
                        let date = att.get("dateCreated").cloned().unwrap_or(Value::Null);
                        let mut meta = Map::new();
                        meta.insert("id".into(), json!(id));
                        meta.insert("name".into(), json!(name.clone()));
                        meta.insert("mimetype".into(), mimetype);
                        meta.insert("dateCreated".into(), date);
                        meta.insert("project".into(), json!(project));
                        meta.insert("event_id".into(), json!(event_id));
                        // attempt download
                        match download_attachment(&cfg, &project, &event_id, &id, Some(&name), size)
                            .await
                        {
                            Ok((path, warning)) => {
                                let sz = tokio::fs::metadata(&path)
                                    .await
                                    .map(|m| m.len() as i64)
                                    .unwrap_or(0);
                                meta.insert("size".into(), json!(sz));
                                meta.insert(
                                    "path".into(),
                                    json!(path.to_string_lossy().to_string()),
                                );
                                if let Some(w) = warning {
                                    meta.insert("warning".into(), json!(w));
                                }
                            }
                            Err(e) => {
                                meta.insert(
                                    "size".into(),
                                    size.map(|s| json!(s)).unwrap_or(Value::Null),
                                );
                                meta.insert("path".into(), Value::Null);
                                meta.insert("error".into(), json!(e.message));
                            }
                        }
                        results.push(Value::Object(meta));
                    }
                    let val = Value::Array(results);
                    let only = only_from_args(&args);
                    project_value(val, only, &list_event_attachments_map())
                })
            }
        );
        // read_event_attachment
        reg!(
            "read_event_attachment",
            "Download one attachment and return a local file path to open. Attachment content is never returned in the tool response.",
            "read",
            {
                let mut m = Map::new();
                m.insert("project".into(), json!({"type":"string","description":"Project slug. Defaults to the integration's project."}));
                m.insert("event_id".into(), json!({"type":"string","description":"Event id (hex) — returned by list_event_attachments."}));
                m.insert("attachment_id".into(), json!({"type":"string","description":"Attachment id — returned by list_event_attachments."}));
                m.insert("name".into(), json!({"type":"string","description":"File name returned by list_event_attachments."}));
                m.insert("size".into(), json!({"type":"integer","minimum":0,"description":"Attachment size in bytes returned by list_event_attachments."}));
                m
            },
            |args: &Value| format!(
                "read_event_attachment {} event={}",
                args.get("attachment_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
                args.get("event_id").and_then(|v| v.as_str()).unwrap_or("")
            ),
            |args: Value, conn: &Integration| {
                let cfg = sentry_config_from(conn);
                Box::pin(async move {
                    let project=args.get("project").and_then(|v|v.as_str()).map(|s|s.to_string()).or(cfg.project.clone()).ok_or_else(|| AdapterError::new("No project given. Pass project or set project_slug in the integration config."))?;
                    let event_id = args
                        .get("event_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let att_id = args
                        .get("attachment_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = args
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let size = args
                        .get("size")
                        .and_then(|v| v.as_i64().or(v.as_u64().map(|x| x as i64)));
                    let (path, warning) = download_attachment(
                        &cfg,
                        &project,
                        &event_id,
                        &att_id,
                        name.as_deref(),
                        size,
                    )
                    .await?;
                    let sz = tokio::fs::metadata(&path)
                        .await
                        .map(|m| m.len() as i64)
                        .unwrap_or(0);
                    let mut out = Map::new();
                    out.insert("id".into(), json!(att_id));
                    out.insert(
                        "name".into(),
                        json!(name.unwrap_or(format!("attachment-{att_id}"))),
                    );
                    out.insert("size".into(), json!(sz));
                    out.insert("path".into(), json!(path.to_string_lossy().to_string()));
                    if let Some(w) = warning {
                        out.insert("warning".into(), json!(w));
                    }
                    Ok::<Value, AdapterError>(Value::Object(out))
                })
            }
        );
        // list_events
        {
            let fallback_proj2 = conn
                .config
                .get("project_slug")
                .and_then(|v| v.as_str())
                .unwrap_or("*")
                .to_string();
            reg!(
                "list_events",
                "List recent error events for a project, optionally with full event bodies.",
                "read",
                {
                    let mut m = Map::new();
                    m.insert("project".into(), json!({"type":"string","description":"Project slug. Defaults to the integration's project if set."}));
                    m.insert("period".into(), json!({"type":"string","default":"24h","description":"Stats period, e.g. 15m, 24h, 7d"}));
                    m.insert("full".into(), json!({"type":"boolean","default":false,"description":"Include full event bodies, including stacktraces."}));
                    m.insert("limit".into(), json!({"type":"integer","minimum":1,"maximum":100,"default":25,"description":"Max events to return"}));
                    m
                },
                {
                    let fp = fallback_proj2.clone();
                    move |args: &Value| {
                        format!(
                            "list_events project={} period={} full={} limit={}",
                            args.get("project").and_then(|v| v.as_str()).unwrap_or(&fp),
                            args.get("period").and_then(|v| v.as_str()).unwrap_or("24h"),
                            args.get("full").and_then(|v| v.as_bool()).unwrap_or(false),
                            args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25)
                        )
                    }
                },
                |args: Value, conn: &Integration| {
                    let cfg = sentry_config_from(conn);
                    Box::pin(async move {
                        let proj=args.get("project").and_then(|v|v.as_str()).map(|s|s.to_string()).or(cfg.project.clone()).ok_or_else(|| AdapterError::new("No project given. Pass project or set project_slug in the integration config."))?;
                        let period = args
                            .get("period")
                            .and_then(|v| v.as_str())
                            .unwrap_or("24h")
                            .to_string();
                        let full = args.get("full").and_then(|v| v.as_bool()).unwrap_or(false);
                        let limit =
                            args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25) as usize;
                        let mut v = sentry_request(
                            &cfg,
                            "GET",
                            &format!(
                                "/projects/{}/{}/events/",
                                urlencoding::encode(&cfg.org),
                                urlencoding::encode(&proj)
                            ),
                            Some(json!({"statsPeriod":period,"full":full})),
                            None,
                        )
                        .await?;
                        if let Some(a) = v.as_array() {
                            let mut c = a.clone();
                            c.truncate(limit);
                            v = Value::Array(c);
                        }
                        Ok::<Value, AdapterError>(v)
                    })
                }
            );
        }
        // query_logs
        {
            let fallback_proj3 = conn
                .config
                .get("project_slug")
                .and_then(|v| v.as_str())
                .unwrap_or("*")
                .to_string();
            reg!(
                "query_logs",
                "Query Sentry structured logs using Explore's logs dataset.",
                "read",
                {
                    let mut m = Map::new();
                    m.insert("query".into(), json!({"type":"string","description":"Sentry log search query, e.g. \"severity:error payment.failed\""}));
                    m.insert("project".into(), json!({"type":"string","description":"Project slug or id. Defaults to the integration's project if set; omit for all projects."}));
                    m.insert("period".into(), json!({"type":"string","default":"24h","description":"Stats period, e.g. 15m, 24h, 7d"}));
                    m.insert("fields".into(), json!({"type":"array","items":{"type":"string"},"default":["timestamp","severity","message","trace_id","project"],"description":"Explore fields to return. Defaults to timestamp, severity, message, trace_id, project."}));
                    m.insert("sort".into(), json!({"type":"string","default":"-timestamp","description":"Sort field, e.g. -timestamp"}));
                    m.insert("limit".into(), json!({"type":"integer","minimum":1,"maximum":100,"default":25,"description":"Max log rows to return"}));
                    m
                },
                {
                    let fp = fallback_proj3.clone();
                    move |args: &Value| {
                        format!(
                            "query_logs project={} query=\"{}\" period={} limit={}",
                            args.get("project").and_then(|v| v.as_str()).unwrap_or(&fp),
                            args.get("query").and_then(|v| v.as_str()).unwrap_or(""),
                            args.get("period").and_then(|v| v.as_str()).unwrap_or("24h"),
                            args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25)
                        )
                    }
                },
                |args: Value, conn: &Integration| {
                    let cfg = sentry_config_from(conn);
                    Box::pin(async move {
                        let query = args
                            .get("query")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let proj = args
                            .get("project")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .or(cfg.project.clone())
                            .unwrap_or("-1".into());
                        let period = args
                            .get("period")
                            .and_then(|v| v.as_str())
                            .unwrap_or("24h")
                            .to_string();
                        let fields = args
                            .get("fields")
                            .and_then(|v| v.as_array())
                            .map(|a| {
                                a.iter()
                                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or(vec![
                                "timestamp".into(),
                                "severity".into(),
                                "message".into(),
                                "trace_id".into(),
                                "project".into(),
                            ]);
                        let sort = args
                            .get("sort")
                            .and_then(|v| v.as_str())
                            .unwrap_or("-timestamp")
                            .to_string();
                        let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25);
                        let f: Vec<Value> = fields.into_iter().take(20).map(|s| json!(s)).collect();
                        sentry_request(&cfg,"GET",&format!("/organizations/{}/events/",urlencoding::encode(&cfg.org)),Some(json!({"dataset":"logs","field":f,"query":query,"project":proj,"statsPeriod":period,"sort":sort,"per_page":limit})),None).await
                    })
                }
            );
        }
        // update_issue
        reg!(
            "update_issue",
            "Resolve, ignore, or reopen an issue (write).",
            "write",
            {
                let mut m = Map::new();
                m.insert(
                    "id".into(),
                    json!({"type":"string","description":"Issue id (numeric) or short id"}),
                );
                m.insert("status".into(), json!({"type":"string","enum":["resolved","ignored","unresolved"],"description":"New status"}));
                m
            },
            |args: &Value| format!(
                "update_issue {} -> {}",
                args.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                args.get("status").and_then(|v| v.as_str()).unwrap_or("")
            ),
            |args: Value, conn: &Integration| {
                let cfg = sentry_config_from(conn);
                Box::pin(async move {
                    let id = args
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let status = args
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    sentry_request(
                        &cfg,
                        "PUT",
                        &format!(
                            "/organizations/{}/issues/{}/",
                            urlencoding::encode(&cfg.org),
                            urlencoding::encode(&id)
                        ),
                        None,
                        Some(json!({"status":status})),
                    )
                    .await
                })
            }
        );
        Ok(())
    }
}
pub fn sentry_adapters(store: Arc<pluk_store::Store>) -> Vec<Arc<dyn Adapter>> {
    vec![SentryAdapter::new(store)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn list_projects_default_and_capabilities() {
        let item = json!({"slug":"browser-pool","name":"Browser Pool","platform":"node","team":{"slug":"backend"},"environments":["production"],"access":["a"],"features":["f1"],"hasInsightsHttp":true,"hasMinifiedStackTrace":false});
        let out = project_value(json!([item.clone()]), None, &list_projects_map()).unwrap();
        assert_eq!(
            out,
            json!([{"slug":"browser-pool","name":"Browser Pool","platform":"node","team":{"slug":"backend"},"environments":["production"]}])
        );
        let cap = project_value(
            item,
            Some(vec!["capabilities".into()]),
            &list_projects_map(),
        )
        .unwrap();
        assert_eq!(cap.get("features").unwrap(), &json!(["f1"]));
        assert_eq!(cap.get("hasInsightsHttp").unwrap(), &json!(true));
        assert!(cap.get("isMember").is_none());
    }
    #[test]
    fn list_issues_and_get_issue_projections() {
        let issue = json!({"shortId":"BACKEND-1A","title":"TypeError","culprit":"app","level":"error","status":"unresolved","priority":"high","count":"42","userCount":3,"firstSeen":"2026-01-01","lastSeen":"2026-08-01","project":{"slug":"backend"},"stats":{"24h":[[1,2]]},"id":"999","permalink":"https://x"});
        let out = project_value(json!([issue]), None, &list_issues_map()).unwrap();
        assert!(out.as_array().unwrap()[0].get("stats").is_none());
        let with_stats = project_value(
            json!({"shortId":"BACKEND-1A","stats":{"24h":[[1,2]]},"lifetime":{"count":"100"}}),
            Some(vec!["stats".into()]),
            &list_issues_map(),
        )
        .unwrap();
        assert!(with_stats.get("stats").is_some());
    }
    #[test]
    fn star_and_unknown() {
        let v = json!({"slug":"a","extra":"x"});
        assert_eq!(
            project_value(v.clone(), Some(vec!["*".into()]), &list_projects_map()).unwrap(),
            v
        );
        assert!(
            project_value(
                json!({"slug":"a"}),
                Some(vec!["bogus".into()]),
                &list_projects_map()
            )
            .is_err()
        );
        let err = project_value(
            json!({"shortId":"X"}),
            Some(vec!["bogus".into()]),
            &get_issue_map(),
        )
        .unwrap_err();
        assert!(err.message.contains("Unknown \"only\" field \"bogus\""));
    }
    #[test]
    fn latest_event_frames_and_presets() {
        let frame = |i: i64, in_app: bool| json!({"filename":format!("src/handler-{i}.ts"),"function":format!("fn{i}"),"lineNo":100+i,"module":format!("pkg-{i}"),"inApp":in_app,"context":[["a","b"]],"vars":{"x":1}});
        let frames: Vec<Value> = (0..6).map(|i| frame(i, i >= 4)).collect();
        let raw_entries = json!([{"type":"exception","data":{"values":[{"type":"TypeError","value":"msg","module":"m","stacktrace":{"frames":frames}}]}},{"type":"breadcrumbs","data":{"values":[{"message":"step1"}]}}]);
        let opts = frame_opts(&None);
        let reduced = reduce_exception(&raw_entries, opts);
        // default only inApp
        let arr = reduced.as_array().unwrap();
        assert_eq!(arr[0].get("frames").unwrap().as_array().unwrap().len(), 2);
        // frames.all includes all
        let all = reduce_exception(&raw_entries, (true, false, false, false));
        assert_eq!(
            all.as_array().unwrap()[0]
                .get("frames")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            6
        );
        // frames.full keeps vars
        let full = reduce_exception(&raw_entries, (false, false, false, true));
        assert!(
            full.as_array().unwrap()[0].get("stacktrace").is_some()
                || full.as_array().unwrap()[0].get("frames").is_none()
        );
    }
    #[tokio::test]
    async fn sentry_request_shapes() {
        let cfg = SentryConfig {
            base_url: "https://sentry.io".into(),
            token: "".into(),
            org: "acme".into(),
            project: None,
        };
        let e = sentry_request(&cfg, "GET", "/organizations/acme/projects/", None, None)
            .await
            .unwrap_err();
        assert!(e.message.contains("auth token is missing"));
        let cfg2 = SentryConfig {
            base_url: "https://sentry.io".into(),
            token: "t".into(),
            org: "".into(),
            project: None,
        };
        let e2 = sentry_request(&cfg2, "GET", "/organizations/acme/projects/", None, None)
            .await
            .unwrap_err();
        assert!(e2.message.contains("organization slug is missing"));
        // mock timeout/api error via runner
        client::set_sentry_runner(Some(std::sync::Arc::new(|_, _, _, _| {
            Box::pin(async { Err(AdapterError::new("Sentry API timed out after 20s")) })
        })));
        let cfg3 = SentryConfig {
            base_url: "https://sentry.io".into(),
            token: "t".into(),
            org: "acme".into(),
            project: None,
        };
        let e3 = sentry_request(&cfg3, "GET", "/organizations/acme/projects/", None, None)
            .await
            .unwrap_err();
        assert!(e3.message.contains("timed out"));
        client::set_sentry_runner(Some(std::sync::Arc::new(|_, _, _, _| {
            Box::pin(async { Err(AdapterError::new("Sentry API 500")) })
        })));
        let e4 = sentry_request(&cfg3, "GET", "/organizations/acme/projects/", None, None)
            .await
            .unwrap_err();
        assert!(e4.message.contains("500"));
        client::set_sentry_runner(None);
        // auth header is Bearer shape verified in source - test via runner capture URL contains base
        let captured = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let cap2 = captured.clone();
        client::set_sentry_runner(Some(std::sync::Arc::new(move |method, url, _, _| {
            let c = cap2.clone();
            let m = method.clone();
            let u = url.clone();
            Box::pin(async move {
                c.lock().unwrap().push_str(&format!("{m} {u}"));
                Ok(client::SentryRawResponse {
                    status: 200,
                    body: b"[]".to_vec(),
                    headers: Default::default(),
                })
            })
        })));
        let _ = sentry_request(
            &cfg3,
            "GET",
            &format!("/organizations/{}/projects/", urlencoding::encode("acme")),
            None,
            None,
        )
        .await
        .unwrap();
        assert!(
            captured
                .lock()
                .unwrap()
                .contains("/api/0/organizations/acme/projects/")
        );
        client::set_sentry_runner(None);
    }
    #[test]
    fn size_warning_logic() {
        assert_eq!(crate::sentry::size_warning(Some(10), 10), None);
        assert!(
            crate::sentry::size_warning(Some(10), 5)
                .unwrap()
                .contains("fewer than")
        );
        assert!(
            crate::sentry::size_warning(Some(10), 12)
                .unwrap()
                .contains("Sentry listed 10")
        );
    }
    #[test]
    fn attachment_path_safety() {
        assert_eq!(safe_part("a/b"), "a_b");
        assert_eq!(safe_part(""), "_");
    }
}
