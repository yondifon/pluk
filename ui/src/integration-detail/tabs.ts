export type TabId = "logs" | "overview" | "tools";

export function renderTabList(
  container: HTMLElement,
  tabs: Array<{ id: string; label: string }>,
  selected: string,
  onSelect: (id: string) => void,
): void {
  container.innerHTML = "";
  container.className = "detail-tabs ui-tabs";
  container.setAttribute("role", "tablist");
  for (const tab of tabs) {
    const button = document.createElement("button");
    button.className = `tab ui-tab ${selected === tab.id ? "tab-active" : ""}`;
    button.setAttribute("role", "tab");
    button.tabIndex = selected === tab.id ? 0 : -1;
    button.id = `tab-${tab.id}`;
    button.textContent = tab.label;
    button.setAttribute("aria-selected", String(selected === tab.id));
    button.addEventListener("click", () => onSelect(tab.id));
    button.addEventListener("keydown", (event) => {
      if (event.key !== "ArrowRight" && event.key !== "ArrowLeft") return;
      event.preventDefault();
      const index = tabs.findIndex((item) => item.id === tab.id);
      const next = tabs[(index + (event.key === "ArrowRight" ? 1 : tabs.length - 1)) % tabs.length];
      onSelect(next.id);
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
