#!/usr/bin/env bash
# Usage: scripts/publish.sh
# Universal (arm64 + x86_64) release build: sign, notarize, staple, verify,
# build the updater manifest, then push a GitHub release for the version tag.
# Run via `make publish` (see docs/release-checklist.md).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# Secrets: a mounted .env takes precedence — source it directly. Otherwise
# fall back to .env.1password, resolved live via `op run` (re-execs this
# script once, wrapped, so everything below always sees plain env vars).
if [ -f .env ]; then
    echo "=== Secrets: sourcing mounted .env ==="
    while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in ''|\#*) continue;; esac
        case "$line" in *=*) ;; *) continue;; esac
        key=${line%%=*}
        value=${line#*=}
        case "$key" in [A-Za-z_]*) ;; *) continue;; esac
        case "$value" in
            \"*\") value=${value#\"}; value=${value%\"};;
            \'*\') value=${value#\'}; value=${value%\'};;
        esac
        export "$key=$value"
    done < ./.env
elif [ -z "${PLUK_OP_WRAPPED:-}" ]; then
    [ -f .env.1password ] || { echo "error: no secrets source found — create .env (see .env.example) or restore .env.1password"; exit 1; }
    command -v op >/dev/null 2>&1 || { echo "error: 1Password CLI (op) not found."; exit 1; }
    op --version >/dev/null 2>&1 || { echo "error: 'op' does not behave like the 1Password CLI — check for a shell alias shadowing it."; exit 1; }
    echo "=== Secrets: resolving via 1Password (op run) — no .env found ==="
    export PLUK_OP_WRAPPED=1
    exec op run --env-file=.env.1password -- "$0" "$@"
fi

APP_NAME="Pluk"
TARGET="universal-apple-darwin"
VERSION="$(tr -d ' \n' < VERSION)"
TAG="v$VERSION"
GH_REPO="$(git remote get-url origin | sed -E 's#.*github\.com[:/]##; s/\.git$//')"

BUNDLE_DIR="target/$TARGET/release/bundle"
APP_PATH="$BUNDLE_DIR/macos/$APP_NAME.app"

: "${APPLE_SIGNING_IDENTITY:?APPLE_SIGNING_IDENTITY required — Developer ID Application certificate identity}"
: "${APPLE_ID:?APPLE_ID required for notarization}"
: "${APPLE_PASSWORD:?APPLE_PASSWORD required for notarization (app-specific password)}"
: "${APPLE_TEAM_ID:?APPLE_TEAM_ID required for notarization}"
: "${TAURI_SIGNING_PRIVATE_KEY:?TAURI_SIGNING_PRIVATE_KEY required to sign updater artifacts}"
command -v gh >/dev/null || { echo "error: gh CLI required — https://cli.github.com/"; exit 1; }
for t in aarch64-apple-darwin x86_64-apple-darwin; do
    rustup target list --installed | grep -qx "$t" || {
        echo "error: rustup target $t missing — run: rustup target add $t"
        exit 1
    }
done

PUBKEY="$(python3 -c "import json; print(json.load(open('crates/pluk-host/tauri.conf.json'))['plugins']['updater']['pubkey'])")"
[ -n "$PUBKEY" ] || {
    echo "error: plugins.updater.pubkey is empty in crates/pluk-host/tauri.conf.json"
    echo "  generate a keypair: cargo tauri signer generate -- -w ~/.tauri/pluk.key"
    echo "  then paste the printed public key into plugins.updater.pubkey, commit it"
    exit 1
}

security find-identity -v -p codesigning | grep -qF "$APPLE_SIGNING_IDENTITY" || {
    echo "error: signing identity \"$APPLE_SIGNING_IDENTITY\" not found in the login keychain."
    echo "  import the certificate once — see docs/release-checklist.md, one-time setup, \"Import the certificate\""
    exit 1
}

echo "=== Building $APP_NAME $VERSION ($TARGET) ==="
cargo tauri build --target "$TARGET" --bundles app,dmg

[ -d "$APP_PATH" ] || { echo "error: app bundle not found at $APP_PATH"; exit 1; }
DMG_PATH="$(find "$BUNDLE_DIR/dmg" -maxdepth 1 -name '*.dmg' -print -quit 2>/dev/null || true)"
[ -n "$DMG_PATH" ] && [ -f "$DMG_PATH" ] || { echo "error: dmg not found under $BUNDLE_DIR/dmg"; exit 1; }
UPDATER_ARCHIVE="$(find "$BUNDLE_DIR/macos" -maxdepth 1 -name '*.app.tar.gz' -print -quit 2>/dev/null || true)"
[ -n "$UPDATER_ARCHIVE" ] && [ -f "$UPDATER_ARCHIVE" ] || {
    echo "error: updater archive (.app.tar.gz) not found — check createUpdaterArtifacts and TAURI_SIGNING_PRIVATE_KEY"
    exit 1
}
UPDATER_SIG="$UPDATER_ARCHIVE.sig"
[ -f "$UPDATER_SIG" ] || { echo "error: updater signature not found at $UPDATER_SIG"; exit 1; }

echo "=== Notarizing dmg ==="
# Tauri notarizes and staples the .app; the dmg wrapping it is a separate
# artifact and needs its own ticket.
if ! xcrun stapler validate "$DMG_PATH" >/dev/null 2>&1; then
    xcrun notarytool submit "$DMG_PATH" \
        --apple-id "$APPLE_ID" \
        --password "$APPLE_PASSWORD" \
        --team-id "$APPLE_TEAM_ID" \
        --wait
    xcrun stapler staple "$DMG_PATH"
fi

echo "=== Verifying code signature ==="
codesign --verify --deep --strict --verbose=2 "$APP_PATH"
spctl -a -vvv -t exec "$APP_PATH"
codesign --verify --deep --strict --verbose=2 "$DMG_PATH"

echo "=== Verifying notarization ticket ==="
xcrun stapler validate "$APP_PATH"
xcrun stapler validate "$DMG_PATH"
spctl -a -vvv -t open --context context:primary-signature "$DMG_PATH"

echo "=== Building update manifest ==="
LAST_TAG="$(git tag --sort=-v:refname | grep -v "^${TAG}$" | sed -n 1p || true)"
if [ -n "$LAST_TAG" ]; then
    NOTES="$(git log "${LAST_TAG}..HEAD" --pretty=format:'- %s' --no-merges)"
else
    NOTES="$(git log -20 --pretty=format:'- %s' --no-merges)"
fi
ARTIFACT_URL="https://github.com/$GH_REPO/releases/download/$TAG/$(basename "$UPDATER_ARCHIVE")"
SIGNATURE="$(cat "$UPDATER_SIG")"
MANIFEST="$BUNDLE_DIR/latest.json"

VERSION="$VERSION" NOTES="$NOTES" SIGNATURE="$SIGNATURE" ARTIFACT_URL="$ARTIFACT_URL" MANIFEST="$MANIFEST" \
python3 <<'PY'
import datetime
import json
import os

manifest = {
    "version": os.environ["VERSION"],
    "notes": os.environ["NOTES"],
    "pub_date": datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).isoformat(),
    "platforms": {
        "darwin-aarch64": {"signature": os.environ["SIGNATURE"], "url": os.environ["ARTIFACT_URL"]},
        "darwin-x86_64": {"signature": os.environ["SIGNATURE"], "url": os.environ["ARTIFACT_URL"]},
    },
}
with open(os.environ["MANIFEST"], "w") as f:
    json.dump(manifest, f, indent=2)
    f.write("\n")
PY

echo "=== Tagging $TAG ==="
if ! git rev-parse "$TAG" >/dev/null 2>&1; then
    git tag -a "$TAG" -m "$TAG"
fi
git push origin "$TAG"

echo "=== Publishing GitHub release ==="
if gh release view "$TAG" --repo "$GH_REPO" >/dev/null 2>&1; then
    gh release upload "$TAG" "$DMG_PATH" "$UPDATER_ARCHIVE" "$UPDATER_SIG" "$MANIFEST" --repo "$GH_REPO" --clobber
else
    gh release create "$TAG" "$DMG_PATH" "$UPDATER_ARCHIVE" "$UPDATER_SIG" "$MANIFEST" \
        --repo "$GH_REPO" --title "$APP_NAME $VERSION" --generate-notes
fi

echo "✅ Published $APP_NAME $VERSION"
echo "DMG: $DMG_PATH"
echo "Manifest: https://github.com/$GH_REPO/releases/latest/download/latest.json"
