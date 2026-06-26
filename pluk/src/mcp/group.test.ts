import { test, expect } from "bun:test";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { namespacedHost, slug } from "./namespace.js";
import { applyOverrides } from "./group.js";
import { registerSqlServer } from "../adapters/sql/server.js";
import type { ConfigField } from "../adapters/index.js";
import type { Integration } from "../store/integrations.js";

test("slug makes a tool-safe prefix", () => {
  expect(slug("Metrics DB")).toBe("metrics_db");
  expect(slug("DB — Production!")).toBe("db_production");
  expect(slug("")).toBe("member");
});

test("namespacedHost prefixes tool/prompt names and resource URIs", () => {
  const calls: string[] = [];
  const fake = {
    tool: (n: string) => calls.push(`tool:${n}`),
    prompt: (n: string) => calls.push(`prompt:${n}`),
    resource: (n: string, uri: string) => calls.push(`res:${n}:${uri}`),
  } as unknown as McpServer;

  const host = namespacedHost(fake, "metrics_db") as unknown as {
    tool: (n: string, cb: () => void) => void;
    prompt: (n: string, cb: () => void) => void;
    resource: (n: string, uri: string, cb: () => void) => void;
  };
  host.tool("query", () => {});
  host.prompt("summarize_schema", () => {});
  host.resource("schema", "schema://full", () => {});

  expect(calls).toEqual([
    "tool:metrics_db__query",
    "prompt:metrics_db__summarize_schema",
    "res:metrics_db__schema:schema://metrics_db/full",
  ]);
});

function fakeSqlite(id: string, name: string): Integration {
  return {
    id,
    name,
    type: "sqlite",
    config: {},
    read_only: 1,
    query_policy: null,
    token: `t_${id}`,
    created_at: "",
  };
}

test("applyOverrides merges per-group config, typed by adapter fields", () => {
  const base: Integration = {
    id: "lin", name: "Linear", type: "linear",
    config: { api_key: "secret", team_key: "ENG" },
    read_only: 0, query_policy: null, token: "t", created_at: "",
  };
  const fields: ConfigField[] = [
    { key: "api_key", label: "API key", type: "password" },
    { key: "team_key", label: "Team", type: "text" },
    { key: "limit", label: "Limit", type: "number" },
    { key: "active", label: "Active", type: "toggle" },
  ];

  const scoped = applyOverrides(base, { team_key: "PROJ1", limit: "50", active: "true" }, fields);

  // Override wins, coerced to declared types; untouched keys are preserved.
  expect(scoped.config.team_key).toBe("PROJ1");
  expect(scoped.config.limit).toBe(50);
  expect(scoped.config.active).toBe(true);
  expect(scoped.config.api_key).toBe("secret");
  // The base integration is not mutated.
  expect(base.config.team_key).toBe("ENG");
});

test("applyOverrides ignores blank values (inherit) and no-op overrides", () => {
  const base: Integration = {
    id: "lin", name: "Linear", type: "linear", config: { team_key: "ENG" },
    read_only: 0, query_policy: null, token: "t", created_at: "",
  };
  const fields: ConfigField[] = [{ key: "team_key", label: "Team", type: "text" }];

  expect(applyOverrides(base, { team_key: "" }, fields).config.team_key).toBe("ENG");
  expect(applyOverrides(base, undefined, fields)).toBe(base);
  expect(applyOverrides(base, {}, fields)).toBe(base);
});

test("two same-type members register on one server without colliding", () => {
  const server = new McpServer({ name: "DB Production", version: "1.0.0" });

  // Both expose a "query" tool, "schema://full" resource, etc. Without
  // namespacing the second registration throws "already registered".
  registerSqlServer(namespacedHost(server, "metrics"), fakeSqlite("a", "Metrics"), { value: "" });
  expect(() =>
    registerSqlServer(namespacedHost(server, "analytics"), fakeSqlite("b", "Analytics"), { value: "" })
  ).not.toThrow();

  const tools = (server as unknown as { _registeredTools: Record<string, unknown> })._registeredTools;
  expect(Object.keys(tools)).toContain("metrics__query");
  expect(Object.keys(tools)).toContain("analytics__query");
});

test("SQL query tool accepts sql and query args", async () => {
  const server = new McpServer({ name: "DB Production", version: "1.0.0" });
  registerSqlServer(server, { ...fakeSqlite("a", "Metrics"), environment: "production" }, { value: "" });

  const tool = (server as unknown as { _registeredTools: Record<string, { inputSchema: { safeParse: (v: unknown) => { success: boolean } }; handler: (v: unknown) => Promise<{ isError?: boolean }> }> })._registeredTools.query;
  if (!tool) throw new Error("query tool was not registered");

  expect(tool.inputSchema.safeParse({ sql: "select 1" }).success).toBe(true);
  expect(tool.inputSchema.safeParse({ query: "select 1" }).success).toBe(true);
  expect((await tool.handler({})).isError).toBe(true);
});
