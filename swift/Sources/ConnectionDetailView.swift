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
    case queries  = "Queries"
    case policy   = "Policy"

    var icon: String {
        switch self {
        case .overview: "link"
        case .queries:  "list.bullet.rectangle"
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
        .background(Color(NSColor.windowBackgroundColor))
    }

    // MARK: - Header

    private var header: some View {
        HStack {
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 7) {
                    Circle()
                        .fill(dotColor)
                        .frame(width: 8, height: 8)
                    Text(conn.name)
                        .font(.system(size: 15, weight: .semibold))
                }
                Text("\(conn.type.label) · \(conn.environment.label)")
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
            }
            Spacer()
            Button("Edit", action: onEdit)
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
                            .font(.system(size: 12, weight: .medium))
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
        .background(Color(NSColor.windowBackgroundColor))
    }

    // MARK: - Tab content

    @ViewBuilder
    private var tabContent: some View {
        switch selectedTab {
        case .overview: overviewTab
        case .queries:  QueriesTab(conn: conn, store: store)
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
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundColor(.primary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer()
                    copyURLButton
                }
            }
            InspectorRow("Agent hint", value: "Use SELECT with LIMIT for production data.")
        }
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
            HStack {
                Text("Client")
                    .font(.system(size: 12))
                    .foregroundColor(.secondary)
                    .frame(width: 86, alignment: .leading)
                Spacer()
                Picker("", selection: $selectedClient) {
                    ForEach(MCPClient.allCases) { client in
                        Text(client.label).tag(client)
                    }
                }
                .pickerStyle(.menu)
                .frame(width: 110)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)

            HStack(alignment: .top, spacing: 8) {
                Text(configSnippet)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundColor(.primary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)

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
            .background(Color(NSColor.textBackgroundColor))
        }
    }

    // MARK: - Connection details

    private var connectionDetailsSection: some View {
        DetailSection("Connection") {
            if conn.type == .sqlite {
                InspectorRow("File", value: conn.filename ?? "-")
            } else {
                InspectorRow("Host", value: conn.host ?? "-")
                InspectorRow("Port", value: conn.port.map(String.init) ?? "-")
                InspectorRow("User", value: conn.user ?? "-")
                InspectorRow("Database", value: conn.database ?? "-")
                InspectorRow("SSH", value: conn.useSSH ? (conn.sshHost ?? "-") : "Off")
                InspectorRow("SSL", value: conn.useSSL ? conn.sslMode.label : "Off")
            }
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
                Text("Testing…").foregroundColor(.secondary)
            }

        case .ok:
            Label("Connected", systemImage: "checkmark.circle.fill")
                .foregroundColor(.green)
                .onAppear { resetTestStatus(after: 3) }

        case .fail(let msg):
            VStack(alignment: .leading, spacing: 4) {
                Label("Failed", systemImage: "xmark.circle.fill")
                    .foregroundColor(.red)
                Text(msg)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundColor(.secondary)
            }
            .onAppear { resetTestStatus(after: 6) }
        }
    }

    private func runTest() {
        testStatus = .testing
        Task {
            do {
                let url = URL(string: "http://localhost:4242/api/connections/\(conn.id)/test")!
                var req = URLRequest(url: url)
                req.httpMethod = "POST"
                req.timeoutInterval = 12
                let (data, _) = try await URLSession.shared.data(for: req)
                let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
                await MainActor.run {
                    testStatus = (json?["ok"] as? Bool == true) ? .ok : .fail(json?["error"] as? String ?? "Unknown error")
                }
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

    private var policyTab: some View {
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
                                .font(.system(size: 12))
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
                            .font(.system(size: 12, design: .monospaced))
                    }
                }
            }
            .padding(18)
        }
    }

    private func guardBadge(blocked: Bool, onLabel: String, offLabel: String) -> some View {
        Text(blocked ? onLabel : offLabel)
            .font(.system(size: 11, weight: .medium))
            .foregroundColor(blocked ? .white : .secondary)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(blocked ? Color.red.opacity(0.75) : Color(NSColor.separatorColor))
            .clipShape(.capsule)
    }

    private var dotColor: Color {
        switch conn.type {
        case .postgres: .green
        case .mysql: .orange
        case .sqlite: .blue
        }
    }
}

// MARK: - Queries tab

private struct QueriesTab: View {
    let conn: Connection
    let store: ConnectionStore

    @State private var entries: [QueryLogEntry] = []
    @State private var filter: VerdictFilter = .all
    @State private var expandedId: Int? = nil
    @State private var isLoading = false

    enum VerdictFilter: String, CaseIterable {
        case all = "All"
        case allowed = "Allowed"
        case blocked = "Blocked"
        case error = "Error"
    }

    private var filtered: [QueryLogEntry] {
        guard filter != .all else { return entries }
        return entries.filter { $0.verdict == filter.rawValue.lowercased() }
    }

    private var stats: (allowed: Int, blocked: Int, error: Int) {
        let a = entries.filter { $0.verdict == "allowed" }.count
        let b = entries.filter { $0.verdict == "blocked" }.count
        let e = entries.filter { $0.verdict == "error" }.count
        return (a, b, e)
    }

    var body: some View {
        VStack(spacing: 0) {
            toolbar
            Divider()
            if isLoading {
                Spacer()
                ProgressView()
                Spacer()
            } else if filtered.isEmpty {
                emptyState
            } else {
                logList
            }
        }
        .onAppear { reload() }
    }

    // MARK: - Toolbar

    private var toolbar: some View {
        HStack(spacing: 12) {
            // Stats pills
            statPill(entries.count, label: "total", color: .secondary)
            statPill(stats.allowed, label: "allowed", color: .green)
            statPill(stats.blocked, label: "blocked", color: .red)
            if stats.error > 0 {
                statPill(stats.error, label: "error", color: .orange)
            }

            Spacer()

            // Filter
            HStack(spacing: 2) {
                ForEach(VerdictFilter.allCases, id: \.self) { f in
                    Button(f.rawValue) { filter = f }
                        .buttonStyle(.plain)
                        .font(.system(size: 11, weight: filter == f ? .semibold : .regular))
                        .foregroundColor(filter == f ? .accentColor : .secondary)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(filter == f ? Color.accentColor.opacity(0.1) : .clear)
                        .clipShape(.capsule)
                }
            }

            Button {
                reload()
            } label: {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 11))
            }
            .buttonStyle(.plain)
            .foregroundColor(.secondary)
            .help("Refresh")
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 9)
    }

    private func statPill(_ count: Int, label: String, color: Color) -> some View {
        HStack(spacing: 3) {
            Text("\(count)")
                .font(.system(size: 11, weight: .semibold, design: .monospaced))
                .foregroundColor(color == .secondary ? .primary : color)
            Text(label)
                .font(.system(size: 11))
                .foregroundColor(.secondary)
        }
    }

    // MARK: - Empty state

    private var emptyState: some View {
        VStack(spacing: 8) {
            Image(systemName: "list.bullet.rectangle")
                .font(.system(size: 28))
                .foregroundColor(.secondary.opacity(0.4))
            Text(filter == .all ? "No queries yet" : "No \(filter.rawValue.lowercased()) queries")
                .font(.system(size: 13))
                .foregroundColor(.secondary)
            Text("Queries run by agents through this connection will appear here.")
                .font(.system(size: 11))
                .foregroundColor(.secondary.opacity(0.7))
                .multilineTextAlignment(.center)
                .frame(maxWidth: 280)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    // MARK: - Log list

    private var logList: some View {
        ScrollView {
            LazyVStack(spacing: 0, pinnedViews: []) {
                ForEach(filtered) { entry in
                    LogEntryRow(
                        entry: entry,
                        isExpanded: expandedId == entry.id,
                        onToggle: {
                            expandedId = expandedId == entry.id ? nil : entry.id
                        }
                    )
                    Divider().padding(.leading, 14)
                }
            }
        }
    }

    private func reload() {
        isLoading = true
        entries = store.recentLog(connectionId: conn.id, limit: 200)
        isLoading = false
    }
}

// MARK: - Log entry row

private struct LogEntryRow: View {
    let entry: QueryLogEntry
    let isExpanded: Bool
    let onToggle: () -> Void

    var body: some View {
        Button(action: onToggle) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(alignment: .top, spacing: 10) {
                    // Verdict indicator bar
                    RoundedRectangle(cornerRadius: 2)
                        .fill(verdictColor)
                        .frame(width: 3)
                        .frame(minHeight: 36)

                    VStack(alignment: .leading, spacing: 4) {
                        // Top row: badge + SQL preview
                        HStack(spacing: 8) {
                            VerdictBadge(verdict: entry.verdict)

                            if let cats = entry.categories, !cats.isEmpty {
                                Text(cats)
                                    .font(.system(size: 10, design: .monospaced))
                                    .foregroundColor(.secondary)
                                    .lineLimit(1)
                            }

                            Spacer()

                            Text(relativeTime(entry.createdAt))
                                .font(.system(size: 10))
                                .foregroundColor(.secondary)
                                .lineLimit(1)
                        }

                        // SQL
                        Text(entry.sql)
                            .font(.system(size: 11.5, design: .monospaced))
                            .foregroundColor(.primary)
                            .lineLimit(isExpanded ? nil : 1)
                            .truncationMode(.tail)
                            .frame(maxWidth: .infinity, alignment: .leading)

                        // Expanded: reason + full timestamp
                        if isExpanded {
                            if let reason = entry.reason, !reason.isEmpty {
                                HStack(spacing: 4) {
                                    Image(systemName: "exclamationmark.circle.fill")
                                        .font(.system(size: 10))
                                        .foregroundColor(verdictColor)
                                    Text(reason)
                                        .font(.system(size: 11))
                                        .foregroundColor(.secondary)
                                }
                                .padding(.top, 2)
                            }
                            Text(entry.createdAt)
                                .font(.system(size: 10))
                                .foregroundColor(.secondary.opacity(0.7))
                                .padding(.top, 1)
                        }
                    }
                    .padding(.vertical, 10)
                    .padding(.trailing, 14)
                }
            }
        }
        .buttonStyle(.plain)
        .background(isExpanded ? Color.accentColor.opacity(0.04) : .clear)
        .contentShape(Rectangle())
        .animation(.easeInOut(duration: 0.12), value: isExpanded)
    }

    private var verdictColor: Color {
        switch entry.verdict {
        case "allowed": return .green
        case "blocked": return .red
        default:        return .orange
        }
    }

    // "2 min ago" / "just now" / falls back to raw string for older entries
    private func relativeTime(_ raw: String) -> String {
        let fmt = DateFormatter()
        fmt.dateFormat = "yyyy-MM-dd HH:mm:ss"
        fmt.locale = Locale(identifier: "en_US_POSIX")
        guard let date = fmt.date(from: raw) else { return raw }
        let secs = Int(-date.timeIntervalSinceNow)
        if secs < 10  { return "just now" }
        if secs < 60  { return "\(secs)s ago" }
        if secs < 3600 { return "\(secs / 60)m ago" }
        if secs < 86400 { return "\(secs / 3600)h ago" }
        return "\(secs / 86400)d ago"
    }
}

// MARK: - Verdict badge

private struct VerdictBadge: View {
    let verdict: String

    var body: some View {
        Text(label)
            .font(.system(size: 9, weight: .bold))
            .foregroundColor(.white)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(color)
            .clipShape(.capsule)
    }

    private var label: String {
        switch verdict {
        case "allowed": return "OK"
        case "blocked": return "BLOCKED"
        default:        return "ERROR"
        }
    }

    private var color: Color {
        switch verdict {
        case "allowed": return .green
        case "blocked": return .red
        default:        return .orange
        }
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
                        .font(.system(size: 10, weight: .medium))
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

// MARK: - Shared helpers

extension View {
    /// Liquid Glass card surface on macOS 26+, with a solid fallback for earlier systems.
    @ViewBuilder
    func cardSurface(cornerRadius: CGFloat = 8) -> some View {
        if #available(macOS 26.0, *) {
            self.glassEffect(.regular, in: .rect(cornerRadius: cornerRadius))
        } else {
            self
                .background(Color(NSColor.controlBackgroundColor))
                .clipShape(.rect(cornerRadius: cornerRadius))
                .overlay(
                    RoundedRectangle(cornerRadius: cornerRadius)
                        .stroke(Color(NSColor.separatorColor), lineWidth: 0.5)
                )
        }
    }
}

struct SectionLabel: View {
    let title: String
    init(_ title: String) { self.title = title }
    var body: some View {
        Text(title)
            .font(.system(size: 11, weight: .semibold))
            .foregroundColor(.secondary)
            .textCase(.uppercase)
            .tracking(0.5)
    }
}

struct DetailSection<Content: View>: View {
    let title: String
    let content: Content

    init(_ title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(title)
                .font(.system(size: 11, weight: .semibold))
                .foregroundColor(.secondary)
                .textCase(.uppercase)
                .padding(.bottom, 6)
            VStack(spacing: 0) {
                content
            }
            .cardSurface()
        }
    }
}

struct InspectorRow<Content: View>: View {
    let label: String
    let content: Content

    init(_ label: String, value: String) where Content == Text {
        self.label = label
        self.content = Text(value)
            .font(.system(size: 12, design: .monospaced))
            .foregroundColor(.primary)
    }

    init(_ label: String, @ViewBuilder content: () -> Content) {
        self.label = label
        self.content = content()
    }

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            Text(label)
                .font(.system(size: 12))
                .foregroundColor(.secondary)
                .frame(width: 86, alignment: .leading)
            content
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .overlay(alignment: .bottom) {
            Divider().padding(.leading, 106)
        }
    }
}
