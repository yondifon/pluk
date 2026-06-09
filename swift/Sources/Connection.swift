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
    }

    mutating func setType(_ newType: ConnectionType) {
        type = newType
        port = newType == .sqlite ? "" : String(newType.defaultPort)
    }
}
