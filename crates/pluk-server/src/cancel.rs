//! Per-query cancellation: `POST /api/log/:id/cancel` aborts a single
//! in-flight tool call by its log row id, mirroring `cancelQuery` in
//! `pluk/src/adapters/sql/pool.ts`.

use std::collections::HashMap;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

/// Registry of abort handles keyed by `query_log` row id.
///
/// Tool calls register their token when they start and remove it when they
/// settle; the HTTP handler aborts by id. Stored as a [`CancellationToken`]
/// so callers can race their work against `cancelled()`.
#[derive(Default)]
pub struct CancelRegistry {
    handles: Mutex<HashMap<i64, CancellationToken>>,
}

impl CancelRegistry {
    pub fn register(&self, log_id: i64) -> CancellationToken {
        let token = CancellationToken::new();
        self.handles.lock().expect("cancel lock").insert(log_id, token.clone());
        token
    }

    pub fn complete(&self, log_id: i64) {
        self.handles.lock().expect("cancel lock").remove(&log_id);
    }

    pub fn token_for(&self, log_id: i64) -> Option<CancellationToken> {
        self.handles.lock().expect("cancel lock").get(&log_id).cloned()
    }

    /// Abort the handle for `log_id` when present. Returns true when found.
    pub fn cancel(&self, log_id: i64) -> bool {
        if let Some(token) = self.handles.lock().expect("cancel lock").remove(&log_id) {
            token.cancel();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_returns_false_for_unknown_and_true_once() {
        let registry = CancelRegistry::default();
        assert!(!registry.cancel(42));
        let token = registry.register(42);
        assert!(!token.is_cancelled());
        assert!(registry.cancel(42));
        assert!(token.is_cancelled());
        assert!(!registry.cancel(42));
    }
}
