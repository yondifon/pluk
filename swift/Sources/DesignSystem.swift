import AppKit
import SwiftUI

// MARK: - Radii

enum Radius {
    static let small: CGFloat = 6
    static let medium: CGFloat = 10
    static let large: CGFloat = 14
}

// MARK: - Spacing

enum Space {
    static let xxs: CGFloat = 2
    static let xs: CGFloat = 4
    static let sm: CGFloat = 8
    static let md: CGFloat = 12
    static let lg: CGFloat = 16
    static let xl: CGFloat = 24
    static let xxl: CGFloat = 32
}

// MARK: - Control metrics

enum Control {
    static let height: CGFloat = 22
}

// MARK: - Surfaces

enum Surface {
    static let sidebar = Color(nsColor: sidebarColor)
    static let content = Color(nsColor: contentColor)
    static let panel = Color(nsColor: panelColor)
    static let sunken = Color.primary.opacity(0.05)

    static let sidebarColor = neutral(light: 0.925, dark: 0.105)
    static let contentColor = neutral(light: 0.965, dark: 0.140)
    static let panelColor = neutral(light: 1.000, dark: 0.180)

    private static func neutral(light: CGFloat, dark: CGFloat) -> NSColor {
        NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            return NSColor(white: isDark ? dark : light, alpha: 1)
        }
    }
}

// MARK: - Local fills

extension Color {
    static let cardFill = Color.primary.opacity(0.035)
    static let hairline = Color.primary.opacity(0.07)
    static let edge = Color.primary.opacity(0.13)
    static let controlFill = Color.primary.opacity(0.05)
}

extension View {
    func card(radius: CGFloat = Radius.medium) -> some View {
        background(Color.cardFill, in: RoundedRectangle(cornerRadius: radius, style: .continuous))
    }

    func rowSeparator(inset: CGFloat = Space.md) -> some View {
        overlay(alignment: .bottom) {
            Rectangle()
                .fill(Color.hairline)
                .frame(height: 0.5)
                .padding(.leading, inset)
        }
    }

    func cardSurface(cornerRadius: CGFloat = Radius.medium) -> some View {
        card(radius: cornerRadius)
    }

    func codeBlockSurface(cornerRadius: CGFloat = Radius.small) -> some View {
        self.background(
            Surface.sunken,
            in: RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
        )
    }
}

struct GlassGroup<Content: View>: View {
    @ViewBuilder var content: Content

    init(spacing: CGFloat = Space.lg, @ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View { content }
}

// MARK: - Typeface

enum Typeface {
    case data
    case mono

    static let current: Typeface = .data

    static func design(_ requested: Font.Design) -> Font.Design {
        current == .mono ? .monospaced : requested
    }
}

// MARK: - UI scale

private struct UIScaleKey: EnvironmentKey {
    static let defaultValue: CGFloat = 1
}

extension EnvironmentValues {
    var uiScale: CGFloat {
        get { self[UIScaleKey.self] }
        set { self[UIScaleKey.self] = newValue }
    }
}

private enum TextStyleSize {
    static func base(_ style: Font.TextStyle) -> CGFloat {
        switch style {
        case .largeTitle: 26
        case .title: 22
        case .title2: 17
        case .title3: 15
        case .headline: 13
        case .body: 13
        case .callout: 12
        case .subheadline: 11
        case .footnote: 10
        case .caption: 10
        case .caption2: 10
        @unknown default: 13
        }
    }

    static func weight(_ style: Font.TextStyle) -> Font.Weight {
        style == .headline ? .semibold : .regular
    }
}

extension Font {
    static func scaled(
        _ style: Font.TextStyle,
        scale: CGFloat,
        weight: Font.Weight? = nil,
        design: Font.Design = .default
    ) -> Font {
        .system(
            size: TextStyleSize.base(style) * scale,
            weight: weight ?? TextStyleSize.weight(style),
            design: Typeface.design(design)
        )
    }

    static func dev(size: CGFloat, weight: Weight = .regular) -> Font {
        let style: Font.TextStyle
        switch size {
        case ..<10: style = .caption2
        case ..<11: style = .caption
        case ..<12: style = .footnote
        case ..<13: style = .callout
        case ..<14: style = .subheadline
        default: style = .body
        }
        return .system(style, design: .monospaced, weight: weight)
    }

    static func mono(_ size: CGFloat, weight: Weight = .regular) -> Font {
        .dev(size: size, weight: weight)
    }
}

private struct ScaledFont: ViewModifier {
    @SwiftUI.Environment(\.uiScale) private var uiScale

    let style: Font.TextStyle
    let weight: Font.Weight?
    let design: Font.Design
    let monospacedDigit: Bool

    func body(content: Content) -> some View {
        var font = Font.scaled(style, scale: uiScale, weight: weight, design: design)
        if monospacedDigit { font = font.monospacedDigit() }
        return content.font(font)
    }
}

extension View {
    func scaledFont(
        _ style: Font.TextStyle,
        weight: Font.Weight? = nil,
        design: Font.Design = .default,
        monospacedDigit: Bool = false
    ) -> some View {
        modifier(ScaledFont(style: style, weight: weight, design: design, monospacedDigit: monospacedDigit))
    }
}

struct SectionLabel: View {
    let text: String

    var body: some View {
        Text(text.uppercased())
            .scaledFont(.caption2, weight: .semibold, design: .monospaced)
            .tracking(0.6)
            .foregroundStyle(.tertiary)
    }
}

// MARK: - Controls

struct IconButton: View {
    let symbol: String
    let label: String
    var tint: AnyShapeStyle = AnyShapeStyle(.secondary)
    var rotation: Angle = .zero
    let action: () -> Void

    @SwiftUI.Environment(\.uiScale) private var uiScale

    var body: some View {
        Button(action: action) {
            Image(systemName: symbol)
                .rotationEffect(rotation)
                .frame(width: 24 * uiScale, height: 24 * uiScale)
                .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .foregroundStyle(tint)
        .help(label)
        .accessibilityLabel(label)
    }
}

struct CopyIconButton: View {
    let text: String
    var label: String = "Copy"
    var symbol: String = "doc.on.doc"

    @State private var copied = false

    var body: some View {
        IconButton(
            symbol: copied ? "checkmark" : symbol,
            label: copied ? "Copied" : label,
            tint: copied ? AnyShapeStyle(Color.green) : AnyShapeStyle(.secondary)
        ) {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text, forType: .string)
            copied = true
            Task {
                try? await Task.sleep(for: .seconds(1.2))
                copied = false
            }
        }
    }
}

// MARK: - Status

enum ConnStatus {
    case ok, failing, unknown

    var color: Color {
        switch self {
        case .ok: .green
        case .failing: .red
        case .unknown: .secondary
        }
    }

    var label: String {
        switch self {
        case .ok: "Healthy"
        case .failing: "Failing"
        case .unknown: "Not checked"
        }
    }
}

struct StatusChip: View {
    let status: ConnStatus
    var checkedAt: Double?
    var detail: String?

    var body: some View {
        HStack(spacing: Space.xs + 1) {
            Circle()
                .fill(status.color)
                .frame(width: 6, height: 6)
            Text(status.label)
                .scaledFont(.caption)
                .foregroundStyle(status == .failing ? Color.red : .secondary)
            if let ago = relativeTime {
                Text(ago)
                    .font(.mono(10))
                    .foregroundStyle(.tertiary)
            }
        }
        .padding(.horizontal, Space.sm)
        .frame(height: Control.height)
        .background(Color.controlFill, in: Capsule())
        .help(detail ?? status.label)
        .accessibilityElement(children: .combine)
    }

    private var relativeTime: String? {
        guard let checkedAt else { return nil }
        let seconds = Int(Date().timeIntervalSince1970 - checkedAt / 1000)
        guard seconds >= 0 else { return nil }
        if seconds < 60 { return "\(max(seconds, 1))s ago" }
        if seconds < 3600 { return "\(seconds / 60)m ago" }
        if seconds < 86_400 { return "\(seconds / 3600)h ago" }
        return "\(seconds / 86_400)d ago"
    }
}

struct OverflowMenu<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        Menu {
            content
        } label: {
            Image(systemName: "ellipsis")
                .scaledFont(.callout)
                .frame(width: Control.height, height: Control.height)
                .contentShape(Rectangle())
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .frame(width: Control.height, height: Control.height)
        .help("More actions")
    }
}

// MARK: - Tabs

struct PillTabs<T: Hashable>: View {
    let tabs: [T]
    let title: (T) -> String
    @Binding var selection: T

    var body: some View {
        HStack(spacing: Space.xs) {
            ForEach(tabs, id: \.self) { tab in
                Button { selection = tab } label: {
                    Text(title(tab))
                        .scaledFont(.callout)
                        .fontWeight(selection == tab ? .semibold : .regular)
                        .foregroundStyle(selection == tab ? Color.primary : .secondary)
                        .padding(.horizontal, Space.md - 2)
                        .padding(.vertical, Space.xs + 1)
                        .background {
                            if selection == tab {
                                RoundedRectangle(cornerRadius: Radius.small, style: .continuous)
                                    .fill(Color.controlFill)
                            }
                        }
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            Spacer()
        }
        .padding(.horizontal, Space.xl - Space.xs)
        .padding(.bottom, Space.sm)
    }
}

struct Tag: View {
    let text: String
    var systemImage: String?
    var tint: Color?

    var body: some View {
        HStack(spacing: Space.xs) {
            if let systemImage {
                Image(systemName: systemImage)
                    .font(.system(size: 9, weight: .semibold))
            }
            Text(text)
                .scaledFont(.caption)
        }
        .foregroundStyle(tint ?? .secondary)
        .padding(.horizontal, Space.sm - 1)
        .padding(.vertical, Space.xxs + 1)
        .background((tint ?? Color.primary).opacity(0.07), in: Capsule())
    }
}

// MARK: - Detail layout

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
                .scaledFont(.caption, weight: .semibold)
                .foregroundStyle(.secondary)
                .padding(.horizontal, Space.xs)
            VStack(spacing: 0) {
                content
            }
            .cardSurface()
        }
    }
}

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
                .scaledFont(.callout)
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
