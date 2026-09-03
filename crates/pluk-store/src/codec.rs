//! Codecs for the JSON blobs stored in TEXT columns.
//!
//! Three columns hold JSON: `integrations.config`, `integrations.query_policy`
//! (also `groups` members via the legacy-named `member_ids` column). The codecs
//! mirror how the TypeScript server hydrates them — malformed blobs fall back
//! to empty values instead of failing, because rows in the wild have been
//! written by three codebases over time.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::models::{Config, GroupMember};

/// Parse a `config` blob. Malformed or non-object JSON becomes `{}`, matching
/// the TS server's hydration.
pub fn parse_config(raw: &str) -> Config {
    match serde_json::from_str::<Config>(raw) {
        Ok(config) => config,
        Err(_) => Map::new(),
    }
}

/// Serialize a `config` blob compactly (the TS server stores `JSON.stringify`
/// output; no spaces).
pub fn serialize_config(config: &Config) -> String {
    serde_json::to_string(config).expect("Config serializes")
}

/// The `query_policy` blob: per-tool enablement and typed settings.
///
/// ```json
/// { "tools": { "query": { "enabled": true, "settings": { "limit": 50 } } } }
/// ```
///
/// Unknown fields are captured so a parse → serialize round trip is lossless,
/// preserving forward compatibility.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QueryPolicy {
    #[serde(default)]
    pub tools: BTreeMap<String, ToolPolicy>,
    /// Any sibling keys written by future versions, preserved on round trip.
    #[serde(flatten)]
    pub extra: Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolPolicy {
    /// Absent means enabled (default).
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub settings: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, serde_json::Value>,
}

fn default_true() -> bool {
    true
}

/// Parse a `query_policy` blob; `None` for an absent column.
pub fn parse_query_policy(raw: Option<&str>) -> Option<QueryPolicy> {
    let raw = raw?;
    if raw.trim().is_empty() {
        return None;
    }
    serde_json::from_str(raw).ok()
}

/// Serialize a `query_policy` blob compactly.
pub fn serialize_query_policy(policy: &QueryPolicy) -> String {
    serde_json::to_string(policy).expect("QueryPolicy serializes")
}

/// Parse the `member_ids` column: an array of `{id, overrides}` objects with
/// entries that predate overrides stored as bare id strings.
///
/// Anything unparseable yields no members rather than an error — a group with
/// garbage members still lists, it just has none.
pub fn parse_members(raw: &str) -> Vec<GroupMember> {
    let parsed: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let Value::Array(entries) = parsed else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter_map(|entry| match entry {
            // Legacy form: bare id string.
            Value::String(id) => Some(GroupMember {
                id,
                overrides: Map::new(),
            }),
            Value::Object(object) => match object.get("id") {
                Some(Value::String(id)) => Some(GroupMember {
                    id: id.clone(),
                    overrides: object
                        .get("overrides")
                        .and_then(|o| serde_json::from_value(o.clone()).ok())
                        .unwrap_or_default(),
                }),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Serialize members in the current form, omitting empty overrides exactly as
/// both existing writers do.
pub fn serialize_members(members: &[GroupMember]) -> String {
    let entries: Vec<Value> = members
        .iter()
        .map(|m| {
            let mut object = Map::new();
            object.insert("id".into(), Value::String(m.id.clone()));
            if !m.overrides.is_empty() {
                object.insert("overrides".into(), Value::Object(m.overrides.clone()));
            }
            Value::Object(object)
        })
        .collect();
    Value::Array(entries).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn config_round_trips_typed_values() {
        let config: Config = serde_json::from_value(json!({
            "host": "localhost",
            "port": 5432,
            "ssl": true,
            "nested": { "a": [1, 2] }
        }))
        .unwrap();
        let serialized = serialize_config(&config);
        assert_eq!(
            serialized,
            r#"{"host":"localhost","nested":{"a":[1,2]},"port":5432,"ssl":true}"#
        );
        assert_eq!(parse_config(&serialized), config);
    }

    #[test]
    fn malformed_config_becomes_empty() {
        assert_eq!(parse_config("not json"), Map::new());
        assert_eq!(parse_config("[1,2]"), Map::new());
        assert_eq!(parse_config(""), Map::new());
        assert_eq!(parse_config("{}"), Map::new());
    }

    #[test]
    fn query_policy_round_trips_with_unknown_fields_intact() {
        let raw = r#"{"tools":{"query":{"enabled":false}},"future":{"x":1}}"#;
        let policy = parse_query_policy(Some(raw)).expect("parses");
        assert!(!policy.tools["query"].enabled);
        assert_eq!(policy.extra["future"], json!({"x": 1}));
        let reparsed = parse_query_policy(Some(&serialize_query_policy(&policy))).unwrap();
        assert_eq!(reparsed, policy);
    }

    #[test]
    fn absent_enabled_defaults_to_true() {
        let policy = parse_query_policy(Some(r#"{"tools":{"run_saved_command":{}}}"#)).unwrap();
        assert!(policy.tools["run_saved_command"].enabled);
    }

    #[test]
    fn absent_or_garbage_policy_yields_none() {
        assert_eq!(parse_query_policy(None), None);
        assert_eq!(parse_query_policy(Some("")), None);
        assert_eq!(parse_query_policy(Some("{")), None);
    }

    #[test]
    fn members_accept_current_and_legacy_forms() {
        let raw = r#"["legacyid",{"id":"current"},{"id":"withov","overrides":{"team_key":"ACME"}},42,null]"#;
        let members = parse_members(raw);
        assert_eq!(
            members,
            vec![
                GroupMember {
                    id: "legacyid".into(),
                    overrides: Map::new()
                },
                GroupMember {
                    id: "current".into(),
                    overrides: Map::new()
                },
                GroupMember {
                    id: "withov".into(),
                    overrides: serde_json::from_value(json!({"team_key": "ACME"})).unwrap(),
                },
            ]
        );
    }

    #[test]
    fn garbage_members_yield_none_rather_than_an_error() {
        assert!(parse_members("[]").is_empty());
        assert!(parse_members("not json").is_empty());
        assert!(parse_members(r#"{"id":"x"}"#).is_empty()); // not an array
    }

    #[test]
    fn serialized_members_omit_empty_overrides() {
        let members = vec![
            GroupMember {
                id: "a".into(),
                overrides: Map::new(),
            },
            GroupMember {
                id: "b".into(),
                overrides: serde_json::from_value(json!({"k": "v"})).unwrap(),
            },
        ];
        assert_eq!(
            serialize_members(&members),
            r#"[{"id":"a"},{"id":"b","overrides":{"k":"v"}}]"#
        );
        assert_eq!(serialize_members(&[]), "[]");
    }
}
