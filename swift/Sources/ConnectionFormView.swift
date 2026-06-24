import AppKit
import SwiftUI
import UniformTypeIdentifiers

struct ConnectionFormView: View {
    let editingConn: Connection?
    let adapters: [AdapterManifest]
    let adaptersLoadFailed: Bool
    let onRetryAdapters: () -> Void
    let onSave: (ConnectionDraft) -> Void
    let onCancel: () -> Void

    @State private var draft: ConnectionDraft
    @State private var manifest: AdapterManifest?
    @State private var picking: Bool
    @FocusState private var nameFocused: Bool

    init(
        editingConn: Connection?,
        adapters: [AdapterManifest],
        adaptersLoadFailed: Bool = false,
        onRetryAdapters: @escaping () -> Void = {},
        onSave: @escaping (ConnectionDraft) -> Void,
        onCancel: @escaping () -> Void
    ) {
        self.editingConn = editingConn
        self.adapters = adapters
        self.adaptersLoadFailed = adaptersLoadFailed
        self.onRetryAdapters = onRetryAdapters
        self.onSave = onSave
        self.onCancel = onCancel
        _draft = State(initialValue: editingConn.map(ConnectionDraft.init) ?? ConnectionDraft())
        _picking = State(initialValue: editingConn == nil)   // new → choose a type first
    }

    var body: some View {
        VStack(spacing: 0) {
            formHeader
            Divider()
            Group {
                if adapters.isEmpty {
                    if adaptersLoadFailed { adapterErrorView } else { loadingView }
                } else if picking {
                    ScrollView { typeChooser.padding(.horizontal, 18).padding(.vertical, 14) }
                } else {
                    ScrollView { formBody.padding(.horizontal, 18).padding(.vertical, 14) }
                }
            }
            Divider()
            formFooter
        }
        .glassPanelBackground()
        .onAppear {
            resolveInitialManifest()
            if !picking { focusName() }
        }
        .onChange(of: picking) { _, isPicking in
            if !isPicking { focusName() }
        }
    }

    private func focusName() {
        DispatchQueue.main.async { nameFocused = true }
    }

    // MARK: - Manifest resolution

    // Editing: resolve the existing adapter immediately. New: leave manifest nil
    // until the user picks a type in the chooser.
    private func resolveInitialManifest() {
        guard manifest == nil, !adapters.isEmpty, let conn = editingConn else { return }
        if let match = adapters.first(where: { $0.id == conn.type }) {
            select(match, resetConfig: false)
        }
    }

    private func select(_ m: AdapterManifest, resetConfig: Bool) {
        manifest = m
        draft.type = m.id
        draft.fields = m.configFields
        draft.policyKind = m.policyKind
        if resetConfig {
            var seeded: [String: String] = [:]
            for f in m.configFields where f.defaultValue != nil { seeded[f.key] = f.defaultValue }
            draft.config = seeded
        } else {
            for f in m.configFields where f.defaultValue != nil && (draft.config[f.key] ?? "").isEmpty {
                draft.config[f.key] = f.defaultValue
            }
        }
    }

    // MARK: - Header

    private var formHeader: some View {
        HStack {
            VStack(alignment: .leading, spacing: 3) {
                Text(editingConn != nil ? "Edit Integration" : "New Integration")
                    .font(.system(size: 15, weight: .semibold))
                Text(draft.name.isEmpty ? "Integration settings" : draft.name)
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
                    .lineLimit(1)
            }
            Spacer()
            Picker("", selection: Binding(
                get: { draft.environment },
                set: { draft.setEnvironment($0) }
            )) {
                ForEach(Environment.allCases, id: \.self) { env in
                    Text(env.label).tag(env)
                }
            }
            .pickerStyle(.menu)
            .frame(width: 130)
            .help("Environment — sets a safe default policy for new integrations")
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
    }

    // MARK: - Body

    private var loadingView: some View {
        VStack(spacing: 8) {
            ProgressView()
            Text("Loading adapters…").font(.system(size: 12)).foregroundColor(.secondary)
        }
        .frame(maxWidth: .infinity, minHeight: 200)
    }

    private var adapterErrorView: some View {
        VStack(spacing: 10) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 24, weight: .light))
                .foregroundStyle(.secondary)
            Text("Couldn't load adapters")
                .font(.system(size: 13, weight: .medium))
            Text("The local pluk server isn't responding. Make sure it's running, then retry.")
                .font(.system(size: 11))
                .foregroundColor(.secondary)
                .multilineTextAlignment(.center)
            Button("Retry", action: onRetryAdapters)
                .buttonStyle(.bordered)
                .controlSize(.small)
                .padding(.top, 2)
        }
        .padding(.horizontal, 24)
        .frame(maxWidth: .infinity, minHeight: 200)
    }

    // MARK: - Type chooser (shown when adding a new integration)

    private var typeChooser: some View {
        VStack(alignment: .leading, spacing: 16) {
            ForEach(groupedAdapters, id: \.category) { category, items in
                DetailSection(prettyCategory(category)) {
                    ForEach(items) { adapter in
                        Button { choose(adapter) } label: {
                            HStack(spacing: 10) {
                                TypeBadge(type: adapter.id)
                                Text(adapter.label).font(.system(size: 13))
                                Spacer()
                                Image(systemName: "chevron.right")
                                    .font(.system(size: 10, weight: .semibold))
                                    .foregroundStyle(.tertiary)
                            }
                            .padding(.horizontal, 10)
                            .padding(.vertical, 9)
                            .contentShape(Rectangle())
                            .overlay(alignment: .bottom) { Divider().padding(.leading, 44) }
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
    }

    private var groupedAdapters: [(category: String, items: [AdapterManifest])] {
        var order: [String] = []
        var byCategory: [String: [AdapterManifest]] = [:]
        for a in adapters {
            if byCategory[a.category] == nil { order.append(a.category) }
            byCategory[a.category, default: []].append(a)
        }
        return order.map { ($0, byCategory[$0] ?? []) }
    }

    private func prettyCategory(_ c: String) -> String {
        c.replacingOccurrences(of: "-", with: " ").capitalized
    }

    private func choose(_ adapter: AdapterManifest) {
        select(adapter, resetConfig: true)
        picking = false
    }

    // MARK: - Field form (shown after a type is chosen)

    @ViewBuilder
    private var formBody: some View {
        GlassGroup(spacing: 16) {
        VStack(alignment: .leading, spacing: 16) {
            DetailSection("General") {
                row("Name") {
                    TextField(namePlaceholder, text: $draft.name)
                        .textFieldStyle(.plain)
                        .focused($nameFocused)
                        .onSubmit { if canSave { onSave(draft) } }
                }
                row("Type") {
                    TypeBadge(type: draft.type)
                    Text(manifest?.label ?? draft.type.capitalized).font(.system(size: 12))
                    Spacer()
                    Button("Change") { picking = true }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                }
            }

            if let manifest {
                ForEach(manifest.groupedFields, id: \.group) { group, fields in
                    let shown = fields.filter(visible)
                    if !shown.isEmpty {
                        DetailSection(group) {
                            ForEach(shown) { field in fieldRow(field) }
                        }
                    }
                }

                if manifest.isSQL {
                    queryPolicySection
                } else if manifest.isAction {
                    actionPolicySection
                } else {
                    confirmPolicySection
                }
            }
        }
        }
    }

    private var namePlaceholder: String {
        switch manifest?.category {
        case "database": "My Prod DB"
        case "issue-tracker": "My Linear Workspace"
        default: "My \(manifest?.label ?? "Service")"
        }
    }

    // MARK: - Dynamic field rendering

    @ViewBuilder
    private func fieldRow(_ f: ConfigFieldDef) -> some View {
        switch f.type {
        case "toggle":
            row(f.label) {
                Toggle("", isOn: boolBinding(f.key)).toggleStyle(.checkbox)
                Spacer(minLength: 0)
            }
        case "password":
            row(f.label) {
                SecureField(f.placeholder ?? "••••••", text: textBinding(f.key)).textFieldStyle(.plain)
            }
        case "select":
            row(f.label) {
                Picker("", selection: textBinding(f.key)) {
                    ForEach(f.options ?? [], id: \.value) { opt in Text(opt.label).tag(opt.value) }
                }
                .pickerStyle(.menu)
                .frame(maxWidth: 200, alignment: .leading)
            }
        case "file":
            row(f.label) {
                HStack {
                    TextField(f.placeholder ?? "", text: textBinding(f.key)).textFieldStyle(.plain)
                    browseButton(title: "Choose…", types: f.fileTypes ?? []) { draft.config[f.key] = $0 }
                }
            }
        case "number":
            row(f.label) {
                TextField(f.placeholder ?? "", text: textBinding(f.key))
                    .textFieldStyle(.plain)
                    .frame(width: 90)
                Spacer(minLength: 0)
            }
        default: // text
            row(f.label) {
                TextField(f.placeholder ?? "", text: textBinding(f.key)).textFieldStyle(.plain)
            }
        }
    }

    private func visible(_ f: ConfigFieldDef) -> Bool {
        guard let s = f.showIf else { return true }
        return (draft.config[s.key] ?? "") == s.equals
    }

    private func textBinding(_ key: String) -> Binding<String> {
        Binding(get: { draft.config[key] ?? "" }, set: { draft.config[key] = $0 })
    }

    private func boolBinding(_ key: String) -> Binding<Bool> {
        Binding(get: { draft.config[key] == "true" }, set: { draft.config[key] = $0 ? "true" : "false" })
    }

    // MARK: - Action policy section (non-SQL adapters)

    // Tools the Write permission unlocks for the selected adapter (write + delete).
    private var writeActionNames: [String] { manifest?.writeActionNames ?? [] }

    private var actionPolicySection: some View {
        DetailSection("Permissions") {
            VStack(alignment: .leading, spacing: 6) {
                Toggle("Read", isOn: .constant(true))
                    .toggleStyle(.checkbox)
                    .font(.system(size: 12))
                    .disabled(true)
                    .help("Read access is always allowed")
                    .padding(.horizontal, 14)
                    .padding(.top, 8)

                Toggle("Write", isOn: $draft.allowWrite)
                    .toggleStyle(.checkbox)
                    .font(.system(size: 12))
                    .help(writeActionNames.isEmpty
                          ? "Allow the agent to modify data"
                          : "Allow the agent to run: \(writeActionNames.joined(separator: ", "))")
                    .padding(.horizontal, 14)

                Text(draft.allowWrite ? writeOnSummary : readOnlySummary)
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
                    .padding(.horizontal, 14)
                    .padding(.bottom, 8)

                // Preview the tools that won't be exposed to the agent in read-only
                // mode — these are filtered out of the MCP server entirely, not just
                // blocked on call.
                if !draft.allowWrite, !writeActionNames.isEmpty {
                    HiddenToolsNote(names: writeActionNames)
                        .padding(.horizontal, 14)
                        .padding(.bottom, 8)
                }
            }
        }
    }

    private var writeOnSummary: String {
        writeActionNames.isEmpty
            ? "Agent can read and modify data."
            : "Agent can read, plus modify via: \(writeActionNames.joined(separator: ", "))."
    }

    private var readOnlySummary: String {
        "Agent can only read. Write actions are blocked."
    }

    // MARK: - Confirmation policy section (no-policy adapters, e.g. SSH)

    private var confirmPolicySection: some View {
        DetailSection("Policy") {
            HStack(alignment: .top, spacing: 8) {
                Image(systemName: "hand.raised.fill")
                    .foregroundColor(.secondary)
                    .font(.system(size: 12))
                Text("Commands run unrestricted as the connecting user. Every command must be confirmed in your agent client before it runs.")
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
        }
    }

    // MARK: - Query policy section (SQL adapters)

    private var queryPolicySection: some View {
        DetailSection("Query Policy") {
            VStack(alignment: .leading, spacing: 12) {
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text("Preset")
                            .font(.system(size: 12, weight: .medium))
                            .frame(width: 80, alignment: .leading)
                        Picker("", selection: Binding(
                            get: { draft.queryPolicy.preset },
                            set: { draft.queryPolicy.apply(preset: $0) }
                        )) {
                            ForEach(QueryPreset.allCases) { p in
                                Text(p.label).tag(p)
                            }
                        }
                        .pickerStyle(.menu)
                        .frame(maxWidth: 160, alignment: .leading)
                    }
                    Text(draft.queryPolicy.preset.description)
                        .font(.system(size: 11))
                        .foregroundColor(.secondary)
                        .padding(.leading, 88)

                    if draft.queryPolicy.preset == .unrestricted {
                        HStack(spacing: 4) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundColor(.red)
                                .font(.system(size: 11))
                            Text("Unrestricted allows all SQL, including DROP, TRUNCATE, and filesystem ops.")
                                .font(.system(size: 11))
                                .foregroundColor(.red)
                        }
                        .padding(.leading, 88)
                    }
                }
                .padding(.horizontal, 10)
                .padding(.top, 8)
                .padding(.bottom, 2)

                Divider()

                let groups: [(String, [StatementCategory])] = [
                    ("Read",   [.select, .inspect]),
                    ("Write",  [.insert, .update, .delete, .merge]),
                    ("Schema", [.create, .alter, .drop, .truncate, .rename]),
                    ("Admin",  statementAdminCategories),
                ]

                ForEach(groups, id: \.0) { groupName, categories in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(groupName)
                            .font(.system(size: 10, weight: .semibold))
                            .foregroundColor(.secondary)
                            .textCase(.uppercase)
                            .padding(.horizontal, 10)
                            .padding(.top, 6)
                        ForEach(categories) { cat in
                            HStack {
                                Toggle(cat.label, isOn: Binding(
                                    get: { draft.queryPolicy.allowed.contains(cat) },
                                    set: { _ in draft.queryPolicy.toggle(cat) }
                                ))
                                .toggleStyle(.checkbox)
                                .font(.system(size: 12))
                            }
                            .padding(.horizontal, 14)
                            .padding(.vertical, 2)
                        }
                    }
                }

                Divider()

                VStack(alignment: .leading, spacing: 6) {
                    Text("Guards")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundColor(.secondary)
                        .textCase(.uppercase)
                        .padding(.horizontal, 10)
                        .padding(.top, 4)

                    Toggle("Block stacked statements (SELECT 1; DROP …)", isOn: $draft.queryPolicy.blockStacked)
                        .toggleStyle(.checkbox)
                        .font(.system(size: 12))
                        .help("Reject queries that contain more than one SQL statement")
                        .padding(.horizontal, 14)

                    Toggle("Require WHERE on UPDATE / DELETE", isOn: $draft.queryPolicy.requireWhere)
                        .toggleStyle(.checkbox)
                        .font(.system(size: 12))
                        .help("Block UPDATE or DELETE without a WHERE clause")
                        .padding(.horizontal, 14)

                    Toggle("Allow filesystem / COPY ops", isOn: $draft.queryPolicy.allowFilesystem)
                        .toggleStyle(.checkbox)
                        .font(.system(size: 12))
                        .foregroundColor(draft.queryPolicy.allowFilesystem ? .red : .primary)
                        .help("Allow COPY … PROGRAM, INTO OUTFILE, LOAD DATA, ATTACH DATABASE, pg_read_file")
                        .padding(.horizontal, 14)

                    HStack {
                        Toggle("Max rows returned", isOn: Binding(
                            get: { draft.queryPolicy.maxRows != nil },
                            set: { on in draft.queryPolicy.maxRows = on ? 1000 : nil }
                        ))
                        .toggleStyle(.checkbox)
                        .font(.system(size: 12))
                        .frame(width: 160)

                        if draft.queryPolicy.maxRows != nil {
                            TextField("1000", value: Binding(
                                get: { draft.queryPolicy.maxRows ?? 1000 },
                                set: { draft.queryPolicy.maxRows = max(1, $0) }
                            ), format: .number)
                            .textFieldStyle(.plain)
                            .frame(width: 80)
                            Text("rows")
                                .font(.system(size: 12))
                                .foregroundColor(.secondary)
                        }
                    }
                    .padding(.horizontal, 14)
                }
                .padding(.bottom, 8)
                .onChange(of: draft.queryPolicy.blockStacked) { _, _ in markCustom() }
                .onChange(of: draft.queryPolicy.requireWhere) { _, _ in markCustom() }
                .onChange(of: draft.queryPolicy.allowFilesystem) { _, _ in markCustom() }
                .onChange(of: draft.queryPolicy.maxRows) { _, _ in markCustom() }
            }
        }
    }

    // Admin categories depend on driver type (SQLite has no GRANT)
    private var statementAdminCategories: [StatementCategory] {
        var cats: [StatementCategory] = [.transaction, .session, .procedure, .maintenance]
        if draft.type != "sqlite" { cats.append(.grant) }
        return cats
    }

    private func markCustom() {
        if draft.queryPolicy.preset != .custom {
            draft.queryPolicy.preset = .custom
        }
    }

    // MARK: - Footer

    private var formFooter: some View {
        HStack {
            Spacer()
            Button("Cancel", action: onCancel).buttonStyle(.bordered)
            if !picking {
                Button(editingConn != nil ? "Save" : "Add") { onSave(draft) }
                    .buttonStyle(.borderedProminent)
                    .disabled(!canSave)
            }
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
    }

    private var canSave: Bool {
        if draft.name.trimmingCharacters(in: .whitespaces).isEmpty { return false }
        for f in draft.fields where (f.required == true) && visible(f) {
            if (draft.config[f.key] ?? "").isEmpty { return false }
        }
        return true
    }

    // MARK: - Layout helpers

    // Form rows are wider than the read-only inspector rows; reuse the shared
    // template (Glass.swift) so the layout stays in one place.
    private func row<C: View>(_ label: String, @ViewBuilder content: () -> C) -> some View {
        InspectorRow(label, labelWidth: 104, dividerInset: 124, content: content)
    }

    private func browseButton(title: String, types: [String], onPick: @escaping (String) -> Void) -> some View {
        Button(title) {
            let panel = NSOpenPanel()
            panel.allowsMultipleSelection = false
            panel.canChooseDirectories = false
            panel.canChooseFiles = true
            if !types.isEmpty {
                panel.allowedContentTypes = types.compactMap { UTType(filenameExtension: $0) }
            }
            if panel.runModal() == .OK, let url = panel.url {
                onPick(url.path)
            }
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
    }
}

// Read-only preview: the tools that are filtered out of the MCP server (not just
// blocked) because the integration grants read only. Shows the agent never sees them.
private struct HiddenToolsNote: View {
    let names: [String]

    var body: some View {
        HStack(alignment: .top, spacing: 6) {
            Image(systemName: "eye.slash")
                .font(.system(size: 10))
                .foregroundColor(.secondary)
            Text("Hidden from the agent: \(names.joined(separator: ", "))")
                .font(.system(size: 11))
                .foregroundColor(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
