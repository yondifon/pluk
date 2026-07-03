# Integration types

Each integration is one **type**, backed by a pluggable **adapter**. The type
determines the config form, how the policy layer treats it, and which tools the
agent gets. Pluk ships with six types across four categories.

| Type | Category | Policy | Tools |
| --- | --- | --- | --- |
| [`postgres`](#postgres) | database | SQL | 12 |
| [`mysql`](#mysql) | database | SQL | 12 |
| [`sqlite`](#sqlite) | database | SQL | 12 |
| [`linear`](#linear) | issue‑tracker | action | 6 |
| [`sentry`](#sentry) | observability | action | 5 |
| [`ssh`](#ssh) | infrastructure | action | 2 |

The **Policy** column is how Pluk decides whether a given call is allowed:

- **SQL** — every statement is classified; in read‑only mode, writes are blocked.
- **action** — each tool is tagged read or write; read‑only blocks the write
  tools. SSH layers a command allowlist on top.

> **Adding a type** is one module — implement the `Adapter` contract in
> `pluk/src/adapters/` and register it. The macOS form and MCP layer pick it up
> with no other changes.

---

## Databases

`postgres`, `mysql`, and `sqlite` share one SQL adapter family, one **SQL
policy** engine, and the same toolset. They differ only in how you connect.

### Shared tools

| Tool | What it does |
| --- | --- |
| `query` | Run a SQL query against the database (subject to policy). |
| `sample_table` | Preview rows from a table without writing SQL. |
| `explain_query` | Show a query's execution plan without running it. |
| `describe_table` | Get column definitions for a table. |
| `list_tables` | List all tables. |
| `list_schemas` | List all schemas / databases. |
| `list_relationships` | List foreign‑key relationships between tables. |
| `search_schema` | Find tables or columns matching a term. |
| `table_stats` | Cheap table statistics (estimated rows, size, indexes). |
| `export_query` | Run a query and save results to a local CSV or JSON file. |
| `run_saved_query` | Run a saved query by name. |
| `list_saved_queries` | List saved queries for this integration. |

With **read‑only** on, the policy engine blocks any write statement before it
reaches the database.

### Postgres

Network database. Config groups: **Connection**, **SSH Tunnel**, **SSL / TLS**.

| Field | Notes |
| --- | --- |
| `host` | Default `localhost`. |
| `port` | Default `5432`. |
| `user`, `password`, `database` | Standard credentials. `password` is secret. |
| `socket_path` | Optional; leave empty for TCP. |
| SSH Tunnel (`use_ssh`, `ssh_host`, …) | Tunnel the connection through a bastion. Auth: agent, private key, or password. |
| SSL / TLS (`use_ssl`, `ssl_mode`, …) | Modes: disable, require, verify‑ca, verify‑full. Optional CA / client cert / key. |

Postgres uses short connect/query timeouts so a failed tunnel doesn't hang the UI.

### MySQL

Same network fields as Postgres (SSH Tunnel and SSL / TLS sections included);
default port `3306`.

### SQLite

Local file — no network, no tunnels.

| Field | Notes |
| --- | --- |
| `filename` | Path to the `.sqlite` / `.db` / `.sqlite3` file (required). |

---

## Linear

Issue tracker over the Linear GraphQL API. **Action policy.**

*Agent hint: start with `list_issues` or `search_issues` before writing.*

| Field | Notes |
| --- | --- |
| `api_key` | Linear API key (`lin_api_…`), required, secret. |
| `team_key` | Optional default team (e.g. `ENG`) that scopes `list_issues`. |

| Tool | Access | What it does |
| --- | --- | --- |
| `list_issues` | read | List issues, optionally scoped to a team. |
| `get_issue` | read | Get one issue by id or identifier (e.g. `ENG-123`). |
| `search_issues` | read | Search issues by text in title or description. |
| `list_teams` | read | List teams (id, name, key). |
| `create_issue` | write | Create an issue (needs a team id). |
| `comment` | write | Add a comment to an issue. |

Read‑only blocks `create_issue` and `comment`.

---

## Sentry

Error monitoring and structured log search over the Sentry API (sentry.io or self‑hosted). **Action policy.**

*Agent hint: start with `list_issues`, then `latest_event` for stack traces; use `query_logs` when you need logs instead of issue groups.*

| Field | Notes |
| --- | --- |
| `auth_token` | Sentry auth token (`sntrys_…` or a personal token), required, secret. |
| `org_slug` | Organization slug (required). |
| `project_slug` | Optional default project that scopes `list_issues`. |
| `base_url` | Default `https://sentry.io`; set for self‑hosted. |

| Tool | Access | What it does |
| --- | --- | --- |
| `list_projects` | read | List projects in the org. |
| `list_issues` | read | List issues, newest first; scoped to the default project if set. |
| `get_issue` | read | Get one issue by id or short id (e.g. `BACKEND-1A`). |
| `latest_event` | read | Latest event for an issue, with stacktrace and tags. |
| `list_events` | read | Recent project error events, optionally with full event bodies. |
| `query_logs` | read | Query Sentry structured logs through Explore's `logs` dataset. |
| `update_issue` | write | Resolve, ignore, or reopen an issue. |

Read‑only blocks `update_issue`.

---

## SSH

Run shell commands on a remote host. **Action policy**, plus a strict command
allowlist on top.

*Agent hint: run `list_allowed_commands` before remote changes.*

| Field | Notes |
| --- | --- |
| `host` | Hostname or an `~/.ssh/config` alias (required). |
| `port` | Default `22`. |
| `user` | Defaults to your local username. |
| `auth_type` | `agent`, `key`, or `password`. |
| `key_path` | Private key file (when `auth_type` is `key`). |
| `password` | Passphrase / password (secret). |

| Tool | Access | What it does |
| --- | --- | --- |
| `run_command` | read/write | Run a shell command; checked against the allowlist first. |
| `list_allowed_commands` | read | Show which commands this integration may run. |

Read commands (e.g. `docker compose ps`) need read access; state‑changing ones
(e.g. `docker compose up`) need write. Anything off the allowlist is blocked
regardless.

The allowlist is built in and conservative by default — run
`list_allowed_commands` to see it. It's not configured per integration, and it's
a guardrail against an agent wandering off, not a hard boundary against a
determined adversary. It's widened in code as real workflows need it.

### SSH config, tunnels, and Cloudflare Access

Pluk reads `~/.ssh/config`. Hosts with a `ProxyCommand` use the system `ssh`
client for forwarding — which supports Cloudflare Access and your existing SSH
agent / keychain. The same applies to database **SSH Tunnel** sections.

```sshconfig
Host app4-ssh-infra.example.com
  ProxyCommand /opt/homebrew/bin/cloudflared access ssh --hostname %h
  IdentityAgent "~/Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock"
```

---

## Related

- [Integrations](./integrations.md) — the shared lifecycle, policy, and MCP URL.
- [Groups](./groups.md) — bundle several of these behind one URL.
