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
            Picker("", selection: $draft.environment) {
                ForEach(Environment.allCases, id: \.self) { env in
                    Text(env.label).tag(env)
                }
            }
            .pickerStyle(.menu)
            .frame(width: 130)
            .help("Environment label (visual only)")
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

            // ── Advanced ──────────────────────────────────────────────
            section("Advanced") {
                VStack(alignment: .leading, spacing: 4) {
                    Toggle("Read-only mode", isOn: $draft.readOnly)
                        .help("Prevents write queries through this connection's MCP tools")
                    Text("Recommended for production. Agents should use SELECT queries with LIMIT.")
                        .font(.system(size: 11))
                        .foregroundColor(.secondary)
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
            }
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
