#!/usr/bin/env python3
"""Compile unmodified slosh C unit suites against Rust's test-only ABI."""

import os
import subprocess
import sys
from pathlib import Path

from _reference import run_original

ROOT = Path(__file__).resolve().parents[1]
SLOSH = ROOT / "tests/reference/slosh"
OUT = ROOT / "target" / "compat"
OUT.mkdir(parents=True, exist_ok=True)

subprocess.run(
    [
        "cargo",
        "build",
        "--offline",
        "--manifest-path",
        str(ROOT / "Cargo.toml"),
        "--features",
        "c-test-api",
        "--lib",
        "--bins",
    ],
    check=True,
)
for suite in ("kdl", "expr", "input", "screen", "shader", "path"):
    executable = OUT / f"{suite}_test"
    subprocess.run(
        [
            os.environ.get("CC", "cc"),
            "-I",
            str(SLOSH / "include"),
            str(SLOSH / "tests" / f"{suite}_test.c"),
            str(ROOT / "target" / "debug" / "libslush.a"),
            "-ldl",
            "-lpthread",
            "-lm",
            "-o",
            str(executable),
        ],
        check=True,
    )
    subprocess.run([str(executable)], check=True)

# CLI checks use the actual Rust executable, with the original test unchanged.
subprocess.run(
    [sys.executable, str(SLOSH / "tests/test_check.py")],
    env=dict(os.environ, SLOSH_BIN=str(ROOT / "target/debug/slush")),
    check=True,
)

# This suite hardcodes its evaluator's location. Substitute only that executable
# path, retaining the original test logic, shipped presets, and JS implementation.
run_original(SLOSH / "tests/test_shadertoy.py", ROOT / "target/debug/expr_eval")
