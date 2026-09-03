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

export function humanizeHealthError(raw: string | null | undefined): string {
  if (!raw) return "Connection is failing. Check the setup and try again.";
  const low = raw.toLowerCase();
  let msg: string;
  if (low.includes("refused") || low.includes("connection")) msg = "Couldn’t connect. Check that the connection is reachable and try again.";
  else if (low.includes("auth") || low.includes("unauthorized") || low.includes("forbidden")) msg = "Authentication failed. Check the credentials and try again.";
  else if (low.includes("timeout")) msg = "Connection timed out. Check the network and try again.";
  else if (low.includes("tunnel") || low.includes("ssh")) msg = "Secure tunnel failed. Check SSH settings and try again.";
  else msg = raw.trim();
  if (!/(?:try again|retry)\.?$/i.test(msg.trim())) {
    msg = msg.replace(/\.?$/, ".") + " Check the setup and try again.";
  }
  return msg;
}

export function humanizeTestError(raw: string | null | undefined): string {
  return humanizeHealthError(raw);
}
