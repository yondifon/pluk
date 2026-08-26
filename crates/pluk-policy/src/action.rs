//! Action policy for non-SQL adapters (Linear, Sentry, Redis, …): coarse
//! read/write/delete/admin gating over tool calls instead of SQL statements.

/// What a tool call does to the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionCategory {
    Read,
    Write,
    Delete,
    Admin,
}

impl ActionCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActionCategory::Read => "read",
            ActionCategory::Write => "write",
            ActionCategory::Delete => "delete",
            ActionCategory::Admin => "admin",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        match id {
            "read" => Some(ActionCategory::Read),
            "write" => Some(ActionCategory::Write),
            "delete" => Some(ActionCategory::Delete),
            "admin" => Some(ActionCategory::Admin),
            _ => None,
        }
    }
}

/// The set of action categories an integration may use.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionPolicy {
    pub allowed: Vec<ActionCategory>,
}

/// Derive an action policy from the stored `query_policy` + `read_only` flag.
///
/// Forward-compatible: an explicit `{ "actions": [...] }` blob wins and is
/// the only way to grant `admin`. Otherwise the flag decides — read-only gets
/// `[read]`, anything else `[read, write, delete]` (the binary toggle means
/// "may the agent modify state", which includes deleting). `admin` is never
/// granted implicitly.
pub fn parse_action_policy(raw: Option<&str>, read_only: bool) -> ActionPolicy {
    let explicit = raw
        .filter(|r| !r.is_empty())
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|parsed| parsed.get("actions").and_then(|a| a.as_array()).cloned())
        .map(|actions| {
            actions
                .iter()
                .filter_map(|a| a.as_str())
                .filter_map(ActionCategory::from_id)
                .collect::<Vec<_>>()
        })
        .filter(|allowed| !allowed.is_empty());
    if let Some(allowed) = explicit {
        return ActionPolicy { allowed };
    }
    let allowed = if read_only {
        vec![ActionCategory::Read]
    } else {
        vec![ActionCategory::Read, ActionCategory::Write, ActionCategory::Delete]
    };
    ActionPolicy { allowed }
}

pub fn action_allowed(policy: &ActionPolicy, category: ActionCategory) -> bool {
    policy.allowed.contains(&category)
}

pub fn action_policy_description(policy: &ActionPolicy) -> String {
    let list = policy.allowed.iter().map(ActionCategory::as_str).collect::<Vec<_>>().join(", ");
    format!("Allowed actions: {list}.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_only_defaults() {
        assert_eq!(
            parse_action_policy(None, true).allowed,
            vec![ActionCategory::Read]
        );
        assert_eq!(
            parse_action_policy(Some(""), false).allowed,
            vec![ActionCategory::Read, ActionCategory::Write, ActionCategory::Delete]
        );
        // admin is never granted implicitly.
        assert!(!action_allowed(
            &parse_action_policy(None, false),
            ActionCategory::Admin
        ));
    }

    #[test]
    fn explicit_action_list_wins_and_can_grant_admin() {
        let policy = parse_action_policy(Some(r#"{"actions":["read","admin"]}"#), false);
        assert_eq!(policy.allowed, vec![ActionCategory::Read, ActionCategory::Admin]);
        assert!(action_allowed(&policy, ActionCategory::Admin));
        assert!(!action_allowed(&policy, ActionCategory::Write));
    }

    #[test]
    fn unknown_actions_are_filtered_and_empty_lists_fall_back() {
        let policy = parse_action_policy(Some(r#"{"actions":["read","purge"]}"#), true);
        assert_eq!(policy.allowed, vec![ActionCategory::Read]);
        // All-unknown → falls back to the flag.
        assert_eq!(
            parse_action_policy(Some(r#"{"actions":["purge"]}"#), false).allowed,
            vec![ActionCategory::Read, ActionCategory::Write, ActionCategory::Delete]
        );
    }

    #[test]
    fn invalid_json_falls_back_to_the_flag() {
        assert_eq!(
            parse_action_policy(Some("not-json"), true).allowed,
            vec![ActionCategory::Read]
        );
    }

    #[test]
    fn description_matches_ts_format() {
        assert_eq!(
            action_policy_description(&parse_action_policy(None, false)),
            "Allowed actions: read, write, delete."
        );
    }
}
