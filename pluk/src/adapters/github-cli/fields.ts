import type { ConfigField } from "../types.js";

export const githubCliFields: ConfigField[] = [
  { key: "gh_bin", label: "gh Executable", type: "text", group: "Connection", default: "gh", placeholder: "gh on PATH, or an absolute path" },
  { key: "timeout_seconds", label: "Timeout (seconds)", type: "number", group: "Connection", default: 30, placeholder: "How long one gh command may run" },
  { key: "default_repo", label: "Default Repo", type: "text", group: "Defaults", placeholder: "owner/repo (optional)" },
  { key: "default_cwd", label: "Default Working Directory", type: "text", group: "Defaults", placeholder: "Used when a call passes no cwd" },
];
