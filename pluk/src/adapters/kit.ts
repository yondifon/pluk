import type { ZodRawShape } from "zod";
import type { Adapter, ActionMeta, ConfigField } from "./types.js";
import type { Integration } from "../store/integrations.js";
import type { ToolHost } from "../mcp/namespace.js";
import {
  parseActionPolicy,
  actionAllowed,
  actionPolicyDescription,
  type ActionCategory,
} from "../mcp/actionPolicy.js";
import { createLogEntry, updateLogEntry, logQuery, type Verdict } from "../store/queryLog.js";
import { buildInstructions } from "../mcp/instructions.js";
import { logError } from "../log.js";

/**
 * Adapter toolkit. Every adapter shares the same job — gate each call by policy,
 * record it in the activity log, and shape the MCP response — so that lifecycle
 * lives here once instead of being re-derived per service. Adapter modules supply
 * only what differs: the tools, the client, and the policy verdict.
 */

// ── MCP response shaping ─────────────────────────────────────────────────────

export type ToolResult = { content: { type: "text"; text: string }[]; isError?: boolean };

export const ok = (text: string): ToolResult => ({ content: [{ type: "text", text }] });
export const err = (text: string): ToolResult => ({ content: [{ type: "text", text }], isError: true });

// ── Gated runner ─────────────────────────────────────────────────────────────

/** Structured snapshot stored in the log (result_json). */
export interface LogSnapshot {
  rows: unknown[];
  fields?: string[];
}

/**
 * The result of running a gated tool's body:
 *  - `blocked`: a post-pending block (e.g. SQL cost gate) — logged as blocked.
 *  - otherwise: the call ran. `isError` marks a ran-but-failed result (e.g. a
 *    non-zero SSH exit) so it is logged "error" yet still returns its `text`.
 */
export type Outcome =
  | { blocked: string }
  | { text: string; isError?: boolean; reason?: string; result?: LogSnapshot; responseText?: string };

export interface GateMeta {
  category: string; // log category (action category, SQL statement categories, …)
  action: string;   // originating tool / operation (the log `source`)
  detail: string;   // human-readable line stored in the log (`sql` column)
}

export interface GateOpts {
  /** Pre-flight permission check. A returned reason blocks the call before any
   *  pending entry is written — matching how policy denials are logged today. */
  precheck?: () => string | undefined;
  /** Map a thrown error message to a terminal verdict (defaults to "error").
   *  Used by SQL to record aborted queries as "cancelled". */
  classifyError?: (msg: string) => Verdict;
  /** Side effects to run only on a true error (not on cancellation), e.g.
   *  evicting a pooled driver. The raw error is passed through. */
  onError?: (error: unknown) => void;
}

/**
 * Run a tool body through the policy gate + activity log, returning a shaped MCP
 * response. The single audited place for the log lifecycle: precheck → pending →
 * run → finalize, plus error/cancel handling. `run` receives the log id so it can
 * register a per-call abort against it.
 */
export async function runGated(
  conn: Pick<Integration, "id" | "name">,
  meta: GateMeta,
  run: (logId: number) => Promise<Outcome>,
  opts: GateOpts = {},
): Promise<ToolResult> {
  const block = opts.precheck?.();
  if (block !== undefined) {
    logQuery(conn.id, conn.name, meta.detail, "blocked", meta.category, block, undefined, meta.action);
    return err(`Blocked: ${block}`);
  }

  const logId = createLogEntry(conn.id, conn.name, meta.detail, "pending", meta.category, undefined, meta.action);
  try {
    const outcome = await run(logId);
    if ("blocked" in outcome) {
      updateLogEntry(logId, "blocked", outcome.blocked);
      return err(`Blocked: ${outcome.blocked}`);
    }
    const status: Verdict = outcome.isError ? "error" : "allowed";
    updateLogEntry(logId, status, outcome.reason, outcome.result, outcome.responseText);
    return outcome.isError ? err(outcome.text) : ok(outcome.text);
  } catch (e) {
    const msg = (e as Error).message;
    const status = opts.classifyError?.(msg) ?? "error";
    const text = `${status === "cancelled" ? "Cancelled" : "Error"}: ${msg}`;
    updateLogEntry(logId, status, msg, undefined, text);
    if (status === "error") opts.onError?.(e);
    return err(text);
  }
}

// ── Action adapter factory ───────────────────────────────────────────────────

/** One tool on a REST/action service. Declares its data fetch + policy category;
 *  the platform handles gating, logging, and response shaping. */
export interface ActionTool {
  name: string;
  description: string;
  schema?: ZodRawShape;
  category: ActionCategory;
  /** Log line for this call. Defaults to the tool name. */
  detail?: (args: Record<string, unknown>) => string;
  /** Fetch the data; the returned value is JSON-stringified for the agent + log. */
  run: (args: Record<string, unknown>) => Promise<unknown>;
}

export interface ActionAdapterSpec<C> {
  id: string;
  label: string;
  category: string;
  agentHint: string;
  /** One line on the access / safety model, shown to connecting agents. */
  access: string;
  /** Optional discovery hint: which tools to reach for first. */
  start?: string;
  configFields: ConfigField[];
  /** Build the per-connection client/config once, reused across tools. Receives
   *  the session's `sessionIdRef` (its `.value` is filled at session init, not at
   *  register time) so a client can scope session-lived resources — e.g. an SSH
   *  tunnel opened lazily on first tool call and torn down on session close. */
  client: (conn: Integration, sessionIdRef: { value: string }) => C;
  testConnection: (conn: Integration) => Promise<void>;
  tools: (conn: Integration, client: C) => ActionTool[];
}

/**
 * Build a complete action `Adapter` from a declarative spec. Linear, Sentry, and
 * any future REST integration declare their tools and client; gating against the
 * integration's action policy, logging, instructions, and server construction are
 * all supplied here.
 */
export function actionAdapter<C>(spec: ActionAdapterSpec<C>): Adapter {
  const instructions = (conn: Integration): string => {
    const policy = parseActionPolicy(conn.query_policy, conn.read_only);
    return buildInstructions(conn, {
      kind: spec.label,
      access: spec.access,
      policy: actionPolicyDescription(policy),
      start: spec.start,
      hint: spec.agentHint,
    });
  };

  const register = (host: ToolHost, conn: Integration, sessionIdRef: { value: string }): void => {
    const client = spec.client(conn, sessionIdRef);
    const policy = parseActionPolicy(conn.query_policy, conn.read_only);
    const policyDesc = actionPolicyDescription(policy);

    for (const tool of spec.tools(conn, client)) {
      // Don't even register a tool the integration's policy disallows — a
      // read-only integration shouldn't advertise write/delete tools to the agent
      // at all. The precheck below stays as defense in depth.
      if (!actionAllowed(policy, tool.category)) continue;

      const handler = (args: Record<string, unknown>): Promise<ToolResult> =>
        runGated(
          conn,
          {
            category: tool.category,
            action: tool.name,
            detail: tool.detail ? tool.detail(args) : tool.name,
          },
          async () => {
            const data = await tool.run(args);
            const rows = Array.isArray(data) ? data : [data];
            const text = JSON.stringify(data, null, 2);
            return { text, result: { rows }, responseText: text };
          },
          {
            precheck: () =>
              actionAllowed(policy, tool.category)
                ? undefined
                : `Action "${tool.name}" needs "${tool.category}" permission; this integration allows: ${policy.allowed.join(", ")}.`,
            onError: (e) => logError(`${spec.id} ${tool.name} failed`, e, { integration: conn.name }),
          },
        );

      // Read tools append the live policy summary to their description (so an
      // agent sees, inline, what it is allowed to do).
      const description = tool.category === "read" ? `${tool.description} ${policyDesc}` : tool.description;

      // The SDK's `tool` is heavily overloaded on the schema shape; cast at the
      // boundary (as namespace.ts does) since our handler is schema-agnostic. Bind
      // to `host` — a standalone endpoint passes the bare McpServer, whose `tool`
      // is a prototype method that needs its receiver (the namespaced host wraps it
      // in a closure, so binding is a harmless no-op there).
      const reg = (host.tool as (...a: unknown[]) => unknown).bind(host);
      if (tool.schema) reg(tool.name, description, tool.schema, handler);
      else reg(tool.name, description, handler);
    }
  };

  // Enumerate the tools once, statically, for the catalog/UI. Tool definitions
  // (name/category/description) don't depend on config — the client is only used
  // inside each tool's `run`. Build a throwaway client/conn defensively: some
  // client builders read config (and may throw on blanks), some tool builders read
  // the client — so fall back through both, never letting metadata break the adapter.
  const actions = ((): ActionMeta[] => {
    const dummyConn = { config: {} } as Integration;
    let dummyClient: C | undefined;
    try { dummyClient = spec.client(dummyConn, { value: "" }); } catch { dummyClient = undefined; }
    try {
      return spec.tools(dummyConn, dummyClient as C).map((t) => ({
        name: t.name,
        category: t.category,
        description: t.description,
      }));
    } catch {
      return [];
    }
  })();

  return {
    id: spec.id,
    label: spec.label,
    category: spec.category,
    policyKind: "action",
    agentHint: spec.agentHint,
    actions,
    configFields: spec.configFields,
    testConnection: spec.testConnection,
    instructions,
    register,
  };
}

// ── Shared config fields ─────────────────────────────────────────────────────

/**
 * The SSH auth block (auth method + private key + password), shared by the SSH
 * adapter and the SQL adapters' SSH-tunnel section. `prefix` keeps each owner's
 * stored keys stable (`""` → `auth_type`/`key_path`/…, `"ssh_"` → `ssh_auth_type`/…),
 * so this is pure deduplication with no schema migration.
 */
export function sshAuthFields(opts: {
  prefix?: string;
  group: string;
  showIf?: { key: string; equals: unknown };
}): ConfigField[] {
  const prefix = opts.prefix ?? "";
  const k = (name: string) => `${prefix}${name}`;
  const base = opts.showIf ? { showIf: opts.showIf } : {};
  return [
    {
      key: k("auth_type"), label: "Auth", type: "select", group: opts.group, default: "agent",
      options: [
        { value: "agent", label: "Agent" },
        { value: "key", label: "Private Key" },
        { value: "password", label: "Password" },
      ],
      ...base,
    },
    { key: k("key_path"), label: "Private Key", type: "file", group: opts.group, showIf: { key: k("auth_type"), equals: "key" } },
    { key: k("password"), label: "Passphrase / Password", type: "password", group: opts.group, secret: true, ...base },
  ];
}
