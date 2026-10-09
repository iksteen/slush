#!/usr/bin/env python3
"""Compare native project discovery, workspace I/O, and identities on Unix."""

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
sys.path.insert(0, str(ROOT / "tests/reference/slosh/tests"))
import harness  # noqa: E402

BIN = Path(os.environ.get("SLOSH_BIN", ROOT / "target/debug/slush")).resolve()
REFERENCE = reference_binary()
COMMAND = [
    sys.executable,
    "-c",
    "import os,sys; print('CWD='+os.getcwdb().hex(),flush=True); sys.stdin.read()",
]


def identity(project):
    value = 2166136261
    for byte in os.fsencode(project.resolve()):
        value = ((value ^ byte) * 16777619) & 0xFFFFFFFF
    return f"project:api.{value:08x}"


def native_api(active, command, **fields):
    active._cmd(json.dumps(dict(cmd=command, **fields), ensure_ascii=False))
    return json.loads(active.proc.stdout.readline())


def opened(binary, config, views, projects):
    harness.BIN = str(binary)
    with harness.Session(COMMAND, cols=240, rows=40, config=config) as active:
        active.proc.stdout.reconfigure(errors="surrogateescape")
        active.proc.stdin.reconfigure(errors="surrogateescape")
        entries = active.api("workspaces")["workspaces"]
        expected = {identity(project) for project in projects}
        assert len(entries) == 2 and {p["purpose"] for p in entries} == expected, (
            binary,
            entries,
            expected,
        )
        actual = {entry["purpose"]: entry for entry in entries}
        for index, project in enumerate(projects):
            entry = actual[identity(project)]
            assert os.fsencode(entry["path"]) == os.fsencode(project.resolve()), (
                binary,
                entry,
            )
            layout = (
                os.fsencode(project.resolve() / "slosh.layout") if index == 0 else b""
            )
            assert os.fsencode(entry["layout"]) == layout, (binary, entry)
        active._cmd("workspaces")
        rows = [
            os.fsencode(active.proc.stdout.readline().rstrip("\n")) for _ in projects
        ]
        assert active.proc.stdout.readline() == "\n"
        expected_rows = [
            b"api".ljust(24)
            + b" "
            + (b"layout" if i == 0 else b".git").ljust(9)
            + b" "
            + os.fsencode(p.resolve())
            for i, p in enumerate(projects)
        ]
        assert sorted(rows) == sorted(expected_rows), (binary, rows, expected_rows)
        print(
            f"ok   {binary.name}: JSON and bare listings retain exact native paths and escaping"
        )
        print(
            f"ok   {binary.name}: discovery deduplicates native paths and hashes exact bytes"
        )
        for index, (view, project) in enumerate(zip(views, projects)):
            result = native_api(active, "open-workspace", path=str(project.resolve()))
            assert result["ok"] and result["created"], (binary, result)
            assert result["purpose"] == identity(project), (binary, result)
            assert os.fsencode(result["path"]) == os.fsencode(project.resolve()), (
                binary,
                result,
            )
            cwd = project / "src" if index == 0 else project
            mark = "CWD=" + os.fsencode(cwd.resolve()).hex()
            snapshot = active.until_text(mark)
            assert mark in snapshot.screen(), (binary, snapshot.screen(), mark)
            again = active.api("open-workspace", path=str(view / "api"))
            assert (
                again["ok"] and not again["created"] and again["tab"] == result["tab"]
            ), (
                binary,
                again,
            )
            print(
                f"ok   {binary.name}: project {index} opens and refocuses at its native cwd"
            )
            if index == 1:
                active._cmd("save-workspace " + str(project.resolve()))
                reply = os.fsencode(active.proc.stdout.readline().rstrip("\n"))
                assert reply.startswith(
                    os.fsencode(project.resolve() / "slosh.layout") + b" ("
                ), (binary, reply)
                print(
                    f"ok   {binary.name}: bare save-workspace reports the exact native filename"
                )
            saved = native_api(
                active,
                "save-workspace",
                path=str(project.resolve()),
                force=True,
                suspend="none",
            )
            assert saved["ok"], (binary, saved)
            assert os.fsencode(saved["path"]) == os.fsencode(
                project.resolve() / "slosh.layout"
            ), (binary, saved)
            text = (project / "slosh.layout").read_text()
            assert f'cwd="{"src" if index == 0 else "."}"' in text, text
            assert "project:" not in text, text
            print(
                f"ok   {binary.name}: project {index} saves a portable layout in its exact directory"
            )
            closed = native_api(active, "close-workspace", name=str(project.resolve()))
            assert closed["ok"] and closed["closed"] == 1, (binary, closed)
            active._cmd("open-workspace " + str(project.resolve()))
            bare = active.proc.stdout.readline()
            assert bare.startswith("opened tab "), (binary, bare)
            assert (
                active.api("close-workspace", purpose=identity(project))["closed"] == 1
            )
            restored = native_api(active, "open-workspace", path=str(project.resolve()))
            assert restored["ok"] and restored["created"], (binary, restored)
            assert mark in active.until_text(mark).screen(), binary
            assert (
                active.api("close-workspace", purpose=identity(project))["closed"] == 1
            )
            print(
                f"ok   {binary.name}: project {index} restores its saved layout and native cwd"
            )
        active.key("w")
        assert "projects" in active.snapshot().screen()
        active.send("\\r")
        active.until(lambda _snap: any(t["purpose"] in expected for t in active.tabs()))
        assert any(t["purpose"] in expected for t in active.tabs()), (
            binary,
            active.tabs(),
        )
        print(f"ok   {binary.name}: the picker opens a discovered native project path")
        result = native_api(
            active,
            "apply-layout",
            path=str(projects[0].resolve() / "slosh.layout"),
            replace=True,
        )
        assert result["ok"], (binary, result)
        mark = "CWD=" + os.fsencode((projects[0] / "src").resolve()).hex()
        assert mark in active.until_text(mark).screen(), binary
        print(f"ok   {binary.name}: apply-layout reads an exact native filename")
        dumped = native_api(
            active, "dump-layout", relative_to=str(projects[0].resolve())
        )
        assert dumped["ok"] and 'cwd="src"' in dumped["kdl"], (binary, dumped)
        print(f"ok   {binary.name}: dump-layout resolves its native relative base")


def inferred(binary, config, project):
    harness.BIN = str(binary)
    with harness.Session(
        ["/bin/sh", "-c", 'cd "$WORK_PROJECT"; read x'],
        cols=160,
        rows=30,
        config=config,
        env={"WORK_PROJECT": str(project)},
    ) as active:
        active.proc.stdout.reconfigure(errors="surrogateescape")
        active.settle()
        saved = active.api("save-workspace", force=True, suspend="none")
        assert saved["ok"], (binary, saved)
        assert (project / "slosh.layout").exists()
        assert 'cwd="."' in (project / "slosh.layout").read_text()
        print(
            f"ok   {binary.name}: save-workspace infers the project from the pane's native cwd"
        )


def native_home(binary, config, project):
    (project / "slosh.layout").unlink(missing_ok=True)
    (project.parent / "shared.layout").write_text('layout { tab { pane cwd="src" } }\n')
    config.write_text(
        'splash_ms 0\nproject_roots "~" depth=2\nproject_layout "~/shared.layout"\n'
    )
    harness.BIN = str(binary)
    # This fixture changes HOME only for this isolated checker child and panes.
    with harness.Session(
        COMMAND, cols=240, rows=40, config=config, env={"HOME": str(project.parent)}
    ) as active:
        active.proc.stdout.reconfigure(errors="surrogateescape")
        entries = active.api("workspaces")["workspaces"]
        assert len(entries) == 1 and entries[0]["purpose"] == identity(project), (
            binary,
            entries,
        )
        print(
            f"ok   {binary.name}: tilde project roots expand through the native home path"
        )
        result = active.api("open-workspace", name="api")
        assert result["ok"], (binary, result)
        mark = "CWD=" + os.fsencode((project / "src").resolve()).hex()
        assert mark in active.until_text(mark).screen(), binary
        print(f"ok   {binary.name}: a shared layout loads through the native home path")


def daemon(binary, config, views, projects):
    config.write_text(
        "splash_ms 0\nproject_roots "
        + " ".join(json.dumps(str(view)) for view in views)
        + " depth=2\n"
    )
    env = dict(os.environ, XDG_RUNTIME_DIR=str(config.parent), SLOSH_CONFIG=str(config))
    env.pop("SLOSH", None)
    env.pop("SLOSH_SESSION", None)
    process = subprocess.Popen(
        [str(binary), "-s", "wire", "--cols", "240", "--rows", "40", "--", *COMMAND],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )

    def control(request):
        line = (
            request
            if isinstance(request, str)
            else json.dumps(request, ensure_ascii=False)
        )
        result = subprocess.run(
            [str(binary), "-s", "wire", "cmd", line],
            env=env,
            capture_output=True,
            timeout=3,
        )
        return result.stdout if result.returncode == 0 else None

    def parsed(request):
        reply = control(request)
        return (
            json.loads(reply.decode(errors="surrogateescape"))
            if reply is not None
            else None
        )

    def wait_for(predicate):
        deadline = time.monotonic() + 5
        while not predicate():
            assert time.monotonic() < deadline, (
                binary,
                "native project daemon timed out",
            )
            time.sleep(0.02)

    try:
        wait_for(lambda: parsed({"cmd": "workspaces"}) is not None)
        entries = parsed({"cmd": "workspaces"})["workspaces"]
        assert {os.fsencode(p["path"]) for p in entries} == {
            os.fsencode(p.resolve()) for p in projects
        }, (binary, entries)
        listing = control("workspaces")
        assert all(os.fsencode(p.resolve()) in listing for p in projects), (
            binary,
            listing,
        )
        print(
            f"ok   {binary.name}: socket replies retain native JSON and plaintext listing paths"
        )
        opened = parsed({"cmd": "open-workspace", "path": str(projects[0].resolve())})
        assert opened["ok"] and os.fsencode(opened["path"]) == os.fsencode(
            projects[0].resolve()
        ), (binary, opened)
        mark = "CWD=" + os.fsencode((projects[0] / "src").resolve()).hex()
        wait_for(lambda: mark in parsed({"cmd": "capture"})["text"])
        print(
            f"ok   {binary.name}: socket open-workspace returns the exact native path"
        )
        saved = parsed(
            {
                "cmd": "save-workspace",
                "path": str(projects[0].resolve()),
                "force": True,
                "suspend": "none",
            }
        )
        assert saved["ok"] and os.fsencode(saved["path"]) == os.fsencode(
            projects[0].resolve() / "slosh.layout"
        ), (binary, saved)
        print(
            f"ok   {binary.name}: socket save-workspace returns the exact native filename"
        )
    finally:
        control({"cmd": "quit"})
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        process.stderr.close()
    wait_for(lambda: not (config.parent / "slosh/wire.sock").exists())


def main():
    with tempfile.TemporaryDirectory(prefix="rs-project-", dir="/tmp") as temporary:
        root = Path(temporary)
        parents = [
            root / os.fsdecode(b'team-\xff "\\\x01 space'),
            root / os.fsdecode(b'team-\xfe "\\\x01 space'),
        ]
        try:
            for parent in parents:
                parent.mkdir()
        except OSError as error:
            if error.errno != errno.EILSEQ:
                raise
            parents = [root / 'team-päne "\\\x01 space', root / 'team-é "\\\x01 space']
            for parent in parents:
                parent.mkdir()
            print(
                "skip filesystem rejects non-UTF-8 directories; exercising Unicode projects"
            )
        projects = [parent / "api" for parent in parents]
        views = [root / "view-a", root / "view-b"]
        for parent, project, view in zip(parents, projects, views):
            (project / "src").mkdir(parents=True)
            (project / ".git").mkdir()
            view.symlink_to(parent, target_is_directory=True)
        config = root / "config.kdl"
        config.write_text(
            "splash_ms 0\nproject_roots "
            + " ".join(json.dumps(str(view)) for view in (*views, views[0]))
            + " depth=2\n"
        )
        for binary in (REFERENCE, BIN):
            (projects[0] / "slosh.layout").write_text(
                'layout { tab { pane cwd="src" } }\n'
            )
            (projects[1] / "slosh.layout").unlink(missing_ok=True)
            opened(binary, config, views, projects)
            inferred(binary, config, projects[1])
        for binary in (REFERENCE, BIN):
            native_home(binary, config, projects[1])
        for binary in (REFERENCE, BIN):
            daemon(binary, config, views, projects)


if __name__ == "__main__":
    main()
