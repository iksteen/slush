#!/usr/bin/env python3
"""Compare native configuration diagnostics in the CLI and reload replies."""

import errno
import json
import os
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

QUIET = ["/bin/sh", "-c", "cat"]


def native_file(directory, raw, fallback):
    file = directory / os.fsdecode(raw)
    try:
        file.write_bytes(b"")
    except OSError as error:
        if error.errno != errno.EILSEQ:
            raise
        file = directory / fallback
        file.write_bytes(b"")
        print(
            "skip raw filename rejected by filesystem; raw message bytes still tested"
        )
    return file


def exercise(request, config, cases, binary, transport):
    replies = []
    for label, source in cases:
        config.write_bytes(source)
        reply = request("reload")
        assert reply["ok"] and "warning" in reply, (binary, label, reply)
        replies.append(reply)
        print(f"ok   {binary.name}: {transport} reload {label}")
    config.unlink()
    reply = request("reload")
    assert not reply["ok"] and "error" in reply, (binary, reply)
    replies.append(reply)
    print(f"ok   {binary.name}: {transport} missing config error")
    return replies


def script(binary, config, cases):
    harness.BIN = str(binary)
    config.write_bytes(b"splash_ms 0\n")
    with harness.Session(QUIET, config=config) as active:
        active.proc.stdin.reconfigure(errors="surrogateescape")
        active.proc.stdout.reconfigure(errors="surrogateescape")

        def request(command):
            active._cmd(json.dumps({"cmd": command}))
            return json.loads(active.proc.stdout.readline())

        return exercise(request, config, cases, binary, "script")


def daemon(binary, config, cases):
    config.write_bytes(b"splash_ms 0\n")
    env = dict(os.environ, SLOSH_CONFIG=str(config), XDG_RUNTIME_DIR=str(config.parent))
    env.pop("SLOSH", None)
    env.pop("SLOSH_SESSION", None)
    process = subprocess.Popen(
        [str(binary), "-s", "diagnostics", "--", *QUIET],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )

    def request(command):
        result = subprocess.run(
            [str(binary), "-s", "diagnostics", "cmd", json.dumps({"cmd": command})],
            env=env,
            capture_output=True,
            timeout=3,
        )
        return (
            json.loads(result.stdout.decode(errors="surrogateescape"))
            if result.returncode == 0
            else None
        )

    try:
        deadline = time.monotonic() + 5
        while request("alive") is None:
            assert process.poll() is None and time.monotonic() < deadline
            time.sleep(0.01)
        return exercise(request, config, cases, binary, "socket")
    finally:
        request("quit")
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        process.stderr.close()


def main():
    with tempfile.TemporaryDirectory(prefix="rs-config-diag-", dir="/tmp") as temporary:
        directory = Path(temporary)
        config = native_file(directory, b"config-\xff.kdl", "config-é.kdl")
        cycle = native_file(directory, b"cycle-\xfe.kdl", "cycle-ö.kdl")
        cycle.write_bytes(b'include "' + os.fsencode(cycle.name) + b'"\n')
        included = native_file(directory, b"included-\xfe.kdl", "included-é.kdl")
        included.write_bytes(b"\nunknown-\xff 1\n")
        cases = [
            ("raw node name", b"unknown-\xff 1\n"),
            ("second raw node name", b"\nunknown-\xfe 1\n"),
            ("long raw node name", b"x" * 175 + b"\xff\xfe\n"),
            ("split UTF-8 diagnostic", ("é" * 120).encode() + b"\n"),
            (
                "included native filename",
                b'include "' + os.fsencode(included.name) + b'"\n',
            ),
            ("missing native include", b'include "missing-\xfe.kdl"\n'),
            ("native include cycle", b'include "' + os.fsencode(cycle.name) + b'"\n'),
            ("32-message limit", b"unknown-\xff 1\n" * 35),
        ]
        for label, source in cases:
            config.write_bytes(source)
            runs = [
                subprocess.run(
                    [str(binary), "--check", str(config)],
                    capture_output=True,
                    timeout=3,
                )
                for binary in (REFERENCE, BIN)
            ]
            assert runs[0].returncode == 1, (label, runs[0])
            assert (runs[0].returncode, runs[0].stdout, runs[0].stderr) == (
                runs[1].returncode,
                runs[1].stdout,
                runs[1].stderr,
            ), (label, runs)
            print(f"ok   CLI {label} matches C exactly")
        config.unlink()
        runs = [
            subprocess.run(
                [str(binary), "--check", str(config)], capture_output=True, timeout=3
            )
            for binary in (REFERENCE, BIN)
        ]
        assert runs[0].returncode == runs[1].returncode == 1
        assert runs[0].stderr == runs[1].stderr, runs
        print("ok   CLI missing native filename matches C exactly")
        for runner in (script, daemon):
            replies = [runner(binary, config, cases) for binary in (REFERENCE, BIN)]
            assert replies[0] == replies[1], (runner.__name__, replies)


if __name__ == "__main__":
    main()
