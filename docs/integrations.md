# Integrations

An **integration** is one service you've connected to Pluk — a single Postgres
database, a Linear workspace, an SSH host — exposed to AI agents at its own URL.

It's the unit you work with most. Everything an agent can reach comes from an
integration, or from a [group](./groups.md) of them.

## What you get

When you add an integration, Pluk gives it a private MCP endpoint:

```
http://localhost:4242/mcp/<token>
```

One URL per integration, so each agent only sees the one service you handed it.
Paste the URL into your MCP client (the detail view has ready‑made snippets for
opencode, Claude Desktop, Cursor, and Windsurf).

## Adding one

1. **Add** — Open Pluk from the menu bar, pick a [type](./integration-types.md),
   and fill in the form. The form only shows the fields that type needs, with
   sections that appear as you need them (SSH tunnel, SSL, …).
2. **Test** — Run *Test* from the detail view to confirm the credentials reach
   the service.
3. **Connect** — Copy the MCP URL into your AI client.
4. **Edit / Duplicate / Delete** — From the detail view. Duplicate is handy for
   the same service across environments.

Set an **environment** (`development` or `production`) as a cue, and turn on
**read‑only** for anything an agent touches in production.

## Staying safe

Every integration enforces its own **policy**, and every tool call — allowed or
blocked — is written to the in‑app activity log.

- **Read‑only mode** is the strongest, simplest guardrail: the policy layer
  refuses writes before they reach the service. Turn it on for production.
- Databases get a SQL policy that classifies each statement; Linear, Sentry, and
  SSH get a read/write gate (SSH also checks a strict command allowlist).

For the full picture of what each type allows, see
[integration types](./integration-types.md).

## Under the hood

*For contributors working on the server or adapters.*

An integration is a thin, uniform record; everything service‑specific lives in
`config`:

| Field | Meaning |
| --- | --- |
| `name` | Label. Becomes the tool prefix inside a [group](./groups.md). |
| `type` | The adapter id (`postgres`, `linear`, …). Resolves the integration to its adapter. |
| `config` | Per‑adapter settings as JSON; holds secrets. |
| `environment` | `development` (default) or `production`. |
| `read_only` | `1` blocks writes via the policy layer. |
| `token` | Unique; routes the MCP URL to this integration. |

- **Storage** — `~/.pluk/pluk.db` (SQLite), shared by the Swift app and the Bun
  server. Secret‑flagged fields are never sent back to the UI after saving.
- **Resolution** — a request to `/mcp/<token>` resolves the token to an
  integration (or a group), looks up the adapter by `type`, and builds a
  per‑session MCP server from `config`.
- **Adapters** — one module per service in `pluk/src/adapters/`, implementing the
  `Adapter` contract. Declaring `configFields` is enough for the macOS form to
  render the type; nothing else needs editing.

## Related

- [Integration types](./integration-types.md) — the catalog and each type's tools.
- [Groups](./groups.md) — expose several integrations through one URL.
