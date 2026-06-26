import mysql from "mysql2/promise";
import type { Driver, SqlConfig } from "./index.js";
import { recordExecutedSql } from "./sqlLog.js";

export const mysqlDateStrings = true;

export function createMysqlDriver(
  cfg: SqlConfig,
  host: string,
  port: number,
  ssl: Record<string, unknown> | false
): Driver {
  const pool = mysql.createPool({
    host,
    port,
    user: cfg.user,
    password: cfg.password,
    database: cfg.database,
    connectTimeout: 8000,
    dateStrings: mysqlDateStrings,
    // Keep pooled sockets warm and recycle idle ones. Over a long-lived SSH
    // tunnel an idle connection's forwarded channel can die silently; TCP
    // keepalive surfaces that as an error (fast retry) instead of a hung query,
    // and idleTimeout discards likely-stale connections before reuse.
    enableKeepAlive: true,
    keepAliveInitialDelay: 10_000,
    idleTimeout: 60_000,
    maxIdle: 4,
    ...(cfg.socket_path ? { socketPath: cfg.socket_path } : {}),
    ...(ssl ? { ssl: ssl as mysql.SslOptions } : {}),
  });

  // Tag every statement this pool runs with the active source context (no-op
  // outside one), so introspection/utility SQL lands in the query log too.
  const rawQuery = pool.query.bind(pool) as (...a: unknown[]) => Promise<[unknown, unknown]>;
  (pool as { query: (...a: unknown[]) => Promise<[unknown, unknown]> }).query =
    async (sql: unknown, params?: unknown) => {
      const sqlText = typeof sql === "string" ? sql : ((sql as { sql?: string })?.sql ?? "");
      try {
        const res = await rawQuery(sql, params);
        const rows = Array.isArray(res) ? res[0] : res;
        recordExecutedSql(sqlText, Array.isArray(rows) ? rows.length : null);
        return res;
      } catch (e) {
        recordExecutedSql(sqlText, null, (e as Error).message);
        throw e;
      }
    };

  return {
    async query(sql, params = []) {
      const [rows, fields] = await pool.query(sql, params);
      return {
        rows: rows as unknown[],
        fields: Array.isArray(fields) ? fields.map((f: mysql.FieldPacket) => f.name ?? "") : undefined,
      };
    },

    async queryReadOnly(sql, params = []) {
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

    async tableStats(table) {
      const [tableRows] = await pool.query(
        `SELECT table_rows, data_length + index_length AS size_bytes
         FROM information_schema.tables
         WHERE table_schema = DATABASE() AND table_name = ?`,
        [table]
      );
      const [indexRows] = await pool.query(
        `SELECT index_name, column_name, non_unique
         FROM information_schema.statistics
         WHERE table_schema = DATABASE() AND table_name = ?
         ORDER BY index_name, seq_in_index`,
        [table]
      );
      const idxMap = new Map<string, { columns: string[]; unique: boolean }>();
      for (const r of indexRows as { index_name: string; column_name: string; non_unique: number }[]) {
        const existing = idxMap.get(r.index_name);
        if (existing) {
          existing.columns.push(r.column_name);
        } else {
          idxMap.set(r.index_name, { columns: [r.column_name], unique: r.non_unique === 0 });
        }
      }
      const first = (tableRows as { table_rows: number; size_bytes: number }[])[0];
      return {
        table,
        estimatedRows: first ? first.table_rows : null,
        sizeBytes: first ? first.size_bytes : null,
        indexes: Array.from(idxMap.entries()).map(([name, { columns, unique }]) => ({ name, columns, unique })),
      };
    },

    async listSchemas() {
      const [rows] = await pool.query("SHOW DATABASES");
      return (rows as Record<string, string>[]).map((r) => Object.values(r)[0] ?? "");
    },

    async getFullSchema() {
      const [columns] = await pool.query(
        `SELECT table_name, column_name, data_type, is_nullable, ordinal_position
         FROM information_schema.columns
         WHERE table_schema = DATABASE()
         ORDER BY table_name, ordinal_position`
      );
      const [keys] = await pool.query(
        `SELECT kcu.table_name, kcu.column_name, tc.constraint_type
         FROM information_schema.table_constraints tc
         JOIN information_schema.key_column_usage kcu
           ON tc.constraint_name = kcu.constraint_name
           AND tc.table_schema = kcu.table_schema
         WHERE tc.table_schema = DATABASE()
           AND tc.constraint_type IN ('PRIMARY KEY', 'FOREIGN KEY')`
      );
      const [fks] = await pool.query(
        `SELECT
           kcu.table_name AS from_table,
           kcu.column_name AS from_column,
           kcu.referenced_table_name AS to_table,
           kcu.referenced_column_name AS to_column
         FROM information_schema.key_column_usage kcu
         JOIN information_schema.table_constraints tc
           ON kcu.constraint_name = tc.constraint_name
           AND kcu.table_schema = tc.table_schema
         WHERE tc.constraint_type = 'FOREIGN KEY'
           AND kcu.table_schema = DATABASE()`
      );

      const tables = new Map<string, { column: string; type: string; nullable: boolean; pk: boolean }[]>();
      for (const r of columns as Record<string, string | number>[]) {
        const t = r.table_name as string;
        if (!tables.has(t)) tables.set(t, []);
        tables.get(t)!.push({
          column: r.column_name as string,
          type: r.data_type as string,
          nullable: r.is_nullable === "YES",
          pk: false,
        });
      }
      for (const r of keys as Record<string, string>[]) {
        const t = tables.get(r.table_name ?? "");
        if (!t) continue;
        const col = t.find((c) => c.column === r.column_name);
        if (col && r.constraint_type === "PRIMARY KEY") col.pk = true;
      }

      const lines: string[] = [];
      for (const [table, cols] of tables) {
        lines.push(`TABLE ${table} (`);
        for (const c of cols) {
          const pk = c.pk ? " PRIMARY KEY" : "";
          const nullability = c.nullable ? "NULL" : "NOT NULL";
          lines.push(`  ${c.column} ${c.type} ${nullability}${pk}`);
        }
        lines.push(")");
        for (const fk of fks as Record<string, string>[]) {
          if (fk.from_table === table) {
            lines.push(`FK ${table}.${fk.from_column} -> ${fk.to_table}.${fk.to_column}`);
          }
        }
        lines.push("");
      }
      return lines.join("\n").trim();
    },

    async testConnection() {
      await pool.query("SELECT 1");
    },

    async close() {
      await pool.end();
    },
  };
}
