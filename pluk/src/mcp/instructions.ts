import type { Integration } from "../store/integrations.js";

// One consistent, agent-facing guidance block for an MCP server. It is returned
// in the MCP `initialize` handshake (ServerOptions.instructions), so a connecting
// agent auto-discovers — without guessing from tool names — what the integration
// is, how it is constrained *right now*, and which tools to reach for first.
// Built per session from live config + policy, so it always reflects the current
// state. Kept terse: agents read this verbatim, so every line must earn its place.

export interface InstructionParts {
  kind: string;        // adapter label, e.g. "PostgreSQL", "Linear"
  access: string;      // one line on the access / safety model
  policy?: string;     // live, dynamic policy or permission summary
  hint?: string;       // adapter workflow guidance (the agentHint)
  start?: string;      // discovery: which tools to use first
}

export function buildInstructions(
  conn: Pick<Integration, "name" | "environment">,
  parts: InstructionParts,
): string {
  const header = `${parts.kind} integration "${conn.name}"${conn.environment ? ` — ${conn.environment} environment` : ""}.`;
  const lines = [header, parts.access];
  if (parts.policy) lines.push(`Current policy: ${parts.policy}`);
  if (parts.start) lines.push(parts.start);
  if (parts.hint) lines.push(parts.hint);
  return lines.join("\n");
}
