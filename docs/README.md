# Pluk Documentation

Pluk turns the services you already use — databases, [Linear](https://linear.app),
[Sentry](https://sentry.io), SSH hosts, and more — into local
[MCP](https://modelcontextprotocol.io) endpoints that AI tools can connect to.
MCP (Model Context Protocol) is the open standard AI clients use to discover and
call tools; Pluk speaks it on `localhost` so each service shows up as a set of
tools your agent can use.

**What stays local, what doesn't.** Pluk itself runs entirely on your machine —
the server is on `localhost`, your config and credentials live in `~/.pluk/`, and
a per‑integration policy keeps agents in bounds. The *services* it connects to may
be remote: a Linear or Sentry integration talks to their APIs, and a database or
SSH integration reaches whatever host you point it at. Pluk is the local broker,
not a guarantee that a given service is local.

These docs explain the three concepts you work with:

| Concept | What it is | Doc |
| --- | --- | --- |
| **Integration** | One configured service (a database, Linear workspace, …) exposed at its own MCP URL. | [integrations.md](./integrations.md) |
| **Integration type** | The kind of service an integration connects to — its adapter, config, and tools. | [integration-types.md](./integration-types.md) |
| **Group** | Several integrations bundled behind a single MCP URL, with tools namespaced per member. | [groups.md](./groups.md) |

## The mental model

```
                ┌─────────────────────────── Pluk (menu bar app + localhost server) ──┐
                │                                                                      │
  AI client ──► │  /mcp/<token>  ─►  Integration  ─►  Adapter  ─►  real service        │
  (opencode,    │                     (Postgres prod)   (sql)       your database      │
   Claude, …)   │                                                                      │
                │  /mcp/<token>  ─►  Group  ─►  many integrations (namespaced tools)    │
                └──────────────────────────────────────────────────────────────────────┘
```

- An **integration** is one service plus its credentials, policy, and a unique
  token. Its tools are exposed at `http://localhost:4242/mcp/<token>`. An
  **adapter** is the built‑in module that knows how to talk to that kind of
  service (the `sql` adapter for databases, the `linear` adapter, …).
- A **group** is a list of integrations behind one token. Its endpoint exposes
  every member's tools at once, each prefixed with the member's name so they
  never collide.
- An AI client only ever sees the tokens you hand it — one URL per integration or
  group, so each agent sees exactly what you intend.

The token in the URL is a bearer secret: anyone with the URL can use that
integration. Treat it like a password — it stays on your machine, so only hand it
to local clients you trust.

## Where things live

| Path | What it holds |
| --- | --- |
| `~/.pluk/pluk.db` | Integrations and groups, plus app data like saved queries (SQLite, shared by the app and server). |
| `~/.pluk/pluk.log` | Server debug log. |
| Activity log (in‑app) | Every query/command an agent ran, allowed or blocked. |

Secrets (passwords, API keys, tokens) are stored in the local config and are
never echoed back to the UI once saved.

## See also

- The repository [`README.md`](../README.md) — install, build, and contribute.
