use crate::config_field::{ConfigField, FieldType, ShowIf};
use crate::ssh_fields::ssh_auth_fields;
use serde_json::json;

pub fn network_sql_fields(default_port: u16) -> Vec<ConfigField> {
    let mut fields = vec![
        ConfigField::new("host", "Host", FieldType::Text)
            .group("Connection")
            .placeholder("localhost")
            .default_value(&json!("localhost")),
        ConfigField::new("port", "Port", FieldType::Number)
            .group("Connection")
            .default_value(&json!(default_port)),
        ConfigField::new("user", "User", FieldType::Text).group("Connection"),
        ConfigField::new("password", "Password", FieldType::Password)
            .group("Connection")
            .secret(),
        ConfigField::new("database", "Database", FieldType::Text).group("Connection"),
        ConfigField::new("socket_path", "Socket", FieldType::Text)
            .group("Connection")
            .placeholder("Leave empty for TCP (optional)"),
        ConfigField::new("use_ssh", "SSH Tunnel", FieldType::Toggle).group("SSH Tunnel"),
        ConfigField::new("ssh_host", "SSH Host", FieldType::Text)
            .group("SSH Tunnel")
            .show_if(ShowIf::eq_str("use_ssh", "true")),
        ConfigField::new("ssh_port", "SSH Port", FieldType::Number)
            .group("SSH Tunnel")
            .default_value(&json!(22))
            .show_if(ShowIf::eq_str("use_ssh", "true")),
        ConfigField::new("ssh_user", "SSH User", FieldType::Text)
            .group("SSH Tunnel")
            .show_if(ShowIf::eq_str("use_ssh", "true")),
    ];
    let ssh = ssh_auth_fields(
        "ssh_",
        "SSH Tunnel",
        Some(ShowIf::eq_str("use_ssh", "true")),
    );
    fields.extend(ssh);
    fields.extend(vec![
        ConfigField::new("use_ssl", "SSL / TLS", FieldType::Toggle).group("SSL / TLS"),
        ConfigField::new("ssl_mode", "Mode", FieldType::Select)
            .group("SSL / TLS")
            .default_value(&json!("require"))
            .options(&[
                ("disable", "Disable"),
                ("require", "Require"),
                ("verify-ca", "Verify CA"),
                ("verify-full", "Verify Full"),
            ])
            .show_if(ShowIf::eq_str("use_ssl", "true")),
        ConfigField::new("ssl_ca_path", "CA Cert", FieldType::File)
            .group("SSL / TLS")
            .file_types(&["pem", "crt", "cert"])
            .show_if(ShowIf::eq_str("use_ssl", "true")),
        ConfigField::new("ssl_cert_path", "Client Cert", FieldType::File)
            .group("SSL / TLS")
            .file_types(&["pem", "crt", "cert"])
            .show_if(ShowIf::eq_str("use_ssl", "true")),
        ConfigField::new("ssl_key_path", "Client Key", FieldType::File)
            .group("SSL / TLS")
            .file_types(&["pem", "key"])
            .show_if(ShowIf::eq_str("use_ssl", "true")),
    ]);
    fields
}

pub fn sqlite_fields() -> Vec<ConfigField> {
    let mut fields = vec![
        ConfigField::new("filename", "Path", FieldType::File)
            .group("File")
            .required()
            .placeholder("/path/to/db.sqlite")
            .file_types(&["db", "sqlite", "sqlite3"]),
        ConfigField::new("use_ssh", "SSH", FieldType::Toggle).group("SSH"),
        ConfigField::new("ssh_host", "SSH Host", FieldType::Text)
            .group("SSH")
            .show_if(ShowIf::eq_str("use_ssh", "true")),
        ConfigField::new("ssh_port", "SSH Port", FieldType::Number)
            .group("SSH")
            .default_value(&json!(22))
            .show_if(ShowIf::eq_str("use_ssh", "true")),
        ConfigField::new("ssh_user", "SSH User", FieldType::Text)
            .group("SSH")
            .show_if(ShowIf::eq_str("use_ssh", "true")),
    ];
    let ssh = ssh_auth_fields("ssh_", "SSH", Some(ShowIf::eq_str("use_ssh", "true")));
    fields.extend(ssh);
    fields
}

pub fn mssql_fields() -> Vec<ConfigField> {
    let mut fields = vec![
        ConfigField::new("host", "Host", FieldType::Text)
            .group("Connection")
            .placeholder("localhost")
            .default_value(&json!("localhost")),
        ConfigField::new("port", "Port", FieldType::Number)
            .group("Connection")
            .default_value(&json!(1433)),
        ConfigField::new("user", "User", FieldType::Text).group("Connection"),
        ConfigField::new("password", "Password", FieldType::Password)
            .group("Connection")
            .secret(),
        ConfigField::new("database", "Database", FieldType::Text).group("Connection"),
        ConfigField::new("encrypt", "Encrypt connection", FieldType::Toggle)
            .group("Security")
            .default_value(&json!(true)),
        ConfigField::new("trust_cert", "Trust server certificate", FieldType::Toggle)
            .group("Security")
            .default_value(&json!(false))
            .danger()
            .show_if(ShowIf::eq_str("encrypt", "true")),
        ConfigField::new("use_ssh", "SSH Tunnel", FieldType::Toggle).group("SSH Tunnel"),
        ConfigField::new("ssh_host", "SSH Host", FieldType::Text)
            .group("SSH Tunnel")
            .show_if(ShowIf::eq_str("use_ssh", "true")),
        ConfigField::new("ssh_port", "SSH Port", FieldType::Number)
            .group("SSH Tunnel")
            .default_value(&json!(22))
            .show_if(ShowIf::eq_str("use_ssh", "true")),
        ConfigField::new("ssh_user", "SSH User", FieldType::Text)
            .group("SSH Tunnel")
            .show_if(ShowIf::eq_str("use_ssh", "true")),
    ];
    fields.extend(ssh_auth_fields(
        "ssh_",
        "SSH Tunnel",
        Some(ShowIf::eq_str("use_ssh", "true")),
    ));
    fields
}
