const sendButton = document.getElementById("send");
const downloadButton = document.getElementById("download");
const statusLine = document.getElementById("status");
let currentUrl = null;
let currentTabId = null;

const THREAD_SITES = [
  /^https?:\/\/news\.ycombinator\.com\/item\?/,
  /^https?:\/\/lobste\.rs\/s\/[a-z0-9]+/i,
];

function setStatus(text) {
  statusLine.textContent = text || "";
}

function setWorking(working) {
  sendButton.disabled = working || !currentUrl;
  downloadButton.disabled = working || !currentUrl;
}

chrome.tabs.query({ active: true, currentWindow: true }, (tabs) => {
  currentUrl = tabs[0]?.url || null;
  currentTabId = tabs[0]?.id || null;
  if (!currentUrl || !/^https?:\/\//.test(currentUrl)) {
    currentUrl = null;
    setWorking(false);
    setStatus("Open a web page first.");
    return;
  }
  setWorking(false);
});

chrome.runtime.onMessage.addListener((message) => {
  if (message.status === "progress") {
    setStatus(`${message.stage}: ${message.detail}`);
  } else if (message.status === "ok") {
    setWorking(false);
    setStatus(message.emailed ? "Sent to Kindle." : "Downloaded EPUB.");
  } else if (message.status === "error") {
    setWorking(false);
    setStatus(message.message);
  }
});

function isThreadUrl(url) {
  return THREAD_SITES.some((pattern) => pattern.test(url));
}

async function capturePageHtml() {
  if (!currentTabId || isThreadUrl(currentUrl)) return null;
  try {
    const results = await chrome.scripting.executeScript({
      target: { tabId: currentTabId },
      func: () => document.documentElement.outerHTML,
    });
    return results?.[0]?.result || null;
  } catch (_err) {
    return null;
  }
}

async function start(action) {
  if (!currentUrl) return;
  setWorking(true);
  setStatus("Starting...");
  const pageHtml = await capturePageHtml();
  chrome.runtime.sendMessage({ action, url: currentUrl, pageHtml });
}

sendButton.addEventListener("click", () => start("send"));
downloadButton.addEventListener("click", () => start("download"));
