const test = require("node:test");
const assert = require("node:assert/strict");
const { TextAnchor } = require("../anchor.js");

test("starts at a UTF-16 offset and appends after it", () => {
  const anchor = new TextAnchor("bonjour monde", 8);
  let actual = "bonjour monde";
  assert.equal(anchor.insert("cher ", (text, at) => {
    actual = actual.slice(0, at) + text + actual.slice(at);
    return true;
  }), true);
  assert.equal(actual, "bonjour cher monde");
  assert.equal(anchor.offset, 13);
});

test("moves an anchor when another caret inserts before it", () => {
  const anchor = new TextAnchor("abcdef", 4);
  assert.equal(anchor.observe("aXbcdef", { start: 1, end: 1, replacement: "X" }), true);
  assert.equal(anchor.offset, 5);
});

test("preserves an anchor when the user edits after it", () => {
  const anchor = new TextAnchor("abcdef", 2);
  assert.equal(anchor.observe("abcdeZf", { start: 5, end: 5, replacement: "Z" }), true);
  assert.equal(anchor.offset, 2);
});

test("moves right for insertion exactly at anchor (right affinity)", () => {
  const anchor = new TextAnchor("abc", 1);
  assert.equal(anchor.observe("aXbc", { start: 1, end: 1, replacement: "X" }), true);
  assert.equal(anchor.offset, 2);
});

test("rebases a replacement wholly before anchor", () => {
  const anchor = new TextAnchor("abXXdef", 5);
  assert.equal(anchor.observe("aXYZdef", { start: 1, end: 4, replacement: "XYZ" }), true);
  assert.equal(anchor.offset, 5);
});

test("rejects a user edit crossing the dictation anchor", () => {
  const anchor = new TextAnchor("abcdef", 3);
  assert.equal(anchor.observe("aXef", { start: 1, end: 4, replacement: "X" }), false);
  assert.equal(anchor.reason, "anchor-overwritten");
  assert.equal(anchor.insert("later", () => true), false);
});

test("rejects a mismatching beforeinput transaction", () => {
  const anchor = new TextAnchor("abcd", 3);
  assert.equal(anchor.observe("aXbcd", { start: 2, end: 2, replacement: "X" }), false);
  assert.equal(anchor.reason, "edit-mismatch");
});

test("rejects invalid coordinates", () => {
  const anchor = new TextAnchor("abc", 2);
  assert.equal(anchor.observe("abcd", { start: -1, end: 0, replacement: "d" }), false);
});

test("allows simple diff rebase when editing metadata is unavailable", () => {
  const anchor = new TextAnchor("cold sea", 8);
  assert.equal(anchor.observe("cold blue sea"), true);
  assert.equal(anchor.offset, 13);
});

test("fails closed for an ambiguous inferred edit", () => {
  const anchor = new TextAnchor("aaaBC", 5);
  assert.equal(anchor.observe("aaaaBC"), false);
  assert.equal(anchor.reason, "ambiguous-untracked-edit");
});

test("handles an emoji as two UTF-16 code units", () => {
  const anchor = new TextAnchor("a😀b", 3);
  assert.equal(anchor.observe("aZ😀b", { start: 1, end: 1, replacement: "Z" }), true);
  assert.equal(anchor.offset, 4);
});

test("rejects invalid initial offset", () => {
  assert.throws(() => new TextAnchor("abc", 4), TypeError);
  assert.throws(() => new TextAnchor("abc", 1.3), TypeError);
  assert.throws(() => new TextAnchor("a😀b", 2), TypeError);
});

test("rejects replacements beginning at the anchor and deleting following text", () => {
  const anchor = new TextAnchor("abcdef", 3);
  assert.equal(anchor.observe("abcXf", { start: 3, end: 5, replacement: "X" }), false);
});

test("does not advance if the DOM writer refuses the edit", () => {
  const anchor = new TextAnchor("abc", 1);
  assert.equal(anchor.insert("X", () => false), false);
  assert.equal(anchor.offset, 1);
  assert.equal(anchor.reason, "write-refused");
});

test("accepts empty insert without invoking the DOM writer", () => {
  const anchor = new TextAnchor("abc", 1);
  assert.equal(anchor.insert("", () => { throw new Error("must not write"); }), true);
});
