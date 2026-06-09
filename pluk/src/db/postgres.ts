import { Pool } from "pg";
import type { Driver } from "./index.js";
import type { Connection } from "../store/connections.js";

export function createPostgresDriver(
  conn: Connection,
  host: string,
  port: number,
  ssl: Record<string, unknown> | false
): Driver {
  const pool = new Pool({
    host,
    port,
    user: conn.user,
    password: conn.password,
    database: conn.database,
    connectionTimeoutMillis: 8000,
    statement_timeout: 15000,
    query_timeout: 20000,
    ...(conn.socket_path ? { host: conn.socket_path } : {}),
    ...(ssl ? { ssl } : {}),
  });

  return {
    async query(sql, params = []) {
      const result = await pool.query(sql, params as unknown[]);
      return { rows: result.rows, fields: result.fields.map((f) => f.name) };
    },

    async listTables() {
      const result = await pool.query(
        "SELECT tablename FROM pg_tables WHERE schemaname = 'public' ORDER BY tablename"
      );
      return result.rows.map((r) => r.tablename as string);
    },

    async describeTable(table) {
      const result = await pool.query(
        `SELECT column_name, data_type, is_nullable
         FROM information_schema.columns
         WHERE table_schema = 'public' AND table_name = $1
         ORDER BY ordinal_position`,
        [table]
      );
      return result.rows.map((r) => ({
        column: r.column_name as string,
        type: r.data_type as string,
        nullable: r.is_nullable === "YES",
      }));
    },

    async listSchemas() {
      const result = await pool.query(
        "SELECT schema_name FROM information_schema.schemata ORDER BY schema_name"
      );
      return result.rows.map((r) => r.schema_name as string);
    },

    async testConnection() {
      const client = await pool.connect();
      client.release();
    },

    async close() {
      await pool.end();
    },
  };
}
