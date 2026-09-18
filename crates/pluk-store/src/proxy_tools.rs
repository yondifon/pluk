//! The tool snapshot of an upstream MCP server (`proxy_tools` table).
//!
//! Discovery talks to the upstream server and writes what it found here;
//! registration reads it back synchronously. Nothing an upstream server
//! offers is exposed to agents until the user approves that exact tool, so an
//! approval is bound to a content hash: the same name coming back with a new
//! description or schema reads as [`ToolState::Changed`] until approved again.

use rusqlite::{Row, params};
use serde::Serialize;

use crate::Store;
use crate::error::Result;

/// One tool as the upstream server just described it. `content_hash` covers
/// name, description, schema and annotations, and is computed by the caller —
/// the store compares hashes but never defines them.
#[derive(Debug, Clone)]
pub struct DiscoveredTool {
    pub name: String,
    pub description: String,
    pub schema_json: String,
    pub annotations_json: Option<String>,
    pub content_hash: String,
}

/// Where a tool stands between what upstream offers and what the user
/// approved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolState {
    /// Never approved.
    New,
    /// Approved, and unchanged since.
    Approved,
    /// Approved earlier, but upstream now describes it differently.
    Changed,
    /// Upstream stopped listing it.
    Missing,
}

/// One row of the snapshot.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProxyTool {
    pub integration_id: String,
    pub name: String,
    pub description: String,
    pub schema_json: String,
    pub annotations_json: Option<String>,
    pub content_hash: String,
    pub approved_hash: Option<String>,
    /// False once upstream stopped listing the tool; the row stays so an
    /// approval survives a server that is briefly incomplete.
    pub present: bool,
    pub discovered_at: String,
    pub updated_at: String,
}

impl ProxyTool {
    /// Only [`ToolState::Approved`] tools may be called.
    pub fn state(&self) -> ToolState {
        if !self.present {
            return ToolState::Missing;
        }
        match &self.approved_hash {
            None => ToolState::New,
            Some(hash) if *hash == self.content_hash => ToolState::Approved,
            Some(_) => ToolState::Changed,
        }
    }
}

const SELECT_FOR_INTEGRATION: &str = "SELECT integration_id, name, description, schema_json, annotations_json, content_hash, approved_hash, present, discovered_at, updated_at FROM proxy_tools WHERE integration_id = ? ORDER BY name";

const UPSERT: &str = "INSERT INTO proxy_tools (integration_id, name, description, schema_json, annotations_json, content_hash, present, discovered_at, updated_at)
     VALUES (?, ?, ?, ?, ?, ?, 1, datetime('now'), datetime('now'))
     ON CONFLICT(integration_id, name) DO UPDATE SET
         description = excluded.description,
         schema_json = excluded.schema_json,
         annotations_json = excluded.annotations_json,
         content_hash = excluded.content_hash,
         present = 1,
         updated_at = datetime('now')";

const APPROVE: &str = "UPDATE proxy_tools SET approved_hash = content_hash, updated_at = datetime('now') WHERE integration_id = ? AND name = ?";

const REVOKE: &str = "UPDATE proxy_tools SET approved_hash = NULL, updated_at = datetime('now') WHERE integration_id = ? AND name = ?";

fn hydrate(row: &Row<'_>) -> rusqlite::Result<ProxyTool> {
    Ok(ProxyTool {
        integration_id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        schema_json: row.get(3)?,
        annotations_json: row.get(4)?,
        content_hash: row.get(5)?,
        approved_hash: row.get(6)?,
        present: row.get(7)?,
        discovered_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

impl Store {
    pub fn list_proxy_tools(&self, integration_id: &str) -> Result<Vec<ProxyTool>> {
        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare_cached(SELECT_FOR_INTEGRATION)?;
        let rows = stmt.query_map([integration_id], hydrate)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Replace the snapshot with what discovery just saw, in one transaction.
    ///
    /// Approvals survive: a tool that comes back unchanged stays approved, one
    /// that comes back different reads as [`ToolState::Changed`], and one that
    /// is gone is marked absent rather than deleted.
    pub fn replace_proxy_tools(
        &self,
        integration_id: &str,
        discovered: &[DiscoveredTool],
    ) -> Result<()> {
        let conn = self.conn.lock().expect("store lock");
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE proxy_tools SET present = 0, updated_at = datetime('now') WHERE integration_id = ? AND present = 1",
            [integration_id],
        )?;
        {
            let mut stmt = tx.prepare(UPSERT)?;
            for tool in discovered {
                stmt.execute(params![
                    integration_id,
                    tool.name,
                    tool.description,
                    tool.schema_json,
                    tool.annotations_json,
                    tool.content_hash,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Approve the named tools as they stand now. Names the snapshot does not
    /// hold are ignored; the count is how many rows were approved.
    pub fn approve_proxy_tools(&self, integration_id: &str, names: &[String]) -> Result<usize> {
        self.update_named_tools(integration_id, names, APPROVE)
    }

    /// Withdraw approval, leaving the tools listed but not callable.
    pub fn revoke_proxy_tools(&self, integration_id: &str, names: &[String]) -> Result<usize> {
        self.update_named_tools(integration_id, names, REVOKE)
    }

    fn update_named_tools(
        &self,
        integration_id: &str,
        names: &[String],
        sql: &str,
    ) -> Result<usize> {
        let conn = self.conn.lock().expect("store lock");
        let tx = conn.unchecked_transaction()?;
        let mut changed = 0;
        {
            let mut stmt = tx.prepare(sql)?;
            for name in names {
                changed += stmt.execute(params![integration_id, name])?;
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    pub fn delete_proxy_tools(&self, integration_id: &str) -> Result<usize> {
        let conn = self.conn.lock().expect("store lock");
        Ok(conn.execute(
            "DELETE FROM proxy_tools WHERE integration_id = ?",
            [integration_id],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_store;

    fn discovered(name: &str, hash: &str) -> DiscoveredTool {
        DiscoveredTool {
            name: name.to_owned(),
            description: format!("{name} does something"),
            schema_json: "{\"type\":\"object\"}".to_owned(),
            annotations_json: None,
            content_hash: hash.to_owned(),
        }
    }

    fn states(store: &Store, integration_id: &str) -> Vec<(String, ToolState)> {
        store
            .list_proxy_tools(integration_id)
            .expect("list")
            .into_iter()
            .map(|tool| (tool.name.clone(), tool.state()))
            .collect()
    }

    #[test]
    fn a_new_snapshot_keeps_approvals_and_flags_what_moved() {
        let (_dir, store) = temp_store();
        store
            .replace_proxy_tools(
                "int-1",
                &[
                    discovered("steady", "hash-steady"),
                    discovered("edited", "hash-edited"),
                    discovered("vanishing", "hash-vanishing"),
                ],
            )
            .expect("first snapshot");
        assert_eq!(
            store.approve_proxy_tools(
                "int-1",
                &["steady".to_owned(), "edited".to_owned(), "vanishing".to_owned()],
            )
            .expect("approve"),
            3
        );

        store
            .replace_proxy_tools(
                "int-1",
                &[
                    discovered("steady", "hash-steady"),
                    discovered("edited", "hash-edited-v2"),
                    discovered("fresh", "hash-fresh"),
                ],
            )
            .expect("second snapshot");

        assert_eq!(
            states(&store, "int-1"),
            [
                ("edited".to_owned(), ToolState::Changed),
                ("fresh".to_owned(), ToolState::New),
                ("steady".to_owned(), ToolState::Approved),
                ("vanishing".to_owned(), ToolState::Missing),
            ]
        );
    }

    #[test]
    fn approving_again_after_a_change_makes_a_tool_callable_and_revoking_undoes_it() {
        let (_dir, store) = temp_store();
        store
            .replace_proxy_tools("int-1", &[discovered("edited", "hash-v1")])
            .expect("first snapshot");
        store
            .approve_proxy_tools("int-1", &["edited".to_owned()])
            .expect("approve");
        store
            .replace_proxy_tools("int-1", &[discovered("edited", "hash-v2")])
            .expect("second snapshot");
        assert_eq!(states(&store, "int-1"), [("edited".to_owned(), ToolState::Changed)]);

        store
            .approve_proxy_tools("int-1", &["edited".to_owned()])
            .expect("re-approve");
        assert_eq!(states(&store, "int-1"), [("edited".to_owned(), ToolState::Approved)]);

        store
            .revoke_proxy_tools("int-1", &["edited".to_owned()])
            .expect("revoke");
        assert_eq!(states(&store, "int-1"), [("edited".to_owned(), ToolState::New)]);
    }

    #[test]
    fn a_snapshot_belongs_to_one_integration() {
        let (_dir, store) = temp_store();
        store
            .replace_proxy_tools("int-1", &[discovered("shared", "hash-1")])
            .expect("first integration");
        store
            .replace_proxy_tools("int-2", &[discovered("shared", "hash-2")])
            .expect("second integration");
        store
            .approve_proxy_tools("int-1", &["shared".to_owned()])
            .expect("approve");

        assert_eq!(states(&store, "int-1"), [("shared".to_owned(), ToolState::Approved)]);
        assert_eq!(states(&store, "int-2"), [("shared".to_owned(), ToolState::New)]);
        assert_eq!(
            store.approve_proxy_tools("int-1", &["absent".to_owned()]).expect("approve"),
            0
        );
    }
}
