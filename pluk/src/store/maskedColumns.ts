import { Database } from "bun:sqlite";
import { homedir } from "os";
import { mkdirSync } from "fs";

const DATA_DIR = `${homedir()}/.pluk`;
mkdirSync(DATA_DIR, { recursive: true });

const db = new Database(`${DATA_DIR}/pluk.db`);

db.run(`
  CREATE TABLE IF NOT EXISTS masked_columns (
    id TEXT PRIMARY KEY,
    connection_id TEXT NOT NULL,
    column_name TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(connection_id, column_name)
  )
`);

export interface MaskedColumn {
  id: string;
  connection_id: string;
  column_name: string;
  created_at: string;
}

export type MaskMode = "redact" | "hash";

export function listMaskedColumns(connectionId: string): string[] {
  return (db.query(`
    SELECT column_name FROM masked_columns
    WHERE connection_id = ?
    ORDER BY column_name
  `).all(connectionId) as { column_name: string }[]).map(r => r.column_name);
}

export function addMaskedColumn(connectionId: string, columnName: string): MaskedColumn {
  const id = crypto.randomUUID();
  db.query(`
    INSERT INTO masked_columns (id, connection_id, column_name)
    VALUES (?, ?, ?)
  `).run(id, connectionId, columnName);
  return { id, connection_id: connectionId, column_name: columnName, created_at: new Date().toISOString() };
}

export function removeMaskedColumn(connectionId: string, columnName: string): boolean {
  const result = db.query("DELETE FROM masked_columns WHERE connection_id = ? AND column_name = ?").run(connectionId, columnName);
  return result.changes > 0;
}

function sha256(input: string): string {
  return new Bun.CryptoHasher("sha256").update(input).digest("hex");
}

export function maskValue(value: unknown, mode: MaskMode): unknown {
  if (value === null || value === undefined) return value;
  if (mode === "redact") return "***";
  return sha256(String(value));
}

export function maskRow(row: Record<string, unknown>, maskedColumns: string[], mode: MaskMode = "redact"): Record<string, unknown> {
  if (maskedColumns.length === 0) return row;
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(row)) {
    out[k] = maskedColumns.includes(k) ? maskValue(v, mode) : v;
  }
  return out;
}
