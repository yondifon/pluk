import SwiftUI

@MainActor
@Observable
final class AppZoom {
    static let steps: [CGFloat] = [0.85, 0.9, 1.0, 1.1, 1.25, 1.4, 1.6, 1.8, 2.0]
    private static let defaultIndex = 2
    private static let key = "PlukUIZoomStep"

    private var index: Int {
        didSet { UserDefaults.standard.set(index, forKey: Self.key) }
    }

    init() {
        let stored = UserDefaults.standard.object(forKey: Self.key) as? Int
        index = (stored?.clamped(to: Self.steps.indices)) ?? Self.defaultIndex
    }

    var scale: CGFloat { Self.steps[index] }
    var canZoomIn: Bool { index < Self.steps.count - 1 }
    var canZoomOut: Bool { index > 0 }
    var isDefault: Bool { index == Self.defaultIndex }

    var label: String { "\(Int((scale * 100).rounded()))%" }

    func zoomIn() { if canZoomIn { index += 1 } }
    func zoomOut() { if canZoomOut { index -= 1 } }
    func reset() { index = Self.defaultIndex }
}

private extension Int {
    func clamped(to range: Range<Int>) -> Int {
        Swift.min(Swift.max(self, range.lowerBound), range.upperBound - 1)
    }
}

struct RootView: View {
    let store: ConnectionStore
    let serverManager: ServerManager
    let updateChecker: UpdateChecker
    let zoom: AppZoom

    var body: some View {
        ContentView(store: store, serverManager: serverManager, updateChecker: updateChecker)
            .environment(\.uiScale, zoom.scale)
    }
}
