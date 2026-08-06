# Contributing

## Dev setup

```bash
cd pluk
bun install
bun test
bunx tsc --noEmit
```

- `make dev` runs the Swift app from source (`swift run`), which starts the app and the bundled server.
- `make install` builds and installs the full app bundle to `/Applications`.

## Tests and types

`bun test` (from `pluk/`) must pass. `bunx tsc --noEmit` must be clean.

The suite covers the adapters (GitHub, Herd, Linear, Redis, Sentry, Slack, Spark,
SSH, and the SQL family), the DB layer (SQLite, SSH tunneling and routing,
timestamp handling), and the MCP transport (server, connection pool, integration
grouping). The SQL policy engine (`mcp/policy.ts`) and the SSH command policy
(`adapters/ssh/policy.ts`) get direct test coverage — they're what stands between
an agent and a production database or shell, so changes there need tests.

## Code style

- Use **Bun**, not `npm`/`yarn`/`pnpm` (`bun install`, `bun test`, `bun run`).
- Keep changes surgical and match the surrounding style.
- Add tests for policy and driver behavior — these guard what agents are allowed to run.

Comments are sparse and speak to *why*, not *what*. The code itself carries the
what. A comment exists only when the reason would not survive the next reader.

Prose in the codebase is plain and concrete — reasons over rules, active voice,
short sentences. Doc strings and tool descriptions follow the same voice.

## Adding an adapter

Adding a service means implementing the `Adapter` contract
(`pluk/src/adapters/types.ts`) and registering it in `pluk/src/adapters/index.ts`.
Nothing else — store, MCP transport, REST layer, or Swift UI — needs editing.
Declaring `configFields` is enough for the macOS add/edit form to render that
adapter's settings; the DB family lives in `adapters/sql/`, Linear in
`adapters/linear/`.

## License

Contributions are accepted under the project license ([AGPL-3.0](LICENSE.md)).
By submitting a contribution you agree to license it under those terms.
