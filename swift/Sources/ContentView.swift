import SwiftUI

struct ContentView: View {
    var store: ConnectionStore
    var serverManager: ServerManager
    var updateChecker: UpdateChecker
    @State private var selectedID: String?
    @State private var sheet: ActiveSheet?
    @State private var pendingDelete: PendingDelete?
    @State private var toastCenter = ToastCenter()
    @State private var search = ""
    @FocusState private var searchFocused: Bool
    @State private var typeFilter: Set<String> = []
    @State private var envFilter: Set<Environment> = []
    @State private var showFilters = false

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

    // MARK: - Filtering

    private var query: String {
        search.trimmingCharacters(in: .whitespaces)
    }

    private var filtersActive: Bool { !typeFilter.isEmpty || !envFilter.isEmpty }
    private var isNarrowed: Bool { !query.isEmpty || filtersActive }

    // Type filters are integration-scoped (groups have no adapter type), so any
    // active type filter hides groups. The environment filter keeps groups whose
    // scope matches — plus unscoped/mixed groups, which may span the chosen env.
    private var filteredGroups: [ConnectionGroup] {
        guard typeFilter.isEmpty else { return [] }
        return store.groups.filter { group in
            let matchSearch = query.isEmpty || group.name.localizedCaseInsensitiveContains(query)
            let matchEnv = envFilter.isEmpty || group.environment.map(envFilter.contains) ?? true
            return matchSearch && matchEnv
        }
    }

    private var filteredConnections: [Connection] {
        store.connections.filter { matchesSearch($0) && matchesType($0) && matchesEnv($0) }
    }

    // Match name, adapter type (raw + label), and environment so users can filter
    // by "postgres", "prod", or a connection's name interchangeably.
    private func matchesSearch(_ conn: Connection) -> Bool {
        guard !query.isEmpty else { return true }
        return conn.name.localizedCaseInsensitiveContains(query)
            || conn.type.localizedCaseInsensitiveContains(query)
            || conn.typeLabel.localizedCaseInsensitiveContains(query)
            || conn.environment.label.localizedCaseInsensitiveContains(query)
    }

    private func matchesType(_ conn: Connection) -> Bool {
        typeFilter.isEmpty || typeFilter.contains(conn.type)
    }

    private func matchesEnv(_ conn: Connection) -> Bool {
        envFilter.isEmpty || envFilter.contains(conn.environment)
    }

    private var noMatches: Bool {
        isNarrowed && filteredGroups.isEmpty && filteredConnections.isEmpty
    }

    // Filter menu options — only adapter types / environments actually in use, so
    // the popover never offers a filter that would yield nothing.
    private var availableTypes: [String] {
        var seen = Set<String>()
        var out: [String] = []
        for conn in store.connections where seen.insert(conn.type).inserted {
            out.append(conn.type)
        }
        return out.sorted { label(forType: $0).localizedCaseInsensitiveCompare(label(forType: $1)) == .orderedAscending }
    }

    private var availableEnvs: [Environment] {
        let present = Set(store.connections.map(\.environment)).union(store.groups.compactMap(\.environment))
        return Environment.allCases.filter { present.contains($0) }
    }

    private func label(forType type: String) -> String {
        store.adapters.first { $0.id == type }?.label
            ?? ConnectionType(rawValue: type)?.label
            ?? type.capitalized
    }

    var body: some View {
        appContent
    }

    private var appContent: some View {
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
        .overlay(alignment: .top) {
            ToastOverlay(center: toastCenter) { connId in store.test(connectionId: connId) }
        }
        .safeAreaInset(edge: .bottom) {
            VStack(spacing: 0) {
                switch updateChecker.state {
                case .updateAvailable(let commit):
                    UpdateBanner(commit: commit, updating: false) { updateChecker.installUpdate() }
                case .updating:
                    UpdateBanner(commit: nil, updating: true) {}
                default:
                    EmptyView()
                }
                if serverManager.status != .running {
                    ServerStatusBanner(serverManager: serverManager)
                }
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
        .task {
            store.toastCenter = toastCenter
            ToastCenter.requestNotificationAccess()
            // Poll connection health so failures (SSH/auth/tunnel) surface as a
            // red dot + toast without the user manually testing each connection.
            while !Task.isCancelled {
                await store.refreshHealth()
                try? await Task.sleep(for: .seconds(15))
            }
        }
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
        VStack(spacing: 0) {
            searchRow
            sidebarList
        }
        .background(Color.pageSurface)
        // Hidden ⌘F shortcut focuses the filter field (searchFocused binding on
        // .searchable is macOS 15+; this custom field keeps ⌘F working on 14).
        .background(
            Button("") { searchFocused = true }
                .keyboardShortcut("f", modifiers: .command)
                .opacity(0)
        )
        .toolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                Button {
                    sheet = .add
                } label: {
                    Label("New Integration", systemImage: "plus")
                }
                .help("New Integration")
                .keyboardShortcut("n", modifiers: .command)

                Button {
                    selectedID = store.createGroup()
                } label: {
                    Label("New Group", systemImage: "square.stack.3d.up")
                }
                .help("New Group")
                .keyboardShortcut("n", modifiers: [.command, .shift])
            }
        }
    }

    private var searchRow: some View {
        HStack(spacing: 6) {
            searchField
            filterButton
        }
        .padding(.horizontal, 10)
        .padding(.top, 10)
        .padding(.bottom, 6)
    }

    private var searchField: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
            TextField("Filter integrations", text: $search)
                .textFieldStyle(.plain)
                .font(.system(size: 12))
                .focused($searchFocused)
                .onExitCommand { search = ""; searchFocused = false }
            if !search.isEmpty {
                Button {
                    search = ""
                    searchFocused = true
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 12))
                        .foregroundStyle(.tertiary)
                }
                .buttonStyle(.plain)
                .help("Clear")
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(.quaternary.opacity(0.4), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
    }

    // Funnel beside the search: opens a popover to narrow the list by adapter type
    // and environment. Fills + tints accent while any filter is applied.
    private var filterButton: some View {
        Button {
            showFilters.toggle()
        } label: {
            Image(systemName: filtersActive ? "line.3.horizontal.decrease.circle.fill" : "line.3.horizontal.decrease.circle")
                .font(.system(size: 13))
                .foregroundStyle(filtersActive ? Color.accentColor : .secondary)
                .frame(width: 27, height: 27)
                .background(.quaternary.opacity(0.4), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
        .buttonStyle(.plain)
        .help("Filter by type and environment")
        .popover(isPresented: $showFilters, arrowEdge: .bottom) {
            FilterPopover(
                types: availableTypes,
                label: label(forType:),
                environments: availableEnvs,
                typeFilter: $typeFilter,
                envFilter: $envFilter
            )
        }
    }

    private var sidebarList: some View {
        List(selection: $selectedID) {
            if !filteredGroups.isEmpty {
                Section("Groups") {
                    ForEach(filteredGroups) { group in
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

            if !filteredConnections.isEmpty {
                Section("Integrations") {
                    ForEach(filteredConnections) { conn in
                        ConnectionRow(conn: conn, health: store.health[conn.id])
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
        }
        .listStyle(.sidebar)
        .scrollContentBackground(.hidden)
        .background(Color.pageSurface)
        .overlay {
            if noMatches {
                if query.isEmpty {
                    ContentUnavailableView(
                        "No matches",
                        systemImage: "line.3.horizontal.decrease.circle",
                        description: Text("No integrations match the selected filters.")
                    )
                } else {
                    ContentUnavailableView.search(text: query)
                }
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
                    .font(.dev(size: 10))
                    .foregroundStyle(.tertiary)
            }
            Spacer()
        }
        .padding(.vertical, 3)
    }
}

struct ConnectionRow: View {
    let conn: Connection
    var health: ConnHealth?

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
            if health?.isError == true {
                Circle()
                    .fill(Color.red)
                    .frame(width: 7, height: 7)
                    .help(health?.error ?? "Connection failing")
            }
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
// Postgres, SQLite, GitHub, Redis, Slack), an SF Symbol for symbol-only adapters
// (SSH), else a 2-letter abbreviation. Glyph is tinted to match the muted badge —
// white when selected.
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
                .font(.dev(size: 9, weight: .semibold))
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
        case "github":   Color(red: 0.22, green: 0.25, blue: 0.30) // GitHub near-black slate
        case "redis":    Color(red: 0.78, green: 0.25, blue: 0.18) // muted Redis red #D82C20
        case "slack":    Color(red: 0.46, green: 0.18, blue: 0.45) // Slack aubergine #4A154B
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
            .map { downsample($0, to: 48) }
        logoCache[type] = img
        return img
    }

    private static func downsample(_ image: NSImage, to maxSize: CGFloat) -> NSImage {
        let src = image.size
        guard src.width > maxSize || src.height > maxSize else { return image }
        let scale = min(maxSize / src.width, maxSize / src.height)
        let dst = NSSize(width: src.width * scale, height: src.height * scale)
        let resized = NSImage(size: dst)
        resized.lockFocus()
        image.draw(in: NSRect(origin: .zero, size: dst),
                   from: NSRect(origin: .zero, size: src),
                   operation: .copy, fraction: 1.0)
        resized.unlockFocus()
        resized.isTemplate = image.isTemplate
        return resized
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
                .font(.dev(size: 9, weight: .medium))
                .tracking(0.4)
                .foregroundStyle(.secondary)
        }
        .help("\(environment.label) environment")
    }
}

// MARK: - Filter popover

// Multi-select filter beside the search field. A popover (not a Menu) so several
// types/environments can be toggled without it dismissing on every choice.
private struct FilterPopover: View {
    let types: [String]
    let label: (String) -> String
    let environments: [Environment]
    @Binding var typeFilter: Set<String>
    @Binding var envFilter: Set<Environment>

    private var active: Bool { !typeFilter.isEmpty || !envFilter.isEmpty }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    if !types.isEmpty {
                        section("Type") {
                            ForEach(types, id: \.self) { type in
                                FilterRow(isOn: typeFilter.contains(type)) {
                                    toggle(&typeFilter, type)
                                } label: {
                                    TypeBadge(type: type, size: 18)
                                    Text(label(type)).font(.system(size: 12))
                                }
                            }
                        }
                    }
                    if !environments.isEmpty {
                        section("Environment") {
                            ForEach(environments, id: \.self) { env in
                                FilterRow(isOn: envFilter.contains(env)) {
                                    toggle(&envFilter, env)
                                } label: {
                                    Circle()
                                        .fill(env.color.opacity(0.85))
                                        .frame(width: 7, height: 7)
                                        .frame(width: 18)
                                    Text(env.label).font(.system(size: 12))
                                }
                            }
                        }
                    }
                }
                .padding(12)
            }
        }
        .frame(width: 220)
        .frame(maxHeight: 360)
    }

    private var header: some View {
        HStack {
            Text("Filters").font(.system(size: 12, weight: .semibold))
            Spacer()
            if active {
                Button("Clear") { typeFilter = []; envFilter = [] }
                    .buttonStyle(.plain)
                    .font(.system(size: 11))
                    .foregroundStyle(Color.accentColor)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
    }

    @ViewBuilder
    private func section<C: View>(_ title: String, @ViewBuilder content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title.uppercased())
                .font(.dev(size: 9, weight: .semibold))
                .tracking(0.45)
                .foregroundStyle(.tertiary)
            VStack(alignment: .leading, spacing: 2) { content() }
        }
    }

    private func toggle<T: Hashable>(_ set: inout Set<T>, _ value: T) {
        if set.contains(value) { set.remove(value) } else { set.insert(value) }
    }
}

private struct FilterRow<Label: View>: View {
    let isOn: Bool
    let action: () -> Void
    @ViewBuilder let label: () -> Label

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                label()
                Spacer(minLength: 4)
                Image(systemName: isOn ? "checkmark.circle.fill" : "circle")
                    .font(.system(size: 12))
                    .foregroundStyle(isOn ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.quaternary))
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Server status banner

private struct ServerStatusBanner: View {
    let serverManager: ServerManager
    @SwiftUI.Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack(spacing: 8) {
            if serverManager.status == .starting {
                ProgressView().scaleEffect(0.7).frame(width: 12, height: 12)
                Text("Server starting…")
                    .font(.dev(size: 11))
                    .foregroundColor(.secondary)
            } else {
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.system(size: 11))
                    .foregroundColor(.orange)
                Text("Server not running")
                    .font(.dev(size: 11))
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
        .background(Color.pageSurface)
        .overlay(alignment: .top) { Divider() }
        .transition(.move(edge: .bottom).combined(with: .opacity))
        .animation(reduceMotion ? nil : .easeInOut(duration: 0.2), value: serverManager.status == .stopped)
    }
}

// MARK: - Update banner

private struct UpdateBanner: View {
    let commit: String?
    let updating: Bool
    let onUpdate: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            if updating {
                ProgressView().scaleEffect(0.7).frame(width: 12, height: 12)
                Text("Updating — rebuilding from source, app will relaunch (log: \(UpdateChecker.updateLogPath))")
                    .font(.dev(size: 11))
                    .foregroundColor(.secondary)
            } else {
                Image(systemName: "arrow.down.circle.fill")
                    .font(.system(size: 11))
                    .foregroundColor(.accentColor)
                Text("Update available — \(commit.map { String($0.prefix(7)) } ?? "new commit") on remote")
                    .font(.dev(size: 11))
                    .foregroundColor(.secondary)
            }
            Spacer()
            if !updating {
                Button("Update & Relaunch", action: onUpdate)
                    .buttonStyle(.bordered)
                    .controlSize(.small)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 7)
        .background(Color.pageSurface)
        .overlay(alignment: .top) { Divider() }
        .transition(.move(edge: .bottom).combined(with: .opacity))
    }
}

// MARK: - Empty state

struct EmptyStateView: View {
    let onAdd: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("NO INTEGRATION SELECTED")
                .font(.dev(size: 10, weight: .semibold))
                .foregroundStyle(.secondary)
                .tracking(0.45)
            Text("Connect a service to get started")
                .font(.system(size: 20, weight: .semibold))
                .textSelection(.disabled)
            Text("Add a database, Linear workspace, or another local MCP endpoint. Pluk keeps the server and policy controls on this Mac.")
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
                .lineSpacing(2)
                .frame(maxWidth: 430, alignment: .leading)
            Button("New Integration", action: onAdd)
                .buttonStyle(.borderedProminent)
                .controlSize(.regular)
                .keyboardShortcut("n", modifiers: .command)
                .padding(.top, 6)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
        .padding(32)
    }
}

#if DEBUG
#Preview {
    ContentView(store: .preview, serverManager: .preview, updateChecker: .preview)
}
#endif
