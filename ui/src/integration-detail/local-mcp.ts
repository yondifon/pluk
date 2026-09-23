/**
 * A local MCP server: the exact command Pluk would run for it, and whether it
 * is running.
 *
 * Nothing here talks to the host; see `local-mcp-panel.ts` for that. This is
 * the words each state is shown with, and the small decisions ("can this be
 * stopped right now?") the panel would otherwise repeat.
 */

export type { LaunchPreview, LaunchEnvRow, McpServerState, McpServerStatus } from "../host";
import type { McpServerState } from "../host";
import type { Integration } from "./types";

/** Whether an MCP integration is a command Pluk starts, rather than a URL. */
export function isLocalMcp(integration: Pick<Integration, "config">): boolean {
  return integration.config.connection === "local";
}

export const RUNS_WITH_FULL_ACCESS =
  "This runs with your full access to this Mac. It can read, change or send anything you can.";

export function stateLabel(state: McpServerState): string {
  switch (state) {
    case "starting":
      return "Starting…";
    case "running":
      return "Running";
    case "stopped":
      return "Stopped";
    case "crashed":
      return "Keeps stopping";
  }
}

/** The tone a status badge takes: settled and good, settled and off, or wrong. */
export function stateTone(state: McpServerState): "on" | "off" | "warn" {
  switch (state) {
    case "running":
    case "starting":
      return "on";
    case "crashed":
      return "warn";
    case "stopped":
      return "off";
  }
}

export function canStop(state: McpServerState): boolean {
  return state === "running" || state === "starting";
}

export function canRestart(state: McpServerState): boolean {
  return state !== "starting";
}

/** The line under a crashed server, pointing at the fix. */
export function stateNote(state: McpServerState): string | null {
  return state === "crashed" ? "This server keeps stopping. Check its output below, then restart it." : null;
}
