import SwiftUI

struct ContentView: View {
    var store: ConnectionStore
    @State private var selectedID: String?
    @State private var mode: Mode = .empty

    enum Mode { case empty, detail, add, edit }

    var selected: Connection? {
        store.connections.first { $0.id == selectedID }
    }

    var body: some View {
        HStack(spacing: 0) {
            sidebar
                .frame(width: 210)
            Divider()
            mainPanel
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(width: 700, height: 540)
        .background(Color(NSColor.windowBackgroundColor))
    }

    // MARK: - Sidebar

    private var sidebar: some View {
        VStack(spacing: 0) {
            List(store.connections, selection: $selectedID) { conn in
                ConnectionRow(conn: conn)
                    .contentShape(Rectangle())
                    .onTapGesture {
                        selectedID = conn.id
                        mode = .detail
                    }
            }
            .listStyle(.sidebar)
            .onChange(of: selectedID) { _, newValue in
                if newValue != nil { mode = .detail }
            }

            Divider()

            Button(action: { selectedID = nil; mode = .add }) {
                Label("New Connection", systemImage: "plus")
                    .font(.system(size: 12))
            }
            .buttonStyle(.plain)
            .foregroundColor(.secondary)
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Color(NSColor.controlBackgroundColor))
    }

    // MARK: - Main panel

    @ViewBuilder
    private var mainPanel: some View {
        switch mode {
        case .empty:
            EmptyStateView { selectedID = nil; mode = .add }

        case .detail:
            if let conn = selected {
                ConnectionDetailView(conn: conn) {
                    mode = .edit
                } onDelete: {
                    store.delete(conn)
                    selectedID = nil
                    mode = .empty
                }
            }

        case .add:
            ConnectionFormView(editingConn: nil) { draft in
                store.create(draft)
                mode = .empty
            } onCancel: {
                mode = selected != nil ? .detail : .empty
            }

        case .edit:
            if let conn = selected {
                ConnectionFormView(editingConn: conn) { draft in
                    store.update(conn, draft: draft)
                    mode = .detail
                } onCancel: {
                    mode = .detail
                }
            }
        }
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
        VStack(spacing: 12) {
            Text("pluk")
                .font(.system(size: 52, weight: .ultraLight, design: .default))
                .foregroundColor(.primary)
                .tracking(-1)

            Text("Plug any database into any AI agent")
                .font(.system(size: 13))
                .foregroundColor(.secondary)

            Button("Add connection", action: onAdd)
                .buttonStyle(.borderedProminent)
                .controlSize(.regular)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
