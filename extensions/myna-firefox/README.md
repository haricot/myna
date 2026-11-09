# Myna Firefox Anchor — opt-in native bridge (experimental)

This is an **unreleased, non-production experiment** for a Firefox-specific text
insertion target. It does not change Myna's existing IBus injector, does not use
AT-SPI. A Rust Native Messaging host and an opt-in `myna-desktop` socket
adapter now exist in source, but **they are not yet validated against a real
Firefox/Wayland session** and must not be enabled as a production default.

Firefox WebExtension activation is deliberately explicit: click the extension
button (or a user-assigned extension shortcut) with a normal writable `input` or
`textarea` selected. The extension receives a temporary `activeTab` grant,
captures the current text offset, and retains the document-position anchor while
the user edits elsewhere within the same field. It never reads the entire page,
reports field values to the native host, or requests `<all_urls>` permissions.

The native host name is `myna.firefox_anchor`. No system-wide service, port,
AT-SPI permission, or microphone privilege is required by the extension.

The content script intentionally supports ordinary text inputs only. It rejects
passwords, payment and one-time-code autocomplete fields, readonly/disabled
fields, non-collapsed selections, and rich `contenteditable` editors (which need
their own transaction adapters). It fails closed when a user edit overlaps the
anchor or an untracked edit is ambiguous. Myna segments use a session ID and an
ordered `sequence`; late or duplicate segments are refused.

The `TextAnchor` algorithm is separated so it can be tested without a desktop:

```sh
node --test extensions/myna-firefox/test/*.test.cjs
```

## Native bridge, for a disposable Firefox Nightly test profile only

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

In Firefox Nightly 162, open `about:debugging#/runtime/this-firefox`, choose
"Load Temporary Add-on", and select this directory's `manifest.json`. Focus a
normal textarea, position the caret, then activate the extension to arm
it. To avoid toolbar-induced focus changes, assign a keyboard shortcut in
Firefox's *Manage Extension Shortcuts* screen: the MV3 `_execute_action` command
is deliberately declared without a default key to avoid stealing another
shortcut. Use your normal Myna dictation shortcut. Subsequent committed segments
must be inserted at that anchor even if you have moved the caret. Click the
extension button again to disarm. Use a disposable Firefox profile with no
signed-in accounts for the first native tests.

**Do not merge this prototype** until the Firefox 162/Wayland hardware gate,
Rust tests, full repository checks, packaging and confinement review pass.

## Local protocol

Firefox starts a Native Messaging host via `browser.runtime.connectNative` and
sends `{ "type":"arm", "protocol":1, "session_id":"..." }` through a dedicated
`myna-firefox-host` process. This process validates the message and relays it
to the daemon over a user-only Unix socket. On a later dictation activation,
the daemon acquires that Firefox target instead of IBus. Each stable text
segment comes back as `{ "type":"commit", "session_id":"...", "sequence":0,
"text":"..." }`. The extension acknowledges with `{ "type":"commit-result",
"protocol":1, "session_id":"...", "sequence":0, "ok":true }`;
the daemon waits for that acknowledgement before sending another segment.
Long UTF-8 transcripts are split at Unicode character boundaries into segments
of at most 4096 bytes, with no data loss.
Failures revoke the current target. Only the currently armed field's value is
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
