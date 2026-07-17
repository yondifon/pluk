import { z } from "zod";
import type { Integration } from "../../store/integrations.js";
import { actionAdapter, type ActionTool } from "../kit.js";
import { linearGraphQL } from "./client.js";
import { linearFields } from "./fields.js";

const AGENT_HINT = "Use this for Linear issue tracking — list, search and read issues and teams, check project progress and issue counts with list_projects, read a project's status-update log with project_updates, and create issues or comment when write is permitted. Start with list_issues, search_issues, or list_projects before writing.";

const ISSUE_FIELDS = `id identifier title state { name } assignee { name } priority url updatedAt`;
const PROJECT_FIELDS = `id name state progress startDate targetDate url lead { name } issueCountHistory completedIssueCountHistory`;

// Linear reports a project's issue counts as a running history; the last sample
// is the current total/completed. Flatten that (and drop the noisy arrays) into
// a count summary the agent can read directly.
function summarizeProject(p: Record<string, unknown>): Record<string, unknown> {
  const { issueCountHistory, completedIssueCountHistory, progress, ...rest } = p;
  const last = (a: unknown): number => (Array.isArray(a) && a.length ? Number(a[a.length - 1]) : 0);
  const total = last(issueCountHistory);
  const completed = last(completedIssueCountHistory);
  return {
    ...rest,
    total_issues: total,
    completed_issues: completed,
    progress_percent: Math.round(Number(progress ?? 0) * 100),
  };
}

// Linear's tools. `apiKey`/`defaultTeam` are captured from the connection; each
// tool just declares its policy category, log line, and GraphQL call — gating,
// logging, and response shaping are handled by actionAdapter.
function linearTools(apiKey: string, defaultTeam: string | undefined): ActionTool[] {
  return [
    {
      name: "list_issues",
      description: "List issues, optionally scoped to a team.",
      category: "read",
      schema: {
        team: z.string().optional().describe("Team key (e.g. ENG). Defaults to the integration's team if set."),
        limit: z.number().int().min(1).max(100).default(25).describe("Max issues to return"),
      },
      detail: (a) => `list_issues team=${(a.team as string) ?? defaultTeam ?? "*"} limit=${a.limit}`,
      run: async (a) => {
        const teamKey = (a.team as string | undefined) ?? defaultTeam;
        const filter = teamKey ? { team: { key: { eq: teamKey } } } : undefined;
        const data = await linearGraphQL<{ issues: { nodes: unknown[] } }>(
          apiKey,
          `query($first:Int!,$filter:IssueFilter){ issues(first:$first, filter:$filter){ nodes { ${ISSUE_FIELDS} } } }`,
          { first: a.limit, filter },
        );
        return data.issues.nodes;
      },
    },
    {
      name: "get_issue",
      description: "Get a single issue by its id or identifier (e.g. ENG-123)",
      category: "read",
      schema: { id: z.string().describe("Issue id or identifier") },
      detail: (a) => `get_issue ${a.id}`,
      run: async (a) => {
        const data = await linearGraphQL<{ issue: unknown }>(
          apiKey,
          `query($id:String!){ issue(id:$id){ id identifier title description state { name } assignee { name } priority url createdAt updatedAt } }`,
          { id: a.id },
        );
        return data.issue;
      },
    },
    {
      name: "search_issues",
      description: "Search issues by text in title or description",
      category: "read",
      schema: {
        query: z.string().describe("Search term"),
        limit: z.number().int().min(1).max(100).default(25).describe("Max issues to return"),
      },
      detail: (a) => `search_issues "${a.query}" limit=${a.limit}`,
      run: async (a) => {
        const filter = { or: [{ title: { containsIgnoreCase: a.query } }, { description: { containsIgnoreCase: a.query } }] };
        const data = await linearGraphQL<{ issues: { nodes: unknown[] } }>(
          apiKey,
          `query($first:Int!,$filter:IssueFilter){ issues(first:$first, filter:$filter){ nodes { ${ISSUE_FIELDS} } } }`,
          { first: a.limit, filter },
        );
        return data.issues.nodes;
      },
    },
    {
      name: "list_teams",
      description: "List teams (id, name, key). Use a team id to create issues.",
      category: "read",
      defaultEnabled: false,
      run: async () => {
        const data = await linearGraphQL<{ teams: { nodes: unknown[] } }>(apiKey, `{ teams { nodes { id name key } } }`);
        return data.teams.nodes;
      },
    },
    {
      name: "list_projects",
      description: "List projects with their state, progress percent, and issue counts (total/completed). Optionally filter by name.",
      category: "read",
      defaultEnabled: false,
      schema: {
        query: z.string().optional().describe("Filter projects whose name contains this text"),
        limit: z.number().int().min(1).max(100).default(25).describe("Max projects to return"),
      },
      detail: (a) => `list_projects query="${(a.query as string) ?? ""}" limit=${a.limit}`,
      run: async (a) => {
        const q = a.query as string | undefined;
        const filter = q ? { name: { containsIgnoreCase: q } } : undefined;
        const data = await linearGraphQL<{ projects: { nodes: Record<string, unknown>[] } }>(
          apiKey,
          `query($first:Int!,$filter:ProjectFilter){ projects(first:$first, filter:$filter){ nodes { ${PROJECT_FIELDS} } } }`,
          { first: a.limit, filter },
        );
        return data.projects.nodes.map(summarizeProject);
      },
    },
    {
      name: "project_updates",
      description: "Read a project's status-update log (the periodic updates with health on-track/at-risk/off-track), newest first. Use list_projects to find the project id.",
      category: "read",
      defaultEnabled: false,
      schema: {
        project_id: z.string().describe("Project id (UUID) from list_projects"),
        limit: z.number().int().min(1).max(50).default(10).describe("Max updates to return"),
      },
      detail: (a) => `project_updates ${a.project_id} limit=${a.limit}`,
      run: async (a) => {
        const data = await linearGraphQL<{ project: { name: string; projectUpdates: { nodes: unknown[] } } | null }>(
          apiKey,
          `query($id:String!,$first:Int!){ project(id:$id){ name projectUpdates(first:$first){ nodes { body health createdAt url user { name } } } } }`,
          { id: a.project_id, first: a.limit },
        );
        if (!data.project) throw new Error(`Project "${a.project_id}" not found.`);
        return { project: data.project.name, updates: data.project.projectUpdates.nodes };
      },
    },
    {
      name: "create_issue",
      description: "Create a new issue. Needs a team id (see list_teams).",
      category: "write",
      schema: {
        team_id: z.string().describe("Team id (UUID) from list_teams"),
        title: z.string().describe("Issue title"),
        description: z.string().optional().describe("Issue description (markdown)"),
      },
      detail: (a) => `create_issue team=${a.team_id} "${a.title}"`,
      run: async (a) => {
        const data = await linearGraphQL<{ issueCreate: { success: boolean; issue: unknown } }>(
          apiKey,
          `mutation($input:IssueCreateInput!){ issueCreate(input:$input){ success issue { id identifier title url } } }`,
          { input: { teamId: a.team_id, title: a.title, description: a.description } },
        );
        return data.issueCreate;
      },
    },
    {
      name: "comment",
      description: "Add a comment to an issue",
      category: "write",
      schema: {
        issue_id: z.string().describe("Issue id or identifier"),
        body: z.string().describe("Comment body (markdown)"),
      },
      detail: (a) => `comment ${a.issue_id}`,
      run: async (a) => {
        const data = await linearGraphQL<{ commentCreate: { success: boolean; comment: unknown } }>(
          apiKey,
          `mutation($input:CommentCreateInput!){ commentCreate(input:$input){ success comment { id url } } }`,
          { input: { issueId: a.issue_id, body: a.body } },
        );
        return data.commentCreate;
      },
    },
  ];
}

export const linearAdapter = actionAdapter<{ apiKey: string; defaultTeam: string | undefined }>({
  id: "linear",
  label: "Linear",
  category: "issue-tracker",
  agentHint: AGENT_HINT,
  access:
    "Read and search Linear issues; create or update them when write is permitted. Every action is policy-checked and recorded in the activity log.",
  configFields: linearFields,
  client: (conn) => ({
    apiKey: String(conn.config.api_key ?? ""),
    defaultTeam: conn.config.team_key ? String(conn.config.team_key) : undefined,
  }),
  async testConnection(conn: Integration): Promise<void> {
    const apiKey = String(conn.config.api_key ?? "");
    await linearGraphQL<{ viewer: { id: string } }>(apiKey, `{ viewer { id name } }`);
  },
  tools: (_conn, client) => linearTools(client.apiKey, client.defaultTeam),
});
