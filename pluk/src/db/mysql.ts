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
