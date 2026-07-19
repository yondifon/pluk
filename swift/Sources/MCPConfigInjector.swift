import Foundation

// MARK: - Config scope + format

// Where a client's MCP config lives. Project = a per-repo file (chosen folder);
// global = the client's single user-level file.
enum ConfigScope: String, CaseIterable, Identifiable {
    case project, global

    var id: String { rawValue }

    var label: String {
        switch self {
        case .project: "Project"
        case .global:  "Global"
        }
    }
}

enum ConfigFormat { case json, toml }

// MARK: - Injector

// Writes an integration's server entry straight into a client's config file so
// the user doesn't copy/paste and merge by hand. Reads any existing file, keeps
// every other server, and never overwrites: if the key is already there it skips
// and reports it. JSON files are reflowed (comments dropped) after a `.bak`
// backup; the Codex TOML file is appended to.
//
// Merge/sanitize live as pure String→String funcs so they can be unit-tested
// without a UI.
enum MCPConfigInjector {
    enum InjectResult {
        case added(path: String)    // entry written
        case skipped(path: String)  // key already present; file untouched
    }

    enum InjectError: LocalizedError {
        case parseFailed(path: String)
        case write(path: String, underlying: String)

        var errorDescription: String? {
            switch self {
            case .parseFailed(let p): "Couldn't parse the existing config at \(p)."
            case .write(let p, let e): "Couldn't write \(p): \(e)"
            }
        }
    }

    // MARK: Entry point

    static func inject(client: MCPClient, scope: ConfigScope,
                       projectDir: String?, key: String, url: String) throws -> InjectResult {
        let path = resolvePath(client: client, scope: scope, projectDir: projectDir)
        let existing = try? String(contentsOfFile: path, encoding: .utf8)

        switch client.format {
        case .json:
            return try injectJSON(path: path, container: client.containerKey,
                                  key: key, entry: client.entryObject(url: url),
                                  existing: existing)
        case .toml:
            return try injectTOML(path: path, key: key, url: url, existing: existing)
        }
    }

    // MARK: Path resolution

    static func resolvePath(client: MCPClient, scope: ConfigScope, projectDir: String?) -> String {
        let raw = client.configPath(scope)
        if scope == .project, let dir = projectDir {
            return (dir as NSString).appendingPathComponent(raw)
        }
        return (raw as NSString).expandingTildeInPath
    }

    // MARK: JSON

    static func injectJSON(path: String, container: String, key: String,
                           entry: [String: Any], existing: String?) throws -> InjectResult {
        // Read the file, find the server map, add the entry, write it back.
        var root: [String: Any] = [:]
        if let text = existing, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            guard let dict = parseObject(text) else { throw InjectError.parseFailed(path: path) }
            root = dict
        }

        var servers = (root[container] as? [String: Any]) ?? [:]
        if servers[key] != nil {
            return .skipped(path: path)
        }
        servers[key] = entry
        root[container] = servers

        try backupAndWrite(path: path, contents: prettyJSON(root), hadExisting: existing != nil)
        return .added(path: path)
    }

    // Standard JSON first; only if that fails, retry tolerating the comments and
    // trailing commas that .jsonc files (e.g. opencode) allow.
    static func parseObject(_ text: String) -> [String: Any]? {
        if let obj = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any] {
            return obj
        }
        return try? JSONSerialization.jsonObject(with: Data(sanitizeJSONC(text).utf8)) as? [String: Any]
    }

    // MARK: TOML (Codex)

    static func injectTOML(path: String, key: String, url: String, existing: String?) throws -> InjectResult {
        let header = "[mcp_servers.\(key)]"
        if let text = existing, tomlHasTable(text, header: header) {
            return .skipped(path: path)
        }
        let block = "\(header)\nurl = \"\(url)\"\n"
        let output: String
        if let text = existing, !text.isEmpty {
            output = text + (text.hasSuffix("\n") ? "\n" : "\n\n") + block
        } else {
            output = block
        }
        try backupAndWrite(path: path, contents: output, hadExisting: existing != nil)
        return .added(path: path)
    }

    // A table header on its own line (ignoring surrounding spaces), so we don't
    // false-match a nested `[mcp_servers.foo.bar]` or a commented-out line.
    static func tomlHasTable(_ text: String, header: String) -> Bool {
        text.components(separatedBy: .newlines)
            .contains { $0.trimmingCharacters(in: .whitespaces) == header }
    }

    // MARK: Write + backup

    static func backupAndWrite(path: String, contents: String, hadExisting: Bool) throws {
        let fm = FileManager.default
        let dir = (path as NSString).deletingLastPathComponent
        if !dir.isEmpty {
            try? fm.createDirectory(atPath: dir, withIntermediateDirectories: true)
        }
        if hadExisting, fm.fileExists(atPath: path) {
            let bak = path + ".bak"
            try? fm.removeItem(atPath: bak)
            try? fm.copyItem(atPath: path, toPath: bak)
        }
        do {
            try Data(contents.utf8).write(to: URL(fileURLWithPath: path), options: .atomic)
        } catch {
            throw InjectError.write(path: path, underlying: error.localizedDescription)
        }
    }

    // MARK: JSON pretty-print

    // Foundation escapes forward slashes ("http:\/\/…"); undo that so URLs read
    // naturally. Sorted keys keep the merged output deterministic.
    static func prettyJSON(_ obj: [String: Any]) -> String {
        guard let data = try? JSONSerialization.data(
                withJSONObject: obj, options: [.prettyPrinted, .sortedKeys]),
              let str = String(data: data, encoding: .utf8) else {
            return "{}\n"
        }
        return str.replacingOccurrences(of: "\\/", with: "/") + "\n"
    }

    // MARK: JSONC sanitizer

    // JSONSerialization rejects the comments and trailing commas that opencode /
    // cursor configs allow. Strip line/block comments (string-aware) and trailing
    // commas so we can parse-then-reflow. Not a full JSON5 parser — enough for
    // real config files.
    static func sanitizeJSONC(_ text: String) -> String {
        var out = [Character]()
        out.reserveCapacity(text.count)
        let chars = Array(text)
        var inString = false
        var escaped = false
        var i = 0
        while i < chars.count {
            let c = chars[i]
            if inString {
                out.append(c)
                if escaped { escaped = false }
                else if c == "\\" { escaped = true }
                else if c == "\"" { inString = false }
                i += 1
                continue
            }
            if c == "\"" {
                inString = true
                out.append(c)
                i += 1
                continue
            }
            if c == "/", i + 1 < chars.count {
                let next = chars[i + 1]
                if next == "/" {                       // line comment
                    i += 2
                    while i < chars.count, chars[i] != "\n" { i += 1 }
                    continue
                }
                if next == "*" {                       // block comment
                    i += 2
                    while i + 1 < chars.count, !(chars[i] == "*" && chars[i + 1] == "/") { i += 1 }
                    i += 2
                    continue
                }
            }
            out.append(c)
            i += 1
        }
        return stripTrailingCommas(String(out))
    }

    // Drop a comma that only whitespace separates from a closing } or ]. Kept
    // string-aware so a comma inside a value survives.
    static func stripTrailingCommas(_ text: String) -> String {
        let chars = Array(text)
        var keep = [Character]()
        keep.reserveCapacity(chars.count)
        var inString = false
        var escaped = false
        var i = 0
        while i < chars.count {
            let c = chars[i]
            if inString {
                keep.append(c)
                if escaped { escaped = false }
                else if c == "\\" { escaped = true }
                else if c == "\"" { inString = false }
                i += 1
                continue
            }
            if c == "\"" { inString = true; keep.append(c); i += 1; continue }
            if c == "," {
                var j = i + 1
                while j < chars.count, chars[j] == " " || chars[j] == "\n"
                        || chars[j] == "\t" || chars[j] == "\r" { j += 1 }
                if j < chars.count, chars[j] == "}" || chars[j] == "]" {
                    i += 1   // skip the trailing comma
                    continue
                }
            }
            keep.append(c)
            i += 1
        }
        return String(keep)
    }
}
