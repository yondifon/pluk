# Pluk

Pluk turns your saved database connections into local [MCP](https://modelcontextprotocol.io) endpoints, so AI tools can query your databases safely from your own machine. Nothing leaves your laptop: the server runs on `localhost`, connections are stored locally, and a policy engine keeps agents read-heavy and out of trouble.

It ships as a macOS menu bar app with an embedded server. You manage connections in the UI, copy one MCP URL per database, and paste it into your AI client.

## How it works

Pluk has two parts that the `Makefile` builds and bundles together:

- **`swift/`** — a native macOS menu bar app (SwiftUI, macOS 14+). It manages connections, shows query logs, and supervises the server process.
- **`pluk/`** — a [Bun](https://bun.sh) + TypeScript server. It speaks MCP over streamable HTTP on `http://localhost:4242`, connects to Postgres / MySQL / SQLite, handles SSH tunneling, and enforces the query policy.

The app launches the server, which exposes each saved connection at `http://localhost:4242/mcp/<token>`.

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

1. Open Pluk from the menu bar and add a database connection (host, port, credentials, optional SSH host and read-only flag).
2. Test the connection from the detail view.
3. Copy the connection's MCP URL — one URL per database, so each agent only sees the connection you intend.
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

### Query safety

Treat production connections as read-heavy inspection tools. Tell your agents to prefer `SELECT`, add explicit `LIMIT` clauses, avoid broad scans and writes, and ask before running expensive queries. For high-risk databases, enable **read-only mode** on the connection — Pluk blocks common write statements when it's on. Postgres connections also use short connect/query timeouts so failed tunnels don't hang the UI.

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
   - **Server / MCP / policy / drivers** → `pluk/src/` (see `pluk/src/mcp/policy.ts` and `pluk/src/db/`)
   - **App / UI** → `swift/Sources/`

4. Verify your change end to end with `make dev`.

5. Open a pull request against `main` with a clear description of the change and why it matters.

Conventions:

- Use **Bun**, not `npm`/`yarn`/`pnpm` (`bun install`, `bun test`, `bun run`).
- Keep changes surgical and match the surrounding style.
- Add tests for policy and driver behavior — these guard what agents are allowed to run.

## License

Copyright © 2025 Pluk. See the repository for license details.
