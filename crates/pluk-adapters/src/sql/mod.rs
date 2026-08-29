pub mod api;
pub mod error;
pub mod fields;
pub mod server;

pub use error::{classify_sql_error, format_sql_error, humanize_sql_error, SqlErrorCategory, SqlErrorInfo};
pub use fields::{network_sql_fields, sqlite_fields};
pub use server::{sql_agent_hint, sql_instructions, sql_label, sql_tool_specs, register_sql_server, SqlCancelRegistry};

use std::sync::Arc;

use async_trait::async_trait;
use pluk_store::Integration;

use crate::adapter::{Adapter, ApiRequest, ApiResponse, PolicyKind};
use crate::config_field::ConfigField;
use crate::error::AdapterError;
use crate::tool_host::ToolHost;
use crate::tool_spec::ToolSpec;

use pluk_store::Store;
use pluk_db::factory::{CreateDriverOpts, create_driver};
use pluk_db::config::SqlConfig as DbSqlConfig;

fn db_config_from(conn: &Integration) -> DbSqlConfig {
    let mut cfg = DbSqlConfig::default();
    cfg.r#type = conn.r#type.clone();
    cfg.host = conn.config.get("host").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.port = conn.config.get("port").and_then(|v| v.as_u64()).map(|n| n as u16)
        .or_else(|| conn.config.get("port").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()));
    cfg.user = conn.config.get("user").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.password = conn.config.get("password").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.database = conn.config.get("database").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.filename = conn.config.get("filename").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.socket_path = conn.config.get("socket_path").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.use_ssl = conn.config.get("use_ssl").and_then(|v| v.as_bool()).unwrap_or(false)
        || conn.config.get("use_ssl").and_then(|v| v.as_str()) == Some("true");
    cfg.ssl_mode = conn.config.get("ssl_mode").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.ssl_ca_path = conn.config.get("ssl_ca_path").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.ssl_cert_path = conn.config.get("ssl_cert_path").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.ssl_key_path = conn.config.get("ssl_key_path").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.use_ssh = conn.config.get("use_ssh").map(|v| match v {
        serde_json::Value::Bool(b) => if *b { "true".to_string() } else { "false".to_string() },
        serde_json::Value::String(s) => s.clone(),
        _ => "".to_string(),
    });
    cfg.ssh_host = conn.config.get("ssh_host").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.ssh_port = conn.config.get("ssh_port").and_then(|v| v.as_u64()).map(|n| n as u16)
        .or_else(|| conn.config.get("ssh_port").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()));
    cfg.ssh_user = conn.config.get("ssh_user").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.ssh_auth_type = conn.config.get("ssh_auth_type").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.ssh_key_path = conn.config.get("ssh_key_path").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg.ssh_password = conn.config.get("ssh_password").and_then(|v| v.as_str()).map(|s| s.to_string());
    cfg
}

async fn test_sql(conn: &Integration, _store: Option<Arc<Store>>) -> Result<(), AdapterError> {
    // Force-evict would go here: we have no global pool, so no-op
    let cfg = db_config_from(conn);
    // Use factory to test connection (it will create fake driver for postgres/mysql)
    let dw = create_driver(CreateDriverOpts::new(cfg)).await.map_err(crate::sql::error::driver_error_to_adapter)?;
    let res = dw.driver.test_connection().await;
    let _ = dw.close().await;
    res.map_err(crate::sql::error::driver_error_to_adapter)
}

pub struct SqlAdapter {
    id: &'static str,
    label: &'static str,
    store: Arc<Store>,
    cancels: Arc<SqlCancelRegistry>,
}

impl SqlAdapter {
    pub fn postgres(store: Arc<Store>, cancels: Arc<SqlCancelRegistry>) -> Arc<Self> {
        Arc::new(Self { id: "postgres", label: "PostgreSQL", store, cancels })
    }
    pub fn mysql(store: Arc<Store>, cancels: Arc<SqlCancelRegistry>) -> Arc<Self> {
        Arc::new(Self { id: "mysql", label: "MySQL", store, cancels })
    }
    pub fn sqlite(store: Arc<Store>, cancels: Arc<SqlCancelRegistry>) -> Arc<Self> {
        Arc::new(Self { id: "sqlite", label: "SQLite", store, cancels })
    }
}

#[async_trait]
impl Adapter for SqlAdapter {
    fn id(&self) -> &str { self.id }
    fn label(&self) -> &str { self.label }
    fn category(&self) -> &str { "database" }
    fn policy_kind(&self) -> PolicyKind { PolicyKind::Sql }
    fn agent_hint(&self) -> &str {
        // leak to satisfy static? Use owned string via Box::leak for now
        // Instead return static str via sql_agent_hint but need owned
        // We can return leaked string: not ideal but works for trait requiring &str
        // Use match to return static literals
        match self.id {
            "postgres" => "Use this to query and inspect a PostgreSQL database — read schema and rows, run SELECTs, and write only when the policy permits. Use SELECT with LIMIT for production data.",
            "mysql" => "Use this to query and inspect a MySQL database — read schema and rows, run SELECTs, and write only when the policy permits. Use SELECT with LIMIT for production data.",
            _ => "Use this to query and inspect a SQLite database — read schema and rows, run SELECTs, and write only when the policy permits. Use SELECT with LIMIT before wider queries.",
        }
    }
    fn tool_specs(&self) -> &[ToolSpec] {
        // Return leaked vec? Need static slice. Use OnceLock.
        static POSTGRES_SPECS: std::sync::OnceLock<Vec<ToolSpec>> = std::sync::OnceLock::new();
        static MYSQL_SPECS: std::sync::OnceLock<Vec<ToolSpec>> = std::sync::OnceLock::new();
        static SQLITE_SPECS: std::sync::OnceLock<Vec<ToolSpec>> = std::sync::OnceLock::new();
        match self.id {
            "postgres" => POSTGRES_SPECS.get_or_init(sql_tool_specs),
            "mysql" => MYSQL_SPECS.get_or_init(sql_tool_specs),
            _ => SQLITE_SPECS.get_or_init(sql_tool_specs),
        }
    }
    fn config_fields(&self) -> &[ConfigField] {
        static PG_FIELDS: std::sync::OnceLock<Vec<ConfigField>> = std::sync::OnceLock::new();
        static MY_FIELDS: std::sync::OnceLock<Vec<ConfigField>> = std::sync::OnceLock::new();
        static SQ_FIELDS: std::sync::OnceLock<Vec<ConfigField>> = std::sync::OnceLock::new();
        match self.id {
            "postgres" => PG_FIELDS.get_or_init(|| network_sql_fields(5432)),
            "mysql" => MY_FIELDS.get_or_init(|| network_sql_fields(3306)),
            _ => SQ_FIELDS.get_or_init(sqlite_fields),
        }
    }
    async fn test_connection(&self, conn: &Integration) -> Result<(), AdapterError> {
        test_sql(conn, Some(self.store.clone())).await
    }
    fn humanize_error(&self, error: &AdapterError) -> Option<String> {
        Some(humanize_sql_error(error))
    }
    async fn handle_api(&self, conn: &Integration, request: ApiRequest, subpath: &str) -> Option<ApiResponse> {
        api::handle_sql_api(self.store.clone(), conn, request, subpath).await
    }
    async fn handle_global_api(&self, request: ApiRequest, path: &str) -> Option<ApiResponse> {
        // global cancel
        if path.starts_with("/api/log/") && path.ends_with("/cancel") {
            let re = regex::Regex::new(r"^/api/log/(\d+)/cancel$").unwrap();
            if let Some(caps) = re.captures(path)
                && request.method == "POST" {
                    let id: i64 = caps.get(1).unwrap().as_str().parse().unwrap_or(0);
                    let ok = self.cancels.cancel(id);
                    return Some(ApiResponse::json(200, &serde_json::json!({ "ok": ok })));
                }
        }
        None
    }
    fn instructions(&self, conn: &Integration) -> String { sql_instructions(conn) }
    fn register(&self, host: &mut dyn ToolHost, conn: &Integration, owner_id: &str) -> Result<(), AdapterError> {
        register_sql_server(host, conn, owner_id, self.store.clone(), self.cancels.clone())
    }
}

pub fn sql_adapters(store: Arc<Store>, cancels: Arc<SqlCancelRegistry>) -> Vec<Arc<dyn Adapter>> {
    vec![
        SqlAdapter::postgres(store.clone(), cancels.clone()),
        SqlAdapter::mysql(store.clone(), cancels.clone()),
        SqlAdapter::sqlite(store.clone(), cancels.clone()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_test_preserves_network_and_tls_config() {
        let conn = Integration {
            id: "pg".into(), name: "Postgres".into(), r#type: "postgres".into(),
            config: serde_json::from_value(serde_json::json!({
                "host": "db.internal", "port": 5432, "ssh_host": "bastion", "ssh_port": 2222,
                "use_ssh": true, "use_ssl": true, "ssl_mode": "require",
                "ssl_ca_path": "/tmp/ca.pem", "ssl_cert_path": "/tmp/client.pem", "ssl_key_path": "/tmp/client.key"
            })).unwrap(),
            environment: None, read_only: 0, query_policy: None, token: "token".into(),
            created_at: String::new(), via_group: None,
        };
        let cfg = db_config_from(&conn);
        assert_eq!(cfg.host.as_deref(), Some("db.internal"));
        assert_eq!(cfg.port, Some(5432));
        assert_eq!(cfg.ssh_port, Some(2222));
        assert!(cfg.is_use_ssh());
        assert!(cfg.use_ssl);
        assert_eq!(cfg.ssl_mode.as_deref(), Some("require"));
        assert_eq!(cfg.ssl_ca_path.as_deref(), Some("/tmp/ca.pem"));
        assert_eq!(cfg.ssl_cert_path.as_deref(), Some("/tmp/client.pem"));
        assert_eq!(cfg.ssl_key_path.as_deref(), Some("/tmp/client.key"));
    }
}
