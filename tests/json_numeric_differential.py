#!/usr/bin/env python3
"""Compare JSON numeric control fields with the original executable."""

import json
import os
import subprocess
from pathlib import Path

from _reference import reference_binary

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get("SLOSH_BIN", ROOT / "target/debug/slush")).resolve()
REFERENCE = reference_binary()


def run(binary, requests):
    script = (
        "\n".join(
            request if isinstance(request, str) else json.dumps(request)
            for request in requests
        )
        + "\nquit\n"
    )
    result = subprocess.run(
        [str(binary), "--script", "--cols", "80", "--rows", "12", "--", "/bin/cat"],
        input=script,
        env=dict(os.environ, SLOSH_CONFIG="/nonexistent/json-numeric-regression.kdl"),
        capture_output=True,
        text=True,
        timeout=8,
    )
    assert result.returncode == 0, (binary, result.stderr)
    return [json.loads(line) for line in result.stdout.splitlines()]


def main():
    for value in (
        40,
        40.9,
        4e1,
        -1.8,
        1e20,
        -1e20,
        2**63 + 1,
        "40",
        " +40suffix",
        "\v40suffix",
        "not-a-number",
        "",
        "9223372036854775808",
        "-9223372036854775809",
        None,
        True,
        [],
        {},
    ):
        requests = [
            {"cmd": "resize", "cols": value, "rows": 1},
            {"cmd": "snapshot"},
        ]
        results = []
        for binary in (REFERENCE, BIN):
            reply, snapshot = run(binary, requests)
            screen = snapshot["screen"]
            results.append((reply, screen["cols"], screen["rows"]))
        assert results[0] == results[1], (value, results)
        print(f"ok   resize numeric value {value!r} matches the reference")

    for value in (
        1.0,
        1.9,
        "1",
        " +1suffix",
        "\v1",
        "-1",
        2**53 + 1,
        0,
        "no",
        None,
        True,
        False,
    ):
        requests = [
            {"cmd": "split", "dir": "cols"},
            {"cmd": "focus", "id": value},
            {"cmd": "panes"},
        ]
        results = []
        for binary in (REFERENCE, BIN):
            _, reply, panes = run(binary, requests)
            focused = [pane["id"] for pane in panes["panes"] if pane["focused"]]
            results.append((reply, focused))
        assert results[0] == results[1], (value, results)
        print(f"ok   focus numeric value {value!r} matches the reference")

    for token in (
        "1e999",
        "-1e999",
        "1e-999",
        '{"$serde_json::private::Number":"40"}',
    ):
        requests = [
            '{"cmd":"resize","cols":' + token + ',"rows":1}',
            {"cmd": "snapshot"},
        ]
        results = []
        for binary in (REFERENCE, BIN):
            reply, snapshot = run(binary, requests)
            screen = snapshot["screen"]
            results.append((reply, screen["cols"], screen["rows"]))
        assert results[0] == results[1], (token, results)
        print(f"ok   raw numeric token {token!r} matches the reference")

    for request in (
        '{"cmd":"alive","ignored":[1e999]}',
        '{"cmd":"alive","$serde_json::private::Number":"40"}',
    ):
        results = [run(binary, [request]) for binary in (REFERENCE, BIN)]
        assert results[0] == results[1], (request, results)
        print("ok   unrelated JSON fields retain their original meaning")


if __name__ == "__main__":
    main()
