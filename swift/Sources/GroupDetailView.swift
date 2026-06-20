import AppKit
import SwiftUI

// Detail panel for a group: one MCP endpoint aggregating several integrations.
// The server exposes each member's tools namespaced by member name (e.g.
// `metrics__query`). Editing (name/environment/members) happens in a sheet via
// the Edit button, mirroring the integration detail view.
struct GroupDetailView: View {
    let group: ConnectionGroup
    let store: ConnectionStore
    let onEdit: () -> Void
    let onDelete: () -> Void

    @State private var urlCopied = false
    @State private var snippetCopied = false
    @State private var selectedClient: MCPClient = .opencode

    private var members: [Connection] {
        group.memberIds.compactMap { id in store.connections.first { $0.id == id } }
    }

    private var subtitle: String {
        let count = "\(members.count) integration\(members.count == 1 ? "" : "s")"
        guard let env = group.environment else { return "Group · \(count)" }
        return "Group · \(count) · \(env.label)"
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    endpointSection
                    configSnippetSection
                    membersSection
                }
                .padding(18)
            }
        }
        .background(.clear)
    }

    // MARK: - Header

    private var header: some View {
        HStack(spacing: 10) {
            Image(systemName: "square.stack.3d.up.fill")
                .font(.system(size: 15))
                .foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 3) {
                Text(group.name)
                    .font(.system(size: 15, weight: .semibold))
                Text(subtitle)
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

    // MARK: - Endpoint

    private var endpointSection: some View {
        DetailSection("MCP endpoint") {
            HStack(spacing: 10) {
                Text(group.mcpURL)
                    .font(.system(size: 12, design: .monospaced))
                    .textSelection(.enabled)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer()
                Button(urlCopied ? "Copied!" : "Copy") {
                    copy(group.mcpURL) { urlCopied = $0 }
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.small)
                .tint(urlCopied ? .green : .accentColor)
                .animation(.easeInOut(duration: 0.15), value: urlCopied)
            }
            .padding(12)
        }
    }

    // MARK: - Config samples (one endpoint, all member tools)

    private var configSnippet: String {
        selectedClient.snippet(key: group.mcpKey, url: group.mcpURL)
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
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(10)
                    .codeBlockSurface()

                Button(snippetCopied ? "Copied!" : "Copy") {
                    copy(configSnippet) { snippetCopied = $0 }
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .tint(snippetCopied ? .green : nil)
                .animation(.easeInOut(duration: 0.15), value: snippetCopied)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
        }
    }

    // MARK: - Members

    private var membersSection: some View {
        DetailSection("Integrations") {
            if members.isEmpty {
                Text("No integrations in this group. Click Edit to add some.")
                    .font(.system(size: 12))
                    .foregroundColor(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
            } else {
                VStack(spacing: 0) {
                    ForEach(members) { conn in
                        let overrides = group.member(conn.id)?.overrides ?? [:]
                        VStack(alignment: .leading, spacing: 4) {
                            HStack(spacing: 9) {
                                TypeBadge(type: conn.type)
                                Text(conn.name).font(.system(size: 13))
                                EnvTag(environment: conn.environment)
                                Spacer()
                                Text("\(NamespaceFormat.slug(conn.name))__*")
                                    .font(.system(size: 10, design: .monospaced))
                                    .foregroundStyle(.tertiary)
                            }
                            if !overrides.isEmpty {
                                Text(overrides.sorted { $0.key < $1.key }
                                        .map { "\($0.key) → \($0.value)" }
                                        .joined(separator: "   "))
                                    .font(.system(size: 10, design: .monospaced))
                                    .foregroundStyle(Color.accentColor)
                                    .padding(.leading, 32)
                            }
                        }
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                        if conn.id != members.last?.id {
                            Divider().padding(.leading, 12)
                        }
                    }
                }
            }
        }
    }

    private func copy(_ text: String, _ flag: @escaping (Bool) -> Void) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        flag(true)
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(1.5))
            flag(false)
        }
    }
}

// Mirrors the server's namespace slug (mcp/namespace.ts) so the detail view can
// show each member's tool prefix (e.g. `metrics_db__*`).
enum NamespaceFormat {
    static func slug(_ name: String) -> String {
        let s = name.lowercased()
            .replacingOccurrences(of: "[^a-z0-9]+", with: "_", options: .regularExpression)
            .trimmingCharacters(in: CharacterSet(charactersIn: "_"))
        return s.isEmpty ? "member" : s
    }
}
