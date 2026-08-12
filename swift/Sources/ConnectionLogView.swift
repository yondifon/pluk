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
    @State private var timeRange: TimeRange = .all
    @State private var search = ""
    @State private var expandedId: Int? = nil
    @State private var showRetentionPicker = false
    @State private var pollTimer: Timer? = nil
    @State private var nextCursor: LogCursor?
    @State private var hasMore = false
    @State private var isLoading = false
    @State private var loadedOlderPage = false
    @State private var requestGeneration = 0
    @State private var scrollPosition: Int?
    @State private var refreshAfterLoad = false
    @SwiftUI.Environment(\.accessibilityReduceMotion) private var reduceMotion

    enum VerdictFilter: String, CaseIterable {
        case all = "All"
        case allowed = "Allowed"
        case blocked = "Blocked"
        case error = "Error"
    }

    enum TimeRange: String, CaseIterable, Identifiable {
        case lastHour = "hour"
        case today
        case last7Days = "7d"
        case last30Days = "30d"
        case all

        var id: String { rawValue }

        var label: String {
            switch self {
            case .lastHour: "Last hour"
            case .today: "Today"
            case .last7Days: "Last 7 days"
            case .last30Days: "Last 30 days"
            case .all: "All"
            }
        }
    }

    private var hasPending: Bool { entries.contains { $0.verdict == "pending" } }

    private var activityKey: String {
        switch scope {
        case .connection(let c): return ConnectionStore.activityKey(connection: c.id)
        case .group(let g): return ConnectionStore.activityKey(group: g.id)
        }
    }

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
            if filtered.isEmpty && !isLoading && !hasMore {
                emptyState
            } else {
                logList
            }
        }
        .onAppear {
            reload(reset: true)
            startPollingIfNeeded()
        }
        .onDisappear {
            stopPolling()
        }
        .onChange(of: hasPending) { _, pending in
            pending ? startPollingIfNeeded() : stopPolling()
        }
        .onChange(of: store.activityRevision(for: activityKey).value) { _, _ in
            reload()
        }
        .onChange(of: timeRange) { _, _ in
            reload(reset: true)
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
        HStack(spacing: Space.md) {
            searchField
            Spacer(minLength: Space.sm)
            timeRangeMenu
            verdictMenu
            retentionMenu
            refreshButton
        }
        .padding(.horizontal, Space.xl)
        .padding(.vertical, Space.sm)
    }

    private var searchField: some View {
        HStack(spacing: Space.xs) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 10))
                .foregroundColor(.secondary)
            TextField(scope.isGroup ? "Filter SQL, tool, integration…" : "Filter SQL or tool…", text: $search)
                .textFieldStyle(.plain)
                .scaledFont(.callout)
                .lineLimit(1)
            if !search.isEmpty {
                Button { search = "" } label: {
                    Image(systemName: "xmark.circle.fill").font(.system(size: 10))
                }
                .buttonStyle(.plain)
                .foregroundColor(.secondary)
            }
        }
        .padding(.horizontal, Space.sm)
        .padding(.vertical, Space.xs)
        .background(Color.controlFill)
        .clipShape(.capsule)
        .frame(minWidth: 160, idealWidth: 280, maxWidth: 320)
        .layoutPriority(1)
    }

    private var verdictMenu: some View {
        Menu {
            ForEach(VerdictFilter.allCases, id: \.self) { f in
                Button { filter = f } label: {
                    HStack(spacing: Space.xs) {
                        Text(f.rawValue)
                        Text(verdictCount(f))
                            .font(.mono(10))
                            .foregroundStyle(.tertiary)
                    }
                }
            }
        } label: {
            HStack(spacing: Space.xs) {
                Image(systemName: "line.3.horizontal.decrease.circle")
                    .font(.system(size: 10))
                Text(filter.rawValue)
                    .scaledFont(.callout)
                Text("· \(verdictCount(filter))")
                    .font(.mono(10))
                    .foregroundStyle(.tertiary)
            }
            .foregroundColor(.secondary)
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .help("Filter by verdict")
    }

    private func verdictCount(_ f: VerdictFilter) -> String {
        switch f {
        case .all:     "\(entries.count)"
        case .allowed: "\(stats.allowed)"
        case .blocked: "\(stats.blocked)"
        case .error:   "\(stats.error)"
        }
    }

    private var timeRangeMenu: some View {
        Menu {
            ForEach(TimeRange.allCases) { range in
                Button { timeRange = range } label: {
                    Text(range.label)
                }
            }
        } label: {
            HStack(spacing: Space.xs) {
                Image(systemName: "calendar")
                    .font(.system(size: 10))
                Text(timeRange.label)
                    .scaledFont(.callout)
            }
            .foregroundColor(.secondary)
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .help("Choose a time range")
    }

    private var retentionMenu: some View {
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
            HStack(spacing: Space.xxs) {
                Image(systemName: "clock.arrow.circlepath")
                    .font(.system(size: 10))
                let days = store.logRetentionDays
                Text(days == 0 ? "Forever" : "\(days)d")
                    .scaledFont(.callout)
            }
            .foregroundColor(.secondary)
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .help("Log retention — how long to keep activity history")
    }

    private var refreshButton: some View {
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

    // MARK: - Empty state

    private var emptyState: some View {
        VStack(spacing: Space.sm) {
            Image(systemName: "list.bullet.rectangle")
                .font(.system(size: 28))
                .foregroundColor(.secondary.opacity(0.4))
            Text(emptyTitle)
                .scaledFont(.body)
                .foregroundColor(.secondary)
            Text(emptySubtitle)
                .scaledFont(.caption)
                .foregroundColor(.secondary.opacity(0.7))
                .multilineTextAlignment(.center)
                .frame(maxWidth: 280)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var emptyTitle: String {
        if !search.trimmingCharacters(in: .whitespaces).isEmpty { return "No matches" }
        if filter != .all { return "No \(filter.rawValue.lowercased()) activity" }
        return timeRange == .all ? "No activity yet" : "No activity in this range"
    }

    private var emptySubtitle: String {
        if !search.trimmingCharacters(in: .whitespaces).isEmpty {
            return "No log entries match “\(search)”."
        }
        if timeRange != .all { return "Try a wider time range." }
        return scope.isGroup
            ? "Activity from agents using this group's endpoint will appear here, across every integration."
            : "Activity from agents using this integration will appear here."
    }

    // MARK: - Log list

    private var logList: some View {
        ScrollView {
            LazyVStack(spacing: Space.xs, pinnedViews: []) {
                ForEach(filtered) { entry in
                    let expanded = expandedId == entry.id
                    LogEntryRow(
                        entry: entry,
                        isExpanded: expanded,
                        showConnection: scope.isGroup,
                        onToggle: { expandedId = expanded ? nil : entry.id },
                        onStop: { stopQuery(entry) }
                    )
                }
                loadOlderRow
            }
            .scrollTargetLayout()
        }
        .scrollPosition(id: $scrollPosition)
    }

    private var loadOlderRow: some View {
        Group {
            if isLoading {
                HStack(spacing: Space.xs) {
                    ProgressView()
                        .controlSize(.small)
                    Text("Loading older entries…")
                        .scaledFont(.callout)
                }
                .foregroundColor(.secondary)
            } else if hasMore {
                Button(action: loadMore) {
                    Label("Load older entries", systemImage: "clock.arrow.down")
                        .scaledFont(.callout)
                }
                .buttonStyle(.plain)
                .foregroundColor(.secondary)
            } else {
                Text("You’re viewing all activity")
                    .scaledFont(.caption)
                    .foregroundColor(.secondary.opacity(0.7))
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, Space.md)
        .onAppear {
            if hasMore && !isLoading { loadMore() }
        }
    }

    private func reload(reset: Bool = false) {
        if isLoading && !reset {
            refreshAfterLoad = true
            return
        }
        requestGeneration += 1
        let generation = requestGeneration
        let range = timeRange.rawValue
        if reset {
            entries = []
            nextCursor = nil
            hasMore = false
            loadedOlderPage = false
            scrollPosition = nil
            refreshAfterLoad = false
        }
        isLoading = true
        Task {
            let page = await fetchPage(range: range, cursor: nil)
            guard generation == requestGeneration else { return }
            guard let page else {
                finishLoading()
                return
            }
            merge(page.entries)
            if reset || !loadedOlderPage {
                nextCursor = page.nextCursor
                hasMore = page.hasMore
            }
            if reset { scrollPosition = entries.first?.id }
            finishLoading()
        }
    }

    private func loadMore() {
        guard !isLoading, hasMore, let cursor = nextCursor else { return }
        let generation = requestGeneration
        let range = timeRange.rawValue
        isLoading = true
        Task {
            let page = await fetchPage(range: range, cursor: cursor)
            guard generation == requestGeneration else { return }
            guard let page else {
                finishLoading()
                return
            }
            merge(page.entries)
            nextCursor = page.nextCursor
            hasMore = page.hasMore
            loadedOlderPage = true
            finishLoading()
        }
    }

    private func finishLoading() {
        isLoading = false
        guard refreshAfterLoad else { return }
        refreshAfterLoad = false
        reload()
    }

    private func fetchPage(range: String, cursor: LogCursor?) async -> LogPage? {
        switch scope {
        case .connection(let connection):
            return await store.fetchLogPage(
                connectionId: connection.id,
                groupId: nil,
                range: range,
                cursor: cursor
            )
        case .group(let group):
            return await store.fetchLogPage(
                connectionId: nil,
                groupId: group.id,
                range: range,
                cursor: cursor
            )
        }
    }

    private func merge(_ incoming: [QueryLogEntry]) {
        var byId = Dictionary(uniqueKeysWithValues: entries.map { ($0.id, $0) })
        for entry in incoming { byId[entry.id] = entry }
        entries = byId.values.sorted {
            if $0.createdAt != $1.createdAt { return $0.createdAt > $1.createdAt }
            return $0.id > $1.id
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
            HStack(alignment: .top, spacing: Space.md) {
                VStack(alignment: .leading, spacing: Space.xs) {
                    // Top row: badge + member/tool chips + SQL preview
                    HStack(spacing: Space.sm) {
                        VerdictBadge(verdict: entry.verdict)

                        if showConnection {
                            chip(entry.connectionName, system: "circle.grid.2x2")
                        }

                        if let source = entry.source, !source.isEmpty {
                            chip(source, system: "wrench.and.screwdriver")
                        }

                        if let cats = entry.categories, !cats.isEmpty {
                            Text(cats)
                                .scaledFont(.caption)
                                .foregroundColor(.secondary)
                                .lineLimit(1)
                        }

                        Spacer()

                        if entry.verdict == "pending" {
                            Button(action: onStop) {
                                Label("Stop", systemImage: "stop.fill")
                                    .scaledFont(.callout)
                                    .fontWeight(.medium)
                            }
                            .buttonStyle(.plain)
                            .foregroundColor(.red)
                            .help("Cancel this running query")
                        }

                        Text(relativeTime(entry.createdAt))
                            .font(.mono(10))
                            .foregroundColor(.secondary)
                            .lineLimit(1)
                    }

                    // Query — a one-line preview when collapsed; a structured,
                    // selectable code block when expanded (mirrors the response).
                    if isExpanded {
                        VStack(alignment: .leading, spacing: Space.xs) {
                            Text("Query")
                                .scaledFont(.caption, weight: .semibold)
                                .foregroundColor(.secondary)
                            Text(CodeStyle.highlighted(entry.sql, language: .sql))
                                .scaledFont(.body, design: .monospaced)
                                .foregroundColor(.primary)
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(Space.sm)
                                .codeBlockSurface(cornerRadius: Radius.small)
                        }
                    } else {
                        Text(entry.sql)
                            .scaledFont(.footnote, design: .monospaced)
                            .foregroundColor(.secondary)
                            .lineLimit(1)
                            .truncationMode(.tail)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }

                    // Expanded: reason + result preview + full timestamp
                    if isExpanded {
                        if let reason = entry.reason, !reason.isEmpty {
                            HStack(spacing: Space.xs) {
                                Image(systemName: "exclamationmark.circle.fill")
                                    .font(.system(size: 10))
                                    .foregroundColor(verdictColor)
                                Text(reason)
                                    .scaledFont(.caption)
                                    .foregroundColor(.secondary)
                            }
                            .padding(.top, Space.xxs)
                        }

                        // Full response: raw tool output when stored, else the
                        // structured result rows as a mini-table.
                        if let raw = entry.responseText, !raw.isEmpty {
                            ResponseTextBlock(text: raw) { showResponseSheet = true }
                                .padding(.top, Space.sm)
                        } else if let json = entry.resultJson {
                            ResultPreview(json: json, rowCount: entry.rowCount)
                                .padding(.top, Space.sm)
                        }

                        Text(localTime(entry.createdAt))
                            .font(.mono(10))
                            .foregroundColor(.secondary.opacity(0.7))
                            .padding(.top, Space.xxs)

                        // Copy actions for the query and its response
                        HStack(spacing: Space.sm) {
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
                        .padding(.top, Space.sm)
                    }
                }
                .padding(.vertical, Space.lg)
                .padding(.trailing, Space.xl)
            }
            .padding(.leading, Space.xl)
        }
        .contentShape(Rectangle())
        .onTapGesture { onToggle() }
        .accessibilityAddTraits(.isButton)
        .accessibilityAction(.default) { onToggle() }
        .overlay(alignment: .leading) {
            if isExpanded {
                AccentRule(color: .secondary.opacity(0.25))
                    .padding(.vertical, Space.lg)
            }
        }
        .animation(reduceMotion ? nil : .easeInOut(duration: 0.12), value: isExpanded)
        .sheet(isPresented: $showResponseSheet) {
            ResponseSheet(title: entry.sql, text: fullResponse ?? "")
        }
    }

    private func copyButton(_ title: String, copied: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(title, systemImage: copied ? "checkmark" : "doc.on.doc")
                .scaledFont(.callout)
                .fontWeight(.medium)
        }
        .buttonStyle(.bordered)
        .controlSize(.mini)
        .tint(copied ? .green : nil)
    }

    // Plain monospace tag for the member integration / originating tool.
    private func chip(_ text: String, system: String) -> some View {
        HStack(spacing: Space.xxs) {
            Image(systemName: system).font(.system(size: 8))
            Text(text).font(.mono(10, weight: .medium)).lineLimit(1)
        }
        .foregroundColor(.secondary)
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
        case "allowed":   return .green.opacity(0.7)
        case "blocked":   return .orange
        case "cancelled": return .secondary
        case "pending":   return .secondary
        default:          return .red.opacity(0.9)
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

    @SwiftUI.Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var pulsing = false

    var body: some View {
        HStack(spacing: Space.xs) {
            dot
            Text(label)
                .scaledFont(.caption, weight: .medium)
                .foregroundColor(.secondary)
        }
    }

    @ViewBuilder
    private var dot: some View {
        if verdict == "pending" {
            Circle()
                .fill(Color.secondary)
                .frame(width: 6, height: 6)
                .scaleEffect(pulsing ? 0.72 : 1)
                .opacity(pulsing ? 0.3 : 1)
                .onAppear {
                    guard !reduceMotion else { return }
                    withAnimation(.easeInOut(duration: 0.85).repeatForever(autoreverses: true)) {
                        pulsing = true
                    }
                }
        } else {
            Circle()
                .fill(color)
                .frame(width: 6, height: 6)
        }
    }

    private var label: String {
        switch verdict {
        case "allowed":   return "ok"
        case "blocked":   return "blocked"
        case "cancelled": return "cancelled"
        case "pending":   return "running"
        default:          return "error"
        }
    }

    private var color: Color {
        switch verdict {
        case "allowed":   return .green.opacity(0.7)
        case "blocked":   return .orange
        case "cancelled": return .secondary
        default:          return .red.opacity(0.9)
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
                                .scaledFont(.footnote, weight: .semibold, design: .monospaced)
                                .foregroundColor(.secondary)
                                .lineLimit(1)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.horizontal, Space.sm)
                                .padding(.vertical, Space.xxs)
                                .background(Color.controlFill)
                        }
                    }
                    .clipShape(.rect(cornerRadius: Radius.small, style: .continuous))

                    // Data rows
                    ForEach(p.rows) { row in
                        HStack(spacing: 0) {
                            ForEach(row.cells.prefix(6)) { cell in
                                Text(cell.text)
                                    .scaledFont(.footnote, design: .monospaced)
                                    .foregroundColor(.primary.opacity(0.75))
                                    .lineLimit(1)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .padding(.horizontal, Space.sm)
                                    .padding(.vertical, Space.xxs)
                            }
                        }
                    }

                    // Footer: row counts
                    let total = rowCount ?? p.rows.count
                    let showing = min(p.rows.count, 5)
                    if total > showing {
                        Text("\(showing) of \(total) rows")
                            .scaledFont(.footnote, design: .monospaced)
                            .foregroundColor(.secondary)
                            .padding(.horizontal, Space.sm)
                            .padding(.top, Space.xxs)
                    }
                }
                .codeBlockSurface(cornerRadius: Radius.small)
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
    @SwiftUI.Environment(\.uiScale) private var uiScale

    var body: some View {
        VStack(alignment: .leading, spacing: Space.xs) {
            HStack {
                Text("Response")
                    .scaledFont(.caption, weight: .semibold)
                    .foregroundColor(.secondary)
                Spacer()
                if moreToShow {
                    Button(action: onOpen) {
                        Label("Open", systemImage: "arrow.up.left.and.arrow.down.right")
                            .scaledFont(.callout)
                            .fontWeight(.medium)
                    }
                    .buttonStyle(.plain)
                    .foregroundColor(.accentColor)
                    .help("Open the full response in a window")
                }
            }
            MarkdownResponseView(markdown: preview, embedded: true, fontSize: 13 * uiScale)
                .padding(Space.sm)
                .frame(maxWidth: .infinity, alignment: .leading)
                .codeBlockSurface(cornerRadius: Radius.small)
            if moreToShow {
                Text("Preview truncated — Open for the full, formatted response")
                    .scaledFont(.caption)
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
            HStack(spacing: Space.md) {
                VStack(alignment: .leading, spacing: Space.xxs) {
                    Text("Response").scaledFont(.headline)
                    Text(title)
                        .font(.mono(10))
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
            .padding(.horizontal, Space.lg)
            .padding(.vertical, Space.md)
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
        .background(Surface.content.ignoresSafeArea())
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
        HStack(spacing: Space.md) {
            Stepper(value: $fontSize, in: 10...24, step: 1) {
                HStack(spacing: Space.xxs) {
                    Image(systemName: "textformat.size").font(.system(size: 10))
                    Text("\(Int(fontSize))").scaledFont(.callout).monospacedDigit()
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
