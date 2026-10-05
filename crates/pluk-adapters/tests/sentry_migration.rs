use std::sync::Arc;

use pluk_adapters::{default_registry, mcp_proxy::local, sentry_migration, sql::SqlCancelRegistry};
use pluk_store::{
    DiscoveredTool, Environment, Group, GroupInput, GroupMember, Integration, IntegrationInput,
    LogDraft, LogRange, LogScope, SecretKind, SecretWrite, Store,
};
use serde_json::json;

fn temp_store() -> (tempfile::TempDir, Arc<Store>) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.oga_tmp");
    std::fs::create_dir_all(&root).unwrap();
    let dir = tempfile::tempdir_in(root).unwrap();
    let store = Arc::new(Store::open(&dir.path().join("pluk.db")).unwrap());
    (dir, store)
}

fn saved_sentry(store: &Store, base_url: Option<&str>, token: Option<&str>) -> Integration {
    let mut input = IntegrationInput::new("Sentry production", "sentry");
    input.environment = Some(Environment::Production);
    input.config = serde_json::from_value(json!({
        "org_slug": "acme",
        "project_slug": "checkout",
    }))
    .unwrap();
    if let Some(url) = base_url {
        input.config.insert("base_url".into(), json!(url));
    }
    if let Some(token) = token {
        input.config.insert("auth_token".into(), json!(token));
    }
    input.query_policy = Some(
        json!({"tools": {"update_issue": {"enabled": true}}, "approvals": {"allow": ["*"]}})
            .to_string(),
    );
    store.create_integration(&input).unwrap()
}

fn saved_group(store: &Store, id: &str) -> Group {
    store
        .create_group(&GroupInput {
            name: "Production tools".into(),
            members: vec![GroupMember {
                id: id.into(),
                overrides: Default::default(),
                tools: None,
            }],
            ..Default::default()
        })
        .unwrap()
}

fn old_approvals(store: &Store, id: &str) {
    store
        .replace_proxy_tools(
            id,
            &[DiscoveredTool {
                name: "update_issue".into(),
                description: String::new(),
                schema_json: "{}".into(),
                annotations_json: None,
                content_hash: "old-definition".into(),
            }],
        )
        .unwrap();
    store
        .approve_proxy_tools(id, &["update_issue".into()])
        .unwrap();
    store.approve_launch(id, "old-launch").unwrap();
}

#[tokio::test]
async fn self_hosted_sentry_keeps_its_identity_and_secret_but_needs_new_approvals() {
    let (_dir, store) = temp_store();
    let before = saved_sentry(
        &store,
        Some("https://sentry.internal.example:8443/"),
        Some("sntrys_saved_secret"),
    );
    let group = saved_group(&store, &before.id);
    let log_id = store
        .create_log_entry(LogDraft::new(&before.id, &before.name, "list_issues"))
        .unwrap();
    old_approvals(&store, &before.id);

    sentry_migration::run(&store).unwrap();

    let converted = store.integration_by_token(&before.token).unwrap().unwrap();
    assert_eq!(converted.r#type, "mcp");
    assert_eq!(converted.id, before.id);
    assert_eq!(converted.name, before.name);
    assert_eq!(converted.created_at, before.created_at);
    assert_eq!(converted.environment, before.environment);
    assert!(local::is_local(&converted));
    assert_eq!(converted.config["command"], "npx");
    assert_eq!(
        converted.config["args"],
        json!(["-y", "@sentry/mcp-server@latest"])
    );
    assert_eq!(converted.query_policy, None);
    assert!(
        !serde_json::to_string(&converted.config)
            .unwrap()
            .contains("sntrys_saved_secret")
    );
    for key in ["auth_token", "base_url", "org_slug", "project_slug"] {
        assert!(!converted.config.contains_key(key));
    }
    let secrets = store.list_proxy_secrets(&before.id).unwrap();
    assert_eq!(secrets.len(), 1);
    assert_eq!(secrets[0].kind, SecretKind::Env);
    assert_eq!(secrets[0].name, "SENTRY_ACCESS_TOKEN");
    assert_eq!(secrets[0].value, "sntrys_saved_secret");
    let spec = local::launch_spec(&store, &converted).await.unwrap();
    let env: Vec<_> = spec
        .env
        .iter()
        .map(|(name, value)| (name.as_str(), value.expose()))
        .collect();
    assert_eq!(
        env,
        [
            ("SENTRY_ACCESS_TOKEN", "sntrys_saved_secret"),
            ("SENTRY_HOST", "sentry.internal.example:8443"),
        ]
    );
    assert_eq!(store.approved_launch(&before.id).unwrap(), None);
    let refused = local::client_for(&store, &converted, spec)
        .await
        .err()
        .unwrap();
    assert!(refused.has_code(local::LAUNCH_NOT_APPROVED_CODE));
    assert!(store.list_proxy_tools(&before.id).unwrap().is_empty());
    let resolved = store.resolve_members(&group).unwrap();
    assert_eq!(resolved[0].integration, converted);
    let logs = store
        .read_log_page(
            &LogScope::Connection(before.id.clone()),
            LogRange::All,
            None,
        )
        .unwrap();
    assert_eq!(logs.entries[0].id, log_id);

    let registry = default_registry(store.clone(), Arc::new(SqlCancelRegistry::default())).unwrap();
    let adapter = registry.get("mcp").unwrap();
    assert!(adapter.tool_specs_for(&converted).is_empty());
    store
        .approve_launch(&before.id, "user-approved-after-conversion")
        .unwrap();
    sentry_migration::run(&store).unwrap();
    assert_eq!(
        store.integration_by_id(&before.id).unwrap().unwrap(),
        converted
    );
    assert_eq!(store.list_proxy_secrets(&before.id).unwrap(), secrets);
    assert_eq!(
        store.approved_launch(&before.id).unwrap().as_deref(),
        Some("user-approved-after-conversion")
    );
    assert_eq!(store.group_by_id(&group.id).unwrap().unwrap(), group);
}

#[test]
fn sentry_cloud_and_missing_tokens_convert_without_a_host_override() {
    let (_dir, store) = temp_store();
    let cases = [
        saved_sentry(&store, Some("https://sentry.io/"), Some("cloud-secret")),
        saved_sentry(&store, None, None),
        saved_sentry(&store, None, Some("")),
    ];
    store
        .write_proxy_secrets(
            &cases[1].id,
            &[SecretWrite::Set {
                kind: SecretKind::Env,
                name: "SENTRY_ACCESS_TOKEN".into(),
                value: "stale-secret".into(),
            }],
        )
        .unwrap();

    sentry_migration::run(&store).unwrap();

    for before in &cases {
        let converted = store.integration_by_id(&before.id).unwrap().unwrap();
        assert_eq!(converted.r#type, "mcp");
        assert!(local::is_local(&converted));
        assert_eq!(
            converted.config["env"],
            json!([
                {"name": "SENTRY_ACCESS_TOKEN", "value": "", "secret": true},
            ])
        );
        let secrets = store.list_proxy_secrets(&before.id).unwrap();
        if before.config.get("auth_token") == Some(&json!("cloud-secret")) {
            assert_eq!(secrets[0].value, "cloud-secret");
        } else {
            assert!(secrets.is_empty());
        }
        assert_eq!(store.approved_launch(&before.id).unwrap(), None);
    }
    let converted = store.list_integrations().unwrap();
    sentry_migration::run(&store).unwrap();
    assert_eq!(store.list_integrations().unwrap(), converted);
}

#[test]
fn a_failed_secret_write_keeps_the_token_and_does_not_block_other_conversions() {
    let (dir, store) = temp_store();
    let path = dir.path().join("pluk.db");
    let failed = saved_sentry(&store, None, Some("retry-secret"));
    let invalid = saved_sentry(&store, Some("not a URL"), Some("invalid-secret"));
    let good = saved_sentry(&store, None, Some("good-secret"));
    let untouched = store
        .create_integration(&IntegrationInput::new("Other server", "mcp"))
        .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(&format!(
        "CREATE TRIGGER refuse_secret BEFORE INSERT ON proxy_secrets
         WHEN NEW.integration_id = '{}' BEGIN SELECT RAISE(FAIL, 'secret write refused'); END;",
        failed.id,
    ))
    .unwrap();

    sentry_migration::run(&store).unwrap();

    assert_eq!(
        store.integration_by_id(&failed.id).unwrap().unwrap(),
        failed
    );
    assert_eq!(
        store.integration_by_id(&invalid.id).unwrap().unwrap(),
        invalid
    );
    assert_eq!(
        store.integration_by_id(&untouched.id).unwrap().unwrap(),
        untouched
    );
    assert_eq!(
        store.integration_by_id(&good.id).unwrap().unwrap().r#type,
        "mcp"
    );
    db.execute_batch("DROP TRIGGER refuse_secret;").unwrap();
    sentry_migration::run(&store).unwrap();
    assert_eq!(
        store.integration_by_id(&failed.id).unwrap().unwrap().r#type,
        "mcp"
    );
    assert_eq!(
        store.list_proxy_secrets(&failed.id).unwrap()[0].value,
        "retry-secret"
    );
}
