//! Statement classification: AST parse per dialect with a keyword fallback.

use sqlparser::parser::Parser;
use std::sync::LazyLock;

use crate::ast::{statement_category, update_or_delete_without_where};
use crate::category::StatementCategory;
use crate::dangerous::DangerousConstruct;
use crate::dialect::Dialect;
use crate::keywords::keyword_classify;

static FALLBACK_MUTATION: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)^\s*(update|delete)\b").expect("valid regex"));
static FALLBACK_WHERE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\bwhere\b").expect("valid regex"));

/// Result of classifying one SQL string.
#[derive(Debug, Clone)]
pub struct ClassifyResult {
    /// One entry per statement; `None` marks a statement whose type could not
    /// be identified (the evaluator denies those).
    pub categories: Vec<Option<StatementCategory>>,
    pub statement_count: usize,
    pub has_update_or_delete_without_where: bool,
    pub dangerous: Option<DangerousConstruct>,
    pub has_go_batch: bool,
}

/// Classify a SQL string: attempt a full AST parse for the dialect and fall
/// back to the keyword classifier when parsing fails. Fail-closed: unknown
/// input yields `None` categories.
///
/// Unlike the TS engine, an empty statement list (empty or semicolon-only
/// input) is denied rather than allowed.
pub fn classify(sql: &str, dialect: Dialect) -> ClassifyResult {
    let dangerous = crate::dangerous::scan_dangerous(sql);
    let has_go_batch = crate::keywords::has_go_batch(sql);

    let mut categories = Vec::new();
    let mut has_update_or_delete_without_where = false;

    match Parser::parse_sql(dialect.sql_dialect(), sql) {
        Ok(stmts) if !stmts.is_empty() => {
            for stmt in &stmts {
                if update_or_delete_without_where(stmt) {
                    has_update_or_delete_without_where = true;
                }
                categories.push(statement_category(stmt));
            }
        }
        _ => {
            // Parse failed (or produced nothing): split on semicolons and
            // classify each part by keyword, mirroring the TS fallback. The
            // WHERE check runs on the raw part, as it does in TS.
            let parts: Vec<&str> = sql
                .split(';')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .collect();
            if parts.is_empty() {
                categories.push(None);
            } else {
                for part in parts {
                    categories.push(keyword_classify(part));
                    let starts_mutation = FALLBACK_MUTATION.is_match(part);
                    if starts_mutation && !FALLBACK_WHERE.is_match(part) {
                        has_update_or_delete_without_where = true;
                    }
                }
            }
        }
    }

    ClassifyResult {
        statement_count: categories.len(),
        has_update_or_delete_without_where,
        categories,
        dangerous,
        has_go_batch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::category::ALL_CATEGORIES;
    use StatementCategory::*;

    fn cats(sql: &str, dialect: Dialect) -> Vec<Option<StatementCategory>> {
        classify(sql, dialect).categories
    }

    #[test]
    fn every_category_is_reachable() {
        let samples: [(Dialect, &str); 16] = [
            (Dialect::PostgreSQL, "SELECT * FROM t"),
            (Dialect::PostgreSQL, "EXPLAIN SELECT 1"),
            (Dialect::PostgreSQL, "INSERT INTO t VALUES (1)"),
            (Dialect::PostgreSQL, "UPDATE t SET x=1 WHERE id=1"),
            (Dialect::PostgreSQL, "DELETE FROM t WHERE id=1"),
            (Dialect::MySQL, "REPLACE INTO t VALUES (1)"),
            (Dialect::PostgreSQL, "CREATE TABLE t (id int)"),
            (Dialect::PostgreSQL, "ALTER TABLE t ADD COLUMN x INT"),
            (Dialect::PostgreSQL, "DROP TABLE t"),
            (Dialect::PostgreSQL, "TRUNCATE TABLE t"),
            (Dialect::MySQL, "RENAME TABLE a TO b"),
            (Dialect::PostgreSQL, "BEGIN"),
            (Dialect::PostgreSQL, "SET search_path = public"),
            (Dialect::PostgreSQL, "CALL my_proc()"),
            (Dialect::SQLite, "VACUUM"),
            (Dialect::PostgreSQL, "GRANT SELECT ON t TO u"),
        ];
        let covered: Vec<_> = samples
            .iter()
            .map(|(d, s)| classify(s, *d).categories[0])
            .collect();
        for cat in ALL_CATEGORIES {
            assert!(covered.contains(&Some(cat)), "{} unreachable", cat.as_str());
        }
    }

    #[test]
    fn postgres_statements_classify_like_the_ts_suite() {
        assert_eq!(
            cats("SELECT * FROM t", Dialect::PostgreSQL),
            vec![Some(Select)]
        );
        assert_eq!(
            cats("WITH x AS (SELECT 1) SELECT * FROM x", Dialect::PostgreSQL),
            vec![Some(Select)]
        );
        assert_eq!(
            cats("INSERT INTO t VALUES (1)", Dialect::PostgreSQL),
            vec![Some(Insert)]
        );
        assert_eq!(cats("DROP TABLE t", Dialect::PostgreSQL), vec![Some(Drop)]);
        assert_eq!(
            cats("ALTER TABLE t ADD COLUMN x INT", Dialect::PostgreSQL),
            vec![Some(Alter)]
        );
        assert_eq!(
            cats("TRUNCATE TABLE t", Dialect::PostgreSQL),
            vec![Some(Truncate)]
        );
        assert_eq!(cats("BEGIN", Dialect::PostgreSQL), vec![Some(Transaction)]);
        assert_eq!(
            cats("GRANT SELECT ON t TO u", Dialect::PostgreSQL),
            vec![Some(Grant)]
        );
        assert_eq!(
            cats("REVOKE SELECT ON t FROM u", Dialect::PostgreSQL),
            vec![Some(Grant)]
        );
        assert_eq!(
            cats("SET search_path = public", Dialect::PostgreSQL),
            vec![Some(Session)]
        );
        assert_eq!(
            cats("RESET search_path", Dialect::PostgreSQL),
            vec![Some(Session)]
        );
        assert_eq!(
            cats("CALL my_proc()", Dialect::PostgreSQL),
            vec![Some(Procedure)]
        );
        assert_eq!(
            cats("SHOW search_path", Dialect::PostgreSQL),
            vec![Some(Inspect)]
        );
    }

    #[test]
    fn statements_the_ts_parser_cannot_read_still_land_in_the_right_category() {
        // node-sql-parser fails all of these; the Rust parser reads them via AST.
        assert_eq!(
            cats("EXPLAIN SELECT 1", Dialect::PostgreSQL),
            vec![Some(Inspect)]
        );
        assert_eq!(cats("VACUUM", Dialect::PostgreSQL), vec![Some(Maintenance)]);
        assert_eq!(
            cats("ANALYZE", Dialect::PostgreSQL),
            vec![Some(Maintenance)]
        );
        assert_eq!(
            cats("SAVEPOINT s", Dialect::PostgreSQL),
            vec![Some(Transaction)]
        );
        assert_eq!(
            cats("RELEASE SAVEPOINT s", Dialect::PostgreSQL),
            vec![Some(Transaction)]
        );
        assert_eq!(cats("VALUES (1)", Dialect::PostgreSQL), vec![Some(Select)]);
        assert_eq!(cats("TABLE users", Dialect::PostgreSQL), vec![Some(Select)]);
        assert_eq!(
            cats("PRAGMA table_info(t)", Dialect::SQLite),
            vec![Some(Inspect)]
        );
        assert_eq!(
            cats(
                "MERGE INTO t USING s ON t.id=s.id WHEN MATCHED THEN UPDATE SET x=1",
                Dialect::PostgreSQL
            ),
            vec![Some(Merge)]
        );
    }

    #[test]
    fn mysql_and_sqlite_classify() {
        assert_eq!(
            cats("REPLACE INTO t VALUES (1)", Dialect::MySQL),
            vec![Some(Merge)]
        );
        assert_eq!(cats("SHOW TABLES", Dialect::MySQL), vec![Some(Inspect)]);
        assert_eq!(cats("DESCRIBE t", Dialect::MySQL), vec![Some(Inspect)]);
        assert_eq!(cats("USE mydb", Dialect::MySQL), vec![Some(Session)]);
        assert_eq!(cats("SELECT 1", Dialect::SQLite), vec![Some(Select)]);
        assert_eq!(
            cats("PRAGMA table_info(t)", Dialect::SQLite),
            vec![Some(Inspect)]
        );
    }

    #[test]
    fn where_detection_on_update_and_delete() {
        let r = classify("UPDATE t SET x=1 WHERE id=1", Dialect::PostgreSQL);
        assert!(!r.has_update_or_delete_without_where);
        let r = classify("UPDATE t SET x=1", Dialect::PostgreSQL);
        assert_eq!(r.categories, vec![Some(Update)]);
        assert!(r.has_update_or_delete_without_where);
        let r = classify("DELETE FROM t", Dialect::PostgreSQL);
        assert!(r.has_update_or_delete_without_where);
        // EXPLAIN wraps but does not propagate the inner flag, like the TS engine.
        let r = classify("EXPLAIN DELETE FROM t", Dialect::PostgreSQL);
        assert!(!r.has_update_or_delete_without_where);
    }

    #[test]
    fn stacked_statements_produce_one_category_each() {
        let r = classify("SELECT 1; DROP TABLE t", Dialect::PostgreSQL);
        assert_eq!(r.statement_count, 2);
        assert!(r.categories.contains(&Some(Select)));
        assert!(r.categories.contains(&Some(Drop)));
    }

    #[test]
    fn comment_prefix_bypass_blocked() {
        let r = classify("/*c*/DELETE FROM t", Dialect::PostgreSQL);
        assert_eq!(r.categories, vec![Some(Delete)]);
        let r = classify("-- c\nDELETE FROM t", Dialect::PostgreSQL);
        assert_eq!(r.categories, vec![Some(Delete)]);
        let r = classify("# c\nDELETE FROM t", Dialect::MySQL);
        assert_eq!(r.categories, vec![Some(Delete)]);
    }

    #[test]
    fn unparseable_input_fails_closed() {
        let r = classify("XYZZY FROBNICATOR 42", Dialect::PostgreSQL);
        assert_eq!(r.categories, vec![None]);
        assert_eq!(r.statement_count, 1);
    }

    #[test]
    fn empty_input_fails_closed() {
        // The TS engine allows "" (zero statements → vacuous pass); this port
        // denies it instead — deny-more is permitted, allow-less is not.
        for sql in ["", "   ", ";"] {
            let r = classify(sql, Dialect::PostgreSQL);
            assert_eq!(r.categories, vec![None], "{sql:?} must be denied");
        }
    }

    #[test]
    fn recognized_but_unmapped_syntax_is_denied() {
        // These parse cleanly but have no safe category, mirroring the TS
        // engine's unmapped types (lock/comment/execute/load_data/attach/…).
        for sql in [
            ("LOCK TABLE t", Dialect::PostgreSQL),
            ("COMMENT ON TABLE t IS 'x'", Dialect::PostgreSQL),
            ("EXECUTE p", Dialect::PostgreSQL),
            ("DEALLOCATE p", Dialect::PostgreSQL),
            ("PREPARE p AS SELECT 1", Dialect::PostgreSQL),
            ("LOAD DATA INFILE '/tmp/x' INTO TABLE t", Dialect::MySQL),
            ("ATTACH DATABASE '/tmp/x' AS aux", Dialect::SQLite),
            ("KILL 123", Dialect::MySQL),
            ("FLUSH PRIVILEGES", Dialect::MySQL),
            ("LISTEN chan", Dialect::PostgreSQL),
        ] {
            let r = classify(sql.0, sql.1);
            assert!(
                r.categories.iter().all(|c| c.is_none()),
                "{sql:?} should have no category"
            );
        }
    }

    #[test]
    fn dangerous_constructs_are_reported_alongside_categories() {
        let r = classify("COPY t FROM PROGRAM 'ls'", Dialect::PostgreSQL);
        assert_eq!(r.dangerous, Some(DangerousConstruct::CopyProgram));
        let r = classify("SELECT pg_read_file('/etc/passwd')", Dialect::PostgreSQL);
        assert_eq!(r.dangerous, Some(DangerousConstruct::PgReadFile));
        let r = classify("SELECT 1", Dialect::PostgreSQL);
        assert_eq!(r.dangerous, None);
    }

    #[test]
    fn mssql_statements_use_tsql_dialect() {
        assert_eq!(cats("SELECT TOP (10) * FROM users", Dialect::MSSQL), vec![Some(Select)]);
        let update = classify("UPDATE users SET active = 0", Dialect::MSSQL);
        assert!(update.has_update_or_delete_without_where);
        assert_eq!(cats("UPDATE users SET active = 0 WHERE id = 1", Dialect::MSSQL), vec![Some(Update)]);
    }

    #[test]
    fn mssql_batches_and_server_capabilities_are_detected() {
        assert!(classify("SELECT 1\nGO\nSELECT 2", Dialect::MSSQL).has_go_batch);
        assert_eq!(
            classify("EXEC xp_cmdshell 'whoami'", Dialect::MSSQL).dangerous,
            Some(DangerousConstruct::XpCmdshell)
        );
        assert_eq!(
            classify("BULK INSERT users FROM '/tmp/users.csv'", Dialect::MSSQL).dangerous,
            Some(DangerousConstruct::BulkInsert)
        );
        assert_eq!(
            classify("SELECT * FROM OPENROWSET(BULK '/tmp/users.csv')", Dialect::MSSQL).dangerous,
            Some(DangerousConstruct::Openrowset)
        );
    }
}
