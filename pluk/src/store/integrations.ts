import { Database } from "bun:sqlite";
import { randomBytes } from "crypto";
import { homedir } from "os";
import { mkdirSync } from "fs";

const DATA_DIR = `${homedir()}/.pluk`;
mkdirSync(DATA_DIR, { recursive: true });

const db = new Database(`${DATA_DIR}/pluk.db`);

// Schema is a shared contract with the Swift app (ConnectionStore.swift), which
// also opens this file and writes rows. Both sides create the table with the
// same shape. Everything service-specific lives in `config` (JSON); only the
// fields every adapter shares are first-class columns.
db.run(`
  CREATE TABLE IF NOT EXISTS integrations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    type TEXT NOT NULL,
    config TEXT NOT NULL DEFAULT '{}',
    environment TEXT DEFAULT 'development',
    read_only INTEGER NOT NULL DEFAULT 0,
    query_policy TEXT,
    token TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
  )
`);

export type Environment = "production" | "staging" | "development" | "local";

/** A configured service: a database, Linear, Sentry, … resolved to an adapter by `type`. */
export interface Integration {
  id: string;
  name: string;
  type: string;                       // adapter id
  config: Record<string, unknown>;    // per-adapter shape; holds secrets
  environment?: Environment;
  read_only: number;
  query_policy?: string | null;
  token: string;
  created_at: string;
  /** Transient, not persisted: set only when this integration is registered as a
   *  member of a group, so the gated runner can attribute its log rows to the
   *  group endpoint that fronted the call. */
  viaGroup?: { id: string; name: string };
}

interface IntegrationRow extends Omit<Integration, "config"> {
  config: string;
}

export type IntegrationInput = {
  name: string;
  type: string;
  config?: Record<string, unknown>;
  environment?: Environment;
  read_only?: number;
  query_policy?: string | null;
};

const SELECT_ALL = `
  SELECT id, name, type, config, environment, read_only, query_policy, token, created_at
  FROM integrations
`;

function hydrate(row: IntegrationRow | null): Integration | null {
  if (!row) return null;
  let config: Record<string, unknown> = {};
  try {
    config = row.config ? JSON.parse(row.config) : {};
  } catch {
    config = {};
  }
  return { ...row, config };
}

export function listIntegrations(): Integration[] {
  const rows = db.query(`${SELECT_ALL} ORDER BY created_at DESC`).all() as IntegrationRow[];
  return rows.map((r) => hydrate(r)!);
}

export function getIntegrationByToken(token: string): Integration | null {
  return hydrate(db.query(`${SELECT_ALL} WHERE token = ?`).get(token) as IntegrationRow | null);
}

export function getIntegrationById(id: string): Integration | null {
  return hydrate(db.query(`${SELECT_ALL} WHERE id = ?`).get(id) as IntegrationRow | null);
}

export function createIntegration(data: IntegrationInput): Integration {
  const id = randomBytes(8).toString("hex");
  const token = `pluk_${randomBytes(12).toString("hex")}`;

  db.query(`
    INSERT INTO integrations (id, name, type, config, environment, read_only, query_policy, token)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?)
  `).run(
    id,
    data.name,
    data.type,
    JSON.stringify(data.config ?? {}),
    data.environment ?? "development",
    data.read_only ?? 0,
    data.query_policy ?? null,
    token
  );

  return getIntegrationById(id)!;
}

export function deleteIntegration(id: string): void {
  db.query("DELETE FROM integrations WHERE id = ?").run(id);
}

export function updateIntegration(id: string, data: Partial<IntegrationInput>): Integration | null {
  const current = getIntegrationById(id);
  if (!current) return null;

  const next = {
    name: data.name ?? current.name,
    type: data.type ?? current.type,
    config: JSON.stringify(data.config ?? current.config),
    environment: data.environment ?? current.environment ?? "development",
    read_only: data.read_only ?? current.read_only,
    query_policy: data.query_policy !== undefined ? data.query_policy : current.query_policy,
  };

  db.query(`
    UPDATE integrations
    SET name = ?, type = ?, config = ?, environment = ?, read_only = ?, query_policy = ?
    WHERE id = ?
  `).run(next.name, next.type, next.config, next.environment, next.read_only, next.query_policy ?? null, id);

  return getIntegrationById(id);
}
