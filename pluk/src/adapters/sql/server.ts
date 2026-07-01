import { z } from "zod";
import { mkdirSync } from "fs";
import { homedir } from "os";
import { join } from "path";
import type { Integration } from "../../store/integrations.js";
import type { Driver } from "../../db/index.js";
import { sqlPolicyFromSettings, evaluate, capRows, dialectFor, policyDescription, parsePostgresCost } from "../../mcp/policy.js";
import { toolGate } from "../../mcp/toolConfig.js";
import type { ConfigField, ToolSpec } from "../types.js";
import { logQuery } from "../../store/queryLog.js";
import { listSavedQueries, getSavedQuery } from "../../store/savedQueries.js";
import { listMaskedColumns, maskRow } from "../../store/maskedColumns.js";
import {
  getDriver,
  evictDriver,
  withToolTimeout,
  withCancellable,
  registerQueryAbort,
  clearQueryAbort,
} from "./pool.js";
import { logError } from "../../log.js";
import { buildInstructions } from "../../mcp/instructions.js";
import { ok, err, runGated, type ToolResult, type LogSnapshot } from "../kit.js";
import type { ToolHost } from "../../mcp/namespace.js";
import { formatSqlError } from "./errors.js";

// Human label for a SQL adapter id — single source for the manifest and the
// agent-facing instructions so they never drift.
export function sqlLabel(type: string): string {
  switch (type) {
    case "postgres": return "PostgreSQL";
    case "mysql": return "MySQL";
    case "sqlite": return "SQLite";
    default: return type;
  }
}

export function sqlAgentHint(type: string): string {
  const db = type === "sqlite" ? "SQLite" : type === "mysql" ? "MySQL" : "PostgreSQL";
  return type === "sqlite"
    ? `Use this to query and inspect a ${db} database — read schema and rows, run SELECTs, and write only when the policy permits. Use SELECT with LIMIT before wider queries.`
    : `Use this to query and inspect a ${db} database — read schema and rows, run SELECTs, and write only when the policy permits. Use SELECT with LIMIT for production data.`;
}

// Live, per-session guidance handed to connecting agents (see instructions.ts).
// Reflects the current query policy, so a read-only DB and an unrestricted one
// announce different constraints.
export function sqlInstructions(conn: Integration): string {
  const gate = toolGate(conn.query_policy);
  const policy = sqlPolicyFromSettings(gate.settings("query"));
  return buildInstructions(conn, {
    kind: sqlLabel(conn.type),
    access: "Query and inspect this database. Every statement is checked against the policy below and recorded in the activity log.",
    policy: policyDescription(policy),
    start: "Start with list_tables and describe_table to learn the schema, then read with SELECT … LIMIT.",
    hint: sqlAgentHint(conn.type),
  });
}

// The `query` tool's settings: the SQL policy, expressed as a single mode plus
// structural guards. Shared by every statement-running tool on the adapter.
const QUERY_SETTINGS: ConfigField[] = [
  {
    key: "mode", label: "Statements", type: "select", default: "read-only",
    help: "Which kinds of SQL this connection may run.",
    options: [
      { value: "read-only", label: "Read-only (SELECT)" },
      { value: "mutations", label: "Mutations (INSERT/UPDATE/DELETE)" },
      { value: "destructive", label: "Destructive (DROP/TRUNCATE, DDL)" },
    ],
  },
  { key: "require_where", label: "Require WHERE on UPDATE/DELETE", type: "toggle", default: true,
    help: "Block UPDATE or DELETE without a WHERE clause." },
  { key: "block_stacked", label: "Block stacked statements", type: "toggle", default: true,
    help: "Reject queries containing more than one statement (SELECT 1; DROP …)." },
  { key: "allow_filesystem", label: "Allow filesystem / COPY ops", type: "toggle", default: false, danger: true,
    help: "Allow COPY … PROGRAM, INTO OUTFILE, LOAD DATA, ATTACH DATABASE, pg_read_file." },
  { key: "max_rows", label: "Max rows returned", type: "number", default: 1000,
    help: "Cap rows returned to the agent. 0 = no cap." },
];

// Static tool catalog for the SQL family. Every tool is individually toggleable
// (all default on); only `query` carries settings (they form the SQL policy that
// also governs export_query / run_saved_query).
export function sqlToolSpecs(): ToolSpec[] {
  const read = (name: string, description: string, settings?: ConfigField[]): ToolSpec =>
    ({ name, description, category: "read", defaultEnabled: true, settings });
  return [
    read("query", "Run a SQL query against the database.", QUERY_SETTINGS),
    read("list_tables", "List all tables in the database."),
    read("sample_table", "Preview rows from a table without writing SQL."),
    read("explain_query", "Show a query's execution plan without running it."),
    read("describe_table", "Get column definitions for a table."),
    read("list_relationships", "List foreign key relationships between tables."),
    read("search_schema", "Find tables or columns matching a term."),
    read("table_stats", "Get cheap table statistics (estimated rows, size, indexes)."),
    read("list_schemas", "List all schemas or databases."),
    read("export_query", "Run a SQL query and save results to a local CSV or JSON file."),
    read("run_saved_query", "Run a saved query by name."),
    read("list_saved_queries", "List saved queries for this connection."),
  ];
}

// Register the SQL surface onto a host (a bare McpServer for a single endpoint,
// or a namespaced host when aggregated into a group).
export function registerSqlServer(server: ToolHost, conn: Integration, sessionIdRef: { value: string }): void {
  const gate = toolGate(conn.query_policy);
  // The SQL policy (mode + guards) lives as the `query` tool's settings and
  // governs every statement-running tool (query / export_query / run_saved_query).
  const policy = sqlPolicyFromSettings(gate.settings("query"));
  const dialect = dialectFor(conn.type);
  const policyDesc = policyDescription(policy);
  // Every SQL tool is individually toggleable; all default on. A disabled tool is
  // simply not registered, so the agent never sees it.
  const on = (name: string): boolean => gate.enabled(name, true);

  const readOnlyMode = policy.allowed.length === 2 && policy.allowed.includes("select") && policy.allowed.includes("inspect");
  const maskedColumns = listMaskedColumns(conn.id);
  const usesSsh = conn.config.use_ssh === true || conn.config.use_ssh === "true";
  const timeoutOption: Record<string, z.ZodTypeAny> = conn.type === "sqlite" && !usesSsh ? {} : {
    timeout: z.number().int().positive().max(600).optional().describe("Max seconds to wait before aborting the query (default 30)."),
  };

  // Read-only introspection tools share one shape: acquire the pooled driver,
  // run under the tool timeout, evict on failure. `fn` produces the response text.
  // Introspection statements are recorded by the driver layer, so there is no
  // tool-level log entry here (only the gated query tools below create one).
  async function introspect(label: string, fn: (driver: Driver) => Promise<string>): Promise<ToolResult> {
    const sid = sessionIdRef.value;
    try {
      const driver = await getDriver(sid, conn);
      return await withToolTimeout((async (): Promise<ToolResult> => ok(await fn(driver)))(), label);
    } catch (e) {
      evictDriver(sid, conn.id);
      logError(`tool ${label} failed`, e, { integration: conn.name, type: conn.type });
      return err(formatSqlError(e));
    }
  }

  type QueryRows = { rows: unknown[]; fields?: string[] };
  type SqlResultMeta = {
    env: string;
    host: string;
    connection: string;
    type: string;
    fields?: string[];
    rows: unknown[];
    truncated: boolean;
    row_cap: number | null;
    row_count: number;
    returned_rows: number;
  };

  function resultWithMeta(result: QueryRows, rows: unknown[], truncated: boolean, rowCap: number | null): SqlResultMeta {
    return {
      env: conn.environment ?? "development",
      host: String(conn.config.host ?? conn.config.filename ?? "localhost"),
      connection: conn.name,
      type: conn.type,
      fields: result.fields ?? [],
      rows,
      truncated,
      row_cap: rowCap,
      row_count: result.rows.length,
      returned_rows: rows.length,
    };
  }

  function missingSqlArg(): ToolResult {
    return err('Missing SQL. Pass either "sql" or "query".');
  }

  function toTimeoutMs(timeout?: number): number | undefined {
    return (conn.type === "sqlite" && !usesSsh) || !timeout ? undefined : timeout * 1000;
  }

  // The postgres cost gate: returns a block reason if the planner's estimate
  // exceeds the policy's row/cost ceiling, else undefined. No-op off postgres or
  // when no ceiling is set.
  async function costBlock(driver: Driver, sql: string): Promise<string | undefined> {
    const enabled = policy.maxEstimatedRows !== null || policy.maxEstimatedCost !== null;
    if (!enabled || conn.type !== "postgres") return undefined;
    const explain = await driver.explain(sql);
    const plan = Array.isArray(explain.rows[0]) ? explain.rows[0][0] : explain.rows[0];
    const estimate = parsePostgresCost(plan);
    if (
      (policy.maxEstimatedRows !== null && estimate.rows !== null && estimate.rows > policy.maxEstimatedRows) ||
      (policy.maxEstimatedCost !== null && estimate.cost !== null && estimate.cost > policy.maxEstimatedCost)
    ) {
      return `Query cost gate exceeded (estimated rows: ${estimate.rows ?? "?"}, cost: ${estimate.cost ?? "?"}).`;
    }
    return undefined;
  }

  // Execute a policy-passed statement under the tool timeout + per-query abort.
  // Postgres read-only mode uses a read-only transaction.
  function runStatement<T extends QueryRows>(driver: Driver, sql: string, signal: AbortSignal, label: string, timeoutMs?: number): Promise<T> {
    const useReadOnly = readOnlyMode && conn.type === "postgres";
    const opts = timeoutMs ? { timeoutMs } : undefined;
    const work = (useReadOnly ? driver.queryReadOnly(sql, undefined, opts) : driver.query(sql, undefined, opts)) as Promise<T>;
    return withToolTimeout(withCancellable(work, signal), label, timeoutMs);
  }

  // Apply the masked-column policy to a row set.
  function mask(rows: unknown[]): unknown[] {
    return maskedColumns.length > 0 ? rows.map((r) => maskRow(r as Record<string, unknown>, maskedColumns)) : rows;
  }

  // Shared SQL error handling for the gated query tools: aborted queries are
  // "cancelled" (no driver eviction); everything else evicts + logs.
  const queryGateOpts = {
    classifyError: (msg: string) => (msg.includes("cancelled") ? "cancelled" : "error") as "cancelled" | "error",
    onError: (e: unknown) => {
      evictDriver(sessionIdRef.value, conn.id);
      logError("query tool failed", e, { integration: conn.name, type: conn.type });
    },
    formatError: (e: unknown) => formatSqlError(e),
  };

  // Run a SQL statement through the policy gate + activity log, returning rows
  // (masked + row-capped). Shared by `query` and `run_saved_query`.
  function gatedQuery(sql: string, source: string, timeoutMs?: number, rowCap?: number): Promise<ToolResult> {
    const verdict = evaluate(sql, policy, dialect);
    const effectiveCap = policy.maxRows === null ? rowCap ?? null : Math.min(rowCap ?? policy.maxRows, policy.maxRows);
    return runGated(
      conn,
      { category: verdict.categories, action: source, detail: sql },
      async (logId) => {
        const sid = sessionIdRef.value;
        const queryAc = registerQueryAbort(logId, sid);
        try {
          const driver = await getDriver(sid, conn);
          const block = await costBlock(driver, sql);
          if (block) return { blocked: block };

          const result = await runStatement<QueryRows>(driver, sql, queryAc.signal, source, timeoutMs);
          const { rows, truncated, limit } = capRows(result.rows, effectiveCap);
          const maskedRows = mask(rows);
          const maskedResult: LogSnapshot & QueryRows = { fields: result.fields, rows: maskedRows };
          let text = JSON.stringify(resultWithMeta(result, maskedRows, truncated, limit), null, 2);
          if (truncated) {
            text += `\n\n[Row limit: showing first ${limit} of ${result.rows.length} rows. Add a LIMIT clause to see all results.]`;
          }
          return { text, result: maskedResult };
        } finally {
          clearQueryAbort(logId);
        }
      },
      { precheck: () => (verdict.ok ? undefined : verdict.reason ?? "blocked"), ...queryGateOpts },
    );
  }

  server.prompt(
    "summarize_schema",
    "Generate a concise summary of the database schema and relationships",
    async () => ({
      messages: [
        { role: "user", content: { type: "text", text: "Read the full schema resource, then list the main tables, their purpose, and how they relate to each other." } },
      ],
    })
  );

  server.prompt(
    "investigate_slow_query",
    "Analyze a slow query using EXPLAIN and table stats",
    { sql: z.string().describe("SQL query to investigate") },
    async ({ sql }) => ({
      messages: [
        { role: "user", content: { type: "text", text: `Investigate why this query is slow. Use explain_query and table_stats, then suggest indexes or rewrites.

${sql}` } },
      ],
    })
  );

  server.prompt(
    "find_unused_indexes",
    "Find indexes that may be unused or redundant",
    async () => ({
      messages: [
        { role: "user", content: { type: "text", text: "List all tables and their indexes. Flag any indexes that look redundant or likely unused based on column patterns." } },
      ],
    })
  );

  server.resource(
    "schema",
    "schema://full",
    { mimeType: "text/plain", description: "Full database schema: tables, columns, primary keys, foreign keys" },
    async () => {
      const sid = sessionIdRef.value;
      try {
        const driver = await getDriver(sid, conn);
        return await withToolTimeout((async () => {
          const text = await driver.getFullSchema();
          return { contents: [{ uri: "schema://full", mimeType: "text/plain", text }] };
        })(), "schema_resource");
      } catch (err) {
        evictDriver(sid, conn.id);
        return { contents: [{ uri: "schema://full", mimeType: "text/plain", text: `Error: ${(err as Error).message}` }] };
      }
    }
  );

  if (on("query")) server.tool(
    "query",
    `Run a SQL query against the database. ${policyDesc}`,
    {
      sql: z.string().optional().describe("SQL query to execute"),
      query: z.string().optional().describe("Alias for sql"),
      ...timeoutOption,
      limit: z.number().int().positive().max(1_000_000).optional().describe("Max rows to return, overriding the default cap (1000)."),
    },
    (args) => {
      const { sql, query, limit } = args;
      const timeout = (args as typeof args & { timeout?: number }).timeout;
      const statement = sql ?? query;
      return statement ? gatedQuery(statement, "query", toTimeoutMs(timeout), limit) : Promise.resolve(missingSqlArg());
    },
  );

  if (on("list_tables")) server.tool("list_tables", "List all tables in the database", () =>
    introspect("list_tables", async (driver) => (await driver.listTables()).join("\n")));

  if (on("sample_table")) server.tool(
    "sample_table",
    "Preview rows from a table without writing SQL",
    {
      table: z.string().describe("Table name"),
      limit: z.number().int().min(1).max(1000).default(20).describe("Max rows to preview"),
    },
    ({ table, limit }) =>
      introspect("sample_table", async (driver) => {
        const effectiveLimit = policy.maxRows === null ? limit : Math.min(limit, policy.maxRows);
        const result = await driver.sampleTable(table, effectiveLimit);
        const { rows, truncated, limit: rowCap } = capRows(result.rows, policy.maxRows);
        const maskedRows = maskedColumns.length > 0 ? rows.map(r => maskRow(r as Record<string, unknown>, maskedColumns)) : rows;
        let text = JSON.stringify(resultWithMeta(result, maskedRows, truncated, rowCap), null, 2);
        if (truncated) {
          text += `\n\n[Row limit: showing first ${policy.maxRows} of ${result.rows.length} rows.]`;
        }
        return text;
      })
  );

  if (on("explain_query")) server.tool(
    "explain_query",
    "Show query execution plan without running the query",
    {
      sql: z.string().optional().describe("SQL query to explain"),
      query: z.string().optional().describe("Alias for sql"),
    },
    ({ sql, query }) => {
      const statement = sql ?? query;
      if (!statement) return Promise.resolve(missingSqlArg());
      // `explain` runs no policy-changing statement, so a passing query is logged
      // by the driver layer (via introspect), not as a gated tool call. A blocked
      // query is still recorded so the audit log shows the denial.
      const verdict = evaluate(statement, policy, dialect);
      if (!verdict.ok) {
        logQuery(conn.id, conn.name, statement, "blocked", verdict.categories, verdict.reason ?? undefined, undefined, undefined, undefined, conn.viaGroup);
        return Promise.resolve(err(`Blocked: ${verdict.reason}`));
      }
      return introspect("explain_query", async (driver) => JSON.stringify(await driver.explain(statement), null, 2));
    }
  );

  if (on("describe_table")) server.tool(
    "describe_table",
    "Get column definitions for a table",
    { table: z.string().describe("Table name") },
    ({ table }) =>
      introspect("describe_table", async (driver) => JSON.stringify(await driver.describeTable(table), null, 2))
  );

  if (on("list_relationships")) server.tool(
    "list_relationships",
    "List foreign key relationships between tables",
    { table: z.string().optional().describe("Filter to a specific table (optional)") },
    ({ table }) =>
      introspect("list_relationships", async (driver) => JSON.stringify(await driver.listRelationships(table), null, 2))
  );

  if (on("search_schema")) server.tool(
    "search_schema",
    "Find tables or columns matching a term",
    { term: z.string().describe("Search term (substring match on table or column names)") },
    ({ term }) =>
      introspect("search_schema", async (driver) => JSON.stringify(await driver.searchSchema(term), null, 2))
  );

  if (on("table_stats")) server.tool(
    "table_stats",
    "Get cheap table statistics (estimated rows, size, indexes)",
    { table: z.string().describe("Table name") },
    ({ table }) =>
      introspect("table_stats", async (driver) => JSON.stringify(await driver.tableStats(table), null, 2))
  );

  if (on("list_schemas")) server.tool("list_schemas", "List all schemas or databases", () =>
    introspect("list_schemas", async (driver) => (await driver.listSchemas()).join("\n")));

  if (on("export_query")) server.tool(
    "export_query",
    "Run a SQL query and save results to a local CSV or JSON file",
    {
      sql: z.string().optional().describe("SQL query to execute"),
      query: z.string().optional().describe("Alias for sql"),
      format: z.enum(["csv", "json"]).default("csv").describe("Export file format"),
      ...timeoutOption,
    },
    (args) => {
      const { sql, query, format } = args;
      const timeout = (args as typeof args & { timeout?: number }).timeout;
      const statement = sql ?? query;
      if (!statement) return Promise.resolve(missingSqlArg());
      const verdict = evaluate(statement, policy, dialect);
      const timeoutMs = toTimeoutMs(timeout);
      return runGated(
        conn,
        { category: verdict.categories, action: "export_query", detail: statement },
        async (logId) => {
          const sid = sessionIdRef.value;
          const queryAc = registerQueryAbort(logId, sid);
          try {
            const driver = await getDriver(sid, conn);
            const result = await runStatement<QueryRows>(driver, statement, queryAc.signal, "export_query", timeoutMs);

            const { rows, truncated, limit } = capRows(result.rows, policy.maxRows);
            const maskedRows = mask(rows);

            const fields = result.fields ?? (maskedRows[0] ? Object.keys(maskedRows[0] as object) : []);
            const payload = format === "csv"
              ? toCsv(maskedRows, fields)
              : JSON.stringify(resultWithMeta({ ...result, fields }, maskedRows, truncated, limit), null, 2);
            const filePath = makeExportPath(conn.name, format);
            await Bun.write(filePath, payload);

            return {
              text: `Exported ${maskedRows.length} rows to ${filePath}`,
              result: { rows: maskedRows, fields: result.fields },
            };
          } finally {
            clearQueryAbort(logId);
          }
        },
        { precheck: () => (verdict.ok ? undefined : verdict.reason ?? "blocked"), ...queryGateOpts },
      );
    }
  );

  if (on("run_saved_query")) server.tool(
    "run_saved_query",
    "Run a saved query by name",
    {
      name: z.string().describe("Name of the saved query"),
      ...timeoutOption,
    },
    (args) => {
      const { name } = args;
      const timeout = (args as typeof args & { timeout?: number }).timeout;
      const saved = getSavedQuery(conn.id, name);
      if (!saved) return Promise.resolve(err(`Saved query "${name}" not found.`));
      return gatedQuery(saved.sql, "run_saved_query", toTimeoutMs(timeout));
    }
  );

  if (on("list_saved_queries")) server.tool(
    "list_saved_queries",
    "List saved queries for this connection",
    async () => {
      try {
        return ok(JSON.stringify(listSavedQueries(conn.id), null, 2));
      } catch (e) {
        return err(`Error: ${(e as Error).message}`);
      }
    }
  );
}

// ── Export helpers ─────────────────────────────────────────────────────────

const EXPORT_DIR = join(homedir(), ".pluk", "exports");

function sanitizeFilename(input: string): string {
  return input.replace(/[^a-zA-Z0-9_-]/g, "_").slice(0, 64);
}

function toCsv(rows: unknown[], fields: string[]): string {
  if (rows.length === 0) return fields.join(",") + "\n";
  const escape = (v: unknown) => {
    const s = v === null || v === undefined ? "" : String(v);
    if (/[",\n\r]/.test(s)) return `"${s.replace(/"/g, '""')}"`;
    return s;
  };
  const lines = [fields.join(",")];
  for (const row of rows) {
    const r = row as Record<string, unknown>;
    lines.push(fields.map((f) => escape(r[f])).join(","));
  }
  return lines.join("\n") + "\n";
}

function makeExportPath(connName: string, format: string): string {
  mkdirSync(EXPORT_DIR, { recursive: true });
  const ts = new Date().toISOString().replace(/[:.]/g, "-");
  return join(EXPORT_DIR, `${sanitizeFilename(connName)}_${ts}.${format}`);
}
