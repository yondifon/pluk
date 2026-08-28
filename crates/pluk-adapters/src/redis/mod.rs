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
use crate::tool_host::{object_schema, ToolHost, ToolRegistration, ToolHandler};
use crate::tool_spec::ToolSpec;

use client::{redis_config_from, RedisAccessor};

pub use client::{
    build_url, set_redis_factory, set_redis_runner, RedisConfig, SshParams,
};

const AGENT_HINT: &str = "Use this to inspect and edit a Redis datastore — list keys, read values, types and TTLs, check server INFO, and set, expire or delete keys. Use scan (not keys) to list keys safely; get/type/ttl inspect a single key.";

pub fn redis_fields() -> Vec<ConfigField> {
    let mut fields = vec![
        ConfigField::new("host", "Host", FieldType::Text)
            .group("Connection")
            .required()
            .placeholder("localhost or 10.0.0.5")
            .default_value(&json!("127.0.0.1")),
        ConfigField::new("port", "Port", FieldType::Number)
            .group("Connection")
            .default_value(&json!(6379)),
        ConfigField::new("db", "Database", FieldType::Number)
            .group("Connection")
            .default_value(&json!(0))
            .placeholder("0"),
        ConfigField::new("tls", "TLS (rediss://)", FieldType::Toggle)
            .group("Connection")
            .default_value(&json!(false)),
        ConfigField::new("password", "Password", FieldType::Password)
            .group("Auth")
            .secret()
            .placeholder("(optional)"),
        ConfigField::new("use_ssh", "SSH Tunnel", FieldType::Toggle).group("SSH Tunnel"),
        ConfigField::new("ssh_host", "SSH Host", FieldType::Text)
            .group("SSH Tunnel")
            .show_if(crate::config_field::ShowIf::eq_str("use_ssh", "true")),
        ConfigField::new("ssh_port", "SSH Port", FieldType::Number)
            .group("SSH Tunnel")
            .default_value(&json!(22))
            .show_if(crate::config_field::ShowIf::eq_str("use_ssh", "true")),
        ConfigField::new("ssh_user", "SSH User", FieldType::Text)
            .group("SSH Tunnel")
            .show_if(crate::config_field::ShowIf::eq_str("use_ssh", "true")),
    ];
    fields.extend(crate::ssh_fields::ssh_auth_fields("ssh_", "SSH Tunnel", Some(crate::config_field::ShowIf::eq_str("use_ssh", "true"))));
    fields
}

pub struct RedisAdapter {
    store: Arc<pluk_store::Store>,
}

impl RedisAdapter {
    pub fn new(store: Arc<pluk_store::Store>) -> Arc<Self> {
        Arc::new(Self { store })
    }
}

#[async_trait]
impl Adapter for RedisAdapter {
    fn id(&self) -> &str {
        "redis"
    }
    fn label(&self) -> &str {
        "Redis"
    }
    fn category(&self) -> &str {
        "datastore"
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
            vec![
                ToolSpec::new("scan", "Incrementally list keys with SCAN (safe on large keyspaces). Returns a cursor and a page of keys.", "read"),
                ToolSpec::new("keys", "List keys matching a pattern with KEYS. Blocks the server on large keyspaces — prefer scan.", "read").with_default_enabled(false),
                ToolSpec::new("get", "Get the string value of a key.", "read"),
                ToolSpec::new("type", "Get the data type of a key (string/list/set/zset/hash/stream).", "read"),
                ToolSpec::new("ttl", "Get the remaining time-to-live of a key in seconds (-1 no expiry, -2 missing).", "read"),
                ToolSpec::new("info", "Get server INFO, optionally for a single section (e.g. memory, clients, stats).", "read").with_default_enabled(false),
                ToolSpec::new("set", "Set a key's string value, optionally with an expiry in seconds.", "write"),
                ToolSpec::new("expire", "Set a key's time-to-live in seconds.", "write"),
                ToolSpec::new("del", "Delete a key.", "delete"),
            ]
        })
    }
    fn config_fields(&self) -> &[ConfigField] {
        static FIELDS: std::sync::OnceLock<Vec<ConfigField>> = std::sync::OnceLock::new();
        FIELDS.get_or_init(redis_fields)
    }
    async fn test_connection(&self, conn: &Integration) -> Result<(), AdapterError> {
        client::test_redis(conn).await
    }
    fn instructions(&self, conn: &Integration) -> String {
        let enabled: Vec<&str> = self
            .tool_specs()
            .iter()
            .filter(|t| pluk_policy::tool_gate(conn.query_policy.as_deref()).enabled(&t.name, t.default_enabled))
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
                kind: "Redis".into(),
                access: "Read keys and values (scan/get/type/ttl/info); set/expire/delete when write is permitted. Every action is policy-checked and recorded in the activity log.".into(),
                policy: Some(policy),
                start: Some("scan".into()),
                hint: Some(AGENT_HINT.into()),
            },
        )
    }
    fn register(&self, host: &mut dyn ToolHost, conn: &Integration, _owner_id: &str) -> Result<(), AdapterError> {
        let store = self.store.clone();
        let cfg = redis_config_from(conn)?;
        let accessor = Arc::new(RedisAccessor::new(cfg));

        macro_rules! reg {
            ($name:expr, $desc:expr, $cat:expr, $schema:expr, $detail:expr, $body:expr) => {
                {
                    let store = store.clone();
                    let conn = conn.clone();
                    let acc = accessor.clone();
                    let handler: ToolHandler = Arc::new(move |args: Value| {
                        let store = store.clone();
                        let conn = conn.clone();
                        let acc = acc.clone();
                        let detail = $detail(&args);
                        let meta = GateMeta::new($cat, $name, detail);
                        let target = CallTarget::from(&conn);
                        Box::pin(async move {
                            run_gated(&store, &target, meta, |_| async {
                                let out = $body(args, acc).await?;
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
                                    result: Some(pluk_store::QueryResult { fields: vec![], rows }),
                                    ..Default::default()
                                }))
                            }, GateOpts::default())
                            .await
                        })
                    });
                    let props = $schema;
                    let schema = if props.is_empty() { Map::new() } else { object_schema(props, &[]) };
                    host.register_tool(
                        ToolRegistration {
                            name: $name.into(),
                            description: $desc.into(),
                            input_schema: schema,
                            annotations: Map::new(),
                        },
                        handler,
                    );
                }
            };
        }

        // scan
        reg!(
            "scan",
            "Incrementally list keys with SCAN (safe on large keyspaces). Returns a cursor and a page of keys.",
            "read",
            {
                let mut m = Map::new();
                m.insert("cursor".into(), json!({"type":"string","default":"0","description":"Cursor from a previous scan; start at 0"}));
                m.insert("match".into(), json!({"type":"string","description":"Glob pattern, e.g. user:*"}));
                m.insert("count".into(), json!({"type":"integer","minimum":1,"maximum":10000,"default":100,"description":"Approximate keys to scan per call"}));
                m
            },
            |args: &Value| {
                let m = args.get("match").and_then(|v| v.as_str()).unwrap_or("*");
                let c = args.get("count").and_then(|v| v.as_i64()).unwrap_or(100);
                format!("scan match={m} count={c}")
            },
            |args: Value, acc: Arc<RedisAccessor>| {
                Box::pin(async move {
                    let cursor = args.get("cursor").and_then(|v| v.as_str()).unwrap_or("0").to_string();
                    let pattern = args.get("match").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let count = args.get("count").and_then(|v| v.as_i64()).unwrap_or(100);
                    let mut cmd_args = vec![cursor.clone()];
                    if let Some(p) = pattern {
                        cmd_args.push("MATCH".to_string());
                        cmd_args.push(p);
                    }
                    cmd_args.push("COUNT".to_string());
                    cmd_args.push(count.to_string());
                    let res = acc.raw("SCAN", cmd_args).await?;
                    // runner returns JSON; if real redis, res is Value from redis crate (which decodes to Value)
                    // For SCAN, expect [cursor, [keys]] or object {cursor,keys}
                    if let Some(obj) = res.as_object()
                        && obj.contains_key("cursor") {
                            return Ok::<Value, AdapterError>(res);
                        }
                    if let Some(arr) = res.as_array()
                        && arr.len() == 2 {
                            let cursor = arr[0].clone();
                            let keys = arr[1].clone();
                            return Ok::<Value, AdapterError>(json!({"cursor": cursor, "keys": keys}));
                        }
                    Ok::<Value, AdapterError>(res)
                })
            }
        );

        // keys (off by default)
        reg!(
            "keys",
            "List keys matching a pattern with KEYS. Blocks the server on large keyspaces — prefer scan.",
            "read",
            {
                let mut m = Map::new();
                m.insert("pattern".into(), json!({"type":"string","default":"*","description":"Glob pattern, e.g. user:*"}));
                m
            },
            |args: &Value| format!("keys {}", args.get("pattern").and_then(|v| v.as_str()).unwrap_or("*")),
            |args: Value, acc: Arc<RedisAccessor>| {
                Box::pin(async move {
                    let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("*").to_string();
                    acc.raw("KEYS", vec![pattern]).await
                })
            }
        );

        // get
        reg!(
            "get",
            "Get the string value of a key.",
            "read",
            {
                let mut m = Map::new();
                m.insert("key".into(), json!({"type":"string","description":"Key name"}));
                m
            },
            |args: &Value| format!("get {}", args.get("key").and_then(|v| v.as_str()).unwrap_or("")),
            |args: Value, acc: Arc<RedisAccessor>| {
                Box::pin(async move {
                    let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    acc.get(&key).await
                })
            }
        );

        // type
        reg!(
            "type",
            "Get the data type of a key (string/list/set/zset/hash/stream).",
            "read",
            {
                let mut m = Map::new();
                m.insert("key".into(), json!({"type":"string","description":"Key name"}));
                m
            },
            |args: &Value| format!("type {}", args.get("key").and_then(|v| v.as_str()).unwrap_or("")),
            |args: Value, acc: Arc<RedisAccessor>| {
                Box::pin(async move {
                    let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    acc.raw("TYPE", vec![key]).await
                })
            }
        );

        // ttl
        reg!(
            "ttl",
            "Get the remaining time-to-live of a key in seconds (-1 no expiry, -2 missing).",
            "read",
            {
                let mut m = Map::new();
                m.insert("key".into(), json!({"type":"string","description":"Key name"}));
                m
            },
            |args: &Value| format!("ttl {}", args.get("key").and_then(|v| v.as_str()).unwrap_or("")),
            |args: Value, acc: Arc<RedisAccessor>| {
                Box::pin(async move {
                    let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    acc.ttl(&key).await
                })
            }
        );

        // info (off)
        reg!(
            "info",
            "Get server INFO, optionally for a single section (e.g. memory, clients, stats).",
            "read",
            {
                let mut m = Map::new();
                m.insert("section".into(), json!({"type":"string","description":"INFO section, e.g. memory"}));
                m
            },
            |args: &Value| format!("info {}", args.get("section").and_then(|v| v.as_str()).unwrap_or("all")),
            |args: Value, acc: Arc<RedisAccessor>| {
                Box::pin(async move {
                    let section = args.get("section").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let a = if let Some(s) = section { vec![s] } else { vec![] };
                    acc.raw("INFO", a).await
                })
            }
        );

        // set (write)
        reg!(
            "set",
            "Set a key's string value, optionally with an expiry in seconds.",
            "write",
            {
                let mut m = Map::new();
                m.insert("key".into(), json!({"type":"string","description":"Key name"}));
                m.insert("value".into(), json!({"type":"string","description":"String value"}));
                m.insert("ex".into(), json!({"type":"integer","minimum":1,"description":"Expiry in seconds (optional)"}));
                m
            },
            |args: &Value| {
                let k = args.get("key").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(ex) = args.get("ex").and_then(|v| v.as_i64()) {
                    format!("set {k} (ex={ex})")
                } else {
                    format!("set {k}")
                }
            },
            |args: Value, acc: Arc<RedisAccessor>| {
                Box::pin(async move {
                    let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let value = args.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let ex = args.get("ex").and_then(|v| v.as_i64());
                    let res: Value = acc.set(&key, &value).await?;
                    if let Some(seconds) = ex {
                        acc.expire(&key, seconds).await?;
                    }
                    Ok::<Value, AdapterError>(res)
                })
            }
        );

        // expire
        reg!(
            "expire",
            "Set a key's time-to-live in seconds.",
            "write",
            {
                let mut m = Map::new();
                m.insert("key".into(), json!({"type":"string","description":"Key name"}));
                m.insert("seconds".into(), json!({"type":"integer","minimum":1,"description":"TTL in seconds"}));
                m
            },
            |args: &Value| format!("expire {} {}", args.get("key").and_then(|v| v.as_str()).unwrap_or(""), args.get("seconds").and_then(|v| v.as_i64()).unwrap_or(0)),
            |args: Value, acc: Arc<RedisAccessor>| {
                Box::pin(async move {
                    let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let seconds = args.get("seconds").and_then(|v| v.as_i64()).unwrap_or(0);
                    acc.expire(&key, seconds).await
                })
            }
        );

        // del
        reg!(
            "del",
            "Delete a key.",
            "delete",
            {
                let mut m = Map::new();
                m.insert("key".into(), json!({"type":"string","description":"Key name"}));
                m
            },
            |args: &Value| format!("del {}", args.get("key").and_then(|v| v.as_str()).unwrap_or("")),
            |args: Value, acc: Arc<RedisAccessor>| {
                Box::pin(async move {
                    let key = args.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    acc.del(&key).await
                })
            }
        );

        Ok(())
    }
}

pub fn redis_adapters(store: Arc<pluk_store::Store>) -> Vec<Arc<dyn Adapter>> {
    vec![RedisAdapter::new(store)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn conn(config: Map<String, Value>) -> Integration {
        Integration {
            id: "r".into(),
            name: "Redis".into(),
            r#type: "redis".into(),
            config,
            environment: None,
            read_only: 0,
            query_policy: None,
            token: "t".into(),
            created_at: String::new(),
            via_group: None,
        }
    }

    #[test]
    fn build_url_encodes_and_picks_scheme() {
        assert_eq!(build_url("redis", "localhost", 6379, 0, ""), "redis://localhost:6379/0");
        assert_eq!(build_url("rediss", "h", 6380, 2, "p@ss"), "rediss://:p%40ss@h:6380/2");
    }

    #[test]
    fn redis_config_reads_and_defaults() {
        let mut m = Map::new();
        m.insert("host".into(), json!("h"));
        m.insert("port".into(), json!(6380));
        m.insert("db".into(), json!(2));
        m.insert("tls".into(), json!(true));
        m.insert("password".into(), json!("p"));
        let cfg = redis_config_from(&conn(m)).unwrap();
        assert_eq!(cfg.host, "h");
        assert_eq!(cfg.port, 6380);
        assert_eq!(cfg.db, 2);
        assert!(cfg.tls);
        assert_eq!(cfg.password, "p");
        assert!(cfg.ssh.is_none());
        assert!(cfg.url.is_none());
    }

    #[test]
    fn redis_config_prefers_url_when_no_tunnel() {
        let mut m = Map::new();
        m.insert("url".into(), json!("rediss://x.upstash.io:6379"));
        m.insert("host".into(), json!("ignored"));
        let cfg = redis_config_from(&conn(m)).unwrap();
        assert_eq!(cfg.url.as_deref(), Some("rediss://x.upstash.io:6379"));
    }

    #[test]
    fn redis_config_parses_ssh_block() {
        let mut m = Map::new();
        m.insert("host".into(), json!("127.0.0.1"));
        m.insert("use_ssh".into(), json!(true));
        m.insert("ssh_host".into(), json!("bastion"));
        m.insert("ssh_port".into(), json!(2222));
        m.insert("ssh_user".into(), json!("deploy"));
        m.insert("ssh_auth_type".into(), json!("key"));
        m.insert("ssh_key_path".into(), json!("~/.ssh/id_ed25519"));
        let cfg = redis_config_from(&conn(m)).unwrap();
        let ssh = cfg.ssh.unwrap();
        assert_eq!(ssh.host, "bastion");
        assert_eq!(ssh.port, 2222);
        assert_eq!(ssh.user, "deploy");
        assert_eq!(ssh.auth_type, "key");
        assert_eq!(ssh.key_path.as_deref(), Some("~/.ssh/id_ed25519"));
    }

    #[test]
    fn redis_config_ignores_url_when_tunnel_present() {
        let mut m = Map::new();
        m.insert("url".into(), json!("rediss://x"));
        m.insert("host".into(), json!("10.0.0.5"));
        m.insert("use_ssh".into(), json!(true));
        m.insert("ssh_host".into(), json!("bastion"));
        let cfg = redis_config_from(&conn(m)).unwrap();
        assert!(cfg.url.is_none());
        assert_eq!(cfg.host, "10.0.0.5");
    }

    #[test]
    fn redis_config_rejects_missing_host() {
        let m = Map::new();
        assert!(redis_config_from(&conn(m)).is_err());
    }

    #[test]
    fn tool_specs_defaults() {
        let store = {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("pluk.db");
            Arc::new(pluk_store::Store::open(&path).unwrap())
        };
        let adapter = RedisAdapter::new(store);
        let specs = adapter.tool_specs();
        assert_eq!(specs.len(), 9);
        let keys = specs.iter().find(|t| t.name == "keys").unwrap();
        assert!(!keys.default_enabled, "keys must be off");
        let info = specs.iter().find(|t| t.name == "info").unwrap();
        assert!(!info.default_enabled, "info must be off");
        let scan = specs.iter().find(|t| t.name == "scan").unwrap();
        assert!(scan.default_enabled);
        // scan must not have `only` param (verify by checking no extra impl — just ensure spec category)
        assert_eq!(scan.category, "read");
        let set = specs.iter().find(|t| t.name == "set").unwrap();
        assert_eq!(set.category, "write");
        assert!(!set.default_enabled);
    }

    #[tokio::test]
    async fn lazy_accessor_opens_once_and_reused() {
        let _g = lock();
        let mut m = Map::new();
        m.insert("host".into(), json!("127.0.0.1"));
        let cfg = redis_config_from(&conn(m)).unwrap();
        let acc = RedisAccessor::new(cfg);
        let acc2 = acc.clone();

        let calls = Arc::new(AtomicUsize::new(0));
        let calls2 = calls.clone();
        let factory = Arc::new(move |_cfg: RedisConfig| {
            let c = calls2.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(Arc::new(client::RedisResource { url: "redis://127.0.0.1:6379/0".into(), tunnel: None }))
            }) as _
        });
        set_redis_factory(Some(factory));

        // runner that returns ok
        set_redis_runner(Some(Arc::new(|cmd: String, _args: Vec<String>| {
            Box::pin(async move { Ok(json!(format!("ok-{cmd}"))) }) as _
        })));

        let r1 = acc.get_resource().await.unwrap();
        let r2 = acc2.get_resource().await.unwrap();
        assert_eq!(r1.url, r2.url);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "open should happen once");
        // additional gets still 1
        let _ = acc.raw("GET", vec!["k".into()]).await.unwrap();
        let _ = acc2.raw("GET", vec!["k2".into()]).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        set_redis_factory(None);
        set_redis_runner(None);
    }

    #[tokio::test]
    async fn redis_command_construction_per_tool() {
        let _g = lock();
        let mut m = Map::new();
        m.insert("host".into(), json!("127.0.0.1"));
        let cfg = redis_config_from(&conn(m)).unwrap();
        let acc = Arc::new(RedisAccessor::new(cfg));

        let captured: Arc<Mutex<Vec<(String, Vec<String>)>>> = Arc::new(Mutex::new(Vec::new()));
        let cap = captured.clone();
        set_redis_runner(Some(Arc::new(move |cmd: String, args: Vec<String>| {
            let c = cap.clone();
            Box::pin(async move {
                c.lock().unwrap().push((cmd.clone(), args.clone()));
                match cmd.as_str() {
                    "SCAN" => Ok(json!(["0", ["a", "b"]])),
                    "GET" => Ok(json!("val")),
                    "TYPE" => Ok(json!("string")),
                    "TTL" => Ok(json!(42)),
                    "INFO" => Ok(json!("# Server")),
                    "KEYS" => Ok(json!(["k1"])),
                    "SET" => Ok(json!("OK")),
                    "EXPIRE" => Ok(json!(1)),
                    "DEL" => Ok(json!(1)),
                    "PING" => Ok(json!("PONG")),
                    _ => Ok(json!(null)),
                }
            }) as _
        })));
        // need factory to avoid ssh
        set_redis_factory(Some(Arc::new(|_cfg| {
            Box::pin(async move { Ok(Arc::new(client::RedisResource { url: "redis://127.0.0.1:6379/0".into(), tunnel: None })) }) as _
        })));

        // scan with match and count
        let res = acc.raw("SCAN", vec!["0".into(), "MATCH".into(), "user:*".into(), "COUNT".into(), "100".into()]).await.unwrap();
        assert!(res.is_array());
        // get
        let v = acc.get("mykey").await.unwrap();
        assert_eq!(v, json!("val"));
        // type
        let v = acc.raw("TYPE", vec!["mykey".into()]).await.unwrap();
        assert_eq!(v, json!("string"));
        // ttl
        let v = acc.ttl("mykey").await.unwrap();
        assert_eq!(v, json!(42));
        // set + expire path (simulate tool set)
        let _ = acc.set("k", "v").await.unwrap();
        let _ = acc.expire("k", 60).await.unwrap();
        // keys
        let _ = acc.raw("KEYS", vec!["*".into()]).await.unwrap();
        // info
        let _ = acc.raw("INFO", vec!["memory".into()]).await.unwrap();
        // del
        let _ = acc.del("k").await.unwrap();

        let log = captured.lock().unwrap().clone();
        // check some commands present
        assert!(log.iter().any(|(c, a)| c == "SCAN" && a.contains(&"MATCH".to_string())));
        assert!(log.iter().any(|(c, _)| c == "GET"));
        assert!(log.iter().any(|(c, _)| c == "TYPE"));
        assert!(log.iter().any(|(c, _)| c == "TTL"));
        assert!(log.iter().any(|(c, _)| c == "SET"));
        assert!(log.iter().any(|(c, _)| c == "EXPIRE"));
        assert!(log.iter().any(|(c, _)| c == "KEYS"));
        assert!(log.iter().any(|(c, _)| c == "INFO"));
        assert!(log.iter().any(|(c, _)| c == "DEL"));

        set_redis_runner(None);
        set_redis_factory(None);
    }

    #[tokio::test]
    async fn api_error_surfaces_clearly() {
        let _g = lock();
        let mut m = Map::new();
        m.insert("host".into(), json!("127.0.0.1"));
        let cfg = redis_config_from(&conn(m)).unwrap();
        let acc = RedisAccessor::new(cfg);
        set_redis_factory(Some(Arc::new(|_cfg| {
            Box::pin(async move { Ok(Arc::new(client::RedisResource { url: "redis://127.0.0.1:6379/0".into(), tunnel: None })) }) as _
        })));
        set_redis_runner(Some(Arc::new(|_cmd: String, _args: Vec<String>| {
            Box::pin(async move { Err(AdapterError::new("Redis GET failed: connection refused")) }) as _
        })));
        let err = acc.get("k").await.unwrap_err();
        assert!(err.message.contains("connection refused"));
        set_redis_runner(None);
        set_redis_factory(None);
    }

    #[tokio::test]
    async fn test_connection_rejects_blank_host() {
        let m = Map::new();
        let bad_conn = conn(m);
        let store = {
            let dir = tempfile::tempdir().unwrap();
            let p = dir.path().join("pluk.db");
            Arc::new(pluk_store::Store::open(&p).unwrap())
        };
        let adapter = RedisAdapter::new(store);
        let err = adapter.test_connection(&bad_conn).await.unwrap_err();
        assert!(err.message.contains("host is missing"));
    }
}
