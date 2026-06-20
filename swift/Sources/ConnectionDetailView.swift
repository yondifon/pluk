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
                    Text(conn.name)
                        .font(.system(size: 15, weight: .semibold))
                }
                Text("\(conn.typeLabel) · \(conn.environment.label)")
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
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
        .background(.clear)
    }

    // MARK: - Tab content

    @ViewBuilder
    private var tabContent: some View {
        switch selectedTab {
        case .overview: overviewTab
        case .logs:     LogsTab(conn: conn, store: store)
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

            Text(configSnippet)
                .font(.system(size: 11, design: .monospaced))
                .foregroundColor(.primary)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(10)
                .codeBlockSurface()
                .overlay(alignment: .topTrailing) {
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
                    .padding(8)
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
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
                let url = URL(string: "http://localhost:4242/api/integrations/\(conn.id)/test")!
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

    // Action-policy adapters (Linear, …) don't have SQL statements — show their
    // read/write permissions instead of statement categories + SQL guards.
    private var isActionPolicy: Bool {
        if let kind = store.adapters.first(where: { $0.id == conn.type })?.policyKind {
            return kind == "action"
        }
        return conn.connectionType == nil
    }

    @ViewBuilder
    private var policyTab: some View {
        if isActionPolicy { actionPolicyTab } else { sqlPolicyTab }
    }

    private var actionPolicyTab: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                DetailSection("Permissions") {
                    InspectorRow("Mode", value: conn.readOnly ? "Read-only" : "Read & write")
                    InspectorRow("Description", value: conn.readOnly
                                 ? "Agent can only read."
                                 : "Agent can read and create/modify.")
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
            .font(.system(size: 11, weight: .medium))
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
        switch conn.connectionType {
        case .postgres: .green
        case .mysql: .orange
        case .sqlite: .blue
        case nil: .gray
        }
    }
}

// MARK: - Logs tab

private struct LogsTab: View {
    let conn: Connection
    let store: ConnectionStore

    @State private var entries: [QueryLogEntry] = []
    @State private var filter: VerdictFilter = .all
    @State private var expandedId: Int? = nil
    @State private var showRetentionPicker = false
    @State private var pollTimer: Timer? = nil

    enum VerdictFilter: String, CaseIterable {
        case all = "All"
        case allowed = "Allowed"
        case blocked = "Blocked"
        case error = "Error"
    }

    private var hasPending: Bool { entries.contains { $0.verdict == "pending" } }

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
            if filtered.isEmpty {
                emptyState
            } else {
                logList
            }
        }
        .onAppear {
            reload()
            startPollingIfNeeded()
        }
        .onDisappear {
            stopPolling()
        }
        .onChange(of: hasPending) { _, pending in
            pending ? startPollingIfNeeded() : stopPolling()
        }
    }

    private func startPollingIfNeeded() {
        guard hasPending, pollTimer == nil else { return }
        pollTimer = Timer.scheduledTimer(withTimeInterval: 1.5, repeats: true) { _ in
            reload()
        }
    }

    private func stopPolling() {
        pollTimer?.invalidate()
        pollTimer = nil
    }

    // MARK: - Toolbar

    private var toolbar: some View {
        HStack(spacing: 10) {
            // Stats pills
            statPill(entries.count, label: "total", color: .secondary)
            statPill(stats.allowed, label: "ok", color: .green)
            if stats.blocked > 0 {
                statPill(stats.blocked, label: "blocked", color: .red)
            }
            if stats.error > 0 {
                statPill(stats.error, label: "err", color: .orange)
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

            Divider().frame(height: 14)

            // Retention
            Menu {
                let options = [7, 14, 30, 60, 90, 0]
                ForEach(options, id: \.self) { days in
                    Button(days == 0 ? "Keep forever" : "Keep \(days) days") {
                        store.logRetentionDays = days
                        store.purgeOldLogs()
                        reload()
                    }
                }
                Divider()
                Button("Clear all logs for this integration", role: .destructive) {
                    store.clearAllLogs(connectionId: conn.id)
                    reload()
                }
            } label: {
                HStack(spacing: 3) {
                    Image(systemName: "clock.arrow.circlepath")
                        .font(.system(size: 10))
                    let days = store.logRetentionDays
                    Text(days == 0 ? "Forever" : "\(days)d")
                        .font(.system(size: 11))
                }
                .foregroundColor(.secondary)
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
            .help("Log retention — how long to keep activity history")

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
        .padding(.horizontal, 18)
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
            Text(filter == .all ? "No activity yet" : "No \(filter.rawValue.lowercased()) activity")
                .font(.system(size: 13))
                .foregroundColor(.secondary)
            Text("Activity from agents using this integration will appear here.")
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
                    let expanded = expandedId == entry.id
                    LogEntryRow(
                        entry: entry,
                        isExpanded: expanded,
                        onToggle: { expandedId = expanded ? nil : entry.id },
                        onStop: { stopQuery(entry) }
                    )
                    Divider().padding(.leading, 18)
                }
            }
        }
    }

    private func reload() {
        entries = store.recentLog(connectionId: conn.id)
    }

    private func stopQuery(_ entry: QueryLogEntry) {
        Task {
            let url = URL(string: "http://localhost:4242/api/log/\(entry.id)/cancel")!
            var req = URLRequest(url: url)
            req.httpMethod = "POST"
            req.timeoutInterval = 5
            _ = try? await URLSession.shared.data(for: req)
            await MainActor.run { reload() }
        }
    }
}

// MARK: - Log entry row

private struct LogEntryRow: View {
    let entry: QueryLogEntry
    let isExpanded: Bool
    let onToggle: () -> Void
    let onStop: () -> Void

    @State private var copiedSQL = false
    @State private var copiedResult = false

    // The agent-visible response: result rows when present, else the verdict reason.
    private var responseText: String? {
        if let json = entry.resultJson, !json.isEmpty { return json }
        if let reason = entry.reason, !reason.isEmpty { return reason }
        return nil
    }

    private func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

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

                            if entry.verdict == "pending" {
                                Button(action: onStop) {
                                    Label("Stop", systemImage: "stop.fill")
                                        .font(.system(size: 10, weight: .medium))
                                }
                                .buttonStyle(.plain)
                                .foregroundColor(.red)
                                .help("Cancel this running query")
                            }

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

                        // Expanded: reason + result preview + full timestamp
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

                            // Result preview mini-table
                            if let json = entry.resultJson {
                                ResultPreview(json: json, rowCount: entry.rowCount)
                                    .padding(.top, 6)
                            }

                            Text(localTime(entry.createdAt))
                                .font(.system(size: 10))
                                .foregroundColor(.secondary.opacity(0.7))
                                .padding(.top, 1)

                            // Copy actions for the query and its response
                            HStack(spacing: 6) {
                                copyButton(copiedSQL ? "Copied!" : "Copy", copied: copiedSQL) {
                                    copy(entry.sql)
                                    flash($copiedSQL)
                                }
                                if let response = responseText {
                                    copyButton(copiedResult ? "Copied!" : "Copy response", copied: copiedResult) {
                                        copy(response)
                                        flash($copiedResult)
                                    }
                                }
                            }
                            .padding(.top, 6)
                        }
                    }
                    .padding(.vertical, 10)
                    .padding(.trailing, 18)
                }
                .padding(.leading, 18)
            }
        }
        .buttonStyle(.plain)
        .background(isExpanded ? Color.accentColor.opacity(0.04) : .clear)
        .contentShape(Rectangle())
        .animation(.easeInOut(duration: 0.12), value: isExpanded)
    }

    private func copyButton(_ title: String, copied: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(title, systemImage: copied ? "checkmark" : "doc.on.doc")
                .font(.system(size: 10, weight: .medium))
        }
        .buttonStyle(.bordered)
        .controlSize(.mini)
        .tint(copied ? .green : nil)
    }

    private func flash(_ flag: Binding<Bool>) {
        flag.wrappedValue = true
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(1.5))
            flag.wrappedValue = false
        }
    }

    private var verdictColor: Color {
        switch entry.verdict {
        case "allowed":   return .green
        case "blocked":   return .red
        case "cancelled": return Color(nsColor: .systemPurple)
        case "pending":   return .secondary
        default:          return .orange
        }
    }

    // "2 min ago" / "just now" / falls back to raw string for older entries
    private func relativeTime(_ raw: String) -> String {
        let fmt = DateFormatter()
        fmt.dateFormat = "yyyy-MM-dd HH:mm:ss"
        fmt.locale = Locale(identifier: "en_US_POSIX")
        fmt.timeZone = TimeZone(identifier: "UTC")  // SQLite datetime('now') is UTC
        guard let date = fmt.date(from: raw) else { return raw }
        let secs = Int(-date.timeIntervalSinceNow)
        if secs < 10  { return "just now" }
        if secs < 60  { return "\(secs)s ago" }
        if secs < 3600 { return "\(secs / 60)m ago" }
        if secs < 86400 { return "\(secs / 3600)h ago" }
        return "\(secs / 86400)d ago"
    }

    // Full UTC timestamp -> local time string
    private func localTime(_ raw: String) -> String {
        let inFmt = DateFormatter()
        inFmt.dateFormat = "yyyy-MM-dd HH:mm:ss"
        inFmt.locale = Locale(identifier: "en_US_POSIX")
        inFmt.timeZone = TimeZone(identifier: "UTC")
        guard let date = inFmt.date(from: raw) else { return raw }
        let outFmt = DateFormatter()
        outFmt.dateFormat = "yyyy-MM-dd HH:mm:ss"
        outFmt.locale = Locale(identifier: "en_US_POSIX")
        outFmt.timeZone = .current
        return outFmt.string(from: date)
    }
}

// MARK: - Verdict badge

private struct VerdictBadge: View {
    let verdict: String

    var body: some View {
        if verdict == "pending" {
            HStack(spacing: 4) {
                ProgressView().scaleEffect(0.55).frame(width: 10, height: 10)
                Text("RUNNING")
                    .font(.system(size: 9, weight: .bold))
                    .foregroundColor(.secondary)
            }
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Color.secondary.opacity(0.1))
            .clipShape(.capsule)
        } else {
            Text(label)
                .font(.system(size: 9, weight: .bold))
                .foregroundColor(.white)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(color)
                .clipShape(.capsule)
        }
    }

    private var label: String {
        switch verdict {
        case "allowed":   return "OK"
        case "blocked":   return "BLOCKED"
        case "cancelled": return "CANCELLED"
        default:          return "ERROR"
        }
    }

    private var color: Color {
        switch verdict {
        case "allowed":   return .green
        case "blocked":   return .red
        case "cancelled": return Color(nsColor: .systemPurple)
        default:          return .orange
        }
    }
}

// MARK: - Result preview (mini-table for expanded log entries)

private struct ResultPreview: View {
    let json: String
    let rowCount: Int?

    private struct ParsedResult {
        let fields: [String]
        let rows: [[String]]
    }

    private var parsed: ParsedResult? {
        guard let data = json.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let fields = obj["fields"] as? [String],
              let rows = obj["rows"] as? [[String: Any]] else { return nil }
        let rowStrings = rows.prefix(5).map { row in
            fields.map { key in
                guard let val = row[key] else { return "NULL" }
                if val is NSNull { return "NULL" }
                return "\(val)"
            }
        }
        return ParsedResult(fields: fields, rows: rowStrings)
    }

    var body: some View {
        if let p = parsed, !p.fields.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                // Header row
                HStack(spacing: 0) {
                    ForEach(p.fields.prefix(6), id: \.self) { field in
                        Text(field)
                            .font(.system(size: 9.5, weight: .semibold, design: .monospaced))
                            .foregroundColor(.secondary)
                            .lineLimit(1)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 3)
                            .background(Color.secondary.opacity(0.08))
                    }
                }
                .clipShape(RoundedRectangle(cornerRadius: 4, style: .continuous).path(in: CGRect(x: 0, y: 0, width: 9999, height: 999)))

                // Data rows
                ForEach(Array(p.rows.enumerated()), id: \.offset) { _, row in
                    HStack(spacing: 0) {
                        ForEach(Array(row.prefix(6).enumerated()), id: \.offset) { _, cell in
                            Text(cell)
                                .font(.system(size: 9.5, design: .monospaced))
                                .foregroundColor(.primary.opacity(0.75))
                                .lineLimit(1)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.horizontal, 6)
                                .padding(.vertical, 2)
                        }
                    }
                    Divider().opacity(0.5)
                }

                // Footer: row counts
                let total = rowCount ?? p.rows.count
                let showing = min(p.rows.count, 5)
                if total > showing {
                    Text("\(showing) of \(total) rows")
                        .font(.system(size: 9.5))
                        .foregroundColor(.secondary)
                        .padding(.horizontal, 6)
                        .padding(.top, 3)
                }
            }
            .codeBlockSurface(cornerRadius: 5)
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
            // Frosted-glass approximation: material over the window's vibrancy
            // plus a hairline edge for the glass-rim highlight.
            self
                .background(.ultraThinMaterial, in: .rect(cornerRadius: cornerRadius))
                .overlay(
                    RoundedRectangle(cornerRadius: cornerRadius)
                        .stroke(Color.primary.opacity(0.06), lineWidth: 0.5)
                )
        }
    }

    /// Inset surface for code / data blocks (config snippets, result tables) —
    /// a subtle translucent fill + hairline so they read as a distinct block over
    /// the card without the opaque slab a solid window color would paint.
    func codeBlockSurface(cornerRadius: CGFloat = 6) -> some View {
        self
            .background(
                Color.secondary.opacity(0.05),
                in: RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
            )
            .overlay(
                RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                    .stroke(Color.secondary.opacity(0.15), lineWidth: 1)
            )
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
