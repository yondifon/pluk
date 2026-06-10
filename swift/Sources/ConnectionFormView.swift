import AppKit
import SwiftUI
import UniformTypeIdentifiers

struct ConnectionFormView: View {
    let editingConn: Connection?
    let onSave: (ConnectionDraft) -> Void
    let onCancel: () -> Void

    @State private var draft: ConnectionDraft

    init(editingConn: Connection?, onSave: @escaping (ConnectionDraft) -> Void, onCancel: @escaping () -> Void) {
        self.editingConn = editingConn
        self.onSave = onSave
        self.onCancel = onCancel
        _draft = State(initialValue: editingConn.map(ConnectionDraft.init) ?? ConnectionDraft())
    }

    var body: some View {
        VStack(spacing: 0) {
            formHeader
            Divider()
            ScrollView { formBody.padding(.horizontal, 18).padding(.vertical, 14) }
            Divider()
            formFooter
        }
    }

    // MARK: - Header

    private var formHeader: some View {
        HStack {
            VStack(alignment: .leading, spacing: 3) {
                Text(editingConn != nil ? "Edit Connection" : "New Connection")
                    .font(.system(size: 15, weight: .semibold))
                Text(draft.name.isEmpty ? "Connection settings" : draft.name)
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
            .help("Environment — sets default query policy for new connections")
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
    }

    // MARK: - Body sections

    private var formBody: some View {
        VStack(alignment: .leading, spacing: 16) {

            // ── General ───────────────────────────────────────────────
            section("General") {
                row("Name") {
                    TextField("My Prod DB", text: $draft.name).textFieldStyle(.roundedBorder)
                }
                row("Type") {
                    Picker("", selection: Binding(
                        get: { draft.type },
                        set: { draft.setType($0) }
                    )) {
                        ForEach(ConnectionType.allCases) { t in Text(t.label).tag(t) }
                    }
                    .pickerStyle(.segmented)
                }
            }

            // ── Connection ────────────────────────────────────────────
            if draft.type == .sqlite {
                section("File") {
                    row("Path") {
                        HStack {
                            TextField("/path/to/db.sqlite", text: $draft.filename)
                                .textFieldStyle(.roundedBorder)
                            browseButton(title: "Choose…", types: ["db", "sqlite", "sqlite3"]) { draft.filename = $0 }
                        }
                    }
                }
            } else {
                section("Connection") {
                    HStack(alignment: .top, spacing: 12) {
                        row("Host") {
                            TextField("localhost", text: $draft.host).textFieldStyle(.roundedBorder)
                        }
                        row("Port") {
                            TextField("5432", text: $draft.port)
                                .textFieldStyle(.roundedBorder)
                                .frame(width: 72)
                        }
                    }
                    row("User") {
                        TextField("postgres", text: $draft.user).textFieldStyle(.roundedBorder)
                    }
                    row("Password") {
                        SecureField("••••••••", text: $draft.password).textFieldStyle(.roundedBorder)
                    }
                    row("Database") {
                        TextField("mydb", text: $draft.database).textFieldStyle(.roundedBorder)
                    }
                    row("Socket") {
                        TextField("Leave empty for TCP (optional)", text: $draft.socketPath)
                            .textFieldStyle(.roundedBorder)
                    }
                }

                // ── SSH Tunnel ────────────────────────────────────────
                section("SSH Tunnel", toggle: $draft.useSSH) {
                    if draft.useSSH {
                        HStack(alignment: .top, spacing: 12) {
                            row("SSH Host") {
                                TextField("jump.example.com", text: $draft.sshHost)
                                    .textFieldStyle(.roundedBorder)
                            }
                            row("Port") {
                                TextField("22", text: $draft.sshPort)
                                    .textFieldStyle(.roundedBorder)
                                    .frame(width: 72)
                            }
                        }
                        row("SSH User") {
                            TextField("ubuntu", text: $draft.sshUser).textFieldStyle(.roundedBorder)
                        }
                        row("Auth") {
                            Picker("", selection: $draft.sshAuthType) {
                                ForEach(SSHAuthType.allCases, id: \.self) { t in
                                    Text(t.label).tag(t)
                                }
                            }
                            .pickerStyle(.segmented)
                        }
                        if draft.sshAuthType == .key {
                            row("Private Key") {
                                HStack {
                                    TextField("~/.ssh/id_rsa", text: $draft.sshKeyPath)
                                        .textFieldStyle(.roundedBorder)
                                    browseButton(title: "Choose…", types: []) { draft.sshKeyPath = $0 }
                                }
                            }
                            row("Passphrase") {
                                SecureField("Leave empty if key has no passphrase", text: $draft.sshPassword)
                                    .textFieldStyle(.roundedBorder)
                            }
                        } else if draft.sshAuthType == .password {
                            row("SSH Password") {
                                SecureField("••••••••", text: $draft.sshPassword)
                                    .textFieldStyle(.roundedBorder)
                            }
                        }
                    }
                }

                // ── SSL / TLS ────────────────────────────────────────
                section("SSL / TLS", toggle: $draft.useSSL) {
                    if draft.useSSL {
                        row("Mode") {
                            Picker("", selection: $draft.sslMode) {
                                ForEach(SSLMode.allCases, id: \.self) { m in
                                    Text(m.label).tag(m)
                                }
                            }
                            .pickerStyle(.menu)
                            .frame(maxWidth: 180, alignment: .leading)
                        }
                        if draft.sslMode != .disable && draft.sslMode != .require {
                            row("CA Cert") {
                                HStack {
                                    TextField("ca.pem", text: $draft.sslCAPath)
                                        .textFieldStyle(.roundedBorder)
                                    browseButton(title: "Choose…", types: ["pem", "crt", "cert"]) { draft.sslCAPath = $0 }
                                }
                            }
                        }
                        row("Client Cert") {
                            HStack {
                                TextField("client-cert.pem (optional)", text: $draft.sslCertPath)
                                    .textFieldStyle(.roundedBorder)
                                browseButton(title: "Choose…", types: ["pem", "crt", "cert"]) { draft.sslCertPath = $0 }
                            }
                        }
                        row("Client Key") {
                            HStack {
                                TextField("client-key.pem (optional)", text: $draft.sslKeyPath)
                                    .textFieldStyle(.roundedBorder)
                                browseButton(title: "Choose…", types: ["pem", "key"]) { draft.sslKeyPath = $0 }
                            }
                        }
                    }
                }
            }

            // ── Query Policy ──────────────────────────────────────────
            queryPolicySection
        }
    }

    // MARK: - Query policy section

    private var queryPolicySection: some View {
        section("Query Policy") {
            VStack(alignment: .leading, spacing: 12) {

                // Preset picker
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

                // Category toggles grouped by Read / Write / Schema / Admin
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

                // Guards
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
                            .textFieldStyle(.roundedBorder)
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
        if draft.type != .sqlite { cats.append(.grant) }
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
            Button(editingConn != nil ? "Save" : "Add") { onSave(draft) }
                .buttonStyle(.borderedProminent)
                .disabled(draft.name.trimmingCharacters(in: .whitespaces).isEmpty)
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
    }

    // MARK: - Layout helpers

    private func section<C: View>(_ title: String, @ViewBuilder content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(title)
                .font(.system(size: 11, weight: .semibold))
                .foregroundColor(.secondary)
                .textCase(.uppercase)
                .padding(.bottom, 6)
            VStack(spacing: 0) {
                content()
            }
            .cardSurface()
        }
    }

    private func section<C: View>(_ title: String, toggle: Binding<Bool>, @ViewBuilder content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(title)
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundColor(.secondary)
                    .textCase(.uppercase)
                Spacer()
                Toggle("", isOn: toggle).toggleStyle(.switch).controlSize(.mini)
            }
            .padding(.bottom, 6)
            VStack(spacing: 0) {
                content()
            }
            .cardSurface()
        }
    }

    private func row<C: View>(_ label: String, @ViewBuilder content: () -> C) -> some View {
        HStack(alignment: .center, spacing: 12) {
            Text(label)
                .font(.system(size: 12))
                .foregroundColor(.secondary)
                .frame(width: 104, alignment: .leading)
            content()
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .overlay(alignment: .bottom) {
            Divider().padding(.leading, 124)
        }
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
