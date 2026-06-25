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
        }
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
        if !resetConfig {
            for f in m.configFields where f.defaultValue != nil && (draft.config[f.key] ?? "").isEmpty {
                draft.config[f.key] = f.defaultValue
            }
        }
        draft.adopt(m, resetConfig: resetConfig)
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
                        .defaultFocus($nameFocused, !picking)
                        .onSubmit { if canSave { onSave(draft) } }
                }
                row("Type") {
                    TypeBadge(type: draft.type)
                    Text(manifest?.label ?? draft.type.capitalized)
                        .font(.dev(size: 12))
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

                toolsSection
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
                SecureField(f.placeholder ?? "••••••", text: textBinding(f.key))
                    .textFieldStyle(.plain)
                    .font(.dev(size: 12))
            }
        case "select":
            row(f.label) {
                Picker("", selection: textBinding(f.key)) {
                    ForEach(f.options ?? [], id: \.value) { opt in
                        Text(opt.label)
                            .font(.dev(size: 12))
                            .tag(opt.value)
                    }
                }
                .pickerStyle(.menu)
                .font(.dev(size: 12))
                .frame(maxWidth: 200, alignment: .leading)
            }
        case "file":
            row(f.label) {
                HStack {
                    TextField(f.placeholder ?? "", text: textBinding(f.key))
                        .textFieldStyle(.plain)
                        .font(.dev(size: 12))
                    browseButton(title: "Choose…", types: f.fileTypes ?? []) { draft.config[f.key] = $0 }
                }
            }
        case "number":
            row(f.label) {
                TextField(f.placeholder ?? "", text: textBinding(f.key))
                    .textFieldStyle(.plain)
                    .font(.dev(size: 12))
                    .frame(width: 90)
                Spacer(minLength: 0)
            }
        default: // text
            row(f.label) {
                TextField(f.placeholder ?? "", text: textBinding(f.key))
                    .textFieldStyle(.plain)
                    .font(.dev(size: 12))
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

    // MARK: - Tools section (every adapter)

    // The unified policy UI: a list of the adapter's tools, each toggled on/off,
    // each with its own settings shown when enabled. Replaces the old per-policy
    // sections (SQL statement categories, action read/write, SSH confirm note).
    private var toolsSection: some View {
        DetailSection("Tools") {
            VStack(alignment: .leading, spacing: 0) {
                Text("Turn off tools to shrink what the agent sees. Expand an enabled tool to set how it behaves.")
                    .font(.system(size: 11))
                    .foregroundColor(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 14)
                    .padding(.top, 8)
                    .padding(.bottom, 4)

                ForEach(draft.tools) { tool in
                    toolRow(tool)
                    if tool.id != draft.tools.last?.id {
                        Divider().padding(.leading, 36)
                    }
                }
            }
            .padding(.bottom, 6)
        }
    }

    @ViewBuilder
    private func toolRow(_ tool: AdapterToolDef) -> some View {
        let enabled = draft.toolConfig[tool.name]?.enabled ?? tool.defaultEnabled
        let hasSettings = !(tool.settings ?? []).isEmpty
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .top, spacing: 10) {
                Toggle("", isOn: toolEnabledBinding(tool))
                    .toggleStyle(.checkbox)
                    .labelsHidden()
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text(tool.name)
                            .font(.dev(size: 12, weight: .medium))
                            .foregroundColor(enabled ? .primary : .secondary)
                        ToolCategoryTag(category: tool.category)
                        if hasSettings, enabled {
                            Image(systemName: "slider.horizontal.3")
                                .font(.system(size: 9))
                                .foregroundStyle(.tertiary)
                        }
                    }
                    Text(tool.description)
                        .font(.system(size: 11))
                        .foregroundColor(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 8)

            if enabled, hasSettings {
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(tool.settings ?? []) { setting in
                        settingRow(tool: tool, setting: setting)
                    }
                }
                .padding(.leading, 38)
                .padding(.trailing, 14)
                .padding(.bottom, 10)
            }
        }
    }

    @ViewBuilder
    private func settingRow(tool: AdapterToolDef, setting: ConfigFieldDef) -> some View {
        let def = setting.defaultValue ?? ""
        VStack(alignment: .leading, spacing: 3) {
            switch setting.type {
            case "toggle":
                let isOn = (draft.toolConfig[tool.name]?.settings[setting.key] ?? def) == "true"
                Toggle(setting.label, isOn: settingBoolBinding(tool, setting.key))
                    .toggleStyle(.checkbox)
                    .font(.system(size: 12))
                    .foregroundColor(setting.danger == true && isOn ? .red : .primary)
            case "select":
                HStack {
                    Text(setting.label).font(.system(size: 12)).frame(width: 120, alignment: .leading)
                    Picker("", selection: settingTextBinding(tool, setting.key, default: def)) {
                        ForEach(setting.options ?? [], id: \.value) { opt in
                            Text(opt.label).font(.dev(size: 12)).tag(opt.value)
                        }
                    }
                    .pickerStyle(.menu).labelsHidden().frame(maxWidth: 240, alignment: .leading)
                    Spacer(minLength: 0)
                }
            case "number":
                HStack {
                    Text(setting.label).font(.system(size: 12)).frame(width: 120, alignment: .leading)
                    TextField(def, text: settingTextBinding(tool, setting.key, default: def))
                        .textFieldStyle(.plain).font(.dev(size: 12)).frame(width: 90)
                    Spacer(minLength: 0)
                }
            default: // text / password
                HStack {
                    Text(setting.label).font(.system(size: 12)).frame(width: 120, alignment: .leading)
                    TextField(setting.placeholder ?? "", text: settingTextBinding(tool, setting.key, default: def))
                        .textFieldStyle(.plain).font(.dev(size: 12))
                }
            }
            if let help = setting.help {
                Text(help)
                    .font(.system(size: 10))
                    .foregroundColor(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.leading, setting.type == "toggle" ? 20 : 124)
            }
        }
    }

    // MARK: - Tool-config bindings

    private func toolEnabledBinding(_ tool: AdapterToolDef) -> Binding<Bool> {
        Binding(
            get: { draft.toolConfig[tool.name]?.enabled ?? tool.defaultEnabled },
            set: { on in
                var state = draft.toolConfig[tool.name] ?? tool.seededState()
                state.enabled = on
                draft.toolConfig[tool.name] = state
            }
        )
    }

    private func settingTextBinding(_ tool: AdapterToolDef, _ key: String, default def: String) -> Binding<String> {
        Binding(
            get: { draft.toolConfig[tool.name]?.settings[key] ?? def },
            set: { value in
                var state = draft.toolConfig[tool.name] ?? tool.seededState()
                state.settings[key] = value
                draft.toolConfig[tool.name] = state
            }
        )
    }

    private func settingBoolBinding(_ tool: AdapterToolDef, _ key: String) -> Binding<Bool> {
        Binding(
            get: { (draft.toolConfig[tool.name]?.settings[key] ?? "false") == "true" },
            set: { on in
                var state = draft.toolConfig[tool.name] ?? tool.seededState()
                state.settings[key] = on ? "true" : "false"
                draft.toolConfig[tool.name] = state
            }
        )
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

// A small colored tag for a tool's category (read / write / delete / admin), so
// the destructive tools stand out in the list.
struct ToolCategoryTag: View {
    let category: String

    private var color: Color {
        switch category {
        case "write": .orange
        case "delete", "admin": .red
        default: .secondary   // read / inspect
        }
    }

    var body: some View {
        Text(category)
            .font(.dev(size: 9, weight: .medium))
            .textCase(.uppercase)
            .foregroundColor(color)
            .padding(.horizontal, 5)
            .padding(.vertical, 1)
            .background(color.opacity(0.12))
            .clipShape(.capsule)
    }
}

#if DEBUG
#Preview {
    ConnectionFormView(
        editingConn: .sample,
        adapters: [.samplePostgres],
        onSave: { _ in },
        onCancel: {}
    )
}
#endif
