# Groups

A **group** bundles several integrations behind a single MCP URL. One endpoint
fronts many services, so an agent that needs more than one — say a database *and*
Linear *and* an SSH host — gets them all from one connection.

## Why use a group

Most MCP clients connect to a fixed set of servers. A group hands an agent one URL
that covers a whole workflow instead of wiring up each service separately. Add or
remove members and the agent's toolset changes — no client reconfiguration.

```
http://localhost:4242/mcp/<group-token>
        │
        ├─ metrics_db__query, metrics_db__list_tables, …   (member "Metrics DB")
        ├─ linear__list_issues, linear__create_issue, …    (member "Linear")
        └─ infra__run_command, infra__list_allowed_commands (member "Infra")
```

Each member's tools are prefixed with the member's name, so two databases that
both expose `query` never collide. **Give members clear, distinct names** — that
prefix is what the agent sees on every tool.

## Creating one

1. In Pluk's menu, choose **New ▸ New Group**. The group appears under *Groups* in
   the sidebar.
2. Open it and **Edit** to set a name, pick an environment, and toggle which
   integrations are members.
3. Optionally set per‑member **overrides** (e.g. a default team for a Linear
   member) in the same sheet.
4. Copy the group's MCP URL from its detail view and paste it into your AI client
   (Codex, opencode, Claude Desktop, Cursor, or Windsurf) — same as an
   [integration](./integrations.md), one URL for all members.

You build a group from integrations that already exist, so add those first.

## What carries over

Grouping changes routing, not permissions. Every member keeps its own
[policy](./integrations.md#staying-safe) and read‑only flag, and every call is
logged the same way. A read‑only database stays read‑only inside a group.

A member can also carry config **overrides** scoped to the group — for example a
Linear member with a per‑group default team — without changing the base
integration.

A group's token is a bearer secret like any [integration's](./integrations.md),
but its blast radius is larger: one group URL grants tool access to **every**
member at once. Only group services an agent should reach together, and keep the
URL to trusted local clients.

## Under the hood

*For contributors working on the server.*

- **Namespacing** — in group mode each member registers through a namespaced host
  that prefixes every tool, prompt, and resource with `<member-name-slug>__`. The
  slug lowercases the name and replaces non‑alphanumerics with `_`. Resource URIs
  are namespaced too (`schema://full` → `schema://metrics_db/full`).
  Single‑integration endpoints register on the bare server and are **not**
  prefixed.
- **Storage** — groups live in `~/.pluk/pluk.db` beside integrations. A group
  holds a `name`, an `environment` (default `production`), a unique `token`, and
  its `members` as `{ id, overrides? }`.
- **Routing** — a token resolves to *either* an integration or a group, so the
  `/mcp/<token>` URL works the same for both in your client.

## Related

- [Integrations](./integrations.md) — the members a group is built from.
- [Integration types](./integration-types.md) — the tools each member contributes.
