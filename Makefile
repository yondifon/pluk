APP       := Pluk
BUNDLE_ID := com.pluk.app
DIST      := dist

.PHONY: dev server swift-build bundle zip release publish publish-minor publish-major clean

# ── Dev ───────────────────────────────────────────────────────────────────────

dev:
	cd swift && swift run

# ── Build ─────────────────────────────────────────────────────────────────────

server:
	@printf "→ compiling server binary\n"
	@mkdir -p $(DIST)
	@# cpu-features ships a native .node addon bun can't bundle; stub it for compile only.
	@# ssh2 wraps the require in try/catch and guards all property accesses — pure-JS fine.
	@cp pluk/node_modules/cpu-features/lib/index.js /tmp/_cpu-features-orig.js
	@printf "'use strict';\nmodule.exports = () => ({});\n" \
		> pluk/node_modules/cpu-features/lib/index.js
	cd pluk && bun build --compile src/server.ts --outfile ../$(DIST)/pluk-server; \
		cp /tmp/_cpu-features-orig.js pluk/node_modules/cpu-features/lib/index.js
	@chmod +x $(DIST)/pluk-server

swift-build:
	@printf "→ building Swift app\n"
	cd swift && swift build -c release

bundle: server swift-build
	@v=$$(cat VERSION); \
	printf "→ assembling Pluk.app v$$v\n"; \
	rm -rf $(DIST)/$(APP).app; \
	mkdir -p $(DIST)/$(APP).app/Contents/MacOS; \
	mkdir -p $(DIST)/$(APP).app/Contents/Resources; \
	cp swift/.build/release/$(APP) $(DIST)/$(APP).app/Contents/MacOS/; \
	cp $(DIST)/pluk-server $(DIST)/$(APP).app/Contents/Resources/pluk-server; \
	chmod +x $(DIST)/$(APP).app/Contents/Resources/pluk-server; \
	sed "s/{{VERSION}}/$$v/g" swift/Info.plist.template \
		> $(DIST)/$(APP).app/Contents/Info.plist
ifdef APPLE_IDENTITY
	@printf "→ signing with $(APPLE_IDENTITY)\n"
	codesign --deep --force --verify --sign "$(APPLE_IDENTITY)" $(DIST)/$(APP).app
endif

zip: bundle
	@v=$$(cat VERSION); \
	cd $(DIST) && zip -qr $(APP)-$$v.zip $(APP).app; \
	printf "→ $(DIST)/$(APP)-$$v.zip ready\n"

release: zip
	@v=$$(cat VERSION); \
	printf "→ releasing v$$v\n"; \
	git add VERSION; \
	git commit -m "chore: release v$$v"; \
	git tag -a "v$$v" -m "v$$v"; \
	git push origin HEAD "v$$v"; \
	gh release create "v$$v" "$(DIST)/$(APP)-$$v.zip" \
		--title "Pluk v$$v" \
		--generate-notes

# ── Publish (bump + release) ──────────────────────────────────────────────────

publish:
	@old=$$(cat VERSION); \
	IFS=. read -r maj min pat <<< "$$old"; \
	echo "$$maj.$$min.$$((pat+1))" > VERSION; \
	printf "→ $$old → $$(cat VERSION)\n"
	@$(MAKE) --no-print-directory release

publish-minor:
	@old=$$(cat VERSION); \
	IFS=. read -r maj min pat <<< "$$old"; \
	echo "$$maj.$$((min+1)).0" > VERSION; \
	printf "→ $$old → $$(cat VERSION)\n"
	@$(MAKE) --no-print-directory release

publish-major:
	@old=$$(cat VERSION); \
	IFS=. read -r maj min pat <<< "$$old"; \
	echo "$$((maj+1)).0.0" > VERSION; \
	printf "→ $$old → $$(cat VERSION)\n"
	@$(MAKE) --no-print-directory release

# ── Clean ─────────────────────────────────────────────────────────────────────

clean:
	rm -rf $(DIST)
	cd swift && swift package clean
