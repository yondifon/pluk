import { test, expect } from "bun:test";
import { classifySqlError, humanizeSqlError } from "./errors.js";

test("agent locked -> actionable auth_failed message", () => {
  const raw = 'sign_and_send_pubkey: signing failed for ED25519 "" from agent: communication with agent failed\nmalico@host: Permission denied (publickey).';
  const out = classifySqlError(new Error(raw));

  expect(out.category).toBe("auth_failed");
  expect(out.hint).toMatch(/1Password|ssh-agent/i);
  expect(humanizeSqlError(new Error(raw))).not.toMatch(/sign_and_send_pubkey/);
});

test.each([
  "read tcp 1.2.3.4:1->5.6.7.8:443: read: connection reset by peer",
  "unexpected EOF",
  "ssh process exited before tunnel was ready",
])("dropped proxy tunnel -> tunnel_failed: %s", (raw) => {
  expect(classifySqlError(new Error(raw)).category).toBe("tunnel_failed");
});

test("postgres auth code -> auth_failed", () => {
  const err = Object.assign(new Error("SASL authentication failed"), { code: "08P01" });
  expect(classifySqlError(err).category).toBe("auth_failed");
});

test("unknown error is query_failed", () => {
  expect(classifySqlError(new Error("boom"))).toMatchObject({ category: "query_failed", message: "boom" });
});
