import { describe, test, expect } from "bun:test";
import { slug, slugsWithCollision, toolPrefix } from "./slug";
import { detectTransitions } from "./health";
import { emptyState } from "./emptyStates";

// slug must match server: crates/pluk-server/src/mcp/namespace.rs and pluk/src/mcp/namespace.ts
describe("slug derivation matches server", () => {
  test("basic cases", () => {
    expect(slug("Metrics DB")).toBe("metrics_db");
    expect(slug("DB — Production!")).toBe("db_production");
    expect(slug("")).toBe("member");
    expect(slug("__--__")).toBe("member");
    expect(slug("Hello World")).toBe("hello_world");
    expect(slug("a--b__c")).toBe("a_b_c");
    expect(slug("___foo___")).toBe("foo");
    expect(slug("123")).toBe("123");
    expect(slug("Metrics DB")).toBe("metrics_db"); // repeat
  });

  test("matches swift NamespaceFormat.slug samples", () => {
    // swift test would give same: lowercased, [^a-z0-9]+ -> "_", trim _
    expect(slug("My Integration")).toBe("my_integration");
    expect(slug("FOO")).toBe("foo");
    expect(slug("foo   bar")).toBe("foo_bar");
  });

  test("collision handling matches group.ts", () => {
    // group.ts: used map, seen=0 -> no suffix, seen=1 -> _2
    const names = ["Metrics DB", "metrics db", "Metrics_DB", "Other"];
    const slugs = slugsWithCollision(names);
    expect(slugs).toEqual(["metrics_db", "metrics_db_2", "metrics_db_3", "other"]);
    // tool prefix includes __*
    expect(slugs.map((s) => `${s}__*`)).toEqual(["metrics_db__*", "metrics_db_2__*", "metrics_db_3__*", "other__*"]);
  });

  test("collision distinct names don't collide", () => {
    const slugs = slugsWithCollision(["Alpha", "Beta", "Gamma"]);
    expect(slugs).toEqual(["alpha", "beta", "gamma"]);
  });

  test("toolPrefix helper", () => {
    expect(toolPrefix("Metrics DB")).toBe("metrics_db__*");
    expect(toolPrefix("")).toBe("member__*");
  });
});

describe("health transition-only firing", () => {
  test("fires only on crossing, not steady state", () => {
    const prev = {
      a: { status: "ok" as const, at: 1 },
      b: { status: "error" as const, error: "down", at: 1 },
    };
    const nextSteady = {
      a: { status: "ok" as const, at: 2 },
      b: { status: "error" as const, error: "down", at: 2 },
    };
    expect(detectTransitions(prev, nextSteady)).toEqual([]);

    const nextToError = {
      a: { status: "error" as const, error: "refused", at: 2 },
      b: { status: "error" as const, error: "down", at: 2 },
    };
    const t1 = detectTransitions(prev, nextToError);
    expect(t1.length).toBe(1);
    expect(t1[0].integrationId).toBe("a");
    expect(t1[0].kind).toBe("to_error");

    const nextRecover = {
      a: { status: "ok" as const, at: 3 },
      b: { status: "ok" as const, at: 3 },
    };
    const prev2 = { a: { status: "error" as const, error: "refused", at: 2 }, b: { status: "error" as const, error: "down", at: 2 } };
    const t2 = detectTransitions(prev2, nextRecover);
    expect(t2.length).toBe(2);
    expect(t2.every((t) => t.kind === "to_ok")).toBe(true);
  });

  test("persistently failing never re-fires without recovery", () => {
    let health: Record<string, { status: "ok" | "error"; error?: string; at: number }> = {
      c: { status: "ok", at: 1 },
    };
    // first poll: c goes to error -> fires
    let next = { c: { status: "error" as const, error: "refused", at: 2 } };
    expect(detectTransitions(health, next).length).toBe(1);
    health = next;
    // second poll still error -> no fire
    next = { c: { status: "error" as const, error: "refused", at: 3 } };
    expect(detectTransitions(health, next).length).toBe(0);
    health = next;
    // third poll still error with different error text -> still no fire (still error)
    next = { c: { status: "error" as const, error: "timeout", at: 4 } };
    expect(detectTransitions(health, next).length).toBe(0);
  });

  test("unknown (absent) to error fires, unknown to ok does not", () => {
    const prev: Record<string, { status: "ok" | "error"; at: number }> = {};
    const next = { x: { status: "error" as const, error: "down", at: 1 } };
    expect(detectTransitions(prev, next).length).toBe(1);
    const nextOk = { x: { status: "ok" as const, at: 2 } };
    // absent -> ok should not fire (was not error)
    expect(detectTransitions(prev, nextOk).length).toBe(0);
  });
});

describe("empty states copy has no internal vocab", () => {
  const banned = ["owner", "manifest", "verdict", "projection", "slug"];
  for (const kind of ["no-integrations", "no-groups", "nothing-selected", "catalog-unavailable", "no-matches"] as const) {
    test(`${kind} has no banned words`, () => {
      const s = emptyState(kind, { query: "test" });
      const text = `${s.title} ${s.body} ${s.actionLabel ?? ""}`.toLowerCase();
      for (const w of banned) expect(text).not.toContain(w);
    });
  }
  test("first-run tells what to do", () => {
    const s = emptyState("no-integrations");
    expect(s.title.toLowerCase()).toContain("connect");
    expect(s.body.toLowerCase()).toContain("add");
    expect(s.actionLabel).toBe("New Integration");
  });
});

describe("error toast copy says what failed and what to try", () => {
  test("health error humanization", async () => {
    const { humanizeHealthError } = await import("./health");
    expect(humanizeHealthError("connection refused")).toContain("reachable");
    expect(humanizeHealthError("authentication failed")).toContain("credentials");
    expect(humanizeHealthError(null)).toContain("try again");
    // banned vocab not in humanized messages
    const banned = ["owner", "manifest", "verdict", "projection", "slug"];
    for (const raw of ["connection refused", "timeout", "ssh tunnel failed", null]) {
      const msg = humanizeHealthError(raw as string).toLowerCase();
      for (const w of banned) expect(msg).not.toContain(w);
    }
  });
});
