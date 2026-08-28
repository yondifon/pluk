//! MCP client config injection.
//!
//! Ports `swift/Sources/MCPConfigInjector.swift` and the client table plus
//! snippet code from `swift/Sources/ConnectionDetailView.swift`.
//!
//! All transforms are pure String→String so they can be tested without touching
//! real config files. File I/O is isolated to `inject`/`inject_many` which
//! honor the safety rules: never overwrite an existing entry, always back up,
//! atomic write, pretty-printed sorted keys, no escaped forward slashes.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::platform::{ConfigFormat, ConfigScope, McpClient, mcp_config_path};

// ── Public types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectResult {
    Added { path: PathBuf },
    Skipped { path: PathBuf },
}

#[derive(Debug, Clone)]
pub enum InjectError {
    ParseFailed { path: PathBuf },
    Write { path: PathBuf, reason: String },
}

impl std::fmt::Display for InjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InjectError::ParseFailed { path } => {
                write!(
                    f,
                    "Couldn't parse the existing config at {}.",
                    path.display()
                )
            }
            InjectError::Write { path, reason } => {
                write!(f, "Couldn't write {}: {}", path.display(), reason)
            }
        }
    }
}

impl std::error::Error for InjectError {}

/// Which clients a write targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientChoice {
    All,
    One(McpClient),
}

impl ClientChoice {
    pub fn label(&self) -> String {
        match self {
            ClientChoice::All => "All detected".to_string(),
            ClientChoice::One(c) => c.label().to_string(),
        }
    }

    /// Clients this choice writes to for `scope`.
    pub fn targets(&self, scope: &ConfigScope) -> Vec<McpClient> {
        match self {
            ClientChoice::One(c) => vec![*c],
            ClientChoice::All => McpClient::ALL
                .iter()
                .copied()
                .filter(|c| {
                    if !c.is_installed() {
                        return false;
                    }
                    match scope {
                        ConfigScope::Global => true,
                        ConfigScope::Project { .. } => c.supports_project_scope(),
                    }
                })
                .collect(),
        }
    }
}

/// Result of a fan-out across multiple clients.
#[derive(Debug, Default, Clone)]
pub struct FanOutResult {
    pub added: Vec<String>,
    pub skipped: Vec<String>,
    pub failed: Vec<String>,
}

// ── Snippet + entry object ─────────────────────────────────────────────────

/// JSON value written into a client's config for `url`.
pub fn entry_object(client: McpClient, url: &str) -> Value {
    match client {
        McpClient::Opencode => serde_json::json!({
            "type": "remote",
            "enabled": true,
            "url": url,
            "oauth": false
        }),
        McpClient::ClaudeCode => serde_json::json!({
            "type": "http",
            "url": url
        }),
        McpClient::Cursor => serde_json::json!({
            "command": "bunx",
            "args": ["mcp-remote", url]
        }),
        McpClient::Windsurf | McpClient::Antigravity => serde_json::json!({
            "serverUrl": url
        }),
        McpClient::Codex => serde_json::json!({
            "url": url
        }),
    }
}

/// Copy-paste snippet shown in the UI for `client`.
pub fn snippet(client: McpClient, key: &str, url: &str) -> String {
    match client {
        McpClient::Opencode => format!(
            "{{\n  \"mcp\": {{\n    \"{key}\": {{\n      \"type\": \"remote\",\n      \"enabled\": true,\n      \"url\": \"{url}\",\n      \"oauth\": false\n    }}\n  }}\n}}"
        ),
        McpClient::Codex => format!("[mcp_servers.{key}]\nurl = \"{url}\"\n"),
        McpClient::ClaudeCode => format!(
            "{{\n  \"mcpServers\": {{\n    \"{key}\": {{\n      \"type\": \"http\",\n      \"url\": \"{url}\"\n    }}\n  }}\n}}"
        ),
        McpClient::Cursor => format!(
            "{{\n  \"mcpServers\": {{\n    \"{key}\": {{\n      \"command\": \"bunx\",\n      \"args\": [\"mcp-remote\", \"{url}\"]\n    }}\n  }}\n}}"
        ),
        McpClient::Windsurf | McpClient::Antigravity => format!(
            "{{\n  \"mcpServers\": {{\n    \"{key}\": {{\n      \"serverUrl\": \"{url}\"\n    }}\n  }}\n}}"
        ),
    }
}

// ── Public inject entry points ────────────────────────────────────────────

/// Inject one client's config.
pub fn inject(
    client: McpClient,
    scope: &ConfigScope,
    key: &str,
    url: &str,
) -> Result<InjectResult, InjectError> {
    let path = mcp_config_path(client, scope);
    let existing = fs::read_to_string(&path).ok();

    match client.config_format() {
        ConfigFormat::Json => inject_json(
            &path,
            client.container_key(),
            key,
            entry_object(client, url),
            existing.as_deref(),
        ),
        ConfigFormat::Toml => inject_toml(&path, key, url, existing.as_deref()),
    }
}

/// Fan-out: write to every target of `choice` for `scope`, collecting results.
pub fn inject_many(
    choice: &ClientChoice,
    scope: &ConfigScope,
    key: &str,
    url: &str,
) -> FanOutResult {
    let mut result = FanOutResult::default();
    for client in choice.targets(scope) {
        match inject(client, scope, key, url) {
            Ok(InjectResult::Added { .. }) => result.added.push(client.label().to_string()),
            Ok(InjectResult::Skipped { .. }) => result.skipped.push(client.label().to_string()),
            Err(e) => result.failed.push(format!("{}: {}", client.label(), e)),
        }
    }
    result
}

// ── JSON path ─────────────────────────────────────────────────────────────

fn inject_json(
    path: &Path,
    container: &str,
    key: &str,
    entry: Value,
    existing: Option<&str>,
) -> Result<InjectResult, InjectError> {
    let mut root: Value = Value::Object(Default::default());
    let had_existing = existing.is_some();

    if let Some(text) = existing
        && !text.trim().is_empty()
    {
        let obj = parse_object(text).ok_or_else(|| InjectError::ParseFailed {
            path: path.to_path_buf(),
        })?;
        root = obj;
    }

    let map = root.as_object_mut().expect("root is object");
    let servers_val = map
        .entry(container.to_string())
        .or_insert_with(|| Value::Object(Default::default()));

    let servers = match servers_val {
        Value::Object(m) => m,
        _ => {
            // Container exists but is not an object — treat as parse failure.
            return Err(InjectError::ParseFailed {
                path: path.to_path_buf(),
            });
        }
    };

    if servers.contains_key(key) {
        return Ok(InjectResult::Skipped {
            path: path.to_path_buf(),
        });
    }

    servers.insert(key.to_string(), entry);
    // root already mutated

    let pretty = pretty_json(&root);
    backup_and_write(path, &pretty, had_existing)?;

    Ok(InjectResult::Added {
        path: path.to_path_buf(),
    })
}

fn parse_object(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        if v.is_object() {
            return Some(v);
        }
        return None;
    }
    let sanitized = sanitize_jsonc(text);
    if let Ok(v) = serde_json::from_str::<Value>(&sanitized)
        && v.is_object()
    {
        return Some(v);
    }
    None
}

fn pretty_json(value: &Value) -> String {
    let sorted = sort_value(value);
    let mut s = serde_json::to_string_pretty(&sorted).expect("serialize");
    // serde_json does not escape '/', but handle the Foundation quirk explicitly.
    s = s.replace("\\/", "/");
    s.push('\n');
    s
}

fn sort_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<String, Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), sort_value(v)))
                .collect();
            let out: serde_json::Map<String, Value> = sorted.into_iter().collect();
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sort_value).collect()),
        _ => value.clone(),
    }
}

// ── JSONC sanitizer ───────────────────────────────────────────────────────

pub fn sanitize_jsonc(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '/' && i + 1 < chars.len() {
            let next = chars[i + 1];
            if next == '/' {
                // line comment
                i += 2;
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            if next == '*' {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2; // past */
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    strip_trailing_commas(&out.into_iter().collect::<String>())
}

fn strip_trailing_commas(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut keep: Vec<char> = Vec::with_capacity(chars.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            keep.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            keep.push(c);
            i += 1;
            continue;
        }
        if c == ',' {
            let mut j = i + 1;
            while j < chars.len()
                && (chars[j] == ' ' || chars[j] == '\n' || chars[j] == '\t' || chars[j] == '\r')
            {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                i += 1;
                continue;
            }
        }
        keep.push(c);
        i += 1;
    }
    keep.into_iter().collect()
}

// ── TOML path ─────────────────────────────────────────────────────────────

fn inject_toml(
    path: &Path,
    key: &str,
    url: &str,
    existing: Option<&str>,
) -> Result<InjectResult, InjectError> {
    let header = format!("[mcp_servers.{key}]");
    if let Some(text) = existing
        && toml_has_table(text, &header)
    {
        return Ok(InjectResult::Skipped {
            path: path.to_path_buf(),
        });
    }
    let block = format!("{header}\nurl = \"{url}\"\n");
    let output = if let Some(text) = existing {
        if text.is_empty() {
            block
        } else if text.ends_with('\n') {
            format!("{text}\n{block}")
        } else {
            format!("{text}\n\n{block}")
        }
    } else {
        block
    };
    let had_existing = existing.is_some();
    backup_and_write(path, &output, had_existing)?;
    Ok(InjectResult::Added {
        path: path.to_path_buf(),
    })
}

pub fn toml_has_table(text: &str, header: &str) -> bool {
    text.lines().any(|line| line.trim() == header)
}

// ── File I/O helpers ──────────────────────────────────────────────────────

fn backup_and_write(path: &Path, contents: &str, had_existing: bool) -> Result<(), InjectError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|e| InjectError::Write {
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;
    }
    if had_existing && path.exists() {
        let bak_path = PathBuf::from(format!("{}.bak", path.display()));
        let _ = fs::remove_file(&bak_path);
        let _ = fs::copy(path, &bak_path);
    }
    atomic_write(path, contents.as_bytes()).map_err(|e| InjectError::Write {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })
}

fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "config".to_string());
    let tmp = parent.join(format!(".{file_name}.tmp.{}", std::process::id()));
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    // ── Entry shapes ──────────────────────────────────────────────────

    #[test]
    fn entry_shapes_match_swift_table() {
        let url = "http://localhost:4242/mcp/token";
        assert_eq!(
            entry_object(McpClient::Opencode, url),
            serde_json::json!({"type":"remote","enabled":true,"url":url,"oauth":false})
        );
        assert_eq!(
            entry_object(McpClient::ClaudeCode, url),
            serde_json::json!({"type":"http","url":url})
        );
        assert_eq!(
            entry_object(McpClient::Cursor, url),
            serde_json::json!({"command":"bunx","args":["mcp-remote", url]})
        );
        assert_eq!(
            entry_object(McpClient::Windsurf, url),
            serde_json::json!({"serverUrl": url})
        );
        assert_eq!(
            entry_object(McpClient::Antigravity, url),
            serde_json::json!({"serverUrl": url})
        );
        assert_eq!(
            entry_object(McpClient::Codex, url),
            serde_json::json!({"url": url})
        );
    }

    #[test]
    fn snippets_match_expected_shapes() {
        let url = "http://localhost:4242/mcp/abc";
        let key = "my-db";
        let s = snippet(McpClient::Opencode, key, url);
        assert!(s.contains("\"mcp\""));
        assert!(s.contains("\"type\": \"remote\""));
        assert!(s.contains(url));

        let s = snippet(McpClient::Codex, key, url);
        assert_eq!(s, format!("[mcp_servers.{key}]\nurl = \"{url}\"\n"));

        let s = snippet(McpClient::ClaudeCode, key, url);
        assert!(s.contains("\"mcpServers\"") && s.contains("\"type\": \"http\""));

        let s = snippet(McpClient::Cursor, key, url);
        assert!(s.contains("\"command\": \"bunx\"") && s.contains("mcp-remote"));

        let s = snippet(McpClient::Windsurf, key, url);
        assert!(s.contains("\"serverUrl\""));
        let s2 = snippet(McpClient::Antigravity, key, url);
        assert!(s2.contains("\"serverUrl\""));
    }

    // ── JSON: basic inject ────────────────────────────────────────────

    #[test]
    fn json_creates_file_when_missing() {
        let dir = tmp();
        let path = dir.path().join("mcp.json");
        let entry = serde_json::json!({"type":"http","url":"http://x"});
        let res = inject_json(&path, "mcpServers", "my-key", entry, None).unwrap();
        assert!(matches!(res, InjectResult::Added { .. }));
        let written = fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(v["mcpServers"]["my-key"]["url"], "http://x");
    }

    #[test]
    fn json_merges_and_preserves_existing() {
        let dir = tmp();
        let path = dir.path().join("mcp.json");
        let initial = r#"{"mcpServers":{"existing":{"url":"http://old"}}}"#;
        fs::write(&path, initial).unwrap();
        let entry = serde_json::json!({"url":"http://new"});
        let res = inject_json(&path, "mcpServers", "new-key", entry, Some(initial)).unwrap();
        assert!(matches!(res, InjectResult::Added { .. }));
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["existing"]["url"], "http://old");
        assert_eq!(v["mcpServers"]["new-key"]["url"], "http://new");
    }

    #[test]
    fn json_skips_existing_key() {
        let dir = tmp();
        let path = dir.path().join("mcp.json");
        let initial = r#"{"mcpServers":{"my-key":{"url":"http://old"}}}"#;
        fs::write(&path, initial).unwrap();
        let entry = serde_json::json!({"url":"http://new"});
        let res = inject_json(&path, "mcpServers", "my-key", entry, Some(initial)).unwrap();
        assert!(matches!(res, InjectResult::Skipped { .. }));
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        // file on disk unchanged (inject_json does not write on skip)
        // but we wrote initial to disk; after skip it should still be old content
        assert_eq!(v["mcpServers"]["my-key"]["url"], "http://old");
    }

    #[test]
    fn json_skip_does_not_write_backup() {
        let dir = tmp();
        let path = dir.path().join("mcp.json");
        let initial = r#"{"mcpServers":{"my-key":{"url":"http://old"}}}"#;
        fs::write(&path, initial).unwrap();
        let entry = serde_json::json!({"url":"http://new"});
        let _ = inject_json(&path, "mcpServers", "my-key", entry, Some(initial)).unwrap();
        assert!(!PathBuf::from(format!("{}.bak", path.display())).exists());
    }

    #[test]
    fn json_writes_backup_before_modify() {
        let dir = tmp();
        let path = dir.path().join("mcp.json");
        let initial = r#"{"mcpServers":{"a":{"url":"http://a"}}}"#;
        fs::write(&path, initial).unwrap();
        let entry = serde_json::json!({"url":"http://b"});
        inject_json(&path, "mcpServers", "b", entry, Some(initial)).unwrap();
        let bak = PathBuf::from(format!("{}.bak", path.display()));
        assert!(bak.exists());
        assert_eq!(fs::read_to_string(bak).unwrap(), initial);
    }

    #[test]
    fn json_atomic_write_leaves_valid_file() {
        let dir = tmp();
        let path = dir.path().join("sub/mcp.json");
        let entry = serde_json::json!({"url":"http://x"});
        inject_json(&path, "mcpServers", "k", entry, None).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(serde_json::from_str::<Value>(&content).is_ok());
        // tmp file cleaned up
        let tmp_file = path
            .parent()
            .unwrap()
            .join(format!(".mcp.json.tmp.{}", std::process::id()));
        assert!(!tmp_file.exists());
    }

    #[test]
    fn json_pretty_sorted_keys_and_no_escaped_slash() {
        let dir = tmp();
        let path = dir.path().join("mcp.json");
        // inject two keys in non-alpha order to test sorting
        let initial = r#"{"mcpServers":{"z":{"url":"http://z"}}}"#;
        fs::write(&path, initial).unwrap();
        let entry = serde_json::json!({"url":"http://localhost:4242/mcp/token"});
        inject_json(&path, "mcpServers", "a", entry, Some(initial)).unwrap();
        let written = fs::read_to_string(&path).unwrap();
        // sorted: a before z
        assert!(written.find("\"a\"").unwrap() < written.find("\"z\"").unwrap());
        // no escaped slash
        assert!(!written.contains("\\/"));
        assert!(written.contains("http://localhost"));
    }

    #[test]
    fn json_opencode_container_key() {
        let dir = tmp();
        let path = dir.path().join("opencode.json");
        let entry = entry_object(McpClient::Opencode, "http://u");
        inject_json(&path, McpClient::Opencode.container_key(), "k", entry, None).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(v.get("mcp").is_some());
        assert!(v.get("mcpServers").is_none());
    }

    // ── JSONC sanitizer ───────────────────────────────────────────────

    #[test]
    fn sanitize_preserves_slash_in_string() {
        let input = r#"{"mcpServers": {"k": {"url": "http://localhost:4242/mcp/tok"}}}"#;
        let out = sanitize_jsonc(input);
        assert!(out.contains("http://localhost"));
        let v: Value = serde_json::from_str(&sanitize_jsonc(input)).unwrap();
        assert_eq!(v["mcpServers"]["k"]["url"], "http://localhost:4242/mcp/tok");
    }

    #[test]
    fn sanitize_strips_line_comments() {
        let input = "// header\n{\"mcpServers\": {} // trailing\n}";
        let sanitized = sanitize_jsonc(input);
        assert!(!sanitized.contains("// header"));
        let v: Value = serde_json::from_str(&sanitized).unwrap();
        assert!(v.is_object());
    }

    #[test]
    fn sanitize_strips_block_comments() {
        let input = "/* block */{\"mcpServers\": {}}";
        let sanitized = sanitize_jsonc(input);
        assert!(!sanitized.contains("block"));
        let v: Value = serde_json::from_str(&sanitized).unwrap();
        assert!(v.is_object());
    }

    #[test]
    fn sanitize_strips_trailing_commas() {
        let input = r#"{"mcpServers": {"a": {"url": "http://a"},},}"#;
        let sanitized = sanitize_jsonc(input);
        let v: Value = serde_json::from_str(&sanitized).unwrap();
        assert_eq!(v["mcpServers"]["a"]["url"], "http://a");
    }

    #[test]
    fn sanitize_preserves_trailing_comma_inside_string() {
        let input = r#"{"msg": "a,}"}"#;
        // malformed but sanitize should not strip the comma inside string
        let sanitized = sanitize_jsonc(input);
        assert!(sanitized.contains("a,}"));
    }

    #[test]
    fn comment_inside_string_survives() {
        let input = r#"{"url": "http://x // not a comment"}"#;
        let sanitized = sanitize_jsonc(input);
        assert!(sanitized.contains("http://x // not a comment"));
    }

    #[test]
    fn json_with_comments_and_trailing_comma_injects() {
        let dir = tmp();
        let path = dir.path().join("mcp.json");
        let initial = "{\n  // comment\n  \"mcpServers\": {\n    \"existing\": {\"url\": \"http://old\"}, // trailing\n  }\n}";
        fs::write(&path, initial).unwrap();
        let entry = serde_json::json!({"url":"http://new"});
        let res = inject_json(&path, "mcpServers", "new", entry, Some(initial)).unwrap();
        assert!(matches!(res, InjectResult::Added { .. }));
        let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["existing"]["url"], "http://old");
        assert_eq!(v["mcpServers"]["new"]["url"], "http://new");
    }

    #[test]
    fn malformed_file_fails_safely_without_destroying_original() {
        let dir = tmp();
        let path = dir.path().join("mcp.json");
        let bad = "{ not json at all";
        fs::write(&path, bad).unwrap();
        let entry = serde_json::json!({"url":"http://x"});
        let err = inject_json(&path, "mcpServers", "k", entry, Some(bad)).unwrap_err();
        assert!(matches!(err, InjectError::ParseFailed { .. }));
        // original file untouched
        assert_eq!(fs::read_to_string(&path).unwrap(), bad);
        assert!(!PathBuf::from(format!("{}.bak", path.display())).exists());
    }

    // ── TOML ──────────────────────────────────────────────────────────

    #[test]
    fn toml_appends_when_absent() {
        let dir = tmp();
        let path = dir.path().join("config.toml");
        let existing = "[mcp_servers.other]\nurl = \"http://old\"\n";
        fs::write(&path, existing).unwrap();
        let res = inject_toml(&path, "my-key", "http://new", Some(existing)).unwrap();
        assert!(matches!(res, InjectResult::Added { .. }));
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("[mcp_servers.my-key]"));
        assert!(content.contains("http://new"));
        assert!(content.contains("[mcp_servers.other]"));
    }

    #[test]
    fn toml_skips_when_present() {
        let dir = tmp();
        let path = dir.path().join("config.toml");
        let existing = "[mcp_servers.my-key]\nurl = \"http://old\"\n";
        fs::write(&path, existing).unwrap();
        let res = inject_toml(&path, "my-key", "http://new", Some(existing)).unwrap();
        assert!(matches!(res, InjectResult::Skipped { .. }));
        assert_eq!(fs::read_to_string(&path).unwrap(), existing);
    }

    #[test]
    fn toml_skip_is_exact_header_match() {
        // nested table should not count as match
        let text = "[mcp_servers.foo.bar]\nurl = \"x\"\n";
        assert!(!toml_has_table(text, "[mcp_servers.foo]"));
        assert!(toml_has_table(text, "[mcp_servers.foo.bar]"));
        // whitespace trimmed
        assert!(toml_has_table(
            "  [mcp_servers.foo]  \n",
            "[mcp_servers.foo]"
        ));
        // commented line ignored
        assert!(!toml_has_table(
            "# [mcp_servers.foo]\n",
            "[mcp_servers.foo]"
        ));
    }

    #[test]
    fn toml_creates_file_when_missing() {
        let dir = tmp();
        let path = dir.path().join("config.toml");
        let res = inject_toml(&path, "k", "http://u", None).unwrap();
        assert!(matches!(res, InjectResult::Added { .. }));
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("[mcp_servers.k]"));
    }

    #[test]
    fn toml_writes_backup() {
        let dir = tmp();
        let path = dir.path().join("config.toml");
        let existing = "[mcp_servers.a]\nurl = \"http://a\"\n";
        fs::write(&path, existing).unwrap();
        inject_toml(&path, "b", "http://b", Some(existing)).unwrap();
        let bak = PathBuf::from(format!("{}.bak", path.display()));
        assert!(bak.exists());
        assert_eq!(fs::read_to_string(bak).unwrap(), existing);
    }

    // ── Backup/atomic via inject_json/toml already tested; also test inject() ─

    #[test]
    fn inject_dispatches_by_format() {
        // JSON client
        let dir = tmp();
        // We cannot easily override mcp_config_path without env; test inject_json directly
        // TOML client path would be ~/.codex/config.toml normally; use inject_toml directly
        let path = dir.path().join("any.json");
        let res = inject_json(
            &path,
            "mcpServers",
            "k",
            serde_json::json!({"url":"u"}),
            None,
        )
        .unwrap();
        assert!(matches!(res, InjectResult::Added { .. }));
        let tpath = dir.path().join("any.toml");
        let res2 = inject_toml(&tpath, "k", "http://u", None).unwrap();
        assert!(matches!(res2, InjectResult::Added { .. }));
    }

    // ── Fan-out result shape ──────────────────────────────────────────

    #[test]
    fn fan_out_result_holds_added_skipped_failed() {
        let r = FanOutResult {
            added: vec!["Cursor".to_string()],
            skipped: vec!["opencode".to_string()],
            failed: vec!["Codex: parse error".to_string()],
        };
        assert_eq!(r.added.len(), 1);
        assert_eq!(r.skipped.len(), 1);
        assert_eq!(r.failed.len(), 1);
    }

    #[test]
    fn client_choice_targets_filters_by_scope_and_install() {
        // Without any installed clients, All yields empty; One yields single regardless.
        let proj = ConfigScope::Project {
            root: PathBuf::from("/tmp/repo"),
        };
        let one = ClientChoice::One(McpClient::Codex);
        assert_eq!(one.targets(&proj), vec![McpClient::Codex]);
        // All with project scope on a clean temp machine: likely 0 installed -> empty
        let all = ClientChoice::All;
        let t = all.targets(&proj);
        // Should only contain project-capable clients if any were installed
        for c in &t {
            assert!(c.supports_project_scope());
        }
    }

    #[test]
    fn fan_out_global_reports_added_skipped_failed() {
        use std::sync::Mutex;
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _lock = ENV_LOCK.lock().unwrap();

        let home = tempfile::tempdir().unwrap();
        let orig_home = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", home.path()) };

        // Mark opencode and cursor as installed; windsurf installed but global-only.
        fs::create_dir_all(home.path().join(".config/opencode")).unwrap();
        fs::create_dir_all(home.path().join(".cursor")).unwrap();
        fs::create_dir_all(home.path().join(".codeium/windsurf")).unwrap();

        // Pre-create a malformed file for windsurf to trigger failure.
        let windsurf_path = home.path().join(".codeium/windsurf/mcp_config.json");
        fs::write(&windsurf_path, "{ bad json").unwrap();

        let key = "test-key";
        let url = "http://localhost:4242/mcp/token";

        // First fan-out: opencode and cursor should add, windsurf should fail.
        let result = inject_many(&ClientChoice::All, &ConfigScope::Global, key, url);
        assert!(result.added.contains(&"opencode".to_string()));
        assert!(result.added.contains(&"Cursor".to_string()));
        assert!(result.failed.iter().any(|s| s.contains("Windsurf")));

        // Second fan-out: now same key should be skipped for those that succeeded.
        let result2 = inject_many(&ClientChoice::All, &ConfigScope::Global, key, url);
        assert!(result2.skipped.contains(&"opencode".to_string()));
        assert!(result2.skipped.contains(&"Cursor".to_string()));
        // windsurf still failed
        assert!(result2.failed.iter().any(|s| s.contains("Windsurf")));

        // Project scope: only opencode + cursor + claude-code are project-capable.
        // Create claude marker too
        fs::create_dir_all(home.path().join(".claude")).unwrap();
        let proj_root = tempfile::tempdir().unwrap();
        let proj_scope = ConfigScope::Project {
            root: proj_root.path().to_path_buf(),
        };
        let proj_result = inject_many(&ClientChoice::All, &proj_scope, key, url);
        // Should not include windsurf (global-only)
        assert!(!proj_result.added.contains(&"Windsurf".to_string()));
        assert!(!proj_result.skipped.contains(&"Windsurf".to_string()));

        // Cleanup HOME
        match orig_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn inject_creates_parent_dirs_atomically() {
        let dir = tmp();
        let path = dir.path().join("a/b/c/mcp.json");
        // inject_json will create parent dirs
        inject_json(
            &path,
            "mcpServers",
            "k",
            serde_json::json!({"url":"http://x"}),
            None,
        )
        .unwrap();
        assert!(path.exists());
    }
}
