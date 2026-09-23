//! A local MCP server: a command, its args, a working folder and environment
//! variables, which Pluk starts and talks to over stdio.
//!
//! Pluk runs the command with the user's full access, so it runs only in the
//! exact form the user approved: the resolved program, every arg, the folder,
//! and each variable's name and value, hashed. Any change to them is a launch
//! nobody approved, and nothing starts it until the user approves it again.
//! Approving is the desktop window's alone; see `approve`.
//!
//! An integration is local when its config says so under [`CONNECTION_KEY`].
//! Everything else about it being remote — the address, the probe, sign-in —
//! is skipped.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use pluk_core::shell_env;
use pluk_store::{Config, Integration, SecretKind, Store};

use crate::error::AdapterError;
use crate::key_value::{self, ConfigProblem};

use super::catalog;
use super::client::{self, McpProxyClient};
use super::transport::{LaunchSpec, RowText, Secret};

/// Which way the integration reaches its server: [`REMOTE`] or [`LOCAL`].
/// Absent means remote, which is every integration saved before local
/// servers existed.
pub const CONNECTION_KEY: &str = "connection";
pub const REMOTE: &str = "remote";
pub const LOCAL: &str = "local";

pub const COMMAND_KEY: &str = "command";
/// A list of strings, passed as they are, one arg each.
pub const ARGS_KEY: &str = "args";
pub const CWD_KEY: &str = "cwd";
/// Key/value rows, their secret values saved as [`SecretKind::Env`].
pub const ENV_KEY: &str = "env";

/// The launch as configured now is not one the user approved.
pub const LAUNCH_NOT_APPROVED_CODE: &str = "MCP_PROXY_LAUNCH_NOT_APPROVED";

pub(super) const LAUNCH_NOT_APPROVED: &str = "Approve this command in Pluk to start it.";
const LAUNCH_CHANGED: &str = "This command changed since you reviewed it. Review it again.";
const NO_COMMAND: &str = "Add the command that starts this server.";
const RELATIVE_COMMAND: &str =
    "Use the program's name, such as node, or its full path starting with /.";
const RELATIVE_CWD: &str = "Use a full path for the working folder, such as ~/code/server.";
const MISSING_CWD: &str = "The working folder does not exist.";
const ARGS_NOT_LIST: &str = "Arguments have to be a list of text values.";

/// Variables that make a program load code it was not asked to run.
const CODE_LOADING: &[&str] = &["NODE_OPTIONS", "LD_PRELOAD", "PYTHONPATH", "PYTHONSTARTUP"];
const CODE_LOADING_PREFIX: &str = "DYLD_";

pub fn is_local(conn: &Integration) -> bool {
    is_local_config(&conn.config)
}

pub fn is_local_config(config: &Config) -> bool {
    config.get(CONNECTION_KEY).and_then(Value::as_str) == Some(LOCAL)
}

/// The launch the integration's config describes now, with the program
/// resolved and secret values read from the store.
pub async fn launch_spec(store: &Store, conn: &Integration) -> Result<LaunchSpec, AdapterError> {
    let command = text(&conn.config, COMMAND_KEY).ok_or_else(|| AdapterError::new(NO_COMMAND))?;
    let path = shell_env::login_path().await;
    let program = shell_env::resolve_program(&command, path)
        .map_err(|error| AdapterError::new(not_found(&command, &error)))?;
    let cwd = working_folder(&conn.config).map_err(AdapterError::new)?;
    if !cwd.is_dir() {
        return Err(AdapterError::new(MISSING_CWD));
    }
    Ok(LaunchSpec {
        program,
        args: args(&conn.config).map_err(AdapterError::new)?,
        cwd,
        env: env(store, conn)?,
        path: path.to_os_string(),
    })
}

/// A client for the local server, refused until the user approved this
/// exact launch.
pub async fn client_for(
    store: &Store,
    conn: &Integration,
    spec: LaunchSpec,
) -> Result<McpProxyClient, AdapterError> {
    let approved = store
        .approved_launch(&conn.id)
        .map_err(|error| AdapterError::new(error.to_string()))?;
    if approved.as_deref() != Some(spec.launch_hash().as_str()) {
        return Err(AdapterError::new(LAUNCH_NOT_APPROVED).with_code(LAUNCH_NOT_APPROVED_CODE));
    }
    Ok(McpProxyClient::stdio(&conn.id, spec))
}

/// Everything the user is shown before approving a launch. Secret values
/// never appear: a variable is its name and whether it is secret.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchPreview {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: Vec<EnvPreview>,
    /// One line per variable that can make the program load other code.
    pub warnings: Vec<String>,
    /// What approving this preview approves.
    pub launch_hash: String,
    pub approved: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvPreview {
    pub name: String,
    pub secret: bool,
}

pub async fn preview(store: &Store, conn: &Integration) -> Result<LaunchPreview, AdapterError> {
    let spec = launch_spec(store, conn).await?;
    let launch_hash = spec.launch_hash();
    let approved = store
        .approved_launch(&conn.id)
        .map_err(|error| AdapterError::new(error.to_string()))?
        .is_some_and(|approved| approved == launch_hash);
    Ok(LaunchPreview {
        program: spec.program.to_string_lossy().into_owned(),
        args: spec.args.clone(),
        cwd: spec.cwd.to_string_lossy().into_owned(),
        env: spec
            .env
            .iter()
            .map(|(name, value)| EnvPreview {
                name: name.clone(),
                secret: matches!(value, RowText::Secret(_)),
            })
            .collect(),
        warnings: spec
            .env
            .iter()
            .filter(|(name, _)| loads_code(name))
            .map(|(name, _)| format!("{name} can make the server load other code."))
            .collect(),
        launch_hash,
        approved,
    })
}

/// Approve the launch the user was shown as `launch_hash`. Refused when the
/// launch changed since, so what runs is always what they saw.
///
/// Only the desktop window may call this. An adapter route would be open to
/// every process on this machine.
pub async fn approve(
    store: &Store,
    conn: &Integration,
    launch_hash: &str,
) -> Result<(), AdapterError> {
    let spec = launch_spec(store, conn).await?;
    if spec.launch_hash() != launch_hash {
        return Err(AdapterError::new(LAUNCH_CHANGED));
    }
    store
        .approve_launch(&conn.id, launch_hash)
        .map_err(|error| AdapterError::new(error.to_string()))
}

/// Stop the server if it runs, clear a stop or the crash limit, and start it
/// again by asking it for its tools.
pub async fn restart(store: &Store, conn: &Integration) -> Result<(), AdapterError> {
    client::restart(&conn.id).await;
    catalog::discover(store, conn).await.map(|_| ())
}

/// A local config the save would refuse. Whether the program exists is not
/// checked here: that depends on the machine, and the preview shows it.
pub fn check_config(config: &Config) -> Result<(), ConfigProblem> {
    let problem = |field: &str, message: &str| ConfigProblem {
        field: field.to_string(),
        row: None,
        message: message.to_string(),
    };
    match text(config, COMMAND_KEY) {
        None => return Err(problem(COMMAND_KEY, NO_COMMAND)),
        Some(command) if is_relative_path(&command) => {
            return Err(problem(COMMAND_KEY, RELATIVE_COMMAND));
        }
        Some(_) => {}
    }
    args(config).map_err(|message| problem(ARGS_KEY, message))?;
    working_folder(config).map_err(|message| problem(CWD_KEY, message))?;
    for (index, row) in key_value::rows(config, ENV_KEY).iter().enumerate() {
        if row.name.contains(['=', '\0']) {
            return Err(ConfigProblem::at_row(
                ENV_KEY,
                index,
                format!("{} cannot be a variable name. Remove the = sign.", row.name),
            ));
        }
    }
    Ok(())
}

fn text(config: &Config, key: &str) -> Option<String> {
    config
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn args(config: &Config) -> Result<Vec<String>, &'static str> {
    match config.get(ARGS_KEY) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_string).ok_or(ARGS_NOT_LIST))
            .collect(),
        Some(_) => Err(ARGS_NOT_LIST),
    }
}

/// The folder the server starts in: the one configured, `~` expanded, or
/// the user's home folder. Never Pluk's own, which is `/` from Finder.
fn working_folder(config: &Config) -> Result<PathBuf, &'static str> {
    let folder = match text(config, CWD_KEY) {
        Some(folder) => shell_env::expand_home(&folder),
        None => shell_env::expand_home("~"),
    };
    if folder.is_absolute() {
        Ok(folder)
    } else {
        Err(RELATIVE_CWD)
    }
}

/// The user's variables in order, secret values read from the store. A
/// secret row with nothing saved is left out.
fn env(store: &Store, conn: &Integration) -> Result<Vec<(String, RowText)>, AdapterError> {
    let rows = key_value::rows(&conn.config, ENV_KEY);
    let saved = if rows.iter().any(|row| row.secret) {
        store
            .list_proxy_secrets(&conn.id)
            .map_err(|error| AdapterError::new(error.to_string()))?
    } else {
        Vec::new()
    };
    Ok(rows
        .into_iter()
        .filter(|row| !row.name.is_empty())
        .filter_map(|row| {
            let value = if row.secret {
                let secret = saved
                    .iter()
                    .find(|secret| secret.kind == SecretKind::Env && secret.name == row.name)?;
                RowText::Secret(Secret::new(secret.value.clone()))
            } else {
                RowText::Plain(row.value)
            };
            Some((row.name, value))
        })
        .collect())
}

fn loads_code(name: &str) -> bool {
    CODE_LOADING.contains(&name) || name.starts_with(CODE_LOADING_PREFIX)
}

fn is_relative_path(command: &str) -> bool {
    command.contains('/') && !command.starts_with('/') && !command.starts_with("~/")
}

/// Why the program could not be found, naming only its file name.
fn not_found(command: &str, error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::InvalidInput {
        return RELATIVE_COMMAND.to_string();
    }
    let name = Path::new(command)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| command.to_string());
    format!("Pluk could not find {name}. Check that it is installed, or use its full path.")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use pluk_store::SecretWrite;

    use super::*;
    use crate::adapter::Adapter;
    use crate::mcp_proxy::McpProxyAdapter;
    use crate::mcp_proxy::client::tests::{STUB, gone, recorded};
    use crate::mcp_proxy::tests::integration;

    fn store() -> (tempfile::TempDir, Arc<Store>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("pluk.db")).expect("open");
        (dir, Arc::new(store))
    }

    fn local(id: &str, dir: &Path, args: &[&str]) -> Integration {
        let script = dir.join("server.sh");
        std::fs::write(&script, STUB).unwrap();
        let mut all = vec![script.to_string_lossy().into_owned()];
        all.extend(args.iter().map(|arg| arg.to_string()));
        integration(
            id,
            json!({
                "connection": "local",
                "command": "/bin/sh",
                "args": all,
                "cwd": dir,
                "env": [
                    {"name": "STUB_STATE", "value": dir, "secret": false},
                    {"name": "STUB_TOKEN", "secret": true},
                ],
            }),
        )
    }

    fn save_env(store: &Store, id: &str, name: &str, value: &str) {
        store
            .write_proxy_secrets(
                id,
                &[SecretWrite::Set {
                    kind: SecretKind::Env,
                    name: name.to_string(),
                    value: value.to_string(),
                }],
            )
            .expect("save env");
    }

    #[tokio::test]
    async fn nothing_starts_until_the_exact_launch_is_approved() {
        let dir = tempfile::tempdir().unwrap();
        let (_db, store) = store();
        let adapter = McpProxyAdapter::new(store.clone());
        let conn = local("local-approval", dir.path(), &[]);
        save_env(&store, &conn.id, "STUB_TOKEN", "env-secret-1");

        let refused = adapter
            .test_connection(&conn)
            .await
            .expect_err("unapproved");
        assert!(refused.has_code(LAUNCH_NOT_APPROVED_CODE), "{refused:?}");
        assert_eq!(
            adapter.humanize_error(&refused).as_deref(),
            Some(LAUNCH_NOT_APPROVED)
        );
        assert!(!dir.path().join("pid").exists(), "it started unapproved");

        let shown = preview(&store, &conn).await.expect("preview");
        assert!(!shown.approved);
        approve(&store, &conn, &shown.launch_hash)
            .await
            .expect("approve");
        adapter.test_connection(&conn).await.expect("approved");
        let first = recorded(dir.path(), "pid").await;

        let changed = local("local-approval", dir.path(), &["--verbose"]);
        let refused = adapter
            .test_connection(&changed)
            .await
            .expect_err("a changed launch");
        assert!(refused.has_code(LAUNCH_NOT_APPROVED_CODE), "{refused:?}");
        assert!(gone(first).await, "the old launch kept running");
        let stale = approve(&store, &changed, &shown.launch_hash)
            .await
            .expect_err("approving what was shown before the change");
        assert_eq!(stale.message, LAUNCH_CHANGED);

        client::shutdown(&conn.id);
    }

    #[tokio::test]
    async fn every_part_of_a_launch_is_bound_by_its_hash() {
        let dir = tempfile::tempdir().unwrap();
        let (_db, store) = store();
        let conn = local("local-hash", dir.path(), &[]);
        save_env(&store, &conn.id, "STUB_TOKEN", "env-secret-1");
        let hash = || async { launch_spec(&store, &conn).await.unwrap().launch_hash() };

        let before = hash().await;
        assert_eq!(hash().await, before);
        save_env(&store, &conn.id, "STUB_TOKEN", "env-secret-2");
        let after_secret = hash().await;
        assert_ne!(after_secret, before, "a new secret value");

        let mut moved = conn.clone();
        moved.config.insert(CWD_KEY.to_string(), json!("~"));
        let moved_hash = launch_spec(&store, &moved).await.unwrap().launch_hash();
        assert_ne!(moved_hash, after_secret, "another folder");
    }

    #[tokio::test]
    async fn the_preview_shows_names_and_warnings_and_never_a_secret() {
        let dir = tempfile::tempdir().unwrap();
        let (_db, store) = store();
        let mut conn = local("local-preview", dir.path(), &["--token", "arg-token-1"]);
        conn.config.insert(
            ENV_KEY.to_string(),
            json!([
                {"name": "STUB_TOKEN", "secret": true},
                {"name": "NODE_OPTIONS", "value": "--require ./x.js", "secret": false},
                {"name": "DYLD_INSERT_LIBRARIES", "secret": true},
            ]),
        );
        save_env(&store, &conn.id, "STUB_TOKEN", "env-secret-1");
        save_env(&store, &conn.id, "DYLD_INSERT_LIBRARIES", "/tmp/x.dylib");

        let shown = preview(&store, &conn).await.expect("preview");
        assert_eq!(shown.program, "/bin/sh");
        assert_eq!(shown.args[1..], ["--token", "arg-token-1"]);
        let env: Vec<(&str, bool)> = shown
            .env
            .iter()
            .map(|row| (row.name.as_str(), row.secret))
            .collect();
        assert_eq!(
            env,
            [
                ("STUB_TOKEN", true),
                ("NODE_OPTIONS", false),
                ("DYLD_INSERT_LIBRARIES", true),
            ]
        );
        assert_eq!(
            shown.warnings,
            [
                "NODE_OPTIONS can make the server load other code.",
                "DYLD_INSERT_LIBRARIES can make the server load other code.",
            ]
        );
        let sent = serde_json::to_string(&shown).unwrap();
        assert!(!sent.contains("env-secret-1"), "{sent}");
        assert!(!sent.contains("x.dylib"), "{sent}");
    }

    #[tokio::test]
    async fn a_missing_program_is_named_by_its_file_name_alone() {
        let (_db, store) = store();
        let conn = integration(
            "local-missing",
            json!({"connection": "local", "command": "/opt/secret-path/no-such-server"}),
        );
        let error = launch_spec(&store, &conn).await.expect_err("missing");
        assert_eq!(
            error.message,
            "Pluk could not find no-such-server. Check that it is installed, or use its full path."
        );
    }

    #[test]
    fn a_launch_pluk_could_not_run_is_refused_at_its_field() {
        let refusal = |config: serde_json::Value| {
            let serde_json::Value::Object(config) = config else {
                unreachable!()
            };
            check_config(&config).expect_err("refused")
        };
        let problem = refusal(json!({"connection": "local"}));
        assert_eq!(
            (problem.field.as_str(), problem.message.as_str()),
            (COMMAND_KEY, NO_COMMAND)
        );
        assert_eq!(
            refusal(json!({"connection": "local", "command": "./server"})).message,
            RELATIVE_COMMAND
        );
        assert_eq!(
            refusal(json!({"connection": "local", "command": "node", "args": "index.js"})).field,
            ARGS_KEY
        );
        assert_eq!(
            refusal(json!({"connection": "local", "command": "node", "cwd": "code/server"}))
                .message,
            RELATIVE_CWD
        );
        let bad_name = refusal(json!({
            "connection": "local",
            "command": "node",
            "env": [{"name": "A", "value": "1"}, {"name": "B=C", "value": "2"}],
        }));
        assert_eq!(bad_name.row, Some(1));

        let serde_json::Value::Object(fine) = json!({
            "connection": "local",
            "command": "~/bin/server",
            "args": ["--port", "0"],
            "cwd": "~",
        }) else {
            unreachable!()
        };
        assert!(check_config(&fine).is_ok());
    }
}
