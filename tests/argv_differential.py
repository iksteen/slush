#!/usr/bin/env python3
"""Compare native Unix pane arguments and executable paths with original slosh."""

import errno
import json
import os
import pty
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path

from _reference import reference_binary

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests/reference/slosh/tests"))
import harness  # noqa: E402

BIN = Path(os.environ.get("SLOSH_BIN", ROOT / "target/debug/slush")).resolve()
REFERENCE = reference_binary()
ARGUMENTS = [b"arg-\xff\xfe", b"", b" two words ", "päne".encode(), b"line\nbreak"]
MARK = "ARGV=" + ",".join(argument.hex() for argument in ARGUMENTS)
PROGRAM = (
    "import os,sys; "
    "print('ARGV='+','.join(os.fsencode(a).hex() for a in sys.argv[1:]),flush=True); "
    "sys.stdin.readline()"
)


def pane_arguments(binary, command, config):
    harness.BIN = str(binary)
    with harness.Session(command, cols=220, rows=40, config=config) as session:
        assert MARK in session.until_text(MARK).screen(), binary
        print(
            f"ok   {binary.name}: native executable and argument bytes reach the pane"
        )
        assert session.api("split", dir="cols")["ok"]
        for pane in session.panes():
            pane_id = pane["id"]
            session.until(
                lambda _snap: MARK in session.api("capture", id=pane_id)["text"]
            )
            assert MARK in session.api("capture", id=pane_id)["text"], pane
        print(f"ok   {binary.name}: splitting preserves all native arguments")
        assert session.api("new-tab")["ok"]
        assert MARK in session.until_text(MARK).screen()
        print(f"ok   {binary.name}: a new tab preserves all native arguments")
        pane_id = session.focused()["id"]
        assert session.api("raw", id=pane_id, data="\n")["ok"]
        session.until(lambda _snap: not session.focused()["alive"])
        assert not session.focused()["alive"]
        assert session.api("rerun", id=pane_id)["ok"]
        session.until(
            lambda _snap: session.api("capture", id=pane_id)["text"].count(MARK) == 2
        )
        assert session.api("capture", id=pane_id)["text"].count(MARK) == 2
        print(f"ok   {binary.name}: rerunning retains the original argument bytes")


def daemon_arguments(binary, command, directory, config):
    env = dict(os.environ, XDG_RUNTIME_DIR=str(directory), SLOSH_CONFIG=str(config))
    env.pop("SLOSH", None)
    env.pop("SLOSH_SESSION", None)
    master, slave = pty.openpty()
    termios.tcsetwinsize(slave, (40, 220))
    client = subprocess.Popen(
        [str(binary), "-s", "argv", "--", *command],
        stdin=slave,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        env=env,
    )
    os.close(slave)

    def control(request):
        return subprocess.run(
            [str(binary), "-s", "argv", "cmd", json.dumps(request)],
            capture_output=True,
            text=True,
            env=env,
            timeout=3,
        )

    try:
        deadline = time.monotonic() + 5
        while True:
            response = control({"cmd": "capture"})
            if response.returncode == 0 and MARK in json.loads(response.stdout)["text"]:
                break
            assert time.monotonic() < deadline, (binary, response)
            time.sleep(0.02)
        print(f"ok   {binary.name}: daemon startup preserves native command arguments")
    finally:
        control({"cmd": "quit"})
        try:
            client.wait(timeout=3)
        except subprocess.TimeoutExpired:
            client.kill()
            client.wait()
        client.stderr.close()
        os.close(master)


def main():
    with tempfile.TemporaryDirectory(prefix="rs-argv-", dir="/tmp") as temporary:
        directory = Path(temporary)
        config = directory / "config.kdl"
        config.write_text('splash_ms 0\nkeep_dead "all"\n')
        executable = os.fsencode(directory) + b"/python-\xff"
        native_filenames = True
        try:
            os.symlink(os.fsencode(sys.executable), executable)
        except OSError as error:
            if error.errno != errno.EILSEQ:
                raise
            native_filenames = False
            executable = os.fsencode(directory / "python")
            os.symlink(os.fsencode(sys.executable), executable)
            print("skip filesystem rejects non-UTF-8 filenames; testing argument bytes")
        command = [executable, "-c", PROGRAM, *ARGUMENTS]
        native_shell = os.fsencode(directory) + (
            b"/shell-\xfe" if native_filenames else b"/shell"
        )
        with open(native_shell, "wb") as script:
            script.write(b"#!/bin/sh\nprintf 'NATIVE-SHELL\\n'\nexec cat\n")
        os.chmod(native_shell, 0o700)
        for binary in (REFERENCE, BIN):
            # Bound the first regression before using the original script harness.
            result = subprocess.run(
                [str(binary), "--headless", "--cols", "220", "--", *command],
                input=b"",
                capture_output=True,
                env=dict(os.environ, SLOSH_CONFIG=str(config)),
                timeout=5,
            )
            assert result.returncode == 0 and MARK.encode() in result.stdout, result
            pane_arguments(binary, command, config)
            harness.BIN = str(binary)
            with harness.Session(
                [], config=config, env={"SHELL": os.fsdecode(native_shell)}
            ) as session:
                assert "NATIVE-SHELL" in session.until_text("NATIVE-SHELL").screen()
                assert session.api("split", dir="cols")["ok"]
                assert (
                    session.until(
                        lambda snapshot: snapshot.screen().count("NATIVE-SHELL") == 2
                    )
                    .screen()
                    .count("NATIVE-SHELL")
                    == 2
                )
            print(f"ok   {binary.name}: the default shell retains its native path")
            daemon_arguments(binary, command, directory, config)
            result = subprocess.run(
                [str(binary), "--dump-config", b"unused-\xff"],
                capture_output=True,
                timeout=3,
            )
            assert result.returncode == 0 and b"prefix" in result.stdout, result
            print(f"ok   {binary.name}: utility modes ignore unused native arguments")


if __name__ == "__main__":
    main()
