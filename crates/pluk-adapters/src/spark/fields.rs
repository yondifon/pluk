use crate::config_field::{ConfigField, FieldType};
use serde_json::json;

pub fn spark_fields() -> Vec<ConfigField> {
    vec![
        ConfigField::new("spark_bin", "Spark CLI", FieldType::Text)
            .group("Spark")
            .placeholder("/usr/local/bin/spark")
            .help("The spark binary installed by Spark Desktop. Spark Desktop must be running."),
        ConfigField::new("timeout_seconds", "Timeout (s)", FieldType::Number)
            .group("Spark")
            .default_value(&json!(30))
            .help("How long a spark command may run before it is killed."),
        ConfigField::new("default_account", "Account", FieldType::Text)
            .group("Defaults")
            .placeholder("you@example.com")
            .help("Confines every folder, search and calendar to this account and drafts from it; blank reaches all accounts. Tools that take a message id are not confined."),
        ConfigField::new("default_folder", "Folder", FieldType::Text)
            .group("Defaults")
            .placeholder("Inbox")
            .help("Folder listed by list_emails when none is given. A bare name like Archive means that folder inside the account above, or the cross-account unified one when no account is set."),
        ConfigField::new("default_team", "Team", FieldType::Text)
            .group("Defaults")
            .help("Team used for comments and team actions when you belong to several."),
        ConfigField::new("max_page_size", "Max Page Size", FieldType::Number)
            .group("Limits")
            .default_value(&json!(25))
            .help("Caps how many emails, meetings or templates one call may return — Spark prints full bodies."),
    ]
}
