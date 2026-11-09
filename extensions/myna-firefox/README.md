# Myna Firefox Anchor — feasibility stage

This is an **unreleased, non-production experiment** for a Firefox-specific text
insertion target. It does not change Myna's existing IBus injector, does not use
AT-SPI, and does not yet provide a native host connected to Myna's Rust client.

Firefox WebExtension activation is deliberately explicit: click the extension
button (or a user-assigned extension shortcut) with a normal writable `input` or
`textarea` selected. The extension receives a temporary `activeTab` grant,
captures the current text offset, and retains the document-position anchor while
the user edits elsewhere within the same field. It never reads the entire page,
reports field values to the native host, or requests `<all_urls>` permissions.

The native host name is `myna.firefox_anchor`; it is **not installed by this
spike**. Until an approved native Myna adapter is implemented, Firefox reports a
missing native host and cannot perform real dictation. Do not interpret a loadable
extension or green model tests as end-to-end success.

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

## Intended local protocol (not connected to Myna yet)

Firefox starts a Native Messaging host via `browser.runtime.connectNative` and
sends `{ "type":"arm", "protocol":1, "session_id":"..." }`. The host may
reply `{ "type":"commit", "session_id":"...", "sequence":0, "text":"..." }`.
The extension acknowledges with `commit-result` and `ok`; an invalid write
ends the session. Native Messaging frames are JSON over Firefox-managed stdio,
not a TCP/HTTP listener. The eventual Rust host manifest must allow **only**
`firefox-anchor@myna.local` and must reject untrusted or oversized payloads.

No test should launch a private D-Bus accessibility bus outside the repository's
`dev/nested-session.sh` wrapper. A real Firefox/Wayland acceptance gate must
verify DOM selection preservation, input event delivery, concurrent edits,
target closure and navigation, as well as compatibility with controlled web
editors before the production Myna injector is changed.
