//! Linux path resolution.

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
    match env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() && Path::new(&dir).is_absolute() => {
            PathBuf::from(dir).join("pluk")
        }
        _ => home().join(".config").join("pluk"),
    }
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

/// Linux installs have no `/Applications` bundles; clients are detected by
/// their config and state directories alone.
pub fn mcp_detection_paths(client: McpClient) -> Vec<PathBuf> {
    let paths = match client {
        McpClient::Opencode => vec!["~/.config/opencode", "~/.local/share/opencode"],
        McpClient::Codex => vec!["~/.codex"],
        McpClient::ClaudeCode => vec!["~/.claude", "~/.mcp.json"],
        McpClient::Cursor => vec!["~/.cursor"],
        McpClient::Windsurf => vec!["~/.codeium/windsurf"],
        McpClient::Antigravity => vec!["~/.gemini"],
    };
    paths.into_iter().map(expand_tilde).collect()
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
