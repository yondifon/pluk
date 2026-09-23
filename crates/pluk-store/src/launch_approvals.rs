//! The launch a user approved for a local MCP server
//! (`proxy_launch_approvals` table).
//!
//! A local server is a command Pluk runs with the user's full access, so it
//! only runs in the exact form the user approved. The row keeps a hash of
//! that form; a launch whose hash differs is one nobody approved yet.

use rusqlite::{OptionalExtension, params};

use crate::Store;
use crate::error::Result;

impl Store {
    /// The hash of the launch the user last approved for this integration.
    pub fn approved_launch(&self, integration_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare_cached(
            "SELECT launch_hash FROM proxy_launch_approvals WHERE integration_id = ?",
        )?;
        Ok(stmt
            .query_row([integration_id], |row| row.get(0))
            .optional()?)
    }

    /// Record that the user approved the launch hashing to `launch_hash`,
    /// replacing any earlier approval.
    pub fn approve_launch(&self, integration_id: &str, launch_hash: &str) -> Result<()> {
        let conn = self.conn.lock().expect("store lock");
        conn.execute(
            "INSERT OR REPLACE INTO proxy_launch_approvals (integration_id, launch_hash)
             VALUES (?, ?)",
            params![integration_id, launch_hash],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::temp_store;

    #[test]
    fn a_new_approval_replaces_the_last_one() {
        let (_dir, store) = temp_store();
        assert_eq!(store.approved_launch("int-1").unwrap(), None);
        store.approve_launch("int-1", "hash-1").unwrap();
        store.approve_launch("int-1", "hash-2").unwrap();
        assert_eq!(
            store.approved_launch("int-1").unwrap().as_deref(),
            Some("hash-2")
        );
        assert_eq!(store.approved_launch("int-2").unwrap(), None);
    }
}
