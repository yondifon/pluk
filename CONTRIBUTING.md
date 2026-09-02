# Contributing

## Dev setup

Frontend:

```bash
cd ui
bun install
```

Server/Host (Rust):

```bash
cargo fetch
cargo build --workspace
```

Then run `make dev` to start the full app, or test components individually:

- `make dev` — starts the Tauri host and Vite dev server
- `cargo run -p pluk-host` — runs the Tauri app directly
- `cd crates/pluk-core && cargo run --bin pluk-server` — runs the MCP server standalone

## Tests and types

Run `make test` to run all tests. The suite covers:

- **Adapters** (in Rust, `crates/pluk-core/src/adapters/`) — GitHub CLI, Linear, Redis, Sentry, Slack, Spark, SSH, and the SQL family
- **Store layer** — SQLite, migrations, and concurrency handling
- **Platform layer** — MCP config injection, tray/window management, update checking
- **Policy engines** — SQL policy (`sql.rs`) and SSH command policy (`ssh/policy.rs`) get direct test coverage — they're what stands between an agent and a production database or shell, so changes there need tests

## Code style

- Use **Bun**, not `npm`/`yarn`/`pnpm` (`bun install`, `bun test`, `bun run`).
- Keep changes surgical and match the surrounding style.
- Add tests for policy and driver behavior — these guard what agents are allowed to run.

Comments are sparse and speak to *why*, not *what*. The code itself carries the
what. A comment exists only when the reason would not survive the next reader.

Prose in the codebase is plain and concrete — reasons over rules, active voice,
short sentences. Doc strings and tool descriptions follow the same voice.

## Adding an adapter

Adapters live in Rust now (`crates/pluk-core/src/adapters/`). The frontend auto-discovers adapters via the `/api/adapters` endpoint, so declaring config fields in the adapter is enough for the UI to render that adapter's settings.

## License

Contributions are accepted under the project license ([AGPL-3.0](LICENSE.md)).
By submitting a contribution you agree to license it under those terms.
