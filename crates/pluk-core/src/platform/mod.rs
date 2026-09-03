//! Platform abstraction for macOS and Linux.
//!
//! Every path or capability that can differ between the two supported
//! platforms is resolved through this module — no `cfg` attributes scattered
//! through the rest of the codebase.
//!
//! Contract:
//!
//! - [`data_dir`] — `~/.pluk` (override: `PLUK_DATA_DIR`), holds the SQLite
//!   databases, log file, exports, and SSH control sockets.
//! - [`app_config_dir`] — Pluk's own config directory.
//! - [`mcp_config_path`] — an MCP client's config file for a scope.
//! - [`mcp_detection_paths`] — paths whose existence marks a client installed.
//! - [`log_file`] — the append-only debug log.
//! - [`export_dir`] — where query exports are written.
//! - [`ssh_control_dir`] — OpenSSH `ControlMaster` sockets. Socket paths are
//!   copied into `sockaddr_un.sun_path`, which is 104 bytes on macOS and 108
//!   on Linux; keep every socket path under 104 bytes so both targets work.

use std::path::PathBuf;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod imp;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod imp;

/// An MCP client whose config Pluk can write itself into.
///
/// Codex config is TOML; every other client reads JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum McpClient {
    Opencode,
    Codex,
    ClaudeCode,
    Cursor,
    Windsurf,
    Antigravity,
}

/// File format of a client's config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    Json,
    Toml,
}

impl McpClient {
    pub const ALL: [McpClient; 6] = [
        McpClient::Opencode,
        McpClient::Codex,
        McpClient::ClaudeCode,
        McpClient::Cursor,
        McpClient::Windsurf,
        McpClient::Antigravity,
    ];

    /// Whether this client understands a per-repository config file.
    ///
    /// Global-only clients fall back to their global path for any scope.
    pub fn supports_project_scope(self) -> bool {
        matches!(
            self,
            McpClient::Opencode | McpClient::ClaudeCode | McpClient::Cursor
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            McpClient::Opencode => "opencode",
            McpClient::Codex => "Codex",
            McpClient::ClaudeCode => "Claude Code",
            McpClient::Cursor => "Cursor",
            McpClient::Windsurf => "Windsurf",
            McpClient::Antigravity => "Antigravity",
        }
    }

    pub fn container_key(self) -> &'static str {
        match self {
            McpClient::Opencode => "mcp",
            _ => "mcpServers",
        }
    }

    pub fn config_format(self) -> ConfigFormat {
        match self {
            McpClient::Codex => ConfigFormat::Toml,
            _ => ConfigFormat::Json,
        }
    }

    pub fn config_language(self) -> &'static str {
        match self {
            McpClient::Codex => "toml",
            _ => "json",
        }
    }

    /// Whether this client is detected on the current machine.
    pub fn is_installed(self) -> bool {
        crate::platform::mcp_detection_paths(self)
            .iter()
            .any(|p| p.exists())
    }
}

/// Which config file of a client to resolve.
pub enum ConfigScope {
    /// A per-repository file rooted at the given repository directory.
    Project { root: PathBuf },
    /// The client's single user-level file.
    Global,
}

/// The user's home directory.
pub fn home_dir() -> Option<PathBuf> {
    imp::home_dir()
}

/// Pluk's data directory: databases, logs, exports, SSH control sockets.
///
/// Honors `PLUK_DATA_DIR` so tests and headless runs never touch real data.
pub fn data_dir() -> PathBuf {
    imp::data_dir()
}

/// Pluk's own configuration directory.
pub fn app_config_dir() -> PathBuf {
    imp::app_config_dir()
}

/// The append-only debug log file.
pub fn log_file() -> PathBuf {
    imp::log_file()
}

/// Directory holding query exports.
pub fn export_dir() -> PathBuf {
    imp::export_dir()
}

/// Directory holding OpenSSH `ControlMaster` control sockets.
///
/// Every socket path built from this must stay under 104 bytes so it fits
/// `sun_path` on macOS as well as Linux.
pub fn ssh_control_dir() -> PathBuf {
    imp::ssh_control_dir()
}

/// Kill `leader_pid` and every process still in its group.
///
/// Both supported platforms are POSIX and answer to the same call, so this is
/// resolved here rather than in the per-platform modules. `leader_pid` must be
/// a process started with its own group (see [`crate::process::run_capture`])
/// and must not have been reaped yet — a reaped pid can be reused, and killing
/// its group would hit whatever inherited the number.
pub fn kill_process_group(leader_pid: u32) {
    // SAFETY: killpg only reads the pid and signal number; any error (the group
    // has already exited) is reported through the return value, not memory.
    unsafe {
        libc::killpg(leader_pid as libc::pid_t, libc::SIGKILL);
    }
}

/// An MCP client's config file for `scope`.
pub fn mcp_config_path(client: McpClient, scope: &ConfigScope) -> PathBuf {
    match scope {
        ConfigScope::Global => global_mcp_config_path(client),
        ConfigScope::Project { root } => {
            if let Some(name) = project_mcp_config_name(client) {
                root.join(name)
            } else {
                global_mcp_config_path(client)
            }
        }
    }
}

/// Paths whose existence marks `client` as installed on this machine.
pub fn mcp_detection_paths(client: McpClient) -> Vec<PathBuf> {
    imp::mcp_detection_paths(client)
}

fn global_mcp_config_path(client: McpClient) -> PathBuf {
    imp::global_mcp_config_path(client)
}

fn project_mcp_config_name(client: McpClient) -> Option<&'static str> {
    match client {
        McpClient::Opencode => Some("opencode.json"),
        McpClient::ClaudeCode => Some(".mcp.json"),
        McpClient::Cursor => Some(".cursor/mcp.json"),
        McpClient::Codex | McpClient::Windsurf | McpClient::Antigravity => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn ssh_control_dir_lives_under_data_dir() {
        assert!(ssh_control_dir().starts_with(data_dir()));
    }

    #[test]
    fn project_scopes_join_the_repo_root() {
        let root = PathBuf::from("/repo");
        let scope = ConfigScope::Project { root: root.clone() };
        assert_eq!(
            mcp_config_path(McpClient::Cursor, &scope),
            root.join(".cursor/mcp.json")
        );
    }

    #[test]
    fn global_only_clients_fall_back_to_global_path() {
        let scope = ConfigScope::Project {
            root: PathBuf::from("/repo"),
        };
        let path = mcp_config_path(McpClient::Codex, &scope);
        assert!(!path.starts_with("/repo"));
        assert!(path.ends_with("config.toml"));
    }
}
