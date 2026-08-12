import Foundation

/// Streaming SSE client for the server's `/api/events` feed. One instance lives
/// on `ConnectionStore`; it reconnects with exponential backoff and hands every
/// decoded frame back to the store. Safe to start again after `stop()`.
@MainActor
final class ActivityStream {
    private let store: ConnectionStore
    private var task: Task<Void, Never>?
    private var attempt = 0

    init(store: ConnectionStore) {
        self.store = store
    }

    func start() {
        guard task == nil else { return }
        task = Task { await run() }
    }

    func stop() {
        task?.cancel()
        task = nil
    }

    private func run() async {
        while !Task.isCancelled {
            await connectOnce()
            if Task.isCancelled { break }
            attempt += 1
            try? await Task.sleep(for: backoffDelay())
        }
    }

    private func connectOnce() async {
        guard var comps = URLComponents(string: PlukServer.api("events")) else { return }
        comps.queryItems = [URLQueryItem(name: "after", value: String(store.logCursor))]
        guard let url = comps.url else { return }
        do {
            let (bytes, response) = try await URLSession.shared.bytes(from: url)
            guard let http = response as? HTTPURLResponse, http.statusCode == 200 else { return }
            attempt = 0
            store.streamConnected()
            try? await consume(bytes)
            store.streamDisconnected()
        } catch {
            // Connect/stream error — the caller retries with backoff.
        }
    }

    private func consume(_ bytes: URLSession.AsyncBytes) async throws {
        var name = ""
        var data = ""
        for try await line in bytes.lines {
            if line.isEmpty {
                if !data.isEmpty { store.handleActivityFrame(name: name, data: data) }
                name = ""
                data = ""
            } else if line.hasPrefix("event:") {
                name = String(line.dropFirst("event:".count)).trimmingCharacters(in: .whitespaces)
            } else if line.hasPrefix("data:") {
                data += String(line.dropFirst("data:".count)).trimmingCharacters(in: .whitespaces)
            }
        }
    }

    private func backoffDelay() -> Duration {
        .seconds(0.5 * pow(2.0, Double(min(attempt, 6))))
    }
}
