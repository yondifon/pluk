import { Database } from "bun:sqlite";
import { homedir } from "os";
import { mkdirSync } from "fs";

// PLUK_DATA_DIR overrides where the DB lives, so tests can isolate from the
// user's real ~/.pluk database.
const DATA_DIR = process.env.PLUK_DATA_DIR ?? `${homedir()}/.pluk`;
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
    response_text TEXT,       -- raw agent-visible response text (capped at LOG_RESPONSE_BYTES)
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
  )
`);

// Migrate existing tables
for (const sql of [
  "ALTER TABLE query_log ADD COLUMN result_json TEXT",
  "ALTER TABLE query_log ADD COLUMN row_count INTEGER",
  "ALTER TABLE query_log ADD COLUMN source TEXT", // originating tool / operation
  "ALTER TABLE query_log ADD COLUMN response_text TEXT", // raw response shown in the log viewer
  "ALTER TABLE query_log ADD COLUMN group_id TEXT",   // set when the call came through a group endpoint
  "ALTER TABLE query_log ADD COLUMN group_name TEXT", // group display name (for the group log view)
  "ALTER TABLE query_log ADD COLUMN database TEXT",   // target database when a call selects one (multi-db connections)
]) {
  try { db.run(sql); } catch { /* column exists */ }
}

db.run("CREATE INDEX IF NOT EXISTS query_log_connection_time_id_idx ON query_log(connection_id, created_at DESC, id DESC)");
db.run("CREATE INDEX IF NOT EXISTS query_log_group_time_id_idx ON query_log(group_id, created_at DESC, id DESC)");

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
const LOG_RESPONSE_BYTES = 100_000; // max raw response text stored per entry

export type Verdict = "pending" | "allowed" | "blocked" | "cancelled" | "error";

/** The group a call was routed through, when a group endpoint fronted the member
 *  integration. Recorded on the log row so the group view can show every member's
 *  activity in one place. Absent for calls hitting an integration's own endpoint. */
export type LogGroup = { id: string; name: string };

function packResult(result?: { rows: unknown[]; fields?: string[] }): { resultJson: string | null; rowCount: number | null } {
  if (!result) return { resultJson: null, rowCount: null };
  const rowCount = result.rows.length;
  const capped = rowCount > LOG_RESULT_ROWS ? result.rows.slice(0, LOG_RESULT_ROWS) : result.rows;
  return { resultJson: JSON.stringify({ fields: result.fields ?? [], rows: capped }), rowCount };
}

// The raw agent-visible response, capped so one huge command output can't bloat
// the log DB. Truncation is marked so the viewer can say so.
function packResponse(text?: string): string | null {
  if (!text) return null;
  return text.length > LOG_RESPONSE_BYTES
    ? `${text.slice(0, LOG_RESPONSE_BYTES)}\n…[truncated]`
    : text;
}

/** Insert a new log entry. Returns the row id for later update. */
export function createLogEntry(
  connectionId: string,
  connectionName: string,
  sql: string,
  verdict: Verdict,
  categories: string,
  reason?: string,
  source?: string,
  group?: LogGroup,
  database?: string,
): number {
  try {
    db.query(
      `INSERT INTO query_log (connection_id, connection_name, sql, verdict, reason, categories, source, group_id, group_name, database)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`
    ).run(connectionId, connectionName, sql, verdict, reason ?? null, categories, source ?? null, group?.id ?? null, group?.name ?? null, database ?? null);
    const row = db.query("SELECT last_insert_rowid() as id").get() as { id: number };
    purgeOldLogs();
    notifyActivity(row.id);
    return row.id;
  } catch {
    return -1;
  }
}

/**
 * Log a single statement actually sent to the database, tagged with its source
 * tool. Used by the driver layer to record every introspection/utility query so
 * the audit log reflects all SQL — not just the user-facing `query` tool.
 */
export function logExecutedStatement(
  connectionId: string,
  connectionName: string,
  sql: string,
  source: string,
  rowCount: number | null,
  error?: string,
  group?: LogGroup,
  database?: string,
): void {
  try {
    db.query(
      `INSERT INTO query_log (connection_id, connection_name, sql, verdict, reason, categories, row_count, source, group_id, group_name, database)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`
    ).run(connectionId, connectionName, sql, error ? "error" : "allowed", error ?? null, null, rowCount, source, group?.id ?? null, group?.name ?? null, database ?? null);
    const row = db.query("SELECT last_insert_rowid() as id").get() as { id: number };
    purgeOldLogs();
    notifyActivity(row.id);
  } catch {
    // Non-fatal
  }
}

/** Update verdict + optional result on an existing log entry. `responseText` is
 *  the raw agent-visible response, shown in full by the log viewer. */
export function updateLogEntry(
  id: number,
  verdict: Verdict,
  reason?: string,
  result?: { rows: unknown[]; fields?: string[] },
  responseText?: string,
): void {
  if (id < 0) return;
  try {
    const { resultJson, rowCount } = packResult(result);
    db.query(
      `UPDATE query_log SET verdict=?, reason=?, result_json=?, row_count=?, response_text=? WHERE id=?`
    ).run(verdict, reason ?? null, resultJson, rowCount, packResponse(responseText), id);
    notifyActivity(id);
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
  source?: string,
  responseText?: string,
  group?: LogGroup,
  database?: string,
): void {
  const id = createLogEntry(connectionId, connectionName, sql, verdict, categories, reason, source, group, database);
  if (id >= 0 && (result || responseText)) updateLogEntry(id, verdict, reason, result, responseText);
}

// ── Paged activity reads ──────────────────────────────────────────────────────

export const LOG_PAGE_SIZE = 100;

export type LogRange = "hour" | "today" | "7d" | "30d" | "all";

export type LogCursor = {
  createdAt: string;
  id: number;
};

export type LogScope =
  | { connectionId: string }
  | { groupId: string };

export type LogPageEntry = {
  id: number;
  connectionId: string;
  connectionName: string;
  sql: string;
  verdict: string;
  reason: string | null;
  categories: string | null;
  source: string | null;
  resultJson: string | null;
  rowCount: number | null;
  responseText: string | null;
  groupId: string | null;
  groupName: string | null;
  createdAt: string;
};

export type LogPage = {
  entries: LogPageEntry[];
  nextCursor: LogCursor | null;
  hasMore: boolean;
};

const PAGE_COLUMNS = "id, connection_id, connection_name, sql, verdict, reason, categories, source, result_json, row_count, response_text, group_id, group_name, created_at" as const;

const RANGE_CUTOFFS: Record<Exclude<LogRange, "all">, string> = {
  hour: "datetime('now', '-1 hour')",
  today: "datetime('now', 'localtime', 'start of day', 'utc')",
  "7d": "datetime('now', '-7 days')",
  "30d": "datetime('now', '-30 days')",
};

export function readLogPage(scope: LogScope, range: LogRange, cursor?: LogCursor): LogPage {
  const scopeColumn = "connectionId" in scope ? "connection_id" : "group_id";
  const scopeValue = "connectionId" in scope ? scope.connectionId : scope.groupId;
  const conditions = [`${scopeColumn} = ?`];
  const values: (string | number)[] = [scopeValue];

  if (range !== "all") conditions.push(`created_at >= ${RANGE_CUTOFFS[range]}`);
  if (cursor) {
    conditions.push("(created_at < ? OR (created_at = ? AND id < ?))");
    values.push(cursor.createdAt, cursor.createdAt, cursor.id);
  }

  values.push(LOG_PAGE_SIZE + 1);
  const rows = db.query(
    `SELECT ${PAGE_COLUMNS}
     FROM query_log
     WHERE ${conditions.join(" AND ")}
     ORDER BY created_at DESC, id DESC
     LIMIT ?`
  ).all(...values) as Record<string, unknown>[];

  const entries = rows
    .slice(0, LOG_PAGE_SIZE)
    .map(rowToPageEntry)
    .filter((entry): entry is LogPageEntry => entry !== null);
  const hasMore = rows.length > LOG_PAGE_SIZE;
  const last = entries[entries.length - 1];

  return {
    entries,
    hasMore,
    nextCursor: hasMore && last ? { createdAt: last.createdAt, id: last.id } : null,
  };
}

function rowToPageEntry(r: Record<string, unknown>): LogPageEntry | null {
  const id = Number(r.id);
  const createdAt = String(r.created_at ?? "");
  if (!Number.isSafeInteger(id) || id <= 0 || !createdAt) return null;
  return {
    id,
    connectionId: String(r.connection_id ?? ""),
    connectionName: String(r.connection_name ?? ""),
    sql: String(r.sql ?? ""),
    verdict: String(r.verdict ?? ""),
    reason: r.reason == null ? null : String(r.reason),
    categories: r.categories == null ? null : String(r.categories),
    source: r.source == null ? null : String(r.source),
    resultJson: r.result_json == null ? null : String(r.result_json),
    rowCount: r.row_count == null ? null : Number(r.row_count),
    responseText: r.response_text == null ? null : String(r.response_text),
    groupId: r.group_id == null ? null : String(r.group_id),
    groupName: r.group_name == null ? null : String(r.group_name),
    createdAt,
  };
}

// ── Activity feed ─────────────────────────────────────────────────────────────
//
// Subscribers learn about every new or updated log row the moment it is written,
// so the app can update its log views without polling. The feed is cursor-based:
// the row id is monotonic, and a subscriber that comes in late can be caught up
// with `logRowsAfter`. Heavy fields (result_json, response_text) stay in the
// shared DB — the app re-reads rows from there; the feed carries only what a
// collapsed log row needs.

export type LogActivity = {
  id: number;
  connectionId: string;
  connectionName: string;
  sql: string;
  verdict: string;
  reason: string | null;
  categories: string | null;
  source: string | null;
  groupId: string | null;
  groupName: string | null;
  database: string | null;
  rowCount: number | null;
  createdAt: string;
};

type ActivityHandler = (row: LogActivity) => void;

const activityHandlers = new Set<ActivityHandler>();

export function subscribeLogActivity(handler: ActivityHandler): () => void {
  activityHandlers.add(handler);
  return () => activityHandlers.delete(handler);
}

export function logHighWater(): number {
  const row = db.query("SELECT COALESCE(MAX(id), 0) AS cursor FROM query_log").get() as { cursor: number };
  return row.cursor;
}

export function logRowsAfter(after: number): LogActivity[] {
  const rows = db.query(
    `SELECT ${ROW_COLUMNS} FROM query_log WHERE id > ? ORDER BY id ASC`
  ).all(after) as Record<string, unknown>[];
  return rows.map(rowToActivity).filter((r): r is LogActivity => r !== null);
}

const ROW_COLUMNS = "id, connection_id, connection_name, sql, verdict, reason, categories, source, group_id, group_name, database, row_count, created_at" as const;

function rowToActivity(r: Record<string, unknown>): LogActivity | null {
  const id = r.id as number;
  if (typeof id !== "number" || id <= 0) return null;
  return {
    id,
    connectionId: String(r.connection_id ?? ""),
    connectionName: String(r.connection_name ?? ""),
    sql: String(r.sql ?? ""),
    verdict: String(r.verdict ?? ""),
    reason: r.reason == null ? null : String(r.reason),
    categories: r.categories == null ? null : String(r.categories),
    source: r.source == null ? null : String(r.source),
    groupId: r.group_id == null ? null : String(r.group_id),
    groupName: r.group_name == null ? null : String(r.group_name),
    database: r.database == null ? null : String(r.database),
    rowCount: r.row_count == null ? null : Number(r.row_count),
    createdAt: String(r.created_at ?? ""),
  };
}

function rowById(id: number): LogActivity | null {
  const rows = db.query(
    `SELECT ${ROW_COLUMNS} FROM query_log WHERE id = ?`
  ).all(id) as Record<string, unknown>[];
  return rowToActivity(rows[0] ?? {});
}

function notifyActivity(id: number): void {
  if (id <= 0 || activityHandlers.size === 0) return;
  const row = rowById(id);
  if (!row) return;
  for (const handler of activityHandlers) handler(row);
}
