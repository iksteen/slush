#!/usr/bin/env python3
"""Compare native pane names, terminal titles, and saved commands on Unix."""

import json
import os
import re
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
COMMAND = ["/bin/sh", "-i"]
PROGRAM = (
    "import os,sys; os.write(1,b'\\x1b]2;'+b't'*254+'é'.encode()+b'\\x07'); "
    "print('ARGV='+os.fsencode(sys.argv[1]).hex(),flush=True); sys.stdin.readline(); "
    "os.execv(sys.executable,[sys.executable,'-c',\"import sys; print(bytes.fromhex('485547455245414459').decode(),flush=True); sys.stdin.read()\",os.fsdecode(bytes([255])+bytes([39])*1400)])"
)
ARGUMENT = b"native-\xff'\xfe space\\\""
CAPTURED_LIMITS = {}
CAPTURED_COMMANDS = {}


def shell_command():
    arguments = [os.fsencode(sys.executable), b"-c", PROGRAM.encode(), ARGUMENT]
    safe = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_./:=@%+-,"
    return b" ".join(
        a
        if a and all(b in safe for b in a)
        else b"'" + a.replace(b"'", b"'\\''") + b"'"
        for a in arguments
    )


def quote(value):
    return value.replace(b"\\", b"\\\\").replace(b'"', b'\\"')


def check(binary, transport, request, directory):
    mark = "ARGV=" + ARGUMENT.hex()

    def until():
        deadline = time.monotonic() + 5
        while mark not in request({"cmd": "capture"})["text"]:
            assert time.monotonic() < deadline, (
                binary,
                transport,
                "native argument timed out",
            )
            time.sleep(0.01)

    command = shell_command()
    assert request({"cmd": "raw", "data": os.fsdecode(command + b"\n")})["ok"]
    until()
    panes = request("panes")
    pid = panes[0]["id"]
    title = b"t" * 254 + b"\xc3"
    assert os.fsencode(panes[0]["title"]) == title, (binary, transport, panes)
    print(f"ok   {binary.name}: {transport} terminal titles truncate by bytes")
    for name in (
        b'pane-\xff"\\\x01\n',
        b"x" * 254 + "é".encode() + b"tail",
        b"prefix\0tail",
        b"",
    ):
        assert request(
            {"cmd": "set-name", "target": "pane", "name": os.fsdecode(name)}
        )["ok"]
        expected = name.split(b"\0")[0][:255] or title
        for response in (request("panes"), request({"cmd": "panes"})["panes"]):
            assert os.fsencode(response[0]["title"]) == expected, (
                binary,
                transport,
                response,
                expected,
            )
        print(
            f"ok   {binary.name}: {transport} pane name preserves controls and native bytes"
        )
    assert not request({"cmd": "set-name", "target": "pane", "id": -1, "name": "bad"})[
        "ok"
    ]
    dumped = request({"cmd": "dump-layout", "suspend": "none"})
    data = os.fsencode(dumped["kdl"])
    # macOS's Python launcher rewrites argv[0] to its framework executable.
    # Verify the requested arguments and compare the full kernel result to C.
    assert quote(command[command.index(b" -c ") :]) + b'"' in data, (
        binary,
        transport,
        dumped,
        command,
    )
    field = re.search(rb' command="((?:\\.|[^"\\])*)"', data)
    assert field
    if binary == REFERENCE:
        CAPTURED_COMMANDS[transport] = field[1]
    else:
        assert field[1] == CAPTURED_COMMANDS[transport], (binary, transport, field[1])
    print(
        f"ok   {binary.name}: {transport} captured foreground command retains argument bytes"
    )
    kdl = b'layout { tab { pane command="' + quote(command) + b'" } }'
    restored = request({"cmd": "apply-layout", "kdl": os.fsdecode(kdl)})
    assert restored["ok"], (binary, transport, restored)
    until()
    dumped = request(
        {"cmd": "dump-layout", "tab": restored["tabs"][-1]["id"], "suspend": "none"}
    )
    data = os.fsencode(dumped["kdl"])
    assert b'command="' + quote(command) + b'"' in data, (
        binary,
        transport,
        dumped,
        command,
    )
    print(
        f"ok   {binary.name}: {transport} declared command label retains argument bytes"
    )
    for fields in (
        {"kdl": os.fsdecode(data)},
        {"path": str(directory / "pane.layout")},
    ):
        (directory / "pane.layout").write_bytes(data)
        restored = request({"cmd": "apply-layout", **fields})
        assert restored["ok"], (binary, transport, restored)
        until()
        print(
            f"ok   {binary.name}: {transport} saved native command runs after layout restore"
        )
    long_command = b"#" + b"a" * 1021 + "é".encode()
    kdl = b'layout { tab { pane command="' + long_command + b'" suspended=true } }'
    restored = request({"cmd": "apply-layout", "kdl": os.fsdecode(kdl)})
    assert restored["ok"]
    dumped = request({"cmd": "dump-layout", "tab": restored["tabs"][-1]["id"]})
    assert b'command="' + long_command[:1023] + b'"' in os.fsencode(dumped["kdl"]), (
        binary,
        transport,
        dumped,
    )
    assert request({"cmd": "panes"})["panes"][-1]["suspended"]
    assert request({"cmd": "panes"})["panes"][0]["id"] == pid
    print(
        f"ok   {binary.name}: {transport} command label length matches the original byte limit"
    )
    assert request({"cmd": "select-tab", "id": panes[0]["tab_id"]})["ok"]
    assert request({"cmd": "raw", "id": pid, "data": "huge\n"})["ok"]
    deadline = time.monotonic() + 5
    while "HUGEREADY" not in request({"cmd": "capture"})["text"]:
        assert time.monotonic() < deadline, (
            binary,
            transport,
            "large argument timed out",
        )
        time.sleep(0.01)
    dumped = request({"cmd": "dump-layout", "tab": panes[0]["tab_id"]})
    field = re.search(rb' command="((?:\\.|[^"\\])*)"', os.fsencode(dumped["kdl"]))
    assert field, (binary, transport, dumped)
    if binary == REFERENCE:
        CAPTURED_LIMITS[transport] = field[1]
    else:
        assert field[1] == CAPTURED_LIMITS[transport], (
            binary,
            transport,
            len(field[1]),
            len(CAPTURED_LIMITS[transport]),
        )
    print(
        f"ok   {binary.name}: {transport} captured command quoting matches the C buffer limit"
    )


def script(binary, directory):
    harness.BIN = str(binary)
    with harness.Session(COMMAND, cols=240, rows=40) as active:
        active.proc.stdin.reconfigure(errors="surrogateescape")
        active.proc.stdout.reconfigure(errors="surrogateescape")

        def request(value):
            if isinstance(value, dict) and value.get("cmd") == "capture":
                active.settle(10)
            active._cmd(
                value
                if isinstance(value, str)
                else json.dumps(value, ensure_ascii=False)
            )
            return json.loads(active.proc.stdout.readline())

        check(binary, "script", request, directory)


def socket(binary, directory):
    env = dict(
        os.environ,
        XDG_RUNTIME_DIR=str(directory),
        SLOSH_CONFIG="/nonexistent/slosh.kdl",
    )
    env.pop("SLOSH", None)
    env.pop("SLOSH_SESSION", None)
    process = subprocess.Popen(
        [
            str(binary),
            "-s",
            "metadata",
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

    def request(value):
        command = (
            value if isinstance(value, str) else json.dumps(value, ensure_ascii=False)
        )
        result = subprocess.run(
            [str(binary), "-s", "metadata", "cmd", command],
            env=env,
            capture_output=True,
            timeout=3,
        )
        return (
            json.loads(result.stdout.decode(errors="surrogateescape"))
            if result.returncode == 0
            else None
        )

    def wait_for(predicate):
        deadline = time.monotonic() + 5
        while not predicate():
            assert time.monotonic() < deadline, (binary, "metadata daemon timed out")
            time.sleep(0.02)

    try:
        wait_for(lambda: request({"cmd": "alive"}))
        check(binary, "socket", request, directory)
    finally:
        request({"cmd": "quit"})
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        process.stderr.close()
    wait_for(lambda: not (directory / "slosh/metadata.sock").exists())


def main():
    for binary in (REFERENCE, BIN):
        with tempfile.TemporaryDirectory(
            prefix="rs-pane-metadata-", dir="/tmp"
        ) as temporary:
            directory = Path(temporary)
            script(binary, directory)
            socket(binary, directory)


if __name__ == "__main__":
    main()
