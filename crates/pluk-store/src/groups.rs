//! Group CRUD (`groups` table) and member resolution.

use rusqlite::{OptionalExtension, Row, params};

use crate::Store;
use crate::codec::{parse_members, serialize_members};
use crate::error::Result;
use crate::ids;
use crate::models::{Environment, Group, GroupMember, ResolvedMember};

#[derive(Debug, Clone, Default)]
pub struct GroupInput {
    pub name: String,
    /// `None` creates an unscoped group (spans all environments); both
    /// existing writers insert NULL explicitly rather than the column default.
    pub environment: Option<Environment>,
    pub members: Vec<GroupMember>,
}

/// A partial update. `environment` is doubly optional so a scoped group can go
/// back to unscoped (`Some(None)`), which the Swift app's full-row update can
/// do today.
#[derive(Debug, Clone, Default)]
pub struct GroupUpdate {
    pub name: Option<String>,
    pub environment: Option<Option<Environment>>,
    pub members: Option<Vec<GroupMember>>,
}

const SELECT_ALL: &str = "SELECT id, name, environment, member_ids, token, created_at FROM groups";

fn hydrate(row: &Row<'_>) -> rusqlite::Result<Group> {
    let environment: Option<String> = row.get(2)?;
    let member_ids: String = row.get(3)?;
    Ok(Group {
        id: row.get(0)?,
        name: row.get(1)?,
        environment: environment.as_deref().and_then(Environment::parse),
        members: parse_members(&member_ids),
        token: row.get(4)?,
        created_at: row.get(5)?,
    })
}

impl Store {
    pub fn list_groups(&self) -> Result<Vec<Group>> {
        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare(&format!("{SELECT_ALL} ORDER BY created_at DESC"))?;
        let rows = stmt.query_map([], hydrate)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn group_by_token(&self, token: &str) -> Result<Option<Group>> {
        let conn = self.conn.lock().expect("store lock");
        Ok(conn
            .query_row(&format!("{SELECT_ALL} WHERE token = ?"), [token], hydrate)
            .optional()?)
    }

    pub fn group_by_id(&self, id: &str) -> Result<Option<Group>> {
        let conn = self.conn.lock().expect("store lock");
        Ok(conn
            .query_row(&format!("{SELECT_ALL} WHERE id = ?"), [id], hydrate)
            .optional()?)
    }

    /// Resolve members to live integrations, skipping any that vanished,
    /// carrying each member's per-group overrides.
    pub fn resolve_members(&self, group: &Group) -> Result<Vec<ResolvedMember>> {
        let mut resolved = Vec::with_capacity(group.members.len());
        for member in &group.members {
            if let Some(integration) = self.integration_by_id(&member.id)? {
                resolved.push(ResolvedMember {
                    integration,
                    overrides: member.overrides.clone(),
                });
            }
        }
        Ok(resolved)
    }

    pub fn create_group(&self, input: &GroupInput) -> Result<Group> {
        let id = ids::new_id();
        let token = ids::new_token();
        let conn = self.conn.lock().expect("store lock");
        conn.execute(
            "INSERT INTO groups (id, name, environment, member_ids, token)
             VALUES (?, ?, ?, ?, ?)",
            params![
                id,
                input.name,
                input.environment.map(Environment::as_str),
                serialize_members(&input.members),
                token,
            ],
        )?;
        drop(conn);
        Ok(self.group_by_id(&id)?.expect("row just inserted"))
    }

    pub fn update_group(&self, id: &str, update: &GroupUpdate) -> Result<Option<Group>> {
        let current = match self.group_by_id(id)? {
            Some(current) => current,
            None => return Ok(None),
        };
        let next_environment = match &update.environment {
            Some(explicit) => *explicit,
            None => current.environment,
        };

        let conn = self.conn.lock().expect("store lock");
        conn.execute(
            "UPDATE groups SET name = ?, environment = ?, member_ids = ? WHERE id = ?",
            params![
                update.name.clone().unwrap_or_else(|| current.name.clone()),
                next_environment.as_ref().map(|env| env.as_str()),
                serialize_members(update.members.as_ref().unwrap_or(&current.members)),
                id,
            ],
        )?;
        drop(conn);
        self.group_by_id(id)
    }

    pub fn delete_group(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("store lock");
        Ok(conn.execute("DELETE FROM groups WHERE id = ?", [id])? > 0)
    }
}
