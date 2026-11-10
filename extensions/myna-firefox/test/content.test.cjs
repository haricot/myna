const test = require("node:test");
const assert = require("node:assert/strict");
const vm = require("node:vm");
const fs = require("node:fs");
const path = require("node:path");

const root = path.join(__dirname, "..");

class Field {
  constructor(value = "abcdef", offset = 4) {
    this.value = value;
    this.selectionStart = offset;
    this.selectionEnd = offset;
    this.isConnected = true;
    this.disabled = false;
    this.readOnly = false;
    this.autocomplete = "off";
    this.listeners = new Map();
  }
  addEventListener(type, callback) {
    if (!this.listeners.has(type)) this.listeners.set(type, new Set());
    this.listeners.get(type).add(callback);
  }
  removeEventListener(type, callback) { this.listeners.get(type)?.delete(callback); }
  dispatchEvent(event) {
    event.target = this;
    for (const fn of this.listeners.get(event.type) || []) fn(event);
    return true;
  }
  setRangeText(text, start, end, mode) {
    assert.equal(mode, "preserve");
    this.value = this.value.slice(0, start) + text + this.value.slice(end);
    const delta = text.length - (end - start);
    if (this.selectionStart > end) this.selectionStart += delta;
    if (this.selectionEnd > end) this.selectionEnd += delta;
  }
  type(text) {
    this.dispatchEvent({ type: "beforeinput", inputType: "insertText", data: text, isComposing: false });
    this.setRangeText(text, this.selectionStart, this.selectionEnd, "preserve");
    this.selectionStart += text.length;
    this.selectionEnd = this.selectionStart;
    this.dispatchEvent({ type: "input", inputType: "insertText", data: text });
  }
}
class Textarea extends Field {}
class Input extends Field { constructor(value, offset, type = "text") { super(value, offset); this.type = type; } }
class FakeInputEvent { constructor(type, detail) { this.type = type; Object.assign(this, detail); } }

function createHarness(field) {
  let listener;
  const browser = { runtime: { onMessage: { addListener(fn) { listener = fn; } } } };
  const context = vm.createContext({
    browser,
    document: { activeElement: field },
    HTMLTextAreaElement: Textarea,
    HTMLInputElement: Input,
    InputEvent: FakeInputEvent,
    addEventListener() {},
    Set,
  });
  vm.runInContext(fs.readFileSync(path.join(root, "anchor.js"), "utf8"), context);
  vm.runInContext(fs.readFileSync(path.join(root, "content.js"), "utf8"), context);
  return { field, send: (msg) => listener(msg) };
}

test("captures a textarea and commits after user moves caret and edits before anchor", async () => {
  const h = createHarness(new Textarea("abcdef", 4));
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "one" })).ok, true);
  h.field.selectionStart = 1;
  h.field.selectionEnd = 1;
  h.field.type("X");
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "one", sequence: 0, text: "YY" })).ok, true);
  assert.equal(h.field.value, "aXbcdYYef");
  assert.equal(h.field.selectionStart, 2);
  assert.equal(h.field.selectionEnd, 2);
});

test("refuses a protected input and never arms it", async () => {
  const h = createHarness(new Input("secret", 3, "password"));
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "a" })).ok, false);
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "a", sequence: 0, text: "x" })).ok, false);
  assert.equal(h.field.value, "secret");
});

test("refuses stale, duplicate, or out-of-order segments", async () => {
  const h = createHarness(new Textarea("abcdef", 4));
  await h.send({ type: "myna.anchor.arm", session_id: "new" });
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "old", sequence: 0, text: "x" })).ok, false);
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "new", sequence: 1, text: "x" })).ok, false);
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "new", sequence: 0, text: "X" })).ok, true);
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "new", sequence: 0, text: "X" })).ok, false);
});

test("revokes the anchor when another edit crosses it", async () => {
  const h = createHarness(new Textarea("abcdef", 3));
  await h.send({ type: "myna.anchor.arm", session_id: "one" });
  h.field.selectionStart = 1;
  h.field.selectionEnd = 5;
  h.field.type("X");
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "one", sequence: 0, text: "YY" })).ok, false);
});

test("rejects writes after field has been removed", async () => {
  const h = createHarness(new Textarea("abcdef", 3));
  await h.send({ type: "myna.anchor.arm", session_id: "one" });
  h.field.isConnected = false;
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "one", sequence: 0, text: "X" })).ok, false);
});

test("consecutive dictated repeats keep a live anchor without double rebasing", async () => {
  const h = createHarness(new Textarea("aaaa", 2));
  await h.send({ type: "myna.anchor.arm", session_id: "one" });
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "one", sequence: 0, text: "a" })).ok, true);
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "one", sequence: 1, text: "a" })).ok, true);
  assert.equal(h.field.value, "aaaaaa");
});

test("a moved selection stays intact when dictation commits at its independent anchor", async () => {
  const h = createHarness(new Textarea("bonjour monde", 8));
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "one" })).ok, true);
  h.field.selectionStart = 0;
  h.field.selectionEnd = 7;
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "one", sequence: 0, text: "cher " })).ok, true);
  assert.equal(h.field.value, "bonjour cher monde");
  assert.equal(h.field.selectionStart, 0);
  assert.equal(h.field.selectionEnd, 7);
});

test("independent user edits remain stable across multiple dictation commits", async () => {
  const h = createHarness(new Textarea("abcdEFGH", 4));
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "one" })).ok, true);
  h.field.selectionStart = 1;
  h.field.selectionEnd = 1;
  h.field.type("X");
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "one", sequence: 0, text: " bon" })).ok, true);
  h.field.selectionStart = h.field.value.length;
  h.field.selectionEnd = h.field.value.length;
  h.field.type("!");
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "one", sequence: 1, text: "jour" })).ok, true);
  assert.equal(h.field.value, "aXbcd bonjourEFGH!");
  assert.equal(h.field.selectionStart, h.field.value.length);
});

test("contenteditable and non-collapsed selections are deliberately unsupported", async () => {
  const h = createHarness({ isContentEditable: true, isConnected: true });
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "one" })).ok, false);
  const field = new Textarea("abcdefgh", 2);
  field.selectionEnd = 6;
  const selected = createHarness(field);
  assert.equal((await selected.send({ type: "myna.anchor.arm", session_id: "one" })).ok, false);
});

test("a field that becomes readonly fails closed", async () => {
  const h = createHarness(new Textarea("abcdef", 3));
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "one" })).ok, true);
  h.field.readOnly = true;
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "one", sequence: 0, text: "X" })).ok, false);
  assert.equal(h.field.value, "abcdef");
});

test("Firefox runtime replies with Promises and ignores unrelated messages", async () => {
  const h = createHarness(new Textarea("bonjour", 3));
  const arm = h.send({ type: "myna.anchor.arm", session_id: "one" });
  assert.equal(typeof arm.then, "function");
  assert.equal((await arm).ok, true);
  const commit = h.send({ type: "myna.anchor.commit", session_id: "one", sequence: 0, text: "X" });
  assert.equal(typeof commit.then, "function");
  assert.equal((await commit).ok, true);
  const stop = h.send({ type: "myna.anchor.stop" });
  assert.equal(typeof stop.then, "function");
  assert.equal((await stop).ok, true);
  assert.equal(h.send({ type: "unrelated.message" }), false);
  assert.equal(h.send(null), false);
});
