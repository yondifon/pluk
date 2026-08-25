import { test, expect, afterEach } from "bun:test";
import {
  ghConfig, ghCwd, repoFlag, resolveRepo, runGh, setGhRunner, humanizeGhError,
  type GhRunner, type GhRunResult,
} from "./client.js";
import { githubCliAdapter, githubCliTools } from "./index.js";
import type { Integration } from "../../store/integrations.js";

function conn(config: Record<string, unknown> = {}): Integration {
  return { id: "g", name: "GitHub CLI", type: "github-cli", config, read_only: 0, query_policy: null, token: "t", created_at: "" };
}

interface Call { bin: string; args: string[]; cwd: string; timeoutMs: number }

const calls: Call[] = [];
const fake: GhRunner = async (bin, args, cwd, timeoutMs) => {
  calls.push({ bin, args, cwd, timeoutMs });
  return { code: 0, stdout: "[]", stderr: "" };
};

setGhRunner(fake);
afterEach(() => { calls.length = 0; });

function tool(name: string) {
  const t = githubCliTools(ghConfig(conn())).find((t) => t.name === name);
  if (!t) throw new Error(`missing tool ${name}`);
  return t;
}

test("ghConfig defaults to gh on PATH, the process cwd, and a 30s timeout", () => {
  expect(ghConfig(conn())).toMatchObject({ bin: "gh", defaultRepo: undefined, defaultCwd: process.cwd(), timeoutMs: 30_000 });
});

test("ghConfig honours gh_bin, default_repo, default_cwd and timeout_seconds", () => {
  const cfg = ghConfig(conn({ gh_bin: " ~/bin/gh ", default_repo: "acme/app", default_cwd: "/wt", timeout_seconds: 10 }));
  expect(cfg.bin.endsWith("/bin/gh")).toBe(true);
  expect(cfg.bin.startsWith("~")).toBe(false);
  expect(cfg).toMatchObject({ defaultRepo: "acme/app", defaultCwd: "/wt", timeoutMs: 10_000 });
});

test("ghConfig rejects nonsense timeouts rather than disabling the cap", () => {
  expect(ghConfig(conn({ timeout_seconds: 0 })).timeoutMs).toBe(30_000);
  expect(ghConfig(conn({ timeout_seconds: -3 })).timeoutMs).toBe(30_000);
});

test("ghCwd prefers the call's cwd over the integration default", () => {
  const cfg = ghConfig(conn({ default_cwd: "/wt" }));
  expect(ghCwd(cfg, "/wt/feature")).toBe("/wt/feature");
  expect(ghCwd(cfg, "  ")).toBe("/wt");
  expect(ghCwd(cfg, undefined)).toBe("/wt");
});

test("repoFlag carries the call's repo over the default and stays empty when neither", () => {
  const cfg = ghConfig(conn({ default_repo: "acme/app" }));
  expect(repoFlag(cfg, "other/repo")).toEqual(["--repo", "other/repo"]);
  expect(repoFlag(cfg, undefined)).toEqual(["--repo", "acme/app"]);
  expect(repoFlag(ghConfig(conn()), undefined)).toEqual([]);
});

test("resolveRepo keeps the arg/default contract and rejects bad input", () => {
  const cfg = ghConfig(conn({ default_repo: "acme/app" }));
  expect(resolveRepo(cfg, "other/repo")).toEqual({ owner: "other", repo: "repo" });
  expect(resolveRepo(cfg, undefined)).toEqual({ owner: "acme", repo: "app" });
  expect(() => resolveRepo(ghConfig(conn()), undefined)).toThrow(/No repo given/);
  expect(() => resolveRepo(cfg, "not-a-repo")).toThrow(/owner\/repo/);
});

test("runGh forwards cwd to the runner", async () => {
  const cfg = ghConfig(conn({ default_cwd: "/wt" }));
  await runGh(cfg, ["auth", "status"], "/wt/feature");
  expect(calls).toHaveLength(1);
  expect(calls[0]!.cwd).toBe("/wt/feature");
  await runGh(cfg, ["auth", "status"]);
  expect(calls[1]!.cwd).toBe("/wt");
});

test("list_pull_requests builds the exact gh argument list and returns parsed JSON", async () => {
  const result = await tool("list_pull_requests").run({ repo: "acme/app", state: "open", limit: 30 }, {});
  expect(calls[0]!.args).toEqual([
    "pr", "list", "--repo", "acme/app", "--state", "open", "--limit", "30",
    "--json", "number,title,state,headRefName,baseRefName,author,createdAt,updatedAt",
  ]);
  expect(calls[0]!.bin).toBe("gh");
  expect(result).toEqual([]);
});

test("get_issue passes the number positionally and forwards cwd", async () => {
  await tool("get_issue").run({ repo: "acme/app", number: 12, cwd: "/wt/feature" }, {});
  expect(calls[0]!.args).toEqual([
    "issue", "view", "12", "--repo", "acme/app",
    "--json", "number,title,body,state,labels,author,comments,createdAt,updatedAt",
  ]);
  expect(calls[0]!.cwd).toBe("/wt/feature");
});

test("a non-zero exit surfaces gh's stderr with the exit code", async () => {
  setGhRunner(async () => ({ code: 1, stdout: "", stderr: "not authorized" }));
  try {
    await expect(tool("list_pull_requests").run({}, {})).rejects.toThrow(/exit 1.*not authorized/);
  } finally {
    setGhRunner(fake);
  }
});

test("missing executable gives a clear error instead of a raw spawn failure", async () => {
  setGhRunner(undefined);
  try {
    const cfg = ghConfig(conn({ gh_bin: "/nope/gh" }));
    await expect(runGh(cfg, ["--version"])).rejects.toThrow(/gh executable not found: \/nope\/gh/);
  } finally {
    setGhRunner(fake);
  }
});

test("worktree PR creation: gh infers repo and branch unless overridden", async () => {
  await tool("create_pull_request").run({ cwd: "/wt/feature", title: "Add auth" }, {});
  expect(calls[0]!.cwd).toBe("/wt/feature");
  expect(calls[0]!.args).toEqual(["pr", "create", "--title", "Add auth"]);
});

test("worktree PR creation: explicit repo/head/base/draft become flags", async () => {
  await tool("create_pull_request").run(
    { cwd: "/wt/feature", title: "Add auth", body: "Body", repo: "acme/app", head: "feature", base: "main", draft: true },
    {},
  );
  expect(calls[0]!.args).toEqual([
    "pr", "create", "--repo", "acme/app", "--title", "Add auth", "--body", "Body",
    "--base", "main", "--head", "feature", "--draft",
  ]);
});

test("api-backed tools resolve the repo before building the URL", async () => {
  await tool("pr_files").run({ repo: "acme/app", number: 7, limit: 30 }, {});
  expect(calls[0]!.args).toEqual(["api", "--method", "GET", "repos/acme/app/pulls/7/files?per_page=30"]);
  await expect(tool("pr_files").run({ number: 7 }, {})).rejects.toThrow(/No repo given/);
});

test("release tools cover list, view, and create", async () => {
  await tool("list_releases").run({ repo: "acme/app", limit: 30 }, {});
  expect(calls[0]!.args[0]).toBe("release");
  expect(calls[0]!.args[1]).toBe("list");

  await tool("get_release").run({ repo: "acme/app", tag: "v1.2.3" }, {});
  expect(calls[1]!.args).toEqual([
    "release", "view", "v1.2.3", "--repo", "acme/app",
    "--json", "tagName,name,body,isDraft,isPrerelease,author,createdAt,publishedAt,assets,url",
  ]);

  await tool("create_release").run({ repo: "acme/app", tag: "v1.2.3", title: "v1.2.3", notes: "Notes", draft: true }, {});
  expect(calls[2]!.args).toEqual([
    "release", "create", "v1.2.3", "--repo", "acme/app", "--title", "v1.2.3", "--notes", "Notes", "--draft",
  ]);
});

test("a tag that reads as a flag is refused before reaching gh", async () => {
  await expect(tool("get_release").run({ repo: "acme/app", tag: "--draft" }, {})).rejects.toThrow(/must not start with/);
});

test("testConnection passes on a clean auth status and fails with guidance", async () => {
  await githubCliAdapter.testConnection(conn());

  setGhRunner(async (): Promise<GhRunResult> => ({ code: 1, stdout: "", stderr: "Please log in first" }));
  try {
    await expect(githubCliAdapter.testConnection(conn())).rejects.toThrow(/not authenticated.*gh auth login/);
  } finally {
    setGhRunner(fake);
  }
});

test("humanizeGhError points at install and login for the two fixable failures", () => {
  expect(humanizeGhError(new Error("gh executable not found: /nope/gh"))).toMatch(/gh auth login/);
  expect(humanizeGhError(new Error("gh pr list failed (exit 1): Please log in"))).toMatch(/gh auth login/);
});

test("the adapter exposes the gh surface once, reads on, writes off", () => {
  const names = githubCliAdapter.toolSpecs.map((t) => t.name);
  expect(new Set(names).size).toBe(names.length);
  expect(names).toContain("create_pull_request");
  expect(names).toContain("list_releases");
  expect(names).toContain("get_repo");

  const byName = Object.fromEntries(githubCliAdapter.toolSpecs.map((t) => [t.name, t]));
  expect(byName.list_pull_requests!.defaultEnabled).toBe(true);
  expect(byName.get_repo!.defaultEnabled).toBe(true);
  expect(byName.commit_status!.defaultEnabled).toBe(false);
  for (const w of ["add_comment", "create_issue", "create_pull_request", "review_pull_request", "create_release"]) {
    expect(byName[w]!.defaultEnabled).toBe(false);
  }
});

test("get_repo supports default, preset, full, and invalid projections", async () => {
  setGhRunner(async () => ({ code: 0, stdout: JSON.stringify({ name: "app", owner: { login: "acme" }, description: "d", url: "u", defaultBranchRef: { name: "main" }, isPrivate: false, stargazerCount: 2 }), stderr: "" }));
  try {
    const repo = tool("get_repo");
    expect(await repo.run({ repo: "acme/app" }, {})).toEqual({ name: "app", owner: { login: "acme" }, description: "d", url: "u", defaultBranchRef: { name: "main" }, isPrivate: false });
    expect(await repo.run({ repo: "acme/app", only: ["stats"] }, {})).toEqual({ stargazerCount: 2, forkCount: undefined, pushedAt: undefined });
    expect(await repo.run({ repo: "acme/app", only: ["*"] }, {})).toEqual({ name: "app", owner: { login: "acme" }, description: "d", url: "u", defaultBranchRef: { name: "main" }, isPrivate: false, stargazerCount: 2 });
    await expect(repo.run({ repo: "acme/app", only: ["missing"] }, {})).rejects.toThrow(/Unknown "only" field/);
  } finally {
    setGhRunner(fake);
  }
});
