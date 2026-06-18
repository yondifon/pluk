import { readFileSync } from "fs";
import type { Connection } from "../store/connections.js";
import { openSSHTunnel } from "./ssh.js";
import { runWithSqlLog } from "./sqlLog.js";

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
  queryReadOnly(sql: string, params?: unknown[]): Promise<QueryResult>;
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

  driver = instrumentDriver(driver, conn);

  if (!tunnel) return driver;

  // Wrap close() to also shut down the SSH tunnel
  const baseClose = driver.close.bind(driver);
  driver.close = async () => { await baseClose(); tunnel!.close(); };
  return driver;
}

// Tag introspection/utility methods with a source so the driver layer logs the
// SQL they send (the user-facing query/queryReadOnly paths log richly elsewhere,
// so they're intentionally left un-instrumented to avoid duplicate entries).
function instrumentDriver(driver: Driver, conn: Connection): Driver {
  const wrap = <A extends unknown[], R>(source: string, fn: (...args: A) => Promise<R>) =>
    (...args: A): Promise<R> =>
      runWithSqlLog({ connId: conn.id, connName: conn.name, source }, () => fn(...args));

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
