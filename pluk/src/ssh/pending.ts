// Shared handling for SSH connects that are blocked on an interactive approval
// (1Password confirm, agent unlock, proxy browser login). A tool call waits
// SSH_CONNECT_WAIT_MS on an in-flight connect, then surfaces this error while
// the connect keeps running in the background — so the user's approval still
// lands and the next retry succeeds instantly. A connect still pending after
// SSH_CONNECT_RESPAWN_MS is doomed (its prompt expired unseen): callers kill it
// and spawn a fresh attempt, which triggers a fresh prompt.
//
// "Waiting for approval" is a guess: from the outside, a connect blocked on a
// 1Password prompt looks exactly like one hanging on a dead tunnel or an
// unreachable host. So the guess is rationed per connect episode — the run of
// attempts for one pool key that has yet to produce a working connection. After
// SSH_PENDING_MAX_REPORTS pending answers with nothing connecting, the guess is
// wrong: callers get the last real connect error, the doomed attempt is torn
// down, and the next call starts a brand-new connection instead of the pool
// answering "waiting for approval" forever.

export const SSH_PENDING_CODE = "SSH_CONNECT_PENDING";
export const SSH_STALLED_CODE = "SSH_CONNECT_STALLED";

export const SSH_CONNECT_WAIT_MS = 25_000;
export const SSH_CONNECT_RESPAWN_MS = 60_000;
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

// Error for a caller whose bounded wait on an in-flight connect ran out: the
// pending guess while it's still plausible, then the real failure.
export function connectWaitError(key: string): Error {
  const ep = episode(key);
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
    "SSH connection is waiting for approval (1Password/SSH agent prompt or proxy login). Approve it, then retry — connecting continues in the background."
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
