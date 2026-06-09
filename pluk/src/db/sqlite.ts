import { Database } from "bun:sqlite";
import type { Driver } from "./index.js";

export function createSqliteDriver(filename: string): Driver {
  const db = new Database(filename);

  return {
    async query(sql) {
      const stmt = db.query(sql);
      const rows = stmt.all();
      return { rows };
    },

    async listTables() {
      const rows = db
        .query("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .all() as { name: string }[];
      return rows.map((r) => r.name);
    },

    async describeTable(table) {
      const rows = db.query(`PRAGMA table_info("${table}")`).all() as {
        name: string;
        type: string;
        notnull: number;
      }[];
      return rows.map((r) => ({
        column: r.name,
        type: r.type,
        nullable: r.notnull === 0,
      }));
    },

    async listSchemas() {
      return ["main"];
    },

    async testConnection() {
      db.query("SELECT 1").get();
    },

    async close() {
      db.close();
    },
  };
}
