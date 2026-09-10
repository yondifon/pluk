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

#[derive(Default)]
struct BinRule {
    sub_allow: Option<HashSet<String>>,
    write_subs: Option<HashSet<String>>,
    forbid_args: Option<HashSet<String>>,
    /// Most positional arguments the command may take. Commands that treat a
    /// trailing positional as an output file (`uniq in out`) are capped here.
    max_positional: Option<usize>,
    write: bool,
}

fn set(items: &[&str]) -> HashSet<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

static PLAIN_READ: &[&str] = &[
    "ls",
    "pwd",
    "whoami",
    "hostname",
    "uptime",
    "date",
    "uname",
    "id",
    "w",
    "who",
    "df",
    "du",
    "free",
    "arch",
    "nproc",
    "lsb_release",
    "stat",
    "file",
    "readlink",
    "realpath",
    "tree",
    "cat",
    "head",
    "tail",
    "less",
    "more",
    "grep",
    "egrep",
    "fgrep",
    "zgrep",
    "zcat",
    "wc",
    "cut",
    "sort",
    "uniq",
    "column",
    "nl",
    "tac",
    "ps",
    "top",
    "htop",
    "vmstat",
    "iostat",
    "mpstat",
    "lsof",
    "ss",
    "netstat",
    "dmesg",
    "echo",
    "printf",
    "basename",
    "dirname",
];

fn build_allow() -> HashMap<String, BinRule> {
    let mut m: HashMap<String, BinRule> = HashMap::new();
    for &b in PLAIN_READ {
        m.insert(b.to_string(), BinRule::default());
    }
    let mut rule = |bin: &str, r: BinRule| {
        m.insert(bin.to_string(), r);
    };
    rule(
        "find",
        BinRule {
            forbid_args: Some(set(&[
                "-exec",
                "-execdir",
                "-delete",
                "-fprint",
                "-fprint0",
                "-fprintf",
                "-fls",
                "-ok",
                "-okdir",
            ])),
            ..Default::default()
        },
    );
    // Follow modes never return, so they hold the connection open.
    rule(
        "tail",
        BinRule {
            forbid_args: Some(set(&["-f", "--follow", "-F", "--retry"])),
            ..Default::default()
        },
    );
    rule(
        "journalctl",
        BinRule {
            forbid_args: Some(set(&["-f", "--follow", "--rotate", "--vacuum-size"])),
            ..Default::default()
        },
    );
    // `sort -o` and a second `uniq` path are file writes wearing a read.
    rule(
        "sort",
        BinRule {
            forbid_args: Some(set(&["-o", "--output"])),
            ..Default::default()
        },
    );
    rule(
        "uniq",
        BinRule {
            max_positional: Some(1),
            ..Default::default()
        },
    );
    // `dmesg -C` empties the kernel ring buffer.
    rule(
        "dmesg",
        BinRule {
            forbid_args: Some(set(&["-C", "--clear", "-c", "--read-clear"])),
            ..Default::default()
        },
    );
    rule(
        "docker",
        BinRule {
            sub_allow: Some(set(&[
                "ps",
                "images",
                "logs",
                "inspect",
                "stats",
                "top",
                "version",
                "info",
                "port",
                "diff",
                "history",
                "compose",
                "system",
                "volume",
                "image",
                "container",
                "network",
                "node",
                "service",
            ])),
            write_subs: Some(HashSet::new()),
            ..Default::default()
        },
    );
    rule(
        "docker-compose",
        BinRule {
            sub_allow: Some(set(&[
                "ps", "ls", "logs", "config", "top", "images", "version", "port", "up", "start",
                "restart",
            ])),
            write_subs: Some(set(&["up", "start", "restart"])),
            ..Default::default()
        },
    );
    rule(
        "systemctl",
        BinRule {
            sub_allow: Some(set(&[
                "status",
                "is-active",
                "is-enabled",
                "is-failed",
                "list-units",
                "list-unit-files",
                "show",
                "cat",
                "get-default",
            ])),
            ..Default::default()
        },
    );
    // `-c` and `--exec-path` reconfigure git into running arbitrary helpers;
    // the delete and force flags turn a read subcommand into a write.
    rule(
        "git",
        BinRule {
            sub_allow: Some(set(&[
                "status", "log", "diff", "show", "branch", "remote", "describe", "rev-parse",
                "tag", "blame", "shortlog",
            ])),
            forbid_args: Some(set(&[
                "-c",
                "-C",
                "--exec-path",
                "--upload-pack",
                "--ext-diff",
                "-d",
                "-D",
                "--delete",
                "-m",
                "-M",
                "--move",
                "-f",
                "--force",
                "-o",
                "--output",
            ])),
            ..Default::default()
        },
    );
    rule(
        "kubectl",
        BinRule {
            sub_allow: Some(set(&[
                "get",
                "describe",
                "logs",
                "top",
                "version",
                "api-resources",
                "cluster-info",
                "explain",
            ])),
            forbid_args: Some(set(&["-f", "--follow"])),
            ..Default::default()
        },
    );
    m
}

/// Read-only verbs of `git remote`; anything else rewrites the remote list.
static GIT_REMOTE_READ: OnceLock<HashSet<String>> = OnceLock::new();
fn git_remote_read() -> &'static HashSet<String> {
    GIT_REMOTE_READ.get_or_init(|| set(&["show", "get-url"]))
}

static ALLOW: OnceLock<HashMap<String, BinRule>> = OnceLock::new();
fn allow() -> &'static HashMap<String, BinRule> {
    ALLOW.get_or_init(build_allow)
}

fn docker_compose_sub() -> (&'static HashSet<String>, &'static HashSet<String>) {
    static CACHE: OnceLock<(HashSet<String>, HashSet<String>)> = OnceLock::new();
    let (a, b) = CACHE.get_or_init(|| {
        (
            [
                "ps", "ls", "logs", "config", "top", "images", "version", "port", "up", "start",
                "restart",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            ["up", "start", "restart"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        )
    });
    (a, b)
}

static DOCKER_GROUPS: OnceLock<HashSet<String>> = OnceLock::new();
fn docker_groups() -> &'static HashSet<String> {
    DOCKER_GROUPS.get_or_init(|| {
        [
            "system",
            "volume",
            "image",
            "container",
            "network",
            "node",
            "service",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    })
}
static DOCKER_GROUP_READ: OnceLock<HashSet<String>> = OnceLock::new();
fn docker_group_read() -> &'static HashSet<String> {
    DOCKER_GROUP_READ.get_or_init(|| {
        [
            "ls", "ps", "inspect", "df", "info", "logs", "top", "stats", "history", "port", "diff",
            "version", "list",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    })
}

static SENSITIVE_RES: OnceLock<Vec<Regex>> = OnceLock::new();
fn sensitive() -> &'static Vec<Regex> {
    SENSITIVE_RES.get_or_init(|| {
        vec![
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
        ]
    })
}

/// A shell character the checker refuses to reason about. Everything here
/// either runs a second command, redirects output, or changes what the words
/// mean after this check has read them.
fn metacharacter_reason(ch: char) -> Option<&'static str> {
    match ch {
        ';' => Some("`;`"),
        '&' => Some("`&`"),
        '<' => Some("`<`"),
        '>' => Some("`>`"),
        '`' => Some("`` ` ``"),
        '$' => Some("`$`"),
        '\\' => Some("`\\`"),
        '(' => Some("`(`"),
        ')' => Some("`)`"),
        '\n' | '\r' => Some("a newline"),
        '\0' => Some("a null byte"),
        _ => None,
    }
}

/// One command of a pipeline, already split into words.
type Segment = Vec<String>;

/// Split a command line into pipeline segments of words.
///
/// This is the whole basis of the check: what the remote shell will run has to
/// be what was read here. So single quotes are literal, double quotes carry
/// only literal text, and every character that would make the shell reinterpret
/// the line — escapes, expansions, substitution, redirection, chaining — ends
/// the parse instead of being skipped over.
fn split_pipeline(command: &str) -> Result<Vec<Segment>, String> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut words: Segment = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;

    let flush_word = |word: &mut String, started: &mut bool, words: &mut Segment| {
        if *started || !word.is_empty() {
            words.push(std::mem::take(word));
            *started = false;
        }
    };

    for ch in command.chars() {
        match quote {
            // Single quotes are literal all the way to the closing quote, so
            // nothing inside them can change what the shell runs.
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                } else if ch == '\0' {
                    return Err("a null byte is not allowed".to_string());
                } else {
                    word.push(ch);
                }
            }
            // Double quotes still expand `$`, `` ` `` and `\`.
            Some('"') => {
                match ch {
                    '"' => quote = None,
                    '$' | '`' | '\\' | '\0' => {
                        return Err(format!(
                            "`{ch}` keeps its shell meaning inside double quotes, so it is not allowed"
                        ));
                    }
                    _ => word.push(ch),
                }
            }
            Some(_) => unreachable!("only ' and \" open a quote"),
            None => {
                if ch == '\'' || ch == '"' {
                    quote = Some(ch);
                    started = true;
                } else if let Some(name) = metacharacter_reason(ch) {
                    return Err(format!(
                        "{name} is not allowed. Chaining, redirection, escapes and \
                         command substitution are blocked — send one command, \
                         optionally through pipes."
                    ));
                } else if ch == '|' {
                    flush_word(&mut word, &mut started, &mut words);
                    segments.push(std::mem::take(&mut words));
                } else if ch.is_whitespace() {
                    flush_word(&mut word, &mut started, &mut words);
                } else {
                    word.push(ch);
                }
            }
        }
    }
    if quote.is_some() {
        return Err("a quote is left open".to_string());
    }
    flush_word(&mut word, &mut started, &mut words);
    segments.push(words);
    Ok(segments)
}

/// Brace expansion turns one word into several, so a checked word is not the
/// word that runs.
fn brace_expansion() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{[^{}]*,[^{}]*\}").unwrap())
}

/// Directories a command may be named from. A bare name resolves through the
/// remote `PATH`; anything else has to be a system binary, so a file dropped
/// in a writable directory cannot borrow an allowed name.
static SYSTEM_BIN_DIRS: &[&str] = &[
    "/bin/",
    "/sbin/",
    "/usr/bin/",
    "/usr/sbin/",
    "/usr/local/bin/",
    "/usr/local/sbin/",
];

fn resolve_bin(word: &str) -> Result<String, String> {
    if !word.contains('/') {
        return Ok(word.to_string());
    }
    for dir in SYSTEM_BIN_DIRS {
        if let Some(name) = word.strip_prefix(dir)
            && !name.is_empty()
            && !name.contains('/')
        {
            return Ok(name.to_string());
        }
    }
    Err(format!(
        "command path not allowed: \"{word}\". Use the command's name, or its \
         path under a system directory such as /usr/bin."
    ))
}

/// A wildcard in a path component that starts with `.` could stand in for a
/// hidden credential directory the sensitive-path check would otherwise catch.
/// Wildcards elsewhere are left alone — the shell never expands them into
/// hidden names.
fn wildcard_hides_a_hidden_path(word: &str) -> bool {
    word.split('/')
        .any(|part| part.starts_with('.') && part.contains(['*', '?', '[']))
}

/// The flag itself, without its `=value` tail.
fn flag_name(arg: &str) -> &str {
    arg.split('=').next().unwrap_or(arg)
}

/// Whether `arg` carries a forbidden flag, in any of the forms a shell accepts:
/// on its own, with an attached value, or bundled into a short-flag cluster.
fn carries_forbidden_flag(arg: &str, forbid: &HashSet<String>) -> Option<String> {
    let name = flag_name(arg);
    if forbid.contains(name) {
        return Some(name.to_string());
    }
    if let Some(cluster) = name.strip_prefix('-')
        && !name.starts_with("--")
    {
        for ch in cluster.chars() {
            let single = format!("-{ch}");
            if forbid.contains(&single) {
                return Some(single);
            }
        }
    }
    None
}

fn first_subcommand(args: &[String]) -> Option<String> {
    for a in args {
        if !a.starts_with('-') {
            return Some(a.clone());
        }
    }
    None
}

fn check_sensitive(tokens: &[String]) -> Option<String> {
    for t in tokens {
        for re in sensitive() {
            if re.is_match(t) {
                return Some(t.clone());
            }
        }
    }
    None
}

enum SegmentResult {
    Ok(CommandCategory),
    Err(String),
}

fn check_segment(tokens: &[String]) -> SegmentResult {
    let Some(first) = tokens.first() else {
        return SegmentResult::Err("empty command".to_string());
    };
    for token in tokens {
        if brace_expansion().is_match(token) {
            return SegmentResult::Err(format!(
                "brace expansion is not allowed: \"{token}\""
            ));
        }
        if wildcard_hides_a_hidden_path(token) {
            return SegmentResult::Err(format!(
                "a wildcard cannot stand in for a hidden path: \"{token}\""
            ));
        }
    }
    let bin = match resolve_bin(first) {
        Ok(bin) => bin,
        Err(reason) => return SegmentResult::Err(reason),
    };
    let rule = match allow().get(&bin) {
        Some(r) => r,
        None => return SegmentResult::Err(format!("command not allowed: \"{bin}\"")),
    };
    let args = &tokens[1..];

    if let Some(sensitive) = check_sensitive(tokens) {
        return SegmentResult::Err(format!(
            "access to sensitive path is blocked: \"{sensitive}\""
        ));
    }

    if let Some(forbid) = &rule.forbid_args {
        for a in args {
            if let Some(flag) = carries_forbidden_flag(a, forbid) {
                return SegmentResult::Err(format!("flag not allowed for \"{bin}\": \"{flag}\""));
            }
        }
    }

    if let Some(max) = rule.max_positional {
        let positional = args.iter().filter(|a| !a.starts_with('-')).count();
        if positional > max {
            return SegmentResult::Err(format!(
                "\"{bin}\" takes at most {max} file here — a further one would be written to"
            ));
        }
    }

    if bin == "docker" {
        let sub = first_subcommand(args);
        if let Some(ref s) = sub {
            if s == "compose" {
                let idx = args.iter().position(|x| x == "compose").unwrap();
                let compose_args = &args[idx + 1..];
                let csub = first_subcommand(compose_args);
                let (allow_set, write_set) = docker_compose_sub();
                match csub {
                    Some(cs) if allow_set.contains(&cs) => {
                        let cat = if write_set.contains(&cs) {
                            CommandCategory::Write
                        } else {
                            CommandCategory::Read
                        };
                        return SegmentResult::Ok(cat);
                    }
                    Some(cs) => {
                        return SegmentResult::Err(format!(
                            "docker compose subcommand not allowed: \"{cs}\""
                        ));
                    }
                    None => {
                        return SegmentResult::Err(
                            "docker compose subcommand not allowed: \"(none)\"".to_string(),
                        );
                    }
                }
            }
            if docker_groups().contains(s) {
                let idx = args.iter().position(|x| x == s).unwrap();
                let group_args = &args[idx + 1..];
                let verb = first_subcommand(group_args);
                match verb {
                    Some(v) if docker_group_read().contains(&v) => {
                        return SegmentResult::Ok(CommandCategory::Read);
                    }
                    Some(v) => {
                        return SegmentResult::Err(format!(
                            "docker {s} verb not allowed: \"{v}\" (read-only verbs only)"
                        ));
                    }
                    None => {
                        return SegmentResult::Err(format!(
                            "docker {s} verb not allowed: \"(none)\" (read-only verbs only)"
                        ));
                    }
                }
            }
        }
    }

    if bin == "git"
        && first_subcommand(args).as_deref() == Some("remote")
        && let Some(idx) = args.iter().position(|x| x == "remote")
        && let Some(verb) = first_subcommand(&args[idx + 1..])
        && !git_remote_read().contains(&verb)
    {
        return SegmentResult::Err(format!(
            "git remote verb not allowed: \"{verb}\" (read-only verbs only)"
        ));
    }

    if let Some(sub_allow) = &rule.sub_allow {
        let sub = first_subcommand(args);
        match sub {
            Some(s) if sub_allow.contains(&s) => {
                let cat = if let Some(write_subs) = &rule.write_subs {
                    if write_subs.contains(&s) {
                        CommandCategory::Write
                    } else {
                        CommandCategory::Read
                    }
                } else {
                    CommandCategory::Read
                };
                return SegmentResult::Ok(cat);
            }
            Some(s) => {
                return SegmentResult::Err(format!("subcommand not allowed for \"{bin}\": \"{s}\""));
            }
            None => {
                return SegmentResult::Err(format!(
                    "subcommand not allowed for \"{bin}\": \"(none)\""
                ));
            }
        }
    }

    SegmentResult::Ok(if rule.write {
        CommandCategory::Write
    } else {
        CommandCategory::Read
    })
}

fn blocked(reason: String) -> CommandVerdict {
    CommandVerdict {
        ok: false,
        category: CommandCategory::Read,
        reason: Some(reason),
    }
}

pub fn evaluate_command(raw: &str) -> CommandVerdict {
    let command = raw.trim();
    if command.is_empty() {
        return blocked("empty command".to_string());
    }
    if command.len() > 4000 {
        return blocked("command too long".to_string());
    }
    let segments = match split_pipeline(command) {
        Ok(segments) => segments,
        Err(reason) => return blocked(reason),
    };
    let mut category = CommandCategory::Read;
    for segment in &segments {
        match check_segment(segment) {
            SegmentResult::Ok(c) => {
                if c == CommandCategory::Write {
                    category = CommandCategory::Write;
                }
            }
            SegmentResult::Err(r) => return blocked(r),
        }
    }
    CommandVerdict {
        ok: true,
        category,
        reason: None,
    }
}

/// The single description of what [`evaluate_command`] permits, shown to a
/// connecting agent as the integration's policy line. Follows the same
/// `Allowed: … Guards: …` shape the SQL adapters use.
pub fn policy_summary() -> String {
    "Allowed: read-only commands — files and logs (ls, cat, tail, grep, find), \
processes and resources (ps, df, free, lsof, ss), and the read subcommands of \
docker, systemctl, git, journalctl and kubectl; `docker compose up/start/restart` \
is the one command that changes the host. \
Guards: pipes only — no chaining, redirection, escapes, variables or command \
substitution; a command runs by name or from a system directory, never from an \
arbitrary path; paths that hold credentials cannot be read. \
Anything else comes back as `Blocked:` with the reason."
        .to_string()
}

/// The working directory a command may be run from. It is spliced into a `cd`
/// before the command, so it is held to a plain path with no shell meaning of
/// its own.
pub fn sanitize_working_dir(dir: &str) -> Option<String> {
    static SAFE_PATH: OnceLock<Regex> = OnceLock::new();
    let safe = SAFE_PATH.get_or_init(|| Regex::new(r"^[\w./@~-]+$").unwrap());
    if dir.is_empty() || !safe.is_match(dir) {
        return None;
    }
    if sensitive().iter().any(|re| re.is_match(dir)) {
        return None;
    }
    Some(dir.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reason(command: &str) -> String {
        let verdict = evaluate_command(command);
        assert!(!verdict.ok, "{command:?} should have been blocked");
        verdict.reason.expect("a blocked command carries a reason")
    }

    fn allowed(command: &str) {
        let verdict = evaluate_command(command);
        assert!(verdict.ok, "{command:?} should be allowed: {:?}", verdict.reason);
    }

    #[test]
    fn all_plain_read_allowed() {
        for bin in ["ls", "ps", "cat"] {
            allowed(bin);
        }
    }

    #[test]
    fn blocked_command_rejected() {
        for command in ["env", "curl https://example.com", "bash -c ls"] {
            reason(command);
        }
    }

    #[test]
    fn sensitive_blocked() {
        for command in [
            "cat .env",
            "cat /home/user/.ssh/id_rsa",
            "cat ~/.aws/credentials",
            "cat /etc/shadow",
            "cat secrets.pem",
        ] {
            reason(command);
        }
    }

    #[test]
    fn brace_expansion_smuggling_blocked() {
        reason("cat {.env,x}");
        allowed("docker ps --format {{.Names}}");
    }

    #[test]
    fn excluded_flags() {
        reason("find . -exec ls {} ;");
        reason("tail -f /var/log/syslog");
    }

    #[test]
    fn chaining_and_redirection_are_refused() {
        for command in [
            "ls; rm -rf /",
            "ls && rm -rf /",
            "ls || rm -rf /",
            "ls & rm -rf /",
            "cat file > /etc/passwd",
            "cat < /etc/shadow",
            "ls\nrm -rf /",
            "ls\rrm -rf /",
        ] {
            reason(command);
        }
    }

    #[test]
    fn command_substitution_is_refused() {
        for command in [
            "ls $(rm -rf /)",
            "ls `rm -rf /`",
            "ls ${HOME}",
            "cat $HOME/.ssh/id_rsa",
            "echo \"$(whoami)\"",
            "ls (echo hi)",
        ] {
            reason(command);
        }
    }

    /// `$'…'` is expanded by the shell, so the hex bytes are the real path and
    /// the literal text the checker sees is not.
    #[test]
    fn ansi_c_quoting_cannot_hide_a_sensitive_path() {
        reason(r"cat $'\x2e\x65\x6e\x76'");
    }

    /// A backslash disappears before the shell resolves the word, so `\.env`
    /// and `.env` name the same file.
    #[test]
    fn backslash_escapes_cannot_hide_a_sensitive_path() {
        reason(r"cat \.env");
        reason(r"cat /etc/sha\dow");
        reason(r"cat .en\v");
    }

    #[test]
    fn quoting_cannot_hide_a_sensitive_path() {
        for command in ["cat '.env'", "cat \".env\"", "cat .e\"\"nv", "cat '.e'nv"] {
            reason(command);
        }
    }

    #[test]
    fn an_open_quote_is_refused() {
        reason("cat \".env");
        reason("cat '.env");
    }

    /// Interpreters and argument-runners are the shortest route to any blocked
    /// command, so none of them is an allowed name.
    #[test]
    fn interpreters_and_prefixes_are_not_allowed_names() {
        for command in [
            "sh -c 'rm -rf /'",
            "bash -c 'rm -rf /'",
            "zsh -c ls",
            "eval ls",
            "xargs rm",
            "env rm -rf /",
            "sudo rm -rf /",
            "nice rm -rf /",
            "nohup rm -rf /",
            "timeout 5 rm -rf /",
            "python3 -c 'import os'",
            "perl -e 'unlink'",
            "awk 'BEGIN{system(\"rm\")}'",
        ] {
            let verdict = evaluate_command(command);
            assert!(!verdict.ok, "{command:?} should have been blocked");
        }
    }

    /// A command named by path could be any file the SSH user can write, so a
    /// path only counts when it is a system directory.
    #[test]
    fn only_system_paths_may_name_a_command() {
        allowed("/bin/ls");
        allowed("/usr/bin/ls -la");
        reason("/tmp/ls");
        reason("./ls");
        reason("../ls");
        reason("/home/deploy/bin/cat /etc/hostname");
        reason("/bin/rm -rf /");
        // A system directory names one binary, not a tree under it.
        reason("/usr/bin/../../tmp/ls");
    }

    /// A wildcard that could expand into a hidden credential directory is the
    /// same read as naming it.
    #[test]
    fn wildcards_cannot_stand_in_for_hidden_paths() {
        reason("cat /home/user/.ss*/id_rsa");
        reason("cat .en*");
        reason("cat /root/.*/credentials");
        // Wildcards the shell never expands into hidden names still work.
        allowed("tail -n 100 /var/log/*.log");
        allowed("ls *.txt");
    }

    /// A flag is the same flag whether it stands alone, carries a value or
    /// rides in a cluster.
    #[test]
    fn forbidden_flags_are_caught_in_every_form() {
        reason("tail --follow=name /var/log/syslog");
        reason("tail -qf /var/log/syslog");
        reason("journalctl --follow");
        reason("kubectl logs -f pod");
        allowed("tail -n 50 /var/log/syslog");
    }

    /// Reading commands that can also write a file are held to reading.
    #[test]
    fn read_commands_cannot_write_a_file() {
        reason("sort -o /etc/passwd /tmp/in");
        reason("sort --output=/etc/passwd /tmp/in");
        reason("uniq /tmp/in /etc/passwd");
        reason("dmesg -C");
        reason("dmesg --clear");
        allowed("sort /tmp/in");
        allowed("uniq /tmp/in");
    }

    /// Allowed git subcommands still carry flags and verbs that rewrite the
    /// repository or run a helper program.
    #[test]
    fn git_read_subcommands_cannot_write() {
        reason("git branch -D main");
        reason("git branch --delete main");
        reason("git tag -d v1");
        reason("git remote add origin https://example.com/x.git");
        reason("git remote set-url origin https://example.com/x.git");
        reason("git -c core.pager=rm log");
        reason("git --exec-path=/tmp status");
        allowed("git status");
        allowed("git log --oneline");
        allowed("git remote show origin");
    }

    #[test]
    fn pipes_still_work_and_every_stage_is_checked() {
        allowed("ps aux | grep nginx | wc -l");
        reason("ps aux | rm -rf /");
        reason("cat /etc/hosts |");
        // A pipe inside quotes is text, not a second command.
        allowed("grep 'a|b' /etc/hosts");
    }

    #[test]
    fn write_category_survives_a_pipeline() {
        let verdict = evaluate_command("docker compose up -d | cat");
        assert!(verdict.ok);
        assert_eq!(verdict.category, CommandCategory::Write);
    }

    #[test]
    fn working_dir_rejects_anything_with_shell_meaning() {
        assert_eq!(sanitize_working_dir("/srv/app"), Some("/srv/app".to_string()));
        for dir in [
            "",
            "/srv/app; rm -rf /",
            "/srv/$(whoami)",
            "/srv/app && ls",
            "/srv/my app",
            "/home/user/.ssh",
            "/srv/`whoami`",
        ] {
            assert_eq!(sanitize_working_dir(dir), None, "{dir:?} should be refused");
        }
    }
}
