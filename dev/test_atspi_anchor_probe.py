"""Hermetic invariants for the AT-SPI anchored insertion feasibility probe."""

import unittest

from atspi_anchor_probe import (
    AnchorConflict,
    ProbeError,
    _editable_ancestor,
    _protected,
    inserted_exactly,
    rebase_anchor,
)


class AtspiStub:
    """Only constants actually exposed by pyatspi on Arch are modeled."""

    STATE_EDITABLE = "editable"
    ROLE_PASSWORD_TEXT = "password text"


class AccessibleStub:
    def __init__(self, role="entry", states=("editable",)):
        self.role = role
        self.states = states
        self.parent = None

    def getState(self):
        return self

    def contains(self, state):
        return state in self.states

    def getRole(self):
        return self.role

    def queryEditableText(self):
        return "editable-proxy"

    def queryText(self):
        return "text-proxy"


class AccessibleFieldTests(unittest.TestCase):
    def test_normal_entry_does_not_require_nonexistent_protected_state(self):
        obj = AccessibleStub()
        self.assertFalse(_protected(AtspiStub, obj))
        self.assertEqual(
            _editable_ancestor(AtspiStub, obj),
            (obj, "editable-proxy", "text-proxy"),
        )

    def test_password_role_is_never_queried(self):
        obj = AccessibleStub(role="password text")
        self.assertTrue(_protected(AtspiStub, obj))
        with self.assertRaisesRegex(ProbeError, "protected"):
            _editable_ancestor(AtspiStub, obj)

    def test_read_only_entry_is_refused(self):
        with self.assertRaisesRegex(ProbeError, "not editable"):
            _editable_ancestor(AtspiStub, AccessibleStub(states=()))


class RebaseAnchorTests(unittest.TestCase):
    def test_no_change(self):
        self.assertEqual(rebase_anchor("Bonjour monde", "Bonjour monde", 8), 8)

    def test_insert_before_anchor(self):
        self.assertEqual(rebase_anchor("Bonjour monde", "Salut, Bonjour monde", 8), 15)

    def test_delete_before_anchor(self):
        self.assertEqual(rebase_anchor("Bonjour monde", "Bon monde", 8), 4)

    def test_replace_before_anchor(self):
        self.assertEqual(rebase_anchor("Bonjour monde", "Bonsoir monde", 8), 8)

    def test_insert_after_anchor(self):
        self.assertEqual(rebase_anchor("Bonjour monde", "Bonjour monde !", 8), 8)

    def test_insert_exactly_at_anchor_follows_user_edit(self):
        self.assertEqual(rebase_anchor("abcd", "abXYZcd", 2), 5)

    def test_reject_replacement_crossing_anchor(self):
        with self.assertRaises(AnchorConflict):
            rebase_anchor("abcdefgh", "abXYgh", 4)

    def test_reject_deletion_starting_at_anchor(self):
        with self.assertRaises(AnchorConflict):
            rebase_anchor("abcd", "ab", 2)

    def test_multiple_edits_across_anchor_fail_closed(self):
        with self.assertRaises(AnchorConflict):
            rebase_anchor("abc def ghi", "ABC def GHI", 6)

    def test_unicode_offsets_are_characters_not_bytes(self):
        self.assertEqual(rebase_anchor("é💬 beaucoup", "🥐é💬 beaucoup", 3), 4)

    def test_invalid_offset_is_rejected(self):
        with self.assertRaises(ValueError):
            rebase_anchor("abc", "abc", 4)


class InsertionReadbackTests(unittest.TestCase):
    def test_verified_at_explicit_offset(self):
        self.assertTrue(inserted_exactly("ABCD", "ABé💬CD", 2, "é💬"))

    def test_false_positive_api_success_is_not_a_write(self):
        self.assertFalse(inserted_exactly("ABCD", "ABCD", 2, "é💬"))

    def test_insertion_at_editing_caret_is_not_at_anchor(self):
        self.assertFalse(inserted_exactly("ABCD", "ABCDé💬", 2, "é💬"))


if __name__ == "__main__":
    unittest.main()
