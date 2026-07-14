// Shared handling for SSH connects that are blocked on an interactive approval
// (1Password confirm, agent unlock, proxy browser login). A tool call waits
// SSH_CONNECT_WAIT_MS on an in-flight connect, then surfaces this error while
// the connect keeps running in the background — so the user's approval still
// lands and the next retry succeeds instantly. A connect still pending after
// SSH_CONNECT_RESPAWN_MS is doomed (its prompt expired unseen): callers kill it
// and spawn a fresh attempt, which triggers a fresh prompt.

export const SSH_PENDING_CODE = "SSH_CONNECT_PENDING";

export const SSH_CONNECT_WAIT_MS = 25_000;
export const SSH_CONNECT_RESPAWN_MS = 90_000;

export function sshPendingError(): Error {
  const err = new Error(
    "SSH connection is waiting for approval (1Password/SSH agent prompt or proxy login). Approve it, then retry — connecting continues in the background."
  );
  (err as Error & { code: string }).code = SSH_PENDING_CODE;
  return err;
}

export function isSshPending(err: unknown): boolean {
  return (err as { code?: string } | null)?.code === SSH_PENDING_CODE;
}
