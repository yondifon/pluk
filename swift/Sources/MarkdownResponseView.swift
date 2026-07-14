import AppKit
import SwiftUI

/// Renders agent-visible output without flattening Markdown into a raw text blob.
/// Fenced code blocks get a language label, stable line numbers, restrained syntax
/// colors, and a copy action while prose keeps Markdown's inline formatting.
struct MarkdownResponseView: View {
    let markdown: String
    /// Embedded mode drops each code block's own surface, header, and line
    /// numbers so it sits flat inside a host that already provides the card
    /// (config sections, log response blocks) — avoids surface-in-surface.
    var embedded: Bool = false
    /// Reader-controlled type so reviews stay legible: base point size and the
    /// extra leading between lines. Threaded to prose and code alike.
    var fontSize: CGFloat = 12
    var lineSpacing: CGFloat = 2

    // Parsed once per source string and cached — reader tweaks to size/leading
    // re-render but must never re-scan the (possibly huge) payload.
    @State private var blocks: [MarkdownBlock] = []

    var body: some View {
        VStack(alignment: .leading, spacing: max(10, lineSpacing * 3)) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                switch block {
                case .prose(let text):
                    if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        MarkdownProse(text: text, fontSize: fontSize, lineSpacing: lineSpacing)
                    }
                case .code(let language, let text):
                    MarkdownCodeBlock(language: language, code: text, embedded: embedded,
                                      fontSize: fontSize, lineSpacing: lineSpacing)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .task(id: markdown) { blocks = MarkdownBlock.parse(markdown) }
    }
}

/// Normalizes an agent's raw tool output into something readable for review.
/// JSON is pretty-printed with two-space indentation and wrapped in a fenced
/// block so it renders as line-numbered, syntax-highlighted code; already-fenced
/// Markdown is passed through untouched; anything else stays as-is.
enum ResponseFormatter {
    static func formatted(_ raw: String) -> String {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return raw }
        if raw.contains("```") { return raw }
        if let pretty = prettyJSON(trimmed) { return "```json\n\(pretty)\n```" }
        return raw
    }

    // Keys are sorted so the same object always prints the same way — a stable
    // shape matters more than original key order when diffing responses.
    static func prettyJSON(_ s: String) -> String? {
        guard let first = s.first, first == "{" || first == "[",
              let data = s.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data),
              let out = try? JSONSerialization.data(
                withJSONObject: obj,
                options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
              ),
              let str = String(data: out, encoding: .utf8)
        else { return nil }
        return str
    }
}

private enum MarkdownBlock {
    case prose(String)
    case code(language: String, text: String)

    static func parse(_ markdown: String) -> [MarkdownBlock] {
        let lines = markdown.components(separatedBy: "\n")
        var blocks: [MarkdownBlock] = []
        var prose: [String] = []
        var index = 0

        func flushProse() {
            guard !prose.isEmpty else { return }
            blocks.append(.prose(prose.joined(separator: "\n")))
            prose.removeAll(keepingCapacity: true)
        }

        while index < lines.count {
            let line = lines[index]
            guard line.hasPrefix("```") else {
                prose.append(line)
                index += 1
                continue
            }

            flushProse()
            let language = String(line.dropFirst(3))
                .trimmingCharacters(in: .whitespacesAndNewlines)
            index += 1

            var code: [String] = []
            while index < lines.count {
                if lines[index].trimmingCharacters(in: .whitespacesAndNewlines) == "```" {
                    index += 1
                    break
                }
                code.append(lines[index])
                index += 1
            }

            blocks.append(.code(
                language: language.isEmpty ? "text" : language,
                text: code.joined(separator: "\n")
            ))
        }

        flushProse()
        return blocks
    }
}

private struct MarkdownProse: View {
    let text: String
    var fontSize: CGFloat = 12
    var lineSpacing: CGFloat = 2

    private var attributedText: AttributedString {
        (try? AttributedString(markdown: text)) ?? AttributedString(text)
    }

    var body: some View {
        Text(attributedText)
            .font(.system(size: fontSize))
            .lineSpacing(lineSpacing)
            .foregroundStyle(.primary)
            .textSelection(.enabled)
            .frame(maxWidth: 760, alignment: .leading)
    }
}

private struct MarkdownCodeBlock: View {
    let language: String
    let code: String
    var embedded: Bool = false
    var fontSize: CGFloat = 11
    var lineSpacing: CGFloat = 2

    @State private var copied = false
    // The whole block renders as two Text views — a line-number gutter and one
    // highlighted code Text — instead of one view per line. Highlighting and the
    // gutter are built once per source (colors are size-independent), so resizing
    // only re-lays the two Texts, never re-runs the regex or rebuilds N rows.
    @State private var highlighted = AttributedString()
    @State private var gutter = ""
    @State private var lineCount = 1

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if !embedded { header }

            ScrollView(.horizontal) {
                HStack(alignment: .top, spacing: 12) {
                    if !embedded {
                        Text(gutter)
                            .font(.system(size: fontSize, design: .monospaced))
                            .foregroundStyle(.tertiary)
                            .lineSpacing(lineSpacing)
                            .multilineTextAlignment(.trailing)
                            .fixedSize()
                    }
                    Text(highlighted)
                        .font(.system(size: fontSize, design: .monospaced))
                        .lineSpacing(lineSpacing)
                        .fixedSize(horizontal: true, vertical: false)
                        .textSelection(.enabled)
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
            }
            .scrollIndicators(.visible)
        }
        .modifier(CodeBlockChrome(embedded: embedded))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(language) code, \(lineCount) lines")
        .task(id: code) { await rebuild() }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Text(language.lowercased())
                .font(.dev(size: 10, weight: .semibold))
                .foregroundStyle(.secondary)
            Spacer(minLength: 8)
            Button(copied ? "Copied" : "Copy Code") { copyCode() }
                .buttonStyle(.plain)
                .font(.dev(size: 10, weight: .medium))
                .foregroundStyle(copied ? .green : Color.accentColor)
                .accessibilityLabel(copied ? "Code copied" : "Copy code")
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .overlay(alignment: .bottom) { Divider().opacity(0.7) }
    }

    private func rebuild() async {
        let src = code
        let lang = language
        let n = max(1, src.components(separatedBy: "\n").count)
        lineCount = n
        gutter = (1...n).map(String.init).joined(separator: "\n")
        // Highlighting scales with payload size; keep it off the main actor so a
        // large block never blocks scrolling or resizing.
        highlighted = await Task.detached { SyntaxHighlighter.highlightAll(src, language: lang) }.value
    }

    private func copyCode() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(code, forType: .string)
        copied = true
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(1.5))
            copied = false
        }
    }
}

/// Standalone code blocks carry their own surface; embedded ones sit flat on
/// the host's card.
private struct CodeBlockChrome: ViewModifier {
    let embedded: Bool

    func body(content: Content) -> some View {
        if embedded {
            content
        } else {
            content.codeBlockSurface(cornerRadius: 6)
        }
    }
}

private enum SyntaxHighlighter {
    private static let pattern = #"(?<comment>//.*|#.*|/\*.*\*/)|(?<string>"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`[^`]*`)|(?<number>\b\d+(?:\.\d+)?\b)|(?<keyword>\b(?:actor|async|await|break|case|catch|class|const|continue|def|else|enum|export|extension|false|final|for|func|function|guard|if|import|in|interface|let|match|new|null|private|protocol|public|return|self|static|struct|switch|throw|throws|true|try|type|var|void|while|with|yield)\b)"#

    // Colors the whole block in one pass and joins with newlines, so the code
    // renders as a single Text. Called once per source and cached by the view.
    static func highlightAll(_ code: String, language: String) -> AttributedString {
        let lines = code.components(separatedBy: "\n")
        var result = AttributedString()
        for (i, line) in lines.enumerated() {
            result.append(highlight(line, language: language))
            if i < lines.count - 1 { result.append(AttributedString("\n")) }
        }
        return result
    }

    // No font is set here — the hosting Text owns point size and design so the
    // reader's chosen size wins. Only token colors are applied.
    static func highlight(_ line: String, language _: String) -> AttributedString {
        var result = AttributedString(line)

        guard let regex = try? NSRegularExpression(pattern: pattern) else {
            return result
        }

        let range = NSRange(line.startIndex..., in: line)
        regex.enumerateMatches(in: line, range: range) { match, _, _ in
            guard let match else { return }
            let kind: TokenKind
            if match.range(withName: "comment").location != NSNotFound {
                kind = .comment
            } else if match.range(withName: "string").location != NSNotFound {
                kind = .string
            } else if match.range(withName: "number").location != NSNotFound {
                kind = .number
            } else {
                kind = .keyword
            }

            let start = String.Index(utf16Offset: match.range.location, in: line)
            let end = String.Index(utf16Offset: match.range.location + match.range.length, in: line)
            guard let attributedStart = index(in: result, offset: line.distance(from: line.startIndex, to: start)),
                  let attributedEnd = index(in: result, offset: line.distance(from: line.startIndex, to: end)) else {
                return
            }

            result[attributedStart..<attributedEnd].foregroundColor = kind.color
        }

        return result
    }

    private static func index(in value: AttributedString, offset: Int) -> AttributedString.Index? {
        var index = value.startIndex
        for _ in 0..<offset {
            guard index < value.endIndex else { return nil }
            index = value.index(afterCharacter: index)
        }
        return index
    }

    private enum TokenKind {
        case comment, string, number, keyword

        var color: Color {
            switch self {
            case .comment: return .secondary
            case .string: return .orange
            case .number: return .purple
            case .keyword: return .accentColor
            }
        }
    }
}
