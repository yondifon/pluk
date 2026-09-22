//! A group member whose tools are discovered per integration: the `mcp`
//! adapter, fronting a real upstream MCP server.

mod common;

use std::sync::Arc;

use axum::Router;
use axum::routing::any;
use rmcp::ErrorData as McpError;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::StreamableHttpServerConfig;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::{RoleServer, ServerHandler};
use serde_json::json;
use tower::ServiceExt as _;

use common::spawn_app_with;
use pluk_adapters::mcp_proxy::McpProxyAdapter;
use pluk_adapters::{Adapter, AdapterRegistry};
use pluk_server::AppState;

/// An upstream offering one `search` tool that answers with what it found.
#[derive(Clone)]
struct Upstream;

impl ServerHandler for Upstream {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let schema = json!({"type": "object", "properties": {"q": {"type": "string"}}});
        let schema = schema.as_object().cloned().unwrap_or_default();
        Ok(ListToolsResult::with_all_items(vec![Tool::new(
            "search".to_string(),
            "Search the issues".to_string(),
            Arc::new(schema),
        )]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let query = request
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get("q"))
            .and_then(|q| q.as_str())
            .unwrap_or_default()
            .to_string();
        Ok(CallToolResponse::Complete(CallToolResult::success(vec![
            ContentBlock::text(format!("found {query}")),
        ])))
    }
}

async fn upstream() -> String {
    let service = StreamableHttpService::new(
        || Ok(Upstream),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let router = Router::new().route(
        "/mcp",
        any(move |request: axum::extract::Request| {
            let service = service.clone();
            async move {
                match service.oneshot(request).await {
                    Ok(response) => response.map(axum::body::Body::new),
                    Err(never) => match never {},
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    format!("http://{addr}/mcp")
}

#[tokio::test]
async fn a_call_to_an_mcp_member_lands_in_the_groups_log() {
    let app = spawn_app_with(|store, _registry, owners, health| {
        let mut registry = AdapterRegistry::new();
        registry
            .register(McpProxyAdapter::new(store.clone()))
            .expect("register mcp");
        AppState::new(store, Arc::new(registry), owners, health)
    })
    .await;

    let mut input = pluk_store::IntegrationInput::new("Linear", "mcp");
    input.config.insert("url".into(), json!(upstream().await));
    input.query_policy = Some(json!({ "tools": { "search": { "enabled": true } } }).to_string());
    let member = app.store.create_integration(&input).expect("create member");

    // Discover the upstream's tools and approve them, as the owner does after
    // connecting.
    McpProxyAdapter::new(app.store.clone())
        .test_connection(&member)
        .await
        .expect("discover");
    app.store
        .approve_proxy_tools(&member.id, &["search".to_string()])
        .expect("approve");

    let group = app
        .store
        .create_group(&pluk_store::GroupInput {
            name: "Planning".to_string(),
            environment: None,
            members: vec![pluk_store::GroupMember {
                id: member.id.clone(),
                overrides: Default::default(),
                tools: None,
            }],
        })
        .expect("create group");

    let (status, _, body) = app
        .mcp_post(
            &group.token,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
                "name": "linear__search", "arguments": { "q": "onboarding" } } }),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        body["result"]["content"][0]["text"], "found onboarding",
        "{body}"
    );

    let page = app
        .store
        .read_log_page(
            &pluk_store::LogScope::Group(group.id.clone()),
            pluk_store::LogRange::All,
            None,
        )
        .expect("group log");
    assert_eq!(page.entries.len(), 1, "the call lands in the group's view");
    assert_eq!(page.entries[0].connection_id, member.id);
    assert_eq!(page.entries[0].group_name.as_deref(), Some("Planning"));
}
