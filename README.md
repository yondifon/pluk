# Pluk

Pluk turns the services you already use — databases, [Linear](https://linear.app), and more — into local [MCP](https://modelcontextprotocol.io) endpoints, so AI tools can use them safely from your own machine. Nothing leaves your laptop: the server runs on `localhost`, integrations are stored locally, and a per-integration policy engine keeps agents in bounds.

Each service is a pluggable **adapter**. Pluk ships with database adapters (Postgres / MySQL / SQLite / MongoDB) and a Linear adapter; adding another is one module — no changes to the app, server, or UI.

It ships as a macOS menu bar app with an embedded server. You add an integration in the UI, copy its MCP URL, and paste it into your AI client.

## How it works

Pluk is a [Tauri](https://tauri.app) app (Rust host + TypeScript webview) that bundles an MCP server:

- **Host (Rust, `crates/pluk-host/`)** — manages the menu bar, window, integrations, and activity logs. It runs the MCP server in-process on `http://localhost:4242`.
- **Server (Rust, `crates/pluk-core/` + `crates/pluk-store/`)** — resolves each integration to its adapter (databases over SSH/SSL, Linear over its GraphQL API, …), enforces that integration's policy, and speaks MCP over HTTP.
- **Frontend (TypeScript, `ui/src/`)** — rendered in a webview from the Tauri host. It manages the UI for adding, editing, and testing integrations.

The app launches and exposes each saved integration at `http://localhost:4242/mcp/<token>`.

## Prerequisites

- **macOS 14** or later
- **[Rust](https://www.rust-lang.org/tools/install)** — for building the host and server
- **[Bun](https://bun.sh)** — `curl -fsSL https://bun.sh/install | bash` (for the frontend)
- **Make** (ships with Xcode Command Line Tools)

## Install locally

Clone, build, and install the app to `/Applications` in one step:

```bash
git clone git@github.com:yondifon/pluk.git
cd pluk
make install
```

`make install` builds the Tauri app in release mode, assembles `Pluk.app`, copies it to `/Applications`, and launches it. Pluk then lives in your menu bar.

## Develop locally

To iterate on the app:

```bash
make dev
```

This starts the Tauri host (which runs the MCP server in-process) and the Vite dev server for the frontend.

To run just the server for testing (useful for working on the MCP / adapter side):

```bash
cd crates/pluk-core
cargo run --bin pluk-server
```

The server listens on `http://localhost:4242`. Health check: `curl http://localhost:4242/health`.

### Make targets

| Target | What it does |
| --- | --- |
| `make dev` | Run the app in dev mode (Tauri host + Vite frontend) |
| `make build` | Build the frontend and compile a debug binary |
| `make bundle` | Build Tauri bundles for distribution |
| `make bundle-signed` | Build and sign/notarize via 1Password |
| `make install` | Build, install to `/Applications`, and launch |
| `make publish` | Full release: bump version, build, sign, notarize, publish to GitHub |
| `make test` | Run all tests (cargo test --workspace) |
| `make lint` | Run clippy and typecheck |
| `make clean` | Remove `dist/`, `target/`, and build artifacts |

## Use it

1. Open Pluk from the menu bar and add an integration. Pick a type — a database (host, port, credentials, optional SSH and read-only flag), MongoDB (connection string), or Linear (API key) — and the form shows just that adapter's settings.
2. Test the integration from the detail view.
3. Copy its MCP URL — one URL per integration, so each agent only sees what you intend.
4. Add it to your MCP client. Examples:

   Codex (`~/.codex/config.toml`):

   ```toml
   [mcp_servers.my-prod-db]
   url = "http://localhost:4242/mcp/<token>"
   ```

   Or via CLI:

   ```bash
   codex mcp add my-prod-db --url http://localhost:4242/mcp/<token>
   ```

   opencode (`opencode.jsonc`):

   ```jsonc
   {
     "$schema": "https://opencode.ai/config.json",
     "mcp": {
       "my-prod-db": {
         "type": "remote",
         "enabled": true,
         "url": "http://localhost:4242/mcp/<token>",
         "oauth": false
       }
     }
   }
   ```

### Policy & safety

Every integration carries its own policy, and all access is recorded in a local activity log.

- **Databases** — a SQL policy engine classifies each statement. Treat production as read-heavy: prefer `SELECT`, add explicit `LIMIT`s, avoid broad scans and writes. Enable **read-only mode** and Pluk blocks write statements; Postgres also uses short connect/query timeouts so failed tunnels don't hang the UI.
- **MongoDB** — reading documents and inspecting collections is on; inserting, updating and deleting stay off until you turn them on. Queries that run server-side JavaScript or write into another collection are refused, an update or delete needs a filter, and one read returns at most 1000 documents.
- **Linear** (and other API adapters) — a coarse read/write policy. Read-only blocks mutating actions (create issue, comment); read & write allows them.

### SSH and Cloudflare Access

Pluk reads `~/.ssh/config`. Hosts with a `ProxyCommand` use the system `ssh` client for forwarding, which supports Cloudflare Access and your existing SSH agent / keychain setup:

```sshconfig
Host app4-ssh-infra.example.com
  ProxyCommand /opt/homebrew/bin/cloudflared access ssh --hostname %h
  IdentityAgent "~/Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock"
```

## Contributing

Contributions are welcome — see [CONTRIBUTING.md](./CONTRIBUTING.md) for dev setup, tests, and code style. Fork and clone as usual:

```bash
git clone git@github.com:yondifon/pluk.git
cd pluk
```

- **Host (Rust)** — `crates/pluk-host/` (tray, window, updater)
- **Core/Server (Rust)** — `crates/pluk-core/` (MCP server), `crates/pluk-store/` (SQLite store)
- **Adapters & drivers (Rust)** — `crates/pluk-core/src/adapters/`, `crates/pluk-core/src/drivers/`
- **Frontend (TypeScript)** — `ui/src/`

Verify a change end to end with `make dev`, then open a pull request against `main` with a clear description of the change and why it matters.

## License

pluk is licensed under the [GNU Affero General Public License v3.0](LICENSE.md).
You may use, modify, and share it freely. If you modify pluk and offer it to
others over a network, you must publish your modified source under the same
license.

Contact: `yong@malico.me`
