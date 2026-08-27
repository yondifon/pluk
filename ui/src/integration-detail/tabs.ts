export type TabId = "logs" | "overview" | "tools";

export function renderTabs(
  container: HTMLElement,
  selected: TabId,
  onSelect: (id: TabId) => void,
): void {
  container.innerHTML = "";
  container.className = "detail-tabs";
  const tabs: Array<{ id: TabId; label: string }> = [
    { id: "logs", label: "Logs" },
    { id: "overview", label: "Overview" },
    { id: "tools", label: "Tools" },
  ];
  for (const t of tabs) {
    const btn = document.createElement("button");
    btn.className = `tab ${selected === t.id ? "tab-active" : ""}`;
    btn.textContent = t.label;
    btn.setAttribute("aria-selected", String(selected === t.id));
    btn.addEventListener("click", () => onSelect(t.id));
    container.appendChild(btn);
  }
}
