import { Database } from "bun:sqlite";
import { homedir } from "os";
import { mkdirSync } from "fs";

const DATA_DIR = `${homedir()}/.pluk`;
mkdirSync(DATA_DIR, { recursive: true });

const db = new Database(`${DATA_DIR}/pluk.db`);

db.run(`
  CREATE TABLE IF NOT EXISTS query_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    connection_id TEXT NOT NULL,
    connection_name TEXT NOT NULL,
    sql TEXT NOT NULL,
    verdict TEXT NOT NULL,    -- allowed | blocked | error
    reason TEXT,
    categories TEXT,          -- csv of statement categories
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
  )
`);

export type Verdict = "allowed" | "blocked" | "error";

export function logQuery(
  connectionId: string,
  connectionName: string,
  sql: string,
  verdict: Verdict,
  categories: string,
  reason?: string,
): void {
  try {
    db.query(
      `INSERT INTO query_log (connection_id, connection_name, sql, verdict, reason, categories)
       VALUES (?, ?, ?, ?, ?, ?)`
    ).run(connectionId, connectionName, sql, verdict, reason ?? null, categories);
  } catch {
    // Never let logging failure break query execution
  }
}
