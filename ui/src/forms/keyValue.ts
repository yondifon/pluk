/**
 * Rows of a key/value field. A secret row arrives without its value; sent back
 * blank, it keeps what is saved under `savedName`, even after a rename.
 */

export interface KeyValueRow {
  name: string;
  value: string;
  secret: boolean;
  /** The name the row had when it was read, when a value is saved for it. */
  savedName?: string;
}

export interface SentRow {
  name: string;
  value: string;
  secret: boolean;
  savedName?: string;
}

/** A save the host would refuse, and the field and row to show it beside. */
export interface ConfigProblem {
  field: string;
  row?: number;
  message: string;
}

export function emptyRow(secret = true): KeyValueRow {
  return { name: "", value: "", secret };
}

export function rowsFromStored(stored: unknown): KeyValueRow[] {
  if (!Array.isArray(stored)) return [];
  return stored.map((item) => {
    const row = (item ?? {}) as { name?: unknown; value?: unknown; secret?: unknown; set?: unknown };
    const name = typeof row.name === "string" ? row.name : "";
    const secret = row.secret !== false;
    if (secret) return row.set === true ? { name, value: "", secret, savedName: name } : { name, value: "", secret };
    return { name, value: typeof row.value === "string" ? row.value : "", secret };
  });
}

/** Whether leaving the value blank keeps the one saved for this row. */
export function keepsSaved(row: KeyValueRow): boolean {
  return row.secret && row.savedName != null && row.value === "";
}

/** Blank rows are kept so the host's row numbers match the form's. */
export function rowsToSave(rows: KeyValueRow[]): SentRow[] {
  return rows.map((row) => {
    const sent: SentRow = { name: row.name.trim(), value: row.value, secret: row.secret };
    if (row.secret && row.savedName != null) sent.savedName = row.savedName;
    return sent;
  });
}

/** The names a stored row list holds, for a one-line summary. Values stay out. */
export function rowNames(stored: unknown): string {
  return rowsFromStored(stored)
    .map((row) => row.name)
    .filter((name) => name !== "")
    .join(", ");
}
