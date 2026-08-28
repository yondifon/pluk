pub mod api;
pub mod client;
pub mod error;
pub mod policy;
pub mod server;
#[cfg(test)]
mod tests;

pub use api::handle_ssh_api;
pub use client::{close_forward, list_forwards, open_forward, run_command, ExecResult, MAX_COMMAND_TIMEOUT_S, set_test_executor, clear_test_executor};
#[cfg(test)]
pub use client::{reset_forwards_for_test, StubExecutor};
#[cfg(not(test))]
pub use client::SshExecutor;
pub use error::humanize_ssh_error;
pub use policy::{evaluate_command, CommandCategory, CommandVerdict, policy_summary, sanitize_working_dir};
pub use server::{ssh_instructions, ssh_tool_specs, register_ssh_server, SSH_AGENT_HINT};

use std::sync::Arc;
use async_trait::async_trait;
use pluk_store::{Integration, Store};
use crate::adapter::{Adapter, ApiRequest, ApiResponse, PolicyKind};
use crate::config_field::{ConfigField, FieldType};
use crate::error::AdapterError;
use crate::tool_host::ToolHost;
use crate::tool_spec::ToolSpec;

fn ssh_fields() -> Vec<ConfigField> {
    let mut fields = vec![
        ConfigField::new("host", "Host", FieldType::Text).group("Connection").required().placeholder("server.example.com or an ~/.ssh/config alias"),
        ConfigField::new("port", "Port", FieldType::Number).group("Connection").default_value(&serde_json::json!(22)),
        ConfigField::new("user", "User", FieldType::Text).group("Connection").placeholder("defaults to your local username"),
    ];
    fields.extend(crate::ssh_fields::ssh_auth_fields("", "Auth", None));
    fields
}

pub struct SshAdapter {
    store: Arc<Store>,
}

impl SshAdapter {
    pub fn new(store: Arc<Store>) -> Arc<Self> {
        Arc::new(Self { store })
    }
}

#[async_trait]
impl Adapter for SshAdapter {
    fn id(&self) -> &str { "ssh" }
    fn label(&self) -> &str { "SSH" }
    fn category(&self) -> &str { "infrastructure" }
    fn policy_kind(&self) -> PolicyKind { PolicyKind::None }
    fn agent_hint(&self) -> &str { SSH_AGENT_HINT }
    fn tool_specs(&self) -> &[ToolSpec] {
        static SPECS: std::sync::OnceLock<Vec<ToolSpec>> = std::sync::OnceLock::new();
        SPECS.get_or_init(ssh_tool_specs)
    }
    fn config_fields(&self) -> &[ConfigField] {
        static FIELDS: std::sync::OnceLock<Vec<ConfigField>> = std::sync::OnceLock::new();
        FIELDS.get_or_init(ssh_fields)
    }
    async fn test_connection(&self, conn: &Integration) -> Result<(), AdapterError> {
        // test via exec "echo pluk-ok" with 15s timeout, no caching
        match run_command(conn, "echo pluk-ok", Some(15_000)).await {
            Ok(res) => {
                if res.code.unwrap_or(1)==0 { Ok(()) } else { Err(AdapterError::new(format!("test command failed with exit {}", res.code.unwrap_or(-1)))) }
            },
            Err(e) => Err(e),
        }
    }
    fn humanize_error(&self, error: &AdapterError) -> Option<String> {
        Some(humanize_ssh_error(error))
    }
    async fn handle_api(&self, conn: &Integration, request: ApiRequest, subpath: &str) -> Option<ApiResponse> {
        handle_ssh_api(self.store.clone(), conn, request, subpath).await
    }
    fn instructions(&self, conn: &Integration) -> String {
        ssh_instructions(conn)
    }
    fn register(&self, host: &mut dyn ToolHost, conn: &Integration, owner_id: &str) -> Result<(), AdapterError> {
        register_ssh_server(host, conn, owner_id, self.store.clone())
    }
}

pub fn ssh_adapters(store: Arc<Store>) -> Vec<Arc<dyn Adapter>> {
    vec![SshAdapter::new(store)]
}
