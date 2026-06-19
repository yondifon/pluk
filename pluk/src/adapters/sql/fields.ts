import type { ConfigField } from "../types.js";

// Shared config fields for the network SQL databases (Postgres, MySQL). The UI
// renders these dynamically; `showIf` drives the SSH/SSL conditional sections.

export function networkSqlFields(defaultPort: number): ConfigField[] {
  return [
    { key: "host", label: "Host", type: "text", group: "Connection", placeholder: "localhost", default: "localhost" },
    { key: "port", label: "Port", type: "number", group: "Connection", default: defaultPort },
    { key: "user", label: "User", type: "text", group: "Connection" },
    { key: "password", label: "Password", type: "password", group: "Connection", secret: true },
    { key: "database", label: "Database", type: "text", group: "Connection" },
    { key: "socket_path", label: "Socket", type: "text", group: "Connection", placeholder: "Leave empty for TCP (optional)" },

    { key: "use_ssh", label: "SSH Tunnel", type: "toggle", group: "SSH Tunnel" },
    { key: "ssh_host", label: "SSH Host", type: "text", group: "SSH Tunnel", showIf: { key: "use_ssh", equals: true } },
    { key: "ssh_port", label: "SSH Port", type: "number", group: "SSH Tunnel", default: 22, showIf: { key: "use_ssh", equals: true } },
    { key: "ssh_user", label: "SSH User", type: "text", group: "SSH Tunnel", showIf: { key: "use_ssh", equals: true } },
    {
      key: "ssh_auth_type", label: "Auth", type: "select", group: "SSH Tunnel", default: "agent",
      options: [
        { value: "agent", label: "Agent" },
        { value: "key", label: "Private Key" },
        { value: "password", label: "Password" },
      ],
      showIf: { key: "use_ssh", equals: true },
    },
    { key: "ssh_key_path", label: "Private Key", type: "file", group: "SSH Tunnel", showIf: { key: "ssh_auth_type", equals: "key" } },
    { key: "ssh_password", label: "Passphrase / Password", type: "password", group: "SSH Tunnel", secret: true, showIf: { key: "use_ssh", equals: true } },

    { key: "use_ssl", label: "SSL / TLS", type: "toggle", group: "SSL / TLS" },
    {
      key: "ssl_mode", label: "Mode", type: "select", group: "SSL / TLS", default: "require",
      options: [
        { value: "disable", label: "Disable" },
        { value: "require", label: "Require" },
        { value: "verify-ca", label: "Verify CA" },
        { value: "verify-full", label: "Verify Full" },
      ],
      showIf: { key: "use_ssl", equals: true },
    },
    { key: "ssl_ca_path", label: "CA Cert", type: "file", group: "SSL / TLS", fileTypes: ["pem", "crt", "cert"], showIf: { key: "use_ssl", equals: true } },
    { key: "ssl_cert_path", label: "Client Cert", type: "file", group: "SSL / TLS", fileTypes: ["pem", "crt", "cert"], showIf: { key: "use_ssl", equals: true } },
    { key: "ssl_key_path", label: "Client Key", type: "file", group: "SSL / TLS", fileTypes: ["pem", "key"], showIf: { key: "use_ssl", equals: true } },
  ];
}

export const sqliteFields: ConfigField[] = [
  { key: "filename", label: "Path", type: "file", group: "File", required: true, placeholder: "/path/to/db.sqlite", fileTypes: ["db", "sqlite", "sqlite3"] },
];
