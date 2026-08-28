import { createIcon, type IconName } from "./icon";

export type ButtonVariant = "default" | "primary" | "secondary" | "danger";

export function createButton(
  label: string,
  options: {
    variant?: ButtonVariant;
    size?: "sm";
    ariaLabel?: string;
    icon?: IconName;
    onClick?: () => void;
  } = {},
): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = `ui-button ui-button-${options.variant ?? "default"}${options.size ? " ui-button-sm" : ""}`;
  if (options.icon) button.appendChild(createIcon(options.icon));
  if (label) button.appendChild(document.createTextNode(label));
  if (options.ariaLabel) button.setAttribute("aria-label", options.ariaLabel);
  if (options.onClick) button.addEventListener("click", options.onClick);
  return button;
}

export function createCard(title?: string, tag: "section" | "div" = "section"): HTMLElement {
  const card = document.createElement(tag);
  card.className = "ui-card";
  if (title) {
    const heading = document.createElement("h2");
    heading.className = "ui-card-title";
    heading.textContent = title;
    card.appendChild(heading);
  }
  return card;
}

export function createBadge(text: string, variant = "default"): HTMLSpanElement {
  const badge = document.createElement("span");
  badge.className = `ui-badge ui-badge-${variant}`;
  badge.textContent = text;
  return badge;
}

export type MenuItem = { label: string; danger?: boolean; onSelect: () => void };

let activeMenuClose: (() => void) | null = null;

export function openMenu(
  anchor: HTMLElement,
  items: MenuItem[],
  position?: { x: number; y: number },
): { close: () => void } {
  activeMenuClose?.();
  const menu = document.createElement("div");
  menu.className = "ui-menu";
  menu.setAttribute("role", "menu");
  menu.tabIndex = -1;
  for (const item of items) {
    const button = createButton(item.label, { variant: item.danger ? "danger" : "default" });
    button.classList.add("ui-menu-item");
    button.setAttribute("role", "menuitem");
    button.addEventListener("click", () => {
      close();
      item.onSelect();
    });
    menu.appendChild(button);
  }
  document.body.appendChild(menu);
  const rect = anchor.getBoundingClientRect();
  menu.style.left = `${position?.x ?? rect.left}px`;
  menu.style.top = `${position?.y ?? rect.bottom}px`;

  let closed = false;
  const close = () => {
    if (closed) return;
    closed = true;
    document.removeEventListener("pointerdown", onPointerDown);
    document.removeEventListener("keydown", onKeydown);
    menu.remove();
    anchor.focus();
    if (activeMenuClose === close) activeMenuClose = null;
  };
  const onPointerDown = (event: PointerEvent) => {
    if (!menu.contains(event.target as Node) && event.target !== anchor) close();
  };
  const onKeydown = (event: KeyboardEvent) => {
    if (event.key === "Escape") {
      event.preventDefault();
      close();
      return;
    }
    const buttons = Array.from(menu.querySelectorAll<HTMLButtonElement>("button"));
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      buttons[(index + (event.key === "ArrowDown" ? 1 : buttons.length - 1)) % buttons.length]?.focus();
    }
  };
  document.addEventListener("pointerdown", onPointerDown);
  document.addEventListener("keydown", onKeydown);
  activeMenuClose = close;
  queueMicrotask(() => menu.querySelector<HTMLButtonElement>("button")?.focus());
  return { close };
}

export function renderLoadingState(container: HTMLElement, message = "Loading…"): void {
  container.innerHTML = "";
  container.className = "ui-state ui-loading";
  container.setAttribute("role", "status");
  container.textContent = message;
}

export function renderErrorState(container: HTMLElement, message: string, onRetry: () => void): void {
  container.innerHTML = "";
  container.className = "ui-state ui-error";
  container.setAttribute("role", "alert");
  const text = document.createElement("p");
  text.textContent = message;
  container.appendChild(text);
  container.appendChild(createButton("Try again", { variant: "secondary", onClick: onRetry }));
}
