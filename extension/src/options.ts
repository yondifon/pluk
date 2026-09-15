import {
  ALL_URLS_ORIGIN,
  type ConnectionSettings,
  type ConnectionStatus,
  DEFAULT_SERVER_URL,
  isValidPlukId,
  parseConnectionSettings,
  parseConnectionStatus,
  parseServerUrl,
  readSettings,
  SITE_ORIGINS,
} from "./state";

const form = requiredElement<HTMLFormElement>("#connection-form");
const serverUrlInput = requiredElement<HTMLInputElement>("#server-url");
const plukIdInput = requiredElement<HTMLInputElement>("#pluk-id");
const keepConnectedInput = requiredElement<HTMLInputElement>("#keep-connected");
const saveButton = requiredElement<HTMLButtonElement>("#save-button");
const saveFeedback = requiredElement<HTMLParagraphElement>("#save-feedback");
const serverUrlError =
  requiredElement<HTMLParagraphElement>("#server-url-error");
const plukIdError = requiredElement<HTMLParagraphElement>("#pluk-id-error");
const statusChip = requiredElement<HTMLDivElement>("#status-chip");
const statusLabel = requiredElement<HTMLSpanElement>("#status-label");
const grantButton = requiredElement<HTMLButtonElement>("#grant-button");
const permissionFeedback = requiredElement<HTMLParagraphElement>(
  "#permission-feedback",
);
const accessCount = requiredElement<HTMLSpanElement>("#access-count");
const siteList = requiredElement<HTMLUListElement>("#site-list");
const captureGrantButton = requiredElement<HTMLButtonElement>(
  "#capture-grant-button",
);
const captureFeedback =
  requiredElement<HTMLParagraphElement>("#capture-feedback");
const captureCount = requiredElement<HTMLSpanElement>("#capture-count");

let lastKnownServerUrl: string | null = null;

void initialize();

form.addEventListener("submit", (event) => {
  event.preventDefault();
  void saveSettings();
});

serverUrlInput.addEventListener("blur", () => {
  validateFields();
});

plukIdInput.addEventListener("blur", () => {
  validateFields();
});

grantButton.addEventListener("click", () => {
  void requestSiteAccess();
});

captureGrantButton.addEventListener("click", () => {
  void requestScreenshotAccess();
});

chrome.storage.onChanged.addListener((changes, areaName) => {
  if (areaName !== "local") {
    return;
  }
  if (changes.connectionStatus !== undefined) {
    const status = parseConnectionStatus(changes.connectionStatus.newValue);
    if (status !== null) {
      renderStatus(status);
    }
  }
  if (changes.connectionSettings !== undefined) {
    const settings = parseConnectionSettings(
      changes.connectionSettings.newValue,
    );
    if (settings !== null) {
      serverUrlInput.value = settings.serverUrl;
    }
  }
});

async function initialize(): Promise<void> {
  serverUrlInput.value = DEFAULT_SERVER_URL;
  try {
    const settings = await readSettings();
    renderSettings(settings);
  } catch {
    showFeedback(
      saveFeedback,
      "Could not read your saved setup. Reload this page.",
      "error",
    );
  }
  await refreshStatus();
  await renderPermissions();
  await renderCapturePermission();
}

async function refreshStatus(): Promise<void> {
  try {
    const response = await chrome.runtime.sendMessage({ type: "get_status" });
    if (!isRecord(response) || response.ok !== true) {
      throw new Error("status request failed");
    }
    const status = parseConnectionStatus(response.status);
    if (status !== null) {
      renderStatus(status);
    }
    const settings = parseSettingsView(response.settings);
    if (settings !== null) {
      lastKnownServerUrl = settings.serverUrl;
      serverUrlInput.value = settings.serverUrl;
      keepConnectedInput.checked = settings.enabled;
    }
  } catch {
    renderStatus({
      state: "error",
      message: "Wande could not report its status. Reload this page.",
      updatedAt: Date.now(),
    });
  }
}

async function saveSettings(): Promise<void> {
  if (!validateFields()) {
    if (!plukIdInput.validity.valid) {
      plukIdInput.focus();
    } else {
      serverUrlInput.focus();
    }
    return;
  }
  const settings: ConnectionSettings = {
    serverUrl: chosenServerUrl(),
    token: plukIdInput.value,
    enabled: keepConnectedInput.checked,
  };
  saveButton.disabled = true;
  showFeedback(saveFeedback, "Saving...", "pending");
  try {
    const response = await chrome.runtime.sendMessage({
      type: "save_settings",
      settings,
    });
    if (!isRecord(response) || response.ok !== true) {
      const message =
        isRecord(response) && typeof response.error === "string"
          ? response.error
          : "Could not save this. Try again.";
      showFeedback(saveFeedback, message, "error");
      return;
    }
    showFeedback(
      saveFeedback,
      settings.enabled ? "Saved. Connecting to Pluk." : "Saved. Paused for now.",
      "success",
    );
    await refreshStatus();
  } catch {
    showFeedback(
      saveFeedback,
      "Could not reach Pluk. Reload this page and try again.",
      "error",
    );
  } finally {
    saveButton.disabled = false;
  }
}

async function requestSiteAccess(): Promise<void> {
  grantButton.disabled = true;
  showFeedback(
    permissionFeedback,
    "Waiting for Chrome's permission prompt...",
    "pending",
  );
  try {
    const granted = await chrome.permissions.request({
      origins: [...SITE_ORIGINS],
    });
    if (!granted) {
      showFeedback(
        permissionFeedback,
        "Chrome did not grant site access. Choose Grant site access again.",
        "error",
      );
    } else {
      showFeedback(
        permissionFeedback,
        "Site access granted for these hosts.",
        "success",
      );
    }
  } catch {
    showFeedback(
      permissionFeedback,
      "Chrome could not grant site access. Open the extension Details page and allow site access, then try again.",
      "error",
    );
  } finally {
    grantButton.disabled = false;
    await renderPermissions();
  }
}

async function requestScreenshotAccess(): Promise<void> {
  captureGrantButton.disabled = true;
  showFeedback(
    captureFeedback,
    "Waiting for Chrome's permission prompt...",
    "pending",
  );
  try {
    const granted = await chrome.permissions.request({
      origins: [ALL_URLS_ORIGIN],
    });
    if (!granted) {
      showFeedback(
        captureFeedback,
        "Chrome did not grant screenshot capture. Choose Grant screenshot capture again to enable unattended snapshots.",
        "error",
      );
    } else {
      showFeedback(captureFeedback, "Screenshot capture granted.", "success");
    }
  } catch {
    showFeedback(
      captureFeedback,
      "Chrome could not grant screenshot capture. Open the extension Details page and allow site access, then try again.",
      "error",
    );
  } finally {
    captureGrantButton.disabled = false;
    await renderCapturePermission();
  }
}

function renderSettings(settings: ConnectionSettings): void {
  lastKnownServerUrl = settings.serverUrl;
  serverUrlInput.value = settings.serverUrl;
  plukIdInput.value = settings.token;
  keepConnectedInput.checked = settings.enabled;
}

function renderStatus(status: ConnectionStatus): void {
  statusChip.dataset.state = status.state;
  statusLabel.textContent = statusLabelFor(status.state);
  statusChip.title = status.message;
  if (status.state === "error" || status.state === "not_configured") {
    showFeedback(saveFeedback, status.message, "error");
  }
}

async function renderPermissions(): Promise<void> {
  try {
    const grants = await Promise.all(
      SITE_ORIGINS.map((origin) =>
        chrome.permissions.contains({ origins: [origin] }),
      ),
    );
    siteList.replaceChildren(
      ...SITE_ORIGINS.map((origin, index) =>
        createSiteRow(origin, grants[index] ?? false),
      ),
    );
    const grantedCount = grants.filter(Boolean).length;
    accessCount.textContent = `${grantedCount} of ${SITE_ORIGINS.length} granted`;
  } catch {
    accessCount.textContent = "Access status unavailable";
    showFeedback(
      permissionFeedback,
      "Chrome could not read site access. Reload this page and try again.",
      "error",
    );
  }
}

async function renderCapturePermission(): Promise<void> {
  try {
    const granted = await chrome.permissions.contains({
      origins: [ALL_URLS_ORIGIN],
    });
    captureCount.textContent = granted ? "Granted" : "Not granted";
  } catch {
    captureCount.textContent = "Access status unavailable";
    showFeedback(
      captureFeedback,
      "Chrome could not read screenshot access. Reload this page and try again.",
      "error",
    );
  }
}

function createSiteRow(origin: string, granted: boolean): HTMLLIElement {
  const row = document.createElement("li");
  row.className = "site-row";
  const hostname = origin.slice("https://".length, -2);
  const label = document.createElement("span");
  label.textContent = hostname;
  const state = document.createElement("span");
  state.className = "site-state";
  state.dataset.granted = String(granted);
  state.textContent = granted ? "Granted" : "Not granted";
  row.append(label, state);
  return row;
}

// A blank address means "wherever Pluk is" — the service worker finds it.
function chosenServerUrl(): string {
  return (
    parseServerUrl(serverUrlInput.value) ??
    lastKnownServerUrl ??
    DEFAULT_SERVER_URL
  );
}

function validateFields(): boolean {
  const validId = isValidPlukId(plukIdInput.value);
  const validUrl =
    serverUrlInput.value.trim() === "" ||
    parseServerUrl(serverUrlInput.value) !== null;
  const idMessage = validId
    ? ""
    : "Paste your Pluk ID, exactly as Pluk shows it.";
  const urlMessage = validUrl
    ? ""
    : "Use an address such as http://127.0.0.1:4242.";
  plukIdInput.setCustomValidity(idMessage);
  serverUrlInput.setCustomValidity(urlMessage);
  plukIdInput.setAttribute("aria-invalid", String(!validId));
  serverUrlInput.setAttribute("aria-invalid", String(!validUrl));
  plukIdError.textContent = idMessage;
  serverUrlError.textContent = urlMessage;
  return validId && validUrl;
}

function showFeedback(
  element: HTMLElement,
  message: string,
  state: "pending" | "success" | "error",
): void {
  element.textContent = message;
  element.dataset.state = state;
}

function statusLabelFor(state: ConnectionStatus["state"]): string {
  switch (state) {
    case "connected":
      return "Connected";
    case "connecting":
      return "Connecting";
    case "disabled":
      return "Paused";
    case "error":
      return "Needs attention";
    default:
      return "Not connected";
  }
}

function parseSettingsView(value: unknown): {
  readonly serverUrl: string;
  readonly enabled: boolean;
} | null {
  if (
    !isRecord(value) ||
    typeof value.serverUrl !== "string" ||
    typeof value.enabled !== "boolean" ||
    parseServerUrl(value.serverUrl) === null
  ) {
    return null;
  }
  return { serverUrl: value.serverUrl, enabled: value.enabled };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function requiredElement<ElementType extends Element>(
  selector: string,
): ElementType {
  const element = document.querySelector<ElementType>(selector);
  if (element === null) {
    throw new Error(`Missing options element: ${selector}`);
  }
  return element;
}
