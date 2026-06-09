# pluk

pluk exposes saved database connections as local MCP endpoints for AI tools.

## Run

```bash
bun install
bun run server
```

The server listens on `http://localhost:4242`.

Each connection gets its own MCP URL:

```text
http://localhost:4242/mcp/<token>
```

## Query Safety

Production connections should be treated as read-heavy inspection tools.

Tell AI agents to:

- prefer `SELECT` queries only
- add explicit `LIMIT` clauses
- avoid broad scans, migrations, locks, and writes
- ask before running expensive queries

For high-risk DBs, enable read-only mode in the connection settings. pluk blocks common write statements when read-only mode is on.

PostgreSQL connections also use short connect/query timeouts so failed tunnels and slow statements do not hang the UI.

## SSH And Cloudflare Access

pluk reads `~/.ssh/config`. Hosts with `ProxyCommand` use the system `ssh` client for forwarding, which supports Cloudflare Access and existing SSH agent/keychain setup.

Example:

```sshconfig
Host app4-ssh-infra.example.com
  ProxyCommand /opt/homebrew/bin/cloudflared access ssh --hostname %h
  IdentityAgent "~/Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock"
```

## MCP Clients

Use the config snippet shown in the macOS menu bar app. Keep one MCP URL per database so the agent only sees the intended connection.
