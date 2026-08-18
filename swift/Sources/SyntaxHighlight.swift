import AppKit
import SwiftUI

// MARK: - Language

/// The languages Pluk's code surfaces actually render. Fenced blocks with any
/// other label fall back to plain text — unknown input is shown uncolored,
/// never mis-highlighted.
enum CodeLanguage {
    case text, json, toml, sql, shell

    init(_ hint: String) {
        switch hint.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() {
        case "json": self = .json
        case "toml": self = .toml
        case "sql": self = .sql
        case "shell", "bash", "sh": self = .shell
        default: self = .text
        }
    }
}

// MARK: - Tint palette

/// Semantic token colors. Each is a dynamic light/dark pair tuned for Pluk's
/// near-white and near-dark content surfaces, deliberately distinct from the
/// green/orange/red status palette so a tint can never read as a verdict.
enum SyntaxTint {
    case comment, string, number, keyword, type, property

    var color: Color { Color(nsColor: nsColor) }

    var nsColor: NSColor {
        NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            let c = isDark ? darkRGB : lightRGB
            return NSColor(srgbRed: c.r, green: c.g, blue: c.b, alpha: 1)
        }
    }

    private var lightRGB: (r: CGFloat, g: CGFloat, b: CGFloat) {
        switch self {
        case .comment:  (0.45, 0.49, 0.53)
        case .string:   (0.05, 0.43, 0.40)
        case .number:   (0.43, 0.31, 0.69)
        case .keyword:  (0.60, 0.20, 0.36)
        case .type:     (0.28, 0.36, 0.71)
        case .property: (0.13, 0.38, 0.65)
        }
    }

    private var darkRGB: (r: CGFloat, g: CGFloat, b: CGFloat) {
        switch self {
        case .comment:  (0.62, 0.65, 0.69)
        case .string:   (0.45, 0.76, 0.72)
        case .number:   (0.71, 0.62, 0.94)
        case .keyword:  (0.87, 0.60, 0.72)
        case .type:     (0.65, 0.68, 0.96)
        case .property: (0.52, 0.69, 0.92)
        }
    }
}

// MARK: - Scanner

/// A single token region plus the tint it carries. Plain runs are never
/// emitted — they are everything between spans and stay uncolored.
struct SyntaxSpan {
    let range: Range<String.Index>
    let tint: SyntaxTint
}

/// Single-pass, O(n) scanner: walks the source once, consuming whole tokens,
/// and returns only the spans worth tinting.
enum SyntaxScanner {
    static func spans(in source: String, language: CodeLanguage) -> [SyntaxSpan] {
        guard let dialect = Dialect.forLanguage[language] else { return [] }
        return scan(source, dialect: dialect)
    }

    private struct Dialect {
        let lineComments: [String]
        let blockComments: Bool
        let quotes: Set<Character>
        let backslashEscapes: Bool   // json "..." and toml basic strings
        let doubledQuotes: Bool      // sql '' escape
        let tripleQuotes: Bool       // toml """ and '''
        let wordExtras: Set<Character>   // chars allowed mid-word (toml - .)
        let keywords: Set<String>
        let types: Set<String>
        let caseInsensitive: Bool
        let shell: Bool
        let propertyPrefix: Character?   // json ":", toml "="
        let headers: Bool                // toml [section]

        static let forLanguage: [CodeLanguage: Dialect] = [
            .json: Dialect(
                lineComments: [], blockComments: false, quotes: ["\""],
                backslashEscapes: true, doubledQuotes: false, tripleQuotes: false,
                wordExtras: [], keywords: ["true", "false", "null"], types: [],
                caseInsensitive: false, shell: false, propertyPrefix: ":", headers: false
            ),
            .toml: Dialect(
                lineComments: ["#"], blockComments: false, quotes: ["\"", "'"],
                backslashEscapes: true, doubledQuotes: false, tripleQuotes: true,
                wordExtras: ["-", "."], keywords: ["true", "false"], types: [],
                caseInsensitive: false, shell: false, propertyPrefix: "=", headers: true
            ),
            .sql: Dialect(
                lineComments: ["--"], blockComments: true, quotes: ["'", "\""],
                backslashEscapes: false, doubledQuotes: true, tripleQuotes: false,
                wordExtras: [], keywords: sqlKeywords, types: sqlTypes,
                caseInsensitive: true, shell: false, propertyPrefix: nil, headers: false
            ),
            .shell: Dialect(
                lineComments: ["#"], blockComments: false, quotes: ["'", "\""],
                backslashEscapes: true, doubledQuotes: false, tripleQuotes: false,
                wordExtras: ["-", ".", "/", "=", ":", "@"],
                keywords: shellCommands, types: [],
                caseInsensitive: false, shell: true, propertyPrefix: nil, headers: false
            ),
        ]

        static let shellCommands: Set<String> = [
            "cd", "echo", "head", "cat", "chmod", "cp", "curl", "docker",
            "git", "grep", "kill", "ls", "make", "mv", "open", "pwd", "rm",
            "sed", "ssh", "sudo", "tail", "tar", "touch", "uname", "whoami",
        ]

        static let sqlKeywords: Set<String> = [
            "select", "from", "where", "insert", "into", "values", "update",
            "set", "delete", "create", "drop", "alter", "table", "view",
            "index", "join", "inner", "left", "right", "outer", "full",
            "cross", "natural", "on", "using", "and", "or", "not", "is",
            "null", "distinct", "group", "order", "by", "having", "limit",
            "offset", "union", "all", "intersect", "except", "exists",
            "between", "like", "ilike", "in", "case", "when", "then", "else",
            "end", "with", "recursive", "returning", "primary", "foreign",
            "key", "references", "constraint", "default", "unique", "check",
            "add", "column", "rename", "if", "true", "false", "begin",
            "commit", "rollback", "transaction", "explain", "analyze", "as",
            "asc", "desc", "nulls", "first", "last", "window", "partition",
            "over", "cast", "collate",
        ]

        static let sqlTypes: Set<String> = [
            "integer", "int", "bigint", "smallint", "tinyint", "serial",
            "bigserial", "numeric", "decimal", "real", "double", "float",
            "money", "text", "varchar", "char", "character", "boolean",
            "bool", "timestamp", "timestamptz", "datetime", "date", "time",
            "timetz", "interval", "uuid", "json", "jsonb", "bytea", "blob",
            "binary", "varbinary", "bit",
        ]
    }

    private static func scan(_ source: String, dialect: Dialect) -> [SyntaxSpan] {
        var spans: [SyntaxSpan] = []
        var i = source.startIndex
        let end = source.endIndex

        func append(_ range: Range<String.Index>, _ tint: SyntaxTint) {
            spans.append(SyntaxSpan(range: range, tint: tint))
        }

        while i < end {
            let ch = source[i]

            if ch.isWhitespace {
                i = source.index(after: i)
                continue
            }

            // Line comments: "#" (toml), "--" (sql).
            if dialect.lineComments.contains(where: { source[i...].hasPrefix($0) }) {
                let start = i
                while i < end, !source[i].isNewline { i = source.index(after: i) }
                append(start..<i, .comment)
                continue
            }

            // Block comments (sql).
            if dialect.blockComments, ch == "/" {
                let next = source.index(after: i)
                if next < end, source[next] == "*" {
                    let start = i
                    i = source.index(after: next)
                    while i < end {
                        if source[i] == "*" {
                            let after = source.index(after: i)
                            if after < end, source[after] == "/" {
                                i = source.index(after: after)
                                break
                            }
                        }
                        i = source.index(after: i)
                    }
                    append(start..<i, .comment)
                    continue
                }
            }

            // Strings.
            if dialect.quotes.contains(ch) {
                let start = i
                var j = source.index(after: i)

                if dialect.tripleQuotes, source[i...].hasPrefix("\(ch)\(ch)\(ch)") {
                    func triple(at idx: String.Index) -> Bool {
                        var k = idx
                        for _ in 0..<3 {
                            guard k < end, source[k] == ch else { return false }
                            k = source.index(after: k)
                        }
                        return true
                    }
                    j = source.index(i, offsetBy: 3)
                    while j < end, !triple(at: j) { j = source.index(after: j) }
                    if j < end { j = source.index(j, offsetBy: 3) }
                    append(start..<j, .string)
                    i = j
                    continue
                }

                while j < end {
                    let c = source[j]
                    if c == ch {
                        if dialect.doubledQuotes {
                            let after = source.index(after: j)
                            if after < end, source[after] == ch {
                                j = source.index(after: after)
                            } else {
                                j = after
                                break
                            }
                        } else {
                            j = source.index(after: j)
                            break
                        }
                    } else if dialect.backslashEscapes, c == "\\" {
                        let after = source.index(after: j)
                        j = after < end ? source.index(after: after) : after
                    } else {
                        j = source.index(after: j)
                    }
                }

                // A string directly followed by the property separator is a
                // key, not a value — json "key":, toml "key" =.
                if let prefix = dialect.propertyPrefix {
                    var k = j
                    while k < end, source[k].isWhitespace { k = source.index(after: k) }
                    if k < end, source[k] == prefix {
                        append(start..<j, .property)
                        i = j
                        continue
                    }
                }
                append(start..<j, .string)
                i = j
                continue
            }

            // Numbers, including the leading sign.
            let signNext = source.index(after: i)
            if ch.isNumber
                || ((ch == "-" || ch == "+") && signNext < end && source[signNext].isNumber) {
                let start = i
                if ch == "-" || ch == "+" { i = source.index(after: i) }
                while i < end, source[i].isNumber { i = source.index(after: i) }
                if i < end, source[i] == "." {
                    let after = source.index(after: i)
                    if after < end, source[after].isNumber {
                        i = source.index(after: after)
                        while i < end, source[i].isNumber { i = source.index(after: i) }
                    }
                }
                if i < end, source[i] == "e" || source[i] == "E" {
                    let after = source.index(after: i)
                    let expSign = source.index(after: after)
                    if after < end,
                       source[after].isNumber
                       || ((source[after] == "-" || source[after] == "+")
                           && expSign < end && source[expSign].isNumber) {
                        i = source.index(after: after)
                        while i < end, source[i].isNumber { i = source.index(after: i) }
                    }
                }
                append(start..<i, .number)
                continue
            }

            // Words: keywords, types, or (json/toml) property keys.
            if ch.isLetter || ch == "_" {
                let start = i
                while i < end {
                    let c = source[i]
                    if c.isLetter || c.isNumber || c == "_" || dialect.wordExtras.contains(c) {
                        i = source.index(after: i)
                    } else {
                        break
                    }
                }
                let key = dialect.caseInsensitive
                    ? String(source[start..<i]).lowercased()
                    : String(source[start..<i])
                if dialect.keywords.contains(key) {
                    append(start..<i, dialect.shell ? .property : .keyword)
                } else if dialect.types.contains(key) {
                    append(start..<i, .type)
                } else if let prefix = dialect.propertyPrefix {
                    var k = i
                    while k < end, source[k].isWhitespace { k = source.index(after: k) }
                    if k < end, source[k] == prefix {
                        append(start..<i, .property)
                    }
                } else if dialect.headers {
                    var k = i
                    while k < end, source[k].isWhitespace { k = source.index(after: k) }
                    if k < end, source[k] == "]" {
                        append(start..<i, .property)
                    }
                }
                continue
            }

            i = source.index(after: i)
        }

        return spans
    }
}

// MARK: - Console output

/// Command output has no grammar, so it is never parsed as a language: one stray
/// quote or `#` in a log line would tint everything after it. Only the
/// line-level signals an operator scans for get a tint — the leading timestamp,
/// banner lines echoed between commands, and severity words.
enum ConsoleScanner {
    static func spans(in source: String) -> [SyntaxSpan] {
        var spans: [SyntaxSpan] = []
        var start = source.startIndex
        while start < source.endIndex {
            let end = source[start...].firstIndex(of: "\n") ?? source.endIndex
            scanLine(source, start..<end, into: &spans)
            start = end < source.endIndex ? source.index(after: end) : source.endIndex
        }
        return spans
    }

    private static let severity: Set<String> = [
        "denied", "err", "error", "errors", "exception", "fail", "failed",
        "failure", "fatal", "panic", "refused", "timeout", "unauthorized",
        "unhealthy", "warn", "warning",
    ]

    private static let banners = ["===", "---", "###"]

    private static func scanLine(_ s: String, _ line: Range<String.Index>, into spans: inout [SyntaxSpan]) {
        var i = line.lowerBound
        while i < line.upperBound, s[i] == " " || s[i] == "\t" { i = s.index(after: i) }
        guard i < line.upperBound else { return }

        if banners.contains(where: { s[i..<line.upperBound].hasPrefix($0) }) {
            spans.append(SyntaxSpan(range: i..<line.upperBound, tint: .property))
            return
        }

        var wordIndex = 0
        while i < line.upperBound {
            guard isWordChar(s[i]) else { i = s.index(after: i); continue }
            let start = i
            while i < line.upperBound, isWordChar(s[i]) { i = s.index(after: i) }
            let word = s[start..<i]
            // A date or clock in the first two words is the line's timestamp; the
            // same digits further in are data, not a prefix.
            if wordIndex < 2, isTimestamp(word) {
                spans.append(SyntaxSpan(range: start..<i, tint: .comment))
            } else if severity.contains(bareWord(word)) {
                spans.append(SyntaxSpan(range: start..<i, tint: .keyword))
            }
            wordIndex += 1
        }
    }

    private static func isWordChar(_ c: Character) -> Bool {
        c.isLetter || c.isNumber || c == "-" || c == ":" || c == "." || c == "_" || c == "+"
    }

    private static func isTimestamp(_ word: Substring) -> Bool {
        guard let first = word.first, first.isNumber else { return false }
        return word.contains(":") || word.contains("-")
    }

    // "failed:" and "ERROR." carry the same signal as the bare word.
    private static func bareWord(_ word: Substring) -> String {
        String(word.drop(while: { !$0.isLetter }).prefix(while: { $0.isLetter })).lowercased()
    }
}

// MARK: - Builder

/// Turns source text into color-only attributed text. No font or background
/// attributes are set, so the hosting Text keeps the reader's point size and
/// plain runs inherit the surrounding foreground.
enum CodeStyle {
    static func highlighted(_ source: String, language: CodeLanguage) -> AttributedString {
        attributed(source, spans: SyntaxScanner.spans(in: source, language: language))
    }

    /// Command output: tinted by `ConsoleScanner`'s line-level rules.
    static func console(_ source: String) -> AttributedString {
        attributed(source, spans: ConsoleScanner.spans(in: source))
    }

    private static func attributed(_ source: String, spans: [SyntaxSpan]) -> AttributedString {
        var result = AttributedString()
        var cursor = source.startIndex
        for span in spans {
            if span.range.lowerBound > cursor {
                result.append(AttributedString(source[cursor..<span.range.lowerBound]))
            }
            var piece = AttributedString(source[span.range])
            piece.foregroundColor = span.tint.color
            result.append(piece)
            cursor = span.range.upperBound
        }
        if cursor < source.endIndex {
            result.append(AttributedString(source[cursor...]))
        }
        return result
    }

    static func highlighted(_ source: String, language: String) -> AttributedString {
        highlighted(source, language: CodeLanguage(language))
    }

    /// AppKit variant for `CodeTextView`: colors only; the view owns the font.
    static func highlightedNS(_ source: String, language: CodeLanguage) -> NSAttributedString {
        let result = NSMutableAttributedString(string: source)
        for span in SyntaxScanner.spans(in: source, language: language) {
            result.addAttribute(.foregroundColor, value: span.tint.nsColor, range: NSRange(span.range, in: source))
        }
        return result
    }

    static func highlightedNS(_ source: String, language: String) -> NSAttributedString {
        highlightedNS(source, language: CodeLanguage(language))
    }
}
