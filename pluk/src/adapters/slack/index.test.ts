import { test, expect, afterEach } from "bun:test";
import { slackConfig, slackRequest, resolveChannel } from "./client.js";
import { slackAdapter } from "./index.js";
import type { Integration } from "../../store/integrations.js";

function conn(config: Record<string, unknown>): Integration {
  return { id: "s", name: "Slack", type: "slack", config, read_only: 0, query_policy: null, token: "t", created_at: "" };
}

const realFetch = globalThis.fetch;
afterEach(() => { globalThis.fetch = realFetch; });

test("slackConfig reads the bot token + default channel and rejects a blank token", () => {
  const cfg = slackConfig(conn({ bot_token: "xoxb-1", default_channel: "C1" }));
  expect(cfg.token).toBe("xoxb-1");
  expect(cfg.defaultChannel).toBe("C1");
  expect(() => slackConfig(conn({}))).toThrow(/bot token is missing/);
});

test("resolveChannel uses the arg, falls back to default, and errors when neither is set", () => {
  const cfg = slackConfig(conn({ bot_token: "x", default_channel: "C1" }));
  expect(resolveChannel(cfg, "C2")).toBe("C2");
  expect(resolveChannel(cfg, undefined)).toBe("C1");
  expect(() => resolveChannel(slackConfig(conn({ bot_token: "x" })), undefined)).toThrow(/No channel given/);
});

test("slackRequest throws on ok:false so the gated runner logs it as an error", async () => {
  globalThis.fetch = (async () =>
    new Response(JSON.stringify({ ok: false, error: "invalid_auth" }), { status: 200 })) as unknown as typeof fetch;
  await expect(slackRequest(slackConfig(conn({ bot_token: "bad" })), "auth.test", {})).rejects.toThrow(/auth\.test: invalid_auth/);
});

test("testConnection rejects when the bot token is blank", async () => {
  await expect(slackAdapter.testConnection(conn({}))).rejects.toThrow(/bot token is missing/);
});
