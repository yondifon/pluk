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
| [`herd`](#laravel-herd) | local‑dev | action | 3 |

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

## Laravel Herd

Give a branch its own local URL. **Action policy** — `list_sites` is read,
`create_site` is write, `destroy_site` is delete, so a read‑only integration can
only look.

*Agent hint: run `list_sites` to see what's already up.*

| Field | Notes |
| --- | --- |
| `app_path` | The Laravel app's git repository — the folder Herd already serves (required). |
| `site` | Base site name; defaults to the app folder name. |
| `tld` | Default `test`. |
| `secure` | Serve feature sites over HTTPS. Default on. |
| `worktree_root` | Where worktrees are created; defaults to `../<app>-worktrees`. |
| `link_paths` | Untracked paths symlinked from the app (comma separated). Default `vendor, node_modules, public/build`. |
| `env_file` | Copied into the worktree with `APP_URL` rewritten. Default `.env`; blank to skip. |
| `herd_bin` | Herd CLI path; defaults to Herd's bundled binary. |

| Tool | Access | What it does |
| --- | --- | --- |
| `list_sites` | read | List the feature sites — feature, branch, URL, worktree path. |
| `create_site` | write | Worktree + links + `herd link`; returns the URL to test. |
| `destroy_site` | delete | `herd unlink` + remove the worktree. |

`create_site checkout-fix` on an app at `~/Herd/app` (served at `app.test`):

1. `git worktree add ~/Herd/app-worktrees/checkout-fix` on branch `checkout-fix`
   — created from `HEAD` unless the branch already exists, or `base` says
   otherwise. Tracked files only.
2. Symlinks each `link_paths` entry back to the app, so the worktree boots
   without a `composer install` / `npm install`. Paths already present in the
   worktree, or missing from the app, are reported as skipped rather than
   clobbered.
3. Copies `.env` with `APP_URL=https://checkout-fix.app.test`. It's copied, not
   linked, so repointing the URL can't touch the app everyone else is using —
   the DB and every other credential stay shared.
4. `herd link checkout-fix.app --secure` → **https://checkout-fix.app.test**.

`destroy_site` unsecures, unlinks, and removes the worktree; it never deletes the
branch. Pass `force` when the worktree has uncommitted changes. Teardown steps
are best‑effort — a link that's already gone is reported, not fatal.

Both sides of a feature site share one database. Migrations run from a worktree
hit the app's DB.

---

## Related

- [Integrations](./integrations.md) — the shared lifecycle, policy, and MCP URL.
- [Groups](./groups.md) — bundle several of these behind one URL.
