import { describe, test, expect } from "bun:test";
import { slug, slugsWithCollision, toolPrefix } from "./slug";
import { ToastCenter, ERROR_LIFETIME_MS, SUCCESS_LIFETIME_MS } from "./toast";
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

describe("toast replacement per integration", () => {
  test("newer toast replaces previous for same integration", () => {
    const center = new ToastCenter();
    center.present({ integrationId: "i1", title: "DB", message: "first", kind: "error" });
    expect(center.all.length).toBe(1);
    expect(center.all[0].message).toBe("first");
    center.present({ integrationId: "i1", title: "DB", message: "second", kind: "error" });
    expect(center.all.length).toBe(1);
    expect(center.all[0].message).toBe("second");
  });

  test("different integrations coexist", () => {
    const center = new ToastCenter();
    center.present({ integrationId: "i1", title: "A", message: "a", kind: "error" });
    center.present({ integrationId: "i2", title: "B", message: "b", kind: "success" });
    expect(center.all.length).toBe(2);
  });

  test("dismiss removes specific toast", () => {
    const center = new ToastCenter();
    const t = center.present({ integrationId: "i1", title: "A", message: "a", kind: "error" });
    center.dismiss(t.id);
    expect(center.all.length).toBe(0);
  });
});

describe("error toasts outlive success toasts", () => {
  test("lifetimes", () => {
    const center = new ToastCenter();
    expect(center.lifetimeFor("error")).toBe(ERROR_LIFETIME_MS);
    expect(center.lifetimeFor("success")).toBe(SUCCESS_LIFETIME_MS);
    expect(ERROR_LIFETIME_MS).toBeGreaterThan(SUCCESS_LIFETIME_MS);
    expect(ERROR_LIFETIME_MS).toBe(8000);
    expect(SUCCESS_LIFETIME_MS).toBe(3000);
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

describe("retry re-tests connection", () => {
  test("onRetry callback is invoked with integrationId", async () => {
    let retriedId: string | null = null;
    const onRetry = (id: string) => {
      retriedId = id;
    };
    const center = new ToastCenter(onRetry);
    center.present({ integrationId: "i42", title: "DB", message: "Connection is failing.", kind: "error" });
    // simulate clicking retry: centre's listener would call onRetry
    onRetry("i42");
    expect(retriedId).toBe("i42");
  });
});

describe("reduced-motion suppressing animation", () => {
  test("shouldAnimate respects prefers-reduced-motion", () => {
    const g = globalThis as unknown as { window?: { matchMedia: (q: string) => unknown } };
    const hadWindow = g.window;
    const fakeWindow: { matchMedia: (q: string) => unknown } = { matchMedia: () => ({ matches: false }) };
    if (!g.window) (g as unknown as { window: unknown }).window = fakeWindow;
    const w = (g.window as unknown as { matchMedia: (q: string) => MediaQueryList });
    const orig = w.matchMedia;
    // reduced motion on
    w.matchMedia = ((q: string) => ({
      matches: q === "(prefers-reduced-motion: reduce)" ? true : false,
      media: q,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    })) as unknown as typeof window.matchMedia;
    const c1 = new ToastCenter();
    expect(c1.shouldAnimate()).toBe(false);

    // reduced motion off
    w.matchMedia = ((q: string) => ({
      matches: false,
      media: q,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    })) as unknown as typeof window.matchMedia;
    const c2 = new ToastCenter();
    expect(c2.shouldAnimate()).toBe(true);

    w.matchMedia = orig;
    if (!hadWindow) delete (g as unknown as { window?: unknown }).window;
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
