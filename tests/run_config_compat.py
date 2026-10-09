#!/usr/bin/env python3
"""Run existing CLI-only checks and compare config results with the C reference.

Session-dependent checks remain part of the full acceptance gate. This driver
selects only the original CLI checks that can run before the app port is ready.
"""

import ast
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

from _reference import reference_binary

ROOT = Path(__file__).resolve().parents[1]
SLOSH = ROOT / "tests/reference/slosh"
RUST = ROOT / "target/debug/slush"
REFERENCE = reference_binary()
os.environ["SLOSH_BIN"] = str(RUST)
sys.path.insert(0, str(SLOSH / "tests"))


def definitions(file):
    # Preserve original test function bodies; remove only the automatic driver.
    tree = ast.parse(file.read_text(), str(file))
    tree.body = [
        n
        for n in tree.body
        if not isinstance(n, ast.For)
        and not (
            isinstance(n, ast.Expr)
            and isinstance(n.value, ast.Call)
            and isinstance(n.value.func, ast.Attribute)
            and n.value.func.attr == "exit"
        )
    ]
    namespace = {"__file__": str(file), "__name__": "config_compat"}
    exec(compile(tree, str(file), "exec"), namespace)
    return namespace


dump = definitions(SLOSH / "tests/test_dump_config.py")
for name in (
    "test_it_dumps_something_that_looks_like_a_config",
    "test_it_parses_back_to_exactly_itself",
    "test_a_value_with_a_quote_in_it_is_written_escaped",
    "test_every_key_the_parser_knows_is_in_it",
    "test_the_config_dump_names_a_theme_rather_than_a_palette",
    "test_every_theme_colour_is_in_the_theme_dump",
):
    dump[name]()
themes = definitions(SLOSH / "tests/test_themes.py")
for name in (
    "test_there_are_themes",
    "test_the_guard_has_something_to_guard",
    "test_each_sets_every_colour_the_config_knows",
    "test_default_is_the_compiled_palette_written_out",
):
    themes[name]()

from harness import check, report


def palette(binary, file):
    output = subprocess.check_output(
        [str(binary), "--dump-theme"],
        text=True,
        env=dict(os.environ, SLOSH_CONFIG=str(file)),
    )
    return dict(re.findall(r'^\s+([a-z_]+)\s+"(#[0-9a-fA-F]{6})"', output, re.M))


with tempfile.TemporaryDirectory(prefix="slush-config-parity-") as directory:
    directory = Path(directory)
    for i, text in enumerate(
        [
            "gap -1\nscrollback -2\npadding 1 2 3\ntab_bar_side down\nkeep_dead maybe\n",
            "unknown 3\nkeys {prefix nope; bind z fly; bind nope zoom}\n",
            'shaders {dim where="chrom"; tint channel="g"; bloom amount=2}\n',
            'project_roots depth=20\nsize_follows "wrong"\ntab_bar_index "left"\n',
            'theme {frame_focus "#zzzzzz"}\nstates {unknown {dim}}\n',
            'gap 3\nkeys {prefix "ctrl+b"; direct {bind f1 zoom}}\n',
        ]
    ):
        file = directory / f"case{i}.kdl"
        file.write_text(text)
        runs = [
            subprocess.run(
                [str(binary), "--check", str(file)], capture_output=True, text=True
            )
            for binary in (REFERENCE, RUST)
        ]
        check(
            f"config diagnostics case {i} match C",
            (runs[0].returncode, runs[0].stdout, runs[0].stderr)
            == (runs[1].returncode, runs[1].stdout, runs[1].stderr),
            runs[0].stdout
            + runs[0].stderr
            + "\nRust:\n"
            + runs[1].stdout
            + runs[1].stderr,
        )

    for theme in sorted((SLOSH / "contrib/themes").glob("*.kdl")):
        check(
            f"{theme.name} resolves every colour like C",
            palette(REFERENCE, theme) == palette(RUST, theme),
            str(theme),
        )
    file = directory / "named.kdl"
    file.write_text(
        f'theme_dir "{SLOSH / "contrib/themes"}"\ntheme_name "amber"\n'
        'theme {frame_focus "#aabbcc"; default_bg "#121212"}\n'
    )
    check(
        "named theme with inline overrides resolves like C",
        palette(REFERENCE, file) == palette(RUST, file),
        str(file),
    )
sys.exit(report())
