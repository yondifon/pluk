// Patches fetch and XMLHttpRequest in Instagram's own page world so the
// site driver can read the JSON Instagram's app already fetches, without
// attaching the Chrome debugger. Registered as a MAIN-world content script
// (see capture-registration.ts); this file is bundled standalone, so it
// cannot rely on anything from module scope beyond what it defines itself.

export interface CaptureEntry {
  readonly url: string;
  readonly method: string;
  readonly receivedAt: number;
  readonly body: unknown;
}

interface CaptureWindow {
  fetch: (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;
  XMLHttpRequest: typeof XMLHttpRequest;
}

interface CaptureElement {
  id: string;
  type: string;
  hidden: boolean;
  textContent: string;
}

interface CaptureDocument {
  getElementById(id: string): CaptureElement | null;
  createElement(tagName: string): CaptureElement;
  readonly head: { appendChild(node: CaptureElement): void } | null;
  readonly documentElement: { appendChild(node: CaptureElement): void } | null;
}

export interface CaptureHandle {
  readonly entries: () => readonly CaptureEntry[];
}

const CONTAINER_ID = "pluk-instagram-captures";
const MAX_ENTRIES = 40;
const MAX_BYTES = 4 * 1024 * 1024;
const HOST_SUFFIX = "instagram.com";

function isInstagramApiUrl(url: string, baseHref: string): boolean {
  let parsed: URL;
  try {
    parsed = new URL(url, baseHref);
  } catch {
    return false;
  }
  const host = parsed.hostname;
  if (host !== HOST_SUFFIX && !host.endsWith(`.${HOST_SUFFIX}`)) {
    return false;
  }
  return parsed.pathname.includes("/graphql") || parsed.pathname.includes("/api/");
}

function byteLength(text: string): number {
  return new TextEncoder().encode(text).length;
}

export function installInstagramCapture(
  win: CaptureWindow,
  doc: CaptureDocument,
  baseHref: string,
): CaptureHandle {
  const buffer: CaptureEntry[] = [];

  function publish(): void {
    try {
      let serialized = JSON.stringify(buffer);
      while (
        buffer.length > 0 &&
        (buffer.length > MAX_ENTRIES || byteLength(serialized) > MAX_BYTES)
      ) {
        buffer.shift();
        serialized = JSON.stringify(buffer);
      }
      let element = doc.getElementById(CONTAINER_ID);
      if (!element) {
        element = doc.createElement("script");
        element.id = CONTAINER_ID;
        element.type = "application/json";
        element.hidden = true;
        const parent = doc.head ?? doc.documentElement;
        parent?.appendChild(element);
      }
      element.textContent = serialized;
    } catch {
      // The host page's DOM is out of this script's control; a hostile or
      // broken createElement/appendChild must not break the real fetch.
    }
  }

  function record(url: string, method: string, body: unknown): void {
    buffer.push({ url, method, receivedAt: Date.now(), body });
    publish();
  }

  const originalFetch = win.fetch;
  if (typeof originalFetch === "function") {
    win.fetch = function patchedFetch(
      this: unknown,
      input: RequestInfo | URL,
      init?: RequestInit,
    ): Promise<Response> {
      let url: string | null = null;
      let method = "GET";
      try {
        method =
          init?.method ??
          (input instanceof Request ? input.method : undefined) ??
          "GET";
        url =
          typeof input === "string"
            ? input
            : input instanceof URL
              ? input.toString()
              : input.url;
      } catch {
        url = null;
      }
      return originalFetch.call(this, input, init).then((response) => {
        try {
          if (url !== null && isInstagramApiUrl(url, baseHref)) {
            const capturedUrl = url;
            const capturedMethod = method;
            response
              .clone()
              .text()
              .then((text) => record(capturedUrl, capturedMethod, JSON.parse(text)))
              .catch(() => {
                // Not JSON, or the clone could not be read; nothing to capture.
              });
          }
        } catch {
          // Capture bookkeeping must never affect the caller's response.
        }
        return response;
      });
    };
  }

  const OriginalXHR = win.XMLHttpRequest;
  if (typeof OriginalXHR === "function") {
    const originalOpen = OriginalXHR.prototype.open;
    const originalSend = OriginalXHR.prototype.send;
    const requests = new WeakMap<XMLHttpRequest, { method: string; url: string }>();
    OriginalXHR.prototype.open = function patchedOpen(
      this: XMLHttpRequest,
      method: string,
      url: string | URL,
      ...rest: unknown[]
    ) {
      requests.set(this, { method, url: typeof url === "string" ? url : url.toString() });
      return (originalOpen as (...args: unknown[]) => unknown).apply(this, [
        method,
        url,
        ...rest,
      ]);
    };
    OriginalXHR.prototype.send = function patchedSend(
      this: XMLHttpRequest,
      ...args: unknown[]
    ) {
      try {
        const request = requests.get(this);
        if (request && isInstagramApiUrl(request.url, baseHref)) {
          this.addEventListener("load", () => {
            try {
              record(request.url, request.method, JSON.parse(this.responseText));
            } catch {
              // Not JSON, or the response body could not be read.
            }
          });
        }
      } catch {
        // Capture bookkeeping must never block the real send call.
      }
      return (originalSend as (...args: unknown[]) => unknown).apply(
        this,
        args,
      );
    };
  }

  return { entries: () => buffer.slice() };
}

if (typeof window !== "undefined" && typeof document !== "undefined") {
  installInstagramCapture(
    window as unknown as CaptureWindow,
    document as unknown as CaptureDocument,
    window.location.href,
  );
}
