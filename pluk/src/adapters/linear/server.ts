import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import type { Integration } from "../../store/integrations.js";
import { parseActionPolicy, actionAllowed, actionPolicyDescription, type ActionCategory } from "../../mcp/actionPolicy.js";
import { createLogEntry, updateLogEntry, logQuery } from "../../store/queryLog.js";
import { linearGraphQL } from "./client.js";
import { logError } from "../../log.js";

// MCP server for the Linear adapter. Tools are gated by the integration's action
// policy (read/write) and recorded in the shared activity log.
export function buildLinearServer(conn: Integration, _sessionIdRef: { value: string }): McpServer {
  const apiKey = String(conn.config.api_key ?? "");
  const defaultTeam = conn.config.team_key ? String(conn.config.team_key) : undefined;
  const policy = parseActionPolicy(conn.query_policy, conn.read_only);

  const server = new McpServer({ name: conn.name, version: "1.0.0" });

  type ToolResult = { content: { type: "text"; text: string }[]; isError?: boolean };

  // Gate by action category, log the attempt/result, and shape the MCP response.
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
      logError(`linear ${action} failed`, err, { integration: conn.name });
      return { content: [{ type: "text", text: `Error: ${msg}` }], isError: true };
    }
  }

  const ISSUE_FIELDS = `id identifier title state { name } assignee { name } priority url updatedAt`;

  server.tool(
    "list_issues",
    `List issues, optionally scoped to a team. ${actionPolicyDescription(policy)}`,
    {
      team: z.string().optional().describe("Team key (e.g. ENG). Defaults to the integration's team if set."),
      limit: z.number().int().min(1).max(100).default(25).describe("Max issues to return"),
    },
    async ({ team, limit }) => {
      const teamKey = team ?? defaultTeam;
      const filter = teamKey ? { team: { key: { eq: teamKey } } } : undefined;
      return run("read", "list_issues", `list_issues team=${teamKey ?? "*"} limit=${limit}`, async () => {
        const data = await linearGraphQL<{ issues: { nodes: unknown[] } }>(
          apiKey,
          `query($first:Int!,$filter:IssueFilter){ issues(first:$first, filter:$filter){ nodes { ${ISSUE_FIELDS} } } }`,
          { first: limit, filter },
        );
        return data.issues.nodes;
      });
    },
  );

  server.tool(
    "get_issue",
    "Get a single issue by its id or identifier (e.g. ENG-123)",
    { id: z.string().describe("Issue id or identifier") },
    async ({ id }) =>
      run("read", "get_issue", `get_issue ${id}`, async () => {
        const data = await linearGraphQL<{ issue: unknown }>(
          apiKey,
          `query($id:String!){ issue(id:$id){ id identifier title description state { name } assignee { name } priority url createdAt updatedAt } }`,
          { id },
        );
        return data.issue;
      }),
  );

  server.tool(
    "search_issues",
    "Search issues by text in title or description",
    {
      query: z.string().describe("Search term"),
      limit: z.number().int().min(1).max(100).default(25).describe("Max issues to return"),
    },
    async ({ query, limit }) =>
      run("read", "search_issues", `search_issues "${query}" limit=${limit}`, async () => {
        const filter = { or: [{ title: { containsIgnoreCase: query } }, { description: { containsIgnoreCase: query } }] };
        const data = await linearGraphQL<{ issues: { nodes: unknown[] } }>(
          apiKey,
          `query($first:Int!,$filter:IssueFilter){ issues(first:$first, filter:$filter){ nodes { ${ISSUE_FIELDS} } } }`,
          { first: limit, filter },
        );
        return data.issues.nodes;
      }),
  );

  server.tool(
    "list_teams",
    "List teams (id, name, key). Use a team id to create issues.",
    async () =>
      run("read", "list_teams", "list_teams", async () => {
        const data = await linearGraphQL<{ teams: { nodes: unknown[] } }>(
          apiKey,
          `{ teams { nodes { id name key } } }`,
        );
        return data.teams.nodes;
      }),
  );

  server.tool(
    "create_issue",
    "Create a new issue. Needs a team id (see list_teams).",
    {
      team_id: z.string().describe("Team id (UUID) from list_teams"),
      title: z.string().describe("Issue title"),
      description: z.string().optional().describe("Issue description (markdown)"),
    },
    async ({ team_id, title, description }) =>
      run("write", "create_issue", `create_issue team=${team_id} "${title}"`, async () => {
        const data = await linearGraphQL<{ issueCreate: { success: boolean; issue: unknown } }>(
          apiKey,
          `mutation($input:IssueCreateInput!){ issueCreate(input:$input){ success issue { id identifier title url } } }`,
          { input: { teamId: team_id, title, description } },
        );
        return data.issueCreate;
      }),
  );

  server.tool(
    "comment",
    "Add a comment to an issue",
    {
      issue_id: z.string().describe("Issue id or identifier"),
      body: z.string().describe("Comment body (markdown)"),
    },
    async ({ issue_id, body }) =>
      run("write", "comment", `comment ${issue_id}`, async () => {
        const data = await linearGraphQL<{ commentCreate: { success: boolean; comment: unknown } }>(
          apiKey,
          `mutation($input:CommentCreateInput!){ commentCreate(input:$input){ success comment { id url } } }`,
          { input: { issueId: issue_id, body } },
        );
        return data.commentCreate;
      }),
  );

  return server;
}
