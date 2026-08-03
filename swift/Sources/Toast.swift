import SwiftUI
import Observation
import UserNotifications

// Lightweight, non-blocking status notifications. A connection that starts
// failing for an agent (SSH/auth/tunnel) shouldn't silently sit broken nor dump
// a raw error into the layout — it surfaces here as a transient toast (and a
// native notification when permitted), with the humanized message and a Retry.

struct Toast: Identifiable, Equatable {
    enum Kind: Equatable { case error, success }
    let id = UUID()
    let connectionId: String
    let title: String
    let message: String
    let kind: Kind
}

@Observable
@MainActor
final class ToastCenter {
    private(set) var toasts: [Toast] = []

    /// Called when a connection's health changes. Only transitions are surfaced
    /// (ok/unknown → error, or error → ok) so a steadily-failing connection
    /// doesn't re-toast on every poll.
    func present(_ toast: Toast) {
        // Replace any existing toast for the same connection so it never stacks
        // duplicates for one integration.
        toasts.removeAll { $0.connectionId == toast.connectionId }
        toasts.append(toast)
        if toast.kind == .error { postNotification(toast) }

        let id = toast.id
        let lifetime: Duration = toast.kind == .error ? .seconds(8) : .seconds(3)
        Task { @MainActor in
            try? await Task.sleep(for: lifetime)
            dismiss(id)
        }
    }

    func dismiss(_ id: UUID) {
        toasts.removeAll { $0.id == id }
    }

    // MARK: - Native notifications (best-effort)

    static func requestNotificationAccess() {
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { _, _ in }
    }

    private func postNotification(_ toast: Toast) {
        let content = UNMutableNotificationContent()
        content.title = toast.title
        content.body = toast.message
        let req = UNNotificationRequest(identifier: toast.id.uuidString, content: content, trigger: nil)
        UNUserNotificationCenter.current().add(req)
    }
}

// MARK: - Overlay

struct ToastOverlay: View {
    let center: ToastCenter
    var onRetry: (String) -> Void
    @SwiftUI.Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(spacing: Space.sm) {
            ForEach(center.toasts) { toast in
                ToastCard(toast: toast,
                          onRetry: { onRetry(toast.connectionId) },
                          onDismiss: { center.dismiss(toast.id) })
                    .transition(.move(edge: .top).combined(with: .opacity))
            }
        }
        .padding(.top, Space.md)
        .padding(.trailing, Space.lg)
        .frame(maxWidth: .infinity, alignment: .topTrailing)
        .animation(reduceMotion ? nil : .easeOut(duration: 0.18), value: center.toasts)
        .allowsHitTesting(true)
    }
}

private struct ToastCard: View {
    let toast: Toast
    var onRetry: () -> Void
    var onDismiss: () -> Void

    private var accent: Color { toast.kind == .error ? .red : .green }
    private var icon: String { toast.kind == .error ? "exclamationmark.triangle.fill" : "checkmark.circle.fill" }

    var body: some View {
        HStack(alignment: .top, spacing: Space.md) {
            Image(systemName: icon)
                .foregroundStyle(accent)
                .font(.system(size: 14, weight: .semibold))
                .padding(.top, Space.xxs)

            VStack(alignment: .leading, spacing: Space.xxs) {
                Text(toast.title)
                    .scaledFont(.headline)
                Text(toast.message)
                    .scaledFont(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if toast.kind == .error {
                    Button("Retry", action: onRetry)
                        .buttonStyle(.borderless)
                        .controlSize(.small)
                        .padding(.top, Space.xxs)
                }
            }

            Spacer(minLength: 0)

            Button(action: onDismiss) {
                Image(systemName: "xmark")
                    .font(.system(size: 9, weight: .bold))
                    .foregroundStyle(.tertiary)
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, Space.md)
        .padding(.vertical, Space.md)
        .frame(width: 320, alignment: .leading)
        .background(Surface.panel, in: RoundedRectangle(cornerRadius: Radius.medium, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: Radius.medium, style: .continuous)
                .strokeBorder(accent.opacity(0.35), lineWidth: 1)
        )
        .overlay(alignment: .leading) {
            RoundedRectangle(cornerRadius: 2)
                .fill(accent)
                .frame(width: 3)
                .padding(.vertical, Space.sm)
                .padding(.leading, Space.xxs)
        }
        .shadow(color: .black.opacity(0.18), radius: 12, y: 4)
    }
}
