//! Rows of a [`FieldType::KeyValue`] field: a list of named values, each one
//! plain or secret.
//!
//! The config holds the rows in order. A plain row keeps its value there; a
//! secret row keeps only its name, and its value is saved in `proxy_secrets`
//! under the field's [`SecretKind`]. So the config can go to the window as it
//! is, and [`show_secret_rows`] adds whether each secret row holds a value.
//!
//! What the window sends back is one object per row:
//! `{ "name", "value", "secret", "savedName" }`. A secret row sent without a
//! value keeps the one saved under `savedName` (or its own name), which is how
//! a row renamed without retyping its value keeps it. A row left out is
//! cleared. `secret` defaults to on.
//!
//! [`FieldType::KeyValue`]: crate::FieldType::KeyValue

use std::collections::{HashMap, HashSet};

use pluk_store::{Config, ProxySecret, SecretKind, SecretWrite};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::config_field::ConfigField;

/// One row as the config or the window holds it.
#[derive(Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    /// Empty for a secret row, whose value lives in `proxy_secrets`, and for a
    /// secret row the window left blank to keep what is saved.
    pub value: String,
    pub secret: bool,
    /// The name the row had when the window read it.
    pub saved_name: Option<String>,
}

impl Row {
    fn is_blank(&self) -> bool {
        self.name.is_empty() && self.value.is_empty()
    }
}

impl std::fmt::Debug for Row {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = if self.secret {
            "<redacted>"
        } else {
            &self.value
        };
        f.debug_struct("Row")
            .field("name", &self.name)
            .field("value", &value)
            .field("secret", &self.secret)
            .field("saved_name", &self.saved_name)
            .finish()
    }
}

/// A config the save would refuse, and where: the field, the row counted
/// from zero as sent, and what to fix. The message names the row, never its
/// value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigProblem {
    pub field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<usize>,
    pub message: String,
}

impl ConfigProblem {
    pub fn at_row(field: &str, row: usize, message: impl Into<String>) -> Self {
        ConfigProblem {
            field: field.to_string(),
            row: Some(row),
            message: message.into(),
        }
    }
}

/// The rows a config holds under `key`, in order. Anything that is not a row
/// reads as a blank one, so positions match what was sent.
pub fn rows(config: &Map<String, Value>, key: &str) -> Vec<Row> {
    let Some(Value::Array(items)) = config.get(key) else {
        return Vec::new();
    };
    items.iter().map(row_of).collect()
}

fn row_of(item: &Value) -> Row {
    let text = |key: &str| {
        item.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string()
    };
    Row {
        name: text("name"),
        value: text("value"),
        secret: item.get("secret").and_then(Value::as_bool).unwrap_or(true),
        saved_name: Some(text("savedName")).filter(|name| !name.is_empty()),
    }
}

/// The secrets a save's blank secret rows keep.
#[derive(Clone, Copy)]
pub enum KeptFrom<'a> {
    /// The integration being saved: a kept value stays where it is.
    Itself(&'a [ProxySecret]),
    /// The integration a new one copies: a kept value is written anew.
    Copy(&'a [ProxySecret]),
}

/// Turn the rows the window sent into the rows the config keeps, and the
/// secret writes that go with them. A field left out of `config` keeps its
/// rows and their saved values as they are.
pub fn fold_secret_rows(
    fields: &[ConfigField],
    config: &mut Config,
    kept: KeptFrom<'_>,
) -> Result<Vec<SecretWrite>, ConfigProblem> {
    let mut writes = Vec::new();
    for field in fields {
        let Some(kind) = field.secret_kind else {
            continue;
        };
        match config.get(&field.key) {
            None => continue,
            Some(Value::Array(_)) => {}
            Some(_) => {
                return Err(ConfigProblem {
                    field: field.key.clone(),
                    row: None,
                    message: format!("{} has to be a list.", field.label),
                });
            }
        }
        let (stored_rows, wanted) = settle_rows(&field.key, &rows(config, &field.key), kind, kept)?;
        config.insert(field.key.clone(), Value::Array(stored_rows));
        writes.extend(writes_for(kind, wanted, kept));
    }
    Ok(writes)
}

/// Where a secret row's value comes from.
enum Wanted {
    New(String),
    Kept { from: String, value: String },
}

type Settled = (Vec<Value>, Vec<(String, Wanted)>);

fn settle_rows(
    key: &str,
    sent: &[Row],
    kind: SecretKind,
    kept: KeptFrom<'_>,
) -> Result<Settled, ConfigProblem> {
    let saved = saved_values(kept, kind);
    let mut stored = Vec::new();
    let mut wanted = Vec::new();
    let mut seen = HashSet::new();
    for (index, row) in sent.iter().enumerate() {
        if row.is_blank() {
            continue;
        }
        if row.name.is_empty() {
            return Err(ConfigProblem::at_row(
                key,
                index,
                "Add a name for this value.",
            ));
        }
        if !seen.insert(row.name.to_ascii_lowercase()) {
            return Err(ConfigProblem::at_row(
                key,
                index,
                format!("{} is already in the list.", row.name),
            ));
        }
        let missing =
            || ConfigProblem::at_row(key, index, format!("Add a value for {}.", row.name));
        if !row.secret {
            if row.value.is_empty() {
                return Err(missing());
            }
            stored.push(json!({ "name": row.name, "value": row.value, "secret": false }));
            continue;
        }
        let source = if row.value.is_empty() {
            let from = row.saved_name.clone().unwrap_or_else(|| row.name.clone());
            let value = saved.get(from.as_str()).ok_or_else(missing)?.to_string();
            Wanted::Kept { from, value }
        } else {
            Wanted::New(row.value.clone())
        };
        stored.push(json!({ "name": row.name, "secret": true }));
        wanted.push((row.name.clone(), source));
    }
    Ok((stored, wanted))
}

fn saved_values<'a>(kept: KeptFrom<'a>, kind: SecretKind) -> HashMap<&'a str, &'a str> {
    let (KeptFrom::Itself(saved) | KeptFrom::Copy(saved)) = kept;
    saved
        .iter()
        .filter(|secret| secret.kind == kind)
        .map(|secret| (secret.name.as_str(), secret.value.as_str()))
        .collect()
}

/// The writes that leave exactly `wanted` saved. On the integration itself a
/// kept value is renamed in place, unless its new name is still saved or its
/// old one is still wanted; then it is written as a set from the value read
/// before the save. A copy starts from nothing, so every value is set.
fn writes_for(
    kind: SecretKind,
    wanted: Vec<(String, Wanted)>,
    kept: KeptFrom<'_>,
) -> Vec<SecretWrite> {
    let set = |name: String, value: String| SecretWrite::Set { kind, name, value };
    let KeptFrom::Itself(saved) = kept else {
        return wanted
            .into_iter()
            .map(|(name, source)| match source {
                Wanted::New(value) | Wanted::Kept { value, .. } => set(name, value),
            })
            .collect();
    };
    let saved: Vec<&str> = saved
        .iter()
        .filter(|secret| secret.kind == kind)
        .map(|secret| secret.name.as_str())
        .collect();
    let wanted_names: HashSet<&str> = wanted.iter().map(|(name, _)| name.as_str()).collect();
    let mut moved: HashSet<&str> = HashSet::new();
    let mut renames = Vec::new();
    let mut sets = Vec::new();
    for (name, source) in &wanted {
        match source {
            Wanted::Kept { from, .. } if from == name => {}
            Wanted::Kept { from, value } => {
                let free = !saved.contains(&name.as_str())
                    && !wanted_names.contains(from.as_str())
                    && !moved.contains(from.as_str());
                if free {
                    moved.insert(from.as_str());
                    renames.push(SecretWrite::Rename {
                        kind,
                        from: from.clone(),
                        to: name.clone(),
                    });
                } else {
                    sets.push(set(name.clone(), value.clone()));
                }
            }
            Wanted::New(value) => sets.push(set(name.clone(), value.clone())),
        }
    }
    let clears = saved
        .into_iter()
        .filter(|name| !wanted_names.contains(name) && !moved.contains(name))
        .map(|name| SecretWrite::Clear {
            kind,
            name: name.to_string(),
        });
    clears.chain(renames).chain(sets).collect()
}

/// Mark each secret row of a config bound for the window with whether a
/// value is saved for it. A secret row never carries a value there.
pub fn show_secret_rows(config: &mut Config, fields: &[ConfigField], saved: &[ProxySecret]) {
    for field in fields {
        let Some(kind) = field.secret_kind else {
            continue;
        };
        let Some(Value::Array(items)) = config.get_mut(&field.key) else {
            continue;
        };
        for item in items.iter_mut() {
            let row = row_of(item);
            if !row.secret {
                continue;
            }
            let set = saved
                .iter()
                .any(|secret| secret.kind == kind && secret.name == row.name);
            *item = json!({ "name": row.name, "secret": true, "set": set });
        }
    }
}

/// Whether any field keeps secret rows, so a caller can skip reading them.
pub fn has_secret_rows(fields: &[ConfigField]) -> bool {
    fields.iter().any(|field| field.secret_kind.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> Vec<ConfigField> {
        vec![ConfigField::key_value(
            "headers",
            "Headers",
            SecretKind::Header,
        )]
    }

    fn config(value: Value) -> Config {
        match value {
            Value::Object(map) => map,
            _ => unreachable!(),
        }
    }

    fn secret(name: &str, value: &str) -> ProxySecret {
        ProxySecret {
            integration_id: "int-1".to_string(),
            kind: SecretKind::Header,
            name: name.to_string(),
            value: value.to_string(),
            version: 1,
        }
    }

    fn set(name: &str, value: &str) -> SecretWrite {
        SecretWrite::Set {
            kind: SecretKind::Header,
            name: name.to_string(),
            value: value.to_string(),
        }
    }

    #[test]
    fn secret_values_leave_the_config_and_plain_ones_stay() {
        let mut sent = config(json!({"headers": [
            {"name": "DD_API_KEY", "value": "api-1"},
            {"name": "X-Grafana-URL", "value": "https://grafana.example.com", "secret": false},
            {"name": "", "value": ""},
        ]}));
        let writes = fold_secret_rows(&fields(), &mut sent, KeptFrom::Itself(&[])).expect("fold");

        assert_eq!(writes, [set("DD_API_KEY", "api-1")]);
        assert_eq!(
            Value::Object(sent),
            json!({"headers": [
                {"name": "DD_API_KEY", "secret": true},
                {"name": "X-Grafana-URL", "value": "https://grafana.example.com", "secret": false},
            ]})
        );
    }

    #[test]
    fn a_blank_secret_keeps_its_value_and_a_renamed_one_takes_it_along() {
        let saved = [
            secret("DD_API_KEY", "api-1"),
            secret("DD_APPLICATION_KEY", "app-1"),
            secret("Old", "o"),
        ];
        let mut sent = config(json!({"headers": [
            {"name": "DD_API_KEY", "value": "", "savedName": "DD_API_KEY"},
            {"name": "X-App-Key", "savedName": "DD_APPLICATION_KEY"},
        ]}));
        let writes =
            fold_secret_rows(&fields(), &mut sent, KeptFrom::Itself(&saved)).expect("fold");

        assert_eq!(
            writes,
            [
                SecretWrite::Clear {
                    kind: SecretKind::Header,
                    name: "Old".to_string(),
                },
                SecretWrite::Rename {
                    kind: SecretKind::Header,
                    from: "DD_APPLICATION_KEY".to_string(),
                    to: "X-App-Key".to_string(),
                },
            ]
        );
    }

    #[test]
    fn two_saved_rows_swapping_names_keep_each_others_values() {
        let saved = [secret("A", "a"), secret("B", "b")];
        let mut sent = config(json!({"headers": [
            {"name": "B", "savedName": "A"},
            {"name": "A", "savedName": "B"},
        ]}));
        let writes =
            fold_secret_rows(&fields(), &mut sent, KeptFrom::Itself(&saved)).expect("fold");
        assert_eq!(writes, [set("B", "a"), set("A", "b")]);
    }

    #[test]
    fn a_copy_writes_every_kept_value_anew() {
        let saved = [secret("DD_API_KEY", "api-1")];
        let mut sent = config(json!({"headers": [{"name": "DD_API_KEY", "secret": true}]}));
        let writes = fold_secret_rows(&fields(), &mut sent, KeptFrom::Copy(&saved)).expect("fold");
        assert_eq!(writes, [set("DD_API_KEY", "api-1")]);
    }

    #[test]
    fn a_field_left_out_keeps_its_rows_and_an_empty_list_clears_them() {
        let saved = [secret("DD_API_KEY", "api-1")];
        let mut untouched = config(json!({"url": "https://up/mcp"}));
        assert!(
            fold_secret_rows(&fields(), &mut untouched, KeptFrom::Itself(&saved))
                .expect("fold")
                .is_empty()
        );

        let mut emptied = config(json!({"headers": []}));
        assert_eq!(
            fold_secret_rows(&fields(), &mut emptied, KeptFrom::Itself(&saved)).expect("fold"),
            [SecretWrite::Clear {
                kind: SecretKind::Header,
                name: "DD_API_KEY".to_string(),
            }]
        );
    }

    #[test]
    fn a_row_the_save_cannot_take_is_named_by_position_and_never_by_value() {
        let problem = |rows: Value| {
            fold_secret_rows(
                &fields(),
                &mut config(json!({ "headers": rows })),
                KeptFrom::Itself(&[]),
            )
            .expect_err("refused")
        };
        assert_eq!(
            problem(json!([{"name": "", "value": "hidden-1"}])),
            ConfigProblem::at_row("headers", 0, "Add a name for this value.")
        );
        assert_eq!(
            problem(json!([{"name": "", "value": ""}, {"name": "DD_API_KEY"}])),
            ConfigProblem::at_row("headers", 1, "Add a value for DD_API_KEY.")
        );
        assert_eq!(
            problem(json!([{"name": "X-Org", "value": "", "secret": false}])),
            ConfigProblem::at_row("headers", 0, "Add a value for X-Org.")
        );
        assert_eq!(
            problem(json!([
                {"name": "DD_API_KEY", "value": "hidden-1"},
                {"name": "dd_api_key", "value": "hidden-2"},
            ])),
            ConfigProblem::at_row("headers", 1, "dd_api_key is already in the list.")
        );
        assert_eq!(
            fold_secret_rows(
                &fields(),
                &mut config(json!({"headers": "X: y"})),
                KeptFrom::Itself(&[])
            )
            .expect_err("not a list")
            .message,
            "Headers has to be a list."
        );
    }

    #[test]
    fn the_window_learns_which_secret_rows_are_saved_and_nothing_more() {
        let mut stored = config(json!({"headers": [
            {"name": "DD_API_KEY", "secret": true},
            {"name": "DD_APPLICATION_KEY", "secret": true, "value": "leaked-1"},
            {"name": "X-Org", "value": "acme", "secret": false},
        ]}));
        show_secret_rows(&mut stored, &fields(), &[secret("DD_API_KEY", "api-1")]);

        assert_eq!(
            Value::Object(stored),
            json!({"headers": [
                {"name": "DD_API_KEY", "secret": true, "set": true},
                {"name": "DD_APPLICATION_KEY", "secret": true, "set": false},
                {"name": "X-Org", "value": "acme", "secret": false},
            ]})
        );
    }

    #[test]
    fn debugging_a_secret_row_shows_no_value() {
        let printed = format!(
            "{:?}",
            rows(
                &config(json!({"headers": [{"name": "K", "value": "hidden-1"}]})),
                "headers"
            )
        );
        assert!(!printed.contains("hidden-1"), "{printed}");
    }

    #[test]
    fn the_field_goes_to_the_window_without_where_its_secrets_are_kept() {
        let value = serde_json::to_value(&fields()[0]).unwrap();
        assert_eq!(
            value,
            json!({"key": "headers", "label": "Headers", "type": "keyvalue"})
        );
    }
}
