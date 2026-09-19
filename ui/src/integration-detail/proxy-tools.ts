/**
 * The tool list an MCP server offers, as the detail screen reads it.
 *
 * Rows arrive from `GET /api/integrations/<id>/proxy/tools` in the shape the
 * adapter writes them. Nothing here talks to the server: this is the order the
 * rows are shown in and the words each state is shown with.
 */

/** Where a tool stands between what the server offers and what the user approved. */
export type ProxyToolState = "new" | "approved" | "changed" | "missing";

export type SignInKind = "none" | "token" | "oauth";

export type SignInStatus = "connected" | "reconnect_needed" | "not_connected";

/** What the server itself asks for, whatever the user has saved so far. */
export type SignInRequired = "none" | "oauth" | "token";

export interface SignIn {
  kind: SignInKind;
  status: SignInStatus;
  required: SignInRequired;
  /** The server hands out no client IDs and none was saved. */
  needsClientId?: boolean;
}

/** The sign-in block, reduced to the one line and the one button it shows. */
export interface SignInView {
  message: string;
  /** The badge to show, or null when this server has nothing to be signed in to. */
  status: SignInStatus | null;
  action: "sign-in" | "sign-in-again" | "sign-out" | null;
}

export interface ProxyToolRow {
  name: string;
  label: string;
  description: string;
  category: string;
  state: ProxyToolState;
  present: boolean;
  updatedAt: string;
}

/** Tools waiting on the user come first, then the settled ones, then the gone ones. */
const STATE_RANK: Record<ProxyToolState, number> = {
  changed: 0,
  new: 1,
  approved: 2,
  missing: 3,
};

export function orderedProxyTools(rows: ProxyToolRow[]): ProxyToolRow[] {
  return [...rows].sort(
    (a, b) => STATE_RANK[a.state] - STATE_RANK[b.state] || a.label.localeCompare(b.label),
  );
}

/** The short word on the row, or null when the row needs no marking. */
export function stateBadge(state: ProxyToolState): string | null {
  switch (state) {
    case "new":
      return "New";
    case "changed":
      return "Changed";
    case "missing":
      return "Unavailable";
    case "approved":
      return null;
  }
}

/** The line under the row explaining what the user can do about it. */
export function stateNote(state: ProxyToolState): string | null {
  switch (state) {
    case "changed":
      return "This tool changed. Read what it does now, then turn it back on.";
    case "missing":
      return "This server no longer offers this tool.";
    case "new":
    case "approved":
      return null;
  }
}

/** A tool reaches an agent only once it is approved and switched on. */
export function canEnable(state: ProxyToolState): boolean {
  return state === "approved";
}

/**
 * What the sign-in block says and offers.
 *
 * The server's own answer leads: a server that lets anyone in is never asked
 * to be signed in to, and one that only takes a token is never offered a
 * button that cannot help. A saved token that works reads as signed in,
 * whatever else the server would have accepted.
 */
export function signInView(auth: SignIn): SignInView {
  if (auth.kind === "token") {
    return { message: "Signed in with the token you saved.", status: "connected", action: null };
  }
  if (auth.required === "none") {
    return { message: "This server does not ask you to sign in.", status: null, action: null };
  }
  if (auth.required === "token") {
    return {
      message: "This server needs a token. Add one in this integration's settings.",
      status: null,
      action: null,
    };
  }
  if (auth.needsClientId) {
    return {
      message:
        "This server needs a client ID. Add one in this integration's settings, then sign in.",
      status: null,
      action: null,
    };
  }
  switch (auth.status) {
    case "connected":
      return { message: "Signed in.", status: "connected", action: "sign-out" };
    case "reconnect_needed":
      return {
        message: "Your sign-in has expired. Sign in again to keep using this server.",
        status: "reconnect_needed",
        action: "sign-in-again",
      };
    case "not_connected":
      return {
        message: "Sign in to see what this server offers.",
        status: "not_connected",
        action: "sign-in",
      };
  }
}

/** Whether an empty tool list is waiting on the user rather than on the server. */
export function awaitingSignIn(auth: SignIn): boolean {
  return auth.required !== "none" && signInView(auth).status !== "connected";
}
