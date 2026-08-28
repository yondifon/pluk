# Pluk UI audit — 2026-08-28

## TL;DR

- **Verdict**: Works but feels like a web page inside a native shell — unfinished states, inconsistent tokens, weak keyboard/a11y.
- **Scope**: Read-only audit of `ui/src/**` (TypeScript + Vite, no React), `ui/index.html`, checked against macOS desktop bar.
- **Not in scope this pass**: Unified line-icon system and unified modal shell — already planned separately, flagged only.

---

## How this was built

- Mapped entry `ui/src/main.ts` → `shell.ts` + `sidebar.ts` + `integration-detail/*` + `groupDetail.ts` + `forms/render.ts` + `activityLog/*`.
- Read every CSS/TS file named above plus `tokens.css`, `typography.css`, `style.css`.
- No running app — findings are from code, not screenshots. Rendered-behavior claims are marked **unconfirmed** where code alone is ambiguous.
- Severity: **Broken** (cannot complete task) > **Confusing** (can, but wrongly/slowly) > **Inconsistent** (works, looks unfinished) > **Polish**.

## Counts

- **Broken**: 7
- **Confusing**: 14
- **Inconsistent**: 18
- **Polish**: 9
- **Flagged for other rebuilds (icons/modal)**: 4

---

## Root causes (fix once, fix many)

These explain clusters below. Tackle these before page-by-page patches.

1. **No shared component library** — `style.css`, `shell.css`, `sidebar.css` each define ad-hoc `.btn`, `.card`, `.tag`, inline `style.*` in TS. Result: every surface invents its own button, radius, shadow. Fix: one `Button`, `Card`, `Badge`, `Input` with variants.
2. **Tokens exist but are bypassed** — `tokens.css` defines `--space-*`, `--radius-*`, surfaces. Code hardcodes `#f3f4f6`, `#6b7280`, `8px` everywhere (`style.css:35`, `style.css:127`, `sidebar.ts:172`). Fix: lint against raw hex/spacing outside tokens.
3. **No focus/hover/disabled contract** — focus styles appear sporadically (`shell.css:128`, `style.css:261`). Most rows/buttons have no `:focus-visible`, no `:disabled`, cursor changes are mixed. Fix: one focus ring, one disabled opacity, one hover spec.
4. **No empty/loading/error convention** — each screen decides alone (activity log has 4 states, integration form has a hint, detail loading is bare text `main.ts:132`). No shared empty component with icon + title + CTA. Fix: shared `Empty`, `Loading`, `Error` with retry slot.
5. **Inline styles and one-off menus** — `sidebar.ts` and `groupDetail.ts` create context menus/dialogs with `position: fixed` + inline `background:` + manual `window.addEventListener("click", close)`. No shared `Popover`/`Menu`/`Dialog` with focus trap/escape/role. Fix: one `Popover` + one `Dialog` primitive.
6. **Terminology drift** — code says `Integration` (type), `adapter`, `service` (chooser label), `connection` (file name). User sees 3 names for one thing (`forms/render.ts:31` "Choose a service" vs sidebar "Integrations"). Fix: one glossary, sift for leaked code words (`policyKind`, `toolConfig`, `verdict`, `slug`).
7. **Layout is fixed, not resilient** — `.shell min-width:720px / min-height:520px`, sidebar 220–320px, detail `min-width:440px` (`shell.css:6-21`). No breakpoint, no container query for narrow windows; long names ellipsis without tooltip. Fix: fluid shell + truncate-with-tooltip primitive.

---

## Findings by surface

### 1 — Shell + window chrome (`ui/src/shell.ts`, `shell.css`, `main.ts`)

- **Broken** — `shell.css:2` `.shell {height:100vh; min-height:520px; min-width:720px; overflow:hidden}` clips content below 720px with no scroll. Narrow window hides controls. Fix: remove `min-width`, allow `shell` to go to 640px, make `.shell-sidebar` collapse or bottom-sheet at `width<680` or show horizontal scroll. File `ui/src/shell.css:6`.
- **Confusing** — Drag resizer `shell.ts:19` is a 4px invisible div, no keyboard, no `role="separator"`, no `aria-label`, no focus. Hits accessibility and hits 28px target rule. Fix: make it a `role=separator` with 12px hit area (4px visible + 8px invisible), arrow-key resize (+/- 16px), double-click resets to 244px. File `ui/src/shell.ts:19`.
- **Inconsistent** — `shell.css:25` sidebar has `border-right: 1px solid transparent` ("matches SplitViewDividerHider") — invisible divider makes sidebar/content seam disappear on light bg. Fix: use `1px solid rgba(0,0,0,0.06)` light / `rgba(255,255,255,0.08)` dark like banners. File `ui/src/shell.css:25`.
- **Inconsistent** — Banner transitions respect `prefers-reduced-motion` (`shell.ts:63`, `shell.ts:101`) but resizer drag has no reduced-motion check — minor. Fix: document or skip.
- **Polish** — Toast mount `shell.css:73` sits `top:12px; right:16px` over content with `z-index:50`, no offset for notched/window controls. Fix: add `top: var(--traffic-lights-offset, 12px)` or inset from titlebar on macOS. File `ui/src/shell.css:73`.
- **Polish** — `main.ts:525` `renderBanners(bottomMount, {serverStatus:"running"}, ()=>{}, ()=>{})` passes no-ops for restart/update — banners render but do nothing. If banners appear, they are dead. Fix: wire or hide. File `ui/src/main.ts:524`.

### 2 — Sidebar (`ui/src/sidebar.ts`, `sidebar.css`)

- **Broken** — Context menus for rows are non-accessible: `div` with two `button`s, positioned `fixed` at `clientX/Y`, dismissed by a racy `setTimeout(() => addEventListener("click"))` (`sidebar.ts:344`, `sidebar.ts:418`). No `role="menu"`, no `aria`, no focus trap, viewport overflow not clamped, cannot be reached by keyboard at all. **Confirms the brief's dead-end concern for right-click.** Fix: replace with shared `Menu` primitive, or at minimum use native context menu / a button with `aria-haspopup`. Files `ui/src/sidebar.ts:316`, `381`.
- **Broken** — Delete confirmation is a `position:fixed` overlay (`sidebar.ts:203`) created per `createSidebar()` call, `display:grid`/`none` toggled, appended inside the sidebar (`root.append(..., confirmOverlay)`). No `aria-modal`, escape does nothing, clicking backdrop does nothing, focus is moved to Delete but not trapped; second opener overlays first. Flagged for modal-shell rebuild — note only. Files `ui/src/sidebar.ts:203`, `sidebar.ts:212`.
- **Confusing** — Toolbar buttons `sidebar.ts:44` (`+` and `⊞`) use bare text glyphs (`+`, `⊞`) with no button styling, tiny hit target, inconsistent with rest of app's `btn` metaphor. Sighted user cannot tell they are primary creates. Fix: reuse shared `IconButton` (also noted for icon-system rebuild). File `ui/src/sidebar.ts:44`.
- **Confusing** — Search icon `⌕` (`sidebar.ts:62`) and filter `☰` (`sidebar.ts:80`) are unicode characters styled as icons, not glyphs; they inherit different fonts/weights across platforms. **Flagged for line-icon rebuild.**
- **Confusing** — Filter popover `sidebar.ts:94` appends itself to `searchRow` with `position:absolute; top:44px; right:12px`, never repositions on scroll/resize, and toggles on every click of `filterBtn` — second click closes but leaves `popover` variable stale if closed via Clear. No keyboard close (Esc), no focus return. Fix: portal popover to `document.body` with one shared helper, or use `<dialog>`/popover API.
- **Inconsistent** — Inline style colors `sidebar.ts:172` use `var(--surface-sidebar-tertiary)` correctly, but siblings use raw `#ef4444` (`sidebar.ts:237`), `#6b7280` (`style.css:110`). Token bypass cluster.
- **Inconsistent** — Health dot `sidebar.ts:450` renders nothing when `health` is absent — "third state" (`sidebar.ts:455` comment). Row then looks healthy; user cannot distinguish "not checked" from "healthy" without opening detail. Fix: always render empty state dot (grey) with tooltip "Not checked". File `ui/src/sidebar.ts:448`.
- **Inconsistent** — Row names use `white-space:nowrap; ellipsis` without `title` (`sidebar.css:110`), so truncated names are not discoverable on hover. Fix: add `title` or tooltip primitive. File `ui/src/sidebar.css:110`.
- **Accessibility** — Rows are `div[role=listitem]` with `tabIndex=0` and `Enter` handling (`sidebar.ts:312`) but not `Space`, no `role=button` nor `aria-selected`, no roving tabindex / arrow-key navigation between rows. Screen reader hears listitems that are not operable without click. Fix: make rows `button` or `role=option` in a `listbox` with `aria-selected`, handle Space, add Up/Down roving focus. File `ui/src/sidebar.ts:309`.
- **Accessibility** — Toolbar buttons lack visible focus ring — they inherit no `:focus-visible` from tokens. Some have `aria-label` (`sidebar.ts:47`), good, but focus style missing.
- **Layout** — `sidebar-list` hides scrollbar (`sidebar.css:83`) permanently, so keyboard users lose affordance that list scrolls. Fix: keep thin scrollbar or show on hover/focus.
- **Polish** — `sidebar.ts:481` registers a global `window.addEventListener("keydown")` on every `createSidebar()` call. On refresh (`main.ts:408` rebuilds sidebar) listeners stack without removal. **Unconfirmed under real Tauri lifecycle** but visible in code — leak risk. Fix: bind once in `main.ts`, or return a destroy.

### 3 — Detail header (`ui/src/integration-detail/header.ts`, `style.css`)

- **Confusing** — Title `style.css:53` truncates with `white-space:nowrap; ellipsis` inside `titleRow` (`gap:12px; flex-wrap:wrap`). Long names vanish with no expansion. Fix: allow two-line wrap (`-webkit-line-clamp:2`) or show full name on click + tooltip. File `ui/src/style.css:53`.
- **Confusing** — Status chip `style.css:63` uses color-only distinction via `color: #16a34a / #dc2626 / #6b7280` classes; text label exists (`Healthy/Failing`) so not strictly color-only, but the dot (`status-dot`) has no text alt. OK but low contrast against chip bg `#f3f4f6` in dark mode (hardcoded, not tokenized). Dark-mode users see washed dot — color token needed. File `ui/src/style.css:63`.
- **Inconsistent** — Badge `style.css:31` is hardcoded `#e0e7ff / #4338ca` — not a token, never adapts to dark mode. Will glare on dark. Fix: tie to `adapterColor`/`surface-panel` or use a translucent badge scale. File `ui/src/style.css:31`.
- **Inconsistent** — Test button block: two rules render similar intent — `main.ts:392` uses `humanizeHealthError` into a toast, while `header.ts:13` has its own `humanize()` duplicate with slightly different phrasing. Yields two copy voices for same failure. Fix: share one humanizer.
- **Accessibility** — Overflow menu uses `<details><summary>` (`header.ts:102`) with no `role`, no focus outline for `summary`, `MenuList` is a plain `div` with two buttons + an `<hr>` inside. `hr` is not focusable but breaks screen-reader menu semantics. Fix: replaced by icon-system/modal rebuild note — in interim give it `role=menu` + `role=menuitem` + `aria-expanded`. File `ui/src/integration-detail/header.ts:102`.
- **Layout** — `header.ts:132` appends `title, chip, testWrap, menu` in one row. At 440px content width the chip + testWrap + menu wrap unpredictably due to `flex-wrap:wrap` on `titleRow` but `chip` has `height:22px` fixed vs title's variable `19px*zoom`. Verify visually — likely ragged baseline. Fix: align items `center` + give chip `flex-shrink:0`.

### 4 — Tabs (`ui/src/integration-detail/tabs.ts`, `groupDetail.ts` tabs)

- **Inconsistent** — Integration tabs (`tabs.ts:17`) are plain `button.tab` with `aria-selected`; group tabs (`groupDetail.ts:126`) are `.tab-bar .tab` with `class="tab active"` and different padding/border (`shell.css:126` vs `style.css:126`). Two tab components for same pattern. Fix: unify.
- **Confusing** — No keyboard roving: Tab/Shift-Tab leaves the tablist rather than arrowing between `Logs/Overview/Tools`. Group tabs wire `ArrowLeft/Right` but integration tabs do not (`tabs.ts` none vs `groupDetail.ts:354`). Fix: add Arrow handling to the shared component. File `ui/src/integration-detail/tabs.ts:3`.
- **Accessibility** — Tabs miss `role="tablist"` on container (group has it, integration does not), each button should have `role="tab"` + `tabIndex` (−1 for inactive), panels need `role="tabpanel"` with `aria-labelledby`. Only group sets it. Fix: bring integration to same standard.
- **Polish** — Tabs have no icon or count. Labs: "Logs" is the most used but looks like the least descriptive (no badge). Consider a small `· 12` count from stats, but avoid over-design.

### 5 — Overview / Tools / Client config (`overview.ts`, `tools.ts`, `client-config.ts`)

- **Broken** — Endpoint URL `overview.ts:25` is a `<code class="mono">` with full `http://localhost:4242/mcp/<token>` but no wrapping control: parent `.inspector-row` is `display:flex; gap:12px; align-items:baseline` (`style.css:157`). At narrow widths the URL overflows the card with `overflow:hidden` nowhere — it pushes copy button off-screen. `groupDetail.ts:192` fixes this (adds `overflow:hidden; text-overflow:ellipsis; white-space:nowrap`) but `overview.ts` does not. Fix: apply same truncation + offer copy tooltip. Files `ui/src/integration-detail/overview.ts:25`, `ui/src/style.css:157`.
- **Confusing** — Tool mute `style.css:191` `.tool-off {opacity:0.6}` uses only opacity to show disabled tools, with no label change — low-contrast tools still look selectable. Fix: also strike/grey label + explanatory "Off — enable in Edit".
- **Confusing** — Client config "All detected" `client-config.ts:68` says "All detected" but `opts?.installed` is empty until Tauri `list_installed_mcp_clients` returns (`client-config.ts:191`). During that gap `targets()` returns empty (because `installed` defaults to `CLIENTS` only when not provided? actually line 53 defaults to all CLIENTS, so race is hidden — but if backend returns `[]`, UI shows "No clients detected" with no guidance). Also no error/empty body for zero install — just `list.textContent = "No clients detected"` (`client-config.ts:156`). Fix: show hint "No MCP client found — paste snippet manually". File `ui/src/integration-detail/client-config.ts:156`.
- **Inconsistent** — Card reuse diverges: `overview.ts` uses `section.card` with `h2.card-title`, `client-config.ts` uses generic `div.card` + `h2` with same class but extra inline controls (`style.css:131` vs `style.css:144`). Titles are `MCP endpoint` vs `Agent setup` — same content, two names. Fix: pick one term (prefer "Agent setup") or keep both but clarify subtitle.
- **Inconsistent** — Spacing: `overview.ts` stacks two sections with no gap container — relies on `card {margin-bottom:24px}` (`style.css:143`). `groupDetail.ts` wraps three sections in a `gap:24px` flex (`groupDetail.ts:173`). Two patterns for vertical rhythm. Fix: wrap every tab body in `.stack-lg` with token gap.
- **Accessibility** — Copy buttons (`overview.ts:29` "Copy" → "Copied!") change text without `aria-live`. User with screen reader hears nothing. Fix: add adjacent `role=status` live region or use `aria-label` toggle. Same for group endpoint (`groupDetail.ts:197`).
- **Polish** — Group endpoint `groupDetail.ts:219` animates copy feedback with `transform:scale(0.96)` gated by `prefers-reduced-motion` — good — but `overview.ts` animates via adding `class="copied"` which paints `#16a34a` but provides no scale/haptic, so feedback differs between two copies of same action. Fix: share one pattern.
- **Copy** — `groupDetail.ts:238` shows `Endpoint key: ${group.id}` (`client-config.ts:238`). `group.id` is a UUID-like identifier — internal value surfaced verbatim. Violates "no code identifiers" rule. Fix: show human-readable group name or truncated key with label "Key — use as endpoint name". File `ui/src/groupDetail.ts:238`.

### 6 — Forms (`ui/src/forms/render.ts`, `connectionDraft.ts`, `groupForm.ts`)

- **Broken** — Validation pattern is "disable Save" only (`render.ts:440` `save.disabled = !canSave(draft)`) with a passive hint underneath (`render.ts:446` "Enter a name …"). No field-level error under the empty Name input, focus is not moved to the invalid field, `aria-invalid` never set. User clicks nothing happens — appears frozen. Fix: enable Save, validate on click, put `role=alert` message under each bad field + `aria-invalid=true` + focus first invalid. Files `ui/src/forms/render.ts:440`, `446`.
- **Broken** — File field `render.ts:151` sets `onChange(file.files[0].name)` — stores filename only, not path. Middle-layer will look up `filename` as a literal path and fail silently. **Unconfirmed against backend**, but the code path cannot deliver a usable file. Should use Tauri `open` dialog and store full path. File `ui/src/forms/render.ts:168`.
- **Confusing** — Environment picker exists twice: a header row `<select>` at top (`render.ts:365`) plus environment-driven side effect in `connectionDraft.applyEnvironmentDefaults` that auto-flips `query.mode` to `"mutations"` for dev/local (`connectionDraft.ts:121`). The flip is invisible to the user — no badge "Mutations enabled for local dev". Fix: show non-blocking hint "Development — write tools enabled by default" linking to Tools section. File `ui/src/forms/connectionDraft.ts:121`.
- **Confusing** — Toggle fields `render.ts:127` are bare `<input type=checkbox>` with no styled switch and no label click target beyond the `div.inspector-label` sibling — the label is a `div`, not a `<label for>`. Click target is just the 13×13px checkbox. Fix: wrap in `<label>` so clicking the text toggles, and enlarge hit to 28px row height. File `ui/src/forms/render.ts:112`.
- **Inconsistent** — Field rendering uses `inspector-row` (flex, baseline, 6px padding `style.css:157`) for both data display and form editing — edit row then has one input flexing to fill, but help text `render.ts:124` sits under the input, not aligned to column — left edge misaligned with inputs above. Fix: two-column grid (`label 88px` + `field`) with help as second-row span.
- **Accessibility** — Number inputs `render.ts:176` fixed `width:120px` but no `inputMode`, `step`, `aria-describedby` for help; Select `field-select max-width:240px` (`style.css:276`) truncates long option labels with no title. Toggle inputs get `aria-label` (`render.ts:131`) but the visual label is a separate `div` — should be `aria-labelledby`.
- **Copy** — Field helper `render.ts:125` shows raw `field.help` (adapter-provided). Those strings sometimes include code-ish phrasing like "SSH host" — borderline internal vocab. Keep but audit against a tone pass ("Where Pluk tunnels with SSH").
- **Layout** — Tools section `render.ts:212` groups enabled first (`orderedTools`). When all tools disabled and a category "More tools" divider inserts (`render.ts:280`), the `card-title` inside a card is reused as a subsection header with no spacing guard — double-title stack feels accidental. Fix: use `h4` subsection style with `margin-top: var(--space-lg)`.
- **Polish** — Dangerous tools `render.ts:260` recolor the entire row `#dc2626` then add a warning div — color-only warning. Fix: keep color but also add `⚠` glyph + sentence in bold, not just color.

### 7 — Group forms / detail (`ui/src/groupDetail.ts`, `forms/render.ts` group branch)

- **Confusing** — Group checklist `render.ts:495` shows each integration as `label > input + span` plus an `envTag` with raw `development/production` lowercased value. The env tag background is fixed `#6b7280` inline (`groupDetail.ts:306` sets `rgba(0,0,0,0.06)` vs `render.ts:506` uses `.tag` `#f3f4f6`). Two tag styles for same concept. Fix: use one `.env-tag`.
- **Confusing** — Overrides `render.ts:514` "blank = inherit" hint appears per-member, but `inheritPlaceholder` (in `groupForm.ts`) may already be `inherit` — empty inputs look like missing data. Fix: show explicit word "Inherited" as a placeholder color distinct from typed value.
- **Inconsistent** — Member row `groupDetail.ts:270` uses `class="type-badge"` with two-letter abbrev for type, while sidebar uses `glyphElement` (`adapterColor` background). Two badge visuals for same adapter. Fix: share `glyphElement`.
- **Accessibility** — Member rows are `button.member-row` with `role=listitem` (`groupDetail.ts:272`), inside `div[role=list]` . Buttons are correct but `role=listitem` on a button inside list is redundant/incorrect — listitem should be wrapper, button inside. Fix: `div[role=listitem] > button` separation or use `ul/li`.

### 8 — Activity log (`ui/src/activityLog/activityLog.ts`, `style.css` al-*)

- **Broken** — The whole log is built via `innerHTML` strings (`activityLog.ts:175` `rowHtml`, `activityLog.ts:236` `metaLineHtml`, `activityLog.ts:254` strip). All dynamic strings escape with `escapeHtml`, good, but row toggle uses delegated `click` on `[data-id]` (`activityLog.ts:540`). The rows have `role="button"` + `tabIndex=0` (`activityLog.ts:177`) but keyboard handling only exists as native button clicks — no `keydown` handling for Enter/Space on rows. Keyboard users cannot expand rows. Fix: add `keydown` to toggle, or render rows as `<button>`.
- **Confusing** — Dual loading paths: `reload()` re-uses generation guard, but failure sets `isLoading=false` and only `renderLoadMore()` (`activityLog.ts:391`), so error is silent — stats stay stale, list appears frozen. No retry UI except manual Refresh button. Fix: show inline error banner "Couldn't load activity — Try again" with action.
- **Confusing** — Empty variants: toolbar has `Time / Show / Keep` + `↻ Refresh` + `Clear` (`activityLog.ts:58`), none disabled when empty. `Clear` confirms with `confirm("Clear all activity …")` (`activityLog.ts:495`) — raw browser confirm, no dialog styling, easy to mis-tap. `Keep` ("7/14/30/60/90/Forever") applies immediately to server retention with no undo (`activityLog.ts:488`). Fix: confirm with real dialog and explain consequence.
- **Inconsistent** — Lists cap row columns at 6 (`al-th flex:1`) with ellipsis (`activityLog.ts:254`), but no truncation affordance (no tooltip). Full-row modal is only for response, not table — row data may be silently hidden.
- **Inconsistent** — Time display: `relativeTime` (`time.ts`) vs `localTimeString` vs `al-time-ago` (mono). Three time styles for same event. Fix: one relative + one tooltip absolute.
- **Accessibility** — Toolbar selects (`activityLog.ts:65`) are inside `<label>` wrappers but have no explicit `for` linkage; OK but the `Keep` select mutates retention server-side without announcing via `aria-live`. Fix: announce after `setRetention`.
- **Accessibility** — `elStats` has `aria-live=polite` (`activityLog.ts:97`) but text changes on every keystroke in search (`updateStats()` reruns labels `activityLog.ts:138`). Noisy live region. Fix: debounce or move counts out of live region.
- **Layout** — Toolbar `al-toolbar {display:flex; gap:12px; flex-wrap:wrap}` (`style.css:280`) wraps at narrow detail widths but `al-search {min-width:160px; max-width:320px; flex:1}` may leave orphan Selects below. Verify at 440px — likely wraps to 3 rows. Acceptable but tight; token gap is not applied (`gap:12px` hardcoded).

### 9 — Response viewer (`ui/src/activityLog/responseViewer.ts`, `style.css`)

- **Inconsistent** — Overlay is `position:fixed; inset:0; z-index:1000` (`style.css:343`) while sidebar confirm overlay is also `z-index:100` (`sidebar.ts:209`) — two systems. Fix: share overlay scale.
- **Confusing** — Viewer is `resize: both` (`style.css:345`) with no min size, so user can drag it to 0×0. No drag handle hint, no persisted size. Fix: clamp `min-width:560px; min-height:360px` and store size to localStorage.
- **Accessibility** — Viewer header has `aria-label="Response"` but subtitle is the raw SQL/truncated query (`responseViewer.ts:52`) with `escapeHtml` but might contain long string. No `aria-describedby`. Escape closes (`responseViewer.ts:104`) — good — but focus is not trapped and not returned to the opener after close. Fix: trap and restore.
- **Polish** — Font/LH steppers (`responseViewer.ts:58`) use localStorage keys `responseFontSize / responseLineHeight` with magic defaults `13`/`4`, range `10–24`/`0–14`. Works, but no toolbar label — "A-/A+" and "LH-" are jargon for layperson. Fix: rename to "Size" / "Spacing".

### 10 — Type, tokens, general CSS (`tokens.css`, `typography.css`, `style.css`)

- **Inconsistent** — Type scale (`typography.css:6`) uses fractional `12.5px` (`--type-callout`), `11.5px` (`--type-caption`). Sub-pixel font sizes render differently on Tauri webview vs Safari; Chrome rounds differently. OK if tested, but no comment on rounding. Verify visually at 1.25× zoom — text may blur.
- **Inconsistent** — `tokens.css:22` comment says surfaces are "light defaults (overridden by prefers-color-scheme)" but `style.css:6` sets `body color-scheme: light dark` and components hardcode `#6b7280` / `#9ca3af` / `#f3f4f6` instead of using `var(--surface-tertiary-label)`. Dark mode contrast fails wherever hardcode leaks. Audit finds 30+ hardcoded colors in TS inline styles and CSS. Fix: codemod to tokens.
- **Accessibility** — Global `:focus-visible` is absent — only two rules (`shell.css:128`, `style.css:261`) wire `outline:2px solid #3b82f6`. Everything else uses default blue which may vanish against white panel. Fix: one global `*:focus-visible { outline:2px solid #3b82f6; outline-offset:2px; border-radius:2px }`.
- **Accessibility** — `color-scheme` alone does not fix form controls: `input`/`select` inside `.sidebar-search` have no explicit `background: transparent` for dark; placeholder contrast passes via `color: var(--surface-sidebar-label)` but `::placeholder` not verified.
- **Polish** — `style.css:278` `.card {background: var(--surface-card); border-radius:10px; padding:16px; margin-bottom:24px}` and `shell.css:131` repeats `.card` with added `border:1px solid rgba(0,0,0,0.06)` — duplicate definition, `style.css` version wins order-dependent. Fix: single card definition.

### 11 — Cross-cutting copy & empty/error states

- **Confusing** — Empty states exist in `emptyStates.ts` (dedicated, good copy) but are bypassed in `main.ts:133` which directly renders text for loading and nothing-selected. Loading shows bare `"Loading…"` (`main.ts:132`) without empty-style container, so transition from loading → empty jumps layout. Fix: share `Empty` component for loading with skeleton, not raw text. File `ui/src/main.ts:130`.
- **Inconsistent** — Sidebar empty copy vs detail empty copy: sidebar says "No integrations yet. Add your first one…" (`sidebar.ts:292`), detail says "Connect a service to get started" (`emptyStates.ts:19`). Near-duplicate, slightly different phrasing. Fix: reuse `emptyState("no-integrations")` for both.
- **Inconsistent** — Terminology: `sidebar.ts` section titles "Groups" / "Integrations" (title case), client config `CLIENTS` labels "opencode" (lowercase), "Claude Code" (title), "Antigravity" (capitalized). Mixed. File `ui/src/integration-detail/client-config.ts:4`.
- **Copy leak** — `groupDetail.ts:238` leaks `group.id` (identifier) and `client-config.ts:198` talks about `projectDir` — acceptable as label, but pairing with "Key" (`groupDetail.ts:238`) is internal seam. See R6.
- **Polish** — `humanizeHealthError` and `humanize` (`header.ts:13`) default to "Check the setup and try again." appended even when raw error already ends with user-friendly text. Double "Try again." possible if backend already contains that phrase — lowercasing guard (`includes("try again")`) is brittle. Fix: normalize once.

### 12 — Known rebuilds (flag only, per brief)

- **Icon system** — Unicode glyphs (`⌕`, `☰`, `▦`, `⊞`, `…`, `✓`, `✕`, "PG/MY/LT") appear throughout; no SVG sprite, sizes vary (`glyph.ts:45` `size*0.25` radius vs `style.css:31` fixed 34px badge). Teams should not patch per-file — wait for unified line-icon pass.
- **Modal / overlay shell** — `sidebar.ts` delete dialog, context menus, response viewer overlay each roll custom fixed overlay with different z-index and dim color (`rgba(0,0,0,0.24)` vs `0.4`). Teams should not add another modal — wait for shared modal shell with focus trap + escape + dim scale.

---

## Verdict

Pluk's UI is coherent in intent — tokens, type scale, and empty-state copy show taste — but the implementation scatters bespoke patterns. Biggest wins are not page patches but the 7 system pieces named under Root causes. Shipped fixes should land in this order: shared primitives (Button/Card/Badge/Menu/Dialog), token discipline lint, empty/loading/error convention, and glossary lock.

## Follow-up Status — 2026-08-28

- **Resolved: 43 of 48 findings.** The current pass closes the remaining behavior, layout, accessibility, copy, and lifecycle findings that had concrete fixes.
- **Resolved anchors:** shell sizing, sidebar menus and lifecycle, detail titles and humanized errors, shared tabs, endpoint truncation, tool states, client empty state, copy announcements, form validation and controls, group semantics, activity retention and time affordances, response viewer sizing, and tokenized styling.
- **Not fixed: 5 findings.** The open items are the dedicated icon track, centralized adapter brand colors, adapter-owned helper copy, optional tab counts, and fractional type-size visual verification.
- **Runtime gap:** No browser automation or Tauri host was available for the requested narrow-window observations.

## Resolution Register

- **Resolved — Broken:** Shell narrow-window clipping, sidebar context-menu access, delete confirmation, endpoint overflow, form validation, file selection in Tauri, and activity-log keyboard/load recovery.
- **Resolved — Confusing:** Resizer keyboard access, filter popover lifecycle, detail title truncation, tab navigation, disabled-tool state, empty client guidance, environment defaults, form toggles, and retention confirmation.
- **Resolved — Inconsistent:** Sidebar seam, header status/badge tokens, shared tabs/cards, overview spacing, copy feedback, group badges and list semantics, activity columns/time display, overlay scale, and token bypasses.
- **Resolved — Accessibility:** Global focus, toolbar focus, tab/panel roles, copy announcements, form descriptions, row activation, retention announcements, and response-modal focus handling.
- **Resolved — Polish:** Toast placement, dead banner actions, dangerous-setting warning, activity toolbar wrapping, response-viewer sizing controls, and response control labels.
- **Not fixed — dedicated icon track:** Unicode and adapter-abbreviation iconography remains intentionally deferred to the unified icon rebuild.
- **Not fixed — canonical adapter colors:** Brand colors remain in `glyph.ts` as the single adapter-color source; moving them into theme tokens would change brand semantics.
- **Not fixed — adapter helper copy:** Field help comes from adapter catalog data and needs review at its source rather than a UI-side rewrite.
- **Not fixed — tab counts:** Product decision needed: should `Logs`, `Overview`, and `Tools` show counts?
- **Not fixed — fractional type sizes:** Runtime zoom verification is still needed before changing the established `12.5px` and `11.5px` scale.

---

## How to use this audit as follow-up tasks

Each H-group under *Findings by surface* is one dispatchable unit. Suggested dispatch shape (for caller):

- `fix-shell-layout` (Root 7 + Shell)
- `fix-sidebar-accessibility-and-menu` (Sidebar + one Menu primitive) — include `Group member list` row mismatch
- `fix-detail-header-tabs` (Header + Tabs unification)
- `fix-overview-client-config` (Overview/Tools/Client — truncation + copy live-region)
- `fix-forms-validation-and-a11y` (Forms validation, file field, toggle labels)
- `fix-activity-log-states` (Log loading/empty/error, keyboard, live-region noise)
- `fix-tokens-and-focus-contract` (global focus ring + hardcoded color sweep)

Leave icon + modal rebuilds to their dedicated tracks; others should not invent icons or dialogs.

---

*Audit performed by reading source only, without a running render. Entries marked "unconfirmed" need browser verification before sizing a fix.*
