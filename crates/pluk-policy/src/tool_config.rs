//! Per-tool configuration — the unified policy model for every adapter.
//!
//! Stored shape in the `query_policy` column:
//!
//! ```json
//! { "tools": { "query": { "enabled": true, "settings": { "mode": "read-only" } } } }
//! ```
//!
//! A tool with no stored entry falls back to its declared default (read tools
//! on, write/delete tools off), so an unconfigured or malformed blob fails
//! safe: nothing is enabled that the tool did not already default to.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// One tool's stored state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StoredToolState {
    /// Absent means "use the declared default".
    pub enabled: Option<bool>,
    pub settings: Map<String, Value>,
}

/// Parsed view of an integration's `query_policy` blob.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolConfig {
    tools: BTreeMap<String, StoredToolState>,
}

impl ToolConfig {
    pub fn get(&self, name: &str) -> Option<&StoredToolState> {
        self.tools.get(name)
    }
}

/// Parse the `query_policy` blob into a tool config. Tolerates legacy and
/// non-JSON blobs: anything unreadable yields an empty config so every tool
/// falls back to its declared default.
pub fn parse_tool_config(raw: Option<&str>) -> ToolConfig {
    let Some(raw) = raw.filter(|r| !r.is_empty()) else {
        return ToolConfig::default();
    };
    let Ok(parsed) = serde_json::from_str::<Value>(raw) else {
        return ToolConfig::default();
    };
    let Some(tools) = parsed.get("tools").filter(|t| t.is_object()) else {
        return ToolConfig::default();
    };
    let Value::Object(entries) = tools else {
        unreachable!("checked above")
    };
    let mut map = BTreeMap::new();
    for (name, entry) in entries {
        let state = match entry {
            Value::Object(object) => StoredToolState {
                // Presence decides; the value is coerced like JS's !!state.enabled.
                enabled: object.get("enabled").map(js_truthy),
                settings: match object.get("settings") {
                    Some(Value::Object(settings)) => settings.clone(),
                    _ => Map::new(),
                },
            },
            // A non-object entry behaves like an absent one.
            _ => StoredToolState::default(),
        };
        map.insert(name.clone(), state);
    }
    ToolConfig { tools: map }
}

/// Resolved view used by adapters at register time.
#[derive(Debug, Clone)]
pub struct ToolGate {
    config: ToolConfig,
}

impl ToolGate {
    /// Whether `name` is enabled; `fallback` is the tool's declared default.
    pub fn enabled(&self, name: &str, fallback: bool) -> bool {
        self.config
            .get(name)
            .and_then(|state| state.enabled)
            .unwrap_or(fallback)
    }

    /// The stored settings for `name` (empty when absent).
    pub fn settings(&self, name: &str) -> Map<String, Value> {
        self.config
            .get(name)
            .map(|state| state.settings.clone())
            .unwrap_or_default()
    }
}

/// Build a gate straight from the stored blob.
pub fn tool_gate(raw: Option<&str>) -> ToolGate {
    ToolGate {
        config: parse_tool_config(raw),
    }
}

/// Default-on state for a tool of the given coarse category: read tools are
/// on by default; anything that can modify state is off until opted in.
pub fn default_enabled_for_category(category: &str) -> bool {
    category == "read" || category == "inspect"
}

// ── Settings readers (typed accessors over the loose settings blob) ──────────

pub(crate) fn read_string(settings: &Map<String, Value>, key: &str) -> Option<String> {
    settings.get(key).and_then(Value::as_str).map(Into::into)
}

pub(crate) fn read_number(settings: &Map<String, Value>, key: &str) -> Option<f64> {
    match settings.get(key) {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// A string setting; empty or missing falls back.
pub fn setting_string(settings: &Map<String, Value>, key: &str, fallback: &str) -> String {
    read_string(settings, key)
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// A boolean setting accepting real booleans plus `"true"`/`"false"` strings.
pub fn setting_bool(settings: &Map<String, Value>, key: &str, fallback: bool) -> bool {
    match settings.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) if s == "true" => true,
        Some(Value::String(s)) if s == "false" => false,
        _ => fallback,
    }
}

/// A numeric setting where a missing (or non-positive) value means "no limit".
pub fn setting_number_or_null(
    settings: &Map<String, Value>,
    key: &str,
    fallback: Option<f64>,
) -> Option<f64> {
    match read_number(settings, key) {
        Some(n) if n.is_finite() => {
            if n > 0.0 {
                Some(n)
            } else {
                None
            }
        }
        Some(_) => fallback, // infinite values fall back
        None => fallback,
    }
}

/// JavaScript truthiness, because stored blobs were written by JS: any value
/// present counts as enabled there (`!!state.enabled`), including `"false"`.
pub(crate) fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Null => false,
        Value::Array(_) | Value::Object(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settings(raw: Option<&str>, tool: &str) -> Map<String, Value> {
        tool_gate(raw).settings(tool)
    }

    #[test]
    fn parses_the_unified_model() {
        let raw = r#"{"tools":{"query":{"enabled":true,"settings":{"mode":"read-only"}},"del":{"enabled":false}}}"#;
        let gate = tool_gate(Some(raw));
        assert!(gate.enabled("query", false));
        assert!(!gate.enabled("del", true));
        assert_eq!(
            settings(Some(raw), "query").get("mode"),
            Some(&json!("read-only"))
        );
    }

    #[test]
    fn missing_entry_falls_back_to_declared_default() {
        let gate = tool_gate(Some(r#"{"tools":{"other":{"enabled":true}}}"#));
        assert!(gate.enabled("query", true));
        assert!(!gate.enabled("query", false));
        assert!(gate.settings("query").is_empty());
    }

    #[test]
    fn entry_without_enabled_falls_back_too() {
        let gate = tool_gate(Some(r#"{"tools":{"query":{}}}"#));
        assert!(!gate.enabled("query", false));
    }

    #[test]
    fn malformed_blobs_fail_safe_to_defaults() {
        for raw in [
            None,
            Some(""),
            Some("not-json"),
            Some("1"),
            Some(r#"{"tools":"x"}"#),
            Some("[1]"),
        ] {
            let gate = tool_gate(raw);
            assert!(
                !gate.enabled("run_command", false),
                "{raw:?} must not enable"
            );
            assert!(
                gate.enabled("list_tables", true),
                "{raw:?} must keep defaults"
            );
            assert!(gate.settings("query").is_empty());
        }
    }

    #[test]
    fn non_object_entries_behave_like_absent_ones() {
        let gate = tool_gate(Some(r#"{"tools":{"drop_all":"yes"}}"#));
        assert!(!gate.enabled("drop_all", false));
        assert!(gate.enabled("drop_all", true));
    }

    #[test]
    fn non_object_settings_yield_empty() {
        assert!(settings(Some(r#"{"tools":{"q":{"settings":"x"}}}"#), "q").is_empty());
    }

    #[test]
    fn truthiness_coercion_matches_js() {
        // "false" is a truthy string → enabled, mirroring !!state.enabled.
        let gate = tool_gate(Some(
            r#"{"tools":{"a":{"enabled":"false"},"b":{"enabled":0},"c":{"enabled":true}}}"#,
        ));
        assert!(gate.enabled("a", false));
        assert!(!gate.enabled("b", true));
        assert!(gate.enabled("c", false));
    }

    #[test]
    fn default_enabled_for_category_locks_writes_off() {
        assert!(default_enabled_for_category("read"));
        assert!(default_enabled_for_category("inspect"));
        assert!(!default_enabled_for_category("write"));
        assert!(!default_enabled_for_category("delete"));
        assert!(!default_enabled_for_category("admin"));
    }

    #[test]
    fn typed_setting_readers() {
        let s = serde_json::from_value::<Map<String, Value>>(json!({
            "s": "hello", "empty": "",
            "bt": true, "bf": false, "bs": "true",
            "n": 50, "ns": "25", "zero": 0, "neg": -3, "junk": "abc"
        }))
        .expect("map");

        assert_eq!(setting_string(&s, "s", "fb"), "hello");
        assert_eq!(setting_string(&s, "empty", "fb"), "fb");
        assert_eq!(setting_string(&s, "missing", "fb"), "fb");

        assert!(setting_bool(&s, "bt", false));
        assert!(!setting_bool(&s, "bf", true));
        assert!(setting_bool(&s, "bs", false));
        assert!(setting_bool(&s, "missing", true));

        assert_eq!(setting_number_or_null(&s, "n", None), Some(50.0));
        assert_eq!(setting_number_or_null(&s, "ns", None), Some(25.0));
        // Zero/negative mean "no limit".
        assert_eq!(setting_number_or_null(&s, "zero", Some(10.0)), None);
        assert_eq!(setting_number_or_null(&s, "neg", Some(10.0)), None);
        // Unparseable falls back.
        assert_eq!(setting_number_or_null(&s, "junk", Some(10.0)), Some(10.0));
        assert_eq!(setting_number_or_null(&s, "missing", None), None);
    }
}
