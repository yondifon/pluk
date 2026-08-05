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
  lastError?: Error;
}

const episodes = new Map<string, Episode>();

function episode(key: string): Episode {
  const existing = episodes.get(key);
  if (existing) return existing;
  const fresh: Episode = { pendingReports: 0 };
  episodes.set(key, fresh);
  return fresh;
}

/** Connect landed (or the user forced a refresh) — forget the run of attempts. */
export function clearConnectEpisode(key: string): void {
  episodes.delete(key);
}

/** Remember why an attempt failed, so a later stall can report a real cause. */
export function recordConnectFailure(key: string, err: unknown): void {
  episode(key).lastError = err instanceof Error ? err : new Error(String(err));
}

// Auth and agent failures are deterministic: a locked or unreachable agent and
// a rejected pubkey won't clear while a caller waits, so "still connecting,
// maybe approving" must never mask one that already happened.
export function isSshAuthError(err: unknown): boolean {
  const msg = (err as { message?: string } | null)?.message ?? "";
  return /permission denied|communication with agent failed|signing failed|publickey|no supported authentication|authentication failed|too many authentication failures|SSH key agent/i.test(msg);
}

// Error for a caller whose bounded wait on an in-flight connect ran out. An
// earlier attempt that died on auth is the real story — report it, not the
// pending guess. Otherwise: the guess while it's still plausible, then the
// last real failure.
export function connectWaitError(key: string): Error {
  const ep = episode(key);
  if (ep.lastError && isSshAuthError(ep.lastError)) {
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
