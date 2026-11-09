"""Hermetic checks for the single-command Firefox / AT-SPI live probe."""

import unittest
from unittest.mock import patch

from atspi_anchor_probe import ProbeError
from atspi_anchor_firefox import (
    ANCHOR_OFFSET,
    INITIAL_TEXT,
    PREFIX,
    INSERTION,
    FirefoxField,
    firefox_capabilities,
    run_anchor_scenario,
)


class ScratchField:
    def __init__(self, insert_at_caret=False):
        self.value = INITIAL_TEXT
        self.caret = 0
        self.insert_at_caret = insert_at_caret
        self.focused = True

    @property
    def characterCount(self):
        return len(self.value)

    @property
    def caretOffset(self):
        return self.caret

    def getText(self, start, end):
        return self.value[start:end]

    def insertText(self, position, payload, length):
        assert length == len(payload)
        if self.insert_at_caret:
            position = self.caret
        self.value = self.value[:position] + payload + self.value[position:]
        return True


class BrowserStub:
    def __init__(self, field):
        self.field = field

    def focus_at(self, offset):
        self.field.caret = offset

    def type_at(self, offset, payload):
        self.field.value = (
            self.field.value[:offset] + payload + self.field.value[offset:]
        )
        self.field.caret = offset + len(payload)

    def state(self):
        return {
            "value": self.field.value,
            "caret": self.field.caret,
            "active": self.field.focused,
            "windowFocused": self.field.focused,
        }


class FirefoxCapabilitiesTests(unittest.TestCase):
    def test_uses_requested_firefox_binary_with_temporary_profile(self):
        caps = firefox_capabilities("/test/nightly/firefox", wayland=True)
        opts = caps["capabilities"]["alwaysMatch"]["moz:firefoxOptions"]
        self.assertEqual(opts["binary"], "/test/nightly/firefox")
        self.assertEqual(opts["prefs"]["accessibility.force_disabled"], 0)
        self.assertEqual(opts["env"]["MOZ_ENABLE_WAYLAND"], "1")
        self.assertNotIn("-profile", opts.get("args", []))

    def test_x11_run_does_not_force_wayland(self):
        caps = firefox_capabilities("/test/nightly/firefox", wayland=False)
        opts = caps["capabilities"]["alwaysMatch"]["moz:firefoxOptions"]
        self.assertNotIn("MOZ_ENABLE_WAYLAND", opts["env"])


class WebDriverInputTests(unittest.TestCase):
    def test_typing_uses_real_webdriver_key_events(self):
        class ClientStub:
            def __init__(self):
                self.calls = []

            def command(self, method, path, payload):
                self.calls.append((method, path, payload))

        client = ClientStub()
        FirefoxField(client, "session-test", "element-test").type_at(0, PREFIX)
        self.assertEqual(
            client.calls[-1],
            (
                "POST",
                "/session/session-test/element/element-test/value",
                {"text": PREFIX},
            ),
        )
        self.assertIn("setSelectionRange", client.calls[0][2]["script"])
        self.assertNotIn(".value=", client.calls[0][2]["script"])


class FirefoxAnchorScenarioTests(unittest.TestCase):
    def test_inserts_at_rebased_offset_with_independent_caret(self):
        field = ScratchField()
        browser = BrowserStub(field)
        with (
            patch(
                "atspi_anchor_firefox._find_focused",
                return_value=(field, field, field),
            ),
            patch("atspi_anchor_firefox._is_focused", return_value=True),
        ):
            result = run_anchor_scenario(browser, object(), timeout=0.5)

        self.assertEqual(result.original_anchor, ANCHOR_OFFSET)
        self.assertEqual(result.rebased_anchor, ANCHOR_OFFSET + len(PREFIX))
        self.assertEqual(
            field.value,
            PREFIX
            + INITIAL_TEXT[:ANCHOR_OFFSET]
            + INSERTION
            + INITIAL_TEXT[ANCHOR_OFFSET:],
        )
        self.assertEqual(field.caret, len(PREFIX))

    def test_rejects_write_at_editing_caret(self):
        field = ScratchField(insert_at_caret=True)
        browser = BrowserStub(field)
        with (
            patch(
                "atspi_anchor_firefox._find_focused",
                return_value=(field, field, field),
            ),
            patch("atspi_anchor_firefox._is_focused", return_value=True),
        ):
            with self.assertRaisesRegex(ProbeError, "not at the anchor"):
                run_anchor_scenario(browser, object(), timeout=0.5)

    def test_rejects_no_longer_focused_window_before_insertion(self):
        field = ScratchField()
        browser = BrowserStub(field)
        original_type = browser.type_at

        def lose_focus(offset, payload):
            original_type(offset, payload)
            field.focused = False

        browser.type_at = lose_focus
        with (
            patch(
                "atspi_anchor_firefox._find_focused",
                return_value=(field, field, field),
            ),
            patch("atspi_anchor_firefox._is_focused", return_value=True),
        ):
            with self.assertRaisesRegex(ProbeError, "focus"):
                run_anchor_scenario(browser, object(), timeout=0.01)


if __name__ == "__main__":
    unittest.main()
