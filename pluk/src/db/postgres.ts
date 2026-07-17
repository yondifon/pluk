import { Pool, types } from "pg";
import type { Driver, SqlConfig } from "./index.js";
import { recordExecutedSql } from "./sqlLog.js";

const TEXT_DATE_TYPE_OIDS = new Set([
  1082, // date
  1114, // timestamp without time zone
  1184, // timestamp with time zone
]);

export const postgresDateTypesAsText = {
  getTypeParser(oid: number, format?: "text" | "binary") {
    if ((format ?? "text") === "text" && TEXT_DATE_TYPE_OIDS.has(oid)) {
      return (value: string) => value;
    }

    return types.getTypeParser(oid, format);
  },
};

function queryConfig(sql: string, params: unknown[], timeoutMs?: number) {
  return timeoutMs ? { text: sql, values: params, query_timeout: timeoutMs } : { text: sql, values: params };
}

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
    types: postgresDateTypesAsText,
    // Keep pooled sockets warm and bound their lifetime. Over a long-lived SSH
    // tunnel an idle connection's forwarded channel can die silently; TCP
    // keepalive surfaces that fast (default idleTimeoutMillis already recycles
    // idle clients after 10s), and maxLifetimeSeconds caps connection age so a
    // long session never keeps reusing one stale socket.
    keepAlive: true,
    maxLifetimeSeconds: 1800,
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

  // On abort, cancel the running statement server-side from a *separate* pooled
  // connection (the busy one can't take commands). Returns a cleanup to detach
  // the listener. Cancelling a finished query is a harmless no-op, so errors are
  // swallowed.
  function onAbortCancel(pid: number | undefined, signal: AbortSignal | undefined): () => void {
    if (!pid || !signal) return () => {};
    const handler = () => { pool.query("SELECT pg_cancel_backend($1)", [pid]).catch(() => {}); };
    signal.addEventListener("abort", handler, { once: true });
    return () => signal.removeEventListener("abort", handler);
  }

  return {
    async query(sql, params = [], opts) {
      // No signal → the simple pooled path (introspection, cost gate, etc.).
      if (!opts?.signal) {
        const result = await pool.query(queryConfig(sql, params as unknown[], opts?.timeoutMs));
        return { rows: result.rows, fields: result.fields.map((f) => f.name) };
      }
      // Cancellable path: a dedicated connection so we know its backend pid.
      const client = await pool.connect();
      const detach = onAbortCancel((client as { processID?: number }).processID,opts.signal);
      try {
        const result = await client.query(queryConfig(sql, params as unknown[], opts?.timeoutMs));
        return { rows: result.rows, fields: result.fields.map((f) => f.name) };
      } finally {
        detach();
        client.release();
      }
    },

    async queryReadOnly(sql, params = [], opts) {
      const client = await pool.connect();
      const detach = onAbortCancel((client as { processID?: number }).processID,opts?.signal);
      try {
        await client.query("BEGIN READ ONLY");
        const result = await client.query(queryConfig(sql, params as unknown[], opts?.timeoutMs));
        return { rows: result.rows, fields: result.fields.map((f) => f.name) };
      } finally {
        detach();
        await client.query("ROLLBACK").catch(() => {});
        client.release();
      }
    },

    async explain(sql, params = []) {
      const result = await pool.query({ text: "EXPLAIN (FORMAT JSON) " + sql, values: params as unknown[] });
      return { rows: result.rows, fields: result.fields.map((f) => f.name) };
    },

    async listTables(schema = "public") {
      const result = await pool.query(
        "SELECT tablename FROM pg_tables WHERE schemaname = $1 ORDER BY tablename",
        [schema]
      );
      return result.rows.map((r) => r.tablename as string);
    },

    async describeTable(table, schema = "public") {
      const result = await pool.query(
        `SELECT column_name, data_type, is_nullable
         FROM information_schema.columns
         WHERE table_schema = $2 AND table_name = $1
         ORDER BY ordinal_position`,
        [table, schema]
      );
      return result.rows.map((r) => ({
        column: r.column_name as string,
        type: r.data_type as string,
        nullable: r.is_nullable === "YES",
      }));
    },

    async sampleTable(table, limit, schema = "public") {
      const quoted = table.replace(/"/g, '""');
      const quotedSchema = schema.replace(/"/g, '""');
      const result = await pool.query(`SELECT * FROM "${quotedSchema}"."${quoted}" LIMIT $1`, [limit]);
      return { rows: result.rows, fields: result.fields.map((f) => f.name) };
    },

    async searchSchema(term, schema = "public") {
      const pattern = `%${term.replace(/\\/g, "\\\\").replace(/%/g, "\\%").replace(/_/g, "\\_")}%`;
      const result = await pool.query(
        `
        SELECT 'table' AS kind, table_name AS "table", NULL::text AS "column", NULL::text AS type
        FROM information_schema.tables
        WHERE table_schema = $2 AND table_name ILIKE $1
        UNION ALL
        SELECT 'column', c.table_name, c.column_name, c.data_type
        FROM information_schema.columns c
        JOIN information_schema.tables t
          ON c.table_schema = t.table_schema AND c.table_name = t.table_name
        WHERE c.table_schema = $2
          AND (c.column_name ILIKE $1 OR c.table_name ILIKE $1)
        ORDER BY "table", kind, "column"
        `,
        [pattern, schema]
      );
      return result.rows.map((r) => ({
        kind: r.kind as "table" | "column",
        table: r.table as string,
        column: r.column as string | undefined,
        type: r.type as string | undefined,
      }));
    },

    async listRelationships(table, schema = "public") {
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
          AND tc.table_schema = $1
      `;
      const params: string[] = [schema];
      if (table) {
        sql += " AND tc.table_name = $2";
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

    async tableStats(table, schema = "public") {
      const rel = await pool.query(
        `SELECT c.reltuples AS estimated_rows, pg_total_relation_size(c.oid) AS size_bytes
         FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = $2 AND c.relname = $1`,
        [table, schema]
      );
      const indexRes = await pool.query(
        `SELECT indexname, indexdef
         FROM pg_indexes
         WHERE schemaname = $2 AND tablename = $1
         ORDER BY indexname`,
        [table, schema]
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

    async listDatabases() {
      // Real databases in the cluster (not schemas). Template databases are
      // excluded — they can't be connected to for queries anyway.
      const result = await pool.query(
        "SELECT datname FROM pg_database WHERE datistemplate = false AND datallowconn = true ORDER BY datname"
      );
      return result.rows.map((r) => r.datname as string);
    },

    async getFullSchema(schema = "public") {
      const columns = await pool.query(
        `SELECT table_name, column_name, data_type, is_nullable, ordinal_position
         FROM information_schema.columns
         WHERE table_schema = $1
         ORDER BY table_name, ordinal_position`,
        [schema]
      );
      const keys = await pool.query(
        `SELECT kcu.table_name, kcu.column_name, tc.constraint_type
         FROM information_schema.table_constraints tc
         JOIN information_schema.key_column_usage kcu
           ON tc.constraint_name = kcu.constraint_name
           AND tc.table_schema = kcu.table_schema
         WHERE tc.table_schema = $1
           AND tc.constraint_type IN ('PRIMARY KEY', 'FOREIGN KEY')`,
        [schema]
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
           AND tc.table_schema = $1`,
        [schema]
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
      await pool.query("SELECT 1");
    },

    async close() {
      await pool.end();
    },
  };
}
