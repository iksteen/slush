#!/usr/bin/env python3
"""Verify copy delivery belongs to the selecting client over the real protocol."""

import base64
import json
import os
import select
import socket
import struct
import subprocess
import tempfile
import time
from pathlib import Path

BIN = Path(
    os.environ.get(
        "SLOSH_BIN", Path(__file__).resolve().parents[1] / "target/debug/slush"
    )
).resolve()


def send(conn, kind, payload):
    conn.sendall(struct.pack(">BI", kind, len(payload)) + payload)


def receive(conn, size):
    data = bytearray()
    while len(data) < size:
        chunk = conn.recv(size - len(data))
        if not chunk:
            raise EOFError("server disconnected")
        data.extend(chunk)
    return bytes(data)


def drain(conn):
    messages = []
    while select.select([conn], [], [], 0.15)[0]:
        kind, size = struct.unpack(">BI", receive(conn, 5))
        payload = receive(conn, size)
        if kind == 5:
            messages.append(payload)
    return b"".join(messages)


def control(path, text):
    with socket.socket(socket.AF_UNIX) as conn:
        conn.settimeout(3)
        conn.connect(str(path))
        send(conn, 7, text.encode())
        kind, size = struct.unpack(">BI", receive(conn, 5))
        assert kind == 8
        return receive(conn, size)


def main():
    with tempfile.TemporaryDirectory(prefix="slush-clipboard-") as tmp:
        path = Path(tmp) / "slosh/clipboard.sock"
        env = dict(os.environ, XDG_RUNTIME_DIR=tmp, SLOSH_CONFIG=tmp + "/none.kdl")
        child = "printf 'hello-selection\\r\\n'; stty raw -echo; while IFS= read -r l; do eval \"$l\"; done"
        with (Path(tmp) / "server.log").open("w+") as log:
            server = subprocess.Popen(
                [str(BIN), "--server", "-s", "clipboard", "--", "/bin/sh", "-c", child],
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=log,
            )
            conns = []
            try:
                end = time.monotonic() + 3
                while (
                    not path.exists()
                    and server.poll() is None
                    and time.monotonic() < end
                ):
                    time.sleep(0.01)
                assert path.exists(), "server did not create its socket"
                for _ in range(2):
                    conn = socket.socket(socket.AF_UNIX)
                    conn.settimeout(3)
                    conn.connect(str(path))
                    conns.append(conn)
                    send(conn, 1, struct.pack(">4H", 60, 12, 8, 16))
                    drain(conn)
                for conn in conns:
                    drain(conn)
                pane = json.loads(control(path, "panes"))[0]
                x, y = pane["content_x"] + 1, pane["content_y"] + 1
                gesture = f"\x1b[<0;{x};{y}M\x1b[<32;{x + 14};{y}M\x1b[<0;{x + 14};{y}m"
                send(conns[0], 2, gesture.encode())
                expected = (
                    b"\x1b]52;c;" + base64.b64encode(b"hello-selection") + b"\x1b\\"
                )
                assert expected in drain(conns[0]), (
                    "selecting client did not receive OSC 52"
                )
                assert b"\x1b]52;" not in drain(conns[1]), (
                    "copy leaked to another attached client"
                )
                print("ok   a selection copies to its originating client only")
                assert (
                    json.loads(control(path, '{"cmd":"clipboard"}'))["text"]
                    == "hello-selection"
                )
                print("ok   the server retains the primary selection")

                # Focus is not typing: it must not change the active copy recipient.
                send(conns[1], 2, b"\x1b[I")
                drain(conns[0])
                drain(conns[1])
                command = "printf '\\033]52;c;ZnJvbS1wYW5l\\007'\n"
                reply = control(path, json.dumps({"cmd": "raw", "data": command}))
                assert json.loads(reply)["ok"]
                expected = b"\x1b]52;c;ZnJvbS1wYW5l\x1b\\"
                assert expected in drain(conns[0]), (
                    "pane copy did not reach the active client"
                )
                assert b"\x1b]52;" not in drain(conns[1]), (
                    "focus event stole the pane clipboard"
                )
                print("ok   pane clipboard writes reach the last active client")
            finally:
                for conn in conns:
                    conn.close()
                if server.poll() is None:
                    server.terminate()
                server.wait(timeout=5)
            assert not path.exists(), "server left its socket behind"
            print("ok   cleanup removes the session socket")


if __name__ == "__main__":
    main()
