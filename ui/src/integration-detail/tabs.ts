export type TabId = "logs" | "overview" | "tools";

/** Latest handler per tab strip, so reused buttons never call a stale closure. */
const selectHandlers = new WeakMap<HTMLElement, (id: string) => void>();

function markSelected(button: HTMLButtonElement, selected: boolean): void {
  button.classList.toggle("tab-active", selected);
  button.tabIndex = selected ? 0 : -1;
  button.setAttribute("aria-selected", String(selected));
}

export function renderTabList(
  container: HTMLElement,
  tabs: Array<{ id: string; label: string }>,
  selected: string,
  onSelect: (id: string) => void,
): void {
  selectHandlers.set(container, onSelect);
  container.className = "detail-tabs ui-tabs";
  container.setAttribute("role", "tablist");

  // Switching tabs keeps the same buttons, so the underline can transition
  // between them. Rebuilding the strip would replace the nodes mid-transition.
  const existing = Array.from(container.querySelectorAll<HTMLButtonElement>("button.ui-tab"));
  if (existing.length === tabs.length && existing.every((button, i) => button.id === `tab-${tabs[i].id}`)) {
    existing.forEach((button, i) => markSelected(button, tabs[i].id === selected));
    return;
  }

  container.innerHTML = "";
  for (const tab of tabs) {
    const button = document.createElement("button");
    button.className = "tab ui-tab";
    button.setAttribute("role", "tab");
    button.id = `tab-${tab.id}`;
    button.textContent = tab.label;
    markSelected(button, selected === tab.id);
    button.addEventListener("click", () => selectHandlers.get(container)?.(tab.id));
    button.addEventListener("keydown", (event) => {
      if (event.key !== "ArrowRight" && event.key !== "ArrowLeft") return;
      event.preventDefault();
      const index = tabs.findIndex((item) => item.id === tab.id);
      const next = tabs[(index + (event.key === "ArrowRight" ? 1 : tabs.length - 1)) % tabs.length];
      selectHandlers.get(container)?.(next.id);
      queueMicrotask(() => container.querySelector<HTMLButtonElement>(`#tab-${next.id}`)?.focus());
    });
    container.appendChild(button);
  }
}

export function renderTabs(
  container: HTMLElement,
  selected: TabId,
  onSelect: (id: TabId) => void,
): void {
  renderTabList(container, [
    { id: "logs", label: "Logs" },
    { id: "overview", label: "Overview" },
    { id: "tools", label: "Tools" },
  ], selected, (id) => onSelect(id as TabId));
  container.setAttribute("aria-label", "Integration sections");
}
