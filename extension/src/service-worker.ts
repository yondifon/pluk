import { BrowserBridge } from "./connection";

const bridge = new BrowserBridge();

chrome.runtime.onInstalled.addListener(() => {
  void bridge.initialize();
});

chrome.runtime.onStartup.addListener(() => {
  void bridge.initialize();
});

chrome.alarms.onAlarm.addListener((alarm) => {
  bridge.handleReconnectAlarm(alarm.name);
});

chrome.action.onClicked.addListener(() => {
  void chrome.runtime.openOptionsPage();
});

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (sender.id !== chrome.runtime.id) {
    sendResponse({ ok: false, error: "This settings request is not allowed." });
    return false;
  }
  void bridge.handleMessage(message).then(
    (response) => sendResponse(response),
    () =>
      sendResponse({
        ok: false,
        error: "Could not complete that settings request.",
      }),
  );
  return true;
});

void bridge.initialize();
