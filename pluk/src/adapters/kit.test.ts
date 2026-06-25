import { test, expect } from "bun:test";
import { getAdapter, buildAdapterServer } from "./index.js";
import type { Integration } from "../store/integrations.js";

// Build a Redis integration with an explicit per-tool config. `enable` lists the
// tool names turned on; everything else falls to its declared default (read tools
// on, write/delete off).
function redisConn(enable?: string[]): Integration {
  const query_policy = enable
    ? JSON.stringify({ tools: Object.fromEntries(enable.map((n) => [n, { enabled: true }])) })
    : null;
  return { id: "r", name: "R", type: "redis", config: { host: "h" }, read_only: 0, query_policy, token: "t", created_at: "" };
}

function toolNames(conn: Integration): string[] {
  const adapter = getAdapter("redis")!;
  const server = buildAdapterServer(adapter, conn, { value: "" });
  return Object.keys((server as unknown as { _registeredTools: Record<string, unknown> })._registeredTools).sort();
}

test("an unconfigured integration exposes read tools but not write/delete (fail safe)", () => {
  // Hidden, not merely blocked — the MCP server doesn't register them at all.
  const names = toolNames(redisConn());
  expect(names).toContain("get");
  expect(names).toContain("scan");
  for (const hidden of ["set", "expire", "del"]) expect(names).not.toContain(hidden);
});

test("enabling a write/delete tool exposes exactly that tool", () => {
  const names = toolNames(redisConn(["set", "del"]));
  expect(names).toContain("set");
  expect(names).toContain("del");
  expect(names).not.toContain("expire"); // not enabled → still hidden
});

test("a disabled read tool is removed from the surface", () => {
  // `get` defaults on; explicitly disabling it drops it from the server.
  const query_policy = JSON.stringify({ tools: { get: { enabled: false } } });
  const conn: Integration = { id: "r", name: "R", type: "redis", config: { host: "h" }, read_only: 0, query_policy, token: "t", created_at: "" };
  const names = toolNames(conn);
  expect(names).toContain("scan");
  expect(names).not.toContain("get");
});

test("the adapter publishes static tool specs for the catalog/UI", () => {
  const specs = getAdapter("redis")!.toolSpecs;
  expect(specs.find((t) => t.name === "del")?.category).toBe("delete");
  expect(specs.find((t) => t.name === "del")?.defaultEnabled).toBe(false);
  expect(specs.find((t) => t.name === "set")?.category).toBe("write");
  expect(specs.find((t) => t.name === "get")?.category).toBe("read");
  expect(specs.find((t) => t.name === "get")?.defaultEnabled).toBe(true);
});
