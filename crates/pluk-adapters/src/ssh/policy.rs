use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandCategory {
    Read,
    Write,
}

#[derive(Debug, Clone)]
pub struct CommandVerdict {
    pub ok: bool,
    pub category: CommandCategory,
    pub reason: Option<String>,
}

struct BinRule {
    sub_allow: Option<HashSet<String>>,
    write_subs: Option<HashSet<String>>,
    forbid_args: Option<HashSet<String>>,
    write: bool,
}

static PLAIN_READ: &[&str] = &[
    "ls", "pwd", "whoami", "hostname", "uptime", "date", "uname", "id", "w", "who",
    "df", "du", "free", "arch", "nproc", "lsb_release", "stat", "file", "readlink",
    "realpath", "tree", "cat", "head", "tail", "less", "more", "grep", "egrep",
    "fgrep", "zgrep", "zcat", "wc", "cut", "sort", "uniq", "column", "nl", "tac",
    "ps", "top", "htop", "vmstat", "iostat", "mpstat", "lsof", "ss", "netstat",
    "dmesg", "echo", "printf", "basename", "dirname",
];

fn build_allow() -> HashMap<String, BinRule> {
    let mut m: HashMap<String, BinRule> = HashMap::new();
    for &b in PLAIN_READ {
        m.insert(b.to_string(), BinRule { sub_allow: None, write_subs: None, forbid_args: None, write: false });
    }
    m.insert("find".to_string(), BinRule {
        sub_allow: None,
        write_subs: None,
        forbid_args: Some(["-exec","-execdir","-delete","-fprint","-fprintf","-ok","-okdir"].iter().map(|s| s.to_string()).collect()),
        write: false,
    });
    m.insert("tail".to_string(), BinRule {
        sub_allow: None,
        write_subs: None,
        forbid_args: Some(["-f","--follow","-F"].iter().map(|s| s.to_string()).collect()),
        write: false,
    });
    m.insert("journalctl".to_string(), BinRule {
        sub_allow: None,
        write_subs: None,
        forbid_args: Some(["-f","--follow"].iter().map(|s| s.to_string()).collect()),
        write: false,
    });
    m.insert("docker".to_string(), BinRule {
        sub_allow: Some([
            "ps","images","logs","inspect","stats","top","version","info","port","diff","history","compose","system","volume","image","container","network","node","service",
        ].iter().map(|s| s.to_string()).collect()),
        write_subs: Some(HashSet::new()),
        forbid_args: None,
        write: false,
    });
    m.insert("docker-compose".to_string(), BinRule {
        sub_allow: Some(["ps","ls","logs","config","top","images","version","port","up","start","restart"].iter().map(|s| s.to_string()).collect()),
        write_subs: Some(["up","start","restart"].iter().map(|s| s.to_string()).collect()),
        forbid_args: None,
        write: false,
    });
    m.insert("systemctl".to_string(), BinRule {
        sub_allow: Some(["status","is-active","is-enabled","is-failed","list-units","list-unit-files","show","cat","get-default"].iter().map(|s| s.to_string()).collect()),
        write_subs: None,
        forbid_args: None,
        write: false,
    });
    m.insert("git".to_string(), BinRule {
        sub_allow: Some(["status","log","diff","show","branch","remote","describe","rev-parse","tag","blame","shortlog"].iter().map(|s| s.to_string()).collect()),
        write_subs: None,
        forbid_args: None,
        write: false,
    });
    m.insert("kubectl".to_string(), BinRule {
        sub_allow: Some(["get","describe","logs","top","version","api-resources","cluster-info","explain"].iter().map(|s| s.to_string()).collect()),
        write_subs: None,
        forbid_args: None,
        write: false,
    });
    m
}

static ALLOW: OnceLock<HashMap<String, BinRule>> = OnceLock::new();
fn allow() -> &'static HashMap<String, BinRule> {
    ALLOW.get_or_init(build_allow)
}

fn docker_compose_sub() -> (&'static HashSet<String>, &'static HashSet<String>) {
    static CACHE: OnceLock<(HashSet<String>, HashSet<String>)> = OnceLock::new();
    let (a,b) = CACHE.get_or_init(|| {
        (["ps","ls","logs","config","top","images","version","port","up","start","restart"].iter().map(|s| s.to_string()).collect(),
         ["up","start","restart"].iter().map(|s| s.to_string()).collect())
    });
    (a,b)
}

static DOCKER_GROUPS: OnceLock<HashSet<String>> = OnceLock::new();
fn docker_groups() -> &'static HashSet<String> {
    DOCKER_GROUPS.get_or_init(|| ["system","volume","image","container","network","node","service"].iter().map(|s| s.to_string()).collect())
}
static DOCKER_GROUP_READ: OnceLock<HashSet<String>> = OnceLock::new();
fn docker_group_read() -> &'static HashSet<String> {
    DOCKER_GROUP_READ.get_or_init(|| ["ls","ps","inspect","df","info","logs","top","stats","history","port","diff","version","list"].iter().map(|s| s.to_string()).collect())
}

static SENSITIVE_RES: OnceLock<Vec<Regex>> = OnceLock::new();
fn sensitive() -> &'static Vec<Regex> {
    SENSITIVE_RES.get_or_init(|| vec![
        Regex::new(r"(^|/)\.env(\.[\w-]+)?$").unwrap(),
        Regex::new(r"(^|/)\.env/").unwrap(),
        Regex::new(r"\bid_(rsa|ed25519|ecdsa|dsa)\b").unwrap(),
        Regex::new(r"\.(pem|key|p12|pfx|keystore|jks)$").unwrap(),
        Regex::new(r"(^|/)\.ssh(/|$)").unwrap(),
        Regex::new(r"(^|/)\.aws(/|$)").unwrap(),
        Regex::new(r"(^|/)\.gnupg(/|$)").unwrap(),
        Regex::new(r"(^|/)\.netrc$").unwrap(),
        Regex::new(r"(^|/)\.npmrc$").unwrap(),
        Regex::new(r"/etc/(shadow|gshadow|sudoers)\b").unwrap(),
        Regex::new(r"(^|/)credentials$").unwrap(),
    ])
}

fn has_forbidden_meta(s: &str) -> Option<String> {
    if s.contains("||") { return Some("||".to_string()); }
    if let Some(m) = Regex::new(r"[;&`<>]").unwrap().find(s) {
        return Some(m.as_str().to_string());
    }
    if s.contains("$(") { return Some("$(".to_string()); }
    if s.contains("${") { return Some("${".to_string()); }
    if s.contains('\n') || s.contains('\r') { return Some("newline".to_string()); }
    if Regex::new(r"\{[^{}]*,[^{}]*\}").unwrap().is_match(s) {
        return Some("{,}".to_string());
    }
    None
}

fn tokenize(segment: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    for ch in segment.chars() {
        if let Some(q) = quote {
            if ch == q { quote = None; } else { cur.push(ch); }
            continue;
        }
        if ch == '\'' || ch == '"' { quote = Some(ch); has = true; continue; }
        if ch.is_whitespace() {
            if has || !cur.is_empty() { out.push(cur.clone()); cur.clear(); has = false; }
            continue;
        }
        cur.push(ch); has = true;
    }
    if has || !cur.is_empty() { out.push(cur); }
    out
}

fn first_subcommand(args: &[String]) -> Option<String> {
    for a in args { if !a.starts_with('-') { return Some(a.clone()); } }
    None
}

fn check_sensitive(tokens: &[String]) -> Option<String> {
    for t in tokens {
        for re in sensitive() {
            if re.is_match(t) { return Some(t.clone()); }
        }
    }
    None
}

enum SegmentResult {
    Ok(CommandCategory),
    Err(String),
}

fn check_segment(segment: &str) -> SegmentResult {
    let trimmed = segment.trim();
    if trimmed.is_empty() { return SegmentResult::Err("empty command segment".to_string()); }
    let tokens = tokenize(trimmed);
    if tokens.is_empty() { return SegmentResult::Err("empty command".to_string()); }
    let bin = tokens[0].split('/').last().unwrap_or(&tokens[0]).to_string();
    let rule = match allow().get(&bin) {
        Some(r) => r,
        None => return SegmentResult::Err(format!("command not allowed: \"{bin}\"")),
    };
    let args = &tokens[1..];

    if let Some(sensitive) = check_sensitive(&tokens) {
        return SegmentResult::Err(format!("access to sensitive path is blocked: \"{sensitive}\""));
    }

    if let Some(forbid) = &rule.forbid_args {
        for a in args {
            if forbid.contains(a) {
                return SegmentResult::Err(format!("flag not allowed for \"{bin}\": \"{a}\""));
            }
        }
    }

    if bin == "docker" {
        let sub = first_subcommand(args);
        if let Some(ref s) = sub {
            if s == "compose" {
                let idx = args.iter().position(|x| x=="compose").unwrap();
                let compose_args = &args[idx+1..];
                let csub = first_subcommand(compose_args);
                let (allow_set, write_set) = docker_compose_sub();
                match csub {
                    Some(cs) if allow_set.contains(&cs) => {
                        let cat = if write_set.contains(&cs) { CommandCategory::Write } else { CommandCategory::Read };
                        return SegmentResult::Ok(cat);
                    },
                    Some(cs) => return SegmentResult::Err(format!("docker compose subcommand not allowed: \"{cs}\"")),
                    None => return SegmentResult::Err("docker compose subcommand not allowed: \"(none)\"".to_string()),
                }
            }
            if docker_groups().contains(s) {
                let idx = args.iter().position(|x| x==s).unwrap();
                let group_args = &args[idx+1..];
                let verb = first_subcommand(group_args);
                match verb {
                    Some(v) if docker_group_read().contains(&v) => return SegmentResult::Ok(CommandCategory::Read),
                    Some(v) => return SegmentResult::Err(format!("docker {s} verb not allowed: \"{v}\" (read-only verbs only)")),
                    None => return SegmentResult::Err(format!("docker {s} verb not allowed: \"(none)\" (read-only verbs only)")),
                }
            }
        }
    }

    if let Some(sub_allow) = &rule.sub_allow {
        let sub = first_subcommand(args);
        match sub {
            Some(s) if sub_allow.contains(&s) => {
                let cat = if let Some(write_subs)=&rule.write_subs { if write_subs.contains(&s) { CommandCategory::Write } else { CommandCategory::Read } } else { CommandCategory::Read };
                return SegmentResult::Ok(cat);
            },
            Some(s) => return SegmentResult::Err(format!("subcommand not allowed for \"{bin}\": \"{s}\"")),
            None => return SegmentResult::Err(format!("subcommand not allowed for \"{bin}\": \"(none)\"")),
        }
    }

    SegmentResult::Ok(if rule.write { CommandCategory::Write } else { CommandCategory::Read })
}

pub fn evaluate_command(raw: &str) -> CommandVerdict {
    let command = raw.trim();
    if command.is_empty() { return CommandVerdict { ok: false, category: CommandCategory::Read, reason: Some("empty command".to_string()) }; }
    if command.len() > 4000 { return CommandVerdict { ok: false, category: CommandCategory::Read, reason: Some("command too long".to_string()) }; }
    if let Some(meta) = has_forbidden_meta(command) {
        return CommandVerdict { ok: false, category: CommandCategory::Read, reason: Some(format!("shell metacharacter not allowed: \"{meta}\". Chaining, redirection, and command substitution are blocked.")) };
    }
    let segments: Vec<&str> = command.split('|').collect();
    let mut category = CommandCategory::Read;
    for seg in segments {
        match check_segment(seg) {
            SegmentResult::Ok(c) => { if c==CommandCategory::Write { category=CommandCategory::Write; } },
            SegmentResult::Err(r) => return CommandVerdict { ok: false, category: CommandCategory::Read, reason: Some(r) },
        }
    }
    CommandVerdict { ok: true, category, reason: None }
}

pub fn policy_summary() -> String {
    let mut bins: Vec<String> = allow().keys().cloned().collect();
    bins.sort();
    vec![
        format!("Allowed (allowlist only): {}.", bins.join(", ")),
        "docker: inspection + `docker compose up/start/restart/ps/logs/config` — never exec/run/rm/down/kill/prune.".to_string(),
        "No shell chaining/redirection/substitution (; && || & ` $() > <). Pipes are allowed.".to_string(),
        "Reading sensitive files (.env, private keys, ~/.ssh, ~/.aws, /etc/shadow, …) is blocked.".to_string(),
    ].join("\n")
}

pub fn sanitize_working_dir(dir: &str) -> Option<String> {
    if dir.is_empty() { return None; }
    if has_forbidden_meta(dir).is_some() { return None; }
    if Regex::new(r#"[|'"\s]"#).unwrap().is_match(dir) { return None; }
    if !Regex::new(r"^[\w./@~-]+$").unwrap().is_match(dir) { return None; }
    for re in sensitive() { if re.is_match(dir) { return None; } }
    Some(dir.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_plain_read_allowed() {
        for &bin in &["ls","ps","cat"] { assert!(evaluate_command(bin).ok, "{bin} should be allowed"); }
    }
    #[test]
    fn blocked_command_rejected() {
        assert!(!evaluate_command("env").ok);
        assert!(!evaluate_command("curl https://example.com").ok);
        assert!(!evaluate_command("bash -c ls").ok);
    }
    #[test]
    fn sensitive_blocked() {
        assert!(!evaluate_command("cat .env").ok);
        assert!(!evaluate_command("cat /home/user/.ssh/id_rsa").ok);
        assert!(!evaluate_command("cat ~/.aws/credentials").ok);
        assert!(!evaluate_command("cat /etc/shadow").ok);
        assert!(!evaluate_command("cat secrets.pem").ok);
    }
    #[test]
    fn brace_expansion_smuggling_blocked() {
        assert!(!evaluate_command("cat {.env,x}").ok);
        assert!(evaluate_command("docker ps --format {{.Names}}").ok);
    }
    #[test]
    fn excluded_flags() {
        assert!(!evaluate_command("find . -exec ls {} \\;").ok);
        assert!(!evaluate_command("tail -f /var/log/syslog").ok);
    }
}
