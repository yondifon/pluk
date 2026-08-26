//! SQL dialects, mapped from Pluk integration types.

/// The dialect a statement is parsed and classified under.
///
/// Mirrors `dialectFor` in `pluk/src/mcp/policy.ts`: anything unrecognized
/// falls back to PostgreSQL rather than failing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    PostgreSQL,
    MySQL,
    SQLite,
}

impl Dialect {
    /// The sqlparser dialect used to parse statements of this kind.
    pub fn sql_dialect(&self) -> &'static dyn sqlparser::dialect::Dialect {
        match self {
            Dialect::PostgreSQL => &sqlparser::dialect::PostgreSqlDialect {},
            Dialect::MySQL => &sqlparser::dialect::MySqlDialect {},
            Dialect::SQLite => &sqlparser::dialect::SQLiteDialect {},
        }
    }
}

/// Map an integration `type` to a dialect. Unknown types get PostgreSQL.
pub fn dialect_for(db_type: &str) -> Dialect {
    match db_type {
        "postgres" => Dialect::PostgreSQL,
        "mysql" => Dialect::MySQL,
        "sqlite" => Dialect::SQLite,
        _ => Dialect::PostgreSQL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integration_types_map_to_dialects() {
        assert_eq!(dialect_for("postgres"), Dialect::PostgreSQL);
        assert_eq!(dialect_for("mysql"), Dialect::MySQL);
        assert_eq!(dialect_for("sqlite"), Dialect::SQLite);
    }

    #[test]
    fn unknown_types_fall_back_to_postgres_like_the_ts_server() {
        assert_eq!(dialect_for("redis"), Dialect::PostgreSQL);
        assert_eq!(dialect_for(""), Dialect::PostgreSQL);
    }
}
