# Updater — packaging contract for R23

This is the spec R23 must fulfil. R17 leaves a disabled updater (empty pubkey + placeholder endpoint) that degrades quietly; R23 enables it at packaging time.

## What R17 ships

- `crates/pluk-host/src/updater.rs` — state machine (`UpdateState`), `Updater` handle, Tauri commands `get_update_state` / `check_for_updates` / `install_update`, events `pluk://update-state`.
- `tauri.conf.json > plugins.updater` — currently:
  ```json
  { "pubkey": "", "endpoints": ["https://example.com/updates/latest.json"] }
  ```
  Any endpoint containing `example.com` or an empty pubkey is treated as **unconfigured** (`UpdaterConfig::is_placeholder`). The app stays in `Disabled`, no banner, no toast, no crash — same as Swift's `isConfigured == false` in `swift run`.
- Menu item `Check for Updates…` in app menu + tray menu, wired to `Updater::begin_check()` and `pluk://update-state` emission.
- Periodic tick every 6 h (`CHECK_INTERVAL`) that marks `Checking` and notifies; the real plugin verification is `tauri-plugin-updater` (Minisign).

## What R23 must produce

### 1. Key pair

- Generate with `tauri signer generate` (or `cargo tauri signer generate`). It emits a Minisign pair:
  - `~/.tauri/<app>.key` — **private key**, base64, never committed. Provide to CI as env var `TAURI_SIGNING_PRIVATE_KEY` (or `TAURI_PRIVATE_KEY` per Tauri docs) at `cargo tauri build` time. The signer uses it to sign each artifact.
  - Public key string — paste into `tauri.conf.json > plugins.updater.pubkey`. This is bundled in the binary and used at runtime to verify the manifest + artifact signatures.
- Do not generate or commit a key in this repo. Rotate by generating a new pair, updating `pubkey` in config, and re-signing artifacts.

### 2. Update manifest (`latest.json`)

Tauri's updater expects a JSON file at each endpoint URL. Shape (per `tauri-plugin-updater` docs):

```json
{
  "version": "0.2.0",
  "notes": "Fixes…",
  "pub_date": "2026-08-27T00:00:00Z",
  "platforms": {
    "darwin-x86_64":  { "signature": "<minisign sig>", "url": "https://…/Pluk_0.2.0_x64.dmg" },
    "darwin-aarch64": { "signature": "<minisign sig>", "url": "https://…/Pluk_0.2.0_aarch64.dmg" },
    "linux-x86_64":   { "signature": "<minisign sig>", "url": "https://…/pluk_0.2.0_amd64.AppImage.tar.gz" }
  }
}
```

- `version` is semver, compared to `tauri.conf.json > version` / `CARGO_PKG_VERSION`.
- `url` is the artifact to download; `signature` is the `.sig` file content produced alongside each artifact by the signer.
- Keep `latest.json` and every artifact under the same origin so CSP/CORS is trivial. Serve with `Content-Type: application/json`.

### 3. Artifacts per platform

- **macOS**: Tauri `bundle.targets` currently `["app","dmg"]`. Updater consumes the signed `.app.tar.gz` (or `.dmg` via `createUpdaterArtifacts` flow). Both x64 and aarch64 need separate entries if not universal. Requires Apple code-signing + notarization separately — R23 decides.
- **Linux**: Updater consumes `AppImage` + `.AppImage.tar.gz` (or `.deb` — R23 decides). Until R23 picks `bundle.targets` for Linux (`appimage`/`deb`/`rpm`), the Linux entry cannot be produced. Flag: **blocked on R23 packaging-format decision for Linux**. The Rust state machine already handles it; only artifact generation is blocked.

### 4. Hosting

- Do not invent a URL now. Replace `https://example.com/updates/latest.json` in `tauri.conf.json` with the real endpoint(s), e.g.:
  - `https://github.com/yondifon/pluk/releases/latest/download/latest.json`
  - `https://releases.pluk.example/latest.json`
- Endpoints is an array — list a primary + fallback. Must be HTTPS.
- Serve `latest.json` and artifacts from immutable, versioned URLs (GitHub Releases does this). Use `TAURI_UPDATER_ENDPOINT` override at build time if needed.

### 5. Failing loudly

- If `pubkey` is empty or any endpoint still contains `example.com`, the updater stays `Disabled` and the app never checks — visible in `get_update_state()` as `{ kind: "disabled" }` and in logs. CI should assert `jq -e '.plugins.updater.pubkey != ""' tauri.conf.json` and `! grep example.com`.
- At packaging time, fail the build if `TAURI_SIGNING_PRIVATE_KEY` is unset when building a release profile.

## Frontend contract (for R18–R22)

```ts
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
type UpdateState =
  | { kind: "disabled", reason: string }
  | { kind: "idle" } | { kind: "checking" } | { kind: "upToDate" }
  | { kind: "available", version: string, notes?: string }
  | { kind: "downloading", progress: number }
  | { kind: "ready", version: string }
  | { kind: "failed", kind: "download"|"signature"|"unreachable"|"other", message: string };

const state: UpdateState = await invoke("get_update_state");
await listen("pluk://update-state", e => renderBanner(e.payload as UpdateState));
await invoke("check_for_updates"); // menu / banner CTA
await invoke("install_update");    // downloads, verifies, then Tauri restarts
```

- Banner: `state.kind === "available" || state.kind === "ready"` (see `should_show_banner`).
- Toast: only `failed` with `kind !== "unreachable"` (see `should_show_toast`). Unreachable/unconfigured never toasts.
- `Disabled`/`Idle`/`UpToDate`/`Checking` never show banner.

## Verification

- `cargo test -p pluk-host -- updater` covers the state machine (no network): no-update, available, download failure, signature failure, unconfigured quiet degrade, unreachable quiet degrade.
- Manual: set real `pubkey` + `endpoints` to a test release, `cargo run -p pluk-host`, trigger `Check for Updates…`, observe `Available` → `Downloading` → `Ready` → restart.
