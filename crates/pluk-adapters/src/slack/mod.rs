pub mod client;

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

use client::{resolve_channel, slack_config_from, slack_request, SlackConfig};

pub use client::{set_slack_runner, TIMEOUT_MS};

const AGENT_HINT: &str = "Use this for Slack workspace access — list channels, read recent channel messages for context, and post messages back. list_channels to find a channel id, channel_history to read it; set default_channel to skip the arg.";

pub fn slack_fields() -> Vec<ConfigField> {
    vec![
        ConfigField::new("bot_token", "Bot Token", FieldType::Password)
            .group("Auth")
            .required()
            .secret()
            .placeholder("xoxb-…"),
        ConfigField::new("default_channel", "Default Channel", FieldType::Text)
            .group("Defaults")
            .placeholder("C0123… or #general (optional)"),
    ]
}

fn channel_map() -> FieldMap {
    FieldMap::new(
        &["id", "name", "is_private", "topic", "purpose", "num_members"],
        &["id", "name", "topic"],
    )
    .with_preset("details", Preset::paths(&["purpose", "num_members", "is_private"]))
}

fn message_map() -> FieldMap {
    FieldMap::new(
        &["type", "user", "text", "ts", "thread_ts", "reply_count", "reactions", "files"],
        &["user", "text", "ts"],
    )
    .with_preset("thread", Preset::paths(&["thread_ts", "reply_count"]))
    .with_preset("attachments", Preset::paths(&["files", "reactions"]))
}

fn post_map() -> FieldMap {
    FieldMap::new(&["ok", "channel", "ts", "message"], &["ok", "channel", "ts"])
        .with_preset("message", Preset::paths(&["message"]))
}

fn only_from_args(args: &Value) -> Option<Vec<String>> {
    args.get("only")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
}

fn project_value(data: Value, only: Option<Vec<String>>, map: &FieldMap) -> Result<Value, AdapterError> {
    apply_only(&data, only.as_ref(), map).map_err(|e| AdapterError::new(e.to_string()))
}

pub struct SlackAdapter {
    store: Arc<pluk_store::Store>,
}

impl SlackAdapter {
    pub fn new(store: Arc<pluk_store::Store>) -> Arc<Self> {
        Arc::new(Self { store })
    }
}

#[async_trait]
impl Adapter for SlackAdapter {
    fn id(&self) -> &str { "slack" }
    fn label(&self) -> &str { "Slack" }
    fn category(&self) -> &str { "chat" }
    fn policy_kind(&self) -> PolicyKind { PolicyKind::Action }
    fn agent_hint(&self) -> &str { AGENT_HINT }
    fn tool_specs(&self) -> &[ToolSpec] {
        static SPECS: std::sync::OnceLock<Vec<ToolSpec>> = std::sync::OnceLock::new();
        SPECS.get_or_init(|| vec![
            ToolSpec::new("list_channels", "List public channels in the workspace (id, name, topic).", "read"),
            ToolSpec::new("channel_history", "Read recent messages in a channel, newest first.", "read"),
            ToolSpec::new("post_message", "Post a message to a channel.", "write"),
        ])
    }
    fn config_fields(&self) -> &[ConfigField] {
        static FIELDS: std::sync::OnceLock<Vec<ConfigField>> = std::sync::OnceLock::new();
        FIELDS.get_or_init(slack_fields)
    }
    async fn test_connection(&self, conn: &Integration) -> Result<(), AdapterError> {
        let cfg = slack_config_from(conn)?;
        slack_request(&cfg, "auth.test", json!({})).await.map(|_| ())
    }
    fn instructions(&self, conn: &Integration) -> String {
        let enabled: Vec<&str> = self.tool_specs().iter()
            .filter(|t| pluk_policy::tool_gate(conn.query_policy.as_deref()).enabled(&t.name, t.default_enabled))
            .map(|t| t.name.as_str()).collect();
        let policy = if enabled.is_empty() { "No tools are enabled on this integration.".to_string() } else { format!("Enabled tools: {}.", enabled.join(", ")) };
        build_instructions(&conn.name, conn.environment, InstructionParts {
            kind: "Slack".into(),
            access: "Read channels and recent messages; post a message when write is permitted. Requires bot scopes channels:read, channels:history, chat:write. Every action is policy-checked and recorded in the activity log.".into(),
            policy: Some(policy),
            start: Some("list_channels".into()),
            hint: Some(AGENT_HINT.into()),
        })
    }
    fn register(&self, host: &mut dyn ToolHost, conn: &Integration, _owner_id: &str) -> Result<(), AdapterError> {
        let store = self.store.clone();
        let cfg = slack_config_from(conn)?;
        macro_rules! reg {
            ($name:expr, $desc:expr, $cat:expr, $schema:expr, $detail:expr, $body:expr) => {{
                let store = store.clone();
                let conn = conn.clone();
                let cfg_clone = cfg.clone();
                let handler: ToolHandler = Arc::new(move |args: Value| {
                    let store = store.clone();
                    let conn = conn.clone();
                    let cfg = cfg_clone.clone();
                    let detail = $detail(&args);
                    let meta = GateMeta::new($cat, $name, detail);
                    let target = CallTarget::from(&conn);
                    Box::pin(async move {
                        run_gated(&store, &target, meta, |_| async {
                            let out = $body(args, &cfg).await?;
                            let text = match &out { Value::String(s) => s.clone(), _ => serde_json::to_string_pretty(&out).unwrap_or("{}".into()) };
                            let rows = match &out { Value::Array(a) => a.clone(), o => vec![o.clone()] };
                            Ok(Outcome::Ran(RunOutcome { text: text.clone(), response_text: Some(text), result: Some(pluk_store::QueryResult{fields:vec![], rows}), ..Default::default() }))
                        }, GateOpts::default()).await
                    })
                });
                let mut props = $schema;
                let schema = if props.is_empty() { Map::new() } else { object_schema(props, &[]) };
                host.register_tool(ToolRegistration { name: $name.into(), description: $desc.into(), input_schema: schema, annotations: Map::new() }, handler);
            }};
        }
        reg!("list_channels", "List public channels in the workspace (id, name, topic).", "read", {
            let mut m = Map::new();
            m.insert("limit".into(), json!({"type":"integer","minimum":1,"maximum":1000,"default":100,"description":"Max channels to return"}));
            m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&["details"])}));
            m
        }, |_args: &Value| "list_channels".to_string(), |args: Value, cfg: &SlackConfig| {
            let cfg = cfg.clone();
            Box::pin(async move {
                let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(100);
                let data = slack_request(&cfg, "conversations.list", json!({"types":"public_channel","limit": limit})).await?;
                let channels = data.get("channels").cloned().unwrap_or(json!([]));
                let only = only_from_args(&args);
                project_value(channels, only, &channel_map())
            })
        });
        reg!("channel_history", "Read recent messages in a channel, newest first.", "read", {
            let mut m = Map::new();
            m.insert("channel".into(), json!({"type":"string","description":"Channel id (e.g. C0123). Defaults to the integration's default_channel."}));
            m.insert("limit".into(), json!({"type":"integer","minimum":1,"maximum":100,"default":20,"description":"Max messages to return"}));
            m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&["thread","attachments"])}));
            m
        }, |args: &Value| {
            let ch = args.get("channel").and_then(|v| v.as_str()).unwrap_or("?");
            let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(20);
            format!("channel_history {ch} limit={limit}")
        }, |args: Value, cfg: &SlackConfig| {
            let cfg = cfg.clone();
            Box::pin(async move {
                let ch_arg = args.get("channel").and_then(|v| v.as_str());
                let channel = resolve_channel(&cfg, ch_arg)?;
                let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(20);
                let data = slack_request(&cfg, "conversations.history", json!({"channel": channel, "limit": limit})).await?;
                let messages = data.get("messages").cloned().unwrap_or(json!([]));
                let only = only_from_args(&args);
                project_value(messages, only, &message_map())
            })
        });
        reg!("post_message", "Post a message to a channel.", "write", {
            let mut m = Map::new();
            m.insert("channel".into(), json!({"type":"string","description":"Channel id. Defaults to the integration's default_channel."}));
            m.insert("text".into(), json!({"type":"string","description":"Message text (markdown)"}));
            m.insert("only".into(), json!({"type":"array","items":{"type":"string"},"description": crate::projection::only_param_description(&["message"])}));
            m
        }, |args: &Value| format!("post_message {}", args.get("channel").and_then(|v| v.as_str()).unwrap_or("?")), |args: Value, cfg: &SlackConfig| {
            let cfg = cfg.clone();
            Box::pin(async move {
                let ch_arg = args.get("channel").and_then(|v| v.as_str());
                let channel = resolve_channel(&cfg, ch_arg)?;
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let data = slack_request(&cfg, "chat.postMessage", json!({"channel": channel, "text": text})).await?;
                let only = only_from_args(&args);
                project_value(data, only, &post_map())
            })
        });
        Ok(())
    }
}

pub fn slack_adapters(store: Arc<pluk_store::Store>) -> Vec<Arc<dyn Adapter>> { vec![SlackAdapter::new(store)] }

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Mutex, OnceLock};
    static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn conn(config: Map<String, Value>) -> Integration {
        Integration { id: "s".into(), name: "Slack".into(), r#type: "slack".into(), config, environment: None, read_only: 0, query_policy: None, token: "t".into(), created_at: String::new(), via_group: None }
    }
    fn cfg_with_default(ch: Option<&str>) -> SlackConfig {
        let mut m = Map::new();
        m.insert("bot_token".into(), json!("xoxb-1"));
        if let Some(c) = ch { m.insert("default_channel".into(), json!(c)); }
        slack_config_from(&conn(m)).unwrap()
    }
    #[test]
    fn slack_config_reads_and_rejects_blank() {
        let mut m = Map::new();
        m.insert("bot_token".into(), json!("xoxb-1"));
        m.insert("default_channel".into(), json!("C1"));
        let cfg = slack_config_from(&conn(m)).unwrap();
        assert_eq!(cfg.token, "xoxb-1");
        assert_eq!(cfg.default_channel.as_deref(), Some("C1"));
        assert!(slack_config_from(&conn(Map::new())).is_err());
    }
    #[test]
    fn resolve_channel_uses_arg_fallback_and_errors() {
        let cfg = cfg_with_default(Some("C1"));
        assert_eq!(resolve_channel(&cfg, Some("C2")).unwrap(), "C2");
        assert_eq!(resolve_channel(&cfg, None).unwrap(), "C1");
        assert_eq!(resolve_channel(&cfg, Some("  C2  ")).unwrap(), "C2");
        let cfg_no_default = cfg_with_default(None);
        assert!(resolve_channel(&cfg_no_default, None).is_err());
        assert!(resolve_channel(&cfg_no_default, Some("   ")).is_err());
    }
    #[tokio::test]
    async fn slack_request_throws_on_ok_false() {
        let _g = lock();
        let cfg = cfg_with_default(None);
        let runner = Arc::new(|method: String, _params: Value| {
            Box::pin(async move { Err(AdapterError::new(format!("Slack API {method}: invalid_auth"))) }) as _
        });
        set_slack_runner(Some(runner));
        let err = slack_request(&cfg, "auth.test", json!({})).await.unwrap_err();
        assert!(err.message.contains("invalid_auth"));
        set_slack_runner(None);
    }
    #[test]
    fn projection_default_and_presets() {
        let ch = json!({"id":"C1","name":"general","topic":{"value":"hi"},"purpose":{"value":"p"},"num_members":3,"is_private":false});
        let out = project_value(json!([ch.clone()]), None, &channel_map()).unwrap();
        assert_eq!(out, json!([{"id":"C1","name":"general","topic":{"value":"hi"}}]));
        let out2 = project_value(json!([ch]), Some(vec!["details".into()]), &channel_map()).unwrap();
        assert_eq!(out2, json!([{"purpose":{"value":"p"},"num_members":3,"is_private":false}]));
        let msg = json!({"user":"U1","text":"hi","ts":"1","thread_ts":"1"});
        assert_eq!(project_value(msg.clone(), Some(vec!["*".into()]), &message_map()).unwrap(), msg);
        assert!(project_value(json!({"user":"U1"}), Some(vec!["bogus".into()]), &message_map()).is_err());
    }
    #[test]
    fn message_default_and_thread_preset() {
        let msg = json!({"user":"U1","text":"hi","ts":"1","thread_ts":"1","reply_count":2,"reactions":[],"files":[]});
        let out = project_value(json!([msg.clone()]), None, &message_map()).unwrap();
        assert_eq!(out, json!([{"user":"U1","text":"hi","ts":"1"}]));
        let out2 = project_value(json!([msg]), Some(vec!["thread".into()]), &message_map()).unwrap();
        assert_eq!(out2, json!([{"thread_ts":"1","reply_count":2}]));
    }
    #[tokio::test]
    async fn channel_history_uses_default_channel_and_projection() {
        let _g = lock();
        let cfg = cfg_with_default(Some("C1"));
        let runner = Arc::new(|method: String, params: Value| {
            Box::pin(async move {
                if method == "conversations.history" {
                    let ch = params.get("channel").and_then(|v| v.as_str()).unwrap_or("");
                    assert_eq!(ch, "C1");
                    Ok(json!({"ok":true,"messages":[{"user":"U1","text":"hi","ts":"1","thread_ts":"1"}]}))
                } else { Ok(json!({"ok":true})) }
            }) as _
        });
        set_slack_runner(Some(runner));
        let ch = resolve_channel(&cfg, None).unwrap();
        assert_eq!(ch, "C1");
        let data = slack_request(&cfg, "conversations.history", json!({"channel": ch, "limit": 20})).await.unwrap();
        let msgs = data.get("messages").cloned().unwrap();
        let out = project_value(msgs, None, &message_map()).unwrap();
        assert_eq!(out, json!([{"user":"U1","text":"hi","ts":"1"}]));
        set_slack_runner(None);
    }
    #[test]
    fn tool_specs_categories() {
        let store = { let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("pluk.db"); Arc::new(pluk_store::Store::open(&path).unwrap()) };
        let adapter = SlackAdapter::new(store);
        let specs = adapter.tool_specs();
        let post = specs.iter().find(|t| t.name == "post_message").unwrap();
        assert_eq!(post.category, "write");
        assert!(!post.default_enabled);
        let list = specs.iter().find(|t| t.name == "list_channels").unwrap();
        assert_eq!(list.category, "read");
        assert!(list.default_enabled);
    }
}
