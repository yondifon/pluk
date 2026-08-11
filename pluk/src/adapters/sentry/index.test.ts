import { afterEach, expect, test } from "bun:test";
import { sentryAdapter, sentryTools } from "./index.js";
import { sentryConfig, sentryRequest, sentryRequestBytes } from "./client.js";
import { resolveIssueProject, resolveLatestEventId } from "./index.js";
import type { Integration } from "../../store/integrations.js";

function conn(config: Record<string, unknown>): Integration {
  return { id: "s", name: "Sentry", type: "sentry", config, read_only: 0, query_policy: null, token: "t", created_at: "" };
}

const realFetch = globalThis.fetch;
afterEach(() => { globalThis.fetch = realFetch; });

test("sentryConfig defaults base URL and reads auth + default project", () => {
  const cfg = sentryConfig(conn({ auth_token: "sntrys_x", org_slug: "acme", project_slug: "api" }));
  expect(cfg.baseUrl).toBe("https://sentry.io");
  expect(cfg.token).toBe("sntrys_x");
  expect(cfg.org).toBe("acme");
  expect(cfg.project).toBe("api");
});

test("sentryRequest appends repeated query params for Explore fields", async () => {
  let seen = "";
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    seen = String(input);
    return new Response(JSON.stringify({ data: [] }), { status: 200 });
  }) as unknown as typeof fetch;

  await sentryRequest(sentryConfig(conn({ auth_token: "t", org_slug: "acme" })), "GET", "/organizations/acme/events/", {
    dataset: "logs",
    field: ["timestamp", "message"],
  });

  expect(seen).toContain("dataset=logs");
  expect(seen).toContain("field=timestamp");
  expect(seen).toContain("field=message");
});

test("sentryRequestBytes returns attachment bytes and headers", async () => {
  globalThis.fetch = (async () =>
    new Response("hello", {
      status: 200,
      headers: { "content-type": "text/plain", "content-length": "5" },
    })) as unknown as typeof fetch;

  const res = await sentryRequestBytes(sentryConfig(conn({ auth_token: "t", org_slug: "acme" })), "GET", "/x/", { download: 1 });
  expect(new TextDecoder().decode(res.bytes)).toBe("hello");
  expect(res.contentType).toBe("text/plain");
  expect(res.contentLength).toBe("5");
});

test("resolveLatestEventId pulls the hex event id from the latest event", async () => {
  globalThis.fetch = (async () =>
    new Response(JSON.stringify({ eventID: "a1b2c3", title: "sign-in stalled" }), { status: 200 })) as unknown as typeof fetch;
  const cfg = sentryConfig(conn({ auth_token: "t", org_slug: "acme" }));
  expect(await resolveLatestEventId(cfg, "18945")).toBe("a1b2c3");
});

test("resolveIssueProject derives the project slug from the issue", async () => {
  globalThis.fetch = (async () =>
    new Response(JSON.stringify({ id: "18945", project: { slug: "browser-pool" } }), { status: 200 })) as unknown as typeof fetch;
  const cfg = sentryConfig(conn({ auth_token: "t", org_slug: "acme" }));
  expect(await resolveIssueProject(cfg, "18945")).toBe("browser-pool");
});

function tool(name: string) {
  const t = sentryTools(sentryConfig(conn({ auth_token: "t", org_slug: "acme" }))).find((t) => t.name === name);
  if (!t) throw new Error(`tool ${name} not found`);
  return t;
}

test("list_event_attachments defaults to the latest event and derives the project from the issue", async () => {
  const urls: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    urls.push(String(input));
    const s = String(input);
    if (s.includes("/events/latest/")) return new Response(JSON.stringify({ eventID: "a1b2c3" }), { status: 200 });
    if (s.includes("/organizations/acme/issues/")) return new Response(JSON.stringify({ project: { slug: "browser-pool" } }), { status: 200 });
    if (s.includes("/attachments/5/")) return new Response("file", { status: 200, headers: { "content-type": "text/html" } });
    return new Response(JSON.stringify([{ id: "5", name: "ms-us-prod.html", mimetype: "text/html", size: 4, dateCreated: "2026-08-06T22:00:00Z", event_id: "a1b2c3" }]), { status: 200 });
  }) as unknown as typeof fetch;

  const out = (await tool("list_event_attachments").run({ id: "18945", only: ["*"] }, {})) as Record<string, unknown>[];
  expect(out[0]?.project).toBe("browser-pool");
  expect(out[0]?.path).toEqual(expect.any(String));
  expect(String(out[0]?.path).startsWith("/")).toBe(true);
  expect(await Bun.file(String(out[0]?.path)).text()).toBe("file");
  expect(urls).toContain("https://sentry.io/api/0/organizations/acme/issues/18945/");
  expect(urls).toContain("https://sentry.io/api/0/issues/18945/events/latest/");
  expect(urls).toContain("https://sentry.io/api/0/projects/acme/browser-pool/events/a1b2c3/attachments/");
});

test("list_event_attachments uses a given event_id and project in one call", async () => {
  const urls: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    urls.push(String(input));
    return new Response(JSON.stringify([]), { status: 200 });
  }) as unknown as typeof fetch;

  await tool("list_event_attachments").run({ id: "18945", event_id: "abc123", project: "browser-pool" }, {});
  expect(urls).toEqual(["https://sentry.io/api/0/projects/acme/browser-pool/events/abc123/attachments/"]);
});

test("list_event_attachments reports a missing project clearly", async () => {
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const s = String(input);
    if (s.includes("/events/latest/")) return new Response(JSON.stringify({ eventID: "a1b2c3" }), { status: 200 });
    return new Response(JSON.stringify({}), { status: 200 });
  }) as unknown as typeof fetch;

  const cfg = sentryConfig(conn({ auth_token: "t", org_slug: "acme" }));
  const list = sentryTools(cfg).find((t) => t.name === "list_event_attachments")!;
  await expect(list.run({ id: "18945", event_id: "a1b2c3" }, {})).rejects.toThrow(/No project given/);
});

test("list_event_attachments reuses a complete cached file", async () => {
  let downloads = 0;
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = String(input);
    if (url.includes("/attachments/") && url.includes("/5/")) {
      downloads++;
      return new Response("file", { status: 200 });
    }
    return new Response(JSON.stringify([{ id: "5", name: "cache.txt", mimetype: "text/plain", size: 4 }]), { status: 200 });
  }) as unknown as typeof fetch;

  const args = { id: "18945", event_id: `cache-${crypto.randomUUID()}`, project: "browser-pool" };
  await tool("list_event_attachments").run(args, {});
  const out = (await tool("list_event_attachments").run(args, {})) as Record<string, unknown>[];
  expect(downloads).toBe(1);
  expect(out[0]?.path).toEqual(expect.any(String));
});

test("list_event_attachments returns successful downloads and errors for failures", async () => {
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = String(input);
    if (url.includes("/attachments/7/")) return new Response("no", { status: 500 });
    if (url.includes("/attachments/8/")) return new Response("ok!", { status: 200 });
    return new Response(JSON.stringify([
      { id: "7", name: "failed.png", mimetype: "image/png", size: 2 },
      { id: "8", name: "worked.log", mimetype: "text/plain", size: 3 },
    ]), { status: 200 });
  }) as unknown as typeof fetch;

  const out = (await tool("list_event_attachments").run({ id: "18945", event_id: "partial-event", project: "browser-pool" }, {})) as Record<string, unknown>[];
  expect(out[0]?.path).toBeNull();
  expect(out[0]?.error).toContain("Sentry API 500");
  expect(out[1]?.path).toEqual(expect.any(String));
  expect(out[1]?.error).toBeUndefined();
});

test("list_event_attachments saves an attachment whose body is longer than the listed size", async () => {
  const body = "<html>é</html>";
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    if (String(input).includes("/attachments/11/")) return new Response(body, { status: 200, headers: { "content-type": "text/html" } });
    return new Response(JSON.stringify([{ id: "11", name: "login.html", mimetype: "text/html", size: body.length }]), { status: 200 });
  }) as unknown as typeof fetch;

  const out = (await tool("list_event_attachments").run({ id: "18945", event_id: `wide-${crypto.randomUUID()}`, project: "browser-pool" }, {})) as Record<string, unknown>[];
  const bytes = new TextEncoder().encode(body).length;
  expect(bytes).toBeGreaterThan(body.length);
  expect(out[0]?.error).toBeUndefined();
  expect(out[0]?.path).toEqual(expect.any(String));
  expect(out[0]?.size).toBe(bytes);
  expect(out[0]?.warning).toContain(`Sentry listed ${body.length}`);
  expect(await Bun.file(String(out[0]?.path)).text()).toBe(body);
});

test("read_event_attachment rejects an empty download", async () => {
  globalThis.fetch = (async () => new Response("", { status: 200 })) as unknown as typeof fetch;

  await expect(tool("read_event_attachment").run(
    { project: "browser-pool", event_id: `empty-${crypto.randomUUID()}`, attachment_id: "12", name: "gone.bin", size: 900 },
    {},
  )).rejects.toThrow(/downloaded empty/);
});

test("attachment tool payloads contain paths, never attachment bytes", async () => {
  const body = "private attachment bytes";
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    if (String(input).includes("/attachments/9/")) return new Response(body, { status: 200, headers: { "content-type": "text/plain" } });
    return new Response(JSON.stringify([{ id: "9", name: "secret.txt", mimetype: "text/plain", size: body.length, content: body }]), { status: 200 });
  }) as unknown as typeof fetch;

  const out = await tool("list_event_attachments").run({ id: "18945", event_id: "payload-event", project: "browser-pool" }, {});
  expect(JSON.stringify(out)).not.toContain(body);
  expect((out as Record<string, unknown>[])[0]?.path).toEqual(expect.any(String));
});

test("read_event_attachment returns a local path instead of attachment content", async () => {
  const body = "binary-like content";
  globalThis.fetch = (async () => new Response(body, { status: 200 })) as unknown as typeof fetch;

  const out = await tool("read_event_attachment").run({ project: "browser-pool", event_id: "read-event", attachment_id: "10", name: "dump.bin", size: body.length }, {});
  expect(out).toEqual({ id: "10", name: "dump.bin", size: body.length, path: expect.any(String) });
  expect(JSON.stringify(out)).not.toContain(body);
});

test("sentryAdapter exposes issue, event, and log read tools", () => {
  expect(sentryAdapter.toolSpecs.map((t) => t.name)).toEqual([
    "list_projects",
    "list_issues",
    "get_issue",
    "latest_event",
    "list_event_attachments",
    "read_event_attachment",
    "list_events",
    "query_logs",
    "update_issue",
  ]);
});

test("testConnection rejects when the auth token is blank", async () => {
  await expect(sentryAdapter.testConnection(conn({ org_slug: "acme" }))).rejects.toThrow(/auth token is missing/);
});

// ── `only` field selection ───────────────────────────────────────────────────

test("list_projects defaults to a trimmed shape and drops access/features/has* flags", async () => {
  globalThis.fetch = (async () =>
    new Response(JSON.stringify([{
      id: "1", slug: "browser-pool", name: "Browser Pool", platform: "node",
      team: { slug: "backend" }, teams: [{ slug: "backend" }],
      environments: ["production"], access: ["a", "b"], features: ["f1"],
      isBookmarked: false, isMember: true, hasAccess: true,
      hasInsightsHttp: true, hasMinifiedStackTrace: false,
      dateCreated: "2026-01-01", firstEvent: null,
    }]), { status: 200 })) as unknown as typeof fetch;

  const out = (await tool("list_projects").run({}, {})) as Record<string, unknown>[];
  expect(out).toEqual([{ slug: "browser-pool", name: "Browser Pool", platform: "node", team: { slug: "backend" }, environments: ["production"] }]);
});

test("list_projects capabilities preset gathers features and every has* flag", async () => {
  globalThis.fetch = (async () =>
    new Response(JSON.stringify([{
      slug: "browser-pool", features: ["f1"], hasInsightsHttp: true, hasMinifiedStackTrace: false, isMember: true,
    }]), { status: 200 })) as unknown as typeof fetch;

  const out = (await tool("list_projects").run({ only: ["capabilities"] }, {})) as Record<string, unknown>[];
  expect(out).toEqual([{ features: ["f1"], hasInsightsHttp: true, hasMinifiedStackTrace: false }]);
});

test("list_issues drops the stats bucket arrays by default and exposes them via the stats preset", async () => {
  const issue = {
    shortId: "BACKEND-1A", title: "TypeError", culprit: "app.handler", level: "error", status: "unresolved",
    priority: "high", count: "42", userCount: 3, firstSeen: "2026-01-01", lastSeen: "2026-08-01",
    project: { slug: "backend" }, stats: { "24h": [[1, 2]], "30d": [[1, 2]] }, lifetime: { count: "100" },
    id: "999", permalink: "https://sentry.io/x",
  };
  globalThis.fetch = (async () => new Response(JSON.stringify([issue]), { status: 200 })) as unknown as typeof fetch;

  const defaults = (await tool("list_issues").run({ period: "14d", limit: 25 }, {})) as Record<string, unknown>[];
  expect(defaults[0]).toEqual({
    shortId: "BACKEND-1A", title: "TypeError", culprit: "app.handler", level: "error", status: "unresolved",
    priority: "high", count: "42", userCount: 3, firstSeen: "2026-01-01", lastSeen: "2026-08-01",
    project: { slug: "backend" },
  });

  globalThis.fetch = (async () => new Response(JSON.stringify([issue]), { status: 200 })) as unknown as typeof fetch;
  const withStats = (await tool("list_issues").run({ period: "14d", limit: 25, only: ["stats"] }, {})) as Record<string, unknown>[];
  expect(withStats[0]).toEqual({ stats: issue.stats, lifetime: issue.lifetime });
});

test("get_issue rejects an unrecognised only field and lists valid fields and presets", async () => {
  globalThis.fetch = (async () => new Response(JSON.stringify({ shortId: "X" }), { status: 200 })) as unknown as typeof fetch;
  await expect(tool("get_issue").run({ id: "1", only: ["bogus"] }, {})).rejects.toThrow(
    /Unknown "only" field "bogus"\. Valid fields: .*Presets: stats, tags, activity, releases\./,
  );
});

test("list_event_attachments only trims to name/size/mimetype/path/event_id by default", async () => {
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const s = String(input);
    if (s.includes("/attachments/5/")) return new Response("file", { status: 200 });
    return new Response(JSON.stringify([{ id: "5", name: "trace.log", mimetype: "text/plain", size: 4, dateCreated: "2026-01-01" }]), { status: 200 });
  }) as unknown as typeof fetch;

  const out = (await tool("list_event_attachments").run({ id: "18945", event_id: "abc", project: "browser-pool" }, {})) as Record<string, unknown>[];
  expect(Object.keys(out[0]!).sort()).toEqual(["error", "event_id", "mimetype", "name", "path", "size", "warning"]);
});

// Fixture shaped like the measured 105,842-char payload from issue 18212: 36
// frames (a handful in-app), 11 tags, breadcrumbs, packages, and _meta noise.
function latestEventFixture() {
  const frame = (i: number, inApp: boolean) => ({
    absPath: `/app/node_modules/pkg-${i}/dist/index.js`, colNo: 12, filename: inApp ? `src/handlers/handler-${i}.ts` : `node_modules/pkg-${i}/dist/index.js`,
    function: `fn${i}`, inApp, instructionAddr: "0x1a2b3c", lineNo: 100 + i, lock: null,
    module: `pkg-${i}`, package: null, platform: "node", rawFunction: `fn${i}`, sourceLink: null,
    symbol: null, symbolAddr: null, trust: "high",
    context: Array.from({ length: 8 }, (_, l) => [100 + i - 4 + l, `  const step${l} = doSomething(${l}); // padding padding padding padding`]),
    vars: { userId: "u_1234567890", payload: { a: 1, b: "x".repeat(80) }, retries: 3, largeBlob: "y".repeat(200) },
    errors: null,
  });
  const frames = Array.from({ length: 36 }, (_, i) => frame(i, i >= 30));
  return {
    eventID: "a1b2c3d4e5f6",
    dateCreated: "2026-08-01T00:00:00Z",
    title: "TypeError: Cannot read properties of undefined",
    culprit: "app.handlers.process",
    message: "Cannot read properties of undefined (reading 'id')",
    tags: Array.from({ length: 11 }, (_, i) => ({ key: `tag${i}`, value: `value${i}` })),
    contexts: {
      os: { name: "Linux", version: "5.15" },
      response: { status_code: 500, headers: Array.from({ length: 10 }, (_, i) => [`h${i}`, `v${i}`]) },
      runtime: { name: "node", version: "20.11.0" },
      trace: { trace_id: "abc123", span_id: "def456" },
    },
    user: { id: "u1", email: "a@b.com" },
    entries: [
      {
        type: "exception",
        data: { values: [{ type: "TypeError", value: "Cannot read properties of undefined (reading 'id')", module: "app.module", stacktrace: { frames } }] },
      },
      { type: "breadcrumbs", data: { values: Array.from({ length: 40 }, (_, i) => ({ timestamp: i, message: `step ${i}`, category: "http", level: "info" })) } },
    ],
    packages: Object.fromEntries(Array.from({ length: 30 }, (_, i) => [`pkg-${i}`, `1.0.${i}`])),
    _meta: { entries: { "0": { data: { values: { "0": { stacktrace: { frames: { "5": { context: { "0": ["", "s"] } } } } } } } } } },
    groupingConfig: { id: "newstyle:2023-01-11" },
    fingerprints: ["{{ default }}"],
  };
}

test("latest_event fits a normal tool response by default and keeps the debugging essentials", async () => {
  const raw = latestEventFixture();
  globalThis.fetch = (async () => new Response(JSON.stringify(raw), { status: 200 })) as unknown as typeof fetch;

  const out = (await tool("latest_event").run({ id: "18212" }, {})) as Record<string, unknown>;
  const text = JSON.stringify(out);
  expect(text.length).toBeLessThan(4000);

  const exception = out.exception as Record<string, unknown>[];
  expect(exception[0]!.type).toBe("TypeError");
  expect(out.message).toBe("Cannot read properties of undefined (reading 'id')");
  const frames = exception[0]!.frames as Record<string, unknown>[];
  expect(frames).toHaveLength(6); // only the inApp:true frames (indices 30-35)
  expect(frames[0]).toEqual({ filename: "src/handlers/handler-30.ts", function: "fn30", lineNo: 130, module: "pkg-30" });
  expect((out.tags as unknown[]).length).toBe(11);
  expect(out.entries).toBeUndefined();
  expect(out.packages).toBeUndefined();
  expect(out._meta).toBeUndefined();
});

test("latest_event only:['*'] returns the raw payload untouched", async () => {
  const raw = latestEventFixture();
  globalThis.fetch = (async () => new Response(JSON.stringify(raw), { status: 200 })) as unknown as typeof fetch;
  const out = await tool("latest_event").run({ id: "18212", only: ["*"] }, {});
  expect(out).toEqual(raw);
});

test("latest_event frames.full preset returns every frame with every key, unfiltered by inApp", async () => {
  const raw = latestEventFixture();
  globalThis.fetch = (async () => new Response(JSON.stringify(raw), { status: 200 })) as unknown as typeof fetch;
  const out = (await tool("latest_event").run({ id: "18212", only: ["frames.full"] }, {})) as Record<string, unknown>;
  const exception = out.exception as Record<string, unknown>[];
  const stacktrace = exception[0]!.stacktrace as { frames: Record<string, unknown>[] };
  expect(stacktrace.frames).toHaveLength(36);
  expect(stacktrace.frames[0]!.vars).toBeDefined();
  expect(stacktrace.frames[0]!.context).toBeDefined();
});

test("latest_event breadcrumbs preset adds the breadcrumbs entry", async () => {
  const raw = latestEventFixture();
  globalThis.fetch = (async () => new Response(JSON.stringify(raw), { status: 200 })) as unknown as typeof fetch;
  const out = (await tool("latest_event").run({ id: "18212", only: ["breadcrumbs"] }, {})) as Record<string, unknown>;
  expect((out.breadcrumbs as { values: unknown[] }).values).toHaveLength(40);
  expect(out.exception).toBeUndefined();
});
