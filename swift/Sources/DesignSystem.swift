import AppKit
import SwiftUI

// MARK: - Radii

enum Radius {
    static let small: CGFloat = 6
    static let medium: CGFloat = 10
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

    // Row selection stays neutral so row text, not a tint, reads as loudest.
    static let selection = Color(nsColor: NSColor(name: nil) { appearance in
        let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        return isDark ? NSColor.white.withAlphaComponent(0.10) : NSColor.black.withAlphaComponent(0.06)
    })

    static let sidebarColor = neutral(light: 0.957, dark: 0.118)
    static let contentColor = neutral(light: 0.980, dark: 0.150)
    static let panelColor = neutral(light: 1.000, dark: 0.180)
    static let sidebarLabel = Color(nsColor: NSColor(name: nil) { appearance in
        let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        return isDark ? NSColor.secondaryLabelColor : NSColor(white: 0.22, alpha: 1)
    })
    static let tertiaryLabel = Color(nsColor: NSColor(name: nil) { appearance in
        let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        return isDark ? NSColor.tertiaryLabelColor : NSColor(white: 0.45, alpha: 1)
    })
    static let sidebarTertiaryLabel = Color(nsColor: NSColor(name: nil) { appearance in
        let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        return isDark ? NSColor.tertiaryLabelColor : NSColor(white: 0.38, alpha: 1)
    })

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
    static let controlFill = Color.primary.opacity(0.05)
}

extension View {
    func card(radius: CGFloat = Radius.medium) -> some View {
        background(Color.cardFill, in: RoundedRectangle(cornerRadius: radius, style: .continuous))
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

// MARK: - Split view divider

// NavigationSplitView exposes no macOS modifier for the NSSplitView divider it
// draws, so silencing it means reaching the NSSplitView through AppKit and
// swapping its runtime class for one that reports a clear divider color.
// NSSplitView paints the divider inside drawDivider(in:) rather than filling
// the divider rect with dividerColor, so both overrides are required: the
// color override alone leaves the line visible.
private final class ClearDividerSplitView: NSSplitView {
    override var dividerColor: NSColor { .clear }
    override func drawDivider(in rect: NSRect) {}
}

private final class SplitViewDividerHiderView: NSView {
    // The NSSplitView can be created after this view enters the window (first
    // layout, restored frame, full-screen), so the swap re-runs on window
    // geometry changes and once more after the initial layout settles.
    private var geometryObserver: NSObjectProtocol?

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        stopObservingGeometry()
        hideEnclosingSplitViewDividers()
        guard let window else { return }
        geometryObserver = NotificationCenter.default.addObserver(
            forName: NSView.frameDidChangeNotification,
            object: window.contentView,
            queue: .main
        ) { [weak self] _ in
            self?.hideEnclosingSplitViewDividers()
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { [weak self] in
            self?.hideEnclosingSplitViewDividers()
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { [weak self] in
            self?.hideEnclosingSplitViewDividers()
        }
    }

    override func layout() {
        super.layout()
        hideEnclosingSplitViewDividers()
    }

    deinit {
        stopObservingGeometry()
    }

    private func stopObservingGeometry() {
        if let geometryObserver {
            NotificationCenter.default.removeObserver(geometryObserver)
        }
        geometryObserver = nil
    }
}

private extension NSView {
    // Reached two ways because neither alone is enough: walking up covers a
    // view already parented under the split view, and sweeping down from the
    // window's root covers first layout, where the view is not yet in that
    // hierarchy and an upward walk finds nothing at all.
    func hideEnclosingSplitViewDividers() {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            var view: NSView? = self
            while let current = view {
                Self.hideDividers(in: current)
                view = current.superview
            }
            if let root = self.window?.contentView {
                Self.hideDividers(in: root)
            }
        }
    }

    // Once a split view carries ClearDividerSplitView the swap is skipped, so
    // re-running this on every layout pass costs a walk and nothing more.
    static func hideDividers(in view: NSView) {
        if let splitView = view as? NSSplitView,
           object_getClass(splitView) !== ClearDividerSplitView.self {
            object_setClass(splitView, ClearDividerSplitView.self)
            splitView.needsDisplay = true
        }
        for subview in view.subviews {
            hideDividers(in: subview)
        }
    }
}

/// Placed in the background of a `NavigationSplitView` pane, this hides the
/// system divider line while leaving its drag-to-resize hit area untouched.
struct SplitViewDividerHider: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        SplitViewDividerHiderView()
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        nsView.hideEnclosingSplitViewDividers()
    }
}

// MARK: - Sidebar list selection

private final class TableSelectionHiderView: NSView {
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        hideEnclosingSelectionHighlight()
    }

    override func layout() {
        super.layout()
        hideEnclosingSelectionHighlight()
    }
}

private extension NSView {
    func hideEnclosingSelectionHighlight() {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            var view: NSView? = self
            while let current = view {
                Self.hideSelection(in: current)
                view = current.superview
            }
            if let root = self.window?.contentView {
                Self.hideSelection(in: root)
            }
        }
    }

    static func hideSelection(in view: NSView) {
        if let tableView = view as? NSTableView, tableView.selectionHighlightStyle != .none {
            tableView.selectionHighlightStyle = .none
        }
        for subview in view.subviews {
            hideSelection(in: subview)
        }
    }
}

/// Placed in the background of a sidebar `List`, this turns off AppKit's own
/// row highlight so selection reads only through `.listRowBackground`.
struct TableSelectionHider: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        TableSelectionHiderView()
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        nsView.hideEnclosingSelectionHighlight()
    }
}

// MARK: - Scroll indicator

private final class ScrollerHiderView: NSView {
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        hideEnclosingScrollers()
    }

    override func layout() {
        super.layout()
        hideEnclosingScrollers()
    }
}

private extension NSView {
    func hideEnclosingScrollers() {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            var view: NSView? = self
            while let current = view {
                Self.hideScrollers(in: current)
                view = current.superview
            }
            if let root = self.window?.contentView {
                Self.hideScrollers(in: root)
            }
        }
    }

    static func hideScrollers(in view: NSView) {
        if let scrollView = view as? NSScrollView {
            scrollView.hasVerticalScroller = false
            scrollView.hasHorizontalScroller = false
        }
        for subview in view.subviews {
            hideScrollers(in: subview)
        }
    }
}

/// Placed in the background of a scrollable pane, this hides the enclosing
/// `NSScrollView`'s scroller entirely while leaving scrolling itself (wheel,
/// trackpad) untouched — only the indicator is turned off, not the behavior.
struct ScrollerHider: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        ScrollerHiderView()
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        nsView.hideEnclosingScrollers()
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

/// IBM Plex Sans for prose, IBM Plex Mono for machine strings. No `.ttf`
/// files are bundled yet, so `resolvedName` always falls back to nil and
/// callers render in the system face until fonts are added to the bundle.
enum Typeface {
    static let sansFamily = "IBM Plex Sans"
    static let monoFamily = "IBM Plex Mono"

    private static let didRegister: Bool = {
        guard let fontsURL = Bundle.main.url(forResource: "Fonts", withExtension: nil) else { return false }
        var error: Unmanaged<CFError>?
        return CTFontManagerRegisterFontsForURL(fontsURL as CFURL, .process, &error)
    }()

    static func resolvedName(for design: Font.Design) -> String? {
        _ = didRegister
        let family = design == .monospaced ? monoFamily : sansFamily
        guard NSFontManager.shared.availableMembers(ofFontFamily: family) != nil else { return nil }
        return family
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
        case .largeTitle: 22
        case .title: 19
        case .title2: 16
        case .title3: 15
        case .headline: 14
        case .body: 13
        case .callout: 12.5
        case .subheadline: 12
        case .footnote: 12
        case .caption: 11.5
        case .caption2: 11
        @unknown default: 13
        }
    }

    static func weight(_ style: Font.TextStyle) -> Font.Weight {
        switch style {
        case .largeTitle, .title, .title2: .bold
        case .title3, .headline: .semibold
        case .subheadline: .medium
        default: .regular
        }
    }
}

extension Font {
    static func scaled(
        _ style: Font.TextStyle,
        scale: CGFloat,
        weight: Font.Weight? = nil,
        design: Font.Design = .default
    ) -> Font {
        let size = TextStyleSize.base(style) * scale
        if let name = Typeface.resolvedName(for: design) {
            return .custom(name, size: size)
        }
        return .system(size: size, weight: weight ?? TextStyleSize.weight(style), design: design)
    }

    static func dev(size: CGFloat, weight: Weight = .regular) -> Font {
        if let name = Typeface.resolvedName(for: .monospaced) {
            return .custom(name, size: size)
        }
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

// MARK: - Pointer feedback

extension View {
    /// Pointing-hand cursor over a custom clickable area. SwiftUI leaves the
    /// arrow on anything that is not a system control, so a plain button or a
    /// tappable row gives no hover feedback without this.
    func pointerCursor(_ enabled: Bool = true) -> some View {
        onContinuousHover { phase in
            guard enabled else { return }
            switch phase {
            case .active: NSCursor.pointingHand.set()
            case .ended: NSCursor.arrow.set()
            }
        }
    }
}

/// `.plain` plus the pointer and a press state, for every button the app draws
/// itself.
struct PointerButtonStyle: ButtonStyle {
    @SwiftUI.Environment(\.isEnabled) private var isEnabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .opacity(configuration.isPressed ? 0.6 : 1)
            .contentShape(Rectangle())
            .pointerCursor(isEnabled)
    }
}

extension ButtonStyle where Self == PointerButtonStyle {
    static var pointer: PointerButtonStyle { PointerButtonStyle() }
}

/// The one stroke the design system allows: a leading-edge accent for a
/// quoted or flagged block. Color carries the meaning; nothing else does.
struct AccentRule: View {
    var color: Color = .secondary

    var body: some View {
        RoundedRectangle(cornerRadius: 1)
            .fill(color)
            .frame(width: 2)
    }
}

struct SectionLabel: View {
    let text: String

    var body: some View {
        Text(text.uppercased())
            .scaledFont(.caption, weight: .semibold, design: .monospaced)
            .tracking(0.6)
            .foregroundStyle(Surface.tertiaryLabel)
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
        .buttonStyle(.pointer)
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
                    .scaledFont(.caption, design: .monospaced)
                    .foregroundStyle(Surface.tertiaryLabel)
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

struct TextTabs<T: Hashable>: View {
    let tabs: [T]
    let title: (T) -> String
    @Binding var selection: T

    var body: some View {
        HStack(spacing: Space.lg) {
            ForEach(tabs, id: \.self) { tab in
                tabButton(tab)
            }
        }
    }

    private func tabButton(_ tab: T) -> some View {
        let selected = selection == tab
        return Button {
            selection = tab
        } label: {
            Text(title(tab))
                .scaledFont(.callout)
                .fontWeight(selected ? .semibold : .regular)
                .foregroundStyle(selected ? Color.primary : .secondary)
                .contentShape(.rect)
        }
        .buttonStyle(.pointer)
        .accessibilityAddTraits(selected ? [.isSelected] : [])
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
                    .font(.system(size: 10, weight: .semibold))
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

// A row value that reads as data: monospaced, on the shared type ramp.
struct MonospacedValue: View {
    let value: String

    init(_ value: String) { self.value = value }

    var body: some View {
        Text(value)
            .scaledFont(.callout, design: .monospaced)
            .foregroundStyle(.primary)
    }
}

struct InspectorRow<Content: View>: View {
    let label: String
    let labelWidth: CGFloat
    let content: Content

    init(_ label: String, value: String) where Content == MonospacedValue {
        self.label = label
        self.labelWidth = 88
        self.content = MonospacedValue(value)
    }

    init(_ label: String, labelWidth: CGFloat = 88, @ViewBuilder content: () -> Content) {
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
