import "./sidebar.css";
import type { Group, Integration, AdapterManifest, Environment, Health } from "./types";
import { envLabel } from "./types";
import { adapterColor, glyphElement } from "./glyph";
import { createIcon } from "./icon";
import { confirmModal } from "./modal";
import { createButton, openMenu, renderErrorState } from "./primitives";
import { emptyState } from "./emptyStates";
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

type SidebarElement = HTMLElement & { _destroy?: () => void };

export function createSidebar(
  state: SidebarState,
  selectedId: string | null,
  cbs: SidebarCallbacks,
): SidebarElement {
  const root = document.createElement("div");
  root.style.display = "flex";
  root.style.flexDirection = "column";
  root.style.height = "100%";
  root.style.minHeight = "0";

  // Top bar
  const toolbar = document.createElement("div");
  toolbar.className = "sidebar-topbar";
  const btnNewInt = createButton("", { icon: "add", ariaLabel: "New Integration", onClick: cbs.onCreateIntegration });
  btnNewInt.classList.add("icon-button");
  btnNewInt.title = "New Integration (⌘N)";
  btnNewInt.setAttribute("aria-label", "New Integration");
  const btnNewGroup = createButton("", { icon: "group-add", ariaLabel: "New Group", onClick: cbs.onCreateGroup });
  btnNewGroup.classList.add("icon-button");
  btnNewGroup.title = "New Group (⇧⌘N)";
  btnNewGroup.setAttribute("aria-label", "New Group");
  toolbar.append(btnNewInt, btnNewGroup);

  // Search row
  const searchRow = document.createElement("div");
  searchRow.className = "sidebar-search-row";

  const searchWrap = document.createElement("div");
  searchWrap.className = "sidebar-search";
  const searchIcon = document.createElement("span");
  searchIcon.className = "sidebar-search-icon";
  searchIcon.appendChild(createIcon("search"));
  const input = document.createElement("input");
  input.placeholder = "Filter integrations";
  input.setAttribute("aria-label", "Filter integrations");
  input.id = "sidebar-search";
  const clearBtn = createButton("", { icon: "close", ariaLabel: "Clear search" });
  clearBtn.classList.add("icon-button", "sidebar-search-clear");
  clearBtn.title = "Clear";
  clearBtn.style.display = "none";
  searchWrap.append(searchIcon, input, clearBtn);

  const filterBtn = createButton("", { icon: "filter", ariaLabel: "Filter by type and environment" });
  filterBtn.classList.add("sidebar-filter-btn", "icon-button");
  filterBtn.title = "Filter by type and environment";
  filterBtn.setAttribute("aria-label", "Filter by type and environment");

  let popover: HTMLElement | null = null;
  let closePopover: (() => void) | null = null;
  let query = "";
  let typeFilter = new Set<string>();
  let envFilter = new Set<Environment>();

  function updateFilterBtn() {
    const active = typeFilter.size > 0 || envFilter.size > 0;
    filterBtn.classList.toggle("active", active);
  }

  function showPopover() {
    if (popover) {
      closePopover?.();
      return;
    }
    const types = availableTypesSorted(state.integrations, state.adapters);
    const envs = availableEnvs(state.integrations, state.groups);
    const el = document.createElement("div");
    el.className = "popover";
    const header = document.createElement("div");
    header.className = "popover-header";
    header.innerHTML = `<strong>Filters</strong>`;
    const clear = createButton("Clear", { size: "sm" });
    clear.classList.add("popover-clear");
    clear.style.display = typeFilter.size || envFilter.size ? "" : "none";
      clear.addEventListener("click", () => {
       typeFilter.clear();
       envFilter.clear();
       updateFilterBtn();
       closePopover?.();
       filterBtn.focus();
       renderList();
      });
    header.appendChild(clear);
    el.appendChild(header);

    if (types.length) {
      const sec = document.createElement("div");
      sec.className = "popover-section";
      const sectionTitle = document.createElement("div");
      sectionTitle.className = "popover-section-title";
      sectionTitle.textContent = "Type";
      sec.appendChild(sectionTitle);
      for (const t of types) {
        const row = document.createElement("label");
        row.className = "popover-option";
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
      sec.className = "popover-section";
      const sectionTitle = document.createElement("div");
      sectionTitle.className = "popover-section-title";
      sectionTitle.textContent = "Environment";
      sec.appendChild(sectionTitle);
      for (const e of envs) {
        const row = document.createElement("label");
        row.className = "popover-option";
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
        dot.className = `environment-dot environment-${e}`;
        const lab = document.createElement("span");
        lab.textContent = envLabel(e);
        row.append(cb, dot, lab);
        sec.appendChild(row);
      }
      el.appendChild(sec);
    }
    const position = () => {
      const rect = filterBtn.getBoundingClientRect();
      el.style.left = `${Math.max(0, rect.right - 224)}px`;
      el.style.top = `${rect.bottom + 4}px`;
    };
    position();
    document.body.appendChild(el);
    popover = el;
    closePopover = () => {
      el.remove();
      popover = null;
      closePopover = null;
      window.removeEventListener("resize", position);
      window.removeEventListener("scroll", position, true);
      document.removeEventListener("pointerdown", close);
    };
    const close = (event: PointerEvent) => {
      if (event.target !== filterBtn && !el.contains(event.target as Node)) {
        closePopover?.();
      }
    };
    window.addEventListener("resize", position);
    window.addEventListener("scroll", position, true);
    document.addEventListener("pointerdown", close);
    el.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        event.preventDefault();
        closePopover?.();
        filterBtn.focus();
      }
    });
    queueMicrotask(() => el.querySelector<HTMLElement>("button, input")?.focus());
  }

  filterBtn.addEventListener("click", showPopover);
  searchRow.append(searchWrap, filterBtn);

  // List container
  const list = document.createElement("div");
  list.className = "sidebar-list";
  list.setAttribute("role", "list");

  function showConfirm(kind: "integration" | "group", id: string, name: string) {
    confirmModal({
      title: `Delete ${kind} “${name}”?`,
      message: "This cannot be undone.",
      confirmLabel: `Delete ${kind}`,
      onConfirm: () => cbs.onDelete(kind, id, name),
    });
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
      renderErrorState(list, "Couldn’t load the integration catalog.", cbs.onRetryAdapters);
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
         const state = emptyState("no-integrations");
         empty.textContent = `${state.title}. ${state.body}`;
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
         row.setAttribute("role", "button");
         row.setAttribute("aria-label", `${g.name}, group`);
        row.tabIndex = 0;
        row.onclick = () => cbs.onSelect(g.id);
        row.onkeydown = (e) => {
           if (e.key === "Enter" || e.key === " ") { e.preventDefault(); cbs.onSelect(g.id); }
           if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
             e.preventDefault();
             openMenu(row, [{ label: "Delete", danger: true, onSelect: () => showConfirm("group", g.id, g.name) }], { x: row.getBoundingClientRect().left, y: row.getBoundingClientRect().bottom });
           }
        };
        // context menu
        row.addEventListener("contextmenu", (e) => {
          e.preventDefault();
          openMenu(row, [{ label: "Delete", danger: true, onSelect: () => showConfirm("group", g.id, g.name) }], { x: e.clientX, y: e.clientY });
        });
        const icon = document.createElement("span");
        icon.className = "sidebar-group-icon";
         icon.appendChild(createIcon("group"));
         const name = document.createElement("span");
         name.className = "sidebar-row-name";
         name.textContent = g.name;
         name.title = g.name;
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
         row.setAttribute("role", "button");
         row.setAttribute("aria-label", `${c.name}, integration`);
        row.tabIndex = 0;
        row.onclick = () => cbs.onSelect(c.id);
        row.onkeydown = (e) => {
           if (e.key === "Enter" || e.key === " ") { e.preventDefault(); cbs.onSelect(c.id); }
           if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
             e.preventDefault();
             openMenu(row, [
               { label: "Duplicate", onSelect: () => cbs.onDuplicate(c.id) },
               { label: "Delete", danger: true, onSelect: () => showConfirm("integration", c.id, c.name) },
             ], { x: row.getBoundingClientRect().left, y: row.getBoundingClientRect().bottom });
           }
        };
        row.addEventListener("contextmenu", (e) => {
          e.preventDefault();
          openMenu(row, [
            { label: "Duplicate", onSelect: () => cbs.onDuplicate(c.id) },
            { label: "Delete", danger: true, onSelect: () => showConfirm("integration", c.id, c.name) },
          ], { x: e.clientX, y: e.clientY });
        });

        const glyph = glyphElement(c.type, 12);
        glyph.style.background = hexToRgba(adapterColor(c.type), 0.14);
         glyph.classList.add("sidebar-glyph");
         const nameEl = document.createElement("span");
         nameEl.className = "sidebar-row-name";
         nameEl.textContent = c.name;
         nameEl.title = c.name;
        const env = document.createElement("span");
        env.className = "sidebar-row-env";
        env.textContent = `· ${envLabel(c.environment)}`;
        const spacer = document.createElement("span");
         spacer.className = "sidebar-row-spacer";
        row.append(glyph, nameEl, env, spacer);
        if (c.readOnly) {
          const lock = document.createElement("span");
           lock.appendChild(createIcon("lock"));
          lock.title = "Read-only";
           lock.className = "sidebar-lock";
          row.appendChild(lock);
        }
        const health = state.health[c.id];
        const dot = document.createElement("span");
        dot.className = `health-dot ${health ? (health.status === "error" ? "error" : "ok") : "unknown"}`;
        dot.title = health ? (health.status === "error" ? health.error ?? "Connection failing" : "Healthy") : "Not checked";
        dot.setAttribute("aria-label", dot.title);
        row.appendChild(dot);

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
  const onKeydown = (e: KeyboardEvent) => {
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
  };
  window.addEventListener("keydown", onKeydown);

  // initial render
  updateFilterBtn();
  renderList();

  root.append(toolbar, searchRow, list);

  // expose helper to update state externally
  (root as unknown as { _render: () => void })._render = renderList;

  (root as SidebarElement)._destroy = () => {
    window.removeEventListener("keydown", onKeydown);
    if (popover) {
      closePopover?.();
    }
  };
  return root;
}

function hexToRgba(hex: string, alpha: number): string {
  const h = hex.replace("#", "");
  const r = parseInt(h.slice(0, 2), 16);
  const g = parseInt(h.slice(2, 4), 16);
  const b = parseInt(h.slice(4, 6), 16);
  return `rgba(${r},${g},${b},${alpha})`;
}
