//! Upstream credentials for one proxied MCP server (`proxy_auth` table).
//!
//! The user signs in to the upstream server once, inside Pluk; agents never
//! see a token. These rows stay out of `integrations.config` because that blob
//! is handed to the UI whole — only the proxy adapter reads this table. They
//! are stored as written, like every other secret in `pluk.db`.
//!
//! A refresh is a read-modify-write against a server that only honors a
//! refresh token once, so writes carry the version they were built from:
//! [`Store::update_proxy_tokens`] applies only when nothing else refreshed
//! first.

use std::fmt;

use rusqlite::{OptionalExtension, Row, params};

use crate::Store;
use crate::error::Result;

/// Whether the stored credentials still reach upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStatus {
    Connected,
    /// Refreshing failed for good; the user has to sign in again.
    ReconnectNeeded,
}

impl AuthStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthStatus::Connected => "connected",
            AuthStatus::ReconnectNeeded => "reconnect_needed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "connected" => AuthStatus::Connected,
            "reconnect_needed" => AuthStatus::ReconnectNeeded,
            _ => return None,
        })
    }
}

/// What a fresh sign-in leaves behind. `expires_at` is Unix milliseconds, and
/// absent when the server gave no lifetime.
pub struct ProxyAuthInput {
    pub integration_id: String,
    /// The authentication style this row records; `oauth` today.
    pub kind: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    /// Authorization-server metadata, kept verbatim so the endpoints a
    /// refresh needs survive without being re-discovered.
    pub metadata_json: Option<String>,
}

/// What a refresh returned. `refresh_token` is `None` when the server reissued
/// none, which keeps the stored one — that is the token the next refresh uses.
pub struct RefreshedTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
}

/// The stored credentials for one integration.
#[derive(Clone)]
pub struct ProxyAuth {
    pub integration_id: String,
    pub kind: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub metadata_json: Option<String>,
    pub status: AuthStatus,
    /// Bumped by every token write; pass it back to [`Store::update_proxy_tokens`].
    pub version: i64,
}

impl fmt::Debug for ProxyAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const HIDDEN: &str = "<redacted>";
        f.debug_struct("ProxyAuth")
            .field("integration_id", &self.integration_id)
            .field("kind", &self.kind)
            .field("access_token", &HIDDEN)
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| HIDDEN))
            .field("expires_at", &self.expires_at)
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.as_ref().map(|_| HIDDEN))
            .field("metadata_json", &self.metadata_json)
            .field("status", &self.status)
            .field("version", &self.version)
            .finish()
    }
}

const SELECT_BY_INTEGRATION: &str = "SELECT integration_id, kind, access_token, refresh_token, expires_at, client_id, client_secret, metadata_json, status, version FROM proxy_auth WHERE integration_id = ?";

fn hydrate(row: &Row<'_>) -> rusqlite::Result<ProxyAuth> {
    let status: String = row.get(8)?;
    Ok(ProxyAuth {
        integration_id: row.get(0)?,
        kind: row.get(1)?,
        access_token: row.get(2)?,
        refresh_token: row.get(3)?,
        expires_at: row.get(4)?,
        client_id: row.get(5)?,
        client_secret: row.get(6)?,
        metadata_json: row.get(7)?,
        // A status that cannot be read fails closed: the user signs in again.
        status: AuthStatus::parse(&status).unwrap_or(AuthStatus::ReconnectNeeded),
        version: row.get(9)?,
    })
}

impl Store {
    pub fn get_proxy_auth(&self, integration_id: &str) -> Result<Option<ProxyAuth>> {
        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare_cached(SELECT_BY_INTEGRATION)?;
        Ok(stmt.query_row([integration_id], hydrate).optional()?)
    }

    /// Record a fresh sign-in, replacing whatever was there. The row starts
    /// connected at version 1.
    pub fn set_proxy_auth(&self, input: &ProxyAuthInput) -> Result<()> {
        let conn = self.conn.lock().expect("store lock");
        conn.execute(
            "INSERT OR REPLACE INTO proxy_auth (integration_id, kind, access_token, refresh_token, expires_at, client_id, client_secret, metadata_json, status, version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1)",
            params![
                input.integration_id,
                input.kind,
                input.access_token,
                input.refresh_token,
                input.expires_at,
                input.client_id,
                input.client_secret,
                input.metadata_json,
                AuthStatus::Connected.as_str(),
            ],
        )?;
        Ok(())
    }

    /// Store refreshed tokens, but only if the row is still at
    /// `expected_version`. Returns whether this caller's refresh is the one
    /// that landed; a `false` means another refresh got there first and its
    /// tokens are the live ones.
    pub fn update_proxy_tokens(
        &self,
        integration_id: &str,
        expected_version: i64,
        tokens: &RefreshedTokens,
    ) -> Result<bool> {
        let conn = self.conn.lock().expect("store lock");
        Ok(conn.execute(
            "UPDATE proxy_auth SET access_token = ?, refresh_token = COALESCE(?, refresh_token), expires_at = ?, version = version + 1
             WHERE integration_id = ? AND version = ?",
            params![
                tokens.access_token,
                tokens.refresh_token,
                tokens.expires_at,
                integration_id,
                expected_version,
            ],
        )? > 0)
    }

    pub fn set_proxy_auth_status(&self, integration_id: &str, status: AuthStatus) -> Result<bool> {
        let conn = self.conn.lock().expect("store lock");
        Ok(conn.execute(
            "UPDATE proxy_auth SET status = ? WHERE integration_id = ?",
            params![status.as_str(), integration_id],
        )? > 0)
    }

    pub fn delete_proxy_auth(&self, integration_id: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("store lock");
        Ok(conn.execute(
            "DELETE FROM proxy_auth WHERE integration_id = ?",
            [integration_id],
        )? > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_store;

    fn connected(integration_id: &str) -> ProxyAuthInput {
        ProxyAuthInput {
            integration_id: integration_id.to_owned(),
            kind: "oauth".to_owned(),
            access_token: "access-1".to_owned(),
            refresh_token: Some("refresh-1".to_owned()),
            expires_at: Some(1_000),
            client_id: Some("client-1".to_owned()),
            client_secret: Some("secret-1".to_owned()),
            metadata_json: Some("{\"token_endpoint\":\"https://up/token\"}".to_owned()),
        }
    }

    #[test]
    fn a_refresh_built_on_a_stale_version_does_not_land() {
        let (_dir, store) = temp_store();
        store.set_proxy_auth(&connected("int-1")).expect("connect");
        let stored = store.get_proxy_auth("int-1").expect("read").expect("row");
        assert_eq!(stored.version, 1);

        let winner = RefreshedTokens {
            access_token: "access-2".to_owned(),
            refresh_token: Some("refresh-2".to_owned()),
            expires_at: Some(2_000),
        };
        assert!(store.update_proxy_tokens("int-1", 1, &winner).expect("refresh"));

        let loser = RefreshedTokens {
            access_token: "access-stale".to_owned(),
            refresh_token: Some("refresh-stale".to_owned()),
            expires_at: Some(3_000),
        };
        assert!(!store.update_proxy_tokens("int-1", 1, &loser).expect("stale refresh"));

        let stored = store.get_proxy_auth("int-1").expect("read").expect("row");
        assert_eq!(stored.access_token, "access-2");
        assert_eq!(stored.refresh_token.as_deref(), Some("refresh-2"));
        assert_eq!(stored.expires_at, Some(2_000));
        assert_eq!(stored.version, 2);
    }

    #[test]
    fn a_refresh_without_a_new_refresh_token_keeps_the_stored_one() {
        let (_dir, store) = temp_store();
        store.set_proxy_auth(&connected("int-1")).expect("connect");
        let tokens = RefreshedTokens {
            access_token: "access-2".to_owned(),
            refresh_token: None,
            expires_at: None,
        };
        assert!(store.update_proxy_tokens("int-1", 1, &tokens).expect("refresh"));

        let stored = store.get_proxy_auth("int-1").expect("read").expect("row");
        assert_eq!(stored.refresh_token.as_deref(), Some("refresh-1"));
        assert_eq!(stored.expires_at, None);
    }

    #[test]
    fn signing_in_again_replaces_the_row_and_resets_the_version() {
        let (_dir, store) = temp_store();
        store.set_proxy_auth(&connected("int-1")).expect("connect");
        let tokens = RefreshedTokens {
            access_token: "access-2".to_owned(),
            refresh_token: None,
            expires_at: None,
        };
        store.update_proxy_tokens("int-1", 1, &tokens).expect("refresh");
        assert!(
            store
                .set_proxy_auth_status("int-1", AuthStatus::ReconnectNeeded)
                .expect("status")
        );

        store.set_proxy_auth(&connected("int-1")).expect("reconnect");
        let stored = store.get_proxy_auth("int-1").expect("read").expect("row");
        assert_eq!(stored.version, 1);
        assert_eq!(stored.status, AuthStatus::Connected);
        assert_eq!(stored.access_token, "access-1");
    }

    #[test]
    fn debugging_a_row_shows_no_token_or_secret() {
        let (_dir, store) = temp_store();
        store.set_proxy_auth(&connected("int-1")).expect("connect");
        let stored = store.get_proxy_auth("int-1").expect("read").expect("row");

        let printed = format!("{stored:?}");
        for secret in ["access-1", "refresh-1", "secret-1"] {
            assert!(!printed.contains(secret), "{printed}");
        }
        assert!(printed.contains("client-1"), "{printed}");
    }

    #[test]
    fn a_missing_row_reads_and_writes_as_nothing() {
        let (_dir, store) = temp_store();
        assert!(store.get_proxy_auth("int-1").expect("read").is_none());
        let tokens = RefreshedTokens {
            access_token: "access-1".to_owned(),
            refresh_token: None,
            expires_at: None,
        };
        assert!(!store.update_proxy_tokens("int-1", 1, &tokens).expect("refresh"));
        assert!(!store.set_proxy_auth_status("int-1", AuthStatus::Connected).expect("status"));
        assert!(!store.delete_proxy_auth("int-1").expect("delete"));
    }
}
