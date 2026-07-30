// Owner-scoped resource lifetime. The 2026-07-28 protocol is stateless — every
// request is answered by a fresh server, and there is no session id to key
// long-lived drivers, tunnels or forwards on. The stable identity is the owner:
// the integration or group the endpoint token resolves to. Owner scope lives for
// the process, and is torn down when the owner's config changes (/api/reload).
const ownerAborts = new Map<string, AbortController>();
const ownerCloseHooks = new Set<(ownerId: string) => void>();

export function openOwner(ownerId: string): void {
  if (!ownerAborts.has(ownerId)) ownerAborts.set(ownerId, new AbortController());
}

export function onOwnerClose(fn: (ownerId: string) => void): void {
  ownerCloseHooks.add(fn);
}

export function ownerSignal(ownerId: string): AbortSignal | undefined {
  return ownerAborts.get(ownerId)?.signal;
}

export function closeOwner(ownerId: string): void {
  ownerAborts.get(ownerId)?.abort();
  ownerAborts.delete(ownerId);
  for (const hook of ownerCloseHooks) {
    try { hook(ownerId); } catch { /* best-effort */ }
  }
}
