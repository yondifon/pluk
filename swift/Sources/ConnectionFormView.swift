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
                .background(Surface.panel)
            Group {
                if adapters.isEmpty {
                    if adaptersLoadFailed { adapterErrorView } else { loadingView }
                } else if picking {
                    ScrollView { typeChooser.padding(.horizontal, Space.xl).padding(.vertical, Space.lg) }
                } else {
                    ScrollView { formBody.padding(.horizontal, Space.xl).padding(.vertical, Space.lg) }
                }
            }
            formFooter
                .background(Surface.panel)
        }
        .background(Surface.content.ignoresSafeArea())
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
            VStack(alignment: .leading, spacing: Space.xxs) {
                Text(editingConn != nil ? "Edit Integration" : "New Integration")
                    .scaledFont(.title2, weight: .semibold)
                Text(draft.name.isEmpty ? "Integration settings" : draft.name)
                    .scaledFont(.caption)
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
        .padding(.horizontal, Space.xl)
        .padding(.vertical, Space.md)
    }

    // MARK: - Body

    private var loadingView: some View {
        VStack(spacing: Space.sm) {
            ProgressView()
            Text("Loading adapters…").scaledFont(.caption).foregroundColor(.secondary)
        }
        .frame(maxWidth: .infinity, minHeight: 200)
    }

    private var adapterErrorView: some View {
        VStack(spacing: Space.md) {
            Image(systemName: "exclamationmark.triangle")
                .font(.system(size: 24, weight: .light))
                .foregroundStyle(.secondary)
            Text("Couldn't load adapters")
                .scaledFont(.headline)
            Text("The local pluk server isn't responding. Make sure it's running, then retry.")
                .scaledFont(.caption)
                .foregroundColor(.secondary)
                .multilineTextAlignment(.center)
            Button("Retry", action: onRetryAdapters)
                .buttonStyle(.bordered)
                .controlSize(.small)
                .padding(.top, Space.xxs)
        }
        .padding(.horizontal, Space.xl)
        .frame(maxWidth: .infinity, minHeight: 200)
    }

    // MARK: - Type chooser (shown when adding a new integration)

    private var typeChooser: some View {
        VStack(alignment: .leading, spacing: Space.lg) {
            ForEach(groupedAdapters, id: \.category) { category, items in
                DetailSection(prettyCategory(category)) {
                    ForEach(items) { adapter in
                        Button { choose(adapter) } label: {
                            HStack(spacing: Space.md) {
                                TypeBadge(type: adapter.id)
                                Text(adapter.label).scaledFont(.body)
                                Spacer()
                                Image(systemName: "chevron.right")
                                    .font(.system(size: 10, weight: .semibold))
                                    .foregroundStyle(Surface.tertiaryLabel)
                            }
                            .padding(.horizontal, Space.md)
                            .padding(.vertical, Space.sm)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.pointer)
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
        GlassGroup(spacing: Space.lg) {
        VStack(alignment: .leading, spacing: Space.lg) {
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
                        .scaledFont(.callout)
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
                    .scaledFont(.callout, design: .monospaced)
            }
        case "select":
            row(f.label) {
                Picker("", selection: textBinding(f.key)) {
                    ForEach(f.options ?? [], id: \.value) { opt in
                        Text(opt.label)
                            .scaledFont(.callout)
                            .tag(opt.value)
                    }
                }
                .pickerStyle(.menu)
                .scaledFont(.callout)
                .frame(maxWidth: 200, alignment: .leading)
            }
        case "file":
            row(f.label) {
                HStack {
                    TextField(f.placeholder ?? "", text: textBinding(f.key))
                        .textFieldStyle(.plain)
                        .scaledFont(.callout, design: .monospaced)
                    browseButton(title: "Choose…", types: f.fileTypes ?? []) { draft.config[f.key] = $0 }
                }
            }
        case "number":
            row(f.label) {
                TextField(f.placeholder ?? "", text: textBinding(f.key))
                    .textFieldStyle(.plain)
                    .scaledFont(.callout, design: .monospaced)
                    .frame(width: 90)
                Spacer(minLength: 0)
            }
        default: // text
            row(f.label) {
                TextField(f.placeholder ?? "", text: textBinding(f.key))
                    .textFieldStyle(.plain)
                    .scaledFont(.callout, design: .monospaced)
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
        let enabledCount = draft.tools.filter { draft.toolConfig[$0.name]?.enabled ?? $0.defaultEnabled }.count
        // Grouped by default state, not current on/off, so a row never jumps
        // between sections when you toggle it — you can enable several in a row.
        let defaults = draft.tools.filter { $0.defaultEnabled }
        let extras = draft.tools.filter { !$0.defaultEnabled }
        return DetailSection("Tools") {
            VStack(alignment: .leading, spacing: 0) {
                Text("\(enabledCount) of \(draft.tools.count) on. Enable tools to give the agent more, disable to shrink what it sees. Expand an enabled tool to configure it.")
                    .scaledFont(.caption)
                    .foregroundColor(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, Space.lg)
                    .padding(.top, Space.sm)
                    .padding(.bottom, Space.xs)

                toolRows(defaults)

                if !extras.isEmpty {
                    VStack(alignment: .leading, spacing: Space.xxs) {
                        Text("More tools")
                            .scaledFont(.caption, weight: .semibold)
                            .foregroundColor(.primary)
                        Text("Off by default — enable the ones you need.")
                            .scaledFont(.caption)
                            .foregroundColor(.secondary)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, Space.lg)
                    .padding(.top, Space.sm)
                    .padding(.bottom, Space.sm)

                    toolRows(extras)
                }
            }
            .padding(.bottom, Space.sm)
        }
    }

    @ViewBuilder
    private func toolRows(_ tools: [AdapterToolDef]) -> some View {
        ForEach(tools) { tool in
            toolRow(tool)
        }
    }

    @ViewBuilder
    private func toolRow(_ tool: AdapterToolDef) -> some View {
        let enabled = draft.toolConfig[tool.name]?.enabled ?? tool.defaultEnabled
        let hasSettings = !(tool.settings ?? []).isEmpty
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .top, spacing: Space.md) {
                Toggle("", isOn: toolEnabledBinding(tool))
                    .toggleStyle(.checkbox)
                    .labelsHidden()
                VStack(alignment: .leading, spacing: Space.xxs) {
                    HStack(spacing: Space.sm) {
                        Text(tool.name)
                            .scaledFont(.callout, weight: .medium, design: .monospaced)
                            .foregroundColor(enabled ? .primary : .secondary)
                        ToolCategoryTag(category: tool.category)
                        if hasSettings, enabled {
                            Image(systemName: "slider.horizontal.3")
                                .font(.system(size: 10))
                                .foregroundStyle(Surface.tertiaryLabel)
                        }
                    }
                    Text(tool.description)
                        .scaledFont(.caption)
                        .foregroundColor(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, Space.lg)
            .padding(.vertical, Space.sm)

            if enabled, hasSettings {
                VStack(alignment: .leading, spacing: Space.md) {
                    ForEach(tool.settings ?? []) { setting in
                        settingRow(tool: tool, setting: setting)
                    }
                }
                .padding(.leading, Space.xxl)
                .padding(.trailing, Space.lg)
                .padding(.bottom, Space.md)
            }
        }
    }

    @ViewBuilder
    private func settingRow(tool: AdapterToolDef, setting: ConfigFieldDef) -> some View {
        let def = setting.defaultValue ?? ""
        VStack(alignment: .leading, spacing: Space.xxs) {
            switch setting.type {
            case "toggle":
                let isOn = (draft.toolConfig[tool.name]?.settings[setting.key] ?? def) == "true"
                Toggle(setting.label, isOn: settingBoolBinding(tool, setting.key))
                    .toggleStyle(.checkbox)
                    .scaledFont(.callout)
                    .foregroundColor(setting.danger == true && isOn ? .red : .primary)
            case "select":
                HStack {
                    Text(setting.label).scaledFont(.callout).frame(width: 120, alignment: .leading)
                    Picker("", selection: settingTextBinding(tool, setting.key, default: def)) {
                        ForEach(setting.options ?? [], id: \.value) { opt in
                            Text(opt.label).scaledFont(.callout).tag(opt.value)
                        }
                    }
                    .pickerStyle(.menu).labelsHidden().frame(maxWidth: 240, alignment: .leading)
                    Spacer(minLength: 0)
                }
            case "number":
                HStack {
                    Text(setting.label).scaledFont(.callout).frame(width: 120, alignment: .leading)
                    TextField(def, text: settingTextBinding(tool, setting.key, default: def))
                        .textFieldStyle(.plain).scaledFont(.callout, design: .monospaced).frame(width: 90)
                    Spacer(minLength: 0)
                }
            default: // text / password
                HStack {
                    Text(setting.label).scaledFont(.callout).frame(width: 120, alignment: .leading)
                    TextField(setting.placeholder ?? "", text: settingTextBinding(tool, setting.key, default: def))
                        .textFieldStyle(.plain).scaledFont(.callout, design: .monospaced)
                }
            }
            if let help = setting.help {
                Text(help)
                    .scaledFont(.caption)
                    .foregroundColor(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.leading, setting.type == "toggle" ? Space.xl : 124)
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
        .padding(.horizontal, Space.xl)
        .padding(.vertical, Space.md)
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
        InspectorRow(label, labelWidth: 104, content: content)
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
            .scaledFont(.caption)
            .foregroundColor(color)
            .padding(.horizontal, Space.xs)
            .padding(.vertical, Space.xxs)
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
