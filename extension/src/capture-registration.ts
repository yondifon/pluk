// Registers the Instagram capture script as a MAIN-world content script so
// it patches fetch/XMLHttpRequest before Instagram's own scripts run. Uses
// chrome.scripting instead of the debugger Network domain, which shows the
// user a warning bar.

const INSTAGRAM_CAPTURE_SCRIPT_ID = "pluk-instagram-capture";

export async function registerInstagramCapture(): Promise<void> {
  try {
    await chrome.scripting.unregisterContentScripts({
      ids: [INSTAGRAM_CAPTURE_SCRIPT_ID],
    });
  } catch {
    // Nothing was registered yet; that is the common case on first install.
  }
  try {
    await chrome.scripting.registerContentScripts([
      {
        id: INSTAGRAM_CAPTURE_SCRIPT_ID,
        matches: ["https://www.instagram.com/*"],
        js: ["instagram-capture.js"],
        runAt: "document_start",
        world: "MAIN",
        persistAcrossSessions: true,
      },
    ]);
  } catch {
    // Instagram host access has not been granted yet. The permission grant
    // flow in options.ts re-triggers registration once it is.
  }
}
