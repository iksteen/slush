# slush

A Rust reimplementation of [slosh](https://github.com/sl0p-foo/slosh), retaining the pinned
Zig Ghostty-VT terminal core through generated C ABI bindings. The application,
session protocol, configuration parser, compositor, and platform adapters are
implemented in Rust. Original C code is used only by compatibility test oracles.

## Build and run

Clone this repository and run these commands from its root. You need Rust
stable, Zig 0.16, a native C linker, and libclang. Ghostty-VT, themes, configuration
examples, and compatibility fixtures are included; no sibling checkout or
submodule initialization is needed. Cargo fetches its locked Rust dependencies
on the first build; Ghostty's required Zig source packages are bundled too.

```sh
cargo build
cargo run -- -s work
cargo run -- ls
cargo run -- --check config/example.layout
```

Run `target/debug/slush --help` for available modes. Configuration uses the
original KDL format and `SLOSH_CONFIG` override; see the bundled [configuration
guide](docs/config.md).

`build.rs` builds the vendored Ghostty-VT library and generates bindings with
bindgen, writing generated files under `target/`. `ZIG` selects the Zig executable; `GHOSTTY_SOURCE_DIR` selects another
compatible source checkout. `GHOSTTY_LIB_DIR` skips the Zig build and must point
to a compatible static library for the selected Rust target.

## Static Linux builds

The Linux build helper uses Rust's bundled linker and musl runtime, plus Zig for
Ghostty-VT. Install the desired Rust targets, then build from this directory:

```sh
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
tools/build-linux x86_64 --release --bin slush
tools/build-linux aarch64 --release --bin slush
```

Executables land in `target/<arch>-unknown-linux-musl/release/slush`. Test an
existing executable with `python3 tests/run_app_compat.py --binary PATH --live
--report musl.json`; each report has its own `.logs/` directory. Clipboard tests
also honor `SLOSH_BIN=PATH`.

## Verify compatibility

From this directory:

```sh
cargo test --offline
cargo fmt --check
cargo clippy --offline --all-targets --all-features -- -D warnings
python3 tests/run_compat.py
python3 tests/run_app_compat.py --live
python3 tests/live_clipboard.py
```

The C runner links six unchanged original unit suites against a test-only Rust
ABI. The application runner invokes the original Python suites against the Rust
binary, rebuilding without that ABI first. `--live` includes the four original
terminal suites. Logs, check counts, and the tested binary's SHA-256 are written
under `target/app-compat/`; select individual suites with arguments such as
`session dump_layout`. Tests need permission to create local sockets and PTYs,
plus the original suite's C compiler, Python, and Node.js tools.

The additional `*_differential.py` scripts are optional comparisons with a full
C application. Set `SLOSH_REFERENCE_BIN=/path/to/slosh` to use an existing reference
executable; this repository does not include or build that application. The regular
Rust, C unit, and application compatibility suites above are self-contained.
`python3 tests/cli_differential.py` and `python3 tests/bell_differential.py`
compare CLI and bell behavior. `python3 tests/session_cli_differential.py`
checks session discovery, byte-based listing widths, and routing through temporary
socket fixtures. It also verifies native session names through daemon startup,
pane environments, splits, new tabs, recursion protection, and socket cleanup;
non-UTF-8 filenames are tested where the filesystem accepts them.
`python3 tests/json_numeric_differential.py` checks JSON numeric conversions
and pane selection against the original executable.
`python3 tests/argv_differential.py` checks native command arguments through
pane creation, splitting, new tabs, reruns, and daemon startup, plus executable
and default-shell paths where the filesystem permits non-UTF-8 filenames.
`python3 tests/path_differential.py` checks native config/layout paths, relative
includes and working directories, editor arguments, saved themes, and automatic
reloads after atomic file replacement.
`python3 tests/editor_differential.py` checks native `EDITOR` commands and
executable paths, config precedence, empty-value fallbacks, exact file arguments,
and ephemeral editor pane cleanup.
`python3 tests/project_differential.py` checks native project discovery and path
hashes, deduplication, picker selection, workspace save/restore, cwd inference,
and home-relative roots and shared layouts. It verifies exact native path bytes
and JSON escaping in script and daemon replies, including bare listing/save
commands.
`python3 tests/project_names_differential.py` checks byte-based project sorting,
name and slug truncation (including split UTF-8), exact script/socket listings,
and lookup by native names and raw versus sanitized workspace purposes. It also
verifies adopted tab labels, byte-based tab creation/rename/move, inline and file
layout round-trips, layout-file validation, and exact native working directories
in layout input and dumps.
`python3 tests/pane_metadata_differential.py` checks native pane names, terminal
title and command-label byte limits, declared and captured foreground commands,
shell-quoting limits, and restoration of exact native arguments from layouts.
It exercises JSON and bare pane listings through scripts and real daemons.
`python3 tests/config_values_differential.py` checks native bytes inside KDL
config values: includes and reloads, shell/editor commands, project roots and
shared layouts, theme directories, and config checking. It exercises both
script and daemon startup; Rust tests also verify native config render/load
round-tripping.
`python3 tests/theme_persistence_differential.py` compares theme-save file bytes
and restart behavior against C through script and socket commands. It covers
native config values, comments, CRLF, missing final newlines, duplicate and
indented selections, empty/missing files, NUL boundaries, and quotes in names.
`python3 tests/theme_names_differential.py` checks native theme filenames and
byte sorting, platform-specific name validation, native diagnostics and reload
warnings, byte-capped picker actions, and discovery's 64-name limit. It compares
script and socket requests with the original C executable.
`python3 tests/driver_differential.py` checks the script driver's 4,095-byte
frames, dispatch before newline, EOF and NUL boundaries, native stderr bytes,
quiet-time parsing, and bare resize scans through script and socket commands.
`cargo test --offline --test config_text_differential` compares fixed text
buffers and saved config round trips with the original C loader.
`python3 tests/config_marks_differential.py` checks native mark bytes in pane
buttons, modals, minimized panes, tab bars and sidebars, snapshot text and
terminal output, plus byte-capped word separators in double-click selection.
`python3 tests/osc_bytes_differential.py` checks raw OSC 5577 status and button
labels, composed OSC 7501 status limits, native shader preset paths and replies,
and compact/sidebar rendering through script and socket clients.
`python3 tests/config_diagnostics_differential.py` checks native filenames and
unknown setting names, include errors/cycles, diagnostic byte and count limits,
and exact CLI and script/socket reload errors against C.
`python3 tests/request_differential.py` checks native pane input through script
and socket requests, Unicode escape decoding, decoded NUL boundaries, malformed
strings, and the original numeric extensions. Project checks also send native
workspace/layout paths and relative dump bases through both front ends.
`SLOSH_BIN` and `SLOSH_REFERENCE_BIN` select the executables to compare.

## Platforms and source layout

The standalone compatibility gate contains 67 application/live suites, six C
compatibility suites, and the Rust tests. Linux is the primary development gate;
use focused checks during iteration and full audits at milestones, completing
Linux before macOS follow-up. Before extracting the standalone fixtures, Linux
passed all 70 upstream suites (2,660 checks) and native ARM64 macOS passed all
70 (2,658 checks), plus six C compatibility suites and 75 Rust tests each.
The C runner passes 375 checks on each platform. The latest reports are
`target/app-compat/linux-readiness-full.json` and
`target/compat/macos-native/readiness/macos-readiness-full.json`; both record
the tested release's SHA-256, with original-suite logs saved alongside them.
These full audits predate the project rename; their generated reports preserve
historical executable names and hashes. Checks of the renamed binary are saved
in `target/app-compat/slush-rename-linux-focused.json`.
Ghostty-VT is statically linked; the macOS executable needs only system libraries.
Intel macOS has cross-build checks.

The original macOS bell suite has an intermittent timing failure: both panes
run delayed ringers, and the second can ring after focus changes. This reproduces
in the original C executable. `tests/bell_differential.py` explicitly triggers
each pane's bell to verify acknowledgement and preservation of a later sibling
bell without relying on competing timers. Original suites remain unchanged.

Windows adapters use ConPTY, Winsock AF_UNIX, native directory notifications,
and console input. `tools/build-windows --bin slush` cross-builds for GNU x86-64
using MinGW and LLD; native Windows session validation remains outstanding.
`tests/windows_session.rs` covers detach, reattach, resize, and config reload.

`src/app/` owns application behavior; `src/ghostty*` wraps the terminal ABI;
`src/client*`, `src/server*`, and `src/pty*` implement sessions and host I/O.
Embedded defaults and terminfo live in `assets/`.

## Bundled dependencies and fixtures

`vendor/libghostty-vt/` contains the pinned Zig source. Its upstream commit and
the two applied patches are recorded in `vendor/libghostty-vt.vendor.json` and
`vendor/patches/`. Its hash-addressed dependency sources live in
`vendor/libghostty-vt/zig-pkg/`, with provenance in `vendor/zig-packages.json`.
`tests/reference/slosh/` contains nine C oracle modules, their required headers,
six unit suites, behavioral Python suites, and necessary test fixtures.
`tests/reference/slosh.vendor.json` records the source commit and selection.
The upstream docs, agent-skill, and shader-REPL tooling suites are excluded;
the C application, website, build system, and release tooling are absent.
Application builds do not compile C oracles. `cargo test` compiles small C
comparators against the same Ghostty library that Cargo built for Rust.
Themes and presets are in `contrib/`, examples in `config/`, and docs in `docs/`.

## Attribution

This port is adapted from slosh by Peter Geissler; its MIT copyright notice is
preserved in [LICENSE](LICENSE). The retained Ghostty-VT core's notice is in
[licenses/libghostty-vt.txt](licenses/libghostty-vt.txt).
