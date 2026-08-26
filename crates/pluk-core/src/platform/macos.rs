//! macOS path resolution.

use std::env;
use std::path::{Path, PathBuf};

use super::McpClient;

pub fn home_dir() -> Option<PathBuf> {
    env::home_dir()
}

pub fn data_dir() -> PathBuf {
    match env::var_os("PLUK_DATA_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home().join(".pluk"),
    }
}

pub fn app_config_dir() -> PathBuf {
    home()
        .join("Library")
        .join("Application Support")
        .join("com.pluk.app")
}

pub fn log_file() -> PathBuf {
    data_dir().join("pluk.log")
}

pub fn export_dir() -> PathBuf {
    data_dir().join("exports")
}

pub fn ssh_control_dir() -> PathBuf {
    data_dir().join("ssh-control")
}

pub fn global_mcp_config_path(client: McpClient) -> PathBuf {
    let raw = match client {
        McpClient::Opencode => "~/.config/opencode/opencode.json",
        McpClient::Codex => "~/.codex/config.toml",
        McpClient::ClaudeCode => "~/.mcp.json",
        McpClient::Cursor => "~/.cursor/mcp.json",
        McpClient::Windsurf => "~/.codeium/windsurf/mcp_config.json",
        McpClient::Antigravity => "~/.gemini/config/mcp_config.json",
    };
    expand_tilde(raw)
}

pub fn mcp_detection_paths(client: McpClient) -> Vec<PathBuf> {
    let mut paths = match client {
        McpClient::Opencode => vec!["~/.config/opencode", "~/.local/share/opencode"],
        McpClient::Codex => vec!["~/.codex"],
        McpClient::ClaudeCode => vec!["~/.claude", "~/.mcp.json"],
        McpClient::Cursor => vec!["~/.cursor", "/Applications/Cursor.app"],
        McpClient::Windsurf => vec!["~/.codeium/windsurf", "/Applications/Windsurf.app"],
        McpClient::Antigravity => vec!["~/.gemini", "/Applications/Antigravity.app"],
    }
    .into_iter()
    .map(expand_tilde)
    .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

fn home() -> PathBuf {
    home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

fn expand_tilde(raw: &str) -> PathBuf {
    match raw.strip_prefix("~/") {
        Some(rest) => home().join(Path::new(rest)),
        None => PathBuf::from(raw),
    }
}
