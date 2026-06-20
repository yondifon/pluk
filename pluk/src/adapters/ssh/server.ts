import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import type { Integration } from "../../store/integrations.js";
import { parseActionPolicy, actionAllowed, actionPolicyDescription } from "../../mcp/actionPolicy.js";
import { createLogEntry, updateLogEntry, logQuery } from "../../store/queryLog.js";
import { evaluateCommand, policySummary, sanitizeWorkingDir } from "./policy.js";
import { runCommand } from "./client.js";
import { logError } from "../../log.js";
import type { ToolHost } from "../../mcp/namespace.js";

// MCP server for the SSH adapter: run shell commands on a remote host, gated by
// an allowlist (see policy.ts) and the integration's action policy. Read commands
// need "read"; state-changing ones (e.g. `docker compose up`) need "write".
export function buildSshServer(conn: Integration, sessionIdRef: { value: string }): McpServer {
  const server = new McpServer({ name: conn.name, version: "1.0.0" });
  registerSshServer(server, conn, sessionIdRef);
  return server;
}

export function registerSshServer(server: ToolHost, conn: Integration, sessionIdRef: { value: string }): void {
  const policy = parseActionPolicy(conn.query_policy, conn.read_only);

  server.tool(
    "run_command",
    `Run a shell command on the remote host. Commands are checked against a strict allowlist before running. ${actionPolicyDescription(policy)}\n\n${policySummary()}`,
    {
      command: z.string().describe("The command to run, e.g. `docker compose ps`"),
      working_dir: z.string().optional().describe("Directory to run in (e.g. /srv/app). Optional."),
    },
    async ({ command, working_dir }) => {
      const detail = working_dir ? `[${working_dir}] ${command}` : command;

      const verdict = evaluateCommand(command);
      if (!verdict.ok) {
        logQuery(conn.id, conn.name, detail, "blocked", "command", verdict.reason, undefined, "run_command");
        return { content: [{ type: "text", text: `Blocked: ${verdict.reason}` }], isError: true };
      }

      if (!actionAllowed(policy, verdict.category)) {
        const reason = `This command needs "${verdict.category}" permission; this integration allows: ${policy.allowed.join(", ")}.`;
        logQuery(conn.id, conn.name, detail, "blocked", verdict.category, reason, undefined, "run_command");
        return { content: [{ type: "text", text: `Blocked: ${reason}` }], isError: true };
      }

      let finalCommand = command.trim();
      if (working_dir) {
        const dir = sanitizeWorkingDir(working_dir);
        if (!dir) {
          const reason = `Invalid working_dir: "${working_dir}".`;
          logQuery(conn.id, conn.name, detail, "blocked", verdict.category, reason, undefined, "run_command");
          return { content: [{ type: "text", text: `Blocked: ${reason}` }], isError: true };
        }
        // `cd` is constructed by us (not user chaining), and `dir` is sanitized.
        finalCommand = `cd ${dir} && ${finalCommand}`;
      }

      const logId = createLogEntry(conn.id, conn.name, detail, "pending", verdict.category, undefined, "run_command");
      try {
        const { stdout, stderr, code, truncated } = await runCommand(sessionIdRef.value, conn, finalCommand);
        const text = formatResult(stdout, stderr, code, truncated);
        updateLogEntry(logId, code === 0 ? "allowed" : "error", code === 0 ? undefined : `exit ${code}`, {
          rows: [{ exit_code: code }],
        });
        return { content: [{ type: "text", text }], isError: code !== 0 };
      } catch (err) {
        const msg = (err as Error).message;
        updateLogEntry(logId, "error", msg);
        logError("ssh run_command failed", err, { integration: conn.name });
        return { content: [{ type: "text", text: `Error: ${msg}` }], isError: true };
      }
    },
  );

  server.tool(
    "list_allowed_commands",
    "Show which commands this SSH integration is permitted to run.",
    async () => ({ content: [{ type: "text", text: policySummary() }] }),
  );
}

function formatResult(stdout: string, stderr: string, code: number | null, truncated?: boolean): string {
  const parts: string[] = [`exit code: ${code ?? "unknown"}`];
  if (stdout.trim()) parts.push(`stdout:\n${stdout.trimEnd()}`);
  if (stderr.trim()) parts.push(`stderr:\n${stderr.trimEnd()}`);
  if (!stdout.trim() && !stderr.trim()) parts.push("(no output)");
  if (truncated) parts.push("[output truncated at 1 MB]");
  return parts.join("\n\n");
}
