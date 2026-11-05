//! macOS path resolution.

use std::env;
use std::path::{Path, PathBuf};

use system_configuration::core_foundation::array::CFArray;
use system_configuration::core_foundation::base::{CFType, TCFType};
use system_configuration::core_foundation::dictionary::CFDictionary;
use system_configuration::core_foundation::number::CFNumber;
use system_configuration::core_foundation::string::{CFString, CFStringRef};
use system_configuration::dynamic_store::SCDynamicStoreBuilder;
use system_configuration::sys::schema_definitions::{
    kSCPropNetProxiesExceptionsList, kSCPropNetProxiesHTTPSEnable, kSCPropNetProxiesHTTPSPort,
    kSCPropNetProxiesHTTPSProxy, kSCPropNetProxiesSOCKSEnable, kSCPropNetProxiesSOCKSPort,
    kSCPropNetProxiesSOCKSProxy,
};

use super::{McpClient, SystemProxy};

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
        .join("com.desgnspace.pluk")
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

/// Reads the HTTPS and SOCKS proxies set in System Settings.
pub fn system_proxy() -> Option<SystemProxy> {
    let settings = SCDynamicStoreBuilder::new("pluk").build()?.get_proxies()?;

    // SAFETY: the `kSCPropNetProxies*` keys are constant strings the framework
    // exports; reading them has no side effects.
    let (https, socks, exceptions_key) = unsafe {
        (
            proxy_address(
                &settings,
                kSCPropNetProxiesHTTPSEnable,
                kSCPropNetProxiesHTTPSProxy,
                kSCPropNetProxiesHTTPSPort,
            ),
            proxy_address(
                &settings,
                kSCPropNetProxiesSOCKSEnable,
                kSCPropNetProxiesSOCKSProxy,
                kSCPropNetProxiesSOCKSPort,
            ),
            kSCPropNetProxiesExceptionsList,
        )
    };
    if https.is_none() && socks.is_none() {
        return None;
    }

    let exceptions = settings
        .find(exceptions_key)
        .and_then(|list| list.downcast::<CFArray>())
        .map(|list| {
            list.iter()
                .filter_map(|item| {
                    // SAFETY: the array owns the item; wrapping under the get
                    // rule takes a new reference instead of stealing it.
                    unsafe { CFType::wrap_under_get_rule(*item) }.downcast::<CFString>()
                })
                .map(|host| host.to_string())
                .collect()
        })
        .unwrap_or_default();

    Some(SystemProxy {
        https,
        socks,
        exceptions,
    })
}

/// `host:port` for one proxy kind, or `None` when it is off or has no host.
fn proxy_address(
    settings: &CFDictionary<CFString, CFType>,
    enabled_key: CFStringRef,
    host_key: CFStringRef,
    port_key: CFStringRef,
) -> Option<String> {
    let enabled = settings
        .find(enabled_key)
        .and_then(|flag| flag.downcast::<CFNumber>())
        .and_then(|flag| flag.to_i32())
        == Some(1);
    if !enabled {
        return None;
    }
    let host = settings
        .find(host_key)
        .and_then(|host| host.downcast::<CFString>())
        .map(|host| host.to_string())
        .filter(|host| !host.is_empty())?;
    let port = settings
        .find(port_key)
        .and_then(|port| port.downcast::<CFNumber>())
        .and_then(|port| port.to_i32());
    Some(match port {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}
