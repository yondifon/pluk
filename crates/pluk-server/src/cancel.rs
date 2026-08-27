//! Per-query cancellation: `POST /api/log/:id/cancel` aborts a single
//! in-flight tool call by its log row id, mirroring `cancelQuery` in
//! `pluk/src/adapters/sql/pool.ts`.

pub use pluk_adapters::sql::SqlCancelRegistry as CancelRegistry;

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
