import { test, expect } from "bun:test";
import { redisConfig, buildUrl } from "./client.js";
import { redisAdapter } from "./index.js";
import type { Integration } from "../../store/integrations.js";

function conn(config: Record<string, unknown>): Integration {
  return { id: "r", name: "Redis", type: "redis", config, read_only: 0, query_policy: null, token: "t", created_at: "" };
}

test("buildUrl encodes the password and picks the scheme", () => {
  expect(buildUrl("redis", "localhost", 6379, 0, "")).toBe("redis://localhost:6379/0");
  expect(buildUrl("rediss", "h", 6380, 2, "p@ss")).toBe("rediss://:p%40ss@h:6380/2");
});

test("redisConfig reads host/port/db/tls/password", () => {
  const cfg = redisConfig(conn({ host: "h", port: 6380, db: 2, tls: true, password: "p" }));
  expect(cfg).toMatchObject({ host: "h", port: 6380, db: 2, tls: true, password: "p" });
  expect(cfg.ssh).toBeUndefined();
});

test("redisConfig prefers an explicit url (managed providers) when no tunnel", () => {
  expect(redisConfig(conn({ url: "rediss://x.upstash.io:6379", host: "ignored" })).url).toBe("rediss://x.upstash.io:6379");
});

test("redisConfig parses the SSH tunnel block when use_ssh is on", () => {
  const cfg = redisConfig(conn({
    host: "127.0.0.1", port: 6379,
    use_ssh: true, ssh_host: "bastion", ssh_port: 2222, ssh_user: "deploy", ssh_auth_type: "key", ssh_key_path: "~/.ssh/id_ed25519",
  }));
  expect(cfg.ssh).toMatchObject({ host: "bastion", port: 2222, user: "deploy", authType: "key", keyPath: "~/.ssh/id_ed25519" });
});

test("redisConfig ignores an explicit url when a tunnel is configured (tunnel needs host/port)", () => {
  const cfg = redisConfig(conn({ url: "rediss://x", host: "10.0.0.5", port: 6379, use_ssh: true, ssh_host: "bastion" }));
  expect(cfg.url).toBeUndefined();
  expect(cfg.host).toBe("10.0.0.5");
  expect(cfg.ssh?.host).toBe("bastion");
});

test("redisConfig rejects a missing host", () => {
  expect(() => redisConfig(conn({}))).toThrow(/host is missing/);
});

test("testConnection rejects when the host is blank (before opening a client)", async () => {
  await expect(redisAdapter.testConnection(conn({}))).rejects.toThrow(/host is missing/);
});
