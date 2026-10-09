#!/usr/bin/env python3
"""Compare bounded script frames and bare numeric commands with original slosh."""

import json
import os
import select
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from _reference import reference_binary

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get("SLOSH_BIN", ROOT / "target/debug/slush")).resolve()
REFERENCE = reference_binary()
sys.path.insert(0, str(ROOT / "tests/reference/slosh/tests"))
import harness  # noqa: E402

ARGS = ["--script", "--cols", "80", "--rows", "24", "--", "/bin/cat"]
RESIZES = [
    b"12 13",
    b"12junk 14",
    b"bad 15",
    b"12 +14tail",
    b"12+14",
    b" +12 -65522tail",
    b"65548 65549",
    b"65536 12",
    b"12 65536",
    b"0 0",
    b"-65524 65549",
    b"012 013",
    b"0x12 15",
    b"12\xff 15",
    b"\xc2\xa012 13",
    b"\v12\f13",
]


def run(binary, env, script):
    result = subprocess.run(
        [str(binary), *ARGS], input=script, capture_output=True, env=env, timeout=5
    )
    assert result.returncode == 0, (binary, result.returncode, result.stderr)
    return result.stdout, result.stderr.replace(
        binary.name.encode() + b": unknown command: ", b"unknown command: "
    )


def frames(env):
    header, tail = b'{"cmd":"alive","padding":"', b'"}'
    cases = [
        (
            f"JSON length {size}",
            header + b"x" * (size - len(header) - len(tail)) + tail + b"\nquit\n",
        )
        for size in (4094, 4095)
    ]
    cases += [
        (
            "second command in a long physical line",
            b"settle" + b" " * 4089 + b"alive\nquit\n",
        ),
        ("quit in the next bounded frame", b"settle" + b" " * 4089 + b"quit\nalive\n"),
        (
            "JSON reset at frame boundary",
            header + b"x" * (4095 - len(header)) + b'{"cmd":"alive"}\nquit\n',
        ),
        ("NUL consumes its physical line", b"\0alive\nalive\nquit\n"),
        ("CR consumes the remaining physical line", b"alive\ralive\nquit\n"),
        ("native unknown command", b"unknown-\xff\xfe\nalive\nquit\n"),
        (
            "partial UTF-8 at byte boundary",
            b"unknown-" + b"x" * (4094 - 8) + "é".encode() + b"\nalive\nquit\n",
        ),
        ("EOF without newline", b"alive"),
    ]
    for label, script in cases:
        results = [run(binary, env, script) for binary in (REFERENCE, BIN)]
        normalized = [
            (
                [
                    json.loads(line) if line.startswith(b"{") else line
                    for line in output.split(b"\n")
                    if line
                ],
                errors,
            )
            for output, errors in results
        ]
        assert normalized[0] == normalized[1], (label, results)
        print(f"ok   {label} matches C replies and native stderr bytes")
    for binary in (REFERENCE, BIN):
        process = subprocess.Popen(
            [str(binary), *ARGS],
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        try:
            process.stdin.write(b"alive" + b" " * 4090)
            process.stdin.flush()
            assert select.select([process.stdout], [], [], 2)[0], (
                binary,
                "waited for a newline at the frame limit",
            )
            assert process.stdout.readline() == b"true\n"
            print(f"ok   {binary.name}: full bounded frame dispatches before newline")
        finally:
            process.stdin.write(b"\nquit\n")
            process.stdin.close()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            process.stdout.close()
            process.stderr.close()


def resize_script(binary, env, argument):
    stdout, _ = run(binary, env, b"resize " + argument + b"\nsnapshot\nquit\n")
    screen = json.loads(stdout)
    return screen["cols"], screen["rows"]


def settling(env):
    program = "import os,time,tty;tty.setraw(0);os.write(1,b'READY\\r\\n');os.read(0,1);os.write(1,b'START\\r\\n');time.sleep(.2);os.write(1,b'LATE\\r\\n');os.read(0,1)"
    cases = [
        (b"settle12", True),
        (b"settle\t12", True),
        (b"settled 12", True),
        (b"settle 12tail", False),
        (b"settle -12", False),
        (b"settle invalid", False),
        (b"settle ", False),
        (b"settle \v+12\xff", False),
    ]
    for command, expected in cases:
        for binary in (REFERENCE, BIN):
            harness.BIN = str(binary)
            with harness.Session(
                [sys.executable, "-c", program], config=env["SLOSH_CONFIG"]
            ) as active:
                active.proc.stdin.reconfigure(errors="surrogateescape")
                # Start with a ready, raw PTY; one explicitly triggered delayed
                # marker distinguishes a decimal prefix from the default quiet.
                active.until(lambda _snap: "READY" in active.api("capture")["text"])
                active._cmd("raw X")
                active._cmd(os.fsdecode(command))
                late = "LATE" in active.api("capture")["text"]
                assert late == expected, (binary, command, late, expected)
                print(f"ok   {binary.name}: {command!r} uses C quiet-time parsing")


def resize_socket(binary, env, arguments):
    env = dict(env, XDG_RUNTIME_DIR=str(Path(env["SLOSH_CONFIG"]).parent))
    env.pop("SLOSH", None)
    env.pop("SLOSH_SESSION", None)
    process = subprocess.Popen(
        [str(binary), "-s", "driver", "--", "/bin/cat"],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )

    def request(line):
        result = subprocess.run(
            [os.fsencode(binary), b"-s", b"driver", b"cmd", line],
            env=env,
            capture_output=True,
            timeout=3,
        )
        return result.stdout if result.returncode == 0 else None

    try:
        deadline = time.monotonic() + 5
        while not request(b"alive"):
            assert process.poll() is None and time.monotonic() < deadline
            time.sleep(0.02)
        sizes = []
        for argument in arguments:
            request(b"resize 80 24")
            request(b"resize " + argument)
            screen = json.loads(request(b"snapshot"))
            sizes.append((screen["cols"], screen["rows"]))
        return sizes
    finally:
        request(b"quit")
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        process.stderr.close()


def main():
    with tempfile.TemporaryDirectory(prefix="rs-driver-", dir="/tmp") as temporary:
        config = Path(temporary) / "config.kdl"
        config.write_text("splash_ms 0\nstatus_bar false\n")
        env = dict(os.environ, SLOSH_CONFIG=str(config))
        frames(env)
        settling(env)
        for argument in RESIZES:
            sizes = [
                resize_script(binary, env, argument) for binary in (REFERENCE, BIN)
            ]
            assert sizes[0] == sizes[1], (argument, sizes)
            print(f"ok   script resize {argument!r} matches C decimal scans")
        sizes = [resize_socket(binary, env, RESIZES) for binary in (REFERENCE, BIN)]
        assert sizes[0] == sizes[1], sizes
        for argument in RESIZES:
            print(f"ok   socket resize {argument!r} matches C decimal scans")


if __name__ == "__main__":
    main()
