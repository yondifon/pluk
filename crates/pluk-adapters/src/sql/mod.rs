pub mod api;
pub mod error;
pub mod fields;
pub mod server;

pub use error::{
    SqlErrorCategory, SqlErrorInfo, classify_sql_error, format_sql_error, humanize_sql_error,
};
pub use fields::{mssql_fields, network_sql_fields, sqlite_fields};
pub use server::{
    SqlCancelRegistry, evict_connections, register_sql_server, sql_agent_hint, sql_instructions,
    sql_label, sql_tool_specs,
};

use std::sync::Arc;

use async_trait::async_trait;
use pluk_store::Integration;

use crate::adapter::{Adapter, ApiRequest, ApiResponse, PolicyKind};
use crate::config_field::ConfigField;
use crate::error::AdapterError;
use crate::tool_host::ToolHost;
use crate::tool_spec::ToolSpec;

use pluk_store::Store;

async fn test_sql(conn: &Integration, _store: Option<Arc<Store>>) -> Result<(), AdapterError> {
    let cfg = server::sql_config_from(conn, None);
    server::with_driver_waiting(cfg, true, server::Patience::WATCHED, |driver| async move {
        driver.test_connection().await
    })
    .await
}

pub struct SqlAdapter {
    id: &'static str,
    label: &'static str,
    store: Arc<Store>,
    cancels: Arc<SqlCancelRegistry>,
}

impl SqlAdapter {
    pub fn postgres(store: Arc<Store>, cancels: Arc<SqlCancelRegistry>) -> Arc<Self> {
        Arc::new(Self {
            id: "postgres",
            label: "PostgreSQL",
            store,
            cancels,
        })
    }
    pub fn mysql(store: Arc<Store>, cancels: Arc<SqlCancelRegistry>) -> Arc<Self> {
        Arc::new(Self {
            id: "mysql",
            label: "MySQL",
            store,
            cancels,
        })
    }
    pub fn sqlite(store: Arc<Store>, cancels: Arc<SqlCancelRegistry>) -> Arc<Self> {
        Arc::new(Self {
            id: "sqlite",
            label: "SQLite",
            store,
            cancels,
        })
    }
    pub fn mssql(store: Arc<Store>, cancels: Arc<SqlCancelRegistry>) -> Arc<Self> {
        Arc::new(Self {
            id: "mssql",
            label: "Microsoft SQL Server",
            store,
            cancels,
        })
    }
}

#[async_trait]
impl Adapter for SqlAdapter {
    fn id(&self) -> &str {
        self.id
    }
    fn label(&self) -> &str {
        self.label
    }
    fn category(&self) -> &str {
        "database"
    }
    fn policy_kind(&self) -> PolicyKind {
        PolicyKind::Sql
    }
    fn agent_hint(&self) -> &str {
        // leak to satisfy static? Use owned string via Box::leak for now
        // Instead return static str via sql_agent_hint but need owned
        // We can return leaked string: not ideal but works for trait requiring &str
        // Use match to return static literals
        match self.id {
            "postgres" => {
                "Use this to query and inspect a PostgreSQL database — read schema and rows, run SELECTs, and write only when the policy permits. Use SELECT with LIMIT for production data."
            }
            "mysql" => {
                "Use this to query and inspect a MySQL database — read schema and rows, run SELECTs, and write only when the policy permits. Use SELECT with LIMIT for production data."
            }
            "mssql" => {
                "Use this to query and inspect a Microsoft SQL Server database — read schema and rows, run SELECTs, and write only when the policy permits. Use SELECT with TOP or OFFSET/FETCH for production data."
            }
            _ => {
                "Use this to query and inspect a SQLite database — read schema and rows, run SELECTs, and write only when the policy permits. Use SELECT with LIMIT before wider queries."
            }
        }
    }
    fn tool_specs(&self) -> &[ToolSpec] {
        // Return leaked vec? Need static slice. Use OnceLock.
        static POSTGRES_SPECS: std::sync::OnceLock<Vec<ToolSpec>> = std::sync::OnceLock::new();
        static MYSQL_SPECS: std::sync::OnceLock<Vec<ToolSpec>> = std::sync::OnceLock::new();
        static SQLITE_SPECS: std::sync::OnceLock<Vec<ToolSpec>> = std::sync::OnceLock::new();
        static MSSQL_SPECS: std::sync::OnceLock<Vec<ToolSpec>> = std::sync::OnceLock::new();
        match self.id {
            "postgres" => POSTGRES_SPECS.get_or_init(sql_tool_specs),
            "mysql" => MYSQL_SPECS.get_or_init(sql_tool_specs),
            "mssql" => MSSQL_SPECS.get_or_init(sql_tool_specs),
            _ => SQLITE_SPECS.get_or_init(sql_tool_specs),
        }
    }
    fn config_fields(&self) -> &[ConfigField] {
        static PG_FIELDS: std::sync::OnceLock<Vec<ConfigField>> = std::sync::OnceLock::new();
        static MY_FIELDS: std::sync::OnceLock<Vec<ConfigField>> = std::sync::OnceLock::new();
        static SQ_FIELDS: std::sync::OnceLock<Vec<ConfigField>> = std::sync::OnceLock::new();
        static MSSQL_FIELDS: std::sync::OnceLock<Vec<ConfigField>> = std::sync::OnceLock::new();
        match self.id {
            "postgres" => PG_FIELDS.get_or_init(|| network_sql_fields(5432)),
            "mysql" => MY_FIELDS.get_or_init(|| network_sql_fields(3306)),
            "mssql" => MSSQL_FIELDS.get_or_init(mssql_fields),
            _ => SQ_FIELDS.get_or_init(sqlite_fields),
        }
    }
    async fn test_connection(&self, conn: &Integration) -> Result<(), AdapterError> {
        test_sql(conn, Some(self.store.clone())).await
    }
    fn humanize_error(&self, error: &AdapterError) -> Option<String> {
        Some(humanize_sql_error(error))
    }
    async fn handle_api(
        &self,
        conn: &Integration,
        request: ApiRequest,
        subpath: &str,
    ) -> Option<ApiResponse> {
        api::handle_sql_api(self.store.clone(), conn, request, subpath).await
    }
    async fn handle_global_api(&self, request: ApiRequest, path: &str) -> Option<ApiResponse> {
        // global cancel
        if path.starts_with("/api/log/") && path.ends_with("/cancel") {
            let re = regex::Regex::new(r"^/api/log/(\d+)/cancel$").unwrap();
            if let Some(caps) = re.captures(path)
                && request.method == "POST"
            {
                let id: i64 = caps.get(1).unwrap().as_str().parse().unwrap_or(0);
                let ok = self.cancels.cancel(id);
                return Some(ApiResponse::json(200, &serde_json::json!({ "ok": ok })));
            }
        }
        None
    }
    fn instructions(&self, conn: &Integration) -> String {
        sql_instructions(conn)
    }
    fn register(
        &self,
        host: &mut dyn ToolHost,
        conn: &Integration,
        owner_id: &str,
    ) -> Result<(), AdapterError> {
        register_sql_server(
            host,
            conn,
            owner_id,
            self.store.clone(),
            self.cancels.clone(),
        )
    }
}

pub fn sql_adapters(store: Arc<Store>, cancels: Arc<SqlCancelRegistry>) -> Vec<Arc<dyn Adapter>> {
    vec![
        SqlAdapter::postgres(store.clone(), cancels.clone()),
        SqlAdapter::mysql(store.clone(), cancels.clone()),
        SqlAdapter::mssql(store.clone(), cancels.clone()),
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
        let cfg = server::sql_config_from(&conn, None);
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

    #[test]
    fn connection_test_preserves_mssql_tls_config() {
        let conn = Integration {
            id: "mssql".into(),
            name: "MSSQL".into(),
            r#type: "mssql".into(),
            config: serde_json::from_value(serde_json::json!({
                "host": "sql.internal",
                "encrypt": false,
                "trust_cert": true,
            }))
            .unwrap(),
            environment: None,
            read_only: 0,
            query_policy: None,
            token: "token".into(),
            created_at: String::new(),
            via_group: None,
        };
        let cfg = server::sql_config_from(&conn, None);
        assert_eq!(cfg.encrypt, Some(false));
        assert_eq!(cfg.trust_cert, Some(true));
    }
}
