import Foundation

final class ServerManager {
    private var process: Process?

    func start() {
        killOrphanOnPort(4242)

        guard let bunPath = findBun(), let serverPath = findServer() else {
            print("[pluk] Could not locate bun or server.ts")
            return
        }

        let p = Process()
        p.executableURL = URL(fileURLWithPath: bunPath)
        p.arguments = ["run", serverPath]
        p.environment = ProcessInfo.processInfo.environment

        do {
            try p.run()
            process = p
            print("[pluk] MCP server started (pid \(p.processIdentifier))")
        } catch {
            print("[pluk] Failed to start server: \(error)")
        }
    }

    func stop() {
        guard let p = process, p.isRunning else { return }
        p.terminate()                     // SIGTERM → Bun drains
        // Wait up to 3 s for clean exit before force-killing
        let deadline = Date().addingTimeInterval(3)
        while p.isRunning && Date() < deadline {
            Thread.sleep(forTimeInterval: 0.05)
        }
        if p.isRunning { p.interrupt() }  // SIGINT fallback
        process = nil
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

    private func findServer() -> String? {
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
