//! The agent-facing guidance block every MCP server returns in its discovery
//! result. Built per request from live config + policy so a connecting agent
//! learns what the integration is, how it is constrained right now, and which
//! tools to reach for first.
//!
//! Kept terse: agents read this verbatim, so every line must earn its place.

use pluk_store::Environment;

/// The pieces an adapter supplies; the rest of the block is shared.
#[derive(Debug, Clone)]
pub struct InstructionParts {
    /// Adapter label, e.g. `PostgreSQL`, `Linear`.
    pub kind: String,
    /// One line on the access / safety model.
    pub access: String,
    /// Live policy or permission summary (`Current policy: …`).
    pub policy: Option<String>,
    /// Adapter workflow guidance (the agent hint).
    pub hint: Option<String>,
    /// Discovery: which tools to use first.
    pub start: Option<String>,
}

/// Assemble the guidance block exactly as the TypeScript server does:
/// header line, access line, then optional policy / start / hint lines.
pub fn build_instructions(name: &str, environment: Option<Environment>, parts: InstructionParts) -> String {
    let mut header = format!("{} integration \"{name}\"", parts.kind);
    if let Some(environment) = environment {
        header.push_str(&format!(" — {environment} environment"));
    }
    header.push('.');
    let mut lines = vec![header, parts.access];
    if let Some(policy) = parts.policy {
        lines.push(format!("Current policy: {policy}"));
    }
    if let Some(start) = parts.start {
        lines.push(start);
    }
    if let Some(hint) = parts.hint {
        lines.push(hint);
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts() -> InstructionParts {
        InstructionParts {
            kind: "PostgreSQL".into(),
            access: "Read-only by default.".into(),
            policy: None,
            hint: None,
            start: None,
        }
    }

    #[test]
    fn minimal_block_is_header_and_access() {
        let text = build_instructions("Main DB", Some(Environment::Production), parts());
        assert_eq!(text, "PostgreSQL integration \"Main DB\" — production environment.\nRead-only by default.");
    }

    #[test]
    fn absent_environment_omits_the_suffix() {
        let text = build_instructions("Main DB", None, parts());
        assert!(text.starts_with("PostgreSQL integration \"Main DB\".\n"));
    }

    #[test]
    fn optional_lines_append_in_fixed_order() {
        let p = InstructionParts { policy: Some("Enabled tools: query.".into()), start: Some("Start with list_tables.".into()), hint: Some("Prefer sample_table.".into()), ..parts() };
        let text = build_instructions("DB", None, p);
        assert_eq!(
            text,
            "PostgreSQL integration \"DB\".\nRead-only by default.\nCurrent policy: Enabled tools: query.\nStart with list_tables.\nPrefer sample_table."
        );
    }
}
