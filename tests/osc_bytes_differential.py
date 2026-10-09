#!/usr/bin/env python3
"""Native OSC payloads, statuses, button labels and shader preset filenames."""

import base64
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

PROGRAM = """import os,tty
tty.setraw(0)
os.write(1,b'\\033]2;probe\\007READY\\r\\n')
reply=b'\\033]5577;1;shader-reply;'
pending=b''
while True:
    pending+=os.read(0,4096)
    if reply.startswith(pending): continue
    if pending.startswith(reply):
        if b'\\033\\\\' not in pending: continue
        message,pending=pending.split(b'\\033\\\\',1)
        os.write(1,b'\\r\\nREPLY='+ (message+b'\\033\\\\').hex().encode()+b'\\r\\n')
    if pending:
        os.write(1,pending)
        pending=b''
"""
COMMAND = [sys.executable, "-c", PROGRAM]


def encoded(command, **fields):
    return json.dumps(dict(cmd=command, **fields), ensure_ascii=False)


def exercise(request, pump, config, preset, transport, binary):
    def until(predicate):
        deadline = time.monotonic() + 5
        while True:
            pump()
            if predicate():
                return
            assert time.monotonic() < deadline, (binary, transport, "timeout")
            time.sleep(0.005)

    def output():
        return request("capture")["text"]

    def emit(payload, terminator=b"\x1b\\", code=b"5577;1;"):
        assert request("raw", data=os.fsdecode(b"\x1b]" + code + payload + terminator))[
            "ok"
        ]

    def status():
        return os.fsencode(request("panes")["panes"][0]["status"])

    def chrome():
        screen = request("snapshot")["screen"]
        footer = screen["rows"] - (1 if b"compact true" in config.read_bytes() else 2)
        return (
            [os.fsencode(row) for y, row in enumerate(screen["text"]) if y != footer],
            [style for style in screen["styles"] if style["y"] != footer],
            sorted(
                (h["action"], h["x"], h["y"], h["w"], h["h"]) for h in screen["hits"]
            ),
        )

    until(lambda: "READY" in output())
    results = {}
    for i, value in enumerate(
        (
            b"building-\xff\xfe;unit=a/b  running another-\xff\xfe step",
            b"x" * 254 + "é".encode(),
            b"z" * 280,
        )
    ):
        emit(b"status;" + value, b"\x07" if i % 2 else b"\x1b\\")
        expected = value[:255]
        until(lambda: status() == expected)
        results[f"status {i}"] = status()
        results[f"status chrome {i}"] = chrome()
        print(
            f"ok   {binary.name}: {transport} status {i} keeps the original bytes and 255-byte cap"
        )

    title, message = b"a" * 128, b"b" * 124 + "é".encode()
    emit(
        b"state=working:title="
        + base64.b64encode(title)
        + b":msg="
        + base64.b64encode(message),
        code=b"7501;",
    )
    expected = (title + b": " + message)[:255]
    until(lambda: status() == expected)
    results["composed 7501 status"] = status()
    print(
        f"ok   {binary.name}: {transport} composed 7501 status follows C byte truncation"
    )

    emit(b"status;status-\xff")
    until(lambda: status() == b"status-\xff")
    for i, buttons in enumerate(
        (
            b"approve:yes-\xff;cancel:no-%FE",
            b"one:" + b"q" * 31 + "é".encode(),
            b"short:before%00ignored;bad%FF:dropped",
        )
    ):
        emit(b"buttons;" + buttons)
        # The following status update is a barrier in the same PTY stream.
        barrier = f"barrier-{i}".encode()
        emit(b"status;" + barrier)
        until(lambda: status() == barrier)
        screen = request("snapshot")["screen"]
        row = request("panes")["panes"][0]["y"] + request("panes")["panes"][0]["h"] - 1
        results[f"buttons {i}"] = (
            os.fsencode(screen["text"][row]),
            sorted(
                (h["action"], h["x"], h["y"], h["w"], h["h"])
                for h in screen["hits"]
                if h["action"].startswith("btn:")
            ),
        )
        print(
            f"ok   {binary.name}: {transport} button labels {i} retain raw and percent-decoded bytes"
        )

    for name, payload, answer in (
        (
            "native preset path",
            b"shader-load;" + os.fsencode(preset),
            b"ok;1 chrome, 1 content",
        ),
        (
            "inline native comment",
            b"shader;content;dim amount=0 // native-\xff",
            b"ok;0 chrome, 1 content",
        ),
        (
            "native missing filename",
            b"shader-load;" + os.fsencode(preset.with_name("missing-\udcff.kdl")),
            b"error;cannot open " + os.fsencode(preset.with_name("missing-\udcff.kdl")),
        ),
    ):
        emit(payload)
        wanted = (b"\x1b]5577;1;shader-reply;" + answer + b"\x1b\\").hex()
        until(lambda: "REPLY=" + wanted in output())
        results[name] = wanted
        print(f"ok   {binary.name}: {transport} {name} matches original reply bytes")
    return results


def script(binary, config, preset):
    harness.BIN = str(binary)
    with harness.Session(COMMAND, config=config, cols=240, rows=40) as active:
        active.proc.stdin.reconfigure(errors="surrogateescape")
        active.proc.stdout.reconfigure(errors="surrogateescape")

        def request(command, **fields):
            active._cmd(encoded(command, **fields))
            return json.loads(active.proc.stdout.readline())

        return exercise(
            request, lambda: active.settle(5), config, preset, "script", binary
        )


def daemon(binary, config, preset):
    env = dict(os.environ, SLOSH_CONFIG=str(config), XDG_RUNTIME_DIR=str(config.parent))
    env.pop("SLOSH", None)
    env.pop("SLOSH_SESSION", None)
    process = subprocess.Popen(
        [
            str(binary),
            "-s",
            "oscbytes",
            "--cols",
            "240",
            "--rows",
            "40",
            "--",
            *COMMAND,
        ],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )

    def request(command, **fields):
        result = subprocess.run(
            [str(binary), "-s", "oscbytes", "cmd", encoded(command, **fields)],
            env=env,
            capture_output=True,
            timeout=3,
        )
        return (
            json.loads(result.stdout.decode(errors="surrogateescape"))
            if result.returncode == 0
            else None
        )

    deadline = time.monotonic() + 5
    try:
        while request("alive") is None:
            assert time.monotonic() < deadline, (binary, "daemon startup")
            time.sleep(0.01)
        return exercise(request, lambda: None, config, preset, "socket", binary)
    finally:
        request("quit")
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        process.stderr.close()


def main():
    with tempfile.TemporaryDirectory(prefix="rs-osc-bytes-", dir="/tmp") as temporary:
        directory = Path(temporary)
        config = directory / "config.kdl"
        preset = directory / "preset-\udcff.kdl"
        source = b'// native comment \xff\nshaders { dim amount=0; dim amount=0 where="chrome" }\n'
        try:
            preset.write_bytes(source)
        except OSError as error:
            if error.errno != errno.EILSEQ:
                raise
            preset = directory / "preset-é.kdl"
            preset.write_bytes(source)
            print(
                "skip raw preset filename rejected by filesystem; raw OSC bytes still tested"
            )
        for side, lines, compact in (
            ("top", 1, False),
            ("left", 3, False),
            ("right", 1, True),
        ):
            config.write_text(
                f'in_band_shaders true\nsplash_ms 0\ntab_bar_side "{side}"\ntab_bar_width 26\ntab_bar_status_lines {lines}\ncompact {str(compact).lower()}\n'
            )
            for runner in (script, daemon):
                results = [
                    runner(binary, config, preset) for binary in (REFERENCE, BIN)
                ]
                differences = {
                    name: (results[0][name], results[1][name])
                    for name in results[0]
                    if results[0][name] != results[1][name]
                }
                assert not differences, (side, runner.__name__, differences)


if __name__ == "__main__":
    main()
