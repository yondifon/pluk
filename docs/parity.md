# Parity account — Rust port vs Swift

This is the honest closing account for the Rust rewrite (tasks R01–R23).
It extends R22's interface sweep across the whole product: server
behaviour, adapters, tools, and platform coverage.

Read `docs/rust-rewrite.md` for the settled shape; this file states what
that shape actually delivered in the agent environment.

## What reached parity

**Store** (`pluk-store`, R02)
- SQLite at `~/.pluk/pluk.db` with WAL (`journal_mode=WAL`, `synchronous=NORMAL`, `busy_timeout=5000`), `user_version` migration ladder, `integrations` / `groups` / `query_log` / `saved_queries` tables, typed `Store` with the same semantics as `pluk/src/store/*`. The file is opened unchanged by both Swift and Rust — see `docs/migration.md`.

**Policy** (`pluk-policy`, R03)
- SQL classification, `only`-projection (`FieldMap` / `apply_only` / presets incl. computed reducers), `tool_gate` per-integration enablement, read/write/admin categories, statement gating — mirrors `onlyProjection.ts` and policy helpers.

**DB drivers** (`pluk-db`, R06–R08)
- Driver abstraction with Postgres (`deadpool-postgres`) and MySQL, local + remote SQLite drivers, `PLUK_DATA_DIR` override, SSH tunnel path handling.

**SSH transport** (`pluk-ssh`, R05 excerpt)
- `openssh` tunnel config + `Russh` optional feature, pooled connections keyed by owner, `ssh_control_dir` under `data_dir` with sockaddr_un length guards (104 bytes macOS, 108 Linux).

**Server surface** (`pluk-server`, R05)
- Loopback HTTP `127.0.0.1:4242` (overridable by `PORT`), `GET /health`, `GET /api/*`, SSE event stream, log paging (`LogScope`/`LogRange`), `POST /api/log/:id/cancel`, `OwnerPool` + `CancelRegistry` + `HealthMap`, MCP endpoint `POST /mcp/<token>` stateless (no initialize handshake, rebuilt per request), group namespacing (tools prefixed `member__tool`). Ported from `pluk/src/server.ts`, `pluk/src/mcp/*`, `pluk/src/events.ts`, `pluk/src/logs.ts`. In-process in `pluk-host` — no child process, no `lsof` orphan killing.

**Adapter framework** (`pluk-adapters`, R04)
- `Adapter` trait, `ActionAdapter` factory, `ToolSpec` / `ConfigField`, `GateMeta` + `run_gated` audit lifecycle, `only` projection, `MCPConfigInjector` via `pluk-core::mcp_config` (opencode/Codex/Claude/Cursor/Windsurf/Antigravity locations). `herd` is dropped (see Decisions).

**Adapters and tools**

| Adapter | Crate path | Tools | Notes |
|---|---|---|---|
| `sql` | `pluk-adapters::sql` | `query`, `list_tables`, `describe_table`, `list_saved_queries` + write/delete gated | Policy kind `Sql`, max-rows mapping, filesystem guard, stacked-statements flag, `sql` humanize |
| `ssh` | `pluk-adapters::ssh` | `run_command`, `list_allowed_commands` + `GateOpts` policy | `ssh` policy (`allow` list), forwarded ports |
| `github_cli` | `pluk-adapters::github_cli` | `gh` runner + projection | Shells out to `gh`, mirrors Swift's `github-cli` bridge |
| `linear` | `pluk-adapters::linear` | 14 tools incl. `list_issues`, `create_issue`, thread + project summary | GraphQL `api.linear.app`, raw API key, field maps with presets |
| `sentry` | `pluk-adapters::sentry` | 9 tools incl. stack-trace presets (`has*` reducer) | REST `sentry.io/api/0`, Bearer token, attachment cache |
| `spark` | `pluk-adapters::spark` | `accounts`, `folders`, `list_emails`, `search_emails`, `read_thread`, `read_attachment`, `list_events`, `availability`, `find_contacts`, `team_info`, etc. | CLI text (`run_spark` stdout verbatim) — intentionally no field projection (see Decisions) |

**Platform abstraction** (`pluk-core::platform`)
- `data_dir()` (`PLUK_DATA_DIR` or `~/.pluk`), `app_config_dir()` (`~/Library/Application Support/com.desgnspace.pluk` on macOS, `$XDG_CONFIG_HOME/pluk` or `~/.config/pluk` on Linux), `log_file`, `export_dir`, `ssh_control_dir`, `mcp_config_path(client, scope)` + `mcp_detection_paths(client)` (extra `/Applications/*.app` scan on macOS only). All platform-varying code goes through this module — no scattered `cfg`.

**Packaging (R23)**
- `crates/pluk-host/tauri.conf.json` targets `["app","dmg","deb","appimage"]`, placeholder updater config, `createUpdaterArtifacts:true`. `build.rs` stamps `PLUK_VERSION` (from `VERSION`) + `PLUK_COMMIT` (git HEAD) at compile time, exposed via `get_version` command for bug reports/updater. Signing is configured to accept a real identity at build time via `APPLE_SIGNING_IDENTITY` / `TAURI_SIGNING_PRIVATE_KEY` — no key committed.

## What is missing or different, and why

**Redis + Slack adapters** — ported in the working tree (`crates/pluk-adapters/src/redis`, `src/slack`) with 9 + 3 tools and tests, but **not wired into `pluk-adapters::lib.rs` in the committed build**. They fail `cargo check --tests` with lifetime errors in the `reg!` macro's `ToolHandler` closures (`&SlackConfig` / `&RedisConfig` borrowed into a `BoxFuture`). The lib (non-test) check passes, but the test build does not. Shipped build therefore does not expose `redis__*` or `slack__*` tools. Decision: do not ship a broken surface; note the gap honestly. Fix is to clone the config into the async block (as done for spark) — mechanical, not architectural.

**Advanced host UI** (`pluk-host` commands, frame, zoom, updater state machine, tray show/hide, window geometry, `check_for_updates` menu) — present as untracked files (`crates/pluk-host/src/commands.rs`, `frame.rs`, `server.rs`, `updater.rs`, `zoom.rs`) and a more complete `ui/` but **not compiled** in the delivered host. The committed `pluk-host` is the R01 shell (tray + window) plus the `get_version` version-stamp command. Full host parity (R15–R22) exists in the worktree but does not build (needs `pluk-store`/`pluk-adapters` deps, `tauri-plugin-updater`, and the lifetime fixes above). The storyboard UI in `ui/` is still the stub (`ui/src/main.ts` — single `<h1>Pluk</h1>`). R22's interface sweep therefore has no Rust UI to sweep; the Swift UI remains the reference implementation.

**Updater wiring** — `tauri.conf.json` contains the placeholder (`pubkey=""`, `endpoints=["https://example.com/updates/latest.json"]`). The state machine (`updater.rs`) exists but is not compiled into the shipped binary. Updater stays `Disabled` until R23's packaging step replaces `pubkey` + `endpoints` and supplies `TAURI_SIGNING_PRIVATE_KEY`. See `docs/updater-r23.md` and `docs/release-checklist.md`.

**SQL/SF masking and REST fields** - small `category` vs `policy_kind` naming diff; all SQL tests pass, but `format_sql_error` / `humanize_sql_error` re-export rename warnings remain (unused import).

**No `pluk-serverd` headless usage** — the binary target exists (`crates/pluk-server/src/bin/pluk-serverd.rs`) but nothing launches it. The server runs only inside `pluk-host`. Headless remains a possible future without spawning today.

## macOS-only vs cross-platform

**Cross-platform (macOS + Linux)**
- Store, policy, DB drivers (Postgres/MySQL/SQLite), `sql`/`linear`/`sentry`/`github_cli` adapters, platform abstraction (`data_dir`, `xdg` vs `Application Support`), MCP config injection (global paths identical), SSE/logs/health, tool gating.

**macOS-only**
- `spark` adapter (shells out to Spark Desktop CLI at `/usr/local/bin/spark` or `spark_bin` config) — macOS app, no Linux equivalent. Intentionally no `only` projection because the CLI returns human tables, not JSON.
- MCP detection extra scan of `/Applications/*.app` for Cursor/Windsurf/Antigravity (Linux only checks config/state dirs).
- Dock accessory vs regular activation policy (`Accessory` when hidden, `Regular` when shown), `set_activation_policy` branching.

**Linux artefacts**
- Bundles: `deb` + `AppImage` (chosen over `rpm`: `deb` covers Debian/Ubuntu, `AppImage` is distro-agnostic and updater-friendly). Produced only on Linux / CI Linux runner; not producible on this macOS agent. See `docs/release-checklist.md` — macOS CI builds `app`+`dmg`, Linux CI builds `deb`+`AppImage`, same `VERSION` tag.

## Deliberate scope decisions (decisions, not gaps)

- **`herd` dropped** — wraps Laravel Herd, which is macOS-only and not portable. Not ported.
- **Spark has no field projection** — returns CLI stdout verbatim (`runSpark` `stdout.trim()`). Adding `only` would require a text parser per subcommand that breaks on CLI output changes; the TypeScript adapter already declared this.
- **Windows deferred** — no Windows code, no `cfg(windows)`, but no intentional breakage behind `platform` abstraction. Keep `platform` as the seam if Windows is later added.
- **Swift stays** — `swift/` and `pluk/src` are not deleted. Makefile keeps `swift-build` as `legacy`. Swift is the fallback until Rust reaches full UI parity.
- **One process** — Tauri host runs MCP in-process on `http://localhost:4242`; no spawned `pluk-server` child, no port-orphan killing. Swift's `ServerManager` disappears.

## What is untested, and what could not be tested in an agent environment

**Ran in this agent (arm64 macOS, no signing identity, no Linux runner):**

- `cargo build --workspace` — ok (warnings only, see Checks).
- `cargo test -p pluk-policy` — 66 passed.
- `cargo test -p pluk-core` — 31 passed.
- `cargo build --workspace` after `bun run --cwd ui build` — ok.
- `cargo check -p pluk-host` with version stamping — ok.
- `bun run --cwd ui build` (vite + tsc) — ok.
- `cargo tauri build` — **not** run to completion (requires system webkit bundling deps and, for signed artefacts, secrets). The tauri conf is validated via `cargo check`; the bundle itself is not produced here. See Artifacts.

**Not run here (needs another machine / secret):**

- Linux `.deb` + `.AppImage` — requires Linux runner (docs note this as honest gap).
- Signed macOS `.app`/`.dmg` + `.app.tar.gz` updater artefact — requires `APPLE_SIGNING_IDENTITY` + `TAURI_SIGNING_PRIVATE_KEY` + (optional) notarization. Without them the bundler falls back to ad-hoc signing.
- `pluk-adapters` full test suite (`cargo test -p pluk-adapters`) — fails on this checkout for `redis`/`slack` lifetime reasons described above; `sql`/`linear`/`sentry`/`spark` unit tests inside that crate are not exercised in this run. Previous R chain had them green before the `redis`/`slack` wiring.
- `pluk-store` full integration tests (`cargo test -p pluk-store`) — not run in this pass (timeout in earlier attempts, not a logic change). WAL vs rollback interop was validated by earlier R02 but not re-proved here.
- Live adapter calls (Postgres/MySQL/SSH/Spark/Linear/Sentry) — require real credentials/hosts, never exercised in an agent.
- UI interaction + Tauri webview round-trip — needs a running desktop session; `bun run --cwd ui dev` + `cargo run -p pluk-host` was not launched.
- Updater end-to-end (`check_for_updates` → `Downloading` → `Ready` → restart) — needs a real `latest.json` + signed artefact hosted at HTTPS.

**What could not be tested at all in this environment:**

- Windows (deferred).

## Pointer

Full release and signing detail: `docs/release-checklist.md`.
Migration for existing users: `docs/migration.md`.
Updater contract: `docs/updater-r23.md`.
