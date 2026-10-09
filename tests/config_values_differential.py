#!/usr/bin/env python3
"""Compare native KDL configuration paths and commands with original slosh."""

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


def quote(value):
    return b'"' + value.replace(b"\\", b"\\\\").replace(b'"', b'\\"') + b'"'


def check(binary, config, include, project, theme_dir):
    env = dict(os.environ, SLOSH_CONFIG=str(config))
    checked = subprocess.run(
        [str(binary), "--check", str(config)], env=env, capture_output=True, timeout=3
    )
    assert checked.returncode == 0, (binary, checked.stdout, checked.stderr)
    print(f"ok   {binary.name}: checker accepts native bytes in config values")
    theme = subprocess.run(
        [str(binary), "--dump-theme"], env=env, capture_output=True, timeout=3
    )
    assert theme.returncode == 0 and any(
        line.split() == [b"frame_focus", b'"#123456"']
        for line in theme.stdout.splitlines()
    ), (binary, theme.stdout, theme.stderr)
    print(f"ok   {binary.name}: named theme loads from the native theme directory")
    harness.BIN = str(binary)
    with harness.Session([], config=config, cols=240, rows=40) as active:
        active.proc.stdin.reconfigure(errors="surrogateescape")
        active.proc.stdout.reconfigure(errors="surrogateescape")

        def request(command, **fields):
            active._cmd(json.dumps(dict(cmd=command, **fields), ensure_ascii=False))
            return json.loads(active.proc.stdout.readline())

        exercise(
            binary,
            "script",
            request,
            lambda: active.settle(10),
            config,
            include,
            project,
            theme_dir,
        )


def exercise(binary, transport, request, pump, config, include, project, theme_dir):
    def until(mark):
        deadline = time.monotonic() + 5
        while True:
            pump()
            if mark in request("capture")["text"]:
                return
            assert time.monotonic() < deadline, (
                binary,
                mark,
                "config value timed out",
            )

    until("CONFIGSHELL=")
    panes = request("panes")["panes"]
    assert panes[0]["x"] == 7, (binary, panes)
    print(
        f"ok   {binary.name}: {transport} native include applies before the local configuration"
    )
    print(f"ok   {binary.name}: {transport} configured native shell executable starts")
    themes = request("theme")
    assert themes["ok"] and themes["theme"] == "custom" and "custom" in themes["themes"]
    assert os.fsencode(themes["dirs"][0]) == os.fsencode(theme_dir)
    print(
        f"ok   {binary.name}: {transport} theme query reports its exact native directory"
    )
    projects = request("workspaces")["workspaces"]
    assert len(projects) == 1 and os.fsencode(projects[0]["path"]) == os.fsencode(
        project.resolve()
    )
    print(
        f"ok   {binary.name}: {transport} project discovery reads a native root from KDL"
    )
    opened = request("open-workspace", name="api")
    assert opened["ok"], (binary, opened)
    until("CONFIGSHELL=" + os.fsencode((project / "src").resolve()).hex())
    print(
        f"ok   {binary.name}: {transport} shared layout loads from its native KDL filename"
    )
    edited = request("edit-config")
    assert edited["ok"], (binary, edited)
    mark = "CONFIGEDITOR=" + b"arg-\xff".hex() + ":" + os.fsencode(config).hex()
    deadline = time.monotonic() + 5
    while True:
        pump()
        if mark in request("capture", id=edited["id"])["text"]:
            break
        assert time.monotonic() < deadline, (
            binary,
            mark,
            "native configured editor timed out",
        )
    print(
        f"ok   {binary.name}: {transport} configured editor preserves native arguments and filename"
    )
    request("close", id=edited["id"])
    include.write_bytes(b"gap 9\ngap_aspect 1\n")
    assert request("select-tab", id=panes[0]["tab_id"])["ok"]
    assert request("reload")["ok"]
    assert request("panes")["panes"][0]["x"] == 9
    print(
        f"ok   {binary.name}: {transport} reload rereads the exact native include filename"
    )


def daemon(binary, config, include, project, theme_dir):
    env = dict(os.environ, SLOSH_CONFIG=str(config), XDG_RUNTIME_DIR=str(config.parent))
    env.pop("SLOSH", None)
    env.pop("SLOSH_SESSION", None)
    process = subprocess.Popen(
        [str(binary), "-s", "config", "--cols", "240", "--rows", "40"],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )

    def request(command, **fields):
        result = subprocess.run(
            [
                str(binary),
                "-s",
                "config",
                "cmd",
                json.dumps(dict(cmd=command, **fields), ensure_ascii=False),
            ],
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
            assert time.monotonic() < deadline, (
                binary,
                "native config daemon timed out",
            )
            time.sleep(0.02)

    try:
        wait_for(lambda: request("alive"))
        exercise(
            binary, "socket", request, lambda: None, config, include, project, theme_dir
        )
    finally:
        request("quit")
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        process.stderr.close()
    wait_for(lambda: not (config.parent / "slosh/config.sock").exists())


def main():
    with tempfile.TemporaryDirectory(
        prefix="rs-config-values-", dir="/tmp"
    ) as temporary:
        root = Path(temporary)
        native = root / os.fsdecode(b"values-\xff")
        try:
            native.mkdir()
        except OSError as error:
            if error.errno != errno.EILSEQ:
                raise
            native = root / "values-é"
            native.mkdir()
            print(
                "skip filesystem rejects non-UTF-8 names; raw editor arguments remain covered"
            )
        include = native / "base.kdl"
        project = native / "projects/api"
        (project / ".git").mkdir(parents=True)
        (project / "src").mkdir()
        shared = native / "shared.layout"
        shared.write_text('layout { tab { pane cwd="src" } }\n')
        themes = native / "themes"
        themes.mkdir()
        (themes / "custom.kdl").write_text('theme { frame_focus "#123456" }\n')
        shell = native / "shell"
        program = "import os,sys; print('CONFIGSHELL='+os.getcwdb().hex(),flush=True);sys.stdin.read()"
        shell.write_text(
            "#!/bin/sh\nexec " + shlex.join([sys.executable, "-c", program]) + "\n"
        )
        shell.chmod(0o755)
        program = "import os,sys;print('CONFIGEDITOR='+os.fsencode(sys.argv[1]).hex()+':'+os.fsencode(sys.argv[-1]).hex(),flush=True);sys.stdin.read()"
        editor = shlex.join([sys.executable, "-c", program]).encode() + b" 'arg-\xff'"
        config = root / "config.kdl"
        config.write_bytes(
            b"splash_ms 0\ninclude "
            + quote(os.fsencode(include))
            + b"\n"
            + b"shell "
            + quote(os.fsencode(shell))
            + b"\neditor "
            + quote(editor)
            + b"\n"
            + b"project_roots "
            + quote(os.fsencode(project.parent))
            + b" depth=1\n"
            + b"project_layout "
            + quote(os.fsencode(shared))
            + b"\n"
            + b"theme_dir "
            + quote(os.fsencode(themes))
            + b'\ntheme_name "custom"\n'
        )
        for binary in (REFERENCE, BIN):
            include.write_bytes(b"gap 7\ngap_aspect 1\n")
            check(binary, config, include, project, themes)
            include.write_bytes(b"gap 7\ngap_aspect 1\n")
            daemon(binary, config, include, project, themes)


if __name__ == "__main__":
    main()
