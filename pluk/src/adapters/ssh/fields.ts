import type { ConfigField } from "../types.js";

export const sshFields: ConfigField[] = [
  { key: "host", label: "Host", type: "text", group: "Connection", required: true, placeholder: "server.example.com or an ~/.ssh/config alias" },
  { key: "port", label: "Port", type: "number", group: "Connection", default: 22 },
  { key: "user", label: "User", type: "text", group: "Connection", placeholder: "defaults to your local username" },
  {
    key: "auth_type", label: "Auth", type: "select", group: "Auth", default: "agent",
    options: [
      { value: "agent", label: "Agent" },
      { value: "key", label: "Private Key" },
      { value: "password", label: "Password" },
    ],
  },
  { key: "key_path", label: "Private Key", type: "file", group: "Auth", showIf: { key: "auth_type", equals: "key" } },
  { key: "password", label: "Passphrase / Password", type: "password", group: "Auth", secret: true },
];
