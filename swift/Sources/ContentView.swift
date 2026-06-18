import SwiftUI

struct ContentView: View {
    var store: ConnectionStore
    var serverManager: ServerManager
    @State private var selectedID: String?
    @State private var sheet: ActiveSheet?

    enum ActiveSheet: Identifiable {
        case add
        case edit(Connection)

        var id: String {
            switch self {
            case .add: "add"
            case .edit(let conn): conn.id
            }
        }
    }

    var selected: Connection? {
        store.connections.first { $0.id == selectedID }
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
        .frame(minWidth: 720, minHeight: 520)
        .safeAreaInset(edge: .bottom) {
            if serverManager.status != .running {
                ServerStatusBanner(serverManager: serverManager)
            }
        }
        .sheet(item: $sheet) { active in
            connectionSheet(active)
        }
    }

    // MARK: - Sidebar

    private var sidebar: some View {
        List(selection: $selectedID) {
            Section("Connections") {
                ForEach(store.connections) { conn in
                    ConnectionRow(conn: conn)
                        .tag(conn.id)
                }
            }
        }
        .listStyle(.sidebar)
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button(action: { sheet = .add }) {
                    Label("New Connection", systemImage: "plus")
                }
                .help("Add a new connection")
            }
        }
    }

    // MARK: - Detail

    @ViewBuilder
    private var detailPanel: some View {
        if let conn = selected {
            ConnectionDetailView(conn: conn, store: store) {
                sheet = .edit(conn)
            } onDelete: {
                store.delete(conn)
                selectedID = nil
            }
        } else {
            EmptyStateView { sheet = .add }
        }
    }

    // MARK: - Add / Edit sheet

    @ViewBuilder
    private func connectionSheet(_ active: ActiveSheet) -> some View {
        let editing: Connection? = {
            if case .edit(let conn) = active { return conn }
            return nil
        }()

        ConnectionFormView(editingConn: editing) { draft in
            if let editing {
                store.update(editing, draft: draft)
                selectedID = editing.id
            } else {
                store.create(draft)
                selectedID = store.connections.first?.id // newest sorts to top
            }
            sheet = nil
        } onCancel: {
            sheet = nil
        }
        .frame(width: 580, height: 620)
    }
}

// MARK: - Sidebar row

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

// Square badge: DB type at a glance. Muted tinted fill, no loud color.
struct TypeBadge: View {
    let type: ConnectionType
    @SwiftUI.Environment(\.backgroundProminence) private var prominence

    private var selected: Bool { prominence == .increased }

    var body: some View {
        Text(abbrev)
            .font(.system(size: 9, weight: .semibold, design: .rounded))
            .foregroundColor(selected ? .white : color)
            .frame(width: 24, height: 24)
            .background(
                (selected ? Color.white.opacity(0.22) : color.opacity(0.14)),
                in: RoundedRectangle(cornerRadius: 6, style: .continuous)
            )
            .help(type.label)
    }

    private var abbrev: String {
        switch type {
        case .postgres: "PG"
        case .mysql: "MY"
        case .sqlite: "LT"
        }
    }

    private var color: Color {
        switch type {
        case .postgres: Color(red: 0.30, green: 0.46, blue: 0.66) // muted postgres blue
        case .mysql: Color(red: 0.78, green: 0.55, blue: 0.20)    // muted mysql amber
        case .sqlite: Color(red: 0.45, green: 0.50, blue: 0.56)   // slate
        }
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

            Text("Plug any database into any AI agent")
                .font(.system(size: 13))
                .foregroundColor(.secondary)

            Button("Add connection", action: onAdd)
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .padding(.top, 4)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
