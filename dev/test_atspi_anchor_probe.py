"""Hermetic invariants for the AT-SPI anchored insertion feasibility probe."""

import unittest

from atspi_anchor_probe import AnchorConflict, inserted_exactly, rebase_anchor


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
