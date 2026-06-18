import { Database } from "bun:sqlite";
import type { Driver, RelationshipInfo } from "./index.js";
import { recordExecutedSql } from "./sqlLog.js";

export function createSqliteDriver(filename: string): Driver {
  const db = new Database(filename);

  // Tag every statement this db runs with the active source context (no-op
  // outside one). SQLite prepares synchronously and executes at
  // .all()/.get()/.run(), so we log at prepare time without a row count.
  const rawDbQuery = db.query.bind(db);
  (db as unknown as { query: (sql: string) => ReturnType<typeof rawDbQuery> }).query = (sql: string) => {
    recordExecutedSql(sql, null);
    return rawDbQuery(sql);
  };

  return {
    async query(sql) {
      const stmt = db.query(sql);
      const rows = stmt.all();
      return { rows };
    },

    async queryReadOnly(sql) {
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

    async searchSchema(term) {
      const pattern = `%${term.replace(/%/g, "\\%").replace(/_/g, "\\_")}%`;
      const results: { kind: "table" | "column"; table: string; column?: string; type?: string }[] = [];
      const tables = (db.query("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE ? ESCAPE '\\' ORDER BY name").all(pattern) as { name: string }[]).map(r => r.name);
      for (const t of tables) {
        results.push({ kind: "table", table: t });
      }
      const allTables = (db.query("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name").all() as { name: string }[]).map(r => r.name);
      for (const t of allTables) {
        const cols = db.query(`PRAGMA table_info("${t}")`).all() as {
          name: string;
          type: string;
        }[];
        for (const c of cols) {
          const like = db.query("SELECT ? LIKE ? ESCAPE '\\' AS matches").get(c.name, pattern) as { matches: number };
          if (like.matches) {
            results.push({ kind: "column", table: t, column: c.name, type: c.type });
          }
        }
      }
      results.sort((a, b) => a.table.localeCompare(b.table) || a.kind.localeCompare(b.kind) || (a.column ?? "").localeCompare(b.column ?? ""));
      return results;
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

    async tableStats(table) {
      const quoted = table.replace(/"/g, '""');
      const indexes = (db.query(`PRAGMA index_list("${quoted}")`).all() as {
        name: string;
        unique: number;
        origin: string;
      }[]).map((r) => {
        const cols = (db.query(`PRAGMA index_info("${r.name}")`).all() as { name: string }[]).map(c => c.name);
        return { name: r.name, columns: cols, unique: r.unique === 1 };
      });
      return {
        table,
        estimatedRows: null,
        sizeBytes: null,
        indexes,
      };
    },

    async listSchemas() {
      return ["main"];
    },

    async getFullSchema() {
      const tables = (db.query("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name").all() as { name: string }[]).map(r => r.name);
      const lines: string[] = [];
      for (const t of tables) {
        const cols = db.query(`PRAGMA table_info("${t}")`).all() as {
          name: string;
          type: string;
          notnull: number;
          pk: number;
        }[];
        const fks = db.query(`PRAGMA foreign_key_list("${t}")`).all() as {
          from: string;
          to: string;
          table: string;
        }[];
        lines.push(`TABLE ${t} (`);
        for (const c of cols) {
          const pk = c.pk ? " PRIMARY KEY" : "";
          const nullability = c.notnull === 0 ? "NULL" : "NOT NULL";
          lines.push(`  ${c.name} ${c.type} ${nullability}${pk}`);
        }
        lines.push(")");
        for (const fk of fks) {
          lines.push(`FK ${t}.${fk.from} -> ${fk.table}.${fk.to}`);
        }
        lines.push("");
      }
      return lines.join("\n").trim();
    },

    async testConnection() {
      db.query("SELECT 1").get();
    },

    async close() {
      db.close();
    },
  };
}
