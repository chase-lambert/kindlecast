const HOST = "com.chaselambert.rustypub";
const extensionApi = globalThis.browser ?? globalThis.chrome;
let port = null;
let badgeClearTimer = null;

function setBadge(text, color) {
  if (badgeClearTimer) {
    clearTimeout(badgeClearTimer);
    badgeClearTimer = null;
  }
  extensionApi.action.setBadgeText({ text });
  if (color) extensionApi.action.setBadgeBackgroundColor({ color });
  if (text) {
    badgeClearTimer = setTimeout(() => {
      extensionApi.action.setBadgeText({ text: "" });
      badgeClearTimer = null;
    }, 15000);
  }
}

function ensurePort() {
  if (port) return port;
  port = extensionApi.runtime.connectNative(HOST);
  port.onMessage.addListener((message) => {
    extensionApi.runtime.sendMessage(message).catch(() => {});
    if (message.status === "ok") setBadge("OK", "#188038");
    if (message.status === "error") setBadge("!", "#d93025");
  });
  port.onDisconnect.addListener(() => {
    const err =
      port?.error?.message ||
      extensionApi.runtime.lastError?.message ||
      "Native helper disconnected";
    port = null;
    extensionApi.runtime
      .sendMessage({ status: "error", message: err })
      .catch(() => {});
    setBadge("!", "#d93025");
  });
  return port;
}

extensionApi.runtime.onMessage.addListener((message) => {
  if (message.action !== "send" && message.action !== "download") return;
  setBadge("...", "#5f6368");
  try {
    ensurePort().postMessage({
      action: message.action,
      url: message.url,
      page_html: message.pageHtml,
    });
  } catch (error) {
    extensionApi.runtime
      .sendMessage({
        status: "error",
        message: error.message || "Native helper unavailable",
      })
      .catch(() => {});
    setBadge("!", "#d93025");
  }
});
