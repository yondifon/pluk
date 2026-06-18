import { Database } from "bun:sqlite";
import { homedir } from "os";
import { mkdirSync } from "fs";

const DATA_DIR = `${homedir()}/.pluk`;
mkdirSync(DATA_DIR, { recursive: true });

const db = new Database(`${DATA_DIR}/pluk.db`);

db.run(`
  CREATE TABLE IF NOT EXISTS saved_queries (
    id TEXT PRIMARY KEY,
    connection_id TEXT NOT NULL,
    name TEXT NOT NULL,
    sql TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(connection_id, name)
  )
`);

export interface SavedQuery {
  id: string;
  connection_id: string;
  name: string;
  sql: string;
  created_at: string;
}

export type SavedQueryInput = Omit<SavedQuery, "id" | "created_at">;

export function listSavedQueries(connectionId: string): SavedQuery[] {
  return db.query(`
    SELECT id, connection_id, name, sql, created_at
    FROM saved_queries
    WHERE connection_id = ?
    ORDER BY name
  `).all(connectionId) as SavedQuery[];
}

export function getSavedQuery(connectionId: string, name: string): SavedQuery | null {
  return db.query(`
    SELECT id, connection_id, name, sql, created_at
    FROM saved_queries
    WHERE connection_id = ? AND name = ?
  `).get(connectionId, name) as SavedQuery | null;
}

export function createSavedQuery(input: SavedQueryInput): SavedQuery {
  const id = crypto.randomUUID();
  db.query(`
    INSERT INTO saved_queries (id, connection_id, name, sql)
    VALUES (?, ?, ?, ?)
  `).run(id, input.connection_id, input.name, input.sql);
  return { ...input, id, created_at: new Date().toISOString() };
}

export function deleteSavedQuery(connectionId: string, name: string): boolean {
  const result = db.query("DELETE FROM saved_queries WHERE connection_id = ? AND name = ?").run(connectionId, name);
  return result.changes > 0;
}
