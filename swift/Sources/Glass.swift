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

extension View {
    /// Solid default window backing (white in light mode).
    func glassWindowBackground() -> some View {
        background(Color(nsColor: .windowBackgroundColor).ignoresSafeArea())
    }

    /// Solid backing for sheets/panels.
    func glassPanelBackground() -> some View {
        background(Color(nsColor: .windowBackgroundColor).ignoresSafeArea())
    }
}

// MARK: - Shared surfaces & rows

extension View {
    /// Solid card surface — a regular control-background fill with a hairline edge.
    func cardSurface(cornerRadius: CGFloat = 8) -> some View {
        self
            .background(
                Color(nsColor: .controlBackgroundColor),
                in: .rect(cornerRadius: cornerRadius)
            )
            .overlay(
                RoundedRectangle(cornerRadius: cornerRadius)
                    .stroke(Color.primary.opacity(0.08), lineWidth: 0.5)
            )
    }

    /// Inset surface for code / data blocks (config snippets, result tables) —
    /// a subtle translucent fill + hairline so they read as a distinct block over
    /// the card without the opaque slab a solid window color would paint.
    func codeBlockSurface(cornerRadius: CGFloat = 6) -> some View {
        self
            .background(
                Color.secondary.opacity(0.05),
                in: RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
            )
            .overlay(
                RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                    .stroke(Color.secondary.opacity(0.15), lineWidth: 1)
            )
    }
}

/// A titled card: uppercase section label over a `cardSurface()`-wrapped group of
/// rows. Shared by the detail inspector and the add/edit forms so sections look
/// identical across surfaces.
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
                .font(.dev(size: 11, weight: .semibold))
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
