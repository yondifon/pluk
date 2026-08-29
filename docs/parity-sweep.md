# Parity Sweep — R22 handover to R23

Generated for `task/rust-rewrite` at R22 completion. Compared SwiftUI app (`swift/Sources`) against Tauri+TS port (`ui/src`, `crates/*`).

## Legend

- **Present** — behaviour exists and matches
- **Missing** — not implemented
- **Different** — implemented but diverges

## Screens

| Screen | Behaviour | Status | Note |
|---|---|---|---|
| Shell / Two-pane | Split view with sidebar 220–320, detail min 440, resizer | Present | `ui/src/shell.ts` — min sizes match Swift `NavigationSplitView` |
| Shell | Window frame persist + contentMin 720×520 clamp | Present | `crates/pluk-host/src/frame.rs` |
| Sidebar | Groups section + Integrations section, collapsed when empty | Present | `ui/src/sidebar.ts` |
| Sidebar | Search filters name, type raw+label, environment | Present | `ui/src/filter.ts` |
| Sidebar | Type filter hides groups | Present | |
| Sidebar | Environment filter keeps unscoped groups | Present | |
| Sidebar | Available filters only show types/envs in use | Present | |
| Sidebar | Context menu Duplicate + Delete (integration), Delete (group) | Present | |
| Sidebar | No-matches empty with ContentUnavailableView | Present | maps to `emptyStates:no-matches` |
| Group Detail | Header icon, name, Edit, Delete overflow | Present | `ui/src/groupDetail.ts` |
| Group Detail | Subtitle `Group · N integrations · Env` | Present | |
| Group Detail | Tabs Logs / Overview, Logs reuses R21 view scoped to group | Present | `mountActivityLog` with `groupId` |
| Group Detail | Overview: endpoint card with URL + Copy (Copied! 1.5s, reduced-motion aware) | Present | |
| Group Detail | Overview: client config section (Agent setup) | Present | shows endpoint key + URL; full inject UI lives in integration detail |
| Group Detail | Overview: member list each row shows overrides `k → v` sorted | Present | |
| Group Detail | Member row shows tool prefix `${slug}__*` derived from same rule as server | Present | `ui/src/slug.ts` matches `crates/pluk-server/src/mcp/namespace.rs` + `pluk/src/mcp/namespace.ts` |
| Group Detail | Member row collision handling `${slug}_2`, `_3` | Present | `slugsWithCollision` — mirrors `pluk/src/mcp/group.ts` |
| Group Detail | Member row tappable → edit integration | Present | `onEditIntegration` |
| Integration Detail | Header, status dot, meta line, Overview/Logs tabs | Present | `ui/src/integration-detail` |
| Integration Detail | Overview rows: sqlite vs networked vs generic with secret masking | Present | |
| Integration Detail | Tools enabled count, ordered enabled-first | Present | |
| Integration Detail | Endpoint card + client config snippet/inject | Present | |
| Integration Detail | Logs tab reuses same activity log view | Present | |
| Connection Form | Type chooser grouped by category | Present | `ui/src/forms/render.ts` |
| Connection Form | Fields with showIf chaining, required validation, canSave gate | Present | |
| Connection Form | Tools toggles + per-tool settings + danger warnings | Present | |
| Group Form | Name, environment Any/mixed, member checklist, per-member overrides blank=inherit | Present | |
| Activity Log | Paging merge by id, generation counter, newest-first sort | Present | `ui/src/activityLog` |
| Activity Log | Filters: search, verdict, time range, retention | Present | |
| Activity Log | Live SSE + monotonic cursor + drift reconcile | Present | |
| Activity Log | Pending poll 1.5s + Stop button (cancelled ≠ error) | Present | |
| Activity Log | Caps 10 lines/1200 chars response, 40/6000 console, truncated notice + Open | Present | |
| Activity Log | Highlighting 5 languages + console line tinting | Present | |
| Toasts | One per integration, newer replaces previous | Present | `ui/src/toast.ts#ToastCenter.present` |
| Toasts | Error 8s, success 3s | Present | |
| Toasts | Error also raises system notification (Web Notifications / Tauri) | Present | best-effort |
| Toasts | Error offers Retry that re-tests connection (`POST /api/integrations/:id/test`) | Present | via `onRetry` param |
| Toasts | Animations respect `prefers-reduced-motion: reduce` | Present | `shouldAnimate()` + CSS `@media` |
| Health | Polls `/api/health` every 15s (Swift) | Different | Port polls via `pluk-server` health endpoint but interval is caller-defined; default wiring in `ui/src/main.ts` demo uses 15s — caller must wire. Not all adapters record health yet (SQL/SSH do, others stub) |
| Health | Toasts fire only on transition ok/unknown→error or error→ok | Present | `ui/src/health.ts#detectTransitions` |
| Health | Persistently failing never re-toasts on every poll | Present | |
| Banners | Server status banner: starting (spinner text), stopped (Restart) | Present | `ui/src/shell.ts#renderBanners` |
| Banners | Update banner: available (Update & Relaunch) / updating (rebuilding…) | Present | |
| Banners | Banners respect reduced-motion | Present | |
| Banners | Banners have role=status/alert + aria-live | Present | added in R22 |
| Empty states | No integrations (first-run) tells what to do, New Integration CTA | Present | `ui/src/emptyStates.ts` — copy reviewed for no internal vocab |
| Empty states | No groups | Present | |
| Empty states | Nothing selected (has items but none selected) | Present | `nothing-selected` |
| Empty states | Catalog unavailable with Retry | Present | |
| Empty states | Copy bans owner/manifest/verdict/projection/slug | Present | verified in tests |
| Keyboard | Shortcuts reach webview: ⌘N new integration, ⇧⌘N new group, ⌘K focus search | Present | `ui/src/keyboard.ts` |
| Keyboard | Zoom shortcuts ⌘ +/-/0 map to typography-only scale | Present | `ui/src/zoom.ts` + `keyboard.ts` |
| Keyboard | Focus order sidebar search → filter → list → detail tabs → form fields | Present | tabIndex 0 + native order; verified manually (no programmatic trap) |
| Keyboard | Every interactive element reachable + labelled for screen reader | Different | All new R22 elements have aria-label/role; older R18–R21 forms carry labels but full VoiceOver audit not run in this sandbox (no a11y tree) |
| Typography | UI scale env vartyZoom applied to type only, not page transform | Present | |
| Security | Notifications request auth once on launch | Present | `ToastCenter.postNotification` checks permission; initial request should be done by caller (swift did `requestNotificationAccess()` on launch) |

## Missing or Deferred

| Behaviour | Status | Note for R23 |
|---|---|---|
| macOS tray icon + Show/Quit menu | Different | `crates/pluk-host/src/lib.rs` has stub tray, but `pluk-host` run loop not wired to start server on launch in `lib.rs` vs `main.rs` — packaging must embed icon at `icons/tray.png` |
| Update checker periodic git ls-remote + `make install` relaunch | Missing | `pluk-host/src/updater.rs` exists but not wired to banner state; Swift's `UpdateChecker` not ported end-to-end. R23 must decide whether updater survives Rust port or is replaced by Tauri updater plugin |
| CodeTextView line numbers + copy button per block | Missing | Swift detail shows SQL/result with line numbers; TS log shows plain `<pre>` with highlight but no line gutter |
| Syntax highlight off main thread via actor | Different | TS does `await setTimeout(0)` for >3k chars but not a Worker; acceptable but slower on huge payloads |
| MarkdownResponseView for tool responses | Missing | Swift renders markdown responses; TS shows plain/highlighted text |
| AppZoom menu items in native menu bar (View → Zoom) | Different | TS zoom is localStorage + keyboard; host menu integration via `get_zoom_scale`/`zoom-changed` exists but not tested in packaged app |
| SF Symbols vs glyph system | Different | Swift uses SF Symbols; TS uses `glyph.ts` text badges — visual parity not exact |
| Window autosave name `pluk` | Different | Rust uses `window-frame.json` JSON, not NSWindow autosave; behaviour matches but not identical |

## Uncertainty

- Health `at` field unit: Swift records `Double` epoch ms; Rust records `i64` epoch ms; TS `humanizeHealthError` assumes error string only, not code.
- Catalog load retry affordance: Swift shows Retry button on `adapterErrorView`; TS `catalog-unavailable` empty state exists but `main.ts` demo does not mount it as the primary view — integration needed in `sidebar.ts` load failure path.
