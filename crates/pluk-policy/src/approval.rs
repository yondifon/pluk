//! Allow and deny rules an integration's owner writes by hand.
//!
//! Rules are glob patterns matched against the whole command or statement the
//! adapter is about to run, case-sensitively. A denied call is refused
//! outright; an allowed one runs even where the adapter's own policy would
//! have refused it; a call matching neither is left to that policy.
//!
//! Stored in the `query_policy` blob beside the per-tool switches:
//!
//! ```json
//! { "approvals": { "ask": true, "allow": ["git pull*"], "deny": ["rm *"] } }
//! ```

use glob::Pattern;

/// What the rules say about one command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleVerdict {
    /// A deny rule matched. Deny always wins.
    Denied,
    /// An allow rule matched and no deny rule did.
    Allowed,
    /// No rule matched; the adapter's own policy decides.
    Unmatched,
}

/// One integration's approval settings.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Approvals {
    /// Whether a refused call asks the owner before it is turned down.
    /// Asking is on until it is turned off.
    #[serde(default = "asking_is_on")]
    pub ask: bool,
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

fn asking_is_on() -> bool {
    true
}

impl Default for Approvals {
    fn default() -> Self {
        Approvals {
            ask: true,
            allow: Vec::new(),
            deny: Vec::new(),
        }
    }
}

impl Approvals {
    /// Whether this is still the untouched default, so it need not be stored.
    pub fn is_unset(&self) -> bool {
        *self == Approvals::default()
    }

    /// Match `subject` against the rules. Patterns that do not compile are
    /// skipped, so one typo cannot take the whole list down — and cannot
    /// silently widen it either, since a broken allow rule matches nothing.
    pub fn verdict(&self, subject: &str) -> RuleVerdict {
        let subject = normalize(subject);
        if matches_any(&self.deny, &subject) {
            return RuleVerdict::Denied;
        }
        if matches_any(&self.allow, &subject) {
            return RuleVerdict::Allowed;
        }
        RuleVerdict::Unmatched
    }
}

fn matches_any(patterns: &[String], subject: &str) -> bool {
    patterns.iter().any(|raw| {
        let raw = raw.trim();
        !raw.is_empty() && Pattern::new(raw).is_ok_and(|pattern| pattern.matches(subject))
    })
}

/// The form a rule is matched against: outer whitespace trimmed and every run
/// of blanks collapsed, so a command reads the same however it was spaced.
pub fn normalize(subject: &str) -> String {
    subject.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Turn one exact command into a rule that matches only itself, so "Always
/// allow" cannot widen into a pattern.
pub fn literal_rule(command: &str) -> String {
    let mut out = String::with_capacity(command.len());
    for ch in normalize(command).chars() {
        if matches!(ch, '*' | '?' | '[' | ']') {
            out.push('[');
            out.push(ch);
            out.push(']');
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(allow: &[&str], deny: &[&str]) -> Approvals {
        Approvals {
            ask: true,
            allow: allow.iter().map(|s| s.to_string()).collect(),
            deny: deny.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn an_unmatched_command_is_left_to_the_adapter() {
        assert_eq!(rules(&[], &[]).verdict("ls -la"), RuleVerdict::Unmatched);
    }

    #[test]
    fn globs_match_the_whole_command() {
        let r = rules(&["git pull*"], &[]);
        assert_eq!(r.verdict("git pull origin main"), RuleVerdict::Allowed);
        assert_eq!(r.verdict("git push origin main"), RuleVerdict::Unmatched);
        // A glob spans separators, because a command is one line of text.
        assert_eq!(
            rules(&["docker*logs*"], &[]).verdict("docker container logs api"),
            RuleVerdict::Allowed
        );
    }

    #[test]
    fn deny_beats_allow() {
        let r = rules(&["rm *"], &["rm -rf *"]);
        assert_eq!(r.verdict("rm -rf /srv"), RuleVerdict::Denied);
        assert_eq!(r.verdict("rm /tmp/x"), RuleVerdict::Allowed);
    }

    #[test]
    fn matching_is_case_sensitive() {
        assert_eq!(
            rules(&["SELECT *"], &[]).verdict("select 1"),
            RuleVerdict::Unmatched
        );
    }

    #[test]
    fn spacing_does_not_change_the_match() {
        assert_eq!(
            rules(&["git status"], &[]).verdict("  git   status  "),
            RuleVerdict::Allowed
        );
    }

    #[test]
    fn a_pattern_that_does_not_compile_matches_nothing() {
        assert_eq!(rules(&["[a-"], &[]).verdict("ls"), RuleVerdict::Unmatched);
        assert_eq!(rules(&["[a-"], &[]).verdict("[a-"), RuleVerdict::Unmatched);
    }

    #[test]
    fn blank_rules_are_ignored() {
        assert_eq!(rules(&["", "   "], &[]).verdict("ls"), RuleVerdict::Unmatched);
    }

    #[test]
    fn an_always_allow_rule_matches_only_that_command() {
        let rule = literal_rule("grep -r 'a*b' /srv");
        let r = rules(&[&rule], &[]);
        assert_eq!(r.verdict("grep -r 'a*b' /srv"), RuleVerdict::Allowed);
        assert_eq!(r.verdict("grep -r 'axxb' /srv"), RuleVerdict::Unmatched);
        assert_eq!(r.verdict("grep -r 'a*b' /etc"), RuleVerdict::Unmatched);
    }
}
