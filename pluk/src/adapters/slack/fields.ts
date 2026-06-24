import type { ConfigField } from "../types.js";

export const slackFields: ConfigField[] = [
  { key: "bot_token", label: "Bot Token", type: "password", group: "Auth", required: true, secret: true, placeholder: "xoxb-…" },
  { key: "default_channel", label: "Default Channel", type: "text", group: "Defaults", placeholder: "C0123… or #general (optional)" },
];
