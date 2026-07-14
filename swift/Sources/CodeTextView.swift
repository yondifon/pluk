import AppKit
import SwiftUI

/// AppKit-backed code viewer. TextKit renders large payloads natively — smooth
/// scrolling, cheap font/line-height changes, and real text selection — which a
/// SwiftUI `Text` (one view per line, re-highlighted on every render) cannot do
/// once responses get big. Used by the full response viewer.
struct CodeTextView: NSViewRepresentable {
    let code: String
    var language: String = "text"
    var fontSize: CGFloat = 13
    var lineSpacing: CGFloat = 4

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSScrollView {
        let textView = NSTextView()
        textView.isEditable = false
        textView.isSelectable = true
        textView.isRichText = false
        textView.drawsBackground = false
        textView.textColor = .labelColor
        textView.textContainerInset = NSSize(width: 6, height: 8)
        textView.isHorizontallyResizable = true
        textView.isVerticallyResizable = true
        textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude,
                                  height: CGFloat.greatestFiniteMagnitude)
        textView.autoresizingMask = []
        if let container = textView.textContainer {
            container.widthTracksTextView = false     // don't wrap — scroll long lines
            container.containerSize = NSSize(width: CGFloat.greatestFiniteMagnitude,
                                             height: CGFloat.greatestFiniteMagnitude)
            container.lineFragmentPadding = 4
        }

        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.drawsBackground = false
        scroll.documentView = textView

        let ruler = LineNumberRulerView(textView: textView)
        scroll.verticalRulerView = ruler
        scroll.hasVerticalRuler = true
        scroll.hasHorizontalRuler = false
        scroll.rulersVisible = true

        scroll.contentView.postsBoundsChangedNotifications = true
        context.coordinator.textView = textView
        context.coordinator.ruler = ruler
        context.coordinator.observe(contentView: scroll.contentView)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let textView = context.coordinator.textView else { return }
        let font = NSFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)

        // Re-run the regex only when the source changes. Font/leading tweaks just
        // re-apply attributes over the cached colored string — no re-highlight.
        if context.coordinator.lastCode != code {
            context.coordinator.colored = CodeHighlighter.attributed(code)
            context.coordinator.lastCode = code
        }

        let paragraph = NSMutableParagraphStyle()
        paragraph.lineSpacing = lineSpacing

        let result = NSMutableAttributedString(attributedString: context.coordinator.colored)
        let full = NSRange(location: 0, length: result.length)
        result.addAttribute(.font, value: font, range: full)
        result.addAttribute(.paragraphStyle, value: paragraph, range: full)
        textView.textStorage?.setAttributedString(result)

        if let ruler = context.coordinator.ruler {
            ruler.font = font
            ruler.updateThickness(lineCount: (code as NSString).lineCount)
            ruler.needsDisplay = true
        }
    }

    final class Coordinator {
        var textView: NSTextView?
        var ruler: LineNumberRulerView?
        var lastCode: String?
        var colored = NSAttributedString()
        private var observer: NSObjectProtocol?

        // The ruler must repaint as the view scrolls so line numbers track.
        func observe(contentView: NSClipView) {
            observer = NotificationCenter.default.addObserver(
                forName: NSView.boundsDidChangeNotification,
                object: contentView,
                queue: .main
            ) { [weak self] _ in self?.ruler?.needsDisplay = true }
        }

        deinit {
            if let observer { NotificationCenter.default.removeObserver(observer) }
        }
    }
}

// MARK: - Line number gutter

/// A left ruler that draws source line numbers aligned to each line fragment.
/// Numbers live outside the text storage, so copy/selection stay clean.
final class LineNumberRulerView: NSRulerView {
    private weak var tv: NSTextView?
    var font: NSFont = .monospacedSystemFont(ofSize: 13, weight: .regular)

    init(textView: NSTextView) {
        self.tv = textView
        super.init(scrollView: textView.enclosingScrollView, orientation: .verticalRuler)
        clientView = textView
        ruleThickness = 40
    }

    required init(coder: NSCoder) { fatalError("init(coder:) unavailable") }

    func updateThickness(lineCount: Int) {
        let digits = max(2, String(lineCount).count)
        let width = font.maximumAdvancement.width * CGFloat(digits) + 14
        let clamped = max(30, width)
        if abs(clamped - ruleThickness) > 0.5 { ruleThickness = clamped }
    }

    override func drawHashMarksAndLabels(in rect: NSRect) {
        guard let tv, let layout = tv.layoutManager, let container = tv.textContainer else { return }
        let content = tv.string as NSString
        guard content.length > 0 else { return }

        let insetY = tv.textContainerInset.height
        let originY = convert(NSPoint.zero, from: tv).y
        let attrs: [NSAttributedString.Key: Any] = [
            .font: font,
            .foregroundColor: NSColor.tertiaryLabelColor,
        ]

        let visible = tv.visibleRect
        let glyphRange = layout.glyphRange(forBoundingRect: visible, in: container)
        let charRange = layout.characterRange(forGlyphRange: glyphRange, actualGlyphRange: nil)

        // Line number of the first visible line.
        var line = 1
        if charRange.location > 0 {
            content.enumerateSubstrings(
                in: NSRange(location: 0, length: charRange.location),
                options: [.byLines, .substringNotRequired]
            ) { _, _, _, _ in line += 1 }
        }

        var charIndex = charRange.location
        let end = NSMaxRange(charRange)
        while charIndex < end {
            let lineRange = content.lineRange(for: NSRange(location: charIndex, length: 0))
            let glyphIndex = layout.glyphIndexForCharacter(at: lineRange.location)
            var effective = NSRange()
            let fragment = layout.lineFragmentRect(forGlyphAt: glyphIndex, effectiveRange: &effective)
            let label = "\(line)" as NSString
            let size = label.size(withAttributes: attrs)
            let y = fragment.minY + insetY + originY + (fragment.height - size.height) / 2
            label.draw(at: NSPoint(x: ruleThickness - size.width - 6, y: y), withAttributes: attrs)

            line += 1
            let next = NSMaxRange(lineRange)
            if next <= charIndex { break }
            charIndex = next
        }
    }
}

// MARK: - Coloring

/// Produces an `NSAttributedString` with token colors only (no font) so the
/// caller can apply the reader's point size without re-running the regex.
/// Mirrors the SwiftUI `SyntaxHighlighter` token rules.
enum CodeHighlighter {
    private static let pattern = #"(?<comment>//.*|#.*|/\*.*\*/)|(?<string>"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`[^`]*`)|(?<number>\b\d+(?:\.\d+)?\b)|(?<keyword>\b(?:actor|async|await|break|case|catch|class|const|continue|def|else|enum|export|extension|false|final|for|func|function|guard|if|import|in|interface|let|match|new|null|private|protocol|public|return|self|static|struct|switch|throw|throws|true|try|type|var|void|while|with|yield)\b)"#

    static func attributed(_ code: String) -> NSMutableAttributedString {
        let result = NSMutableAttributedString(string: code)
        guard let regex = try? NSRegularExpression(pattern: pattern) else { return result }
        let range = NSRange(location: 0, length: (code as NSString).length)
        regex.enumerateMatches(in: code, range: range) { match, _, _ in
            guard let match else { return }
            let color: NSColor
            if match.range(withName: "comment").location != NSNotFound {
                color = .secondaryLabelColor
            } else if match.range(withName: "string").location != NSNotFound {
                color = .systemOrange
            } else if match.range(withName: "number").location != NSNotFound {
                color = .systemPurple
            } else {
                color = .controlAccentColor
            }
            result.addAttribute(.foregroundColor, value: color, range: match.range)
        }
        return result
    }
}

private extension NSString {
    var lineCount: Int {
        var count = 0
        enumerateSubstrings(
            in: NSRange(location: 0, length: length),
            options: [.byLines, .substringNotRequired]
        ) { _, _, _, _ in count += 1 }
        return max(1, count)
    }
}
