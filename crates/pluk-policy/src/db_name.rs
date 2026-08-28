//! Database-name validation and the pin rule for multi-database connections.

use crate::error::PolicyError;

/// A database name is only ever passed as a connection-config value (never
/// interpolated into SQL), but validate it anyway as defense in depth against
/// a hostile identifier reaching a driver that might build a `USE`/qualified name.
///
/// Pattern: `[A-Za-z0-9_$-]{1,128}`.
pub fn is_valid_database_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '-'))
}

/// Resolve the effective database for a driver from the connection's
/// configured database and an optional per-call override. Fails closed:
///
/// - a hostile identifier is rejected;
/// - a connection *pinned* to a database at setup can never be pointed at
///   another (the override must match, or be absent).
///
/// An absent or empty override means "no override". Returns the database the
/// driver should connect to (`None` = server default for an unpinned
/// connection with no override).
pub fn resolve_override_database(
    configured: Option<&str>,
    db_override: Option<&str>,
) -> Result<Option<String>, PolicyError> {
    let Some(db_override) = db_override.filter(|o| !o.is_empty()) else {
        return Ok(configured.map(str::to_string));
    };
    if !is_valid_database_name(db_override) {
        return Err(PolicyError::InvalidDatabaseName(db_override.to_string()));
    }
    if let Some(configured) = configured
        && configured != db_override
    {
        return Err(PolicyError::DatabasePinned(configured.to_string()));
    }
    Ok(Some(db_override.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use PolicyError::*;

    #[test]
    fn accepts_plain_identifiers_rejects_injection() {
        for ok in ["app", "app_prod", "billing-2", "db$x", "A1"] {
            assert!(is_valid_database_name(ok), "{ok} should be valid");
        }
        for bad in [
            "",
            "a b",
            "a;b",
            "a`b",
            "a\"b",
            "a.b",
            "a'b",
            "a/*x*/",
            "a\nb",
            &"x".repeat(129),
        ] {
            assert!(!is_valid_database_name(bad), "{bad:?} should be invalid");
        }
    }

    #[test]
    fn no_override_falls_back_to_configured() {
        assert_eq!(
            resolve_override_database(Some("appdb"), None),
            Ok(Some("appdb".into()))
        );
        assert_eq!(resolve_override_database(None, None), Ok(None));
        // Empty override counts as absent, like JS falsiness.
        assert_eq!(
            resolve_override_database(Some("appdb"), Some("")),
            Ok(Some("appdb".into()))
        );
    }

    #[test]
    fn unpinned_connection_may_target_any_valid_database() {
        assert_eq!(
            resolve_override_database(None, Some("analytics")),
            Ok(Some("analytics".into()))
        );
    }

    #[test]
    fn pinned_connection_is_locked_to_its_database() {
        assert_eq!(
            resolve_override_database(Some("appdb"), Some("appdb")),
            Ok(Some("appdb".into()))
        );
        assert_eq!(
            resolve_override_database(Some("appdb"), Some("otherdb")),
            Err(DatabasePinned("appdb".into()))
        );
    }

    #[test]
    fn hostile_identifiers_are_rejected() {
        assert_eq!(
            resolve_override_database(None, Some("evil; DROP")),
            Err(InvalidDatabaseName("evil; DROP".into()))
        );
        assert_eq!(
            resolve_override_database(None, Some("a.b")),
            Err(InvalidDatabaseName("a.b".into()))
        );
        assert!(matches!(
            resolve_override_database(Some("appdb"), Some("other db")),
            Err(InvalidDatabaseName(_))
        ));
    }

    #[test]
    fn error_messages_match_the_ts_server() {
        assert_eq!(
            DatabasePinned("appdb".into()).to_string(),
            "Connection is locked to database \"appdb\"."
        );
        assert_eq!(
            InvalidDatabaseName("a b".into()).to_string(),
            "Invalid database name: a b"
        );
    }
}
