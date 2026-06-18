import mysql from "mysql2/promise";
import type { Driver } from "./index.js";
import type { Connection } from "../store/connections.js";

export function createMysqlDriver(
  conn: Connection,
  host: string,
  port: number,
  ssl: Record<string, unknown> | false
): Driver {
  const pool = mysql.createPool({
    host,
    port,
    user: conn.user,
    password: conn.password,
    database: conn.database,
    connectTimeout: 8000,
    ...(conn.socket_path ? { socketPath: conn.socket_path } : {}),
    ...(ssl ? { ssl: ssl as mysql.SslOptions } : {}),
  });

  return {
    async query(sql, params = []) {
      const [rows, fields] = await pool.query(sql, params);
      return {
        rows: rows as unknown[],
        fields: Array.isArray(fields) ? fields.map((f: mysql.FieldPacket) => f.name ?? "") : undefined,
      };
    },

    async explain(sql) {
      const [rows, fields] = await pool.query("EXPLAIN " + sql);
      return {
        rows: rows as unknown[],
        fields: Array.isArray(fields) ? fields.map((f: mysql.FieldPacket) => f.name ?? "") : undefined,
      };
    },

    async listTables() {
      const [rows] = await pool.query("SHOW TABLES");
      return (rows as Record<string, string>[]).map((r) => Object.values(r)[0] ?? "");
    },

    async describeTable(table) {
      const [rows] = await pool.query(`DESCRIBE \`${table}\``);
      return (rows as Record<string, string>[]).map((r) => ({
        column: r.Field ?? "",
        type: r.Type ?? "",
        nullable: r.Null === "YES",
      }));
    },

    async sampleTable(table, limit) {
      const quoted = table.replace(/`/g, "``");
      const [rows, fields] = await pool.query(`SELECT * FROM \`${quoted}\` LIMIT ?`, [limit]);
      return {
        rows: rows as unknown[],
        fields: Array.isArray(fields) ? fields.map((f: mysql.FieldPacket) => f.name ?? "") : undefined,
      };
    },

    async searchSchema(term) {
      const pattern = `%${term.replace(/\\/g, "\\\\").replace(/%/g, "\\%").replace(/_/g, "\\_")}%`;
      const [rows] = await pool.query(
        `
        SELECT 'table' AS kind, table_name AS \`table\`, NULL AS \`column\`, NULL AS type
        FROM information_schema.tables
        WHERE table_schema = DATABASE() AND table_name LIKE ?
        UNION ALL
        SELECT 'column', c.table_name, c.column_name, c.data_type
        FROM information_schema.columns c
        JOIN information_schema.tables t
          ON c.table_schema = t.table_schema AND c.table_name = t.table_name
        WHERE c.table_schema = DATABASE()
          AND (c.column_name LIKE ? OR c.table_name LIKE ?)
        ORDER BY \`table\`, kind, \`column\`
        `,
        [pattern, pattern, pattern]
      );
      return (rows as Record<string, string>[]).map((r) => ({
        kind: r.kind as "table" | "column",
        table: r.table ?? "",
        column: r.column ?? undefined,
        type: r.type ?? undefined,
      }));
    },

    async listRelationships(table) {
      let sql = `
        SELECT
          kcu.table_name AS from_table,
          kcu.column_name AS from_column,
          kcu.referenced_table_name AS to_table,
          kcu.referenced_column_name AS to_column,
          kcu.constraint_name AS constraint_name
        FROM information_schema.key_column_usage kcu
        JOIN information_schema.table_constraints tc
          ON kcu.constraint_name = tc.constraint_name
          AND kcu.table_schema = tc.table_schema
        WHERE tc.constraint_type = 'FOREIGN KEY'
          AND kcu.table_schema = DATABASE()
      `;
      const params: string[] = [];
      if (table) {
        sql += " AND kcu.table_name = ?";
        params.push(table);
      }
      sql += " ORDER BY kcu.table_name, kcu.ordinal_position";
      const [rows] = await pool.query(sql, params);
      return (rows as Record<string, string>[]).map((r) => ({
        from_table: r.from_table ?? "",
        from_column: r.from_column ?? "",
        to_table: r.to_table ?? "",
        to_column: r.to_column ?? "",
        constraint_name: r.constraint_name ?? "",
      }));
    },

    async listSchemas() {
      const [rows] = await pool.query("SHOW DATABASES");
      return (rows as Record<string, string>[]).map((r) => Object.values(r)[0] ?? "");
    },

    async testConnection() {
      const c = await pool.getConnection();
      c.release();
    },

    async close() {
      await pool.end();
    },
  };
}
