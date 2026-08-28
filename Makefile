APP       := Pluk
BUNDLE_ID := com.desgnspace.pluk
DIST      := dist
VERSION   := $(shell cat VERSION 2>/dev/null | tr -d ' \n')
COMMIT    := $(shell git rev-parse --short HEAD 2>/dev/null || echo unknown)

.PHONY: dev deps build build-ui bundle bundle-unsigned bundle-signed install test lint clean sync-version check-tauri swift-build swift-bundle help

# ── Help ──────────────────────────────────────────────────────────────────────
help:
	@printf "Rust (Tauri) targets — primary:\n"
	@printf "  make dev              Run the Rust app in dev (webview -> http://localhost:1420)\n"
	@printf "  make build            Frontend + cargo build (debug)\n"
	@printf "  make bundle           Frontend + cargo tauri build (release bundles, unsigned if no identity)\n"
	@printf "  make bundle-signed    Signed + notarized via 1Password (op run --env-file=.env.1password)\n"
	@printf "  make bundle-unsigned  Force ad-hoc signing (no identity)\n"
	@printf "  make install          Build bundles and install Pluk.app to /Applications (macOS)\n"
	@printf "  make test             cargo test --workspace\n"
	@printf "  make lint             cargo clippy + frontend typecheck\n"
	@printf "  make clean            Remove dist/ and build artefacts\n"
	@printf "Legacy Swift (fallback, not required for Rust):\n"
	@printf "  make swift-build      Legacy Swift build (swift/ — do not run in agent)\n"
	@printf "  make swift-bundle     Legacy Swift bundle (server + Swift app)\n"

# ── Dev (Rust) ────────────────────────────────────────────────────────────────
dev:
	@printf "→ dev: vite on http://localhost:1420, then the Tauri host\n"
	bun install --cwd ui --silent
	@bash -c 'bun run --silent --cwd ui dev & UI=$$!; trap "kill $$UI 2>/dev/null" EXIT; until curl -sf http://localhost:1420 >/dev/null; do sleep 0.3; done; cargo run -p pluk-host'

deps:
	@printf "→ installing ui deps and Rust deps\n"
	bun install --cwd ui
	cargo fetch

build-ui:
	@printf "→ building frontend (ui/dist)\n"
	bun install --cwd ui --silent
	bun run --cwd ui build

sync-version:
	@printf "→ syncing version $(VERSION) into Cargo.toml and tauri.conf.json\n"
	@if [ -z "$(VERSION)" ]; then echo "VERSION file missing"; exit 1; fi
	@# Update workspace version in Cargo.toml (workspace.package.version)
	@# Use sed -i '' on macOS, fallback to sed -i on Linux
	@if sed --version >/dev/null 2>&1; then \
		sed -i "s/^version = \".*\"/version = \"$(VERSION)\"/" Cargo.toml; \
	else \
		sed -i '' "s/^version = \".*\"/version = \"$(VERSION)\"/" Cargo.toml; \
	fi
	@# Update tauri.conf.json version via python (jq not guaranteed)
	@python3 -c "import json, pathlib; p=pathlib.Path('crates/pluk-host/tauri.conf.json'); d=json.loads(p.read_text()); d['version']='$(VERSION)'; p.write_text(json.dumps(d, indent=2)+'\n')"
	@printf "  Cargo.toml + tauri.conf.json now at $(VERSION)\n"

build: build-ui
	@printf "→ cargo build --workspace\n"
	cargo build --workspace

# ── Prereq ────────────────────────────────────────────────────────────────────
check-tauri:
	@if ! cargo tauri --version >/dev/null 2>&1; then \
		echo "error: cargo-tauri not found."; \
		echo "  install it with: cargo install tauri-cli --version \"^2\" --locked"; \
		echo "  then verify: cargo tauri --version"; \
		exit 1; \
	fi

# ── Bundle (Tauri) ───────────────────────────────────────────────────────────
# Bundling artefacts:
#   macOS: Pluk.app + Pluk_*.dmg; Linux: .deb + .AppImage (see tauri.conf.json targets).
#   Signing (macOS): bundle.macOS.signingIdentity in tauri.conf.json is null so
#   APPLE_SIGNING_IDENTITY from the environment is used. Hardened runtime and
#   entitlements are declared in tauri.conf.json (entitlements.plist). Notarization
#   via notarytool is triggered automatically when APPLE_ID + APPLE_PASSWORD +
#   APPLE_TEAM_ID are set (or API key vars). See .env.1password and bundle-signed.
#   Unsigned local builds (no Apple ID needed) remain the default — just run make bundle.
bundle: check-tauri sync-version build-ui
	@printf "→ bundling $(APP) v$(VERSION) ($(COMMIT)) via Tauri\n"
	@printf "  macOS artefacts: target/release/bundle/macos/*.app + target/release/bundle/dmg/*.dmg\n"
	@printf "  signing: APPLE_SIGNING_IDENTITY=\"\$$APPLE_SIGNING_IDENTITY\" (macOS), TAURI_SIGNING_PRIVATE_KEY for updater\n"
	@printf "  unsigned? omit APPLE_SIGNING_IDENTITY (ad-hoc). signed? make bundle-signed\n"
	cargo tauri build

bundle-unsigned: check-tauri sync-version build-ui
	@printf "→ bundling without signing (ad-hoc / unsigned)\n"
	APPLE_SIGNING_IDENTITY="-" cargo tauri build

bundle-signed: check-tauri sync-version build-ui
	@printf "→ bundling signed + notarized via 1Password\n"
	@if ! command -v op >/dev/null 2>&1; then \
		echo "error: 1Password CLI (op) not found."; \
		echo "  install: https://developer.1password.com/docs/cli/get-started/"; \
		echo "  then run: op signin && make bundle-signed"; \
		exit 1; \
	fi
	@if [ ! -f .env.1password ]; then \
		echo "error: .env.1password not found (template .env.1password should be committed)."; \
		exit 1; \
	fi
	op run --env-file=.env.1password -- cargo tauri build

# ── Install (macOS) ─────────────────────────────────────────────────────────
install: bundle
	@printf "→ installing $(APP).app to /Applications\n"
	@osascript -e 'tell application "$(APP)" to quit' >/dev/null 2>&1 || true
	@rm -rf "/Applications/$(APP).app"
	@cp -R "crates/pluk-host/target/release/bundle/macos/$(APP).app" "/Applications/$(APP).app" 2>/dev/null || \
		cp -R "target/release/bundle/macos/$(APP).app" "/Applications/$(APP).app" 2>/dev/null || \
		( echo "no bundle found — check dist/bundle or target/release/bundle"; exit 1 )
	@printf "→ installed /Applications/$(APP).app — launching\n"
	@open "/Applications/$(APP).app" 2>/dev/null || true

# ── Test / Lint ─────────────────────────────────────────────────────────────
test:
	cargo test --workspace

lint:
	cargo clippy --workspace -- -D warnings
	bun run --silent --cwd ui build 2>&1 | head -20

# ── Legacy Swift (fallback) ────────────────────────────────────────────────
# The Swift app stays buildable as the fallback until the Rust app reaches
# parity. Do not run these in an agent sandbox — XCBBuildService gets EPERM.
# A human with Xcode runs them locally.

swift-build:
	@printf "→ [legacy] building Swift app\n"
	@printf "  (requires Xcode, not run in agent — see docs)\n"
	cd swift && swift build -c release

swift-bundle: swift-build
	@printf "→ [legacy] assembling Pluk.app from Swift + bun server (old flow)\n"
	@printf "  See git history before R23 for the full legacy bundle recipe.\n"
	@false

# ── Clean ───────────────────────────────────────────────────────────────────
clean:
	rm -rf $(DIST)
	rm -rf target
	rm -rf ui/dist
	rm -rf ui/node_modules/.vite
	cd swift && swift package clean 2>/dev/null || true
