import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import type { Integration } from "../../store/integrations.js";
import { createLogEntry, updateLogEntry } from "../../store/queryLog.js";
import { getSavedCommand, listSavedCommands } from "../../store/savedCommands.js";
import { runCommand } from "./client.js";
import { logError } from "../../log.js";
import { buildInstructions } from "../../mcp/instructions.js";
import type { ToolHost } from "../../mcp/namespace.js";

export const SSH_AGENT_HINT = "Every command runs as the SSH user and must be confirmed.";

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

// MCP server for the SSH adapter: run shell commands on a remote host. Commands
// run unrestricted as the connecting SSH user; the client is expected to confirm
// each one (tools are annotated destructive so hosts prompt every time).
export function buildSshServer(conn: Integration, sessionIdRef: { value: string }): McpServer {
  const server = new McpServer(
    { name: conn.name, version: "1.0.0" },
    { instructions: sshInstructions(conn) },
  );
  registerSshServer(server, conn, sessionIdRef);
  return server;
}

export function sshInstructions(conn: Integration): string {
  return buildInstructions(conn, {
    kind: "SSH",
    access: "Run shell commands on the remote host as the connecting SSH user. Commands run unmodified and are recorded in the activity log.",
    policy: "Unrestricted — there is no allowlist; every command must be confirmed in your client before it runs.",
    start: "Start with debug_snapshot for a host overview, or list_saved_commands for curated commands.",
    hint: SSH_AGENT_HINT,
  });
}

export function registerSshServer(server: ToolHost, conn: Integration, sessionIdRef: { value: string }): void {
  // Run one command on the remote host and return its formatted output. No
  // allowlist or permission check — the command runs as-is. Shared by every
  // command-running tool so logging stays consistent.
  async function runOne(command: string, workingDir: string | undefined, toolName: string): Promise<{ text: string; isError: boolean }> {
    const trimmed = command.trim();
    if (!trimmed) return { text: "Error: empty command.", isError: true };

    const detail = workingDir ? `[${workingDir}] ${trimmed}` : trimmed;
    const finalCommand = workingDir ? `cd ${quoteDir(workingDir)} && ${trimmed}` : trimmed;

    const logId = createLogEntry(conn.id, conn.name, detail, "pending", "command", undefined, toolName);
    try {
      const { stdout, stderr, code, truncated } = await runCommand(sessionIdRef.value, conn, finalCommand);
      const text = formatResult(stdout, stderr, code, truncated);
      updateLogEntry(logId, code === 0 ? "allowed" : "error", code === 0 ? undefined : `exit ${code}`, {
        rows: [{ exit_code: code }],
      }, text);
      return { text, isError: code !== 0 };
    } catch (err) {
      const msg = (err as Error).message;
      updateLogEntry(logId, "error", msg, undefined, `Error: ${msg}`);
      logError(`ssh ${toolName} failed`, err, { integration: conn.name });
      return { text: `Error: ${msg}`, isError: true };
    }
  }

  server.tool(
    "run_command",
    "Run a shell command on the remote host over SSH. The command runs unmodified as the connecting user — confirm before running, as it can change or destroy remote state.",
    {
      command: z.string().describe("The command to run, e.g. `docker compose ps`"),
      working_dir: z.string().optional().describe("Directory to run in (e.g. /srv/app). Optional."),
    },
    CONFIRM_ANNOTATIONS,
    async ({ command, working_dir }) => {
      const { text, isError } = await runOne(command, working_dir, "run_command");
      return { content: [{ type: "text", text }], isError };
    },
  );

  server.tool(
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
        const { text, isError } = await runOne(cmd, working_dir, "run_batch");
        sections.push(`$ ${cmd}\n${text}`);
        if (isError) {
          anyError = true;
          if (stop) {
            const skipped = commands.length - i - 1;
            if (skipped > 0) sections.push(`[stopped on error — ${skipped} command(s) not run]`);
            break;
          }
        }
      }

      return { content: [{ type: "text", text: sections.join("\n\n———\n\n") }], isError: anyError };
    },
  );

  server.tool(
    "debug_snapshot",
    "Capture a quick health snapshot of the remote host (kernel, load, disk, memory, processes, logins, containers). Useful as a first step when debugging.",
    CONFIRM_ANNOTATIONS,
    async () => {
      const sections: string[] = [];
      let anyError = false;
      for (const item of DEBUG_SNAPSHOT) {
        const { text, isError } = await runOne(item.command, undefined, "debug_snapshot");
        if (isError) anyError = true;
        sections.push(`## ${item.label} — \`${item.command}\`\n${text}`);
      }
      return { content: [{ type: "text", text: sections.join("\n\n") }], isError: anyError };
    },
  );

  server.tool(
    "run_saved_command",
    "Run a pre-selected (saved) command by name. Confirm before running — saved commands run unmodified as the connecting user.",
    { name: z.string().describe("Name of the saved command") },
    CONFIRM_ANNOTATIONS,
    async ({ name }) => {
      const saved = getSavedCommand(conn.id, name);
      if (!saved) {
        const names = listSavedCommands(conn.id).map((c) => c.name);
        const hint = names.length
          ? ` Available: ${names.map((n) => `"${n}"`).join(", ")}.`
          : " There are no saved commands for this integration yet.";
        return { content: [{ type: "text", text: `Saved command "${name}" not found.${hint}` }], isError: true };
      }
      const { text, isError } = await runOne(saved.command, saved.working_dir ?? undefined, "run_saved_command");
      return { content: [{ type: "text", text }], isError };
    },
  );

  server.tool(
    "list_saved_commands",
    "List the pre-selected (saved) commands available for this SSH integration.",
    { readOnlyHint: true, openWorldHint: false } as const,
    async () => {
      const saved = listSavedCommands(conn.id).map((c) => ({ name: c.name, command: c.command, working_dir: c.working_dir }));
      if (saved.length === 0) {
        return { content: [{ type: "text", text: "No saved commands for this integration." }] };
      }
      return { content: [{ type: "text", text: JSON.stringify(saved, null, 2) }] };
    },
  );
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
