import { z } from "zod";
import type { Integration } from "../../store/integrations.js";
import { getSavedCommand, listSavedCommands } from "../../store/savedCommands.js";
import { runCommand, openForward, listForwards, closeForward } from "./client.js";
import { logError } from "../../log.js";
import { buildInstructions } from "../../mcp/instructions.js";
import { ok, err, runGated, type ToolResult } from "../kit.js";
import { toolGate } from "../../mcp/toolConfig.js";
import type { ToolSpec } from "../types.js";
import type { ToolHost } from "../../mcp/namespace.js";

export const SSH_AGENT_HINT = "Use this for SSH access to the remote host — run shell commands to inspect logs, processes, disk and memory, and Docker/systemd services for debugging and ops, and open local port forwards (ssh -L) so a remote service like a database or web UI is reachable at localhost on this machine. Every command runs as the SSH user and must be confirmed before it runs.";

// Read-only host triage. Commands run unmodified; each runs independently so a
// missing binary (e.g. docker) only fails its own line.
const DEBUG_SNAPSHOT: { label: string; command: string }[] = [
  { label: "Host", command: "uname -a" },
  { label: "Uptime / load", command: "uptime" },
  { label: "Disk", command: "df -h" },
  { label: "Memory", command: "free -m" },
  { label: "Processes", command: "ps aux" },
  { label: "Logged in", command: "who" },
  { label: "Containers", command: "docker ps" },
];

const MAX_BATCH = 50;

// Commands that change the remote host need explicit user confirmation. We mark
// every command-running tool so the MCP client always prompts before executing
// (there is no allowlist or read/write gate — confirmation is the safeguard).
// A command-prevention policy may be layered on later (see policy.ts).
const CONFIRM_ANNOTATIONS = {
  readOnlyHint: false,
  destructiveHint: true,
  openWorldHint: true,
} as const;

// Opening a forward doesn't change remote state, but it does open a local network
// bridge into the remote side — worth a client confirm, not flagged destructive.
const FORWARD_ANNOTATIONS = {
  readOnlyHint: false,
  destructiveHint: false,
  openWorldHint: true,
} as const;

const MAX_PORT = 65535;

export function sshInstructions(conn: Integration): string {
  return buildInstructions(conn, {
    kind: "SSH",
    access: "Run shell commands on the remote host as the connecting SSH user. Commands run unmodified and are recorded in the activity log.",
    policy: "Unrestricted — there is no allowlist; every command must be confirmed in your client before it runs.",
    start: "Start with debug_snapshot for a host overview, or list_saved_commands for curated commands. Use open_forward to reach a remote service (e.g. a database) at localhost on this machine.",
    hint: SSH_AGENT_HINT,
  });
}

// Static tool catalog for SSH. Every tool is individually toggleable (all default
// on); none carry settings — confirmation in the client is the safeguard.
export function sshToolSpecs(): ToolSpec[] {
  const t = (name: string, description: string): ToolSpec =>
    ({ name, description, category: "read", defaultEnabled: true });
  return [
    t("run_command", "Run a shell command on the remote host over SSH."),
    t("run_batch", "Run several shell commands in sequence on the remote host."),
    t("debug_snapshot", "Capture a quick health snapshot of the remote host."),
    t("run_saved_command", "Run a pre-selected (saved) command by name."),
    t("list_saved_commands", "List the saved commands available for this integration."),
    t("open_forward", "Open a local port forward (ssh -L) to a remote service."),
    t("list_forwards", "List the open local port forwards for this connection."),
    t("close_forward", "Close an open local port forward by its id."),
  ];
}

export function registerSshServer(server: ToolHost, conn: Integration, sessionIdRef: { value: string }): void {
  const gate = toolGate(conn.query_policy);
  // Every SSH tool is individually toggleable; all default on. A disabled tool is
  // not registered, so the agent never sees it.
  const on = (name: string): boolean => gate.enabled(name, true);

  // Run one command on the remote host through the activity log and return its
  // shaped MCP result. No allowlist or permission check — the command runs as-is;
  // confirmation by the client is the safeguard. Shared by every command tool so
  // logging stays consistent. A non-zero exit is logged "error" with `exit N`.
  function runOne(command: string, workingDir: string | undefined, toolName: string): Promise<ToolResult> {
    const trimmed = command.trim();
    if (!trimmed) return Promise.resolve(err("Error: empty command."));

    const detail = workingDir ? `[${workingDir}] ${trimmed}` : trimmed;
    const finalCommand = workingDir ? `cd ${quoteDir(workingDir)} && ${trimmed}` : trimmed;

    return runGated(
      conn,
      { category: "command", action: toolName, detail },
      async () => {
        const { stdout, stderr, code, truncated } = await runCommand(sessionIdRef.value, conn, finalCommand);
        const text = formatResult(stdout, stderr, code, truncated);
        const result = { rows: [{ exit_code: code }] };
        return code === 0
          ? { text, result, responseText: text }
          : { text, isError: true, reason: `exit ${code}`, result, responseText: text };
      },
      { onError: (e) => logError(`ssh ${toolName} failed`, e, { integration: conn.name }) },
    );
  }

  if (on("run_command")) server.tool(
    "run_command",
    "Run a shell command on the remote host over SSH. The command runs unmodified as the connecting user — confirm before running, as it can change or destroy remote state.",
    {
      command: z.string().describe("The command to run, e.g. `docker compose ps`"),
      working_dir: z.string().optional().describe("Directory to run in (e.g. /srv/app). Optional."),
    },
    CONFIRM_ANNOTATIONS,
    ({ command, working_dir }) => runOne(command, working_dir, "run_command"),
  );

  if (on("run_batch")) server.tool(
    "run_batch",
    "Run several shell commands in sequence on the remote host. Returns each command's output in order. Confirm before running — commands run unmodified as the connecting user.",
    {
      commands: z.array(z.string()).min(1).max(MAX_BATCH).describe(`Commands to run in order (max ${MAX_BATCH}).`),
      working_dir: z.string().optional().describe("Directory to run every command in. Optional."),
      stop_on_error: z.boolean().optional().describe("Stop at the first failed command instead of continuing. Default true."),
    },
    CONFIRM_ANNOTATIONS,
    async ({ commands, working_dir, stop_on_error }) => {
      const stop = stop_on_error ?? true;
      const sections: string[] = [];
      let anyError = false;

      for (let i = 0; i < commands.length; i++) {
        const cmd = commands[i]!;
        const res = await runOne(cmd, working_dir, "run_batch");
        sections.push(`$ ${cmd}\n${textOf(res)}`);
        if (res.isError) {
          anyError = true;
          if (stop) {
            const skipped = commands.length - i - 1;
            if (skipped > 0) sections.push(`[stopped on error — ${skipped} command(s) not run]`);
            break;
          }
        }
      }

      const text = sections.join("\n\n———\n\n");
      return anyError ? err(text) : ok(text);
    },
  );

  if (on("debug_snapshot")) server.tool(
    "debug_snapshot",
    "Capture a quick health snapshot of the remote host (kernel, load, disk, memory, processes, logins, containers). Useful as a first step when debugging.",
    CONFIRM_ANNOTATIONS,
    async () => {
      const sections: string[] = [];
      let anyError = false;
      for (const item of DEBUG_SNAPSHOT) {
        const res = await runOne(item.command, undefined, "debug_snapshot");
        if (res.isError) anyError = true;
        sections.push(`## ${item.label} — \`${item.command}\`\n${textOf(res)}`);
      }
      const text = sections.join("\n\n");
      return anyError ? err(text) : ok(text);
    },
  );

  if (on("run_saved_command")) server.tool(
    "run_saved_command",
    "Run a pre-selected (saved) command by name. Confirm before running — saved commands run unmodified as the connecting user.",
    { name: z.string().describe("Name of the saved command") },
    CONFIRM_ANNOTATIONS,
    ({ name }) => {
      const saved = getSavedCommand(conn.id, name);
      if (!saved) {
        const names = listSavedCommands(conn.id).map((c) => c.name);
        const hint = names.length
          ? ` Available: ${names.map((n) => `"${n}"`).join(", ")}.`
          : " There are no saved commands for this integration yet.";
        return Promise.resolve(err(`Saved command "${name}" not found.${hint}`));
      }
      return runOne(saved.command, saved.working_dir ?? undefined, "run_saved_command");
    },
  );

  if (on("list_saved_commands")) server.tool(
    "list_saved_commands",
    "List the pre-selected (saved) commands available for this SSH integration.",
    { readOnlyHint: true, openWorldHint: false } as const,
    async () => {
      const saved = listSavedCommands(conn.id).map((c) => ({ name: c.name, command: c.command, working_dir: c.working_dir }));
      if (saved.length === 0) return ok("No saved commands for this integration.");
      return ok(JSON.stringify(saved, null, 2));
    },
  );

  if (on("open_forward")) server.tool(
    "open_forward",
    "Open a local port forward (ssh -L) over this connection so a service reachable from the remote host becomes available at localhost on this machine. Returns the local port to connect to (e.g. `psql -h localhost -p <port>`). The forward stays open for the session until closed.",
    {
      remote_port: z.number().int().min(1).max(MAX_PORT).describe("Port on the remote side to forward, e.g. 5432 for Postgres or 6379 for Redis"),
      remote_host: z.string().optional().describe("Host to reach from the remote side. Defaults to `localhost` (a service running on the SSH host itself); set this to reach another host on the remote network."),
      local_port: z.number().int().min(1).max(MAX_PORT).optional().describe("Local port to listen on. Omit to auto-assign a free port."),
    },
    FORWARD_ANNOTATIONS,
    async ({ remote_port, remote_host, local_port }) => {
      const remoteHost = remote_host?.trim() || "localhost";
      const detail = `open_forward localhost:${local_port ?? "auto"} -> ${remoteHost}:${remote_port}`;
      return runGated(
        conn,
        { category: "forward", action: "open_forward", detail },
        async () => {
          const fwd = await openForward(sessionIdRef.value, conn, remoteHost, remote_port, local_port);
          const text =
            `Forward open: localhost:${fwd.localPort} → ${fwd.remoteHost}:${fwd.remotePort} (id "${fwd.id}").\n` +
            `Connect to it at 127.0.0.1:${fwd.localPort} on this machine. Close it with close_forward "${fwd.id}".`;
          return { text, result: { rows: [forwardRow(fwd)] }, responseText: text };
        },
        { onError: (e) => logError("ssh open_forward failed", e, { integration: conn.name }) },
      );
    },
  );

  if (on("list_forwards")) server.tool(
    "list_forwards",
    "List the open local port forwards for this connection (local port → remote target).",
    { readOnlyHint: true, openWorldHint: false } as const,
    async () => {
      const forwards = listForwards(sessionIdRef.value, conn).map(forwardRow);
      if (forwards.length === 0) return ok("No open forwards for this connection.");
      return ok(JSON.stringify(forwards, null, 2));
    },
  );

  if (on("close_forward")) server.tool(
    "close_forward",
    "Close an open local port forward by its id (the `remoteHost:remotePort` returned by open_forward / list_forwards).",
    { id: z.string().describe('Forward id, e.g. "localhost:5432"') },
    FORWARD_ANNOTATIONS,
    ({ id }) => {
      const closed = closeForward(sessionIdRef.value, conn, id);
      return Promise.resolve(closed ? ok(`Closed forward "${id}".`) : err(`No open forward with id "${id}".`));
    },
  );
}

// Flatten a forward into the log/JSON shape used by list_forwards and the
// open_forward result row.
function forwardRow(f: { id: string; remoteHost: string; remotePort: number; localPort: number }) {
  return { id: f.id, local: `127.0.0.1:${f.localPort}`, remote: `${f.remoteHost}:${f.remotePort}` };
}

// Pull the text payload out of a shaped tool result for composition (batch /
// snapshot concatenate several runs into one response).
function textOf(res: ToolResult): string {
  return res.content[0]?.text ?? "";
}

// Quote a working directory so a `cd` into it can't be broken by spaces. The
// command itself is unrestricted, so this is hygiene, not a security boundary.
function quoteDir(dir: string): string {
  return `'${dir.replace(/'/g, "'\\''")}'`;
}

function formatResult(stdout: string, stderr: string, code: number | null, truncated?: boolean): string {
  const parts: string[] = [`exit code: ${code ?? "unknown"}`];
  if (stdout.trim()) parts.push(`stdout:\n${stdout.trimEnd()}`);
  if (stderr.trim()) parts.push(`stderr:\n${stderr.trimEnd()}`);
  if (!stdout.trim() && !stderr.trim()) parts.push("(no output)");
  if (truncated) parts.push("[output truncated at 1 MB]");
  return parts.join("\n\n");
}
