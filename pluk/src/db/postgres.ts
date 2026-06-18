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
    // NB: statement_timeout is sent in the libpq startup packet, which poolers like
    // PgBouncer (transaction mode, common behind Cloudflare tunnels) reject with
    // "unsupported startup parameter". query_timeout is client-side, so it's safe.
    query_timeout: 20000,
    ...(conn.socket_path ? { host: conn.socket_path } : {}),
    ...(ssl ? { ssl } : {}),
  });

  return {
    async query(sql, params = []) {
      const result = await pool.query(sql, params as unknown[]);
      return { rows: result.rows, fields: result.fields.map((f) => f.name) };
    },

    async explain(sql) {
      const result = await pool.query("EXPLAIN (FORMAT JSON) " + sql);
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

    async sampleTable(table, limit) {
      const quoted = table.replace(/"/g, '""');
      const result = await pool.query(`SELECT * FROM "${quoted}" LIMIT $1`, [limit]);
      return { rows: result.rows, fields: result.fields.map((f) => f.name) };
    },

    async listRelationships(table) {
      let sql = `
        SELECT
          tc.table_name AS from_table,
          kcu.column_name AS from_column,
          ccu.table_name AS to_table,
          ccu.column_name AS to_column,
          tc.constraint_name AS constraint_name
        FROM information_schema.table_constraints tc
        JOIN information_schema.key_column_usage kcu
          ON tc.constraint_name = kcu.constraint_name
          AND tc.table_schema = kcu.table_schema
        JOIN information_schema.constraint_column_usage ccu
          ON ccu.constraint_name = tc.constraint_name
          AND ccu.table_schema = tc.table_schema
        WHERE tc.constraint_type = 'FOREIGN KEY'
          AND tc.table_schema = 'public'
      `;
      const params: string[] = [];
      if (table) {
        sql += " AND tc.table_name = $1";
        params.push(table);
      }
      sql += " ORDER BY tc.table_name, kcu.ordinal_position";
      const result = await pool.query(sql, params);
      return result.rows.map((r) => ({
        from_table: r.from_table as string,
        from_column: r.from_column as string,
        to_table: r.to_table as string,
        to_column: r.to_column as string,
        constraint_name: r.constraint_name as string,
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
