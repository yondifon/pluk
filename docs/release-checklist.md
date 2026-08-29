# Release runbook

How to cut a signed, notarized, auto-updatable release of Pluk. Every secret
traces back to 1Password — nothing is typed into a shell, exported by hand
outside a `.env` file, or committed. `make publish` is the only entry point;
it runs `scripts/publish.sh`, which sources a mounted `.env` if one exists, or
resolves `.env.1password` live via `op run` otherwise (see step 4).

## One-time setup

Do this once per machine (or once per signer, if more than one person cuts
releases).

### 1. Find your signing identity

```sh
security find-identity -v -p codesigning
```

A valid line looks like:

```
1) AB12CD34EF56... "Developer ID Application: Your Name (A1B2C3D4E5)"
```

The quoted string — `Developer ID Application: Your Name (A1B2C3D4E5)` — is
the value that goes into 1Password as `APPLE_SIGNING_IDENTITY` in step 3.
Not the hash before it, not the parenthesized team ID alone: the whole quoted
string, exactly as printed.

If a `Developer ID Application` line is already there, skip to **Branch A**
below — the certificate is already in the keychain, nothing to import. If
`security find-identity` prints nothing (or only other identities), you need
the `.p12` file — see **Branch B**.

### 2. Get the certificate into the keychain

**Branch A — it's already in the keychain.** Nothing to do here. Copy the
quoted identity string from step 1 and use it for `APPLE_SIGNING_IDENTITY`
in step 3.

**Branch B — you have it as a `.p12` file.** Store the file and its export
password in 1Password, then import once:

```sh
# store the file (Document item, same vault as the secrets)
op document create ~/path/to/developer-id.p12 \
  --title="Pluk Developer ID Certificate" \
  --vault="DesgnSpace"

# store its export password on the item you're about to create in step 3 —
# do that step first if you're doing both in one sitting, then come back here

TMPDIR_CERT="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_CERT"' EXIT

op document get "Pluk Developer ID Certificate" --vault="DesgnSpace" \
  --out-file "$TMPDIR_CERT/developer-id.p12"

security import "$TMPDIR_CERT/developer-id.p12" \
  -k ~/Library/Keychains/login.keychain-db \
  -P "$(op read "op://DesgnSpace/Pluk-signing/APPLE_CERT_P12_PASSWORD")" \
  -T /usr/bin/codesign

# confirm it's there, and copy the quoted identity string it prints:
security find-identity -v -p codesigning
```

The temp file lives only in `$TMPDIR_CERT`; the `trap` removes it the moment
the shell exits, however it exits. Nothing lands in the repo or in your
shell history.

### 3. Create the 1Password item that holds the release secrets

Vault: `DesgnSpace`. Item: `Pluk-signing` (Secure Note, one item, seven fields).
Replace every `<...>` placeholder with your real value before running this —
the command itself takes no secret from anywhere but your own input. Use the
quoted identity string from step 1 verbatim for `APPLE_SIGNING_IDENTITY`. If
you're on Branch A (no `.p12`), drop the `APPLE_CERT_P12_PASSWORD` field.

```sh
op item create \
  --category="Secure Note" \
  --title="Pluk-signing" \
  --vault="DesgnSpace" \
  "APPLE_SIGNING_IDENTITY[text]=Developer ID Application: <Your Name> (<TEAMID>)" \
  "APPLE_ID[text]=<your-apple-id-email>" \
  "APPLE_PASSWORD[password]=<app-specific-password>" \
  "APPLE_TEAM_ID[text]=<TEAMID>" \
  "TAURI_SIGNING_PRIVATE_KEY[password]=$(cat ~/.tauri/pluk.key)" \
  "TAURI_SIGNING_PRIVATE_KEY_PASSWORD[password]=<empty if you set none>" \
  "APPLE_CERT_P12_PASSWORD[password]=<the .p12 export password, Branch B only>"
```

Field meanings:

| Field | What it is |
|---|---|
| `APPLE_SIGNING_IDENTITY` | The exact string `codesign` matches against the certificate in your keychain |
| `APPLE_ID` | Apple ID email used for notarization |
| `APPLE_PASSWORD` | App-specific password for that Apple ID (generate at appleid.apple.com) |
| `APPLE_TEAM_ID` | 10-character Apple Developer Team ID |
| `TAURI_SIGNING_PRIVATE_KEY` | Contents of the minisign private key file the updater signs with |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password for that key, if you set one when generating it |
| `APPLE_CERT_P12_PASSWORD` | Branch B only — export password of the `.p12`, used once, to import it |

`.env.1password` (already committed) references every field above except
`APPLE_CERT_P12_PASSWORD` by `op://DesgnSpace/Pluk-signing/<FIELD>` — that's what
`op run` injects into `publish.sh`'s environment. Nothing here is a secret at
rest: it's an `op://` pointer, not a value.

### 4. Choose how secrets reach `publish.sh`: mounted `.env` or live `op run`

`scripts/publish.sh` accepts both, in one order of precedence: **if `.env`
exists at the repo root, it wins — `publish.sh` sources it directly and never
calls `op`.** If `.env` doesn't exist, it falls back to resolving
`.env.1password` live via `op run` on every invocation. Pick one:

**Mounted `.env` (no live 1Password call per release):**

```sh
op inject -i .env.1password -o .env
```

This resolves every `op://` reference once and writes the real values to
`.env` — which is gitignored, never committed. `.env.example` (committed, no
real values) lists every variable the app and the publish pipeline read from
the environment, each with the exact 1Password reference to fill it from, if
you'd rather copy it by hand: `cp .env.example .env`. Re-run the `op inject`
command whenever a secret rotates.

**Live 1Password (no file on disk, `op` required every run):** do nothing —
this is what happens automatically when no `.env` is present, as long as
`.env.1password` is (already committed).

### 5. Commit the updater public key

The public key is not a secret — it belongs in git. Paste it into
`crates/pluk-host/tauri.conf.json > plugins.updater.pubkey` and commit that
file. (If you haven't generated the keypair yet: `cargo tauri signer generate
-- -w ~/.tauri/pluk.key` prints the public key and writes the private key to
`~/.tauri/pluk.key` — that private key's *contents* are what go into the
`TAURI_SIGNING_PRIVATE_KEY` field in step 3, not the file path.)

```sh
jq -e '.plugins.updater.pubkey != ""' crates/pluk-host/tauri.conf.json
```

should print `true` once it's pasted in. `make publish` refuses to run
otherwise.

### 6. Sign in to the tools `make publish` shells out to

```sh
op signin
gh auth login
```

## Per-release steps

From a cold terminal, once one-time setup is done:

```sh
git checkout main
git pull origin main

# 1) bump the version
echo "0.2.0" > VERSION
git add VERSION
git commit -m "chore: release v0.2.0"
git push origin main

# 2) publish
make publish
```

`make publish`:

1. Checks `cargo tauri` and `gh` are installed, then checks whichever secrets
   route applies: if `.env` exists, that's it; otherwise checks `op` is the
   real 1Password CLI, signed in, and every `op://` reference in
   `.env.1password` resolves — before touching a single secret.
2. Runs `scripts/publish.sh`, which sources `.env` directly if present, or
   re-execs itself once under `op run --env-file=.env.1password` if not — so
   every secret above is an environment variable inside that one process and
   nowhere else, regardless of which route supplied it.
3. Inside `publish.sh`: confirms the signing identity is actually present in
   your keychain, and that `plugins.updater.pubkey` isn't empty — both fail
   loud, before any build starts.
4. Builds the universal (arm64 + x86_64) bundle with `cargo tauri build`,
   which signs with `APPLE_SIGNING_IDENTITY`, notarizes with
   `APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID`, and staples automatically.
5. Verifies the result: `codesign --verify`, `spctl -a -vvv -t exec`,
   `xcrun stapler validate`, `spctl` again against the `.dmg`.
6. Builds `latest.json` (the updater manifest) from the signed
   `.app.tar.gz` and its `.sig`.
7. Tags `vX.Y.Z`, pushes the tag, and creates (or updates) the GitHub
   release with the `.dmg`, updater archive, signature, and manifest.

Nothing uploads if signing, notarization, or verification fails.

## What each preflight check says when it fails

| Check | Failure message names |
|---|---|
| No `.env` and no `.env.1password` | how to create either (`op inject` or restore the template) |
| `op` missing or shadowed by an alias | install link, or `unalias op && type op` |
| `op` not signed in | `op signin` |
| An `op://` reference doesn't resolve | the exact vault/item to check |
| `gh` missing | install link |
| Signing identity not in keychain | which identity, and to redo one-time setup step 2 (Branch B) |
| `plugins.updater.pubkey` empty | how to generate/paste the key |

## Build without signing (CI PR checks, local dev)

```sh
make build            # frontend + cargo build --workspace (debug)
make bundle-unsigned  # forces ad-hoc / unsigned — for CI smoke tests
```

## Cut a release — Linux

Tauri Linux bundles **cannot be produced on macOS**. Build on a Linux machine
or CI runner (GitHub Actions `ubuntu-latest`), same version and git tag as
macOS:

```sh
# Ubuntu/Debian build deps
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libssl-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev dpkg

git checkout v0.2.0   # the tag macOS's publish run created
op run --env-file=.env.1password -- make bundle   # or, with a mounted .env: set -a; . ./.env; set +a; make bundle
gh release upload v0.2.0 target/release/bundle/deb/*.deb target/release/bundle/appimage/*.AppImage*
```

Chosen Linux formats (`tauri.conf.json > bundle.targets`): `deb` (native
Debian/Ubuntu package) and `AppImage` (portable, no install — the updater
consumes its tarball). Add `"rpm"` there for Fedora/RHEL.

`latest.json` ends up listing every platform published so far, e.g.:

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
