import SwiftUI

// The app's design tokens and shared primitives. Every spacing, radius, type
// size, and surface used in the UI comes from here — a screen that needs a new
// value adds it to the scale rather than inventing a one-off number, so the
// whole app keeps one rhythm.

// MARK: - Spacing

/// 4pt scale. `md` is the default gap inside a control, `lg` inside a card,
/// `xl` between sections and around a page.
enum Space {
    static let xxs: CGFloat = 2
    static let xs: CGFloat = 4
    static let sm: CGFloat = 8
    static let md: CGFloat = 12
    static let lg: CGFloat = 16
    static let xl: CGFloat = 24
    static let xxl: CGFloat = 32
}

/// Corner radii. `sm` for chips and inline blocks, `md` for cards and fields,
/// `lg` for floating surfaces (toasts, popovers).
enum Radius {
    static let sm: CGFloat = 6
    static let md: CGFloat = 10
    static let lg: CGFloat = 14
}

// MARK: - Typography

extension Font {
    /// System font for everything a person reads as language: titles, labels,
    /// descriptions, buttons. Monospace is reserved for machine data (see
    /// `.mono`), which is what makes that data stand out at all.
    static let uiTitle = Font.system(size: 17, weight: .semibold)
    static let uiHeadline = Font.system(size: 13, weight: .semibold)
    static let uiBody = Font.system(size: 13)
    static let uiLabel = Font.system(size: 12)
    static let uiCaption = Font.system(size: 11)
    /// Section headers: sentence case, never uppercase-with-tracking — the
    /// weight and color carry the hierarchy without shouting.
    static let uiSection = Font.system(size: 11, weight: .semibold)

    /// Monospace for machine data only: URLs, hosts, ports, identifiers, code,
    /// log rows, durations. Mapped to a scalable text style so the dense UI
    /// still respects Dynamic Type.
    static func mono(_ size: CGFloat, weight: Weight = .regular) -> Font {
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
}

// MARK: - Surfaces

extension Color {
    /// Grouped container fill. A hair off the page so a card reads as a held
    /// group without a border drawing a box around everything.
    static let cardFill = Color.primary.opacity(0.035)

    /// Row separators and any remaining rule. Deliberately fainter than
    /// `Divider()` — structure should come from spacing first, lines last.
    static let hairline = Color.primary.opacity(0.07)

    /// Structural boundary between regions (sidebar ↔ detail, chrome ↔ content).
    /// Derived from `.primary`, so it lightens in dark mode instead of reading
    /// as a black seam the way an opaque-on-opaque edge does.
    static let edge = Color.primary.opacity(0.13)

    /// Fill for a control that sits on the page (search field, segmented chip).
    static let controlFill = Color.primary.opacity(0.05)
}

extension View {
    /// Grouped container: fill, rounded, no stroke. Replaces the old
    /// bordered card — one less line per section, same grouping.
    func card(radius: CGFloat = Radius.md) -> some View {
        background(Color.cardFill, in: RoundedRectangle(cornerRadius: radius, style: .continuous))
    }

    /// Hairline under a row, inset to line up with the row's content so the
    /// separators read as one column edge rather than full-width rules.
    func rowSeparator(inset: CGFloat = Space.md) -> some View {
        overlay(alignment: .bottom) {
            Rectangle()
                .fill(Color.hairline)
                .frame(height: 0.5)
                .padding(.leading, inset)
        }
    }
}

// MARK: - Status

/// What we actually know about a connection right now. `unknown` is its own
/// state so a gray dot never implies "connected".
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

/// Status pill: dot, state, and when we last knew it. The time is the part
/// people actually need — "Healthy" from an hour ago is not the same claim as
/// "Healthy" from ten seconds ago.
struct StatusChip: View {
    let status: ConnStatus
    /// Epoch milliseconds of the last health check, if any.
    var checkedAt: Double?
    var detail: String?

    var body: some View {
        HStack(spacing: Space.xs + 1) {
            Circle()
                .fill(status.color)
                .frame(width: 6, height: 6)
            Text(status.label)
                .font(.uiCaption)
                .foregroundStyle(status == .failing ? Color.red : .secondary)
            if let ago = relativeTime {
                Text(ago)
                    .font(.mono(10))
                    .foregroundStyle(.tertiary)
            }
        }
        .padding(.horizontal, Space.sm)
        .padding(.vertical, Space.xs)
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

// MARK: - Tabs

/// Detail-view tabs as quiet pills. An underlined accent tab reads like a web
/// nav; a filled pill sits closer to the rest of macOS and needs no rule under
/// the bar to anchor it.
struct PillTabs<T: Hashable>: View {
    let tabs: [T]
    let title: (T) -> String
    @Binding var selection: T

    var body: some View {
        HStack(spacing: Space.xs) {
            ForEach(tabs, id: \.self) { tab in
                Button { selection = tab } label: {
                    Text(title(tab))
                        .font(.uiLabel)
                        .fontWeight(selection == tab ? .semibold : .regular)
                        .foregroundStyle(selection == tab ? Color.primary : .secondary)
                        .padding(.horizontal, Space.md - 2)
                        .padding(.vertical, Space.xs + 1)
                        .background {
                            if selection == tab {
                                RoundedRectangle(cornerRadius: Radius.sm, style: .continuous)
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

/// Small neutral pill for a fact that qualifies the thing it sits beside —
/// environment, read-only, a count. Never colored unless the fact is a warning.
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
                .font(.uiCaption)
        }
        .foregroundStyle(tint ?? .secondary)
        .padding(.horizontal, Space.sm - 1)
        .padding(.vertical, Space.xxs + 1)
        .background((tint ?? Color.primary).opacity(0.07), in: Capsule())
    }
}
