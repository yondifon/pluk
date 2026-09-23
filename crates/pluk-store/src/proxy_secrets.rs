//! Secret values a proxied MCP server is reached with (`proxy_secrets` table):
//! the values of header rows, and of environment variables, marked secret.
//!
//! These rows stay out of `integrations.config` for the same reason
//! `proxy_auth` does: that blob is what the window reads. The config keeps
//! each row's name and its secret flag; the value lives only here, and the
//! window learns no more than whether one is saved. Values are stored as
//! written, like every other secret in `pluk.db`.
//!
//! Writes go by name and land together: [`Store::write_proxy_secrets`]
//! applies a whole save, sets, clears and renames, in one transaction.

use std::fmt;

use rusqlite::{Row, params};

use crate::Store;
use crate::error::Result;

/// What a secret value is sent as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretKind {
    /// An HTTP header on every request to the server.
    Header,
    /// An environment variable of a server Pluk starts.
    Env,
}

impl SecretKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SecretKind::Header => "header",
            SecretKind::Env => "env",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "header" => SecretKind::Header,
            "env" => SecretKind::Env,
            _ => return None,
        })
    }
}

/// One saved secret value.
#[derive(Clone, PartialEq, Eq)]
pub struct ProxySecret {
    pub integration_id: String,
    pub kind: SecretKind,
    pub name: String,
    pub value: String,
    /// Bumped by every write to the row.
    pub version: i64,
}

impl fmt::Debug for ProxySecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxySecret")
            .field("integration_id", &self.integration_id)
            .field("kind", &self.kind)
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .field("version", &self.version)
            .finish()
    }
}

/// One change to an integration's saved secrets.
#[derive(Clone, PartialEq, Eq)]
pub enum SecretWrite {
    /// Save `value` under `name`, replacing what was there.
    Set {
        kind: SecretKind,
        name: String,
        value: String,
    },
    /// Drop the value saved under `name`, if any.
    Clear { kind: SecretKind, name: String },
    /// Move the value saved under `from` to `to`, keeping it.
    Rename {
        kind: SecretKind,
        from: String,
        to: String,
    },
}

impl fmt::Debug for SecretWrite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretWrite::Set { kind, name, .. } => f
                .debug_struct("Set")
                .field("kind", kind)
                .field("name", name)
                .field("value", &"<redacted>")
                .finish(),
            SecretWrite::Clear { kind, name } => f
                .debug_struct("Clear")
                .field("kind", kind)
                .field("name", name)
                .finish(),
            SecretWrite::Rename { kind, from, to } => f
                .debug_struct("Rename")
                .field("kind", kind)
                .field("from", from)
                .field("to", to)
                .finish(),
        }
    }
}

fn hydrate(row: &Row<'_>) -> rusqlite::Result<Option<ProxySecret>> {
    let kind: String = row.get(1)?;
    // A kind this build does not know is skipped rather than guessed at.
    let Some(kind) = SecretKind::parse(&kind) else {
        return Ok(None);
    };
    Ok(Some(ProxySecret {
        integration_id: row.get(0)?,
        kind,
        name: row.get(2)?,
        value: row.get(3)?,
        version: row.get(4)?,
    }))
}

impl Store {
    /// Every secret saved for one integration, by kind and then name.
    pub fn list_proxy_secrets(&self, integration_id: &str) -> Result<Vec<ProxySecret>> {
        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare_cached(
            "SELECT integration_id, kind, name, value, version FROM proxy_secrets
             WHERE integration_id = ? ORDER BY kind, name",
        )?;
        let rows = stmt.query_map([integration_id], hydrate)?;
        let mut secrets = Vec::new();
        for row in rows {
            secrets.extend(row?);
        }
        Ok(secrets)
    }

    /// Apply one save's changes in order, all or none.
    pub fn write_proxy_secrets(&self, integration_id: &str, writes: &[SecretWrite]) -> Result<()> {
        if writes.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock().expect("store lock");
        let tx = conn.transaction()?;
        for write in writes {
            match write {
                SecretWrite::Set { kind, name, value } => {
                    tx.execute(
                        "INSERT INTO proxy_secrets (integration_id, kind, name, value, version)
                         VALUES (?, ?, ?, ?, 1)
                         ON CONFLICT (integration_id, kind, name)
                         DO UPDATE SET value = excluded.value, version = version + 1",
                        params![integration_id, kind.as_str(), name, value],
                    )?;
                }
                SecretWrite::Clear { kind, name } => {
                    tx.execute(
                        "DELETE FROM proxy_secrets WHERE integration_id = ? AND kind = ? AND name = ?",
                        params![integration_id, kind.as_str(), name],
                    )?;
                }
                SecretWrite::Rename { kind, from, to } => {
                    tx.execute(
                        "UPDATE proxy_secrets SET name = ?, version = version + 1
                         WHERE integration_id = ? AND kind = ? AND name = ?",
                        params![to, integration_id, kind.as_str(), from],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_store;

    fn set(kind: SecretKind, name: &str, value: &str) -> SecretWrite {
        SecretWrite::Set {
            kind,
            name: name.to_owned(),
            value: value.to_owned(),
        }
    }

    fn saved(store: &Store, id: &str) -> Vec<(SecretKind, String, String, i64)> {
        store
            .list_proxy_secrets(id)
            .expect("list")
            .into_iter()
            .map(|s| (s.kind, s.name, s.value, s.version))
            .collect()
    }

    #[test]
    fn a_save_sets_renames_and_clears_by_name() {
        let (_dir, store) = temp_store();
        store
            .write_proxy_secrets(
                "int-1",
                &[
                    set(SecretKind::Header, "DD_API_KEY", "api-1"),
                    set(SecretKind::Header, "DD_APPLICATION_KEY", "app-1"),
                    set(SecretKind::Env, "DD_API_KEY", "env-1"),
                ],
            )
            .expect("first save");

        store
            .write_proxy_secrets(
                "int-1",
                &[
                    SecretWrite::Clear {
                        kind: SecretKind::Header,
                        name: "DD_APPLICATION_KEY".to_owned(),
                    },
                    SecretWrite::Rename {
                        kind: SecretKind::Header,
                        from: "DD_API_KEY".to_owned(),
                        to: "X-Api-Key".to_owned(),
                    },
                    set(SecretKind::Env, "DD_API_KEY", "env-2"),
                ],
            )
            .expect("second save");

        assert_eq!(
            saved(&store, "int-1"),
            [
                (
                    SecretKind::Env,
                    "DD_API_KEY".to_owned(),
                    "env-2".to_owned(),
                    2
                ),
                (
                    SecretKind::Header,
                    "X-Api-Key".to_owned(),
                    "api-1".to_owned(),
                    2
                ),
            ]
        );
        assert!(saved(&store, "int-2").is_empty());
    }

    #[test]
    fn a_save_that_fails_part_way_changes_nothing() {
        let (_dir, store) = temp_store();
        store
            .write_proxy_secrets(
                "int-1",
                &[
                    set(SecretKind::Header, "A", "a"),
                    set(SecretKind::Header, "B", "b"),
                ],
            )
            .expect("seed");

        let clash = store.write_proxy_secrets(
            "int-1",
            &[
                set(SecretKind::Header, "C", "c"),
                SecretWrite::Rename {
                    kind: SecretKind::Header,
                    from: "A".to_owned(),
                    to: "B".to_owned(),
                },
            ],
        );

        assert!(clash.is_err());
        assert_eq!(
            saved(&store, "int-1")
                .into_iter()
                .map(|(_, name, value, _)| (name, value))
                .collect::<Vec<_>>(),
            [
                ("A".to_owned(), "a".to_owned()),
                ("B".to_owned(), "b".to_owned())
            ]
        );
    }

    #[test]
    fn debugging_a_secret_or_a_write_shows_no_value() {
        let (_dir, store) = temp_store();
        let write = set(SecretKind::Header, "DD_API_KEY", "api-value-1");
        store
            .write_proxy_secrets("int-1", std::slice::from_ref(&write))
            .expect("save");

        let printed = format!(
            "{:?}{write:?}",
            store.list_proxy_secrets("int-1").expect("list")
        );
        assert!(!printed.contains("api-value-1"), "{printed}");
        assert!(printed.contains("DD_API_KEY"), "{printed}");
    }
}
