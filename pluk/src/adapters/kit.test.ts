import { test, expect } from "bun:test";
import { getAdapter, buildAdapterServer } from "./index.js";
import { parseActionPolicy } from "../mcp/actionPolicy.js";
import type { Integration } from "../store/integrations.js";

function redisConn(readOnly: number, query_policy: string | null = null): Integration {
  return { id: "r", name: "R", type: "redis", config: { host: "h" }, read_only: readOnly, query_policy, token: "t", created_at: "" };
}

function toolNames(conn: Integration): string[] {
  const adapter = getAdapter("redis")!;
  const server = buildAdapterServer(adapter, conn, { value: "" });
  return Object.keys((server as unknown as { _registeredTools: Record<string, unknown> })._registeredTools).sort();
}

test("the binary write toggle grants delete too (a modify means it can delete)", () => {
  expect(parseActionPolicy(null, 1).allowed).toEqual(["read"]);
  expect(parseActionPolicy(null, 0).allowed).toEqual(["read", "write", "delete"]);
});

test("an explicit policy blob wins and is the only way to grant admin", () => {
  expect(parseActionPolicy('{"actions":["read"]}', 0).allowed).toEqual(["read"]);
  expect(parseActionPolicy('{"actions":["read","write","delete","admin"]}', 1).allowed).toContain("admin");
});

test("a read-only integration never advertises write/delete tools to the agent", () => {
  // Hidden, not merely blocked — the MCP server doesn't register them at all.
  const names = toolNames(redisConn(1));
  expect(names).toContain("get");
  expect(names).toContain("scan");
  for (const hidden of ["set", "expire", "del"]) expect(names).not.toContain(hidden);
});

test("a read+write integration exposes write and delete tools (incl. del)", () => {
  const names = toolNames(redisConn(0));
  for (const shown of ["set", "expire", "del"]) expect(names).toContain(shown);
});

test("the adapter publishes static action metadata for the catalog/UI", () => {
  const actions = getAdapter("redis")!.actions ?? [];
  expect(actions.find((a) => a.name === "del")?.category).toBe("delete");
  expect(actions.find((a) => a.name === "set")?.category).toBe("write");
  expect(actions.find((a) => a.name === "get")?.category).toBe("read");
});
