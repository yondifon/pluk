import { afterEach, expect, test } from "bun:test";
import { sentryAdapter, sentryTools } from "./index.js";
import { sentryConfig, sentryRequest, sentryRequestText } from "./client.js";
import { formatTextChunk, resolveIssueProject, resolveLatestEventId } from "./index.js";
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

test("sentryRequestText returns the raw body plus content type and length", async () => {
  globalThis.fetch = (async () =>
    new Response("<html>hello</html>", {
      status: 200,
      headers: { "content-type": "text/html; charset=utf-8", "content-length": "18" },
    })) as unknown as typeof fetch;

  const res = await sentryRequestText(sentryConfig(conn({ auth_token: "t", org_slug: "acme" })), "GET", "/x/", { download: 1 });
  expect(res.text).toBe("<html>hello</html>");
  expect(res.contentType).toBe("text/html; charset=utf-8");
  expect(res.contentLength).toBe("18");
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

test("formatTextChunk cuts long text with an explicit marker and a resume offset", () => {
  const body = "a".repeat(5000);
  const out = formatTextChunk(body, 0, 20);
  expect(out).toContain("[…truncated: showing characters 1–20 of 5000.");
  expect(out).toContain("Read on with offset=20.");
  expect(formatTextChunk(body, 4980, 100)).toBe("a".repeat(20));
  expect(formatTextChunk(body, 5000, 100)).toBe("Offset 5000 is past the end of 5000 characters.");
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
    return new Response(
      JSON.stringify([{ id: "5", name: "ms-us-prod.html", mimetype: "text/html", size: 4000, dateCreated: "2026-08-06T22:00:00Z", event_id: "a1b2c3" }]),
      { status: 200 },
    );
  }) as unknown as typeof fetch;

  const out = (await tool("list_event_attachments").run({ id: "18945" }, {})) as Record<string, unknown>[];
  expect(out[0]?.project).toBe("browser-pool");
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

test("read_event_attachment returns text with a truncation marker", async () => {
  globalThis.fetch = (async () =>
    new Response("a".repeat(5000), { status: 200, headers: { "content-type": "text/html" } })) as unknown as typeof fetch;

  const out = await tool("read_event_attachment").run({ project: "browser-pool", event_id: "a1b2c3", attachment_id: "5", limit: 20, offset: 0 }, {});
  expect(out).toContain("Attachment #5 (text/html, 5000 characters)");
  expect(out).toContain("[…truncated: showing characters 1–20 of 5000.");
});

test("read_event_attachment refuses non-text attachments with a clear message", async () => {
  globalThis.fetch = (async () =>
    new Response("not really an image", { status: 200, headers: { "content-type": "image/png", "content-length": "999" } })) as unknown as typeof fetch;

  const out = await tool("read_event_attachment").run({ project: "browser-pool", event_id: "a1b2c3", attachment_id: "7" }, {});
  expect(out).toBe("Attachment #7 is image/png (999 bytes) — not text, contents cannot be shown.");
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
