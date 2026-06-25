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

// ── Tool config ───────────────────────────────────────────────────────────────
// The unified policy model: each adapter tool is toggled on/off and may carry its
// own settings. Persisted in the `query_policy` column as
// `{ "tools": { "<name>": { "enabled": Bool, "settings": { … } } } }`.

struct ToolState: Equatable {
    var enabled: Bool
    /// Setting key → string value; coerced to the declared type on serialize.
    var settings: [String: String]

    init(enabled: Bool, settings: [String: String] = [:]) {
        self.enabled = enabled
        self.settings = settings
    }
}

extension AdapterToolDef {
    /// A fresh tool state seeded from this tool's declared defaults.
    func seededState() -> ToolState {
        var s: [String: String] = [:]
        for f in settings ?? [] where f.defaultValue != nil { s[f.key] = f.defaultValue }
        return ToolState(enabled: defaultEnabled, settings: s)
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
    var toolConfig: [String: ToolState]
    var token: String
    var createdAt: String

    var mcpURL: String { PlukServer.mcpURL(token: token) }

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
    var policyKind: String = "sql"     // "sql" | "action" | "none"

    /// Per-tool enable + settings, keyed by tool name.
    var toolConfig: [String: ToolState] = [:]

    /// Field definitions for the selected adapter — set by the form from the
    /// catalog. Drives both rendering and type-correct JSON serialization.
    var fields: [ConfigFieldDef] = []
    /// Tool definitions for the selected adapter, used to render the Tools section
    /// and to type-coerce each tool's settings on serialize.
    var tools: [AdapterToolDef] = []

    init() {}

    init(from conn: Connection) {
        name = conn.name
        type = conn.type
        config = conn.config
        environment = conn.environment
        toolConfig = conn.toolConfig
    }

    func value(_ key: String) -> String { config[key] ?? "" }

    /// Adopt an adapter manifest: store its field + tool defs, and seed any tool
    /// not already configured with its declared default (so editing a connection
    /// saved before a tool existed still shows it).
    mutating func adopt(_ manifest: AdapterManifest, resetConfig: Bool) {
        type = manifest.id
        policyKind = manifest.policyKind
        fields = manifest.configFields
        tools = manifest.tools
        if resetConfig {
            var seededCfg: [String: String] = [:]
            for f in manifest.configFields where f.defaultValue != nil { seededCfg[f.key] = f.defaultValue }
            config = seededCfg
            toolConfig = [:]
        }
        for t in manifest.tools where toolConfig[t.name] == nil {
            toolConfig[t.name] = t.seededState()
        }
        applyEnvironmentDefaults()
    }

    /// SQL connections in a writable environment default the query tool to allow
    /// mutations; production/staging stay read-only. Only applied while the query
    /// tool is still at its seeded read-only default (never overrides a choice).
    private mutating func applyEnvironmentDefaults() {
        guard policyKind == "sql", var q = toolConfig["query"] else { return }
        if (q.settings["mode"] ?? "read-only") == "read-only",
           environment == .development || environment == .local {
            q.settings["mode"] = "mutations"
            toolConfig["query"] = q
        }
    }

    mutating func setEnvironment(_ newEnv: Environment) {
        environment = newEnv
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

    var mcpURL: String { PlukServer.mcpURL(token: token) }

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
