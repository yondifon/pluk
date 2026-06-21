import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { mkdirSync } from "fs";
import { homedir } from "os";
import { join } from "path";
import type { Integration } from "../../store/integrations.js";
import type { Driver } from "../../db/index.js";
import { parsePolicy, evaluate, capRows, dialectFor, policyDescription, parsePostgresCost } from "../../mcp/policy.js";
import { createLogEntry, updateLogEntry, logQuery } from "../../store/queryLog.js";
import { listSavedQueries, getSavedQuery } from "../../store/savedQueries.js";
import { listMaskedColumns, maskRow } from "../../store/maskedColumns.js";
import {
  getDriver,
  evictDriver,
  withToolTimeout,
  withCancellable,
  registerQueryAbort,
  clearQueryAbort,
} from "../../mcp/pool.js";
import { logError } from "../../log.js";
import { buildInstructions } from "../../mcp/instructions.js";
import type { ToolHost } from "../../mcp/namespace.js";

// MCP server for the SQL database adapter family (Postgres, MySQL, SQLite).
// Tools, resources, and prompts for query + schema introspection, all gated by
// the per-integration query policy and recorded in the activity log.
export function buildSqlServer(conn: Integration, sessionIdRef: { value: string }): McpServer {
  const server = new McpServer(
    { name: conn.name, version: "1.0.0" },
    { instructions: sqlInstructions(conn) },
  );
  registerSqlServer(server, conn, sessionIdRef);
  return server;
}

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
  return type === "sqlite"
    ? "Use SELECT with LIMIT before wider queries."
    : "Use SELECT with LIMIT for production data.";
}

// Live, per-session guidance handed to connecting agents (see instructions.ts).
// Reflects the current query policy, so a read-only DB and an unrestricted one
// announce different constraints.
export function sqlInstructions(conn: Integration): string {
  const policy = parsePolicy(conn.query_policy, conn.read_only);
  return buildInstructions(conn, {
    kind: sqlLabel(conn.type),
    access: "Query and inspect this database. Every statement is checked against the policy below and recorded in the activity log.",
    policy: policyDescription(policy),
    start: "Start with list_tables and describe_table to learn the schema, then read with SELECT … LIMIT.",
    hint: sqlAgentHint(conn.type),
  });
}

// Register the SQL surface onto a host (a bare McpServer for a single endpoint,
// or a namespaced host when aggregated into a group).
export function registerSqlServer(server: ToolHost, conn: Integration, sessionIdRef: { value: string }): void {
  const policy = parsePolicy(conn.query_policy, conn.read_only);
  const dialect = dialectFor(conn.type);
  const policyDesc = policyDescription(policy);

  const readOnlyMode = policy.allowed.length === 2 && policy.allowed.includes("select") && policy.allowed.includes("inspect");
  const maskedColumns = listMaskedColumns(conn.id);

  type ToolText = { content: { type: "text"; text: string }[]; isError?: boolean };

  // Read-only introspection tools share one shape: acquire the pooled driver,
  // run under the tool timeout, evict on failure. `fn` produces the response text.
  async function introspect(label: string, fn: (driver: Driver) => Promise<string>): Promise<ToolText> {
    const sid = sessionIdRef.value;
    try {
      const driver = await getDriver(sid, conn);
      return await withToolTimeout((async (): Promise<ToolText> => {
        return { content: [{ type: "text", text: await fn(driver) }] };
      })(), label);
    } catch (err) {
      evictDriver(sid, conn.id);
      logError(`tool ${label} failed`, err, { integration: conn.name, type: conn.type });
      return { content: [{ type: "text", text: `Error: ${(err as Error).message}` }], isError: true };
    }
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

  server.tool(
    "query",
    `Run a SQL query against the database. ${policyDesc}`,
    { sql: z.string().describe("SQL query to execute") },
    async ({ sql }) => {
      // Policy check (parse-driven)
      const verdict = evaluate(sql, policy, dialect);
      if (!verdict.ok) {
        logQuery(conn.id, conn.name, sql, "blocked", verdict.categories, verdict.reason ?? undefined);
        return { content: [{ type: "text", text: `Blocked: ${verdict.reason}` }], isError: true };
      }

      const sid = sessionIdRef.value;

      // Create pending log entry before executing so it's visible immediately
      const logId = createLogEntry(conn.id, conn.name, sql, "pending", verdict.categories, undefined, "query");

      // Per-query abort controller; also tripped by a session-wide abort.
      const queryAc = registerQueryAbort(logId, sid);

      try {
        const costGateEnabled = policy.maxEstimatedRows !== null || policy.maxEstimatedCost !== null;
        const driver = await getDriver(sid, conn);

        if (costGateEnabled && conn.type === "postgres") {
          const explain = await driver.explain(sql);
          const plan = Array.isArray(explain.rows[0]) ? explain.rows[0][0] : explain.rows[0];
          const estimate = parsePostgresCost(plan);
          if (
            (policy.maxEstimatedRows !== null && estimate.rows !== null && estimate.rows > policy.maxEstimatedRows) ||
            (policy.maxEstimatedCost !== null && estimate.cost !== null && estimate.cost > policy.maxEstimatedCost)
          ) {
            const reason = `Query cost gate exceeded (estimated rows: ${estimate.rows ?? "?"}, cost: ${estimate.cost ?? "?"}).`;
            updateLogEntry(logId, "blocked", reason);
            return { content: [{ type: "text", text: `Blocked: ${reason}` }], isError: true };
          }
        }

        const useReadOnly = readOnlyMode && conn.type === "postgres";
        const work = useReadOnly ? driver.queryReadOnly(sql) : driver.query(sql);

        const result = await withToolTimeout(
          withCancellable(work, queryAc.signal),
          "query"
        );

        const { rows, truncated, limit } = capRows(result.rows, policy.maxRows);
        const maskedRows = maskedColumns.length > 0 ? rows.map(r => maskRow(r as Record<string, unknown>, maskedColumns)) : rows;
        const maskedResult = { ...result, rows: maskedRows };
        updateLogEntry(logId, "allowed", undefined, maskedResult);

        let text = JSON.stringify(maskedResult, null, 2);
        if (truncated) {
          text += `\n\n[Row limit: showing first ${limit} of ${result.rows.length} rows. Add a LIMIT clause to see all results.]`;
        }
        return { content: [{ type: "text", text }] };
      } catch (err) {
        const msg = (err as Error).message;
        const isCancelled = msg.includes("cancelled");
        updateLogEntry(logId, isCancelled ? "cancelled" : "error", msg);
        if (!isCancelled) {
          evictDriver(sid, conn.id);
          logError("query tool failed", err, { integration: conn.name, type: conn.type });
        }
        return { content: [{ type: "text", text: `${isCancelled ? "Cancelled" : "Error"}: ${msg}` }], isError: true };
      } finally {
        clearQueryAbort(logId);
      }
    }
  );

  server.tool("list_tables", "List all tables in the database", () =>
    introspect("list_tables", async (driver) => (await driver.listTables()).join("\n")));

  server.tool(
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
        const { rows, truncated } = capRows(result.rows, policy.maxRows);
        const maskedRows = maskedColumns.length > 0 ? rows.map(r => maskRow(r as Record<string, unknown>, maskedColumns)) : rows;
        let text = JSON.stringify({ fields: result.fields ?? [], rows: maskedRows }, null, 2);
        if (truncated) {
          text += `\n\n[Row limit: showing first ${policy.maxRows} of ${result.rows.length} rows.]`;
        }
        return text;
      })
  );

  server.tool(
    "explain_query",
    "Show query execution plan without running the query",
    {
      sql: z.string().describe("SQL query to explain"),
    },
    async ({ sql }) => {
      const verdict = evaluate(sql, policy, dialect);
      if (!verdict.ok) {
        logQuery(conn.id, conn.name, sql, "blocked", verdict.categories, verdict.reason ?? undefined);
        return { content: [{ type: "text", text: `Blocked: ${verdict.reason}` }], isError: true };
      }

      return introspect("explain_query", async (driver) => JSON.stringify(await driver.explain(sql), null, 2));
    }
  );

  server.tool(
    "describe_table",
    "Get column definitions for a table",
    { table: z.string().describe("Table name") },
    ({ table }) =>
      introspect("describe_table", async (driver) => JSON.stringify(await driver.describeTable(table), null, 2))
  );

  server.tool(
    "list_relationships",
    "List foreign key relationships between tables",
    { table: z.string().optional().describe("Filter to a specific table (optional)") },
    ({ table }) =>
      introspect("list_relationships", async (driver) => JSON.stringify(await driver.listRelationships(table), null, 2))
  );

  server.tool(
    "search_schema",
    "Find tables or columns matching a term",
    { term: z.string().describe("Search term (substring match on table or column names)") },
    ({ term }) =>
      introspect("search_schema", async (driver) => JSON.stringify(await driver.searchSchema(term), null, 2))
  );

  server.tool(
    "table_stats",
    "Get cheap table statistics (estimated rows, size, indexes)",
    { table: z.string().describe("Table name") },
    ({ table }) =>
      introspect("table_stats", async (driver) => JSON.stringify(await driver.tableStats(table), null, 2))
  );

  server.tool("list_schemas", "List all schemas or databases", () =>
    introspect("list_schemas", async (driver) => (await driver.listSchemas()).join("\n")));

  server.tool(
    "export_query",
    "Run a SQL query and save results to a local CSV or JSON file",
    {
      sql: z.string().describe("SQL query to execute"),
      format: z.enum(["csv", "json"]).default("csv").describe("Export file format"),
    },
    async ({ sql, format }) => {
      const verdict = evaluate(sql, policy, dialect);
      if (!verdict.ok) {
        logQuery(conn.id, conn.name, sql, "blocked", verdict.categories, verdict.reason ?? undefined);
        return { content: [{ type: "text", text: `Blocked: ${verdict.reason}` }], isError: true };
      }

      const sid = sessionIdRef.value;
      const logId = createLogEntry(conn.id, conn.name, sql, "pending", verdict.categories, undefined, "export_query");
      const queryAc = registerQueryAbort(logId, sid);

      try {
        const driver = await getDriver(sid, conn);
        const useReadOnly = readOnlyMode && conn.type === "postgres";
        const result = await withToolTimeout(
          withCancellable(useReadOnly ? driver.queryReadOnly(sql) : driver.query(sql), queryAc.signal),
          "export_query"
        );

        const { rows } = capRows(result.rows, policy.maxRows);
        const maskedRows = maskedColumns.length > 0 ? rows.map(r => maskRow(r as Record<string, unknown>, maskedColumns)) : rows;
        updateLogEntry(logId, "allowed", undefined, { rows: maskedRows, fields: result.fields });

        const fields = result.fields ?? (maskedRows[0] ? Object.keys(maskedRows[0] as object) : []);
        const payload = format === "csv" ? toCsv(maskedRows, fields) : JSON.stringify({ fields, rows: maskedRows }, null, 2);
        const filePath = makeExportPath(conn.name, format);
        await Bun.write(filePath, payload);

        return { content: [{ type: "text", text: `Exported ${maskedRows.length} rows to ${filePath}` }] };
      } catch (err) {
        const msg = (err as Error).message;
        const isCancelled = msg.includes("cancelled");
        updateLogEntry(logId, isCancelled ? "cancelled" : "error", msg);
        if (!isCancelled) {
          evictDriver(sid, conn.id);
          logError("query tool failed", err, { integration: conn.name, type: conn.type });
        }
        return { content: [{ type: "text", text: `${isCancelled ? "Cancelled" : "Error"}: ${msg}` }], isError: true };
      } finally {
        clearQueryAbort(logId);
      }
    }
  );

  server.tool(
    "run_saved_query",
    "Run a saved query by name",
    { name: z.string().describe("Name of the saved query") },
    async ({ name }) => {
      const saved = getSavedQuery(conn.id, name);
      if (!saved) {
        return { content: [{ type: "text", text: `Saved query "${name}" not found.` }], isError: true };
      }

      const verdict = evaluate(saved.sql, policy, dialect);
      if (!verdict.ok) {
        logQuery(conn.id, conn.name, saved.sql, "blocked", verdict.categories, verdict.reason ?? undefined);
        return { content: [{ type: "text", text: `Blocked: ${verdict.reason}` }], isError: true };
      }

      const sid = sessionIdRef.value;
      const logId = createLogEntry(conn.id, conn.name, saved.sql, "pending", verdict.categories, undefined, "run_saved_query");
      const queryAc = registerQueryAbort(logId, sid);

      try {
        const driver = await getDriver(sid, conn);

        if ((policy.maxEstimatedRows !== null || policy.maxEstimatedCost !== null) && conn.type === "postgres") {
          const explain = await driver.explain(saved.sql);
          const plan = Array.isArray(explain.rows[0]) ? explain.rows[0][0] : explain.rows[0];
          const estimate = parsePostgresCost(plan);
          if (
            (policy.maxEstimatedRows !== null && estimate.rows !== null && estimate.rows > policy.maxEstimatedRows) ||
            (policy.maxEstimatedCost !== null && estimate.cost !== null && estimate.cost > policy.maxEstimatedCost)
          ) {
            const reason = `Query cost gate exceeded (estimated rows: ${estimate.rows ?? "?"}, cost: ${estimate.cost ?? "?"}).`;
            updateLogEntry(logId, "blocked", reason);
            return { content: [{ type: "text", text: `Blocked: ${reason}` }], isError: true };
          }
        }

        const useReadOnly = readOnlyMode && conn.type === "postgres";
        const work = useReadOnly ? driver.queryReadOnly(saved.sql) : driver.query(saved.sql);
        const result = await withToolTimeout(withCancellable(work, queryAc.signal), "run_saved_query");

        const { rows, truncated, limit } = capRows(result.rows, policy.maxRows);
        const maskedRows = maskedColumns.length > 0 ? rows.map(r => maskRow(r as Record<string, unknown>, maskedColumns)) : rows;
        const maskedResult = { ...result, rows: maskedRows };
        updateLogEntry(logId, "allowed", undefined, maskedResult);

        let text = JSON.stringify(maskedResult, null, 2);
        if (truncated) {
          text += `\n\n[Row limit: showing first ${limit} of ${result.rows.length} rows. Add a LIMIT clause to see all results.]`;
        }
        return { content: [{ type: "text", text }] };
      } catch (err) {
        const msg = (err as Error).message;
        const isCancelled = msg.includes("cancelled");
        updateLogEntry(logId, isCancelled ? "cancelled" : "error", msg);
        if (!isCancelled) {
          evictDriver(sid, conn.id);
          logError("query tool failed", err, { integration: conn.name, type: conn.type });
        }
        return { content: [{ type: "text", text: `${isCancelled ? "Cancelled" : "Error"}: ${msg}` }], isError: true };
      } finally {
        clearQueryAbort(logId);
      }
    }
  );

  server.tool(
    "list_saved_queries",
    "List saved queries for this connection",
    async () => {
      try {
        const queries = listSavedQueries(conn.id);
        return { content: [{ type: "text", text: JSON.stringify(queries, null, 2) }] };
      } catch (err) {
        return { content: [{ type: "text", text: `Error: ${(err as Error).message}` }], isError: true };
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
