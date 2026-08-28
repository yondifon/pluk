/**
 * Empty states — user-facing copy only, no internal vocabulary.
 * Banned: owner, manifest, verdict, projection, slug
 */
import { createButton } from "./primitives";

export type EmptyKind = "no-integrations" | "no-groups" | "nothing-selected" | "catalog-unavailable" | "no-matches";

export interface EmptyState {
  kind: EmptyKind;
  title: string;
  body: string;
  actionLabel?: string;
  actionId?: string;
}

export function emptyState(kind: EmptyKind, opts?: { query?: string }): EmptyState {
  switch (kind) {
    case "no-integrations":
      return {
        kind,
      title: "Connect an integration to get started",
      body: "Add a database, Linear workspace, or another local connection. Pluk keeps the server and access rules on this Mac.",
        actionLabel: "New Integration",
        actionId: "new-integration",
      };
    case "no-groups":
      return {
        kind,
        title: "No groups yet",
        body: "Groups bundle integrations behind one endpoint. Create a group to combine connections for an agent.",
        actionLabel: "New Group",
        actionId: "new-group",
      };
    case "nothing-selected":
      // Fallback when sidebar has items but none selected
      return {
        kind,
        title: "Select an integration or group",
        body: "Choose an item from the sidebar to see its details, logs, and setup steps.",
      };
    case "catalog-unavailable":
      return {
        kind,
        title: "Couldn’t load integrations",
        body: "The integration catalog is unavailable. Check that the server is running and try again.",
        actionLabel: "Retry",
        actionId: "retry-catalog",
      };
    case "no-matches":
      return {
        kind,
        title: "No matches",
        body: opts?.query ? `No integrations match “${opts.query}”.` : "No integrations match the selected filters.",
      };
  }
}

export function renderEmptyState(container: HTMLElement, state: EmptyState, onAction?: (id: string) => void): void {
  container.innerHTML = "";
  container.className = "empty-state";
  container.setAttribute("role", "status");

  const title = document.createElement("h2");
  title.className = "empty-title";
  title.textContent = state.title;

  const body = document.createElement("p");
  body.className = "empty-body";
  body.textContent = state.body;

  container.append(title, body);

  if (state.actionLabel && state.actionId) {
    const btn = createButton(state.actionLabel, { variant: "primary", ariaLabel: state.actionLabel });
    if (state.actionId === "new-integration") btn.setAttribute("data-shortcut", "⌘N");
    if (state.actionId === "new-group") btn.setAttribute("data-shortcut", "⇧⌘N");
    btn.addEventListener("click", () => onAction?.(state.actionId!));
    container.appendChild(btn);
  }
}
