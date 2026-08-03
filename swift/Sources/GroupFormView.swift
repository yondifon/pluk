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
                Text("Edit Group").scaledFont(.title2, weight: .semibold)
                Spacer()
            }
            .padding(.horizontal, Space.xl)
            .padding(.vertical, Space.lg)
            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: Space.lg) {
                    field("Name") {
                        TextField("Group name", text: $name)
                            .textFieldStyle(.plain)
                            .focused($nameFocused)
                            .defaultFocus($nameFocused, true)
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
                                .scaledFont(.callout)
                                .foregroundColor(.secondary)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.horizontal, Space.md)
                                .padding(.vertical, Space.md)
                                .cardSurface()
                        } else {
                            VStack(spacing: 0) {
                                ForEach(connections) { conn in
                                    memberRow(conn)
                                    if conn.id != connections.last?.id {
                                        Rectangle().fill(Color.hairline).frame(height: 0.5).padding(.leading, Space.xxl - 2)
                                    }
                                }
                            }
                            .cardSurface()
                        }
                    }
                }
                .padding(Space.xl)
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
            .padding(.horizontal, Space.xl)
            .padding(.vertical, Space.lg)
        }
        .background(Surface.content.ignoresSafeArea())
        .frame(width: 480, height: 580)
    }

    @ViewBuilder
    private func memberRow(_ conn: Connection) -> some View {
        let on = included.contains(conn.id)
        let fields = overridableFields(for: conn)

        VStack(alignment: .leading, spacing: 0) {
            Button {
                if on { included.remove(conn.id) } else { included.insert(conn.id) }
            } label: {
                HStack(spacing: Space.sm + 1) {
                    Image(systemName: on ? "checkmark.circle.fill" : "circle")
                        .scaledFont(.body)
                        .foregroundStyle(on ? Color.accentColor : Color.secondary)
                    TypeBadge(type: conn.type)
                    Text(conn.name).scaledFont(.body).lineLimit(1)
                    EnvTag(environment: conn.environment)
                    Spacer()
                }
                .contentShape(Rectangle())
                .padding(.horizontal, Space.md)
                .padding(.vertical, Space.sm)
            }
            .buttonStyle(.plain)

            if on && !fields.isEmpty {
                VStack(alignment: .leading, spacing: Space.sm) {
                    Text("Overrides for this group (blank = inherit)")
                        .scaledFont(.caption)
                        .foregroundStyle(.tertiary)
                    ForEach(fields) { f in
                        HStack(spacing: Space.sm) {
                            Text(f.label)
                                .scaledFont(.caption)
                                .foregroundColor(.secondary)
                                .frame(width: 110, alignment: .leading)
                            TextField(inheritPlaceholder(conn, f), text: binding(conn.id, f.key))
                                .textFieldStyle(.plain)
                                .font(.mono(12))
                        }
                    }
                }
                .padding(.leading, Space.xxl - 2)
                .padding(.trailing, Space.md)
                .padding(.bottom, Space.md)
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
        HStack(alignment: .top, spacing: Space.md) {
            Text(label)
                .scaledFont(.caption, weight: .semibold)
                .foregroundColor(.secondary)
                .frame(width: 90, alignment: .leading)
                .padding(.top, Space.xxs)
            content()
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

#if DEBUG
#Preview {
    GroupFormView(
        group: .sample,
        connections: [.sample, .sampleGroupMember],
        adapters: [.samplePostgres, .sampleLinear],
        onSave: { _ in },
        onCancel: {}
    )
}
#endif
