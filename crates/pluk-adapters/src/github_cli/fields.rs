use crate::config_field::{ConfigField, FieldType};
use serde_json::json;

pub fn github_cli_fields() -> Vec<ConfigField> {
    vec![
        ConfigField::new("gh_bin", "gh Executable", FieldType::Text)
            .group("Connection")
            .default_value(&json!("gh"))
            .placeholder("gh on PATH, or an absolute path"),
        ConfigField::new("timeout_seconds", "Timeout (seconds)", FieldType::Number)
            .group("Connection")
            .default_value(&json!(30))
            .placeholder("How long one gh command may run"),
        ConfigField::new("default_repo", "Default Repo", FieldType::Text)
            .group("Defaults")
            .placeholder("owner/repo (optional)"),
        ConfigField::new("default_cwd", "Default Working Directory", FieldType::Text)
            .group("Defaults")
            .placeholder("Used when a call passes no cwd"),
    ]
}
