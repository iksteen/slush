# Compatibility fixtures

`slosh/` is a selected test fixture tree from the commit recorded in
`slosh.vendor.json`, not a copy of the upstream application.

- Nine C modules and eleven headers support small configuration, screen,
  graphics, and OSC oracles compiled by `cargo test`.
- Six original C unit suites link against Rust's test-only ABI through
  `python3 tests/run_compat.py`.
- 63 behavioral Python suites and four live terminal suites run against Rust
  through `python3 tests/run_app_compat.py --live`.
- Fuzz seeds and shader evaluator/preset fixtures retain original assertions.
  Configuration, themes, and presets link to the repository's shipped assets.

The upstream documentation generator, agent skill, and shader-REPL tool tests
are excluded because those components are not shipped. There is no C application,
Makefile, website, release tooling, or duplicate terminal core in this directory.

Optional full-application `*_differential.py` comparisons require an explicit
`SLOSH_REFERENCE_BIN` executable. Normal builds and tests need no external slosh
checkout or binary. The shader test's hardcoded expression-evaluator path is
redirected to Rust. One layout check that inspects the unshipped C application's
internal property lists is explicitly excluded and reported as skipped; all
retained behavioral assertions remain unchanged.
