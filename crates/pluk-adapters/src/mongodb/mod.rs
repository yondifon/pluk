//! The MongoDB adapter: inspect a deployment and read its documents, with
//! inserts, updates and deletes behind the write toggles.
//!
//! Every tool is declared once in [`tools`], which both the catalog and
//! registration read, so the toggle a user sees and the tool an agent calls
//! can never drift apart.

pub mod client;
pub mod guard;

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value, json};

use pluk_store::Integration;

use crate::adapter::{Adapter, PolicyKind};
use crate::config_field::{ConfigField, FieldType};
use crate::error::AdapterError;
use crate::gate::{CallTarget, GateMeta, GateOpts, Outcome, RunOutcome, run_gated};
use crate::instructions::{InstructionParts, build_instructions};
use crate::tool_host::{BoxFuture, ToolHandler, ToolHost, ToolRegistration, object_schema};
use crate::tool_spec::ToolSpec;

use client::{DOCUMENT_CAP, MongoAccessor, capped, mongo_config_from, to_document, to_pipeline};

const AGENT_HINT: &str = "Use this to inspect and query a MongoDB deployment — list databases and collections, read a collection's indexes and sampled field shape, and read documents with find, count and aggregate. Filters, projections, sorts and pipelines are JSON. Insert, update and delete only when write is permitted.";

pub fn mongodb_fields() -> Vec<ConfigField> {
    vec![
        ConfigField::new("uri", "Connection String", FieldType::Password)
            .group("Connection")
            .required()
            .secret()
            .placeholder("mongodb+srv://user:password@cluster.example.net")
            .help("Also sets TLS, the replica set and the auth source."),
        ConfigField::new("database", "Default Database", FieldType::Text)
            .group("Connection")
            .placeholder("Used when a call names no database"),
    ]
}

// Argument readers. An agent that sends a filter or pipeline as a JSON string
// gets the same treatment as one that sends the object itself.

fn text(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn opt_text(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn json_arg(args: &Value, key: &str) -> Option<Value> {
    match args.get(key) {
        None | Some(Value::Null) => None,
        Some(Value::String(raw)) => Some(
            serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.clone())),
        ),
        Some(value) => Some(value.clone()),
    }
}

fn filter_arg(args: &Value) -> Value {
    json_arg(args, "filter").unwrap_or_else(|| json!({}))
}

fn limit_arg(args: &Value, fallback: i64) -> i64 {
    capped(args.get("limit").and_then(|v| v.as_i64()), fallback)
}

/// `database.collection`, or just the collection when the call took the
/// integration's default.
fn target(args: &Value) -> String {
    let collection = text(args, "collection");
    match opt_text(args, "database") {
        Some(database) => format!("{database}.{collection}"),
        None => collection,
    }
}

/// What the activity log records as this call's rows: the documents a read
/// returned, or the single result object.
fn rows_of(value: &Value) -> Vec<Value> {
    match value.get("documents") {
        Some(Value::Array(documents)) => documents.clone(),
        _ => vec![value.clone()],
    }
}

type Detail = fn(&Value) -> String;
type Check = fn(&Value) -> Option<String>;
type Body = fn(Value, Arc<MongoAccessor>) -> BoxFuture<Result<Value, AdapterError>>;

/// One tool: its catalog entry, its argument schema, and what it runs.
struct MongoTool {
    name: &'static str,
    description: &'static str,
    category: &'static str,
    properties: Map<String, Value>,
    required: &'static [&'static str],
    detail: Detail,
    check: Check,
    body: Body,
}

fn no_check(_args: &Value) -> Option<String> {
    None
}

fn props(entries: Vec<(&str, Value)>) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(key, schema)| (key.to_string(), schema))
        .collect()
}

fn database_prop() -> (&'static str, Value) {
    (
        "database",
        json!({"type":"string","description":"Database to read; defaults to the integration's."}),
    )
}

fn collection_prop() -> (&'static str, Value) {
    (
        "collection",
        json!({"type":"string","description":"Collection name"}),
    )
}

fn limit_prop(default: i64) -> (&'static str, Value) {
    (
        "limit",
        json!({
            "type": "integer",
            "minimum": 1,
            "maximum": DOCUMENT_CAP,
            "default": default,
            "description": format!("Documents to return (at most {DOCUMENT_CAP})"),
        }),
    )
}

fn tools() -> Vec<MongoTool> {
    vec![
        MongoTool {
            name: "list_databases",
            description: "List the databases on this deployment.",
            category: "read",
            properties: Map::new(),
            required: &[],
            detail: |_| "list_databases".to_string(),
            check: no_check,
            body: |_args, acc| Box::pin(async move { acc.list_databases().await }),
        },
        MongoTool {
            name: "list_collections",
            description: "List the collections in a database.",
            category: "read",
            properties: props(vec![database_prop()]),
            required: &[],
            detail: |args| match opt_text(args, "database") {
                Some(database) => format!("list_collections {database}"),
                None => "list_collections".to_string(),
            },
            check: no_check,
            body: |args, acc| {
                Box::pin(async move {
                    acc.list_collections(opt_text(&args, "database").as_deref())
                        .await
                })
            },
        },
        MongoTool {
            name: "describe_collection",
            description: "Show a collection's indexes and the field shape of a document sample.",
            category: "read",
            properties: props(vec![
                collection_prop(),
                database_prop(),
                (
                    "sample",
                    json!({
                        "type": "integer",
                        "minimum": 1,
                        "maximum": DOCUMENT_CAP,
                        "default": 100,
                        "description": "Documents to sample for the field shape",
                    }),
                ),
            ]),
            required: &["collection"],
            detail: |args| format!("describe_collection {}", target(args)),
            check: no_check,
            body: |args, acc| {
                Box::pin(async move {
                    let sample = capped(args.get("sample").and_then(|v| v.as_i64()), 100);
                    acc.describe_collection(
                        opt_text(&args, "database").as_deref(),
                        &text(&args, "collection"),
                        sample,
                    )
                    .await
                })
            },
        },
        MongoTool {
            name: "find",
            description: "Read documents from a collection, with an optional filter, projection and sort.",
            category: "read",
            properties: props(vec![
                collection_prop(),
                database_prop(),
                (
                    "filter",
                    json!({"type":"object","description":"Query filter, e.g. {\"status\":\"open\"}"}),
                ),
                (
                    "projection",
                    json!({"type":"object","description":"Fields to include or exclude, e.g. {\"name\":1}"}),
                ),
                (
                    "sort",
                    json!({"type":"object","description":"Sort order, e.g. {\"created_at\":-1}"}),
                ),
                limit_prop(50),
            ]),
            required: &["collection"],
            detail: |args| {
                format!(
                    "find {} filter={} limit={}",
                    target(args),
                    filter_arg(args),
                    limit_arg(args, 50)
                )
            },
            check: |args| {
                guard::check_query(&filter_arg(args))
                    .or_else(|| json_arg(args, "projection").and_then(|p| guard::check_query(&p)))
                    .or_else(|| json_arg(args, "sort").and_then(|s| guard::check_query(&s)))
            },
            body: |args, acc| {
                Box::pin(async move {
                    let filter = to_document(&filter_arg(&args), "filter")?;
                    let projection = json_arg(&args, "projection")
                        .map(|p| to_document(&p, "projection"))
                        .transpose()?;
                    let sort = json_arg(&args, "sort")
                        .map(|s| to_document(&s, "sort"))
                        .transpose()?;
                    acc.find(
                        opt_text(&args, "database").as_deref(),
                        &text(&args, "collection"),
                        filter,
                        projection,
                        sort,
                        limit_arg(&args, 50),
                    )
                    .await
                })
            },
        },
        MongoTool {
            name: "count",
            description: "Count the documents in a collection that match a filter.",
            category: "read",
            properties: props(vec![
                collection_prop(),
                database_prop(),
                (
                    "filter",
                    json!({"type":"object","description":"Query filter; omit to count everything"}),
                ),
            ]),
            required: &["collection"],
            detail: |args| format!("count {} filter={}", target(args), filter_arg(args)),
            check: |args| guard::check_query(&filter_arg(args)),
            body: |args, acc| {
                Box::pin(async move {
                    let filter = to_document(&filter_arg(&args), "filter")?;
                    acc.count(
                        opt_text(&args, "database").as_deref(),
                        &text(&args, "collection"),
                        filter,
                    )
                    .await
                })
            },
        },
        MongoTool {
            name: "aggregate",
            description: "Run an aggregation pipeline over a collection.",
            category: "read",
            properties: props(vec![
                collection_prop(),
                database_prop(),
                (
                    "pipeline",
                    json!({
                        "type": "array",
                        "items": {"type": "object"},
                        "description": "Pipeline stages, e.g. [{\"$match\":{\"a\":1}},{\"$group\":{\"_id\":\"$a\"}}]",
                    }),
                ),
                limit_prop(100),
            ]),
            required: &["collection", "pipeline"],
            detail: |args| {
                format!(
                    "aggregate {} pipeline={}",
                    target(args),
                    json_arg(args, "pipeline").unwrap_or_else(|| json!([]))
                )
            },
            check: |args| match json_arg(args, "pipeline") {
                Some(pipeline) => guard::check_query(&pipeline),
                None => Some("`pipeline` is required.".to_string()),
            },
            body: |args, acc| {
                Box::pin(async move {
                    let pipeline = to_pipeline(&json_arg(&args, "pipeline").unwrap_or_else(|| json!([])))?;
                    acc.aggregate(
                        opt_text(&args, "database").as_deref(),
                        &text(&args, "collection"),
                        pipeline,
                        limit_arg(&args, 100),
                    )
                    .await
                })
            },
        },
        MongoTool {
            name: "insert_one",
            description: "Insert one document into a collection.",
            category: "write",
            properties: props(vec![
                collection_prop(),
                database_prop(),
                (
                    "document",
                    json!({"type":"object","description":"The document to insert"}),
                ),
            ]),
            required: &["collection", "document"],
            detail: |args| format!("insert_one {}", target(args)),
            check: |args| match json_arg(args, "document") {
                Some(document) => guard::check_document(&document),
                None => Some("`document` is required.".to_string()),
            },
            body: |args, acc| {
                Box::pin(async move {
                    let document = json_arg(&args, "document")
                        .ok_or_else(|| AdapterError::new("`document` is required."))?;
                    acc.insert_one(
                        opt_text(&args, "database").as_deref(),
                        &text(&args, "collection"),
                        to_document(&document, "document")?,
                    )
                    .await
                })
            },
        },
        MongoTool {
            name: "update_many",
            description: "Update every document matching a filter.",
            category: "write",
            properties: props(vec![
                collection_prop(),
                database_prop(),
                (
                    "filter",
                    json!({"type":"object","description":"Which documents to update; may not be empty"}),
                ),
                (
                    "update",
                    json!({"type":"object","description":"Update operators, e.g. {\"$set\":{\"status\":\"done\"}}"}),
                ),
            ]),
            required: &["collection", "filter", "update"],
            detail: |args| {
                format!(
                    "update_many {} filter={}",
                    target(args),
                    filter_arg(args)
                )
            },
            check: |args| {
                let filter = filter_arg(args);
                guard::require_filter(&filter)
                    .or_else(|| guard::check_query(&filter))
                    .or_else(|| match json_arg(args, "update") {
                        Some(update) => guard::check_query(&update),
                        None => Some("`update` is required.".to_string()),
                    })
            },
            body: |args, acc| {
                Box::pin(async move {
                    let update = json_arg(&args, "update")
                        .ok_or_else(|| AdapterError::new("`update` is required."))?;
                    acc.update_many(
                        opt_text(&args, "database").as_deref(),
                        &text(&args, "collection"),
                        to_document(&filter_arg(&args), "filter")?,
                        to_document(&update, "update")?,
                    )
                    .await
                })
            },
        },
        MongoTool {
            name: "delete_many",
            description: "Delete every document matching a filter.",
            category: "delete",
            properties: props(vec![
                collection_prop(),
                database_prop(),
                (
                    "filter",
                    json!({"type":"object","description":"Which documents to delete; may not be empty"}),
                ),
            ]),
            required: &["collection", "filter"],
            detail: |args| format!("delete_many {} filter={}", target(args), filter_arg(args)),
            check: |args| {
                let filter = filter_arg(args);
                guard::require_filter(&filter).or_else(|| guard::check_query(&filter))
            },
            body: |args, acc| {
                Box::pin(async move {
                    acc.delete_many(
                        opt_text(&args, "database").as_deref(),
                        &text(&args, "collection"),
                        to_document(&filter_arg(&args), "filter")?,
                    )
                    .await
                })
            },
        },
    ]
}

fn tool_handler(
    store: Arc<pluk_store::Store>,
    conn: &Integration,
    accessor: Arc<MongoAccessor>,
    tool: &MongoTool,
) -> ToolHandler {
    let target = CallTarget::from(conn);
    let name = tool.name;
    let category = tool.category;
    let detail = tool.detail;
    let check = tool.check;
    let body = tool.body;

    Arc::new(move |args: Value| {
        let store = store.clone();
        let target = target.clone();
        let accessor = accessor.clone();
        let meta = GateMeta::new(category, name, detail(&args));
        let checked = args.clone();
        Box::pin(async move {
            run_gated(
                &store,
                &target,
                meta,
                |_| async move {
                    let output = body(args, accessor).await?;
                    let text = serde_json::to_string_pretty(&output)
                        .unwrap_or_else(|_| "{}".to_string());
                    Ok(Outcome::Ran(RunOutcome {
                        text: text.clone(),
                        response_text: Some(text),
                        result: Some(pluk_store::QueryResult {
                            fields: Vec::new(),
                            rows: rows_of(&output),
                        }),
                        ..Default::default()
                    }))
                },
                GateOpts::default().precheck(move || check(&checked)),
            )
            .await
        })
    })
}

pub struct MongoAdapter {
    store: Arc<pluk_store::Store>,
}

impl MongoAdapter {
    pub fn new(store: Arc<pluk_store::Store>) -> Arc<Self> {
        Arc::new(Self { store })
    }
}

#[async_trait]
impl Adapter for MongoAdapter {
    fn id(&self) -> &str {
        "mongodb"
    }
    fn label(&self) -> &str {
        "MongoDB"
    }
    fn category(&self) -> &str {
        "database"
    }
    fn policy_kind(&self) -> PolicyKind {
        PolicyKind::Action
    }
    fn agent_hint(&self) -> &str {
        AGENT_HINT
    }
    fn tool_specs(&self) -> &[ToolSpec] {
        static SPECS: std::sync::OnceLock<Vec<ToolSpec>> = std::sync::OnceLock::new();
        SPECS.get_or_init(|| {
            tools()
                .iter()
                .map(|tool| ToolSpec::new(tool.name, tool.description, tool.category))
                .collect()
        })
    }
    fn config_fields(&self) -> &[ConfigField] {
        static FIELDS: std::sync::OnceLock<Vec<ConfigField>> = std::sync::OnceLock::new();
        FIELDS.get_or_init(mongodb_fields)
    }
    async fn test_connection(&self, conn: &Integration) -> Result<(), AdapterError> {
        client::test_mongo(conn).await
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
            "No tools are enabled on this integration.".to_string()
        } else {
            format!("Enabled tools: {}.", enabled.join(", "))
        };
        build_instructions(
            &conn.name,
            conn.environment,
            InstructionParts {
                kind: "MongoDB".into(),
                access: format!(
                    "Read documents and inspect collections; insert, update and delete when write is permitted. Reads return at most {DOCUMENT_CAP} documents. Every call is policy-checked and recorded in the activity log."
                ),
                policy: Some(policy),
                start: Some(
                    "Start with list_collections and describe_collection, then find with a filter and a limit."
                        .into(),
                ),
                hint: Some(AGENT_HINT.into()),
            },
        )
    }
    fn register(
        &self,
        host: &mut dyn ToolHost,
        conn: &Integration,
        _owner_id: &str,
    ) -> Result<(), AdapterError> {
        let accessor = Arc::new(MongoAccessor::new(mongo_config_from(conn)?));
        for tool in tools() {
            let handler = tool_handler(self.store.clone(), conn, accessor.clone(), &tool);
            let input_schema = if tool.properties.is_empty() {
                Map::new()
            } else {
                object_schema(tool.properties, tool.required)
            };
            host.register_tool(
                ToolRegistration {
                    name: tool.name.into(),
                    description: tool.description.into(),
                    input_schema,
                    annotations: Map::new(),
                },
                handler,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pluk_policy::tool_gate;

    fn conn(config: Value) -> Integration {
        Integration {
            id: "m".into(),
            name: "Mongo".into(),
            r#type: "mongodb".into(),
            config: serde_json::from_value(config).expect("config"),
            environment: None,
            read_only: 0,
            query_policy: None,
            token: "t".into(),
            created_at: String::new(),
            via_group: None,
        }
    }

    fn store() -> Arc<pluk_store::Store> {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("pluk.db");
        Arc::new(pluk_store::Store::open(&path).expect("open"))
    }

    #[test]
    fn config_reads_the_uri_and_default_database() {
        let cfg = mongo_config_from(&conn(json!({
            "uri": "  mongodb+srv://user:pw@cluster.example.net/?retryWrites=true  ",
            "database": " shop "
        })))
        .expect("config");
        assert_eq!(
            cfg.uri,
            "mongodb+srv://user:pw@cluster.example.net/?retryWrites=true"
        );
        assert_eq!(cfg.database.as_deref(), Some("shop"));
    }

    #[test]
    fn a_blank_default_database_stays_unset() {
        let cfg = mongo_config_from(&conn(json!({"uri":"mongodb://localhost:27017","database":"  "})))
            .expect("config");
        assert!(cfg.database.is_none());
    }

    #[test]
    fn config_rejects_a_missing_or_foreign_connection_string() {
        let missing = mongo_config_from(&conn(json!({}))).unwrap_err();
        assert!(missing.message.contains("connection string is missing"));
        let foreign = mongo_config_from(&conn(json!({"uri":"postgres://localhost/db"}))).unwrap_err();
        assert!(foreign.message.contains("mongodb://"));
    }

    #[tokio::test]
    async fn a_connection_test_without_a_connection_string_fails_before_any_dial() {
        let error = MongoAdapter::new(store())
            .test_connection(&conn(json!({})))
            .await
            .unwrap_err();
        assert!(error.message.contains("connection string is missing"));
    }

    #[test]
    fn writes_ship_off_and_reads_ship_on() {
        let adapter = MongoAdapter::new(store());
        let specs = adapter.tool_specs();
        assert_eq!(specs.len(), 9);
        for read in [
            "list_databases",
            "list_collections",
            "describe_collection",
            "find",
            "count",
            "aggregate",
        ] {
            let spec = specs.iter().find(|s| s.name == read).expect(read);
            assert_eq!(spec.category, "read");
            assert!(spec.default_enabled, "{read} must ship on");
        }
        for write in ["insert_one", "update_many", "delete_many"] {
            let spec = specs.iter().find(|s| s.name == write).expect(write);
            assert!(!spec.default_enabled, "{write} must ship off");
        }
        assert_eq!(
            specs.iter().find(|s| s.name == "delete_many").unwrap().category,
            "delete"
        );
    }

    #[test]
    fn the_toggle_decides_which_write_tools_exist() {
        let adapter = MongoAdapter::new(store());
        let specs = adapter.tool_specs();
        let gate = tool_gate(Some(r#"{"tools":{"update_many":{"enabled":true}}}"#));
        let enabled: Vec<&str> = specs
            .iter()
            .filter(|s| gate.enabled(&s.name, s.default_enabled))
            .map(|s| s.name.as_str())
            .collect();
        assert!(enabled.contains(&"update_many"));
        assert!(!enabled.contains(&"delete_many"));
        assert!(!enabled.contains(&"insert_one"));
    }

    #[test]
    fn every_tool_publishes_a_catalog_entry() {
        let adapter = MongoAdapter::new(store());
        for tool in tools() {
            assert!(
                adapter.tool_specs().iter().any(|s| s.name == tool.name),
                "{} has no toggle a user could switch off",
                tool.name
            );
        }
    }

    #[test]
    fn writes_without_a_filter_are_blocked_before_they_run() {
        let all = tools();
        let update = all.iter().find(|t| t.name == "update_many").unwrap();
        let delete = all.iter().find(|t| t.name == "delete_many").unwrap();
        assert!(
            (update.check)(&json!({"collection":"users","update":{"$set":{"a":1}}})).is_some()
        );
        assert!((delete.check)(&json!({"collection":"users","filter":{}})).is_some());
        assert!((delete.check)(&json!({"collection":"users","filter":{"_id":1}})).is_none());
    }

    #[test]
    fn server_side_code_is_blocked_in_reads_and_writes() {
        let all = tools();
        let find = all.iter().find(|t| t.name == "find").unwrap();
        let aggregate = all.iter().find(|t| t.name == "aggregate").unwrap();
        assert!(
            (find.check)(&json!({"collection":"users","filter":{"$where":"this.a==1"}})).is_some()
        );
        assert!(
            (aggregate.check)(&json!({"collection":"users","pipeline":[{"$out":"copy"}]}))
                .is_some()
        );
        assert!(
            (aggregate.check)(&json!({"collection":"users","pipeline":[{"$match":{"a":1}}]}))
                .is_none()
        );
    }

    #[test]
    fn a_filter_sent_as_a_json_string_is_still_guarded() {
        let all = tools();
        let find = all.iter().find(|t| t.name == "find").unwrap();
        assert!(
            (find.check)(&json!({"collection":"users","filter":"{\"$where\":\"1\"}"})).is_some()
        );
    }

    #[test]
    fn reads_never_return_more_than_the_cap() {
        assert_eq!(limit_arg(&json!({"limit": 50_000}), 50), DOCUMENT_CAP);
        assert_eq!(limit_arg(&json!({}), 50), 50);
    }

    #[test]
    fn the_log_line_names_the_database_and_collection() {
        let all = tools();
        let find = all.iter().find(|t| t.name == "find").unwrap();
        assert_eq!(
            (find.detail)(&json!({"database":"shop","collection":"orders"})),
            "find shop.orders filter={} limit=50"
        );
    }
}
