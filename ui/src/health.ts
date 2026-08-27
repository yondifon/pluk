/**
 * Health transition detection — only fire toasts when crossing between
 * working and failing. Mirrors `swift/Sources/ConnectionStore.swift#emitHealthTransitions`.
 */

export type Health = { status: "ok" | "error"; error?: string | null; at?: number };
export type HealthMap = Record<string, Health>;
export type TransitionKind = "to_error" | "to_ok";

export type Transition = {
  integrationId: string;
  kind: TransitionKind;
  next: Health;
  prev: Health | undefined;
};

export function detectTransitions(prev: HealthMap, next: HealthMap): Transition[] {
  const out: Transition[] = [];
  for (const [id, n] of Object.entries(next)) {
    const p = prev[id];
    const wasError = p?.status === "error";
    const isError = n.status === "error";
    if (isError && !wasError) {
      out.push({ integrationId: id, kind: "to_error", next: n, prev: p });
    } else if (!isError && wasError) {
      out.push({ integrationId: id, kind: "to_ok", next: n, prev: p });
    }
  }
  return out;
}

export function transitionToast(transition: Transition, integrationName: string): { title: string; message: string; kind: "error" | "success" } {
  if (transition.kind === "to_error") {
    return {
      title: integrationName,
      message: transition.next.error ?? "Connection is failing. Check the setup and try again.",
      kind: "error",
    };
  }
  return {
    title: integrationName,
    message: "Connection restored.",
    kind: "success",
  };
}

// Human-facing error copy: say what failed and what to try, never internal vocab
export function humanizeHealthError(raw: string | null | undefined): string {
  if (!raw) return "Connection is failing. Check the setup and try again.";
  const low = raw.toLowerCase();
  if (low.includes("refused") || low.includes("connection")) return "Couldn’t connect. Check that the service is reachable and try again.";
  if (low.includes("auth") || low.includes("unauthorized") || low.includes("forbidden")) return "Authentication failed. Check the credentials and try again.";
  if (low.includes("timeout")) return "Connection timed out. Check the network and try again.";
  if (low.includes("tunnel") || low.includes("ssh")) return "Secure tunnel failed. Check SSH settings and try again.";
  return raw;
}
