import Foundation
import Observation
import SQLite3

// MARK: - Query log entry (for audit viewer)

struct QueryLogEntry: Identifiable {
    let id: Int
    let connectionId: String
    let connectionName: String
    let sql: String
    let verdict: String       // allowed | blocked | error
    let reason: String?
    let categories: String?
    let createdAt: String
}

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
            query_policy TEXT,
            token TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )
        """)

        exec("""
        CREATE TABLE IF NOT EXISTS query_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            connection_id TEXT NOT NULL,
            connection_name TEXT NOT NULL,
            sql TEXT NOT NULL,
            verdict TEXT NOT NULL,
            reason TEXT,
            categories TEXT,
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
            "ALTER TABLE connections ADD COLUMN query_policy TEXT",
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
               environment,read_only,query_policy,token,created_at
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
           environment,read_only,query_policy,token)
        VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
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
          environment=?,read_only=?,query_policy=?
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

    // MARK: - Query log

    func recentLog(connectionId: String, limit: Int = 50) -> [QueryLogEntry] {
        var stmt: OpaquePointer?
        let sql = """
        SELECT id, connection_id, connection_name, sql, verdict, reason, categories, created_at
        FROM query_log
        WHERE connection_id = ?
        ORDER BY id DESC
        LIMIT ?
        """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        defer { sqlite3_finalize(stmt) }
        bindText(stmt, 1, connectionId)
        sqlite3_bind_int(stmt, 2, Int32(limit))

        var result: [QueryLogEntry] = []
        while sqlite3_step(stmt) == SQLITE_ROW {
            func str(_ i: Int32) -> String? {
                guard let p = sqlite3_column_text(stmt, i) else { return nil }
                return String(cString: p)
            }
            guard let createdAt = str(7) else { continue }
            result.append(QueryLogEntry(
                id: Int(sqlite3_column_int(stmt, 0)),
                connectionId: str(1) ?? "",
                connectionName: str(2) ?? "",
                sql: str(3) ?? "",
                verdict: str(4) ?? "",
                reason: str(5),
                categories: str(6),
                createdAt: createdAt
            ))
        }
        return result
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
        bindInt(stmt, 24, draft.queryPolicy.preset == .readOnly ? 1 : 0)
        bindNullableText(stmt, 25, draft.queryPolicy.toJSON())
        bindText(stmt, 26, token)
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
        bindInt(stmt, 23, draft.queryPolicy.preset == .readOnly ? 1 : 0)
        bindNullableText(stmt, 24, draft.queryPolicy.toJSON())
        bindText(stmt, 25, id)
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
              let token = str(25), let createdAt = str(26) else { return nil }

        let environment = Environment(rawValue: str(22) ?? "development") ?? .development
        let readOnly = bool(23)
        let queryPolicyJSON = str(24)

        // Parse stored policy, falling back to legacy read_only flag
        let queryPolicy = QueryPolicy.fromJSON(queryPolicyJSON)
            ?? (readOnly ? .make(.readOnly) : .default(for: environment))

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
            environment: environment,
            readOnly: readOnly,
            queryPolicy: queryPolicy,
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
