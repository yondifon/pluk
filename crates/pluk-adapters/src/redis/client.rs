use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;

use crate::error::AdapterError;
use pluk_ssh::openssh::{SshTunnelConfig, Tunnel};

fn redis_value_to_json(v: redis::Value) -> Value {
    match v {
        redis::Value::Nil => Value::Null,
        redis::Value::Int(n) => serde_json::json!(n),
        redis::Value::BulkString(b) => String::from_utf8_lossy(&b).into(),
        redis::Value::SimpleString(s) => Value::String(s),
        redis::Value::Okay => Value::String("OK".into()),
        redis::Value::Array(arr) => {
            Value::Array(arr.into_iter().map(redis_value_to_json).collect())
        }
        redis::Value::Map(m) => {
            let mut map = serde_json::Map::new();
            for (k, v2) in m {
                let key = match redis_value_to_json(k) {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                map.insert(key, redis_value_to_json(v2));
            }
            Value::Object(map)
        }
        redis::Value::Double(d) => serde_json::json!(d),
        redis::Value::Boolean(b) => Value::Bool(b),
        _ => Value::Null,
    }
}

#[derive(Debug, Clone)]
pub struct SshParams {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth_type: String,
    pub key_path: Option<String>,
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RedisConfig {
    pub url: Option<String>,
    pub host: String,
    pub port: u16,
    pub db: i64,
    pub tls: bool,
    pub password: String,
    pub ssh: Option<SshParams>,
}

pub fn redis_config_from(conn: &pluk_store::Integration) -> Result<RedisConfig, AdapterError> {
    let c = &conn.config;
    let use_ssh = match c.get("use_ssh") {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "true",
        _ => false,
    };
    let ssh = if use_ssh {
        c.get("ssh_host")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|ssh_host| SshParams {
                host: ssh_host,
                port: c.get("ssh_port").and_then(|v| v.as_u64()).unwrap_or(22) as u16,
                user: c
                    .get("ssh_user")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                auth_type: c
                    .get("ssh_auth_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("agent")
                    .to_string(),
                key_path: c
                    .get("ssh_key_path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty()),
                passphrase: c
                    .get("ssh_password")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty()),
            })
    } else {
        None
    };

    let explicit = c
        .get("url")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(url) = explicit
        && ssh.is_none()
    {
        return Ok(RedisConfig {
            url: Some(url),
            host: String::new(),
            port: 0,
            db: 0,
            tls: false,
            password: String::new(),
            ssh: None,
        });
    }

    let host = c
        .get("host")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let host = match host {
        Some(h) => h,
        None => {
            return Err(AdapterError::new(
                "Redis host is missing. Set it in the integration config.",
            ));
        }
    };
    let port = c.get("port").and_then(|v| v.as_u64()).unwrap_or(6379) as u16;
    let db = c.get("db").and_then(|v| v.as_i64()).unwrap_or(
        c.get("db")
            .and_then(|v| v.as_u64().map(|n| n as i64))
            .unwrap_or(0),
    );
    let tls = match c.get("tls") {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "true",
        _ => false,
    };
    let password = c
        .get("password")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(RedisConfig {
        url: None,
        host,
        port,
        db,
        tls,
        password,
        ssh,
    })
}

pub fn build_url(scheme: &str, host: &str, port: u16, db: i64, password: &str) -> String {
    let auth = if password.is_empty() {
        String::new()
    } else {
        format!(":{}@", urlencoding::encode(password))
    };
    format!("{scheme}://{auth}{host}:{port}/{db}")
}

// ── lazy accessor with tunnel handling ───────────────────────────────────────

pub struct RedisResource {
    pub url: String,
    pub tunnel: Option<Arc<Tunnel>>,
}

// Test hook: override opening
pub type RedisFactory = Arc<
    dyn Fn(
            RedisConfig,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Arc<RedisResource>, AdapterError>> + Send>,
        > + Send
        + Sync,
>;

static FACTORY: OnceLock<Mutex<Option<RedisFactory>>> = OnceLock::new();
fn factory_slot() -> &'static Mutex<Option<RedisFactory>> {
    FACTORY.get_or_init(|| Mutex::new(None))
}
pub fn set_redis_factory(f: Option<RedisFactory>) {
    *factory_slot().lock().unwrap() = f;
}
fn get_factory() -> Option<RedisFactory> {
    factory_slot().lock().unwrap().clone()
}

// Runner for command execution (used by tools)
pub type RedisRunner = Arc<
    dyn Fn(
            String,
            Vec<String>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Value, AdapterError>> + Send>,
        > + Send
        + Sync,
>;

static RUNNER: OnceLock<Mutex<Option<RedisRunner>>> = OnceLock::new();
fn runner_slot() -> &'static Mutex<Option<RedisRunner>> {
    RUNNER.get_or_init(|| Mutex::new(None))
}
pub fn set_redis_runner(r: Option<RedisRunner>) {
    *runner_slot().lock().unwrap() = r;
}
fn get_runner() -> Option<RedisRunner> {
    runner_slot().lock().unwrap().clone()
}

async fn open_resource(cfg: RedisConfig) -> Result<Arc<RedisResource>, AdapterError> {
    if let Some(factory) = get_factory() {
        return factory(cfg).await;
    }
    if let Some(ref url) = cfg.url {
        return Ok(Arc::new(RedisResource {
            url: url.clone(),
            tunnel: None,
        }));
    }
    if let Some(ssh) = cfg.ssh.clone() {
        let tunnel_cfg = SshTunnelConfig {
            host: ssh.host.clone(),
            port: ssh.port,
            user: ssh.user.clone(),
            auth_type: ssh.auth_type.clone(),
            key_path: ssh.key_path.clone(),
            passphrase: ssh.passphrase.clone(),
            remote_host: cfg.host.clone(),
            remote_port: cfg.port,
            local_port: None,
        };
        let tunnel = pluk_ssh::open_ssh_tunnel(tunnel_cfg, None)
            .await
            .map_err(|e| AdapterError::new(format!("SSH tunnel failed: {e}")))?;
        let url = build_url(
            "redis",
            "127.0.0.1",
            tunnel.local_port,
            cfg.db,
            &cfg.password,
        );
        return Ok(Arc::new(RedisResource {
            url,
            tunnel: Some(Arc::new(tunnel)),
        }));
    }
    let scheme = if cfg.tls { "rediss" } else { "redis" };
    let url = build_url(scheme, &cfg.host, cfg.port, cfg.db, &cfg.password);
    Ok(Arc::new(RedisResource { url, tunnel: None }))
}

#[derive(Clone)]
pub struct RedisAccessor {
    config: RedisConfig,
    cell: Arc<tokio::sync::OnceCell<Arc<RedisResource>>>,
    open_count: Arc<AtomicUsize>,
}

impl RedisAccessor {
    pub fn new(config: RedisConfig) -> Self {
        Self {
            config,
            cell: Arc::new(tokio::sync::OnceCell::new()),
            open_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub async fn get_resource(&self) -> Result<Arc<RedisResource>, AdapterError> {
        let cfg = self.config.clone();
        let count = self.open_count.clone();
        let res = self
            .cell
            .get_or_try_init(|| async move {
                count.fetch_add(1, Ordering::SeqCst);
                open_resource(cfg).await
            })
            .await?;
        Ok(res.clone())
    }

    pub fn open_count(&self) -> usize {
        self.open_count.load(Ordering::SeqCst)
    }

    pub async fn raw(&self, cmd: &str, args: Vec<String>) -> Result<Value, AdapterError> {
        let resource = self.get_resource().await?;
        if let Some(runner) = get_runner() {
            return runner(cmd.to_string(), args).await;
        }
        run_real_redis_command(&resource.url, cmd, args).await
    }

    // Typed helpers
    pub async fn get(&self, key: &str) -> Result<Value, AdapterError> {
        self.raw("GET", vec![key.to_string()]).await
    }
    pub async fn ttl(&self, key: &str) -> Result<Value, AdapterError> {
        self.raw("TTL", vec![key.to_string()]).await
    }
    pub async fn set(&self, key: &str, value: &str) -> Result<Value, AdapterError> {
        self.raw("SET", vec![key.to_string(), value.to_string()])
            .await
    }
    pub async fn expire(&self, key: &str, seconds: i64) -> Result<Value, AdapterError> {
        self.raw("EXPIRE", vec![key.to_string(), seconds.to_string()])
            .await
    }
    pub async fn del(&self, key: &str) -> Result<Value, AdapterError> {
        self.raw("DEL", vec![key.to_string()]).await
    }
}

/// Build a client for `url`. `rediss://` needs the crate's rustls backend, so a
/// URL that parses here is one this build can actually connect over.
fn redis_client(url: &str) -> Result<redis::Client, AdapterError> {
    redis::Client::open(url.to_string())
        .map_err(|e| AdapterError::new(format!("Redis client error: {e}")))
}

async fn run_real_redis_command(
    url: &str,
    cmd: &str,
    args: Vec<String>,
) -> Result<Value, AdapterError> {
    let client = redis_client(url)?;
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| AdapterError::new(format!("Redis connection failed: {e}")))?;
    let cmd_upper = cmd.to_uppercase();
    // Use redis crate's cmd interface
    let mut rcmd = redis::cmd(&cmd_upper);
    for a in &args {
        rcmd.arg(a);
    }
    let redis_val: redis::Value = rcmd
        .query_async(&mut conn)
        .await
        .map_err(|e| AdapterError::new(format!("Redis {cmd_upper} failed: {e}")))?;
    // Convert redis::Value → serde_json::Value via string round-trip helper.
    let res = redis_value_to_json(redis_val);
    Ok(res)
}

pub async fn test_redis(conn: &pluk_store::Integration) -> Result<(), AdapterError> {
    let cfg = redis_config_from(conn)?;
    let resource = open_resource(cfg).await?;
    if let Some(runner) = get_runner() {
        // use runner to simulate PING
        runner("PING".to_string(), vec![]).await.map(|_| ())?;
        return Ok(());
    }
    // real ping
    let client = redis_client(&resource.url)?;
    let mut c = client
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| AdapterError::new(format!("Redis connection failed: {e}")))?;
    redis::cmd("PING")
        .query_async::<()>(&mut c)
        .await
        .map_err(|e| AdapterError::new(format!("Redis PING failed: {e}")))?;
    Ok(())
}

// Helper to capture open counts across clones for testing
pub fn accessor_for_test(cfg: RedisConfig) -> RedisAccessor {
    RedisAccessor::new(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_schemes_build_a_client() {
        // A build without the TLS backend rejects `rediss://` outright, which is
        // every managed provider.
        assert!(redis_client("rediss://:pw@example.upstash.io:6379/0").is_ok());
        assert!(redis_client("redis://127.0.0.1:6379/0").is_ok());
    }
}
