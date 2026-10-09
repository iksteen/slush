#!/usr/bin/env python3
"""Compare native editor selection and arguments with original slosh on Unix."""

import errno
import json
import os
import shlex
import sys
import tempfile
from pathlib import Path

from _reference import reference_binary

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests/reference/slosh/tests"))
import harness  # noqa: E402

BIN = Path(os.environ.get("SLOSH_BIN", ROOT / "target/debug/slush")).resolve()
REFERENCE = reference_binary()


def wrapper(path, prefix, extra=""):
    path.write_text(f'#!/bin/sh\nexec {prefix} {extra} "$@"\n')
    path.chmod(0o700)
    return shlex.quote(str(path))


def selected(binary, config, text, env, argument, description):
    config.write_text("splash_ms 0\n" + text)
    harness.BIN = str(binary)
    with harness.Session(
        ["/bin/cat"], cols=240, rows=30, config=config, env=env
    ) as active:
        # The reference includes native command bytes in its JSON pane labels.
        active.proc.stdout.reconfigure(errors="surrogateescape")
        result = active.api("edit-config")
        assert result["ok"], (binary, description, result)
        expected = "ARGS=" + argument.hex()
        file_mark = "FILE=" + os.fsencode(config).hex()
        snapshot = active.until(
            lambda snap: expected in snap.screen() and file_mark in snap.screen()
        )
        assert expected in snapshot.screen(), (binary, description, snapshot.screen())
        assert file_mark in snapshot.screen(), (binary, description, snapshot.screen())
        assert active.focused()["id"] == result["id"], (binary, result)
        print(
            f"ok   {binary.name}: {description} selects the editor with exact arguments"
        )
        active.raw("\\n")
        active.until(lambda _snap: len(active.panes()) == 1)
        assert len(active.panes()) == 1, (binary, description, active.panes())
        print(f"ok   {binary.name}: {description} closes its ephemeral pane on exit")


def main():
    with tempfile.TemporaryDirectory(prefix="rs-editor-", dir="/tmp") as temporary:
        root = Path(temporary)
        script = root / "editor.py"
        script.write_text(
            "import os,sys\n"
            "print('ARGS='+','.join(os.fsencode(a).hex() for a in sys.argv[1:-1]),flush=True)\n"
            "print('FILE='+os.fsencode(sys.argv[-1]).hex(),flush=True)\n"
            "sys.stdin.readline()\n"
        )
        prefix = f"{shlex.quote(sys.executable)} {shlex.quote(str(script))}"
        config = root / "config.kdl"
        argument = b"native-\xff\xfe ' two words"
        native_command = prefix + " " + shlex.quote(os.fsdecode(argument))
        unicode_program = wrapper(root / "editor-päne ' space", prefix)
        wrapper(root / "vi", prefix, "--fallback")
        cases = [
            ("", {"EDITOR": native_command}, argument, "native EDITOR command"),
            (
                "",
                {"EDITOR": unicode_program + " --unicode"},
                b"--unicode",
                "Unicode editor executable",
            ),
            (
                "editor " + json.dumps(prefix + " --configured") + "\n",
                {"EDITOR": os.fsdecode(b"nonexistent-editor-\xff")},
                b"--configured",
                "configured editor precedence",
            ),
            (
                'editor ""\n',
                {"EDITOR": native_command},
                argument,
                "empty configured editor fallback",
            ),
            (
                "",
                {"EDITOR": "", "PATH": str(root) + os.pathsep + os.environ["PATH"]},
                b"--fallback",
                "empty environment editor fallback",
            ),
        ]
        try:
            native_program = wrapper(root / os.fsdecode(b"editor-\xff space"), prefix)
        except OSError as error:
            if error.errno != errno.EILSEQ:
                raise
            print("skip filesystem rejects non-UTF-8 editor executable filenames")
        else:
            cases.append(
                (
                    "",
                    {"EDITOR": native_program + " --program"},
                    b"--program",
                    "native editor executable",
                )
            )
        for text, env, expected, description in cases:
            for binary in (REFERENCE, BIN):
                selected(binary, config, text, env, expected, description)


if __name__ == "__main__":
    main()
