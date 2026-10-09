#!/usr/bin/env python3
"""Compare byte-based project names, slugs, and lookup with original slosh."""

import errno
import json
import os
import shlex
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
COMMAND = [sys.executable, "-c", "import sys; sys.stdin.read()"]


def identity(project):
    value = 2166136261
    for byte in os.fsencode(project.resolve()):
        value = ((value ^ byte) * 16777619) & 0xFFFFFFFF
    return b"project:" + os.fsencode(project.name)[:46] + f".{value:08x}".encode()


def check(binary, transport, request, listing, projects):
    entries = request({"cmd": "workspaces"})["workspaces"]
    expected = sorted(projects, key=lambda p: os.fsencode(p.name)[:63])
    assert len(entries) == len(expected), (binary, transport, entries)
    for entry, project in zip(entries, expected):
        assert os.fsencode(entry["name"]) == os.fsencode(project.name)[:63], (
            binary,
            transport,
            entry,
            project,
        )
        assert os.fsencode(entry["purpose"]) == identity(project), (
            binary,
            transport,
            entry,
            identity(project),
        )
    print(f"ok   {binary.name}: {transport} native names sort and truncate by bytes")
    print(f"ok   {binary.name}: {transport} slugs keep split UTF-8 and native bytes")
    rows = listing().splitlines()
    expected_rows = [
        os.fsencode(p.name)[:63].ljust(24) + b" .git      " + os.fsencode(p.resolve())
        for p in expected
    ]
    assert rows == expected_rows, (binary, transport, rows, expected_rows)
    print(f"ok   {binary.name}: {transport} bare listing preserves exact name bytes")
    for project in expected:
        name = os.fsencode(project.name)[:63]
        result = request({"cmd": "open-workspace", "name": os.fsdecode(name)})
        assert result["ok"] and result["created"], (binary, transport, result)
        assert os.fsencode(result["purpose"]) == identity(project), (
            binary,
            transport,
            result,
        )
        tabs = request({"cmd": "tabs"})["tabs"]
        tab = next(t for t in tabs if t["id"] == result["tab"])
        assert os.fsencode(tab["name"]) == name, (binary, transport, tab, name)
        # The original sanitizes membership but returns the raw project slug.
        # Compare both forms so a lossy conversion cannot accidentally refocus.
        sanitized = bytes(
            b
            for b in identity(project)
            if b
            in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_.:/-"
        )[:63]
        assert request({"cmd": "close-workspace", "name": os.fsdecode(name)})[
            "closed"
        ] == int(sanitized == identity(project)), (binary, transport, name)
        if sanitized != identity(project):
            assert (
                request(
                    {
                        "cmd": "close-workspace",
                        "purpose": os.fsdecode(identity(project)),
                    }
                )["closed"]
                == 0
            ), (binary, transport, name)
            assert (
                request({"cmd": "close-workspace", "purpose": sanitized.decode()})[
                    "closed"
                ]
                == 1
            ), (binary, transport, name)
        if len(os.fsencode(project.name)) > 63:
            assert not request({"cmd": "open-workspace", "name": project.name})["ok"], (
                binary,
                transport,
                project,
            )
    print(f"ok   {binary.name}: {transport} lookup uses the truncated native name")
    print(f"ok   {binary.name}: {transport} raw and sanitized purposes remain distinct")
    print(
        f"ok   {binary.name}: {transport} adopted tab labels retain project name bytes"
    )


def names_and_layouts(binary, transport, request, directory, project):
    for name in (b'name-\xff"\\\x01\n', b"n" * 62 + "é".encode() + b"z" * 20):
        created = request({"cmd": "new-tab", "name": os.fsdecode(name)})
        assert created["ok"], (binary, transport, created)
        tid = created["id"]

        def tab_name(reply=None):
            tabs = reply or request({"cmd": "tabs"})
            if isinstance(tabs, dict):
                tabs = tabs["tabs"]
            return os.fsencode(next(t for t in tabs if t["id"] == tid)["name"])

        assert tab_name() == name[:63], (binary, transport, tab_name(), name)
        assert tab_name(request("tabs")) == name[:63]
        renamed = name[::-1]
        assert request({"cmd": "set-name", "id": tid, "name": os.fsdecode(renamed)})[
            "ok"
        ]
        assert tab_name() == renamed[:63], (binary, transport, tab_name(), renamed)
        assert not request({"cmd": "set-name", "id": 0, "name": "ignored"})["ok"]
        assert not request({"cmd": "move-tab", "id": 0, "index": 1})["ok"]
        moved = request({"cmd": "move-tab", "id": tid, "index": 1})
        assert moved["ok"] and tab_name(moved) == renamed[:63], (
            binary,
            transport,
            moved,
        )
        print(
            f"ok   {binary.name}: {transport} tab creation, rename and move keep bytes"
        )
        dumped = request({"cmd": "dump-layout", "tab": tid, "suspend": "none"})
        kdl = os.fsencode(dumped["kdl"])
        expected = bytes(b for b in renamed[:63] if b >= 32)
        escaped = expected.replace(b"\\", b"\\\\").replace(b'"', b'\\"')
        assert b'name="' + escaped + b'"' in kdl, (binary, transport, kdl, escaped)
        restored = request({"cmd": "apply-layout", "kdl": os.fsdecode(kdl)})
        assert restored["ok"], (binary, transport, restored)
        assert os.fsencode(restored["tabs"][-1]["name"]) == expected, (
            binary,
            transport,
            restored,
            expected,
        )
        file = directory / "native.layout"
        file.write_bytes(kdl)
        restored = request({"cmd": "apply-layout", "path": str(file)})
        assert restored["ok"] and os.fsencode(restored["tabs"][-1]["name"]) == expected
        checked = subprocess.run(
            [str(binary), "--check", str(file)], capture_output=True, timeout=3
        )
        assert checked.returncode == 0, (
            binary,
            transport,
            checked.stdout,
            checked.stderr,
        )
        print(
            f"ok   {binary.name}: {transport} names round-trip through inline and file layouts"
        )
    command = shlex.join(
        [
            sys.executable,
            "-c",
            "import os,sys; print('LAYOUTCWD='+os.getcwdb().hex(),flush=True); sys.stdin.read()",
        ]
    ).encode()
    cwd = os.fsencode(project.resolve())
    escape = lambda v: v.replace(b"\\", b"\\\\").replace(b'"', b'\\"')
    kdl = (
        b'layout { tab { pane cwd="'
        + escape(cwd)
        + b'" command="'
        + escape(command)
        + b'" } }'
    )
    restored = request({"cmd": "apply-layout", "kdl": os.fsdecode(kdl)})
    assert restored["ok"], (binary, transport, restored)
    mark = "LAYOUTCWD=" + cwd.hex()
    deadline = time.monotonic() + 5
    while mark not in request({"cmd": "capture"})["text"]:
        assert time.monotonic() < deadline, (
            binary,
            transport,
            "native layout cwd timed out",
        )
        time.sleep(0.01)
    dumped = request({"cmd": "dump-layout", "tab": restored["tabs"][-1]["id"]})
    assert b'cwd="' + escape(cwd) + b'"' in os.fsencode(dumped["kdl"]), (
        binary,
        transport,
        dumped,
        cwd,
    )
    print(
        f"ok   {binary.name}: {transport} inline layout starts and dumps an exact native cwd"
    )
    split = request({"cmd": "split"})
    assert split["ok"], (binary, transport, split)
    name = b"moved-\xfe"
    moved = request({"cmd": "move-pane", "id": split["id"], "name": os.fsdecode(name)})
    assert moved["ok"], (binary, transport, moved)
    tabs = request("tabs")
    assert os.fsencode(next(t for t in tabs if t["id"] == moved["tab"])["name"]) == name
    print(
        f"ok   {binary.name}: {transport} moving a pane adopts an exact native tab name"
    )


def script(binary, config, projects):
    harness.BIN = str(binary)
    with harness.Session(COMMAND, config=config, cols=240, rows=40) as active:
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

        def listing():
            active._cmd("workspaces")
            lines = [active.proc.stdout.readline() for _ in projects]
            assert active.proc.stdout.readline() == "\n"
            return os.fsencode("".join(lines))

        check(binary, "script", request, listing, projects)
        names_and_layouts(binary, "script", request, config.parent, projects[0])


def socket(binary, config, projects):
    env = dict(os.environ, XDG_RUNTIME_DIR=str(config.parent), SLOSH_CONFIG=str(config))
    env.pop("SLOSH", None)
    env.pop("SLOSH_SESSION", None)
    process = subprocess.Popen(
        [str(binary), "-s", "names", "--cols", "240", "--rows", "40", "--", *COMMAND],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )

    def control(line):
        result = subprocess.run(
            [str(binary), "-s", "names", "cmd", line],
            env=env,
            capture_output=True,
            timeout=3,
        )
        return result.stdout if result.returncode == 0 else None

    def request(value):
        reply = control(
            value if isinstance(value, str) else json.dumps(value, ensure_ascii=False)
        )
        return json.loads(reply.decode(errors="surrogateescape")) if reply else None

    def wait_for(predicate):
        deadline = time.monotonic() + 5
        while not predicate():
            assert time.monotonic() < deadline, (binary, "names daemon timed out")
            time.sleep(0.02)

    try:
        wait_for(lambda: request({"cmd": "alive"}))
        check(
            binary,
            "socket",
            request,
            lambda: control("workspaces").rstrip(b"\n"),
            projects,
        )
        names_and_layouts(binary, "socket", request, config.parent, projects[0])
    finally:
        control('{"cmd":"quit"}')
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        process.stderr.close()
    wait_for(lambda: not (config.parent / "slosh/names.sock").exists())


def main():
    with tempfile.TemporaryDirectory(
        prefix="rs-project-names-", dir="/tmp"
    ) as temporary:
        root = Path(temporary)
        projects = []
        for name in (
            b"raw-\xff",
            b"raw-\xfe",
            b"a" * 45 + "é".encode() + b"z" * 30,
            b"b" * 62 + "é".encode(),
        ):
            project = root / os.fsdecode(name)
            try:
                (project / ".git").mkdir(parents=True)
            except OSError as error:
                if error.errno != errno.EILSEQ:
                    raise
                project = root / ("raw-é" if name.endswith(b"\xff") else "raw-ê")
                (project / ".git").mkdir(parents=True)
                print(
                    "skip filesystem rejects non-UTF-8 project basename; using Unicode"
                )
            projects.append(project)
        config = root / "config.kdl"
        config.write_text(f'splash_ms 0\nproject_roots "{root}" depth=1\n')
        for binary in (REFERENCE, BIN):
            script(binary, config, projects)
            socket(binary, config, projects)


if __name__ == "__main__":
    main()
