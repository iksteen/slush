#!/usr/bin/env python3
"""Compare session CLI discovery and socket routing with the original binary."""

import errno
import json
import os
import socket
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from _reference import reference_binary

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get("SLOSH_BIN", ROOT / "target/debug/slush")).resolve()
REFERENCE = reference_binary()


def receive(connection, size):
    data = bytearray()
    while len(data) < size:
        chunk = connection.recv(size - len(data))
        assert chunk, "control client disconnected before sending its command"
        data.extend(chunk)
    return bytes(data)


def control(binary, directory, name, command=b'{"cmd":"alive"}'):
    # Concatenation matches the original namespace, including leading slashes.
    path = Path(f"{directory}/slosh/{name}.sock")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.unlink(missing_ok=True)
    reply = b'{"ok":true,"fixture":"session-route"}'
    with socket.socket(socket.AF_UNIX) as listener:
        listener.bind(str(path))
        listener.listen(1)
        listener.settimeout(3)
        process = subprocess.Popen(
            [str(binary), "-s", name, "cmd", os.fsdecode(command)],
            env=dict(os.environ, XDG_RUNTIME_DIR=str(directory)),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        try:
            with listener.accept()[0] as connection:
                connection.settimeout(3)
                kind, size = struct.unpack(">BI", receive(connection, 5))
                assert kind == 7
                assert receive(connection, size) == command
                # The control client must ignore unrelated frames.
                connection.sendall(struct.pack(">BI", 5, 7) + b"ignored")
                connection.sendall(struct.pack(">BI", 8, len(reply)) + reply)
            stdout, stderr = process.communicate(timeout=3)
            assert process.returncode == 0, (binary, name, stderr)
            assert stdout == reply + b"\n", (binary, name, stdout)
        finally:
            if process.poll() is None:
                process.kill()
            process.wait()
            process.stdout.close()
            process.stderr.close()
    path.unlink()


def listing(binary, directory, names):
    root = directory / "slosh"
    root.mkdir(parents=True, exist_ok=True)
    for name in names:
        (root / f"{name}.sock").touch()
    for ignored in (".sock", "ignored.log", "not.sock.bak"):
        (root / ignored).touch()
    live = root / "live.sock"
    with socket.socket(socket.AF_UNIX) as listener:
        listener.bind(str(live))
        listener.listen(16)
        result = subprocess.run(
            [str(binary), "ls"],
            env=dict(os.environ, XDG_RUNTIME_DIR=str(directory)),
            capture_output=True,
            timeout=3,
        )
        assert result.returncode == 0 and not result.stderr, (binary, result)
        expected = []
        for filename in os.listdir(os.fsencode(root)):
            if filename.endswith(b".sock") and len(filename) > 5:
                name = filename[:-5]
                state = b"running" if name == b"live" else b"stale"
                expected.append(name.ljust(20) + b" " + state)
        assert sorted(result.stdout.splitlines()) == sorted(expected), (
            binary,
            result.stdout,
            expected,
        )
    live.unlink()
    return result.stdout


def native_daemon(binary, directory, name):
    env = dict(
        os.environ,
        XDG_RUNTIME_DIR=str(directory),
        SLOSH_CONFIG="/nonexistent/slosh.kdl",
    )
    env.pop("SLOSH", None)
    env.pop("SLOSH_SESSION", None)
    code = "import os,sys; print('SESSION='+os.environb.get(b'SLOSH_SESSION',b'').hex(),flush=True); sys.stdin.read()"
    process = subprocess.Popen(
        [
            str(binary),
            "-s",
            name,
            "--cols",
            "120",
            "--rows",
            "24",
            "--",
            sys.executable,
            "-c",
            code,
        ],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        env=env,
    )

    def command(request):
        result = subprocess.run(
            [str(binary), "-s", name, "cmd", json.dumps(request)],
            env=env,
            capture_output=True,
            timeout=3,
        )
        if result.returncode:
            return None
        return json.loads(result.stdout.decode(errors="surrogateescape"))

    def wait_for(predicate):
        deadline = time.monotonic() + 5
        while not predicate():
            assert time.monotonic() < deadline, (
                binary,
                name,
                "session did not reach expected state",
            )
            time.sleep(0.02)

    expected = "SESSION=" + os.fsencode(name).hex()
    try:
        wait_for(
            lambda: (
                (reply := command({"cmd": "capture"})) is not None
                and expected in reply["text"]
            )
        )
        print(
            f"ok   {binary.name}: native session {name!r} reaches daemon sockets and pane environments"
        )
        assert command({"cmd": "split", "dir": "rows"})["ok"]
        panes = command({"cmd": "panes"})["panes"]
        for pane in panes:
            wait_for(
                lambda: (
                    expected in command({"cmd": "capture", "id": pane["id"]})["text"]
                )
            )
        assert command({"cmd": "new-tab"})["ok"]
        wait_for(lambda: expected in command({"cmd": "capture"})["text"])
        print(
            f"ok   {binary.name}: native session identities survive splits and new tabs"
        )
        nested = dict(env, SLOSH="1", SLOSH_SESSION=name)
        result = subprocess.run(
            [str(binary), "-s", name], env=nested, capture_output=True, timeout=3
        )
        assert (
            result.returncode == 1
            and b"this pane is already inside session" in result.stderr
        ), result
        assert not result.stdout
        print(
            f"ok   {binary.name}: recursion protection compares the native session identity"
        )
    finally:
        command({"cmd": "quit"})
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        process.stderr.close()
    socket_path = Path(f"{directory}/slosh/{name}.sock")
    wait_for(lambda: not socket_path.exists())
    print(f"ok   {binary.name}: native session cleanup removes its exact socket path")


def main():
    # Keep AF_UNIX paths short on Darwin too, regardless of its default TMPDIR.
    with tempfile.TemporaryDirectory(prefix="rs-cli-", dir="/tmp") as temporary:
        directory = Path(temporary)
        unavailable = [directory / "missing", directory / "file", directory / "bad"]
        unavailable[1].write_text("fixture")
        unavailable[2].mkdir()
        (unavailable[2] / "slosh").write_text("fixture")
        for base in unavailable:
            for binary in (REFERENCE, BIN):
                result = subprocess.run(
                    [str(binary), "ls"],
                    env=dict(os.environ, XDG_RUNTIME_DIR=str(base)),
                    capture_output=True,
                    timeout=3,
                )
                assert (result.returncode, result.stdout, result.stderr) == (
                    0,
                    b"",
                    b"",
                ), (
                    binary,
                    base,
                    result,
                )
            print(f"ok   unavailable runtime {base.name!r} lists no sessions")

        native_name = os.fsdecode(b"native-\xff")
        try:
            probe = directory / native_name
            probe.touch()
            probe.unlink()
        except OSError as error:
            if error.errno != errno.EILSEQ:
                raise
            native_name = None
            print("skip filesystem rejects non-UTF-8 session filenames")
        names = ["normal", "/leading", "//double", "nested/leaf", "a session", "päne"]
        if native_name is not None:
            names.append(native_name)
        for name in names:
            for binary in (REFERENCE, BIN):
                control(binary, directory, name)
            print(f"ok   session {name!r} routes through the runtime namespace")
        for command in (
            b"raw \xff\xfe",
            b'{"cmd":"alive","ignored":"\xff"}',
            b'{"cmd":"open-workspace","path":"/tmp/native-\xfe"}',
        ):
            for binary in (REFERENCE, BIN):
                control(binary, directory, "native-request", command)
            print(
                f"ok   native command argument {command!r} reaches its socket unchanged"
            )
        list_names = [
            "päne",
            "e\u0301",
            "a session",
            ".hidden",
            "long-session-name-over-twenty",
        ]
        if native_name is not None:
            list_names.append(native_name)
        outputs = [
            listing(binary, directory / "listing", list_names)
            for binary in (REFERENCE, BIN)
        ]
        assert sorted(outputs[0].splitlines()) == sorted(outputs[1].splitlines())
        print(
            "ok   session listing preserves native names, byte padding, and running/stale state"
        )
        for name in ("päne", native_name):
            if name is not None:
                for binary in (REFERENCE, BIN):
                    native_daemon(binary, directory, name)


if __name__ == "__main__":
    main()
