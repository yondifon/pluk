import SwiftUI
import AppKit

// MARK: - Developer-tool typography

extension Font {
    /// SF Mono at a given size/weight, mapped to scalable text styles so the
    /// dense dev-tool UI still respects Dynamic Type. Used for config values,
    /// code blocks, status text, labels, and any other surface that should read
    /// like a developer tool rather than a consumer app.
    static func dev(size: CGFloat, weight: Weight = .regular) -> Font {
        let style: Font.TextStyle
        switch size {
        case ..<10:  style = .caption2
        case ..<11:  style = .caption
        case ..<12:  style = .footnote
        case ..<13:  style = .callout
        case ..<14:  style = .subheadline
        default:     style = .body
        }
        return .system(style, design: .monospaced, weight: weight)
    }
}

/// Layout passthrough kept for call-site compatibility — no vibrancy/glass.
struct GlassGroup<Content: View>: View {
    @ViewBuilder var content: Content

    init(spacing: CGFloat = 10, @ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View { content }
}

extension Color {
    /// Uniform page surface — white in light mode, content-dark in dark mode.
    /// Window, sidebar, detail, sheets, and section cards all share this so the
    /// app reads as one continuous surface, not gray-page-with-white-cards.
    static let pageSurface = Color(nsColor: .textBackgroundColor)

    /// A gently "projected" fill for content we want to set apart — code blocks,
    /// config snippets, data tables. Just a hair off the page so it reads as a
    /// recessed panel, never a hard slab.
    static let projectedSurface = Color.secondary.opacity(0.06)
}

extension View {
    /// Uniform white/dark page backing for the window.
    func glassWindowBackground() -> some View {
        background(Color.pageSurface.ignoresSafeArea())
    }

    /// Uniform page backing for sheets/panels.
    func glassPanelBackground() -> some View {
        background(Color.pageSurface.ignoresSafeArea())
    }
}

// MARK: - Shared surfaces & rows

extension View {
    /// Section card — the uniform page fill delineated by a hairline, so groups
    /// read as one surface with the rest of the app rather than a lighter slab.
    func cardSurface(cornerRadius: CGFloat = 8) -> some View {
        self
            .background(
                Color.pageSurface,
                in: .rect(cornerRadius: min(cornerRadius, 5))
            )
            .overlay(
                RoundedRectangle(cornerRadius: min(cornerRadius, 5), style: .continuous)
                    .stroke(Color.primary.opacity(0.10), lineWidth: 0.5)
            )
    }

    /// Projected surface for code / data blocks (config snippets, result tables)
    /// — a slight grey fill, no border, so the content reads as a recessed panel
    /// inside its card, never a card-in-a-card.
    func codeBlockSurface(cornerRadius: CGFloat = 6) -> some View {
        self.background(
            Color.projectedSurface,
            in: RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
        )
    }
}

/// A titled inspector section: a restrained label over a flat group of rows.
struct DetailSection<Content: View>: View {
    let title: String
    let content: Content

    init(_ title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(title)
                .font(.dev(size: 10, weight: .semibold))
                .foregroundColor(.secondary)
                .textCase(.uppercase)
                .padding(.bottom, 6)
            VStack(spacing: 0) {
                content
            }
            .cardSurface()
        }
    }
}

/// A labeled row inside a `DetailSection`: a fixed-width label column, the value
/// or editable control, and a hairline divider. `labelWidth`/`dividerInset` let
/// the wider form rows reuse the same template as the read-only inspector rows.
struct InspectorRow<Content: View>: View {
    let label: String
    let labelWidth: CGFloat
    let dividerInset: CGFloat
    let content: Content

    init(_ label: String, value: String) where Content == Text {
        self.label = label
        self.labelWidth = 86
        self.dividerInset = 106
        self.content = Text(value)
            .font(.dev(size: 12))
            .foregroundColor(.primary)
    }

    init(_ label: String, labelWidth: CGFloat = 86, dividerInset: CGFloat = 106, @ViewBuilder content: () -> Content) {
        self.label = label
        self.labelWidth = labelWidth
        self.dividerInset = dividerInset
        self.content = content()
    }

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            Text(label)
                .font(.dev(size: 11, weight: .semibold))
                .foregroundColor(.secondary)
                .textCase(.uppercase)
                .frame(width: labelWidth, alignment: .leading)
            content
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .overlay(alignment: .bottom) {
            Divider().padding(.leading, dividerInset)
        }
    }
}
