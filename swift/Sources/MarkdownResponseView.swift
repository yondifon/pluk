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
        VStack(alignment: .leading, spacing: max(Space.md, lineSpacing * 3)) {
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
    var fontSize: CGFloat = 12
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
                HStack(alignment: .top, spacing: Space.md) {
                    if !embedded {
                        Text(gutter)
                            .font(.system(size: fontSize, design: .monospaced))
                            .foregroundStyle(Surface.tertiaryLabel)
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
                .padding(.horizontal, Space.md)
                .padding(.vertical, Space.sm)
            }
            .scrollIndicators(.visible)
        }
        .modifier(CodeBlockChrome(embedded: embedded))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(language) code, \(lineCount) lines")
        .task(id: code) { await rebuild() }
    }

    private var header: some View {
        HStack(spacing: Space.sm) {
            Text(language.lowercased())
                .scaledFont(.caption, weight: .semibold, design: .monospaced)
                .foregroundStyle(.secondary)
            Spacer(minLength: Space.sm)
            Button(copied ? "Copied" : "Copy Code") { copyCode() }
                .buttonStyle(.pointer)
                .scaledFont(.callout)
                .foregroundStyle(copied ? .green : Color.accentColor)
                .accessibilityLabel(copied ? "Code copied" : "Copy code")
        }
        .padding(.horizontal, Space.md)
        .padding(.vertical, Space.sm)
    }

    private func rebuild() async {
        let src = code
        let lang = language
        let n = max(1, src.components(separatedBy: "\n").count)
        lineCount = n
        gutter = (1...n).map(String.init).joined(separator: "\n")
        // Highlighting scales with payload size; keep it off the main actor so a
        // large block never blocks scrolling or resizing.
        highlighted = await Task.detached { CodeStyle.highlighted(src, language: lang) }.value
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
            content.codeBlockSurface(cornerRadius: Radius.small)
        }
    }
}

// Highlighting lives in `CodeStyle` (SyntaxHighlight.swift): one scanner pass
// for the whole block, colors only, so the hosting Text owns point size.
