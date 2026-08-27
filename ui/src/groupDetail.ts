/**
 * Group detail screen — header, tabs (Logs/Overview), endpoint card,
 * client config, member list with slug-derived tool prefix.
 */

import { slug, slugsWithCollision } from "./slug";
import { mcpUrl } from "./integration-detail/logic";
import type { Integration as DetailIntegration } from "./integration-detail/types";
import { mountActivityLog } from "./activityLog/activityLog";

export type Group = {
  id: string;
  name: string;
  environment?: string | null;
  token: string;
  members: Array<{ id: string; overrides: Record<string, string> }>;
};

export type GroupDetailDeps = {
  group: Group;
  integrations: DetailIntegration[];
  adapters?: Array<{ id: string; label: string }>;
  onEdit: () => void;
  onDelete: () => void;
  onEditIntegration: (id: string) => void;
  toastCenter?: { present: (t: { integrationId: string; title: string; message: string; kind: "error" | "success" }) => void };
};

function envLabel(env: string | null | undefined): string {
  if (!env) return "Any";
  return env.charAt(0).toUpperCase() + env.slice(1);
}

export function renderGroupDetail(container: HTMLElement, deps: GroupDetailDeps): void {
  const { group, integrations, onEdit, onDelete, onEditIntegration } = deps;
  container.innerHTML = "";
  container.className = "group-detail";
  container.setAttribute("role", "main");
  container.setAttribute("aria-label", `Group ${group.name}`);

  // Header
  const header = document.createElement("div");
  header.className = "detail-header";
  header.style.padding = "16px 24px 24px";

  const top = document.createElement("div");
  top.className = "detail-header-top";

  const icon = document.createElement("div");
  icon.textContent = "▦";
  icon.setAttribute("aria-hidden", "true");
  icon.className = "type-badge";

  const headerMain = document.createElement("div");
  headerMain.className = "detail-header-stack";

  const titleRow = document.createElement("div");
  titleRow.className = "detail-title-row";

  const title = document.createElement("h1");
  title.textContent = group.name;
  title.className = "detail-title";
  title.id = "group-title";
  title.tabIndex = 0;

  const editBtn = document.createElement("button");
  editBtn.textContent = "Edit";
  editBtn.className = "btn btn-sm";
  editBtn.setAttribute("aria-label", `Edit group ${group.name}`);
  editBtn.addEventListener("click", onEdit);

  const menuBtn = document.createElement("button");
  menuBtn.textContent = "⋯";
  menuBtn.className = "btn btn-sm";
  menuBtn.setAttribute("aria-label", "More actions");
  menuBtn.setAttribute("aria-haspopup", "menu");
  menuBtn.addEventListener("click", () => {
    const menu = document.createElement("div");
    menu.setAttribute("role", "menu");
    menu.style.position = "absolute";
    menu.style.background = "var(--surface-panel)";
    menu.style.border = "1px solid rgba(0,0,0,0.08)";
    menu.style.borderRadius = "6px";
    menu.style.padding = "4px";
    menu.style.zIndex = "20";
    const del = document.createElement("button");
    del.textContent = "Delete…";
    del.setAttribute("role", "menuitem");
    del.style.color = "#dc2626";
    del.addEventListener("click", () => {
      menu.remove();
      onDelete();
    });
    menu.appendChild(del);
    menuBtn.parentElement!.appendChild(menu);
    const close = (e: MouseEvent) => {
      if (!menu.contains(e.target as Node) && e.target !== menuBtn) {
        menu.remove();
        window.removeEventListener("click", close);
      }
    };
    setTimeout(() => window.addEventListener("click", close), 0);
  });

  titleRow.append(title, editBtn, menuBtn);

  const subtitle = document.createElement("div");
  subtitle.className = "detail-meta";
  const membersForSubtitle = memberIntegrations(group, integrations);
  const countLabel = `${membersForSubtitle.length} integration${membersForSubtitle.length === 1 ? "" : "s"}`;
  subtitle.textContent = group.environment ? `Group · ${countLabel} · ${envLabel(group.environment)}` : `Group · ${countLabel}`;

  headerMain.append(titleRow, subtitle);
  top.append(icon, headerMain);
  header.appendChild(top);

  // Tabs
  const tabBar = document.createElement("div");
  tabBar.className = "tab-bar";
  tabBar.style.display = "flex";
  tabBar.style.gap = "16px";
  tabBar.style.padding = "8px 24px 16px";
  tabBar.setAttribute("role", "tablist");
  tabBar.setAttribute("aria-label", "Group sections");

  const logsBtn = document.createElement("button");
  logsBtn.textContent = "Logs";
  logsBtn.setAttribute("role", "tab");
  logsBtn.setAttribute("aria-selected", "true");
  logsBtn.className = "tab active";
  logsBtn.id = "tab-logs";

  const overviewBtn = document.createElement("button");
  overviewBtn.textContent = "Overview";
  overviewBtn.setAttribute("role", "tab");
  overviewBtn.setAttribute("aria-selected", "false");
  overviewBtn.className = "tab";
  overviewBtn.id = "tab-overview";

  tabBar.append(logsBtn, overviewBtn);

  const content = document.createElement("div");
  content.className = "group-content";
  content.style.padding = "16px 24px";

  function showLogs(): void {
    logsBtn.classList.add("active");
    overviewBtn.classList.remove("active");
    logsBtn.setAttribute("aria-selected", "true");
    overviewBtn.setAttribute("aria-selected", "false");
    content.innerHTML = "";
    // Reuse R21's activity log view scoped to group
    const logMount = document.createElement("div");
    logMount.setAttribute("role", "tabpanel");
    logMount.setAttribute("aria-labelledby", "tab-logs");
    content.appendChild(logMount);
    try {
      mountActivityLog(logMount, { scope: { groupId: group.id } });
    } catch {
      logMount.textContent = "Logs unavailable.";
      logMount.setAttribute("role", "status");
    }
  }

  function showOverview(): void {
    overviewBtn.classList.add("active");
    logsBtn.classList.remove("active");
    overviewBtn.setAttribute("aria-selected", "true");
    logsBtn.setAttribute("aria-selected", "false");
    content.innerHTML = "";
    const wrap = document.createElement("div");
    wrap.style.display = "flex";
    wrap.style.flexDirection = "column";
    wrap.style.gap = "24px";
    wrap.setAttribute("role", "tabpanel");
    wrap.setAttribute("aria-labelledby", "tab-overview");

    // Endpoint card
    const endpoint = document.createElement("section");
    endpoint.className = "card";
    endpoint.setAttribute("aria-label", "MCP endpoint");
    const epTitle = document.createElement("h2");
    epTitle.className = "card-title";
    epTitle.textContent = "MCP endpoint";
    const urlRow = document.createElement("div");
    urlRow.style.display = "flex";
    urlRow.style.gap = "8px";
    urlRow.style.alignItems = "center";
    const urlText = document.createElement("code");
    urlText.textContent = mcpUrl(group.token);
    urlText.className = "mono";
    urlText.style.flex = "1";
    urlText.style.overflow = "hidden";
    urlText.style.textOverflow = "ellipsis";
    urlText.style.whiteSpace = "nowrap";
    const copyBtn = document.createElement("button");
    copyBtn.textContent = "Copy";
    copyBtn.className = "btn btn-primary btn-sm";
    copyBtn.setAttribute("aria-label", "Copy endpoint URL");
    let copyTimer: ReturnType<typeof setTimeout> | null = null;
    copyBtn.addEventListener("click", async () => {
      const url = mcpUrl(group.token);
      try {
        await navigator.clipboard.writeText(url);
      } catch {
        const ta = document.createElement("textarea");
        ta.value = url;
        document.body.appendChild(ta);
        ta.select();
        document.execCommand("copy");
        ta.remove();
      }
      copyBtn.textContent = "Copied!";
      if (copyTimer) clearTimeout(copyTimer);
      copyTimer = setTimeout(() => (copyBtn.textContent = "Copy"), 1500);
    });
    // Respect reduced motion for feedback animation
    const reduce = typeof window !== "undefined" && window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    copyBtn.addEventListener("click", () => {
      if (!reduce) {
        copyBtn.style.transition = "transform 150ms ease";
        copyBtn.style.transform = "scale(0.96)";
        setTimeout(() => (copyBtn.style.transform = ""), 150);
      }
    });
    urlRow.append(urlText, copyBtn);
    endpoint.append(epTitle, urlRow);

    // Client config reuse (group-level: R19)
    const configSection = document.createElement("section");
    configSection.className = "card";
    configSection.setAttribute("aria-label", "Agent setup");
    const cfgTitle = document.createElement("h2");
    cfgTitle.className = "card-title";
    cfgTitle.textContent = "Agent setup";
    const cfgHint = document.createElement("p");
    cfgHint.className = "hint";
    cfgHint.textContent = `Endpoint key: ${group.id}`;
    const cfgUrl = document.createElement("code");
    cfgUrl.className = "mono";
    cfgUrl.textContent = mcpUrl(group.token);
    cfgUrl.style.display = "block";
    cfgUrl.style.marginTop = "8px";
    cfgUrl.style.wordBreak = "break-all";
    configSection.append(cfgTitle, cfgHint, cfgUrl);

    // Member list
    const membersSection = document.createElement("section");
    membersSection.className = "card";
    membersSection.setAttribute("aria-label", "Integrations in this group");
    const mTitle = document.createElement("h2");
    mTitle.className = "card-title";
    mTitle.textContent = "Integrations";
    membersSection.appendChild(mTitle);

    const members = memberIntegrations(group, integrations);
    if (members.length === 0) {
      const empty = document.createElement("p");
      empty.className = "hint";
      empty.textContent = "No integrations in this group. Choose Edit to add some.";
      empty.setAttribute("role", "status");
      membersSection.appendChild(empty);
    } else {
      const list = document.createElement("div");
      list.setAttribute("role", "list");
      // Collision-aware prefixes
      const names = members.map((m) => m.name);
      const slugs = slugsWithCollision(names);
      members.forEach((conn, idx) => {
        const row = document.createElement("button");
        row.className = "member-row";
        row.setAttribute("role", "listitem");
        row.setAttribute("aria-label", `${conn.name}, tools under ${slugs[idx]}__`);
        row.style.display = "flex";
        row.style.width = "100%";
        row.style.alignItems = "flex-start";
        row.style.gap = "12px";
        row.style.padding = "8px 12px";
        row.style.border = "none";
        row.style.background = "transparent";
        row.style.cursor = "pointer";
        row.style.textAlign = "left";
        row.tabIndex = 0;

        const badge = document.createElement("span");
        badge.className = "type-badge";
        badge.textContent = conn.type.slice(0, 2).toUpperCase();
        badge.setAttribute("aria-hidden", "true");

        const info = document.createElement("div");
        info.style.flex = "1";
        info.style.minWidth = "0";
        const nameRow = document.createElement("div");
        nameRow.style.display = "flex";
        nameRow.style.gap = "8px";
        nameRow.style.alignItems = "center";
        const nameEl = document.createElement("span");
        nameEl.textContent = conn.name;
        nameEl.style.fontWeight = "500";
        const envTag = document.createElement("span");
        envTag.textContent = envLabel(conn.environment as string);
        envTag.className = "env-tag";
        envTag.style.fontSize = "11px";
        envTag.style.padding = "2px 6px";
        envTag.style.borderRadius = "4px";
        envTag.style.background = "rgba(0,0,0,0.06)";
        nameRow.append(nameEl, envTag);

        info.appendChild(nameRow);

        const overrides = group.members.find((m) => m.id === conn.id)?.overrides ?? {};
        const overrideKeys = Object.keys(overrides);
        if (overrideKeys.length) {
          const ov = document.createElement("div");
          ov.className = "mono";
          ov.style.fontSize = "11px";
          ov.style.color = "var(--surface-tertiary-label)";
          ov.textContent = overrideKeys
            .sort()
            .map((k) => `${k} → ${overrides[k]}`)
            .join("   ");
          info.appendChild(ov);
        }

        const prefix = document.createElement("code");
        prefix.className = "mono";
        prefix.style.fontSize = "11px";
        prefix.style.color = "var(--surface-tertiary-label)";
        prefix.textContent = `${slugs[idx]}__*`;
        prefix.setAttribute("aria-label", `Tool prefix ${slugs[idx]}__`);

        const chev = document.createElement("span");
        chev.textContent = "›";
        chev.setAttribute("aria-hidden", "true");
        chev.style.color = "var(--surface-tertiary-label)";

        row.append(badge, info, prefix, chev);
        row.addEventListener("click", () => onEditIntegration(conn.id));
        row.addEventListener("keydown", (e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onEditIntegration(conn.id);
          }
        });
        list.appendChild(row);
      });
      membersSection.appendChild(list);
    }

    wrap.append(endpoint, configSection, membersSection);
    content.appendChild(wrap);
  }

  logsBtn.addEventListener("click", showLogs);
  logsBtn.addEventListener("keydown", (e) => {
    if (e.key === "ArrowRight") {
      e.preventDefault();
      overviewBtn.focus();
      showOverview();
    }
  });
  overviewBtn.addEventListener("click", showOverview);
  overviewBtn.addEventListener("keydown", (e) => {
    if (e.key === "ArrowLeft") {
      e.preventDefault();
      logsBtn.focus();
      showLogs();
    }
  });

  // Keyboard shortcuts
  container.addEventListener("keydown", (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "e") {
      e.preventDefault();
      onEdit();
    }
  });

  container.append(header, tabBar, content);
  showLogs();

  // Focus order: title -> tabs -> content
  title.tabIndex = 0;
  logsBtn.tabIndex = 0;
  overviewBtn.tabIndex = 0;
}

function memberIntegrations(group: Group, integrations: DetailIntegration[]): DetailIntegration[] {
  return group.members
    .map((m) => integrations.find((c) => c.id === m.id))
    .filter((c): c is DetailIntegration => !!c);
}

export { slug };
