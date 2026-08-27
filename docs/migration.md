# Migration — existing Pluk users

Both the Swift app (`swift/`) and the Rust app (`crates/pluk-host`) open
the **same** SQLite file at the same path, with no migration step.

## Where the data lives

`pluk-core::platform::data_dir()` is the source of truth, in both apps:

- Respects `PLUK_DATA_DIR` if set, otherwise `~/.pluk`.
- The Swift app and the Rust store both open `~/.pluk/pluk.db` (or
  `$PLUK_DATA_DIR/pluk.db`).

Inside that directory:

- `pluk.db` — integrations, groups, query log, saved queries.
- `pluk.db-wal` / `pluk.db-shm` — WAL sidecars (created once the Rust store
  has opened the file; recovered automatically after a crash).
- `pluk.log` / `exports/` / `ssh-control/` — logs, CSV exports, SSH control sockets.

## What opening with the new app does

No import. The Rust store opens `pluk.db` unchanged.

On first open it sets:

- `journal_mode=WAL`, `synchronous=NORMAL`, `busy_timeout=5000ms`.

This is a header flag in the database file itself: once the Rust app has
opened it once, any future `sqlite3_open` from the Swift app picks WAL up
unchanged — no Swift-side change needed. Both apps bundle modern SQLite, so
WAL is always available.

Schema moves only through `pluk-store`'s `user_version` migration ladder.
New databases get version 1 from the Rust side; the TypeScript
`try/catch ALTER` loop in the Swift app stays authoritative for its own
process but will not conflict — columns are added idempotently.

## Running both apps

You can, but they share one writable file and one log. Practical guidance:

- **Same version, same file:** both apps read and write `~/.pluk/pluk.db`.
  WAL lets readers and the writer run concurrently (the Swift app reads while
  the Rust server writes log rows). Rare contention (checkpoint starvation,
  concurrent migrations) waits up to 5 s and then reports `SQLITE_BUSY` rather
  than losing data. Do not force-close one app mid-migration.

- **`pluk.db-wal` / `-shm`:** live next to `pluk.db` while either app is
  running. Do not delete them while an app is running — they hold uncheckpointed
  pages. After a crash they are recovered automatically on next open.

- **Tokens and endpoints:** group/integration tokens and the MCP endpoint
  `http://localhost:4242/mcp/<token>` are the same in both apps. An agent
  config pointing at `localhost:4242` keeps working — the Rust host binds the
  same port at startup whether or not a window is open (Swift used a spawned
  child server on the same port).

- **Which app wins the port:** only one can bind `4242`. If both apps are
  launched, the second fails to bind and logs `cannot bind 4242`. Quit one
  before opening the other, or set `PORT` to run the Rust host on another port
  (agents must then update their config — loopback-only is a product promise,
  so the host never binds beyond `127.0.0.1`).

- **Downgrade:** if you go back to the Swift app only, WAL remains enabled
  (the file stays in WAL mode). That is supported — Swift's `sqlite3_open`
  reads the header and continues in WAL. No downgrade step is needed. If you
  ever need rollback journal, run `PRAGMA journal_mode=DELETE;` with no app
  running.

## What is not migrated

- Adapter credentials and MCP config injections (`~/.config/opencode/*`, etc.)
  are untouched — `pluk-core::mcp_config` mirrors the Swift `MCPClient` paths.
- `herd` integrations stay in the database but are ignored by the Rust app
  (the adapter was dropped).

## Version and commit in the new app

Today's Swift bundle baked `VERSION` + `git rev-parse HEAD` into
`Info.plist`. The Rust bundle does the same at compile time via
`crates/pluk-host/build.rs` (`PLUK_VERSION` from `VERSION`, `PLUK_COMMIT`
from `git rev-parse HEAD`), exposed in-app via `get_version` for bug
reports and compared by the updater against `latest.json`'s `version`.
