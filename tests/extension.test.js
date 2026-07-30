const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const root = path.resolve(__dirname, "..");
const extensionDir = path.join(root, "extension");

class Element {
  constructor(tagName, id = "") {
    this.tagName = tagName.toUpperCase();
    this.id = id;
    this.disabled = false;
    this.textContent = "";
    this.listeners = new Map();
  }

  addEventListener(name, listener) {
    this.listeners.set(name, listener);
  }

  click() {
    const listener = this.listeners.get("click");
    if (listener) return listener();
  }
}

function rejectIfCallback(apiNamespace, name, maybeCallback) {
  if (apiNamespace === "browser" && typeof maybeCallback === "function") {
    throw new Error(`${name} is promise-only under browser; callback form rejected`);
  }
}

function popupHarness(apiNamespace = "chrome", options = {}) {
  const tabUrl = options.tabUrl ?? "https://example.com/article";
  const tabId = options.tabId ?? 7;
  const pageHtml = options.pageHtml ?? "<html><body>captured</body></html>";
  const elements = new Map([
    ["send", new Element("button", "send")],
    ["download", new Element("button", "download")],
    ["status", new Element("div", "status")],
  ]);
  const runtimeListeners = [];
  const sentMessages = [];
  let executeScriptCalls = 0;

  const api = {
    runtime: {
      onMessage: {
        addListener: (listener) => runtimeListeners.push(listener),
      },
      sendMessage: (message, callback) => {
        rejectIfCallback(apiNamespace, "runtime.sendMessage", callback);
        sentMessages.push(message);
        return Promise.resolve();
      },
    },
    tabs: {
      query: (queryInfo, callback) => {
        rejectIfCallback(apiNamespace, "tabs.query", callback);
        return Promise.resolve([{ url: tabUrl, id: tabId }]);
      },
    },
    scripting: {
      executeScript: (details, callback) => {
        rejectIfCallback(apiNamespace, "scripting.executeScript", callback);
        executeScriptCalls += 1;
        return Promise.resolve([{ result: pageHtml }]);
      },
    },
  };

  const context = {
    console,
    document: {
      getElementById: (id) => elements.get(id),
    },
    [apiNamespace]: api,
  };

  vm.createContext(context);
  const source = fs.readFileSync(path.join(extensionDir, "popup.js"), "utf8");
  vm.runInContext(
    `${source}
globalThis.popupTest = {
  ready: initialization,
  status: () => document.getElementById("status").textContent,
  send: () => document.getElementById("send").click(),
  download: () => document.getElementById("download").click(),
};
`,
    context,
  );

  return {
    ready: context.popupTest.ready,
    status: () => context.popupTest.status(),
    send: async () => context.popupTest.send(),
    download: async () => context.popupTest.download(),
    deliver: (message) => {
      for (const listener of runtimeListeners) listener(message);
    },
    sentMessages,
    executeScriptCalls: () => executeScriptCalls,
    elements,
  };
}

function backgroundHarness(apiNamespace = "chrome", options = {}) {
  const badgeTexts = [];
  const forwardedMessages = [];
  const nativePosts = [];
  let disconnectListener = null;
  let messageListener = null;
  let nativeMessageListener = null;
  let connectCalls = 0;
  let portError = null;
  const postMessageThrows = options.postMessageThrows === true;

  const port = {
    postMessage: (message) => {
      if (postMessageThrows) throw new Error("Native host missing");
      nativePosts.push(message);
    },
    onMessage: {
      addListener: (listener) => {
        nativeMessageListener = listener;
      },
    },
    onDisconnect: {
      addListener: (listener) => {
        disconnectListener = listener;
      },
    },
    get error() {
      return portError;
    },
  };

  const api = {
    action: {
      setBadgeText: ({ text }) => badgeTexts.push(text),
      setBadgeBackgroundColor: () => {},
    },
    runtime: {
      connectNative: () => {
        connectCalls += 1;
        return port;
      },
      onMessage: {
        addListener: (listener) => {
          messageListener = listener;
        },
      },
      sendMessage: (message) => {
        forwardedMessages.push(message);
        return Promise.resolve();
      },
      lastError: null,
    },
  };

  const context = {
    console,
    setTimeout: () => 0,
    clearTimeout: () => {},
    [apiNamespace]: api,
  };

  vm.createContext(context);
  const source = fs.readFileSync(path.join(extensionDir, "background.js"), "utf8");
  vm.runInContext(source, context);

  return {
    request: (message) => messageListener(message),
    receiveFromNative: (message) => nativeMessageListener(message),
    disconnect: (detail, via = "port") => {
      if (via === "port") {
        portError = detail ? { message: detail } : null;
        api.runtime.lastError = null;
      } else {
        portError = null;
        api.runtime.lastError = detail ? { message: detail } : null;
      }
      disconnectListener();
    },
    nativePosts,
    forwardedMessages,
    badgeTexts,
    connectCalls: () => connectCalls,
  };
}

test("popup initializes through Chrome and enables actions", async () => {
  const harness = popupHarness("chrome");
  await harness.ready;
  assert.equal(harness.elements.get("send").disabled, false);
  assert.equal(harness.status(), "");
});

test("popup runs through Firefox's promise-based browser namespace", async () => {
  const harness = popupHarness("browser");
  await harness.ready;
  assert.equal(harness.elements.get("send").disabled, false);
});

test("popup captures page HTML for articles on send", async () => {
  const harness = popupHarness("browser", {
    tabUrl: "https://example.com/long-read",
    pageHtml: "<html><body>article body</body></html>",
  });
  await harness.ready;
  await harness.send();

  assert.equal(harness.executeScriptCalls(), 1);
  assert.equal(harness.sentMessages.length, 1);
  const sent = harness.sentMessages[0];
  assert.equal(sent.action, "send");
  assert.equal(sent.url, "https://example.com/long-read");
  assert.equal(sent.pageHtml, "<html><body>article body</body></html>");
});

test("popup skips capture for Hacker News threads", async () => {
  const harness = popupHarness("chrome", {
    tabUrl: "https://news.ycombinator.com/item?id=1",
  });
  await harness.ready;
  await harness.download();

  assert.equal(harness.executeScriptCalls(), 0);
  assert.equal(harness.sentMessages[0].pageHtml, null);
  assert.equal(harness.sentMessages[0].action, "download");
});

test("popup rejects pages that are not http(s)", async () => {
  const harness = popupHarness("browser", { tabUrl: "about:blank" });
  await harness.ready;
  assert.equal(harness.elements.get("send").disabled, true);
  assert.match(harness.status(), /Open a web page first/i);
});

test("popup surfaces progress and completion messages", async () => {
  const harness = popupHarness("chrome");
  await harness.ready;
  harness.deliver({ status: "progress", stage: "fetching", detail: "article" });
  assert.equal(harness.status(), "fetching: article");
  harness.deliver({ status: "ok", emailed: true });
  assert.equal(harness.status(), "EPUB emailed.");
});

test("background posts to the native host", () => {
  const harness = backgroundHarness("chrome");
  harness.request({
    action: "send",
    url: "https://example.com/",
    pageHtml: "<html></html>",
  });

  assert.equal(harness.connectCalls(), 1);
  const posted = harness.nativePosts[0];
  assert.equal(posted.action, "send");
  assert.equal(posted.url, "https://example.com/");
  assert.equal(posted.page_html, "<html></html>");
  assert.equal(harness.badgeTexts.at(-1), "...");
});

test("background runs through Firefox's promise-based browser namespace", () => {
  const harness = backgroundHarness("browser");
  harness.request({
    action: "download",
    url: "https://example.com/",
    pageHtml: null,
  });
  assert.equal(harness.nativePosts.length, 1);
  assert.equal(harness.nativePosts[0].action, "download");
});

test("background reports Firefox port disconnect errors", () => {
  const harness = backgroundHarness("browser");
  harness.request({
    action: "send",
    url: "https://example.com/",
    pageHtml: null,
  });
  harness.disconnect("Native host has exited.", "port");
  assert.equal(harness.forwardedMessages.at(-1).status, "error");
  assert.equal(harness.forwardedMessages.at(-1).message, "Native host has exited.");
  assert.equal(harness.badgeTexts.at(-1), "!");
});

test("background falls back to runtime.lastError on disconnect", () => {
  const harness = backgroundHarness("chrome");
  harness.request({
    action: "download",
    url: "https://example.com/",
    pageHtml: null,
  });
  harness.disconnect("Specified native messaging host not found.", "lastError");
  assert.equal(
    harness.forwardedMessages.at(-1).message,
    "Specified native messaging host not found.",
  );
});

test("background uses a generic disconnect message when no detail is available", () => {
  const harness = backgroundHarness("browser");
  harness.request({
    action: "send",
    url: "https://example.com/",
    pageHtml: null,
  });
  harness.disconnect(null, "port");
  assert.equal(harness.forwardedMessages.at(-1).message, "Native helper disconnected");
});

test("background reports postMessage failures", () => {
  const harness = backgroundHarness("browser", { postMessageThrows: true });
  harness.request({
    action: "send",
    url: "https://example.com/",
    pageHtml: null,
  });
  assert.equal(harness.forwardedMessages.at(-1).status, "error");
  assert.match(harness.forwardedMessages.at(-1).message, /Native host missing/i);
  assert.equal(harness.badgeTexts.at(-1), "!");
});

test("browser manifests share capabilities but use native backgrounds", () => {
  const chromium = JSON.parse(
    fs.readFileSync(path.join(extensionDir, "manifest.json"), "utf8"),
  );
  const firefox = JSON.parse(
    fs.readFileSync(path.join(extensionDir, "manifest.firefox.json"), "utf8"),
  );

  for (const key of [
    "manifest_version",
    "name",
    "version",
    "description",
    "icons",
    "action",
    "permissions",
  ]) {
    assert.deepEqual(firefox[key], chromium[key], `${key} differs by browser`);
  }

  assert.deepEqual(chromium.background, {
    service_worker: "background.js",
  });
  assert.equal(chromium.browser_specific_settings, undefined);
  assert.ok(chromium.permissions.includes("nativeMessaging"));
  assert.ok(chromium.permissions.includes("scripting"));

  assert.deepEqual(firefox.background, {
    scripts: ["background.js"],
  });
  assert.equal(firefox.browser_specific_settings.gecko.id, "@rustypub.chaselambert");
  assert.equal(
    firefox.browser_specific_settings.gecko.strict_min_version,
    "142.0",
  );
  assert.deepEqual(
    firefox.browser_specific_settings.gecko.data_collection_permissions.required,
    ["browsingActivity", "websiteContent"],
  );
});
