# Bundled Ghostty-VT

`libghostty-vt/` is the source distribution pinned by
`libghostty-vt.vendor.json`. The two slosh fixes in `patches/` are already applied.
The source and headers are built directly by Cargo using Zig 0.16; generated
libraries, bindings, and compiler caches belong in `target/`.

`libghostty-vt/zig-pkg/` contains the eight hash-addressed source packages required
by this pin, including their licenses. `zig-packages.json` records their hashes
and archive URLs. These are committed source dependencies, not compiled outputs;
they let Zig build the terminal core without downloading packages. Git preserves
their original line endings because package hashes refer to exact source bytes.
The local `zig-pkg/.gitattributes` overrides upstream line-ending rules without
adding files inside the hash-addressed package directories.

When updating Ghostty, update the pin, reapply the recorded patches, refresh the
required source packages and their provenance, and run the Rust and original
compatibility suites. Keep upstream files unchanged except for documented patches.
