//! Static per-tool descriptors for the catalog/UI.
//!
//! Tool definitions never depend on a live connection: an adapter publishes
//! its fixed tool set once at startup so the catalog can render toggles and
//! (optional) settings forms without touching any service.

use serde::Serialize;

use crate::config_field::ConfigField;
use pluk_policy::default_enabled_for_category;

/// Static description of one tool an adapter exposes. Drives the per-tool
/// enable toggle and the optional expandable settings form.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// Coarse class for grouping and default-on (`read`, `write`, `delete`,
    /// `admin`, `inspect`).
    pub category: String,
    /// Whether this tool is on by default for a fresh integration.
    pub default_enabled: bool,
    /// The tool's own settings, rendered when the tool is expanded. Keys are
    /// scoped to the tool's settings object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<Vec<ConfigField>>,
}

impl ToolSpec {
    /// Build a spec whose default-on state derives from the category: read
    /// and inspect tools ship on; write/delete/admin ship off until opted in.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        category: impl Into<String>,
    ) -> Self {
        let category = category.into();
        let default_enabled = default_enabled_for_category(&category);
        ToolSpec {
            name: name.into(),
            description: description.into(),
            category,
            default_enabled,
            settings: None,
        }
    }

    /// Override the derived default-on state — set `false` on a niche or heavy
    /// read tool that should ship off; never `true` on a state-changing tool.
    pub fn with_default_enabled(mut self, default_enabled: bool) -> Self {
        self.default_enabled = default_enabled;
        self
    }

    pub fn with_settings(mut self, settings: Vec<ConfigField>) -> Self {
        self.settings = Some(settings);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_enabled_derives_from_category() {
        for on in ["read", "inspect"] {
            assert!(
                ToolSpec::new("t", "T", on).default_enabled,
                "{on} must default on"
            );
        }
        for off in ["write", "delete", "admin", "other"] {
            assert!(
                !ToolSpec::new("t", "T", off).default_enabled,
                "{off} must default off"
            );
        }
    }

    #[test]
    fn explicit_override_wins_over_the_derived_default() {
        assert!(
            !ToolSpec::new("keys", "K", "read")
                .with_default_enabled(false)
                .default_enabled
        );
    }

    #[test]
    fn serializes_camel_case_and_omits_absent_settings() {
        let value = serde_json::to_value(ToolSpec::new("get", "Get a key", "read")).unwrap();
        assert_eq!(
            value,
            serde_json::json!({ "name": "get", "description": "Get a key", "category": "read", "defaultEnabled": true })
        );
    }
}
