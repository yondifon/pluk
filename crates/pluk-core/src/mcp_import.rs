//! Reading MCP server configs copied from other clients into server drafts.
//!
//! [`parse`] is pure: text in, one [`ServerDraft`] per server it recognised
//! out, plus a [`ServerProblem`] for each entry it could not read. One bad
//! entry never costs the others.
//!
//! Accepted shapes, JSON or TOML:
//! - `mcpServers` (Claude Code, Cursor, Windsurf, Claude Desktop), `servers`
//!   and `mcp.servers` (VS Code), `mcp` (opencode), `mcp_servers` (Codex),
//!   anywhere at the top of a full client config file.
//! - The server map on its own, `{ "<name>": { … } }`, or a single server
//!   object, whose name is made up from its address or command.
//!
//! Every entry is read with the union of those clients' keys, so a server
//! copied from one client's docs into another's file still reads. A key that
//! decides nothing Pluk stores is listed in [`ServerDraft::not_imported`],
//! never dropped quietly.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// How Pluk reaches a server: at a URL, or by starting a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Connection {
    Remote,
    Local,
}

/// One header or environment variable, with a guess at whether it is secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftRow {
    pub name: String,
    pub value: String,
    pub secret: bool,
}

/// One server as Pluk would add it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerDraft {
    pub name: String,
    pub connection: Connection,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: Vec<DraftRow>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<DraftRow>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// Tools to turn off once Pluk finds them on the server.
    #[serde(default)]
    pub disabled_tools: Vec<String>,
    /// Keys, and `mcp-remote` options, that Pluk has nowhere to put.
    #[serde(default)]
    pub not_imported: Vec<String>,
    /// The config started `mcp-remote` to reach this URL, so Pluk connects to
    /// the URL itself.
    #[serde(default)]
    pub via_mcp_remote: bool,
    /// The config had this server turned off.
    #[serde(default)]
    pub turned_off: bool,
    /// The config asked for the older SSE transport.
    #[serde(default)]
    pub sse: bool,
}

/// An entry that could not be read, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerProblem {
    pub name: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParsedImport {
    pub servers: Vec<ServerDraft>,
    pub problems: Vec<ServerProblem>,
}

/// Text that is not JSON or TOML, or holds no server. `line` and `column`
/// count from 1 and point at the problem when the parser knows where it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportError {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
}

impl ImportError {
    fn plain(message: &str) -> Self {
        ImportError {
            message: message.to_string(),
            line: None,
            column: None,
        }
    }
}

const NOTHING_PASTED: &str = "Paste a server config first.";
const NO_SERVERS: &str =
    "No MCP servers found. Paste the part of the config that lists them, such as mcpServers.";

/// Env names that usually hold a credential.
const SECRET_WORDS: &[&str] = &["TOKEN", "KEY", "SECRET", "PASSWORD", "AUTH", "CREDENTIAL"];

/// The keys a remote server's URL goes under, in the order they are tried.
const ADDRESS_KEYS: &[&str] = &["url", "serverUrl", "httpUrl"];

pub fn parse(text: &str) -> Result<ParsedImport, ImportError> {
    let root = read_text(text)?;
    let Value::Object(root) = root else {
        return Err(ImportError::plain(NO_SERVERS));
    };
    let entries = server_entries(&root).ok_or_else(|| ImportError::plain(NO_SERVERS))?;
    let mut parsed = ParsedImport {
        servers: Vec::new(),
        problems: Vec::new(),
    };
    for (name, entry) in entries {
        match read_entry(&name, &entry) {
            Ok(draft) => parsed.servers.push(draft),
            Err(message) => parsed.problems.push(ServerProblem { name, message }),
        }
    }
    Ok(parsed)
}

/// Whether an env variable's name suggests its value is a credential.
pub fn looks_secret(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_WORDS.iter().any(|word| upper.contains(word))
}

/// JSON when it starts like JSON, TOML otherwise. A JSON fragment such as
/// `"name": { … }`, copied without its outer braces, is read as an object.
fn read_text(text: &str) -> Result<Value, ImportError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(ImportError::plain(NOTHING_PASTED));
    }
    if trimmed.starts_with('{') {
        return serde_json::from_str(text).map_err(json_error);
    }
    if trimmed.starts_with('"') {
        let wrapped = format!("{{{}}}", trimmed.trim_end_matches(','));
        return serde_json::from_str(&wrapped).map_err(json_error);
    }
    let table: toml::Table = toml::from_str(text).map_err(|error| toml_error(text, &error))?;
    serde_json::to_value(table).map_err(|_| ImportError::plain(NO_SERVERS))
}

fn json_error(error: serde_json::Error) -> ImportError {
    ImportError {
        message: format!("This JSON has a mistake: {}.", json_reason(&error)),
        line: Some(error.line()),
        column: Some(error.column()),
    }
}

/// serde_json's message without the position it appends, which the error
/// carries on its own.
fn json_reason(error: &serde_json::Error) -> String {
    let message = error.to_string();
    match message.rfind(" at line ") {
        Some(at) => message[..at].to_string(),
        None => message,
    }
}

fn toml_error(text: &str, error: &toml::de::Error) -> ImportError {
    let (line, column) = match error.span() {
        Some(span) => position(text, span.start),
        None => (None, None),
    };
    ImportError {
        message: format!(
            "This isn't JSON or TOML Pluk can read: {}.",
            error.message().trim_end_matches('.')
        ),
        line,
        column,
    }
}

fn position(text: &str, offset: usize) -> (Option<usize>, Option<usize>) {
    let before = text.get(..offset).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(0, |tail| tail.chars().count())
        + 1;
    (Some(line), Some(column))
}

/// The named server entries `root` holds, from the first container found.
fn server_entries(root: &Map<String, Value>) -> Option<Vec<(String, Value)>> {
    let containers = [
        root.get("mcpServers"),
        root.get("mcp_servers"),
        root.get("mcp")
            .and_then(|mcp| mcp.get("servers").filter(|servers| servers.is_object())),
        root.get("servers"),
        root.get("mcp"),
    ];
    if let Some(Value::Object(map)) = containers.into_iter().flatten().find(|c| c.is_object()) {
        return Some(named(map));
    }
    if is_server(root) {
        return Some(vec![(made_up_name(root), Value::Object(root.clone()))]);
    }
    let all_servers = !root.is_empty()
        && root
            .values()
            .all(|value| value.as_object().is_some_and(is_server));
    all_servers.then(|| named(root))
}

fn named(map: &Map<String, Value>) -> Vec<(String, Value)> {
    map.iter()
        .map(|(name, entry)| (name.clone(), entry.clone()))
        .collect()
}

fn is_server(entry: &Map<String, Value>) -> bool {
    entry.contains_key("command") || ADDRESS_KEYS.iter().any(|key| entry.contains_key(*key))
}

/// A name for a single pasted server: its URL's host, or what the command
/// runs, with any version and file extension taken off.
fn made_up_name(entry: &Map<String, Value>) -> String {
    let url = ADDRESS_KEYS
        .iter()
        .find_map(|key| entry.get(*key).and_then(Value::as_str));
    if let Some(host) = url.and_then(host_of) {
        return host;
    }
    let mut words: Vec<String> = match entry.get("command") {
        Some(Value::Array(items)) => items.iter().filter_map(text_of).collect(),
        Some(value) => text_of(value).into_iter().collect(),
        None => Vec::new(),
    };
    if let Some(Value::Array(args)) = entry.get("args") {
        words.extend(args.iter().filter_map(text_of));
    }
    let runs = if words.first().is_some_and(|program| is_runner(program)) {
        words.iter().skip(1).find(|word| !word.starts_with('-'))
    } else {
        words.first()
    };
    runs.map(|word| file_stem(word))
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "mcp-server".to_string())
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?.split(':').next()?;
    (!host.is_empty()).then(|| host.to_string())
}

/// A program that runs whatever package or file it is given.
fn is_runner(program: &str) -> bool {
    matches!(
        file_stem(program).as_str(),
        "npx" | "bunx" | "pnpx" | "uvx" | "node" | "bun" | "deno" | "python" | "python3" | "uv"
    )
}

fn file_stem(word: &str) -> String {
    let last = word.rsplit('/').next().unwrap_or(word);
    // `@scope/pkg@1.2` leaves `pkg@1.2` here; drop the version.
    let unversioned = match last.find('@') {
        Some(0) | None => last,
        Some(at) => &last[..at],
    };
    match unversioned.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => unversioned.to_string(),
    }
}

/// Keys read into the draft or used to classify it. Any other key is listed
/// as not imported.
const READ_KEYS: &[&str] = &[
    "type",
    "transport",
    "url",
    "serverUrl",
    "httpUrl",
    "headers",
    "http_headers",
    "command",
    "args",
    "env",
    "environment",
    "cwd",
    "disabledTools",
    "disabled_tools",
    "enabled",
    "disabled",
];

fn read_entry(name: &str, entry: &Value) -> Result<ServerDraft, String> {
    let Value::Object(entry) = entry else {
        return Err(
            "This entry isn't a server. Each server needs its own set of settings in braces."
                .to_string(),
        );
    };
    let url = address(entry)?;
    let (command, args) = command_line(entry)?;
    let kind = entry
        .get("type")
        .or_else(|| entry.get("transport"))
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase);
    let connection = match (kind.as_deref(), &url, &command) {
        (Some("stdio" | "local"), _, Some(_)) => Connection::Local,
        (Some("stdio" | "local"), _, None) => {
            return Err("This local server has no command to start it.".to_string());
        }
        (Some(_), Some(_), _) | (None, Some(_), None) => Connection::Remote,
        (None, None, Some(_)) => Connection::Local,
        (None, Some(_), Some(_)) => {
            return Err("This entry has both a URL and a command. Keep one of them.".to_string());
        }
        (_, None, _) => return Err("This entry has no URL or command.".to_string()),
    };
    let mut not_imported: Vec<String> = entry
        .keys()
        .filter(|key| !READ_KEYS.contains(&key.as_str()))
        .cloned()
        .collect();
    let mut draft = ServerDraft {
        name: name.to_string(),
        connection,
        url: None,
        headers: Vec::new(),
        command: None,
        args: Vec::new(),
        env: Vec::new(),
        cwd: None,
        disabled_tools: disabled_tools(entry)?,
        not_imported: Vec::new(),
        via_mcp_remote: false,
        turned_off: turned_off(entry),
        sse: kind.as_deref() == Some("sse"),
    };
    let headers = rows_under(entry, &["headers", "http_headers"], |_| true)?;
    let env = rows_under(entry, &["env", "environment"], looks_secret)?;
    let cwd = optional_text(entry, "cwd")?;

    if connection == Connection::Local
        && let Some(wrapped) = command
            .as_deref()
            .and_then(|program| mcp_remote(program, &args))
    {
        draft.connection = Connection::Remote;
        draft.via_mcp_remote = true;
        draft.url = Some(wrapped.url);
        draft.headers = wrapped.headers;
        not_imported.extend(wrapped.not_imported);
        let local_only = [("env", !env.is_empty()), ("cwd", cwd.is_some())];
        not_imported.extend(
            local_only
                .iter()
                .filter(|(_, set)| *set)
                .map(|(key, _)| key.to_string()),
        );
        not_imported.extend(present(entry, &["headers", "http_headers"]));
        draft.not_imported = not_imported;
        return Ok(draft);
    }
    match draft.connection {
        Connection::Remote => {
            draft.url = url;
            draft.headers = headers;
            not_imported.extend(present(
                entry,
                &["command", "args", "env", "environment", "cwd"],
            ));
        }
        Connection::Local => {
            draft.command = command;
            draft.args = args;
            draft.env = env;
            draft.cwd = cwd;
            not_imported.extend(present(
                entry,
                &["url", "serverUrl", "httpUrl", "headers", "http_headers"],
            ));
        }
    }
    draft.not_imported = not_imported;
    Ok(draft)
}

fn present(entry: &Map<String, Value>, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .filter(|key| entry.contains_key(**key))
        .map(|key| key.to_string())
        .collect()
}

fn address(entry: &Map<String, Value>) -> Result<Option<String>, String> {
    for key in ADDRESS_KEYS {
        if let Some(value) = entry.get(*key) {
            return match value.as_str().map(str::trim) {
                Some(url) if !url.is_empty() => Ok(Some(url.to_string())),
                _ => Err(format!("{key} has to be a web address.")),
            };
        }
    }
    Ok(None)
}

/// The program and its args. opencode gives both as one `command` list.
fn command_line(entry: &Map<String, Value>) -> Result<(Option<String>, Vec<String>), String> {
    let mut words = match entry.get("command") {
        None => return args_too(entry, (None, Vec::new())),
        Some(Value::Array(items)) => texts(items, "command")?,
        Some(value) => vec![text_of(value).ok_or("command has to be text.")?],
    };
    if words.is_empty() || words[0].trim().is_empty() {
        return Err("command is empty.".to_string());
    }
    let program = words.remove(0);
    args_too(entry, (Some(program), words))
}

fn args_too(
    entry: &Map<String, Value>,
    (program, mut args): (Option<String>, Vec<String>),
) -> Result<(Option<String>, Vec<String>), String> {
    match entry.get("args") {
        None => {}
        Some(Value::Array(items)) => args.extend(texts(items, "args")?),
        Some(_) => return Err("args has to be a list.".to_string()),
    }
    Ok((program, args))
}

fn texts(items: &[Value], key: &str) -> Result<Vec<String>, String> {
    items
        .iter()
        .map(|item| text_of(item).ok_or_else(|| format!("Every item in {key} has to be text.")))
        .collect()
}

/// Text, or a number or true/false written as text.
fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn optional_text(entry: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match entry.get(key) {
        None => Ok(None),
        Some(value) => text_of(value)
            .map(|text| Some(text).filter(|text| !text.trim().is_empty()))
            .ok_or_else(|| format!("{key} has to be text.")),
    }
}

/// Rows under the first of `keys` present, each guessed secret by `secret`.
fn rows_under(
    entry: &Map<String, Value>,
    keys: &[&str],
    secret: fn(&str) -> bool,
) -> Result<Vec<DraftRow>, String> {
    let Some((key, value)) = keys
        .iter()
        .find_map(|key| entry.get(*key).map(|v| (key, v)))
    else {
        return Ok(Vec::new());
    };
    let Value::Object(map) = value else {
        return Err(format!("{key} has to be a set of names and values."));
    };
    map.iter()
        .map(|(name, value)| {
            let value = text_of(value)
                .ok_or_else(|| format!("The value of {name} in {key} has to be text."))?;
            Ok(DraftRow {
                name: name.clone(),
                value,
                secret: secret(name),
            })
        })
        .collect()
}

fn disabled_tools(entry: &Map<String, Value>) -> Result<Vec<String>, String> {
    for key in ["disabledTools", "disabled_tools"] {
        match entry.get(key) {
            None => continue,
            Some(Value::Array(items)) => return texts(items, key),
            Some(_) => return Err(format!("{key} has to be a list of tool names.")),
        }
    }
    Ok(Vec::new())
}

fn turned_off(entry: &Map<String, Value>) -> bool {
    entry.get("enabled").and_then(Value::as_bool) == Some(false)
        || entry.get("disabled").and_then(Value::as_bool) == Some(true)
}

/// What an `npx mcp-remote <url>` entry reaches, read from its args.
struct McpRemote {
    url: String,
    headers: Vec<DraftRow>,
    not_imported: Vec<String>,
}

fn mcp_remote(program: &str, args: &[String]) -> Option<McpRemote> {
    if !matches!(file_stem(program).as_str(), "npx" | "bunx" | "pnpx") {
        return None;
    }
    let mut rest = args.iter().skip_while(|arg| arg.starts_with('-'));
    let package = rest.next()?;
    if file_stem(package) != "mcp-remote" {
        return None;
    }
    let url = rest
        .next()
        .filter(|url| url.starts_with("http://") || url.starts_with("https://"))?;
    let mut wrapped = McpRemote {
        url: url.clone(),
        headers: Vec::new(),
        not_imported: Vec::new(),
    };
    let mut options = Vec::new();
    while let Some(arg) = rest.next() {
        let header = (arg == "--header").then(|| rest.next()).flatten();
        match header.and_then(|header| header.split_once(':')) {
            Some((name, value)) => wrapped.headers.push(DraftRow {
                name: name.trim().to_string(),
                value: value.trim().to_string(),
                secret: true,
            }),
            None => options.push(arg.as_str()),
        }
    }
    if !options.is_empty() {
        wrapped
            .not_imported
            .push(format!("mcp-remote {}", options.join(" ")));
    }
    Some(wrapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(text: &str) -> ServerDraft {
        let parsed = parse(text).expect("parses");
        assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
        assert_eq!(parsed.servers.len(), 1, "{:?}", parsed.servers);
        parsed.servers.into_iter().next().unwrap()
    }

    fn row(name: &str, value: &str, secret: bool) -> DraftRow {
        DraftRow {
            name: name.to_string(),
            value: value.to_string(),
            secret,
        }
    }

    #[test]
    fn the_sentry_example_is_a_local_server_with_its_token_secret() {
        let draft = only(
            r#"{"mcpServers":{"sentry-selfhosted":{"command":"node","args":["/path/to/sentry-mcp/build/index.js"],
             "env":{"SENTRY_URL":"https://sentry.internal.domain","SENTRY_AUTH_TOKEN":"tok","SENTRY_ORG_SLUG":"my-org"},
             "disabledTools":["create_sentry_issue_comment","update_sentry_issue_status"]}}}"#,
        );
        assert_eq!(draft.name, "sentry-selfhosted");
        assert_eq!(draft.connection, Connection::Local);
        assert_eq!(draft.command.as_deref(), Some("node"));
        assert_eq!(draft.args, ["/path/to/sentry-mcp/build/index.js"]);
        assert_eq!(
            draft.env,
            [
                row("SENTRY_AUTH_TOKEN", "tok", true),
                row("SENTRY_ORG_SLUG", "my-org", false),
                row("SENTRY_URL", "https://sentry.internal.domain", false),
            ]
        );
        assert_eq!(
            draft.disabled_tools,
            ["create_sentry_issue_comment", "update_sentry_issue_status"]
        );
        assert!(draft.not_imported.is_empty());
        assert!(draft.url.is_none() && draft.headers.is_empty());
    }

    #[test]
    fn claude_code_remote_servers_keep_their_headers_as_secrets() {
        let draft = only(
            r#"{"mcpServers":{"datadog":{"type":"http","url":"https://mcp.datadoghq.com/mcp",
             "headers":{"DD-API-KEY":"a","DD-APPLICATION-KEY":"b"}}}}"#,
        );
        assert_eq!(draft.connection, Connection::Remote);
        assert_eq!(draft.url.as_deref(), Some("https://mcp.datadoghq.com/mcp"));
        assert_eq!(
            draft.headers,
            [
                row("DD-API-KEY", "a", true),
                row("DD-APPLICATION-KEY", "b", true)
            ]
        );
        assert!(draft.not_imported.is_empty());
    }

    #[test]
    fn cursor_local_servers_read_like_claude_codes() {
        let draft = only(
            r#"{"mcpServers":{"github":{"command":"npx","args":["-y","@modelcontextprotocol/server-github"],
             "env":{"GITHUB_PERSONAL_ACCESS_TOKEN":"x"}}}}"#,
        );
        assert_eq!(draft.connection, Connection::Local);
        assert_eq!(draft.command.as_deref(), Some("npx"));
        assert_eq!(draft.args, ["-y", "@modelcontextprotocol/server-github"]);
        assert_eq!(draft.env, [row("GITHUB_PERSONAL_ACCESS_TOKEN", "x", true)]);
    }

    #[test]
    fn windsurf_server_url_is_the_address() {
        let draft = only(r#"{"mcpServers":{"linear":{"serverUrl":"https://mcp.linear.app/mcp"}}}"#);
        assert_eq!(draft.connection, Connection::Remote);
        assert_eq!(draft.url.as_deref(), Some("https://mcp.linear.app/mcp"));
    }

    #[test]
    fn opencode_splits_its_command_list_and_reads_environment() {
        let parsed = parse(
            r#"{"$schema":"https://opencode.ai/config.json","theme":"dark","mcp":{
              "files":{"type":"local","command":["bunx","my-mcp","--stdio"],"environment":{"API_KEY":"k","MODE":"fast"},"enabled":true},
              "remote":{"type":"remote","url":"https://example.com/mcp","headers":{"X-Team":"t"},"oauth":false}}}"#,
        )
        .unwrap();
        assert!(parsed.problems.is_empty());
        let files = &parsed.servers[0];
        assert_eq!(files.name, "files");
        assert_eq!(files.connection, Connection::Local);
        assert_eq!(files.command.as_deref(), Some("bunx"));
        assert_eq!(files.args, ["my-mcp", "--stdio"]);
        assert_eq!(
            files.env,
            [row("API_KEY", "k", true), row("MODE", "fast", false)]
        );
        assert!(!files.turned_off);
        let remote = &parsed.servers[1];
        assert_eq!(remote.connection, Connection::Remote);
        assert_eq!(remote.headers, [row("X-Team", "t", true)]);
        assert_eq!(remote.not_imported, ["oauth"]);
    }

    #[test]
    fn codex_toml_reads_http_headers_env_and_disabled_tools() {
        let parsed = parse(
            r#"
model = "gpt-5"

[mcp_servers.docs]
command = "uvx"
args = ["docs-mcp"]
env = { DOCS_TOKEN = "t", DOCS_REGION = "eu" }
startup_timeout_sec = 20
disabled_tools = ["delete_page"]

[mcp_servers.figma]
url = "https://mcp.figma.com/mcp"
http_headers = { "X-Figma-Region" = "us-east-1" }
"#,
        )
        .unwrap();
        assert!(parsed.problems.is_empty());
        let docs = &parsed.servers[0];
        assert_eq!(docs.connection, Connection::Local);
        assert_eq!(docs.command.as_deref(), Some("uvx"));
        assert_eq!(docs.args, ["docs-mcp"]);
        assert_eq!(
            docs.env,
            [
                row("DOCS_REGION", "eu", false),
                row("DOCS_TOKEN", "t", true)
            ]
        );
        assert_eq!(docs.disabled_tools, ["delete_page"]);
        assert_eq!(docs.not_imported, ["startup_timeout_sec"]);
        let figma = &parsed.servers[1];
        assert_eq!(figma.connection, Connection::Remote);
        assert_eq!(figma.headers, [row("X-Figma-Region", "us-east-1", true)]);
    }

    #[test]
    fn a_bare_server_object_is_named_after_what_it_reaches() {
        let remote = only(r#"{"type":"http","url":"https://mcp.sentry.dev/mcp"}"#);
        assert_eq!(remote.name, "mcp.sentry.dev");
        let local = only(r#"{"command":"npx","args":["-y","@acme/weather-mcp@1.2.0"]}"#);
        assert_eq!(local.name, "weather-mcp");
        assert_eq!(local.connection, Connection::Local);
    }

    #[test]
    fn the_server_map_on_its_own_and_a_fragment_without_braces_both_read() {
        let map = parse(
            r#"{"one":{"url":"https://a.example/mcp"},"two":{"command":"node","args":["b.js"]}}"#,
        )
        .unwrap();
        assert_eq!(map.servers.len(), 2);
        let fragment = only(r#""weather": {"command": "node", "args": ["w.js"]},"#);
        assert_eq!(fragment.name, "weather");
    }

    #[test]
    fn a_full_client_file_finds_the_server_list_among_other_keys() {
        let draft = only(
            r#"{"numStartups":4,"theme":"dark","projects":{"/x":{"allowedTools":[]}},
             "mcpServers":{"linear":{"type":"http","url":"https://mcp.linear.app/mcp"}}}"#,
        );
        assert_eq!(draft.name, "linear");
    }

    #[test]
    fn vs_code_servers_read_under_either_key() {
        assert_eq!(
            only(r#"{"servers":{"a":{"type":"http","url":"https://a.example/mcp"}}}"#).name,
            "a"
        );
        assert_eq!(
            only(r#"{"editor.fontSize":12,"mcp":{"servers":{"b":{"type":"stdio","command":"node"}}}}"#).name,
            "b"
        );
    }

    #[test]
    fn mcp_remote_is_imported_as_the_remote_server_it_wraps() {
        let draft = only(
            r#"{"mcpServers":{"linear":{"command":"npx","args":["-y","mcp-remote@latest","https://mcp.linear.app/sse",
             "--header","Authorization: Bearer abc","--transport","sse-only"],"env":{"AUTH":"x"}}}}"#,
        );
        assert_eq!(draft.connection, Connection::Remote);
        assert!(draft.via_mcp_remote);
        assert_eq!(draft.url.as_deref(), Some("https://mcp.linear.app/sse"));
        assert_eq!(draft.headers, [row("Authorization", "Bearer abc", true)]);
        assert!(draft.command.is_none() && draft.args.is_empty() && draft.env.is_empty());
        assert_eq!(
            draft.not_imported,
            ["mcp-remote --transport sse-only", "env"]
        );

        let bunx = only(
            r#"{"mcpServers":{"x":{"command":"bunx","args":["mcp-remote","https://x.example/mcp"]}}}"#,
        );
        assert!(bunx.via_mcp_remote);
        assert!(bunx.not_imported.is_empty());
    }

    #[test]
    fn a_command_that_only_mentions_mcp_remote_stays_local() {
        let draft = only(
            r#"{"mcpServers":{"x":{"command":"node","args":["mcp-remote","https://x.example"]}}}"#,
        );
        assert_eq!(draft.connection, Connection::Local);
        assert!(!draft.via_mcp_remote);
    }

    #[test]
    fn secret_guess_follows_the_env_name() {
        for name in [
            "GITHUB_TOKEN",
            "api_key",
            "ClientSecret",
            "DB_PASSWORD",
            "AUTH_HEADER",
            "GOOGLE_CREDENTIALS",
        ] {
            assert!(looks_secret(name), "{name}");
        }
        for name in ["SENTRY_URL", "REGION", "LOG_LEVEL", "HOME"] {
            assert!(!looks_secret(name), "{name}");
        }
    }

    #[test]
    fn unknown_keys_are_listed_and_keys_for_the_other_mode_too() {
        let draft = only(
            r#"{"mcpServers":{"x":{"url":"https://x.example/mcp","autoApprove":["a"],"timeout":30,"env":{"A":"b"}}}}"#,
        );
        assert_eq!(draft.not_imported, ["autoApprove", "timeout", "env"]);
        let local = only(
            r#"{"mcpServers":{"y":{"type":"stdio","command":"node","headers":{"A":"b"},"envFile":".env"}}}"#,
        );
        assert_eq!(local.not_imported, ["envFile", "headers"]);
    }

    #[test]
    fn a_server_turned_off_in_the_source_says_so() {
        assert!(
            only(r#"{"mcp":{"x":{"type":"remote","url":"https://x.example","enabled":false}}}"#)
                .turned_off
        );
        assert!(only(r#"{"mcpServers":{"x":{"command":"node","disabled":true}}}"#).turned_off);
        assert!(only(r#"{"mcpServers":{"x":{"type":"sse","url":"https://x.example/sse"}}}"#).sse);
    }

    #[test]
    fn bad_json_points_at_the_mistake() {
        let error = parse("{\n  \"mcpServers\": {\n    \"x\": { \"command\": \"node\" \n  }\n")
            .unwrap_err();
        assert!(
            error.message.starts_with("This JSON has a mistake"),
            "{}",
            error.message
        );
        assert!(!error.message.contains(" at line "), "{}", error.message);
        assert!(error.line.is_some() && error.column.is_some());
    }

    #[test]
    fn bad_toml_points_at_the_line() {
        let error = parse("\n[mcp_servers.x]\ncommand = \"node\nargs = []\n").unwrap_err();
        assert!(
            error.message.starts_with("This isn't JSON or TOML"),
            "{}",
            error.message
        );
        assert_eq!(error.line, Some(3));
    }

    #[test]
    fn empty_text_and_text_without_servers_are_told_apart() {
        assert_eq!(parse("  \n").unwrap_err().message, NOTHING_PASTED);
        assert_eq!(
            parse(r#"{"theme":"dark"}"#).unwrap_err().message,
            NO_SERVERS
        );
        assert_eq!(parse("theme = \"dark\"").unwrap_err().message, NO_SERVERS);
    }

    #[test]
    fn one_bad_entry_leaves_the_others_to_import() {
        let parsed = parse(
            r#"{"mcpServers":{
              "good":{"url":"https://good.example/mcp"},
              "both":{"url":"https://x.example","command":"node"},
              "neither":{"env":{"A":"b"}},
              "not-an-object":"https://x.example",
              "bad-args":{"command":"node","args":"index.js"},
              "local":{"command":"node","args":["a.js"],"env":{"PORT":8080}}}}"#,
        )
        .unwrap();
        let names: Vec<&str> = parsed.servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["good", "local"]);
        assert_eq!(parsed.servers[1].env, [row("PORT", "8080", false)]);
        let problems: Vec<&str> = parsed.problems.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(problems, ["bad-args", "both", "neither", "not-an-object"]);
        assert!(parsed.problems.iter().all(|p| !p.message.is_empty()));
    }
}
