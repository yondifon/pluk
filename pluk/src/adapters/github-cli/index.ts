import { z } from "zod";
import { actionAdapter, type ActionTool } from "../kit.js";
import { githubCliFields } from "./fields.js";
import { ghConfig, ghJson, ghText, humanizeGhError, positional, repoFlag, resolveRepo, testGh, type GhConfig } from "./client.js";

const AGENT_HINT =
  "Use this for GitHub work through the installed gh CLI — issues, pull requests, releases, code search, file contents at a ref, and CI status. gh uses your own login and infers the repository and branch from the cwd you pass (e.g. a git worktree). Start with list_pull_requests or list_issues; set default_repo to skip the repo arg.";

// GitHub CLI's tools. Each declares its policy category, log line, and gh
// command; gating, logging, and response shaping are handled by actionAdapter.
// Every tool runs gh in the caller's cwd (a worktree), where gh infers the
// repository and current branch; repo/base/head args are overrides.
export function githubCliTools(cfg: GhConfig): ActionTool[] {
  const cwdArg = z.string().optional().describe("Working directory (e.g. a git worktree) to run gh in. Defaults to the integration's default working directory.");
  const repoArg = z.string().optional().describe("Repo as owner/repo. Defaults to the integration's default_repo; otherwise gh infers it from the cwd.");
  const limitArg = z.number().int().min(1).max(100).default(30).describe("Max results to return");

  return [
    {
      name: "list_issues",
      description: "List issues in a repo, newest first (excludes pull requests).",
      category: "read",
      schema: {
        cwd: cwdArg,
        repo: repoArg,
        state: z.enum(["open", "closed", "all"]).default("open").describe("Issue state"),
        limit: limitArg,
      },
      detail: (a) => `list_issues ${(a.repo as string) ?? cfg.defaultRepo ?? "?"} state=${a.state}`,
      run: async (a) =>
        ghJson(cfg, ["issue", "list", ...repoFlag(cfg, a.repo as string | undefined),
          "--state", a.state as string, "--limit", String(a.limit),
          "--json", "number,title,state,labels,author,createdAt,updatedAt"], a.cwd as string | undefined),
    },
    {
      name: "get_issue",
      description: "Get a single issue by number, with its comments.",
      category: "read",
      schema: { cwd: cwdArg, repo: repoArg, number: z.number().int().describe("Issue number") },
      detail: (a) => `get_issue ${(a.repo as string) ?? cfg.defaultRepo ?? "?"}#${a.number}`,
      run: async (a) =>
        ghJson(cfg, ["issue", "view", String(a.number), ...repoFlag(cfg, a.repo as string | undefined),
          "--json", "number,title,body,state,labels,author,comments,createdAt,updatedAt"], a.cwd as string | undefined),
    },
    {
      name: "list_pull_requests",
      description: "List pull requests in a repo.",
      category: "read",
      schema: {
        cwd: cwdArg,
        repo: repoArg,
        state: z.enum(["open", "closed", "all"]).default("open").describe("PR state"),
        limit: limitArg,
      },
      detail: (a) => `list_pull_requests ${(a.repo as string) ?? cfg.defaultRepo ?? "?"} state=${a.state}`,
      run: async (a) =>
        ghJson(cfg, ["pr", "list", ...repoFlag(cfg, a.repo as string | undefined),
          "--state", a.state as string, "--limit", String(a.limit),
          "--json", "number,title,state,headRefName,baseRefName,author,createdAt,updatedAt"], a.cwd as string | undefined),
    },
    {
      name: "get_pull_request",
      description: "Get a single pull request by number (title, body, state, mergeability).",
      category: "read",
      schema: { cwd: cwdArg, repo: repoArg, number: z.number().int().describe("PR number") },
      detail: (a) => `get_pull_request ${(a.repo as string) ?? cfg.defaultRepo ?? "?"}#${a.number}`,
      run: async (a) =>
        ghJson(cfg, ["pr", "view", String(a.number), ...repoFlag(cfg, a.repo as string | undefined),
          "--json", "number,title,body,state,headRefName,baseRefName,mergeable,author,createdAt,updatedAt"], a.cwd as string | undefined),
    },
    {
      name: "pr_files",
      description: "List the changed files (with patches) for a pull request.",
      category: "read",
      schema: { cwd: cwdArg, repo: repoArg, number: z.number().int().describe("PR number"), limit: limitArg },
      detail: (a) => `pr_files ${(a.repo as string) ?? cfg.defaultRepo ?? "?"}#${a.number}`,
      run: async (a) => {
        const { owner, repo } = resolveRepo(cfg, a.repo as string | undefined);
        return ghJson(cfg, ["api", "--method", "GET",
          `repos/${owner}/${repo}/pulls/${a.number}/files?per_page=${a.limit}`], a.cwd as string | undefined);
      },
    },
    {
      name: "search_code",
      description: "Search code with GitHub's code search syntax (e.g. 'addUser repo:owner/name').",
      category: "read",
      schema: { cwd: cwdArg, query: z.string().describe("Code search query"), limit: limitArg },
      detail: (a) => `search_code "${a.query}"`,
      run: async (a) => {
        const data = await ghJson(cfg, ["api", "--method", "GET",
          `search/code?q=${encodeURIComponent(String(a.query))}&per_page=${a.limit}`], a.cwd as string | undefined);
        return (data as { items?: unknown } | undefined)?.items ?? [];
      },
    },
    {
      name: "get_file",
      description: "Get a file's contents at an optional ref (branch/tag/sha).",
      category: "read",
      schema: {
        cwd: cwdArg,
        repo: repoArg,
        path: z.string().describe("File path within the repo"),
        ref: z.string().optional().describe("Branch, tag, or commit sha (defaults to the default branch)"),
      },
      detail: (a) => `get_file ${(a.repo as string) ?? cfg.defaultRepo ?? "?"}:${a.path}`,
      run: async (a) => {
        const { owner, repo } = resolveRepo(cfg, a.repo as string | undefined);
        const ref = a.ref ? `?ref=${encodeURIComponent(String(a.ref))}` : "";
        return ghJson(cfg, ["api", "--method", "GET",
          `repos/${owner}/${repo}/contents/${encodeURIComponent(String(a.path))}${ref}`], a.cwd as string | undefined);
      },
    },
    {
      name: "commit_status",
      description: "Get the combined commit status and check-runs for a ref (CI state).",
      category: "read",
      defaultEnabled: false,
      schema: { cwd: cwdArg, repo: repoArg, ref: z.string().describe("Branch, tag, or commit sha") },
      detail: (a) => `commit_status ${(a.repo as string) ?? cfg.defaultRepo ?? "?"}@${a.ref}`,
      run: async (a) => {
        const { owner, repo } = resolveRepo(cfg, a.repo as string | undefined);
        const ref = encodeURIComponent(String(a.ref));
        const [status, checks] = await Promise.all([
          ghJson(cfg, ["api", "--method", "GET", `repos/${owner}/${repo}/commits/${ref}/status`], a.cwd as string | undefined),
          ghJson(cfg, ["api", "--method", "GET", `repos/${owner}/${repo}/commits/${ref}/check-runs`], a.cwd as string | undefined),
        ]);
        return { status, check_runs: checks };
      },
    },
    {
      name: "get_repo",
      description: "Get repository metadata (owner, description, default branch, visibility).",
      category: "read",
      schema: { cwd: cwdArg, repo: repoArg },
      detail: (a) => `get_repo ${(a.repo as string) ?? cfg.defaultRepo ?? "?"}`,
      run: async (a) =>
        ghJson(cfg, ["repo", "view", ...repoFlag(cfg, a.repo as string | undefined),
          "--json", "name,owner,description,url,defaultBranchRef,isPrivate,pushedAt,stargazerCount,forkCount"], a.cwd as string | undefined),
    },
    {
      name: "list_releases",
      description: "List releases in a repo, newest first.",
      category: "read",
      schema: { cwd: cwdArg, repo: repoArg, limit: limitArg },
      detail: (a) => `list_releases ${(a.repo as string) ?? cfg.defaultRepo ?? "?"}`,
      run: async (a) =>
        ghJson(cfg, ["release", "list", ...repoFlag(cfg, a.repo as string | undefined),
          "--limit", String(a.limit),
          "--json", "tagName,name,isDraft,isPrerelease,author,publishedAt"], a.cwd as string | undefined),
    },
    {
      name: "get_release",
      description: "Get a single release by tag (body, assets, draft/prerelease state).",
      category: "read",
      schema: { cwd: cwdArg, repo: repoArg, tag: z.string().describe("Release tag, e.g. v1.2.3") },
      detail: (a) => `get_release ${(a.repo as string) ?? cfg.defaultRepo ?? "?"}@${a.tag}`,
      run: async (a) =>
        ghJson(cfg, ["release", "view", positional(a.tag, "tag"), ...repoFlag(cfg, a.repo as string | undefined),
          "--json", "tagName,name,body,isDraft,isPrerelease,author,createdAt,publishedAt,assets,url"], a.cwd as string | undefined),
    },

    // ── Write tools ──────────────────────────────────────────────────────────
    {
      name: "add_comment",
      description: "Add a comment to an issue or pull request (PRs share the issue comment endpoint).",
      category: "write",
      schema: { cwd: cwdArg, repo: repoArg, number: z.number().int().describe("Issue or PR number"), body: z.string().describe("Comment body (markdown)") },
      detail: (a) => `add_comment ${(a.repo as string) ?? cfg.defaultRepo ?? "?"}#${a.number}`,
      run: async (a) =>
        ghText(cfg, ["issue", "comment", String(a.number), ...repoFlag(cfg, a.repo as string | undefined),
          "--body", String(a.body)], a.cwd as string | undefined),
    },
    {
      name: "create_issue",
      description: "Create a new issue.",
      category: "write",
      schema: { cwd: cwdArg, repo: repoArg, title: z.string().describe("Issue title"), body: z.string().optional().describe("Issue body (markdown)") },
      detail: (a) => `create_issue ${(a.repo as string) ?? cfg.defaultRepo ?? "?"} "${a.title}"`,
      run: async (a) => {
        const args = ["issue", "create", ...repoFlag(cfg, a.repo as string | undefined), "--title", String(a.title)];
        if (a.body) args.push("--body", String(a.body));
        return ghText(cfg, args, a.cwd as string | undefined);
      },
    },
    {
      name: "create_pull_request",
      description: "Open a pull request from the current branch of the given cwd (a worktree) into base; pass head/base/repo to override what gh infers.",
      category: "write",
      schema: {
        cwd: cwdArg,
        repo: repoArg,
        title: z.string().describe("PR title"),
        base: z.string().optional().describe("Target branch (defaults to the repo's default branch)"),
        head: z.string().optional().describe("Source branch (defaults to the current branch of cwd)"),
        body: z.string().optional().describe("PR body (markdown)"),
        draft: z.boolean().optional().describe("Open as a draft"),
      },
      detail: (a) => `create_pull_request ${(a.repo as string) ?? cfg.defaultRepo ?? "?"} ${(a.head as string) ?? "?"}->${(a.base as string) ?? "default"}`,
      run: async (a) => {
        const args = ["pr", "create", ...repoFlag(cfg, a.repo as string | undefined), "--title", String(a.title)];
        if (a.body) args.push("--body", String(a.body));
        if (a.base) args.push("--base", String(a.base));
        if (a.head) args.push("--head", String(a.head));
        if (a.draft === true) args.push("--draft");
        return ghText(cfg, args, a.cwd as string | undefined);
      },
    },
    {
      name: "review_pull_request",
      description: "Submit a review on a pull request: approve, comment, or request changes.",
      category: "write",
      schema: {
        cwd: cwdArg,
        repo: repoArg,
        number: z.number().int().describe("PR number"),
        event: z.enum(["APPROVE", "COMMENT", "REQUEST_CHANGES"]).describe("Review action"),
        body: z.string().optional().describe("Review body (required for REQUEST_CHANGES/COMMENT)"),
      },
      detail: (a) => `review_pull_request ${(a.repo as string) ?? cfg.defaultRepo ?? "?"}#${a.number} ${a.event}`,
      run: async (a) => {
        const flag = a.event === "APPROVE" ? "--approve" : a.event === "COMMENT" ? "--comment" : "--request-changes";
        const args = ["pr", "review", String(a.number), ...repoFlag(cfg, a.repo as string | undefined), flag];
        if (a.body) args.push("--body", String(a.body));
        return ghText(cfg, args, a.cwd as string | undefined);
      },
    },
    {
      name: "create_release",
      description: "Publish a release for a tag (draft or prerelease optional).",
      category: "write",
      schema: {
        cwd: cwdArg,
        repo: repoArg,
        tag: z.string().describe("Release tag, e.g. v1.2.3"),
        title: z.string().optional().describe("Release title (defaults to the tag)"),
        notes: z.string().optional().describe("Release notes (markdown)"),
        draft: z.boolean().optional().describe("Create as a draft"),
        prerelease: z.boolean().optional().describe("Mark as a prerelease"),
      },
      detail: (a) => `create_release ${(a.repo as string) ?? cfg.defaultRepo ?? "?"} ${a.tag}`,
      run: async (a) => {
        const args = ["release", "create", positional(a.tag, "tag"), ...repoFlag(cfg, a.repo as string | undefined)];
        if (a.title) args.push("--title", String(a.title));
        if (a.notes) args.push("--notes", String(a.notes));
        if (a.draft === true) args.push("--draft");
        if (a.prerelease === true) args.push("--prerelease");
        return ghText(cfg, args, a.cwd as string | undefined);
      },
    },
  ];
}

export const githubCliAdapter = actionAdapter<GhConfig>({
  id: "github-cli",
  label: "GitHub CLI",
  category: "code-host",
  agentHint: AGENT_HINT,
  access:
    "Runs the locally installed gh CLI with your own GitHub login — Pluk stores no credentials. Reads issues, PRs, diffs, code search, file contents, CI status, and releases; comments and opens issues/PRs/releases when write is permitted. Every action is policy-checked and recorded in the activity log.",
  start: "list_pull_requests",
  configFields: githubCliFields,
  client: (conn) => ghConfig(conn),
  testConnection: (conn) => testGh(conn),
  humanizeError: humanizeGhError,
  tools: (_conn, cfg) => githubCliTools(cfg),
});
