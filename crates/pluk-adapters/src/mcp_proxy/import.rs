//! Adding servers read from another client's config, through the same config
//! the settings form sends. Tools it turned off wait under [`PENDING_OFF_KEY`]
//! until discovery finds them.

use serde_json::{Value, json};

use pluk_core::mcp_import::{Connection, DraftRow, ServerDraft};
use pluk_store::{
    Config, IntegrationUpdate, QueryPolicy, Store, StoreError, ToolPolicy, parse_query_policy,
    serialize_query_policy,
};

use super::local;
use super::transport::HEADERS_KEY;

/// Tool names to switch off once discovery finds them, beside `tools` in the
/// policy blob, whose other writers keep sibling keys as they are.
pub const PENDING_OFF_KEY: &str = "pendingToolsOff";

pub fn config_for(draft: &ServerDraft) -> Config {
    let mut config = Config::new();
    match draft.connection {
        Connection::Remote => {
            config.insert(local::CONNECTION_KEY.into(), json!(local::REMOTE));
            if let Some(url) = &draft.url {
                config.insert("url".into(), json!(url));
            }
            config.insert(HEADERS_KEY.into(), rows(&draft.headers));
        }
        Connection::Local => {
            config.insert(local::CONNECTION_KEY.into(), json!(local::LOCAL));
            if let Some(command) = &draft.command {
                config.insert(local::COMMAND_KEY.into(), json!(command));
            }
            config.insert(local::ARGS_KEY.into(), json!(draft.args));
            if let Some(cwd) = &draft.cwd {
                config.insert(local::CWD_KEY.into(), json!(cwd));
            }
            config.insert(local::ENV_KEY.into(), rows(&draft.env));
        }
    }
    config
}

/// Every row carries its own secret flag, so no field default decides it.
fn rows(rows: &[DraftRow]) -> Value {
    Value::Array(
        rows.iter()
            .map(|row| json!({ "name": row.name, "value": row.value, "secret": row.secret }))
            .collect(),
    )
}

pub fn policy_with_pending_off(names: &[String]) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    let mut policy = QueryPolicy::default();
    set_pending(&mut policy, names.to_vec());
    Some(serialize_query_policy(&policy))
}

pub fn pending_off(policy: Option<&QueryPolicy>) -> Vec<String> {
    policy
        .and_then(|policy| policy.extra.get(PENDING_OFF_KEY))
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn set_pending(policy: &mut QueryPolicy, names: Vec<String>) {
    if names.is_empty() {
        policy.extra.remove(PENDING_OFF_KEY);
    } else {
        policy.extra.insert(PENDING_OFF_KEY.into(), json!(names));
    }
}

/// Switch off the waiting tools that `found` names. Reads the stored policy
/// afresh, so a toggle written since the caller loaded the integration stays.
pub(super) fn apply_pending_off(
    store: &Store,
    integration_id: &str,
    found: &[String],
) -> Result<(), StoreError> {
    let Some(stored) = store.integration_by_id(integration_id)? else {
        return Ok(());
    };
    let Some(mut policy) = parse_query_policy(stored.query_policy.as_deref()) else {
        return Ok(());
    };
    let (now, still): (Vec<String>, Vec<String>) = pending_off(Some(&policy))
        .into_iter()
        .partition(|name| found.contains(name));
    if now.is_empty() {
        return Ok(());
    }
    for name in now {
        policy
            .tools
            .entry(name)
            .and_modify(|tool| tool.enabled = false)
            .or_insert_with(|| ToolPolicy {
                enabled: false,
                settings: Default::default(),
                extra: Default::default(),
            });
    }
    set_pending(&mut policy, still);
    let update = IntegrationUpdate {
        query_policy: Some(Some(serialize_query_policy(&policy))),
        ..Default::default()
    };
    store
        .update_integration(integration_id, &update)
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pluk_store::IntegrationInput;

    fn draft(connection: Connection) -> ServerDraft {
        ServerDraft {
            name: "server".into(),
            connection,
            url: None,
            headers: Vec::new(),
            command: None,
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
            disabled_tools: Vec::new(),
            not_imported: Vec::new(),
            via_mcp_remote: false,
            turned_off: false,
            sse: false,
        }
    }

    fn row(name: &str, value: &str, secret: bool) -> DraftRow {
        DraftRow {
            name: name.into(),
            value: value.into(),
            secret,
        }
    }

    #[test]
    fn a_local_draft_becomes_the_forms_local_config() {
        let local = ServerDraft {
            command: Some("node".into()),
            args: vec!["index.js".into()],
            env: vec![row("TOKEN", "t", true), row("REGION", "eu", false)],
            ..draft(Connection::Local)
        };
        assert_eq!(
            Value::Object(config_for(&local)),
            json!({
                "connection": "local",
                "command": "node",
                "args": ["index.js"],
                "env": [
                    {"name": "TOKEN", "value": "t", "secret": true},
                    {"name": "REGION", "value": "eu", "secret": false},
                ],
            })
        );
    }

    #[test]
    fn a_remote_draft_keeps_each_headers_own_secret_flag() {
        let remote = ServerDraft {
            url: Some("https://x.example/mcp".into()),
            headers: vec![row("X-Team", "t", false)],
            ..draft(Connection::Remote)
        };
        assert_eq!(
            Value::Object(config_for(&remote)),
            json!({
                "connection": "remote",
                "url": "https://x.example/mcp",
                "headers": [{"name": "X-Team", "value": "t", "secret": false}],
            })
        );
    }

    #[test]
    fn found_tools_switch_off_and_the_rest_keep_waiting() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("pluk.db")).expect("open");
        let mut input = IntegrationInput::new("Docs", super::super::ADAPTER_ID);
        input.query_policy = policy_with_pending_off(&["search".into(), "later".into()]);
        let created = store.create_integration(&input).expect("create");

        apply_pending_off(&store, &created.id, &["other".into()]).expect("apply");
        let untouched = store.integration_by_id(&created.id).unwrap().unwrap();
        assert_eq!(untouched.query_policy, input.query_policy);

        apply_pending_off(&store, &created.id, &["search".into(), "other".into()]).expect("apply");
        let stored = store.integration_by_id(&created.id).unwrap().unwrap();
        let policy = parse_query_policy(stored.query_policy.as_deref()).expect("policy");
        assert!(!policy.tools["search"].enabled);
        assert!(!policy.tools.contains_key("other"));
        assert_eq!(pending_off(Some(&policy)), ["later"]);
    }
}
