/**
 * macOS WKWebView rings the system alert sound for every key event the page
 * leaves unhandled. Only plain character keys landing on the frame itself are
 * swallowed: modifiers, Tab, arrows, Escape and anything aimed at a control
 * still reach their handler.
 */

const CONTROL =
  "input, textarea, select, button, a[href], summary, [contenteditable]:not([contenteditable='false']), [tabindex]";

export function suppressWebViewKeyBeep(): () => void {
  const controller = new AbortController();
  document.addEventListener(
    "keydown",
    (event) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      if (event.key.length !== 1) return;
      if (event.target instanceof Element && event.target.closest(CONTROL)) return;
      event.preventDefault();
    },
    { signal: controller.signal },
  );
  return () => controller.abort();
}
