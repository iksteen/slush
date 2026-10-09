#!/usr/bin/env python3
"""Compare native configuration text through drawing, replies and selection."""

import json
import os
import sys
import tempfile
from pathlib import Path

from _reference import reference_binary

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get("SLOSH_BIN", ROOT / "target/debug/slush")).resolve()
REFERENCE = reference_binary()
sys.path.insert(0, str(ROOT / "tests/reference/slosh/tests"))
import harness  # noqa: E402

PROGRAM = """import os,tty
tty.setraw(0)
os.write(1,b'\\033]2;probe\\007\\033]5577;1;status;working\\007\\033]5577;1;busy;1\\007alpha|beta\\r\\n')
while True:
    os.write(1,os.read(0,4096))
"""
MARKS = [
    "zoom_mark",
    "zoom_on_mark",
    "close_mark",
    "min_mark",
    "newtab_mark",
    "bell_mark",
    "busy_mark",
]


def replies(s, first=False):
    # Compare byte-preserving JSON semantically, including native text bytes
    # and hit geometry. JSON member order is deliberately irrelevant.
    screen = s.api("snapshot")["screen"]
    text = s.api("snapshot", format="text")["text"]
    assert text == "".join(row + "\n" for row in screen["text"])
    wire = s.api("snapshot", format="bytes")["bytes"]
    assert "\ufffd" not in wire
    if first:
        assert "\udcff" in wire
    # The footer identifies each build; retain every row carrying configured
    # marks and its complete hit geometry while excluding that version label.
    footer = screen["rows"] - 2
    screen["text"][footer] = ""
    screen["styles"] = [r for r in screen["styles"] if r["y"] != footer]
    hits = screen["hits"]
    screen["hit_at"] = [
        next(
            (
                hit["action"]
                for hit in reversed(hits)
                if hit["x"] <= x < hit["x"] + hit["w"]
                and hit["y"] <= y < hit["y"] + hit["h"]
            ),
            None,
        )
        for y in range(screen["rows"])
        for x in range(screen["cols"])
    ]
    screen["hits"] = sorted(hits, key=lambda hit: json.dumps(hit, sort_keys=True))
    return screen


def exercise(binary, config):
    harness.BIN = str(binary)
    with harness.Session(
        [sys.executable, "-c", PROGRAM], config=config, cols=120, rows=30
    ) as s:
        s.proc.stdout.reconfigure(errors="surrogateescape")
        assert "alpha|beta" in s.until_text("alpha|beta").screen()
        first = s.tabs()[0]["id"]
        s.api("set-name", id=first, name="first")
        results = {"ordinary chrome": replies(s, first=True)}
        s.key("z")
        results["zoom chrome"] = replies(s)
        s.key("z")
        s.key("?")
        results["modal close mark"] = replies(s)
        s.key("?")
        split = s.api("split", dir="cols")["id"]
        s.until(lambda _: "alpha|beta" in s.api("capture", id=split)["text"])
        s.key("m")
        results["minimized mark"] = replies(s)
        tab = s.api("new-tab", name="second")["id"]
        pane = next(p["id"] for p in s.panes() if p["tab_id"] == tab)
        s.until(lambda _: "alpha|beta" in s.api("capture", id=pane)["text"])
        s.api("select-tab", id=first)
        before = sum(row.count("\udcff") for row in s.snapshot().text)
        s.api("raw", id=pane, data="\a")
        snap = s.until(lambda sn: sum(row.count("\udcff") for row in sn.text) > before)
        assert sum(row.count("\udcff") for row in snap.text) > before
        results["bell mark"] = replies(s)
        return results


def selection(binary, config):
    harness.BIN = str(binary)
    with harness.Session(
        [sys.executable, "-c", PROGRAM], config=config, cols=120, rows=30
    ) as s:
        s.until_text("alpha|beta")
        p = s.pane()
        x, y = p["content_x"] + 1, p["content_y"]
        for _ in range(2):
            s.click(x, y)
        return s.api("clipboard")["text"]


def main():
    with tempfile.TemporaryDirectory(prefix="rs-config-marks-", dir="/tmp") as temp:
        config = Path(temp) / "config.kdl"
        for side in ("top", "left", "right"):
            source = (
                f'splash_ms 0\ntab_bar_side "{side}"\ntab_bar_width 24\n'
            ).encode()
            for name in MARKS:
                value = b"!\xff?" if name != "busy_mark" else b"!\xff" + b"s" * 30
                source += name.encode() + b' "' + value + b'"\n'
            config.write_bytes(source)
            results = [exercise(binary, config) for binary in (REFERENCE, BIN)]
            for stage in results[0]:
                expected, actual = results[0][stage], results[1][stage]
                differences = {
                    key: (expected[key], actual[key])
                    for key in expected
                    if expected[key] != actual[key]
                }
                assert not differences, (side, stage, differences)
                print(
                    f"ok   {side}: {stage} retains native marks in all snapshot formats"
                )
        # A replacement character expands one byte into three and can push
        # a real ASCII separator out of the original 63-byte buffer.
        for separators, expected in (
            (b"\xff" * 62 + b"|", "alpha"),
            (b"x" * 62 + "é".encode() + b"|", "alpha|beta"),
        ):
            config.write_bytes(b'word_separators "' + separators + b'"\nsplash_ms 0\n')
            for binary in (REFERENCE, BIN):
                actual = selection(binary, config)
                assert actual == expected, (binary, actual, expected)
                print(
                    f"ok   {binary.name}: word selection uses the original byte limit"
                )


if __name__ == "__main__":
    main()
