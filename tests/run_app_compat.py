#!/usr/bin/env python3
"""Run unchanged slosh application suites against the native Rust executable.

With no arguments, run every test_*.py suite. Names select suites during
implementation, e.g. `python3 tests/run_app_compat.py layout screen prefix`.
Each suite retains its own assertions and exit status. Logs and a machine-
readable report live in target/app-compat; failures are never suppressed.
"""

import argparse
import hashlib
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REFERENCE = ROOT / "tests/reference/slosh" / "tests"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("suites", nargs="*")
    parser.add_argument("--timeout", type=float, default=60)
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument(
        "--binary",
        type=Path,
        help="test an existing executable instead of building the debug binary",
    )
    parser.add_argument(
        "--live",
        action="store_true",
        help="also run the four original live terminal suites",
    )
    parser.add_argument(
        "--report",
        default="report.json",
        help="report filename within target/app-compat",
    )
    args = parser.parse_args()
    if not args.no_build and args.binary is None:
        subprocess.run(
            [
                "cargo",
                "build",
                "--offline",
                "--manifest-path",
                str(ROOT / "Cargo.toml"),
            ],
            check=True,
        )
    suites = (
        [
            REFERENCE
            / (s if s.endswith(".py") else f"test_{s.removeprefix('test_')}.py")
            for s in args.suites
        ]
        if args.suites
        else sorted(REFERENCE.glob("test_*.py"))
    )
    if args.live:
        suites.extend(
            REFERENCE / name
            for name in (
                "live_m0.py",
                "live_input.py",
                "live_reload.py",
                "live_graphics.py",
            )
            if REFERENCE / name not in suites
        )
    if any(not suite.is_file() for suite in suites):
        parser.error("a requested suite does not exist")
    destination = ROOT / "target" / "app-compat"
    destination.mkdir(parents=True, exist_ok=True)
    binary = (args.binary or ROOT / "target" / "debug" / "slush").resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error(f"not an executable file: {binary}")
    env = dict(os.environ, SLOSH_BIN=str(binary))
    report = destination / args.report
    logs = report.with_suffix(".logs")
    logs.mkdir(parents=True, exist_ok=True)
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    results = []
    for suite in suites:
        started = time.monotonic()
        log_path = logs / f"{suite.stem}.log"
        with log_path.open("w") as log:
            proc = subprocess.Popen(
                [
                    sys.executable,
                    str(ROOT / "tests/_reference.py"),
                    str(suite),
                    str(ROOT / "target/debug/expr_eval"),
                ],
                cwd=REFERENCE,
                env=env,
                stdout=log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
            try:
                code = proc.wait(timeout=args.timeout)
                status = "passed" if code == 0 else "failed"
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)
                code = proc.wait()
                status = "timeout"
        output = log_path.read_text(errors="replace")
        result = {
            "suite": suite.name,
            "status": status,
            "exit_code": code,
            "log": str(log_path),
            "checks_passed": sum(
                line.startswith("ok   ") for line in output.splitlines()
            ),
            "checks_failed": sum(
                line.startswith("FAIL ") for line in output.splitlines()
            ),
            "seconds": round(time.monotonic() - started, 3),
        }
        results.append(result)
        print(
            f"{status:7} {suite.name}: {result['checks_passed']} checks passed, {result['checks_failed']} failed",
            flush=True,
        )
        report.write_text(
            json.dumps(
                {
                    "binary": str(binary),
                    "binary_sha256": digest,
                    "timeout_seconds": args.timeout,
                    "suites": results,
                },
                indent=2,
            )
            + "\n"
        )
    passed = sum(result["status"] == "passed" for result in results)
    print(f"{passed}/{len(results)} suites passed; logs: {logs}")
    return 0 if passed == len(results) else 1


if __name__ == "__main__":
    sys.exit(main())
