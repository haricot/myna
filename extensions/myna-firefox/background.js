"use strict";

// The optional `myna-firefox-host` relays bounded messages to the Rust daemon.
const HOST_NAME = "myna.firefox_anchor";
let active = null;

async function badge(tabId, label, color) {
  await browser.action.setBadgeText({ tabId, text: label });
  await browser.action.setBadgeBackgroundColor({ tabId, color });
}

async function stop() {
  if (!active) return;
  const old = active;
  active = null;
  try { old.port?.disconnect(); } catch (_) { /* already disconnected */ }
  try { await browser.tabs.sendMessage(old.tabId, { type: "myna.anchor.stop" }); } catch (_) { /* tab gone */ }
  try { await badge(old.tabId, "", "#555555"); } catch (_) { /* tab gone */ }
}

browser.action.onClicked.addListener(async (tab) => {
  if (!Number.isInteger(tab.id)) return;
  if (active) {
    const sameTab = active.tabId === tab.id;
    await stop();
    if (sameTab) return;
  }
  const sessionId = crypto.randomUUID();
  try {
    await browser.scripting.executeScript({ target: { tabId: tab.id }, files: ["anchor.js", "content.js"] });
    const result = await browser.tabs.sendMessage(tab.id, { type: "myna.anchor.arm", session_id: sessionId });
    if (!result?.ok) throw new Error(result?.error || "No writable text field");

    const state = { tabId: tab.id, sessionId, port: null };
    active = state;
    await badge(tab.id, "ARM", "#367c55");
    const port = browser.runtime.connectNative(HOST_NAME);
    state.port = port;
    port.onMessage.addListener(async (message) => {
      if (active !== state || message?.session_id !== state.sessionId) return;
      if (message.type !== "commit" || typeof message.text !== "string") return;
      // The session and monotonic sequence are checked again inside the tab.
      try {
        const response = await browser.tabs.sendMessage(state.tabId, {
          type: "myna.anchor.commit",
          session_id: state.sessionId,
          sequence: message.sequence,
          text: message.text
        });
        port.postMessage({ type: "commit-result", protocol: 1, session_id: state.sessionId, sequence: message.sequence, ok: response?.ok === true });
        if (!response?.ok) await stop();
      } catch (_) {
        await stop();
      }
    });
    port.onDisconnect.addListener(() => { if (active === state) void stop(); });
    port.postMessage({ type: "arm", protocol: 1, session_id: sessionId });
  } catch (_) {
    await stop();
    await badge(tab.id, "ERR", "#b33838");
  }
});

browser.tabs.onRemoved.addListener((tabId) => { if (active?.tabId === tabId) void stop(); });
browser.tabs.onUpdated.addListener((tabId, info) => {
  if (active?.tabId === tabId && (info.status === "loading" || info.url)) void stop();
});
