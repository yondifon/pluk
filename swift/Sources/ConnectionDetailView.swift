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
    case policy   = "Policy"

    var icon: String {
        switch self {
        case .overview: "link"
        case .logs:     "list.bullet.rectangle"
        case .policy:   "shield"
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
                testSection
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

    // MARK: - Test

    private var testSection: some View {
        DetailSection("Status") {
            InspectorRow("Connection") {
                testButton
            }
        }
    }

    @ViewBuilder
    private var testButton: some View {
        switch testStatus {
        case .idle:
            Button("Test connection") { runTest() }
                .buttonStyle(.bordered)

        case .testing:
            HStack(spacing: 6) {
                ProgressView().scaleEffect(0.7)
                Text("Testing…")
                    .font(.dev(size: 12))
                    .foregroundColor(.secondary)
            }

        case .ok:
            Label("Connected", systemImage: "checkmark.circle.fill")
                .font(.dev(size: 12, weight: .medium))
                .foregroundColor(.green)
                .onAppear { resetTestStatus(after: 3) }

        case .fail(let msg):
            VStack(alignment: .leading, spacing: 4) {
                Label("Failed", systemImage: "xmark.circle.fill")
                    .font(.dev(size: 12, weight: .medium))
                    .foregroundColor(.red)
                Text(msg)
                    .font(.dev(size: 11))
                    .foregroundColor(.secondary)
            }
            .onAppear { resetTestStatus(after: 6) }
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
                await MainActor.run {
                    testStatus = (json?["ok"] as? Bool == true) ? .ok : .fail(json?["error"] as? String ?? "Unknown error")
                }
                // The test wrote health server-side; pull it so the dot updates now.
                await store.refreshHealth()
            } catch {
                await MainActor.run { testStatus = .fail(error.localizedDescription) }
            }
        }
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
    private var policyKind: String {
        if let kind = store.adapters.first(where: { $0.id == conn.type })?.policyKind {
            return kind
        }
        return conn.connectionType == nil ? "action" : "sql"
    }

    @ViewBuilder
    private var policyTab: some View {
        switch policyKind {
        case "action": actionPolicyTab
        case "none":   confirmPolicyTab
        default:       sqlPolicyTab
        }
    }

    private var confirmPolicyTab: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                DetailSection("Confirmation") {
                    InspectorRow("Commands", value: "Unrestricted")
                    InspectorRow("Gate", value: "Confirmed per command")
                }
                Text("Commands run unrestricted as the connecting user. There is no allowlist or read/write policy — every command must be confirmed in your agent client before it runs.")
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 2)
            }
            .padding(18)
        }
    }

    private var actionPolicyTab: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                DetailSection("Permissions") {
                    InspectorRow("Mode", value: conn.readOnly ? "Read-only" : "Read & write")
                    InspectorRow("Description", value: conn.readOnly
                                 ? "Agent can only read."
                                 : "Agent can read and modify data.")
                }
                DetailSection("Allowed Actions") {
                    InspectorRow("Read") { actionBadge(allowed: true) }
                    InspectorRow("Write") { actionBadge(allowed: !conn.readOnly) }
                }
            }
            .padding(18)
        }
    }

    private func actionBadge(allowed: Bool) -> some View {
        Text(allowed ? "Allowed" : "Blocked")
            .font(.dev(size: 11, weight: .medium))
            .foregroundColor(allowed ? .white : .secondary)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(allowed ? Color.green.opacity(0.7) : Color(NSColor.separatorColor))
            .clipShape(.capsule)
    }

    private var sqlPolicyTab: some View {
        let policy = conn.queryPolicy
        return ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                DetailSection("Preset") {
                    InspectorRow("Name", value: policy.preset.label)
                    InspectorRow("Description", value: policy.preset.description)
                }

                DetailSection("Allowed Statement Types") {
                    let groups: [(String, [StatementCategory])] = [
                        ("Read",   [.select, .inspect]),
                        ("Write",  [.insert, .update, .delete, .merge]),
                        ("Schema", [.create, .alter, .drop, .truncate, .rename]),
                        ("Admin",  [.transaction, .session, .procedure, .maintenance, .grant]),
                    ]
                    ForEach(groups, id: \.0) { groupName, cats in
                        HStack(alignment: .top) {
                            Text(groupName)
                                .font(.dev(size: 12))
                                .foregroundColor(.secondary)
                                .frame(width: 86, alignment: .leading)
                            FlowRow(cats.map { cat in
                                (cat.label, policy.allowed.contains(cat))
                            })
                            Spacer(minLength: 0)
                        }
                        .padding(.horizontal, 10)
                        .padding(.vertical, 7)
                        .overlay(alignment: .bottom) {
                            Divider().padding(.leading, 106)
                        }
                    }
                }

                DetailSection("Guards") {
                    InspectorRow("Stacked statements") {
                        guardBadge(blocked: policy.blockStacked, onLabel: "Blocked", offLabel: "Allowed")
                    }
                    InspectorRow("WHERE on mutations") {
                        guardBadge(blocked: policy.requireWhere, onLabel: "Required", offLabel: "Optional")
                    }
                    InspectorRow("Filesystem / COPY") {
                        guardBadge(blocked: !policy.allowFilesystem, onLabel: "Blocked", offLabel: "Allowed")
                    }
                    InspectorRow("Max rows") {
                        Text(policy.maxRows.map { "\($0)" } ?? "Unlimited")
                            .font(.dev(size: 12))
                    }
                }
            }
            .padding(18)
        }
    }

    private func guardBadge(blocked: Bool, onLabel: String, offLabel: String) -> some View {
        Text(blocked ? onLabel : offLabel)
            .font(.dev(size: 11, weight: .medium))
            .foregroundColor(blocked ? .white : .secondary)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(blocked ? Color.red.opacity(0.75) : Color(NSColor.separatorColor))
            .clipShape(.capsule)
    }

    private var health: ConnHealth? { store.health[conn.id] }

    // Health, not type: red when failing, green when known-good, gray when
    // untested this session — so the dot never falsely implies "connected".
    private var dotColor: Color {
        guard let health else { return .gray }
        return health.isError ? .red : .green
    }
}

// MARK: - Flow row (wrapping category chips)

private struct FlowRow: View {
    let items: [(label: String, active: Bool)]

    init(_ items: [(String, Bool)]) {
        self.items = items.map { (label: $0.0, active: $0.1) }
    }

    var body: some View {
        // Simple wrap using a fixed-width approach for macOS
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 4) {
                ForEach(items, id: \.label) { item in
                    Text(item.label)
                        .font(.dev(size: 10, weight: .medium))
                        .foregroundColor(item.active ? .white : .secondary)
                        .padding(.horizontal, 7)
                        .padding(.vertical, 3)
                        .background(item.active ? Color.accentColor.opacity(0.8) : Color(NSColor.separatorColor).opacity(0.5))
                        .clipShape(.capsule)
                        .opacity(item.active ? 1 : 0.6)
                }
            }
        }
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

