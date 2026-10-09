"""Execute pinned upstream suites with only the evaluator executable redirected."""

import ast
import os
import sys
from pathlib import Path


def reference_binary():
    """Optional full-application comparisons require an explicit external binary."""
    value = os.environ.get("SLOSH_REFERENCE_BIN")
    if not value:
        raise SystemExit(
            "Set SLOSH_REFERENCE_BIN to a slosh executable for this optional comparison."
        )
    binary = Path(value).resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise SystemExit(f"Not an executable reference: {binary}")
    return binary


def run_original(file, evaluator):
    source = file.read_text()
    if file.name == "test_shadertoy.py":
        old = 'EVAL = os.path.join(HERE, "..", "build", "expr_eval")'
        assert source.count(old) == 1
        source = source.replace(old, f"EVAL = {str(evaluator)!r}")
    sys.path.insert(0, str(file.parent))
    sys.argv = [str(file)]
    tree = ast.parse(source, str(file))
    if file.name == "test_portable_layouts.py":
        # This guard inspects app_session.c's two internal property lists, not
        # executable behavior. Slush does not ship that C application module.
        excluded = "test_the_layout_property_lists_cannot_go_stale"
        assert (
            sum(
                isinstance(n, ast.FunctionDef) and n.name == excluded for n in tree.body
            )
            == 1
        )
        tree.body = [
            n
            for n in tree.body
            if not (isinstance(n, ast.FunctionDef) and n.name == excluded)
        ]
        print(f"SKIP upstream C source consistency check: {excluded}", flush=True)
    exec(
        compile(tree, str(file), "exec"),
        {"__file__": str(file), "__name__": "__main__"},
    )


if __name__ == "__main__":
    run_original(Path(sys.argv[1]), Path(sys.argv[2]))
