//! Config fields: the form schema an adapter declares for its integrations.
//!
//! Definitions cross the wire to the frontend verbatim — they never carry
//! secret values, only the shape of the inputs (`secret` marks which stored
//! values must not be echoed back).
//!
//! Secret values are write-only for the window. [`withhold_secrets`] takes
//! them out of a config before it is sent there, and [`keep_secrets`] folds
//! what the window sends back over the stored config, so a secret it never
//! saw is not lost.
//!
//! Two normalisations are part of the contract:
//!
//! - [`ConfigField`] `default` accepts a string, integer or boolean and is
//!   normalised to a string at construction.
//! - [`ShowIf`] `equals` compares as a string after the same normalisation,
//!   so a toggle's `true` matches the string `"true"`.

use pluk_store::SecretKind;
use serde::Serialize;
use serde_json::{Map, Value};

/// Normalise a JSON value the way config defaults and `show_if.equals`
/// compare: strings verbatim, booleans as `"true"`/`"false"`, numbers by
/// their display form. Containers fall back to their compact JSON text.
pub fn normalize_scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// How a config input renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Text,
    Password,
    Number,
    File,
    Select,
    Toggle,
    /// A list of named values, each either plain or secret. See
    /// [`crate::key_value`] for how the rows are stored.
    KeyValue,
    /// An ordered list of strings, stored as a JSON array.
    List,
}

impl FieldType {
    pub fn as_str(self) -> &'static str {
        match self {
            FieldType::Text => "text",
            FieldType::Password => "password",
            FieldType::Number => "number",
            FieldType::File => "file",
            FieldType::Select => "select",
            FieldType::Toggle => "toggle",
            FieldType::KeyValue => "keyvalue",
            FieldType::List => "list",
        }
    }
}

/// One option of a [`FieldType::Select`] field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

/// Conditional visibility: show this field only when `config[key]` equals
/// `equals` (or, with [`ShowIf::negated`], only when it does not), both
/// compared as normalised strings. A missing key never equals anything, so a
/// negated condition is how a field shows by default for integrations saved
/// before the key existed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowIf {
    pub key: String,
    /// Normalised comparison target (booleans become `"true"`/`"false"`).
    pub equals: String,
    pub negate: bool,
}

impl ShowIf {
    /// Build from any JSON scalar; the comparison value is normalised once.
    pub fn new(key: impl Into<String>, equals: &Value) -> Self {
        ShowIf {
            key: key.into(),
            equals: normalize_scalar(equals),
            negate: false,
        }
    }

    pub fn eq_str(key: impl Into<String>, equals: &str) -> Self {
        ShowIf {
            key: key.into(),
            equals: equals.to_string(),
            negate: false,
        }
    }

    /// Show when the config value does *not* equal `equals`, instead of when
    /// it does.
    pub fn negated(mut self) -> Self {
        self.negate = true;
        self
    }

    /// Whether a stored config value satisfies the condition.
    pub fn matches(&self, value: Option<&Value>) -> bool {
        let equal = value.map(normalize_scalar).as_deref() == Some(self.equals.as_str());
        equal != self.negate
    }
}

impl Serialize for ShowIf {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let len = if self.negate { 3 } else { 2 };
        let mut state = serializer.serialize_struct("ShowIf", len)?;
        state.serialize_field("key", &self.key)?;
        state.serialize_field("equals", &self.equals)?;
        if self.negate {
            state.serialize_field("negate", &self.negate)?;
        }
        state.end()
    }
}

/// A single config input, rendered dynamically by the UI form.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigField {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub field_type: FieldType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
    /// Never echoed back to the UI.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub secret: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// Normalised default (always a string on the wire).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<SelectOption>,
    #[serde(rename = "showIf", skip_serializing_if = "Option::is_none")]
    pub show_if: Option<ShowIf>,
    #[serde(rename = "fileTypes", skip_serializing_if = "Vec::is_empty")]
    pub file_types: Vec<String>,
    /// Flag a risky setting; the UI styles it red.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub danger: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Where a [`FieldType::KeyValue`] field's secret row values are saved.
    /// Never sent to the window.
    #[serde(skip)]
    pub secret_kind: Option<SecretKind>,
    /// Whether a new row of a [`FieldType::KeyValue`] field starts secret.
    /// Headers default to secret; a field of routing data, such as
    /// environment variables, can default the other way.
    #[serde(rename = "defaultSecret", skip_serializing_if = "is_true")]
    pub default_secret: bool,
}

fn is_true(value: &bool) -> bool {
    *value
}

impl ConfigField {
    pub fn new(key: impl Into<String>, label: impl Into<String>, field_type: FieldType) -> Self {
        ConfigField {
            key: key.into(),
            label: label.into(),
            field_type,
            group: None,
            required: false,
            secret: false,
            placeholder: None,
            default: None,
            options: Vec::new(),
            show_if: None,
            file_types: Vec::new(),
            danger: false,
            help: None,
            secret_kind: None,
            default_secret: true,
        }
    }

    /// A list of named values whose secret rows are saved as `kind`.
    pub fn key_value(key: impl Into<String>, label: impl Into<String>, kind: SecretKind) -> Self {
        ConfigField {
            secret_kind: Some(kind),
            ..ConfigField::new(key, label, FieldType::KeyValue)
        }
    }

    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn secret(mut self) -> Self {
        self.secret = true;
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Set the default from any JSON scalar; it is normalised to a string.
    pub fn default_value(mut self, default: &Value) -> Self {
        self.default = Some(normalize_scalar(default));
        self
    }

    pub fn options(mut self, options: &[(&str, &str)]) -> Self {
        self.options = options
            .iter()
            .map(|(value, label)| SelectOption {
                value: (*value).into(),
                label: (*label).into(),
            })
            .collect();
        self
    }

    pub fn show_if(mut self, show_if: ShowIf) -> Self {
        self.show_if = Some(show_if);
        self
    }

    pub fn show_if_eq(mut self, key: impl Into<String>, equals: &Value) -> Self {
        self.show_if = Some(ShowIf::new(key, equals));
        self
    }

    /// Show this field except when `config[key]` equals `equals`. A key the
    /// config never set counts as not equal, so this is how a field already
    /// on integrations saved before `key` existed keeps showing.
    pub fn show_unless_eq(mut self, key: impl Into<String>, equals: &Value) -> Self {
        self.show_if = Some(ShowIf::new(key, equals).negated());
        self
    }

    /// A new row of this [`FieldType::KeyValue`] field starts plain instead
    /// of secret.
    pub fn default_not_secret(mut self) -> Self {
        self.default_secret = false;
        self
    }

    pub fn file_types(mut self, types: &[&str]) -> Self {
        self.file_types = types.iter().map(|t| (*t).to_string()).collect();
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}

/// Take every secret field's value out of a config bound for the window, and
/// name the fields that hold one. An empty value counts as not set.
pub fn withhold_secrets(config: &mut Map<String, Value>, fields: &[ConfigField]) -> Vec<String> {
    fields
        .iter()
        .filter(|field| field.secret)
        .filter_map(|field| {
            let value = config.remove(&field.key)?;
            (!is_blank(&value)).then(|| field.key.clone())
        })
        .collect()
}

/// Fold a config the window sent over the stored one. For each secret field,
/// an absent or empty value keeps what is stored, `null` removes it, and any
/// other value replaces it. Every other key is taken as sent.
pub fn keep_secrets(
    stored: &Map<String, Value>,
    mut sent: Map<String, Value>,
    fields: &[ConfigField],
) -> Map<String, Value> {
    for field in fields.iter().filter(|field| field.secret) {
        match sent.remove(&field.key) {
            Some(Value::Null) => {}
            Some(value) if !is_blank(&value) => {
                sent.insert(field.key.clone(), value);
            }
            _ => {
                if let Some(kept) = stored.get(&field.key) {
                    sent.insert(field.key.clone(), kept.clone());
                }
            }
        }
    }
    sent
}

fn is_blank(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_normalises_scalars_to_strings() {
        assert_eq!(
            ConfigField::new("a", "A", FieldType::Text)
                .default_value(&json!("agent"))
                .default,
            Some("agent".into())
        );
        assert_eq!(
            ConfigField::new("p", "P", FieldType::Number)
                .default_value(&json!(5432))
                .default,
            Some("5432".into())
        );
        assert_eq!(
            ConfigField::new("t", "T", FieldType::Toggle)
                .default_value(&json!(true))
                .default,
            Some("true".into())
        );
        assert_eq!(
            ConfigField::new("f", "F", FieldType::Toggle)
                .default_value(&json!(false))
                .default,
            Some("false".into())
        );
    }

    #[test]
    fn serializes_with_omitted_empties_like_the_ts_shape() {
        let field = ConfigField::new("host", "Host", FieldType::Text)
            .group("Connection")
            .required()
            .placeholder("localhost")
            .help("Where to connect.");
        let value = serde_json::to_value(&field).unwrap();
        assert_eq!(
            value,
            json!({
                "key": "host",
                "label": "Host",
                "type": "text",
                "group": "Connection",
                "required": true,
                "placeholder": "localhost",
                "help": "Where to connect.",
            })
        );
    }

    #[test]
    fn show_if_normalises_booleans_for_comparison() {
        let show_if = ShowIf::new("use_ssh", &json!(true));
        assert_eq!(show_if.equals, "true");
        // A stored boolean and its string form match alike.
        assert!(show_if.matches(Some(&json!(true))));
        assert!(show_if.matches(Some(&json!("true"))));
        assert!(!show_if.matches(Some(&json!(false))));
        assert!(!show_if.matches(None));
    }

    #[test]
    fn a_negated_show_if_shows_by_default_for_a_key_never_saved() {
        let show_if = ShowIf::new("connection", &json!("local")).negated();
        // Never saved (old integrations) or saved as something else: shown.
        assert!(show_if.matches(None));
        assert!(show_if.matches(Some(&json!("remote"))));
        // Saved as the excluded value: hidden.
        assert!(!show_if.matches(Some(&json!("local"))));

        let field = ConfigField::new("url", "URL", FieldType::Text)
            .show_unless_eq("connection", &json!("local"));
        let value = serde_json::to_value(&field).unwrap();
        assert_eq!(
            value["showIf"],
            json!({ "key": "connection", "equals": "local", "negate": true })
        );
    }

    #[test]
    fn a_key_value_field_defaults_new_rows_to_secret_unless_told_otherwise() {
        let headers = ConfigField::key_value("headers", "Headers", SecretKind::Header);
        assert!(headers.default_secret);
        assert!(
            serde_json::to_value(&headers)
                .unwrap()
                .get("defaultSecret")
                .is_none()
        );

        let env = ConfigField::key_value("env", "Environment variables", SecretKind::Env)
            .default_not_secret();
        assert!(!env.default_secret);
        assert_eq!(
            serde_json::to_value(&env).unwrap()["defaultSecret"],
            json!(false)
        );
    }

    #[test]
    fn select_options_round_trip() {
        let field = ConfigField::new("auth_type", "Auth", FieldType::Select)
            .options(&[("agent", "Agent"), ("key", "Private Key")]);
        let value = serde_json::to_value(&field).unwrap();
        assert_eq!(
            value["options"],
            json!([{ "value": "agent", "label": "Agent" }, { "value": "key", "label": "Private Key" }])
        );
    }

    fn secret_fields() -> Vec<ConfigField> {
        vec![
            ConfigField::new("url", "URL", FieldType::Text),
            ConfigField::new("token", "Token", FieldType::Password).secret(),
            ConfigField::new("client_secret", "Client secret", FieldType::Password).secret(),
        ]
    }

    fn map(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            _ => unreachable!(),
        }
    }

    #[test]
    fn the_window_is_told_which_secrets_are_set_and_never_their_values() {
        let mut config = map(json!({"url": "https://x", "token": "t0k", "client_secret": ""}));
        let set = withhold_secrets(&mut config, &secret_fields());
        assert_eq!(set, vec!["token".to_string()]);
        assert_eq!(config, map(json!({"url": "https://x"})));
    }

    #[test]
    fn a_secret_the_window_leaves_alone_keeps_its_stored_value() {
        let stored = map(json!({"url": "https://x", "token": "t0k", "client_secret": "s3c"}));
        let kept = keep_secrets(
            &stored,
            map(json!({"url": "https://y", "client_secret": ""})),
            &secret_fields(),
        );
        assert_eq!(
            kept,
            map(json!({"url": "https://y", "token": "t0k", "client_secret": "s3c"}))
        );
    }

    #[test]
    fn a_secret_is_replaced_by_a_new_value_and_removed_by_null() {
        let stored = map(json!({"token": "t0k", "client_secret": "s3c"}));
        let kept = keep_secrets(
            &stored,
            map(json!({"token": "new", "client_secret": null})),
            &secret_fields(),
        );
        assert_eq!(kept, map(json!({"token": "new"})));
    }
}
