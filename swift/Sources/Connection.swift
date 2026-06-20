import Foundation
import SwiftUI

enum ConnectionType: String, CaseIterable, Identifiable {
    case postgres, mysql, sqlite
    var id: String { rawValue }

    var label: String {
        switch self {
        case .postgres: "PostgreSQL"
        case .mysql: "MySQL"
        case .sqlite: "SQLite"
        }
    }

    var defaultPort: Int {
        switch self {
        case .postgres: 5432
        case .mysql: 3306
        case .sqlite: 0
        }
    }

    var supportsNetwork: Bool { self != .sqlite }
}

enum SSHAuthType: String, CaseIterable {
    case agent, key, password

    var label: String {
        switch self {
        case .agent: "Agent"
        case .key: "Private Key"
        case .password: "Password"
        }
    }
}

enum SSLMode: String, CaseIterable {
    case disable, require
    case verifyCA = "verify-ca"
    case verifyFull = "verify-full"

    var label: String {
        switch self {
        case .disable: "Disable"
        case .require: "Require"
        case .verifyCA: "Verify CA"
        case .verifyFull: "Verify Full"
        }
    }
}

enum Environment: String, CaseIterable {
    case production, staging, development, local
    var label: String { rawValue.capitalized }
    var color: Color {
        switch self {
        case .production: .red
        case .staging: .orange
        case .development: .blue
        case .local: .gray
        }
    }
}

// ── Query Policy ─────────────────────────────────────────────────────────────

enum QueryPreset: String, CaseIterable, Codable, Identifiable {
    case readOnly       = "read-only"
    case readWrite      = "read-write"
    case migrations     = "migrations"
    case unrestricted   = "unrestricted"
    case custom         = "custom"

    var id: String { rawValue }

    var label: String {
        switch self {
        case .readOnly:     "Read-only"
        case .readWrite:    "Read & Write"
        case .migrations:   "Migrations"
        case .unrestricted: "Unrestricted"
        case .custom:       "Custom"
        }
    }

    var description: String {
        switch self {
        case .readOnly:     "SELECT, DESCRIBE, EXPLAIN, SHOW. Recommended for production."
        case .readWrite:    "SELECT, DESCRIBE + INSERT/UPDATE/DELETE. Requires WHERE on mutations."
        case .migrations:   "Full DDL access. CREATE, ALTER, DROP, stored procedures."
        case .unrestricted: "No restrictions. All statement types, filesystem ops allowed."
        case .custom:       "Custom combination of allowed statement types and guards."
        }
    }

    var isDestructive: Bool { self == .unrestricted }
}

enum StatementCategory: String, CaseIterable, Codable, Identifiable {
    // Read
    case select, inspect
    // Write
    case insert, update, delete, merge
    // Schema
    case create, alter, drop, truncate, rename
    // Admin
    case transaction, session, procedure, maintenance, grant

    var id: String { rawValue }

    var label: String {
        switch self {
        case .select:      "SELECT"
        case .inspect:     "DESCRIBE / EXPLAIN / SHOW / PRAGMA"
        case .insert:      "INSERT"
        case .update:      "UPDATE"
        case .delete:      "DELETE"
        case .merge:       "MERGE / REPLACE / UPSERT"
        case .create:      "CREATE"
        case .alter:       "ALTER"
        case .drop:        "DROP"
        case .truncate:    "TRUNCATE"
        case .rename:      "RENAME"
        case .transaction: "BEGIN / COMMIT / ROLLBACK"
        case .session:     "SET / RESET / USE"
        case .procedure:   "CALL / DO / EXEC"
        case .maintenance: "VACUUM / ANALYZE / OPTIMIZE"
        case .grant:       "GRANT / REVOKE"
        }
    }

    var group: String {
        switch self {
        case .select, .inspect:                                  "Read"
        case .insert, .update, .delete, .merge:                  "Write"
        case .create, .alter, .drop, .truncate, .rename:         "Schema"
        case .transaction, .session, .procedure, .maintenance, .grant: "Admin"
        }
    }
}

struct QueryPolicy: Codable, Equatable {
    var preset: QueryPreset
    var allowed: [StatementCategory]
    var blockStacked: Bool
    var requireWhere: Bool
    var allowFilesystem: Bool
    var maxRows: Int?

    // MARK: - Preset factories

    static func make(_ preset: QueryPreset) -> QueryPolicy {
        switch preset {
        case .readOnly:
            return QueryPolicy(
                preset: .readOnly,
                allowed: [.select, .inspect],
                blockStacked: true, requireWhere: false,
                allowFilesystem: false, maxRows: 1000
            )
        case .readWrite:
            return QueryPolicy(
                preset: .readWrite,
                allowed: [.select, .inspect, .insert, .update, .delete, .merge, .transaction, .session],
                blockStacked: true, requireWhere: true,
                allowFilesystem: false, maxRows: 1000
            )
        case .migrations:
            return QueryPolicy(
                preset: .migrations,
                allowed: [
                    .select, .inspect,
                    .insert, .update, .delete, .merge,
                    .create, .alter, .drop, .truncate, .rename,
                    .transaction, .session, .procedure, .maintenance,
                ],
                blockStacked: false, requireWhere: true,
                allowFilesystem: false, maxRows: nil
            )
        case .unrestricted:
            return QueryPolicy(
                preset: .unrestricted,
                allowed: StatementCategory.allCases,
                blockStacked: false, requireWhere: false,
                allowFilesystem: true, maxRows: nil
            )
        case .custom:
            return QueryPolicy(
                preset: .custom,
                allowed: [.select, .inspect],
                blockStacked: true, requireWhere: false,
                allowFilesystem: false, maxRows: nil
            )
        }
    }

    static func `default`(for environment: Environment) -> QueryPolicy {
        switch environment {
        case .production, .staging: return .make(.readOnly)
        case .development, .local:  return .make(.readWrite)
        }
    }

    // MARK: - JSON helpers

    func toJSON() -> String? {
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        guard let data = try? encoder.encode(self) else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func fromJSON(_ string: String?) -> QueryPolicy? {
        guard let string, let data = string.data(using: .utf8) else { return nil }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try? decoder.decode(QueryPolicy.self, from: data)
    }

    // MARK: - Mutations

    mutating func toggle(_ category: StatementCategory) {
        if allowed.contains(category) {
            allowed.removeAll { $0 == category }
        } else {
            allowed.append(category)
        }
        preset = .custom
    }

    mutating func apply(preset newPreset: QueryPreset) {
        self = QueryPolicy.make(newPreset)
    }
}

// ── Connection ────────────────────────────────────────────────────────────────

struct Connection: Identifiable, Equatable {
    let id: String
    var name: String
    var type: String                  // adapter id (postgres/mysql/sqlite/linear/…)
    var config: [String: String]      // service-specific, mirrors the TS config blob
    var environment: Environment
    var readOnly: Bool
    var queryPolicy: QueryPolicy
    var token: String
    var createdAt: String

    var mcpURL: String { "http://localhost:4242/mcp/\(token)" }

    /// MCP client key: slugified name + environment, so agents can tell e.g.
    /// `marketing-db-local` from `marketing-db-production` at a glance.
    var mcpKey: String {
        let slug = name.lowercased()
            .components(separatedBy: CharacterSet.alphanumerics.inverted)
            .filter { !$0.isEmpty }
            .joined(separator: "-")
        return "\(slug)-\(environment.rawValue)"
    }

    /// Resolves to a known DB type, or nil for non-database adapters.
    var connectionType: ConnectionType? { ConnectionType(rawValue: type) }

    var typeLabel: String { connectionType?.label ?? type.capitalized }
}

struct ConnectionDraft {
    var name: String = ""
    var type: String = "postgres"
    var config: [String: String] = [:]
    var environment: Environment = .development
    var queryPolicy: QueryPolicy = .make(.readWrite)
    var policyKind: String = "sql"     // "sql" | "action"
    /// For action-policy adapters (Linear, …): whether writes are allowed.
    var allowWrite: Bool = true

    /// Field definitions for the selected adapter — set by the form from the
    /// catalog. Drives both rendering and type-correct JSON serialization.
    var fields: [ConfigFieldDef] = []

    init() {}

    init(from conn: Connection) {
        name = conn.name
        type = conn.type
        config = conn.config
        environment = conn.environment
        queryPolicy = conn.queryPolicy
        allowWrite = !conn.readOnly
    }

    func value(_ key: String) -> String { config[key] ?? "" }

    /// Switch adapter type and seed defaults for the new adapter's fields.
    mutating func setType(_ newType: String, fields newFields: [ConfigFieldDef]) {
        type = newType
        fields = newFields
        var seeded: [String: String] = [:]
        for f in newFields where f.defaultValue != nil {
            seeded[f.key] = f.defaultValue
        }
        config = seeded
    }

    mutating func setEnvironment(_ newEnv: Environment) {
        environment = newEnv
        // Only update policy if it hasn't been customized
        if queryPolicy.preset != .custom {
            queryPolicy = .default(for: newEnv)
        }
    }
}

// ── Group ───────────────────────────────────────────────────────────────────
// A group fronts several integrations behind one MCP endpoint. The server
// aggregates every member's tools under one server, namespaced by member.

// A group member: an integration plus optional config overrides scoped to this
// group (e.g. a Linear member with a per-group `team_key`). Overrides are kept as
// strings; the server coerces them to each field's declared type.
struct GroupMember: Identifiable, Equatable {
    let id: String
    var overrides: [String: String]

    init(id: String, overrides: [String: String] = [:]) {
        self.id = id
        self.overrides = overrides
    }
}

struct ConnectionGroup: Identifiable, Equatable {
    let id: String
    var name: String
    /// Optional: a group may span environments (prod + staging + local), so it
    /// need not be tied to one. `nil` means unscoped/mixed.
    var environment: Environment?
    var members: [GroupMember]
    var token: String
    var createdAt: String

    var memberIds: [String] { members.map(\.id) }
    func member(_ id: String) -> GroupMember? { members.first { $0.id == id } }

    var mcpURL: String { "http://localhost:4242/mcp/\(token)" }

    /// MCP client key: slugified name, suffixed with the environment only when the
    /// group is scoped to one (so a mixed group reads `db-prod`, not `db-prod-`).
    var mcpKey: String {
        let slug = name.lowercased()
            .components(separatedBy: CharacterSet.alphanumerics.inverted)
            .filter { !$0.isEmpty }
            .joined(separator: "-")
        guard let environment else { return slug }
        return "\(slug)-\(environment.rawValue)"
    }
}
