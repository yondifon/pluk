import { test, expect } from "bun:test";
import { sqlToolSpecs } from "./sql/server.js";
import { sshToolSpecs } from "./ssh/server.js";
import { githubAdapter } from "./github/index.js";
import { redisAdapter } from "./redis/index.js";
import { linearAdapter } from "./linear/index.js";
import type { ToolSpec } from "./types.js";

// The default-on tool set: what a developer sees the moment they connect an
// integration, before configuring anything. Kept lean — the common tools on,
// everything niche/heavy/state-changing off (opt-in). These lock the curation so
// a new tool can't silently re-expand the default surface.

function defaults(specs: ToolSpec[]): { on: string[]; off: string[] } {
  return {
    on: specs.filter((t) => t.defaultEnabled).map((t) => t.name).sort(),
    off: specs.filter((t) => !t.defaultEnabled).map((t) => t.name).sort(),
  };
}

test("SQL default-on is the lean core; the rest is opt-in", () => {
  const { on, off } = defaults(sqlToolSpecs());
  expect(on).toEqual(["describe_table", "list_tables", "query", "sample_table", "search_schema"]);
  // Perf / discovery / setup-dependent / side-effecting tools ship off.
  for (const t of ["explain_query", "list_relationships", "table_stats", "list_schemas",
    "list_databases", "export_query", "run_saved_query", "list_saved_queries"]) {
    expect(off).toContain(t);
  }
});

test("SSH defaults to just run_command; batching/forwards/saved are opt-in", () => {
  const { on, off } = defaults(sshToolSpecs());
  expect(on).toEqual(["run_command"]);
  for (const t of ["run_batch", "debug_snapshot", "open_forward", "list_forwards", "close_forward"]) {
    expect(off).toContain(t);
  }
});

test("action adapters: writes/deletes off, niche reads off, core reads on", () => {
  const gh = new Map(githubAdapter.toolSpecs.map((t) => [t.name, t.defaultEnabled]));
  expect(gh.get("list_pull_requests")).toBe(true);
  expect(gh.get("search_code")).toBe(true);
  expect(gh.get("commit_status")).toBe(false);          // niche read, opted out
  expect(gh.get("create_pull_request")).toBe(false);    // write, always off

  const redis = new Map(redisAdapter.toolSpecs.map((t) => [t.name, t.defaultEnabled]));
  expect(redis.get("scan")).toBe(true);
  expect(redis.get("get")).toBe(true);
  expect(redis.get("keys")).toBe(false);                // O(N), prefer scan
  expect(redis.get("del")).toBe(false);                 // delete, always off

  const linear = new Map(linearAdapter.toolSpecs.map((t) => [t.name, t.defaultEnabled]));
  expect(linear.get("list_issues")).toBe(true);
  expect(linear.get("list_teams")).toBe(false);         // metadata feeder, opted out
  expect(linear.get("create_issue")).toBe(false);       // write, always off
});

test("no state-changing tool is ever default-on", () => {
  for (const adapter of [githubAdapter, redisAdapter, linearAdapter]) {
    for (const t of adapter.toolSpecs) {
      if (t.category !== "read" && t.category !== "inspect") {
        expect(t.defaultEnabled).toBe(false);
      }
    }
  }
});
