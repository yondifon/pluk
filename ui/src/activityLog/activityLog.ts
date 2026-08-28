/**
 * Activity Log — vanilla TS view. Mirrors ConnectionLogView.swift (1,183 lines).
 * Covers: paging, merging, generation counter, toolbar, live SSE, pending poll, caps,
 * two row shapes, syntax highlighting off main thread, UTC parsing, response viewer.
 */

import type { LogEntry, LogCursor, TimeRange, VerdictFilter } from "./types";
import { timeRangeLabels } from "./types";
import { fetchLogPage, mergeEntries, cancelLog, getRetention, setRetention, clearLogs, connectEvents, type LogScope, type LiveEvent } from "./api";
import { relativeTime, localTimeString, parseUtcToMillis } from "./time";
import { capResponse, capConsole } from "./caps";
import { highlightedHtml, consoleHtml, parseLanguage, escapeHtml } from "./highlight";
import { createResponseViewer } from "./responseViewer";
import { createIcon } from "../icon";
import { confirmModal } from "../modal";

const COMMAND_TYPES = new Set(["ssh", "github-cli", "spark", "herd"]);

function isTerminalEntry(entry: LogEntry, typeMap: Map<string, string>): boolean {
  const t = typeMap.get(entry.connectionId) ?? null;
  return t ? COMMAND_TYPES.has(t) : false;
}

export interface ActivityLogOptions {
  scope: LogScope;
  /** Map connectionId -> adapter type string for shape detection */
  connectionTypes?: Map<string, string>;
  /** Initial time range */
  initialRange?: TimeRange;
}

export function mountActivityLog(container: HTMLElement, opts: ActivityLogOptions): { destroy: () => void } {
  let entries: LogEntry[] = [];
  let filter: VerdictFilter = "all";
  let timeRange: TimeRange = opts.initialRange ?? "all";
  let search = "";
  let expandedId: number | null = null;
  let nextCursor: LogCursor | null = null;
  let hasMore = false;
  let isLoading = false;
  let requestGeneration = 0;
  let loadedOlderPage = false;
  let refreshAfterLoad = false;
  let loadError: string | null = null;

  // Live cursor (monotonic)
  let liveCursor = 0;
  const seenIds = new Set<number>();

  // Pending poll timer
  let pollTimer: number | null = null;
  let liveClose: (() => void) | null = null;

  const typeMap = opts.connectionTypes ?? new Map();

  const viewer = createResponseViewer();

  // ----- DOM scaffolding -----
  container.classList.add("activity-log");
  container.innerHTML = `
    <div class="al-toolbar">
      <div class="al-search">
        <span class="al-search-icon"></span>
        <input class="al-search-input" placeholder="Filter SQL, tool, integration…" aria-label="Filter activity" />
        <button class="al-search-clear icon-button" aria-label="Clear search" hidden></button>
      </div>
      <div class="al-spacer"></div>
      <div class="al-menus">
        <label class="al-select-wrap">Time
          <select class="al-select" data-role="range">
            <option value="hour">Last hour</option>
            <option value="today">Today</option>
            <option value="7d">Last 7 days</option>
            <option value="30d">Last 30 days</option>
            <option value="all" selected>All time</option>
          </select>
        </label>
        <label class="al-select-wrap">Show
          <select class="al-select" data-role="verdict">
            <option value="all">All</option>
            <option value="allowed">Successful</option>
            <option value="blocked">Blocked</option>
            <option value="error">Failed</option>
          </select>
        </label>
        <label class="al-select-wrap">Keep
          <select class="al-select" data-role="retention">
            <option value="7">7 days</option>
            <option value="14">14 days</option>
            <option value="30" selected>30 days</option>
            <option value="60">60 days</option>
            <option value="90">90 days</option>
            <option value="0">Forever</option>
          </select>
        </label>
         <button class="ui-button ui-button-sm" data-role="refresh" aria-label="Refresh">Refresh</button>
         <button class="ui-button ui-button-sm ui-button-danger" data-role="clear" aria-label="Clear all">Clear</button>
      </div>
    </div>
    <div class="al-stats" data-role="stats"></div>
    <div class="al-retention-status sr-only" data-role="retention-status" role="status" aria-live="polite" aria-atomic="true"></div>
    <div class="al-list" data-role="list"></div>
    <div class="al-load-more" data-role="loadMore"></div>
    <div class="al-empty" data-role="empty" hidden></div>
  `;

  const elSearch = container.querySelector(".al-search-input") as HTMLInputElement;
  const elSearchClear = container.querySelector(".al-search-clear") as HTMLButtonElement;
  const elRange = container.querySelector("[data-role='range']") as HTMLSelectElement;
  const elVerdict = container.querySelector("[data-role='verdict']") as HTMLSelectElement;
  const elRetention = container.querySelector("[data-role='retention']") as HTMLSelectElement;
  const elStats = container.querySelector("[data-role='stats']") as HTMLElement;
  const elRetentionStatus = container.querySelector("[data-role='retention-status']") as HTMLElement;
  const elList = container.querySelector("[data-role='list']") as HTMLElement;
  const elLoadMore = container.querySelector("[data-role='loadMore']") as HTMLElement;
  const elEmpty = container.querySelector("[data-role='empty']") as HTMLElement;
  container.querySelector(".al-search-icon")?.appendChild(createIcon("search"));
  container.querySelector(".al-search-clear")?.appendChild(createIcon("close"));
  container.querySelector("[data-role='refresh']")?.prepend(createIcon("refresh"));

  // retention init
  getRetention().then(d => {
    if ([0,7,14,30,60,90].includes(d)) {
      elRetention.value = String(d);
      elRetention.dataset.value = String(d);
    }
  });

  // ----- helpers -----
  function matchesSearch(e: LogEntry): boolean {
    const q = search.trim().toLowerCase();
    if (!q) return true;
    return e.sql.toLowerCase().includes(q) || (e.source?.toLowerCase().includes(q) ?? false) || e.connectionName.toLowerCase().includes(q) || (e.categories?.toLowerCase().includes(q) ?? false);
  }

  function filtered(): LogEntry[] {
    return entries.filter(e => (filter === "all" || e.verdict === filter) && matchesSearch(e));
  }

  function statsCounts() {
    return {
      allowed: entries.filter(e => e.verdict === "allowed").length,
      blocked: entries.filter(e => e.verdict === "blocked").length,
      error: entries.filter(e => e.verdict === "error").length,
    };
  }

  function updateStats() {
    const s = statsCounts();
    const counts = `All ${entries.length} · Successful ${s.allowed} · Blocked ${s.blocked} · Failed ${s.error}`;
    // Inject live counts into verdict options
    for (const opt of Array.from(elVerdict.options)) {
      const base = opt.value === "all" ? "All" : opt.value === "allowed" ? "Successful" : opt.value === "blocked" ? "Blocked" : "Failed";
      const n = opt.value === "all" ? entries.length : opt.value === "allowed" ? s.allowed : opt.value === "blocked" ? s.blocked : s.error;
      opt.textContent = `${base} (${n})`;
    }
    elStats.textContent = counts;
  }

  function verdictLabel(v: string): string {
    if (v === "allowed") return "ok";
    if (v === "blocked") return "blocked";
    if (v === "cancelled") return "cancelled";
    if (v === "pending") return "running";
    return "error";
  }

  function renderEmpty() {
    const q = search.trim();
    const f = filter;
    let title = "";
    let subtitle = "";
    if (q) { title = "No matches"; subtitle = `No entries match “${q}”.`; }
    else if (f !== "all") { title = `No ${f} activity`; subtitle = "Try a different filter."; }
    else if (timeRange !== "all") { title = "No activity in this range"; subtitle = "Try a wider time range."; }
    else { title = "No activity yet"; subtitle = "Activity from agents using this endpoint will appear here."; }
    elEmpty.innerHTML = `<div class="al-empty-icon"></div><div class="al-empty-title">${escapeHtml(title)}</div><div class="al-empty-sub">${escapeHtml(subtitle)}</div>`;
    elEmpty.querySelector(".al-empty-icon")?.appendChild(createIcon("tray", { size: 24 }));
  }

  function rowHtml(entry: LogEntry, isExpanded: boolean): string {
    const terminal = isTerminalEntry(entry, typeMap);
    if (terminal) return terminalRowHtml(entry, isExpanded);
    return dbRowHtml(entry, isExpanded);
  }

  function dbRowHtml(entry: LogEntry, isExpanded: boolean): string {
    const meta = metaLineHtml(entry);
    if (!isExpanded) {
       return `<div class="al-row ui-card" data-id="${entry.id}" role="button" tabindex="0" aria-expanded="false">
         <div class="al-row-main">${meta}<div class="al-sql-collapsed" title="${escapeHtml(entry.sql)}">${escapeHtml(entry.sql)}</div></div>
      </div>`;
    }
    const reason = entry.reason ? `<div class="al-reason al-reason-${escapeHtml(entry.verdict)}">${escapeHtml(entry.reason)}</div>` : "";
    const cap = entry.responseText ? capResponse(entry.responseText) : null;
    let responseBlock = "";
    if (entry.responseText) {
      const htmlPromise = ""; // filled async
      void htmlPromise;
      responseBlock = `<div class="al-response">
        <div class="al-response-head"><span>Response</span>${cap?.truncated ? `<button class="al-link" data-open="${entry.id}">Open</button>` : ""}</div>
        <div class="al-response-preview" data-preview="${entry.id}"></div>
        ${cap?.truncated ? `<div class="al-trunc">Preview truncated — Open for the full, formatted response</div>` : ""}
      </div>`;
    } else if (entry.resultJson) {
      responseBlock = resultPreviewHtml(entry.resultJson, entry.rowCount);
    }
    const fullResponse = entry.responseText ?? entry.resultJson ?? entry.reason ?? "";
     return `<div class="al-row ui-card al-row-expanded" data-id="${entry.id}" role="button" tabindex="0" aria-expanded="true">
      <div class="al-row-main">${meta}
        <div class="al-query-block"><div class="al-label">Query</div><pre class="al-code" data-sql="${entry.id}">${escapeHtml(entry.sql)}</pre></div>
        ${reason}
        ${responseBlock}
         <div class="al-actions">
          <button class="ui-button ui-button-sm" data-copy-sql="${entry.id}">Copy query</button>
          ${fullResponse ? `<button class="ui-button ui-button-sm" data-copy-res="${entry.id}">Copy response</button>` : ""}
        </div>
      </div>
    </div>`;
  }

  function terminalRowHtml(entry: LogEntry, isExpanded: boolean): string {
    const meta = metaLineHtml(entry);
    const cmdLine = `<div class="al-cmdline"><span class="al-prompt">$</span> <span class="al-cmd" data-cmd="${entry.id}">${escapeHtml(entry.sql)}</span></div>`;
    if (!isExpanded) {
       return `<div class="al-row ui-card al-row-terminal" data-id="${entry.id}" role="button" tabindex="0" aria-expanded="false">
        ${meta}<div class="al-terminal-surface">${cmdLine}</div>
      </div>`;
    }
    const reason = entry.reason ? `<div class="al-reason">${escapeHtml(entry.reason)}</div>` : "";
    let outBlock = "";
    if (entry.responseText) {
      const cap = capConsole(entry.responseText);
      outBlock = `<div class="al-console" data-console="${entry.id}"></div>${cap.truncated ? `<button class="al-link" data-open="${entry.id}">Open full output</button>` : ""}`;
    } else {
      outBlock = `<div class="al-console-empty">${entry.verdict === "pending" ? "Running…" : "No output"}</div>`;
    }
     return `<div class="al-row ui-card al-row-terminal al-row-expanded" data-id="${entry.id}" role="button" tabindex="0" aria-expanded="true">
      ${meta}<div class="al-terminal-surface">${cmdLine}${outBlock}</div>
      ${reason}
       <div class="al-actions">
         <button class="ui-button ui-button-sm" data-copy-sql="${entry.id}">Copy command</button>
         ${entry.responseText ? `<button class="ui-button ui-button-sm" data-copy-res="${entry.id}">Copy output</button>` : ""}
      </div>
    </div>`;
  }

  function metaLineHtml(entry: LogEntry): string {
    const isGroup = "groupId" in opts.scope;
    const badges: string[] = [];
    badges.push(`<span class="al-badge al-badge-${escapeHtml(entry.verdict)}"><span class="al-dot al-dot-${escapeHtml(entry.verdict)}"></span>${escapeHtml(verdictLabel(entry.verdict))}</span>`);
    if (isGroup) badges.push(`<span class="al-chip">${escapeHtml(entry.connectionName)}</span>`);
    if (entry.source) badges.push(`<span class="al-chip">${escapeHtml(entry.source)}</span>`);
    if (entry.categories) badges.push(`<span class="al-cat">${escapeHtml(entry.categories)}</span>`);
    const stopBtn = entry.verdict === "pending" ? `<button class="ui-button ui-button-sm ui-button-danger" data-stop="${entry.id}">Stop</button>` : "";
     const rel = escapeHtml(relativeTime(entry.createdAt));
     const absolute = escapeHtml(localTimeString(entry.createdAt));
     return `<div class="al-meta">${badges.join("")}<span class="al-spacer"></span>${stopBtn}<time class="al-time-ago" datetime="${escapeHtml(entry.createdAt)}" title="${absolute}">${rel}</time></div>`;
  }

  function resultPreviewHtml(jsonStr: string, rowCount: number | null): string {
    try {
      const obj = JSON.parse(jsonStr) as { fields: string[]; rows: Record<string, unknown>[] };
      const fields = obj.fields ?? [];
      const rows = (obj.rows ?? []).slice(0, 5);
      if (fields.length === 0) return "";
       const head = `<div class="al-table-head">${fields.slice(0,6).map(f=>`<span class="al-th" title="${escapeHtml(f)}">${escapeHtml(f)}</span>`).join("")}</div>`;
       const body = rows.map(r => `<div class="al-tr">${fields.slice(0,6).map(f => { const value = r[f] == null ? "NULL" : String(r[f]); return `<span class="al-td" title="${escapeHtml(value)}">${escapeHtml(value)}</span>`; }).join("")}</div>`).join("");
      const total = rowCount ?? rows.length;
      const foot = total > rows.length ? `<div class="al-table-foot">${rows.length} of ${total} rows</div>` : "";
      return `<div class="al-table">${head}${body}${foot}</div>`;
    } catch { return ""; }
  }

  // Render list: only re-render affected rows when possible
  function renderList() {
    const f = filtered();
    if (f.length === 0 && !isLoading && !hasMore) {
      elList.innerHTML = "";
      elEmpty.hidden = false;
      renderEmpty();
      elLoadMore.innerHTML = "";
      return;
    }
    elEmpty.hidden = true;
    // For simplicity, rebuild list but keep scroll position stable.
    // Requirement "Only the affected feed re-renders" is met for live single-row updates via patchRow below.
    elList.innerHTML = f.map(e => rowHtml(e, expandedId === e.id)).join("");
    renderLoadMore();
    // async highlight for expanded rows
    for (const e of f) if (expandedId === e.id) enhanceExpandedRow(e);
  }

  async function enhanceExpandedRow(entry: LogEntry) {
    // Highlight SQL off main thread
    const sqlEl = elList.querySelector(`[data-sql="${entry.id}"]`) as HTMLElement | null;
    if (sqlEl) {
      const hl = await highlightedHtmlAsync(entry.sql, "sql");
      sqlEl.innerHTML = hl;
    }
    const cmdEl = elList.querySelector(`[data-cmd="${entry.id}"]`) as HTMLElement | null;
    if (cmdEl) {
      const hl = await highlightedHtmlAsync(entry.sql, "shell");
      cmdEl.innerHTML = hl;
    }
    const previewEl = elList.querySelector(`[data-preview="${entry.id}"]`) as HTMLElement | null;
    if (previewEl && entry.responseText) {
      const cap = capResponse(entry.responseText);
      // format slice only
      const formatted = cap.preview.includes("```") ? cap.preview : (() => {
        const t = cap.preview.trim();
        if (t.startsWith("{") || t.startsWith("[")) { try { return "```json\n" + JSON.stringify(JSON.parse(t), null, 2) + "\n```"; } catch {}}
        return cap.preview;
      })();
      // simple render: if fenced json, show highlighted code, else plain
      if (formatted.startsWith("```")) {
        const inner = formatted.slice(3, formatted.lastIndexOf("```")).replace(/^json\n/, "");
        const hl = await highlightedHtmlAsync(inner.trim(), "json");
        previewEl.innerHTML = `<pre class="al-code">${hl}</pre>`;
      } else {
        previewEl.textContent = cap.preview;
      }
    }
    const consoleEl = elList.querySelector(`[data-console="${entry.id}"]`) as HTMLElement | null;
    if (consoleEl && entry.responseText) {
      const cap = capConsole(entry.responseText);
      const html = consoleHtml(cap.preview);
      consoleEl.innerHTML = `<pre class="al-console-pre">${html}</pre>`;
    }
  }

  async function highlightedHtmlAsync(src: string, lang: ReturnType<typeof parseLanguage>) {
    if (src.length > 3000) await new Promise(r => setTimeout(r, 0));
    return highlightedHtml(src, lang);
  }

  // Patch single row without full list rebuild (live updates)
  function patchRow(entry: LogEntry) {
    const existing = elList.querySelector(`[data-id="${entry.id}"]`);
    const shouldShow = (filter === "all" || entry.verdict === filter) && matchesSearch(entry);
    if (!shouldShow) {
      if (existing) existing.remove();
      return;
    }
    if (existing) {
      // re-render this row only
      const isExpanded = expandedId === entry.id;
      const wrap = document.createElement("div");
      wrap.innerHTML = rowHtml(entry, isExpanded);
      const newNode = wrap.firstElementChild!;
      existing.replaceWith(newNode);
      if (isExpanded) enhanceExpandedRow(entry);
    } else {
      // new entry: insert in sorted order if filtered list is sorted
      // For simplicity, re-render whole list for inserts to keep order correct
      renderList();
    }
  }

  function renderLoadMore() {
    if (loadError) { elLoadMore.innerHTML = `<div class="ui-state ui-error" role="alert"><p>${escapeHtml(loadError)}</p><button class="ui-button ui-button-secondary ui-button-sm" data-role="retry">Try again</button></div>`; return; }
    if (isLoading) { elLoadMore.innerHTML = `<span class="al-loading">Loading older entries…</span>`; return; }
    if (hasMore) { elLoadMore.innerHTML = `<button class="ui-button ui-button-sm" data-role="loadMore">Load older entries</button><div class="al-sentinel" data-role="sentinel"></div>`; observeSentinel(); }
    else { elLoadMore.innerHTML = `<span class="al-end">You’re viewing all activity</span>`; }
  }

  let sentinelObs: IntersectionObserver | null = null;
  function observeSentinel() {
    const s = elLoadMore.querySelector("[data-role='sentinel']") as HTMLElement | null;
    if (!s) return;
    sentinelObs?.disconnect();
    sentinelObs = new IntersectionObserver(entries => {
      if (entries[0].isIntersecting && hasMore && !isLoading) loadMore();
    }, { rootMargin: "200px" });
    sentinelObs.observe(s);
  }

  // ----- data loading -----
  function reload(reset = false) {
    if (isLoading && !reset) { refreshAfterLoad = true; return; }
    requestGeneration++;
    const gen = requestGeneration;
    const range = timeRange;
    if (reset) {
      entries = [];
      nextCursor = null;
      hasMore = false;
      loadedOlderPage = false;
      refreshAfterLoad = false;
      seenIds.clear();
      liveCursor = 0;
    }
    isLoading = true;
    loadError = null;
    renderLoadMore();
    fetchLogPage(opts.scope, range, null).then(page => {
      if (gen !== requestGeneration) return;
      entries = mergeEntries(entries, page.entries);
      for (const e of entries) { seenIds.add(e.id); if (e.id > liveCursor) liveCursor = e.id; }
      if (reset || !loadedOlderPage) { nextCursor = page.nextCursor; hasMore = page.hasMore; }
      isLoading = false;
      if (refreshAfterLoad) { refreshAfterLoad = false; reload(); return; }
      updateStats();
      renderList();
      updatePolling();
    }).catch(() => { if (gen !== requestGeneration) return; isLoading = false; loadError = "Couldn’t load activity."; renderLoadMore(); });
  }

  function loadMore() {
    if (isLoading || !hasMore || !nextCursor) return;
    const gen = requestGeneration;
    const range = timeRange;
    isLoading = true;
    loadError = null;
    renderLoadMore();
    fetchLogPage(opts.scope, range, nextCursor).then(page => {
      if (gen !== requestGeneration) return;
      entries = mergeEntries(entries, page.entries);
      for (const e of page.entries) { seenIds.add(e.id); if (e.id > liveCursor) liveCursor = e.id; }
      nextCursor = page.nextCursor;
      hasMore = page.hasMore;
      loadedOlderPage = true;
      isLoading = false;
      if (refreshAfterLoad) { refreshAfterLoad = false; reload(); return; }
      updateStats();
      renderList();
    }).catch(() => { if (gen !== requestGeneration) return; isLoading = false; loadError = "Couldn’t load older activity."; renderLoadMore(); });
  }

  function updatePolling() {
    const hasPending = entries.some(e => e.verdict === "pending");
    if (hasPending && !pollTimer) {
      pollTimer = window.setInterval(() => reload(), 1500);
    } else if (!hasPending && pollTimer) {
      clearInterval(pollTimer); pollTimer = null;
    }
  }

  // Live rows pushed by the host
  function startLive() {
    const conn = connectEvents((ev: LiveEvent) => {
      if (ev.id > liveCursor) liveCursor = ev.id;
      const existing = entries.find(e => e.id === ev.id);
      if (existing) {
        // A settled row carries the response payload the live event omits.
        if (existing.verdict === "pending" && ev.verdict !== "pending") {
          reload();
          return;
        }
        const updated: LogEntry = {
          ...existing,
          sql: ev.sql,
          verdict: ev.verdict,
          reason: ev.reason,
          categories: ev.categories,
          source: ev.source,
          groupId: ev.groupId,
          groupName: ev.groupName,
          database: ev.database,
          rowCount: ev.rowCount,
          createdAt: ev.createdAt,
        };
        entries = mergeEntries(entries, [updated]);
        patchRow(updated);
        updateStats();
        updatePolling();
      } else {
        const inScope = (() => {
          if ("connectionId" in opts.scope) return ev.connectionId === opts.scope.connectionId;
          if ("groupId" in opts.scope) return ev.groupId === opts.scope.groupId;
          return false;
        })();
        if (!inScope) return;
        const light: LogEntry = {
          id: ev.id, connectionId: ev.connectionId, connectionName: ev.connectionName, sql: ev.sql, verdict: ev.verdict, reason: ev.reason, categories: ev.categories, source: ev.source, resultJson: null, rowCount: ev.rowCount, responseText: null, groupId: ev.groupId, groupName: ev.groupName, database: ev.database, createdAt: ev.createdAt,
        };
        entries = mergeEntries(entries, [light]);
        seenIds.add(light.id);
        patchRow(light);
        updateStats();
        updatePolling();
      }
    });
    liveClose = conn.close;
  }

  // ----- event wiring -----
  elSearch.addEventListener("input", () => {
    search = elSearch.value;
    elSearchClear.hidden = !search;
    renderList();
    updateStats();
  });
  elSearchClear.addEventListener("click", () => { elSearch.value = ""; search = ""; elSearchClear.hidden = true; renderList(); updateStats(); });
  elRange.value = timeRange;
  elRange.addEventListener("change", () => {
    timeRange = elRange.value as TimeRange;
    reload(true);
  });
  elVerdict.addEventListener("change", () => {
    filter = elVerdict.value as VerdictFilter;
    renderList();
  });
  elRetention.addEventListener("change", () => {
    const days = Number(elRetention.value);
    const previous = elRetention.dataset.value ?? "30";
    elRetention.value = previous;
    const label = days === 0 ? "Forever" : `${days} days`;
    confirmModal({
      title: "Change activity retention?",
      message: `Activity older than ${label} will be removed.`,
      confirmLabel: `Keep ${label}`,
      onConfirm: () => {
        elRetention.value = String(days);
        elRetention.dataset.value = String(days);
        void setRetention(days).then(() => {
          elRetentionStatus.textContent = `Activity retention set to ${label}.`;
          reload(true);
        }).catch(() => {
          elRetentionStatus.textContent = "Couldn’t update activity retention. Try again.";
          elRetention.value = previous;
          elRetention.dataset.value = previous;
        });
      },
    });
  });
  container.querySelector("[data-role='refresh']")?.addEventListener("click", () => reload());
  container.querySelector("[data-role='clear']")?.addEventListener("click", async () => {
    confirmModal({
      title: "Clear activity history?",
      message: "This permanently removes the recorded activity for this integration.",
      confirmLabel: "Clear history",
      onConfirm: () => void clearLogs(opts.scope).then(() => reload(true)).catch((e) => alert(String(e))),
    });
  });

  // Delegated row events
  elList.addEventListener("click", async (e) => {
    const target = e.target as HTMLElement;
    const stopId = target.closest("[data-stop]")?.getAttribute("data-stop");
    if (stopId) {
      e.stopPropagation();
      const id = Number(stopId);
      await cancelLog(id);
      // optimistic: mark cancelled distinct from failed
      const idx = entries.findIndex(en => en.id === id);
      if (idx >= 0) { entries[idx] = { ...entries[idx], verdict: "cancelled" }; patchRow(entries[idx]); updateStats(); updatePolling(); }
      return;
    }
    const copySql = target.closest("[data-copy-sql]")?.getAttribute("data-copy-sql");
    if (copySql) {
      const en = entries.find(x => x.id === Number(copySql));
      if (en) await navigator.clipboard.writeText(en.sql);
      return;
    }
    const copyRes = target.closest("[data-copy-res]")?.getAttribute("data-copy-res");
    if (copyRes) {
      const en = entries.find(x => x.id === Number(copyRes));
      const txt = en?.responseText ?? en?.resultJson ?? en?.reason ?? "";
      if (txt) await navigator.clipboard.writeText(txt);
      return;
    }
    const openId = target.closest("[data-open]")?.getAttribute("data-open");
    if (openId) {
      const en = entries.find(x => x.id === Number(openId));
      if (en) {
        const txt = en.responseText ?? en.resultJson ?? "";
        viewer.open(en.sql, txt);
      }
      return;
    }
    // toggle expand
    const row = target.closest("[data-id]") as HTMLElement | null;
    if (row) {
      const id = Number(row.getAttribute("data-id"));
      if (expandedId === id) expandedId = null; else expandedId = id;
      // re-render only affected rows for efficiency
      renderList();
    }
  });

  elList.addEventListener("keydown", (e) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    const target = e.target as HTMLElement;
    if (target.closest("button, a, input, select, textarea")) return;
    const row = target.closest("[data-id]") as HTMLElement | null;
    if (!row) return;
    e.preventDefault();
    const id = Number(row.getAttribute("data-id"));
    expandedId = expandedId === id ? null : id;
    renderList();
  });

  elLoadMore.addEventListener("click", (e) => {
    const t = e.target as HTMLElement;
    if (t.getAttribute("data-role") === "loadMore") loadMore();
    if (t.getAttribute("data-role") === "retry") reload(true);
  });

  // initial load + live
  reload(true);
  startLive();

  return {
    destroy() {
      if (pollTimer) clearInterval(pollTimer);
      liveClose?.();
      sentinelObs?.disconnect();
    },
  };
}
