#!/usr/bin/env bash
# Usage: scripts/bump-version.sh [major|minor|fix]
# Bumps VERSION, syncs it into Cargo.toml + tauri.conf.json and commits the
# result, so the tag scripts/publish.sh cuts points at the bumped version.
# Run via `make publish fix` (see docs/release-checklist.md).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    echo "error: uncommitted changes — commit or stash them before publishing."
    git status --short --untracked-files=no
    exit 1
fi

CURRENT="$(tr -d ' \n' < VERSION)"
case "$CURRENT" in
    [0-9]*.[0-9]*.[0-9]*) ;;
    *) echo "error: VERSION holds \"$CURRENT\", expected x.y.z"; exit 1 ;;
esac
MAJOR="${CURRENT%%.*}"
PATCH="${CURRENT##*.}"
MINOR="${CURRENT#*.}"
MINOR="${MINOR%.*}"

BUMP="${1:-}"
if [ -z "$BUMP" ]; then
    echo "Current version: $CURRENT"
    echo ""
    echo "Release type:"
    echo "  1) major  → $((MAJOR + 1)).0.0"
    echo "  2) minor  → ${MAJOR}.$((MINOR + 1)).0"
    echo "  3) fix    → ${MAJOR}.${MINOR}.$((PATCH + 1))"
    printf "Choice [1/2/3]: "
    read -r CHOICE
    case "$CHOICE" in
        1) BUMP=major ;;
        2) BUMP=minor ;;
        3) BUMP=fix   ;;
        *) echo "Invalid choice"; exit 1 ;;
    esac
fi

case "$BUMP" in
    major) NEW_VERSION="$((MAJOR + 1)).0.0" ;;
    minor) NEW_VERSION="${MAJOR}.$((MINOR + 1)).0" ;;
    fix)   NEW_VERSION="${MAJOR}.${MINOR}.$((PATCH + 1))" ;;
    *)     echo "Usage: $0 [major|minor|fix]"; exit 1 ;;
esac

echo ""
printf "Publish Pluk %s (from %s)? Builds, notarizes, pushes tag v%s. [y/N]: " \
    "$NEW_VERSION" "$CURRENT" "$NEW_VERSION"
read -r CONFIRM
[[ "$CONFIRM" =~ ^[Yy]$ ]] || { echo "Aborted."; exit 1; }

echo "$NEW_VERSION" > VERSION
make sync-version
git add VERSION Cargo.toml crates/pluk-host/tauri.conf.json
git commit -m "chore: release v$NEW_VERSION"
echo "✅ VERSION $CURRENT → $NEW_VERSION, committed"
