import type { ConfigField } from "../types.js";
import { sshAuthFields } from "../kit.js";

export const redisFields: ConfigField[] = [
  { key: "host", label: "Host", type: "text", group: "Connection", required: true, default: "127.0.0.1", placeholder: "localhost or 10.0.0.5" },
  { key: "port", label: "Port", type: "number", group: "Connection", default: 6379 },
  { key: "db", label: "Database", type: "number", group: "Connection", default: 0, placeholder: "0" },
  { key: "tls", label: "TLS (rediss://)", type: "toggle", group: "Connection", default: false },
  { key: "password", label: "Password", type: "password", group: "Auth", secret: true, placeholder: "(optional)" },

  // SSH tunnel — Redis is commonly bound to localhost on a server and reached over
  // SSH. Mirrors the SQL adapters' tunnel section; the host/port above become the
  // tunnel's remote target. Reuses the shared SSH auth block (prefix `ssh_`).
  { key: "use_ssh", label: "SSH Tunnel", type: "toggle", group: "SSH Tunnel" },
  { key: "ssh_host", label: "SSH Host", type: "text", group: "SSH Tunnel", showIf: { key: "use_ssh", equals: true } },
  { key: "ssh_port", label: "SSH Port", type: "number", group: "SSH Tunnel", default: 22, showIf: { key: "use_ssh", equals: true } },
  { key: "ssh_user", label: "SSH User", type: "text", group: "SSH Tunnel", showIf: { key: "use_ssh", equals: true } },
  ...sshAuthFields({ prefix: "ssh_", group: "SSH Tunnel", showIf: { key: "use_ssh", equals: true } }),
];
