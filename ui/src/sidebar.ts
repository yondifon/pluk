import "./sidebar.css";
import type { Group, Integration, AdapterManifest, Environment, Health } from "./types";
import { envLabel } from "./types";
import { adapterColor, glyphElement } from "./glyph";
import {
  filteredGroups,
  filteredIntegrations,
  availableTypesSorted,
  availableEnvs,
} from "./filter";

export type SidebarState = {
  integrations: Integration[];
  groups: Group[];
  adapters: AdapterManifest[];
  health: Record<string, Health>;
  adaptersLoadFailed: boolean;
  loading: boolean;
};

export type SidebarCallbacks = {
  onSelect: (id: string) => void;
  onCreateIntegration: () => void;
  onCreateGroup: () => void;
  onDuplicate: (id: string) => void;
  onDelete: (kind: "integration" | "group", id: string, name: string) => void;
  onRetryAdapters: () => void;
};

export function createSidebar(
  state: SidebarState,
  selectedId: string | null,
  cbs: SidebarCallbacks,
): HTMLElement {
  const root = document.createElement("div");
  root.style.display = "flex";
  root.style.flexDirection = "column";
  root.style.height = "100%";
  root.style.minHeight = "0";

  // Top bar
  const toolbar = document.createElement("div");
  toolbar.className = "sidebar-topbar";
  const btnNewInt = document.createElement("button");
  btnNewInt.textContent = "+";
  btnNewInt.title = "New Integration (⌘N)";
  btnNewInt.setAttribute("aria-label", "New Integration");
  btnNewInt.onclick = cbs.onCreateIntegration;
  const btnNewGroup = document.createElement("button");
  btnNewGroup.textContent = "⊞";
  btnNewGroup.title = "New Group (⇧⌘N)";
  btnNewGroup.setAttribute("aria-label", "New Group");
  btnNewGroup.onclick = cbs.onCreateGroup;
  toolbar.append(btnNewInt, btnNewGroup);

  // Search row
  const searchRow = document.createElement("div");
  searchRow.className = "sidebar-search-row";

  const searchWrap = document.createElement("div");
  searchWrap.className = "sidebar-search";
  const searchIcon = document.createElement("span");
  searchIcon.textContent = "⌕";
  searchIcon.style.color = "var(--surface-tertiary-label)";
  const input = document.createElement("input");
  input.placeholder = "Filter integrations";
  input.setAttribute("aria-label", "Filter integrations");
  input.id = "sidebar-search";
  const clearBtn = document.createElement("button");
  clearBtn.textContent = "×";
  clearBtn.title = "Clear";
  clearBtn.style.display = "none";
  clearBtn.style.border = "none";
  clearBtn.style.background = "transparent";
  clearBtn.style.cursor = "pointer";
  searchWrap.append(searchIcon, input, clearBtn);

  const filterBtn = document.createElement("button");
  filterBtn.className = "sidebar-filter-btn";
  filterBtn.textContent = "☰";
  filterBtn.title = "Filter by type and environment";
  filterBtn.setAttribute("aria-label", "Filter by type and environment");

  let popover: HTMLElement | null = null;
  let query = "";
  let typeFilter = new Set<string>();
  let envFilter = new Set<Environment>();

  function updateFilterBtn() {
    const active = typeFilter.size > 0 || envFilter.size > 0;
    filterBtn.classList.toggle("active", active);
  }

  function showPopover() {
    if (popover) {
      popover.remove();
      popover = null;
      return;
    }
    const types = availableTypesSorted(state.integrations, state.adapters);
    const envs = availableEnvs(state.integrations, state.groups);
    const el = document.createElement("div");
    el.className = "popover";
    el.style.position = "absolute";
    // simple inline popover under filterBtn via container relative
    const header = document.createElement("div");
    header.style.display = "flex";
    header.style.justifyContent = "space-between";
    header.style.padding = "8px 12px";
    header.innerHTML = `<strong>Filters</strong>`;
    const clear = document.createElement("button");
    clear.textContent = "Clear";
    clear.style.display = typeFilter.size || envFilter.size ? "" : "none";
    clear.onclick = () => {
      typeFilter.clear();
      envFilter.clear();
      updateFilterBtn();
      el.remove();
      popover = null;
      renderList();
    };
    header.appendChild(clear);
    el.appendChild(header);

    if (types.length) {
      const sec = document.createElement("div");
      sec.style.padding = "8px 12px";
      sec.innerHTML = `<div style="font-size:11px;color:var(--surface-sidebar-tertiary);font-weight:600;margin-bottom:4px">Type</div>`;
      for (const t of types) {
        const row = document.createElement("label");
        row.style.display = "flex";
        row.style.alignItems = "center";
        row.style.gap = "8px";
        row.style.cursor = "pointer";
        const cb = document.createElement("input");
        cb.type = "checkbox";
        cb.checked = typeFilter.has(t);
        cb.onchange = () => {
          if (cb.checked) typeFilter.add(t);
          else typeFilter.delete(t);
          updateFilterBtn();
          renderList();
        };
        const badge = glyphElement(t, 18);
        const lab = document.createElement("span");
        lab.textContent = state.adapters.find((a) => a.id === t)?.label ?? t;
        row.append(cb, badge, lab);
        sec.appendChild(row);
      }
      el.appendChild(sec);
    }
    if (envs.length) {
      const sec = document.createElement("div");
      sec.style.padding = "8px 12px";
      sec.innerHTML = `<div style="font-size:11px;color:var(--surface-sidebar-tertiary);font-weight:600;margin-bottom:4px">Environment</div>`;
      for (const e of envs) {
        const row = document.createElement("label");
        row.style.display = "flex";
        row.style.alignItems = "center";
        row.style.gap = "8px";
        row.style.cursor = "pointer";
        const cb = document.createElement("input");
        cb.type = "checkbox";
        cb.checked = envFilter.has(e);
        cb.onchange = () => {
          if (cb.checked) envFilter.add(e);
          else envFilter.delete(e);
          updateFilterBtn();
          renderList();
        };
        const dot = document.createElement("span");
        dot.style.width = "7px";
        dot.style.height = "7px";
        dot.style.borderRadius = "50%";
        dot.style.display = "inline-block";
        dot.style.background = envDotColor(e);
        const lab = document.createElement("span");
        lab.textContent = envLabel(e);
        row.append(cb, dot, lab);
        sec.appendChild(row);
      }
      el.appendChild(sec);
    }
    // mount near filterBtn
    filterBtn.style.position = "relative";
    // Place popover as child of searchRow with absolute anchoring
    searchRow.style.position = "relative";
    el.style.top = "44px";
    el.style.right = "12px";
    searchRow.appendChild(el);
    popover = el;
  }

  filterBtn.addEventListener("click", showPopover);
  searchRow.append(searchWrap, filterBtn);

  // List container
  const list = document.createElement("div");
  list.className = "sidebar-list";
  list.setAttribute("role", "list");

  // Delete confirmation
  const confirmOverlay = document.createElement("div");
  confirmOverlay.style.position = "fixed";
  confirmOverlay.style.inset = "0";
  confirmOverlay.style.display = "none";
  confirmOverlay.style.placeItems = "center";
  confirmOverlay.style.background = "rgba(0,0,0,0.24)";
  confirmOverlay.style.zIndex = "100";
  confirmOverlay.style.padding = "24px";
  function showConfirm(kind: "integration" | "group", id: string, name: string) {
    confirmOverlay.style.display = "grid";
    confirmOverlay.innerHTML = "";
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    dialog.style.background = "var(--surface-panel)";
    dialog.style.padding = "20px";
    dialog.style.borderRadius = "10px";
    dialog.style.maxWidth = "360px";
    dialog.style.boxShadow = "0 12px 32px rgba(0,0,0,0.2)";
    const title = document.createElement("h3");
    title.style.margin = "0 0 8px";
    title.textContent = `Delete ${kind} “${name}”?`;
    const msg = document.createElement("p");
    msg.style.margin = "0 0 16px";
    msg.style.color = "var(--surface-tertiary-label)";
    msg.textContent = "This can’t be undone.";
    const actions = document.createElement("div");
    actions.style.display = "flex";
    actions.style.justifyContent = "flex-end";
    actions.style.gap = "8px";
    const cancel = document.createElement("button");
    cancel.textContent = "Cancel";
    cancel.onclick = () => (confirmOverlay.style.display = "none");
    const del = document.createElement("button");
    del.textContent = "Delete";
    del.style.background = "#ef4444";
    del.style.color = "white";
    del.style.border = "none";
    del.style.padding = "6px 12px";
    del.style.borderRadius = "6px";
    del.style.cursor = "pointer";
    del.onclick = () => {
      confirmOverlay.style.display = "none";
      cbs.onDelete(kind, id, name);
    };
    actions.append(cancel, del);
    dialog.append(title, msg, actions);
    confirmOverlay.appendChild(dialog);
    del.focus();
  }

  function renderList() {
    list.innerHTML = "";
    if (state.loading) {
      const loading = document.createElement("div");
      loading.className = "sidebar-loading";
      loading.textContent = "Loading integrations…";
      list.appendChild(loading);
      return;
    }
    if (state.adaptersLoadFailed) {
      const err = document.createElement("div");
      err.className = "sidebar-empty";
      err.textContent = "Couldn’t load the integration catalog. ";
      const retry = document.createElement("button");
      retry.textContent = "Try again";
      retry.onclick = cbs.onRetryAdapters;
      err.appendChild(retry);
      list.appendChild(err);
      return;
    }

    const groups = filteredGroups(state.groups, query, typeFilter, envFilter);
    const integrations = filteredIntegrations(
      state.integrations,
      query,
      typeFilter,
      envFilter,
      state.adapters,
    );

    if (!groups.length && !integrations.length) {
      const empty = document.createElement("div");
      empty.className = "sidebar-empty";
      const isFiltered = query.trim() !== "" || typeFilter.size > 0 || envFilter.size > 0;
      if (isFiltered) {
        empty.textContent = query.trim()
          ? `No results for “${query.trim()}”`
          : "No integrations match the selected filters.";
      } else if (!state.integrations.length && !state.groups.length) {
        empty.textContent = "No integrations yet. Add your first one to get started.";
      } else {
        empty.textContent = "No matches.";
      }
      list.appendChild(empty);
      return;
    }

    if (groups.length) {
      const title = document.createElement("div");
      title.className = "sidebar-section-title";
      title.textContent = "Groups";
      list.appendChild(title);
      for (const g of groups) {
        const row = document.createElement("div");
        row.className = "sidebar-row";
        if (g.id === selectedId) row.classList.add("selected");
        row.setAttribute("role", "listitem");
        row.tabIndex = 0;
        row.onclick = () => cbs.onSelect(g.id);
        row.onkeydown = (e) => {
          if (e.key === "Enter") cbs.onSelect(g.id);
        };
        // context menu
        row.addEventListener("contextmenu", (e) => {
          e.preventDefault();
          // simple inline menu: show confirm delete
          // For R18 we trigger delete confirmation directly on right-click? Use prompt.
          // Instead show a tiny menu with Delete
          const menu = document.createElement("div");
          menu.style.position = "fixed";
          menu.style.left = `${e.clientX}px`;
          menu.style.top = `${e.clientY}px`;
          menu.style.background = "var(--surface-panel)";
          menu.style.border = "1px solid rgba(0,0,0,0.1)";
          menu.style.borderRadius = "6px";
          menu.style.padding = "4px";
          menu.style.zIndex = "60";
          const del = document.createElement("button");
          del.textContent = "Delete";
          del.style.display = "block";
          del.style.width = "100%";
          del.style.textAlign = "left";
          del.style.background = "transparent";
          del.style.border = "none";
          del.style.padding = "6px 12px";
          del.style.cursor = "pointer";
          del.onclick = () => {
            menu.remove();
            showConfirm("group", g.id, g.name);
          };
          menu.appendChild(del);
          document.body.appendChild(menu);
          const close = () => {
            menu.remove();
            window.removeEventListener("click", close);
          };
          setTimeout(() => window.addEventListener("click", close), 0);
        });
        const icon = document.createElement("span");
        icon.textContent = "▦";
        icon.style.color = "var(--surface-tertiary-label)";
        icon.style.fontSize = "11px";
        const name = document.createElement("span");
        name.className = "sidebar-row-name";
        name.textContent = g.name;
        const count = document.createElement("span");
        count.className = "sidebar-row-env";
        count.textContent = `· ${g.memberIds.length} integration${g.memberIds.length === 1 ? "" : "s"}`;
        row.append(icon, name, count);
        list.appendChild(row);
      }
    }

    if (integrations.length) {
      const title = document.createElement("div");
      title.className = "sidebar-section-title";
      title.textContent = "Integrations";
      list.appendChild(title);
      for (const c of integrations) {
        const row = document.createElement("div");
        row.className = "sidebar-row";
        if (c.id === selectedId) row.classList.add("selected");
        row.setAttribute("role", "listitem");
        row.tabIndex = 0;
        row.onclick = () => cbs.onSelect(c.id);
        row.onkeydown = (e) => {
          if (e.key === "Enter") cbs.onSelect(c.id);
        };
        row.addEventListener("contextmenu", (e) => {
          e.preventDefault();
          const menu = document.createElement("div");
          menu.style.position = "fixed";
          menu.style.left = `${e.clientX}px`;
          menu.style.top = `${e.clientY}px`;
          menu.style.background = "var(--surface-panel)";
          menu.style.border = "1px solid rgba(0,0,0,0.1)";
          menu.style.borderRadius = "6px";
          menu.style.padding = "4px";
          menu.style.zIndex = "60";
          const dup = document.createElement("button");
          dup.textContent = "Duplicate";
          dup.style.display = "block";
          dup.style.width = "100%";
          dup.style.textAlign = "left";
          dup.style.background = "transparent";
          dup.style.border = "none";
          dup.style.padding = "6px 12px";
          dup.style.cursor = "pointer";
          dup.onclick = () => {
            menu.remove();
            cbs.onDuplicate(c.id);
          };
          const del = document.createElement("button");
          del.textContent = "Delete";
          del.style.display = "block";
          del.style.width = "100%";
          del.style.textAlign = "left";
          del.style.background = "transparent";
          del.style.border = "none";
          del.style.padding = "6px 12px";
          del.style.cursor = "pointer";
          del.onclick = () => {
            menu.remove();
            showConfirm("integration", c.id, c.name);
          };
          menu.append(dup, del);
          document.body.appendChild(menu);
          const close = () => {
            menu.remove();
            window.removeEventListener("click", close);
          };
          setTimeout(() => window.addEventListener("click", close), 0);
        });

        const glyph = glyphElement(c.type, 12);
        glyph.style.background = hexToRgba(adapterColor(c.type), 0.14);
        glyph.style.borderRadius = "3px";
        glyph.style.padding = "2px";
        const nameEl = document.createElement("span");
        nameEl.className = "sidebar-row-name";
        nameEl.textContent = c.name;
        const env = document.createElement("span");
        env.className = "sidebar-row-env";
        env.textContent = `· ${envLabel(c.environment)}`;
        const spacer = document.createElement("span");
        spacer.style.flex = "1";
        row.append(glyph, nameEl, env, spacer);
        if (c.readOnly) {
          const lock = document.createElement("span");
          lock.textContent = "🔒";
          lock.title = "Read-only";
          lock.style.fontSize = "10px";
          lock.style.color = "var(--surface-sidebar-tertiary)";
          row.appendChild(lock);
        }
        const health = state.health[c.id];
        if (health) {
          const dot = document.createElement("span");
          dot.className = `health-dot ${health.status === "error" ? "error" : "ok"}`;
          dot.title = health.status === "error" ? health.error ?? "Connection failing" : "Healthy";
          row.appendChild(dot);
        }
        // absent health renders nothing — third state

        list.appendChild(row);
      }
    }
  }

  input.addEventListener("input", () => {
    query = input.value;
    clearBtn.style.display = query ? "" : "none";
    renderList();
  });
  clearBtn.addEventListener("click", () => {
    input.value = "";
    query = "";
    clearBtn.style.display = "none";
    input.focus();
    renderList();
  });
  // keyboard shortcut Cmd+F focuses search
  window.addEventListener("keydown", (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "f") {
      e.preventDefault();
      input.focus();
    }
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "n" && !e.shiftKey) {
      e.preventDefault();
      cbs.onCreateIntegration();
    }
    if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "n") {
      e.preventDefault();
      cbs.onCreateGroup();
    }
  });

  // initial render
  updateFilterBtn();
  renderList();

  root.append(toolbar, searchRow, list, confirmOverlay);

  // expose helper to update state externally
  (root as unknown as { _render: () => void })._render = renderList;

  return root;
}

function envDotColor(e: Environment): string {
  switch (e) {
    case "production":
      return "#ef4444";
    case "staging":
      return "#f97316";
    case "development":
      return "#3b82f6";
    case "local":
      return "#6b7280";
  }
}

function hexToRgba(hex: string, alpha: number): string {
  const h = hex.replace("#", "");
  const r = parseInt(h.slice(0, 2), 16);
  const g = parseInt(h.slice(2, 4), 16);
  const b = parseInt(h.slice(4, 6), 16);
  return `rgba(${r},${g},${b},${alpha})`;
}
