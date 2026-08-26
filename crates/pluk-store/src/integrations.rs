//! Integration CRUD (`integrations` table).

use rusqlite::{OptionalExtension, Row, params};

use crate::Store;
use crate::codec::{parse_config, serialize_config};
use crate::error::Result;
use crate::ids;
use crate::models::{Config, Environment, Integration};

/// Everything a caller supplies to register an integration. Id and token are
/// minted here, matching both existing writers.
#[derive(Debug, Clone)]
pub struct IntegrationInput {
    pub name: String,
    /// Adapter id, e.g. `postgres`, `linear`, `github-cli`.
    pub r#type: String,
    pub config: Config,
    /// Defaults to `development` when absent, like the column default.
    pub environment: Option<Environment>,
    /// Legacy flag; the server ignores it but the schema requires the column.
    pub read_only: i64,
    pub query_policy: Option<String>,
}

impl IntegrationInput {
    pub fn new(name: impl Into<String>, r#type: impl Into<String>) -> Self {
        IntegrationInput {
            name: name.into(),
            r#type: r#type.into(),
            config: Config::new(),
            environment: None,
            read_only: 0,
            query_policy: None,
        }
    }
}

/// A partial update; `None` fields leave the stored value untouched.
///
/// `query_policy` is doubly optional: outer `None` keeps it, `Some(None)`
/// clears it (mirroring how the TypeScript API distinguishes "absent" from
/// explicit null).
#[derive(Debug, Clone, Default)]
pub struct IntegrationUpdate {
    pub name: Option<String>,
    pub r#type: Option<String>,
    pub config: Option<Config>,
    pub environment: Option<Environment>,
    pub read_only: Option<i64>,
    pub query_policy: Option<Option<String>>,
}

const SELECT_ALL: &str = "SELECT id, name, type, config, environment, read_only, query_policy, token, created_at FROM integrations";

fn hydrate(row: &Row<'_>) -> rusqlite::Result<Integration> {
    let raw_config: String = row.get(3)?;
    let environment: Option<String> = row.get(4)?;
    Ok(Integration {
        id: row.get(0)?,
        name: row.get(1)?,
        r#type: row.get(2)?,
        config: parse_config(&raw_config),
        environment: environment.as_deref().and_then(Environment::parse),
        read_only: row.get(5)?,
        query_policy: row.get(6)?,
        token: row.get(7)?,
        created_at: row.get(8)?,
        via_group: None,
    })
}

impl Store {
    pub fn list_integrations(&self) -> Result<Vec<Integration>> {
        let conn = self.conn.lock().expect("store lock");
        let mut stmt = conn.prepare(&format!("{SELECT_ALL} ORDER BY created_at DESC"))?;
        let rows = stmt.query_map([], hydrate)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn integration_by_token(&self, token: &str) -> Result<Option<Integration>> {
        let conn = self.conn.lock().expect("store lock");
        Ok(conn
            .query_row(&format!("{SELECT_ALL} WHERE token = ?"), [token], hydrate)
            .optional()?)
    }

    pub fn integration_by_id(&self, id: &str) -> Result<Option<Integration>> {
        let conn = self.conn.lock().expect("store lock");
        Ok(conn
            .query_row(&format!("{SELECT_ALL} WHERE id = ?"), [id], hydrate)
            .optional()?)
    }

    pub fn create_integration(&self, input: &IntegrationInput) -> Result<Integration> {
        let id = ids::new_id();
        let token = ids::new_token();
        let conn = self.conn.lock().expect("store lock");
        conn.execute(
            "INSERT INTO integrations (id, name, type, config, environment, read_only, query_policy, token)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                id,
                input.name,
                input.r#type,
                serialize_config(&input.config),
                input.environment.unwrap_or(Environment::Development).as_str(),
                input.read_only,
                input.query_policy,
                token,
            ],
        )?;
        drop(conn);
        // Read back so `created_at` carries the database's own stamp.
        Ok(self.integration_by_id(&id)?.expect("row just inserted"))
    }

    pub fn update_integration(
        &self,
        id: &str,
        update: &IntegrationUpdate,
    ) -> Result<Option<Integration>> {
        let current = match self.integration_by_id(id)? {
            Some(current) => current,
            None => return Ok(None),
        };
        let next_environment = update
            .environment
            .or(current.environment)
            .unwrap_or(Environment::Development);
        let next_policy = match &update.query_policy {
            Some(explicit) => explicit.clone(),
            None => current.query_policy.clone(),
        };

        let conn = self.conn.lock().expect("store lock");
        conn.execute(
            "UPDATE integrations
             SET name = ?, type = ?, config = ?, environment = ?, read_only = ?, query_policy = ?
             WHERE id = ?",
            params![
                update.name.clone().unwrap_or_else(|| current.name.clone()),
                update
                    .r#type
                    .clone()
                    .unwrap_or_else(|| current.r#type.clone()),
                serialize_config(update.config.as_ref().unwrap_or(&current.config)),
                next_environment.as_str(),
                update.read_only.unwrap_or(current.read_only),
                next_policy,
                id,
            ],
        )?;
        drop(conn);
        self.integration_by_id(id)
    }

    pub fn delete_integration(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("store lock");
        Ok(conn.execute("DELETE FROM integrations WHERE id = ?", [id])? > 0)
    }
}
