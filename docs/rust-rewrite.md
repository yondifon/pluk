# Rust rewrite

Pluk is being rewritten in Rust end to end (decided 2026-08-25): one process — a Tauri host running the MCP server in-process with a TypeScript webview frontend. Full behavioural parity with the SwiftUI app is the goal. `swift/` and `pluk/src` stay in place and buildable as the port source until parity lands.

This file is the skeleton contract for the rewrite chain (tasks R01–R23). Later tasks fill the crates inside this shape; nothing here is open for re-deriving.

## Crate map

| Crate | Path | Purpose | Filled by |
| --- | --- | --- | --- |
| `pluk-core` | `crates/pluk-core` | Shared types, errors, platform abstraction (`platform` module). Depends on no workspace crate. | R01 (skeleton), later tasks extend |
| `pluk-store` | `crates/pluk-store` | SQLite persistence: integrations, groups, query log, saved queries. | R02 |
| `pluk-policy` | `crates/pluk-policy` | SQL policy engine: statement classification, read/write gating. | R03 |
| `pluk-adapters` | `crates/pluk-adapters` | Adapter framework + every adapter (databases, SSH hosts, APIs, CLIs). The `herd` adapter is dropped, not ported. | R04, R09–R14 |
| `pluk-server` | `crates/pluk-server` | MCP serving: HTTP routes, SSE transport, tool registration. Library crate; thin `pluk-serverd` binary target kept for a possible future headless server — nothing uses it yet. | R05 |
| `pluk-host` | `crates/pluk-host` | Tauri application: window, tray, webview, in-process server startup binding `http://localhost:4242`. | R01 (shell), R15–R17 |

## Architecture decisions (settled)

- **One process.** The MCP server runs inside the Tauri host. No spawned child server, no port-orphan killing, no cross-process health polling. The Swift `ServerManager` disappears entirely.
- **MCP endpoint stays `http://localhost:4242/mcp/<token>`.** The host binds port 4242 at startup whether or not a window is open, so existing agent configs keep working.
- **Frontend is TypeScript** in the Tauri webview (not Rust WASM), living in `ui/`.
- **Targets are macOS and Linux.** Windows is deferred — do not write Windows code, but do not make it impossible.
- **`only`-projection** response shaping is settled in `pluk/src/adapters/onlyProjection.ts` (post `ba26ec4`); adapters must mirror it. It does not apply to `spark`, which returns CLI text verbatim.

### SQLite journal mode (decided by R02)

`pluk.db` is opened by two processes during the port — the Rust store and the SwiftUI app (`pluk/src/store/*` never set a journal mode, so until now every open ran the default rollback journal and any concurrent read/write collided with `SQLITE_BUSY`). `pluk-store` now sets, on every open:

- **`journal_mode=WAL`** — readers never block the writer across processes, which is exactly this file's access pattern (the Swift app reads while the server writes log rows). The mode is persistent in the database header: once the Rust store has opened the file once, the Swift app's plain `sqlite3_open` picks WAL up unchanged — no Swift-side changes needed. Both consumers bundle/ship modern SQLite, so WAL support is a given. The `-wal`/`-shm` sidecar files live next to `pluk.db`; they are recovered automatically after a crash.
- **`synchronous=NORMAL`** — the standard WAL pairing. Safe against application crashes; on OS/power loss the final seconds of commits may be lost. Accepted for an audit log that regenerates from live traffic.
- **`busy_timeout=5000ms`** — backstop for the rare moments WAL still contends (checkpoint starvation, another process mid-migration): wait briefly instead of erroring.

Schema moves only through `pluk-store`'s `user_version` migration ladder; the TypeScript try/catch ALTER loop stays authoritative for its own process but new databases get version 1 from the Rust side.

## Platform abstraction contract

Everything platform-varying resolves through `pluk_core::platform` — no scattered `cfg` attributes elsewhere. Functions return paths/capabilities, not constants. macOS impl: `platform/macos.rs`; Linux impl: `platform/linux.rs`.

| Function | macOS | Linux |
| --- | --- | --- |
| `home_dir()` | `$HOME` via `std::env::home_dir` | same |
| `data_dir()` | `PLUK_DATA_DIR` if set, else `~/.pluk` | same |
| `app_config_dir()` | `~/Library/Application Support/com.pluk.app` | `$XDG_CONFIG_HOME/pluk` or `~/.config/pluk` |
| `log_file()` | `<data_dir>/pluk.log` | same |
| `export_dir()` | `<data_dir>/exports` | same |
| `ssh_control_dir()` | `<data_dir>/ssh-control` | same |
| `mcp_config_path(client, scope)` | see table below | identical global paths |
| `mcp_detection_paths(client)` | config/state dirs **plus `/Applications/*.app`** for Cursor, Windsurf, Antigravity | config/state dirs only |

SSH control sockets are copied into `sockaddr_un.sun_path`: 104 bytes on macOS, 108 on Linux. Every socket path under `ssh_control_dir()` must stay **under 104 bytes total** so both targets work.

Per-client MCP config locations (mirrors `MCPClient` in `swift/Sources/ConnectionDetailView.swift`):

| Client | Global path | Project path | Project scope? | Format |
| --- | --- | --- | --- | --- |
| opencode | `~/.config/opencode/opencode.json` | `opencode.json` | yes | JSON(C) |
| Codex | `~/.codex/config.toml` | — | no | TOML |
| Claude Code | `~/.mcp.json` | `.mcp.json` | yes | JSON |
| Cursor | `~/.cursor/mcp.json` | `.cursor/mcp.json` | yes | JSON |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` | — | no | JSON |
| Antigravity | `~/.gemini/config/mcp_config.json` | — | no | JSON |

Global-only clients fall back to their global path when asked for a project scope (matches the injector's behaviour).

## Build and development

```sh
# Frontend (must build before pluk-host: tauri embeds ui/dist at compile time)
bun install --cwd ui
bun run --cwd ui dev      # vite dev server on http://localhost:1420
bun run --cwd ui build    # emits ui/dist

# Workspace
cargo build --workspace
cargo clippy --workspace

# App (debug builds load devUrl http://localhost:1420)
cargo run -p pluk-host
```

Tauri 2.11 / tauri-build 2.6. Frontend: Vite 7, vanilla TypeScript, Bun as package manager. No framework chosen yet by design — tasks R18–R22 build the real UI on this stub and pick the framework then.

## Chain position

You are reading task R01's deliverable: workspace skeleton, empty crates that compile, Tauri shell that launches on macOS with tray presence, frontend stub, and this document. Everything behavioural arrives in R02+.
