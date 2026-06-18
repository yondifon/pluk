import { readFileSync } from "fs";
import type { Connection } from "../store/connections.js";
import { openSSHTunnel } from "./ssh.js";

export interface QueryResult {
  rows: unknown[];
  fields?: string[];
}

export interface ColumnInfo {
  column: string;
  type: string;
  nullable: boolean;
}

export interface RelationshipInfo {
  from_table: string;
  from_column: string;
  to_table: string;
  to_column: string;
  constraint_name?: string;
}

export interface SchemaSearchResult {
  kind: "table" | "column";
  table: string;
  column?: string;
  type?: string;
}

export interface TableStats {
  table: string;
  estimatedRows: number | null;
  sizeBytes: number | null;
  indexes: { name: string; columns: string[]; unique: boolean }[];
}

export interface Driver {
  query(sql: string, params?: unknown[]): Promise<QueryResult>;
  explain(sql: string): Promise<QueryResult>;
  listTables(): Promise<string[]>;
  describeTable(table: string): Promise<ColumnInfo[]>;
  sampleTable(table: string, limit: number): Promise<QueryResult>;
  listRelationships(table?: string): Promise<RelationshipInfo[]>;
  searchSchema(term: string): Promise<SchemaSearchResult[]>;
  tableStats(table: string): Promise<TableStats>;
  listSchemas(): Promise<string[]>;
  testConnection(): Promise<void>;
  close(): Promise<void>;
}

export async function createDriver(conn: Connection): Promise<Driver> {
  // Resolve effective host/port, tunneling through SSH if configured
  let effectiveHost = conn.host ?? "localhost";
  let effectivePort = conn.port ?? defaultPort(conn.type);
  let tunnel: { close: () => void } | null = null;

  if (conn.use_ssh && conn.ssh_host) {
    const t = await openSSHTunnel({
      host: conn.ssh_host,
      port: conn.ssh_port ?? 22,
      user: conn.ssh_user ?? "",
      authType: conn.ssh_auth_type ?? "agent",
      keyPath: conn.ssh_key_path,
      passphrase: conn.ssh_password,
      remoteHost: effectiveHost,
      remotePort: effectivePort,
    });
    tunnel = t;
    effectiveHost = "127.0.0.1";
    effectivePort = t.localPort;
  }

  let driver: Driver;

  try {
    const sslConfig = buildSSL(conn);

    switch (conn.type) {
      case "postgres": {
        const { createPostgresDriver } = await import("./postgres.js");
        driver = createPostgresDriver(conn, effectiveHost, effectivePort, sslConfig);
        break;
      }
      case "mysql": {
        const { createMysqlDriver } = await import("./mysql.js");
        driver = createMysqlDriver(conn, effectiveHost, effectivePort, sslConfig);
        break;
      }
      case "sqlite": {
        const { createSqliteDriver } = await import("./sqlite.js");
        driver = createSqliteDriver(conn.filename!);
        break;
      }
      default:
        throw new Error(`Unsupported DB type: ${conn.type}`);
    }
  } catch (err) {
    tunnel?.close();
    throw err;
  }

  if (!tunnel) return driver;

  // Wrap close() to also shut down the SSH tunnel
  const baseClose = driver.close.bind(driver);
  driver.close = async () => { await baseClose(); tunnel!.close(); };
  return driver;
}

function defaultPort(type: string) {
  return type === "mysql" ? 3306 : 5432;
}

function buildSSL(conn: Connection): Record<string, unknown> | false {
  if (!conn.use_ssl || conn.ssl_mode === "disable") return false;

  const ssl: Record<string, unknown> = {
    rejectUnauthorized: conn.ssl_mode === "verify-full" || conn.ssl_mode === "verify-ca",
  };

  try {
    if (conn.ssl_ca_path) ssl.ca = readFileSync(conn.ssl_ca_path);
    if (conn.ssl_cert_path) ssl.cert = readFileSync(conn.ssl_cert_path);
    if (conn.ssl_key_path) ssl.key = readFileSync(conn.ssl_key_path);
  } catch (e) {
    throw new Error(`SSL cert read error: ${(e as Error).message}`);
  }

  return ssl;
}
