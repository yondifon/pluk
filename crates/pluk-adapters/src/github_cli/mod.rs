pub mod client;
pub mod fields;

use std::sync::Arc;

use serde_json::{json, Map, Value};

use pluk_policy::ActionCategory;
use pluk_store::Integration;

use crate::action::{ActionAdapter, ActionAdapterSpec, ActionOutput, ActionTool};
use crate::error::AdapterError;
use crate::projection::{apply_only, only_param_description, FieldMap, Preset};
use crate::tool_host::ToolHost;

pub use client::{gh_command, gh_config, gh_cwd, gh_text, gh_json, humanize_gh_error, positional, repo_flag, resolve_repo, run_gh, set_gh_runner, test_gh, GhConfig, GhRunResult, GhRunner};
pub use fields::github_cli_fields;

const AGENT_HINT: &str = "Use this for GitHub work through the installed gh CLI — issues, pull requests, releases, code search, file contents at a ref, and CI status. gh uses your own login and infers the repository and branch from the cwd you pass (e.g. a git worktree). Start with list_pull_requests or list_issues; set default_repo to skip the repo arg.";

fn json_fields(map: &FieldMap) -> String {
    map.fields.join(",")
}

fn prop_opt_string(desc: &str) -> Value {
    json!({"type":"string","description":desc})
}

fn issue_list_map() -> FieldMap {
    FieldMap::new(&["number","title","state","labels","author","createdAt","updatedAt"], &["number","title","state","labels"])
        .with_preset("authorship", Preset::paths(&["author","createdAt","updatedAt"]))
}
fn issue_map() -> FieldMap {
    FieldMap::new(&["number","title","body","state","labels","author","comments","createdAt","updatedAt"], &["number","title","body","state","comments"])
        .with_preset("metadata", Preset::paths(&["author","createdAt","updatedAt","labels"]))
}
fn pr_list_map() -> FieldMap {
    FieldMap::new(&["number","title","state","headRefName","baseRefName","author","createdAt","updatedAt"], &["number","title","state","headRefName","baseRefName"])
        .with_preset("authorship", Preset::paths(&["author","createdAt","updatedAt"]))
}
fn pr_map() -> FieldMap {
    FieldMap::new(&["number","title","body","state","headRefName","baseRefName","mergeable","author","createdAt","updatedAt"], &["number","title","body","state","mergeable"])
        .with_preset("branch", Preset::paths(&["headRefName","baseRefName"]))
        .with_preset("metadata", Preset::paths(&["author","createdAt","updatedAt"]))
}
fn file_map() -> FieldMap {
    FieldMap::new(&["name","path","sha","size","url","html_url","git_url","download_url","type","content","encoding"], &["path","content","encoding"])
        .with_preset("metadata", Preset::paths(&["name","path","sha","size","html_url","type"]))
}
fn files_map() -> FieldMap {
    FieldMap::new(&["sha","filename","status","additions","deletions","changes","blob_url","raw_url","contents_url","patch"], &["filename","status","additions","deletions","patch"])
        .with_preset("links", Preset::paths(&["blob_url","raw_url","contents_url"]))
}
fn search_map() -> FieldMap {
    FieldMap::new(&["name","path","sha","url","html_url","repository","score","text_matches"], &["name","path","repository","html_url"])
        .with_preset("ranking", Preset::paths(&["score","sha"]))
        .with_preset("matches", Preset::paths(&["text_matches"]))
}
fn status_map() -> FieldMap {
    FieldMap::new(&["status","check_runs"], &["status.state","status.total_count","check_runs.name","check_runs.status","check_runs.conclusion"])
        .with_preset("links", Preset::paths(&["check_runs.html_url"]))
}
fn repo_map() -> FieldMap {
    FieldMap::new(&["name","owner","description","url","defaultBranchRef","isPrivate","pushedAt","stargazerCount","forkCount"], &["name","owner.login","description","url","defaultBranchRef.name","isPrivate"])
        .with_preset("stats", Preset::paths(&["pushedAt","stargazerCount","forkCount"]))
}
fn release_list_map() -> FieldMap {
    FieldMap::new(&["tagName","name","isDraft","isPrerelease","author","publishedAt"], &["tagName","name","isDraft","isPrerelease"])
        .with_preset("authorship", Preset::paths(&["author","publishedAt"]))
}
fn release_map() -> FieldMap {
    FieldMap::new(&["tagName","name","body","isDraft","isPrerelease","author","createdAt","publishedAt","assets","url"], &["tagName","name","body","isDraft","isPrerelease","assets"])
        .with_preset("metadata", Preset::paths(&["author","createdAt","publishedAt","url"]))
}

fn cwd_prop() -> (String, Value) {
    ("cwd".to_string(), prop_opt_string("Working directory (e.g. a git worktree) to run gh in. Defaults to the integration's default working directory."))
}
fn repo_prop() -> (String, Value) {
    ("repo".to_string(), prop_opt_string("Repo as owner/repo. Defaults to the integration's default_repo; otherwise gh infers it from the cwd."))
}
fn limit_prop() -> (String, Value) {
    ("limit".to_string(), json!({"type":"integer","description":"Max results to return","default":30}))
}
fn only_prop(presets: &[&str]) -> (String, Value) {
    ("only".to_string(), json!({"type":"array","items":{"type":"string"},"description": only_param_description(presets)}))
}

fn extract_str(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}
fn extract_i64(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|n| n as i64)))
}
fn extract_bool(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(|v| v.as_bool())
}
fn extract_only(args: &Value) -> Option<Vec<String>> {
    args.get("only").and_then(|v| v.as_array()).map(|arr| arr.iter().filter_map(|e| e.as_str().map(|s| s.to_string())).collect())
}

fn project(data: Value, only: Option<Vec<String>>, map: &FieldMap) -> Result<Value, AdapterError> {
    let only_ref = only.as_ref();
    apply_only(&data, only_ref, map).map_err(|e| AdapterError::new(e.to_string()))
}


pub fn github_cli_tools(cfg: GhConfig) -> Vec<ActionTool> {
    let mut tools: Vec<ActionTool> = Vec::new();

    // list_issues
    {
        let cfg_tool = cfg.clone();
        let cfg_detail = cfg_tool.clone();
        let cfg_cmd = cfg_tool.clone();
        let cfg_run = cfg_tool.clone();
        let map = issue_list_map();
        let map_cmd = map.clone();
        let fields_str = json_fields(&map_cmd);
        let fields_cmd = fields_str.clone();
        let fields_run = fields_str.clone();
        let mut props = Map::new();
        props.insert(cwd_prop().0, cwd_prop().1);
        props.insert(repo_prop().0, repo_prop().1);
        props.insert("state".to_string(), json!({"type":"string","enum":["open","closed","all"],"description":"Issue state","default":"open"}));
        props.insert(limit_prop().0, limit_prop().1);
        props.insert(only_prop(&["authorship"]).0, only_prop(&["authorship"]).1);
        tools.push(
            ActionTool::new("list_issues", "List issues in a repo, newest first (excludes pull requests).", ActionCategory::Read)
                .schema(props)
                .detail_fn(move |args| {
                    let repo = extract_str(args, "repo").unwrap_or_else(|| cfg_detail.default_repo.clone().unwrap_or("?".to_string()));
                    let state = extract_str(args, "state").unwrap_or("open".to_string());
                    format!("list_issues {repo} state={state}")
                })
                .command_fn({
                    let c = cfg_cmd.clone();
                    let f = fields_cmd.clone();
                    move |args, _| {
                        let repo = extract_str(args, "repo");
                        let state = extract_str(args, "state").unwrap_or("open".to_string());
                        let limit = extract_i64(args, "limit").unwrap_or(30);
                        let mut a = vec!["issue".to_string(), "list".to_string()];
                        a.extend(repo_flag(&c, repo.as_deref()));
                        a.extend(vec!["--state".to_string(), state, "--limit".to_string(), limit.to_string(), "--json".to_string(), f.clone()]);
                        gh_command(&c, &a)
                    }
                })
                .run({
                    let c = cfg_run.clone();
                    let m = map.clone();
                    let f = fields_run.clone();
                    move |args, _| {
                        let c = c.clone();
                        let m = m.clone();
                        let f = f.clone();
                        async move {
                            let repo = extract_str(&args, "repo");
                            let state = extract_str(&args, "state").unwrap_or("open".to_string());
                            let limit = extract_i64(&args, "limit").unwrap_or(30);
                            let cwd = extract_str(&args, "cwd");
                            let only = extract_only(&args);
                            let mut a = vec!["issue".to_string(), "list".to_string()];
                            a.extend(repo_flag(&c, repo.as_deref()));
                            a.extend(vec!["--state".to_string(), state, "--limit".to_string(), limit.to_string(), "--json".to_string(), f.clone()]);
                            let cmd = gh_command(&c, &a);
                            let data = gh_json(&c, a, cwd.as_deref()).await?;
                            let projected = project(data, only, &m)?;
                            let out = ActionOutput::with_command(projected, cmd);
                            Ok(out)
                        }
                    }
                })
        );
    }
    // get_issue
    {
        let cfg_tool = cfg.clone();
        let cfg_detail = cfg_tool.clone();
        let cfg_cmd = cfg_tool.clone();
        let cfg_run = cfg_tool.clone();
        let map = issue_map();
        let fields_str = json_fields(&map);
        let fields_cmd = fields_str.clone();
        let fields_run = fields_str.clone();
        let mut props = Map::new();
        props.insert(cwd_prop().0, cwd_prop().1);
        props.insert(repo_prop().0, repo_prop().1);
        props.insert("number".to_string(), json!({"type":"integer","description":"Issue number"}));
        props.insert(only_prop(&["metadata"]).0, only_prop(&["metadata"]).1);
        tools.push(
            ActionTool::new("get_issue", "Get a single issue by number, with its comments.", ActionCategory::Read)
                .schema(props)
                .detail_fn(move |args| {
                    let repo = extract_str(args, "repo").unwrap_or_else(|| cfg_detail.default_repo.clone().unwrap_or("?".to_string()));
                    let num = extract_i64(args, "number").unwrap_or(0);
                    format!("get_issue {repo}#{num}")
                })
                .command_fn({
                    let c = cfg_cmd.clone();
                    let f = fields_cmd.clone();
                    move |args, _| {
                        let repo = extract_str(args, "repo");
                        let num = extract_i64(args, "number").unwrap_or(0);
                        let mut a = vec!["issue".to_string(), "view".to_string(), num.to_string()];
                        a.extend(repo_flag(&c, repo.as_deref()));
                        a.extend(vec!["--json".to_string(), f.clone()]);
                        gh_command(&c, &a)
                    }
                })
                .run({
                    let c = cfg_run.clone();
                    let m = map.clone();
                    let f = fields_run.clone();
                    move |args, _| {
                        let c = c.clone();
                        let m = m.clone();
                        let f = f.clone();
                        async move {
                            let repo = extract_str(&args, "repo");
                            let num = extract_i64(&args, "number").unwrap_or(0);
                            let cwd = extract_str(&args, "cwd");
                            let only = extract_only(&args);
                            let mut a = vec!["issue".to_string(), "view".to_string(), num.to_string()];
                            a.extend(repo_flag(&c, repo.as_deref()));
                            a.extend(vec!["--json".to_string(), f.clone()]);
                            let cmd = gh_command(&c, &a);
                            let data = gh_json(&c, a, cwd.as_deref()).await?;
                            let projected = project(data, only, &m)?;
                            let out = ActionOutput::with_command(projected, cmd);
                            Ok(out)
                        }
                    }
                })
        );
    }
    // list_pull_requests
    {
        let cfg_tool = cfg.clone();
        let cfg_detail = cfg_tool.clone();
        let cfg_cmd = cfg_tool.clone();
        let cfg_run = cfg_tool.clone();
        let map = pr_list_map();
        let fields_str = json_fields(&map);
        let fields_cmd = fields_str.clone();
        let fields_run = fields_str.clone();
        let mut props = Map::new();
        props.insert(cwd_prop().0, cwd_prop().1);
        props.insert(repo_prop().0, repo_prop().1);
        props.insert("state".to_string(), json!({"type":"string","enum":["open","closed","all"],"description":"PR state","default":"open"}));
        props.insert(limit_prop().0, limit_prop().1);
        props.insert(only_prop(&["authorship"]).0, only_prop(&["authorship"]).1);
        tools.push(
            ActionTool::new("list_pull_requests", "List pull requests in a repo.", ActionCategory::Read)
                .schema(props)
                .detail_fn(move |args| {
                    let repo = extract_str(args, "repo").unwrap_or_else(|| cfg_detail.default_repo.clone().unwrap_or("?".to_string()));
                    let state = extract_str(args, "state").unwrap_or("open".to_string());
                    format!("list_pull_requests {repo} state={state}")
                })
                .command_fn({
                    let c = cfg_cmd.clone();
                    let f = fields_cmd.clone();
                    move |args, _| {
                        let repo = extract_str(args, "repo");
                        let state = extract_str(args, "state").unwrap_or("open".to_string());
                        let limit = extract_i64(args, "limit").unwrap_or(30);
                        let mut a = vec!["pr".to_string(), "list".to_string()];
                        a.extend(repo_flag(&c, repo.as_deref()));
                        a.extend(vec!["--state".to_string(), state, "--limit".to_string(), limit.to_string(), "--json".to_string(), f.clone()]);
                        gh_command(&c, &a)
                    }
                })
                .run({
                    let c = cfg_run.clone();
                    let m = map.clone();
                    let f = fields_run.clone();
                    move |args, _| {
                        let c = c.clone();
                        let m = m.clone();
                        let f = f.clone();
                        async move {
                            let repo = extract_str(&args, "repo");
                            let state = extract_str(&args, "state").unwrap_or("open".to_string());
                            let limit = extract_i64(&args, "limit").unwrap_or(30);
                            let cwd = extract_str(&args, "cwd");
                            let only = extract_only(&args);
                            let mut a = vec!["pr".to_string(), "list".to_string()];
                            a.extend(repo_flag(&c, repo.as_deref()));
                            a.extend(vec!["--state".to_string(), state, "--limit".to_string(), limit.to_string(), "--json".to_string(), f.clone()]);
                            let cmd = gh_command(&c, &a);
                            let data = gh_json(&c, a, cwd.as_deref()).await?;
                            let projected = project(data, only, &m)?;
                            let out = ActionOutput::with_command(projected, cmd);
                            Ok(out)
                        }
                    }
                })
        );
    }
    // get_pull_request
    {
        let cfg_tool = cfg.clone();
        let cfg_detail = cfg_tool.clone();
        let cfg_cmd = cfg_tool.clone();
        let cfg_run = cfg_tool.clone();
        let map = pr_map();
        let fields_str = json_fields(&map);
        let fields_cmd = fields_str.clone();
        let fields_run = fields_str.clone();
        let mut props = Map::new();
        props.insert(cwd_prop().0, cwd_prop().1);
        props.insert(repo_prop().0, repo_prop().1);
        props.insert("number".to_string(), json!({"type":"integer","description":"PR number"}));
        props.insert(only_prop(&["branch","metadata"]).0, only_prop(&["branch","metadata"]).1);
        tools.push(
            ActionTool::new("get_pull_request", "Get a single pull request by number (title, body, state, mergeability).", ActionCategory::Read)
                .schema(props)
                .detail_fn(move |args| {
                    let repo = extract_str(args, "repo").unwrap_or_else(|| cfg_detail.default_repo.clone().unwrap_or("?".to_string()));
                    let n = extract_i64(args, "number").unwrap_or(0);
                    format!("get_pull_request {repo}#{n}")
                })
                .command_fn({
                    let c = cfg_cmd.clone();
                    let f = fields_cmd.clone();
                    move |args, _| {
                        let repo = extract_str(args, "repo");
                        let n = extract_i64(args, "number").unwrap_or(0);
                        let mut a = vec!["pr".to_string(), "view".to_string(), n.to_string()];
                        a.extend(repo_flag(&c, repo.as_deref()));
                        a.extend(vec!["--json".to_string(), f.clone()]);
                        gh_command(&c, &a)
                    }
                })
                .run({
                    let c = cfg_run.clone();
                    let m = map.clone();
                    let f = fields_run.clone();
                    move |args, _| {
                        let c = c.clone();
                        let m = m.clone();
                        let f = f.clone();
                        async move {
                            let repo = extract_str(&args, "repo");
                            let n = extract_i64(&args, "number").unwrap_or(0);
                            let cwd = extract_str(&args, "cwd");
                            let only = extract_only(&args);
                            let mut a = vec!["pr".to_string(), "view".to_string(), n.to_string()];
                            a.extend(repo_flag(&c, repo.as_deref()));
                            a.extend(vec!["--json".to_string(), f.clone()]);
                            let cmd = gh_command(&c, &a);
                            let data = gh_json(&c, a, cwd.as_deref()).await?;
                            let projected = project(data, only, &m)?;
                            let out = ActionOutput::with_command(projected, cmd);
                            Ok(out)
                        }
                    }
                })
        );
    }
    // pr_files
    {
        let cfg_tool = cfg.clone();
        let cfg_detail = cfg_tool.clone();
        let cfg_cmd = cfg_tool.clone();
        let cfg_run = cfg_tool.clone();
        let map = files_map();
        let mut props = Map::new();
        props.insert(cwd_prop().0, cwd_prop().1);
        props.insert(repo_prop().0, repo_prop().1);
        props.insert("number".to_string(), json!({"type":"integer","description":"PR number"}));
        props.insert(limit_prop().0, limit_prop().1);
        props.insert(only_prop(&["links"]).0, only_prop(&["links"]).1);
        tools.push(
            ActionTool::new("pr_files", "List the changed files (with patches) for a pull request.", ActionCategory::Read)
                .schema(props)
                .detail_fn(move |args| {
                    let repo = extract_str(args, "repo").unwrap_or_else(|| cfg_detail.default_repo.clone().unwrap_or("?".to_string()));
                    let n = extract_i64(args, "number").unwrap_or(0);
                    format!("pr_files {repo}#{n}")
                })
                .command_fn({
                    let c = cfg_cmd.clone();
                    move |args, _| {
                        let repo = extract_str(args, "repo");
                        let n = extract_i64(args, "number").unwrap_or(0);
                        let limit = extract_i64(args, "limit").unwrap_or(30);
                        let (owner, repo_name) = resolve_repo(&c, repo.as_deref()).unwrap_or(("?".to_string(), "?".to_string()));
                        let url = format!("repos/{owner}/{repo_name}/pulls/{n}/files?per_page={limit}");
                        gh_command(&c, &vec!["api".to_string(), "--method".to_string(), "GET".to_string(), url])
                    }
                })
                .run({
                    let c = cfg_run.clone();
                    let m = map.clone();
                    move |args, _| {
                        let c = c.clone();
                        let m = m.clone();
                        async move {
                            let repo = extract_str(&args, "repo");
                            let n = extract_i64(&args, "number").unwrap_or(0);
                            let limit = extract_i64(&args, "limit").unwrap_or(30);
                            let cwd = extract_str(&args, "cwd");
                            let only = extract_only(&args);
                            let (owner, repo_name) = resolve_repo(&c, repo.as_deref())?;
                            let url = format!("repos/{owner}/{repo_name}/pulls/{n}/files?per_page={limit}");
                            let a = vec!["api".to_string(), "--method".to_string(), "GET".to_string(), url.clone()];
                            let cmd = gh_command(&c, &a);
                            let data = gh_json(&c, a, cwd.as_deref()).await?;
                            let projected = project(data, only, &m)?;
                            let out = ActionOutput::with_command(projected, cmd);
                            Ok(out)
                        }
                    }
                })
        );
    }
    // search_code
    {
        let cfg_tool = cfg.clone();
        let cfg_cmd = cfg_tool.clone();
        let cfg_run = cfg_tool.clone();
        let map = search_map();
        let mut props = Map::new();
        props.insert(cwd_prop().0, cwd_prop().1);
        props.insert("query".to_string(), json!({"type":"string","description":"Code search query"}));
        props.insert(limit_prop().0, limit_prop().1);
        props.insert(only_prop(&["ranking","matches"]).0, only_prop(&["ranking","matches"]).1);
        tools.push(
            ActionTool::new("search_code", "Search code with GitHub's code search syntax (e.g. 'addUser repo:owner/name').", ActionCategory::Read)
                .schema(props)
                .detail_fn(|args| {
                    let q = extract_str(args, "query").unwrap_or_default();
                    format!("search_code \"{q}\"")
                })
                .command_fn({
                    let c = cfg_cmd.clone();
                    move |args, _| {
                        let q = extract_str(args, "query").unwrap_or_default();
                        let limit = extract_i64(args, "limit").unwrap_or(30);
                        let url = format!("search/code?q={}&per_page={limit}", urlencoding::encode(&q));
                        gh_command(&c, &vec!["api".to_string(), "--method".to_string(), "GET".to_string(), url])
                    }
                })
                .run({
                    let c = cfg_run.clone();
                    let m = map.clone();
                    move |args, _| {
                        let c = c.clone();
                        let m = m.clone();
                        async move {
                            let q = extract_str(&args, "query").unwrap_or_default();
                            let limit = extract_i64(&args, "limit").unwrap_or(30);
                            let cwd = extract_str(&args, "cwd");
                            let only = extract_only(&args);
                            let url = format!("search/code?q={}&per_page={limit}", urlencoding::encode(&q));
                            let a = vec!["api".to_string(), "--method".to_string(), "GET".to_string(), url.clone()];
                            let cmd = gh_command(&c, &a);
                            let data = gh_json(&c, a, cwd.as_deref()).await?;
                            let items = data.get("items").cloned().unwrap_or_else(|| if data.is_array() { data.clone() } else { json!([]) });
                            let arr = if items.is_array() { items } else { json!([]) };
                            let projected = project(arr, only, &m)?;
                            Ok(ActionOutput::with_command(projected, cmd))
                        }
                    }
                })
        );
    }
    // get_file
    {
        let cfg_tool = cfg.clone();
        let cfg_detail = cfg_tool.clone();
        let cfg_cmd = cfg_tool.clone();
        let cfg_run = cfg_tool.clone();
        let map = file_map();
        let mut props = Map::new();
        props.insert(cwd_prop().0, cwd_prop().1);
        props.insert(repo_prop().0, repo_prop().1);
        props.insert("path".to_string(), json!({"type":"string","description":"File path within the repo"}));
        props.insert("ref".to_string(), json!({"type":"string","description":"Branch, tag, or commit sha (defaults to the default branch)"}));
        props.insert(only_prop(&["metadata"]).0, only_prop(&["metadata"]).1);
        tools.push(
            ActionTool::new("get_file", "Get a file's contents at an optional ref (branch/tag/sha).", ActionCategory::Read)
                .schema(props)
                .detail_fn(move |args| {
                    let repo = extract_str(args, "repo").unwrap_or_else(|| cfg_detail.default_repo.clone().unwrap_or("?".to_string()));
                    let path = extract_str(args, "path").unwrap_or_default();
                    format!("get_file {repo}:{path}")
                })
                .command_fn({
                    let c = cfg_cmd.clone();
                    move |args, _| {
                        let repo = extract_str(args, "repo");
                        let path = extract_str(args, "path").unwrap_or_default();
                        let r = extract_str(args, "ref");
                        let (owner, repo_name) = resolve_repo(&c, repo.as_deref()).unwrap_or(("?".to_string(), "?".to_string()));
                        let ref_q = r.map(|v| format!("?ref={}", urlencoding::encode(&v))).unwrap_or_default();
                        let url = format!("repos/{owner}/{repo_name}/contents/{}" , urlencoding::encode(&path)) + &ref_q;
                        gh_command(&c, &vec!["api".to_string(), "--method".to_string(), "GET".to_string(), url])
                    }
                })
                .run({
                    let c = cfg_run.clone();
                    let m = map.clone();
                    move |args, _| {
                        let c = c.clone();
                        let m = m.clone();
                        async move {
                            let repo = extract_str(&args, "repo");
                            let path = extract_str(&args, "path").unwrap_or_default();
                            let r = extract_str(&args, "ref");
                            let cwd = extract_str(&args, "cwd");
                            let only = extract_only(&args);
                            let (owner, repo_name) = resolve_repo(&c, repo.as_deref())?;
                            let ref_q = r.map(|v| format!("?ref={}", urlencoding::encode(&v))).unwrap_or_default();
                            let url = format!("repos/{owner}/{repo_name}/contents/{}" , urlencoding::encode(&path)) + &ref_q;
                            let a = vec!["api".to_string(), "--method".to_string(), "GET".to_string(), url.clone()];
                            let cmd = gh_command(&c, &a);
                            let data = gh_json(&c, a, cwd.as_deref()).await?;
                            let projected = project(data, only, &m)?;
                            let out = ActionOutput::with_command(projected, cmd);
                            Ok(out)
                        }
                    }
                })
        );
    }
    // commit_status
    {
        let cfg_tool = cfg.clone();
        let cfg_detail = cfg_tool.clone();
        let cfg_cmd = cfg_tool.clone();
        let cfg_run = cfg_tool.clone();
        let map = status_map();
        let mut props = Map::new();
        props.insert(cwd_prop().0, cwd_prop().1);
        props.insert(repo_prop().0, repo_prop().1);
        props.insert("ref".to_string(), json!({"type":"string","description":"Branch, tag, or commit sha"}));
        props.insert(only_prop(&["links"]).0, only_prop(&["links"]).1);
        tools.push(
            ActionTool::new("commit_status", "Get the combined commit status and check-runs for a ref (CI state).", ActionCategory::Read)
                .default_enabled(false)
                .schema(props)
                .detail_fn(move |args| {
                    let repo = extract_str(args, "repo").unwrap_or_else(|| cfg_detail.default_repo.clone().unwrap_or("?".to_string()));
                    let r = extract_str(args, "ref").unwrap_or_default();
                    format!("commit_status {repo}@{r}")
                })
                .command_fn({
                    let c = cfg_cmd.clone();
                    move |args, _| {
                        let repo = extract_str(args, "repo");
                        let r = extract_str(args, "ref").unwrap_or_default();
                        let (owner, repo_name) = resolve_repo(&c, repo.as_deref()).unwrap_or(("?".to_string(), "?".to_string()));
                        let url = format!("repos/{owner}/{repo_name}/commits/{}/status", urlencoding::encode(&r));
                        gh_command(&c, &vec!["api".to_string(), "--method".to_string(), "GET".to_string(), url])
                    }
                })
                .run({
                    let c = cfg_run.clone();
                    let m = map.clone();
                    move |args, _| {
                        let c = c.clone();
                        let m = m.clone();
                        async move {
                            let repo = extract_str(&args, "repo");
                            let r = extract_str(&args, "ref").unwrap_or_default();
                            let cwd = extract_str(&args, "cwd");
                            let only = extract_only(&args);
                            let (owner, repo_name) = resolve_repo(&c, repo.as_deref())?;
                            let enc = urlencoding::encode(&r).to_string();
                            let a1 = vec!["api".to_string(), "--method".to_string(), "GET".to_string(), format!("repos/{owner}/{repo_name}/commits/{enc}/status")];
                            let a2 = vec!["api".to_string(), "--method".to_string(), "GET".to_string(), format!("repos/{owner}/{repo_name}/commits/{enc}/check-runs")];
                            let cmd = gh_command(&c, &a1);
                            let status = gh_json(&c, a1, cwd.as_deref()).await?;
                            let checks = gh_json(&c, a2, cwd.as_deref()).await?;
                            let merged = json!({"status": status, "check_runs": checks});
                            let projected = project(merged, only, &m)?;
                            Ok(ActionOutput::with_command(projected, cmd))
                        }
                    }
                })
        );
    }
    // get_repo
    {
        let cfg_tool = cfg.clone();
        let cfg_detail = cfg_tool.clone();
        let cfg_cmd = cfg_tool.clone();
        let cfg_run = cfg_tool.clone();
        let map = repo_map();
        let fields_str = json_fields(&map);
        let fields_cmd = fields_str.clone();
        let fields_run = fields_str.clone();
        let mut props = Map::new();
        props.insert(cwd_prop().0, cwd_prop().1);
        props.insert(repo_prop().0, repo_prop().1);
        props.insert(only_prop(&["stats"]).0, only_prop(&["stats"]).1);
        tools.push(
            ActionTool::new("get_repo", "Get repository metadata (owner, description, default branch, visibility).", ActionCategory::Read)
                .schema(props)
                .detail_fn(move |args| {
                    let repo = extract_str(args, "repo").unwrap_or_else(|| cfg_detail.default_repo.clone().unwrap_or("?".to_string()));
                    format!("get_repo {repo}")
                })
                .command_fn({
                    let c = cfg_cmd.clone();
                    let f = fields_cmd.clone();
                    move |args, _| {
                        let repo = extract_str(args, "repo");
                        let mut a = vec!["repo".to_string(), "view".to_string()];
                        a.extend(repo_flag(&c, repo.as_deref()));
                        a.extend(vec!["--json".to_string(), f.clone()]);
                        gh_command(&c, &a)
                    }
                })
                .run({
                    let c = cfg_run.clone();
                    let m = map.clone();
                    let f = fields_run.clone();
                    move |args, _| {
                        let c = c.clone();
                        let m = m.clone();
                        let f = f.clone();
                        async move {
                            let repo = extract_str(&args, "repo");
                            let cwd = extract_str(&args, "cwd");
                            let only = extract_only(&args);
                            let mut a = vec!["repo".to_string(), "view".to_string()];
                            a.extend(repo_flag(&c, repo.as_deref()));
                            a.extend(vec!["--json".to_string(), f.clone()]);
                            let cmd = gh_command(&c, &a);
                            let data = gh_json(&c, a, cwd.as_deref()).await?;
                            let projected = project(data, only, &m)?;
                            let out = ActionOutput::with_command(projected, cmd);
                            Ok(out)
                        }
                    }
                })
        );
    }
    // list_releases
    {
        let cfg_tool = cfg.clone();
        let cfg_detail = cfg_tool.clone();
        let cfg_cmd = cfg_tool.clone();
        let cfg_run = cfg_tool.clone();
        let map = release_list_map();
        let fields_str = json_fields(&map);
        let fields_cmd = fields_str.clone();
        let fields_run = fields_str.clone();
        let mut props = Map::new();
        props.insert(cwd_prop().0, cwd_prop().1);
        props.insert(repo_prop().0, repo_prop().1);
        props.insert(limit_prop().0, limit_prop().1);
        props.insert(only_prop(&["authorship"]).0, only_prop(&["authorship"]).1);
        tools.push(
            ActionTool::new("list_releases", "List releases in a repo, newest first.", ActionCategory::Read)
                .schema(props)
                .detail_fn(move |args| {
                    let repo = extract_str(args, "repo").unwrap_or_else(|| cfg_detail.default_repo.clone().unwrap_or("?".to_string()));
                    format!("list_releases {repo}")
                })
                .command_fn({
                    let c = cfg_cmd.clone();
                    let f = fields_cmd.clone();
                    move |args, _| {
                        let repo = extract_str(args, "repo");
                        let limit = extract_i64(args, "limit").unwrap_or(30);
                        let mut a = vec!["release".to_string(), "list".to_string()];
                        a.extend(repo_flag(&c, repo.as_deref()));
                        a.extend(vec!["--limit".to_string(), limit.to_string(), "--json".to_string(), f.clone()]);
                        gh_command(&c, &a)
                    }
                })
                .run({
                    let c = cfg_run.clone();
                    let m = map.clone();
                    let f = fields_run.clone();
                    move |args, _| {
                        let c = c.clone();
                        let m = m.clone();
                        let f = f.clone();
                        async move {
                            let repo = extract_str(&args, "repo");
                            let limit = extract_i64(&args, "limit").unwrap_or(30);
                            let cwd = extract_str(&args, "cwd");
                            let only = extract_only(&args);
                            let mut a = vec!["release".to_string(), "list".to_string()];
                            a.extend(repo_flag(&c, repo.as_deref()));
                            a.extend(vec!["--limit".to_string(), limit.to_string(), "--json".to_string(), f.clone()]);
                            let cmd = gh_command(&c, &a);
                            let data = gh_json(&c, a, cwd.as_deref()).await?;
                            let projected = project(data, only, &m)?;
                            let out = ActionOutput::with_command(projected, cmd);
                            Ok(out)
                        }
                    }
                })
        );
    }
    // get_release
    {
        let cfg_tool = cfg.clone();
        let cfg_detail = cfg_tool.clone();
        let cfg_cmd = cfg_tool.clone();
        let cfg_run = cfg_tool.clone();
        let map = release_map();
        let fields_str = json_fields(&map);
        let fields_cmd = fields_str.clone();
        let fields_run = fields_str.clone();
        let mut props = Map::new();
        props.insert(cwd_prop().0, cwd_prop().1);
        props.insert(repo_prop().0, repo_prop().1);
        props.insert("tag".to_string(), json!({"type":"string","description":"Release tag, e.g. v1.2.3"}));
        props.insert(only_prop(&["metadata"]).0, only_prop(&["metadata"]).1);
        tools.push(
            ActionTool::new("get_release", "Get a single release by tag (body, assets, draft/prerelease state).", ActionCategory::Read)
                .schema(props)
                .detail_fn(move |args| {
                    let repo = extract_str(args, "repo").unwrap_or_else(|| cfg_detail.default_repo.clone().unwrap_or("?".to_string()));
                    let tag = extract_str(args, "tag").unwrap_or_default();
                    format!("get_release {repo}@{tag}")
                })
                .command_fn({
                    let c = cfg_cmd.clone();
                    let f = fields_cmd.clone();
                    move |args, _| {
                        let tag = extract_str(args, "tag").unwrap_or_default();
                        let pos = positional(&tag, "tag").unwrap_or(tag.clone());
                        let repo = extract_str(args, "repo");
                        let mut a = vec!["release".to_string(), "view".to_string(), pos];
                        a.extend(repo_flag(&c, repo.as_deref()));
                        a.extend(vec!["--json".to_string(), f.clone()]);
                        gh_command(&c, &a)
                    }
                })
                .run({
                    let c = cfg_run.clone();
                    let m = map.clone();
                    let f = fields_run.clone();
                    move |args, _| {
                        let c = c.clone();
                        let m = m.clone();
                        let f = f.clone();
                        async move {
                            let repo = extract_str(&args, "repo");
                            let tag = extract_str(&args, "tag").unwrap_or_default();
                            let pos = positional(&tag, "tag")?;
                            let cwd = extract_str(&args, "cwd");
                            let only = extract_only(&args);
                            let mut a = vec!["release".to_string(), "view".to_string(), pos];
                            a.extend(repo_flag(&c, repo.as_deref()));
                            a.extend(vec!["--json".to_string(), f.clone()]);
                            let cmd = gh_command(&c, &a);
                            let data = gh_json(&c, a, cwd.as_deref()).await?;
                            let projected = project(data, only, &m)?;
                            let out = ActionOutput::with_command(projected, cmd);
                            Ok(out)
                        }
                    }
                })
        );
    }
    // add_comment
    {
        let cfg_tool = cfg.clone();
        let cfg_detail = cfg_tool.clone();
        let cfg_cmd = cfg_tool.clone();
        let cfg_run = cfg_tool.clone();
        let mut props = Map::new();
        props.insert(cwd_prop().0, cwd_prop().1);
        props.insert(repo_prop().0, repo_prop().1);
        props.insert("number".to_string(), json!({"type":"integer","description":"Issue or PR number"}));
        props.insert("body".to_string(), json!({"type":"string","description":"Comment body (markdown)"}));
        tools.push(
            ActionTool::new("add_comment", "Add a comment to an issue or pull request (PRs share the issue comment endpoint).", ActionCategory::Write)
                .schema(props)
                .detail_fn(move |args| {
                    let repo = extract_str(args, "repo").unwrap_or_else(|| cfg_detail.default_repo.clone().unwrap_or("?".to_string()));
                    let n = extract_i64(args, "number").unwrap_or(0);
                    format!("add_comment {repo}#{n}")
                })
                .command_fn({
                    let c = cfg_cmd.clone();
                    move |args, _| {
                        let repo = extract_str(args, "repo");
                        let n = extract_i64(args, "number").unwrap_or(0);
                        let body = extract_str(args, "body").unwrap_or_default();
                        let mut a = vec!["issue".to_string(), "comment".to_string(), n.to_string()];
                        a.extend(repo_flag(&c, repo.as_deref()));
                        a.extend(vec!["--body".to_string(), body]);
                        gh_command(&c, &a)
                    }
                })
                .run({
                    let c = cfg_run.clone();
                    move |args, _| {
                        let c = c.clone();
                        async move {
                            let repo = extract_str(&args, "repo");
                            let n = extract_i64(&args, "number").unwrap_or(0);
                            let body = extract_str(&args, "body").unwrap_or_default();
                            let cwd = extract_str(&args, "cwd");
                            let mut a = vec!["issue".to_string(), "comment".to_string(), n.to_string()];
                            a.extend(repo_flag(&c, repo.as_deref()));
                            a.extend(vec!["--body".to_string(), body]);
                            let cmd = gh_command(&c, &a);
                            let text = gh_text(&c, a, cwd.as_deref()).await?;
                            Ok(ActionOutput::with_command(Value::String(text), cmd))
                        }
                    }
                })
        );
    }
    // create_issue
    {
        let cfg_tool = cfg.clone();
        let cfg_detail = cfg_tool.clone();
        let cfg_cmd = cfg_tool.clone();
        let cfg_run = cfg_tool.clone();
        let mut props = Map::new();
        props.insert(cwd_prop().0, cwd_prop().1);
        props.insert(repo_prop().0, repo_prop().1);
        props.insert("title".to_string(), json!({"type":"string","description":"Issue title"}));
        props.insert("body".to_string(), json!({"type":"string","description":"Issue body (markdown)"}));
        tools.push(
            ActionTool::new("create_issue", "Create a new issue.", ActionCategory::Write)
                .schema(props)
                .detail_fn(move |args| {
                    let repo = extract_str(args, "repo").unwrap_or_else(|| cfg_detail.default_repo.clone().unwrap_or("?".to_string()));
                    let title = extract_str(args, "title").unwrap_or_default();
                    format!("create_issue {repo} \"{title}\"")
                })
                .command_fn({
                    let c = cfg_cmd.clone();
                    move |args, _| {
                        let repo = extract_str(args, "repo");
                        let title = extract_str(args, "title").unwrap_or_default();
                        let body = extract_str(args, "body");
                        let mut a = vec!["issue".to_string(), "create".to_string()];
                        a.extend(repo_flag(&c, repo.as_deref()));
                        a.extend(vec!["--title".to_string(), title]);
                        if let Some(b) = body { if !b.is_empty() { a.extend(vec!["--body".to_string(), b]); } }
                        gh_command(&c, &a)
                    }
                })
                .run({
                    let c = cfg_run.clone();
                    move |args, _| {
                        let c = c.clone();
                        async move {
                            let repo = extract_str(&args, "repo");
                            let title = extract_str(&args, "title").unwrap_or_default();
                            let body = extract_str(&args, "body");
                            let cwd = extract_str(&args, "cwd");
                            let mut a = vec!["issue".to_string(), "create".to_string()];
                            a.extend(repo_flag(&c, repo.as_deref()));
                            a.extend(vec!["--title".to_string(), title]);
                            if let Some(b) = body { if !b.is_empty() { a.extend(vec!["--body".to_string(), b]); } }
                            let cmd = gh_command(&c, &a);
                            let text = gh_text(&c, a, cwd.as_deref()).await?;
                            Ok(ActionOutput::with_command(Value::String(text), cmd))
                        }
                    }
                })
        );
    }
    // create_pull_request
    {
        let cfg_tool = cfg.clone();
        let cfg_detail = cfg_tool.clone();
        let cfg_cmd = cfg_tool.clone();
        let cfg_run = cfg_tool.clone();
        let mut props = Map::new();
        props.insert(cwd_prop().0, cwd_prop().1);
        props.insert(repo_prop().0, repo_prop().1);
        props.insert("title".to_string(), json!({"type":"string","description":"PR title"}));
        props.insert("base".to_string(), json!({"type":"string","description":"Target branch (defaults to the repo's default branch)"}));
        props.insert("head".to_string(), json!({"type":"string","description":"Source branch (defaults to the current branch of cwd)"}));
        props.insert("body".to_string(), json!({"type":"string","description":"PR body (markdown)"}));
        props.insert("draft".to_string(), json!({"type":"boolean","description":"Open as a draft"}));
        tools.push(
            ActionTool::new("create_pull_request", "Open a pull request from the current branch of the given cwd (a worktree) into base; pass head/base/repo to override what gh infers.", ActionCategory::Write)
                .schema(props)
                .detail_fn(move |args| {
                    let repo = extract_str(args, "repo").unwrap_or_else(|| cfg_detail.default_repo.clone().unwrap_or("?".to_string()));
                    let head = extract_str(args, "head").unwrap_or("?".to_string());
                    let base = extract_str(args, "base").unwrap_or("default".to_string());
                    format!("create_pull_request {repo} {head}->{base}")
                })
                .command_fn({
                    let c = cfg_cmd.clone();
                    move |args, _| {
                        let repo = extract_str(args, "repo");
                        let title = extract_str(args, "title").unwrap_or_default();
                        let body = extract_str(args, "body");
                        let base = extract_str(args, "base");
                        let head = extract_str(args, "head");
                        let draft = extract_bool(args, "draft").unwrap_or(false);
                        let mut a = vec!["pr".to_string(), "create".to_string()];
                        a.extend(repo_flag(&c, repo.as_deref()));
                        a.extend(vec!["--title".to_string(), title]);
                        if let Some(b) = body { if !b.is_empty() { a.extend(vec!["--body".to_string(), b]); } }
                        if let Some(b) = base { if !b.is_empty() { a.extend(vec!["--base".to_string(), b]); } }
                        if let Some(h) = head { if !h.is_empty() { a.extend(vec!["--head".to_string(), h]); } }
                        if draft { a.push("--draft".to_string()); }
                        gh_command(&c, &a)
                    }
                })
                .run({
                    let c = cfg_run.clone();
                    move |args, _| {
                        let c = c.clone();
                        async move {
                            let repo = extract_str(&args, "repo");
                            let title = extract_str(&args, "title").unwrap_or_default();
                            let body = extract_str(&args, "body");
                            let base = extract_str(&args, "base");
                            let head = extract_str(&args, "head");
                            let draft = extract_bool(&args, "draft").unwrap_or(false);
                            let cwd = extract_str(&args, "cwd");
                            let mut a = vec!["pr".to_string(), "create".to_string()];
                            a.extend(repo_flag(&c, repo.as_deref()));
                            a.extend(vec!["--title".to_string(), title]);
                            if let Some(b) = body { if !b.is_empty() { a.extend(vec!["--body".to_string(), b]); } }
                            if let Some(b) = base { if !b.is_empty() { a.extend(vec!["--base".to_string(), b]); } }
                            if let Some(h) = head { if !h.is_empty() { a.extend(vec!["--head".to_string(), h]); } }
                            if draft { a.push("--draft".to_string()); }
                            let cmd = gh_command(&c, &a);
                            let text = gh_text(&c, a, cwd.as_deref()).await?;
                            Ok(ActionOutput::with_command(Value::String(text), cmd))
                        }
                    }
                })
        );
    }
    // review_pull_request
    {
        let cfg_tool = cfg.clone();
        let cfg_detail = cfg_tool.clone();
        let cfg_cmd = cfg_tool.clone();
        let cfg_run = cfg_tool.clone();
        let mut props = Map::new();
        props.insert(cwd_prop().0, cwd_prop().1);
        props.insert(repo_prop().0, repo_prop().1);
        props.insert("number".to_string(), json!({"type":"integer","description":"PR number"}));
        props.insert("event".to_string(), json!({"type":"string","enum":["APPROVE","COMMENT","REQUEST_CHANGES"],"description":"Review action"}));
        props.insert("body".to_string(), json!({"type":"string","description":"Review body (required for REQUEST_CHANGES/COMMENT)"}));
        tools.push(
            ActionTool::new("review_pull_request", "Submit a review on a pull request: approve, comment, or request changes.", ActionCategory::Write)
                .schema(props)
                .detail_fn(move |args| {
                    let repo = extract_str(args, "repo").unwrap_or_else(|| cfg_detail.default_repo.clone().unwrap_or("?".to_string()));
                    let n = extract_i64(args, "number").unwrap_or(0);
                    let ev = extract_str(args, "event").unwrap_or_default();
                    format!("review_pull_request {repo}#{n} {ev}")
                })
                .command_fn({
                    let c = cfg_cmd.clone();
                    move |args, _| {
                        let repo = extract_str(args, "repo");
                        let n = extract_i64(args, "number").unwrap_or(0);
                        let ev = extract_str(args, "event").unwrap_or_default();
                        let body = extract_str(args, "body");
                        let flag = match ev.as_str() { "APPROVE" => "--approve", "COMMENT" => "--comment", _ => "--request-changes" };
                        let mut a = vec!["pr".to_string(), "review".to_string(), n.to_string()];
                        a.extend(repo_flag(&c, repo.as_deref()));
                        a.extend(vec![flag.to_string()]);
                        if let Some(b) = body { if !b.is_empty() { a.extend(vec!["--body".to_string(), b]); } }
                        gh_command(&c, &a)
                    }
                })
                .run({
                    let c = cfg_run.clone();
                    move |args, _| {
                        let c = c.clone();
                        async move {
                            let repo = extract_str(&args, "repo");
                            let n = extract_i64(&args, "number").unwrap_or(0);
                            let ev = extract_str(&args, "event").unwrap_or_default();
                            let body = extract_str(&args, "body");
                            let cwd = extract_str(&args, "cwd");
                            let flag = match ev.as_str() { "APPROVE" => "--approve", "COMMENT" => "--comment", _ => "--request-changes" };
                            let mut a = vec!["pr".to_string(), "review".to_string(), n.to_string()];
                            a.extend(repo_flag(&c, repo.as_deref()));
                            a.extend(vec![flag.to_string()]);
                            if let Some(b) = body { if !b.is_empty() { a.extend(vec!["--body".to_string(), b]); } }
                            let cmd = gh_command(&c, &a);
                            let text = gh_text(&c, a, cwd.as_deref()).await?;
                            Ok(ActionOutput::with_command(Value::String(text), cmd))
                        }
                    }
                })
        );
    }
    // create_release
    {
        let cfg_tool = cfg.clone();
        let cfg_detail = cfg_tool.clone();
        let cfg_cmd = cfg_tool.clone();
        let cfg_run = cfg_tool.clone();
        let mut props = Map::new();
        props.insert(cwd_prop().0, cwd_prop().1);
        props.insert(repo_prop().0, repo_prop().1);
        props.insert("tag".to_string(), json!({"type":"string","description":"Release tag, e.g. v1.2.3"}));
        props.insert("title".to_string(), json!({"type":"string","description":"Release title (defaults to the tag)"}));
        props.insert("notes".to_string(), json!({"type":"string","description":"Release notes (markdown)"}));
        props.insert("draft".to_string(), json!({"type":"boolean","description":"Create as a draft"}));
        props.insert("prerelease".to_string(), json!({"type":"boolean","description":"Mark as a prerelease"}));
        tools.push(
            ActionTool::new("create_release", "Publish a release for a tag (draft or prerelease optional).", ActionCategory::Write)
                .schema(props)
                .detail_fn(move |args| {
                    let repo = extract_str(args, "repo").unwrap_or_else(|| cfg_detail.default_repo.clone().unwrap_or("?".to_string()));
                    let tag = extract_str(args, "tag").unwrap_or_default();
                    format!("create_release {repo} {tag}")
                })
                .command_fn({
                    let c = cfg_cmd.clone();
                    move |args, _| {
                        let repo = extract_str(args, "repo");
                        let tag = extract_str(args, "tag").unwrap_or_default();
                        let pos = positional(&tag, "tag").unwrap_or(tag.clone());
                        let title = extract_str(args, "title");
                        let notes = extract_str(args, "notes");
                        let draft = extract_bool(args, "draft").unwrap_or(false);
                        let prerelease = extract_bool(args, "prerelease").unwrap_or(false);
                        let mut a = vec!["release".to_string(), "create".to_string(), pos];
                        a.extend(repo_flag(&c, repo.as_deref()));
                        if let Some(t) = title { if !t.is_empty() { a.extend(vec!["--title".to_string(), t]); } }
                        if let Some(n) = notes { if !n.is_empty() { a.extend(vec!["--notes".to_string(), n]); } }
                        if draft { a.push("--draft".to_string()); }
                        if prerelease { a.push("--prerelease".to_string()); }
                        gh_command(&c, &a)
                    }
                })
                .run({
                    let c = cfg_run.clone();
                    move |args, _| {
                        let c = c.clone();
                        async move {
                            let repo = extract_str(&args, "repo");
                            let tag = extract_str(&args, "tag").unwrap_or_default();
                            let pos = positional(&tag, "tag")?;
                            let title = extract_str(&args, "title");
                            let notes = extract_str(&args, "notes");
                            let draft = extract_bool(&args, "draft").unwrap_or(false);
                            let prerelease = extract_bool(&args, "prerelease").unwrap_or(false);
                            let cwd = extract_str(&args, "cwd");
                            let mut a = vec!["release".to_string(), "create".to_string(), pos];
                            a.extend(repo_flag(&c, repo.as_deref()));
                            if let Some(t) = title { if !t.is_empty() { a.extend(vec!["--title".to_string(), t]); } }
                            if let Some(n) = notes { if !n.is_empty() { a.extend(vec!["--notes".to_string(), n]); } }
                            if draft { a.push("--draft".to_string()); }
                            if prerelease { a.push("--prerelease".to_string()); }
                            let cmd = gh_command(&c, &a);
                            let text = gh_text(&c, a, cwd.as_deref()).await?;
                            Ok(ActionOutput::with_command(Value::String(text), cmd))
                        }
                    }
                })
        );
    }

    tools
}

pub fn github_cli_adapter_spec(store: Arc<pluk_store::Store>) -> ActionAdapterSpec<GhConfig> {
    ActionAdapterSpec::new("github-cli", "GitHub CLI", "code-host")
        .agent_hint(AGENT_HINT)
        .access("Runs the locally installed gh CLI with your own GitHub login — Pluk stores no credentials. Reads issues, PRs, diffs, code search, file contents, CI status, and releases; comments and opens issues/PRs/releases when write is permitted. Every action is policy-checked and recorded in the activity log.")
        .start("list_pull_requests")
        .config_fields(github_cli_fields())
        .client(|conn, _owner| Ok(gh_config(conn)))
        .test_connection(|conn| { let owned = conn.clone(); async move { test_gh(&owned).await } })
        .humanize_error(|e| humanize_gh_error(e))
        .tools(|_conn, cfg| github_cli_tools(cfg.clone()))
}

pub fn build_github_cli_adapter(store: Arc<pluk_store::Store>) -> ActionAdapter<GhConfig> {
    crate::action::action_adapter(github_cli_adapter_spec(store.clone()), store)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::Adapter;
    use crate::tool_host::{ToolHost, ToolRegistration, PromptHandler, ResourceHandler, BoxFuture, PromptResult, ResourceContents, ToolHandler};
    use pluk_store::{Store, Integration, Environment};
    use serde_json::{json, Value, Map};
    use std::sync::{Arc, Mutex};

    fn conn(config: Map<String, Value>) -> Integration {
        Integration { id: "g".into(), name: "GitHub CLI".into(), r#type: "github-cli".into(), config, read_only: 0, query_policy: None, token: "t".into(), created_at: "".into(), environment: None, via_group: None }
    }
    fn map(v: Value) -> Map<String, Value> { v.as_object().cloned().unwrap_or_default() }

    fn gh_cfg(extra: Value) -> GhConfig {
        let m = extra.as_object().cloned().unwrap_or_default();
        gh_config(&conn(m))
    }

    struct Calls { bin: String, args: Vec<String>, cwd: String }

    fn set_fake(calls: Arc<Mutex<Vec<Calls>>>, responder: Arc<dyn Fn(&Vec<String>) -> (i32, String, String) + Send + Sync>) {
        let c = calls.clone();
        let runner: GhRunner = Arc::new(move |bin, args, cwd, _timeout| {
            let c = c.clone();
            let responder = responder.clone();
            let args_clone = args.clone();
            Box::pin(async move {
                c.lock().unwrap().push(Calls{ bin: bin.clone(), args: args_clone.clone(), cwd: cwd.clone() });
                let (code, out, err) = responder(&args_clone);
                Ok(GhRunResult{ code, stdout: out, stderr: err })
            })
        });
        set_gh_runner(Some(runner));
    }
    fn clear_fake() { set_gh_runner(None); }

    #[test]
    fn gh_config_defaults() {
        let cfg = gh_cfg(json!({}));
        assert_eq!(cfg.bin, "gh");
        assert_eq!(cfg.default_repo, None);
        assert_eq!(cfg.timeout_ms, 30_000);
        assert!(!cfg.default_cwd.is_empty());
    }
    #[test]
    fn gh_config_honours_fields() {
        let cfg = gh_cfg(json!({"gh_bin":" ~/bin/gh ", "default_repo":"acme/app", "default_cwd":"/wt", "timeout_seconds":10}));
        assert!(cfg.bin.ends_with("/bin/gh"));
        assert!(!cfg.bin.starts_with('~'));
        assert_eq!(cfg.default_repo.as_deref(), Some("acme/app"));
        assert_eq!(cfg.default_cwd, "/wt");
        assert_eq!(cfg.timeout_ms, 10_000);
    }
    #[test]
    fn gh_config_rejects_nonsense_timeouts() {
        assert_eq!(gh_cfg(json!({"timeout_seconds":0})).timeout_ms, 30_000);
        assert_eq!(gh_cfg(json!({"timeout_seconds":-3})).timeout_ms, 30_000);
    }
    #[test]
    fn gh_cwd_prefers_arg() {
        let cfg = gh_cfg(json!({"default_cwd":"/wt"}));
        assert_eq!(gh_cwd(&cfg, Some("/wt/feature")), "/wt/feature");
        assert_eq!(gh_cwd(&cfg, Some("  ")), "/wt");
        assert_eq!(gh_cwd(&cfg, None), "/wt");
    }
    #[test]
    fn repo_flag_carries_overrides() {
        let cfg = gh_cfg(json!({"default_repo":"acme/app"}));
        assert_eq!(repo_flag(&cfg, Some("other/repo")), vec!["--repo","other/repo"]);
        assert_eq!(repo_flag(&cfg, None), vec!["--repo","acme/app"]);
        assert_eq!(repo_flag(&gh_cfg(json!({})), None), Vec::<String>::new());
    }
    #[test]
    fn resolve_repo_contract() {
        let cfg = gh_cfg(json!({"default_repo":"acme/app"}));
        assert_eq!(resolve_repo(&cfg, Some("other/repo")).unwrap(), ("other".to_string(),"repo".to_string()));
        assert_eq!(resolve_repo(&cfg, None).unwrap(), ("acme".to_string(),"app".to_string()));
        assert!(resolve_repo(&gh_cfg(json!({})), None).is_err());
        assert!(resolve_repo(&cfg, Some("not-a-repo")).is_err());
    }

    #[tokio::test]
    async fn run_gh_forwards_cwd() {
        let cfg = gh_cfg(json!({"default_cwd":"/wt"}));
        let calls = Arc::new(Mutex::new(Vec::new()));
        set_fake(calls.clone(), Arc::new(|_| (0,"[]".to_string(),"".to_string())));
        crate::github_cli::client::run_gh(&cfg, vec!["auth".to_string(),"status".to_string()], Some("/wt/feature")).await.unwrap();
        assert_eq!(calls.lock().unwrap()[0].cwd, "/wt/feature");
        crate::github_cli::client::run_gh(&cfg, vec!["auth".to_string(),"status".to_string()], None).await.unwrap();
        assert_eq!(calls.lock().unwrap()[1].cwd, "/wt");
        clear_fake();
    }

        #[test]
    fn list_pull_requests_builds_args() {
        let cfg = gh_cfg(json!({}));
        let tools = github_cli_tools(cfg.clone());
        let t = tools.into_iter().find(|x| x.name=="list_pull_requests").unwrap();
        let cmd_fn = t.command.unwrap();
        let cmd = cmd_fn(&json!({"repo":"acme/app","state":"open","limit":30}), &Map::new());
        assert!(cmd.contains("pr") && cmd.contains("list"));
        let args = vec!["pr".to_string(), "list".to_string(), "--repo".to_string(), "acme/app".to_string(), "--state".to_string(), "open".to_string(), "--limit".to_string(), "30".to_string(), "--json".to_string(), "number,title,state,headRefName,baseRefName,author,createdAt,updatedAt".to_string()];
        let expected = gh_command(&cfg, &args);
        assert_eq!(cmd, expected);
    }

    #[tokio::test]
    async fn get_issue_passes_number_and_cwd() {
        let cfg = gh_cfg(json!({}));
        let calls = Arc::new(Mutex::new(Vec::new()));
        set_fake(calls.clone(), Arc::new(|_| (0,"[]".to_string(),"".to_string())));
        let tools = github_cli_tools(cfg);
        let t = tools.into_iter().find(|x| x.name=="get_issue").unwrap();
        let _ = (t.run)(json!({"repo":"acme/app","number":12,"cwd":"/wt/feature"}), Map::new()).await.unwrap();
        assert_eq!(calls.lock().unwrap()[0].args, vec!["issue","view","12","--repo","acme/app","--json","number,title,body,state,labels,author,comments,createdAt,updatedAt"]);
        assert_eq!(calls.lock().unwrap()[0].cwd, "/wt/feature");
        clear_fake();
    }

    #[tokio::test]
    async fn nonzero_surfaces_stderr() {
        let cfg = gh_cfg(json!({}));
        let calls = Arc::new(Mutex::new(Vec::new()));
        set_fake(calls, Arc::new(|_| (1,"".to_string(),"not authorized".to_string())));
        let tools = github_cli_tools(cfg);
        let t = tools.into_iter().find(|x| x.name=="list_pull_requests").unwrap();
        let err = (t.run)(json!({}), Map::new()).await.expect_err("should err");
        assert!(err.message.contains("exit 1") && err.message.contains("not authorized"));
        clear_fake();
    }

    #[tokio::test]
    async fn missing_executable_clear_error() {
        clear_fake();
        let cfg = gh_cfg(json!({"gh_bin":"/nope/gh"}));
        let err = crate::github_cli::client::run_gh(&cfg, vec!["--version".to_string()], None).await.expect_err("should err");
        assert!(err.message.contains("gh executable not found: /nope/gh"));
        clear_fake();
    }

    #[tokio::test]
    async fn worktree_pr_creation_defaults() {
        let cfg = gh_cfg(json!({}));
        let calls = Arc::new(Mutex::new(Vec::new()));
        set_fake(calls.clone(), Arc::new(|_| (0,"ok".to_string(),"".to_string())));
        let tools = github_cli_tools(cfg);
        let t = tools.into_iter().find(|x| x.name=="create_pull_request").unwrap();
        let _ = (t.run)(json!({"cwd":"/wt/feature","title":"Add auth"}), Map::new()).await.unwrap();
        assert_eq!(calls.lock().unwrap()[0].cwd, "/wt/feature");
        assert_eq!(calls.lock().unwrap()[0].args, vec!["pr","create","--title","Add auth"]);
        clear_fake();
    }
    #[tokio::test]
    async fn worktree_pr_creation_flags() {
        let cfg = gh_cfg(json!({}));
        let calls = Arc::new(Mutex::new(Vec::new()));
        set_fake(calls.clone(), Arc::new(|_| (0,"ok".to_string(),"".to_string())));
        let tools = github_cli_tools(cfg);
        let t = tools.into_iter().find(|x| x.name=="create_pull_request").unwrap();
        let _ = (t.run)(json!({"cwd":"/wt/feature","title":"Add auth","body":"Body","repo":"acme/app","head":"feature","base":"main","draft":true}), Map::new()).await.unwrap();
        assert_eq!(calls.lock().unwrap()[0].args, vec!["pr","create","--repo","acme/app","--title","Add auth","--body","Body","--base","main","--head","feature","--draft"]);
        clear_fake();
    }

    #[tokio::test]
    async fn api_backed_pr_files_resolves_repo() {
        let cfg = gh_cfg(json!({}));
        let calls = Arc::new(Mutex::new(Vec::new()));
        set_fake(calls.clone(), Arc::new(|_| (0,"[]".to_string(),"".to_string())));
        let tools = github_cli_tools(cfg.clone());
        let t = tools.into_iter().find(|x| x.name=="pr_files").unwrap();
        let _ = (t.run)(json!({"repo":"acme/app","number":7,"limit":30}), Map::new()).await.unwrap();
        assert_eq!(calls.lock().unwrap()[0].args, vec!["api","--method","GET","repos/acme/app/pulls/7/files?per_page=30"]);
        clear_fake();
        let tools2 = github_cli_tools(cfg);
        let t2 = tools2.into_iter().find(|x| x.name=="pr_files").unwrap();
        let err = (t2.run)(json!({"number":7}), Map::new()).await.expect_err("should err no repo");
        assert!(err.message.contains("No repo given"));
        clear_fake();
    }

    #[tokio::test]
    async fn release_tools_cover_list_view_create() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        set_fake(calls.clone(), Arc::new(|_| (0,"[]".to_string(),"".to_string())));
        let tools = github_cli_tools(gh_cfg(json!({})));
        let lr = tools.iter().find(|x| x.name=="list_releases").unwrap();
        let _ = (lr.run)(json!({"repo":"acme/app","limit":30}), Map::new()).await.unwrap();
        assert_eq!(calls.lock().unwrap()[0].args[0], "release");
        assert_eq!(calls.lock().unwrap()[0].args[1], "list");
        clear_fake();
        let calls = Arc::new(Mutex::new(Vec::new()));
        set_fake(calls.clone(), Arc::new(|_| (0,r#"{"tagName":"v1.2.3","name":"v1.2.3","body":"b","isDraft":false,"isPrerelease":false,"assets":[]}"#.to_string(),"".to_string())));
        let tools = github_cli_tools(gh_cfg(json!({})));
        let gr = tools.iter().find(|x| x.name=="get_release").unwrap();
        let _ = (gr.run)(json!({"repo":"acme/app","tag":"v1.2.3"}), Map::new()).await.unwrap();
        assert_eq!(calls.lock().unwrap()[0].args, vec!["release","view","v1.2.3","--repo","acme/app","--json","tagName,name,body,isDraft,isPrerelease,author,createdAt,publishedAt,assets,url"]);
        clear_fake();
        let calls = Arc::new(Mutex::new(Vec::new()));
        set_fake(calls.clone(), Arc::new(|_| (0,"ok".to_string(),"".to_string())));
        let tools = github_cli_tools(gh_cfg(json!({})));
        let cr = tools.iter().find(|x| x.name=="create_release").unwrap();
        let _ = (cr.run)(json!({"repo":"acme/app","tag":"v1.2.3","title":"v1.2.3","notes":"Notes","draft":true}), Map::new()).await.unwrap();
        assert_eq!(calls.lock().unwrap()[0].args, vec!["release","create","v1.2.3","--repo","acme/app","--title","v1.2.3","--notes","Notes","--draft"]);
        clear_fake();
    }

    #[tokio::test]
    async fn tag_flag_refused() {
        let cfg = gh_cfg(json!({}));
        let tools = github_cli_tools(cfg);
        let t = tools.into_iter().find(|x| x.name=="get_release").unwrap();
        let err = (t.run)(json!({"repo":"acme/app","tag":"--draft"}), Map::new()).await.expect_err("should err");
        assert!(err.message.contains("must not start with"));
        clear_fake();
    }

    #[tokio::test]
    async fn test_connection_guidance() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        set_fake(calls.clone(), Arc::new(|_| (0,"".to_string(),"".to_string())));
        let store = Arc::new(Store::open(&tempfile::tempdir().unwrap().path().join("pluk.db")).unwrap());
        let adapter = crate::github_cli::build_github_cli_adapter(store);
        let integ = conn(map(json!({})));
        adapter.test_connection(&integ).await.expect("should pass");
        clear_fake();
        set_fake(calls.clone(), Arc::new(|_| (1,"".to_string(),"Please log in first".to_string())));
        let err = adapter.test_connection(&integ).await.expect_err("should fail");
        assert!(err.message.contains("not authenticated") && err.message.contains("gh auth login"));
        clear_fake();
    }

    #[test]
    fn humanize_points_to_login() {
        assert!(humanize_gh_error(&AdapterError::new("gh executable not found: /nope/gh")).contains("gh auth login"));
        assert!(humanize_gh_error(&AdapterError::new("gh pr list failed (exit 1): Please log in")).contains("gh auth login"));
    }

    #[test]
    fn adapter_exposes_surface() {
        let store = Arc::new(Store::open(&tempfile::tempdir().unwrap().path().join("pluk.db")).unwrap());
        let adapter = crate::github_cli::build_github_cli_adapter(store);
        let specs = adapter.tool_specs();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names.len(), names.iter().collect::<std::collections::HashSet<_>>().len());
        assert!(names.contains(&"create_pull_request"));
        assert!(names.contains(&"list_releases"));
        assert!(names.contains(&"get_repo"));
        let by_name: std::collections::HashMap<&str, &crate::tool_spec::ToolSpec> = specs.iter().map(|s| (s.name.as_str(), s)).collect();
        assert!(by_name["list_pull_requests"].default_enabled);
        assert!(by_name["get_repo"].default_enabled);
        assert!(!by_name["commit_status"].default_enabled);
        for w in ["add_comment","create_issue","create_pull_request","review_pull_request","create_release"] {
            assert!(!by_name[w].default_enabled, "{w} should be off");
        }
    }

    #[tokio::test]
    async fn get_repo_projection() {
        let cfg = gh_cfg(json!({}));
        clear_fake();
        let calls = Arc::new(Mutex::new(Vec::new()));
        set_fake(calls.clone(), Arc::new(|_| (0, r#"{"name":"app","owner":{"login":"acme"},"description":"d","url":"u","defaultBranchRef":{"name":"main"},"isPrivate":false,"stargazerCount":2,"pushedAt":"2026-01-01","forkCount":1}"#.to_string(),"".to_string())));
        let tools = github_cli_tools(cfg);
        let repo = tools.into_iter().find(|x| x.name=="get_repo").unwrap();
        let res = (repo.run)(json!({"repo":"acme/app"}), Map::new()).await.unwrap();
        let val = match res { ActionOutput::WithCommand{value,..} => value, ActionOutput::Value(v) => v };
        assert_eq!(val, json!({"name":"app","owner":{"login":"acme"},"description":"d","url":"u","defaultBranchRef":{"name":"main"},"isPrivate":false}));
        let res2 = (repo.run)(json!({"repo":"acme/app","only":["stats"]}), Map::new()).await.unwrap();
        let val2 = match res2 { ActionOutput::WithCommand{value,..} => value, ActionOutput::Value(v) => v };
        assert!(val2.get("stargazerCount").is_some() || val2.get("forkCount").is_some());
        let res3 = (repo.run)(json!({"repo":"acme/app","only":["*"]}), Map::new()).await.unwrap();
        let val3 = match res3 { ActionOutput::WithCommand{value,..} => value, ActionOutput::Value(v) => v };
        assert_eq!(val3.get("stargazerCount"), Some(&json!(2)));
        let err = (repo.run)(json!({"repo":"acme/app","only":["missing"]}), Map::new()).await.expect_err("should err unknown");
        assert!(err.message.contains("Unknown \"only\" field"));
        clear_fake();
    }

    #[tokio::test]
    async fn list_issues_default_and_preset_and_star() {
        let cfg = gh_cfg(json!({}));
        clear_fake();
        let calls = Arc::new(Mutex::new(Vec::new()));
        set_fake(calls.clone(), Arc::new(|_| (0, r#"[{"number":1,"title":"T","state":"open","labels":[],"author":{"login":"a"},"createdAt":"c","updatedAt":"u"}]"#.to_string(),"".to_string())));
        let tools = github_cli_tools(cfg);
        let t = tools.into_iter().find(|x| x.name=="list_issues").unwrap();
        let res = (t.run)(json!({"repo":"acme/app","state":"open","limit":30}), Map::new()).await.unwrap();
        let v = match res { ActionOutput::WithCommand{value,..} => value, ActionOutput::Value(x)=>x };
        assert_eq!(v, json!([{"number":1,"title":"T","state":"open","labels":[]}]));
        let res2 = (t.run)(json!({"repo":"acme/app","only":["authorship"]}), Map::new()).await.unwrap();
        let v2 = match res2 { ActionOutput::WithCommand{value,..} => value, ActionOutput::Value(x)=>x };
        assert_eq!(v2, json!([{"author":{"login":"a"},"createdAt":"c","updatedAt":"u"}]));
        let res3 = (t.run)(json!({"repo":"acme/app","only":["*"]}), Map::new()).await.unwrap();
        let v3 = match res3 { ActionOutput::WithCommand{value,..} => value, ActionOutput::Value(x)=>x };
        assert_eq!(v3, json!([{"number":1,"title":"T","state":"open","labels":[],"author":{"login":"a"},"createdAt":"c","updatedAt":"u"}]));
        clear_fake();
    }

    #[test]
    fn only_arg_presence_matches_spec() {
        let cfg = gh_cfg(json!({}));
        let tools = github_cli_tools(cfg);
        let has_only = |name: &str| tools.iter().find(|t| t.name==name).and_then(|t| t.schema.as_ref()).map(|m| m.contains_key("only")).unwrap_or(false);
        for with in ["list_issues","get_issue","list_pull_requests","get_pull_request","pr_files","search_code","get_file","commit_status","get_repo","list_releases","get_release"] {
            assert!(has_only(with), "{with} should have only");
        }
        for without in ["add_comment","create_issue","create_pull_request","review_pull_request","create_release"] {
            assert!(!has_only(without), "{without} should NOT have only");
        }
    }
}
