import SwiftUI

// Edit a group: name, environment, and which integrations it fronts. Mirrors the
// connection add/edit sheet. For each included integration you can override
// config fields (e.g. a Linear `team_key`) scoped to this group; blank = inherit.
struct GroupFormView: View {
    let group: ConnectionGroup
    let connections: [Connection]
    let adapters: [AdapterManifest]
    let onSave: (ConnectionGroup) -> Void
    let onCancel: () -> Void

    @State private var name: String
    @State private var environment: Environment?
    @State private var included: Set<String>
    @State private var overrides: [String: [String: String]]  // connId → field → value
    @FocusState private var nameFocused: Bool

    private var trimmedName: String { name.trimmingCharacters(in: .whitespacesAndNewlines) }

    init(
        group: ConnectionGroup,
        connections: [Connection],
        adapters: [AdapterManifest],
        onSave: @escaping (ConnectionGroup) -> Void,
        onCancel: @escaping () -> Void
    ) {
        self.group = group
        self.connections = connections
        self.adapters = adapters
        self.onSave = onSave
        self.onCancel = onCancel
        _name = State(initialValue: group.name)
        _environment = State(initialValue: group.environment)
        _included = State(initialValue: Set(group.members.map(\.id)))
        _overrides = State(initialValue: Dictionary(uniqueKeysWithValues: group.members.map { ($0.id, $0.overrides) }))
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Edit Group").font(.system(size: 15, weight: .semibold))
                Spacer()
            }
            .padding(.horizontal, 18)
            .padding(.vertical, 14)
            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    field("Name") {
                        TextField("Group name", text: $name)
                            .textFieldStyle(.roundedBorder)
                            .focused($nameFocused)
                            .onSubmit { if !trimmedName.isEmpty { save() } }
                    }
                    field("Environment") {
                        Picker("", selection: $environment) {
                            Text("Any (mixed)").tag(Environment?.none)
                            ForEach(Environment.allCases, id: \.self) { Text($0.label).tag(Environment?.some($0)) }
                        }
                        .labelsHidden()
                        .frame(width: 160)
                    }
                    field("Integrations") {
                        if connections.isEmpty {
                            Text("No integrations yet.")
                                .font(.system(size: 12))
                                .foregroundColor(.secondary)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.horizontal, 12)
                                .padding(.vertical, 10)
                                .cardSurface()
                        } else {
                            VStack(spacing: 0) {
                                ForEach(connections) { conn in
                                    memberRow(conn)
                                    if conn.id != connections.last?.id {
                                        Divider().padding(.leading, 30)
                                    }
                                }
                            }
                            .cardSurface()
                        }
                    }
                }
                .padding(18)
            }

            Divider()
            HStack {
                Spacer()
                Button("Cancel", action: onCancel).keyboardShortcut(.cancelAction)
                Button("Save", action: save)
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(trimmedName.isEmpty)
            }
            .padding(.horizontal, 18)
            .padding(.vertical, 14)
        }
        .glassPanelBackground()
        .frame(width: 480, height: 580)
        .onAppear { DispatchQueue.main.async { nameFocused = true } }
    }

    @ViewBuilder
    private func memberRow(_ conn: Connection) -> some View {
        let on = included.contains(conn.id)
        let fields = overridableFields(for: conn)

        VStack(alignment: .leading, spacing: 0) {
            Button {
                if on { included.remove(conn.id) } else { included.insert(conn.id) }
            } label: {
                HStack(spacing: 9) {
                    Image(systemName: on ? "checkmark.circle.fill" : "circle")
                        .font(.system(size: 14))
                        .foregroundStyle(on ? Color.accentColor : Color.secondary)
                    TypeBadge(type: conn.type)
                    Text(conn.name).font(.system(size: 13)).lineLimit(1)
                    EnvTag(environment: conn.environment)
                    Spacer()
                }
                .contentShape(Rectangle())
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
            }
            .buttonStyle(.plain)

            if on && !fields.isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    Text("Overrides for this group (blank = inherit)")
                        .font(.system(size: 10))
                        .foregroundStyle(.tertiary)
                    ForEach(fields) { f in
                        HStack(spacing: 8) {
                            Text(f.label)
                                .font(.system(size: 11))
                                .foregroundColor(.secondary)
                                .frame(width: 110, alignment: .leading)
                            TextField(inheritPlaceholder(conn, f), text: binding(conn.id, f.key))
                                .textFieldStyle(.roundedBorder)
                                .font(.system(size: 12))
                        }
                    }
                }
                .padding(.leading, 30)
                .padding(.trailing, 12)
                .padding(.bottom, 10)
            }
        }
    }

    // MARK: - Helpers

    private func overridableFields(for conn: Connection) -> [ConfigFieldDef] {
        let adapter = adapters.first { $0.id == conn.type }
        // Secrets aren't overridable here (avoid duplicating credentials per group).
        return adapter?.configFields.filter { !($0.secret ?? false) } ?? []
    }

    private func inheritPlaceholder(_ conn: Connection, _ f: ConfigFieldDef) -> String {
        if let current = conn.config[f.key], !current.isEmpty { return "inherit (\(current))" }
        return f.placeholder ?? "inherit"
    }

    private func binding(_ connId: String, _ key: String) -> Binding<String> {
        Binding(
            get: { overrides[connId]?[key] ?? "" },
            set: { newVal in
                var m = overrides[connId] ?? [:]
                let trimmed = newVal.trimmingCharacters(in: .whitespaces)
                if trimmed.isEmpty { m.removeValue(forKey: key) } else { m[key] = newVal }
                overrides[connId] = m
            }
        )
    }

    private func save() {
        var g = group
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        g.name = trimmed.isEmpty ? group.name : trimmed
        g.environment = environment
        // Preserve connection order for stable namespacing; keep only non-empty overrides.
        g.members = connections.compactMap { conn in
            guard included.contains(conn.id) else { return nil }
            let ov = (overrides[conn.id] ?? [:]).filter { !$0.value.isEmpty }
            return GroupMember(id: conn.id, overrides: ov)
        }
        onSave(g)
    }

    @ViewBuilder
    private func field<Content: View>(_ label: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(label)
                .font(.system(size: 11, weight: .semibold))
                .foregroundColor(.secondary)
                .textCase(.uppercase)
            content()
        }
    }
}
