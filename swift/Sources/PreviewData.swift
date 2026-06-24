#if DEBUG
import Foundation
import SwiftUI

// Self-contained sample data for SwiftUI previews. Kept behind DEBUG so it never
// ships in release builds and never depends on live services, disk state, or
// network calls.

extension ConfigFieldDef {
    init(
        key: String,
        label: String,
        type: String,
        group: String? = nil,
        required: Bool? = nil,
        secret: Bool? = nil,
        placeholder: String? = nil,
        fileTypes: [String]? = nil,
        options: [FieldOption]? = nil,
        showIf: ShowIf? = nil,
        defaultValue: String? = nil
    ) {
        self.key = key
        self.label = label
        self.type = type
        self.group = group
        self.required = required
        self.secret = secret
        self.placeholder = placeholder
        self.fileTypes = fileTypes
        self.options = options
        self.showIf = showIf
        self.defaultValue = defaultValue
    }
}

extension Connection {
    static let sample = Connection(
        id: "conn-prod-pg",
        name: "Production DB",
        type: "postgres",
        config: [
            "host": "db.example.com",
            "port": "5432",
            "user": "pluk",
            "database": "app",
            "use_ssl": "true",
            "ssl_mode": "require",
        ],
        environment: .production,
        readOnly: true,
        queryPolicy: .default(for: .production),
        token: "pluk_prod_pg_token",
        createdAt: "2024-01-15 09:00:00"
    )

    static let sampleGroupMember = Connection(
        id: "conn-linear",
        name: "Linear Workspace",
        type: "linear",
        config: [
            "api_key": "lin_api_xxxx",
            "team_key": "ENG",
        ],
        environment: .production,
        readOnly: false,
        queryPolicy: .make(.readWrite),
        token: "pluk_linear_token",
        createdAt: "2024-02-10 14:30:00"
    )
}

extension ConnectionGroup {
    static let sample = ConnectionGroup(
        id: "group-api",
        name: "API Services",
        environment: .production,
        members: [GroupMember(id: Connection.sample.id, overrides: [:])],
        token: "pluk_group_api_token",
        createdAt: "2024-03-01 10:00:00"
    )
}

extension AdapterManifest {
    static let samplePostgres = AdapterManifest(
        id: "postgres",
        label: "PostgreSQL",
        category: "database",
        policyKind: "sql",
        agentHint: "Use this connection for read-only analytics queries.",
        actions: nil,
        configFields: [
            ConfigFieldDef(key: "host", label: "Host", type: "text", group: "Connection", required: true, placeholder: "localhost"),
            ConfigFieldDef(key: "port", label: "Port", type: "number", group: "Connection", required: true, placeholder: "5432"),
            ConfigFieldDef(key: "user", label: "User", type: "text", group: "Connection", required: true, placeholder: "postgres"),
            ConfigFieldDef(key: "password", label: "Password", type: "password", group: "Connection", required: true, placeholder: "••••••"),
            ConfigFieldDef(key: "database", label: "Database", type: "text", group: "Connection", required: true, placeholder: "postgres"),
            ConfigFieldDef(key: "use_ssl", label: "Use SSL", type: "toggle", group: "SSL", defaultValue: "true"),
            ConfigFieldDef(key: "ssl_mode", label: "SSL Mode", type: "select", group: "SSL", options: [
                FieldOption(value: "disable", label: "Disable"),
                FieldOption(value: "require", label: "Require"),
            ], defaultValue: "require"),
        ]
    )

    static let sampleLinear = AdapterManifest(
        id: "linear",
        label: "Linear",
        category: "issue-tracker",
        policyKind: "action",
        agentHint: "Manage issues and projects.",
        actions: [
            AdapterAction(name: "search_issues", category: "read"),
            AdapterAction(name: "create_issue", category: "write"),
        ],
        configFields: [
            ConfigFieldDef(key: "api_key", label: "API Key", type: "password", required: true, placeholder: "lin_api_…"),
            ConfigFieldDef(key: "team_key", label: "Team Key", type: "text", required: false, placeholder: "ENG"),
        ]
    )
}

extension ConnHealth {
    static let ok = ConnHealth(status: "ok", error: nil, at: 0)
    static let error = ConnHealth(status: "error", error: "connection refused", at: 0)
}
#endif
