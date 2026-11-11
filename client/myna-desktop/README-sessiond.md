# Experimental shared Myna session service

`myna-sessiond` is a deliberately opt-in companion to `myna-desktop`. It
reuses the tested `myna-orchestrator::run_dictation` path from `myna-testbed`,
the PipeWire capture source and the existing session controller. No inference
model is loaded in the Firefox extension or in `myna-egui`.

## Try it on Linux

First stop the running `myna-desktop` user service (do not disable it
permanently). **Never run both session owners together.** The new binary
refuses to bind the existing Myna control socket, rather than replacing it.

```sh
cd client
cargo run -p myna-desktop --bin myna-sessiond -- \
    --socket /run/user/1000/myna-candle/asr.sock --language fr
```

Replace the example socket path with your actual provider's socket. In
another terminal, after explicitly arming a field through the existing
Firefox extension or `myna-egui`:

```sh
cd client
cargo run -p myna-desktop --bin myna-sessiond -- --toggle  # start
# speak
cargo run -p myna-desktop --bin myna-sessiond -- --toggle  # finish
```

The shortcut that already runs `myna-desktop --toggle` can continue to do so:
both binaries use `$XDG_RUNTIME_DIR/myna-desktop.sock`. `myna-sessiond`
deliberately serves **no D-Bus status interface or IBus fallback** yet;
the GNOME status extension therefore still requires the established daemon.
For a real test, use the compiled `myna-sessiond --toggle` command instead
of paying a Cargo build on each activation.

Native anchor endpoints stay unchanged:

- Firefox native host: `$XDG_RUNTIME_DIR/myna-firefox-anchor.sock`.
- egui client: `$XDG_RUNTIME_DIR/myna-egui-anchor.sock`.

The browser extension's DOM anchor and native-messaging protocol v1 have not
changed. The egui `arm`/`commit`/`commit-result` protocol v1 has not changed.
An explicitly armed egui client is selected first; Firefox is selected when
no egui target is armed. If neither has been authorized, no capture starts.

## Trust boundary

- Unix domain sockets only; no TCP listening ports, no AT-SPI or synthetic
  keystrokes. Each socket is restricted to the current user (`0600`) inside
  a private runtime directory (`0700`). The accepted peer UID is checked.
- The control socket accepts only the six-byte `toggle` request and is
  bounded by a short timeout. Existing sockets are never unlinked on bind.
- Transcripts are never published on the status channel or logged by the
  service. Stable text is sent only to the acquired target and acknowledged.
- This is a **same-user trust boundary**, not a sandbox from malicious code
  already executing as the logged-in user. A per-app capability/approval broker
  is future work before exposing this to arbitrary third-party applications.

This prototype is not yet a drop-in replacement for the packaged
`myna-desktop` daemon: service startup, HUD status, safe daemon switching,
CI acceptance, and system testing under the user's Arch/GNOME/Wayland session
remain to be completed before making it the default.
