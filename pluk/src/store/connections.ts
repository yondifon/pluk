import { Database } from "bun:sqlite";
import { randomBytes } from "crypto";
import { homedir } from "os";
import { mkdirSync } from "fs";

const DATA_DIR = `${homedir()}/.pluk`;
mkdirSync(DATA_DIR, { recursive: true });

const db = new Database(`${DATA_DIR}/pluk.db`);

db.run(`
  CREATE TABLE IF NOT EXISTS connections (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    type TEXT NOT NULL,
    -- Basic
    host TEXT,
    port INTEGER,
    "user" TEXT,
    password TEXT,
    database TEXT,
    filename TEXT,
    socket_path TEXT,
    -- SSH tunnel
    use_ssh INTEGER NOT NULL DEFAULT 0,
    ssh_host TEXT,
    ssh_port INTEGER DEFAULT 22,
    ssh_user TEXT,
    ssh_auth_type TEXT DEFAULT 'agent',
    ssh_key_path TEXT,
    ssh_password TEXT,
    -- SSL/TLS
    use_ssl INTEGER NOT NULL DEFAULT 0,
    ssl_mode TEXT DEFAULT 'require',
    ssl_ca_path TEXT,
    ssl_cert_path TEXT,
    ssl_key_path TEXT,
    -- Meta
    environment TEXT DEFAULT 'development',
    read_only INTEGER NOT NULL DEFAULT 0,
    query_policy TEXT,
    token TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
  )
`);

// Add columns for users upgrading from older schema
const migrations = [
  `ALTER TABLE connections ADD COLUMN socket_path TEXT`,
  `ALTER TABLE connections ADD COLUMN use_ssh INTEGER NOT NULL DEFAULT 0`,
  `ALTER TABLE connections ADD COLUMN ssh_auth_type TEXT DEFAULT 'agent'`,
  `ALTER TABLE connections ADD COLUMN ssh_password TEXT`,
  `ALTER TABLE connections ADD COLUMN use_ssl INTEGER NOT NULL DEFAULT 0`,
  `ALTER TABLE connections ADD COLUMN ssl_mode TEXT DEFAULT 'require'`,
  `ALTER TABLE connections ADD COLUMN ssl_ca_path TEXT`,
  `ALTER TABLE connections ADD COLUMN ssl_cert_path TEXT`,
  `ALTER TABLE connections ADD COLUMN ssl_key_path TEXT`,
  `ALTER TABLE connections ADD COLUMN environment TEXT DEFAULT 'development'`,
  `ALTER TABLE connections ADD COLUMN read_only INTEGER NOT NULL DEFAULT 0`,
  `ALTER TABLE connections ADD COLUMN query_policy TEXT`,
];
for (const sql of migrations) {
  try { db.run(sql); } catch { /* column already exists */ }
}

export type ConnectionType = "postgres" | "mysql" | "sqlite";
export type SSHAuthType = "agent" | "key" | "password";
export type SSLMode = "disable" | "require" | "verify-ca" | "verify-full";
export type Environment = "production" | "staging" | "development" | "local";

export interface Connection {
  id: string;
  name: string;
  type: ConnectionType;
  // Basic
  host?: string;
  port?: number;
  user?: string;
  password?: string;
  database?: string;
  filename?: string;
  socket_path?: string;
  // SSH
  use_ssh: number;
  ssh_host?: string;
  ssh_port?: number;
  ssh_user?: string;
  ssh_auth_type?: SSHAuthType;
  ssh_key_path?: string;
  ssh_password?: string;
  // SSL
  use_ssl: number;
  ssl_mode?: SSLMode;
  ssl_ca_path?: string;
  ssl_cert_path?: string;
  ssl_key_path?: string;
  // Meta
  environment?: Environment;
  read_only: number;
  query_policy?: string | null;
  token: string;
  created_at: string;
}

export type ConnectionInput = Omit<Connection, "id" | "token" | "created_at">;

const SELECT_ALL = `
  SELECT id,name,type,host,port,"user",password,database,filename,socket_path,
         use_ssh,ssh_host,ssh_port,ssh_user,ssh_auth_type,ssh_key_path,ssh_password,
         use_ssl,ssl_mode,ssl_ca_path,ssl_cert_path,ssl_key_path,
         environment,read_only,query_policy,token,created_at
  FROM connections
`;

export function listConnections(): Connection[] {
  return db.query(`${SELECT_ALL} ORDER BY created_at DESC`).all() as Connection[];
}

export function getConnectionByToken(token: string): Connection | null {
  return db.query(`${SELECT_ALL} WHERE token = ?`).get(token) as Connection | null;
}

export function getConnectionById(id: string): Connection | null {
  return db.query(`${SELECT_ALL} WHERE id = ?`).get(id) as Connection | null;
}

export function createConnection(data: ConnectionInput): Connection {
  const id = randomBytes(8).toString("hex");
  const token = `pluk_${randomBytes(12).toString("hex")}`;

  db.query(`
    INSERT INTO connections (
      id, name, type, host, port, "user", password, database, filename, socket_path,
      use_ssh, ssh_host, ssh_port, ssh_user, ssh_auth_type, ssh_key_path, ssh_password,
      use_ssl, ssl_mode, ssl_ca_path, ssl_cert_path, ssl_key_path,
      environment, read_only, query_policy, token
    ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
  `).run(
    id, data.name, data.type,
    data.host ?? null, data.port ?? null, data.user ?? null,
    data.password ?? null, data.database ?? null, data.filename ?? null,
    data.socket_path ?? null,
    data.use_ssh ?? 0,
    data.ssh_host ?? null, data.ssh_port ?? 22, data.ssh_user ?? null,
    data.ssh_auth_type ?? "agent", data.ssh_key_path ?? null, data.ssh_password ?? null,
    data.use_ssl ?? 0,
    data.ssl_mode ?? "require", data.ssl_ca_path ?? null,
    data.ssl_cert_path ?? null, data.ssl_key_path ?? null,
    data.environment ?? "development", data.read_only ?? 0,
    data.query_policy ?? null,
    token
  );

  return getConnectionById(id)!;
}

export function deleteConnection(id: string): void {
  db.query("DELETE FROM connections WHERE id = ?").run(id);
}

export function updateConnection(id: string, data: Partial<ConnectionInput>): Connection | null {
  if (!getConnectionById(id)) return null;

  const allowed = [
    "name", "type", "host", "port", "user", "password", "database", "filename", "socket_path",
    "use_ssh", "ssh_host", "ssh_port", "ssh_user", "ssh_auth_type", "ssh_key_path", "ssh_password",
    "use_ssl", "ssl_mode", "ssl_ca_path", "ssl_cert_path", "ssl_key_path",
    "environment", "read_only", "query_policy",
  ];
  const entries = Object.entries(data).filter(([k]) => allowed.includes(k));
  if (entries.length === 0) return getConnectionById(id);

  const fields = entries.map(([k]) => `"${k}" = ?`).join(", ");
  db.query(`UPDATE connections SET ${fields} WHERE id = ?`).run(
    ...entries.map(([, v]) => v), id
  );
  return getConnectionById(id);
}
