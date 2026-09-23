/**
 * Rows of a key/value field, such as an MCP server's headers.
 *
 * The window never reads a secret row's value back: the host sends such a
 * row as its name, `secret: true`, and whether a value is saved (`set`). A
 * row sent back with a blank value keeps what is saved under `savedName`, so
 * renaming a saved row without retyping its value keeps the value too.
 */

export interface KeyValueRow {
  name: string;
  value: string;
  secret: boolean;
  /** The name the row had when it was read, when a value is saved for it. */
  savedName?: string;
}

/** One row as the host takes it back. */
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

/** The rows a stored config holds, ready to edit. */
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

/**
 * The rows a save sends, blank ones included so the host's row numbers match
 * the form's. A plain row never claims a saved value.
 */
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
