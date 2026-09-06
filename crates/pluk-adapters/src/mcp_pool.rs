//! Cache of live upstream MCP sessions, keyed by owner.
//!
//! Pluk rebuilds its MCP surface on every protocol request so config edits
//! apply with no restart. Reconnecting to every upstream server on that path
//! would be unusable — a stdio upstream spawns a child process per connect — so
//! the session and the tool list it advertised are cached here while the
//! registrations built from them stay per-request.
//!
//! The pool is process-global because the two crates that need it cannot share
//! a handle: the upstream adapter lives in this crate, and `pluk-server`, which
//! owns endpoint lifetime, depends on it. `pluk-server` evicts an owner from its
//! [`OwnerPool`](pluk_server::OwnerPool) close hook, which `POST /api/reload`
//! and every integration edit already fire.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rmcp::model::{CallToolResult, JsonObject, Tool};

use crate::error::AdapterError;
use crate::mcp_client::{McpClient, McpUpstream};

/// A connected upstream server and the tools it listed at handshake time.
pub struct UpstreamSession {
    client: McpClient,
    tools: Vec<Tool>,
}

impl UpstreamSession {
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, AdapterError> {
        self.client.call_tool(name, arguments).await
    }
}

/// One cache key's slot. The async mutex serializes connection attempts, so
/// concurrent requests for the same upstream wait on one attempt instead of
/// each opening its own.
#[derive(Default)]
struct Slot {
    session: tokio::sync::Mutex<Option<Arc<UpstreamSession>>>,
    evicted: AtomicBool,
}

#[derive(Default)]
pub struct McpSessionPool {
    slots: Mutex<HashMap<String, Arc<Slot>>>,
}

fn session_key(owner_id: &str, integration_id: &str, server: &str) -> String {
    format!("{owner_id}::{integration_id}::{server}")
}

impl McpSessionPool {
    /// The cached session for one upstream server, connecting on a miss. A
    /// session whose transport has closed is torn down and replaced.
    pub async fn session(
        &self,
        owner_id: &str,
        integration_id: &str,
        server: &str,
        upstream: &McpUpstream,
        timeout: Duration,
    ) -> Result<Arc<UpstreamSession>, AdapterError> {
        let key = session_key(owner_id, integration_id, server);
        let slot = self
            .slots
            .lock()
            .expect("mcp session pool")
            .entry(key)
            .or_default()
            .clone();

        let mut held = slot.session.lock().await;
        if let Some(session) = held.as_ref() {
            if !session.client.is_closed() {
                return Ok(session.clone());
            }
            close_in_background(held.take());
        }

        let client = McpClient::connect(upstream, timeout).await?;
        let tools = client.list_tools().await?;
        let session = Arc::new(UpstreamSession { client, tools });

        if slot.evicted.load(Ordering::SeqCst) {
            close_in_background(Some(session));
            return Err(AdapterError::new(
                "this connection was reloaded while it was starting up — try again",
            ));
        }
        *held = Some(session.clone());
        Ok(session)
    }

    /// Drop every session an owner holds. Safe to call from a synchronous
    /// context: the teardown itself runs on a spawned task.
    pub fn evict_owner(&self, owner_id: &str) {
        let prefix = format!("{owner_id}::");
        let mut slots = self.slots.lock().expect("mcp session pool");
        let keys: Vec<String> = slots
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect();
        for key in keys {
            if let Some(slot) = slots.remove(&key) {
                slot.evicted.store(true, Ordering::SeqCst);
                drain(slot);
            }
        }
    }
}

/// The pool every upstream adapter and `pluk-server`'s owner close hook share.
pub fn mcp_sessions() -> &'static McpSessionPool {
    static POOL: OnceLock<McpSessionPool> = OnceLock::new();
    POOL.get_or_init(McpSessionPool::default)
}

/// Evict one owner from the shared pool — the entry point `pluk-server` calls
/// when an endpoint's owner scope closes.
pub fn evict_owner(owner_id: &str) {
    mcp_sessions().evict_owner(owner_id);
}

fn drain(slot: Arc<Slot>) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn(async move {
        close_in_background(slot.session.lock().await.take());
    });
}

/// Tear a session down without waiting on the caller's path. Dropping the last
/// handle kills a stdio child on its own, so a session still held by an
/// in-flight call is simply released here.
fn close_in_background(session: Option<Arc<UpstreamSession>>) {
    let Some(session) = session else { return };
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    if let Ok(session) = Arc::try_unwrap(session) {
        handle.spawn(async move {
            let _ = session.client.close().await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn missing_upstream() -> McpUpstream {
        McpUpstream::Stdio {
            command: "pluk-no-such-mcp-server".into(),
            args: vec![],
            env: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn a_failed_connect_is_not_cached() {
        let pool = McpSessionPool::default();
        let upstream = missing_upstream();

        for _ in 0..2 {
            let result = pool
                .session("owner", "int", "srv", &upstream, Duration::from_secs(1))
                .await
                .map(|_| ());
            let error = result.expect_err("the command does not exist");
            assert!(error.to_string().contains("pluk-no-such-mcp-server"));
        }

        let slots = pool.slots.lock().unwrap();
        assert!(
            slots["owner::int::srv"].session.try_lock().unwrap().is_none(),
            "a broken upstream must never be served from cache"
        );
    }

    #[tokio::test]
    async fn evicting_an_owner_leaves_other_owners_alone() {
        let pool = McpSessionPool::default();
        {
            let mut slots = pool.slots.lock().unwrap();
            slots.insert("a::int::srv".into(), Arc::default());
            slots.insert("ab::int::srv".into(), Arc::default());
            slots.insert("b::int::srv".into(), Arc::default());
        }

        pool.evict_owner("a");

        let slots = pool.slots.lock().unwrap();
        assert!(!slots.contains_key("a::int::srv"));
        assert!(slots.contains_key("ab::int::srv"), "prefix is not a match");
        assert!(slots.contains_key("b::int::srv"));
    }
}
