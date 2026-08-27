# Release checklist

How a human cuts a release of the Rust (Tauri) app on each platform. The Swift
app remains as fallback (`swift/`), but releases now ship the Rust bundle.

## Secrets you must have to hand

| Secret | Where it is used | Where to set it | Required for |
|---|---|---|---|
| `APPLE_SIGNING_IDENTITY` | macOS code-signing (`codesign --sign`) | Env var at `make bundle` / `cargo tauri build` time, or `crates/pluk-host/tauri.conf.json > bundle.macOS.signingIdentity` | macOS `.app` + `.dmg` to be trusted outside ad-hoc |
| `APPLE_ID` + `APPLE_PASSWORD` / `APPLE_API_KEY` | Notarization (`xcrun notarytool`) if you notarize the dmg | Env / keychain, per Apple docs | macOS notarized dmg (Gatekeeper) |
| `TAURI_SIGNING_PRIVATE_KEY` | Minisign private key for updater signatures | Env var at build time (`cargo tauri build` signs artifacts) | Updater (`latest.json` signatures) — any platform |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password for the Minisign private key, if set when generating | Env var | Updater signing if key is password-protected |
| GitHub token (`gh auth`) | `gh release create` | `gh auth login` | Publishing the GitHub Release |

No key or certificate is committed. The repo ships with `tauri.conf.json > plugins.updater.pubkey = ""` and `endpoints = ["https://example.com/…"]` (disabled). CI/human replaces both at build time. `TAURI_SIGNING_PRIVATE_KEY` is never written to disk in the repo; pass it as a CI secret.

How to generate the updater key pair (once):

```sh
cargo tauri signer generate -- -w ~/.tauri/pluk.key
# emits ~/.tauri/pluk.key (private, keep secret) and prints the public key
# paste the public key into crates/pluk-host/tauri.conf.json > plugins.updater.pubkey
```

## Version of record

`VERSION` is the single source of truth (`0.1.0` currently). `make bundle` runs `make sync-version` which copies `VERSION` into `Cargo.toml` (workspace) and `crates/pluk-host/tauri.conf.json`. The git commit is stamped at compile time by `crates/pluk-host/build.rs` (`PLUK_COMMIT`). Both are exposed to the app via `get_version` (Tauri command) for bug reports and the updater's version comparison.

Bump the version before any release:

```sh
# patch (default): 0.1.0 -> 0.1.1
make publish        # bumps VERSION, commits, tags, pushes, creates GitHub release
# or manually:
echo "0.1.1" > VERSION
make sync-version
```

`make publish` / `publish-minor` / `publish-major` are thin wrappers around that flow; they call `release` which tags and pushes.

## Cut a release — macOS (local or CI macOS runner)

Prereqs: Xcode CLT, Rust, Bun, `cargo tauri` CLI (`cargo install tauri-cli`).

```sh
git pull origin main
git checkout main
# 1) bump
echo "0.2.0" > VERSION
make sync-version
git diff   # confirm Cargo.toml + tauri.conf.json updated

# 2) configure signing (if you have a Developer ID)
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/pluk.key)"
# optional: export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="..."

# 3) build bundles — produces app + dmg + updater tarballs
make bundle
# artefacts (macOS):
#   target/release/bundle/macos/Pluk.app
#   target/release/bundle/dmg/Pluk_0.2.0_aarch64.dmg
#   target/release/bundle/macos/Pluk_0.2.0_aarch64.app.tar.gz        (updater)
#   target/release/bundle/macos/Pluk_0.2.0_aarch64.app.tar.gz.sig    (signature)

# 4) (optional) notarize the dmg
xcrun notarytool submit target/release/bundle/dmg/*.dmg --wait
xcrun stapler staple target/release/bundle/dmg/*.dmg

# 5) publish
git add VERSION Cargo.toml crates/pluk-host/tauri.conf.json
git commit -m "chore: release v0.2.0"
git tag -a v0.2.0 -m v0.2.0
git push origin main v0.2.0
gh release create v0.2.0 target/release/bundle/dmg/*.dmg target/release/bundle/macos/*.tar.gz* \
  --title "Pluk v0.2.0" --generate-notes

# 6) update the updater manifest (latest.json) hosted at your endpoints URL
# (see docs/updater-r23.md for shape). Tauri's updater compares
# tauri.conf.json version against latest.json > version.
```

If you have no Apple identity, skip the `APPLE_SIGNING_IDENTITY` export — the build
falls back to ad-hoc signing (`-`). For distribution, ad-hoc is not trusted.

## Cut a release — Linux

Tauri Linux bundles **cannot be produced on macOS**. They require a Linux machine
or CI Linux runner (GitHub Actions `ubuntu-latest`).

Prereqs on Linux: `cargo`, `bun`, system webkit + bundling deps:

```sh
# Ubuntu/Debian
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libssl-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev dpkg
```

Steps (same version as macOS, same git tag):

```sh
git pull origin main
git checkout v0.2.0   # or the commit you tagged on macOS

export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/pluk.key)"
make bundle
# artefacts (Linux):
#   target/release/bundle/deb/pluk_0.2.0_amd64.deb
#   target/release/bundle/appimage/pluk_0.2.0_amd64.AppImage
#   target/release/bundle/appimage/pluk_0.2.0_amd64.AppImage.tar.gz
#   *.sig alongside each updater artefact when createUpdaterArtifacts is true

# publish alongside the macOS artefacts:
gh release upload v0.2.0 target/release/bundle/deb/*.deb target/release/bundle/appimage/*.AppImage*
```

Chosen Linux formats (see `tauri.conf.json > bundle.targets`):

- `deb` — native package for Debian/Ubuntu (apt). Covers the largest Linux desktop share.
- `AppImage` — single portable binary, no install, works across distros. The updater consumes the AppImage tarball.

`rpm` is not built by default; add `"rpm"` to `bundle.targets` if Fedora/RHEL support is needed. All three can be built together, but `deb` + `AppImage` is the minimal useful set.

CI should build macOS and Linux in parallel and upload to the same GitHub Release. The updater's `latest.json` should list both platforms:

```json
{
  "version": "0.2.0",
  "platforms": {
    "darwin-aarch64": { "signature": "...", "url": "https://…/Pluk_0.2.0_aarch64.app.tar.gz" },
    "darwin-x86_64":  { "signature": "...", "url": "https://…/Pluk_0.2.0_x64.app.tar.gz" },
    "linux-x86_64":   { "signature": "...", "url": "https://…/pluk_0.2.0_amd64.AppImage.tar.gz" }
  }
}
```

## Build without signing (CI PR checks, local dev)

```sh
make build          # frontend + cargo build --workspace (debug)
make bundle-unsigned  # forces ad-hoc / unsigned — for CI smoke tests
```

CI should assert the updater is configured for releases:

```sh
jq -e '.plugins.updater.pubkey != ""' crates/pluk-host/tauri.conf.json
! grep -q example.com crates/pluk-host/tauri.conf.json
test -n "$TAURI_SIGNING_PRIVATE_KEY"
```

If `pubkey` is empty or `example.com` remains, the app stays in `Disabled` (no banner, no network) — same as Swift's `isConfigured == false`.

## Secrets summary

- At `cargo tauri build` time, the signer reads `TAURI_SIGNING_PRIVATE_KEY` (and password). Paste the matching **public** key into `tauri.conf.json > plugins.updater.pubkey` and push it — that is bundled in the binary.
- At macOS `cargo tauri build` time, the bundler reads `APPLE_SIGNING_IDENTITY` (or `bundle.macOS.signingIdentity`). Without it, the `.app` is ad-hoc signed.
- Replace `https://example.com/updates/latest.json` with your real manifest URL(s) (must be HTTPS, e.g. `https://github.com/yondifon/pluk/releases/latest/download/latest.json`). Updater stays disabled until you do.
