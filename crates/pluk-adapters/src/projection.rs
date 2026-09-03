//! Shared `only` field-selection for read tools.
//!
//! A tool declares a [`FieldMap`] (its default dot paths, its named presets,
//! and the full set of top-level field names it accepts); [`apply_only`]
//! projects a fetched payload down to whatever the caller asked for, or the
//! default set when `only` is omitted.
//!
//! Semantics (mirrors `pluk/src/adapters/onlyProjection.ts`):
//!
//! - `["*"]` bypasses projection entirely.
//! - An omitted or empty selection falls back to the map's default paths.
//! - Each entry is a preset name or a dot path; an unknown entry is an error
//!   listing the valid fields and presets.
//! - Projection walks nested structures and maps over arrays wherever they
//!   occur, preserving the original nesting.
//!
//! A path into a missing key yields no key in the output — matching the
//! TypeScript behaviour where the key holds `undefined` and disappears when
//! the response is serialised.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Map, Value};

/// A reducer preset computes its own slice of one item directly — for fields
/// that can't be named in advance (e.g. Sentry's `has*` capability flags).
pub type ReduceFn = Arc<dyn Fn(&Value) -> Map<String, Value> + Send + Sync>;

/// A preset either expands to more dot paths, or reduces one item itself.
#[derive(Clone)]
pub enum Preset {
    Paths(Vec<String>),
    Reduce(ReduceFn),
}

impl Preset {
    pub fn paths(paths: &[&str]) -> Self {
        Preset::Paths(paths.iter().map(|p| (*p).to_string()).collect())
    }

    pub fn reduce(f: impl Fn(&Value) -> Map<String, Value> + Send + Sync + 'static) -> Self {
        Preset::Reduce(Arc::new(f))
    }
}

/// The field-selection contract of one read tool.
#[derive(Clone, Default)]
pub struct FieldMap {
    /// Every top-level field name this tool's payload may carry. Used to
    /// validate `only` entries and to list valid fields in the error.
    pub fields: Vec<String>,
    /// Dot paths returned when `only` is omitted.
    pub default: Vec<String>,
    /// Named shortcuts. A preset name must not be mistaken for a literal
    /// path, so pick names that don't collide with entries in `fields`.
    pub presets: BTreeMap<String, Preset>,
}

impl FieldMap {
    pub fn new(fields: &[&str], default: &[&str]) -> Self {
        FieldMap {
            fields: fields.iter().map(|f| (*f).to_string()).collect(),
            default: default.iter().map(|d| (*d).to_string()).collect(),
            presets: BTreeMap::new(),
        }
    }

    pub fn with_preset(mut self, name: impl Into<String>, preset: Preset) -> Self {
        self.presets.insert(name.into(), preset);
        self
    }
}

/// An `only` entry that names neither a preset nor a known top-level field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnlyError(String);

impl std::fmt::Display for OnlyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for OnlyError {}


/// One node of the trie built from dot paths; empty children mean a leaf.
struct PathNode(BTreeMap<String, PathNode>);

fn build_tree(paths: &[String]) -> PathNode {
    let mut root = PathNode(BTreeMap::new());
    for path in paths {
        let mut node = &mut root;
        for segment in path.split('.') {
            node = node
                .0
                .entry(segment.to_string())
                .or_insert_with(|| PathNode(BTreeMap::new()));
        }
    }
    root
}

fn project_tree(value: &Value, tree: &PathNode) -> Value {
    match value {
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| project_tree(item, tree)).collect())
        }
        Value::Object(object) => {
            let mut out = Map::new();
            for (key, subtree) in &tree.0 {
                // A missing key is omitted, like `undefined` after stringify.
                if let Some(raw) = object.get(key) {
                    let projected = if subtree.0.is_empty() {
                        raw.clone()
                    } else {
                        project_tree(raw, subtree)
                    };
                    out.insert(key.clone(), projected);
                }
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// Project `value` onto the given dot paths, mapping over arrays wherever
/// they occur and preserving the original nesting.
pub fn pick_paths(value: &Value, paths: &[String]) -> Value {
    project_tree(value, &build_tree(paths))
}


fn unknown_field_error(entry: &str, map: &FieldMap) -> OnlyError {
    let mut message = format!(
        "Unknown \"only\" field \"{entry}\". Valid fields: {}.",
        map.fields.join(", ")
    );
    if !map.presets.is_empty() {
        let names: Vec<&str> = map.presets.keys().map(String::as_str).collect();
        message.push_str(&format!(" Presets: {}.", names.join(", ")));
    }
    OnlyError(message)
}

fn project_one(item: &Value, entries: &[String], map: &FieldMap) -> Result<Value, OnlyError> {
    let mut paths: Vec<String> = Vec::new();
    let mut reducers: Vec<&ReduceFn> = Vec::new();
    for entry in entries {
        match map.presets.get(entry.as_str()) {
            Some(Preset::Paths(preset_paths)) => paths.extend(preset_paths.iter().cloned()),
            Some(Preset::Reduce(reduce)) => reducers.push(reduce),
            None => {
                let top = entry.split('.').next().unwrap_or_default();
                if !map.fields.iter().any(|field| field == top) {
                    return Err(unknown_field_error(entry, map));
                }
                paths.push(entry.clone());
            }
        }
    }

    let base = if paths.is_empty() {
        Value::Object(Map::new())
    } else {
        pick_paths(item, &paths)
    };
    if reducers.is_empty() {
        return Ok(base);
    }
    // Reducers merge over the base left-to-right; later keys win, mirroring
    // the object-spread reduce. Spreading a non-object yields nothing, so a
    // non-object base contributes no entries of its own.
    let mut acc = match base {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    for reduce in reducers {
        acc.extend(reduce(item));
    }
    Ok(Value::Object(acc))
}

/// Project a fetched payload (single object or array of objects) according to
/// `only`: `["*"]` bypasses filtering entirely; an omitted or empty `only`
/// falls back to `map.default`; otherwise each entry is a preset name or a
/// dot path, validated against `map.fields`.
pub fn apply_only(
    data: &Value,
    only: Option<&Vec<String>>,
    map: &FieldMap,
) -> Result<Value, OnlyError> {
    if only
        .iter()
        .any(|entries| entries.iter().any(|entry| entry == "*"))
    {
        return Ok(data.clone());
    }
    let entries: &[String] = match only.filter(|o| !o.is_empty()) {
        Some(entries) => entries,
        None => &map.default,
    };
    match data {
        Value::Array(items) => items
            .iter()
            .map(|item| project_one(item, entries, map))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        other => project_one(other, entries, map),
    }
}


/// The `only` parameter description shared by every read tool, so all tools
/// explain the convention identically (including their own presets).
pub fn only_param_description(preset_names: &[&str]) -> String {
    let preset_line = if preset_names.is_empty() {
        String::new()
    } else {
        format!(" Presets: {}.", preset_names.join(", "))
    };
    format!(
        "Trim the response to just these fields — omit for a lighter default, pass [\"*\"] for the full payload. \
         Entries are dot paths (e.g. \"project.slug\") or presets.{preset_line}"
    )
}

/// The JSON-schema fragment for the shared `only` argument.
pub fn only_param_schema(preset_names: &[&str]) -> Value {
    serde_json::json!({
        "type": "array",
        "items": { "type": "string" },
        "description": only_param_description(preset_names),
    })
}

/// Extract the caller's `only` selection from a tool-call argument object.
/// Absent, non-array, or non-string entries yield `None`, which callers treat
/// as "use the default set".
pub fn only_value(args: &Map<String, Value>) -> Option<Vec<String>> {
    match args.get("only") {
        Some(Value::Array(entries)) if entries.iter().all(|e| e.is_string()) => Some(
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(Into::into)
                .collect(),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map() -> FieldMap {
        FieldMap::new(
            &["id", "title", "state", "assignee", "priority", "labels"],
            &["id", "title", "state.name"],
        )
        .with_preset("priority", Preset::paths(&["priority"]))
        .with_preset("ids", Preset::paths(&["id"]))
        .with_preset(
            "flags",
            Preset::reduce(|item| {
                let has_labels = item
                    .get("labels")
                    .and_then(Value::as_array)
                    .is_some_and(|l| !l.is_empty());
                let mut out = Map::new();
                out.insert("hasLabels".to_string(), Value::Bool(has_labels));
                out
            }),
        )
    }

    fn apply(data: Value, only: Option<&[&str]>) -> Result<Value, OnlyError> {
        let owned = only.map(|o| o.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        apply_only(&data, owned.as_ref(), &map())
    }

    #[test]
    fn nested_dot_path_preserves_nesting() {
        let item = json!({ "id": "1", "title": "T", "state": { "name": "Open", "type": "unstarted" }, "assignee": { "name": "Ada" } });
        assert_eq!(
            apply(item, Some(&["assignee.name"])).unwrap(),
            json!({ "assignee": { "name": "Ada" } })
        );
    }

    #[test]
    fn path_crossing_an_array_maps_over_elements() {
        let data = json!({
            "issue": "ENG-1",
            "comments": [
                { "user": { "name": "Ada" }, "body": "hi" },
                { "user": { "name": "Bob" }, "body": "yo" },
            ],
        });
        assert_eq!(
            pick_paths(&data, &["comments.user.name".to_string()]),
            json!({ "comments": [ { "user": { "name": "Ada" } }, { "user": { "name": "Bob" } } ] })
        );
        let issue_map = FieldMap::new(&["issue", "comments"], &["issue", "comments.user.name"]);
        assert_eq!(
            apply_only(&data, None, &issue_map).unwrap(),
            json!({ "issue": "ENG-1", "comments": [ { "user": { "name": "Ada" } }, { "user": { "name": "Bob" } } ] })
        );
    }

    #[test]
    fn path_into_a_missing_key_omits_the_key_rather_than_throwing() {
        let item = json!({ "id": "1", "title": "T" });
        // TS leaves `{ assignee: undefined }`; after stringify that is `{}`.
        assert_eq!(apply(item, Some(&["assignee.name"])).unwrap(), json!({}));
    }

    #[test]
    fn preset_expands_to_its_dot_paths() {
        let item = json!({ "id": "1", "title": "T", "priority": 2 });
        assert_eq!(
            apply(item, Some(&["priority"])).unwrap(),
            json!({ "priority": 2 })
        );
    }

    #[test]
    fn preset_and_literal_path_compose_in_one_selection() {
        let item = json!({ "id": "1", "title": "T", "priority": 2 });
        assert_eq!(
            apply(item, Some(&["title", "priority"])).unwrap(),
            json!({ "title": "T", "priority": 2 })
        );
    }

    #[test]
    fn function_preset_computes_its_own_slice() {
        let item = json!({ "id": "1", "title": "T", "labels": ["bug"] });
        assert_eq!(
            apply(item, Some(&["flags"])).unwrap(),
            json!({ "hasLabels": true })
        );
    }

    #[test]
    fn reducers_merge_over_base_paths_left_to_right() {
        let item = json!({ "id": "1", "labels": ["bug"], "extra": true });
        assert_eq!(
            apply(item, Some(&["flags", "ids", "flags"])).unwrap(),
            json!({ "hasLabels": true, "id": "1" })
        );
    }

    #[test]
    fn omitted_only_returns_the_default_set() {
        let item = json!({ "id": "1", "title": "T", "state": { "name": "Open", "type": "unstarted" }, "priority": 3 });
        assert_eq!(
            apply(item, None).unwrap(),
            json!({ "id": "1", "title": "T", "state": { "name": "Open" } })
        );
    }

    #[test]
    fn star_bypasses_filtering_entirely() {
        let item = json!({ "id": "1", "title": "T", "extra": { "deep": true } });
        assert_eq!(apply(item.clone(), Some(&["*"])).unwrap(), item);
    }

    #[test]
    fn only_applies_per_element_on_a_list() {
        let list = json!([
            { "id": "1", "title": "A", "priority": 1 },
            { "id": "2", "title": "B", "priority": 2 },
        ]);
        assert_eq!(
            apply(list, Some(&["priority"])).unwrap(),
            json!([ { "priority": 1 }, { "priority": 2 } ])
        );
    }

    #[test]
    fn empty_only_array_falls_back_to_the_default_set() {
        let item = json!({ "id": "1", "title": "T", "state": { "name": "Open" } });
        assert_eq!(
            apply(item, Some(&[])).unwrap(),
            json!({ "id": "1", "title": "T", "state": { "name": "Open" } })
        );
    }

    #[test]
    fn unknown_entry_errors_listing_valid_fields_and_presets() {
        let error = apply(json!({ "id": "1" }), Some(&["bogus"])).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Unknown \"only\" field \"bogus\". Valid fields: id, title, state, assignee, priority, labels. Presets: flags, ids, priority."
        );
    }

    #[test]
    fn only_extractor_reads_string_arrays_and_rejects_the_rest() {
        let args =
            serde_json::from_value::<Map<String, Value>>(json!({ "only": ["a", "b.c"] })).unwrap();
        assert_eq!(
            only_value(&args),
            Some(vec!["a".to_string(), "b.c".to_string()])
        );

        let missing = serde_json::from_value::<Map<String, Value>>(json!({})).unwrap();
        assert_eq!(only_value(&missing), None);

        let junk =
            serde_json::from_value::<Map<String, Value>>(json!({ "only": ["a", 3] })).unwrap();
        assert_eq!(only_value(&junk), None);

        let not_array =
            serde_json::from_value::<Map<String, Value>>(json!({ "only": "*" })).unwrap();
        assert_eq!(only_value(&not_array), None);
    }

    #[test]
    fn shared_argument_description_lists_presets() {
        assert_eq!(
            only_param_description(&[]),
            "Trim the response to just these fields — omit for a lighter default, pass [\"*\"] for the full payload. \
             Entries are dot paths (e.g. \"project.slug\") or presets."
        );
        assert!(
            only_param_description(&["connection", "limits"])
                .ends_with(" Presets: connection, limits.")
        );
        let schema = only_param_schema(&[]);
        assert_eq!(schema["type"], "array");
        assert_eq!(schema["items"]["type"], "string");
    }
}
