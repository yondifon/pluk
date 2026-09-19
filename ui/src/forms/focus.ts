const FOCUSABLE = "input, select, textarea, button";

/** Where someone was working, held as a position in the form so it survives the rebuild. */
export type FormFocus = { index: number; caret: number | null };

export function markFocus(host: HTMLElement): FormFocus | null {
  const active = document.activeElement;
  if (!(active instanceof HTMLElement)) return null;
  const index = Array.from(host.querySelectorAll<HTMLElement>(FOCUSABLE)).indexOf(active);
  if (index < 0) return null;
  return { index, caret: active instanceof HTMLInputElement ? active.selectionStart : null };
}

export function restoreFocus(host: HTMLElement, mark: FormFocus | null): void {
  if (!mark) return;
  const target = host.querySelectorAll<HTMLElement>(FOCUSABLE)[mark.index];
  if (!target) return;
  target.focus();
  if (target instanceof HTMLInputElement && mark.caret != null) target.setSelectionRange(mark.caret, mark.caret);
}
