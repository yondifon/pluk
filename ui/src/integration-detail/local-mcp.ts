import type { McpServerState } from "../host";
import type { Integration } from "./types";

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

export function stateNote(state: McpServerState): string | null {
  return state === "crashed" ? "This server keeps stopping. Check its output below, then restart it." : null;
}
