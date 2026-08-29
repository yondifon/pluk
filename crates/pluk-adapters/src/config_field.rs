//! Config fields: the form schema an adapter declares for its integrations.
//!
//! Definitions cross the wire to the frontend verbatim — they never carry
//! secret values, only the shape of the inputs (`secret` marks which stored
//! values must not be echoed back).
//!
//! Two normalisations are part of the contract:
//!
//! - [`ConfigField`] `default` accepts a string, integer or boolean and is
//!   normalised to a string at construction.
//! - [`ShowIf`] `equals` compares as a string after the same normalisation,
//!   so a toggle's `true` matches the string `"true"`.

use serde::Serialize;
use serde_json::Value;

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
/// `equals`, both compared as normalised strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowIf {
    pub key: String,
    /// Normalised comparison target (booleans become `"true"`/`"false"`).
    pub equals: String,
}

impl ShowIf {
    /// Build from any JSON scalar; the comparison value is normalised once.
    pub fn new(key: impl Into<String>, equals: &Value) -> Self {
        ShowIf {
            key: key.into(),
            equals: normalize_scalar(equals),
        }
    }

    pub fn eq_str(key: impl Into<String>, equals: &str) -> Self {
        ShowIf {
            key: key.into(),
            equals: equals.to_string(),
        }
    }

    /// Whether a stored config value satisfies the condition.
    pub fn matches(&self, value: Option<&Value>) -> bool {
        value.map(normalize_scalar).as_deref() == Some(self.equals.as_str())
    }
}

impl Serialize for ShowIf {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("ShowIf", 2)?;
        state.serialize_field("key", &self.key)?;
        state.serialize_field("equals", &self.equals)?;
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
    fn select_options_round_trip() {
        let field = ConfigField::new("auth_type", "Auth", FieldType::Select)
            .options(&[("agent", "Agent"), ("key", "Private Key")]);
        let value = serde_json::to_value(&field).unwrap();
        assert_eq!(
            value["options"],
            json!([{ "value": "agent", "label": "Agent" }, { "value": "key", "label": "Private Key" }])
        );
    }
}
