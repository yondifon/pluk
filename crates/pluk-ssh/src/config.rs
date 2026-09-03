use std::path::PathBuf;

/// Mirrors `SSHConfigEntry` in `pluk/src/ssh/config.ts`.
#[derive(Debug, Clone, Default)]
pub struct SshConfigEntry {
    pub host_name: Option<String>,
    pub identity_file: Option<String>,
    pub identity_agent: Option<String>,
    pub proxy_command: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
}

pub fn expand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = pluk_core::platform::home_dir()
    {
        return format!("{}/{}", home.display(), rest);
    }
    p.to_string()
}

/// Minimal `~/.ssh/config` reader covering Host patterns, HostName, Port,
/// User, IdentityFile and ProxyCommand with `%h`, `%p` and `%r` expansion.
///
/// First-match-wins within matching Host blocks, mirroring OpenSSH's
/// first-obtained-value rule and the TS `parseSSHConfig`.
pub fn parse_ssh_config(target_host: &str) -> SshConfigEntry {
    let config_path = pluk_core::platform::home_dir()
        .map(|h| h.join(".ssh").join("config"))
        .unwrap_or_else(|| PathBuf::from("~/.ssh/config"));
    if !config_path.exists() {
        return SshConfigEntry::default();
    }
    let content = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return SshConfigEntry::default(),
    };
    parse_ssh_config_str(&content, target_host)
}

pub fn parse_ssh_config_str(content: &str, target_host: &str) -> SshConfigEntry {
    let mut result = SshConfigEntry::default();
    let mut in_matching_block = true;

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts = split_command(line);
        if parts.is_empty() {
            continue;
        }
        let key = parts[0].to_ascii_lowercase();
        let val = parts[1..].join(" ");

        if key == "host" {
            // Host line may contain multiple patterns separated by whitespace
            in_matching_block = val
                .split_whitespace()
                .any(|p| match_ssh_pattern(p, target_host));
            continue;
        }

        if !in_matching_block {
            continue;
        }

        match key.as_str() {
            "hostname" => {
                if result.host_name.is_none() {
                    result.host_name = Some(val);
                }
            }
            "identityfile" => {
                if result.identity_file.is_none() {
                    result.identity_file = Some(expand_home(&val));
                }
            }
            "identityagent" => {
                if result.identity_agent.is_none() {
                    result.identity_agent = Some(expand_home(&val));
                }
            }
            "proxycommand" => {
                if result.proxy_command.is_none() && !val.eq_ignore_ascii_case("none") {
                    result.proxy_command = Some(val);
                }
            }
            "user" => {
                if result.user.is_none() {
                    result.user = Some(val);
                }
            }
            "port" => {
                if result.port.is_none()
                    && let Ok(p) = val.parse::<u16>()
                {
                    result.port = Some(p);
                }
            }
            _ => {}
        }
    }

    result
}

pub fn match_ssh_pattern(pattern: &str, host: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    // Convert SSH glob to regex: escape regex metachars, then * -> .*, ? -> .
    let mut re_str = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => re_str.push_str(".*"),
            '?' => re_str.push('.'),
            '.' | '+' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' | '\\' => {
                re_str.push('\\');
                re_str.push(ch);
            }
            _ => re_str.push(ch),
        }
    }
    re_str.push('$');
    // Case-insensitive
    let _re_str_lower = format!("(?i){re_str}");
    // Use simple matching without regex crate: manual glob
    glob_match(pattern, host)
}

fn glob_match(pattern: &str, text: &str) -> bool {
    // Case-insensitive glob with * and ?
    let pat = pattern.to_ascii_lowercase();
    let txt = text.to_ascii_lowercase();
    let pat_chars: Vec<char> = pat.chars().collect();
    let txt_chars: Vec<char> = txt.chars().collect();
    glob_match_inner(&pat_chars, &txt_chars)
}

fn glob_match_inner(pat: &[char], txt: &[char]) -> bool {
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut match_idx) = (None::<usize>, 0usize);
    while ti < txt.len() {
        if pi < pat.len() && (pat[pi] == '?' || pat[pi] == txt[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pat.len() && pat[pi] == '*' {
            star = Some(pi);
            match_idx = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            match_idx += 1;
            ti = match_idx;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == '*' {
        pi += 1;
    }
    pi == pat.len()
}

pub fn expand_proxy_command(template: &str, host: &str, port: u16, user: &str) -> String {
    template
        .replace("%h", host)
        .replace("%p", &port.to_string())
        .replace("%r", user)
        .replace("%u", user)
}

pub fn split_command(command: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaping = false;

    for ch in command.chars() {
        if escaping {
            current.push(ch);
            escaping = false;
            continue;
        }
        if ch == '\\' {
            escaping = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        if ch.is_whitespace() {
            if !current.is_empty() {
                args.push(current.clone());
                current.clear();
            }
            continue;
        }
        current.push(ch);
    }
    if escaping {
        current.push('\\');
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Spawn a ProxyCommand and return a duplex stream backed by its stdin/stdout.
///
/// Returns a `(DuplexStream, Child)` pair; caller must hold child to keep it alive.
pub fn spawn_proxy_command(command: &str) -> std::io::Result<tokio::process::Child> {
    let parts = split_command(command);
    if parts.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "ProxyCommand is empty",
        ));
    }
    let mut cmd = tokio::process::Command::new(&parts[0]);
    cmd.args(&parts[1..])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());
    cmd.spawn()
}

/// Resolve agent socket for host — mirrors `resolveAgentSocket` in config.ts:
/// IdentityAgent from SSH config, then env.
pub fn resolve_agent_socket(host: &str) -> Option<String> {
    let from_config = parse_ssh_config(host).identity_agent;
    if let Some(s) = from_config {
        return Some(s);
    }
    std::env::var("SSH_AUTH_SOCK")
        .ok()
        .filter(|s| !s.is_empty())
}

/// Like `expand_home` but returns PathBuf.
pub fn expand_home_path(p: &str) -> PathBuf {
    PathBuf::from(expand_home(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_home_replaces_tilde() {
        let h = pluk_core::platform::home_dir().unwrap_or_else(|| PathBuf::from("/home/test"));
        let expected = format!("{}/foo/bar", h.display());
        assert_eq!(expand_home("~/foo/bar"), expected);
        assert_eq!(expand_home("/absolute"), "/absolute");
        assert_eq!(expand_home("relative"), "relative");
    }

    #[test]
    fn match_pattern_star() {
        assert!(match_ssh_pattern("*", "anything"));
        assert!(match_ssh_pattern("*.example.com", "foo.example.com"));
        assert!(!match_ssh_pattern("*.example.com", "example.com"));
        assert!(match_ssh_pattern("bastion-*", "bastion-prod"));
        assert!(match_ssh_pattern("host?", "host1"));
        assert!(!match_ssh_pattern("host?", "host12"));
    }

    #[test]
    fn match_pattern_case_insensitive() {
        assert!(match_ssh_pattern("BASTION", "bastion"));
        assert!(match_ssh_pattern("bastion", "BASTION"));
    }

    #[test]
    fn expand_proxy_replaces_tokens() {
        let cmd = expand_proxy_command(
            "cloudflared access ssh --hostname %h --port %p --user %r",
            "db.example.com",
            22,
            "alice",
        );
        assert!(cmd.contains("db.example.com"));
        assert!(cmd.contains("22"));
        assert!(cmd.contains("alice"));
    }

    #[test]
    fn split_command_handles_quotes() {
        assert_eq!(split_command("a b c"), vec!["a", "b", "c"]);
        assert_eq!(split_command("a \"b c\" d"), vec!["a", "b c", "d"]);
        assert_eq!(split_command("a 'b c' d"), vec!["a", "b c", "d"]);
        assert_eq!(
            split_command("a \"b \\\"c\\\"\" d"),
            vec!["a", "b \"c\"", "d"]
        );
    }

    #[test]
    fn parse_config_first_match_wins() {
        let cfg = r#"
Host bastion
  HostName bastion.example.com
  User admin
  Port 2222
  IdentityAgent ~/my-agent.sock
  ProxyCommand cloudflared access ssh --hostname %h

Host *
  User defaultuser
  Port 22
"#;
        let e = parse_ssh_config_str(cfg, "bastion");
        assert_eq!(e.host_name.as_deref(), Some("bastion.example.com"));
        assert_eq!(e.user.as_deref(), Some("admin"));
        assert_eq!(e.port, Some(2222));
        assert!(
            e.identity_agent
                .as_deref()
                .unwrap()
                .ends_with("my-agent.sock")
        );
        assert!(e.proxy_command.is_some());

        let e2 = parse_ssh_config_str(cfg, "other");
        assert_eq!(e2.user.as_deref(), Some("defaultuser"));
        assert_eq!(e2.host_name, None);
    }

    #[test]
    fn parse_config_proxycommand_none() {
        let cfg = "Host *\n  ProxyCommand none\n";
        let e = parse_ssh_config_str(cfg, "any");
        assert!(e.proxy_command.is_none());
    }
}
