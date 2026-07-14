import AppKit
import SwiftUI

// What a LogsTab is showing: a single integration's activity, or the aggregated
// feed for every member called through a group endpoint.
enum LogScope {
    case connection(Connection)
    case group(ConnectionGroup)

    var isGroup: Bool { if case .group = self { return true }; return false }
}

struct LogsTab: View {
    let scope: LogScope
    let store: ConnectionStore

    @State private var entries: [QueryLogEntry] = []
    @State private var filter: VerdictFilter = .all
    @State private var search = ""
    @State private var expandedId: Int? = nil
    @State private var showRetentionPicker = false
    @State private var pollTimer: Timer? = nil
    @SwiftUI.Environment(\.accessibilityReduceMotion) private var reduceMotion

    enum VerdictFilter: String, CaseIterable {
        case all = "All"
        case allowed = "Allowed"
        case blocked = "Blocked"
        case error = "Error"
    }

    private var hasPending: Bool { entries.contains { $0.verdict == "pending" } }

    // Free-text match across the fields an operator scans for: the SQL/command,
    // the originating tool, and (in group mode) the member name.
    private func matchesSearch(_ e: QueryLogEntry) -> Bool {
        let q = search.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return true }
        return e.sql.lowercased().contains(q)
            || (e.source?.lowercased().contains(q) ?? false)
            || e.connectionName.lowercased().contains(q)
            || (e.categories?.lowercased().contains(q) ?? false)
    }

    private var filtered: [QueryLogEntry] {
        entries.filter {
            (filter == .all || $0.verdict == filter.rawValue.lowercased()) && matchesSearch($0)
        }
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
        HStack(spacing: 12) {
            // Stats pills
            HStack(spacing: 10) {
                statPill(entries.count, label: "total", color: .secondary)
                statPill(stats.allowed, label: "ok", color: .green)
                if stats.blocked > 0 {
                    statPill(stats.blocked, label: "blocked", color: .red)
                }
                if stats.error > 0 {
                    statPill(stats.error, label: "err", color: .orange)
                }
            }
            .fixedSize()

            // Search — scan SQL / tool / member
            HStack(spacing: 5) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 10))
                    .foregroundColor(.secondary)
                TextField(scope.isGroup ? "Filter SQL, tool, integration…" : "Filter SQL or tool…", text: $search)
                    .textFieldStyle(.plain)
                    .font(.dev(size: 11))
                    .lineLimit(1)
                if !search.isEmpty {
                    Button { search = "" } label: {
                        Image(systemName: "xmark.circle.fill").font(.system(size: 10))
                    }
                    .buttonStyle(.plain)
                    .foregroundColor(.secondary)
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(Color.secondary.opacity(0.08))
            .clipShape(.capsule)
            .frame(minWidth: 120, maxWidth: 240)
            .layoutPriority(-1)

            Spacer(minLength: 8)

            // Verdict filter — segmented capsule track; labels never wrap
            HStack(spacing: 2) {
                ForEach(VerdictFilter.allCases, id: \.self) { f in
                    Button { filter = f } label: {
                        Text(f.rawValue)
                            .font(.dev(size: 11, weight: filter == f ? .semibold : .regular))
                            .foregroundColor(filter == f ? .accentColor : .secondary)
                            .lineLimit(1)
                            .fixedSize()
                            .padding(.horizontal, 9)
                            .padding(.vertical, 4)
                            .background(filter == f ? Color.accentColor.opacity(0.12) : .clear)
                            .clipShape(.capsule)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(2)
            .background(Color.secondary.opacity(0.06))
            .clipShape(.capsule)
            .fixedSize()

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
                Button(scope.isGroup ? "Clear all logs for this group" : "Clear all logs for this integration", role: .destructive) {
                    switch scope {
                    case .connection(let c): store.clearAllLogs(connectionId: c.id)
                    case .group(let g): store.clearAllLogs(groupId: g.id)
                    }
                    reload()
                }
            } label: {
                HStack(spacing: 3) {
                    Image(systemName: "clock.arrow.circlepath")
                        .font(.system(size: 10))
                    let days = store.logRetentionDays
                    Text(days == 0 ? "Forever" : "\(days)d")
                        .font(.dev(size: 11))
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
                .font(.dev(size: 11, weight: .semibold))
                .foregroundColor(color == .secondary ? .primary : color)
            Text(label)
                .font(.dev(size: 11))
                .foregroundColor(.secondary)
        }
    }

    // MARK: - Empty state

    private var emptyState: some View {
        VStack(spacing: 8) {
            Image(systemName: "list.bullet.rectangle")
                .font(.system(size: 28))
                .foregroundColor(.secondary.opacity(0.4))
            Text(emptyTitle)
                .font(.system(size: 13))
                .foregroundColor(.secondary)
            Text(emptySubtitle)
                .font(.system(size: 11))
                .foregroundColor(.secondary.opacity(0.7))
                .multilineTextAlignment(.center)
                .frame(maxWidth: 280)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var emptyTitle: String {
        if !search.trimmingCharacters(in: .whitespaces).isEmpty { return "No matches" }
        return filter == .all ? "No activity yet" : "No \(filter.rawValue.lowercased()) activity"
    }

    private var emptySubtitle: String {
        if !search.trimmingCharacters(in: .whitespaces).isEmpty {
            return "No log entries match “\(search)”."
        }
        return scope.isGroup
            ? "Activity from agents using this group's endpoint will appear here, across every integration."
            : "Activity from agents using this integration will appear here."
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
                        showConnection: scope.isGroup,
                        onToggle: { expandedId = expanded ? nil : entry.id },
                        onStop: { stopQuery(entry) }
                    )
                    Divider().padding(.leading, 18)
                }
            }
        }
    }

    private func reload() {
        switch scope {
        case .connection(let c): entries = store.recentLog(connectionId: c.id)
        case .group(let g): entries = store.recentLogForGroup(groupId: g.id)
        }
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
    let showConnection: Bool   // group view: label each row with its member integration
    let onToggle: () -> Void
    let onStop: () -> Void

    @State private var copiedSQL = false
    @State private var copiedResult = false
    @State private var showResponseSheet = false
    @SwiftUI.Environment(\.accessibilityReduceMotion) private var reduceMotion

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
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .top, spacing: 10) {
                // Verdict indicator bar
                RoundedRectangle(cornerRadius: 2)
                    .fill(verdictColor)
                    .frame(width: 3)
                    .frame(minHeight: 36)

                VStack(alignment: .leading, spacing: 4) {
                    // Top row: badge + member/tool chips + SQL preview
                    HStack(spacing: 6) {
                        VerdictBadge(verdict: entry.verdict)

                        if showConnection {
                            chip(entry.connectionName, system: "circle.grid.2x2", color: .accentColor)
                        }

                        if let source = entry.source, !source.isEmpty {
                            chip(source, system: "wrench.and.screwdriver", color: .secondary)
                        }

                        if let cats = entry.categories, !cats.isEmpty {
                            Text(cats)
                                .font(.dev(size: 10))
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
                            .font(.dev(size: 10))
                            .foregroundColor(.secondary)
                            .lineLimit(1)
                    }

                    // Query — a one-line preview when collapsed; a structured,
                    // selectable code block when expanded (mirrors the response).
                    if isExpanded {
                        VStack(alignment: .leading, spacing: 4) {
                            Text("QUERY")
                                .font(.dev(size: 9.5, weight: .semibold))
                                .foregroundColor(.secondary)
                                .tracking(0.4)
                            Text(entry.sql)
                                .font(.dev(size: 11.5))
                                .foregroundColor(.primary)
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(8)
                                .codeBlockSurface(cornerRadius: 5)
                        }
                    } else {
                        Text(entry.sql)
                            .font(.dev(size: 11.5))
                            .foregroundColor(.primary)
                            .lineLimit(1)
                            .truncationMode(.tail)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }

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
                            .font(.dev(size: 10))
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
        .contentShape(Rectangle())
        .onTapGesture { onToggle() }
        .accessibilityAddTraits(.isButton)
        .accessibilityAction(.default) { onToggle() }
        .background(isExpanded ? Color.accentColor.opacity(0.04) : .clear)
        .animation(reduceMotion ? nil : .easeInOut(duration: 0.12), value: isExpanded)
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

    // Compact monospace tag for the member integration / originating tool.
    private func chip(_ text: String, system: String, color: Color) -> some View {
        HStack(spacing: 3) {
            Image(systemName: system).font(.system(size: 8))
            Text(text).font(.dev(size: 10, weight: .medium)).lineLimit(1)
        }
        .foregroundColor(color)
        .padding(.horizontal, 6)
        .padding(.vertical, 2)
        .background(color.opacity(0.1))
        .clipShape(.capsule)
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

    private static let utcFormatter: DateFormatter = {
        let fmt = DateFormatter()
        fmt.dateFormat = "yyyy-MM-dd HH:mm:ss"
        fmt.locale = Locale(identifier: "en_US_POSIX")
        fmt.timeZone = TimeZone(identifier: "UTC")  // SQLite datetime('now') is UTC
        return fmt
    }()

    private static let localFormatter: DateFormatter = {
        let fmt = DateFormatter()
        fmt.dateFormat = "yyyy-MM-dd HH:mm:ss"
        fmt.locale = Locale(identifier: "en_US_POSIX")
        fmt.timeZone = .current
        return fmt
    }()

    // "2 min ago" / "just now" / falls back to raw string for older entries
    private func relativeTime(_ raw: String) -> String {
        guard let date = Self.utcFormatter.date(from: raw) else { return raw }
        let secs = Int(-date.timeIntervalSinceNow)
        if secs < 10  { return "just now" }
        if secs < 60  { return "\(secs)s ago" }
        if secs < 3600 { return "\(secs / 60)m ago" }
        if secs < 86400 { return "\(secs / 3600)h ago" }
        return "\(secs / 86400)d ago"
    }

    // Full UTC timestamp -> local time string
    private func localTime(_ raw: String) -> String {
        guard let date = Self.utcFormatter.date(from: raw) else { return raw }
        return Self.localFormatter.string(from: date)
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
                    .font(.dev(size: 9, weight: .bold))
                    .foregroundColor(.secondary)
            }
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Color.secondary.opacity(0.1))
            .clipShape(.capsule)
        } else {
            Text(label)
                .font(.dev(size: 9, weight: .bold))
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
        struct Cell: Identifiable {
            let id: String
            let text: String
        }
        struct Row: Identifiable {
            let id: String
            let cells: [Cell]
        }
        let fields: [String]
        let rows: [Row]
    }

    private var parsed: ParsedResult? {
        guard let data = json.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let fields = obj["fields"] as? [String],
              let rows = obj["rows"] as? [[String: Any]] else { return nil }

        let parsedRows: [ParsedResult.Row] = rows.prefix(5).enumerated().map { rowIndex, row in
            let cells: [ParsedResult.Cell] = fields.enumerated().map { colIndex, key in
                let text: String
                if let val = row[key], !(val is NSNull) {
                    text = "\(val)"
                } else {
                    text = "NULL"
                }
                return ParsedResult.Cell(id: "\(rowIndex)-\(colIndex)", text: text)
            }
            let contentId = cells.map(\.text).joined(separator: "\u{001F}")
            return ParsedResult.Row(id: "\(rowIndex)-\(contentId)", cells: cells)
        }
        return ParsedResult(fields: fields, rows: parsedRows)
    }

    var body: some View {
        Group {
            if let p = parsed, !p.fields.isEmpty {
                VStack(alignment: .leading, spacing: 0) {
                    // Header row
                    HStack(spacing: 0) {
                        ForEach(p.fields.prefix(6), id: \.self) { field in
                            Text(field)
                                .font(.dev(size: 9.5, weight: .semibold))
                                .foregroundColor(.secondary)
                                .lineLimit(1)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.horizontal, 6)
                                .padding(.vertical, 3)
                                .background(Color.secondary.opacity(0.08))
                        }
                    }
                    .clipShape(.rect(cornerRadius: 4, style: .continuous))

                    // Data rows
                    ForEach(p.rows) { row in
                        HStack(spacing: 0) {
                            ForEach(row.cells.prefix(6)) { cell in
                                Text(cell.text)
                                    .font(.dev(size: 9.5))
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
                            .font(.dev(size: 9.5))
                            .foregroundColor(.secondary)
                            .padding(.horizontal, 6)
                            .padding(.top, 3)
                    }
                }
                .codeBlockSurface(cornerRadius: 5)
            }
        }
    }
}

// MARK: - Raw response (full tool output in an expanded log entry)

// Shows the full agent-visible response inline using the same Markdown renderer
// as the focused response sheet. Long responses keep an "Open" affordance for
// keyboard-friendly inspection in a larger surface.
private struct ResponseTextBlock: View {
    let text: String
    let onOpen: () -> Void

    // Inline is a cheap teaser only. The full response can be megabytes of
    // minified JSON; pretty-printing or highlighting all of it to show a peek is
    // what froze the row on expand. Slice raw first, format only the slice.
    private static let previewLines = 10
    private static let previewChars = 1200

    @State private var preview = ""
    @State private var moreToShow = false

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text("RESPONSE")
                    .font(.dev(size: 9.5, weight: .semibold))
                    .foregroundColor(.secondary)
                    .tracking(0.4)
                Spacer()
                if moreToShow {
                    Button(action: onOpen) {
                        Label("Open", systemImage: "arrow.up.left.and.arrow.down.right")
                            .font(.dev(size: 9.5, weight: .medium))
                    }
                    .buttonStyle(.plain)
                    .foregroundColor(.accentColor)
                    .help("Open the full response in a window")
                }
            }
            MarkdownResponseView(markdown: preview, embedded: true)
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .codeBlockSurface(cornerRadius: 5)
            if moreToShow {
                Text("Preview truncated — Open for the full, formatted response")
                    .font(.dev(size: 9))
                    .foregroundColor(.secondary)
            }
        }
        .task(id: text) { buildPreview() }
    }

    private func buildPreview() {
        let rawLines = text.components(separatedBy: "\n")
        let slice = rawLines.prefix(Self.previewLines).joined(separator: "\n")
        let capped = String(slice.prefix(Self.previewChars))
        moreToShow = rawLines.count > Self.previewLines || text.count > capped.count
        // Formatting a ~1 KB slice is cheap; if the slice isn't valid JSON on its
        // own it just renders as-is.
        preview = ResponseFormatter.formatted(capped)
    }
}

// Focused, resizable view of a full response: pretty-printed, scrollable,
// selectable, copyable. Reader controls the point size and line height (both
// persisted) so long reviews stay legible.
private struct ResponseSheet: View {
    let title: String
    let text: String
    @SwiftUI.Environment(\.dismiss) private var dismiss
    @State private var copied = false
    @State private var display = ""
    @AppStorage("responseFontSize") private var fontSize: Double = 13
    @AppStorage("responseLineHeight") private var lineHeight: Double = 4

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 1) {
                    Text("Response").font(.system(size: 13, weight: .semibold))
                    Text(title)
                        .font(.dev(size: 10))
                        .foregroundColor(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer()
                typeControls
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
            Group {
                if display.isEmpty {
                    ProgressView("Formatting…")
                        .controlSize(.small)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    let block = Self.singleCodeBlock(display)
                    CodeTextView(
                        code: block?.code ?? display,
                        language: block?.language ?? "text",
                        fontSize: CGFloat(fontSize),
                        lineSpacing: CGFloat(lineHeight)
                    )
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(minWidth: 560, idealWidth: 780, maxWidth: .infinity,
               minHeight: 460, idealHeight: 660, maxHeight: .infinity)
        .glassPanelBackground()
        .task {
            guard display.isEmpty else { return }
            // Off the main actor: pretty-printing a large payload must not block
            // the sheet from appearing.
            display = await Task.detached { ResponseFormatter.formatted(text) }.value
        }
    }

    // When the formatted response is a single fenced block (the usual case —
    // pretty-printed JSON), unwrap it so the code viewer gets clean source and
    // the right language. Mixed prose falls back to rendering the raw text.
    private static func singleCodeBlock(_ s: String) -> (code: String, language: String)? {
        let trimmed = s.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix("```"), trimmed.hasSuffix("```"),
              trimmed.components(separatedBy: "```").count == 3 else { return nil }
        var lines = trimmed.components(separatedBy: "\n")
        let language = String(lines.removeFirst().dropFirst(3))
            .trimmingCharacters(in: .whitespaces)
        if lines.last?.trimmingCharacters(in: .whitespaces) == "```" { lines.removeLast() }
        return (lines.joined(separator: "\n"), language.isEmpty ? "text" : language)
    }

    // Font size + line-height steppers. Small, monospaced-digit readout so the
    // header width doesn't jump as the numbers change.
    private var typeControls: some View {
        HStack(spacing: 10) {
            Stepper(value: $fontSize, in: 10...24, step: 1) {
                HStack(spacing: 3) {
                    Image(systemName: "textformat.size").font(.system(size: 10))
                    Text("\(Int(fontSize))").font(.dev(size: 11)).monospacedDigit()
                }
            }
            .controlSize(.small)
            .fixedSize()
            .help("Text size")

            Stepper(value: $lineHeight, in: 0...14, step: 1) {
                Image(systemName: "arrow.up.and.down.text.horizontal").font(.system(size: 11))
            }
            .controlSize(.small)
            .fixedSize()
            .help("Line height")
        }
    }
}
