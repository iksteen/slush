#!/usr/bin/env python3
"""Compare native theme names, limits, selection, diagnostics, and picker hits."""

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
QUIET = ["/bin/sh", "-c", "cat"]


def payload(command, fields):
    return json.dumps(dict(cmd=command, **fields), ensure_ascii=False).encode(
        errors="surrogateescape"
    )


def decode(reply):
    return json.loads(reply.decode(errors="surrogateescape"))


def shape(reply):
    # Executable-relative search directories legitimately differ between builds.
    reply = dict(reply)
    reply.pop("dirs", None)
    if "error" in reply and reply["error"].startswith("no theme named "):
        reply["error"] = reply["error"].split(" in ")[0]
    return reply


def exercise(binary, transport, request, bare, config, names):
    results = []
    listed = request("theme")
    encoded = [os.fsencode(name) for name in listed["themes"]]
    assert encoded == sorted(set(encoded)) and set(names) <= set(encoded), (
        binary,
        encoded,
    )
    assert b".hidden" not in encoded and b"y" * 64 not in encoded
    results.append(shape(listed))
    results.append(bare(b"theme"))
    print(f"ok   {binary.name}: {transport} lists distinct native names in byte order")
    candidates = [
        "plain",
        "x" * 63,
        "y" * 64,
        "é",
        os.fsdecode(b"raw-\xff"),
        os.fsdecode(b"raw-\xfe"),
        ".",
        "..",
        "bad/name",
        "bad\\name",
        "bad:name",
        "bad\x01name",
        "long-" + "é" * 100,
        "plain\x00ignored",
    ]
    for name in candidates:
        response = request("theme", name=name)
        results.append(shape(response))
        print(f"ok   {binary.name}: {transport} selection {os.fsencode(name).hex()}")
    for name in (b"raw-\xff", b"raw-\xfe", "é".encode(), b"x" * 63):
        results.append(bare(b"theme " + name))
    print(f"ok   {binary.name}: {transport} bare selection retains diagnostic bytes")
    assert request("theme", name="plain", save=True)["ok"]
    assert b'editor "arg-\xff"' in config.read_bytes()
    results.append(config.read_bytes())
    print(f"ok   {binary.name}: {transport} save retains native config values")
    saved = config.read_bytes()
    config.write_bytes(b'theme_name "raw-\xff"\neditor "arg-\xfe"\n')
    results.append(shape(request("reload")))
    config.write_bytes(saved)
    assert request("reload")["ok"]
    print(f"ok   {binary.name}: {transport} reload retains native theme warning bytes")
    return results


def script(binary, config, names, picker=False):
    harness.BIN = str(binary)
    with harness.Session(QUIET, config=config, cols=120, rows=32) as active:
        active.proc.stdin.reconfigure(errors="surrogateescape")
        active.proc.stdout.reconfigure(errors="surrogateescape")

        def bare(line):
            active._cmd(os.fsdecode(line))
            return os.fsencode(active.proc.stdout.readline().removesuffix("\n"))

        def request(command, **fields):
            return decode(bare(payload(command, fields)))

        results = exercise(binary, "script", request, bare, config, names)
        if picker:
            active.key("t")
            active.settle(30)
            active.send("raw-")
            active.settle(30)
            hits = request("snapshot")["screen"]["hits"]
            actions = sorted(
                os.fsencode(hit["action"])
                for hit in hits
                if hit["action"].startswith("theme:")
            )
            assert len(actions) == 2, (binary, actions)
            results.append(actions)
            print(f"ok   {binary.name}: picker preserves both native row actions")
            active.send(r"\e")
            active.settle(30)
            active.key("t")
            active.settle(30)
            active.send("split-")
            active.settle(30)
            hits = request("snapshot")["screen"]["hits"]
            actions = [
                os.fsencode(hit["action"])
                for hit in hits
                if hit["action"].startswith("theme:")
            ]
            expected = (b"theme:split-" + b"z" * 34 + "é".encode())[:47]
            assert actions == [expected], (binary, expected, actions)
            bare_actions = [
                os.fsencode(hit["action"])
                for hit in decode(bare(b"snapshot"))["hits"]
                if hit["action"].startswith("theme:")
            ]
            assert bare_actions == actions, (binary, bare_actions, actions)
            results.append(actions)
            print(f"ok   {binary.name}: picker hit limit splits UTF-8 like C")
        return results


def socket(binary, config, names):
    env = dict(os.environ, SLOSH_CONFIG=str(config), XDG_RUNTIME_DIR=str(config.parent))
    env.pop("SLOSH", None)
    env.pop("SLOSH_SESSION", None)
    process = subprocess.Popen(
        [str(binary), "-s", "themes", "--", *QUIET],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )

    def bare(line):
        response = subprocess.run(
            [os.fsencode(binary), b"-s", b"themes", b"cmd", line],
            env=env,
            capture_output=True,
            timeout=3,
        )
        return response.stdout.removesuffix(b"\n") if response.returncode == 0 else None

    def request(command, **fields):
        reply = bare(payload(command, fields))
        return decode(reply) if reply else None

    try:
        deadline = time.monotonic() + 5
        while not request("alive"):
            assert process.poll() is None and time.monotonic() < deadline
            time.sleep(0.02)
        return exercise(binary, "socket", request, bare, config, names)
    finally:
        request("quit")
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        process.stderr.close()


def main():
    with tempfile.TemporaryDirectory(prefix="rs-theme-names-", dir="/tmp") as temporary:
        root = Path(temporary)
        themes = root / "themes"
        themes.mkdir()
        names = [b"plain", b"x" * 63, "é".encode(), b"raw-\xfe", b"raw-\xff"]
        for index, name in enumerate(names):
            try:
                (themes / os.fsdecode(name + b".kdl")).write_bytes(
                    b'theme { frame_focus "#123456" }\n'
                )
            except OSError as error:
                if error.errno != errno.EILSEQ:
                    raise
                names[index] = (
                    "raw-" + ("é" if name.endswith(b"\xfe") else "ö")
                ).encode()
                (themes / os.fsdecode(names[index] + b".kdl")).write_bytes(
                    b'theme { frame_focus "#123456" }\n'
                )
                print(
                    "skip filesystem rejects raw name; native request errors remain covered"
                )
        for name in (b".hidden", b"y" * 64, b"split-" + b"z" * 34 + "é".encode()):
            (themes / os.fsdecode(name + b".kdl")).write_bytes(b"theme {}\n")
        config = root / "config.kdl"
        source = b'editor "arg-\xff"\ntheme_name "plain"\nsplash_ms 0\n'
        for name in (b"raw-\xff", "é".encode(), ("long-" + "é" * 100).encode()):
            config.write_bytes(b'theme_name "' + name + b'"\n')
            checked = [
                subprocess.run(
                    [str(binary), "--check", str(config)],
                    capture_output=True,
                    timeout=3,
                )
                for binary in (REFERENCE, BIN)
            ]
            assert [(r.returncode, r.stdout, r.stderr) for r in checked[:1]] == [
                (r.returncode, r.stdout, r.stderr) for r in checked[1:]
            ], checked
            print(
                f"ok   slush: native theme checker diagnostics match C for {name.hex()}"
            )
        for transport in (script, socket):
            baseline = None
            for binary in (REFERENCE, BIN):
                config.write_bytes(source)
                result = (
                    transport(binary, config, names, picker=True)
                    if transport == script
                    else transport(binary, config, names)
                )
                if binary == REFERENCE:
                    baseline = result
                else:
                    assert result == baseline, (transport.__name__, baseline, result)
                    print(f"ok   slush: {transport.__name__} results match C exactly")
        # The first directory fills the result before sorting; later directories
        # cannot displace its names just because they sort earlier.
        for name in names:
            (themes / os.fsdecode(name + b".kdl")).unlink()
        for index in range(80):
            (themes / f"zz-{index:02}.kdl").write_bytes(b"theme {}\n")
        lists = []
        for binary in (REFERENCE, BIN):
            harness.BIN = str(binary)
            config.write_bytes(b"splash_ms 0\n")
            with harness.Session(QUIET, config=config) as active:
                listed = active.api("theme")["themes"]
                assert len(listed) == 64 and "amber" not in listed, (binary, listed)
                lists.append(listed)
                print(f"ok   {binary.name}: first directory owns the 64-name limit")
        assert lists[0] == lists[1], lists
        print("ok   slush: capped discovery matches C directory order before sorting")


if __name__ == "__main__":
    main()
