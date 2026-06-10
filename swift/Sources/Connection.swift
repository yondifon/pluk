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
        case .readOnly:     "SELECT and EXPLAIN only. Recommended for production."
        case .readWrite:    "Read + INSERT/UPDATE/DELETE. Requires WHERE on mutations."
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
        case .inspect:     "EXPLAIN / SHOW / DESCRIBE"
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
    var type: ConnectionType
    // Basic
    var host: String?
    var port: Int?
    var user: String?
    var password: String?
    var database: String?
    var filename: String?
    var socketPath: String?
    // SSH
    var useSSH: Bool
    var sshHost: String?
    var sshPort: Int?
    var sshUser: String?
    var sshAuthType: SSHAuthType
    var sshKeyPath: String?
    var sshPassword: String?
    // SSL
    var useSSL: Bool
    var sslMode: SSLMode
    var sslCAPath: String?
    var sslCertPath: String?
    var sslKeyPath: String?
    // Meta
    var environment: Environment
    var readOnly: Bool
    var queryPolicy: QueryPolicy
    var token: String
    var createdAt: String

    var mcpURL: String { "http://localhost:4242/mcp/\(token)" }

    var mcpKey: String {
        name.lowercased()
            .components(separatedBy: .whitespacesAndNewlines)
            .filter { !$0.isEmpty }
            .joined(separator: "-")
    }
}

struct ConnectionDraft {
    // Basic
    var name: String = ""
    var type: ConnectionType = .postgres
    var host: String = "localhost"
    var port: String = "5432"
    var user: String = ""
    var password: String = ""
    var database: String = ""
    var filename: String = ""
    var socketPath: String = ""
    // SSH
    var useSSH: Bool = false
    var sshHost: String = ""
    var sshPort: String = "22"
    var sshUser: String = ""
    var sshAuthType: SSHAuthType = .agent
    var sshKeyPath: String = ""
    var sshPassword: String = ""
    // SSL
    var useSSL: Bool = false
    var sslMode: SSLMode = .require
    var sslCAPath: String = ""
    var sslCertPath: String = ""
    var sslKeyPath: String = ""
    // Meta
    var environment: Environment = .development
    var readOnly: Bool = false
    var queryPolicy: QueryPolicy = .make(.readWrite)

    init() {}

    init(from conn: Connection) {
        name = conn.name
        type = conn.type
        host = conn.host ?? "localhost"
        port = conn.port.map(String.init) ?? String(conn.type.defaultPort)
        user = conn.user ?? ""
        password = conn.password ?? ""
        database = conn.database ?? ""
        filename = conn.filename ?? ""
        socketPath = conn.socketPath ?? ""
        useSSH = conn.useSSH
        sshHost = conn.sshHost ?? ""
        sshPort = conn.sshPort.map(String.init) ?? "22"
        sshUser = conn.sshUser ?? ""
        sshAuthType = conn.sshAuthType
        sshKeyPath = conn.sshKeyPath ?? ""
        sshPassword = conn.sshPassword ?? ""
        useSSL = conn.useSSL
        sslMode = conn.sslMode
        sslCAPath = conn.sslCAPath ?? ""
        sslCertPath = conn.sslCertPath ?? ""
        sslKeyPath = conn.sslKeyPath ?? ""
        environment = conn.environment
        readOnly = conn.readOnly
        queryPolicy = conn.queryPolicy
    }

    mutating func setType(_ newType: ConnectionType) {
        type = newType
        port = newType == .sqlite ? "" : String(newType.defaultPort)
    }

    mutating func setEnvironment(_ newEnv: Environment) {
        environment = newEnv
        // Only update policy if it hasn't been customized
        if queryPolicy.preset != .custom {
            queryPolicy = .default(for: newEnv)
        }
    }
}
