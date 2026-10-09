#!/usr/bin/env python3
"""Compare theme saves byte for byte with original slosh, over both transports."""

import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from _reference import reference_binary

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests/reference/slosh/tests"))
import harness  # noqa: E402

BIN = Path(os.environ.get("SLOSH_BIN", ROOT / "target/debug/slush")).resolve()
REFERENCE = reference_binary()
QUIET = ["/bin/sh", "-c", "cat"]
CASES = [
    (
        "native-values",
        b'// comment-\xff\neditor "arg-\xfe"\ntheme_name "old"\ngap 3\n',
        "amber",
    ),
    ("crlf", b'// mine\r\ntheme_name "old" // selected\r\ngap 3\r\n', "amber"),
    ("append-no-newline", b"gap 3", "amber"),
    ("replace-no-newline", b'gap 3\ntheme_name "old"', "amber"),
    ("indented", b'  theme_name "old"\ngap 3\n', "amber"),
    ("first-only", b'theme_name "old"\ntheme_name "old" // keep\n', "amber"),
    ("touching-quote", b'theme_name"old"\ngap 3\n', "amber"),
    ("tabs-and-comments", b'// theme_name "old"\ntheme_name\t"old"\n\n', "amber"),
    ("empty", b"", "amber"),
    ("absent", None, "amber"),
    ("nul-in-source", b"gap 3\n\x00// ignored-\xff\n", "amber"),
    ("quote-in-name", b'// preserve-\xff\ntheme_name "old"\n', 'say"hi'),
]


def exercise(binary, transport, request, config, name):
    response = request("theme", name=name, save=True)
    assert response["ok"] and response["saved"] and response["theme"] == name, (
        binary,
        transport,
        config.parent.name,
        response,
    )
    saved = config.read_bytes()
    assert not config.with_name(config.name + ".new").exists()
    print(f"ok   {binary.name}: {transport} saves {config.parent.name}")
    return saved


def script(binary, config, name):
    harness.BIN = str(binary)
    with harness.Session(QUIET, config=config) as active:
        return exercise(binary, "script", active.api, config, name)


def socket(binary, config, name):
    env = dict(os.environ, SLOSH_CONFIG=str(config), XDG_RUNTIME_DIR=str(config.parent))
    env.pop("SLOSH", None)
    env.pop("SLOSH_SESSION", None)
    process = subprocess.Popen(
        [str(binary), "-s", "save", "--", *QUIET],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )

    def request(command, **fields):
        response = subprocess.run(
            [str(binary), "-s", "save", "cmd", json.dumps(dict(cmd=command, **fields))],
            env=env,
            capture_output=True,
            timeout=3,
        )
        return json.loads(response.stdout) if response.returncode == 0 else None

    try:
        deadline = time.monotonic() + 5
        while not request("alive"):
            assert process.poll() is None and time.monotonic() < deadline, (
                binary,
                "theme save daemon did not start",
            )
            time.sleep(0.02)
        return exercise(binary, "socket", request, config, name)
    finally:
        request("quit")
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        process.stderr.close()


def main():
    with tempfile.TemporaryDirectory(prefix="rs-theme-save-", dir="/tmp") as temporary:
        root = Path(temporary)
        for transport in (script, socket):
            for label, source, name in CASES:
                folder = root / label
                folder.mkdir(exist_ok=True)
                config = folder / "config.kdl"
                themes = folder / "themes"
                themes.mkdir(exist_ok=True)
                for theme in ("old", "amber", 'say"hi'):
                    (themes / (theme + ".kdl")).write_text(
                        'theme { frame_focus "#123456" }\n'
                    )
                baseline = None
                for binary in (REFERENCE, BIN):
                    if source is None:
                        config.unlink(missing_ok=True)
                    else:
                        config.write_bytes(source)
                    saved = transport(binary, config, name)
                    harness.BIN = str(binary)
                    with harness.Session(QUIET, config=config) as restarted:
                        current = restarted.api("theme")["theme"]
                    if binary == REFERENCE:
                        baseline = saved, current
                    else:
                        assert (saved, current) == baseline, (
                            transport.__name__,
                            label,
                            baseline,
                            (saved, current),
                        )
                        print(
                            f"ok   slush: {transport.__name__} {label} matches C file bytes and restart"
                        )


if __name__ == "__main__":
    main()
