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
    let resultJson: String?   // JSON snapshot of result rows
    let rowCount: Int?        // total rows before cap
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
        purgeOldLogs()
        load()
    }

    // MARK: - Schema

    private func migrate() {
        // Shared contract with the TS server (store/integrations.ts). Everything
        // service-specific lives in the `config` JSON blob; only the fields every
        // adapter shares are first-class columns.
        exec("""
        CREATE TABLE IF NOT EXISTS integrations (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            type TEXT NOT NULL,
            config TEXT NOT NULL DEFAULT '{}',
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
            result_json TEXT,
            row_count INTEGER,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )
        """)

        exec("""
        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )
        """)

        let alters = [
            "ALTER TABLE query_log ADD COLUMN result_json TEXT",
            "ALTER TABLE query_log ADD COLUMN row_count INTEGER",
        ]
        for sql in alters { exec(sql) } // silently ignores if column exists
    }

    // MARK: - Settings

    func getSetting(_ key: String, default defaultValue: String) -> String {
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, "SELECT value FROM settings WHERE key = ?", -1, &stmt, nil) == SQLITE_OK else { return defaultValue }
        defer { sqlite3_finalize(stmt) }
        bindText(stmt, 1, key)
        guard sqlite3_step(stmt) == SQLITE_ROW,
              let p = sqlite3_column_text(stmt, 0) else { return defaultValue }
        return String(cString: p)
    }

    func setSetting(_ key: String, value: String) {
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, "INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)", -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }
        bindText(stmt, 1, key)
        bindText(stmt, 2, value)
        sqlite3_step(stmt)
    }

    var logRetentionDays: Int {
        get { Int(getSetting("log_retention_days", default: "30")) ?? 30 }
        set { setSetting("log_retention_days", value: String(newValue)) }
    }

    func purgeOldLogs() {
        let days = logRetentionDays
        guard days > 0 else { return }
        exec("DELETE FROM query_log WHERE created_at < datetime('now', '-\(days) days')")
    }

    // MARK: - CRUD

    func load() {
        var stmt: OpaquePointer?
        let sql = """
        SELECT id,name,type,config,environment,read_only,query_policy,token,created_at
        FROM integrations ORDER BY created_at DESC
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
        INSERT INTO integrations (id,name,type,config,environment,read_only,query_policy,token)
        VALUES (?,?,?,?,?,?,?,?)
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }
        bindText(stmt, 1, id)
        bindText(stmt, 2, draft.name)
        bindText(stmt, 3, draft.type)
        bindText(stmt, 4, configJSON(from: draft))
        bindText(stmt, 5, draft.environment.rawValue)
        bindInt(stmt, 6, readOnlyFlag(draft))
        bindNullableText(stmt, 7, policyJSON(draft))
        bindText(stmt, 8, token)
        sqlite3_step(stmt)
        load()
    }

    func update(_ conn: Connection, draft: ConnectionDraft) {
        let sql = """
        UPDATE integrations SET name=?,type=?,config=?,environment=?,read_only=?,query_policy=?
        WHERE id=?
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }
        bindText(stmt, 1, draft.name)
        bindText(stmt, 2, draft.type)
        bindText(stmt, 3, configJSON(from: draft))
        bindText(stmt, 4, draft.environment.rawValue)
        bindInt(stmt, 5, readOnlyFlag(draft))
        bindNullableText(stmt, 6, policyJSON(draft))
        bindText(stmt, 7, conn.id)
        sqlite3_step(stmt)
        load()
    }

    func delete(_ conn: Connection) {
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, "DELETE FROM integrations WHERE id=?", -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }
        bindText(stmt, 1, conn.id)
        sqlite3_step(stmt)
        load()
    }

    // MARK: - Query log

    func recentLog(connectionId: String, limit: Int = 200) -> [QueryLogEntry] {
        var stmt: OpaquePointer?
        let sql = """
        SELECT id, connection_id, connection_name, sql, verdict, reason, categories,
               result_json, row_count, created_at
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
            func optInt(_ i: Int32) -> Int? {
                sqlite3_column_type(stmt, i) == SQLITE_NULL ? nil : Int(sqlite3_column_int(stmt, i))
            }
            guard let createdAt = str(9) else { continue }
            result.append(QueryLogEntry(
                id: Int(sqlite3_column_int(stmt, 0)),
                connectionId: str(1) ?? "",
                connectionName: str(2) ?? "",
                sql: str(3) ?? "",
                verdict: str(4) ?? "",
                reason: str(5),
                categories: str(6),
                resultJson: str(7),
                rowCount: optInt(8),
                createdAt: createdAt
            ))
        }
        return result
    }

    func clearAllLogs(connectionId: String) {
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, "DELETE FROM query_log WHERE connection_id = ?", -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }
        bindText(stmt, 1, connectionId)
        sqlite3_step(stmt)
    }

    // MARK: - Adapter catalog

    var adapters: [AdapterManifest] = []

    func loadAdapters() async {
        for _ in 0..<12 {
            if let list = await fetchAdapters() { adapters = list; return }
            try? await Task.sleep(for: .milliseconds(500))
        }
    }

    private func fetchAdapters() async -> [AdapterManifest]? {
        guard let url = URL(string: "http://localhost:4242/api/adapters") else { return nil }
        do {
            let (data, _) = try await URLSession.shared.data(from: url)
            return try JSONDecoder().decode(AdapterCatalogResponse.self, from: data).adapters
        } catch {
            return nil
        }
    }

    // MARK: - Policy encoding

    private func readOnlyFlag(_ d: ConnectionDraft) -> Int {
        if d.policyKind == "action" { return d.allowWrite ? 0 : 1 }
        return d.queryPolicy.preset == .readOnly ? 1 : 0
    }

    private func policyJSON(_ d: ConnectionDraft) -> String? {
        if d.policyKind == "action" {
            let actions = d.allowWrite ? ["read", "write"] : ["read"]
            let obj: [String: Any] = ["actions": actions]
            guard let data = try? JSONSerialization.data(withJSONObject: obj) else { return nil }
            return String(data: data, encoding: .utf8)
        }
        return d.queryPolicy.toJSON()
    }

    // MARK: - Config blob (pack)

    // Serialize the draft's config dict to JSON, coercing values to the types the
    // adapter declared (number → Int, toggle → Bool) so the TS side reads them
    // correctly. Empty values are omitted to keep the blob minimal.
    private func configJSON(from d: ConnectionDraft) -> String {
        let typeByKey = Dictionary(d.fields.map { ($0.key, $0.type) }, uniquingKeysWith: { a, _ in a })
        var c: [String: Any] = [:]
        for (key, value) in d.config {
            if value.isEmpty { continue }
            switch typeByKey[key] {
            case "number": if let i = Int(value) { c[key] = i }
            case "toggle": if value == "true" { c[key] = true }
            default: c[key] = value
            }
        }
        guard let data = try? JSONSerialization.data(withJSONObject: c),
              let json = String(data: data, encoding: .utf8) else { return "{}" }
        return json
    }

    // MARK: - Row parsing

    private func parse(_ stmt: OpaquePointer?) -> Connection? {
        guard let stmt else { return nil }
        func str(_ i: Int32) -> String? {
            guard let p = sqlite3_column_text(stmt, i) else { return nil }
            return String(cString: p)
        }
        func bool(_ i: Int32) -> Bool { sqlite3_column_int(stmt, i) != 0 }

        // Columns: 0 id,1 name,2 type,3 config,4 environment,5 read_only,6 query_policy,7 token,8 created_at
        guard let id = str(0), let name = str(1), let type = str(2),
              let token = str(7), let createdAt = str(8) else { return nil }

        // Config blob → string dict (values may be string/number/bool in JSON).
        let cfgAny = (try? JSONSerialization.jsonObject(with: Data((str(3) ?? "{}").utf8))) as? [String: Any] ?? [:]
        var config: [String: String] = [:]
        for (k, v) in cfgAny {
            switch v {
            case let s as String: config[k] = s
            case let b as Bool: config[k] = b ? "true" : "false"
            case let i as Int: config[k] = String(i)
            case let d as Double: config[k] = String(Int(d))
            default: config[k] = "\(v)"
            }
        }

        let environment = Environment(rawValue: str(4) ?? "development") ?? .development
        let readOnly = bool(5)
        let queryPolicyJSON = str(6)

        // Parse stored SQL policy, falling back to the read_only flag. (Action
        // adapters store {actions:[…]}, which won't parse here — the read_only
        // flag carries their read/write intent.)
        let queryPolicy = QueryPolicy.fromJSON(queryPolicyJSON)
            ?? (readOnly ? .make(.readOnly) : .default(for: environment))

        return Connection(
            id: id, name: name, type: type, config: config,
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
