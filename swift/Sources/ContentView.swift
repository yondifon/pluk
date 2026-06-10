import SwiftUI

struct ContentView: View {
    var store: ConnectionStore
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
        .safeAreaInset(edge: .bottom) {
            Button(action: { sheet = .add }) {
                Label("New Connection", systemImage: "plus")
                    .font(.system(size: 12))
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .buttonStyle(.plain)
            .foregroundColor(.secondary)
            .padding(.horizontal, 14)
            .padding(.vertical, 9)
            .background(.bar)
        }
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
            ConnectionDetailView(conn: conn) {
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
        HStack(spacing: 8) {
            Circle()
                .fill(dotColor)
                .frame(width: 8, height: 8)
            Text(conn.name)
                .font(.system(size: 13))
                .lineLimit(1)
            Spacer()
            Text(conn.type.rawValue)
                .font(.system(size: 10, weight: .medium))
                .foregroundColor(.secondary)
        }
        .padding(.vertical, 2)
    }

    private var dotColor: Color {
        switch conn.type {
        case .postgres: .green
        case .mysql: .orange
        case .sqlite: .blue
        }
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
