import Foundation
import Observation

@Observable
@MainActor
final class ServerManager {
    enum Status: Equatable { case starting, running, stopped }
    private(set) var status: Status = .stopped

    private var process: Process?

    func start() {
        killOrphanOnPort(PlukServer.port)

        guard let server = resolveServer() else {
            print("[pluk] Could not locate server binary")
            status = .stopped
            return
        }

        let p = Process()
        p.executableURL = URL(fileURLWithPath: server.executable)
        p.arguments = server.args
        p.environment = ProcessInfo.processInfo.environment

        do {
            try p.run()
            process = p
            status = .starting
            print("[pluk] MCP server started (pid \(p.processIdentifier))")
            checkUntilReady()
        } catch {
            print("[pluk] Failed to start server: \(error)")
            status = .stopped
        }
    }

    func stop() {
        guard let p = process, p.isRunning else { return }
        p.terminate()
        let deadline = Date().addingTimeInterval(3)
        while p.isRunning && Date() < deadline {
            Thread.sleep(forTimeInterval: 0.05)
        }
        if p.isRunning { p.interrupt() }
        process = nil
        status = .stopped
    }

    // MARK: - Health check

    private func checkUntilReady() {
        Task { @MainActor in
            for _ in 0..<20 {
                try? await Task.sleep(for: .milliseconds(500))
                if await isReachable() {
                    status = .running
                    return
                }
            }
            status = .stopped
        }
    }

    private func isReachable() async -> Bool {
        guard let url = URL(string: "\(PlukServer.baseURL)/health") else { return false }
        var req = URLRequest(url: url)
        req.timeoutInterval = 1
        return (try? await URLSession.shared.data(for: req)) != nil
    }

    // MARK: - Server resolution

    private typealias ServerSpec = (executable: String, args: [String])

    private func resolveServer() -> ServerSpec? {
        if let resources = Bundle.main.resourcePath {
            let binary = (resources as NSString).appendingPathComponent("pluk-server")
            if FileManager.default.fileExists(atPath: binary) {
                return (binary, [])
            }
        }

        guard let bun = findBun(), let serverTS = findServerTS() else { return nil }
        return (bun, ["run", serverTS])
    }

    // MARK: - Helpers

    private func killOrphanOnPort(_ port: Int) {
        let sh = Process()
        sh.executableURL = URL(fileURLWithPath: "/bin/sh")
        sh.arguments = ["-c", "lsof -ti:\(port) | xargs kill -TERM 2>/dev/null; sleep 0.3"]
        sh.standardOutput = FileHandle.nullDevice
        sh.standardError = FileHandle.nullDevice
        try? sh.run()
        sh.waitUntilExit()
    }

    private func findBun() -> String? {
        let candidates = [
            ProcessInfo.processInfo.environment["HOME"].map { "\($0)/.bun/bin/bun" },
            "/opt/homebrew/bin/bun",
            "/usr/local/bin/bun",
        ].compactMap { $0 }
        return candidates.first { FileManager.default.fileExists(atPath: $0) } ?? which("bun")
    }

    private func findServerTS() -> String? {
        let candidates: [String] = [
            ProcessInfo.processInfo.environment["PLUK_SERVER"],
            URL(fileURLWithPath: #filePath)
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .appendingPathComponent("pluk/src/server.ts").path,
        ].compactMap { $0 }
        return candidates.first { FileManager.default.fileExists(atPath: $0) }
    }

    private func which(_ cmd: String) -> String? {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/which")
        p.arguments = [cmd]
        let pipe = Pipe()
        p.standardOutput = pipe
        try? p.run()
        p.waitUntilExit()
        let out = String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return out?.isEmpty == false ? out : nil
    }
}
