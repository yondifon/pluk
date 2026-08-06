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

test("agent-unreachable code maps to auth_failed even without a matching message", () => {
  const err = Object.assign(new Error("anything"), { code: "SSH_AGENT_UNREACHABLE" });
  expect(classifySqlError(err)).toMatchObject({ category: "auth_failed", code: "SSH_AGENT_UNREACHABLE" });
});

test("agent refused operation -> distinct SSH_AGENT_DENIED code", () => {
  const raw = "sign_and_send_pubkey: signing failed: agent refused operation";
  expect(classifySqlError(new Error(raw))).toMatchObject({ category: "auth_failed", code: "SSH_AGENT_DENIED" });
});

test.each([
  "No reply from server",
  'sign_and_send_pubkey: could not open a connection to your authentication agent',
])("genuinely unreachable agent keeps SSH_AGENT_UNREACHABLE: %s", (raw) => {
  expect(classifySqlError(new Error(raw))).toMatchObject({ category: "auth_failed", code: "SSH_AGENT_UNREACHABLE" });
});

test.each([
  new Error('sign_and_send_pubkey: signing failed for ED25519 "" from agent: communication with agent failed'),
  new Error("malico@host: Permission denied (publickey)."),
  new Error("unexpected EOF"),
  new Error("Timed out after 30s (connect)"),
  new Error("boom"),
])("every classification carries a code: %s", (err) => {
  expect(classifySqlError(err).code).toBeTruthy();
});
