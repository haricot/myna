/* An anchor uses DOM UTF-16 offsets, not AT-SPI Unicode character offsets. */
(function (root) {
  "use strict";

  class TextAnchor {
    constructor(value, offset) {
      if (typeof value !== "string" || !Number.isInteger(offset) || offset < 0 || offset > value.length) {
        throw new TypeError("Invalid text anchor");
      }
      if (offset > 0 && offset < value.length &&
          value.charCodeAt(offset - 1) >= 0xD800 && value.charCodeAt(offset - 1) <= 0xDBFF &&
          value.charCodeAt(offset) >= 0xDC00 && value.charCodeAt(offset) <= 0xDFFF) {
        throw new TypeError("Anchor splits a surrogate pair");
      }
      this.value = value;
      this.offset = offset;
      this.valid = true;
      this.reason = null;
    }

    invalidate(reason) {
      this.valid = false;
      this.reason = reason;
      return false;
    }

    // A page mutation is permitted only if it is wholly before or after the anchor.
    // An edit straddling the anchor cannot be rebased without guessing intent.
    observe(nextValue, expectedRange) {
      if (!this.valid || typeof nextValue !== "string") return false;
      if (nextValue === this.value) return true;

      const previous = this.value;
      let start;
      let oldEnd;
      let newEnd;

      if (expectedRange) {
        const { start: s, end: e, replacement } = expectedRange;
        if (!Number.isInteger(s) || !Number.isInteger(e) || s < 0 || e < s || e > previous.length || typeof replacement !== "string") {
          return this.invalidate("bad-edit-metadata");
        }
        if (nextValue !== previous.slice(0, s) + replacement + previous.slice(e)) {
          return this.invalidate("edit-mismatch");
        }
        start = s;
        oldEnd = e;
        newEnd = s + replacement.length;
      } else {
        // Programmatic changes without a corresponding beforeinput may be ambiguous.
        // Fall back only when the unchanged prefix/suffix uniquely bracket a change.
        start = 0;
        while (start < previous.length && start < nextValue.length && previous[start] === nextValue[start]) start++;
        let suffix = 0;
        while (suffix < previous.length - start && suffix < nextValue.length - start &&
               previous[previous.length - 1 - suffix] === nextValue[nextValue.length - 1 - suffix]) suffix++;
        oldEnd = previous.length - suffix;
        newEnd = nextValue.length - suffix;
        // A repeated character around an inferred edit could move its location.
        const inserted = nextValue.slice(start, newEnd);
        const removed = previous.slice(start, oldEnd);
        const sharedEdge = (segment) => segment.length > 0 &&
          ((start > 0 && previous[start - 1] === segment[0]) ||
           (oldEnd < previous.length && previous[oldEnd] === segment[segment.length - 1]));
        if (sharedEdge(inserted) || sharedEdge(removed)) {
          return this.invalidate("ambiguous-untracked-edit");
        }
      }

      if (start <= this.offset && oldEnd > this.offset) return this.invalidate("anchor-overwritten");
      if (oldEnd <= this.offset) this.offset += newEnd - oldEnd;
      this.value = nextValue;
      return true;
    }

    insert(text, write) {
      if (!this.valid || typeof text !== "string" || typeof write !== "function") return false;
      if (!text.length) return true;
      const before = this.value;
      const at = this.offset;
      if (!write(text, at)) return this.invalidate("write-refused");
      this.value = before.slice(0, at) + text + before.slice(at);
      this.offset = at + text.length;
      return true;
    }
  }

  root.MynaTextAnchor = TextAnchor;
  if (typeof module === "object" && module.exports) module.exports = { TextAnchor };
})(globalThis);
