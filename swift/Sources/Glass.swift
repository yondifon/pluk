import SwiftUI
import AppKit

// Liquid Glass design helpers. Real glass (refraction, morphing, specular) is
// macOS 26+; on earlier systems we approximate with vibrancy + frosted material
// so the look degrades gracefully rather than going flat.

/// Behind-window vibrancy. Gives every glass/material surface real content to
/// refract, which is what makes Liquid Glass read as glass instead of gray.
struct VisualEffectView: NSViewRepresentable {
    var material: NSVisualEffectView.Material = .underWindowBackground
    var blending: NSVisualEffectView.BlendingMode = .behindWindow

    func makeNSView(context: Context) -> NSVisualEffectView {
        let view = NSVisualEffectView()
        view.material = material
        view.blendingMode = blending
        view.state = .active
        return view
    }

    func updateNSView(_ view: NSVisualEffectView, context: Context) {
        view.material = material
        view.blendingMode = blending
    }
}

/// Groups nearby glass shapes so they blend/morph as one fluid surface (macOS 26);
/// a no-op passthrough on earlier systems.
struct GlassGroup<Content: View>: View {
    var spacing: CGFloat
    @ViewBuilder var content: Content

    init(spacing: CGFloat = 10, @ViewBuilder content: () -> Content) {
        self.spacing = spacing
        self.content = content()
    }

    var body: some View {
        if #available(macOS 26.0, *) {
            GlassEffectContainer(spacing: spacing) { content }
        } else {
            content
        }
    }
}

extension View {
    /// Translucent app/window backing.
    func glassWindowBackground() -> some View {
        background(VisualEffectView(material: .underWindowBackground).ignoresSafeArea())
    }

    /// Translucent backing for sheets/panels — slightly more opaque so dense
    /// content (forms, logs) stays legible over it.
    func glassPanelBackground() -> some View {
        background(VisualEffectView(material: .menu).ignoresSafeArea())
    }
}

// MARK: - Shared surfaces & rows

extension View {
    /// Liquid Glass card surface on macOS 26+, with a solid fallback for earlier systems.
    @ViewBuilder
    func cardSurface(cornerRadius: CGFloat = 8) -> some View {
        if #available(macOS 26.0, *) {
            self.glassEffect(.regular, in: .rect(cornerRadius: cornerRadius))
        } else {
            // Frosted-glass approximation: material over the window's vibrancy
            // plus a hairline edge for the glass-rim highlight.
            self
                .background(.ultraThinMaterial, in: .rect(cornerRadius: cornerRadius))
                .overlay(
                    RoundedRectangle(cornerRadius: cornerRadius)
                        .stroke(Color.primary.opacity(0.06), lineWidth: 0.5)
                )
        }
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
                .font(.system(size: 11, weight: .semibold))
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
            .font(.system(size: 12, design: .monospaced))
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
                .font(.system(size: 12))
                .foregroundColor(.secondary)
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
