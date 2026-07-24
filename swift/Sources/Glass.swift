import SwiftUI
import AppKit

// Shared surfaces and rows. Design tokens (spacing, radii, type, fills) live in
// Theme.swift — this file only composes them into the two structural pieces the
// detail screens are built from: a titled section and a labeled row.

// MARK: - Typography

extension Font {
    /// Monospace, for machine data. Kept as the short name the dense screens
    /// (logs, config values, code) already call; `.mono` is the same font.
    static func dev(size: CGFloat, weight: Weight = .regular) -> Font {
        .mono(size, weight: weight)
    }
}

/// Layout passthrough kept for call-site compatibility — no vibrancy/glass.
struct GlassGroup<Content: View>: View {
    @ViewBuilder var content: Content

    init(spacing: CGFloat = Space.lg, @ViewBuilder content: () -> Content) {
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

// MARK: - Shared surfaces

extension View {
    /// Section card — a soft fill instead of a stroked box. Grouping comes from
    /// the fill and the space around it, so the screen holds fewer lines.
    func cardSurface(cornerRadius: CGFloat = Radius.md) -> some View {
        card(radius: cornerRadius)
    }

    /// Projected surface for code / data blocks (config snippets, result tables)
    /// — a slight grey fill, no border, so the content reads as a recessed panel
    /// inside its card, never a card-in-a-card.
    func codeBlockSurface(cornerRadius: CGFloat = Radius.sm) -> some View {
        self.background(
            Color.projectedSurface,
            in: RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
        )
    }
}

/// A titled inspector section: a quiet sentence-case header over a flat group of
/// rows. The header is weight-and-color hierarchy, not uppercase tracking.
struct DetailSection<Content: View>: View {
    let title: String
    let content: Content

    init(_ title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Space.sm) {
            Text(title)
                .font(.uiSection)
                .foregroundStyle(.secondary)
                .padding(.horizontal, Space.xs)
            VStack(spacing: 0) {
                content
            }
            .cardSurface()
        }
    }
}

/// A labeled row inside a `DetailSection`: a fixed-width label column and the
/// value or editable control. No separator — the card fill and the row rhythm
/// carry the grouping, which keeps dense screens from turning into a grid of
/// rules. `labelWidth`/`dividerInset` let the wider form rows reuse the same
/// template as the read-only inspector rows.
struct InspectorRow<Content: View>: View {
    let label: String
    let labelWidth: CGFloat
    let content: Content

    init(_ label: String, value: String) where Content == Text {
        self.label = label
        self.labelWidth = 88
        self.content = Text(value)
            .font(.mono(12))
            .foregroundStyle(.primary)
    }

    init(_ label: String, labelWidth: CGFloat = 88, dividerInset: CGFloat = 0, @ViewBuilder content: () -> Content) {
        self.label = label
        self.labelWidth = labelWidth
        self.content = content()
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: Space.md) {
            Text(label)
                .font(.uiLabel)
                .foregroundStyle(.secondary)
                .frame(width: labelWidth, alignment: .leading)
            content
                .textSelection(.enabled)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, Space.md)
        .padding(.vertical, Space.sm + 1)
    }
}
