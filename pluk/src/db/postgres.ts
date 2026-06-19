import { Pool } from "pg";
import type { Driver, SqlConfig } from "./index.js";
import { recordExecutedSql } from "./sqlLog.js";

export function createPostgresDriver(
  cfg: SqlConfig,
  host: string,
  port: number,
  ssl: Record<string, unknown> | false
): Driver {
  const pool = new Pool({
    host,
    port,
    user: cfg.user,
    password: cfg.password,
    database: cfg.database,
    connectionTimeoutMillis: 8000,
    // NB: statement_timeout is sent in the libpq startup packet, which poolers like
    // PgBouncer (transaction mode, common behind Cloudflare tunnels) reject with
    // "unsupported startup parameter". query_timeout is client-side, so it's safe.
    query_timeout: 20000,
    ...(cfg.socket_path ? { host: cfg.socket_path } : {}),
    // Over TLS, negotiate SCRAM channel binding (SCRAM-SHA-256-PLUS) when the
    // server offers it. node-pg leaves this off by default and falls back to a
    // bare gs2 "n" flag, which PgBouncer rejects with 08P01 ("SASL authentication
    // failed") — even though libpq clients (psql, TablePlus) connect fine.
    ...(ssl ? { ssl, enableChannelBinding: true } : {}),
  });

  // Tag every statement this pool runs with the active source context (no-op
  // outside one), so introspection/utility SQL lands in the query log too.
  const rawQuery = pool.query.bind(pool);
  (pool as { query: (...a: unknown[]) => Promise<{ rowCount?: number | null; rows?: unknown[] }> }).query =
    async (text: unknown, params?: unknown) => {
      const sql = typeof text === "string" ? text : ((text as { text?: string })?.text ?? "");
      try {
        const res = await (rawQuery as (...a: unknown[]) => Promise<{ rowCount?: number | null; rows?: unknown[] }>)(text, params);
        recordExecutedSql(sql, res.rowCount ?? res.rows?.length ?? null);
        return res;
      } catch (e) {
        recordExecutedSql(sql, null, (e as Error).message);
        throw e;
      }
    };

  return {
    async query(sql, params = []) {
      const result = await pool.query(sql, params as unknown[]);
      return { rows: result.rows, fields: result.fields.map((f) => f.name) };
    },

    async queryReadOnly(sql, params = []) {
      const client = await pool.connect();
      try {
        await client.query("BEGIN READ ONLY");
        const result = await client.query(sql, params as unknown[]);
        return { rows: result.rows, fields: result.fields.map((f) => f.name) };
      } finally {
        await client.query("ROLLBACK").catch(() => {});
        client.release();
      }
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

    async searchSchema(term) {
      const pattern = `%${term.replace(/\\/g, "\\\\").replace(/%/g, "\\%").replace(/_/g, "\\_")}%`;
      const result = await pool.query(
        `
        SELECT 'table' AS kind, table_name AS "table", NULL::text AS "column", NULL::text AS type
        FROM information_schema.tables
        WHERE table_schema = 'public' AND table_name ILIKE $1
        UNION ALL
        SELECT 'column', c.table_name, c.column_name, c.data_type
        FROM information_schema.columns c
        JOIN information_schema.tables t
          ON c.table_schema = t.table_schema AND c.table_name = t.table_name
        WHERE c.table_schema = 'public'
          AND (c.column_name ILIKE $1 OR c.table_name ILIKE $1)
        ORDER BY "table", kind, "column"
        `,
        [pattern]
      );
      return result.rows.map((r) => ({
        kind: r.kind as "table" | "column",
        table: r.table as string,
        column: r.column as string | undefined,
        type: r.type as string | undefined,
      }));
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

    async tableStats(table) {
      const rel = await pool.query(
        `SELECT c.reltuples AS estimated_rows, pg_total_relation_size(c.oid) AS size_bytes
         FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'public' AND c.relname = $1`,
        [table]
      );
      const indexRes = await pool.query(
        `SELECT indexname, indexdef
         FROM pg_indexes
         WHERE schemaname = 'public' AND tablename = $1
         ORDER BY indexname`,
        [table]
      );
      const indexes = (indexRes.rows as { indexname: string; indexdef: string }[]).map((r) => {
        const match = r.indexdef.match(/\(([^)]+)\)/);
        return {
          name: r.indexname,
          columns: match ? match[1]!.split(",").map(c => c.trim()) : [],
          unique: /UNIQUE/i.test(r.indexdef),
        };
      });
      return {
        table,
        estimatedRows: rel.rows[0] ? Math.round(rel.rows[0].estimated_rows as number) : null,
        sizeBytes: rel.rows[0] ? (rel.rows[0].size_bytes as number) : null,
        indexes,
      };
    },

    async listSchemas() {
      const result = await pool.query(
        "SELECT schema_name FROM information_schema.schemata ORDER BY schema_name"
      );
      return result.rows.map((r) => r.schema_name as string);
    },

    async getFullSchema() {
      const columns = await pool.query(
        `SELECT table_name, column_name, data_type, is_nullable, ordinal_position
         FROM information_schema.columns
         WHERE table_schema = 'public'
         ORDER BY table_name, ordinal_position`
      );
      const keys = await pool.query(
        `SELECT kcu.table_name, kcu.column_name, tc.constraint_type
         FROM information_schema.table_constraints tc
         JOIN information_schema.key_column_usage kcu
           ON tc.constraint_name = kcu.constraint_name
           AND tc.table_schema = kcu.table_schema
         WHERE tc.table_schema = 'public'
           AND tc.constraint_type IN ('PRIMARY KEY', 'FOREIGN KEY')`
      );
      const fks = await pool.query(
        `SELECT
           tc.table_name AS from_table,
           kcu.column_name AS from_column,
           ccu.table_name AS to_table,
           ccu.column_name AS to_column
         FROM information_schema.table_constraints tc
         JOIN information_schema.key_column_usage kcu
           ON tc.constraint_name = kcu.constraint_name
           AND tc.table_schema = kcu.table_schema
         JOIN information_schema.constraint_column_usage ccu
           ON ccu.constraint_name = tc.constraint_name
           AND ccu.table_schema = tc.table_schema
         WHERE tc.constraint_type = 'FOREIGN KEY'
           AND tc.table_schema = 'public'`
      );

      const tables = new Map<string, { column: string; type: string; nullable: boolean; pk: boolean }[]>();
      for (const r of columns.rows) {
        const t = r.table_name as string;
        if (!tables.has(t)) tables.set(t, []);
        tables.get(t)!.push({
          column: r.column_name as string,
          type: r.data_type as string,
          nullable: r.is_nullable === "YES",
          pk: false,
        });
      }
      for (const r of keys.rows) {
        const t = tables.get(r.table_name as string);
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
        for (const fk of fks.rows) {
          if (fk.from_table === table) {
            lines.push(`FK ${table}.${fk.from_column} -> ${fk.to_table}.${fk.to_column}`);
          }
        }
        lines.push("");
      }
      return lines.join("\n").trim();
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
