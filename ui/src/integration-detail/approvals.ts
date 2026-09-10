import type { Integration } from "./types";

/** The one-line account of what asking is set to. */
export function askingSummary(ask: boolean): string {
  return ask ? "Anything else asks you first." : "Anything else is refused without asking.";
}

/** The rules in force, read-only — the Edit screen is where they are written. */
export function renderApprovals(container: HTMLElement, integration: Integration): void {
  const approvals = integration.approvals ?? { ask: true, allow: [], deny: [] };
  const card = document.createElement("section");
  card.className = "ui-card";

  const title = document.createElement("h2");
  title.className = "ui-card-title";
  title.textContent = "What the agent may run";
  card.appendChild(title);

  const lists: Array<[string, string[]]> = [
    ["Always allow", approvals.allow],
    ["Never allow", approvals.deny],
  ];
  for (const [label, rules] of lists) {
    const row = document.createElement("div");
    row.className = "tool-row tool-on";
    const head = document.createElement("div");
    head.className = "tool-head";
    const name = document.createElement("span");
    name.className = "tool-name";
    name.textContent = label;
    const count = document.createElement("span");
    count.className = "tool-category";
    count.textContent = rules.length === 1 ? "1 rule" : `${rules.length} rules`;
    head.append(name, count);
    row.appendChild(head);
    if (rules.length) {
      const body = document.createElement("div");
      body.className = "tool-body";
      const list = document.createElement("div");
      list.className = "tool-summary mono";
      list.textContent = rules.join("\n");
      body.appendChild(list);
      row.appendChild(body);
    }
    card.appendChild(row);
  }

  const summary = document.createElement("p");
  summary.className = "hint";
  summary.textContent = askingSummary(approvals.ask);
  card.appendChild(summary);

  container.appendChild(card);
}
