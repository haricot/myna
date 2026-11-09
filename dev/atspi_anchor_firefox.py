#!/usr/bin/env python3
"""Automated *live* Firefox/AT-SPI anchor feasibility test.

Starts the explicitly selected Firefox with geckodriver's disposable profile,
focuses an isolated textarea, types a character at its start through WebDriver,
then tries inserting at the original (rebased) AT-SPI anchor. The browser is
driven in the existing desktop session; no nested GNOME Shell or private D-Bus
session is started. Firefox must be visible (never run this headless).

Run with the system Python (pyatspi must be importable):

    /usr/bin/python3 dev/atspi_anchor_firefox.py --firefox /path/to/firefox

Requires geckodriver in PATH (on Arch/Manjaro: sudo pacman -S geckodriver).
Only a generated scratch field is ever modified. User profiles are not used.
"""

import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import time
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import ProxyHandler, Request, build_opener

from atspi_anchor_probe import (
    ProbeError,
    _find_focused,
    _is_focused,
    _snapshot,
    inserted_exactly,
    rebase_anchor,
)


INITIAL_TEXT = "Bonjour monde"
ANCHOR_OFFSET = 8
PREFIX = "X"
INSERTION = " [MYNA PROBE]"
SCRATCH_HTML = (
    "<!doctype html><html lang='fr'><meta charset='utf-8'>"
    "<title>MYNA ANCHOR AUTOMATED TEST</title>"
    "<label for='myna-anchor-test'>Champ provisoire</label>"
    f"<textarea id='myna-anchor-test' rows='12' cols='55'>{INITIAL_TEXT}</textarea>"
    "</html>"
)
ELEMENT_KEY = "element-6066-11e4-a52e-4f735466cecf"


@dataclass(frozen=True)
class AnchorResult:
    original_anchor: int
    rebased_anchor: int
    editing_caret: int
    editing_caret_after: int


def firefox_capabilities(firefox_path: str, *, wayland: bool) -> dict:
    env = {"MOZ_ACCESSIBILITY_ATK2": "1"}
    if wayland:
        env["MOZ_ENABLE_WAYLAND"] = "1"
    return {
        "capabilities": {
            "alwaysMatch": {
                "browserName": "firefox",
                "moz:firefoxOptions": {
                    "binary": firefox_path,
                    "env": env,
                    "prefs": {"accessibility.force_disabled": 0},
                },
            }
        }
    }


def _wait_until(predicate, *, timeout: float, description: str):
    deadline = time.monotonic() + timeout
    last_detail = ""
    while True:
        try:
            result = predicate()
            if result:
                return result
        except ProbeError as exc:
            last_detail = str(exc)
        if time.monotonic() >= deadline:
            detail = f" ({last_detail})" if last_detail else ""
            raise ProbeError(f"timeout awaiting {description}{detail}")
        time.sleep(0.15)


def _assert_our_window_focused(state: dict) -> bool:
    return bool(state.get("active") and state.get("windowFocused"))


def run_anchor_scenario(browser, pyatspi, *, timeout: float = 20.0) -> AnchorResult:
    """Drive the disposable Firefox field, checking both WebDriver and AT-SPI.

    The original accessible target must remain focused until the insertion.
    No write is attempted unless the test document and caret match exactly.
    """
    browser.focus_at(ANCHOR_OFFSET)

    def acquire():
        state = browser.state()
        if not _assert_our_window_focused(state):
            raise ProbeError(
                f"Firefox window/textarea has no focus "
                f"(window={bool(state.get('windowFocused'))}, "
                f"field={bool(state.get('active'))})"
            )
        if state.get("value") != INITIAL_TEXT:
            raise ProbeError("scratch field did not contain the expected initial text")
        obj, editable, text = _find_focused(pyatspi)
        if _snapshot(text, 10_000) != INITIAL_TEXT:
            raise ProbeError("AT-SPI focus is not on the scratch Firefox field")
        if text.caretOffset != ANCHOR_OFFSET:
            raise ProbeError("AT-SPI caret does not match Firefox's initial caret")
        return obj, editable, text

    obj, editable, text = _wait_until(
        acquire, timeout=timeout, description="Firefox textarea AT-SPI focus"
    )
    before = _snapshot(text, 10_000)
    original_anchor = text.caretOffset

    # User editing is simulated by WebDriver key events, not by assigning
    # textarea.value in JavaScript, so Firefox exercises its real editing path.
    browser.type_at(0, PREFIX)
    expected_current = PREFIX + INITIAL_TEXT

    def edited():
        state = browser.state()
        if not _assert_our_window_focused(state):
            raise ProbeError("Firefox focus was lost during the editing step")
        if state.get("value") != expected_current or state.get("caret") != len(PREFIX):
            raise ProbeError(
                "WebDriver edit or editing-caret move is not yet observable"
            )
        if not _is_focused(pyatspi, obj) or _snapshot(text, 10_000) != expected_current:
            raise ProbeError("AT-SPI target lost focus or has not observed the edit")
        if text.caretOffset != len(PREFIX):
            raise ProbeError("AT-SPI editing caret differs from Firefox's caret")
        return True

    _wait_until(edited, timeout=timeout, description="focused edited Firefox text")
    rebased = rebase_anchor(before, expected_current, original_anchor)
    editing_caret = text.caretOffset

    # Fail closed on document/focus drift. No other field is ever eligible for
    # insertion; this uses the exact accessible object captured above.
    state = browser.state()
    if (
        not _assert_our_window_focused(state)
        or state.get("value") != expected_current
        or state.get("caret") != editing_caret
        or _snapshot(text, 10_000) != expected_current
        or not _is_focused(pyatspi, obj)
    ):
        raise ProbeError("Firefox field lost focus or changed before insertion")

    editable.insertText(rebased, INSERTION, len(INSERTION))
    expected_after = expected_current[:rebased] + INSERTION + expected_current[rebased:]

    def readback():
        state = browser.state()
        observed = _snapshot(text, 10_000)
        if state.get("value") != observed:
            return None
        return state, observed

    state, after = _wait_until(
        readback, timeout=timeout, description="AT-SPI/Firefox post-insertion read-back"
    )
    if not inserted_exactly(expected_current, after, rebased, INSERTION):
        raise ProbeError("AT-SPI insertion was not at the anchor (read-back mismatch)")
    if after != expected_after:
        raise ProbeError("AT-SPI write was not confirmed at the requested offset")

    caret_after = text.caretOffset
    if caret_after != editing_caret or state.get("caret") != editing_caret:
        raise ProbeError(
            f"text inserted, but editing caret moved: "
            f"{editing_caret} -> {caret_after}; "
            "independent caret is NOT yet supported"
        )
    return AnchorResult(original_anchor, rebased, editing_caret, caret_after)


class WebDriverClient:
    """Small W3C WebDriver client; no Selenium/Python packaging dependency."""

    def __init__(self, port: int):
        self.url = f"http://127.0.0.1:{port}"
        self.opener = build_opener(ProxyHandler({}))

    def command(self, method: str, path: str, payload=None):
        data = None if payload is None else json.dumps(payload).encode("utf-8")
        request = Request(
            self.url + path,
            data=data,
            method=method,
            headers={"Content-Type": "application/json"},
        )
        try:
            with self.opener.open(request, timeout=20) as response:
                reply = json.load(response)
        except HTTPError as exc:
            try:
                detail = json.load(exc)["value"]
                message = detail.get(
                    "message", detail.get("error", "WebDriver rejected command")
                )
            except (ValueError, KeyError, TypeError):
                message = "WebDriver rejected command"
            raise ProbeError(f"WebDriver HTTP {exc.code}: {message}") from exc
        except (URLError, OSError, ValueError) as exc:
            raise ProbeError(f"WebDriver unavailable: {exc}") from exc
        value = reply.get("value")
        if isinstance(value, dict) and "error" in value:
            raise ProbeError(f"WebDriver: {value.get('message', value['error'])}")
        return value


class FirefoxField:
    def __init__(self, client: WebDriverClient, session: str, element: str):
        self.client = client
        self.session = session
        self.element = element

    def _script(self, source, args=None):
        return self.client.command(
            "POST", f"/session/{self.session}/execute/sync",
            {"script": source, "args": args or []},
        )

    def focus_at(self, offset: int):
        self.client.command(
            "POST", f"/session/{self.session}/element/{self.element}/click", {}
        )
        self._script(
            "const t=document.getElementById('myna-anchor-test');"
            "t.focus();t.setSelectionRange(arguments[0],arguments[0]);",
            [offset],
        )

    def type_at(self, offset: int, payload: str):
        self._script(
            "const t=document.getElementById('myna-anchor-test');"
            "t.focus();t.setSelectionRange(arguments[0],arguments[0]);",
            [offset],
        )
        self.client.command(
            "POST", f"/session/{self.session}/element/{self.element}/value",
            {"text": payload},
        )

    def state(self):
        return self._script(
            "const t=document.getElementById('myna-anchor-test');"
            "return {value:t.value,caret:t.selectionStart,"
            "active:document.activeElement===t,windowFocused:document.hasFocus()};"
        )


def _free_loopback_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def _run_live(firefox: Path, geckodriver: Path, *, timeout: float):
    try:
        import pyatspi
    except ImportError as exc:
        raise ProbeError(
            "pyatspi unavailable; use /usr/bin/python3 and python-atspi"
        ) from exc

    if not os.environ.get("WAYLAND_DISPLAY") and not os.environ.get("DISPLAY"):
        raise ProbeError("a running graphical session is required; do not use headless")

    port = _free_loopback_port()
    client = WebDriverClient(port)
    process = subprocess.Popen(
        [
            str(geckodriver), "--host", "127.0.0.1", "--port", str(port),
            "--log", "error",
        ],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    session = None
    try:
        def server_ready():
            if process.poll() is not None:
                raise OSError(
                    f"geckodriver stopped prematurely (exit {process.returncode})"
                )
            try:
                status = client.command("GET", "/status")
                return status if status.get("ready") else None
            except ProbeError:
                return None

        _wait_until(server_ready, timeout=timeout, description="geckodriver startup")
        payload = firefox_capabilities(
            str(firefox), wayland=bool(os.environ.get("WAYLAND_DISPLAY"))
        )
        result = client.command("POST", "/session", payload)
        session = result["sessionId"]
        url = "data:text/html;charset=utf-8," + quote(SCRATCH_HTML, safe="")
        client.command("POST", f"/session/{session}/url", {"url": url})
        element = client.command(
            "POST", f"/session/{session}/element",
            {"using": "css selector", "value": "#myna-anchor-test"},
        )[ELEMENT_KEY]
        return run_anchor_scenario(
            FirefoxField(client, session, element), pyatspi, timeout=timeout
        )
    finally:
        if session is not None:
            try:
                client.command("DELETE", f"/session/{session}")
            except ProbeError:
                pass
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--firefox",
        type=Path,
        required=True,
        help="full path to Firefox Nightly binary",
    )
    parser.add_argument(
        "--geckodriver", default="geckodriver", help="geckodriver executable or path"
    )
    parser.add_argument(
        "--timeout", type=float, default=20.0, help="seconds per browser/AT-SPI stage"
    )
    args = parser.parse_args(argv)
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    firefox = args.firefox.expanduser().resolve()
    driver = shutil.which(args.geckodriver)
    if not firefox.is_file() or not os.access(firefox, os.X_OK):
        parser.error(f"Firefox executable not found: {firefox}")
    if driver is None:
        parser.error("geckodriver is missing; install once: sudo pacman -S geckodriver")
    print(
        "Launching disposable Firefox profile; "
        "testing AT-SPI anchor without any clicks..."
    )
    try:
        result = _run_live(firefox, Path(driver), timeout=args.timeout)
    except (ProbeError, ValueError, KeyError, OSError) as exc:
        print(f"FAIL SAFE: {exc}", file=sys.stderr)
        return 1
    print(
        f"PASS: initial anchor {result.original_anchor} -> {result.rebased_anchor}; "
        f"editing caret {result.editing_caret} -> {result.editing_caret_after}; "
        "AT-SPI write verified at the anchor."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
