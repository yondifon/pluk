// Shared handling for SSH connects that take longer than a tool call should
// wait. A caller waits SSH_CONNECT_WAIT_MS on an in-flight connect, then gets
// this error while the connect keeps running in the background — so an approval
// the user still has to give can land, and the next retry finds it connected.
//
// Why the connect is slow is NOT knowable from here: from the outside, a connect
// blocked on an agent approval looks exactly like one hanging on a dead tunnel
// or an unreachable host. So this message describes the state (still connecting)
// and names an approval only as one possibility — it must never send the user
// hunting for a prompt that was never shown. The report is also rationed per
// connect episode — the run of attempts for one pool key that has yet to produce
// a working connection. After SSH_PENDING_MAX_REPORTS answers with nothing
// connecting, callers get the last real connect error instead.

export const SSH_PENDING_CODE = "SSH_CONNECT_PENDING";
export const SSH_STALLED_CODE = "SSH_CONNECT_STALLED";

export const SSH_CONNECT_WAIT_MS = 25_000;
export const SSH_PENDING_MAX_REPORTS = 2;

interface Episode {
  pendingReports: number;
  attemptSeq: number;
  lastError?: Error;
  lastErrorSeq: number;
}

const episodes = new Map<string, Episode>();

// Program-order stamps, so "recorded before this attempt started" is exact —
// a wall clock can put a failure and the next attempt in the same millisecond.
let seq = 0;

function episode(key: string): Episode {
  const existing = episodes.get(key);
  if (existing) return existing;
  const fresh: Episode = { pendingReports: 0, attemptSeq: 0, lastErrorSeq: 0 };
  episodes.set(key, fresh);
  return fresh;
}

/** Connect landed (or the user forced a refresh) — forget the run of attempts. */
export function clearConnectEpisode(key: string): void {
  episodes.delete(key);
}

/** A fresh connect is starting for this key. Failures from the attempts it
 *  replaces stay on record for the stall report, but can no longer answer a
 *  caller waiting on this one — an agent that refused an hour ago says nothing
 *  about the request this attempt is about to make. */
export function startConnectAttempt(key: string): void {
  episode(key).attemptSeq = ++seq;
}

/** Remember why an attempt failed, so a later stall can report a real cause. */
export function recordConnectFailure(key: string, err: unknown): void {
  const ep = episode(key);
  ep.lastError = err instanceof Error ? err : new Error(String(err));
  ep.lastErrorSeq = ++seq;
}

// Authentication failures after the agent is available are deterministic.
export function isSshAuthError(err: unknown): boolean {
  const msg = (err as { message?: string } | null)?.message ?? "";
  return /permission denied|publickey|no supported authentication|authentication failed|too many authentication failures/i.test(msg);
}

export function isSshHostVerificationError(err: unknown): boolean {
  const msg = (err as { message?: string } | null)?.message ?? "";
  return /host key verification failed|remote host identification has changed|could not resolve hostname|name or service not known|nodename nor servname provided|unknown host/i.test(msg);
}

export function isSshPolicyError(err: unknown): boolean {
  const msg = (err as { message?: string } | null)?.message ?? "";
  return /administratively prohibited|channel open failed: prohibited|operation not permitted/i.test(msg);
}

export function isSshAgentRetryableError(err: unknown): boolean {
  const msg = (err as { message?: string } | null)?.message ?? "";
  return /communication with agent failed|signing failed|agent refused operation|SSH key agent|SSH_AGENT_UNREACHABLE|agent unreachable|could not connect to agent|approval/i.test(msg);
}

export function isSshFatalError(err: unknown): boolean {
  return isSshAuthError(err) || isSshHostVerificationError(err) || isSshPolicyError(err);
}

export function isSshRetryableError(err: unknown): boolean {
  return !isSshFatalError(err) || isSshAgentRetryableError(err);
}

// Error for a caller whose bounded wait on an in-flight connect ran out. This
// attempt dying on auth is the real story — report it, not the pending guess.
// Otherwise: the guess while it's still plausible, then the last real failure.
export function connectWaitError(key: string): Error {
  const ep = episode(key);
  const fromThisAttempt = ep.lastErrorSeq >= ep.attemptSeq;
  if (ep.lastError && fromThisAttempt && isSshAuthError(ep.lastError)) {
    episodes.delete(key);
    return ep.lastError;
  }
  ep.pendingReports++;
  if (ep.pendingReports <= SSH_PENDING_MAX_REPORTS) return sshPendingError();
  episodes.delete(key);
  return sshStalledError(ep.lastError);
}

function coded(code: string, message: string): Error {
  const err = new Error(message);
  (err as Error & { code: string }).code = code;
  return err;
}

export function sshPendingError(): Error {
  return coded(
    SSH_PENDING_CODE,
    "SSH connect is still running — authenticating, or waiting on an SSH agent or proxy approval. It continues in the background; retry in a moment. If it keeps repeating, check for a pending agent (e.g. 1Password) prompt."
  );
}

export function sshStalledError(lastError?: Error): Error {
  const detail = lastError?.message ? ` Last connect error: ${lastError.message}` : "";
  return coded(
    SSH_STALLED_CODE,
    `SSH connection never came up and no approval landed after ${SSH_PENDING_MAX_REPORTS} attempts.${detail}`
  );
}

export function isSshPending(err: unknown): boolean {
  return (err as { code?: string } | null)?.code === SSH_PENDING_CODE;
}

export function isSshStalled(err: unknown): boolean {
  return (err as { code?: string } | null)?.code === SSH_STALLED_CODE;
}

// Both codes are transient: an approval is in flight or the agent has not yet
// responded. They are not permanent failures — the operation succeeds on the
// next attempt once the approval lands. All other codes are deterministic and
// must not be retried.
export function isTransientSshError(err: unknown): boolean {
  const code = (err as { code?: string } | null)?.code;
  return code === SSH_PENDING_CODE || code === "SSH_AGENT_DENIED";
}

// Retry delays: 3s then 6s. Human approval takes at least a few seconds
// (reaching for a phone or 1Password), so a shorter first delay is useless.
// Two retries cover the alternating DENIED/PENDING pattern seen in practice.
// Total wait (~9s) stays well inside the 25s connect budget and any caller
// timeout. Back off rather than hammer: each attempt can itself trigger a
// new agent prompt.
const RETRY_DELAYS_MS = [3_000, 6_000];

export async function withSshApprovalRetry<T>(fn: () => Promise<T>): Promise<T> {
  const start = Date.now();
  for (let attempt = 0; attempt <= RETRY_DELAYS_MS.length; attempt++) {
    try {
      return await fn();
    } catch (err) {
      if (!isTransientSshError(err)) throw err;
      if (attempt < RETRY_DELAYS_MS.length) {
        await new Promise<void>((res) => setTimeout(res, RETRY_DELAYS_MS[attempt]!));
        continue;
      }
      const elapsed = Math.round((Date.now() - start) / 1000);
      const original = err as { message?: string; code?: string; hint?: string };
      const tagged = new Error(
        `${original.message ?? String(err)} (retried ${attempt} time${attempt === 1 ? "" : "s"} over ${elapsed}s — no further automatic retry)`
      );
      (tagged as typeof tagged & { code?: string }).code = original.code;
      (tagged as typeof tagged & { hint?: string }).hint = original.hint;
      throw tagged;
    }
  }
  throw new Error("unreachable");
}
