//! Statement categories and their grouping.

/// The category of a single SQL statement.
///
/// Mirrors `StatementCategory` in `pluk/src/mcp/policy.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatementCategory {
    Select,
    Inspect,
    Insert,
    Update,
    Delete,
    Merge,
    Create,
    Alter,
    Drop,
    Truncate,
    Rename,
    Transaction,
    Session,
    Procedure,
    Maintenance,
    Grant,
}

pub const ALL_CATEGORIES: [StatementCategory; 16] = [
    StatementCategory::Select,
    StatementCategory::Inspect,
    StatementCategory::Insert,
    StatementCategory::Update,
    StatementCategory::Delete,
    StatementCategory::Merge,
    StatementCategory::Create,
    StatementCategory::Alter,
    StatementCategory::Drop,
    StatementCategory::Truncate,
    StatementCategory::Rename,
    StatementCategory::Transaction,
    StatementCategory::Session,
    StatementCategory::Procedure,
    StatementCategory::Maintenance,
    StatementCategory::Grant,
];

impl StatementCategory {
    /// The identifier stored in policies and written to the audit log
    /// (matches the TypeScript ids exactly).
    pub fn as_str(&self) -> &'static str {
        match self {
            StatementCategory::Select => "select",
            StatementCategory::Inspect => "inspect",
            StatementCategory::Insert => "insert",
            StatementCategory::Update => "update",
            StatementCategory::Delete => "delete",
            StatementCategory::Merge => "merge",
            StatementCategory::Create => "create",
            StatementCategory::Alter => "alter",
            StatementCategory::Drop => "drop",
            StatementCategory::Truncate => "truncate",
            StatementCategory::Rename => "rename",
            StatementCategory::Transaction => "transaction",
            StatementCategory::Session => "session",
            StatementCategory::Procedure => "procedure",
            StatementCategory::Maintenance => "maintenance",
            StatementCategory::Grant => "grant",
        }
    }

    /// Human-readable name used in the MCP tool description.
    pub fn display_name(&self) -> &'static str {
        match self {
            StatementCategory::Select => "SELECT",
            StatementCategory::Inspect => "DESCRIBE/EXPLAIN/SHOW",
            StatementCategory::Insert => "INSERT",
            StatementCategory::Update => "UPDATE",
            StatementCategory::Delete => "DELETE",
            StatementCategory::Merge => "MERGE/REPLACE",
            StatementCategory::Create => "CREATE",
            StatementCategory::Alter => "ALTER",
            StatementCategory::Drop => "DROP",
            StatementCategory::Truncate => "TRUNCATE",
            StatementCategory::Rename => "RENAME",
            StatementCategory::Transaction => "BEGIN/COMMIT/ROLLBACK",
            StatementCategory::Session => "SET/USE",
            StatementCategory::Procedure => "CALL/DO",
            StatementCategory::Maintenance => "VACUUM/ANALYZE",
            StatementCategory::Grant => "GRANT/REVOKE",
        }
    }

    /// Parse a stored category id. Only exact lowercase ids are valid;
    /// anything else must be rejected (fail-closed filtering).
    pub fn from_id(id: &str) -> Option<Self> {
        ALL_CATEGORIES.iter().copied().find(|c| c.as_str() == id)
    }
}

/// A named group of categories, as shown in the UI (Read / Write / Schema / Admin).
pub struct CategoryGroup {
    pub label: &'static str,
    pub categories: &'static [StatementCategory],
}

pub static CATEGORY_GROUPS: [CategoryGroup; 4] = [
    CategoryGroup {
        label: "Read",
        categories: &[StatementCategory::Select, StatementCategory::Inspect],
    },
    CategoryGroup {
        label: "Write",
        categories: &[
            StatementCategory::Insert,
            StatementCategory::Update,
            StatementCategory::Delete,
            StatementCategory::Merge,
        ],
    },
    CategoryGroup {
        label: "Schema",
        categories: &[
            StatementCategory::Create,
            StatementCategory::Alter,
            StatementCategory::Drop,
            StatementCategory::Truncate,
            StatementCategory::Rename,
        ],
    },
    CategoryGroup {
        label: "Admin",
        categories: &[
            StatementCategory::Transaction,
            StatementCategory::Session,
            StatementCategory::Procedure,
            StatementCategory::Maintenance,
            StatementCategory::Grant,
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_membership_covers_every_category_exactly_once() {
        let mut seen = Vec::new();
        for group in &CATEGORY_GROUPS {
            for cat in group.categories {
                assert!(!seen.contains(cat), "{:?} in two groups", cat);
                seen.push(*cat);
            }
        }
        seen.sort_by_key(|c| c.as_str());
        let mut all = ALL_CATEGORIES.to_vec();
        all.sort_by_key(|c| c.as_str());
        assert_eq!(seen, all);
    }

    #[test]
    fn ids_round_trip_and_reject_unknowns() {
        for cat in ALL_CATEGORIES {
            assert_eq!(StatementCategory::from_id(cat.as_str()), Some(cat));
        }
        assert_eq!(StatementCategory::from_id("SELECT"), None);
        assert_eq!(StatementCategory::from_id("foobar"), None);
        assert_eq!(StatementCategory::from_id(""), None);
    }

    #[test]
    fn display_names_match_the_ts_server() {
        assert_eq!(StatementCategory::Inspect.display_name(), "DESCRIBE/EXPLAIN/SHOW");
        assert_eq!(StatementCategory::Transaction.display_name(), "BEGIN/COMMIT/ROLLBACK");
        assert_eq!(StatementCategory::Maintenance.display_name(), "VACUUM/ANALYZE");
    }
}
