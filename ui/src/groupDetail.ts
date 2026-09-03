/**
 * Group detail screen — header, tabs (Logs/Overview), MCP endpoint and agent
 * install, member list with slug-derived tool prefix.
 */

import { slug, slugsWithCollision } from "./slug";
import { mcpKey, mcpUrl } from "./integration-detail/logic";
import { renderMcpSection, type InjectFn } from "./integration-detail/mcp-section";
import type { Integration as DetailIntegration } from "./integration-detail/types";
import { mountActivityLog } from "./activityLog/activityLog";
import { createIcon } from "./icon";
import { createButton, createBadge, openMenu } from "./primitives";
import { glyphElement } from "./glyph";
import { renderTabList } from "./integration-detail/tabs";

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
  inject: InjectFn;
};

function envLabel(env: string | null | undefined): string {
  if (!env) return "Any";
  return env.charAt(0).toUpperCase() + env.slice(1);
}

export function renderGroupDetail(container: HTMLElement, deps: GroupDetailDeps): void {
  const { group, integrations, onEdit, onDelete, onEditIntegration, inject } = deps;
  container.innerHTML = "";
  container.className = "group-detail";
  container.setAttribute("role", "main");
  container.setAttribute("aria-label", `Group ${group.name}`);

  // Header
  const header = document.createElement("div");
  header.className = "detail-header";

  const top = document.createElement("div");
  top.className = "detail-header-top";

  const icon = document.createElement("div");
  icon.appendChild(createIcon("group", { size: 20 }));
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

  const editBtn = createButton("Edit", { size: "sm", ariaLabel: `Edit group ${group.name}`, onClick: onEdit });

  const menuBtn = createButton("", { icon: "more", ariaLabel: "More actions" });
  menuBtn.classList.add("icon-button");
  menuBtn.setAttribute("aria-haspopup", "menu");
  menuBtn.addEventListener("click", () => openMenu(menuBtn, [{ label: "Delete…", icon: "trash", danger: true, onSelect: onDelete }]));

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
  tabBar.setAttribute("aria-label", "Group sections");
  let selectedTab: "logs" | "overview" = "logs";

  const content = document.createElement("div");
  content.className = "group-content";

  function showLogs(): void {
    selectedTab = "logs";
    renderTabs();
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
    selectedTab = "overview";
    renderTabs();
    content.innerHTML = "";
    const wrap = document.createElement("div");
    wrap.className = "stack-lg";
    wrap.setAttribute("role", "tabpanel");
    wrap.setAttribute("aria-labelledby", "tab-overview");

    // A group has one endpoint that fans out to every member, so one server
    // entry — the same section an integration shows.
    const mcp = document.createElement("section");
    renderMcpSection(
      mcp,
      { key: mcpKey(group.name, group.environment), url: mcpUrl(group.token) },
      inject,
    );

    // Member list
    const membersSection = document.createElement("section");
    membersSection.className = "ui-card";
    membersSection.setAttribute("aria-label", "Integrations in this group");
    const mTitle = document.createElement("h2");
    mTitle.className = "ui-card-title";
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
      list.className = "member-list";
      list.setAttribute("role", "list");
      // Collision-aware prefixes
      const names = members.map((m) => m.name);
      const slugs = slugsWithCollision(names);
      members.forEach((conn, idx) => {
        const row = document.createElement("button");
        row.className = "member-row";
        row.setAttribute("aria-label", `${conn.name}, tools under ${slugs[idx]}__`);

        const badge = glyphElement(conn.type, 16);
        badge.classList.add("member-glyph");
        badge.setAttribute("aria-hidden", "true");

        const info = document.createElement("div");
        info.className = "member-info";
        const nameRow = document.createElement("div");
        nameRow.className = "member-name-row";
        const nameEl = document.createElement("span");
        nameEl.textContent = conn.name;
        nameEl.className = "member-name";
        const envTag = createBadge(envLabel(conn.environment as string), "environment");
        nameRow.append(nameEl, envTag);

        info.appendChild(nameRow);

        const overrides = group.members.find((m) => m.id === conn.id)?.overrides ?? {};
        const overrideKeys = Object.keys(overrides);
        if (overrideKeys.length) {
          const ov = document.createElement("div");
          ov.className = "mono member-overrides";
          ov.textContent = overrideKeys
            .sort()
            .map((k) => `${k} → ${overrides[k]}`)
            .join("   ");
          info.appendChild(ov);
        }

        const prefix = document.createElement("code");
        prefix.className = "mono member-prefix";
        prefix.textContent = `${slugs[idx]}__*`;
        prefix.setAttribute("aria-label", `Tool prefix ${slugs[idx]}__`);

        const chev = document.createElement("span");
        chev.className = "member-chevron";
        chev.appendChild(createIcon("chevron-right"));
        chev.setAttribute("aria-hidden", "true");

        row.append(badge, info, prefix, chev);
        row.addEventListener("click", () => onEditIntegration(conn.id));

        const item = document.createElement("div");
        item.className = "member-item";
        item.setAttribute("role", "listitem");
        item.appendChild(row);
        list.appendChild(item);
      });
      membersSection.appendChild(list);
    }

    wrap.append(mcp, membersSection);
    content.appendChild(wrap);
  }

  function renderTabs(): void {
    renderTabList(tabBar, [{ id: "logs", label: "Logs" }, { id: "overview", label: "Overview" }], selectedTab, (id) => {
      if (id === "logs") showLogs();
      else showOverview();
    });
    tabBar.setAttribute("aria-label", "Group sections");
  }

  // Keyboard shortcuts
  container.addEventListener("keydown", (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "e") {
      e.preventDefault();
      onEdit();
    }
  });

  container.append(header, tabBar, content);
  showLogs();

  // Keep the active tab in the keyboard tab order.
  title.tabIndex = 0;
}

function memberIntegrations(group: Group, integrations: DetailIntegration[]): DetailIntegration[] {
  return group.members
    .map((m) => integrations.find((c) => c.id === m.id))
    .filter((c): c is DetailIntegration => !!c);
}

export { slug };
