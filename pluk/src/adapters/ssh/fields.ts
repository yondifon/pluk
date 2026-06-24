import type { ConfigField } from "../types.js";
import { sshAuthFields } from "../kit.js";

export const sshFields: ConfigField[] = [
  { key: "host", label: "Host", type: "text", group: "Connection", required: true, placeholder: "server.example.com or an ~/.ssh/config alias" },
  { key: "port", label: "Port", type: "number", group: "Connection", default: 22 },
  { key: "user", label: "User", type: "text", group: "Connection", placeholder: "defaults to your local username" },
  ...sshAuthFields({ group: "Auth" }),
];
