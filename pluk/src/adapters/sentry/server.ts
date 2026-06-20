import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import type { Integration } from "../../store/integrations.js";
import { parseActionPolicy, actionAllowed, actionPolicyDescription, type ActionCategory } from "../../mcp/actionPolicy.js";
import { createLogEntry, updateLogEntry, logQuery } from "../../store/queryLog.js";
import { sentryConfig, sentryRequest } from "./client.js";
import { logError } from "../../log.js";
import type { ToolHost } from "../../mcp/namespace.js";

// MCP server for the Sentry adapter. Read tools cover projects, issues, and the
// latest event (stacktrace); the one write tool resolves/ignores an issue. All
// gated by the integration's action policy and recorded in the activity log.
export function buildSentryServer(conn: Integration, sessionIdRef: { value: string }): McpServer {
  const server = new McpServer({ name: conn.name, version: "1.0.0" });
  registerSentryServer(server, conn, sessionIdRef);
  return server;
}

export function registerSentryServer(server: ToolHost, conn: Integration, _sessionIdRef: { value: string }): void {
  const cfg = sentryConfig(conn);
  const policy = parseActionPolicy(conn.query_policy, conn.read_only);

  type ToolResult = { content: { type: "text"; text: string }[]; isError?: boolean };

  async function run(
    category: ActionCategory,
    action: string,
    detail: string,
    fn: () => Promise<unknown>,
  ): Promise<ToolResult> {
    if (!actionAllowed(policy, category)) {
      const reason = `Action "${action}" needs "${category}" permission; this integration allows: ${policy.allowed.join(", ")}.`;
      logQuery(conn.id, conn.name, detail, "blocked", category, reason, undefined, action);
      return { content: [{ type: "text", text: `Blocked: ${reason}` }], isError: true };
    }
    const logId = createLogEntry(conn.id, conn.name, detail, "pending", category, undefined, action);
    try {
      const data = await fn();
      const rows = Array.isArray(data) ? data : [data];
      updateLogEntry(logId, "allowed", undefined, { rows });
      return { content: [{ type: "text", text: JSON.stringify(data, null, 2) }] };
    } catch (err) {
      const msg = (err as Error).message;
      updateLogEntry(logId, "error", msg);
      logError(`sentry ${action} failed`, err, { integration: conn.name });
      return { content: [{ type: "text", text: `Error: ${msg}` }], isError: true };
    }
  }

  server.tool(
    "list_projects",
    `List projects in the organization (slug, name, platform). ${actionPolicyDescription(policy)}`,
    async () =>
      run("read", "list_projects", "list_projects", () =>
        sentryRequest(cfg, "GET", `/organizations/${cfg.org}/projects/`)),
  );

  server.tool(
    "list_issues",
    "List issues, newest first. Scoped to the default project if set, else all projects.",
    {
      query: z.string().optional().describe('Sentry search query, e.g. "is:unresolved level:error"'),
      project: z.string().optional().describe("Project slug. Defaults to the integration's project if set."),
      period: z.string().default("14d").describe("Stats period, e.g. 24h, 14d, 90d"),
      limit: z.number().int().min(1).max(100).default(25).describe("Max issues to return"),
    },
    async ({ query, project, period, limit }) => {
      const proj = project ?? cfg.project;
      const detail = `list_issues project=${proj ?? "*"} query="${query ?? ""}" period=${period} limit=${limit}`;
      return run("read", "list_issues", detail, async () => {
        const issues = proj
          ? await sentryRequest<unknown[]>(cfg, "GET", `/projects/${cfg.org}/${proj}/issues/`, { query, statsPeriod: period })
          : await sentryRequest<unknown[]>(cfg, "GET", `/organizations/${cfg.org}/issues/`, { query, statsPeriod: period, project: "-1" });
        return Array.isArray(issues) ? issues.slice(0, limit) : issues;
      });
    },
  );

  server.tool(
    "get_issue",
    "Get a single issue by its id or short id (e.g. BACKEND-1A)",
    { id: z.string().describe("Issue id (numeric) or short id") },
    async ({ id }) =>
      run("read", "get_issue", `get_issue ${id}`, () =>
        sentryRequest(cfg, "GET", `/organizations/${cfg.org}/issues/${encodeURIComponent(id)}/`)),
  );

  server.tool(
    "latest_event",
    "Get the latest event for an issue, including the stacktrace and tags",
    { id: z.string().describe("Issue id (numeric) or short id") },
    async ({ id }) =>
      run("read", "latest_event", `latest_event ${id}`, () =>
        sentryRequest(cfg, "GET", `/issues/${encodeURIComponent(id)}/events/latest/`)),
  );

  server.tool(
    "update_issue",
    "Resolve, ignore, or reopen an issue (write).",
    {
      id: z.string().describe("Issue id (numeric) or short id"),
      status: z.enum(["resolved", "ignored", "unresolved"]).describe("New status"),
    },
    async ({ id, status }) =>
      run("write", "update_issue", `update_issue ${id} -> ${status}`, () =>
        sentryRequest(cfg, "PUT", `/organizations/${cfg.org}/issues/${encodeURIComponent(id)}/`, undefined, { status })),
  );
}
