# Myna Firefox Anchor — opt-in native bridge (experimental)

This is an **unreleased, non-production experiment** for a Firefox-specific text
insertion target. It does not change Myna's existing IBus injector, does not use
AT-SPI. A Rust Native Messaging host and an opt-in `myna-desktop` socket
adapter now exist in source, but **the textarea path has now been manually validated in Firefox on GNOME/Wayland**:
arming returned `ok: true`, dictation inserted into the selected text field,
and the anchor survived the field losing focus. This is not yet a complete
production compatibility or security gate.

Firefox WebExtension activation requires explicit site authorization for the
**unified GNOME shortcut**. Click the extension button with a writable `input`
or `textarea` selected to offer a narrowly scoped host permission for that
website. Firefox may show a permission prompt the first time; if the prompt
changes focus, focus the field again and use the extension shortcut to arm it.
The extension can still arm manually using a temporary `activeTab` grant
without a persistent site grant, but GNOME cannot auto-arm that site without
one. The permission covers only that site; the manifest declares optional
HTTP(S) patterns solely to permit per-site requests. No `<all_urls>` or
microphone permission is requested. No page content goes to the native host.

The native host name is `myna.firefox_anchor`. No system-wide service, port,
AT-SPI permission, or microphone privilege is required by the extension.
The extension maintains a native port while available and reconnects after a
temporary loss of the opt-in daemon, without touching the page on its own.
Clicking the Firefox action (or its optional Firefox shortcut) explicitly pins
the selected caret: subsequent GNOME activations must not recapture it while
the user edits elsewhere. Click again to disarm; navigation or an invalidated
field also revokes the anchor.

When the user's focused, collapsed caret happens to coincide with the pinned
anchor, dictation advances that visible caret just like ordinary typing. Once
the user moves the caret or selects text elsewhere, dictated insertions keep
the new caret or selection independent. An unfocused field's remembered
selection is not moved just because it happens to equal the anchor offset.

The content script intentionally supports ordinary text inputs only. It rejects
passwords, payment and one-time-code autocomplete fields, readonly/disabled
fields, non-collapsed selections, and rich `contenteditable` editors (which need
their own transaction adapters). It fails closed when a user edit overlaps the
anchor or an untracked edit is ambiguous. Myna segments use a session ID and an
ordered `sequence`; late or duplicate segments are refused.

Firefox `runtime.onMessage` listeners return a Promise for arm, stop and
commit responses; a plain object is silently delivered as `undefined` by
`tabs.sendMessage`, causing a misleading `ERR` / `No writable text field`
badge despite a valid focused textarea. The regression suite covers this
Firefox messaging contract as well as field edits and anchor safety.

The `TextAnchor` algorithm is separated so it can be tested without a desktop:

```sh
node --test extensions/myna-firefox/test/*.test.cjs
```

## Native bridge, for a disposable Firefox test profile only

From the repository root, after the Rust client build succeeds:

```sh
cd client
cargo build --bin myna-desktop --bin myna-firefox-host
./target/debug/myna-firefox-host --install
```

The user-level host manifest installed by the Rust library permits exactly
`firefox-anchor@myna.local`; remove **only this manifest** after testing with
`./target/debug/myna-firefox-host --uninstall`. Installation is an explicit
step, never performed by running the daemon or loading the extension.

Start your existing working `myna-desktop` command (same backend, audio and
shortcut settings as before) with `MYNA_FIREFOX_ANCHOR=1` in its environment.
Both processes must share an accessible `$XDG_RUNTIME_DIR`. A snap-confined
daemon generally has a different runtime directory, so this initial test is
for an unconfined developer run only. You can override the path **for both
processes** with `MYNA_FIREFOX_ANCHOR_SOCKET` in their environments. Use only
a private, user-owned runtime directory, never a public `/tmp` socket.

In Firefox, open `about:debugging#/runtime/this-firefox`, choose
"Load Temporary Add-on", and select this directory's `manifest.json`. Focus a
normal textarea, position the caret, then activate the extension to authorize
the site and arm it. To avoid toolbar-induced focus changes, assign a keyboard shortcut in
Firefox's *Manage Extension Shortcuts* screen: the MV3 `_execute_action` command
is deliberately declared without a default key to avoid stealing another
shortcut. Use your normal Myna dictation shortcut. Subsequent committed segments
must be inserted at that anchor even if you have moved the caret. Click the
extension button again to disarm. Use a disposable Firefox profile with no
signed-in accounts for the first native tests.

**Do not merge this prototype** until the remaining Firefox/Wayland hardware gate,
Rust tests, full repository checks, packaging and confinement review pass.

## Local protocol

Firefox starts a Native Messaging host via `browser.runtime.connectNative` and
sends `{ "type":"ready", "protocol":1 }` through a dedicated
`myna-firefox-host` process. This process validates bounded messages and relays
them over the private Unix socket. When Myna's global shortcut starts a session,
the daemon asks Firefox to prepare its injection target by sending
`{ "type":"prepare", "protocol":1 }`. If Firefox is foreground and the
current site is explicitly authorized, the extension captures the caret,
sends `{ "type":"arm", "protocol":1, "session_id":"..." }` and replies
`{ "type":"prepare-result", "protocol":1, "ok":true }`. The daemon waits
up to 750 ms before falling back to IBus. Each stable text
segment comes back as `{ "type":"commit", "session_id":"...", "sequence":0,
"text":"..." }`. The extension acknowledges with `{ "type":"commit-result",
"protocol":1, "session_id":"...", "sequence":0, "ok":true }`;
the daemon waits for that acknowledgement before sending another segment.
Long UTF-8 transcripts are split at Unicode character boundaries into segments
of at most 4096 bytes, with no data loss.
Disarming sends `{ "type":"disarm", "protocol":1 }` without closing the
background connection. Failures revoke the current target. Only the currently armed field's value is
used within Firefox; no field value travels to the native process.

Native Messaging framing uses the reviewed MIT-licensed Rust crate
[`native_messaging`](https://github.com/IberAI/native-messaging), not a custom
browser frame decoder. The local daemon protocol is bounded newline JSON.
The user-level manifest and socket permissions limit accidental exposure,
but they **do not cryptographically authenticate against malware running as
the same user**. A separate security review is required before enabling it
by default.

No test should launch a private D-Bus accessibility bus outside the repository's
`dev/nested-session.sh` wrapper. A real Firefox/Wayland acceptance gate must
verify DOM selection preservation, input event delivery, concurrent edits,
target closure and navigation, as well as compatibility with controlled web
editors before the production Myna injector is changed.

## Unified GNOME dictation shortcut (experimental)

Keep the existing GNOME shortcut (`<Super>t` on the user's workstation) as the
**only** primary dictation toggle. The extension does not need to discover or
copy that accelerator. `Ctrl+Alt+M` may remain the optional Firefox-specific
manual arm/disarm shortcut. Do not assign the GNOME key to both programs.

The automatic path is gated by `MYNA_FIREFOX_ANCHOR=1`, a live native port,
Firefox being the focused window, and a prior per-site grant. On other apps or
denied sites, Myna keeps its original IBus injector. Hermetic JavaScript tests
cover browser permission and focus behavior; the native protocol needs its
GNOME/Firefox/Wayland real-device timing and confinement acceptance gate before
being enabled by default.

A manually pinned field is acknowledged using its existing arm token, even
when Firefox is no longer foreground; the caret is not recaptured. An
automatically captured caret can still refresh on a later GNOME activation
when Firefox is foreground.
