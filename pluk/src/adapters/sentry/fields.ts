import type { ConfigField } from "../types.js";

export const sentryFields: ConfigField[] = [
  { key: "auth_token", label: "Auth Token", type: "password", group: "Auth", required: true, secret: true, placeholder: "sntrys_… or a personal token" },
  { key: "org_slug", label: "Organization", type: "text", group: "Scope", required: true, placeholder: "my-org" },
  { key: "project_slug", label: "Default Project", type: "text", group: "Scope", placeholder: "backend (optional, scopes list_issues)" },
  { key: "base_url", label: "Base URL", type: "text", group: "Connection", default: "https://sentry.io", placeholder: "https://sentry.io (or self-hosted)" },
];
