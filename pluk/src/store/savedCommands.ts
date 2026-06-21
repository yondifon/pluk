import { Database } from "bun:sqlite";
import { homedir } from "os";
import { mkdirSync } from "fs";

const DATA_DIR = `${homedir()}/.pluk`;
mkdirSync(DATA_DIR, { recursive: true });

const db = new Database(`${DATA_DIR}/pluk.db`);

// Pre-selected shell commands for SSH integrations. The user curates these
// (via the REST API / UI); agents run them by name through `run_saved_command`.
// There is no allowlist — saved commands run unrestricted as the connecting SSH
// user, exactly like an ad-hoc command. The MCP confirm prompt is the only gate.
db.run(`
  CREATE TABLE IF NOT EXISTS saved_commands (
    id TEXT PRIMARY KEY,
    connection_id TEXT NOT NULL,
    name TEXT NOT NULL,
    command TEXT NOT NULL,
    working_dir TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(connection_id, name)
  )
`);

export interface SavedCommand {
  id: string;
  connection_id: string;
  name: string;
  command: string;
  working_dir: string | null;
  created_at: string;
}

export type SavedCommandInput = Omit<SavedCommand, "id" | "created_at">;

export function listSavedCommands(connectionId: string): SavedCommand[] {
  return db.query(`
    SELECT id, connection_id, name, command, working_dir, created_at
    FROM saved_commands
    WHERE connection_id = ?
    ORDER BY name
  `).all(connectionId) as SavedCommand[];
}

export function getSavedCommand(connectionId: string, name: string): SavedCommand | null {
  return db.query(`
    SELECT id, connection_id, name, command, working_dir, created_at
    FROM saved_commands
    WHERE connection_id = ? AND name = ?
  `).get(connectionId, name) as SavedCommand | null;
}

export function createSavedCommand(input: SavedCommandInput): SavedCommand {
  const id = crypto.randomUUID();
  db.query(`
    INSERT INTO saved_commands (id, connection_id, name, command, working_dir)
    VALUES (?, ?, ?, ?, ?)
  `).run(id, input.connection_id, input.name, input.command, input.working_dir);
  return { ...input, id, created_at: new Date().toISOString() };
}

export function deleteSavedCommand(connectionId: string, name: string): boolean {
  const result = db.query("DELETE FROM saved_commands WHERE connection_id = ? AND name = ?").run(connectionId, name);
  return result.changes > 0;
}
