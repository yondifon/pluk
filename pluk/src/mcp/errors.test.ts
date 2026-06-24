import { test, expect } from "bun:test";
import { humanizeConnError } from "./errors.js";

// The real failure from the field: 1Password agent locked while a cloudflared
// tunnel tries to authenticate. Must point the user at unlocking the agent, not
// leak the raw "sign_and_send_pubkey" string.
test("agent locked → actionable 1Password message", () => {
  const raw = 'sign_and_send_pubkey: signing failed for ED25519 "" from agent: communication with agent failed\nmalico@host: Permission denied (publickey).';
  const out = humanizeConnError(new Error(raw));
  expect(out).toMatch(/1Password|unlocked/i);
  expect(out).not.toMatch(/sign_and_send_pubkey/);
});

// Flaky Cloudflare Access tunnel — the strings seen in pluk.log.
test.each([
  "read tcp 1.2.3.4:1->5.6.7.8:443: read: connection reset by peer",
  "unexpected EOF",
  "ssh process exited before tunnel was ready",
])("dropped proxy tunnel → retry/Cloudflare hint: %s", (raw) => {
  expect(humanizeConnError(new Error(raw))).toMatch(/proxy|Cloudflare|retry/i);
});

test("postgres auth code → credentials message", () => {
  const err = Object.assign(new Error("SASL authentication failed"), { code: "08P01" });
  expect(humanizeConnError(err)).toMatch(/username and password/i);
});

test("unknown error is kept but points at logs", () => {
  expect(humanizeConnError(new Error("boom"))).toBe("boom (see Logs for details)");
});
