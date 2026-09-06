import { test, expect, afterAll } from "bun:test";
import {
  isSshAgentRetryableError,
  isSshFatalError,
  isSshRetryableError,
  withSshApprovalRetry,
  SSH_PENDING_CODE,
} from "./pending.js";

// Intercept setTimeout so tests don't incur real retry delays.
const realSetTimeout = globalThis.setTimeout;
const realClearTimeout = globalThis.clearTimeout;
const pending: Array<{ fn: () => void; ms: number }> = [];
globalThis.setTimeout = ((fn: () => void, ms?: number) => {
  pending.push({ fn, ms: ms ?? 0 });
  return 0 as unknown as ReturnType<typeof setTimeout>;
}) as unknown as typeof setTimeout;
globalThis.clearTimeout = (() => {}) as unknown as typeof clearTimeout;

afterAll(() => {
  globalThis.setTimeout = realSetTimeout;
  globalThis.clearTimeout = realClearTimeout;
});

function flushTimers(): void {
  while (pending.length > 0) pending.shift()!.fn();
}

function pendingErr(): Error {
  const e = new Error("SSH connect is still running");
  (e as Error & { code: string }).code = SSH_PENDING_CODE;
  return e;
}

function deniedErr(): Error {
  const e = new Error("Your SSH agent refused to sign.");
  (e as Error & { code: string; hint: string }).code = "SSH_AGENT_DENIED";
  (e as Error & { hint: string }).hint = "Check 1Password for a pending approval, or unlock it, then retry.";
  return e;
}

async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

test("SSH_CONNECT_PENDING then success returns the success", async () => {
  let calls = 0;
  const promise = withSshApprovalRetry(async () => {
    calls++;
    if (calls === 1) throw pendingErr();
    return "ok";
  });
  await settle();
  flushTimers();
  const result = await promise;
  expect(result).toBe("ok");
  expect(calls).toBe(2);
});

test("SSH_AGENT_DENIED then success returns the success", async () => {
  let calls = 0;
  const promise = withSshApprovalRetry(async () => {
    calls++;
    if (calls === 1) throw deniedErr();
    return "ok";
  });
  await settle();
  flushTimers();
  const result = await promise;
  expect(result).toBe("ok");
  expect(calls).toBe(2);
});

test("exhausting retries reports attempt count and preserves code and hint", async () => {
  let calls = 0;
  const promise = withSshApprovalRetry(async () => {
    calls++;
    throw deniedErr();
  }).catch((e) => e as Error & { code?: string; hint?: string });

  await settle();
  flushTimers();
  await settle();
  flushTimers();

  const err = await promise;
  expect(calls).toBe(3);
  expect(err.message).toMatch(/retried 2 times/);
  expect(err.message).toMatch(/no further automatic retry/);
  expect(err.code).toBe("SSH_AGENT_DENIED");
  expect(err.hint).toMatch(/1Password/);
});

test("unrelated error is not retried", async () => {
  let calls = 0;
  const err = await withSshApprovalRetry(async () => {
    calls++;
    throw new Error("Permission denied (publickey).");
  }).catch((e) => e as Error);

  expect(calls).toBe(1);
  expect(err.message).toBe("Permission denied (publickey).");
  expect(pending.length).toBe(0);
});

test("SSH_CONNECT_STALLED is not retried", async () => {
  let calls = 0;
  const err = await withSshApprovalRetry(async () => {
    calls++;
    const e = new Error("SSH connection never came up");
    (e as Error & { code: string }).code = "SSH_CONNECT_STALLED";
    throw e;
  }).catch((e) => e as Error);

  expect(calls).toBe(1);
  expect(err.message).toBe("SSH connection never came up");
  expect(pending.length).toBe(0);
});

test("alternating DENIED/PENDING/DENIED exhausts retries and reports correctly", async () => {
  const errors = [deniedErr(), pendingErr(), deniedErr()];
  let i = 0;
  const promise = withSshApprovalRetry(async () => {
    const e = errors[i++];
    if (e) throw e;
    return "ok";
  }).catch((e: unknown) => e as Error & { code?: string });

  await settle();
  flushTimers();
  await settle();
  flushTimers();

  const err = await promise;
  expect(i).toBe(3);
  expect((err as Error & { code?: string }).code).toBe("SSH_AGENT_DENIED");
  expect((err as Error).message).toMatch(/retried 2 times/);
});

test("agent states retry while host and policy failures fail fast", () => {
  for (const message of [
    "SSH_AGENT_UNREACHABLE",
    "signing failed: agent refused operation",
    "communication with agent failed",
  ]) {
    expect(isSshAgentRetryableError(new Error(message))).toBe(true);
    expect(isSshRetryableError(new Error(message))).toBe(true);
  }
  for (const message of [
    "Host key verification failed.",
    "Could not resolve hostname missing.example: unknown host",
    "channel open failed: administratively prohibited",
    "Permission denied (publickey).",
  ]) {
    expect(isSshFatalError(new Error(message))).toBe(true);
    expect(isSshRetryableError(new Error(message))).toBe(false);
  }
});
