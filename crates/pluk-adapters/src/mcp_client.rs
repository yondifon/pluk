//! Pluk as an MCP *client*: connect to someone else's MCP server, list its
//! tools, and call one.
//!
//! It lives here rather than in `pluk-server` because the coming `mcp-proxy`
//! adapter is an adapter, and `pluk-server` already depends on this crate — the
//! other direction would be a cycle.
//!
//! Both transports come from `rmcp` itself: [`TokioChildProcess`] for a local
//! command over stdio, and `StreamableHttpClientTransport` for a URL. The HTTP
//! one carries its own `reqwest` client (a different major than the one
//! [`crate::http_client`] shares) which it deliberately builds without idle
//! pooling and without redirect following, so custom headers are never replayed
//! to another host.

use std::collections::HashMap;
use std::time::Duration;

use http::{HeaderName, HeaderValue};
use rmcp::ServiceExt;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientInfo, Implementation, JsonObject, Tool,
};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::child_process::TokioChildProcess;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use tokio::process::Command;

use crate::error::AdapterError;

/// How to reach an upstream MCP server.
#[derive(Debug, Clone)]
pub enum McpUpstream {
    /// A local program speaking MCP over its stdin/stdout.
    Stdio {
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    },
    /// A streamable-HTTP endpoint.
    Http {
        url: String,
        headers: HashMap<String, String>,
    },
}

/// A live session with one upstream MCP server.
pub struct McpClient {
    service: RunningService<RoleClient, ClientInfo>,
    timeout: Duration,
}

impl McpClient {
    /// Spawn or dial the upstream and complete the MCP handshake. `timeout`
    /// bounds the handshake and every later operation on this session.
    pub async fn connect(upstream: &McpUpstream, timeout: Duration) -> Result<Self, AdapterError> {
        let service = match upstream {
            McpUpstream::Stdio { command, args, env } => {
                let mut cmd = Command::new(command);
                cmd.args(args).envs(env);
                let child = TokioChildProcess::new(cmd).map_err(|e| spawn_error(command, &e))?;
                bounded(timeout, client_info().serve(child), "handshake").await?
            }
            McpUpstream::Http { url, headers } => {
                let config = StreamableHttpClientTransportConfig::with_uri(url.as_str())
                    .custom_headers(parse_headers(headers)?);
                let transport = StreamableHttpClientTransport::from_config(config);
                bounded(timeout, client_info().serve(transport), "handshake").await?
            }
        }
        .map_err(|e| handshake_error(upstream, &e.to_string()))?;

        Ok(McpClient { service, timeout })
    }

    /// Every tool the upstream exposes, following pagination.
    pub async fn list_tools(&self) -> Result<Vec<Tool>, AdapterError> {
        bounded(self.timeout, self.service.list_all_tools(), "tools/list")
            .await?
            .map_err(|e| AdapterError::new(format!("MCP server rejected tools/list: {e}")))
    }

    /// Call one tool. Arguments and result pass through untouched.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, AdapterError> {
        let mut params = CallToolRequestParams::default();
        params.name = name.to_string().into();
        params.arguments = arguments;
        bounded(self.timeout, self.service.call_tool(params), "tools/call")
            .await?
            .map_err(|e| AdapterError::new(format!("MCP tool `{name}` failed: {e}")))
    }

    /// Whether the transport has gone away — a cached session that answers
    /// `true` here is dead and has to be replaced.
    pub fn is_closed(&self) -> bool {
        self.service.is_closed()
    }

    /// Shut the session down. A stdio child is also killed when an [`McpClient`]
    /// is simply dropped, so this is only needed to wait for a clean exit.
    pub async fn close(self) -> Result<(), AdapterError> {
        bounded(self.timeout, self.service.cancel(), "shutdown")
            .await?
            .map_err(|e| AdapterError::new(format!("MCP session did not shut down cleanly: {e}")))?;
        Ok(())
    }
}

fn client_info() -> ClientInfo {
    let mut info = ClientInfo::default();
    info.client_info = Implementation::new("pluk", env!("CARGO_PKG_VERSION"));
    info
}

/// Run `fut` under the session deadline, turning an overrun into a message that
/// names the operation that hung.
async fn bounded<F: Future>(
    timeout: Duration,
    fut: F,
    operation: &str,
) -> Result<F::Output, AdapterError> {
    tokio::time::timeout(timeout, fut).await.map_err(|_| {
        AdapterError::new(format!(
            "MCP server did not answer {operation} within {}s",
            timeout.as_secs_f32()
        ))
    })
}

fn parse_headers(
    headers: &HashMap<String, String>,
) -> Result<HashMap<HeaderName, HeaderValue>, AdapterError> {
    headers
        .iter()
        .map(|(name, value)| {
            let name = HeaderName::try_from(name)
                .map_err(|_| AdapterError::new(format!("`{name}` is not a valid header name")))?;
            let value = HeaderValue::try_from(value).map_err(|_| {
                AdapterError::new(format!("the value for header `{name}` is not valid"))
            })?;
            Ok((name, value))
        })
        .collect()
}

fn spawn_error(command: &str, error: &std::io::Error) -> AdapterError {
    match error.kind() {
        std::io::ErrorKind::NotFound => {
            AdapterError::new(format!("`{command}` was not found on this machine"))
        }
        std::io::ErrorKind::PermissionDenied => {
            AdapterError::new(format!("`{command}` is not executable"))
        }
        _ => AdapterError::new(format!("`{command}` could not be started: {error}")),
    }
}

fn handshake_error(upstream: &McpUpstream, detail: &str) -> AdapterError {
    let target = match upstream {
        McpUpstream::Stdio { command, .. } => command.as_str(),
        McpUpstream::Http { url, .. } => url.as_str(),
    };
    if detail.contains("Auth required") || detail.contains("Insufficient scope") {
        return AdapterError::new(format!(
            "{target} rejected these credentials — check the token and its scopes"
        ));
    }
    if matches!(upstream, McpUpstream::Http { .. })
        && (detail.contains("Io error") || detail.contains("Client error"))
    {
        return AdapterError::new(format!("{target} could not be reached: {detail}"));
    }
    AdapterError::new(format!("MCP handshake with {target} failed: {detail}"))
}
