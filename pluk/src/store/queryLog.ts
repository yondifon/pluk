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
    result_json TEXT,         -- JSON snapshot of result rows (allowed only, capped at LOG_RESULT_ROWS)
    row_count INTEGER,        -- total rows before cap
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
  )
`);

// Migrate existing tables
for (const sql of [
  "ALTER TABLE query_log ADD COLUMN result_json TEXT",
  "ALTER TABLE query_log ADD COLUMN row_count INTEGER",
]) {
  try { db.run(sql); } catch { /* column exists */ }
}

db.run(`
  CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
  )
`);

// ── Settings ─────────────────────────────────────────────────────────────────

export function getSetting(key: string, defaultValue: string): string {
  const row = db.query("SELECT value FROM settings WHERE key = ?").get(key) as { value: string } | null;
  return row?.value ?? defaultValue;
}

export function setSetting(key: string, value: string): void {
  db.query("INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)").run(key, value);
}

export function getRetentionDays(): number {
  return parseInt(getSetting("log_retention_days", "30"), 10);
}

export function setRetentionDays(days: number): void {
  setSetting("log_retention_days", String(days));
}

// ── Cleanup ───────────────────────────────────────────────────────────────────

export function purgeOldLogs(): void {
  const days = getRetentionDays();
  if (days <= 0) return; // 0 = keep forever
  try {
    db.query(
      `DELETE FROM query_log WHERE created_at < datetime('now', ? || ' days')`
    ).run(`-${days}`);
  } catch {
    // Non-fatal
  }
}

// ── Log write ─────────────────────────────────────────────────────────────────

const LOG_RESULT_ROWS = 100; // max rows stored in result_json

export type Verdict = "pending" | "allowed" | "blocked" | "cancelled" | "error";

function packResult(result?: { rows: unknown[]; fields?: string[] }): { resultJson: string | null; rowCount: number | null } {
  if (!result) return { resultJson: null, rowCount: null };
  const rowCount = result.rows.length;
  const capped = rowCount > LOG_RESULT_ROWS ? result.rows.slice(0, LOG_RESULT_ROWS) : result.rows;
  return { resultJson: JSON.stringify({ fields: result.fields ?? [], rows: capped }), rowCount };
}

/** Insert a new log entry. Returns the row id for later update. */
export function createLogEntry(
  connectionId: string,
  connectionName: string,
  sql: string,
  verdict: Verdict,
  categories: string,
  reason?: string,
): number {
  try {
    db.query(
      `INSERT INTO query_log (connection_id, connection_name, sql, verdict, reason, categories)
       VALUES (?, ?, ?, ?, ?, ?)`
    ).run(connectionId, connectionName, sql, verdict, reason ?? null, categories);
    const row = db.query("SELECT last_insert_rowid() as id").get() as { id: number };
    purgeOldLogs();
    return row.id;
  } catch {
    return -1;
  }
}

/** Update verdict + optional result on an existing log entry. */
export function updateLogEntry(
  id: number,
  verdict: Verdict,
  reason?: string,
  result?: { rows: unknown[]; fields?: string[] },
): void {
  if (id < 0) return;
  try {
    const { resultJson, rowCount } = packResult(result);
    db.query(
      `UPDATE query_log SET verdict=?, reason=?, result_json=?, row_count=? WHERE id=?`
    ).run(verdict, reason ?? null, resultJson, rowCount, id);
  } catch {
    // Non-fatal
  }
}

/** Convenience: create + immediately finalize (for blocked/error with no async work). */
export function logQuery(
  connectionId: string,
  connectionName: string,
  sql: string,
  verdict: Verdict,
  categories: string,
  reason?: string,
  result?: { rows: unknown[]; fields?: string[] },
): void {
  const id = createLogEntry(connectionId, connectionName, sql, verdict, categories, reason);
  if (result && id >= 0) updateLogEntry(id, verdict, reason, result);
}
