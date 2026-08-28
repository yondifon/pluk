import { createIcon } from "./icon";
import { createButton } from "./primitives";

export type ModalSize = "small" | "large";

type ModalOptions = {
  title: string;
  size: ModalSize;
  content: HTMLElement;
  sidebar?: HTMLElement;
  dismissible?: boolean;
  headerActions?: HTMLElement;
  onClose?: () => void;
};

type Modal = {
  close: () => void;
  content: HTMLElement;
};

let activeModal: Modal | null = null;

const focusableSelector = [
  "button:not([disabled])",
  "[href]",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[tabindex]:not([tabindex='-1'])",
].join(",");

export function openModal(options: ModalOptions): Modal {
  if (activeModal) throw new Error("Only one modal can be open at a time.");

  const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const dismissible = options.dismissible ?? true;
  const overlay = document.createElement("div");
  overlay.className = "modal-overlay";

  const dialog = document.createElement("section");
  dialog.className = `modal modal-${options.size}`;
  if (options.sidebar) dialog.classList.add("modal-with-sidebar");
  dialog.setAttribute("role", "dialog");
  dialog.setAttribute("aria-modal", "true");
  dialog.setAttribute("aria-labelledby", "modal-title");
  dialog.tabIndex = -1;

  const header = document.createElement("header");
  header.className = "modal-header";
  const title = document.createElement("h2");
  title.className = "modal-title";
  title.id = "modal-title";
  title.textContent = options.title;
  const actions = document.createElement("div");
  actions.className = "modal-header-actions";
  if (options.headerActions) actions.appendChild(options.headerActions);
  const closeButton = document.createElement("button");
  closeButton.type = "button";
  closeButton.className = "modal-close icon-button";
  closeButton.setAttribute("aria-label", "Close dialog");
  closeButton.appendChild(createIcon("close"));
  actions.appendChild(closeButton);
  header.append(title, actions);

  const body = document.createElement("div");
  body.className = "modal-body";
  if (options.sidebar) {
    const layout = document.createElement("div");
    layout.className = "modal-large-layout";
    options.sidebar.classList.add("modal-sidebar");
    const content = document.createElement("div");
    content.className = "modal-content";
    content.appendChild(options.content);
    layout.append(options.sidebar, content);
    body.appendChild(layout);
  } else {
    body.appendChild(options.content);
  }
  dialog.append(header, body);
  overlay.appendChild(dialog);
  document.body.appendChild(overlay);
  const previousOverflow = document.body.style.overflow;
  document.body.style.overflow = "hidden";

  let closed = false;
  const onKeydown = (event: KeyboardEvent) => {
    if (event.key === "Escape" && dismissible) {
      event.preventDefault();
      close();
      return;
    }
    if (event.key !== "Tab") return;
    const focusable = Array.from(dialog.querySelectorAll<HTMLElement>(focusableSelector));
    if (!focusable.length) {
      event.preventDefault();
      dialog.focus();
      return;
    }
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };
  const close = () => {
    if (closed) return;
    closed = true;
    document.removeEventListener("keydown", onKeydown);
    document.body.style.overflow = previousOverflow;
    overlay.classList.add("modal-closing");
    window.setTimeout(() => {
      overlay.remove();
      if (activeModal?.close === close) activeModal = null;
      opener?.focus();
      options.onClose?.();
    }, 140);
  };

  closeButton.addEventListener("click", close);
  overlay.addEventListener("click", (event) => {
    if (dismissible && event.target === overlay) close();
  });
  document.addEventListener("keydown", onKeydown);
  activeModal = { close, content: body };
  queueMicrotask(() => (dialog.querySelector<HTMLElement>(focusableSelector) ?? dialog).focus());
  return activeModal;
}

export function confirmModal(options: {
  title: string;
  message: string;
  confirmLabel: string;
  onConfirm: () => void;
}): void {
  const content = document.createElement("div");
  content.className = "modal-confirmation";
  const message = document.createElement("p");
  message.textContent = options.message;
  const actions = document.createElement("div");
  actions.className = "modal-actions";
  const cancel = createButton("Cancel");
  const confirm = createButton(options.confirmLabel, { variant: "danger" });
  confirm.classList.add("modal-danger");
  actions.append(cancel, confirm);
  content.append(message, actions);
  const modal = openModal({ title: options.title, size: "small", content });
  cancel.addEventListener("click", modal.close);
  confirm.addEventListener("click", () => {
    modal.close();
    options.onConfirm();
  });
}
