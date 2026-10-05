use pluk_core::mcp_import::{Connection, DraftRow, ServerDraft};
use pluk_store::{Integration, IntegrationUpdate, SecretKind, SecretWrite, Store};
use serde_json::Value;

use crate::mcp_proxy::{ADAPTER_ID, import};

pub fn run(store: &Store) -> pluk_store::Result<()> {
    for integration in store.list_integrations()? {
        if integration.r#type != "sentry" {
            continue;
        }
        if let Err(error) = convert(store, &integration) {
            eprintln!("Sentry conversion failed for {}: {error}", integration.id);
        }
    }
    Ok(())
}

fn convert(store: &Store, integration: &Integration) -> Result<(), String> {
    let base_url = integration
        .config
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or("https://sentry.io");
    let url = reqwest::Url::parse(base_url)
        .map_err(|_| "Sentry base_url is not a valid URL.".to_string())?;
    let host = url.host_str().ok_or("Sentry base_url has no host.")?;
    let mut env = vec![DraftRow {
        name: "SENTRY_ACCESS_TOKEN".into(),
        value: String::new(),
        secret: true,
    }];
    if host != "sentry.io" {
        let host = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        };
        env.push(DraftRow {
            name: "SENTRY_HOST".into(),
            value: host,
            secret: false,
        });
    }
    let draft = ServerDraft {
        name: integration.name.clone(),
        connection: Connection::Local,
        url: None,
        headers: Vec::new(),
        command: Some("npx".into()),
        args: vec!["-y".into(), "@sentry/mcp-server@latest".into()],
        env,
        cwd: None,
        disabled_tools: Vec::new(),
        not_imported: Vec::new(),
        via_mcp_remote: false,
        turned_off: false,
        sse: false,
    };
    let update = IntegrationUpdate {
        r#type: Some(ADAPTER_ID.into()),
        config: Some(import::config_for(&draft)),
        query_policy: Some(None),
        ..Default::default()
    };
    let token = integration
        .config
        .get("auth_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty());
    let secret = match token {
        Some(token) => SecretWrite::Set {
            kind: SecretKind::Env,
            name: "SENTRY_ACCESS_TOKEN".into(),
            value: token.to_string(),
        },
        None => SecretWrite::Clear {
            kind: SecretKind::Env,
            name: "SENTRY_ACCESS_TOKEN".into(),
        },
    };

    // The old config keeps the token until its secret write succeeds.
    store
        .write_proxy_secrets(&integration.id, &[secret])
        .map_err(|error| error.to_string())?;
    store
        .clear_launch_approval(&integration.id)
        .map_err(|error| error.to_string())?;
    store
        .delete_proxy_tools(&integration.id)
        .map_err(|error| error.to_string())?;
    store
        .update_integration(&integration.id, &update)
        .map_err(|error| error.to_string())?;
    Ok(())
}
