const sessionAborts = new Map<string, AbortController>();
const sessionCloseHooks = new Set<(sessionId: string) => void>();

export function openSession(sessionId: string): void {
  sessionAborts.set(sessionId, new AbortController());
}

export function onSessionClose(fn: (sessionId: string) => void): void {
  sessionCloseHooks.add(fn);
}

export function sessionSignal(sessionId: string): AbortSignal | undefined {
  return sessionAborts.get(sessionId)?.signal;
}

export function closeSession(sessionId: string): void {
  sessionAborts.get(sessionId)?.abort();
  sessionAborts.delete(sessionId);
  for (const hook of sessionCloseHooks) {
    try { hook(sessionId); } catch { /* best-effort */ }
  }
}
