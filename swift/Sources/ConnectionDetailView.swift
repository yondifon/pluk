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

// MARK: - Detail view

struct ConnectionDetailView: View {
    let conn: Connection
    let store: ConnectionStore
    let onEdit: () -> Void
    let onDelete: () -> Void

    @State private var urlCopied = false
    @State private var snippetCopied = false
    @State private var selectedClient: MCPClient = .opencode
    @State private var testStatus: TestStatus = .idle
    @State private var showLog = false
    @State private var logEntries: [QueryLogEntry] = []

    enum TestStatus { case idle, testing, ok, fail(String) }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header
                Divider()
                VStack(alignment: .leading, spacing: 18) {
                    mcpURLSection
                    configSnippetSection
                    connectionDetailsSection
                    policySection
                    testSection
                    queryLogSection
                }
                .padding(18)
            }
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

    // MARK: - Policy summary

    private var policySection: some View {
        let policy = conn.queryPolicy
        return DetailSection("Query Policy") {
            InspectorRow("Preset", value: policy.preset.label)
            InspectorRow("Allowed") {
                Text(policy.allowed.map(\.rawValue).joined(separator: ", "))
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundColor(.secondary)
                    .lineLimit(2)
            }
            InspectorRow("Guards") {
                HStack(spacing: 6) {
                    if policy.blockStacked {
                        badge("no stack", color: .blue)
                    }
                    if policy.requireWhere {
                        badge("WHERE req.", color: .orange)
                    }
                    if !policy.allowFilesystem {
                        badge("no FS", color: .purple)
                    }
                    if let max = policy.maxRows {
                        badge("≤\(max) rows", color: .gray)
                    }
                }
            }
        }
    }

    private func badge(_ text: String, color: Color) -> some View {
        Text(text)
            .font(.system(size: 10, weight: .medium))
            .foregroundColor(.white)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(color.opacity(0.8))
            .clipShape(.capsule)
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

    // MARK: - Query log

    private var queryLogSection: some View {
        DetailSection("Query Log") {
            VStack(alignment: .leading, spacing: 0) {
                HStack {
                    Text("Recent agent queries and verdicts")
                        .font(.system(size: 12))
                        .foregroundColor(.secondary)
                    Spacer()
                    Button(showLog ? "Hide" : "Show") {
                        showLog.toggle()
                        if showLog { logEntries = store.recentLog(connectionId: conn.id) }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 8)

                if showLog {
                    Divider()
                    if logEntries.isEmpty {
                        Text("No queries logged yet.")
                            .font(.system(size: 12))
                            .foregroundColor(.secondary)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 8)
                    } else {
                        ForEach(logEntries) { entry in
                            QueryLogRow(entry: entry)
                            Divider().padding(.leading, 10)
                        }
                    }
                    Button("Refresh") {
                        logEntries = store.recentLog(connectionId: conn.id)
                    }
                    .buttonStyle(.plain)
                    .font(.system(size: 11))
                    .foregroundColor(.accentColor)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                }
            }
        }
    }

    private var dotColor: Color {
        switch conn.type {
        case .postgres: .green
        case .mysql: .orange
        case .sqlite: .blue
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

struct QueryLogRow: View {
    let entry: QueryLogEntry

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            // Verdict badge
            Text(entry.verdict.uppercased())
                .font(.system(size: 9, weight: .bold))
                .foregroundColor(.white)
                .padding(.horizontal, 5)
                .padding(.vertical, 2)
                .background(verdictColor)
                .clipShape(.rect(cornerRadius: 3))
                .frame(width: 54, alignment: .leading)

            VStack(alignment: .leading, spacing: 2) {
                Text(entry.sql)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundColor(.primary)
                    .lineLimit(2)

                HStack(spacing: 8) {
                    if let reason = entry.reason {
                        Text(reason)
                            .font(.system(size: 10))
                            .foregroundColor(.red)
                            .lineLimit(1)
                    }
                    Text(entry.createdAt)
                        .font(.system(size: 10))
                        .foregroundColor(.secondary)
                }
            }

            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
    }

    private var verdictColor: Color {
        switch entry.verdict {
        case "allowed": return .green
        case "blocked": return .red
        default:        return .orange
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
