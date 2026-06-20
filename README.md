# Pluk

Pluk turns the services you already use — databases, [Linear](https://linear.app), and more — into local [MCP](https://modelcontextprotocol.io) endpoints, so AI tools can use them safely from your own machine. Nothing leaves your laptop: the server runs on `localhost`, integrations are stored locally, and a per-integration policy engine keeps agents in bounds.

Each service is a pluggable **adapter**. Pluk ships with database adapters (Postgres / MySQL / SQLite) and a Linear adapter; adding another is one module — no changes to the app, server, or UI.

It ships as a macOS menu bar app with an embedded server. You add an integration in the UI, copy its MCP URL, and paste it into your AI client.

## How it works

Pluk has two parts that the `Makefile` builds and bundles together:

- **`swift/`** — a native macOS menu bar app (SwiftUI, macOS 14+). It manages integrations, shows activity logs, and supervises the server process. The add/edit form renders itself from the server's adapter catalog, so new adapters appear automatically.
- **`pluk/`** — a [Bun](https://bun.sh) + TypeScript server. It speaks MCP over streamable HTTP on `http://localhost:4242`, resolves each integration to its adapter (databases over SSH/SSL, Linear over its GraphQL API, …), and enforces that integration's policy.

The app launches the server, which exposes each saved integration at `http://localhost:4242/mcp/<token>`.

## Documentation

Concept docs live in [`docs/`](./docs/):

- [Integrations](./docs/integrations.md) — what an integration is, its lifecycle, policy, and MCP URL.
- [Integration types](./docs/integration-types.md) — every service type and the tools it exposes.
- [Groups](./docs/groups.md) — bundle several integrations behind one URL.

## Prerequisites

- **macOS 14** or later
- **[Bun](https://bun.sh)** — `curl -fsSL https://bun.sh/install | bash`
- **Swift toolchain** — install Xcode or the Command Line Tools (`xcode-select --install`)
- **Make** (ships with the Command Line Tools)

## Install locally

Clone, build, and install the app to `/Applications` in one step:

```bash
git clone git@github.com:yondifon/pluk.git
cd pluk
make install
```

`make install` compiles the Bun server into a standalone binary, builds the Swift app in release mode, assembles `Pluk.app`, copies it to `/Applications`, and launches it. Pluk then lives in your menu bar.

## Develop locally

To iterate on the Swift app with the server running from source:

```bash
make dev
```

This runs `swift run` from `swift/`, which starts the app and the bundled server.

To run just the server (useful for working on the TypeScript / MCP side):

```bash
cd pluk
bun install
bun run server
```

The server listens on `http://localhost:4242`. Health check: `curl http://localhost:4242/health`.

### Make targets

| Target | What it does |
| --- | --- |
| `make dev` | Run the app from source (`swift run`) |
| `make server` | Compile the Bun server to `dist/pluk-server` |
| `make swift-build` | Build the Swift app in release mode |
| `make bundle` | Assemble `dist/Pluk.app` (server + app + `Info.plist`) |
| `make install` | Build, install to `/Applications`, and launch |
| `make zip` | Bundle and zip the app for distribution |
| `make clean` | Remove `dist/` and clean the Swift package |

Set `APPLE_IDENTITY` to code-sign the bundle: `make bundle APPLE_IDENTITY="Developer ID Application: …"`.

## Use it

1. Open Pluk from the menu bar and add an integration. Pick a type — a database (host, port, credentials, optional SSH and read-only flag) or Linear (API key) — and the form shows just that adapter's settings.
2. Test the integration from the detail view.
3. Copy its MCP URL — one URL per integration, so each agent only sees what you intend.
4. Add it to your MCP client. Example (`opencode.jsonc`):

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
- **Linear** (and other API adapters) — a coarse read/write policy. Read-only blocks mutating actions (create issue, comment); read & write allows them.

### SSH and Cloudflare Access

Pluk reads `~/.ssh/config`. Hosts with a `ProxyCommand` use the system `ssh` client for forwarding, which supports Cloudflare Access and your existing SSH agent / keychain setup:

```sshconfig
Host app4-ssh-infra.example.com
  ProxyCommand /opt/homebrew/bin/cloudflared access ssh --hostname %h
  IdentityAgent "~/Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock"
```

## Contributing

Contributions are welcome. The fastest loop:

1. Fork and clone:

   ```bash
   git clone git@github.com:yondifon/pluk.git
   cd pluk
   ```

2. Install server dependencies and run the test suite:

   ```bash
   cd pluk
   bun install
   bun test
   ```

3. Hack on the relevant side:
   - **Adapters** (add a new service) → `pluk/src/adapters/` — implement the `Adapter` contract (`types.ts`) and register it in `adapters/index.ts`. The DB family lives in `adapters/sql/`, Linear in `adapters/linear/`. Declaring `configFields` is enough for the macOS form to render it; nothing else needs editing.
   - **Server / MCP / policy** → `pluk/src/` (SQL policy in `mcp/policy.ts`, action policy in `mcp/actionPolicy.ts`, drivers in `pluk/src/db/`)
   - **App / UI** → `swift/Sources/`

4. Verify your change end to end with `make dev`.

5. Open a pull request against `main` with a clear description of the change and why it matters.

Conventions:

- Use **Bun**, not `npm`/`yarn`/`pnpm` (`bun install`, `bun test`, `bun run`).
- Keep changes surgical and match the surrounding style.
- Add tests for policy and driver behavior — these guard what agents are allowed to run.

## License

Copyright © 2025 Pluk. See the repository for license details.
