# pluk

pluk exposes your services — databases, Linear, and more — as local MCP endpoints for AI tools. Each service is a pluggable adapter; every saved integration gets its own MCP URL.

## Adapters

An adapter (`src/adapters/`) declares its config fields, a connectivity test, and the MCP tools it serves. Register it in `src/adapters/index.ts` and it shows up everywhere — including the macOS form, which renders from `GET /api/adapters`. Built in today:

- **Databases** (`adapters/sql/`) — Postgres, MySQL, SQLite. SQL statement-policy engine, SSH tunneling, SSL.
- **Linear** (`adapters/linear/`) — issues, teams, comments over the GraphQL API. Read/write action policy.

## Run

```bash
bun install
bun run server
```

The server listens on `http://localhost:4242`.

Each integration gets its own MCP URL:

```text
http://localhost:4242/mcp/<token>
```

## Policy & Safety

Every integration carries its own policy, enforced on each tool call and recorded in a local activity log.

**Databases** — treat production as read-heavy inspection. Tell AI agents to:

- prefer `SELECT` queries only
- add explicit `LIMIT` clauses
- avoid broad scans, migrations, locks, and writes
- ask before running expensive queries

For high-risk DBs, enable read-only mode in the integration settings — pluk blocks common write statements when it's on. PostgreSQL also uses short connect/query timeouts so failed tunnels and slow statements do not hang the UI.

**Linear** (and other API adapters) — a read/write action policy. Read-only blocks mutating actions (create issue, comment); read & write allows them.

## SSH And Cloudflare Access

pluk reads `~/.ssh/config`. Hosts with `ProxyCommand` use the system `ssh` client for forwarding, which supports Cloudflare Access and existing SSH agent/keychain setup.

Example:

```sshconfig
Host app4-ssh-infra.example.com
  ProxyCommand /opt/homebrew/bin/cloudflared access ssh --hostname %h
  IdentityAgent "~/Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock"
```

## MCP Clients

Use the config snippet shown in the macOS menu bar app. Keep one MCP URL per integration so the agent only sees the service you intend.
