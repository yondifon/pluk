import SwiftUI

struct ContentView: View {
    var store: ConnectionStore
    var serverManager: ServerManager
    @State private var selectedID: String?
    @State private var sheet: ActiveSheet?
    @State private var pendingDelete: PendingDelete?

    // A delete awaiting confirmation. Every delete entry point (sidebar context
    // menu, detail header button, for both integrations and groups) routes here
    // so a destructive, irreversible action is never one stray click away.
    enum PendingDelete: Identifiable {
        case connection(Connection)
        case group(ConnectionGroup)

        var id: String {
            switch self {
            case .connection(let conn): conn.id
            case .group(let group): "group:\(group.id)"
            }
        }
        var name: String {
            switch self {
            case .connection(let conn): conn.name
            case .group(let group): group.name
            }
        }
        var noun: String {
            switch self {
            case .connection: "integration"
            case .group: "group"
            }
        }
    }

    enum ActiveSheet: Identifiable {
        case add
        case edit(Connection)
        case editGroup(ConnectionGroup)

        var id: String {
            switch self {
            case .add: "add"
            case .edit(let conn): conn.id
            case .editGroup(let group): "group:\(group.id)"
            }
        }
    }

    var selected: Connection? {
        store.connections.first { $0.id == selectedID }
    }

    var selectedGroup: ConnectionGroup? {
        store.groups.first { $0.id == selectedID }
    }

    var body: some View {
        NavigationSplitView {
            sidebar
                .navigationSplitViewColumnWidth(min: 200, ideal: 220, max: 300)
        } detail: {
            detailPanel
                .frame(minWidth: 440, minHeight: 480)
        }
        .navigationSplitViewStyle(.balanced)
        .glassWindowBackground()
        .frame(minWidth: 720, minHeight: 520)
        .safeAreaInset(edge: .bottom) {
            if serverManager.status != .running {
                ServerStatusBanner(serverManager: serverManager)
            }
        }
        .sheet(item: $sheet) { active in
            connectionSheet(active)
        }
        .confirmationDialog(
            pendingDelete.map { "Delete \($0.noun) “\($0.name)”?" } ?? "",
            isPresented: Binding(get: { pendingDelete != nil }, set: { if !$0 { pendingDelete = nil } }),
            presenting: pendingDelete
        ) { item in
            Button("Delete", role: .destructive) { performDelete(item) }
            Button("Cancel", role: .cancel) {}
        } message: { _ in
            Text("This can't be undone.")
        }
        .task { await store.loadAdapters() }
    }

    private func performDelete(_ item: PendingDelete) {
        switch item {
        case .connection(let conn):
            store.delete(conn)
            if selectedID == conn.id { selectedID = nil }
        case .group(let group):
            store.deleteGroup(group)
            if selectedID == group.id { selectedID = nil }
        }
    }

    // MARK: - Sidebar

    private var sidebar: some View {
        List(selection: $selectedID) {
            if !store.groups.isEmpty {
                Section("Groups") {
                    ForEach(store.groups) { group in
                        GroupRow(group: group)
                            .tag(group.id)
                            .contextMenu {
                                Button("Delete", role: .destructive) {
                                    pendingDelete = .group(group)
                                }
                            }
                    }
                }
            }

            Section("Integrations") {
                ForEach(store.connections) { conn in
                    ConnectionRow(conn: conn)
                        .tag(conn.id)
                        .contextMenu {
                            Button("Duplicate") { selectedID = store.duplicate(conn) }
                            Button("Delete", role: .destructive) {
                                pendingDelete = .connection(conn)
                            }
                        }
                }
            }
        }
        .listStyle(.sidebar)
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Menu {
                    Button("New Integration") { sheet = .add }
                    Button("New Group") { selectedID = store.createGroup() }
                } label: {
                    Label("New", systemImage: "plus")
                }
                .help("Add an integration or group")
            }
        }
    }

    // MARK: - Detail

    @ViewBuilder
    private var detailPanel: some View {
        if let group = selectedGroup {
            GroupDetailView(group: group, store: store) {
                sheet = .editGroup(group)
            } onDelete: {
                pendingDelete = .group(group)
            }
            .id(group.id)
        } else if let conn = selected {
            ConnectionDetailView(conn: conn, store: store) {
                sheet = .edit(conn)
            } onDelete: {
                pendingDelete = .connection(conn)
            } onDuplicate: {
                selectedID = store.duplicate(conn)
            }
            // Rebuild the whole detail subtree when the selected project changes
            // so per-project @State (active tab, loaded logs) resets instead of
            // lingering from the previously selected connection.
            .id(conn.id)
        } else {
            EmptyStateView { sheet = .add }
        }
    }

    // MARK: - Add / Edit sheet

    @ViewBuilder
    private func connectionSheet(_ active: ActiveSheet) -> some View {
        if case .editGroup(let group) = active {
            GroupFormView(group: group, connections: store.connections, adapters: store.adapters) { updated in
                store.updateGroup(updated)
                selectedID = updated.id
                sheet = nil
            } onCancel: {
                sheet = nil
            }
        } else {
            connectionForm(active)
        }
    }

    @ViewBuilder
    private func connectionForm(_ active: ActiveSheet) -> some View {
        let editing: Connection? = {
            if case .edit(let conn) = active { return conn }
            return nil
        }()

        ConnectionFormView(
            editingConn: editing,
            adapters: store.adapters,
            adaptersLoadFailed: store.adaptersLoadFailed,
            onRetryAdapters: { Task { await store.loadAdapters() } }
        ) { draft in
            if let editing {
                store.update(editing, draft: draft)
                selectedID = editing.id
            } else {
                selectedID = store.create(draft)
            }
            sheet = nil
        } onCancel: {
            sheet = nil
        }
        .frame(width: 580, height: 620)
    }
}

// MARK: - Sidebar row

struct GroupRow: View {
    let group: ConnectionGroup

    var body: some View {
        HStack(spacing: 9) {
            Image(systemName: "square.stack.3d.up.fill")
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
                .frame(width: 24, height: 24)
            VStack(alignment: .leading, spacing: 1) {
                Text(group.name)
                    .font(.system(size: 13))
                    .lineLimit(1)
                Text("\(group.memberIds.count) integration\(group.memberIds.count == 1 ? "" : "s")")
                    .font(.system(size: 10))
                    .foregroundStyle(.tertiary)
            }
            Spacer()
        }
        .padding(.vertical, 3)
    }
}

struct ConnectionRow: View {
    let conn: Connection

    var body: some View {
        HStack(spacing: 9) {
            TypeBadge(type: conn.type)
            VStack(alignment: .leading, spacing: 1) {
                Text(conn.name)
                    .font(.system(size: 13))
                    .lineLimit(1)
                EnvTag(environment: conn.environment)
            }
            Spacer()
            if conn.readOnly {
                Image(systemName: "lock.fill")
                    .font(.system(size: 9))
                    .foregroundStyle(.tertiary)
                    .help("Read-only")
            }
        }
        .padding(.vertical, 3)
    }
}

// Square badge: adapter type at a glance. Muted tinted fill, no loud color.
struct TypeBadge: View {
    let type: String
    var size: CGFloat = 24
    @SwiftUI.Environment(\.backgroundProminence) private var prominence

    private var selected: Bool { prominence == .increased }

    var body: some View {
        AdapterGlyph(type: type, color: AdapterStyle.color(for: type), selected: selected)
            .frame(width: size, height: size)
            .background(
                (selected ? Color.white.opacity(0.22) : AdapterStyle.color(for: type).opacity(0.14)),
                in: RoundedRectangle(cornerRadius: size * 0.25, style: .continuous)
            )
            .help(ConnectionType(rawValue: type)?.label ?? type.capitalized)
    }
}

// Brand mark for an adapter: bundled brand logo where we have one (Linear, Sentry,
// Postgres, SQLite), an SF Symbol for symbol-only adapters (SSH), else a 2-letter
// abbreviation. Glyph is tinted to match the muted badge — white when selected.
struct AdapterGlyph: View {
    let type: String
    let color: Color
    let selected: Bool

    var body: some View {
        if let logo = AdapterStyle.logo(for: type) {
            Image(nsImage: logo)
                .renderingMode(.template)
                .resizable()
                .scaledToFit()
                .padding(4)
                .foregroundStyle(selected ? .white : color)
        } else if let symbol = AdapterStyle.symbol(for: type) {
            Image(systemName: symbol)
                .font(.system(size: 11, weight: .semibold))
                .foregroundColor(selected ? .white : color)
        } else {
            Text(AdapterStyle.abbrev(for: type))
                .font(.system(size: 9, weight: .semibold, design: .rounded))
                .foregroundColor(selected ? .white : color)
        }
    }
}

// Per-adapter visual treatment: brand tint, bundled logo, SF Symbol fallback.
enum AdapterStyle {
    static func color(for type: String) -> Color {
        switch type {
        case "postgres": Color(red: 0.30, green: 0.46, blue: 0.66) // muted postgres blue
        case "mysql":    Color(red: 0.78, green: 0.55, blue: 0.20) // muted mysql amber
        case "sqlite":   Color(red: 0.45, green: 0.50, blue: 0.56) // slate
        case "linear":   Color(red: 0.37, green: 0.42, blue: 0.82) // Linear indigo #5E6AD2
        case "sentry":   Color(red: 0.49, green: 0.42, blue: 0.78) // Sentry purple
        case "ssh":      Color(red: 0.27, green: 0.55, blue: 0.45) // terminal teal
        default:         Color(red: 0.40, green: 0.42, blue: 0.50) // neutral
        }
    }

    static func symbol(for type: String) -> String? {
        switch type {
        case "ssh": "terminal"
        default: nil
        }
    }

    static func abbrev(for type: String) -> String {
        switch type {
        case "postgres": "PG"
        case "mysql": "MY"
        case "sqlite": "LT"
        default: String(type.prefix(2)).uppercased()
        }
    }

    // Release copies logos into the app's Resources dir (Bundle.main); dev serves them
    // from the SwiftPM resource bundle. Resolve by URL — never `Bundle.module`, whose
    // accessor fatalErrors when the bundle is absent (i.e. in the packaged app).
    private static let logoBundle: Bundle = {
        if let url = Bundle.main.url(forResource: "Pluk_Pluk", withExtension: "bundle"),
           let bundle = Bundle(url: url) {
            return bundle
        }
        return Bundle.main
    }()

    // Bundled brand logos are white alpha masks rendered as template images.
    private static var logoCache: [String: NSImage?] = [:]
    static func logo(for type: String) -> NSImage? {
        if let hit = logoCache[type] { return hit }
        let img = logoBundle
            .url(forResource: type, withExtension: "png", subdirectory: "AdapterLogos")
            .flatMap { NSImage(contentsOf: $0) }
        logoCache[type] = img
        return img
    }
}

// Environment cue: small dot + neutral caption that adapts to row selection.
struct EnvTag: View {
    let environment: Environment
    @SwiftUI.Environment(\.backgroundProminence) private var prominence

    private var selected: Bool { prominence == .increased }

    var body: some View {
        HStack(spacing: 4) {
            Circle()
                .fill(selected ? Color.white : environment.color.opacity(0.85))
                .frame(width: 5, height: 5)
            Text(environment.label.uppercased())
                .font(.system(size: 9, weight: .medium))
                .tracking(0.4)
                .foregroundStyle(.secondary)
        }
        .help("\(environment.label) environment")
    }
}

// MARK: - Server status banner

private struct ServerStatusBanner: View {
    let serverManager: ServerManager

    var body: some View {
        HStack(spacing: 8) {
            if serverManager.status == .starting {
                ProgressView().scaleEffect(0.7).frame(width: 12, height: 12)
                Text("Server starting…")
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
            } else {
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.system(size: 11))
                    .foregroundColor(.orange)
                Text("Server not running")
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
            }
            Spacer()
            if serverManager.status == .stopped {
                Button("Restart") { serverManager.start() }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 7)
        .background(.bar)
        .overlay(alignment: .top) { Divider() }
        .transition(.move(edge: .bottom).combined(with: .opacity))
        .animation(.easeInOut(duration: 0.2), value: serverManager.status == .stopped)
    }
}

// MARK: - Empty state

struct EmptyStateView: View {
    let onAdd: () -> Void

    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: "cable.connector")
                .font(.system(size: 38, weight: .light))
                .foregroundStyle(.secondary)

            Text("pluk")
                .font(.system(size: 52, weight: .ultraLight))
                .tracking(-1)

            Text("Plug any service into your AI agents — databases, Linear, and more")
                .font(.system(size: 13))
                .foregroundColor(.secondary)

            Button("New Integration", action: onAdd)
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .padding(.top, 4)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
