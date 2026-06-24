import Foundation

// Mirrors the TS adapter manifest exposed at GET /api/adapters. The form renders
// itself from this catalog, so adding a backend adapter needs no Swift changes.

struct FieldOption: Codable, Hashable {
    let value: String
    let label: String
}

struct ShowIf: Codable, Hashable {
    let key: String
    let equals: String   // normalized to string ("true"/"false"/value)

    enum CodingKeys: String, CodingKey { case key, equals }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        key = try c.decode(String.self, forKey: .key)
        if let b = try? c.decode(Bool.self, forKey: .equals) {
            equals = b ? "true" : "false"
        } else {
            equals = (try? c.decode(String.self, forKey: .equals)) ?? ""
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(key, forKey: .key)
        try c.encode(equals, forKey: .equals)
    }
}

struct ConfigFieldDef: Codable, Identifiable, Hashable {
    let key: String
    let label: String
    let type: String            // text | password | number | file | select | toggle
    var group: String?
    var required: Bool?
    var secret: Bool?
    var placeholder: String?
    var fileTypes: [String]?
    var options: [FieldOption]?
    var showIf: ShowIf?
    var defaultValue: String?

    var id: String { key }

    enum CodingKeys: String, CodingKey {
        case key, label, type, group, required, secret, placeholder
        case fileTypes, options, showIf
        case defaultValue = "default"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        key = try c.decode(String.self, forKey: .key)
        label = try c.decode(String.self, forKey: .label)
        type = try c.decode(String.self, forKey: .type)
        group = try? c.decode(String.self, forKey: .group)
        required = try? c.decode(Bool.self, forKey: .required)
        secret = try? c.decode(Bool.self, forKey: .secret)
        placeholder = try? c.decode(String.self, forKey: .placeholder)
        fileTypes = try? c.decode([String].self, forKey: .fileTypes)
        options = try? c.decode([FieldOption].self, forKey: .options)
        showIf = try? c.decode(ShowIf.self, forKey: .showIf)
        // `default` may be string, number, or bool — normalize to string.
        if let s = try? c.decode(String.self, forKey: .defaultValue) {
            defaultValue = s
        } else if let i = try? c.decode(Int.self, forKey: .defaultValue) {
            defaultValue = String(i)
        } else if let b = try? c.decode(Bool.self, forKey: .defaultValue) {
            defaultValue = b ? "true" : "false"
        } else {
            defaultValue = nil
        }
    }
}

// One tool an action adapter exposes, with its policy category. Lets the form
// describe — per adapter — what Write unlocks and what a read-only integration
// hides from the agent.
struct AdapterAction: Codable, Hashable {
    let name: String
    let category: String        // read | write | delete | admin
}

struct AdapterManifest: Codable, Identifiable, Hashable {
    let id: String
    let label: String
    let category: String
    let policyKind: String       // "sql" | "action" | "none"
    let agentHint: String?
    var actions: [AdapterAction]?
    let configFields: [ConfigFieldDef]

    var isSQL: Bool { policyKind == "sql" }
    var isAction: Bool { policyKind == "action" }
    var hasPolicy: Bool { policyKind != "none" }

    /// Tool names the Write permission unlocks (write + delete categories), in
    /// declaration order. Hidden from the agent entirely in read-only mode.
    var writeActionNames: [String] {
        var names: [String] = []
        for a in actions ?? [] where a.category == "write" || a.category == "delete" {
            names.append(a.name)
        }
        return names
    }

    /// Fields grouped in declaration order, preserving first-seen group order.
    var groupedFields: [(group: String, fields: [ConfigFieldDef])] {
        var order: [String] = []
        var byGroup: [String: [ConfigFieldDef]] = [:]
        for f in configFields {
            let g = f.group ?? "General"
            if byGroup[g] == nil { order.append(g) }
            byGroup[g, default: []].append(f)
        }
        return order.map { ($0, byGroup[$0] ?? []) }
    }
}

struct AdapterCatalogResponse: Codable {
    let adapters: [AdapterManifest]
}
