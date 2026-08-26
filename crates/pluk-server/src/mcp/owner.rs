//! Owner-scoped resource lifetime.
//!
//! The MCP protocol is served statelessly — every request is answered by a
//! fresh server, and there is no session id to key long-lived drivers, tunnels
//! or forwards on. The stable identity is the **owner**: the integration or
//! group the endpoint token resolves to. Owner scope lives for the process and
//! is torn down when the owner's config changes (`POST /api/reload`).
//!
//! Ported from `pluk/src/mcp/pool.ts`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

type CloseHook = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Default)]
pub struct OwnerPool {
    aborts: Mutex<HashMap<String, CancellationToken>>,
    hooks: Mutex<Vec<CloseHook>>,
}

impl OwnerPool {
    /// Ensure an owner scope exists, returning its cancellation token. Tool
    /// bodies and pooled resources watch it to stop promptly on teardown.
    pub fn open_owner(&self, owner_id: &str) -> CancellationToken {
        let mut aborts = self.aborts.lock().expect("owner lock");
        aborts
            .entry(owner_id.to_string())
            .or_default()
            .clone()
    }

    pub fn owner_token(&self, owner_id: &str) -> Option<CancellationToken> {
        self.aborts.lock().expect("owner lock").get(owner_id).cloned()
    }

    /// Register a hook run whenever any owner closes (adapter-owned pools
    /// evict their cached resources for that owner here).
    pub fn on_owner_close(&self, hook: CloseHook) {
        self.hooks.lock().expect("close hooks").push(hook);
    }

    /// Abort one owner's in-flight calls and notify adapter-owned pools. A
    /// standalone integration and the same integration inside a group are
    /// different owners — different pools — so their connections stay isolated.
    pub fn close_owner(&self, owner_id: &str) {
        let token = self.aborts.lock().expect("owner lock").remove(owner_id);
        if let Some(token) = token {
            token.cancel();
        }
        for hook in self.hooks.lock().expect("close hooks").iter() {
            hook(owner_id);
        }
    }

    /// Close owners: only `Some(id)`'s scope when given, else every owner.
    /// Returns how many scopes were closed.
    pub fn reset_owners(&self, owner_id: Option<&str>) -> usize {
        let ids: Vec<String> = match owner_id {
            Some(id) => self.aborts.lock().expect("owner lock").contains_key(id).then(|| id.to_string()).into_iter().collect(),
            None => self.aborts.lock().expect("owner lock").keys().cloned().collect(),
        };
        let count = ids.len();
        for id in ids {
            self.close_owner(&id);
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_scopes_to_the_requested_owner() {
        let pool = OwnerPool::default();
        pool.open_owner("a");
        pool.open_owner("b");

        assert_eq!(pool.reset_owners(Some("a")), 1);
        assert!(pool.owner_token("a").is_none());
        assert!(pool.owner_token("b").is_some());

        assert_eq!(pool.reset_owners(None), 1);
        assert!(pool.owner_token("b").is_none());
    }

    #[test]
    fn unknown_owner_resets_nothing() {
        let pool = OwnerPool::default();
        pool.open_owner("a");
        assert_eq!(pool.reset_owners(Some("zzz")), 0);
        assert!(pool.owner_token("a").is_some());
    }

    #[tokio::test]
    async fn close_aborts_the_token_and_runs_hooks() {
        let pool = OwnerPool::default();
        let token = pool.open_owner("a");
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = closed.clone();
        pool.on_owner_close(Arc::new(move |id| {
            assert_eq!(id, "a");
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }));

        assert!(!token.is_cancelled());
        pool.close_owner("a");
        assert!(token.is_cancelled(), "the owner's token must be aborted");
        assert!(closed.load(std::sync::atomic::Ordering::SeqCst));
    }
}
