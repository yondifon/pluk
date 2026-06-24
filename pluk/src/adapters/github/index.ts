import { z } from "zod";
import type { Integration } from "../../store/integrations.js";
import { actionAdapter, type ActionTool } from "../kit.js";
import { githubFields } from "./fields.js";
import { githubConfig, githubRequest, resolveRepo, type GitHubConfig } from "./client.js";

const AGENT_HINT = "Use this for GitHub repository work — reading and triaging issues and pull requests, searching code, reading files at a ref, and checking CI status, plus commenting and opening issues/PRs. Start with list_issues or list_pull_requests; set default_repo to skip the owner/repo args.";

// GitHub's tools. Each declares its policy category, log line, and REST call;
// gating, logging, and response shaping are handled by actionAdapter. Repo-scoped
// tools default to the connection's default_repo when no repo arg is given.
function githubTools(cfg: GitHubConfig): ActionTool[] {
  const repoArg = z.string().optional().describe("Repo as owner/repo. Defaults to the integration's default_repo.");
  const limitArg = z.number().int().min(1).max(100).default(30).describe("Max results to return");

  return [
    {
      name: "list_issues",
      description: "List issues in a repo, newest first (excludes pull requests).",
      category: "read",
      schema: {
        repo: repoArg,
        state: z.enum(["open", "closed", "all"]).default("open").describe("Issue state"),
        limit: limitArg,
      },
      detail: (a) => `list_issues ${(a.repo as string) ?? cfg.defaultRepo ?? "?"} state=${a.state}`,
      run: async (a) => {
        const { owner, repo } = resolveRepo(cfg, a.repo as string | undefined);
        const issues = await githubRequest<{ pull_request?: unknown }[]>(
          cfg, "GET", `/repos/${owner}/${repo}/issues`,
          { state: a.state as string, per_page: a.limit as number },
        );
        // The issues endpoint includes PRs; drop them so list_issues means issues.
        return issues.filter((i) => !i.pull_request);
      },
    },
    {
      name: "get_issue",
      description: "Get a single issue by number.",
      category: "read",
      schema: { repo: repoArg, number: z.number().int().describe("Issue number") },
      detail: (a) => `get_issue ${(a.repo as string) ?? cfg.defaultRepo ?? "?"}#${a.number}`,
      run: async (a) => {
        const { owner, repo } = resolveRepo(cfg, a.repo as string | undefined);
        return githubRequest(cfg, "GET", `/repos/${owner}/${repo}/issues/${a.number}`);
      },
    },
    {
      name: "list_pull_requests",
      description: "List pull requests in a repo.",
      category: "read",
      schema: {
        repo: repoArg,
        state: z.enum(["open", "closed", "all"]).default("open").describe("PR state"),
        limit: limitArg,
      },
      detail: (a) => `list_pull_requests ${(a.repo as string) ?? cfg.defaultRepo ?? "?"} state=${a.state}`,
      run: async (a) => {
        const { owner, repo } = resolveRepo(cfg, a.repo as string | undefined);
        return githubRequest(cfg, "GET", `/repos/${owner}/${repo}/pulls`, { state: a.state as string, per_page: a.limit as number });
      },
    },
    {
      name: "get_pull_request",
      description: "Get a single pull request by number (title, body, state, mergeability).",
      category: "read",
      schema: { repo: repoArg, number: z.number().int().describe("PR number") },
      detail: (a) => `get_pull_request ${(a.repo as string) ?? cfg.defaultRepo ?? "?"}#${a.number}`,
      run: async (a) => {
        const { owner, repo } = resolveRepo(cfg, a.repo as string | undefined);
        return githubRequest(cfg, "GET", `/repos/${owner}/${repo}/pulls/${a.number}`);
      },
    },
    {
      name: "pr_files",
      description: "List the changed files (with patches) for a pull request.",
      category: "read",
      schema: { repo: repoArg, number: z.number().int().describe("PR number"), limit: limitArg },
      detail: (a) => `pr_files ${(a.repo as string) ?? cfg.defaultRepo ?? "?"}#${a.number}`,
      run: async (a) => {
        const { owner, repo } = resolveRepo(cfg, a.repo as string | undefined);
        return githubRequest(cfg, "GET", `/repos/${owner}/${repo}/pulls/${a.number}/files`, { per_page: a.limit as number });
      },
    },
    {
      name: "search_code",
      description: "Search code with GitHub's code search syntax (e.g. 'addUser repo:owner/name').",
      category: "read",
      schema: { query: z.string().describe("Code search query"), limit: limitArg },
      detail: (a) => `search_code "${a.query}"`,
      run: async (a) => {
        const data = await githubRequest<{ items: unknown[] }>(cfg, "GET", `/search/code`, { q: a.query as string, per_page: a.limit as number });
        return data.items;
      },
    },
    {
      name: "get_file",
      description: "Get a file's contents at an optional ref (branch/tag/sha).",
      category: "read",
      schema: {
        repo: repoArg,
        path: z.string().describe("File path within the repo"),
        ref: z.string().optional().describe("Branch, tag, or commit sha (defaults to the default branch)"),
      },
      detail: (a) => `get_file ${(a.repo as string) ?? cfg.defaultRepo ?? "?"}:${a.path}`,
      run: async (a) => {
        const { owner, repo } = resolveRepo(cfg, a.repo as string | undefined);
        return githubRequest(cfg, "GET", `/repos/${owner}/${repo}/contents/${encodeURIComponent(String(a.path))}`, { ref: a.ref as string | undefined });
      },
    },
    {
      name: "commit_status",
      description: "Get the combined commit status and check-runs for a ref (CI state).",
      category: "read",
      schema: { repo: repoArg, ref: z.string().describe("Branch, tag, or commit sha") },
      detail: (a) => `commit_status ${(a.repo as string) ?? cfg.defaultRepo ?? "?"}@${a.ref}`,
      run: async (a) => {
        const { owner, repo } = resolveRepo(cfg, a.repo as string | undefined);
        const ref = encodeURIComponent(String(a.ref));
        const [status, checks] = await Promise.all([
          githubRequest(cfg, "GET", `/repos/${owner}/${repo}/commits/${ref}/status`),
          githubRequest(cfg, "GET", `/repos/${owner}/${repo}/commits/${ref}/check-runs`),
        ]);
        return { status, check_runs: checks };
      },
    },

    // ── Write tools ──────────────────────────────────────────────────────────
    {
      name: "add_comment",
      description: "Add a comment to an issue or pull request (PRs share the issue comment endpoint).",
      category: "write",
      schema: { repo: repoArg, number: z.number().int().describe("Issue or PR number"), body: z.string().describe("Comment body (markdown)") },
      detail: (a) => `add_comment ${(a.repo as string) ?? cfg.defaultRepo ?? "?"}#${a.number}`,
      run: async (a) => {
        const { owner, repo } = resolveRepo(cfg, a.repo as string | undefined);
        return githubRequest(cfg, "POST", `/repos/${owner}/${repo}/issues/${a.number}/comments`, undefined, { body: a.body });
      },
    },
    {
      name: "create_issue",
      description: "Create a new issue.",
      category: "write",
      schema: { repo: repoArg, title: z.string().describe("Issue title"), body: z.string().optional().describe("Issue body (markdown)") },
      detail: (a) => `create_issue ${(a.repo as string) ?? cfg.defaultRepo ?? "?"} "${a.title}"`,
      run: async (a) => {
        const { owner, repo } = resolveRepo(cfg, a.repo as string | undefined);
        return githubRequest(cfg, "POST", `/repos/${owner}/${repo}/issues`, undefined, { title: a.title, body: a.body });
      },
    },
    {
      name: "create_pull_request",
      description: "Open a pull request from head into base.",
      category: "write",
      schema: {
        repo: repoArg,
        title: z.string().describe("PR title"),
        head: z.string().describe("Source branch (or owner:branch for forks)"),
        base: z.string().describe("Target branch"),
        body: z.string().optional().describe("PR body (markdown)"),
      },
      detail: (a) => `create_pull_request ${(a.repo as string) ?? cfg.defaultRepo ?? "?"} ${a.head}->${a.base}`,
      run: async (a) => {
        const { owner, repo } = resolveRepo(cfg, a.repo as string | undefined);
        return githubRequest(cfg, "POST", `/repos/${owner}/${repo}/pulls`, undefined, { title: a.title, head: a.head, base: a.base, body: a.body });
      },
    },
    {
      name: "review_pull_request",
      description: "Submit a review on a pull request: approve, comment, or request changes.",
      category: "write",
      schema: {
        repo: repoArg,
        number: z.number().int().describe("PR number"),
        event: z.enum(["APPROVE", "COMMENT", "REQUEST_CHANGES"]).describe("Review action"),
        body: z.string().optional().describe("Review body (required for REQUEST_CHANGES/COMMENT)"),
      },
      detail: (a) => `review_pull_request ${(a.repo as string) ?? cfg.defaultRepo ?? "?"}#${a.number} ${a.event}`,
      run: async (a) => {
        const { owner, repo } = resolveRepo(cfg, a.repo as string | undefined);
        return githubRequest(cfg, "POST", `/repos/${owner}/${repo}/pulls/${a.number}/reviews`, undefined, { event: a.event, body: a.body });
      },
    },
  ];
}

export const githubAdapter = actionAdapter<GitHubConfig>({
  id: "github",
  label: "GitHub",
  category: "code-host",
  agentHint: AGENT_HINT,
  access:
    "Read issues, PRs, diffs, code search, file contents, and CI status; comment and open issues/PRs when write is permitted. Every action is policy-checked and recorded in the activity log.",
  start: "list_issues",
  configFields: githubFields,
  client: (conn) => githubConfig(conn),
  async testConnection(conn: Integration): Promise<void> {
    const cfg = githubConfig(conn);
    // Cheapest authenticated call that validates the token.
    await githubRequest(cfg, "GET", `/user`);
  },
  tools: (_conn, cfg) => githubTools(cfg),
});
