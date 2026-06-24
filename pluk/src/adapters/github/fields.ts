import type { ConfigField } from "../types.js";

export const githubFields: ConfigField[] = [
  { key: "token", label: "Token", type: "password", group: "Auth", required: true, secret: true, placeholder: "github_pat_… (fine-grained PAT)" },
  { key: "default_repo", label: "Default Repo", type: "text", group: "Defaults", placeholder: "owner/repo (optional, scopes repo tools)" },
  { key: "base_url", label: "Base URL", type: "text", group: "Connection", default: "https://api.github.com", placeholder: "https://api.github.com (or GHES)" },
];
