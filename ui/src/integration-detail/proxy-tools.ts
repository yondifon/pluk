/**
 * The tool list an MCP server offers, as the detail screen reads it.
 *
 * Rows arrive from `GET /api/integrations/<id>/proxy/tools` in the shape the
 * adapter writes them. Nothing here talks to the server: this is the order the
 * rows are shown in, the words each state is shown with, and which rows a
 * first run offers to approve.
 */

/** Where a tool stands between what the server offers and what the user approved. */
export type ProxyToolState = "new" | "approved" | "changed" | "missing";

export type SignInKind = "none" | "token" | "oauth";

export type SignInStatus = "connected" | "reconnect_needed" | "not_connected";

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

/** How many tools are waiting on a decision. */
export function attentionCount(rows: ProxyToolRow[]): number {
  return rows.filter((row) => row.state === "new" || row.state === "changed").length;
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
    case "new":
      return "Approve this tool to let your agents use it.";
    case "changed":
      return "This tool changed since you approved it. Review and approve it again.";
    case "missing":
      return "This server no longer offers this tool.";
    case "approved":
      return null;
  }
}

/** A tool reaches an agent only once it is approved and switched on. */
export function canEnable(state: ProxyToolState): boolean {
  return state === "approved";
}

export function signInMessage(kind: SignInKind, status: SignInStatus): string {
  if (kind === "none") return "This server does not ask you to sign in.";
  if (kind === "token") return "Signed in with the token you saved.";
  switch (status) {
    case "connected":
      return "Signed in.";
    case "reconnect_needed":
      return "Your sign-in has expired. Sign in again to keep using this server.";
    case "not_connected":
      return "Sign in to see what this server offers.";
  }
}

/**
 * What a first run offers to approve: the tools that only read. Anything that
 * writes or deletes stays unticked, and nothing is approved until the user
 * says so.
 */
export function readOnlyDefaults(rows: ProxyToolRow[]): string[] {
  return rows
    .filter((row) => row.category === "read" && (row.state === "new" || row.state === "changed"))
    .map((row) => row.name);
}
