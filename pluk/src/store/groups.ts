import { Database } from "bun:sqlite";
import { randomBytes } from "crypto";
import { homedir } from "os";
import { mkdirSync } from "fs";
import { getIntegrationById, type Integration, type Environment } from "./integrations.js";

const DATA_DIR = `${homedir()}/.pluk`;
mkdirSync(DATA_DIR, { recursive: true });

const db = new Database(`${DATA_DIR}/pluk.db`);

// A group bundles several integrations behind one MCP token/endpoint. Members are
// stored as a JSON array of integration ids; each member keeps its own policy.
// Schema is a shared contract with the Swift app, which also opens this file.
db.run(`
  CREATE TABLE IF NOT EXISTS groups (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    environment TEXT DEFAULT 'production',
    member_ids TEXT NOT NULL DEFAULT '[]',
    token TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
  )
`);

/** A group member: an integration plus optional config overrides scoped to this
 *  group (e.g. a Linear member with a per-group `team_key`). */
export interface GroupMember {
  id: string;
  overrides?: Record<string, unknown>;
}

export interface Group {
  id: string;
  name: string;
  environment?: Environment;
  members: GroupMember[];
  token: string;
  created_at: string;
}

interface GroupRow extends Omit<Group, "members"> {
  // Column is still named `member_ids` (legacy); it holds the members JSON.
  member_ids: string;
}

export type GroupInput = {
  name: string;
  environment?: Environment;
  members?: GroupMember[];
};

const SELECT_ALL = `SELECT id, name, environment, member_ids, token, created_at FROM groups`;

// Accepts both the legacy shape (array of id strings) and the current shape
// (array of {id, overrides}) so old rows keep working.
function parseMembers(raw: string): GroupMember[] {
  let parsed: unknown;
  try {
    parsed = raw ? JSON.parse(raw) : [];
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  return parsed
    .map((m): GroupMember | null => {
      if (typeof m === "string") return { id: m };
      if (m && typeof m === "object" && typeof (m as GroupMember).id === "string") {
        return { id: (m as GroupMember).id, overrides: (m as GroupMember).overrides };
      }
      return null;
    })
    .filter((m): m is GroupMember => m !== null);
}

function hydrate(row: GroupRow | null): Group | null {
  if (!row) return null;
  return { ...row, members: parseMembers(row.member_ids) };
}

export function listGroups(): Group[] {
  const rows = db.query(`${SELECT_ALL} ORDER BY created_at DESC`).all() as GroupRow[];
  return rows.map((r) => hydrate(r)!);
}

export function getGroupByToken(token: string): Group | null {
  return hydrate(db.query(`${SELECT_ALL} WHERE token = ?`).get(token) as GroupRow | null);
}

export function getGroupById(id: string): Group | null {
  return hydrate(db.query(`${SELECT_ALL} WHERE id = ?`).get(id) as GroupRow | null);
}

/** A group member resolved to its live integration, with this group's overrides. */
export interface ResolvedMember {
  integration: Integration;
  overrides?: Record<string, unknown>;
}

/** Resolve members to live integrations (skipping any that vanished), carrying
 *  each member's per-group overrides for the caller to apply. */
export function resolveMembers(group: Group): ResolvedMember[] {
  return group.members
    .map((m): ResolvedMember | null => {
      const integration = getIntegrationById(m.id);
      return integration ? { integration, overrides: m.overrides } : null;
    })
    .filter((m): m is ResolvedMember => m !== null);
}

export function createGroup(data: GroupInput): Group {
  const id = randomBytes(8).toString("hex");
  const token = `pluk_${randomBytes(12).toString("hex")}`;
  db.query(`
    INSERT INTO groups (id, name, environment, member_ids, token)
    VALUES (?, ?, ?, ?, ?)
  `).run(
    id,
    data.name,
    data.environment ?? null, // a group may span environments
    JSON.stringify(data.members ?? []),
    token
  );
  return getGroupById(id)!;
}

export function updateGroup(id: string, data: Partial<GroupInput>): Group | null {
  const current = getGroupById(id);
  if (!current) return null;
  const next = {
    name: data.name ?? current.name,
    environment: data.environment ?? current.environment ?? null,
    members: JSON.stringify(data.members ?? current.members),
  };
  db.query(`UPDATE groups SET name = ?, environment = ?, member_ids = ? WHERE id = ?`)
    .run(next.name, next.environment, next.members, id);
  return getGroupById(id);
}

export function deleteGroup(id: string): void {
  db.query("DELETE FROM groups WHERE id = ?").run(id);
}
