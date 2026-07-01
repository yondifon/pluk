import { readFileSync } from "fs";
import type { Integration } from "../store/integrations.js";
import { openSSHTunnel } from "./ssh.js";
import { runWithSqlLog } from "./sqlLog.js";

/** Flat SQL connection config extracted from an Integration's `config` blob. */
export interface SqlConfig {
  type: string;
  host?: string;
  port?: number;
  user?: string;
  password?: string;
  database?: string;
  filename?: string;
  socket_path?: string;
  use_ssh?: boolean | string;
  ssh_host?: string;
  ssh_port?: number;
  ssh_user?: string;
  ssh_auth_type?: "agent" | "key" | "password";
  ssh_key_path?: string;
  ssh_password?: string;
  use_ssl?: boolean;
  ssl_mode?: "disable" | "require" | "verify-ca" | "verify-full";
  ssl_ca_path?: string;
  ssl_cert_path?: string;
  ssl_key_path?: string;
}

export type SSHConfig = Pick<SqlConfig, "filename" | "ssh_host" | "ssh_port" | "ssh_user" | "ssh_auth_type" | "ssh_key_path" | "ssh_password">;

function sqlConfigFrom(integration: Integration): SqlConfig {
  return { type: integration.type, ...(integration.config as Partial<SqlConfig>) };
}

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
  query(sql: string, params?: unknown[], opts?: { timeoutMs?: number }): Promise<QueryResult>;
  queryReadOnly(sql: string, params?: unknown[], opts?: { timeoutMs?: number }): Promise<QueryResult>;
  explain(sql: string): Promise<QueryResult>;
  listTables(): Promise<string[]>;
  describeTable(table: string): Promise<ColumnInfo[]>;
  sampleTable(table: string, limit: number): Promise<QueryResult>;
  listRelationships(table?: string): Promise<RelationshipInfo[]>;
  searchSchema(term: string): Promise<SchemaSearchResult[]>;
  tableStats(table: string): Promise<TableStats>;
  listSchemas(): Promise<string[]>;
  getFullSchema(): Promise<string>;
  testConnection(): Promise<void>;
  close(): Promise<void>;
}

export async function createDriver(
  integration: Integration,
  sessionId?: string,
  onFatal?: () => void
): Promise<Driver> {
  const cfg = sqlConfigFrom(integration);

  // Resolve effective host/port, tunneling through SSH if configured
  let effectiveHost = cfg.host ?? "localhost";
  let effectivePort = cfg.port ?? defaultPort(cfg.type);
  let tunnel: { close: () => void } | null = null;

  const useSsh = cfg.use_ssh === true || cfg.use_ssh === "true";

  if (cfg.type !== "sqlite" && useSsh && cfg.ssh_host) {
    const t = await openSSHTunnel({
      host: cfg.ssh_host,
      port: cfg.ssh_port ?? 22,
      user: cfg.ssh_user ?? "",
      authType: cfg.ssh_auth_type ?? "agent",
      keyPath: cfg.ssh_key_path,
      passphrase: cfg.ssh_password,
      remoteHost: effectiveHost,
      remotePort: effectivePort,
    }, sessionId, onFatal);
    tunnel = t;
    effectiveHost = "127.0.0.1";
    effectivePort = t.localPort;
  }

  let driver: Driver;

  try {
    const sslConfig = buildSSL(cfg);

    switch (cfg.type) {
      case "postgres": {
        const { createPostgresDriver } = await import("./postgres.js");
        driver = createPostgresDriver(cfg, effectiveHost, effectivePort, sslConfig);
        break;
      }
      case "mysql": {
        const { createMysqlDriver } = await import("./mysql.js");
        driver = createMysqlDriver(cfg, effectiveHost, effectivePort, sslConfig);
        break;
      }
      case "sqlite": {
        if (useSsh) {
          const { createRemoteSqliteDriver } = await import("./sqliteRemote.js");
          driver = createRemoteSqliteDriver(cfg, sessionId ?? `test:${integration.id}`);
        } else {
          const { createSqliteDriver } = await import("./sqlite.js");
          driver = createSqliteDriver(cfg.filename!);
        }
        break;
      }
      default:
        throw new Error(`Unsupported DB type: ${cfg.type}`);
    }
  } catch (err) {
    tunnel?.close();
    throw err;
  }

  driver = instrumentDriver(driver, integration);

  if (!tunnel) return driver;

  // Wrap close() to also shut down the SSH tunnel
  const baseClose = driver.close.bind(driver);
  driver.close = async () => { await baseClose(); tunnel!.close(); };
  return driver;
}

// Tag introspection/utility methods with a source so the driver layer logs the
// SQL they send (the user-facing query/queryReadOnly paths log richly elsewhere,
// so they're intentionally left un-instrumented to avoid duplicate entries).
function instrumentDriver(driver: Driver, integration: Integration): Driver {
  const wrap = <A extends unknown[], R>(source: string, fn: (...args: A) => Promise<R>) =>
    (...args: A): Promise<R> =>
      runWithSqlLog({ connId: integration.id, connName: integration.name, source, group: integration.viaGroup }, () => fn(...args));

  driver.explain = wrap("explain_query", driver.explain.bind(driver));
  driver.listTables = wrap("list_tables", driver.listTables.bind(driver));
  driver.describeTable = wrap("describe_table", driver.describeTable.bind(driver));
  driver.sampleTable = wrap("sample_table", driver.sampleTable.bind(driver));
  driver.listRelationships = wrap("list_relationships", driver.listRelationships.bind(driver));
  driver.searchSchema = wrap("search_schema", driver.searchSchema.bind(driver));
  driver.tableStats = wrap("table_stats", driver.tableStats.bind(driver));
  driver.listSchemas = wrap("list_schemas", driver.listSchemas.bind(driver));
  driver.getFullSchema = wrap("schema_resource", driver.getFullSchema.bind(driver));
  return driver;
}

function defaultPort(type: string) {
  return type === "mysql" ? 3306 : 5432;
}

function buildSSL(cfg: SqlConfig): Record<string, unknown> | false {
  if (!cfg.use_ssl || cfg.ssl_mode === "disable") return false;

  const ssl: Record<string, unknown> = {
    rejectUnauthorized: cfg.ssl_mode === "verify-full" || cfg.ssl_mode === "verify-ca",
  };

  try {
    if (cfg.ssl_ca_path) ssl.ca = readFileSync(cfg.ssl_ca_path);
    if (cfg.ssl_cert_path) ssl.cert = readFileSync(cfg.ssl_cert_path);
    if (cfg.ssl_key_path) ssl.key = readFileSync(cfg.ssl_key_path);
  } catch (e) {
    throw new Error(`SSL cert read error: ${(e as Error).message}`);
  }

  return ssl;
}
