import AppKit
import SwiftUI

// MARK: - MCP client config snippets

enum MCPClient: String, CaseIterable, Identifiable {
    case opencode, codex, claudeCode, cursor, windsurf, antigravity

    var id: String { rawValue }

    var label: String {
        switch self {
        case .opencode: "opencode"
        case .codex: "Codex"
        case .claudeCode: "Claude Code"
        case .cursor: "Cursor"
        case .windsurf: "Windsurf"
        case .antigravity: "Antigravity"
        }
    }

    var format: ConfigFormat { self == .codex ? .toml : .json }

    // JSON key holding the server map. opencode nests under "mcp"; every other
    // JSON client uses "mcpServers". (Codex is TOML and ignores this.)
    var containerKey: String { self == .opencode ? "mcp" : "mcpServers" }

    // Config locations this client understands. Project-scoped clients get a
    // per-repo file; the rest only have a single global file.
    var supportedScopes: [ConfigScope] {
        switch self {
        case .opencode, .claudeCode, .cursor: [.project, .global]
        case .codex, .windsurf, .antigravity: [.global]
        }
    }

    // Config file for a scope. Project paths are relative to a chosen repo root;
    // global paths are absolute (a leading ~ is expanded by the injector).
    // Global-only clients fall back to their global path for either scope.
    func configPath(_ scope: ConfigScope) -> String {
        switch scope {
        case .project:
            switch self {
            case .opencode: "opencode.json"
            case .claudeCode: ".mcp.json"
            case .cursor: ".cursor/mcp.json"
            case .codex, .windsurf, .antigravity: configPath(.global)
            }
        case .global:
            switch self {
            case .opencode: "~/.config/opencode/opencode.json"
            case .codex: "~/.codex/config.toml"
            case .claudeCode: "~/.claude.json"
            case .cursor: "~/.cursor/mcp.json"
            case .windsurf: "~/.codeium/windsurf/mcp_config.json"
            case .antigravity: "~/.gemini/config/mcp_config.json"
            }
        }
    }

    var configLanguage: String { self == .codex ? "toml" : "json" }

    // Paths that mark the client as present on this machine — its state dir, its
    // global config, or the app bundle. Any hit counts; a project file may not
    // exist yet even when the client is installed.
    var detectionPaths: [String] {
        switch self {
        case .opencode:    ["~/.config/opencode", "~/.local/share/opencode"]
        case .codex:       ["~/.codex"]
        case .claudeCode:  ["~/.claude", "~/.claude.json"]
        case .cursor:      ["~/.cursor", "/Applications/Cursor.app"]
        case .windsurf:    ["~/.codeium/windsurf", "/Applications/Windsurf.app"]
        case .antigravity: ["~/.gemini", "/Applications/Antigravity.app"]
        }
    }

    var isInstalled: Bool {
        detectionPaths.contains {
            FileManager.default.fileExists(atPath: ($0 as NSString).expandingTildeInPath)
        }
    }

    static var installed: [MCPClient] { allCases.filter(\.isInstalled) }

    // The server's value object as written into a JSON config. Mirrors the shape
    // rendered by `snippet` below — keep the two in sync. (Codex is TOML; the
    // injector writes its `url = "…"` block directly.)
    func entryObject(url: String) -> [String: Any] {
        switch self {
        case .opencode:
            return ["type": "remote", "enabled": true, "url": url, "oauth": false]
        case .claudeCode:
            return ["type": "http", "url": url]
        case .cursor:
            return ["command": "bunx", "args": ["mcp-remote", url]]
        case .windsurf, .antigravity:
            return ["serverUrl": url]
        case .codex:
            return ["url": url]
        }
    }

    func snippet(key: String, url: String) -> String {
        switch self {
        case .opencode:
            return """
            {
              "mcp": {
                "\(key)": {
                  "type": "remote",
                  "enabled": true,
                  "url": "\(url)",
                  "oauth": false
                }
              }
            }
            """
        case .codex:
            return """
            [mcp_servers.\(key)]
            url = "\(url)"
            """
        case .claudeCode:
            // Claude Code speaks HTTP transport natively — no mcp-remote wrapper.
            return """
            {
              "mcpServers": {
                "\(key)": {
                  "type": "http",
                  "url": "\(url)"
                }
              }
            }
            """
        case .cursor:
            return """
            {
              "mcpServers": {
                "\(key)": {
                  "command": "bunx",
                  "args": ["mcp-remote", "\(url)"]
                }
              }
            }
            """
        case .windsurf, .antigravity:
            // Both read mcpServers with serverUrl for remote (Streamable HTTP)
            // servers; Antigravity's config is shared by its IDE and CLI.
            return """
            {
              "mcpServers": {
                "\(key)": {
                  "serverUrl": "\(url)"
                }
              }
            }
            """
        }
    }
}

// What the client picker holds: one client, or every client detected on this
// machine. "All" fans the write out instead of making the user repeat it per
// tool — global writes each client's user-level file, project writes the
// per-repo file of every client that has one.
enum ClientChoice: Hashable, Identifiable {
    case all
    case one(MCPClient)

    var id: String {
        switch self {
        case .all: "all"
        case .one(let client): client.rawValue
        }
    }

    var label: String {
        switch self {
        case .all: "All detected"
        case .one(let client): client.label
        }
    }

    // Clients this choice writes to for a scope. "All" keeps only detected
    // clients that actually own a file at that scope, so Project skips the
    // global-only ones (Codex, Windsurf, Antigravity).
    func targets(scope: ConfigScope) -> [MCPClient] {
        switch self {
        case .one(let client): [client]
        case .all: MCPClient.installed.filter { $0.supportedScopes.contains(scope) }
        }
    }

    var supportedScopes: [ConfigScope] {
        switch self {
        case .one(let client): client.supportedScopes
        case .all: ConfigScope.allCases
        }
    }

    static var allChoices: [ClientChoice] { [.all] + MCPClient.allCases.map(ClientChoice.one) }
}

// MARK: - Config snippet section

// Shared "Config" card for integration and group detail views: client picker +
// one Copy action above a flat, chrome-less snippet. The snippet renders
// embedded so the DetailSection card stays the only surface.
struct ConfigSnippetSection: View {
    let mcpKey: String
    let mcpURL: String
    // Identify the integration/group for the result toast.
    let title: String
    let id: String
    let toastCenter: ToastCenter?

    @State private var selectedChoice: ClientChoice = .all
    @State private var selectedScope: ConfigScope = .project
    @State private var copied = false
    @SwiftUI.Environment(\.accessibilityReduceMotion) private var reduceMotion

    // The single client a snippet can be shown/copied for. Nil under "All".
    private var singleClient: MCPClient? {
        if case .one(let client) = selectedChoice { return client }
        return nil
    }

    private var targets: [MCPClient] { selectedChoice.targets(scope: selectedScope) }

    private var snippetMarkdown: String? {
        guard let client = singleClient else { return nil }
        return "```\(client.configLanguage)\n\(client.snippet(key: mcpKey, url: mcpURL))\n```"
    }

    var body: some View {
        DetailSection("Config") {
            HStack(spacing: Space.sm) {
                Text("Client")
                    .scaledFont(.callout)
                    .foregroundColor(.secondary)
                Menu {
                    ForEach(ClientChoice.allChoices) { choice in
                        Button { selectedChoice = choice } label: {
                            Text(choice.label)
                        }
                    }
                } label: {
                    HStack(spacing: Space.xs) {
                        Text(selectedChoice.label)
                            .scaledFont(.callout)
                        Image(systemName: "chevron.up.chevron.down")
                            .font(.system(size: 8))
                    }
                    .foregroundColor(.secondary)
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
                .onChange(of: selectedChoice) { _, choice in
                    // Keep the scope valid when switching to a global-only client.
                    if !choice.supportedScopes.contains(selectedScope) {
                        selectedScope = choice.supportedScopes.first ?? .global
                    }
                }
                // Only offer a scope choice when the selection has more than one.
                if selectedChoice.supportedScopes.count > 1 {
                    Menu {
                        ForEach(selectedChoice.supportedScopes) { scope in
                            Button { selectedScope = scope } label: {
                                Text(scope.label)
                            }
                        }
                    } label: {
                        HStack(spacing: Space.xs) {
                            Text(selectedScope.label)
                                .scaledFont(.callout)
                            Image(systemName: "chevron.up.chevron.down")
                                .font(.system(size: 8))
                        }
                        .foregroundColor(.secondary)
                    }
                    .menuStyle(.borderlessButton)
                    .menuIndicator(.hidden)
                    .fixedSize()
                }
                Spacer()
                Button("Add") { addToConfig() }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .disabled(targets.isEmpty)
                if let client = singleClient {
                    Button(copied ? "Copied!" : "Copy") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(
                            client.snippet(key: mcpKey, url: mcpURL), forType: .string)
                        copied = true
                        Task { @MainActor in
                            try? await Task.sleep(for: .seconds(1.5))
                            copied = false
                        }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .tint(copied ? .green : nil)
                    .animation(reduceMotion ? nil : .easeInOut(duration: 0.15), value: copied)
                }
            }
            .padding(.horizontal, Space.md)
            .padding(.top, Space.md)
            .padding(.bottom, Space.sm)

            VStack(alignment: .leading, spacing: Space.sm) {
                if let markdown = snippetMarkdown, let client = singleClient {
                    HStack(spacing: Space.xs + 1) {
                        Text("Add to")
                            .scaledFont(.caption)
                            .foregroundColor(.secondary)
                        Text(client.configPath(selectedScope))
                            .font(.mono(11))
                            .textSelection(.enabled)
                    }

                    MarkdownResponseView(markdown: markdown, embedded: true)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(Space.md)
                        .codeBlockSurface(cornerRadius: Radius.small)
                } else {
                    allTargetList
                }
            }
            .padding(.horizontal, Space.md)
            .padding(.bottom, Space.md)
        }
    }

    // Under "All", the snippet is replaced by exactly what Add will touch — one
    // line per detected client, so the fan-out is never a blind write.
    private var allTargetList: some View {
        VStack(alignment: .leading, spacing: Space.sm) {
            Text(targets.isEmpty ? "No AI clients detected" : "Add to")
                .scaledFont(.caption)
                .foregroundColor(.secondary)

            if !targets.isEmpty {
                VStack(spacing: 0) {
                    ForEach(targets) { client in
                        HStack(alignment: .firstTextBaseline, spacing: Space.md) {
                            Text(client.label)
                                .scaledFont(.callout)
                            Spacer(minLength: Space.md)
                            Text(client.configPath(selectedScope))
                                .font(.mono(10))
                                .foregroundColor(.secondary)
                                .lineLimit(1)
                                .truncationMode(.middle)
                                .textSelection(.enabled)
                        }
                        .padding(.horizontal, Space.md)
                        .padding(.vertical, Space.sm + 1)
                    }
                }
            }
        }
    }

    // Write the entry into the selected client's config. Project scope asks for
    // the repo folder first; global writes straight to the user-level file.
    private func addToConfig() {
        let clients = targets
        guard !clients.isEmpty else { return }
        guard selectedScope != .project else {
            let panel = NSOpenPanel()
            panel.canChooseDirectories = true
            panel.canChooseFiles = false
            panel.allowsMultipleSelection = false
            panel.prompt = "Add Here"
            panel.message = clients.count == 1
                ? "Choose the project folder for \(clients[0].label)"
                : "Choose the project folder for \(clients.count) clients"
            guard panel.runModal() == .OK, let dir = panel.url?.path else { return }
            inject(clients: clients, projectDir: dir)
            return
        }
        inject(clients: clients, projectDir: nil)
    }

    // One write per client, then a single toast. A client that fails doesn't
    // stop the others — its error is reported alongside what did land.
    private func inject(clients: [MCPClient], projectDir: String?) {
        var added: [String] = []
        var skipped: [String] = []
        var failed: [String] = []
        var lastPath = ""

        for client in clients {
            do {
                let result = try MCPConfigInjector.inject(
                    client: client, scope: selectedScope,
                    projectDir: projectDir, key: mcpKey, url: mcpURL)
                switch result {
                case .added(let path):
                    added.append(client.label)
                    lastPath = path
                case .skipped(let path):
                    skipped.append(client.label)
                    lastPath = path
                }
            } catch {
                failed.append("\(client.label): \(error.localizedDescription)")
            }
        }

        // Single client keeps the path-first wording it always had.
        if clients.count == 1 {
            if let error = failed.first {
                presentToast(.error, error)
            } else if added.isEmpty {
                presentToast(.success, "\(mcpKey) already in \(pretty(lastPath)) — left unchanged")
            } else {
                presentToast(.success, "Added \(mcpKey) to \(pretty(lastPath))")
            }
            return
        }

        var parts: [String] = []
        if !added.isEmpty { parts.append("Added \(mcpKey) to \(added.joined(separator: ", "))") }
        if !skipped.isEmpty { parts.append("already in \(skipped.joined(separator: ", "))") }
        if !failed.isEmpty { parts.append("failed: \(failed.joined(separator: "; "))") }
        presentToast(failed.isEmpty ? .success : .error, parts.joined(separator: " · "))
    }

    private func presentToast(_ kind: Toast.Kind, _ message: String) {
        toastCenter?.present(Toast(connectionId: id, title: title, message: message, kind: kind))
    }

    // Collapse the home dir back to ~ so the toast path stays readable.
    private func pretty(_ path: String) -> String {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        return path.hasPrefix(home) ? "~" + path.dropFirst(home.count) : path
    }
}

// MARK: - Detail tabs

private enum DetailTab: String, CaseIterable {
    case logs     = "Logs"
    case overview = "Overview"
    case policy   = "Tools"
}

// MARK: - Detail view

struct ConnectionDetailView: View {
    let conn: Connection
    let store: ConnectionStore
    let onEdit: () -> Void
    let onDelete: () -> Void
    let onDuplicate: () -> Void

    @State private var selectedTab: DetailTab = .logs
    @State private var urlCopied = false
    @State private var testStatus: TestStatus = .idle
    @SwiftUI.Environment(\.accessibilityReduceMotion) private var reduceMotion

    enum TestStatus { case idle, testing, ok, fail(String) }

    var body: some View {
        VStack(spacing: 0) {
            header
            tabBar
            tabContent
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background(Surface.content)
    }

    // MARK: - Header

    // Identity on the left, state in the middle, actions on the right. The meta
    // line answers the questions someone pointing an agent at this integration
    // actually asks — what service, which environment, can it write, how much
    // of its tool surface is exposed — without needing a trip to another tab.
    // The controls ride inside the title row, not alongside the whole block. Sat
    // in the outer HStack they were top-aligned against a 34pt badge and a
    // two-line text stack, so chip, button and menu each landed at a different
    // height; here they simply centre on the title line.
    private var header: some View {
        HStack(alignment: .top, spacing: Space.md) {
            TypeBadge(type: conn.type, size: 34)
            VStack(alignment: .leading, spacing: Space.xs + 1) {
                HStack(alignment: .center, spacing: Space.md) {
                    Text(conn.name)
                        .scaledFont(.title2, weight: .semibold)
                        .tracking(-0.2)
                        .lineLimit(1)
                    Spacer(minLength: Space.md)
                    StatusChip(status: status, checkedAt: health?.at, detail: health?.error)
                    headerActions
                }
                HStack(spacing: Space.sm - 2) {
                    Text(metaLine)
                        .scaledFont(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                    if conn.readOnly {
                        Tag(text: "Read-only", systemImage: "lock.fill")
                    }
                }
            }
        }
        .padding(.horizontal, Space.xl)
        .padding(.top, Space.lg)
        .padding(.bottom, Space.md)
    }

    private var metaLine: String {
        var parts = ["\(conn.typeLabel) · \(conn.environment.label)"]
        let tools = adapterManifest?.tools ?? []
        if !tools.isEmpty { parts.append("\(tools.filter(isEnabled).count)/\(tools.count) tools") }
        return parts.joined(separator: "  ·  ")
    }

    // One primary action plus an overflow: Test is the thing people press, the
    // rest are occasional and don't need to sit on screen as four equal buttons.
    private var headerActions: some View {
        HStack(alignment: .center, spacing: Space.sm - 2) {
            headerTestButton
            OverflowMenu {
                Button("Edit…", action: onEdit)
                Button("Duplicate", action: onDuplicate)
                Divider()
                Button("Delete…", role: .destructive, action: onDelete)
            }
        }
    }

    // MARK: - Tab bar

    private var tabBar: some View {
        TextTabs(tabs: DetailTab.allCases, title: \.rawValue, selection: $selectedTab)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.top, Space.sm)
            .padding(.horizontal, Space.xl - Space.xs)
            .padding(.bottom, Space.sm)
    }

    // MARK: - Tab content

    @ViewBuilder
    private var tabContent: some View {
        switch selectedTab {
        case .logs:     LogsTab(scope: .connection(conn), store: store)
        case .overview: overviewTab
        case .policy:   policyTab
        }
    }

    // MARK: - Overview tab

    private var overviewTab: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Space.xl) {
                mcpURLSection
                ConfigSnippetSection(mcpKey: conn.mcpKey, mcpURL: conn.mcpURL,
                                     title: conn.name, id: conn.id,
                                     toastCenter: store.toastCenter)
                connectionDetailsSection
            }
            .padding(Space.xl)
        }
    }

    // MARK: - MCP URL

    private var mcpURLSection: some View {
        DetailSection("MCP endpoint") {
            InspectorRow("URL") {
                HStack(spacing: Space.sm) {
                    Text(conn.mcpURL)
                        .font(.mono(12))
                        .foregroundColor(.primary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer()
                    copyURLButton
                }
            }
            if let hint = agentHint {
                InspectorRow("Agent hint", value: hint)
            }
        }
    }

    private var agentHint: String? {
        store.adapters.first { $0.id == conn.type }?.agentHint
    }

    private var copyURLButton: some View {
        Button(urlCopied ? "Copied!" : "Copy") {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(conn.mcpURL, forType: .string)
            urlCopied = true
            Task { @MainActor in
                try? await Task.sleep(for: .seconds(1.5))
                urlCopied = false
            }
        }
        .buttonStyle(.borderedProminent)
        .controlSize(.small)
        .tint(urlCopied ? .green : .accentColor)
        .animation(reduceMotion ? nil : .easeInOut(duration: 0.15), value: urlCopied)
    }

    // MARK: - Connection details

    private var connectionDetailsSection: some View {
        DetailSection("Configuration") {
            if conn.type == "sqlite" {
                InspectorRow("File", value: conn.config["filename"] ?? "-")
                InspectorRow("SSH", value: conn.config["use_ssh"] == "true" ? (conn.config["ssh_host"] ?? "-") : "Off")
            } else if conn.connectionType != nil {
                InspectorRow("Host", value: conn.config["host"] ?? "-")
                InspectorRow("Port", value: conn.config["port"] ?? "-")
                InspectorRow("User", value: conn.config["user"] ?? "-")
                InspectorRow("Database", value: conn.config["database"] ?? "-")
                InspectorRow("SSH", value: conn.config["use_ssh"] == "true" ? (conn.config["ssh_host"] ?? "-") : "Off")
                InspectorRow("SSL", value: conn.config["use_ssl"] == "true" ? (conn.config["ssl_mode"] ?? "On") : "Off")
            } else {
                // Non-database adapter: show its config, masking secret-looking values.
                ForEach(genericConfigRows, id: \.0) { key, value in
                    InspectorRow(key, value: value)
                }
            }
        }
    }

    private var genericConfigRows: [(String, String)] {
        let secretKeys = Set(
            (store.adapters.first { $0.id == conn.type }?.configFields ?? [])
                .filter { $0.secret == true }
                .map(\.key)
        )
        return conn.config.sorted { $0.key < $1.key }.map { key, value in
            let pretty = key.replacingOccurrences(of: "_", with: " ").capitalized
            return (pretty, secretKeys.contains(key) ? "••••••" : value)
        }
    }

    // MARK: - Test (header action)

    private var isTesting: Bool { if case .testing = testStatus { return true }; return false }

    // A top-right action: tap to test. The result is just a small glyph beside the
    // button (spinner / green check / red x) — the outcome, success or failure, is
    // delivered as a toast, so no message crowds the header.
    @ViewBuilder
    private var headerTestButton: some View {
        HStack(alignment: .center, spacing: 6) {
            switch testStatus {
            case .idle:
                EmptyView()
            case .testing:
                ProgressView().scaleEffect(0.55)
            case .ok:
                Image(systemName: "checkmark.circle.fill")
                    .foregroundStyle(.green)
                    .font(.system(size: 13))
                    .onAppear { resetTestStatus(after: 3) }
            case .fail:
                Image(systemName: "xmark.circle.fill")
                    .foregroundStyle(.red)
                    .font(.system(size: 13))
                    .onAppear { resetTestStatus(after: 5) }
            }
            Button("Test", action: runTest)
                .buttonStyle(.bordered)
                .controlSize(.small)
                .frame(height: Control.height)
                .disabled(isTesting)
        }
    }

    private func runTest() {
        testStatus = .testing
        Task {
            do {
                let url = URL(string: PlukServer.api("integrations/\(conn.id)/test"))!
                var req = URLRequest(url: url)
                req.httpMethod = "POST"
                req.timeoutInterval = 12
                let (data, _) = try await URLSession.shared.data(for: req)
                let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
                let ok = json?["ok"] as? Bool == true
                let error = json?["error"] as? String ?? "Unknown error"
                await MainActor.run {
                    testStatus = ok ? .ok : .fail(error)
                    presentTestToast(ok: ok, message: ok ? "Connected." : error)
                }
                // The test wrote health server-side; pull it so the dot updates now.
                await store.refreshHealth()
            } catch {
                await MainActor.run {
                    testStatus = .fail(error.localizedDescription)
                    presentTestToast(ok: false, message: error.localizedDescription)
                }
            }
        }
    }

    private func presentTestToast(ok: Bool, message: String) {
        store.toastCenter?.present(Toast(
            connectionId: conn.id,
            title: conn.name,
            message: message,
            kind: ok ? .success : .error
        ))
    }

    private func resetTestStatus(after seconds: Double) {
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(seconds))
            testStatus = .idle
        }
    }

    // MARK: - Policy tab

    // Action-policy adapters (Linear, …) don't have SQL statements — show their
    // read/write permissions instead of statement categories + SQL guards.
    // Resolved policy kind for this connection's adapter: "sql" | "action" |
    // "none". Falls back to SQL vs action by connection shape when unknown.
    private var adapterManifest: AdapterManifest? {
        store.adapters.first(where: { $0.id == conn.type })
    }

    private func isEnabled(_ tool: AdapterToolDef) -> Bool {
        conn.toolConfig[tool.name]?.enabled ?? tool.defaultEnabled
    }

    // A read-only mirror of the per-tool config: which tools the agent can see and
    // how each enabled tool is configured.
    @ViewBuilder
    private var policyTab: some View {
        let tools = adapterManifest?.tools ?? []
        ScrollView {
            VStack(alignment: .leading, spacing: Space.xl) {
                if tools.isEmpty {
                    DetailSection("Tools") {
                        Text("Tool list unavailable — the local pluk server isn't responding.")
                            .scaledFont(.callout)
                            .foregroundColor(.secondary)
                            .padding(.horizontal, Space.md)
                            .padding(.vertical, Space.md)
                    }
                } else {
                    // Enabled first: the surface the agent actually has, then the
                    // off tools below for reference. The count lives in the header
                    // instead of its own card — same fact, one less box.
                    DetailSection("\(tools.filter(isEnabled).count) of \(tools.count) tools exposed to the agent") {
                        ForEach(tools.filter(isEnabled) + tools.filter { !isEnabled($0) }) { tool in
                            toolStatusRow(tool)
                        }
                    }
                }
            }
            .padding(Space.xl)
        }
    }

    @ViewBuilder
    private func toolStatusRow(_ tool: AdapterToolDef) -> some View {
        let enabled = isEnabled(tool)
        HStack(alignment: .top, spacing: Space.md) {
            // A dot, not a pill: the list is long, and "which of these is live"
            // reads faster as a column of marks than as a column of words.
            Circle()
                .fill(enabled ? Color.green : Color.secondary.opacity(0.35))
                .frame(width: 6, height: 6)
                .padding(.top, 5)
                .help(enabled ? "Exposed to the agent" : "Not exposed")
            VStack(alignment: .leading, spacing: Space.xxs) {
                HStack(spacing: Space.sm - 2) {
                    Text(tool.name)
                        .font(.mono(12))
                        .foregroundColor(enabled ? .primary : .secondary)
                    ToolCategoryTag(category: tool.category)
                }
                if enabled, let summary = settingsSummary(tool) {
                    Text(summary).scaledFont(.caption).foregroundColor(.secondary)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, Space.md)
        .padding(.vertical, Space.sm)
        .opacity(enabled ? 1 : 0.6)
    }

    // One-line summary of an enabled tool's settings (e.g. "Statements: Mutations").
    private func settingsSummary(_ tool: AdapterToolDef) -> String? {
        guard let settings = tool.settings, !settings.isEmpty else { return nil }
        let state = conn.toolConfig[tool.name]
        let parts: [String] = settings.compactMap { f in
            let v = state?.settings[f.key] ?? f.defaultValue ?? ""
            if v.isEmpty { return nil }
            let display = f.options?.first(where: { $0.value == v })?.label ?? v
            return "\(f.label): \(display)"
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    private var health: ConnHealth? { store.health[conn.id] }

    // Health, not type: failing when the last check errored, healthy when it
    // passed, unknown when nothing has checked yet — so the chip never falsely
    // implies "connected".
    private var status: ConnStatus {
        guard let health else { return .unknown }
        return health.isError ? .failing : .ok
    }
}

#if DEBUG
#Preview {
    ConnectionDetailView(
        conn: .sample,
        store: .preview,
        onEdit: {},
        onDelete: {},
        onDuplicate: {}
    )
}
#endif
