#!/usr/bin/env python3
"""Compare native config/layout paths and hot reloads with original slosh."""

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
LIME, ROSE, EDITED = "#13579b", "#b97531", "#9753bd"
COMMAND = [
    sys.executable,
    "-c",
    "import os,sys; print('CWD='+os.getcwdb().hex(),flush=True); sys.stdin.read()",
]


def write(file, text):
    with open(file, "wb") as output:
        output.write(text.encode())


def atomic_write(file, text):
    temporary = file + b".new"
    write(temporary, text)
    os.replace(temporary, file)


def cli(binary, args, env):
    result = subprocess.run(
        [str(binary), *args], env=env, capture_output=True, timeout=5
    )
    assert result.returncode == 0 and not result.stderr, (binary, args, result)
    return result.stdout


def session(binary, command, config, layout=None):
    harness.BIN = str(binary)
    active = harness.Session(
        command,
        cols=240,
        rows=30,
        config=os.fsdecode(config),
        layout=os.fsdecode(layout) if layout else None,
    )
    # The original's JSON reports retain Unix filename bytes too.
    active.proc.stdout.reconfigure(errors="surrogateescape")
    return active


def wait_for(predicate):
    deadline = time.monotonic() + 5
    while not predicate():
        assert time.monotonic() < deadline, (
            "native-path session did not reach expected state"
        )
        time.sleep(0.02)


def live_reload(
    binary, config, layout, include, lime, rose, runtime, expected_cwd, text
):
    env = dict(
        os.environ, XDG_RUNTIME_DIR=str(runtime), SLOSH_CONFIG=os.fsdecode(config)
    )
    env.pop("SLOSH", None)
    env.pop("SLOSH_SESSION", None)
    client = subprocess.Popen(
        [
            str(binary),
            "-s",
            "path",
            "--cols",
            "240",
            "--rows",
            "30",
            "--layout",
            layout,
            "--",
            *COMMAND,
        ],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )

    def control(request):
        line = request if isinstance(request, str) else json.dumps(request)
        result = subprocess.run(
            [str(binary), "-s", "path", "cmd", line],
            env=env,
            capture_output=True,
            timeout=3,
        )
        return result.returncode, result.stdout.decode(errors="surrogateescape")

    def has_color(color):
        status, output = control({"cmd": "snapshot", "format": "json"})
        return status == 0 and f'"{color}"' in output

    try:
        wait_for(lambda: has_color(LIME))
        wait_for(lambda: expected_cwd in control({"cmd": "capture"})[1])
        print(
            f"ok   {binary.name}: daemon launch retains the native layout path and cwd"
        )
        atomic_write(lime, f'theme {{ frame_focus "{EDITED}" }}\n')
        wait_for(lambda: has_color(EDITED))
        print(
            f"ok   {binary.name}: an atomically replaced theme reloads through its native directory"
        )
        atomic_write(config, text.replace('theme_name "lime"', 'theme_name "rose"'))
        wait_for(lambda: has_color(ROSE))
        print(
            f"ok   {binary.name}: atomic config replacement retains the native watch filename"
        )
        atomic_write(include, 'padding 0\ntheme { tab_active_fg "#2468ac" }\n')
        wait_for(lambda: has_color("#2468ac"))
        print(f"ok   {binary.name}: an included file still triggers automatic reload")
    finally:
        control({"cmd": "quit"})
        try:
            client.wait(timeout=3)
        except subprocess.TimeoutExpired:
            client.kill()
            client.wait()
        client.stderr.close()
        write(include, "padding 0\n")
        write(lime, f'theme {{ frame_focus "{LIME}" }}\n')
        write(rose, f'theme {{ frame_focus "{ROSE}" }}\n')
        write(config, text)


def main():
    with tempfile.TemporaryDirectory(prefix="rs-path-", dir="/tmp") as temporary:
        root = os.fsencode(temporary) + b"/native-\xff space"
        try:
            os.mkdir(root)
            native_filenames = True
        except OSError as error:
            if error.errno != errno.EILSEQ:
                raise
            root = os.fsencode(temporary) + "/native-päne space".encode()
            os.mkdir(root)
            native_filenames = False
            print(
                "skip filesystem rejects non-UTF-8 filenames; exercising Unicode paths"
            )
        config = root + (b"/config-\xfe.kdl" if native_filenames else b"/config.kdl")
        include = root + b"/base.kdl"
        layout = root + b"/session.layout"
        palettes = root + b"/themes"
        work = root + b"/work"
        os.mkdir(palettes)
        os.mkdir(work)
        lime, rose = palettes + b"/lime.kdl", palettes + b"/rose.kdl"
        write(include, "padding 0\n")
        write(lime, f'theme {{ frame_focus "{LIME}" }}\n')
        write(rose, f'theme {{ frame_focus "{ROSE}" }}\n')
        write(layout, 'layout { tab cwd="work" { pane } }\n')
        editor_script = Path(temporary) / "editor.py"
        editor_script.write_text(
            "import os,sys\nprint('EDITOR='+os.fsencode(sys.argv[-1]).hex(),flush=True)\n"
            "sys.stdin.read()\n"
        )
        editor = (
            f"{shlex.quote(sys.executable)} {shlex.quote(str(editor_script))} --flag"
        )
        text = (
            'include "././base.kdl"\ntheme_dir "themes"\ntheme_name "lime"\nsplash_ms 0\n'
            f"editor {json.dumps(editor)}\n"
        )
        write(config, text)
        env = dict(os.environ, SLOSH_CONFIG=os.fsdecode(config))
        for args in (["--check", config], ["--check"], ["--check", layout]):
            output = [cli(binary, args, env) for binary in (REFERENCE, BIN)]
            assert output[0] == output[1], (args, output)
            print(f"ok   native checker paths and stdout agree: {args[0:1]}")
        xdg = root + b"/xdg"
        os.makedirs(xdg + b"/slosh")
        write(xdg + b"/slosh/config.kdl", "gap 2\n")
        xdg_env = dict(os.environ, XDG_CONFIG_HOME=os.fsdecode(xdg))
        xdg_env.pop("SLOSH_CONFIG", None)
        for suffix in ("", "/"):
            xdg_env["XDG_CONFIG_HOME"] = os.fsdecode(xdg) + suffix
            assert cli(REFERENCE, ["--check"], xdg_env) == cli(
                BIN, ["--check"], xdg_env
            )
            print(
                "ok   native XDG_CONFIG_HOME and trailing separators retain the same report"
            )
        os.makedirs(root + b"/.config/slosh")
        write(root + b"/.config/slosh/config.kdl", "gap 2\n")
        for suffix in ("", "/"):
            # Only these checker children receive the isolated home fixture.
            home_env = dict(os.environ, HOME=os.fsdecode(root) + suffix)
            home_env.pop("SLOSH_CONFIG", None)
            home_env.pop("XDG_CONFIG_HOME", None)
            assert cli(REFERENCE, ["--check"], home_env) == cli(
                BIN, ["--check"], home_env
            )
            print(
                "ok   native default home paths and trailing separators retain the same report"
            )
        expected_cwd = "CWD=" + os.path.realpath(work).hex()
        for binary in (REFERENCE, BIN):
            assert LIME.encode() in cli(binary, ["--dump-theme"], env)
            print(
                f"ok   {binary.name}: relative includes and named themes load from native paths"
            )
            with session(binary, COMMAND, config, layout) as active:
                assert expected_cwd in active.until_text(expected_cwd).screen()
                assert active.api("split", dir="rows")["ok"]
                active.until(
                    lambda snapshot: snapshot.screen().count(expected_cwd) == 2
                )
                assert active.snapshot().screen().count(expected_cwd) == 2
            print(
                f"ok   {binary.name}: layout-relative cwd and split inheritance retain native bytes"
            )
            with session(binary, ["/bin/cat"], config) as active:
                result = active.api("edit-config")
                assert result["ok"]
                editor_mark = "EDITOR=" + config.hex()
                snapshot = active.until_text(editor_mark)
                assert editor_mark in snapshot.screen(), (binary, snapshot.screen())
                assert active.api("close", id=result["id"])["ok"]
                reply = active.api("theme", name="rose", save=True)
                assert reply["ok"], (binary, reply)
                with open(config, "rb") as source:
                    saved = source.read()
                assert (
                    b'theme_name "rose"' in saved and b'include "././base.kdl"' in saved
                )
                assert active.api("reload")["ok"]
                assert active.api("theme")["theme"] == "rose"
            print(
                f"ok   {binary.name}: editor arguments and saved themes retain the config's native path"
            )
            write(config, text)
            live_reload(
                binary,
                config,
                layout,
                include,
                lime,
                rose,
                Path(temporary),
                expected_cwd,
                text,
            )


if __name__ == "__main__":
    main()
