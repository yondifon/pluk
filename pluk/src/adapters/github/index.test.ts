import { test, expect, afterEach } from "bun:test";
import { githubConfig, githubRequest, resolveRepo } from "./client.js";
import { githubAdapter } from "./index.js";
import type { Integration } from "../../store/integrations.js";

function conn(config: Record<string, unknown>): Integration {
  return { id: "g", name: "GitHub", type: "github", config, read_only: 0, query_policy: null, token: "t", created_at: "" };
}

const realFetch = globalThis.fetch;
afterEach(() => { globalThis.fetch = realFetch; });

test("githubConfig defaults base URL and reads token + default repo", () => {
  const cfg = githubConfig(conn({ token: "github_pat_x", default_repo: "acme/app" }));
  expect(cfg.baseUrl).toBe("https://api.github.com");
  expect(cfg.token).toBe("github_pat_x");
  expect(cfg.defaultRepo).toBe("acme/app");
});

test("resolveRepo uses the arg, falls back to default, and rejects bad input", () => {
  const cfg = githubConfig(conn({ token: "t", default_repo: "acme/app" }));
  expect(resolveRepo(cfg, "other/repo")).toEqual({ owner: "other", repo: "repo" });
  expect(resolveRepo(cfg, undefined)).toEqual({ owner: "acme", repo: "app" });
  // Neither arg nor default → explicit error so the agent fixes its call.
  expect(() => resolveRepo(githubConfig(conn({ token: "t" })), undefined)).toThrow(/No repo given/);
  expect(() => resolveRepo(cfg, "not-a-repo")).toThrow(/owner\/repo/);
});

test("githubRequest throws a clear error before any network call when token is blank", async () => {
  await expect(githubRequest(githubConfig(conn({})), "GET", "/user")).rejects.toThrow(/token is missing/);
});

test("githubRequest maps a 401 to an actionable message", async () => {
  globalThis.fetch = (async () =>
    new Response(JSON.stringify({ message: "Bad credentials" }), { status: 401 })) as unknown as typeof fetch;
  await expect(githubRequest(githubConfig(conn({ token: "bad" })), "GET", "/user")).rejects.toThrow(/unauthorized \(401\).*Bad credentials/);
});

test("testConnection rejects when the token is blank", async () => {
  await expect(githubAdapter.testConnection(conn({}))).rejects.toThrow(/token is missing/);
});
