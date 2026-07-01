import AppKit
import SwiftUI

// MARK: - MCP client config snippets

enum MCPClient: String, CaseIterable, Identifiable {
    case opencode, claude, cursor, windsurf

    var id: String { rawValue }

    var label: String {
        switch self {
        case .opencode: "opencode"
        case .claude: "Claude"
        case .cursor: "Cursor"
        case .windsurf: "Windsurf"
        }
    }

    var configPath: String {
        switch self {
        case .opencode: "~/.opencode/opencode.jsonc"
        case .claude: "~/Library/Application Support/Claude/claude_desktop_config.json"
        case .cursor: "~/.cursor/mcp.json"
        case .windsurf: "~/.codeium/windsurf/mcp_config.json"
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
        case .claude:
            // Claude Desktop wraps remote servers via mcp-remote (no native HTTP yet).
            // --allow-http is required for non-HTTPS (localhost) URLs.
            return """
            {
              "mcpServers": {
                "\(key)": {
                  "command": "bunx",
                  "args": ["mcp-remote", "\(url)", "--allow-http"]
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
        case .windsurf:
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

// MARK: - Detail tabs

private enum DetailTab: String, CaseIterable {
    case overview = "Overview"
    case logs     = "Logs"
    case policy   = "Tools"

    var icon: String {
        switch self {
        case .overview: "link"
        case .logs:     "list.bullet.rectangle"
        case .policy:   "wrench.and.screwdriver"
        }
    }
}

// MARK: - Detail view

struct ConnectionDetailView: View {
    let conn: Connection
    let store: ConnectionStore
    let onEdit: () -> Void
    let onDelete: () -> Void
    let onDuplicate: () -> Void

    @State private var selectedTab: DetailTab = .overview
    @State private var urlCopied = false
    @State private var snippetCopied = false
    @State private var selectedClient: MCPClient = .opencode
    @State private var testStatus: TestStatus = .idle

    enum TestStatus { case idle, testing, ok, fail(String) }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            tabBar
            Divider()
            tabContent
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background(.clear)
    }

    // MARK: - Header

    private var header: some View {
        HStack(spacing: 11) {
            TypeBadge(type: conn.type, size: 32)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 7) {
                    Circle()
                        .fill(dotColor)
                        .frame(width: 8, height: 8)
                        .help(health?.isError == true ? (health?.error ?? "Connection failing") : "")
                        .accessibilityHidden(true) // color re-encodes the status shown as text below
                    Text(conn.name)
                        .font(.system(size: 15, weight: .semibold))
                }
                if health?.isError == true {
                    Text("Connection issue")
                        .font(.dev(size: 11))
                        .foregroundColor(.red)
                        .help(health?.error ?? "Connection failing")
                } else {
                    Text("\(conn.typeLabel) · \(conn.environment.label)")
                        .font(.dev(size: 11))
                        .foregroundColor(.secondary)
                }
            }
            Spacer()
            headerTestButton
            Button("Edit", action: onEdit)
                .buttonStyle(.bordered)
                .controlSize(.small)
            Button("Duplicate", action: onDuplicate)
                .buttonStyle(.bordered)
                .controlSize(.small)
            Button(role: .destructive, action: onDelete) {
                Label("Delete", systemImage: "trash")
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
    }

    // MARK: - Tab bar

    private var tabBar: some View {
        HStack(spacing: 0) {
            ForEach(DetailTab.allCases, id: \.self) { tab in
                Button {
                    selectedTab = tab
                } label: {
                    HStack(spacing: 5) {
                        Image(systemName: tab.icon)
                            .font(.system(size: 11))
                        Text(tab.rawValue)
                            .font(.dev(size: 12, weight: .medium))
                    }
                    .foregroundColor(selectedTab == tab ? .accentColor : .secondary)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
                    .overlay(alignment: .bottom) {
                        if selectedTab == tab {
                            Rectangle()
                                .fill(Color.accentColor)
                                .frame(height: 2)
                        }
                    }
                }
                .buttonStyle(.plain)
            }
            Spacer()
        }
        .padding(.horizontal, 4)
        .background(.clear)
    }

    // MARK: - Tab content

    @ViewBuilder
    private var tabContent: some View {
        switch selectedTab {
        case .overview: overviewTab
        case .logs:     LogsTab(scope: .connection(conn), store: store)
        case .policy:   policyTab
        }
    }

    // MARK: - Overview tab

    private var overviewTab: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                mcpURLSection
                configSnippetSection
                connectionDetailsSection
            }
            .padding(18)
        }
    }

    // MARK: - MCP URL

    private var mcpURLSection: some View {
        DetailSection("MCP") {
            InspectorRow("URL") {
                HStack(spacing: 8) {
                    Text(conn.mcpURL)
                        .font(.dev(size: 12))
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
        .controlSize(.regular)
        .tint(urlCopied ? .green : .accentColor)
        .animation(.easeInOut(duration: 0.15), value: urlCopied)
    }

    // MARK: - Config snippet

    private var configSnippet: String {
        selectedClient.snippet(key: conn.mcpKey, url: conn.mcpURL)
    }

    private var configSnippetSection: some View {
        DetailSection("Config") {
            HStack(spacing: 8) {
                Text("Client")
                    .font(.dev(size: 11, weight: .semibold))
                    .foregroundColor(.secondary)
                    .textCase(.uppercase)
                Picker("", selection: $selectedClient) {
                    ForEach(MCPClient.allCases) { client in
                        Text(client.label).tag(client)
                    }
                }
                .pickerStyle(.menu)
                .fixedSize()
                Spacer()
                Button(snippetCopied ? "Copied!" : "Copy") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(configSnippet, forType: .string)
                    snippetCopied = true
                    Task { @MainActor in
                        try? await Task.sleep(for: .seconds(1.5))
                        snippetCopied = false
                    }
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .tint(snippetCopied ? .green : nil)
                .animation(.easeInOut(duration: 0.15), value: snippetCopied)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)

            Divider()

            Text(configSnippet)
                .font(.dev(size: 11))
                .foregroundColor(.primary)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(12)
        }
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
        HStack(spacing: 6) {
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
            VStack(alignment: .leading, spacing: 18) {
                if tools.isEmpty {
                    DetailSection("Tools") {
                        Text("Tool list unavailable — the local pluk server isn't responding.")
                            .font(.system(size: 11))
                            .foregroundColor(.secondary)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 8)
                    }
                } else {
                    DetailSection("Tools") {
                        InspectorRow("Enabled", value: "\(tools.filter(isEnabled).count) of \(tools.count)")
                    }
                    DetailSection("Exposed to the agent") {
                        ForEach(tools) { tool in toolStatusRow(tool) }
                    }
                }
            }
            .padding(18)
        }
    }

    @ViewBuilder
    private func toolStatusRow(_ tool: AdapterToolDef) -> some View {
        let enabled = isEnabled(tool)
        HStack(alignment: .top, spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(tool.name)
                        .font(.dev(size: 12))
                        .foregroundColor(enabled ? .primary : .secondary)
                    ToolCategoryTag(category: tool.category)
                }
                if enabled, let summary = settingsSummary(tool) {
                    Text(summary).font(.system(size: 10)).foregroundColor(.secondary)
                }
            }
            Spacer(minLength: 0)
            Text(enabled ? "On" : "Off")
                .font(.dev(size: 11, weight: .medium))
                .foregroundColor(enabled ? .white : .secondary)
                .padding(.horizontal, 8)
                .padding(.vertical, 3)
                .background(enabled ? Color.green.opacity(0.7) : Color(NSColor.separatorColor))
                .clipShape(.capsule)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .overlay(alignment: .bottom) { Divider().padding(.leading, 10) }
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

    // Health, not type: red when failing, green when known-good, gray when
    // untested this session — so the dot never falsely implies "connected".
    private var dotColor: Color {
        guard let health else { return .gray }
        return health.isError ? .red : .green
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
