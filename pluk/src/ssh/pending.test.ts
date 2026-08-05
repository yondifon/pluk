import { test, expect } from "bun:test";
import {
  connectWaitError,
  isSshAuthError,
  isSshPending,
  isSshStalled,
  recordConnectFailure,
} from "./pending.js";

test("auth failure from an earlier attempt beats the pending guess", () => {
  const key = "auth-episode";
  const authErr = new Error('sign_and_send_pubkey: signing failed for ED25519 "" from agent: communication with agent failed');
  recordConnectFailure(key, authErr);

  const reported = connectWaitError(key);
  expect(reported).toBe(authErr);
  expect(isSshPending(reported)).toBe(false);
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
