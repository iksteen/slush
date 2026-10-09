#!/usr/bin/env python3
"""Compare native command bytes and JSON string decoding with original slosh."""

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
PROGRAM = """import os,tty
tty.setraw(0)
os.write(1,b'READY\\r\\n')
data=bytearray()
while True:
    for byte in os.read(0,4096):
        data.append(byte)
        if byte in (10,13):
            os.write(1,b'\\r\\nDATA='+data.hex().encode()+b'\\r\\n')
            data.clear()
"""
COMMAND = [sys.executable, "-c", PROGRAM]
CASES = [
    (
        b'{"cmd":"raw","data":"json-\xff\xfe\\n"}',
        b"json-\xff\xfe\n",
        "native JSON bytes",
    ),
    (
        rb'{"cmd":"raw","data":"\u00e9\ud83d\ude80\n"}',
        "é🚀\n".encode(),
        "Unicode escape pair",
    ),
    (rb'{"cmd":"raw","data":"\udcff\n"}', b"\xed\xb3\xbf\n", "unpaired Unicode escape"),
    (b"raw bare-\xff\xfe\\n", b"bare-\xff\xfe\n", "native bare raw command"),
    (
        b'{"cmd":"send","data":"keys-\xff\xfe\\n"}',
        b"keys-\xff\xfe\r",
        "native keyboard input",
    ),
]


def check(binary, request, capture, pump, transport):
    def until(mark):
        deadline = time.monotonic() + 5
        while True:
            pump()
            text = capture()
            if mark in text:
                return
            assert time.monotonic() < deadline, (binary, transport, mark, text)
            time.sleep(0.01)

    until("READY")
    for command, expected, description in CASES:
        response = request(command)
        if command.startswith(b"{"):
            assert response["ok"], (binary, transport, response)
        until("DATA=" + expected.hex())
        print(
            f"ok   {binary.name}: {transport} {description} reaches the pane unchanged"
        )
    assert request(rb'{"cmd":"raw","data":"before\u0000ignored"}')["ok"]
    assert request(rb'{"cmd":"raw","data":"\n"}')["ok"]
    until("DATA=" + b"before\n".hex())
    print(
        f"ok   {binary.name}: {transport} decoded NUL uses the original string boundary"
    )
    for bad in (rb'{"cmd":"raw","data":"\u1"}', b'{"cmd":"raw","data":"\\'):
        response = request(bad)
        assert response == {"ok": False, "error": "malformed json"}, (binary, response)
    assert request(b'{"cmd":"alive","ignored":+0x1p2}')["ok"]
    print(
        f"ok   {binary.name}: {transport} malformed strings and numeric extensions match the reference"
    )


def script(binary):
    process = subprocess.Popen(
        [str(binary), "--script", "--cols", "240", "--rows", "40", "--", *COMMAND],
        env=dict(os.environ, SLOSH_CONFIG="/nonexistent/slosh.kdl"),
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )

    def request(command):
        process.stdin.write(command + b"\n")
        process.stdin.flush()
        if command.startswith(b"{"):
            return json.loads(process.stdout.readline())
        return None

    try:
        check(
            binary,
            request,
            lambda: request(b'{"cmd":"capture"}')["text"],
            lambda: request(b"settle 10"),
            "script",
        )
    finally:
        request(b"quit")
        process.stdin.close()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        process.stdout.close()
        process.stderr.close()


def daemon(binary, directory):
    env = dict(
        os.environ,
        XDG_RUNTIME_DIR=str(directory),
        SLOSH_CONFIG="/nonexistent/slosh.kdl",
    )
    env.pop("SLOSH", None)
    env.pop("SLOSH_SESSION", None)
    process = subprocess.Popen(
        [str(binary), "-s", "request", "--cols", "240", "--rows", "40", "--", *COMMAND],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )

    def request(command):
        result = subprocess.run(
            [str(binary), "-s", "request", "cmd", os.fsdecode(command)],
            env=env,
            capture_output=True,
            timeout=3,
        )
        if result.returncode:
            return None
        return json.loads(result.stdout) if command.startswith(b"{") else None

    deadline = time.monotonic() + 5
    try:
        while request(b'{"cmd":"alive"}') is None:
            assert time.monotonic() < deadline, (binary, "daemon did not start")
            time.sleep(0.02)
        check(
            binary,
            request,
            lambda: request(b'{"cmd":"capture"}')["text"],
            lambda: None,
            "socket",
        )
    finally:
        request(b"quit")
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        process.stderr.close()


def main():
    for binary in (REFERENCE, BIN):
        script(binary)
        with tempfile.TemporaryDirectory(prefix="rs-request-", dir="/tmp") as temporary:
            daemon(binary, Path(temporary))


if __name__ == "__main__":
    main()
