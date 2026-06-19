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
