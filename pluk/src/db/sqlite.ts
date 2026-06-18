import { Database } from "bun:sqlite";
import type { Driver, RelationshipInfo } from "./index.js";

export function createSqliteDriver(filename: string): Driver {
  const db = new Database(filename);

  return {
    async query(sql) {
      const stmt = db.query(sql);
      const rows = stmt.all();
      return { rows };
    },

    async explain(sql) {
      const stmt = db.query("EXPLAIN QUERY PLAN " + sql);
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

    async sampleTable(table, limit) {
      const quoted = table.replace(/"/g, '""');
      const stmt = db.query(`SELECT * FROM "${quoted}" LIMIT ?`);
      const rows = stmt.all(limit);
      return { rows };
    },

    async listRelationships(table) {
      const tables = table ? [table] : (db.query("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name").all() as { name: string }[]).map(r => r.name);
      const results: RelationshipInfo[] = [];
      for (const t of tables) {
        const rows = db.query(`PRAGMA foreign_key_list("${t}")`).all() as {
          from: string;
          to: string;
          table: string;
          id: number;
          seq: number;
        }[];
        for (const r of rows) {
          results.push({
            from_table: t,
            from_column: r.from,
            to_table: r.table,
            to_column: r.to,
            constraint_name: `fk_${t}_${r.id}`,
          });
        }
      }
      return results;
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
