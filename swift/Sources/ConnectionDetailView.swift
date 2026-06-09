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
    let onEdit: () -> Void
    let onDelete: () -> Void

    @State private var urlCopied = false
    @State private var snippetCopied = false
    @State private var selectedClient: MCPClient = .opencode
    @State private var testStatus: TestStatus = .idle

    enum TestStatus { case idle, testing, ok, fail(String) }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header
                Divider()
                VStack(alignment: .leading, spacing: 20) {
                    mcpURLSection
                    configSnippetSection
                    connectionDetailsSection
                    testSection
                }
                .padding()
            }
        }
    }

    // MARK: - Header

    private var header: some View {
        HStack {
            Circle()
                .fill(dotColor)
                .frame(width: 10, height: 10)
            Text(conn.name)
                .font(.system(size: 17, weight: .semibold))
            Text(conn.type.label)
                .font(.system(size: 11, weight: .medium))
                .foregroundColor(.secondary)
                .padding(.horizontal, 7)
                .padding(.vertical, 3)
                .background(Color(NSColor.controlBackgroundColor))
                .clipShape(.rect(cornerRadius: 4))
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
        .padding()
    }

    // MARK: - MCP URL

    private var mcpURLSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel("MCP URL")

            HStack(spacing: 8) {
                Text(conn.mcpURL)
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundColor(.primary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)
                    .background(Color(NSColor.controlBackgroundColor))
                    .clipShape(.rect(cornerRadius: 6))
                    .overlay(
                        RoundedRectangle(cornerRadius: 6)
                            .stroke(Color(NSColor.separatorColor), lineWidth: 0.5)
                    )

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

            Text("Paste this URL into your AI agent's MCP config. The agent only accesses this database.")
                .font(.system(size: 11))
                .foregroundColor(.secondary)
        }
    }

    // MARK: - Config snippet

    private var configSnippet: String {
        selectedClient.snippet(key: conn.mcpKey, url: conn.mcpURL)
    }

    private var configSnippetSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                SectionLabel("Config Snippet")
                Spacer()
                Picker("", selection: $selectedClient) {
                    ForEach(MCPClient.allCases) { client in
                        Text(client.label).tag(client)
                    }
                }
                .pickerStyle(.menu)
                .frame(width: 110)
            }

            HStack(alignment: .top, spacing: 8) {
                Text(configSnippet)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundColor(.primary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(10)
                    .background(Color(NSColor.controlBackgroundColor))
                    .clipShape(.rect(cornerRadius: 6))
                    .overlay(
                        RoundedRectangle(cornerRadius: 6)
                            .stroke(Color(NSColor.separatorColor), lineWidth: 0.5)
                    )

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

            Text("Add the highlighted block to your \(selectedClient.configPath).")
                .font(.system(size: 11))
                .foregroundColor(.secondary)
        }
    }

    // MARK: - Connection details

    private var connectionDetailsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            SectionLabel("Connection")

            if conn.type == .sqlite {
                KVRow(key: "File", value: conn.filename ?? "—")
            } else {
                KVRow(key: "Host", value: conn.host ?? "—")
                KVRow(key: "Port", value: conn.port.map(String.init) ?? "—")
                KVRow(key: "User", value: conn.user ?? "—")
                KVRow(key: "Database", value: conn.database ?? "—")
            }
        }
    }

    // MARK: - Test

    private var testSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionLabel("Status")
            testButton
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

    private var dotColor: Color {
        switch conn.type {
        case .postgres: .green
        case .mysql: .orange
        case .sqlite: .blue
        }
    }
}

// MARK: - Shared helpers

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

struct KVRow: View {
    let key: String
    let value: String
    var body: some View {
        HStack {
            Text(key)
                .frame(width: 70, alignment: .leading)
                .foregroundColor(.secondary)
                .font(.system(size: 12))
            Text(value)
                .font(.system(size: 12, design: .monospaced))
                .foregroundColor(.primary)
            Spacer()
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(Color(NSColor.controlBackgroundColor))
        .clipShape(.rect(cornerRadius: 5))
    }
}
