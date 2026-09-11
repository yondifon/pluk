# New Integration: UX diagnosis and proposed flow

## Verdict

The modal treats every integration as a config form. Wande is not a config form, it is a pairing handshake plus a tool list, and the one step that actually makes it work (pairing Chrome) is not in the modal at all. It shows up after save, on a tab the user has to find. Replace the single scrolling modal with a short numbered flow where each adapter type only shows the steps it actually needs, and give Wande a pairing step inside that flow instead of leaving it for Overview to discover.

## Findings

### P0: Chrome pairing is missing from the flow entirely

- **Evidence (Observed)**: `renderIntegrationForm` (`ui/src/forms/render.ts:508`) builds General, config fields, Tools, and Approvals. Nothing in it reads or shows a Pluk ID or a Chrome connection status. That card only exists in `renderBrowserAccess` (`ui/src/integration-detail/browser-access.ts:39`), mounted from `renderOverview` (`ui/src/integration-detail/overview.ts:18-26`) after the integration is already saved. `saveIntegration` in `ui/src/main.ts:374-399` creates the row and jumps straight to the Overview tab, never checking pairing state first.
- **Effect**: a user finishes the modal, clicks Save, and lands on a screen that looks done. There is nothing in the flow that says "one more step." Finding the Chrome card, reading the Pluk ID, opening the Wande extension, and pasting it in is left to the user stumbling onto the Overview tab.
- **Direction**: pull pairing into the creation flow as its own step, before the integration is marked ready to use.
- **Verify**: open the app, click New Integration, choose Wande, fill in a name, and save. Confirm nothing in the modal ever shows a Pluk ID or asks about Chrome.

### P1: The Tools step asks Wande users to tune tools they cannot test yet

- **Evidence (Observed)**: `WandeAdapter::config_fields` returns `&[]` (`crates/pluk-adapters/src/wande/mod.rs:112-114`), so `groupedFields` in the modal renders nothing for Wande. The very next thing shown is `renderToolsSection` (`ui/src/forms/render.ts:587-616`), a full on/off list of every X read, post, and reply tool, with per-tool settings expanded for anything enabled.
- **Effect**: right after typing a name, a Wande user is asked to decide which posting and reading abilities an agent should have, for a service that is not connected to anything yet. There is no way to try a tool or see what it does before deciding.
- **Direction**: for an adapter with no config fields, tool tuning is not the second thing a person needs. Default the tools on and let refinement happen once Chrome is paired and the integration is real, not mid-creation.
- **Verify**: same repro as P0. Watch the modal go from Name straight to a full tools checklist with no adapter-specific setup in between.

### P2: The type chooser is one flat, undescribed list

- **Evidence (Observed)**: `renderTypeChooser` (`ui/src/forms/render.ts:12-91`) iterates `adapters` in registration order with only a label and a badge (`btn.innerHTML = ...a.label...`). `default_registry` (`crates/pluk-adapters/src/registry.rs:60-84`) currently registers SQL databases, SSH, Wande, Redis, MongoDB, Slack, Linear, Sentry, GitHub CLI, and Spark, nine-plus entries in one undifferentiated grid. `groupedByCategory` already exists in `ui/src/forms/catalog.ts:79-90` (adapters carry a `category`, Wande's is `"social"`) but `renderTypeChooser` never calls it.
- **Effect**: a person who wants "post to X" has to recognize "Wande" as the right row with no description and no grouping to narrow the list, in a set that has grown well past what the brief assumed (SSH, SQL, GitHub CLI, Wande).
- **Direction**: group the chooser by the category data that already exists, or add a one-line description per row so the label is not the only signal.
- **Verify**: open the type chooser and count the rows against `crates/pluk-adapters/src/registry.rs:66-82`; none carry a description.

### P3: Save succeeds without checking Chrome is actually connected

- **Evidence (Observed)**: `saveIntegration` (`ui/src/main.ts:374-399`) only validates approval rules (`check_approval_rules`) before calling `create_integration`. It never calls the pairing check the adapter already exposes (`WandeAdapter::test_connection`, `crates/pluk-adapters/src/wande/mod.rs:127-134`, backed by `chrome_is_connected`). Pairing status only surfaces later, via polling in `renderBrowserAccess` (`ui/src/integration-detail/browser-access.ts:89-98`, every 5 seconds) or when a tool call fails.
- **Effect**: the modal's Save button behaves identically whether Chrome is paired or not, so "saved" does not mean "usable." There is no error state in the creation flow for "not paired yet," only a badge discovered afterward.
- **Direction**: this does not need to block Save (pairing is legitimately a separate, async step), but the flow should say so directly, "saved, now connect Chrome," instead of implying the two are the same milestone.
- **Verify**: `crates/pluk-adapters/src/wande/mod.rs:127-134` for the existing check, `crates/pluk-host/src/commands.rs` for `check_approval_rules` and `create_integration` to confirm neither calls it.

## Proposed flow

A short numbered flow replaces the one scrolling modal. Each step is its own screen; Back and Cancel are always available. Below is the path for creating a Wande integration end to end.

1. **Choose what to connect**
   - Helper: "Pick what Pluk should talk to."
   - Primary action: none, picking a tile advances.
   - On error: catalog fails to load, shows "Couldn't load integrations" with a "Try again" button (existing copy, unchanged).

2. **Name it**
   - Helper: "Agents will see this name when they use it."
   - Primary button: "Continue"
   - On error: "Enter a name to continue." (existing copy, kept), shown under the name field, focus moves there.

3. **Connect Chrome**
   - Helper: "Paste this Pluk ID into the Wande extension in Chrome, then come back here."
   - Body: the Pluk ID with a Copy button, and a live status line ("Not connected" / "Connected") that updates on its own once the extension checks in.
   - Primary button: "Continue" (enabled once status reads Connected; a "Skip for now" link lets someone finish setup and pair later).
   - On error: pairing check fails to load, "Pluk can't show this right now. Restart Pluk and try again." (existing copy, kept).

4. **Choose what the agent can do**
   - Helper: "Turn off anything you don't want an agent posting or reading."
   - Body: the same tool list as today, defaulted on, no settings to fill in first.
   - Primary button: "Save integration"
   - On error: save fails, a toast with the error message; the step stays open so nothing already chosen is lost.

5. **Install into your agent**
   - Helper: "Add this to the AI tool you use, so it can reach Wande."
   - Body: the existing endpoint and client install controls (OpenCode, Claude Code, Cursor, and the rest), unchanged.
   - Primary button: "Install" (per client) or "Done" to close the flow and land on the integration's page.
   - On error: "No AI client found", "Copy the snippet and paste it into your client's config." (existing copy, kept); a failed install shows "Install didn't finish" with per-client detail (existing copy, kept).

Five steps, one job each. Someone who wants to skip ahead (already paired, already knows their tool choices) can hit Continue through each without reading anything twice, and nothing after Save requires a trip to a different tab to finish setup.

## What changes for the other adapter types

- **Steps 1, 2, and 5 are the same for every adapter.** Every type publishes an MCP endpoint (`renderMcpSection` is called for all integrations in `ui/src/integration-detail/overview.ts:56-67`), so "install into your agent" closes every flow, not just Wande's.
- **Step 3 becomes "Connect" instead of "Connect Chrome"** for adapters with config fields (the SQL databases, Redis, MongoDB, Slack, Linear, Sentry, GitHub CLI): it shows that adapter's connection fields, grouped the way `groupedFields` already groups them, instead of a pairing card. Adapters with zero config fields (Wande today, and any future adapter shaped like it) get the pairing card instead. Adapters with neither (none currently) skip straight to step 4.
- **Adapters that run commands get one extra step.** SSH, GitHub CLI, and Spark set `runs_commands(true)` (`crates/pluk-adapters/src/ssh/mod.rs:77`, `crates/pluk-adapters/src/github_cli/mod.rs:1573`, `crates/pluk-adapters/src/spark/mod.rs:1409`). For those, step 4 becomes two screens: "Choose what the agent can do" (tools) and "What it's allowed to run" (the existing allow/deny/ask rules), instead of both stacked in one long card as they are in today's modal.
- **Step 1 gains grouping.** With nine-plus adapter types now registered, the flat list in `renderTypeChooser` should use the category data `groupedByCategory` already computes, so "Post and read" (Wande), "Run commands" (SSH, GitHub CLI), and "Connect data" (the rest) separate the list instead of one long undifferentiated grid.
