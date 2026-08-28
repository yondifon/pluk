use std::collections::HashMap;
use std::sync::Arc;
use serde_json::{Map, Value};
use pluk_store::{Integration, Store};

use crate::gate::{CallTarget, GateMeta, GateOpts, Outcome, RunOutcome, ToolResult, err, ok, run_gated};
use crate::instructions::{build_instructions, InstructionParts};
use crate::projection::{FieldMap, Preset, apply_only};
use crate::tool_host::{object_schema, BoxFuture, ToolHost, ToolRegistration};
use crate::tool_spec::ToolSpec;

use super::client::{run_command, open_forward, list_forwards, close_forward, MAX_COMMAND_TIMEOUT_S};
use super::error::humanize_ssh_error;
use super::policy::evaluate_command;

pub const SSH_AGENT_HINT: &str = "Use this for SSH access to the remote host — run shell commands to inspect logs, processes, disk and memory, and Docker/systemd services for debugging and ops, and open local port forwards (ssh -L) so a remote service like a database or web UI is reachable at localhost on this machine. Every command runs as the SSH user and must be confirmed before it runs.";

const DEBUG_SNAPSHOT: &[(&str, &str)] = &[
    ("Host", "uname -a"),
    ("Uptime / load", "uptime"),
    ("Disk", "df -h"),
    ("Memory", "free -m"),
    ("Processes", "ps aux"),
    ("Logged in", "who"),
    ("Containers", "docker ps"),
];

const MAX_BATCH: usize = 50;
fn saved_commands_map() -> FieldMap {
    FieldMap::new(&["name","command","working_dir"], &["name","command"])
        .with_preset("location", Preset::paths(&["working_dir"]))
}

pub fn ssh_instructions(conn: &Integration) -> String {
    build_instructions(
        &conn.name,
        conn.environment,
        InstructionParts {
            kind: "SSH".to_string(),
            access: "Run shell commands on the remote host as the connecting SSH user. Commands run unmodified and are recorded in the activity log.".to_string(),
            policy: Some("Unrestricted — there is no allowlist; every command must be confirmed in your client before it runs.".to_string()),
            hint: Some(SSH_AGENT_HINT.to_string()),
            start: Some("Start with debug_snapshot for a host overview, or list_saved_commands for curated commands. Use open_forward to reach a remote service (e.g. a database) at localhost on this machine.".to_string()),
        }
    )
}

pub fn ssh_tool_specs() -> Vec<ToolSpec> {
    let opt_in: std::collections::HashSet<&str> = ["run_batch","debug_snapshot","run_saved_command","list_saved_commands","open_forward","list_forwards","close_forward"].into_iter().collect();
    let mk = |name: &str, desc: &str| {
        ToolSpec::new(name, desc, "read").with_default_enabled(!opt_in.contains(name))
    };
    vec![
        mk("run_command", "Run a shell command on the remote host over SSH."),
        mk("run_batch", "Run several shell commands in sequence on the remote host."),
        mk("debug_snapshot", "Capture a quick health snapshot of the remote host."),
        mk("run_saved_command", "Run a pre-selected (saved) command by name."),
        mk("list_saved_commands", "List the saved commands available for this integration."),
        mk("open_forward", "Open a local port forward (ssh -L) to a remote service."),
        mk("list_forwards", "List the open local port forwards for this connection."),
        mk("close_forward", "Close an open local port forward by its id."),
    ]
}

fn quote_dir(dir: &str) -> String {
    format!("'{}'", dir.replace('\'', "'\\''"))
}

fn format_result(stdout: &str, stderr: &str, code: Option<i32>, truncated: bool) -> String {
    let mut parts = vec![format!("exit code: {}", code.map(|c| c.to_string()).unwrap_or_else(|| "unknown".to_string()))];
    if !stdout.trim().is_empty() { parts.push(format!("stdout:\n{}", stdout.trim_end())); }
    if !stderr.trim().is_empty() { parts.push(format!("stderr:\n{}", stderr.trim_end())); }
    if stdout.trim().is_empty() && stderr.trim().is_empty() { parts.push("(no output)".to_string()); }
    if truncated { parts.push("[output truncated at 1 MB]".to_string()); }
    parts.join("\n\n")
}

fn forward_row(f: &super::client::ForwardInfo) -> Value {
    serde_json::json!({ "id": f.id, "local": format!("127.0.0.1:{}", f.local_port), "remote": format!("{}:{}", f.remote_host, f.remote_port) })
}

fn text_of(res: &ToolResult) -> String {
    res.content.first().map(|c| c.text.clone()).unwrap_or_default()
}

pub fn register_ssh_server(host: &mut dyn ToolHost, conn: &Integration, owner_id: &str, store: Arc<Store>) -> Result<(), crate::error::AdapterError> {
    let gate = pluk_policy::tool_gate(conn.query_policy.as_deref());
    let tool_defaults: HashMap<String,bool> = ssh_tool_specs().into_iter().map(|t| (t.name, t.default_enabled)).collect();
    let on = |name: &str| gate.enabled(name, *tool_defaults.get(name).unwrap_or(&true));

    // run_command tool
    if on("run_command") {
        let mut props = Map::new();
        props.insert("command".into(), serde_json::json!({"type":"string","description":"The command to run, e.g. `docker compose ps`"}));
        props.insert("working_dir".into(), serde_json::json!({"type":"string","description":"Directory to run in (e.g. /srv/app). Optional."}));
        props.insert("timeout".into(), serde_json::json!({"type":"number","description": format!("Max seconds to wait before aborting the command (default 60).")}));
        let schema = object_schema(props, &["command"]);
        let conn_c = conn.clone();
        let owner_c = owner_id.to_string();
        let store_c = store.clone();
        host.register_tool(
            ToolRegistration { name: "run_command".into(), description: "Run a shell command on the remote host over SSH. The command runs unmodified as the connecting user — confirm before running, as it can change or destroy remote state. Commands time out after 60 seconds by default; pass `timeout` (up to 600 seconds) for long-running commands.".into(), input_schema: schema, annotations: {
                let mut m=Map::new(); m.insert("readOnlyHint".into(), Value::Bool(false)); m.insert("destructiveHint".into(), Value::Bool(true)); m.insert("openWorldHint".into(), Value::Bool(true)); m
            } },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let conn = conn_c.clone();
                let owner = owner_c.clone();
                let store = store_c.clone();
                Box::pin(async move {
                    let obj = args.as_object().cloned().unwrap_or_default();
                    let command = obj.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let working_dir = obj.get("working_dir").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let timeout = obj.get("timeout").and_then(|v| v.as_u64());
                    if let Some(t) = timeout && t>MAX_COMMAND_TIMEOUT_S { return err(format!("timeout must be <= {}", MAX_COMMAND_TIMEOUT_S)); }
                    let trimmed = command.trim().to_string();
                    if trimmed.is_empty() { return err("Error: empty command."); }
                    let detail = if let Some(ref wd)=working_dir { format!("[{}] {}", wd, trimmed) } else { trimmed.clone() };
                    let final_command = if let Some(wd)=working_dir.clone() { format!("cd {} && {}", quote_dir(&wd), trimmed) } else { trimmed.clone() };
                    let verdict = evaluate_command(&final_command);
                    if !verdict.ok {
                        let reason = verdict.reason.unwrap_or_else(|| "blocked".into());
                        let draft = pluk_store::LogDraft { connection_id: conn.id.clone(), connection_name: conn.name.clone(), sql: final_command.clone(), verdict: pluk_store::Verdict::Blocked, categories: Some("command".into()), reason: Some(reason.clone()), source: Some("run_command".into()), group: conn.via_group.clone(), database: None };
                        let _=store.create_log_entry(draft);
                        return err(format!("Blocked: {}", reason));
                    }
                    let timeout_ms = timeout.map(|t| t*1000);
                    let target = CallTarget { connection_id: conn.id.clone(), connection_name: conn.name.clone(), group: conn.via_group.clone() };
                    let meta = GateMeta { category: "command".into(), action: "run_command".into(), detail, database: None, command: Some(final_command.clone()) };
                    run_gated(&store, &target, meta, move |_log_id| {
                        let conn = conn.clone();
                        let _owner = owner.clone();
                        let final_command = final_command.clone();
                        async move {
                            match run_command(&conn, &final_command, timeout_ms).await {
                                Ok(res) => {
                                    let text = format_result(&res.stdout, &res.stderr, res.code, res.truncated);
                                    let output = format!("{}{}{}", res.stdout, res.stderr, if res.truncated { "\n[output truncated at 1 MB]" } else { "" });
                                    let response = if output.trim().is_empty() { text.clone() } else { output };
                                    let result = pluk_store::QueryResult { fields: vec!["exit_code".into()], rows: vec![serde_json::json!(res.code)] };
                                    if res.code.unwrap_or(0)==0 { Ok(Outcome::Ran(RunOutcome { text, is_error: false, reason: None, result: Some(result), response_text: Some(response), command: Some(final_command) })) }
                                    else { Ok(Outcome::Ran(RunOutcome { text, is_error: true, reason: Some(format!("exit {}", res.code.unwrap_or(0))), result: Some(result), response_text: Some(response), command: Some(final_command) })) }
                                },
                                Err(e) => Err(crate::error::AdapterError::new(e.message).with_code(e.code.unwrap_or_default())),
                            }
                        }
                    }, GateOpts::default().format_error(|e,_| humanize_ssh_error(e))).await
                })
            })
        );
    }

    if on("run_batch") {
        let mut props = Map::new();
        props.insert("commands".into(), serde_json::json!({"type":"array","items":{"type":"string"},"description": format!("Commands to run in order (max {}).", MAX_BATCH)}));
        props.insert("working_dir".into(), serde_json::json!({"type":"string","description":"Directory to run every command in. Optional."}));
        props.insert("stop_on_error".into(), serde_json::json!({"type":"boolean","description":"Stop at the first failed command instead of continuing. Default true."}));
        let schema = object_schema(props, &["commands"]);
        let conn_c = conn.clone();
        let owner_c = owner_id.to_string();
        let store_c = store.clone();
        host.register_tool(
            ToolRegistration { name: "run_batch".into(), description: "Run several shell commands in sequence on the remote host. Returns each command's output in order. Confirm before running — commands run unmodified as the connecting user.".into(), input_schema: schema, annotations: { let mut m=Map::new(); m.insert("readOnlyHint".into(), Value::Bool(false)); m.insert("destructiveHint".into(), Value::Bool(true)); m.insert("openWorldHint".into(), Value::Bool(true)); m } },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let conn = conn_c.clone();
                let owner = owner_c.clone();
                let store = store_c.clone();
                Box::pin(async move {
                    let obj = args.as_object().cloned().unwrap_or_default();
                    let commands = obj.get("commands").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                    let working_dir = obj.get("working_dir").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let stop_on_error = obj.get("stop_on_error").and_then(|v| v.as_bool()).unwrap_or(true);
                    if commands.is_empty() { return err("No commands"); }
                    if commands.len()>MAX_BATCH { return err(format!("Too many commands (max {})", MAX_BATCH)); }
                    let mut sections: Vec<String> = Vec::new();
                    let mut any_error = false;
                    for (i, cmd_val) in commands.iter().enumerate() {
                        let cmd = cmd_val.as_str().unwrap_or("").to_string();
                        let trimmed = cmd.trim().to_string();
                        if trimmed.is_empty() { sections.push(format!("$ {}\nError: empty command.", cmd)); any_error=true; if stop_on_error { break; } else { continue; } }
                        let final_cmd = if let Some(ref wd)=working_dir { format!("cd {} && {}", quote_dir(wd), trimmed) } else { trimmed.clone() };
                        let verdict = evaluate_command(&final_cmd);
                        if !verdict.ok {
                            let reason = verdict.reason.unwrap_or_else(|| "blocked".into());
                            sections.push(format!("$ {}\nBlocked: {}", cmd, reason));
                            any_error=true;
                            if stop_on_error { let skipped = commands.len()-i-1; if skipped>0 { sections.push(format!("[stopped on error — {} command(s) not run]", skipped)); } break; }
                            continue;
                        }
                        // run via gate
                        let detail = if let Some(ref wd)=working_dir { format!("[{}] {}", wd, cmd) } else { cmd.clone() };
                        let target = CallTarget { connection_id: conn.id.clone(), connection_name: conn.name.clone(), group: conn.via_group.clone() };
                        let meta = GateMeta { category: "command".into(), action: "run_batch".into(), detail, database: None, command: Some(final_cmd.clone()) };
                        let conn_clone = conn.clone();
                        let owner_clone = owner.clone();
                        let final_clone = final_cmd.clone();
                        let store_clone = store.clone();
                        let res = run_gated(&store_clone, &target, meta, move |_log_id| {
                            let conn = conn_clone.clone();
                            let _owner = owner_clone.clone();
                            let final_command = final_clone.clone();
                            async move {
                                match run_command(&conn, &final_command, None).await {
                                    Ok(r) => {
                                        let text = format_result(&r.stdout, &r.stderr, r.code, r.truncated);
                                        let output = format!("{}{}{}", r.stdout, r.stderr, if r.truncated { "\n[output truncated at 1 MB]" } else { "" });
                                        let response = if output.trim().is_empty() { text.clone() } else { output };
                                        let result = pluk_store::QueryResult { fields: vec!["exit_code".into()], rows: vec![serde_json::json!(r.code)] };
                                        if r.code.unwrap_or(0)==0 { Ok(Outcome::Ran(RunOutcome { text, is_error: false, reason: None, result: Some(result), response_text: Some(response), command: Some(final_command) })) }
                                        else { Ok(Outcome::Ran(RunOutcome { text, is_error: true, reason: Some(format!("exit {}", r.code.unwrap_or(0))), result: Some(result), response_text: Some(response), command: Some(final_command) })) }
                                    },
                                    Err(e) => Err(crate::error::AdapterError::new(e.message).with_code(e.code.unwrap_or_default())),
                                }
                            }
                        }, GateOpts::default().format_error(|e,_| humanize_ssh_error(e))).await;
                        sections.push(format!("$ {}\n{}", cmd, text_of(&res)));
                        if res.is_error { any_error=true; if stop_on_error { let skipped = commands.len()-i-1; if skipped>0 { sections.push(format!("[stopped on error — {} command(s) not run]", skipped)); } break; } }
                    }
                    let text = sections.join("\n\n———\n\n");
                    if any_error { err(text) } else { ok(text) }
                })
            })
        );
    }

    if on("debug_snapshot") {
        let conn_c = conn.clone();
        let owner_c = owner_id.to_string();
        let store_c = store.clone();
        host.register_tool(
            ToolRegistration { name: "debug_snapshot".into(), description: "Capture a quick health snapshot of the remote host (kernel, load, disk, memory, processes, logins, containers). Useful as a first step when debugging.".into(), input_schema: Map::new(), annotations: { let mut m=Map::new(); m.insert("readOnlyHint".into(), Value::Bool(false)); m.insert("destructiveHint".into(), Value::Bool(true)); m.insert("openWorldHint".into(), Value::Bool(true)); m } },
            Arc::new(move |_args: Value| -> BoxFuture<ToolResult> {
                let conn = conn_c.clone();
                let owner = owner_c.clone();
                let store = store_c.clone();
                Box::pin(async move {
                    let mut sections: Vec<String> = Vec::new();
                    let mut any_error=false;
                    for (label, cmd) in DEBUG_SNAPSHOT {
                        let final_cmd = cmd.to_string();
                        let verdict = evaluate_command(&final_cmd);
                        if !verdict.ok {
                            let reason = verdict.reason.unwrap_or_else(|| "blocked".into());
                            sections.push(format!("## {} — `{}`\nBlocked: {}", label, cmd, reason));
                            any_error=true;
                            continue;
                        }
                        let detail = format!("{} — {}", label, cmd);
                        let target = CallTarget { connection_id: conn.id.clone(), connection_name: conn.name.clone(), group: conn.via_group.clone() };
                        let meta = GateMeta { category: "command".into(), action: "debug_snapshot".into(), detail, database: None, command: Some(final_cmd.clone()) };
                        let conn_clone = conn.clone();
                        let owner_clone = owner.clone();
                        let final_clone = final_cmd.clone();
                        let store_clone = store.clone();
                        let res = run_gated(&store_clone, &target, meta, move |_log_id| {
                            let conn = conn_clone.clone();
                            let _owner = owner_clone.clone();
                            let final_command = final_clone.clone();
                            async move {
                                match run_command(&conn, &final_command, None).await {
                                    Ok(r) => {
                                        let text = format_result(&r.stdout, &r.stderr, r.code, r.truncated);
                                        let output = format!("{}{}{}", r.stdout, r.stderr, if r.truncated { "\n[output truncated at 1 MB]" } else { "" });
                                        let response = if output.trim().is_empty() { text.clone() } else { output };
                                        let result = pluk_store::QueryResult { fields: vec!["exit_code".into()], rows: vec![serde_json::json!(r.code)] };
                                        if r.code.unwrap_or(0)==0 { Ok(Outcome::Ran(RunOutcome { text, is_error: false, reason: None, result: Some(result), response_text: Some(response), command: Some(final_command) })) }
                                        else { Ok(Outcome::Ran(RunOutcome { text, is_error: true, reason: Some(format!("exit {}", r.code.unwrap_or(0))), result: Some(result), response_text: Some(response), command: Some(final_command) })) }
                                    },
                                    Err(e) => Err(crate::error::AdapterError::new(e.message).with_code(e.code.unwrap_or_default())),
                                }
                            }
                        }, GateOpts::default().format_error(|e,_| humanize_ssh_error(e))).await;
                        if res.is_error { any_error=true; }
                        sections.push(format!("## {} — `{}`\n{}", label, cmd, text_of(&res)));
                    }
                    let text = sections.join("\n\n");
                    if any_error { err(text) } else { ok(text) }
                })
            })
        );
    }

    if on("run_saved_command") {
        let conn_c = conn.clone();
        let owner_c = owner_id.to_string();
        let store_c = store.clone();
        host.register_tool(
            ToolRegistration { name: "run_saved_command".into(), description: "Run a pre-selected (saved) command by name. Confirm before running — saved commands run unmodified as the connecting user.".into(), input_schema: object_schema({ let mut m=Map::new(); m.insert("name".into(), serde_json::json!({"type":"string","description":"Name of the saved command"})); m }, &["name"]), annotations: { let mut m=Map::new(); m.insert("readOnlyHint".into(), Value::Bool(false)); m.insert("destructiveHint".into(), Value::Bool(true)); m.insert("openWorldHint".into(), Value::Bool(true)); m } },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let conn = conn_c.clone();
                let owner = owner_c.clone();
                let store = store_c.clone();
                Box::pin(async move {
                    let obj = args.as_object().cloned().unwrap_or_default();
                    let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let saved = store.get_saved_command(&conn.id, &name).unwrap_or(None);
                    let saved = match saved { Some(s)=>s, None=> {
                        let names = store.list_saved_commands(&conn.id).unwrap_or_default().into_iter().map(|c| c.name).collect::<Vec<_>>();
                        let hint = if names.is_empty() { " There are no saved commands for this integration yet.".to_string() } else { format!(" Available: {}.", names.iter().map(|n| format!("\"{}\"", n)).collect::<Vec<_>>().join(", ")) };
                        return err(format!("Saved command \"{}\" not found.{}", name, hint));
                    }};
                    let command = saved.command.clone();
                    let working_dir = saved.working_dir.clone();
                    let detail = if let Some(ref wd)=working_dir { format!("[{}] {}", wd, command) } else { command.clone() };
                    let final_command = if let Some(wd)=working_dir.clone() { format!("cd {} && {}", quote_dir(&wd), command) } else { command.clone() };
                    // Saved commands intentionally not filtered by policy (same freedom as ad-hoc) — but we still run policy? per brief they have no allowlist, run with same freedom. So skip policy check.
                    let target = CallTarget { connection_id: conn.id.clone(), connection_name: conn.name.clone(), group: conn.via_group.clone() };
                    let meta = GateMeta { category: "command".into(), action: "run_saved_command".into(), detail, database: None, command: Some(final_command.clone()) };
                    run_gated(&store, &target, meta, move |_log_id| {
                        let conn = conn.clone();
                        let _owner = owner.clone();
                        let final_command = final_command.clone();
                        async move {
                            match run_command(&conn, &final_command, None).await {
                                Ok(r) => {
                                    let text = format_result(&r.stdout, &r.stderr, r.code, r.truncated);
                                    let output = format!("{}{}{}", r.stdout, r.stderr, if r.truncated { "\n[output truncated at 1 MB]" } else { "" });
                                    let response = if output.trim().is_empty() { text.clone() } else { output };
                                    let result = pluk_store::QueryResult { fields: vec!["exit_code".into()], rows: vec![serde_json::json!(r.code)] };
                                    if r.code.unwrap_or(0)==0 { Ok(Outcome::Ran(RunOutcome { text, is_error: false, reason: None, result: Some(result), response_text: Some(response), command: Some(final_command) })) }
                                    else { Ok(Outcome::Ran(RunOutcome { text, is_error: true, reason: Some(format!("exit {}", r.code.unwrap_or(0))), result: Some(result), response_text: Some(response), command: Some(final_command) })) }
                                },
                                Err(e) => Err(crate::error::AdapterError::new(e.message).with_code(e.code.unwrap_or_default())),
                            }
                        }
                    }, GateOpts::default().format_error(|e,_| humanize_ssh_error(e))).await
                })
            })
        );
    }

    if on("list_saved_commands") {
        let store_c = store.clone();
        let conn_c = conn.clone();
        host.register_tool(
            ToolRegistration { name: "list_saved_commands".into(), description: "List the pre-selected (saved) commands available for this SSH integration.".into(), input_schema: object_schema({ let mut m=Map::new(); m.insert("only".into(), crate::projection::only_param_schema(&["location"])); m }, &[]), annotations: { let mut m=Map::new(); m.insert("readOnlyHint".into(), Value::Bool(true)); m.insert("openWorldHint".into(), Value::Bool(false)); m } },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let store = store_c.clone();
                let _conn = conn_c.clone();
                Box::pin(async move {
                    let obj = args.as_object().cloned().unwrap_or_default();
                    let only = obj.get("only").and_then(|v| v.as_array()).map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>());
                    let saved = store.list_saved_commands(&_conn.id).unwrap_or_default();
                    if saved.is_empty() { return ok("No saved commands for this integration."); }
                    let vals: Vec<Value> = saved.into_iter().map(|c| {
                        let mut m=Map::new();
                        m.insert("name".into(), Value::String(c.name));
                        m.insert("command".into(), Value::String(c.command));
                        if let Some(wd)=c.working_dir { m.insert("working_dir".into(), Value::String(wd)); }
                        Value::Object(m)
                    }).collect();
                    let val = Value::Array(vals);
                    let map = saved_commands_map();
                    match apply_only(&val, only.as_ref(), &map) {
                        Ok(projected) => ok(serde_json::to_string_pretty(&projected).unwrap()),
                        Err(e) => err(e.to_string()),
                    }
                })
            })
        );
    }

    if on("open_forward") {
        let conn_c = conn.clone();
        let owner_c = owner_id.to_string();
        let store_c = store.clone();
        host.register_tool(
            ToolRegistration { name: "open_forward".into(), description: "Open a local port forward (ssh -L) over this connection so a service reachable from the remote host becomes available at localhost on this machine. Returns the local port to connect to (e.g. `psql -h localhost -p <port>`). The forward stays open until closed.".into(), input_schema: object_schema({
                let mut m=Map::new();
                m.insert("remote_port".into(), serde_json::json!({"type":"number","description":"Port on the remote side to forward, e.g. 5432 for Postgres or 6379 for Redis"}));
                m.insert("remote_host".into(), serde_json::json!({"type":"string","description":"Host to reach from the remote side. Defaults to `localhost` (a service running on the SSH host itself); set this to reach another host on the remote network."}));
                m.insert("local_port".into(), serde_json::json!({"type":"number","description":"Local port to listen on. Omit to auto-assign a free port."}));
                m
            }, &["remote_port"]), annotations: { let mut m=Map::new(); m.insert("readOnlyHint".into(), Value::Bool(false)); m.insert("destructiveHint".into(), Value::Bool(false)); m.insert("openWorldHint".into(), Value::Bool(true)); m } },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let conn = conn_c.clone();
                let owner = owner_c.clone();
                let store = store_c.clone();
                Box::pin(async move {
                    let obj = args.as_object().cloned().unwrap_or_default();
                    let remote_port = obj.get("remote_port").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
                    if remote_port==0 { return err("remote_port must be 1-65535"); }
                    let remote_host = obj.get("remote_host").and_then(|v| v.as_str()).unwrap_or("localhost").to_string();
                    let remote_host_trim = remote_host.trim();
                    let rh = if remote_host_trim.is_empty() { "localhost".to_string() } else { remote_host_trim.to_string() };
                    let local_port = obj.get("local_port").and_then(|v| v.as_u64()).map(|n| n as u16);
                    if let Some(p)=local_port && p==0 { return err("local_port must be 1-65535"); }
                    let detail = format!("open_forward localhost:{} -> {}:{}", local_port.map(|p| p.to_string()).unwrap_or_else(|| "auto".into()), rh, remote_port);
                    let target = CallTarget { connection_id: conn.id.clone(), connection_name: conn.name.clone(), group: conn.via_group.clone() };
                    let meta = GateMeta { category: "forward".into(), action: "open_forward".into(), detail, database: None, command: None };
                    let conn_clone = conn.clone();
                    let owner_clone = owner.clone();
                    let rh_clone = rh.clone();
                    run_gated(&store, &target, meta, move |_log_id| {
                        let conn = conn_clone.clone();
                        let owner = owner_clone.clone();
                        let rh = rh_clone.clone();
                        async move {
                            match open_forward(&owner, &conn, &rh, remote_port, local_port).await {
                                Ok(fwd) => {
                                    let text = format!("Forward open: localhost:{} → {}:{} (id \"{}\").\nConnect to it at 127.0.0.1:{} on this machine. Close it with close_forward \"{}\".", fwd.local_port, fwd.remote_host, fwd.remote_port, fwd.id, fwd.local_port, fwd.id);
                                    let row = forward_row(&fwd);
                                    let result = pluk_store::QueryResult { fields: vec!["id".into(),"local".into(),"remote".into()], rows: vec![row] };
                                    Ok(Outcome::Ran(RunOutcome { text: text.clone(), result: Some(result), response_text: Some(text), command: None, ..Default::default() }))
                                },
                                Err(e) => Err(e),
                            }
                        }
                    }, GateOpts::default().format_error(|e,_| humanize_ssh_error(e))).await
                })
            })
        );
    }

    if on("list_forwards") {
        let conn_c = conn.clone();
        let owner_c = owner_id.to_string();
        host.register_tool(
            ToolRegistration { name: "list_forwards".into(), description: "List the open local port forwards for this connection (local port → remote target).".into(), input_schema: Map::new(), annotations: { let mut m=Map::new(); m.insert("readOnlyHint".into(), Value::Bool(true)); m.insert("openWorldHint".into(), Value::Bool(false)); m } },
            Arc::new(move |_args: Value| -> BoxFuture<ToolResult> {
                let conn = conn_c.clone();
                let owner = owner_c.clone();
                Box::pin(async move {
                    let forwards = list_forwards(&owner, &conn).into_iter().map(|f| forward_row(&f)).collect::<Vec<_>>();
                    if forwards.is_empty() { return ok("No open forwards for this connection."); }
                    ok(serde_json::to_string_pretty(&forwards).unwrap())
                })
            })
        );
    }

    if on("close_forward") {
        let conn_c = conn.clone();
        let owner_c = owner_id.to_string();
        host.register_tool(
            ToolRegistration { name: "close_forward".into(), description: "Close an open local port forward by its id (the `remoteHost:remotePort` returned by open_forward / list_forwards).".into(), input_schema: object_schema({ let mut m=Map::new(); m.insert("id".into(), serde_json::json!({"type":"string","description":"Forward id, e.g. \"localhost:5432\""})); m }, &["id"]), annotations: { let mut m=Map::new(); m.insert("readOnlyHint".into(), Value::Bool(false)); m.insert("destructiveHint".into(), Value::Bool(false)); m.insert("openWorldHint".into(), Value::Bool(true)); m } },
            Arc::new(move |args: Value| -> BoxFuture<ToolResult> {
                let conn = conn_c.clone();
                let owner = owner_c.clone();
                Box::pin(async move {
                    let obj = args.as_object().cloned().unwrap_or_default();
                    let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let closed = close_forward(&owner, &conn, &id);
                    if closed { ok(format!("Closed forward \"{}\".", id)) } else { err(format!("No open forward with id \"{}\".", id)) }
                })
            })
        );
    }

    Ok(())
}
