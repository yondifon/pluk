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
    let source: String?       // originating tool / operation (e.g. "query", "list_tables")
    let resultJson: String?   // JSON snapshot of result rows
    let rowCount: Int?        // total rows before cap
    let responseText: String? // raw agent-visible response text (capped server-side)
    let groupId: String?      // set when the call was routed through a group endpoint
    let groupName: String?    // group display name
    let createdAt: String
}

/// Last-observed health of a connection, mirrored from the server's
/// `/api/health`. `nil` (absent) means untested this session.
struct ConnHealth: Decodable {
    let status: String        // "ok" | "error"
    let error: String?
    let at: Double            // epoch ms
    var isError: Bool { status == "error" }
}

@Observable
@MainActor
final class ConnectionStore {
    var connections: [Connection] = []
    var groups: [ConnectionGroup] = []
    /// Per-connection health keyed by integration id. Polled from the server so a
    /// connection that fails for an agent (SSH/auth/tunnel) shows red without the
    /// user manually testing it.
    var health: [String: ConnHealth] = [:]

    /// Set by the view layer so health transitions can raise toasts/notifications.
    @ObservationIgnored var toastCenter: ToastCenter?

    @ObservationIgnored private var db: OpaquePointer?

    init() {
        let dir = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".pluk").path
        try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        sqlite3_open("\(dir)/pluk.db", &db)
        migrate()
        purgeOldLogs()
        load()
    }

    #if DEBUG
    /// Preview store with no DB access and sample data.
    private init(preview: ()) {
        self.db = nil
    }

    @MainActor
    static var preview: ConnectionStore {
        let store = ConnectionStore(preview: ())
        store.connections = [.sample, .sampleGroupMember]
        store.groups = [.sample]
        store.adapters = [.samplePostgres, .sampleLinear]
        store.health = [Connection.sample.id: .ok]
        return store
    }
    #endif

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
            response_text TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )
        """)

        exec("""
        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )
        """)

        // Shared contract with the TS server (store/groups.ts). A group fronts
        // several integrations (member_ids JSON) behind one MCP token/endpoint.
        exec("""
        CREATE TABLE IF NOT EXISTS groups (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            environment TEXT DEFAULT 'production',
            member_ids TEXT NOT NULL DEFAULT '[]',
            token TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        )
        """)

        let alters = [
            "ALTER TABLE query_log ADD COLUMN result_json TEXT",
            "ALTER TABLE query_log ADD COLUMN row_count INTEGER",
            "ALTER TABLE query_log ADD COLUMN response_text TEXT",
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
        loadGroups()
    }

    // MARK: - Groups

    func loadGroups() {
        var stmt: OpaquePointer?
        let sql = "SELECT id,name,environment,member_ids,token,created_at FROM groups ORDER BY created_at DESC"
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }
        var result: [ConnectionGroup] = []
        while sqlite3_step(stmt) == SQLITE_ROW { if let g = parseGroup(stmt) { result.append(g) } }
        groups = result
    }

    @discardableResult
    func createGroup(name: String = "New Group") -> String {
        let id = String(UUID().uuidString.lowercased().replacingOccurrences(of: "-", with: "").prefix(16))
        let token = "pluk_" + UUID().uuidString.lowercased().replacingOccurrences(of: "-", with: "")
        let sql = "INSERT INTO groups (id,name,environment,member_ids,token) VALUES (?,?,?,?,?)"
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return id }
        defer { sqlite3_finalize(stmt) }
        bindText(stmt, 1, id)
        bindText(stmt, 2, name)
        sqlite3_bind_null(stmt, 3) // env unscoped by default; a group may span environments
        bindText(stmt, 4, "[]")
        bindText(stmt, 5, token)
        sqlite3_step(stmt)
        loadGroups()
        return id
    }

    func updateGroup(_ group: ConnectionGroup) {
        let membersJSON = serializeMembers(group.members)
        let sql = "UPDATE groups SET name=?,environment=?,member_ids=? WHERE id=?"
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }
        bindText(stmt, 1, group.name)
        bindNullableText(stmt, 2, group.environment?.rawValue)
        bindText(stmt, 3, membersJSON)
        bindText(stmt, 4, group.id)
        sqlite3_step(stmt)
        loadGroups()
        reloadServer(id: group.id) // overrides/members changed → rebuild this group's sessions
    }

    func deleteGroup(_ group: ConnectionGroup) {
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, "DELETE FROM groups WHERE id=?", -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }
        bindText(stmt, 1, group.id)
        sqlite3_step(stmt)
        loadGroups()
        reloadServer(id: group.id) // group gone → drop its sessions
    }

    private func parseGroup(_ stmt: OpaquePointer?) -> ConnectionGroup? {
        guard let stmt else { return nil }
        func str(_ i: Int32) -> String? {
            guard let p = sqlite3_column_text(stmt, i) else { return nil }
            return String(cString: p)
        }
        // Columns: 0 id,1 name,2 environment,3 member_ids,4 token,5 created_at
        guard let id = str(0), let name = str(1), let token = str(4), let createdAt = str(5) else { return nil }
        let environment = str(2).flatMap { Environment(rawValue: $0) } // nil = unscoped/mixed
        let members = parseMembers(str(3) ?? "[]")
        return ConnectionGroup(id: id, name: name, environment: environment, members: members, token: token, createdAt: createdAt)
    }

    // member_ids holds a JSON array; accepts the legacy form (id strings) and the
    // current form ({ id, overrides }), so old rows keep working.
    private func parseMembers(_ raw: String) -> [GroupMember] {
        guard let arr = (try? JSONSerialization.jsonObject(with: Data(raw.utf8))) as? [Any] else { return [] }
        return arr.compactMap { el in
            if let id = el as? String { return GroupMember(id: id) }
            guard let obj = el as? [String: Any], let id = obj["id"] as? String else { return nil }
            var overrides: [String: String] = [:]
            if let ov = obj["overrides"] as? [String: Any] {
                for (k, v) in ov { overrides[k] = stringify(v) }
            }
            return GroupMember(id: id, overrides: overrides)
        }
    }

    private func serializeMembers(_ members: [GroupMember]) -> String {
        let arr: [[String: Any]] = members.map { m in
            m.overrides.isEmpty ? ["id": m.id] : ["id": m.id, "overrides": m.overrides]
        }
        return (try? JSONSerialization.data(withJSONObject: arr))
            .flatMap { String(data: $0, encoding: .utf8) } ?? "[]"
    }

    private func stringify(_ v: Any) -> String {
        switch v {
        case let s as String: return s
        case let b as Bool: return b ? "true" : "false"
        case let i as Int: return String(i)
        case let d as Double: return String(Int(d))
        default: return "\(v)"
        }
    }

    @discardableResult
    func create(_ draft: ConnectionDraft) -> String {
        let id = String(UUID().uuidString.lowercased().replacingOccurrences(of: "-", with: "").prefix(16))
        let token = "pluk_" + UUID().uuidString.lowercased().replacingOccurrences(of: "-", with: "")
        let sql = """
        INSERT INTO integrations (id,name,type,config,environment,read_only,query_policy,token)
        VALUES (?,?,?,?,?,?,?,?)
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return id }
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
        return id
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

    /// Clone a connection: copies its config/policy verbatim (config JSON is
    /// copied at the SQL level so typed values survive), with a fresh id+token
    /// and a " copy" suffix. Returns the new connection's id.
    @discardableResult
    func duplicate(_ conn: Connection) -> String {
        let id = String(UUID().uuidString.lowercased().replacingOccurrences(of: "-", with: "").prefix(16))
        let token = "pluk_" + UUID().uuidString.lowercased().replacingOccurrences(of: "-", with: "")
        let sql = """
        INSERT INTO integrations (id,name,type,config,environment,read_only,query_policy,token)
        SELECT ?,?,type,config,environment,read_only,query_policy,?
        FROM integrations WHERE id=?
        """
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return id }
        defer { sqlite3_finalize(stmt) }
        bindText(stmt, 1, id)
        bindText(stmt, 2, conn.name + " copy")
        bindText(stmt, 3, token)
        bindText(stmt, 4, conn.id)
        sqlite3_step(stmt)
        load()
        return id
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

    // Columns shared by every log query, kept in one place so the column indices
    // used by `parseLogRows` stay in sync across the per-connection and per-group
    // reads below.
    private static let logColumns = """
    id, connection_id, connection_name, sql, verdict, reason, categories,
    source, result_json, row_count, response_text, group_id, group_name, created_at
    """

    /// Activity for a single integration (its own endpoint + any group routing).
    func recentLog(connectionId: String, limit: Int = 200) -> [QueryLogEntry] {
        var stmt: OpaquePointer?
        let sql = """
        SELECT \(Self.logColumns)
        FROM query_log
        WHERE connection_id = ?
        ORDER BY id DESC
        LIMIT ?
        """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        defer { sqlite3_finalize(stmt) }
        bindText(stmt, 1, connectionId)
        sqlite3_bind_int(stmt, 2, Int32(limit))
        return parseLogRows(stmt)
    }

    /// Activity for every member integration that was called through this group's
    /// endpoint — the group view's single, aggregated activity feed.
    func recentLogForGroup(groupId: String, limit: Int = 400) -> [QueryLogEntry] {
        var stmt: OpaquePointer?
        let sql = """
        SELECT \(Self.logColumns)
        FROM query_log
        WHERE group_id = ?
        ORDER BY id DESC
        LIMIT ?
        """
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        defer { sqlite3_finalize(stmt) }
        bindText(stmt, 1, groupId)
        sqlite3_bind_int(stmt, 2, Int32(limit))
        return parseLogRows(stmt)
    }

    private func parseLogRows(_ stmt: OpaquePointer?) -> [QueryLogEntry] {
        var result: [QueryLogEntry] = []
        while sqlite3_step(stmt) == SQLITE_ROW {
            func str(_ i: Int32) -> String? {
                guard let p = sqlite3_column_text(stmt, i) else { return nil }
                return String(cString: p)
            }
            func optInt(_ i: Int32) -> Int? {
                sqlite3_column_type(stmt, i) == SQLITE_NULL ? nil : Int(sqlite3_column_int(stmt, i))
            }
            guard let createdAt = str(13) else { continue }
            result.append(QueryLogEntry(
                id: Int(sqlite3_column_int(stmt, 0)),
                connectionId: str(1) ?? "",
                connectionName: str(2) ?? "",
                sql: str(3) ?? "",
                verdict: str(4) ?? "",
                reason: str(5),
                categories: str(6),
                source: str(7),
                resultJson: str(8),
                rowCount: optInt(9),
                responseText: str(10),
                groupId: str(11),
                groupName: str(12),
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

    func clearAllLogs(groupId: String) {
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, "DELETE FROM query_log WHERE group_id = ?", -1, &stmt, nil) == SQLITE_OK else { return }
        defer { sqlite3_finalize(stmt) }
        bindText(stmt, 1, groupId)
        sqlite3_step(stmt)
    }

    // MARK: - Adapter catalog

    var adapters: [AdapterManifest] = []
    var adaptersLoadFailed = false

    func loadAdapters() async {
        adaptersLoadFailed = false
        for _ in 0..<12 {
            if let list = await fetchAdapters() { adapters = list; adaptersLoadFailed = false; return }
            try? await Task.sleep(for: .milliseconds(500))
        }
        // Exhausted retries with nothing to show — surface a retry affordance
        // instead of leaving forms spinning on an empty catalog forever.
        if adapters.isEmpty { adaptersLoadFailed = true }
    }

    /// Ask the server to drop the live MCP sessions for one integration/group (by
    /// id) so config/override edits take effect on the next agent request. Other
    /// integrations' and groups' sessions are left untouched.
    func reloadServer(id: String) {
        Task {
            guard var comps = URLComponents(string: PlukServer.api("reload")) else { return }
            comps.queryItems = [URLQueryItem(name: "id", value: id)]
            guard let url = comps.url else { return }
            var req = URLRequest(url: url)
            req.httpMethod = "POST"
            _ = try? await URLSession.shared.data(for: req)
        }
    }

    /// Poll per-connection health from the server. Records the manual-test result
    /// and any agent-driven connect/tunnel/auth failure, so the UI can show red.
    func refreshHealth() async {
        guard let url = URL(string: PlukServer.api("health")) else { return }
        struct Resp: Decodable { let health: [String: ConnHealth] }
        do {
            let (data, _) = try await URLSession.shared.data(from: url)
            let next = try JSONDecoder().decode(Resp.self, from: data).health
            emitHealthTransitions(from: health, to: next)
            health = next
        } catch {
            // Server down / not ready — leave the last snapshot in place.
        }
    }

    /// Raise a toast only when a connection crosses a boundary — newly failing,
    /// or recovered — so a steadily-broken connection doesn't notify every poll.
    private func emitHealthTransitions(from old: [String: ConnHealth], to next: [String: ConnHealth]) {
        guard let center = toastCenter else { return }
        for (id, new) in next {
            let was = old[id]
            if new.isError, was?.isError != true {
                let name = connections.first { $0.id == id }?.name ?? "Connection"
                center.present(Toast(connectionId: id, title: name,
                                     message: new.error ?? "Connection is failing.", kind: .error))
            } else if !new.isError, was?.isError == true {
                let name = connections.first { $0.id == id }?.name ?? "Connection"
                center.present(Toast(connectionId: id, title: name,
                                     message: "Reconnected.", kind: .success))
            }
        }
    }

    /// Test one connection (mirrors the detail view's Test) and refresh health.
    /// Used by a toast's Retry action.
    func test(connectionId: String) {
        Task {
            guard let url = URL(string: PlukServer.api("integrations/\(connectionId)/test")) else { return }
            var req = URLRequest(url: url)
            req.httpMethod = "POST"
            req.timeoutInterval = 12
            _ = try? await URLSession.shared.data(for: req)
            await refreshHealth()
        }
    }

    private func fetchAdapters() async -> [AdapterManifest]? {
        guard let url = URL(string: PlukServer.api("adapters")) else { return nil }
        do {
            let (data, _) = try await URLSession.shared.data(from: url)
            return try JSONDecoder().decode(AdapterCatalogResponse.self, from: data).adapters
        } catch {
            return nil
        }
    }

    // MARK: - Policy encoding

    // The `read_only` column is legacy (the per-tool config now carries all policy);
    // the TS server ignores it. Kept zero so the shared schema stays populated.
    private func readOnlyFlag(_ d: ConnectionDraft) -> Int { 0 }

    // Serialize the draft's per-tool config to the query_policy blob:
    // { "tools": { "<name>": { "enabled": Bool, "settings": { …typed… } } } }.
    // Each setting is coerced to the type the tool declared (number → Int, toggle
    // → Bool) so the TS side reads it correctly.
    private func policyJSON(_ d: ConnectionDraft) -> String? {
        var settingTypeByTool: [String: [String: String]] = [:]
        for t in d.tools {
            settingTypeByTool[t.name] = Dictionary((t.settings ?? []).map { ($0.key, $0.type) }, uniquingKeysWith: { a, _ in a })
        }

        var tools: [String: Any] = [:]
        for (name, state) in d.toolConfig {
            var entry: [String: Any] = ["enabled": state.enabled]
            let types = settingTypeByTool[name] ?? [:]
            var settings: [String: Any] = [:]
            for (key, value) in state.settings {
                if value.isEmpty { continue }
                switch types[key] {
                case "number": if let i = Int(value) { settings[key] = i }
                case "toggle": settings[key] = (value == "true")
                default: settings[key] = value
                }
            }
            if !settings.isEmpty { entry["settings"] = settings }
            tools[name] = entry
        }

        let obj: [String: Any] = ["tools": tools]
        guard let data = try? JSONSerialization.data(withJSONObject: obj),
              let json = String(data: data, encoding: .utf8) else { return nil }
        return json
    }

    // Parse the query_policy blob into per-tool state. Settings values are kept as
    // strings (mirroring the config-blob parse) for the form's string bindings.
    private func parseToolConfig(_ raw: String?) -> [String: ToolState] {
        guard let raw,
              let obj = (try? JSONSerialization.jsonObject(with: Data(raw.utf8))) as? [String: Any],
              let tools = obj["tools"] as? [String: Any] else { return [:] }
        var result: [String: ToolState] = [:]
        for (name, any) in tools {
            guard let entry = any as? [String: Any] else { continue }
            let enabled = (entry["enabled"] as? Bool) ?? true
            var settings: [String: String] = [:]
            if let s = entry["settings"] as? [String: Any] {
                for (k, v) in s { settings[k] = stringify(v) }
            }
            result[name] = ToolState(enabled: enabled, settings: settings)
        }
        return result
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
        let toolConfig = parseToolConfig(str(6))

        return Connection(
            id: id, name: name, type: type, config: config,
            environment: environment,
            readOnly: readOnly,
            toolConfig: toolConfig,
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
