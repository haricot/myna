const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const root = path.join(__dirname, "..");

function text(data) { return { nodeType: 3, data, childNodes: [], isConnected: true }; }
function element(tagName, ...childNodes) {
  const node = {
    nodeType: 1, tagName, childNodes, isConnected: true,
    getAttribute() { return null; },
  };
  for (const child of childNodes) { child.parentNode = node; child.parentElement = node; }
  return node;
}

function scan(...args) {
  const { scanRich } = require(path.join(root, "rich.js"));
  return scanRich(...args);
}

test("rich text index preserves paragraphs, inline spans and UTF-16 character offsets", () => {
  const a = text("Salut ");
  const b = text("🌍!");
  const c = text("Suite");
  const host = element("DIV", element("P", a, element("STRONG", b)), element("P", c));
  const shot = scan(host, [{ node: b, offset: 2 }]);
  assert.equal(shot.value, "Salut 🌍!\nSuite");
  assert.equal(shot.offsets[0], 8);
  assert.ok(shot.points[8].some((p) => p.node === b && p.offset === 2));
});

test("rich snapshot refuses embedded controls instead of writing into an ambiguous host", () => {
  const host = element("DIV", text("Bonjour"), element("INPUT"));
  assert.equal(scan(host), null);
});

test("rich snapshot refuses an oversized editor", () => {
  const host = element("DIV", text("a".repeat(120001)));
  assert.equal(scan(host), null);
});

const jsdomPath = process.env.MYNA_TEST_JSDOM;
let JSDOM = null;
try { JSDOM = require(jsdomPath || "jsdom").JSDOM; } catch (error) { if (jsdomPath) throw error; }

function browserHarness(markup, caretSelector = "[contenteditable]") {
  const dom = new JSDOM(`<!doctype html><body>${markup}</body>`, { url: "https://example.com", runScripts: "outside-only" });
  const { window } = dom;
  const { document } = window;
  let listener;
  window.browser = { runtime: { onMessage: { addListener(fn) { listener = fn; } } } };
  const editable = document.querySelector("[contenteditable]");
  editable.focus();
  const node = document.querySelector(caretSelector);
  const selection = document.getSelection();
  const range = document.createRange();
  if (node.nodeType === 3) range.setStart(node, 0);
  else if (node.firstChild?.nodeType === 3) range.setStart(node.firstChild, node.firstChild.data.length);
  else range.setStart(node, 0);
  range.collapse(true);
  selection.removeAllRanges(); selection.addRange(range);
  for (const file of ["anchor.js", "rich.js", "content.js"]) {
    window.eval(fs.readFileSync(path.join(root, file), "utf8"));
  }
  return { window, document, editable, selection, send: (message) => listener(message) };
}

test("editable rich paragraphs keep the dictation anchor while typing elsewhere", { skip: !JSDOM }, async () => {
  const h = browserHarness('<div contenteditable="true"><p>abc</p><p>def</p></div>', "p:last-child");
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "rich" })).ok, true);
  const first = h.editable.querySelector("p").firstChild;
  const r = h.document.createRange(); r.setStart(first, 1); r.collapse(true);
  h.selection.removeAllRanges(); h.selection.addRange(r);
  first.insertData(1, "X");
  h.editable.dispatchEvent(new h.window.InputEvent("input", { bubbles: true, inputType: "insertText", data: "X" }));
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "rich", sequence: 0, text: "!" })).ok, true);
  assert.equal(h.editable.querySelectorAll("p")[0].textContent, "aXbc");
  assert.equal(h.editable.querySelectorAll("p")[1].textContent, "def!");
  assert.equal(h.selection.anchorNode, first);
  assert.equal(h.selection.anchorOffset, 1);
});

test("a co-located rich caret follows consecutive dictated segments", { skip: !JSDOM }, async () => {
  const h = browserHarness('<div contenteditable="true"><p>Bonjour </p></div>', "p");
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "rich" })).ok, true);
  for (const [sequence, value] of [[0, "le"], [1, " monde"]]) {
    assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "rich", sequence, text: value })).ok, true);
  }
  assert.equal(h.editable.textContent, "Bonjour le monde");
  assert.equal(h.selection.anchorNode.textContent, " monde");
  assert.equal(h.selection.anchorOffset, 6);
});

test("dictated repeated letters do not get misread as user input", { skip: !JSDOM }, async () => {
  const h = browserHarness('<div contenteditable="true">aaaa</div>');
  const r = h.document.createRange(); r.setStart(h.editable.firstChild, 2); r.collapse(true);
  h.selection.removeAllRanges(); h.selection.addRange(r);
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "rich" })).ok, true);
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "rich", sequence: 0, text: "a" })).ok, true);
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "rich", sequence: 1, text: "a" })).ok, true);
  assert.equal(h.editable.textContent, "aaaaaa");
  assert.equal(h.selection.anchorOffset, 1);
});

test("exact beforeinput metadata keeps a user insertion at the rich anchor", { skip: !JSDOM }, async () => {
  const h = browserHarness('<div contenteditable="true">BonjourX</div>');
  const r = h.document.createRange(); r.setStart(h.editable.firstChild, 7); r.collapse(true);
  h.selection.removeAllRanges(); h.selection.addRange(r);
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "rich" })).ok, true);
  h.editable.dispatchEvent(new h.window.InputEvent("beforeinput", { bubbles: true, data: " ", inputType: "insertText" }));
  h.editable.firstChild.insertData(7, " ");
  h.editable.dispatchEvent(new h.window.InputEvent("input", { bubbles: true, data: " ", inputType: "insertText" }));
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "rich", sequence: 0, text: "monde" })).ok, true);
  assert.equal(h.editable.textContent, "Bonjour mondeX");
});

test("empty contenteditable can be pinned and filled", { skip: !JSDOM }, async () => {
  const h = browserHarness('<div contenteditable="true"></div>');
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "rich" })).ok, true);
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "rich", sequence: 0, text: "Bonjour" })).ok, true);
  assert.equal(h.editable.textContent, "Bonjour");
});

test("rich inline formatting stays intact across a dictation insert", { skip: !JSDOM }, async () => {
  const h = browserHarness('<div contenteditable="true"><p>Salut <strong>monde</strong> !</p></div>', "strong");
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "rich" })).ok, true);
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "rich", sequence: 0, text: " magnifique" })).ok, true);
  assert.equal(h.editable.querySelector("strong").textContent, "monde magnifique");
  assert.equal(h.editable.textContent, "Salut monde magnifique !");
});

test("a boundary caret before the second paragraph inserts inside that paragraph", { skip: !JSDOM }, async () => {
  const h = browserHarness('<div contenteditable="true"><p>abc</p><p>def</p></div>');
  const caret = h.document.createRange(); caret.setStart(h.editable, 1); caret.collapse(true);
  h.selection.removeAllRanges(); h.selection.addRange(caret);
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "rich" })).ok, true);
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "rich", sequence: 0, text: "X" })).ok, true);
  assert.equal(h.editable.querySelectorAll("p")[1].textContent, "Xdef");
  assert.equal(h.editable.children.length, 2);
});

test("an explicit line break is preserved as a logical newline", { skip: !JSDOM }, async () => {
  const h = browserHarness('<div contenteditable="true">Bonjour<br>monde</div>');
  const caret = h.document.createRange(); caret.setStart(h.editable, 2); caret.collapse(true);
  h.selection.removeAllRanges(); h.selection.addRange(caret);
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "rich" })).ok, true);
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "rich", sequence: 0, text: "X" })).ok, true);
  assert.equal(h.editable.textContent, "BonjourXmonde");
  assert.equal(h.editable.querySelectorAll("br").length, 1);
});

test("a rich selection elsewhere survives dictation without replacing its text", { skip: !JSDOM }, async () => {
  const h = browserHarness('<div contenteditable="true"><p>abc</p><p>def</p></div>', "p:last-child");
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "rich" })).ok, true);
  const first = h.editable.firstElementChild.firstChild;
  const r = h.document.createRange(); r.setStart(first, 0); r.setEnd(first, 2);
  h.selection.removeAllRanges(); h.selection.addRange(r);
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "rich", sequence: 0, text: "!" })).ok, true);
  assert.equal(h.selection.toString(), "ab");
  assert.equal(h.editable.textContent, "abcdef!");
});

test("a rich caret after the anchor keeps its original text location", { skip: !JSDOM }, async () => {
  const h = browserHarness('<div contenteditable="true">abcdef</div>');
  const first = h.editable.firstChild;
  const anchor = h.document.createRange(); anchor.setStart(first, 2); anchor.collapse(true);
  h.selection.removeAllRanges(); h.selection.addRange(anchor);
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "rich" })).ok, true);
  const caret = h.document.createRange(); caret.setStart(first, 5); caret.collapse(true);
  h.selection.removeAllRanges(); h.selection.addRange(caret);
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "rich", sequence: 0, text: "XX" })).ok, true);
  const { scanRich } = require(path.join(root, "rich.js"));
  const location = scanRich(h.editable, [{ node: h.selection.anchorNode, offset: h.selection.anchorOffset }]);
  assert.equal(location.offsets[0], 7);
  assert.equal(h.editable.textContent, "abXXcdef");
});

test("a rich selection overlapping the insert remains selected", { skip: !JSDOM }, async () => {
  const h = browserHarness('<div contenteditable="true">abcdef</div>');
  const first = h.editable.firstChild;
  const anchor = h.document.createRange(); anchor.setStart(first, 3); anchor.collapse(true);
  h.selection.removeAllRanges(); h.selection.addRange(anchor);
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "rich" })).ok, true);
  const selected = h.document.createRange(); selected.setStart(first, 2); selected.setEnd(first, 5);
  h.selection.removeAllRanges(); h.selection.addRange(selected);
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "rich", sequence: 0, text: "Y" })).ok, true);
  assert.equal(h.editable.textContent, "abcYdef");
  assert.equal(h.selection.toString(), "cYde");
});

test("rich input that overwrites the anchor invalidates the session", { skip: !JSDOM }, async () => {
  const h = browserHarness('<div contenteditable="true">abcdef</div>');
  const r = h.document.createRange(); r.setStart(h.editable.firstChild, 3); r.collapse(true);
  h.selection.removeAllRanges(); h.selection.addRange(r);
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "rich" })).ok, true);
  const edit = h.document.createRange(); edit.setStart(h.editable.firstChild, 1); edit.setEnd(h.editable.firstChild, 5);
  h.selection.removeAllRanges(); h.selection.addRange(edit);
  h.editable.dispatchEvent(new h.window.InputEvent("beforeinput", { bubbles: true, inputType: "insertText", data: "X" }));
  h.editable.firstChild.replaceData(1, 4, "X");
  h.editable.dispatchEvent(new h.window.InputEvent("input", { bubbles: true, inputType: "insertText", data: "X" }));
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "rich", sequence: 0, text: "Y" })).ok, false);
});

test("rich editor rerender without input fails closed", { skip: !JSDOM }, async () => {
  const h = browserHarness('<div contenteditable="true"><p>abc</p></div>', "p");
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "rich" })).ok, true);
  h.editable.firstElementChild.replaceWith(h.document.createElement("p"));
  h.editable.firstElementChild.textContent = "abc";
  assert.equal((await h.send({ type: "myna.anchor.commit", session_id: "rich", sequence: 0, text: "x" })).ok, false);
});

test("a rich editor containing noneditable controls is refused", { skip: !JSDOM }, async () => {
  const h = browserHarness('<div contenteditable="true">Bonjour <span contenteditable="false">bouton</span></div>');
  assert.equal((await h.send({ type: "myna.anchor.arm", session_id: "rich" })).ok, false);
});
