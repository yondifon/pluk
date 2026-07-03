import { afterEach, expect, test } from "bun:test";
import { sentryAdapter } from "./index.js";
import { sentryConfig, sentryRequest } from "./client.js";
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

test("sentryAdapter exposes issue, event, and log read tools", () => {
  expect(sentryAdapter.toolSpecs.map((t) => t.name)).toEqual([
    "list_projects",
    "list_issues",
    "get_issue",
    "latest_event",
    "list_events",
    "query_logs",
    "update_issue",
  ]);
});

test("testConnection rejects when the auth token is blank", async () => {
  await expect(sentryAdapter.testConnection(conn({ org_slug: "acme" }))).rejects.toThrow(/auth token is missing/);
});
