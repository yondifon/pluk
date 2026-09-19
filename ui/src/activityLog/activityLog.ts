/**
 * Activity Log, vanilla TS view. Mirrors ConnectionLogView.swift (1,183 lines).
 * Covers: paging, merging, generation counter, toolbar, live SSE, pending poll, caps,
 * two row shapes, syntax highlighting off main thread, UTC parsing, response viewer.
 */

import type { LogEntry, LogCursor, TimeRange, VerdictFilter } from "./types";
import { verdictFilters, verdictFilterCounts, verdictFilterLabel, verdictLabel } from "./types";
import { fetchLogPage, mergeEntries, cancelLog, getRetention, setRetention, clearLogs, connectEvents, type LogScope, type LiveEvent } from "./api";
import { relativeTime, localTimeString } from "./time";
import { highlightedHtml, consoleHtml, parseLanguage, escapeHtml } from "./highlight";
import { capResponse } from "./caps";
import { createResponseViewer } from "./responseViewer";
import { createIcon } from "../icon";
import { confirmModal } from "../modal";
import { toast } from "../toast";
import { ENTRY_RENDERERS, entryCategory, entryType, responseTextForCopy, type EntryType } from "./renderers";

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
  let searchTimer: number | null = null;

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
        <button class="ui-button" data-role="refresh">Refresh</button>
        <span class="al-toolbar-divider" aria-hidden="true"></span>
        <button class="ui-button al-clear" data-role="clear">Clear history</button>
      </div>
    </div>
    <div class="al-filters" role="group" aria-label="Filter by result" data-role="filters"></div>
    <div class="al-retention-status sr-only" data-role="retention-status" role="status" aria-live="polite" aria-atomic="true"></div>
    <div class="al-list" data-role="list"></div>
    <div class="al-load-more" data-role="loadMore"></div>
    <div class="al-empty" data-role="empty" hidden></div>
  `;

  const elSearch = container.querySelector(".al-search-input") as HTMLInputElement;
  const elSearchClear = container.querySelector(".al-search-clear") as HTMLButtonElement;
  const elRange = container.querySelector("[data-role='range']") as HTMLSelectElement;
  const elRetention = container.querySelector("[data-role='retention']") as HTMLSelectElement;
  const elFilters = container.querySelector("[data-role='filters']") as HTMLElement;
  const elRetentionStatus = container.querySelector("[data-role='retention-status']") as HTMLElement;
  const elList = container.querySelector("[data-role='list']") as HTMLElement;
  const elLoadMore = container.querySelector("[data-role='loadMore']") as HTMLElement;
  const elEmpty = container.querySelector("[data-role='empty']") as HTMLElement;
  container.querySelector(".al-search-icon")?.appendChild(createIcon("search"));
  container.querySelector(".al-search-clear")?.appendChild(createIcon("close"));
  container.querySelector("[data-role='refresh']")?.prepend(createIcon("refresh"));
  elFilters.innerHTML = verdictFilters
    .map(value => `<button type="button" class="al-filter" data-verdict="${value}" aria-pressed="false">${escapeHtml(verdictFilterLabel(value))} <span class="al-filter-count">0</span></button>`)
    .join("");

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

  function updateStats() {
    const counts = verdictFilterCounts(entries);
    for (const chip of Array.from(elFilters.querySelectorAll<HTMLButtonElement>(".al-filter"))) {
      const value = chip.dataset.verdict as VerdictFilter;
      chip.setAttribute("aria-pressed", String(value === filter));
      const count = chip.querySelector(".al-filter-count");
      if (count) count.textContent = String(counts[value]);
    }
  }

  function renderEmpty() {
    const q = search.trim();
    const f = filter;
    let title = "";
    let subtitle = "";
    if (q) { title = "No matches"; subtitle = `No entries match “${q}”.`; }
    else if (f !== "all") { title = `No ${verdictFilterLabel(f).toLowerCase()} activity`; subtitle = "Try a different filter."; }
    else if (timeRange !== "all") { title = "No activity in this range"; subtitle = "Try a wider time range."; }
    else { title = "No activity yet"; subtitle = "Activity from agents using this endpoint will appear here."; }
    elEmpty.innerHTML = `<div class="al-empty-icon"></div><div class="al-empty-title">${escapeHtml(title)}</div><div class="al-empty-sub">${escapeHtml(subtitle)}</div>`;
    elEmpty.querySelector(".al-empty-icon")?.appendChild(createIcon("tray", { size: 24 }));
  }

  function rowHtml(entry: LogEntry, isExpanded: boolean): string {
    const connectionType = typeMap.get(entry.connectionId);
    const detailId = `al-detail-${entry.id}`;
    const detail = isExpanded ? `<div id="${detailId}" class="al-expanded" role="region">${ENTRY_RENDERERS[entryType(entry, connectionType)](entry)}</div>` : "";
    return `<div class="al-row${isExpanded ? " al-row-expanded" : ""}" data-id="${entry.id}" data-verdict="${escapeHtml(entry.verdict)}" role="button" tabindex="0" aria-expanded="${isExpanded}" aria-controls="${detailId}">${metaLineHtml(entry, entryCategory(entry, connectionType))}<div class="al-summary" title="${escapeHtml(entry.sql)}">${escapeHtml(entry.sql)}</div>${detail}</div>`;
  }

  function metaLineHtml(entry: LogEntry, category: EntryType): string {
    // A successful call says so only to assistive tech; the left rule carries it on screen.
    const statusClass = entry.verdict === "allowed" ? "sr-only" : "al-status";
    const status = `<span class="${statusClass}">${escapeHtml(verdictLabel(entry.verdict))}</span>`;
    const facets = entry.source ? `${entry.source} · ${category}` : category;
    const stopBtn = entry.verdict === "pending" ? `<button class="ui-button ui-button-sm ui-button-danger" data-stop="${entry.id}">Stop</button>` : "";
    const rel = escapeHtml(relativeTime(entry.createdAt));
    const absolute = escapeHtml(localTimeString(entry.createdAt));
    return `<div class="al-meta">${status}<span class="al-name">${escapeHtml(entry.connectionName)}</span><span class="al-facets">${escapeHtml(facets)}</span>${stopBtn}<time class="al-time-ago" datetime="${escapeHtml(entry.createdAt)}" title="${absolute}">${rel}</time></div>`;
  }

  const renderedRows = new Map<number, { node: HTMLElement; entry: LogEntry; expanded: boolean }>();

  function renderList() {
    const f = filtered();
    const visible = new Set(f.map(entry => entry.id));
    for (const [id, row] of renderedRows) {
      if (!visible.has(id)) {
        row.node.remove();
        renderedRows.delete(id);
      }
    }
    if (f.length === 0 && !isLoading && !hasMore) {
      elEmpty.hidden = false;
      renderEmpty();
      elLoadMore.innerHTML = "";
      return;
    }
    elEmpty.hidden = true;
    let next = elList.firstElementChild;
    for (const entry of f) {
      const isExpanded = expandedId === entry.id;
      let row = renderedRows.get(entry.id);
      const previous = row?.entry;
      const keys = Object.keys(entry) as (keyof LogEntry)[];
      if (!row || row.expanded !== isExpanded || !previous || keys.some(key => entry[key] !== previous[key])) {
        const wrap = document.createElement("div");
        wrap.innerHTML = rowHtml(entry, isExpanded);
        const node = wrap.firstElementChild as HTMLElement;
        for (const button of node.querySelectorAll(".al-copy-block")) {
          button.appendChild(createIcon("copy", { size: 14 }));
        }
        if (row) {
          const focused = document.activeElement === row.node;
          if (next === row.node) next = node;
          row.node.replaceWith(node);
          if (focused) node.focus({ preventScroll: true });
        }
        row = { node, entry, expanded: isExpanded };
        renderedRows.set(entry.id, row);
        if (isExpanded) void enhanceExpandedRow(entry, node);
      }
      if (row.node !== next) elList.insertBefore(row.node, next);
      next = row.node.nextElementSibling;
      const time = row.node.querySelector("time");
      const relative = relativeTime(entry.createdAt);
      if (time && time.textContent !== relative) time.textContent = relative;
    }
    renderLoadMore();
  }

  async function enhanceExpandedRow(entry: LogEntry, node: HTMLElement) {
    const sqlEl = node.querySelector(`[data-sql="${entry.id}"]`) as HTMLElement | null;
    if (sqlEl) {
      const hl = await highlightedHtmlAsync(entry.sql, "sql");
      sqlEl.innerHTML = hl;
    }
    const cmdEl = node.querySelector(`[data-cmd="${entry.id}"]`) as HTMLElement | null;
    if (cmdEl) {
      const hl = await highlightedHtmlAsync(entry.sql, "shell");
      cmdEl.innerHTML = hl;
    }
    const previewEl = null as HTMLElement | null;
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
    const consoleEl = node.querySelector(`[data-console="${entry.id}"]`) as HTMLElement | null;
    if (consoleEl && entry.responseText) {
      consoleEl.innerHTML = consoleHtml(entry.responseText);
    }
  }

  async function highlightedHtmlAsync(src: string, lang: ReturnType<typeof parseLanguage>) {
    if (src.length > 3000) await new Promise(r => setTimeout(r, 0));
    return highlightedHtml(src, lang);
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
        renderList();
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
        renderList();
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
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = window.setTimeout(() => {
      searchTimer = null;
      renderList();
    }, 150);
  });
  elSearchClear.addEventListener("click", () => {
    elSearch.value = "";
    search = "";
    elSearchClear.hidden = true;
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = null;
    renderList();
  });
  elRange.value = timeRange;
  elRange.addEventListener("change", () => {
    timeRange = elRange.value as TimeRange;
    reload(true);
  });
  elFilters.addEventListener("click", (e) => {
    const chip = (e.target as HTMLElement).closest<HTMLButtonElement>(".al-filter");
    if (!chip) return;
    filter = chip.dataset.verdict as VerdictFilter;
    updateStats();
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
      onConfirm: () =>
        void clearLogs(opts.scope)
          .then(() => reload(true))
          .catch((e) => toast.error("History not cleared", { description: String(e) })),
    });
  });

  // Delegated row events
  elList.addEventListener("click", async (e) => {
    const target = e.target as HTMLElement;
    const copyBlock = target.closest<HTMLButtonElement>("[data-copy-block]");
    if (copyBlock) {
      const row = copyBlock.closest<HTMLElement>("[data-id]");
      const entry = row ? entries.find(item => item.id === Number(row.dataset.id)) : undefined;
      if (entry) {
        const isRequest = copyBlock.dataset.copyBlock === "request";
        await navigator.clipboard.writeText(isRequest ? entry.sql : responseTextForCopy(entry));
        toast.success(isRequest ? "Request copied" : "Response copied");
      }
      return;
    }
    if (target.closest(".al-expanded")) return;
    const stopId = target.closest("[data-stop]")?.getAttribute("data-stop");
    if (stopId) {
      e.stopPropagation();
      const id = Number(stopId);
      await cancelLog(id);
      // optimistic: mark cancelled distinct from failed
      const idx = entries.findIndex(en => en.id === id);
      if (idx >= 0) { entries[idx] = { ...entries[idx], verdict: "cancelled" }; renderList(); updateStats(); updatePolling(); }
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
    if (target.closest(".al-expanded")) return;
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
  updateStats();
  reload(true);
  startLive();

  return {
    destroy() {
      if (pollTimer) clearInterval(pollTimer);
      if (searchTimer) clearTimeout(searchTimer);
      liveClose?.();
      sentinelObs?.disconnect();
    },
  };
}
