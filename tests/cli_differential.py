#!/usr/bin/env python3
"""Compare CLI boundary behavior with the original, separately built executable."""

import json
import os
import subprocess
from pathlib import Path

from _reference import reference_binary

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get("SLOSH_BIN", ROOT / "target/debug/slush")).resolve()
REFERENCE = reference_binary()
ENV = dict(os.environ, SLOSH_CONFIG="/nonexistent/slush-cli-regression.kdl")


def run(binary, args, script=None):
    result = subprocess.run(
        [str(binary), *args],
        input=script,
        capture_output=True,
        text=True,
        env=ENV,
        timeout=8,
    )
    assert result.returncode == 0, (binary, args, result.stderr)
    return result.stdout


def snapshot(binary, args):
    return json.loads(run(binary, args, "snapshot\nquit\n").splitlines()[0])


def main():
    delayed = [
        "--headless",
        "--cols",
        "60",
        "--rows",
        "12",
        "--",
        "/bin/sh",
        "-c",
        "sleep .2; printf 'after-default-quiet\\n'; sleep .5",
    ]
    for binary in [REFERENCE, BIN]:
        assert "after-default-quiet" in run(binary, delayed), binary
    print("ok   default quiet time retains delayed output like the reference")

    for flag, other in [("--cols", "--rows"), ("--rows", "--cols")]:
        for value in [
            "0",
            "not-a-number",
            "12junk",
            " +12suffix",
            "\v12suffix",
            "65536",
            "65548",
            "-65524",
        ]:
            args = ["--script", flag, value, other, "0", "--", "/bin/cat"]
            screens = [snapshot(binary, args) for binary in [REFERENCE, BIN]]
            assert [(s["cols"], s["rows"]) for s in screens[:1]] == [
                (screens[1]["cols"], screens[1]["rows"])
            ], (flag, value, screens)
            print(f"ok   {flag} {value!r} retains decimal-prefix and uint16 behavior")

    args = [
        "--script",
        "--cols",
        "80",
        "--rows",
        "12",
        "--",
        "/bin/sh",
        "-c",
        'printf "argv-count=%s\\n" "$#"; cat',
        "argv0",
        *map(str, range(70)),
    ]
    for binary in [REFERENCE, BIN]:
        screen = json.loads(run(binary, args, "settle 200\nsnapshot\nquit\n"))
        assert "argv-count=59" in "\n".join(screen["text"]), (binary, screen)
    print("ok   pane CLI arguments retain the reference's 63-word limit")


if __name__ == "__main__":
    main()
