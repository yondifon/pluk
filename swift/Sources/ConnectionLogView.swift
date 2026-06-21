import AppKit
import SwiftUI

struct LogsTab: View {
    let conn: Connection
    let store: ConnectionStore

    @State private var entries: [QueryLogEntry] = []
    @State private var filter: VerdictFilter = .all
    @State private var expandedId: Int? = nil
    @State private var showRetentionPicker = false
    @State private var pollTimer: Timer? = nil

    enum VerdictFilter: String, CaseIterable {
        case all = "All"
        case allowed = "Allowed"
        case blocked = "Blocked"
        case error = "Error"
    }

    private var hasPending: Bool { entries.contains { $0.verdict == "pending" } }

    private var filtered: [QueryLogEntry] {
        guard filter != .all else { return entries }
        return entries.filter { $0.verdict == filter.rawValue.lowercased() }
    }

    private var stats: (allowed: Int, blocked: Int, error: Int) {
        let a = entries.filter { $0.verdict == "allowed" }.count
        let b = entries.filter { $0.verdict == "blocked" }.count
        let e = entries.filter { $0.verdict == "error" }.count
        return (a, b, e)
    }

    var body: some View {
        VStack(spacing: 0) {
            toolbar
            Divider()
            if filtered.isEmpty {
                emptyState
            } else {
                logList
            }
        }
        .onAppear {
            reload()
            startPollingIfNeeded()
        }
        .onDisappear {
            stopPolling()
        }
        .onChange(of: hasPending) { _, pending in
            pending ? startPollingIfNeeded() : stopPolling()
        }
    }

    private func startPollingIfNeeded() {
        guard hasPending, pollTimer == nil else { return }
        pollTimer = Timer.scheduledTimer(withTimeInterval: 1.5, repeats: true) { _ in
            reload()
        }
    }

    private func stopPolling() {
        pollTimer?.invalidate()
        pollTimer = nil
    }

    // MARK: - Toolbar

    private var toolbar: some View {
        HStack(spacing: 10) {
            // Stats pills
            statPill(entries.count, label: "total", color: .secondary)
            statPill(stats.allowed, label: "ok", color: .green)
            if stats.blocked > 0 {
                statPill(stats.blocked, label: "blocked", color: .red)
            }
            if stats.error > 0 {
                statPill(stats.error, label: "err", color: .orange)
            }

            Spacer()

            // Filter
            HStack(spacing: 2) {
                ForEach(VerdictFilter.allCases, id: \.self) { f in
                    Button(f.rawValue) { filter = f }
                        .buttonStyle(.plain)
                        .font(.system(size: 11, weight: filter == f ? .semibold : .regular))
                        .foregroundColor(filter == f ? .accentColor : .secondary)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(filter == f ? Color.accentColor.opacity(0.1) : .clear)
                        .clipShape(.capsule)
                }
            }

            Divider().frame(height: 14)

            // Retention
            Menu {
                let options = [7, 14, 30, 60, 90, 0]
                ForEach(options, id: \.self) { days in
                    Button(days == 0 ? "Keep forever" : "Keep \(days) days") {
                        store.logRetentionDays = days
                        store.purgeOldLogs()
                        reload()
                    }
                }
                Divider()
                Button("Clear all logs for this integration", role: .destructive) {
                    store.clearAllLogs(connectionId: conn.id)
                    reload()
                }
            } label: {
                HStack(spacing: 3) {
                    Image(systemName: "clock.arrow.circlepath")
                        .font(.system(size: 10))
                    let days = store.logRetentionDays
                    Text(days == 0 ? "Forever" : "\(days)d")
                        .font(.system(size: 11))
                }
                .foregroundColor(.secondary)
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
            .help("Log retention — how long to keep activity history")

            Button {
                reload()
            } label: {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 11))
            }
            .buttonStyle(.plain)
            .foregroundColor(.secondary)
            .help("Refresh")
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 9)
    }

    private func statPill(_ count: Int, label: String, color: Color) -> some View {
        HStack(spacing: 3) {
            Text("\(count)")
                .font(.system(size: 11, weight: .semibold, design: .monospaced))
                .foregroundColor(color == .secondary ? .primary : color)
            Text(label)
                .font(.system(size: 11))
                .foregroundColor(.secondary)
        }
    }

    // MARK: - Empty state

    private var emptyState: some View {
        VStack(spacing: 8) {
            Image(systemName: "list.bullet.rectangle")
                .font(.system(size: 28))
                .foregroundColor(.secondary.opacity(0.4))
            Text(filter == .all ? "No activity yet" : "No \(filter.rawValue.lowercased()) activity")
                .font(.system(size: 13))
                .foregroundColor(.secondary)
            Text("Activity from agents using this integration will appear here.")
                .font(.system(size: 11))
                .foregroundColor(.secondary.opacity(0.7))
                .multilineTextAlignment(.center)
                .frame(maxWidth: 280)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    // MARK: - Log list

    private var logList: some View {
        ScrollView {
            LazyVStack(spacing: 0, pinnedViews: []) {
                ForEach(filtered) { entry in
                    let expanded = expandedId == entry.id
                    LogEntryRow(
                        entry: entry,
                        isExpanded: expanded,
                        onToggle: { expandedId = expanded ? nil : entry.id },
                        onStop: { stopQuery(entry) }
                    )
                    Divider().padding(.leading, 18)
                }
            }
        }
    }

    private func reload() {
        entries = store.recentLog(connectionId: conn.id)
    }

    private func stopQuery(_ entry: QueryLogEntry) {
        Task {
            let url = URL(string: PlukServer.api("log/\(entry.id)/cancel"))!
            var req = URLRequest(url: url)
            req.httpMethod = "POST"
            req.timeoutInterval = 5
            _ = try? await URLSession.shared.data(for: req)
            await MainActor.run { reload() }
        }
    }
}

// MARK: - Log entry row

private struct LogEntryRow: View {
    let entry: QueryLogEntry
    let isExpanded: Bool
    let onToggle: () -> Void
    let onStop: () -> Void

    @State private var copiedSQL = false
    @State private var copiedResult = false
    @State private var showResponseSheet = false

    // The best full response to copy/open: the raw text the tool returned,
    // falling back to the stored result rows, then the verdict reason.
    private var fullResponse: String? {
        if let raw = entry.responseText, !raw.isEmpty { return raw }
        if let json = entry.resultJson, !json.isEmpty { return json }
        if let reason = entry.reason, !reason.isEmpty { return reason }
        return nil
    }

    private func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    var body: some View {
        Button(action: onToggle) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(alignment: .top, spacing: 10) {
                    // Verdict indicator bar
                    RoundedRectangle(cornerRadius: 2)
                        .fill(verdictColor)
                        .frame(width: 3)
                        .frame(minHeight: 36)

                    VStack(alignment: .leading, spacing: 4) {
                        // Top row: badge + SQL preview
                        HStack(spacing: 8) {
                            VerdictBadge(verdict: entry.verdict)

                            if let cats = entry.categories, !cats.isEmpty {
                                Text(cats)
                                    .font(.system(size: 10, design: .monospaced))
                                    .foregroundColor(.secondary)
                                    .lineLimit(1)
                            }

                            Spacer()

                            if entry.verdict == "pending" {
                                Button(action: onStop) {
                                    Label("Stop", systemImage: "stop.fill")
                                        .font(.system(size: 10, weight: .medium))
                                }
                                .buttonStyle(.plain)
                                .foregroundColor(.red)
                                .help("Cancel this running query")
                            }

                            Text(relativeTime(entry.createdAt))
                                .font(.system(size: 10))
                                .foregroundColor(.secondary)
                                .lineLimit(1)
                        }

                        // SQL
                        Text(entry.sql)
                            .font(.system(size: 11.5, design: .monospaced))
                            .foregroundColor(.primary)
                            .lineLimit(isExpanded ? nil : 1)
                            .truncationMode(.tail)
                            .frame(maxWidth: .infinity, alignment: .leading)

                        // Expanded: reason + result preview + full timestamp
                        if isExpanded {
                            if let reason = entry.reason, !reason.isEmpty {
                                HStack(spacing: 4) {
                                    Image(systemName: "exclamationmark.circle.fill")
                                        .font(.system(size: 10))
                                        .foregroundColor(verdictColor)
                                    Text(reason)
                                        .font(.system(size: 11))
                                        .foregroundColor(.secondary)
                                }
                                .padding(.top, 2)
                            }

                            // Full response: raw tool output when stored, else the
                            // structured result rows as a mini-table.
                            if let raw = entry.responseText, !raw.isEmpty {
                                ResponseTextBlock(text: raw) { showResponseSheet = true }
                                    .padding(.top, 6)
                            } else if let json = entry.resultJson {
                                ResultPreview(json: json, rowCount: entry.rowCount)
                                    .padding(.top, 6)
                            }

                            Text(localTime(entry.createdAt))
                                .font(.system(size: 10))
                                .foregroundColor(.secondary.opacity(0.7))
                                .padding(.top, 1)

                            // Copy actions for the query and its response
                            HStack(spacing: 6) {
                                copyButton(copiedSQL ? "Copied!" : "Copy", copied: copiedSQL) {
                                    copy(entry.sql)
                                    flash($copiedSQL)
                                }
                                if let response = fullResponse {
                                    copyButton(copiedResult ? "Copied!" : "Copy response", copied: copiedResult) {
                                        copy(response)
                                        flash($copiedResult)
                                    }
                                }
                            }
                            .padding(.top, 6)
                        }
                    }
                    .padding(.vertical, 10)
                    .padding(.trailing, 18)
                }
                .padding(.leading, 18)
            }
        }
        .buttonStyle(.plain)
        .background(isExpanded ? Color.accentColor.opacity(0.04) : .clear)
        .contentShape(Rectangle())
        .animation(.easeInOut(duration: 0.12), value: isExpanded)
        .sheet(isPresented: $showResponseSheet) {
            ResponseSheet(title: entry.sql, text: fullResponse ?? "")
        }
    }

    private func copyButton(_ title: String, copied: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(title, systemImage: copied ? "checkmark" : "doc.on.doc")
                .font(.system(size: 10, weight: .medium))
        }
        .buttonStyle(.bordered)
        .controlSize(.mini)
        .tint(copied ? .green : nil)
    }

    private func flash(_ flag: Binding<Bool>) {
        flag.wrappedValue = true
        Task { @MainActor in
            try? await Task.sleep(for: .seconds(1.5))
            flag.wrappedValue = false
        }
    }

    private var verdictColor: Color {
        switch entry.verdict {
        case "allowed":   return .green
        case "blocked":   return .red
        case "cancelled": return Color(nsColor: .systemPurple)
        case "pending":   return .secondary
        default:          return .orange
        }
    }

    // "2 min ago" / "just now" / falls back to raw string for older entries
    private func relativeTime(_ raw: String) -> String {
        let fmt = DateFormatter()
        fmt.dateFormat = "yyyy-MM-dd HH:mm:ss"
        fmt.locale = Locale(identifier: "en_US_POSIX")
        fmt.timeZone = TimeZone(identifier: "UTC")  // SQLite datetime('now') is UTC
        guard let date = fmt.date(from: raw) else { return raw }
        let secs = Int(-date.timeIntervalSinceNow)
        if secs < 10  { return "just now" }
        if secs < 60  { return "\(secs)s ago" }
        if secs < 3600 { return "\(secs / 60)m ago" }
        if secs < 86400 { return "\(secs / 3600)h ago" }
        return "\(secs / 86400)d ago"
    }

    // Full UTC timestamp -> local time string
    private func localTime(_ raw: String) -> String {
        let inFmt = DateFormatter()
        inFmt.dateFormat = "yyyy-MM-dd HH:mm:ss"
        inFmt.locale = Locale(identifier: "en_US_POSIX")
        inFmt.timeZone = TimeZone(identifier: "UTC")
        guard let date = inFmt.date(from: raw) else { return raw }
        let outFmt = DateFormatter()
        outFmt.dateFormat = "yyyy-MM-dd HH:mm:ss"
        outFmt.locale = Locale(identifier: "en_US_POSIX")
        outFmt.timeZone = .current
        return outFmt.string(from: date)
    }
}

// MARK: - Verdict badge

private struct VerdictBadge: View {
    let verdict: String

    var body: some View {
        if verdict == "pending" {
            HStack(spacing: 4) {
                ProgressView().scaleEffect(0.55).frame(width: 10, height: 10)
                Text("RUNNING")
                    .font(.system(size: 9, weight: .bold))
                    .foregroundColor(.secondary)
            }
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Color.secondary.opacity(0.1))
            .clipShape(.capsule)
        } else {
            Text(label)
                .font(.system(size: 9, weight: .bold))
                .foregroundColor(.white)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(color)
                .clipShape(.capsule)
        }
    }

    private var label: String {
        switch verdict {
        case "allowed":   return "OK"
        case "blocked":   return "BLOCKED"
        case "cancelled": return "CANCELLED"
        default:          return "ERROR"
        }
    }

    private var color: Color {
        switch verdict {
        case "allowed":   return .green
        case "blocked":   return .red
        case "cancelled": return Color(nsColor: .systemPurple)
        default:          return .orange
        }
    }
}

// MARK: - Result preview (mini-table for expanded log entries)

private struct ResultPreview: View {
    let json: String
    let rowCount: Int?

    private struct ParsedResult {
        let fields: [String]
        let rows: [[String]]
    }

    private var parsed: ParsedResult? {
        guard let data = json.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let fields = obj["fields"] as? [String],
              let rows = obj["rows"] as? [[String: Any]] else { return nil }
        let rowStrings = rows.prefix(5).map { row in
            fields.map { key in
                guard let val = row[key] else { return "NULL" }
                if val is NSNull { return "NULL" }
                return "\(val)"
            }
        }
        return ParsedResult(fields: fields, rows: rowStrings)
    }

    var body: some View {
        if let p = parsed, !p.fields.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                // Header row
                HStack(spacing: 0) {
                    ForEach(p.fields.prefix(6), id: \.self) { field in
                        Text(field)
                            .font(.system(size: 9.5, weight: .semibold, design: .monospaced))
                            .foregroundColor(.secondary)
                            .lineLimit(1)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 3)
                            .background(Color.secondary.opacity(0.08))
                    }
                }
                .clipShape(RoundedRectangle(cornerRadius: 4, style: .continuous).path(in: CGRect(x: 0, y: 0, width: 9999, height: 999)))

                // Data rows
                ForEach(Array(p.rows.enumerated()), id: \.offset) { _, row in
                    HStack(spacing: 0) {
                        ForEach(Array(row.prefix(6).enumerated()), id: \.offset) { _, cell in
                            Text(cell)
                                .font(.system(size: 9.5, design: .monospaced))
                                .foregroundColor(.primary.opacity(0.75))
                                .lineLimit(1)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.horizontal, 6)
                                .padding(.vertical, 2)
                        }
                    }
                    Divider().opacity(0.5)
                }

                // Footer: row counts
                let total = rowCount ?? p.rows.count
                let showing = min(p.rows.count, 5)
                if total > showing {
                    Text("\(showing) of \(total) rows")
                        .font(.system(size: 9.5))
                        .foregroundColor(.secondary)
                        .padding(.horizontal, 6)
                        .padding(.top, 3)
                }
            }
            .codeBlockSurface(cornerRadius: 5)
        }
    }
}

// MARK: - Raw response (full tool output in an expanded log entry)

// Shows the full agent-visible response inline. Short responses render whole;
// long ones are clamped with an "Open" affordance to a focused, scrollable sheet.
private struct ResponseTextBlock: View {
    let text: String
    let onOpen: () -> Void

    private static let inlineLineCap = 16
    private var lineCount: Int { text.reduce(1) { $1 == "\n" ? $0 + 1 : $0 } }
    private var isLong: Bool { lineCount > Self.inlineLineCap || text.count > 1400 }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text("RESPONSE")
                    .font(.system(size: 9.5, weight: .semibold))
                    .foregroundColor(.secondary)
                    .tracking(0.4)
                Spacer()
                if isLong {
                    Button(action: onOpen) {
                        Label("Open", systemImage: "arrow.up.left.and.arrow.down.right")
                            .font(.system(size: 9.5, weight: .medium))
                    }
                    .buttonStyle(.plain)
                    .foregroundColor(.accentColor)
                    .help("Open the full response in a window")
                }
            }
            Text(text)
                .font(.system(size: 10.5, design: .monospaced))
                .foregroundColor(.primary.opacity(0.8))
                .lineLimit(isLong ? Self.inlineLineCap : nil)
                .truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(8)
                .codeBlockSurface(cornerRadius: 5)
            if isLong {
                Text("\(lineCount) lines — Open to see the full response")
                    .font(.system(size: 9))
                    .foregroundColor(.secondary)
            }
        }
    }
}

// Focused, resizable view of a full response: scrollable, selectable, copyable.
private struct ResponseSheet: View {
    let title: String
    let text: String
    @SwiftUI.Environment(\.dismiss) private var dismiss
    @State private var copied = false

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                VStack(alignment: .leading, spacing: 1) {
                    Text("Response").font(.system(size: 13, weight: .semibold))
                    Text(title)
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundColor(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer()
                Button(copied ? "Copied" : "Copy") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(text, forType: .string)
                    copied = true
                }
                .controlSize(.small)
                Button("Done") { dismiss() }
                    .controlSize(.small)
                    .keyboardShortcut(.defaultAction)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
            Divider()
            ScrollView {
                Text(text)
                    .font(.system(size: 12, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(16)
            }
        }
        .frame(width: 680, height: 560)
        .glassPanelBackground()
    }
}
