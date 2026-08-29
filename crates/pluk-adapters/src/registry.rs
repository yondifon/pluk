//! The adapter registry: id → adapter, with a hard error on duplicates.
//!
//! To add a service: build an adapter and register it here. Nothing else —
//! store, MCP transport, REST layer, UI — needs editing.

use std::collections::HashMap;
use std::sync::Arc;

use crate::adapter::Adapter;
use crate::error::AdapterError;

/// Every registered adapter, keyed by its id (the integration's stored
/// `type`). Preserves insertion order for listings, like the TypeScript Map.
#[derive(Default)]
pub struct AdapterRegistry {
    adapters: HashMap<String, Arc<dyn Adapter>>,
    order: Vec<String>,
}

impl AdapterRegistry {
    pub fn new() -> Self {
        AdapterRegistry::default()
    }

    /// Add an adapter. A duplicate id is a hard error — two integrations can
    /// never resolve to the same adapter unambiguously.
    pub fn register(&mut self, adapter: Arc<dyn Adapter>) -> Result<(), AdapterError> {
        let id = adapter.id().to_string();
        if self.adapters.contains_key(&id) {
            return Err(AdapterError::new(format!("Duplicate adapter id: {id}")));
        }
        self.order.push(id.clone());
        self.adapters.insert(id, adapter);
        Ok(())
    }

    /// Resolve an integration's `type` to its adapter.
    pub fn get(&self, id: &str) -> Option<Arc<dyn Adapter>> {
        self.adapters.get(id).cloned()
    }

    /// Every adapter in registration order — drives the catalog the frontend
    /// reads.
    pub fn list(&self) -> Vec<Arc<dyn Adapter>> {
        self.order
            .iter()
            .filter_map(|id| self.adapters.get(id).cloned())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

/// The registry the app runs on: every adapter Pluk ships, in catalog order.
pub fn default_registry(
    store: Arc<pluk_store::Store>,
    cancels: Arc<crate::sql::SqlCancelRegistry>,
) -> Result<AdapterRegistry, AdapterError> {
    let mut registry = AdapterRegistry::new();
    for adapter in crate::sql::sql_adapters(store.clone(), cancels) {
        registry.register(adapter)?;
    }
    registry.register(crate::ssh::SshAdapter::new(store.clone()))?;
    registry.register(crate::redis::RedisAdapter::new(store.clone()))?;
    registry.register(crate::slack::SlackAdapter::new(store.clone()))?;
    registry.register(crate::linear::LinearAdapter::new(store.clone()))?;
    registry.register(crate::sentry::SentryAdapter::new(store.clone()))?;
    registry.register(Arc::new(crate::github_cli::build_github_cli_adapter(
        store.clone(),
    )))?;
    registry.register(Arc::new(crate::action::action_adapter(
        crate::spark::spark_adapter_spec(),
        store,
    )))?;
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{ApiRequest, ApiResponse, PolicyKind};
    use crate::config_field::ConfigField;
    use crate::error::AdapterError as AdapterFailure;
    use crate::tool_host::ToolHost;
    use crate::tool_spec::ToolSpec;
    use async_trait::async_trait;
    use pluk_store::Integration;

    struct StubAdapter {
        id: String,
    }

    #[async_trait]
    impl Adapter for StubAdapter {
        fn id(&self) -> &str {
            &self.id
        }

        fn label(&self) -> &str {
            "Stub"
        }

        fn category(&self) -> &str {
            "misc"
        }

        fn policy_kind(&self) -> PolicyKind {
            PolicyKind::None
        }

        fn agent_hint(&self) -> &str {
            ""
        }

        fn tool_specs(&self) -> &[ToolSpec] {
            &[]
        }

        fn config_fields(&self) -> &[ConfigField] {
            &[]
        }

        async fn test_connection(&self, _conn: &Integration) -> Result<(), AdapterFailure> {
            Ok(())
        }

        async fn handle_api(
            &self,
            _conn: &Integration,
            _request: ApiRequest,
            _subpath: &str,
        ) -> Option<ApiResponse> {
            None
        }

        fn instructions(&self, conn: &Integration) -> String {
            format!("stub for {}", conn.name)
        }

        fn register(
            &self,
            _host: &mut dyn ToolHost,
            _conn: &Integration,
            _owner_id: &str,
        ) -> Result<(), AdapterFailure> {
            Ok(())
        }
    }

    fn stub(id: &str) -> Arc<dyn Adapter> {
        Arc::new(StubAdapter { id: id.to_string() })
    }

    #[test]
    fn duplicate_ids_are_a_hard_error() {
        let mut registry = AdapterRegistry::new();
        registry
            .register(stub("postgres"))
            .expect("first registration");
        let error = registry
            .register(stub("postgres"))
            .expect_err("duplicate must fail");
        assert_eq!(error.message, "Duplicate adapter id: postgres");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn lookups_resolve_by_id_and_listings_preserve_insertion_order() {
        let mut registry = AdapterRegistry::new();
        for id in ["postgres", "linear", "ssh"] {
            registry.register(stub(id)).expect("register");
        }
        assert!(registry.get("postgres").is_some());
        assert!(registry.get("redis").is_none());
        let listed: Vec<String> = registry
            .list()
            .into_iter()
            .map(|a| a.id().to_string())
            .collect();
        assert_eq!(listed, ["postgres", "linear", "ssh"]);
    }
}
