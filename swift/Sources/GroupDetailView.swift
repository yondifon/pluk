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
    @State private var tab: GroupTab = .overview
    @SwiftUI.Environment(\.accessibilityReduceMotion) private var reduceMotion

    enum GroupTab: String, CaseIterable {
        case overview = "Overview"
        case logs = "Logs"
        var icon: String { self == .overview ? "square.stack.3d.up" : "list.bullet.rectangle" }
    }

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
            tabBar
            Rectangle().fill(Color.hairline).frame(height: 0.5)
            switch tab {
            case .overview:
                ScrollView {
                    VStack(alignment: .leading, spacing: Space.xl) {
                        endpointSection
                        ConfigSnippetSection(mcpKey: group.mcpKey, mcpURL: group.mcpURL,
                                             title: group.name, id: group.id,
                                             toastCenter: store.toastCenter)
                        membersSection
                    }
                    .padding(Space.xl)
                }
            case .logs:
                LogsTab(scope: .group(group), store: store)
            }
        }
        .background(.clear)
    }

    // MARK: - Tab bar

    private var tabBar: some View {
        PillTabs(tabs: GroupTab.allCases, title: \.rawValue, selection: $tab)
    }

    // MARK: - Header

    private var header: some View {
        HStack(alignment: .top, spacing: Space.md) {
            Image(systemName: "square.stack.3d.up.fill")
                .font(.system(size: 15))
                .foregroundStyle(.secondary)
                .frame(width: 34, height: 34)
                .background(Color.controlFill, in: RoundedRectangle(cornerRadius: Radius.md - 2, style: .continuous))
            VStack(alignment: .leading, spacing: Space.xs + 1) {
                HStack(alignment: .center, spacing: Space.md) {
                    Text(group.name)
                        .font(.uiTitle)
                        .tracking(-0.2)
                        .lineLimit(1)
                    Spacer(minLength: Space.md)
                    Button("Edit", action: onEdit)
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                        .frame(height: Control.height)
                    OverflowMenu {
                        Button("Delete…", role: .destructive, action: onDelete)
                    }
                }
                Text(subtitle)
                    .font(.uiCaption)
                    .foregroundColor(.secondary)
            }
        }
        .padding(.horizontal, Space.xl)
        .padding(.top, Space.lg)
        .padding(.bottom, Space.md)
    }

    // MARK: - Endpoint

    private var endpointSection: some View {
        DetailSection("MCP endpoint") {
            InspectorRow("URL") {
                HStack(spacing: Space.sm) {
                    Text(group.mcpURL)
                        .font(.mono(12))
                        .foregroundColor(.primary)
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
                    .animation(reduceMotion ? nil : .easeInOut(duration: 0.15), value: urlCopied)
                }
            }
        }
    }

    // MARK: - Members

    private var membersSection: some View {
        DetailSection("Integrations") {
            if members.isEmpty {
                Text("No integrations in this group. Click Edit to add some.")
                    .font(.uiLabel)
                    .foregroundColor(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(Space.md)
            } else {
                VStack(spacing: 0) {
                    ForEach(members) { conn in
                        let overrides = group.member(conn.id)?.overrides ?? [:]
                        VStack(alignment: .leading, spacing: Space.xs) {
                            HStack(spacing: Space.sm + 2) {
                                TypeBadge(type: conn.type)
                                Text(conn.name).font(.uiBody)
                                EnvTag(environment: conn.environment)
                                Spacer()
                                // The prefix every tool of this member carries
                                // inside the group's namespace.
                                Text("\(NamespaceFormat.slug(conn.name))__*")
                                    .font(.mono(10))
                                    .foregroundStyle(.tertiary)
                            }
                            if !overrides.isEmpty {
                                Text(overrides.sorted { $0.key < $1.key }
                                        .map { "\($0.key) → \($0.value)" }
                                        .joined(separator: "   "))
                                    .font(.mono(10))
                                    .foregroundStyle(Color.accentColor)
                                    .padding(.leading, Space.xxl)
                            }
                        }
                        .padding(.horizontal, Space.md)
                        .padding(.vertical, Space.sm + 1)
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

#if DEBUG
#Preview {
    GroupDetailView(
        group: .sample,
        store: .preview,
        onEdit: {},
        onDelete: {}
    )
}
#endif

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
