/* An intentionally bounded DOM adapter for native, inline-rich contenteditable. */
(function (root) {
  "use strict";

  const MAX_TEXT = 100000;
  const MAX_NODES = 10000;
  const ALLOWED = new Set([
    "DIV", "P", "BR", "SPAN", "B", "STRONG", "I", "EM", "U", "S", "STRIKE",
    "CODE", "A", "MARK", "SUB", "SUP", "BLOCKQUOTE", "H1", "H2", "H3",
    "H4", "H5", "H6", "UL", "OL", "LI", "PRE",
  ]);
  const BLOCKS = new Set([
    "DIV", "P", "BLOCKQUOTE", "H1", "H2", "H3", "H4", "H5", "H6", "UL", "OL", "LI", "PRE",
  ]);

  function editingHost(element) {
    let host = null;
    for (let node = element; node && node.nodeType === 1; node = node.parentElement) {
      const attr = node.getAttribute?.("contenteditable");
      if (attr === "false") return null;
      if (attr === "" || attr === "true" || attr === "plaintext-only") host = node;
    }
    if (!host || !host.isConnected || host.getAttribute("aria-readonly") === "true" ||
        host.getAttribute("role") === "password" ||
        /password|one-time-code|cc-/.test(host.getAttribute("autocomplete") || "")) return null;
    return host;
  }

  // The snapshot is a *logical* UTF-16 stream. A <br> and a block boundary
  // each occupy one newline, whereas inline markup occupies no characters.
  // DOM boundary points map to exact offsets; ambiguous positions are retained
  // as alternatives rather than guessed.
  function scanRich(host, locations = []) {
    if (!host?.isConnected) return null;
    const offsets = locations.map(() => null);
    const points = [];
    const nodes = [];
    let value = "";

    function mark(node, offset) {
      for (let i = 0; i < locations.length; i++) {
        if (locations[i].node === node && locations[i].offset === offset) offsets[i] = value.length;
      }
      if (!points[value.length]) points[value.length] = [];
      points[value.length].push({ node, offset });
    }

    function walk(node) {
      if (nodes.length >= MAX_NODES) throw new Error("oversized-rich-dom");
      nodes.push(node);
      if (node.nodeType === 3) {
        for (let i = 0; i <= node.data.length; i++) {
          mark(node, i);
          if (i < node.data.length) {
            value += node.data[i];
            if (value.length > MAX_TEXT) throw new Error("oversized-rich-text");
          }
        }
        return;
      }
      if (node.nodeType !== 1 && node.nodeType !== 11) throw new Error("unsupported-rich-node");
      if (node.nodeType === 1) {
        if (!ALLOWED.has(node.tagName) ||
            (node !== host && node.getAttribute("contenteditable") !== null) ||
            node.getAttribute("aria-hidden") === "true") throw new Error("unsafe-rich-child");
        if (node.tagName === "BR") {
          value += "\n";
          return;
        }
      }
      const children = node.childNodes;
      for (let i = 0; i < children.length; i++) {
        if (i > 0 && (BLOCKS.has(children[i - 1].tagName) || BLOCKS.has(children[i].tagName))) {
          value += "\n";
        }
        if (value.length > MAX_TEXT) throw new Error("oversized-rich-text");
        mark(node, i);
        walk(children[i]);
      }
      mark(node, children.length);
    }

    try { walk(host); } catch (_) { return null; }
    return { value, offsets, points, nodes };
  }

  function sameNodes(left, right) {
    return left.length === right.length && left.every((node, index) => node === right[index]);
  }

  function belongsTo(host, node) {
    return node === host || host.contains(node);
  }

  function selectionInfo(host) {
    const selection = document.getSelection();
    if (!selection || selection.rangeCount !== 1) return null;
    const range = selection.getRangeAt(0);
    if (!belongsTo(host, range.startContainer) || !belongsTo(host, range.endContainer)) return null;
    const shot = scanRich(host, [
      { node: range.startContainer, offset: range.startOffset },
      { node: range.endContainer, offset: range.endOffset },
    ]);
    if (!shot || shot.offsets.some((n) => n === null)) return null;
    return { selection, range, start: shot.offsets[0], end: shot.offsets[1] };
  }

  function positionAt(shot, index) {
    const positions = shot.points[index] || [];
    const texts = positions.filter((point) => point.node.nodeType === 3);
    if (texts.length && texts.every((point) => point.node === texts[0].node)) return texts[0];
    return positions.length === 1 ? positions[0] : null;
  }

  class RichAnchor {
    constructor(host) {
      const selection = selectionInfo(host);
      if (!selection || selection.start !== selection.end) throw new Error("Select a collapsed rich caret");
      const shot = scanRich(host);
      if (!shot) throw new Error("Unsupported rich editor");
      this.host = host;
      this.anchor = new root.MynaTextAnchor(shot.value, selection.start);
      this.nodes = shot.nodes;
      this.pending = null;
      this.sending = false;
      this.bookmark = document.createRange();
      this.bookmark.setStart(selection.range.startContainer, selection.range.startOffset);
      this.bookmark.collapse(true);
    }

    safe() {
      return this.host.isConnected && editingHost(this.host) === this.host;
    }

    beforeInput(event) {
      this.pending = null;
      if (event.isComposing || !["insertText", "insertReplacementText"].includes(event.inputType) ||
          typeof event.data !== "string") return;
      const ranges = typeof event.getTargetRanges === "function" ? event.getTargetRanges() : [];
      const range = ranges.length === 1 ? ranges[0] : selectionInfo(this.host)?.range;
      if (!range) return;
      const shot = scanRich(this.host, [
        { node: range.startContainer, offset: range.startOffset },
        { node: range.endContainer, offset: range.endOffset },
      ]);
      if (!shot || shot.offsets.some((n) => n === null)) return;
      this.pending = { start: shot.offsets[0], end: shot.offsets[1], replacement: event.data };
    }

    observe(fromInput = false) {
      if (!this.safe()) return false;
      const shot = scanRich(this.host);
      if (!shot || (!fromInput && !sameNodes(this.nodes, shot.nodes))) return false;
      const pending = this.pending;
      this.pending = null;
      if (!this.anchor.observe(shot.value, pending)) return false;
      this.nodes = shot.nodes;
      return true;
    }

    insert(text) {
      if (!this.observe()) return false;
      if (!text.length) return true;
      return this.anchor.insert(text, (value, at) => {
        const shot = scanRich(this.host, [
          { node: this.bookmark.startContainer, offset: this.bookmark.startOffset },
        ]);
        if (!shot) return false;
        const previousPoint = shot.offsets[0] === at
          ? { node: this.bookmark.startContainer, offset: this.bookmark.startOffset } : null;
        const textPoint = positionAt(shot, at);
        // A caret at the boundary between two paragraphs belongs inside the
        // following paragraph, not as a new text node between block elements.
        const point = previousPoint?.node.nodeType === 3
          ? previousPoint : (textPoint || previousPoint);
        if (!point) return false;
        const current = selectionInfo(this.host);
        const focused = document.activeElement === this.host;
        const followCaret = focused && current?.start === at && current.end === at;
        const preserved = focused && current ? current.range.cloneRange() : null;
        const inserted = document.createTextNode(value);
        try {
          const range = document.createRange();
          range.setStart(point.node, point.offset);
          range.collapse(true);
          range.insertNode(inserted);
          const expected = shot.value.slice(0, at) + value + shot.value.slice(at);
          if (scanRich(this.host)?.value !== expected) {
            inserted.remove();
            return false;
          }
          const selection = document.getSelection();
          if (followCaret || preserved) {
            const replacement = document.createRange();
            if (followCaret) {
              replacement.setStart(inserted, inserted.data.length);
              replacement.collapse(true);
            }
            selection.removeAllRanges();
            selection.addRange(followCaret ? replacement : preserved);
          }
          this.sending = true;
          try {
            this.host.dispatchEvent(new InputEvent("input", {
              bubbles: true, composed: true, data: value, inputType: "insertText",
            }));
          } finally {
            this.sending = false;
          }
          const after = scanRich(this.host);
          if (!this.safe() || !inserted.isConnected || after?.value !== expected) return false;
          this.bookmark.setStart(inserted, inserted.data.length);
          this.bookmark.collapse(true);
          this.nodes = after.nodes;
          return true;
        } catch (_) {
          return false;
        }
      });
    }
  }

  root.MynaRichAnchor = RichAnchor;
  root.MynaRichEditingHost = editingHost;
  if (typeof module === "object" && module.exports) module.exports = { scanRich, editingHost, RichAnchor };
})(globalThis);
