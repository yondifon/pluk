import Foundation
import Observation
import SQLite3

@Observable
@MainActor
final class ConnectionStore {
    var connections: [Connection] = []

    @ObservationIgnored private var db: OpaquePointer?

    init() {
        let dir = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".pluk").path
        try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        sqlite3_open("\(dir)/pluk.db", &db)
        migrate()
        load()
    }

    // MARK: - Schema

    private func migrate() {
        exec("""
        CREATE TABLE IF NOT EXISTS connections (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            type TEXT NOT NULL,
            host TEXT, port INTEGER, "user" TEXT, password TEXT, database TEXT,
            filename TEXT, socket_path TEXT,
            use_ssh INTEGER NOT NULL DEFAULT 0,
            ssh_host TEXT, ssh_port INTEGER DEFAULT 22, ssh_user TEXT,
            ssh_auth_type TEXT DEFAULT 'agent', ssh_key_path TEXT, ssh_password TEXT,
            use_ssl INTEGER NOT NULL DEFAULT 0,
            ssl_mode TEXT DEFAULT 'require',
            ssl_ca_path TEXT, ssl_cert_path TEXT, ssl_key_path TEXT,
            environment TEXT DEFAULT 'development',
            read_only INTEGER NOT NULL DEFAULT 0,
            token TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )
        """)

        let alters = [
            "ALTER TABLE connections ADD COLUMN socket_path TEXT",
            "ALTER TABLE connections ADD COLUMN use_ssh INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE connections ADD COLUMN ssh_auth_type TEXT DEFAULT 'agent'",
            "ALTER TABLE connections ADD COLUMN ssh_password TEXT",
            "ALTER TABLE connections ADD COLUMN use_ssl INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE connections ADD COLUMN ssl_mode TEXT DEFAULT 'require'",
            "ALTER TABLE connections ADD COLUMN ssl_ca_path TEXT",
            "ALTER TABLE connections ADD COLUMN ssl_cert_path TEXT",
            "ALTER TABLE connections ADD COLUMN ssl_key_path TEXT",
            "ALTER TABLE connections ADD COLUMN environment TEXT DEFAULT 'development'",
            "ALTER TABLE connections ADD COLUMN read_only INTEGER NOT NULL DEFAULT 0",
        ]
        for sql in alters { exec(sql) } // silently ignores if column exists
    }

    // MARK: - CRUD

    func load() {
        var stmt: OpaquePointer?
        let sql = """
        SELECT id,name,type,host,port,"user",password,database,filename,socket_path,
               use_ssh,ssh_host,ssh_port,ssh_user,ssh_auth_type,ssh_key_path,ssh_password,
               use_ssl,ssl_mode,ssl_ca_path,ssl_cert_path,ssl_key_path,
               environment,read_only,token,created_at
        FROM connections ORDER BY created_at DESC
        """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }
        var result: [Connection] = []
        while sqlite3_step(stmt) == SQLITE_ROW { if let c = parse(stmt) { result.append(c) } }
        connections = result
    }

    func create(_ draft: ConnectionDraft) {
        let id = String(UUID().uuidString.lowercased().replacingOccurrences(of: "-", with: "").prefix(16))
        let token = "pluk_" + UUID().uuidString.lowercased().replacingOccurrences(of: "-", with: "")
        let sql = """
        INSERT INTO connections
          (id,name,type,host,port,"user",password,database,filename,socket_path,
           use_ssh,ssh_host,ssh_port,ssh_user,ssh_auth_type,ssh_key_path,ssh_password,
           use_ssl,ssl_mode,ssl_ca_path,ssl_cert_path,ssl_key_path,
           environment,read_only,token)
        VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }
        bindInsert(stmt, draft, id: id, token: token)
        sqlite3_step(stmt)
        load()
    }

    func update(_ conn: Connection, draft: ConnectionDraft) {
        let sql = """
        UPDATE connections SET
          name=?,type=?,host=?,port=?,"user"=?,password=?,database=?,filename=?,socket_path=?,
          use_ssh=?,ssh_host=?,ssh_port=?,ssh_user=?,ssh_auth_type=?,ssh_key_path=?,ssh_password=?,
          use_ssl=?,ssl_mode=?,ssl_ca_path=?,ssl_cert_path=?,ssl_key_path=?,
          environment=?,read_only=?
        WHERE id=?
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }
        bindUpdate(stmt, draft, id: conn.id)
        sqlite3_step(stmt)
        load()
    }

    func delete(_ conn: Connection) {
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, "DELETE FROM connections WHERE id=?", -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }
        bindText(stmt, 1, conn.id)
        sqlite3_step(stmt)
        load()
    }

    // MARK: - Bind helpers (INSERT)

    private func bindInsert(_ stmt: OpaquePointer?, _ draft: ConnectionDraft, id: String, token: String) {
        let isNet = draft.type.supportsNetwork
        bindText(stmt, 1, id)
        bindText(stmt, 2, draft.name)
        bindText(stmt, 3, draft.type.rawValue)
        bindNullableText(stmt, 4, isNet ? draft.host : nil)
        bindInt(stmt, 5, isNet ? Int(draft.port) : nil)
        bindNullableText(stmt, 6, isNet ? draft.user : nil)
        bindNullableText(stmt, 7, isNet ? (draft.password.isEmpty ? nil : draft.password) : nil)
        bindNullableText(stmt, 8, isNet ? draft.database : nil)
        bindNullableText(stmt, 9, draft.type == .sqlite ? draft.filename : nil)
        bindNullableText(stmt, 10, draft.socketPath.isEmpty ? nil : draft.socketPath)
        bindInt(stmt, 11, draft.useSSH ? 1 : 0)
        bindNullableText(stmt, 12, draft.sshHost.isEmpty ? nil : draft.sshHost)
        bindInt(stmt, 13, Int(draft.sshPort) ?? 22)
        bindNullableText(stmt, 14, draft.sshUser.isEmpty ? nil : draft.sshUser)
        bindText(stmt, 15, draft.sshAuthType.rawValue)
        bindNullableText(stmt, 16, draft.sshKeyPath.isEmpty ? nil : draft.sshKeyPath)
        bindNullableText(stmt, 17, draft.sshPassword.isEmpty ? nil : draft.sshPassword)
        bindInt(stmt, 18, draft.useSSL ? 1 : 0)
        bindText(stmt, 19, draft.sslMode.rawValue)
        bindNullableText(stmt, 20, draft.sslCAPath.isEmpty ? nil : draft.sslCAPath)
        bindNullableText(stmt, 21, draft.sslCertPath.isEmpty ? nil : draft.sslCertPath)
        bindNullableText(stmt, 22, draft.sslKeyPath.isEmpty ? nil : draft.sslKeyPath)
        bindText(stmt, 23, draft.environment.rawValue)
        bindInt(stmt, 24, draft.readOnly ? 1 : 0)
        bindText(stmt, 25, token)
    }

    private func bindUpdate(_ stmt: OpaquePointer?, _ draft: ConnectionDraft, id: String) {
        let isNet = draft.type.supportsNetwork
        bindText(stmt, 1, draft.name)
        bindText(stmt, 2, draft.type.rawValue)
        bindNullableText(stmt, 3, isNet ? draft.host : nil)
        bindInt(stmt, 4, isNet ? Int(draft.port) : nil)
        bindNullableText(stmt, 5, isNet ? draft.user : nil)
        bindNullableText(stmt, 6, isNet ? (draft.password.isEmpty ? nil : draft.password) : nil)
        bindNullableText(stmt, 7, isNet ? draft.database : nil)
        bindNullableText(stmt, 8, draft.type == .sqlite ? draft.filename : nil)
        bindNullableText(stmt, 9, draft.socketPath.isEmpty ? nil : draft.socketPath)
        bindInt(stmt, 10, draft.useSSH ? 1 : 0)
        bindNullableText(stmt, 11, draft.sshHost.isEmpty ? nil : draft.sshHost)
        bindInt(stmt, 12, Int(draft.sshPort) ?? 22)
        bindNullableText(stmt, 13, draft.sshUser.isEmpty ? nil : draft.sshUser)
        bindText(stmt, 14, draft.sshAuthType.rawValue)
        bindNullableText(stmt, 15, draft.sshKeyPath.isEmpty ? nil : draft.sshKeyPath)
        bindNullableText(stmt, 16, draft.sshPassword.isEmpty ? nil : draft.sshPassword)
        bindInt(stmt, 17, draft.useSSL ? 1 : 0)
        bindText(stmt, 18, draft.sslMode.rawValue)
        bindNullableText(stmt, 19, draft.sslCAPath.isEmpty ? nil : draft.sslCAPath)
        bindNullableText(stmt, 20, draft.sslCertPath.isEmpty ? nil : draft.sslCertPath)
        bindNullableText(stmt, 21, draft.sslKeyPath.isEmpty ? nil : draft.sslKeyPath)
        bindText(stmt, 22, draft.environment.rawValue)
        bindInt(stmt, 23, draft.readOnly ? 1 : 0)
        bindText(stmt, 24, id)
    }

    // MARK: - Row parsing

    private func parse(_ stmt: OpaquePointer?) -> Connection? {
        guard let stmt else { return nil }
        func str(_ i: Int32) -> String? {
            guard let p = sqlite3_column_text(stmt, i) else { return nil }
            return String(cString: p)
        }
        func int(_ i: Int32) -> Int? {
            sqlite3_column_type(stmt, i) == SQLITE_NULL ? nil : Int(sqlite3_column_int(stmt, i))
        }
        func bool(_ i: Int32) -> Bool { sqlite3_column_int(stmt, i) != 0 }

        guard let id = str(0), let name = str(1), let typeRaw = str(2),
              let type = ConnectionType(rawValue: typeRaw),
              let token = str(24), let createdAt = str(25) else { return nil }

        return Connection(
            id: id, name: name, type: type,
            host: str(3), port: int(4), user: str(5), password: str(6),
            database: str(7), filename: str(8), socketPath: str(9),
            useSSH: bool(10),
            sshHost: str(11), sshPort: int(12), sshUser: str(13),
            sshAuthType: SSHAuthType(rawValue: str(14) ?? "agent") ?? .agent,
            sshKeyPath: str(15), sshPassword: str(16),
            useSSL: bool(17),
            sslMode: SSLMode(rawValue: str(18) ?? "require") ?? .require,
            sslCAPath: str(19), sslCertPath: str(20), sslKeyPath: str(21),
            environment: Environment(rawValue: str(22) ?? "development") ?? .development,
            readOnly: bool(23),
            token: token, createdAt: createdAt
        )
    }

    // MARK: - SQLite low-level helpers

    private func exec(_ sql: String) {
        sqlite3_exec(db, sql, nil, nil, nil)
    }

    private func bindText(_ stmt: OpaquePointer?, _ idx: Int32, _ value: String) {
        sqlite3_bind_text(stmt, idx, (value as NSString).utf8String, -1, nil)
    }

    private func bindNullableText(_ stmt: OpaquePointer?, _ idx: Int32, _ value: String?) {
        if let value, !value.isEmpty { bindText(stmt, idx, value) } else { sqlite3_bind_null(stmt, idx) }
    }

    private func bindInt(_ stmt: OpaquePointer?, _ idx: Int32, _ value: Int?) {
        if let value { sqlite3_bind_int(stmt, idx, Int32(value)) } else { sqlite3_bind_null(stmt, idx) }
    }
}
