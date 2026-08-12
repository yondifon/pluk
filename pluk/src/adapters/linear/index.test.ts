import { afterEach, expect, test } from "bun:test";
import { linearAdapter, linearTools } from "./index.js";
import { resolveLabels, resolveState, resolveTeam, resolveUser } from "./resolve.js";

const realFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = realFetch;
});

interface Call {
  query: string;
  variables: Record<string, unknown>;
}

function mockGraphQL(handler: (call: Call) => { data?: unknown; errors?: { message: string }[] }): Call[] {
  const calls: Call[] = [];
  globalThis.fetch = (async (_input: RequestInfo | URL, init?: RequestInit) => {
    const body = JSON.parse(String(init?.body)) as { query: string; variables?: Record<string, unknown> };
    const call = { query: body.query, variables: body.variables ?? {} };
    calls.push(call);
    return new Response(JSON.stringify(handler(call)), { status: 200 });
  }) as unknown as typeof fetch;
  return calls;
}

function tool(name: string) {
  const t = linearTools("lin_api_x", "ENG").find((t) => t.name === name);
  if (!t) throw new Error(`tool ${name} not found`);
  return t;
}

const TEAMS = { data: { teams: { nodes: [{ id: "team-1", key: "ENG", name: "Engineering" }] } } };
const USERS = { data: { users: { nodes: [{ id: "u-1", name: "Ada Lovelace", email: "ada@acme.com" }] } } };
const STATES = {
  data: { workflowStates: { nodes: [{ id: "st-1", name: "In Progress" }, { id: "st-2", name: "Done" }] } },
};
const LABELS = { data: { issueLabels: { nodes: [{ id: "lb-1", name: "bug" }, { id: "lb-2", name: "api" }] } } };
const CREATE = {
  data: { issueCreate: { success: true, issue: { id: "iss-1", identifier: "ENG-42", title: "T", url: "https://linear.app/acme/issue/ENG-42" } } },
};
const UPDATE = {
  data: { issueUpdate: { success: true, issue: { identifier: "ENG-42", title: "T", state: { name: "Done" }, assignee: { name: "Ada Lovelace" }, priority: 2, url: "https://linear.app/acme/issue/ENG-42" } } },
};

test("create_issue resolves the team and returns the created issue", async () => {
  const calls = mockGraphQL((c) => {
    if (c.query.includes("nodes { id key name }")) return TEAMS;
    if (c.query.includes("issueCreate")) return CREATE;
    throw new Error(`unexpected query: ${c.query}`);
  });
  const out = (await tool("create_issue").run({ team: "ENG", title: "Fix sign-in", only: ["*"] }, {})) as {
    success: boolean;
    issue: { identifier: string; url: string };
  };
  expect(out.success).toBe(true);
  expect(out.issue.identifier).toBe("ENG-42");
  expect(out.issue.url).toContain("linear.app");
  const create = calls.find((c) => c.query.includes("issueCreate"))!;
  expect(create.variables.input).toEqual({ teamId: "team-1", title: "Fix sign-in" });
});

test("create_issue resolves assignee, state, and labels by name, case-insensitively", async () => {
  const calls = mockGraphQL((c) => {
    if (c.query.includes("nodes { id key name }")) return TEAMS;
    if (c.query.includes("users(")) return USERS;
    if (c.query.includes("workflowStates")) return STATES;
    if (c.query.includes("issueLabels")) return LABELS;
    if (c.query.includes("issueCreate")) return CREATE;
    throw new Error(`unexpected query: ${c.query}`);
  });
  await tool("create_issue").run(
    { team: "engineering", title: "Add retries", description: "Retry 3x", assignee: "ADA@acme.com", state: "in progress", priority: 2, labels: ["BUG", "Api"] },
    {},
  );
  const create = calls.find((c) => c.query.includes("issueCreate"))!;
  expect(create.variables.input).toEqual({
    teamId: "team-1",
    title: "Add retries",
    description: "Retry 3x",
    assigneeId: "u-1",
    stateId: "st-1",
    priority: 2,
    labelIds: ["lb-1", "lb-2"],
  });
});

test("update_issue resolves state, assignee, and labels, then updates", async () => {
  const calls = mockGraphQL((c) => {
    if (c.query.includes("issue(id:$id)")) return { data: { issue: { team: { key: "ENG" } } } };
    if (c.query.includes("workflowStates")) return STATES;
    if (c.query.includes("users(")) return USERS;
    if (c.query.includes("issueLabels")) return LABELS;
    if (c.query.includes("issueUpdate")) return UPDATE;
    throw new Error(`unexpected query: ${c.query}`);
  });
  await tool("update_issue").run({ id: "ENG-42", state: "done", assignee: "ada lovelace", title: "Renamed", labels: ["bug"] }, {});
  const update = calls.find((c) => c.query.includes("issueUpdate"))!;
  expect(update.variables.input).toEqual({ stateId: "st-2", assigneeId: "u-1", title: "Renamed", labelIds: ["lb-1"] });
});

test("update_issue unassigns when assignee is null", async () => {
  const calls = mockGraphQL((c) => {
    if (c.query.includes("issueUpdate")) return UPDATE;
    throw new Error(`unexpected query: ${c.query}`);
  });
  await tool("update_issue").run({ id: "ENG-42", assignee: null }, {});
  expect(calls).toHaveLength(1);
  expect(calls[0]!.variables.input).toEqual({ assigneeId: null });
});

test("update_issue rejects when no fields to change are given", async () => {
  const calls = mockGraphQL((c) => {
    throw new Error(`unexpected query: ${c.query}`);
  });
  await expect(tool("update_issue").run({ id: "ENG-42" }, {})).rejects.toThrow(/update_issue needs at least one field/);
  expect(calls).toHaveLength(0);
});

test("update_issue reports a missing issue", async () => {
  const calls = mockGraphQL((c) => {
    if (c.query.includes("issue(id:$id)")) return { data: { issue: null } };
    throw new Error(`unexpected query: ${c.query}`);
  });
  await expect(tool("update_issue").run({ id: "NOPE-1", state: "Done" }, {})).rejects.toThrow(/Issue "NOPE-1" not found/);
  expect(calls).toHaveLength(1);
});

test("create_issue rejects an unknown team and names the known ones", async () => {
  const calls = mockGraphQL((c) => {
    if (c.query.includes("nodes { id key name }")) return TEAMS;
    throw new Error(`unexpected query: ${c.query}`);
  });
  await expect(tool("create_issue").run({ team: "Ops", title: "X" }, {})).rejects.toThrow(
    /No team named "Ops"\. Known teams: ENG \(Engineering\)/,
  );
  expect(calls.some((c) => c.query.includes("issueCreate"))).toBe(false);
});

test("create_issue rejects an ambiguous team name with the candidates", async () => {
  mockGraphQL((c) => {
    if (c.query.includes("nodes { id key name }"))
      return {
        data: {
          teams: { nodes: [{ id: "t1", key: "ENG", name: "Engineering" }, { id: "t2", key: "ENG2", name: "Engineering" }] },
        },
      };
    throw new Error(`unexpected query: ${c.query}`);
  });
  await expect(tool("create_issue").run({ team: "engineering", title: "X" }, {})).rejects.toThrow(
    /matches more than one team: ENG \(Engineering\), ENG2 \(Engineering\)/,
  );
});

test("create_issue surfaces an auth failure from the API", async () => {
  mockGraphQL(() => ({ errors: [{ message: "Authentication required" }] }));
  await expect(tool("create_issue").run({ team: "ENG", title: "X" }, {})).rejects.toThrow(/Linear: Authentication required/);
});

test("resolveTeam matches by key or name, case-insensitively", async () => {
  mockGraphQL(() => TEAMS);
  expect(await resolveTeam("k", "ENG")).toEqual({ id: "team-1", key: "ENG", name: "Engineering" });
  expect(await resolveTeam("k", "engineering")).toEqual({ id: "team-1", key: "ENG", name: "Engineering" });
});

test("resolveUser matches by email or display name, case-insensitively", async () => {
  mockGraphQL(() => USERS);
  expect(await resolveUser("k", "ADA@acme.com")).toEqual({ id: "u-1", name: "Ada Lovelace", email: "ada@acme.com" });
  expect(await resolveUser("k", "ada lovelace")).toEqual({ id: "u-1", name: "Ada Lovelace", email: "ada@acme.com" });
});

test("resolveUser rejects an ambiguous name with the candidates", async () => {
  mockGraphQL(() => ({
    data: {
      users: { nodes: [{ id: "u-1", name: "Ada Lovelace", email: "ada@acme.com" }, { id: "u-2", name: "Ada Lovelace", email: "ada@corp.com" }] },
    },
  }));
  await expect(resolveUser("k", "Ada Lovelace")).rejects.toThrow(
    /matches more than one user: Ada Lovelace <ada@acme.com>, Ada Lovelace <ada@corp.com>/,
  );
});

test("resolveUser reports no match with near matches", async () => {
  mockGraphQL(() => USERS);
  await expect(resolveUser("k", "Ada")).rejects.toThrow(/No user matches "Ada"\. Near matches: Ada Lovelace <ada@acme.com>/);
});

test("resolveState and resolveLabels match by name, case-insensitively", async () => {
  const calls: Call[] = [];
  mockGraphQL((c) => {
    calls.push(c);
    if (c.query.includes("workflowStates")) return STATES;
    if (c.query.includes("issueLabels")) return LABELS;
    throw new Error(`unexpected query: ${c.query}`);
  });
  expect(await resolveState("k", "ENG", "DONE")).toEqual({ id: "st-2", name: "Done" });
  expect(await resolveLabels("k", ["BUG", "api"])).toEqual(["lb-1", "lb-2"]);
  expect(calls[0]!.variables.filter).toEqual({ team: { key: { eq: "ENG" } } });
});

test("resolveState reports an unknown state with the team's states", async () => {
  mockGraphQL(() => STATES);
  await expect(resolveState("k", "ENG", "Shipped")).rejects.toThrow(
    /No workflow state named "Shipped" in team ENG\. States: In Progress, Done/,
  );
});

test("resolveLabels reports an unknown label", async () => {
  mockGraphQL(() => LABELS);
  await expect(resolveLabels("k", ["bug", "urgent"])).rejects.toThrow(/No label named "urgent"\. Existing labels: bug, api/);
});

// ── `only` field selection ───────────────────────────────────────────────────

const ISSUE_NODE = { id: "iss-1", identifier: "ENG-42", title: "Fix sign-in", state: { name: "In Progress" }, assignee: { name: "Ada Lovelace" }, priority: 2, url: "https://linear.app/acme/issue/ENG-42", updatedAt: "2026-08-01" };

test("list_issues defaults to identifier/title/state/assignee/updatedAt and drops id/url/priority", async () => {
  mockGraphQL(() => ({ data: { issues: { nodes: [ISSUE_NODE] } } }));
  const out = (await tool("list_issues").run({ limit: 25 }, {})) as Record<string, unknown>[];
  expect(out).toEqual([{ identifier: "ENG-42", title: "Fix sign-in", state: { name: "In Progress" }, assignee: { name: "Ada Lovelace" }, updatedAt: "2026-08-01" }]);
});

test("list_issues only:['priority','url'] returns just those", async () => {
  mockGraphQL(() => ({ data: { issues: { nodes: [ISSUE_NODE] } } }));
  const out = (await tool("list_issues").run({ limit: 25, only: ["priority", "url"] }, {})) as Record<string, unknown>[];
  expect(out).toEqual([{ priority: 2, url: "https://linear.app/acme/issue/ENG-42" }]);
});

test("get_issue defaults keep description and drop branchName/labels/team", async () => {
  const issue = { id: "iss-1", identifier: "ENG-42", title: "Fix sign-in", description: "Repro steps", state: { name: "In Progress", type: "started" }, assignee: { name: "Ada Lovelace" }, priority: 2, estimate: 3, dueDate: "2026-09-01", branchName: "eng-42-fix-sign-in", url: "https://linear.app/acme/issue/ENG-42", createdAt: "2026-01-01", updatedAt: "2026-08-01", team: { key: "ENG" }, project: { name: "Auth" }, parent: null, labels: { nodes: [{ name: "bug" }] } };
  mockGraphQL(() => ({ data: { issue } }));
  const out = await tool("get_issue").run({ id: "ENG-42" }, {});
  expect(out).toEqual({ identifier: "ENG-42", title: "Fix sign-in", description: "Repro steps", state: { name: "In Progress" }, assignee: { name: "Ada Lovelace" }, priority: 2, url: "https://linear.app/acme/issue/ENG-42" });
});

test("get_issue meta preset returns labels/project/parent/team/timestamps", async () => {
  const issue = { identifier: "ENG-42", labels: { nodes: [{ name: "bug" }] }, project: { name: "Auth" }, parent: null, team: { key: "ENG" }, createdAt: "2026-01-01", updatedAt: "2026-08-01" };
  mockGraphQL(() => ({ data: { issue } }));
  const out = await tool("get_issue").run({ id: "ENG-42", only: ["meta"] }, {});
  expect(out).toEqual({ labels: { nodes: [{ name: "bug" }] }, project: { name: "Auth" }, parent: null, team: { key: "ENG" }, createdAt: "2026-01-01", updatedAt: "2026-08-01" });
});

test("get_issue rejects an unrecognised only field and lists valid fields and presets", async () => {
  mockGraphQL(() => ({ data: { issue: { identifier: "ENG-42" } } }));
  await expect(tool("get_issue").run({ id: "ENG-42", only: ["bogus"] }, {})).rejects.toThrow(
    /Unknown "only" field "bogus"\..*Presets: meta, planning, branch\./,
  );
});

test("list_comments defaults drop id/url/resolvedAt/botActor per comment", async () => {
  mockGraphQL(() => ({
    data: {
      issue: {
        identifier: "ENG-42",
        comments: {
          nodes: [{ id: "c1", body: "hi", url: "https://x", createdAt: "2026-08-01", resolvedAt: null, parentId: null, user: { name: "Ada" }, botActor: null }],
        },
      },
    },
  }));
  const out = await tool("list_comments").run({ issue_id: "ENG-42", limit: 50 }, {});
  expect(out).toEqual({
    issue: "ENG-42",
    comments: [{ body: "hi", createdAt: "2026-08-01", user: { name: "Ada" }, replies: [] }],
  });
});

test("inbox defaults drop the duplicate top-level title/url", async () => {
  mockGraphQL(() => ({
    data: {
      notifications: {
        nodes: [{ id: "n1", type: "issueCommentMention", title: "dup", subtitle: "Ada replied", url: "https://x", createdAt: "2026-08-01", readAt: null, actor: { name: "Ada" }, issue: { identifier: "ENG-42", title: "Fix sign-in", url: "https://y" }, comment: { body: "looks good", url: "https://z" }, parentComment: null }],
      },
    },
  }));
  const out = (await tool("inbox").run({ unread_only: true, limit: 25 }, {})) as Record<string, unknown>[];
  expect(out).toEqual([{ type: "issueCommentMention", subtitle: "Ada replied", createdAt: "2026-08-01", actor: { name: "Ada" }, issue: { identifier: "ENG-42", title: "Fix sign-in" }, comment: { body: "looks good" } }]);
});

test("list_projects defaults drop url/startDate/targetDate/lead", async () => {
  mockGraphQL(() => ({
    data: {
      projects: {
        nodes: [{ id: "p1", name: "Auth", state: "started", url: "https://x", startDate: "2026-01-01", targetDate: "2026-09-01", lead: { name: "Ada" }, issueCountHistory: [1, 2, 5], completedIssueCountHistory: [0, 1, 2], progress: 0.4 }],
      },
    },
  }));
  const out = (await tool("list_projects").run({ limit: 25 }, {})) as Record<string, unknown>[];
  expect(out).toEqual([{ id: "p1", name: "Auth", state: "started", total_issues: 5, completed_issues: 2, progress_percent: 40 }]);
});

test("list_projects dates preset returns startDate/targetDate", async () => {
  mockGraphQL(() => ({
    data: { projects: { nodes: [{ id: "p1", name: "Auth", startDate: "2026-01-01", targetDate: "2026-09-01", issueCountHistory: [], completedIssueCountHistory: [] }] } },
  }));
  const out = (await tool("list_projects").run({ limit: 25, only: ["dates"] }, {})) as Record<string, unknown>[];
  expect(out).toEqual([{ startDate: "2026-01-01", targetDate: "2026-09-01" }]);
});

test("project_updates defaults drop the update url", async () => {
  mockGraphQL(() => ({
    data: { project: { name: "Auth", projectUpdates: { nodes: [{ body: "shipping soon", health: "onTrack", createdAt: "2026-08-01", url: "https://x", user: { name: "Ada" } }] } } },
  }));
  const out = await tool("project_updates").run({ project_id: "p1", limit: 10 }, {});
  expect(out).toEqual({ project: "Auth", updates: [{ health: "onTrack", createdAt: "2026-08-01", user: { name: "Ada" }, body: "shipping soon" }] });
});

test("create_issue/comment/update_issue default to a write confirmation, not the full record", async () => {
  mockGraphQL((c) => {
    if (c.query.includes("nodes { id key name }")) return TEAMS;
    if (c.query.includes("issueCreate")) return CREATE;
    if (c.query.includes("commentCreate")) return { data: { commentCreate: { success: true, comment: { id: "c1", url: "https://x", parentId: null } } } };
    if (c.query.includes("issueUpdate")) return UPDATE;
    throw new Error(`unexpected query: ${c.query}`);
  });
  expect(await tool("create_issue").run({ team: "ENG", title: "Fix sign-in" }, {})).toEqual({ issue: { identifier: "ENG-42", url: "https://linear.app/acme/issue/ENG-42" } });
  expect(await tool("comment").run({ issue_id: "ENG-42", body: "hi" }, {})).toEqual({ comment: { url: "https://x" } });
  expect(await tool("update_issue").run({ id: "ENG-42", assignee: null }, {})).toEqual({ issue: { identifier: "ENG-42", state: { name: "Done" } } });
});

test("linearAdapter exposes the linear toolset", () => {
  expect(linearAdapter.toolSpecs.map((t) => t.name)).toEqual([
    "list_issues",
    "my_issues",
    "get_issue",
    "search_issues",
    "list_comments",
    "inbox",
    "list_teams",
    "list_states",
    "list_projects",
    "project_updates",
    "create_issue",
    "comment",
    "update_issue",
    "link_url",
  ]);
});
