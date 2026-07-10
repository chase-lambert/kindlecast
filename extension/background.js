const HOST = "com.chaselambert.kindlecast";
let port = null;
let badgeClearTimer = null;

function setBadge(text, color) {
  if (badgeClearTimer) {
    clearTimeout(badgeClearTimer);
    badgeClearTimer = null;
  }
  chrome.action.setBadgeText({ text });
  if (color) chrome.action.setBadgeBackgroundColor({ color });
  if (text) {
    badgeClearTimer = setTimeout(() => {
      chrome.action.setBadgeText({ text: "" });
      badgeClearTimer = null;
    }, 15000);
  }
}

function ensurePort() {
  if (port) return port;
  port = chrome.runtime.connectNative(HOST);
  port.onMessage.addListener((message) => {
    chrome.runtime.sendMessage(message).catch(() => {});
    if (message.status === "ok") setBadge("OK", "#188038");
    if (message.status === "error") setBadge("!", "#d93025");
  });
  port.onDisconnect.addListener(() => {
    const err = chrome.runtime.lastError?.message;
    port = null;
    if (err) {
      chrome.runtime.sendMessage({ status: "error", message: err }).catch(() => {});
      setBadge("!", "#d93025");
    }
  });
  return port;
}

chrome.runtime.onMessage.addListener((message) => {
  if (message.action !== "send" && message.action !== "download") return;
  setBadge("...", "#5f6368");
  ensurePort().postMessage({
    action: message.action,
    url: message.url,
    page_html: message.pageHtml,
  });
});
