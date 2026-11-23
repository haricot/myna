const test = require("node:test");
const assert = require("node:assert/strict");
const vm = require("node:vm");
const fs = require("node:fs");
const path = require("node:path");

const source = fs.readFileSync(path.join(__dirname, "..", "background.js"), "utf8");
const flush = () => new Promise((resolve) => setImmediate(resolve));

function harness({ focused = true, permission = true, armOk = true } = {}) {
  let granted = permission;
  const events = {};
  const outbound = [];
  const injections = [];
  const requests = [];
  const badges = [];
  const tabs = [{ id: 7, windowId: 2, url: "https://example.org/editor" }];
  const port = {
    onMessage: { addListener(fn) { events.nativeMessage = fn; } },
    onDisconnect: { addListener(fn) { events.nativeDisconnect = fn; } },
    postMessage(message) { outbound.push(message); },
    disconnect() {},
  };
  const browser = {
    runtime: { connectNative() { return port; } },
    action: {
      onClicked: { addListener(fn) { events.clicked = fn; } },
      async setBadgeText(info) { badges.push(info); },
      async setBadgeBackgroundColor() {},
    },
    scripting: { async executeScript(info) { injections.push(info); } },
    windows: { async getLastFocused() { return { id: 2, focused, type: "normal" }; } },
    tabs: {
      onUpdated: { addListener(fn) { events.updated = fn; } },
      onRemoved: { addListener(fn) { events.removed = fn; } },
      async query() { return tabs; },
      async sendMessage(_tabId, message) {
        if (message.type === "myna.anchor.arm") return { ok: armOk };
        return { ok: true };
      },
    },
    permissions: {
      async contains() { return granted; },
      async request(info) { requests.push(info); granted = true; return true; },
    },
  };
  vm.runInNewContext(source, { browser, URL, crypto: { randomUUID: () => "badc0ffe-1234-4321-abcd-000000000000" }, setTimeout() { return 1; }, clearTimeout() {} });
  return { events, outbound, injections, requests, badges, tabs };
}

test("background advertises readiness without an active tab permission", () => {
  const h = harness();
  assert.equal(h.outbound[0]?.type, "ready");
  assert.equal(h.injections.length, 0);
});

test("a Myna prepare arms the focused, already-authorized Firefox field", async () => {
  const h = harness();
  h.events.nativeMessage({ type: "prepare", protocol: 1 });
  await flush();
  assert.equal(h.injections.length, 1);
  assert.deepEqual(Array.from(h.injections[0].files), ["anchor.js", "rich.js", "content.js"]);
  assert.deepEqual(h.outbound.map((m) => m.type), ["ready", "arm", "prepare-result"]);
  assert.equal(h.outbound.at(-1).ok, true);
});

test("a manually pinned anchor is not replaced by a Myna activation", async () => {
  const h = harness();
  await h.events.clicked(h.tabs[0]);
  h.events.nativeMessage({ type: "prepare", protocol: 1 });
  await flush();
  h.events.nativeMessage({ type: "prepare", protocol: 1 });
  await flush();
  assert.deepEqual(h.outbound.map((m) => m.type), ["ready", "arm", "prepare-result", "prepare-result"]);
  assert.equal(h.injections.length, 1);
  assert.equal(h.outbound.at(-1).ok, true);
});

test("a manually pinned anchor survives Firefox losing foreground focus", async () => {
  const h = harness({ focused: false });
  await h.events.clicked(h.tabs[0]);
  h.events.nativeMessage({ type: "prepare", protocol: 1 });
  await flush();
  assert.deepEqual(h.outbound.map((m) => m.type), ["ready", "arm", "prepare-result"]);
  assert.equal(h.outbound.at(-1).ok, true);
});

test("a new GNOME activation can refresh an automatically captured anchor", async () => {
  const h = harness();
  h.events.nativeMessage({ type: "prepare", protocol: 1 });
  await flush();
  h.events.nativeMessage({ type: "prepare", protocol: 1 });
  await flush();
  assert.deepEqual(h.outbound.map((m) => m.type), ["ready", "arm", "prepare-result", "arm", "prepare-result"]);
  assert.equal(h.injections.length, 2);
});

test("a prepared session routes transcription only to its matching session", async () => {
  const h = harness();
  h.events.nativeMessage({ type: "prepare", protocol: 1 });
  await flush();
  h.events.nativeMessage({ type: "commit", session_id: "badc0ffe-1234-4321-abcd-000000000000", sequence: 0, text: "bonjour" });
  await flush();
  assert.equal(h.outbound.at(-1).type, "commit-result");
  assert.equal(h.outbound.at(-1).ok, true);
  const count = h.outbound.length;
  h.events.nativeMessage({ type: "commit", session_id: "stale-session", sequence: 1, text: "ignored" });
  await flush();
  assert.equal(h.outbound.length, count);
});

test("a global trigger never requests website permissions without a Firefox gesture", async () => {
  const h = harness({ permission: false });
  h.events.nativeMessage({ type: "prepare", protocol: 1 });
  await flush();
  assert.equal(h.outbound.at(-1).ok, false);
  assert.equal(h.requests.length, 0);
  assert.equal(h.injections.length, 0);
});

test("a global trigger ignores Firefox when its window is not foreground", async () => {
  const h = harness({ focused: false });
  h.events.nativeMessage({ type: "prepare", protocol: 1 });
  await flush();
  assert.equal(h.outbound.at(-1).ok, false);
  assert.equal(h.injections.length, 0);
});

test("a late preparation cancelled by Myna revokes its DOM anchor", async () => {
  const h = harness();
  h.events.nativeMessage({ type: "prepare", protocol: 1 });
  await flush();
  h.events.nativeMessage({ type: "prepare-cancel", protocol: 1 });
  await flush();
  assert.equal(h.outbound.at(-1).type, "disarm");
});

test("the Firefox toolbar gesture requests only the current site and preserves the bridge", async () => {
  const h = harness({ permission: false });
  await h.events.clicked(h.tabs[0]);
  assert.equal(h.requests.length, 1);
  assert.equal(h.requests[0].origins[0], "https://example.org/*");
  assert.equal(h.outbound.at(-1).type, "arm");
  // Once authorized, a later GNOME activation can prepare without a prompt.
  await h.events.clicked(h.tabs[0]);
  h.events.nativeMessage({ type: "prepare", protocol: 1 });
  await flush();
  assert.equal(h.outbound.at(-1).type, "prepare-result");
  assert.equal(h.outbound.at(-1).ok, true);
  assert.equal(h.requests.length, 1);
});
