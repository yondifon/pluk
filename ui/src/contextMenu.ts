function isEditable(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest("input, textarea, [contenteditable]:not([contenteditable='false'])") !== null;
}

function hasSelectedText(target: EventTarget | null): boolean {
  const selection = document.getSelection();
  return target instanceof Node
    && selection?.rangeCount === 1
    && !selection.getRangeAt(0).collapsed
    && selection.getRangeAt(0).intersectsNode(target);
}

export function suppressWebViewContextMenu(isDevelopment: boolean): () => void {
  if (isDevelopment) return () => {};

  const controller = new AbortController();
  document.addEventListener("contextmenu", (event) => {
    if (isEditable(event.target) || hasSelectedText(event.target)) return;
    event.preventDefault();
  }, { signal: controller.signal });
  return () => controller.abort();
}
