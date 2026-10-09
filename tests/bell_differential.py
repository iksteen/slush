#!/usr/bin/env python3
"""Compare bell acknowledgement without the original test's two sleep timers."""

import os
import sys
from pathlib import Path

from _reference import reference_binary

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests/reference/slosh/tests"))
import harness  # noqa: E402

BIN = Path(os.environ.get("SLOSH_BIN", ROOT / "target/debug/slush")).resolve()
REFERENCE = reference_binary()
QUIET = ["/bin/sh", "-c", "stty raw -echo; printf 'ready\\n'; cat"]
MARK = "[!]"


def scenario(binary):
    harness.BIN = str(binary)
    with harness.Session(QUIET, cols=90, rows=20) as session:
        assert session.api("split", dir="cols")["ok"]
        panes = session.panes()
        gone, other = (pane["id"] for pane in panes)
        for pane in panes:
            session.until(
                lambda _snap: "ready" in session.api("capture", id=pane["id"])["text"]
            )
            assert "ready" in session.api("capture", id=pane["id"])["text"]
        assert session.api("focus", id=gone)["ok"]
        session.send(r"\x01m")
        session.settle(20)
        assert next(p for p in session.panes() if p["id"] == gone)["w"] == 0
        assert session.api("raw", id=gone, data="\x07")["ok"]
        snap = session.until(lambda snapshot: MARK in snapshot.line(1))
        assert MARK in snap.line(1)
        chip = next(
            hit
            for hit in snap.hits
            if hit["action"] == f"focus:{gone}" and hit["h"] == 3
        )
        session.click(chip["x"] + 1, chip["y"])
        session.settle(30)
        assert next(p for p in session.panes() if p["id"] == gone)["w"] > 0
        assert MARK not in session.snapshot().screen()
        print(f"ok   {binary.name}: restoring a minimized pane answers its bell")

        # A different pane ringing after focus moves must retain its own mark.
        assert session.api("raw", id=other, data="\x07")["ok"]
        snap = session.until(lambda snapshot: MARK in snapshot.line(1))
        assert MARK in snap.line(1)
        assert session.focused()["id"] == gone
        assert session.api("focus", id=other)["ok"]
        assert MARK not in session.snapshot().screen()
        print(f"ok   {binary.name}: a later sibling bell needs its own acknowledgement")


def main():
    for binary in (REFERENCE, BIN):
        for _ in range(3):
            scenario(binary)


if __name__ == "__main__":
    main()
