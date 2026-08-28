//! Shared SSH auth config fields.
//!
//! The SSH adapter and the SQL adapters' SSH-tunnel sections declare the same
//! three fields (auth method, private key, passphrase/password). A key prefix
//! keeps each owner's stored keys stable — `""` yields `auth_type`/`key_path`/
//! `password`, `"ssh_"` yields `ssh_auth_type`/`ssh_key_path`/… — so this is
//! pure deduplication with no schema migration.

use crate::config_field::{ConfigField, FieldType, ShowIf};

/// The SSH auth block: `auth_type` select (agent / private key / password),
/// the key picker shown only for key auth, and the secret password field.
pub fn ssh_auth_fields(prefix: &str, group: &str, show_if: Option<ShowIf>) -> Vec<ConfigField> {
    let k = |name: &str| format!("{prefix}{name}");
    let mut fields = vec![
        ConfigField::new(k("auth_type"), "Auth", FieldType::Select)
            .group(group)
            .default_value(&serde_json::json!("agent"))
            .options(&[
                ("agent", "Agent"),
                ("key", "Private Key"),
                ("password", "Password"),
            ]),
        ConfigField::new(k("key_path"), "Private Key", FieldType::File)
            .group(group)
            .show_if(ShowIf::eq_str(k("auth_type"), "key")),
        ConfigField::new(k("password"), "Passphrase / Password", FieldType::Password)
            .group(group)
            .secret(),
    ];
    if let Some(show_if) = show_if {
        // The base condition hides the whole block; the key picker keeps its
        // own tighter condition on top of it.
        fields[0] = fields[0].clone().show_if(show_if.clone());
        fields[2] = fields[2].clone().show_if(show_if);
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bare_prefix_yields_the_stored_key_names() {
        let fields = ssh_auth_fields("", "Auth", None);
        let keys: Vec<&str> = fields.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, ["auth_type", "key_path", "password"]);
    }

    #[test]
    fn prefixed_keys_stay_stable_across_integrations() {
        let fields = ssh_auth_fields(
            "ssh_",
            "SSH Tunnel",
            Some(ShowIf::new("use_ssh", &json!(true))),
        );
        let keys: Vec<&str> = fields.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, ["ssh_auth_type", "ssh_key_path", "ssh_password"]);
        // Every field except the key picker carries the block's own
        // visibility condition; the picker is keyed to key auth instead.
        assert_eq!(fields[0].show_if, Some(ShowIf::eq_str("use_ssh", "true")));
        assert_eq!(
            fields[1].show_if,
            Some(ShowIf::eq_str("ssh_auth_type", "key"))
        );
        assert_eq!(fields[2].show_if, Some(ShowIf::eq_str("use_ssh", "true")));
        // And the block ships agent auth as its default.
        assert_eq!(fields[0].default.as_deref(), Some("agent"));
        assert!(fields[2].secret);
    }
}
