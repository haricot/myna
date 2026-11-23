"use strict";

// GNOME owns the global shortcut. The native port lets Myna ask Firefox to
// capture the current caret *before* acquiring a dictation text target.
const HOST_NAME = "myna.firefox_anchor";
let active = null;
let port = null;
let reconnectTimer = null;

async function badge(tabId, label, color) {
  await browser.action.setBadgeText({ tabId, text: label });
  await browser.action.setBadgeBackgroundColor({ tabId, color });
}

function send(message) {
  if (!port) return false;
  try { port.postMessage(message); return true; } catch (_) { return false; }
}

function connect() {
  if (port) return;
  try {
    const connected = browser.runtime.connectNative(HOST_NAME);
    port = connected;
    connected.onMessage.addListener((message) => { void onNativeMessage(message); });
    connected.onDisconnect.addListener(() => {
      if (port !== connected) return;
      port = null;
      void stop(false);
      if (reconnectTimer === null) {
        reconnectTimer = setTimeout(() => { reconnectTimer = null; connect(); }, 2000);
      }
    });
    connected.postMessage({ type: "ready", protocol: 1 });
  } catch (_) {
    port = null;
    if (reconnectTimer === null) {
      reconnectTimer = setTimeout(() => { reconnectTimer = null; connect(); }, 2000);
    }
  }
}

async function stop(notifyDaemon = true) {
  if (!active) return;
  const old = active;
  active = null;
  if (notifyDaemon) send({ type: "disarm", protocol: 1 });
  try { await browser.tabs.sendMessage(old.tabId, { type: "myna.anchor.stop" }); } catch (_) { /* tab gone */ }
  try { await badge(old.tabId, "", "#555555"); } catch (_) { /* tab gone */ }
}

function sitePattern(urlString) {
  try {
    const url = new URL(urlString);
    if (url.protocol !== "https:" && url.protocol !== "http:") return null;
    // A site-specific optional grant; never request a wildcard domain.
    return `${url.protocol}//${url.hostname}/*`;
  } catch (_) { return null; }
}

async function arm(tab, automatic = false) {
  if (!Number.isInteger(tab?.id)) return false;
  connect();
  if (!port) return false;
  const sessionId = crypto.randomUUID();
  try {
    // Revoke an older session *before* installing the new DOM anchor: a late
    // "stop" message must never delete the newly captured caret.
    if (active) await stop(!automatic);
    await browser.scripting.executeScript({ target: { tabId: tab.id }, files: ["anchor.js", "rich.js", "content.js"] });
    const result = await browser.tabs.sendMessage(tab.id, { type: "myna.anchor.arm", session_id: sessionId });
    if (!result?.ok) throw new Error("No writable text field");
    // An explicit Firefox gesture pins the caret. Subsequent GNOME toggles
    // must not silently move it while the user edits another location.
    active = { tabId: tab.id, sessionId, pinned: !automatic };
    if (!send({ type: "arm", protocol: 1, session_id: sessionId })) throw new Error("Native host disconnected");
    await badge(tab.id, "ARM", "#367c55");
    return true;
  } catch (_) {
    await stop();
    // The automatic path must not reuse a stale target when the user moved
    // to an unsafe or non-editable field on the same site.
    if (automatic) send({ type: "disarm", protocol: 1 });
    await badge(tab.id, "ERR", "#b33838").catch(() => {});
    return false;
  }
}

async function prepare() {
  try {
    // The daemon already holds this manual arm token. Acknowledge the lease
    // without recapturing the cursor, even when Firefox lost focus.
    if (active?.pinned) return true;
    const window = await browser.windows.getLastFocused();
    if (!window?.focused || window.type !== "normal") return false;
    const [tab] = await browser.tabs.query({ active: true, windowId: window.id });
    const origin = sitePattern(tab?.url);
    if (!origin || !await browser.permissions.contains({ origins: [origin] })) {
      // Firefox is foreground but this page is not authorized. Do not leak
      // the preceding tab's anchor into a new dictation session.
      await stop();
      return false;
    }
    return await arm(tab, true);
  } catch (_) { return false; }
}

async function onNativeMessage(message) {
  if (message?.type === "prepare-cancel" && message.protocol === 1) {
    await stop();
    return;
  }
  if (message?.type === "prepare" && message.protocol === 1) {
    const ok = await prepare();
    send({ type: "prepare-result", protocol: 1, ok });
    return;
  }
  if (message?.type !== "commit" || !active || message.session_id !== active.sessionId || typeof message.text !== "string") return;
  const state = active;
  let ok = false;
  try {
    const response = await browser.tabs.sendMessage(state.tabId, {
      type: "myna.anchor.commit",
      session_id: state.sessionId,
      sequence: message.sequence,
      text: message.text,
    });
    ok = response?.ok === true;
  } catch (_) { /* tab closed or anchor no longer valid */ }
  send({ type: "commit-result", protocol: 1, session_id: state.sessionId, sequence: message.sequence, ok });
  if (!ok) await stop();
}

browser.action.onClicked.addListener(async (tab) => {
  if (!Number.isInteger(tab.id)) return;
  if (active) {
    const sameTab = active.tabId === tab.id;
    await stop();
    if (sameTab) return;
  }
  // Only a Firefox user gesture may request a site's permission.
  // The GNOME shortcut must never open a permission prompt.
  const origin = sitePattern(tab.url);
  if (origin) {
    try {
      if (!await browser.permissions.contains({ origins: [origin] })) {
        await browser.permissions.request({ origins: [origin] });
      }
    } catch (_) { /* explicit arm still works through activeTab */ }
  }
  await arm(tab);
});

browser.tabs.onRemoved.addListener((tabId) => { if (active?.tabId === tabId) void stop(); });
browser.tabs.onUpdated.addListener((tabId, info) => {
  if (active?.tabId === tabId && (info.status === "loading" || info.url)) void stop();
});

connect();
