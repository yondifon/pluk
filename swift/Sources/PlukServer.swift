import Foundation

// Single source of truth for the local pluk server endpoint. The TS server binds
// to this port on loopback (see pluk/src/server.ts) — keep the two in sync.
enum PlukServer {
    static let port = 4242
    static let baseURL = "http://localhost:\(port)"

    static func api(_ path: String) -> String { "\(baseURL)/api/\(path)" }
    static func mcpURL(token: String) -> String { "\(baseURL)/mcp/\(token)" }
}
