import { z } from "zod";
import type { Integration } from "../../store/integrations.js";
import { actionAdapter, type ActionTool } from "../kit.js";
import { linearGraphQL } from "./client.js";
import { linearFields } from "./fields.js";

const AGENT_HINT = "Use this for Linear issue tracking — start with my_issues for the work assigned to you, or list_issues / search_issues / list_projects to look wider. Read a thread with list_comments and check who replied to you with inbox. Check project progress and issue counts with list_projects and a project's status-update log with project_updates. Create issues, comment or reply, move an issue with update_issue, and attach a pull request with link_url when write is permitted. Read before writing.";

const ISSUE_FIELDS = `id identifier title state { name } assignee { name } priority url updatedAt`;
const PROJECT_FIELDS = `id name state progress startDate targetDate url lead { name } issueCountHistory completedIssueCountHistory`;
const COMMENT_FIELDS = `id body url createdAt parentId resolvedAt user { name } botActor { name }`;
// Notification is an interface; the issue/comment context an agent actually needs
// only exists on IssueNotification, so it comes in through an inline fragment.
const NOTIFICATION_FIELDS = `id type title subtitle url createdAt readAt actor { name }
  ... on IssueNotification { issue { identifier title url } comment { body url } parentComment { id url } }`;

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

// Linear returns an issue's comments as one flat list — a reply is just a comment
// carrying `parentId`. Rebuild the threads so a reply reads next to the comment it
// answers instead of leaving the agent to reassemble the tree from ids.
export function threadComments(nodes: Record<string, unknown>[]): Record<string, unknown>[] {
  const byId = new Map<string, Record<string, unknown>>();
  // `parentId` is dropped on the way in: once a reply sits inside its parent's
  // `replies`, the id is redundant noise in the agent's payload.
  for (const n of nodes) {
    const { parentId, ...rest } = n;
    byId.set(String(n.id), { ...rest, replies: [] });
  }

  const roots: Record<string, unknown>[] = [];
  for (const n of nodes) {
    const node = byId.get(String(n.id));
    if (!node) continue;
    const parent = n.parentId ? byId.get(String(n.parentId)) : undefined;
    if (parent) (parent.replies as unknown[]).push(node);
    else roots.push(node);
  }
  return roots;
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
      name: "my_issues",
      description: "List the issues assigned to you — the starting point for \"what am I working on\". Open issues only unless include_done is set.",
      category: "read",
      schema: {
        include_done: z.boolean().default(false).describe("Also return completed and canceled issues"),
        limit: z.number().int().min(1).max(100).default(25).describe("Max issues to return"),
      },
      detail: (a) => `my_issues include_done=${a.include_done} limit=${a.limit}`,
      run: async (a) => {
        const filter: Record<string, unknown> = { assignee: { isMe: { eq: true } } };
        if (!a.include_done) filter.state = { type: { nin: ["completed", "canceled"] } };
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
          `query($id:String!){ issue(id:$id){ id identifier title description state { name type } assignee { name } priority estimate dueDate branchName url createdAt updatedAt team { key } project { name } parent { identifier title } labels { nodes { name } } } }`,
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
      name: "list_comments",
      description: "Read an issue's comment thread, oldest first, with replies nested under the comment they answer. Use this to see the discussion and whether anyone responded.",
      category: "read",
      schema: {
        issue_id: z.string().describe("Issue id or identifier (e.g. ENG-123)"),
        limit: z.number().int().min(1).max(100).default(50).describe("Max comments to fetch; replies count towards this"),
      },
      detail: (a) => `list_comments ${a.issue_id} limit=${a.limit}`,
      run: async (a) => {
        const data = await linearGraphQL<{ issue: { identifier: string; comments: { nodes: Record<string, unknown>[] } } | null }>(
          apiKey,
          `query($id:String!,$first:Int!){ issue(id:$id){ identifier comments(first:$first, orderBy:createdAt){ nodes { ${COMMENT_FIELDS} } } } }`,
          { id: a.issue_id, first: a.limit },
        );
        if (!data.issue) throw new Error(`Issue "${a.issue_id}" not found.`);
        return { issue: data.issue.identifier, comments: threadComments(data.issue.comments.nodes) };
      },
    },
    {
      name: "inbox",
      description: "Read your Linear notifications — replies to your comments, mentions, assignments and status changes, newest first. Unread only by default. Use this to find out who responded to you and where.",
      category: "read",
      schema: {
        unread_only: z.boolean().default(true).describe("Only notifications you have not read yet"),
        limit: z.number().int().min(1).max(50).default(25).describe("Max notifications to return"),
      },
      detail: (a) => `inbox unread_only=${a.unread_only} limit=${a.limit}`,
      run: async (a) => {
        const limit = a.limit as number;
        // NotificationFilter has no read/unread field, so the unread cut happens
        // here. Over-fetch first, otherwise a page of already-read notifications
        // would return far fewer than the caller asked for.
        const first = a.unread_only ? Math.min(limit * 4, 200) : limit;
        const data = await linearGraphQL<{ notifications: { nodes: Record<string, unknown>[] } }>(
          apiKey,
          `query($first:Int!){ notifications(first:$first){ nodes { ${NOTIFICATION_FIELDS} } } }`,
          { first },
        );
        const nodes = a.unread_only ? data.notifications.nodes.filter((n) => n.readAt == null) : data.notifications.nodes;
        return nodes.slice(0, limit);
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
      name: "list_states",
      description: "List a team's workflow states (id, name, type). Use a state id with update_issue to move an issue.",
      category: "read",
      defaultEnabled: false,
      schema: {
        team: z.string().optional().describe("Team key (e.g. ENG). Defaults to the integration's team if set."),
      },
      detail: (a) => `list_states team=${(a.team as string) ?? defaultTeam ?? "*"}`,
      run: async (a) => {
        const teamKey = (a.team as string | undefined) ?? defaultTeam;
        const filter = teamKey ? { team: { key: { eq: teamKey } } } : undefined;
        const data = await linearGraphQL<{ workflowStates: { nodes: unknown[] } }>(
          apiKey,
          `query($filter:WorkflowStateFilter){ workflowStates(filter:$filter){ nodes { id name type position team { key } } } }`,
          { filter },
        );
        return data.workflowStates.nodes;
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
      description: "Add a comment to an issue, or reply in a thread by passing the comment id you are answering as parent_id. Get comment ids from list_comments.",
      category: "write",
      schema: {
        issue_id: z.string().describe("Issue id or identifier"),
        body: z.string().describe("Comment body (markdown)"),
        parent_id: z.string().optional().describe("Comment id to reply to; omit to start a new thread"),
      },
      detail: (a) => (a.parent_id ? `comment ${a.issue_id} reply-to=${a.parent_id}` : `comment ${a.issue_id}`),
      run: async (a) => {
        const data = await linearGraphQL<{ commentCreate: { success: boolean; comment: unknown } }>(
          apiKey,
          `mutation($input:CommentCreateInput!){ commentCreate(input:$input){ success comment { id url parentId } } }`,
          { input: { issueId: a.issue_id, body: a.body, parentId: a.parent_id } },
        );
        return data.commentCreate;
      },
    },
    {
      name: "update_issue",
      description: "Update an issue — move it to another state, reassign it, or change priority, estimate, title or description. Get state ids from list_states.",
      category: "write",
      schema: {
        id: z.string().describe("Issue id or identifier (e.g. ENG-123)"),
        state_id: z.string().optional().describe("Workflow state id from list_states"),
        assignee_id: z.string().optional().describe("User id to assign the issue to"),
        priority: z.number().int().min(0).max(4).optional().describe("0 none, 1 urgent, 2 high, 3 normal, 4 low"),
        estimate: z.number().int().min(0).optional().describe("Estimate points"),
        title: z.string().optional().describe("New title"),
        description: z.string().optional().describe("New description (markdown); replaces the existing one"),
      },
      detail: (a) => `update_issue ${a.id} ${Object.keys(a).filter((k) => k !== "id" && a[k] !== undefined).join(",")}`,
      run: async (a) => {
        const input: Record<string, unknown> = {};
        if (a.state_id !== undefined) input.stateId = a.state_id;
        if (a.assignee_id !== undefined) input.assigneeId = a.assignee_id;
        if (a.priority !== undefined) input.priority = a.priority;
        if (a.estimate !== undefined) input.estimate = a.estimate;
        if (a.title !== undefined) input.title = a.title;
        if (a.description !== undefined) input.description = a.description;
        if (!Object.keys(input).length) throw new Error("update_issue needs at least one field to change.");
        const data = await linearGraphQL<{ issueUpdate: { success: boolean; issue: unknown } }>(
          apiKey,
          `mutation($id:String!,$input:IssueUpdateInput!){ issueUpdate(id:$id, input:$input){ success issue { identifier title state { name } assignee { name } priority url } } }`,
          { id: a.id, input },
        );
        return data.issueUpdate;
      },
    },
    {
      name: "link_url",
      description: "Attach a URL to an issue — a pull request, build, or doc. URLs from a configured integration (GitHub, GitLab, Slack) become rich attachments that sync status back to the issue.",
      category: "write",
      schema: {
        issue_id: z.string().describe("Issue id or identifier"),
        url: z.url().describe("URL to attach"),
        title: z.string().optional().describe("Link title shown on the issue"),
      },
      detail: (a) => `link_url ${a.issue_id} ${a.url}`,
      run: async (a) => {
        const data = await linearGraphQL<{ attachmentLinkURL: { success: boolean; attachment: unknown } }>(
          apiKey,
          `mutation($issueId:String!,$url:String!,$title:String){ attachmentLinkURL(issueId:$issueId, url:$url, title:$title){ success attachment { id title url } } }`,
          { issueId: a.issue_id, url: a.url, title: a.title },
        );
        return data.attachmentLinkURL;
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
