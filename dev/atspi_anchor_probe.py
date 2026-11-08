#!/usr/bin/env python3
"""Probe anchored text insertion in a *scratch* field on the actual desktop.

This is a feasibility experiment, NOT a Myna injection backend. In particular,
the read/rebase/insert sequence is not atomic with the user's concurrent edits.

The default run is read-only. Example (give yourself time to focus a test field
before the first snapshot, then edit the same field and move its caret):

    python3 dev/atspi_anchor_probe.py --acquire-delay 5 --edit-delay 10

To test a REAL insertion at the old anchor, and whether it moves the *editing*
caret, explicitly opt in on a disposable text field:

    python3 dev/atspi_anchor_probe.py --acquire-delay 5 --edit-delay 10 \
        --insert ' [MYNA PROBE]'

Requires the distro's python3-pyatspi and an accessibility-enabled desktop.
Run with the system Python if a virtual environment cannot import pyatspi.
No field contents are printed or written to a log.
"""

import argparse
import sys
import time


class AnchorConflict(Exception):
    """The document changed across the anchor; inserting is unsafe."""


def rebase_anchor(previous: str, current: str, offset: int) -> int:
    """Rebase a Unicode-character offset across at most one contiguous edit.

    The longest shared prefix/suffix conservatively bounds the edit. Changes
    strictly after the anchor preserve it; changes ending at or before it move
    it by their delta. An insertion *exactly* at the anchor belongs to the
    user's editing caret, and the dictation anchor follows it. A change whose
    span crosses the anchor fails closed. Multiple disjoint edits straddling
    the anchor are deliberately treated as a conflict, not guessed apart.

    This pure helper is only a proof of the offset policy; a production target
    needs serialized text-change events, identity/focus guards and write acks.
    """
    if not 0 <= offset <= len(previous):
        raise ValueError("anchor offset outside previous document")
    if previous == current:
        return offset

    prefix = 0
    while (
        prefix < min(len(previous), len(current))
        and previous[prefix] == current[prefix]
    ):
        prefix += 1

    suffix = 0
    max_suffix = min(len(previous) - prefix, len(current) - prefix)
    while (
        suffix < max_suffix
        and previous[len(previous) - suffix - 1]
        == current[len(current) - suffix - 1]
    ):
        suffix += 1

    old_edit_end = len(previous) - suffix
    if old_edit_end <= offset:
        return offset + len(current) - len(previous)
    if prefix > offset:
        return offset
    raise AnchorConflict("editing intersected the dictation anchor")


class ProbeError(Exception):
    """The field cannot be safely inspected or written by this experiment."""


def inserted_exactly(previous: str, after: str, offset: int, payload: str) -> bool:
    """Require full read-back equality: an AT-SPI true reply can be a no-op."""
    return (
        0 <= offset <= len(previous)
        and bool(payload)
        and after == previous[:offset] + payload + previous[offset:]
    )


def _protected(pyatspi, obj) -> bool:
    state = obj.getState()
    return state.contains(pyatspi.STATE_PROTECTED) or (
        obj.getRole() == pyatspi.ROLE_PASSWORD_TEXT
    )


def _is_focused(pyatspi, obj) -> bool:
    return obj.getState().contains(pyatspi.STATE_FOCUSED)


def _editable_ancestor(pyatspi, obj):
    """A focused text node can live inside an editable accessible container."""
    while obj is not None:
        if _protected(pyatspi, obj):
            raise ProbeError("focused field is protected")
        try:
            editable = obj.queryEditableText()
            text = obj.queryText()
            return obj, editable, text
        except (NotImplementedError, LookupError, RuntimeError):
            obj = obj.parent
    raise ProbeError("focused element has no AT-SPI EditableText interface")


def _find_focused(pyatspi, max_nodes=20_000):
    desktop = pyatspi.Registry.getDesktop(0)
    stack = [desktop]
    visited = 0
    while stack:
        obj = stack.pop()
        visited += 1
        if visited > max_nodes:
            raise ProbeError("AT-SPI tree is too large to locate focus safely")
        try:
            if _is_focused(pyatspi, obj):
                return _editable_ancestor(pyatspi, obj)
            stack.extend(obj[i] for i in range(obj.childCount - 1, -1, -1))
        except ProbeError:
            raise
        except (LookupError, RuntimeError):
            # A window may disappear while the accessibility tree is walked.
            continue
    raise ProbeError("no AT-SPI focused editable element found")


def _snapshot(text, max_chars: int) -> str:
    length = text.characterCount
    if length < 0 or length > max_chars:
        raise ProbeError("document length outside bounded probe range")
    result = text.getText(0, length)
    if len(result) != length:
        raise ProbeError("AT-SPI character count differs from retrieved text")
    return result


def _run_probe(args):
    try:
        import pyatspi
    except ImportError as exc:
        raise ProbeError("install python3-pyatspi for the system Python") from exc

    print(f"Focus a disposable editable field now ({args.acquire_delay}s).")
    time.sleep(args.acquire_delay)
    obj, editable, text = _find_focused(pyatspi)
    before = _snapshot(text, args.max_chars)
    anchor = text.caretOffset
    if not 0 <= anchor <= len(before):
        raise ProbeError("initial caret is outside AT-SPI text bounds")
    print(f"Captured anchor at character {anchor}; document has {len(before)} characters.")

    print(f"Move the editing caret and edit that SAME field ({args.edit_delay}s).")
    time.sleep(args.edit_delay)
    if not _is_focused(pyatspi, obj) or _protected(pyatspi, obj):
        raise ProbeError("original target lost focus or became protected; nothing inserted")

    current = _snapshot(text, args.max_chars)
    rebased = rebase_anchor(before, current, anchor)
    editing_caret = text.caretOffset
    print(
        f"Anchor rebased to {rebased}; editing caret is {editing_caret}; "
        f"text length is {len(current)}."
    )
    if args.insert is None:
        print("READ-ONLY: no text was inserted. Pass --insert on a scratch field.")
        return

    # This standalone experiment has a read/write race. Even after validation,
    # never use it on a valuable document; a production backend must serialize
    # updates and verify the target and caret on each insertion.
    if _snapshot(text, args.max_chars) != current or not _is_focused(pyatspi, obj):
        raise ProbeError("field changed during the probe; refusing to insert")
    success = editable.insertText(rebased, args.insert, len(args.insert))
    after = _snapshot(text, args.max_chars)
    inserted = inserted_exactly(current, after, rebased, args.insert)
    caret_after = text.caretOffset
    print(
        f"EditableText.insertText returned {success!r}; "
        f"READ-BACK={'PASS' if inserted else 'FAIL'}; "
        f"editing caret {editing_caret} -> {caret_after}."
    )
    if not inserted:
        raise ProbeError("insertion was not confirmed; API success may be a no-op")
    if caret_after != editing_caret:
        print("CARET MOVED: this backend cannot yet preserve an independent editing caret.")
    else:
        print("CARET PRESERVED: explicit-position insertion appears usable in this field.")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--acquire-delay", type=float, default=5)
    parser.add_argument("--edit-delay", type=float, default=10)
    parser.add_argument("--max-chars", type=int, default=50_000)
    parser.add_argument(
        "--insert",
        help="explicitly insert this string in a disposable field (default: read-only)",
    )
    args = parser.parse_args(argv)
    if args.acquire_delay < 0 or args.edit_delay < 0 or args.max_chars <= 0:
        parser.error("delays must be nonnegative and max-chars must be positive")
    if args.insert == "":
        parser.error("--insert must be nonempty")
    try:
        _run_probe(args)
    except (ProbeError, AnchorConflict) as exc:
        print(f"SAFE STOP: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
