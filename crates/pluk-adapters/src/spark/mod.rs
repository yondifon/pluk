pub mod client;
pub mod fields;

use std::sync::Arc;

use serde_json::{Map, Value, json};

use pluk_policy::ActionCategory;

use crate::action::{ActionAdapter, ActionAdapterSpec, ActionOutput, ActionTool};
use crate::error::AdapterError;

pub use client::{
    SparkCfg, assert_message_id, assert_positional, flag, flag_each, paging, range_args,
    same_account, scoped, set_spark_runner, spark_command, spark_config, toggle,
};
pub use fields::spark_fields;

const AGENT_HINT: &str = "Use this for the user's mail, calendar, contacts and meetings in Spark. accounts first to see accounts, calendars and each one's access level; list_emails to browse a folder, search_emails to answer questions (it returns bodies), read_thread for the whole conversation. When this integration names an account every folder, scope and calendar is confined to it — a bare folder name means that account's folder, and another account, shared inbox or team is refused, not silently redirected. Spark itself gates writes per account (read-only / triage / send) on top of this integration's tools.";
const ACCESS: &str = "Reads mail, calendar, contacts, meetings and teams from the Spark Desktop running on this machine; drafts, comments, email and contact actions, and calendar writes only when those tools are enabled. Sending a draft and deleting an event are separate tools, off by default. Every call is policy-checked and recorded in the activity log — including the message bodies Spark returns.";

// ── helpers for JSON arg extraction ───────────────────────────────────────

fn s(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|x| x.to_string())
        .filter(|x| !x.trim().is_empty())
}
fn b(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(|v| v.as_bool())
}
fn b_true(args: &Value, key: &str) -> bool {
    b(args, key).unwrap_or(false)
}
fn i64_opt(args: &Value, key: &str) -> Option<i64> {
    args.get(key)
        .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|n| n as i64)))
}
fn arr_str(args: &Value, key: &str) -> Vec<String> {
    match args.get(key) {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        Some(Value::String(x)) if !x.trim().is_empty() => vec![x.trim().to_string()],
        _ => vec![],
    }
}

fn prop_str(desc: &str) -> Value {
    json!({"type":"string","description":desc})
}
fn prop_opt_str(desc: &str) -> Value {
    json!({"type":"string","description":desc})
}

// ── tool builders ─────────────────────────────────────────────────────────

pub fn spark_tools(cfg: SparkCfg) -> Vec<ActionTool> {
    let mut tools: Vec<ActionTool> = Vec::new();

    let email_actions = [
        "pin",
        "unpin",
        "mute",
        "unmute",
        "snooze",
        "unsnooze",
        "changeReminder",
        "clearReminder",
        "setAside",
        "archive",
        "moveToInbox",
        "moveToTrash",
        "moveToFolder",
        "attachLabel",
        "detachLabel",
        "markAsDone",
        "markAsUndone",
        "markAsSeen",
        "markAsUnseen",
        "markAsSpam",
        "markThreadAsPriority",
        "unmarkThreadAsPriority",
        "unsubscribe",
        "changeCategoryPersonal",
        "changeCategoryNotification",
        "changeCategoryNewsletters",
        "shareInTeam",
        "assign",
        "delegationComplete",
        "delegationReopen",
    ];
    let contact_actions = [
        "changeCategoryPersonal",
        "changeCategoryNotification",
        "changeCategoryNewsletters",
        "groupEmailsFromContact",
        "groupEmailsFromContactAndShowInInbox",
        "ungroupEmailsFromContact",
        "markContactAsImportant",
        "unmarkContactAsImportant",
        "markContactAsPrimary",
        "unmarkContactAsPrimary",
        "acceptContact",
        "blockContact",
        "acceptDomain",
        "blockDomain",
        "enableAutosummaryForContact",
        "disableAutosummaryForContact",
    ];

    // accounts
    {
        let c = cfg.clone();
        let c2 = c.clone();
        let c3 = c.clone();
        tools.push(
            ActionTool::new("accounts", "List accounts with their calendars, teams, shared inboxes and each one's Spark access level. Run this first.", ActionCategory::Read)
                .detail_fn(|_| "accounts".to_string())
                .command_fn(move |_, _| client::spark_command(&c2, &["accounts".to_string()]))
                .run(move |_, _| {
                    let c = c.clone();
                    async move {
                        let out = client::run_spark(&c, vec!["accounts".to_string()]).await?;
                        let cmd = client::spark_command(&c, &["accounts".to_string()]);
                        Ok(ActionOutput::with_command(Value::String(out), cmd))
                    }
                }),
        );
        let _ = c3;
    }
    // folders
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let c_detail = c.clone();
        let mut props = Map::new();
        props.insert("accounts".to_string(), json!({"type":"array","items":{"type":"string"},"description":"Account or shared-inbox addresses; the integration's account, or all of them when it names none, when omitted"}));
        tools.push(
            ActionTool::new("folders", "List folders and labels with message counts. Returns the qualified identifiers other tools take.", ActionCategory::Read)
                .schema(props)
                .detail_fn(move |args| {
                    let v = arr_str(args, "accounts").join(" ");
                    if v.is_empty() { "folders all".to_string() } else { format!("folders {v}") }
                })
                .command_fn(move |args, _| {
                    let c = &c_cmd;
                    let asked: Result<Vec<String>, _> = arr_str(args, "accounts").into_iter().map(|v| {
                        let pos = assert_positional(&v, "account")?;
                        same_account(c, Some(&pos), "account")
                    }).collect();
                    match asked {
                        Ok(a) => {
                            let list = if a.is_empty() {
                                if c.account.is_empty() { vec![] } else { vec![c.account.clone()] }
                            } else { a };
                            let mut cmd_args = vec!["folders".to_string()];
                            cmd_args.extend(list);
                            client::spark_command(c, &cmd_args)
                        }
                        Err(e) => format!("folders error: {}", e.message),
                    }
                })
                .run(move |args, _| {
                    let c = c_run.clone();
                    let _d = c_detail.clone();
                    async move {
                        let asked: Vec<String> = arr_str(&args, "accounts").into_iter().map(|v| {
                            let pos = assert_positional(&v, "account")?;
                            same_account(&c, Some(&pos), "account")
                        }).collect::<Result<Vec<_>, _>>()?;
                        let list = if asked.is_empty() {
                            if c.account.is_empty() { vec![] } else { vec![c.account.clone()] }
                        } else { asked };
                        let mut cmd_args = vec!["folders".to_string()];
                        cmd_args.extend(list.clone());
                        let cmd = client::spark_command(&c, &cmd_args);
                        let out = client::run_spark(&c, cmd_args).await?;
                        Ok(ActionOutput::with_command(Value::String(out), cmd))
                    }
                }),
        );
        let _ = c;
    }
    // list_emails
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert("folders".to_string(), json!({"type":"array","items":{"type":"string"},"description":"Folder ids from folders, e.g. \"you@co.com:Archive\". The integration's inbox — or the cross-account Unified Inbox when it names no account — when omitted"}));
        props.insert("filter".to_string(), prop_opt_str("Gmail-style filter, combinable: from: to: cc: subject: before:yyyy/MM/dd after: newer_than:7d older_than:30d has:attachment is:unread is:starred is:pinned category:priority assigned_to:me filename:"));
        props.insert(
            "order".to_string(),
            json!({"type":"string","enum":["ascending","descending"]}),
        );
        props.insert("new_senders".to_string(), json!({"type":"boolean","description":"Only mail from senders GateKeeper is holding back"}));
        props.insert(
            "page".to_string(),
            json!({"type":"integer","minimum":1,"description":"Page number, 1-based"}),
        );
        props.insert("page_size".to_string(), json!({"type":"integer","minimum":1,"description":"Rows per page; capped by the integration"}));
        tools.push(
            ActionTool::new("list_emails", "List emails in a folder — id, account, sender, date, subject, flags. Browsing only: use search_emails to find mail across every folder.", ActionCategory::Read)
                .schema(props)
                .detail_fn(|args| {
                    let folders = arr_str(args, "folders").join(" ");
                    let filter = s(args, "filter").map(|x| format!(" [{x}]")).unwrap_or_default();
                    format!("list_emails {}{filter}", if folders.is_empty() { "inbox".to_string() } else { folders })
                })
                .command_fn(move |args, _| {
                    let c = &c_cmd;
                    let asked_raw = arr_str(args, "folders");
                    let asked = if asked_raw.is_empty() {
                        if c.folder.is_empty() { vec![] } else { vec![c.folder.clone()] }
                    } else { asked_raw };
                    let folders = if asked.is_empty() {
                        if c.account.is_empty() { vec![] } else { vec![c.account.clone()] }
                    } else { asked };
                    let mut a = vec!["emails".to_string()];
                    for f in folders {
                        match assert_positional(&f, "folder").and_then(|v| scoped(c, Some(&v), "folder")) {
                            Ok(v) => a.push(v),
                            Err(e) => return format!("list_emails error: {}", e.message),
                        }
                    }
                    flag(&mut a, "--filter", s(args, "filter").as_deref());
                    flag(&mut a, "--order", s(args, "order").as_deref());
                    toggle(&mut a, "--new-senders", b_true(args, "new_senders"));
                    paging(&mut a, c, i64_opt(args, "page"), i64_opt(args, "page_size"));
                    client::spark_command(c, &a)
                })
                .run(move |args, _| {
                    let c = c_run.clone();
                    async move {
                        let asked_raw = arr_str(&args, "folders");
                        let asked = if asked_raw.is_empty() {
                            if c.folder.is_empty() { vec![] } else { vec![c.folder.clone()] }
                        } else { asked_raw };
                        let folders = if asked.is_empty() {
                            if c.account.is_empty() { vec![] } else { vec![c.account.clone()] }
                        } else { asked };
                        let mut a = vec!["emails".to_string()];
                        for f in folders {
                            let pos = assert_positional(&f, "folder")?;
                            let sc = scoped(&c, Some(&pos), "folder")?;
                            a.push(sc);
                        }
                        flag(&mut a, "--filter", s(&args, "filter").as_deref());
                        flag(&mut a, "--order", s(&args, "order").as_deref());
                        toggle(&mut a, "--new-senders", b_true(&args, "new_senders"));
                        paging(&mut a, &c, i64_opt(&args, "page"), i64_opt(&args, "page_size"));
                        let cmd = client::spark_command(&c, &a);
                        let out = client::run_spark(&c, a).await?;
                        Ok(ActionOutput::with_command(Value::String(out), cmd))
                    }
                }),
        );
    }
    // search_emails
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert(
            "about".to_string(),
            prop_opt_str("Topic to search for; omit to list by filter instead"),
        );
        props.insert("filter".to_string(), prop_opt_str("Gmail-style filter, combinable: from: to: cc: subject: before:yyyy/MM/dd after: newer_than:7d older_than:30d has:attachment is:unread is:starred is:pinned category:priority assigned_to:me filename:"));
        props.insert("in".to_string(), prop_opt_str("Scope: account, \"Team Name\", shared inbox or a qualified folder. The integration's account — or every folder when it names none — when omitted"));
        props.insert("order".to_string(), json!({"type":"string","enum":["ascending","descending"],"description":"List mode only"}));
        props.insert(
            "page".to_string(),
            json!({"type":"integer","minimum":1,"description":"Page number, 1-based"}),
        );
        props.insert("page_size".to_string(), json!({"type":"integer","minimum":1,"description":"Rows per page; capped by the integration"}));
        tools.push(
            ActionTool::new("search_emails", "Search mail across every folder. With about it does keyword + semantic matching and returns bodies — use it to answer questions. Without it, filters across all folders.", ActionCategory::Read)
                .schema(props)
                .detail_fn(|args| {
                    let about = s(args, "about").unwrap_or_default();
                    let filter = s(args, "filter").map(|x| format!(" [{x}]")).unwrap_or_default();
                    format!("search_emails {about}{filter}").trim().to_string()
                })
                .command_fn(move |args, _| {
                    let c = &c_cmd;
                    let mut a = vec!["search".to_string()];
                    flag(&mut a, "--filter", s(args, "filter").as_deref());
                    let scope = match scoped(c, s(args, "in").as_deref(), "scope") {
                        Ok(v) => if v.is_empty() { c.account.clone() } else { v },
                        Err(e) => return format!("search_emails error: {}", e.message),
                    };
                    flag(&mut a, "--in", if scope.is_empty() { None } else { Some(scope.as_str()) });
                    flag(&mut a, "--order", s(args, "order").as_deref());
                    paging(&mut a, c, i64_opt(args, "page"), i64_opt(args, "page_size"));
                    if let Some(about) = s(args, "about") {
                        match assert_positional(&about, "search topic") {
                            Ok(v) => a.push(v),
                            Err(e) => return format!("search_emails error: {}", e.message),
                        }
                    }
                    client::spark_command(c, &a)
                })
                .run(move |args, _| {
                    let c = c_run.clone();
                    async move {
                        let mut a = vec!["search".to_string()];
                        flag(&mut a, "--filter", s(&args, "filter").as_deref());
                        let scope_raw = s(&args, "in");
                        let scope = scoped(&c, scope_raw.as_deref(), "scope")?;
                        let scope_val = if scope.is_empty() { c.account.clone() } else { scope };
                        flag(&mut a, "--in", if scope_val.is_empty() { None } else { Some(scope_val.as_str()) });
                        flag(&mut a, "--order", s(&args, "order").as_deref());
                        paging(&mut a, &c, i64_opt(&args, "page"), i64_opt(&args, "page_size"));
                        if let Some(about) = s(&args, "about") {
                            let pos = assert_positional(&about, "search topic")?;
                            a.push(pos);
                        }
                        let cmd = client::spark_command(&c, &a);
                        let out = client::run_spark(&c, a).await?;
                        Ok(ActionOutput::with_command(Value::String(out), cmd))
                    }
                }),
        );
    }
    // read_thread
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert(
            "message_id".to_string(),
            prop_str("Message id from list_emails / search_emails, or a Spark deep link"),
        );
        props.insert("download_attachments".to_string(), json!({"type":"boolean","description":"Fetch attachments that aren't cached locally yet"}));
        tools.push(
            ActionTool::new("read_thread", "Read a full thread — headers, plain-text bodies, attachment table and the thread's custom labels.", ActionCategory::Read)
                .schema(props)
                .detail_fn(|args| format!("read_thread {}", s(args, "message_id").unwrap_or_default()))
                .command_fn(move |args, _| {
                    let c = &c_cmd;
                    let mut a = vec!["thread".to_string()];
                    toggle(&mut a, "--download-attachments", b_true(args, "download_attachments"));
                    match s(args, "message_id").map(|v| assert_message_id(&v, "message id")) {
                        Some(Ok(v)) => a.push(v),
                        Some(Err(e)) => return format!("read_thread error: {}", e.message),
                        None => return "read_thread error: message_id required".to_string(),
                    }
                    client::spark_command(c, &a)
                })
                .run(move |args, _| {
                    let c = c_run.clone();
                    async move {
                        let mut a = vec!["thread".to_string()];
                        toggle(&mut a, "--download-attachments", b_true(&args, "download_attachments"));
                        let mid = s(&args, "message_id").ok_or_else(|| AdapterError::new("message_id is required."))?;
                        let id = assert_message_id(&mid, "message id")?;
                        a.push(id);
                        let cmd = client::spark_command(&c, &a);
                        let out = client::run_spark(&c, a).await?;
                        Ok(ActionOutput::with_command(Value::String(out), cmd))
                    }
                }),
        );
    }
    // read_attachment
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert("id".to_string(), json!({"type":"integer","description":"Attachment id (pk) from the thread's Attachments table"}));
        tools.push(
            ActionTool::new("read_attachment", "Show one attachment's metadata — name, size, MIME type and local path — downloading it first if needed. Ids come from read_thread.", ActionCategory::Read)
                .schema(props)
                .detail_fn(|args| format!("read_attachment {}", i64_opt(args, "id").unwrap_or(0)))
                .command_fn(move |args, _| {
                    let c = &c_cmd;
                    let id = i64_opt(args, "id").unwrap_or(0);
                    client::spark_command(c, &["attachment".to_string(), id.to_string()])
                })
                .run(move |args, _| {
                    let c = c_run.clone();
                    async move {
                        let id = i64_opt(&args, "id").ok_or_else(|| AdapterError::new("id is required."))?;
                        let a = vec!["attachment".to_string(), id.to_string()];
                        let cmd = client::spark_command(&c, &a);
                        let out = client::run_spark(&c, a).await?;
                        Ok(ActionOutput::with_command(Value::String(out), cmd))
                    }
                }),
        );
    }
    // list_events
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert("range".to_string(), json!({"type":"string","enum":["today","tomorrow","week"],"description":"Range shortcut; ignored when start/end are given"}));
        props.insert(
            "start".to_string(),
            prop_opt_str("Start date: yyyy-MM-dd, dd/MM/yyyy or yyyy-MM-ddTHH:mm"),
        );
        props.insert(
            "end".to_string(),
            prop_opt_str("End date, same formats as start"),
        );
        props.insert("in".to_string(), prop_opt_str("Account or calendar, e.g. \"you@co.com:Work\". The integration's account — or every calendar when it names none — when omitted"));
        tools.push(
            ActionTool::new(
                "list_events",
                "List calendar events for a time range. Defaults to the rest of today.",
                ActionCategory::Read,
            )
            .schema(props)
            .detail_fn(|args| {
                format!(
                    "list_events {}",
                    s(args, "range").unwrap_or_else(|| format!(
                        "{}..{}",
                        s(args, "start").unwrap_or_default(),
                        s(args, "end").unwrap_or_default()
                    ))
                )
            })
            .command_fn(move |args, _| {
                let c = &c_cmd;
                let mut a = vec!["events".to_string()];
                range_args(
                    &mut a,
                    s(args, "start").as_deref(),
                    s(args, "end").as_deref(),
                    s(args, "range").as_deref(),
                );
                let cal = match scoped(c, s(args, "in").as_deref(), "calendar") {
                    Ok(v) => {
                        if v.is_empty() {
                            c.account.clone()
                        } else {
                            v
                        }
                    }
                    Err(e) => return format!("list_events error: {}", e.message),
                };
                flag(
                    &mut a,
                    "--in",
                    if cal.is_empty() {
                        None
                    } else {
                        Some(cal.as_str())
                    },
                );
                client::spark_command(c, &a)
            })
            .run(move |args, _| {
                let c = c_run.clone();
                async move {
                    let mut a = vec!["events".to_string()];
                    range_args(
                        &mut a,
                        s(&args, "start").as_deref(),
                        s(&args, "end").as_deref(),
                        s(&args, "range").as_deref(),
                    );
                    let cal_raw = s(&args, "in");
                    let cal = scoped(&c, cal_raw.as_deref(), "calendar")?;
                    let cal_val = if cal.is_empty() {
                        c.account.clone()
                    } else {
                        cal
                    };
                    flag(
                        &mut a,
                        "--in",
                        if cal_val.is_empty() {
                            None
                        } else {
                            Some(cal_val.as_str())
                        },
                    );
                    let cmd = client::spark_command(&c, &a);
                    let out = client::run_spark(&c, a).await?;
                    Ok(ActionOutput::with_command(Value::String(out), cmd))
                }
            }),
        );
    }
    // availability
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert("attendees".to_string(), json!({"type":"array","items":{"type":"string"},"description":"Attendee addresses; your own calendar when omitted"}));
        props.insert("range".to_string(), json!({"type":"string","enum":["today","tomorrow","week"],"description":"Range shortcut; ignored when start/end are given"}));
        props.insert(
            "start".to_string(),
            prop_opt_str("Start date: yyyy-MM-dd, dd/MM/yyyy or yyyy-MM-ddTHH:mm"),
        );
        props.insert(
            "end".to_string(),
            prop_opt_str("End date, same formats as start"),
        );
        tools.push(
            ActionTool::new(
                "availability",
                "Find free time slots — your own, or the mutual windows for a set of attendees.",
                ActionCategory::Read,
            )
            .schema(props)
            .detail_fn(|args| {
                format!(
                    "availability {}",
                    if arr_str(args, "attendees").is_empty() {
                        "self".to_string()
                    } else {
                        arr_str(args, "attendees").join(",")
                    }
                )
            })
            .command_fn(move |args, _| {
                let c = &c_cmd;
                let mut a = vec!["availability".to_string()];
                range_args(
                    &mut a,
                    s(args, "start").as_deref(),
                    s(args, "end").as_deref(),
                    s(args, "range").as_deref(),
                );
                let attendees = arr_str(args, "attendees").join(",");
                flag(
                    &mut a,
                    "--attendees",
                    if attendees.is_empty() {
                        None
                    } else {
                        Some(attendees.as_str())
                    },
                );
                client::spark_command(c, &a)
            })
            .run(move |args, _| {
                let c = c_run.clone();
                async move {
                    let mut a = vec!["availability".to_string()];
                    range_args(
                        &mut a,
                        s(&args, "start").as_deref(),
                        s(&args, "end").as_deref(),
                        s(&args, "range").as_deref(),
                    );
                    let attendees = arr_str(&args, "attendees").join(",");
                    flag(
                        &mut a,
                        "--attendees",
                        if attendees.is_empty() {
                            None
                        } else {
                            Some(attendees.as_str())
                        },
                    );
                    let cmd = client::spark_command(&c, &a);
                    let out = client::run_spark(&c, a).await?;
                    Ok(ActionOutput::with_command(Value::String(out), cmd))
                }
            }),
        );
    }
    // find_contacts
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert("query".to_string(), prop_str("Name or email fragment"));
        tools.push(
            ActionTool::new("find_contacts", "Search contacts by name, part of a name, or any part of an email address including the domain.", ActionCategory::Read)
                .schema(props)
                .detail_fn(|args| format!("find_contacts {}", s(args, "query").unwrap_or_default()))
                .command_fn(move |args, _| {
                    let c = &c_cmd;
                    match s(args, "query").map(|v| assert_positional(&v, "query")) {
                        Some(Ok(v)) => client::spark_command(c, &["contacts".to_string(), v]),
                        Some(Err(e)) => format!("find_contacts error: {}", e.message),
                        None => "find_contacts error: query required".to_string(),
                    }
                })
                .run(move |args, _| {
                    let c = c_run.clone();
                    async move {
                        let q = s(&args, "query").ok_or_else(|| AdapterError::new("query is required."))?;
                        let pos = assert_positional(&q, "query")?;
                        let a = vec!["contacts".to_string(), pos.clone()];
                        let cmd = client::spark_command(&c, &a);
                        let out = client::run_spark(&c, a).await?;
                        Ok(ActionOutput::with_command(Value::String(out), cmd))
                    }
                }),
        );
    }
    // team_info
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert(
            "name".to_string(),
            prop_opt_str("Team name or a partial match"),
        );
        tools.push(
            ActionTool::new("team_info", "Show a team's members, shared inboxes and assignments. Omit the name to list the available teams.", ActionCategory::Read)
                .schema(props)
                .detail_fn(|args| format!("team_info {}", s(args, "name").unwrap_or_default()).trim().to_string())
                .command_fn(move |args, _| {
                    let c = &c_cmd;
                    match s(args, "name") {
                        Some(n) if !n.trim().is_empty() => match assert_positional(n.trim(), "team name") {
                            Ok(v) => client::spark_command(c, &["team".to_string(), v]),
                            Err(e) => format!("team_info error: {}", e.message),
                        },
                        _ => client::spark_command(c, &["team".to_string()]),
                    }
                })
                .run(move |args, _| {
                    let c = c_run.clone();
                    async move {
                        let name = s(&args, "name").map(|v| v.trim().to_string()).filter(|x| !x.is_empty());
                        let a = if let Some(n) = name {
                            let pos = assert_positional(&n, "team name")?;
                            vec!["team".to_string(), pos]
                        } else {
                            vec!["team".to_string()]
                        };
                        let cmd = client::spark_command(&c, &a);
                        let out = client::run_spark(&c, a).await?;
                        Ok(ActionOutput::with_command(Value::String(out), cmd))
                    }
                }),
        );
    }
    // list_meetings
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert(
            "filter".to_string(),
            prop_opt_str(
                "subject:<text>, before:/after:yyyy/MM/dd, newer_than:30d, older_than:30d",
            ),
        );
        props.insert(
            "page".to_string(),
            json!({"type":"integer","minimum":1,"description":"Page number, 1-based"}),
        );
        props.insert("page_size".to_string(), json!({"type":"integer","minimum":1,"description":"Rows per page; capped by the integration"}));
        tools.push(
            ActionTool::new(
                "list_meetings",
                "List the meeting transcripts Spark recorded, newest first.",
                ActionCategory::Read,
            )
            .schema(props)
            .detail_fn(|args| {
                format!(
                    "list_meetings{}",
                    s(args, "filter")
                        .map(|x| format!(" [{x}]"))
                        .unwrap_or_default()
                )
            })
            .command_fn(move |args, _| {
                let c = &c_cmd;
                let mut a = vec!["meetings".to_string()];
                flag(&mut a, "--filter", s(args, "filter").as_deref());
                paging(&mut a, c, i64_opt(args, "page"), i64_opt(args, "page_size"));
                client::spark_command(c, &a)
            })
            .run(move |args, _| {
                let c = c_run.clone();
                async move {
                    let mut a = vec!["meetings".to_string()];
                    flag(&mut a, "--filter", s(&args, "filter").as_deref());
                    paging(
                        &mut a,
                        &c,
                        i64_opt(&args, "page"),
                        i64_opt(&args, "page_size"),
                    );
                    let cmd = client::spark_command(&c, &a);
                    let out = client::run_spark(&c, a).await?;
                    Ok(ActionOutput::with_command(Value::String(out), cmd))
                }
            }),
        );
    }
    // read_meeting
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert(
            "message_id".to_string(),
            prop_str("Meeting id from list_meetings, or a Spark deep link"),
        );
        props.insert(
            "transcript".to_string(),
            json!({"type":"boolean","description":"Include the full transcript"}),
        );
        props.insert(
            "notes".to_string(),
            json!({"type":"boolean","description":"Include the user's notes"}),
        );
        tools.push(
            ActionTool::new("read_meeting", "Read a meeting's summary, and optionally its full transcript and the user's notes.", ActionCategory::Read)
                .schema(props)
                .detail_fn(|args| format!("read_meeting {}", s(args, "message_id").unwrap_or_default()))
                .command_fn(move |args, _| {
                    let c = &c_cmd;
                    let mut a = vec!["meeting".to_string()];
                    toggle(&mut a, "--transcript", b_true(args, "transcript"));
                    toggle(&mut a, "--notes", b_true(args, "notes"));
                    match s(args, "message_id").map(|v| assert_message_id(&v, "meeting id")) {
                        Some(Ok(v)) => a.push(v),
                        Some(Err(e)) => return format!("read_meeting error: {}", e.message),
                        None => return "read_meeting error: message_id required".to_string(),
                    }
                    client::spark_command(c, &a)
                })
                .run(move |args, _| {
                    let c = c_run.clone();
                    async move {
                        let mut a = vec!["meeting".to_string()];
                        toggle(&mut a, "--transcript", b_true(&args, "transcript"));
                        toggle(&mut a, "--notes", b_true(&args, "notes"));
                        let mid = s(&args, "message_id").ok_or_else(|| AdapterError::new("message_id is required."))?;
                        let id = assert_message_id(&mid, "meeting id")?;
                        a.push(id);
                        let cmd = client::spark_command(&c, &a);
                        let out = client::run_spark(&c, a).await?;
                        Ok(ActionOutput::with_command(Value::String(out), cmd))
                    }
                }),
        );
    }
    // list_templates
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert(
            "personal".to_string(),
            json!({"type":"boolean","description":"Only templates not tied to a team"}),
        );
        props.insert(
            "team".to_string(),
            prop_opt_str("Only this team's templates"),
        );
        props.insert(
            "page".to_string(),
            json!({"type":"integer","minimum":1,"description":"Page number, 1-based"}),
        );
        props.insert("page_size".to_string(), json!({"type":"integer","minimum":1,"description":"Rows per page; capped by the integration"}));
        tools.push(
            ActionTool::new(
                "list_templates",
                "List the user's saved message templates, personal and team.",
                ActionCategory::Read,
            )
            .schema(props)
            .detail_fn(|_| "list_templates".to_string())
            .command_fn(move |args, _| {
                let c = &c_cmd;
                let mut a = vec!["templates".to_string()];
                toggle(&mut a, "--personal", b_true(args, "personal"));
                flag(&mut a, "--team", s(args, "team").as_deref());
                paging(&mut a, c, i64_opt(args, "page"), i64_opt(args, "page_size"));
                client::spark_command(c, &a)
            })
            .run(move |args, _| {
                let c = c_run.clone();
                async move {
                    let mut a = vec!["templates".to_string()];
                    toggle(&mut a, "--personal", b_true(&args, "personal"));
                    flag(&mut a, "--team", s(&args, "team").as_deref());
                    paging(
                        &mut a,
                        &c,
                        i64_opt(&args, "page"),
                        i64_opt(&args, "page_size"),
                    );
                    let cmd = client::spark_command(&c, &a);
                    let out = client::run_spark(&c, a).await?;
                    Ok(ActionOutput::with_command(Value::String(out), cmd))
                }
            }),
        );
    }
    // read_template
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert("ref".to_string(), prop_str("Template id or name"));
        tools.push(
            ActionTool::new("read_template", "Show one template's recipients, subject, body and placeholders. Run it before drafting from a template — manual placeholders are required.", ActionCategory::Read)
                .schema(props)
                .detail_fn(|args| format!("read_template {}", s(args, "ref").unwrap_or_default()))
                .command_fn(move |args, _| {
                    let c = &c_cmd;
                    match s(args, "ref").map(|v| assert_positional(&v, "template id or name")) {
                        Some(Ok(v)) => client::spark_command(c, &["template".to_string(), v]),
                        Some(Err(e)) => format!("read_template error: {}", e.message),
                        None => "read_template error: ref required".to_string(),
                    }
                })
                .run(move |args, _| {
                    let c = c_run.clone();
                    async move {
                        let r = s(&args, "ref").ok_or_else(|| AdapterError::new("ref is required."))?;
                        let pos = assert_positional(&r, "template id or name")?;
                        let a = vec!["template".to_string(), pos];
                        let cmd = client::spark_command(&c, &a);
                        let out = client::run_spark(&c, a).await?;
                        Ok(ActionOutput::with_command(Value::String(out), cmd))
                    }
                }),
        );
    }
    // draft
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert(
            "to".to_string(),
            json!({"type":"array","items":{"type":"string"},"description":"Recipient addresses"}),
        );
        props.insert(
            "cc".to_string(),
            json!({"type":"array","items":{"type":"string"}}),
        );
        props.insert(
            "bcc".to_string(),
            json!({"type":"array","items":{"type":"string"}}),
        );
        props.insert("subject".to_string(), prop_opt_str("Subject"));
        props.insert(
            "body".to_string(),
            prop_opt_str(
                "Body in markdown; required for a new draft unless a template supplies one",
            ),
        );
        props.insert("account".to_string(), prop_opt_str("From address; the integration's account when omitted, and refused when it names a different one"));
        props.insert(
            "edit".to_string(),
            prop_opt_str("Message id of an existing draft to update"),
        );
        props.insert(
            "reply_to".to_string(),
            prop_opt_str("Message id to reply to — required to stay in an existing thread"),
        );
        props.insert("forward".to_string(), prop_opt_str("Message id to forward"));
        props.insert("attach".to_string(), json!({"type":"array","items":{"type":"string"},"description":"Absolute paths Spark can read; max 25 MB each"}));
        props.insert(
            "template".to_string(),
            prop_opt_str("Template id or name to apply"),
        );
        props.insert("placeholder".to_string(), json!({"type":"array","items":{"type":"string"},"description":"Fill a manual template placeholder: \"name=value\""}));
        props.insert(
            "no_signature".to_string(),
            json!({"type":"boolean","description":"Drop the account's default signature"}),
        );
        tools.push(
            ActionTool::new("draft", "Create or edit an email draft (body in markdown). Never sends. Replying to an existing conversation? Pass reply_to with the thread's last message id, or the draft starts a new thread.", ActionCategory::Write)
                .schema(props)
                .detail_fn(|args| {
                    if let Some(e) = s(args, "edit") { format!("draft edit {e}") }
                    else if let Some(r) = s(args, "reply_to") { format!("draft reply {r}") }
                    else if let Some(f) = s(args, "forward") { format!("draft forward {f}") }
                    else { format!("draft {}", arr_str(args, "to").join(",")) }
                })
                .command_fn(move |args, _| {
                    let c = &c_cmd;
                    let mut a = vec!["draft".to_string()];
                    flag_each(&mut a, "--to", &arr_str(args, "to"));
                    flag_each(&mut a, "--cc", &arr_str(args, "cc"));
                    flag_each(&mut a, "--bcc", &arr_str(args, "bcc"));
                    flag(&mut a, "--subject", s(args, "subject").as_deref());
                    flag(&mut a, "--body", s(args, "body").as_deref());
                    match same_account(c, s(args, "account").as_deref(), "from address") {
                        Ok(v) => flag(&mut a, "--account", if v.is_empty() { None } else { Some(v.as_str()) }),
                        Err(e) => return format!("draft error: {}", e.message),
                    }
                    if let Some(edit) = s(args, "edit") {
                        match assert_message_id(&edit, "draft id") {
                            Ok(v) => flag(&mut a, "--edit", Some(v.as_str())),
                            Err(e) => return format!("draft error: {}", e.message),
                        }
                    }
                    if let Some(rt) = s(args, "reply_to") {
                        match assert_message_id(&rt, "message id") {
                            Ok(v) => flag(&mut a, "--reply-to", Some(v.as_str())),
                            Err(e) => return format!("draft error: {}", e.message),
                        }
                    }
                    if let Some(fw) = s(args, "forward") {
                        match assert_message_id(&fw, "message id") {
                            Ok(v) => flag(&mut a, "--forward", Some(v.as_str())),
                            Err(e) => return format!("draft error: {}", e.message),
                        }
                    }
                    flag_each(&mut a, "--attach", &arr_str(args, "attach"));
                    flag(&mut a, "--template", s(args, "template").as_deref());
                    flag_each(&mut a, "--placeholder", &arr_str(args, "placeholder"));
                    toggle(&mut a, "--no-signature", b_true(args, "no_signature"));
                    client::spark_command(c, &a)
                })
                .run(move |args, _| {
                    let c = c_run.clone();
                    async move {
                        let mut a = vec!["draft".to_string()];
                        flag_each(&mut a, "--to", &arr_str(&args, "to"));
                        flag_each(&mut a, "--cc", &arr_str(&args, "cc"));
                        flag_each(&mut a, "--bcc", &arr_str(&args, "bcc"));
                        flag(&mut a, "--subject", s(&args, "subject").as_deref());
                        flag(&mut a, "--body", s(&args, "body").as_deref());
                        let acct = same_account(&c, s(&args, "account").as_deref(), "from address")?;
                        flag(&mut a, "--account", if acct.is_empty() { None } else { Some(acct.as_str()) });
                        if let Some(edit) = s(&args, "edit") {
                            let v = assert_message_id(&edit, "draft id")?;
                            flag(&mut a, "--edit", Some(v.as_str()));
                        }
                        if let Some(rt) = s(&args, "reply_to") {
                            let v = assert_message_id(&rt, "message id")?;
                            flag(&mut a, "--reply-to", Some(v.as_str()));
                        }
                        if let Some(fw) = s(&args, "forward") {
                            let v = assert_message_id(&fw, "message id")?;
                            flag(&mut a, "--forward", Some(v.as_str()));
                        }
                        flag_each(&mut a, "--attach", &arr_str(&args, "attach"));
                        flag(&mut a, "--template", s(&args, "template").as_deref());
                        flag_each(&mut a, "--placeholder", &arr_str(&args, "placeholder"));
                        toggle(&mut a, "--no-signature", b_true(&args, "no_signature"));
                        let cmd = client::spark_command(&c, &a);
                        let out = client::run_spark(&c, a).await?;
                        Ok(ActionOutput::with_command(Value::String(out), cmd))
                    }
                }),
        );
    }
    // comment
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert(
            "message_id".to_string(),
            prop_opt_str("A message in the thread to comment on"),
        );
        props.insert("body".to_string(), prop_str("Comment text"));
        props.insert(
            "team".to_string(),
            prop_opt_str("Team name; the integration's default team when omitted"),
        );
        props.insert("user".to_string(), json!({"type":"array","items":{"type":"string"},"description":"Teammates to share with when the thread isn't shared yet"}));
        props.insert(
            "edit".to_string(),
            prop_opt_str("Message id of an existing comment to edit instead"),
        );
        tools.push(
            ActionTool::new("comment", "Post a team comment on a thread, sharing the thread with the team first when it isn't shared yet.", ActionCategory::Write)
                .schema(props)
                .detail_fn(|args| if let Some(e) = s(args, "edit") { format!("comment edit {e}") } else { format!("comment {}", s(args, "message_id").unwrap_or_default()) })
                .command_fn(move |args, _| {
                    let c = &c_cmd;
                    let mut a = vec!["comment".to_string()];
                    if s(args, "edit").is_none() {
                        match s(args, "message_id").map(|v| assert_message_id(&v, "message id")) {
                            Some(Ok(v)) => a.push(v),
                            Some(Err(e)) => return format!("comment error: {}", e.message),
                            None => return "comment error: message_id required".to_string(),
                        }
                    }
                    flag(&mut a, "--body", s(args, "body").as_deref());
                    let team_val = s(args, "team").map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).unwrap_or_else(|| c.team.clone());
                    flag(&mut a, "--team", if team_val.is_empty() { None } else { Some(team_val.as_str()) });
                    flag_each(&mut a, "--user", &arr_str(args, "user"));
                    if let Some(edit) = s(args, "edit") {
                        match assert_message_id(&edit, "comment id") {
                            Ok(v) => flag(&mut a, "--edit", Some(v.as_str())),
                            Err(e) => return format!("comment error: {}", e.message),
                        }
                    }
                    client::spark_command(c, &a)
                })
                .run(move |args, _| {
                    let c = c_run.clone();
                    async move {
                        let mut a = vec!["comment".to_string()];
                        let is_edit = s(&args, "edit").is_some();
                        if !is_edit {
                            let mid = s(&args, "message_id").ok_or_else(|| AdapterError::new("message_id is required."))?;
                            let id = assert_message_id(&mid, "message id")?;
                            a.push(id);
                        }
                        flag(&mut a, "--body", s(&args, "body").as_deref());
                        let team_val = s(&args, "team").map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).unwrap_or_else(|| c.team.clone());
                        flag(&mut a, "--team", if team_val.is_empty() { None } else { Some(team_val.as_str()) });
                        flag_each(&mut a, "--user", &arr_str(&args, "user"));
                        if let Some(edit) = s(&args, "edit") {
                            let v = assert_message_id(&edit, "comment id")?;
                            flag(&mut a, "--edit", Some(v.as_str()));
                        }
                        let cmd = client::spark_command(&c, &a);
                        let out = client::run_spark(&c, a).await?;
                        Ok(ActionOutput::with_command(Value::String(out), cmd))
                    }
                }),
        );
    }
    // email_action
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert(
            "action".to_string(),
            json!({"type":"string","enum": email_actions, "description":"The verb to apply"}),
        );
        props.insert(
            "message_ids".to_string(),
            json!({"type":"array","items":{"type":"string"},"description":"Message ids to act on"}),
        );
        props.insert(
            "date".to_string(),
            prop_opt_str("Required by snooze and changeReminder; the due date for assign"),
        );
        props.insert(
            "folder".to_string(),
            prop_opt_str("Qualified folder for moveToFolder, attachLabel and detachLabel"),
        );
        props.insert(
            "team".to_string(),
            prop_opt_str("Team for team actions; the integration's default when omitted"),
        );
        props.insert("user".to_string(), json!({"type":"array","items":{"type":"string"},"description":"Teammates for shareInTeam"}));
        props.insert(
            "assignee".to_string(),
            prop_opt_str("Teammate address for assign"),
        );
        props.insert(
            "comment".to_string(),
            prop_opt_str("Comment attached to an assign"),
        );
        tools.push(
            ActionTool::new("email_action", "Act on one or more emails: archive, pin, snooze, move, label, categorize, share, assign, mark read/unread and so on. Sending drafts is a separate tool.", ActionCategory::Write)
                .schema(props)
                .detail_fn(|args| format!("{} {}", s(args, "action").unwrap_or_default(), arr_str(args, "message_ids").join(" ")))
                .command_fn(move |args, _| {
                    let c = &c_cmd;
                    let action = s(args, "action").unwrap_or_default();
                    let ids: Result<Vec<String>, _> = arr_str(args, "message_ids").into_iter().map(|v| assert_message_id(&v, "message id")).collect();
                    let ids = match ids { Ok(v) if !v.is_empty() => v, Ok(_) => return "email_action error: At least one message id is required.".to_string(), Err(e) => return format!("email_action error: {}", e.message) };
                    let mut a = vec!["action".to_string(), action];
                    a.extend(ids);
                    flag(&mut a, "--date", s(args, "date").as_deref());
                    match s(args, "folder").map(|v| scoped(c, Some(&v), "folder")) {
                        Some(Ok(v)) => flag(&mut a, "--folder", if v.is_empty() { None } else { Some(v.as_str()) }),
                        Some(Err(e)) => return format!("email_action error: {}", e.message),
                        None => {}
                    }
                    let team_val = s(args, "team").map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).unwrap_or_else(|| c.team.clone());
                    flag(&mut a, "--team", if team_val.is_empty() { None } else { Some(team_val.as_str()) });
                    flag_each(&mut a, "--user", &arr_str(args, "user"));
                    flag(&mut a, "--assignee", s(args, "assignee").as_deref());
                    flag(&mut a, "--comment", s(args, "comment").as_deref());
                    client::spark_command(c, &a)
                })
                .run(move |args, _| {
                    let c = c_run.clone();
                    async move {
                        let action = s(&args, "action").ok_or_else(|| AdapterError::new("action is required."))?;
                        let raw_ids = arr_str(&args, "message_ids");
                        if raw_ids.is_empty() { return Err(AdapterError::new("At least one message id is required.")); }
                        let ids: Vec<String> = raw_ids.into_iter().map(|v| assert_message_id(&v, "message id")).collect::<Result<_, _>>()?;
                        let mut a = vec!["action".to_string(), action];
                        a.extend(ids);
                        flag(&mut a, "--date", s(&args, "date").as_deref());
                        if let Some(folder) = s(&args, "folder") {
                            let sc = scoped(&c, Some(&folder), "folder")?;
                            flag(&mut a, "--folder", if sc.is_empty() { None } else { Some(sc.as_str()) });
                        }
                        let team_val = s(&args, "team").map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).unwrap_or_else(|| c.team.clone());
                        flag(&mut a, "--team", if team_val.is_empty() { None } else { Some(team_val.as_str()) });
                        flag_each(&mut a, "--user", &arr_str(&args, "user"));
                        flag(&mut a, "--assignee", s(&args, "assignee").as_deref());
                        flag(&mut a, "--comment", s(&args, "comment").as_deref());
                        let cmd = client::spark_command(&c, &a);
                        let out = client::run_spark(&c, a).await?;
                        Ok(ActionOutput::with_command(Value::String(out), cmd))
                    }
                }),
        );
    }
    // contact_action
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert(
            "action".to_string(),
            json!({"type":"string","enum": contact_actions, "description":"The verb to apply"}),
        );
        props.insert("emails".to_string(), json!({"type":"array","items":{"type":"string"},"description":"Contact addresses to act on"}));
        tools.push(
            ActionTool::new("contact_action", "Act on contacts: block or accept a sender or their domain, recategorize their mail, toggle priority, notifications or auto-summary.", ActionCategory::Write)
                .schema(props)
                .detail_fn(|args| format!("{} {}", s(args, "action").unwrap_or_default(), arr_str(args, "emails").join(" ")))
                .command_fn(move |args, _| {
                    let c = &c_cmd;
                    let action = s(args, "action").unwrap_or_default();
                    let emails: Result<Vec<String>, _> = arr_str(args, "emails").into_iter().map(|v| assert_positional(&v, "contact address")).collect();
                    let emails = match emails { Ok(v) if !v.is_empty() => v, Ok(_) => return "contact_action error: At least one contact address is required.".to_string(), Err(e) => return format!("contact_action error: {}", e.message) };
                    let mut a = vec!["contact-action".to_string(), action];
                    a.extend(emails);
                    client::spark_command(c, &a)
                })
                .run(move |args, _| {
                    let c = c_run.clone();
                    async move {
                        let action = s(&args, "action").ok_or_else(|| AdapterError::new("action is required."))?;
                        let raw = arr_str(&args, "emails");
                        if raw.is_empty() { return Err(AdapterError::new("At least one contact address is required.")); }
                        let emails: Vec<String> = raw.into_iter().map(|v| assert_positional(&v, "contact address")).collect::<Result<_, _>>()?;
                        let mut a = vec!["contact-action".to_string(), action];
                        a.extend(emails);
                        let cmd = client::spark_command(&c, &a);
                        let out = client::run_spark(&c, a).await?;
                        Ok(ActionOutput::with_command(Value::String(out), cmd))
                    }
                }),
        );
    }
    // event_write
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert(
            "mode".to_string(),
            json!({"type":"string","enum":["create","update","rsvp"]}),
        );
        props.insert("event_id".to_string(), prop_opt_str("Required for update and rsvp: a calendar event id, or the invitation email's message id"));
        props.insert("status".to_string(), json!({"type":"string","enum":["accept","decline","maybe"],"description":"Required for rsvp"}));
        props.insert("title".to_string(), prop_opt_str("Title"));
        props.insert(
            "start".to_string(),
            prop_opt_str("Start date: yyyy-MM-dd, dd/MM/yyyy or yyyy-MM-ddTHH:mm"),
        );
        props.insert(
            "end".to_string(),
            prop_opt_str("End date, same formats as start"),
        );
        props.insert("all_day".to_string(), json!({"type":"boolean"}));
        props.insert("description".to_string(), prop_opt_str("Description"));
        props.insert("location".to_string(), prop_opt_str("Location"));
        props.insert("calendar".to_string(), prop_opt_str("Target calendar for create: \"you@co.com\" or \"you@co.com:Work\". The integration's account when omitted"));
        props.insert(
            "video_conference".to_string(),
            json!({"type":"string","enum":["auto","meet","zoom","teams"]}),
        );
        props.insert(
            "alerts".to_string(),
            prop_opt_str("Comma-separated offsets in seconds (300s,600s) or absolute dates"),
        );
        props.insert("add".to_string(), json!({"type":"array","items":{"type":"string"},"description":"Attendees to invite — they receive an invitation"}));
        props.insert("remove".to_string(), json!({"type":"array","items":{"type":"string"},"description":"Attendees to remove on update — they receive a cancellation"}));
        tools.push(
            ActionTool::new("event_write", "Create or update a calendar event, or RSVP to an invitation. Adding or removing attendees mails invitations and cancellations, so Spark requires send access on the account.", ActionCategory::Write)
                .schema(props)
                .detail_fn(|args| format!("event {} {}", s(args, "mode").unwrap_or_default(), s(args, "event_id").or_else(|| s(args, "title")).unwrap_or_default()).trim().to_string())
                .command_fn(move |args, _| {
                    let c = &c_cmd;
                    let mode = s(args, "mode").unwrap_or_default();
                    let mut a = vec!["event".to_string()];
                    flag(&mut a, "--title", s(args, "title").as_deref());
                    flag(&mut a, "--start", s(args, "start").as_deref());
                    flag(&mut a, "--end", s(args, "end").as_deref());
                    toggle(&mut a, "--all-day", b_true(args, "all_day"));
                    flag(&mut a, "--description", s(args, "description").as_deref());
                    flag(&mut a, "--location", s(args, "location").as_deref());
                    match scoped(c, s(args, "calendar").as_deref(), "calendar") {
                        Ok(v) => {
                            let val = if v.is_empty() { c.account.clone() } else { v };
                            flag(&mut a, "--calendar", if val.is_empty() { None } else { Some(val.as_str()) });
                        }
                        Err(e) => return format!("event_write error: {}", e.message),
                    }
                    flag(&mut a, "--video-conference", s(args, "video_conference").as_deref());
                    flag(&mut a, "--alerts", s(args, "alerts").as_deref());
                    flag_each(&mut a, "--add", &arr_str(args, "add"));
                    flag_each(&mut a, "--remove", &arr_str(args, "remove"));
                    a.push(mode.clone());
                    if mode != "create" {
                        match s(args, "event_id").map(|v| assert_positional(&v, "event id")) {
                            Some(Ok(v)) => a.push(v),
                            Some(Err(e)) => return format!("event_write error: {}", e.message),
                            None => return "event_write error: event_id required for update/rsvp".to_string(),
                        }
                    }
                    if mode == "rsvp" {
                        match s(args, "status").map(|v| assert_positional(&v, "rsvp status")) {
                            Some(Ok(v)) => a.push(v),
                            Some(Err(e)) => return format!("event_write error: {}", e.message),
                            None => return "event_write error: status required for rsvp".to_string(),
                        }
                    }
                    client::spark_command(c, &a)
                })
                .run(move |args, _| {
                    let c = c_run.clone();
                    async move {
                        let mode = s(&args, "mode").ok_or_else(|| AdapterError::new("mode is required."))?;
                        let mut a = vec!["event".to_string()];
                        flag(&mut a, "--title", s(&args, "title").as_deref());
                        flag(&mut a, "--start", s(&args, "start").as_deref());
                        flag(&mut a, "--end", s(&args, "end").as_deref());
                        toggle(&mut a, "--all-day", b_true(&args, "all_day"));
                        flag(&mut a, "--description", s(&args, "description").as_deref());
                        flag(&mut a, "--location", s(&args, "location").as_deref());
                        let cal_raw = s(&args, "calendar");
                        let cal = scoped(&c, cal_raw.as_deref(), "calendar")?;
                        let cal_val = if cal.is_empty() { c.account.clone() } else { cal };
                        flag(&mut a, "--calendar", if cal_val.is_empty() { None } else { Some(cal_val.as_str()) });
                        flag(&mut a, "--video-conference", s(&args, "video_conference").as_deref());
                        flag(&mut a, "--alerts", s(&args, "alerts").as_deref());
                        flag_each(&mut a, "--add", &arr_str(&args, "add"));
                        flag_each(&mut a, "--remove", &arr_str(&args, "remove"));
                        a.push(mode.clone());
                        if mode != "create" {
                            let eid = s(&args, "event_id").ok_or_else(|| AdapterError::new("event_id is required for update/rsvp."))?;
                            let pos = assert_positional(&eid, "event id")?;
                            a.push(pos);
                        }
                        if mode == "rsvp" {
                            let st = s(&args, "status").ok_or_else(|| AdapterError::new("status is required for rsvp."))?;
                            let pos = assert_positional(&st, "rsvp status")?;
                            a.push(pos);
                        }
                        let cmd = client::spark_command(&c, &a);
                        let out = client::run_spark(&c, a).await?;
                        Ok(ActionOutput::with_command(Value::String(out), cmd))
                    }
                }),
        );
    }
    // delete_event
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert(
            "event_id".to_string(),
            prop_str("Calendar event id from list_events"),
        );
        tools.push(
            ActionTool::new(
                "delete_event",
                "Delete a calendar event. An event with attendees also mails them a cancellation.",
                ActionCategory::Delete,
            )
            .schema(props)
            .detail_fn(|args| format!("delete_event {}", s(args, "event_id").unwrap_or_default()))
            .command_fn(move |args, _| {
                let c = &c_cmd;
                match s(args, "event_id").map(|v| assert_positional(&v, "event id")) {
                    Some(Ok(v)) => {
                        client::spark_command(c, &["event".to_string(), "delete".to_string(), v])
                    }
                    Some(Err(e)) => format!("delete_event error: {}", e.message),
                    None => "delete_event error: event_id required".to_string(),
                }
            })
            .run(move |args, _| {
                let c = c_run.clone();
                async move {
                    let eid = s(&args, "event_id")
                        .ok_or_else(|| AdapterError::new("event_id is required."))?;
                    let pos = assert_positional(&eid, "event id")?;
                    let a = vec!["event".to_string(), "delete".to_string(), pos.clone()];
                    let cmd = client::spark_command(&c, &a);
                    let out = client::run_spark(&c, a).await?;
                    Ok(ActionOutput::with_command(Value::String(out), cmd))
                }
            }),
        );
    }
    // send_draft
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert(
            "message_ids".to_string(),
            json!({"type":"array","items":{"type":"string"},"description":"Draft ids to send"}),
        );
        props.insert(
            "date".to_string(),
            prop_opt_str("Future date to schedule for (Send Later); sends now when omitted"),
        );
        tools.push(
            ActionTool::new("send_draft", "Send an existing draft now, or schedule it with a future date. The only tool here that emits mail.", ActionCategory::Admin)
                .schema(props)
                .detail_fn(|args| {
                    let ids = arr_str(args, "message_ids").join(" ");
                    let date = s(args, "date").map(|d| format!(" at {d}")).unwrap_or_default();
                    format!("send_draft {ids}{date}")
                })
                .command_fn(move |args, _| {
                    let c = &c_cmd;
                    let ids: Result<Vec<String>, _> = arr_str(args, "message_ids").into_iter().map(|v| assert_message_id(&v, "draft id")).collect();
                    let ids = match ids { Ok(v) if !v.is_empty() => v, Ok(_) => return "send_draft error: At least one draft id is required.".to_string(), Err(e) => return format!("send_draft error: {}", e.message) };
                    let mut a = vec!["action".to_string(), "send".to_string()];
                    a.extend(ids);
                    flag(&mut a, "--date", s(args, "date").as_deref());
                    client::spark_command(c, &a)
                })
                .run(move |args, _| {
                    let c = c_run.clone();
                    async move {
                        let raw = arr_str(&args, "message_ids");
                        if raw.is_empty() { return Err(AdapterError::new("At least one draft id is required.")); }
                        let ids: Vec<String> = raw.into_iter().map(|v| assert_message_id(&v, "draft id")).collect::<Result<_, _>>()?;
                        let mut a = vec!["action".to_string(), "send".to_string()];
                        a.extend(ids);
                        flag(&mut a, "--date", s(&args, "date").as_deref());
                        let cmd = client::spark_command(&c, &a);
                        let out = client::run_spark(&c, a).await?;
                        Ok(ActionOutput::with_command(Value::String(out), cmd))
                    }
                }),
        );
    }
    // unschedule_draft
    {
        let c = cfg.clone();
        let c_cmd = c.clone();
        let c_run = c.clone();
        let mut props = Map::new();
        props.insert(
            "message_ids".to_string(),
            json!({"type":"array","items":{"type":"string"},"description":"Scheduled message ids"}),
        );
        tools.push(
            ActionTool::new("unschedule_draft", "Cancel a scheduled send and return the message to drafts. Spark mints a new draft id, so re-list Drafts afterwards.", ActionCategory::Admin)
                .schema(props)
                .detail_fn(|args| format!("unschedule_draft {}", arr_str(args, "message_ids").join(" ")))
                .command_fn(move |args, _| {
                    let c = &c_cmd;
                    let ids: Result<Vec<String>, _> = arr_str(args, "message_ids").into_iter().map(|v| assert_message_id(&v, "message id")).collect();
                    let ids = match ids { Ok(v) if !v.is_empty() => v, Ok(_) => return "unschedule_draft error: At least one message id is required.".to_string(), Err(e) => return format!("unschedule_draft error: {}", e.message) };
                    let mut a = vec!["action".to_string(), "unschedule".to_string()];
                    a.extend(ids);
                    client::spark_command(c, &a)
                })
                .run(move |args, _| {
                    let c = c_run.clone();
                    async move {
                        let raw = arr_str(&args, "message_ids");
                        if raw.is_empty() { return Err(AdapterError::new("At least one message id is required.")); }
                        let ids: Vec<String> = raw.into_iter().map(|v| assert_message_id(&v, "message id")).collect::<Result<_, _>>()?;
                        let mut a = vec!["action".to_string(), "unschedule".to_string()];
                        a.extend(ids);
                        let cmd = client::spark_command(&c, &a);
                        let out = client::run_spark(&c, a).await?;
                        Ok(ActionOutput::with_command(Value::String(out), cmd))
                    }
                }),
        );
    }

    tools
}

pub fn spark_adapter_spec() -> ActionAdapterSpec<SparkCfg> {
    ActionAdapterSpec::new("spark", "Spark Mail", "email")
        .agent_hint(AGENT_HINT)
        .access(ACCESS)
        .start("accounts")
        .config_fields(spark_fields())
        .client(|conn, _| Ok(spark_config(conn)))
        .test_connection(|conn| {
            let conn = conn.clone();
            async move { client::test_spark(&conn).await }
        })
        .tools(|_, cfg| spark_tools(cfg.clone()))
        .humanize_error(client::humanize_spark_error)
}

pub fn spark_adapter(store: Arc<pluk_store::Store>) -> Arc<ActionAdapter<SparkCfg>> {
    Arc::new(crate::action::action_adapter(spark_adapter_spec(), store))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::Adapter;
    use serde_json::json;

    fn test_store() -> std::sync::Arc<pluk_store::Store> {
        let dir = tempfile::tempdir().unwrap();
        // leak dir to keep DB alive for test duration; store holds path, not dir
        let path = dir.path().join("pluk.db");
        let store = std::sync::Arc::new(pluk_store::Store::open(&path).unwrap());
        // keep dir alive by forgetting - not ideal but prevents drop; replace with Box::leak
        std::mem::forget(dir);
        store
    }

    #[test]
    fn tool_defaults_gated_correctly() {
        let store = test_store();
        let adapter = spark_adapter(store);
        let specs = adapter.tool_specs();
        let find = |name: &str| specs.iter().find(|s| s.name == name).unwrap();
        for name in [
            "accounts",
            "folders",
            "list_emails",
            "search_emails",
            "read_thread",
            "read_attachment",
            "list_events",
            "availability",
            "find_contacts",
            "team_info",
            "list_meetings",
            "read_meeting",
            "list_templates",
            "read_template",
        ] {
            assert!(find(name).default_enabled, "{name} must default on");
        }
        for name in [
            "draft",
            "comment",
            "email_action",
            "contact_action",
            "event_write",
        ] {
            assert!(!find(name).default_enabled, "{name} must default off");
        }
        assert!(!find("delete_event").default_enabled, "delete_event off");
        assert!(!find("send_draft").default_enabled, "send_draft off");
        assert!(
            !find("unschedule_draft").default_enabled,
            "unschedule_draft off"
        );
    }

    #[test]
    fn no_tool_exposes_only_param() {
        let cfg = SparkCfg {
            bin: "/usr/local/bin/spark".to_string(),
            account: String::new(),
            folder: String::new(),
            team: String::new(),
            max_page_size: 25,
            timeout_ms: 5000,
        };
        for tool in spark_tools(cfg) {
            if let Some(schema) = tool.schema {
                assert!(
                    !schema.contains_key("only"),
                    "tool {} must not expose only",
                    tool.name
                );
            }
        }
    }

    #[test]
    fn account_scope_refusal_in_tool_command() {
        let cfg = SparkCfg {
            bin: "/usr/local/bin/spark".to_string(),
            account: "you@co.com".to_string(),
            folder: String::new(),
            team: String::new(),
            max_page_size: 25,
            timeout_ms: 5000,
        };
        let tools = spark_tools(cfg);
        let folders = tools.iter().find(|t| t.name == "folders").unwrap();
        let cmd = (folders.command.as_ref().unwrap())(
            &json!({"accounts":["other@co.com"]}),
            &serde_json::Map::new(),
        );
        assert!(
            cmd.contains("another mailbox") || cmd.contains("error"),
            "got: {cmd}"
        );
    }

    #[tokio::test]
    async fn verbatim_passthrough_via_mock_runner() {
        let _g = client::RUNNER_LOCK.lock().await;
        client::set_spark_runner(None);
        let cfg = SparkCfg {
            bin: "/usr/local/bin/spark".to_string(),
            account: String::new(),
            folder: String::new(),
            team: String::new(),
            max_page_size: 25,
            timeout_ms: 5000,
        };
        let runner: client::SparkRunner = std::sync::Arc::new(|_, args, _| {
            let out = format!("MOCK OUTPUT for {}", args.join(" "));
            Box::pin(async move {
                Ok(client::SparkRunResult {
                    code: 0,
                    stdout: out,
                    stderr: String::new(),
                })
            })
        });
        client::set_spark_runner(Some(runner));
        let tools = spark_tools(cfg.clone());
        let accounts = tools.iter().find(|t| t.name == "accounts").unwrap();
        let out = (accounts.run)(json!({}), serde_json::Map::new())
            .await
            .unwrap();
        match out {
            crate::action::ActionOutput::Value(v)
            | crate::action::ActionOutput::WithCommand { value: v, .. } => {
                assert_eq!(v, json!("MOCK OUTPUT for accounts"));
            }
        }
        client::set_spark_runner(None);
    }

    #[test]
    fn spark_fields_present() {
        let fields = spark_fields();
        assert!(fields.iter().any(|f| f.key == "spark_bin"));
        assert!(fields.iter().any(|f| f.key == "default_account"));
    }
}
