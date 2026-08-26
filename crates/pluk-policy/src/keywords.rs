//! Comment stripping and the keyword fallback classifier.

use std::sync::LazyLock;

use regex::Regex;

use crate::category::StatementCategory;

static BLOCK_COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)/\*.*?\*/").expect("valid regex"));
static LINE_DASH_COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"--[^\n]*").expect("valid regex"));
static LINE_HASH_COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#[^\n]*").expect("valid regex"));

/// Strip comments so a comment prefix can't disguise the leading keyword.
///
/// `strip_hash` adds MySQL's `#` line form. Callers mirror the TS server:
/// the keyword fallback strips all three forms, the dangerous-construct scan
/// only block and `--` comments.
pub fn strip_comments(sql: &str, strip_hash: bool) -> String {
    let s = BLOCK_COMMENT.replace_all(sql, " ");
    let s = LINE_DASH_COMMENT.replace_all(&s, " ");
    if strip_hash {
        let s = LINE_HASH_COMMENT.replace_all(&s, " ");
        return s.into_owned();
    }
    s.into_owned()
}

/// Ordered keyword rules, mirroring `KEYWORD_MAP` in the TS policy engine —
/// first match wins.
static KEYWORD_MAP: LazyLock<Vec<(Regex, StatementCategory)>> = LazyLock::new(|| {
    use StatementCategory as C;
    let rules: &[(&str, C)] = &[
        (r"(?i)^\s*select\b", C::Select),
        (r"(?i)^\s*with\b", C::Select),
        (r"(?i)^\s*values\b", C::Select),
        (r"(?i)^\s*table\b", C::Select),
        (r"(?i)^\s*insert\b", C::Insert),
        (r"(?i)^\s*update\b", C::Update),
        (r"(?i)^\s*delete\b", C::Delete),
        (r"(?i)^\s*replace\b", C::Merge),
        (r"(?i)^\s*merge\b", C::Merge),
        (r"(?i)^\s*upsert\b", C::Merge),
        (r"(?i)^\s*create\b", C::Create),
        (r"(?i)^\s*alter\b", C::Alter),
        (r"(?i)^\s*drop\b", C::Drop),
        (r"(?i)^\s*truncate\b", C::Truncate),
        (r"(?i)^\s*rename\b", C::Rename),
        (r"(?i)^\s*copy\b", C::Insert),
        (r"(?i)^\s*begin\b", C::Transaction),
        (r"(?i)^\s*commit\b", C::Transaction),
        (r"(?i)^\s*rollback\b", C::Transaction),
        (r"(?i)^\s*savepoint\b", C::Transaction),
        (r"(?i)^\s*release\s+savepoint", C::Transaction),
        (r"(?i)^\s*set\b", C::Session),
        (r"(?i)^\s*reset\b", C::Session),
        (r"(?i)^\s*use\b", C::Session),
        (r"(?i)^\s*show\b", C::Inspect),
        (r"(?i)^\s*explain\b", C::Inspect),
        (r"(?i)^\s*describe\b", C::Inspect),
        (r"(?i)^\s*desc\b", C::Inspect),
        (r"(?i)^\s*pragma\b", C::Inspect),
        (r"(?i)^\s*call\b", C::Procedure),
        (r"(?i)^\s*exec(?:ute)?\b", C::Procedure),
        (r"(?i)^\s*do\b", C::Procedure),
        (r"(?i)^\s*vacuum\b", C::Maintenance),
        (r"(?i)^\s*analyze\b", C::Maintenance),
        (r"(?i)^\s*reindex\b", C::Maintenance),
        (r"(?i)^\s*optimize\b", C::Maintenance),
        (r"(?i)^\s*checkpoint\b", C::Maintenance),
        (r"(?i)^\s*cluster\b", C::Maintenance),
        (r"(?i)^\s*grant\b", C::Grant),
        (r"(?i)^\s*revoke\b", C::Grant),
    ];
    rules
        .iter()
        .map(|(pattern, category)| (Regex::new(pattern).expect("valid keyword regex"), *category))
        .collect()
});

/// Classify one statement by its leading keyword. `None` means unknown —
/// the caller must deny.
pub fn keyword_classify(statement: &str) -> Option<StatementCategory> {
    let stripped = strip_comments(statement, true);
    let trimmed = stripped.trim();
    KEYWORD_MAP
        .iter()
        .find(|(re, _)| re.is_match(trimmed))
        .map(|(_, category)| *category)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_keywords_classify() {
        assert_eq!(keyword_classify("SELECT 1"), Some(StatementCategory::Select));
        assert_eq!(keyword_classify("  delete from t"), Some(StatementCategory::Delete));
        assert_eq!(keyword_classify("EXECUTE p"), Some(StatementCategory::Procedure));
        assert_eq!(keyword_classify("EXEC p"), Some(StatementCategory::Procedure));
        assert_eq!(keyword_classify("RELEASE SAVEPOINT s"), Some(StatementCategory::Transaction));
        assert_eq!(keyword_classify("XYZZY"), None);
    }

    #[test]
    fn comment_prefixes_cannot_disguise_the_keyword() {
        assert_eq!(
            keyword_classify("/*c*/DELETE FROM t"),
            Some(StatementCategory::Delete)
        );
        assert_eq!(keyword_classify("-- c\nDROP TABLE t"), Some(StatementCategory::Drop));
        // MySQL's # line comment is stripped here too.
        assert_eq!(keyword_classify("# c\nDELETE FROM t"), Some(StatementCategory::Delete));
    }

    #[test]
    fn order_matters_describe_before_desc() {
        assert_eq!(keyword_classify("DESCRIBE t"), Some(StatementCategory::Inspect));
        assert_eq!(keyword_classify("DESC t"), Some(StatementCategory::Inspect));
    }
}
