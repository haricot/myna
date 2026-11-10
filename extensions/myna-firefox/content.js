(() => {
  "use strict";
  if (globalThis.__mynaFirefoxAnchorInstalled) return;
  globalThis.__mynaFirefoxAnchorInstalled = true;

  let session = null;

  function safeField(element) {
    if (!element || element.disabled || element.readOnly || !element.isConnected) return false;
    if (element instanceof HTMLTextAreaElement) return true;
    if (!(element instanceof HTMLInputElement)) return false;
    if (!new Set(["text", "search", "url", "tel", "email"]).has(element.type)) return false;
    if (/password|one-time-code|cc-/.test(element.autocomplete || "")) return false;
    return true;
  }

  function end() {
    if (session) {
      session.element.removeEventListener("beforeinput", onBeforeInput, true);
      session.element.removeEventListener("input", onInput, true);
      session = null;
    }
  }

  function onBeforeInput(event) {
    if (!session || session.sending || event.target !== session.element) return;
    const el = session.element;
    const start = el.selectionStart;
    const end = el.selectionEnd;
    // For an insertion with actual supplied data, we can verify the exact edit.
    // For deletion, paste with hidden payload, and IME composition, diff conservatively.
    if (!event.isComposing && (event.inputType === "insertText" || event.inputType === "insertReplacementText") &&
        typeof event.data === "string") {
      session.pending = { start, end, replacement: event.data };
    } else {
      session.pending = null;
    }
  }

  function onInput(event) {
    if (!session || session.sending || event.target !== session.element) return;
    const next = session.element.value;
    const pending = session.pending;
    session.pending = null;
    if (!session.anchor.observe(next, pending)) end();
  }

  function arm(sessionId) {
    end();
    const element = document.activeElement;
    if (!safeField(element) || !Number.isInteger(element.selectionStart) || element.selectionStart !== element.selectionEnd) {
      return { ok: false, error: "Select one writable text input and position a collapsed caret" };
    }
    const anchor = new globalThis.MynaTextAnchor(element.value, element.selectionStart);
    session = { id: sessionId, element, anchor, nextSequence: 0, pending: null, sending: false };
    element.addEventListener("beforeinput", onBeforeInput, true);
    element.addEventListener("input", onInput, true);
    return { ok: true };
  }

  function insert(message) {
    if (!session || message.session_id !== session.id || message.sequence !== session.nextSequence) {
      return { ok: false, error: "session-or-sequence-mismatch" };
    }
    if (typeof message.text !== "string" || message.text.length > 4096 || !safeField(session.element)) {
      end();
      return { ok: false, error: "unsafe-target-or-payload" };
    }
    const el = session.element;
    if (!session.anchor.observe(el.value, session.pending)) {
      end();
      return { ok: false, error: "anchor-invalidated" };
    }
    session.pending = null;
    const succeeded = session.anchor.insert(message.text, (text, at) => {
      try {
        const originalStart = el.selectionStart;
        const originalEnd = el.selectionEnd;
        el.setRangeText(text, at, at, "preserve");
        if (el.value !== session.anchor.value.slice(0, at) + text + session.anchor.value.slice(at)) return false;
        // Notify page frameworks, without claiming universal compatibility.
        session.sending = true;
        try {
          el.dispatchEvent(new InputEvent("input", { bubbles: true, composed: true, data: text, inputType: "insertText" }));
        } finally {
          session.sending = false;
        }
        // "preserve" must keep a caret before the insertion from moving.
        if (originalEnd <= at && (el.selectionStart !== originalStart || el.selectionEnd !== originalEnd)) return false;
        return true;
      } catch (_) {
        return false;
      }
    });
    if (!succeeded) {
      end();
      return { ok: false, error: "write-or-caret-failed" };
    }
    session.nextSequence++;
    return { ok: true, sequence: message.sequence };
  }

  // Firefox's runtime.onMessage only delivers a response when the listener
  // returns a Promise or calls sendResponse; returning a plain object results
  // in tabs.sendMessage resolving to undefined in the background script.
  browser.runtime.onMessage.addListener((message) => {
    if (!message || typeof message.type !== "string") return false;
    if (message.type === "myna.anchor.arm") return Promise.resolve(arm(message.session_id));
    if (message.type === "myna.anchor.stop") {
      end();
      return Promise.resolve({ ok: true });
    }
    if (message.type === "myna.anchor.commit") return Promise.resolve(insert(message));
    return false;
  });
  addEventListener("pagehide", end);
})();
