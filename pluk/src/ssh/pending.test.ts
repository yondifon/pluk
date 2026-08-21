import { test, expect } from "bun:test";
import {
  connectWaitError,
  isSshAuthError,
  isSshPending,
  isSshStalled,
  recordConnectFailure,
  startConnectAttempt,
} from "./pending.js";

test("auth failure from an earlier attempt beats the pending guess", () => {
  const key = "auth-episode";
  const authErr = new Error('sign_and_send_pubkey: signing failed for ED25519 "" from agent: communication with agent failed');
  recordConnectFailure(key, authErr);

  const reported = connectWaitError(key);
  expect(reported).toBe(authErr);
  expect(isSshPending(reported)).toBe(false);
});

// Regression: an agent refusal belongs to the attempt that made the request.
// Replaying it at a caller waiting on a later attempt reported a denial no
// agent had given — instantly, with no prompt, since no ssh had run yet.
test("an earlier attempt's auth failure never answers a caller waiting on a new one", () => {
  const key = "retried-episode";
  recordConnectFailure(key, new Error('sign_and_send_pubkey: signing failed for ED25519 "" from agent: agent refused operation'));

  startConnectAttempt(key);

  expect(isSshPending(connectWaitError(key))).toBe(true);
});

test("no recorded failure -> pending twice, then stalled", () => {
  const key = "slow-episode";
  expect(isSshPending(connectWaitError(key))).toBe(true);
  expect(isSshPending(connectWaitError(key))).toBe(true);
  expect(isSshStalled(connectWaitError(key))).toBe(true);
});

test("non-auth failure still rations pending reports", () => {
  const key = "flaky-episode";
  recordConnectFailure(key, new Error("connect ETIMEDOUT"));
  expect(isSshPending(connectWaitError(key))).toBe(true);
});

test("isSshAuthError matches agent and pubkey failures only", () => {
  expect(isSshAuthError(new Error("communication with agent failed"))).toBe(true);
  expect(isSshAuthError(new Error("malico@host: Permission denied (publickey)."))).toBe(true);
  expect(isSshAuthError(new Error("Can't reach your SSH key agent — no agent socket answered"))).toBe(true);
  expect(isSshAuthError(new Error("connect ETIMEDOUT"))).toBe(false);
});
