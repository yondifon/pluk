import type { ConfigField } from "../types.js";

export const linearFields: ConfigField[] = [
  { key: "api_key", label: "API Key", type: "password", group: "Auth", required: true, secret: true, placeholder: "lin_api_…" },
  { key: "team_key", label: "Default Team", type: "text", group: "Defaults", placeholder: "ENG (optional, scopes list_issues)" },
];
