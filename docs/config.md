# Configuration

Slush reads `~/.config/slosh/config.kdl` for compatibility with existing slosh
configurations. Set `SLOSH_CONFIG=/path/to/config.kdl` to select another file.
Running sessions reload configuration when that file changes.

Start with [config/config.kdl](../config/config.kdl). Validate a configuration
or layout before using it:

```sh
slush --check config/config.kdl
slush --check config/example.layout
slush --dump-config
slush --dump-theme
```

Configuration uses KDL nodes. For example:

```kdl
gap 1
scrollback 10000
keys {
    prefix "ctrl+b"
    bind z zoom
}
theme_name "amber"
```

Shipped themes live in `contrib/themes/`; chrome and shader presets live in
`contrib/chrome/` and `contrib/shaders/`. Use `theme_dir "/path/to/themes"` for
custom named themes, or `include "relative/preset.kdl"` to load a preset.
Relative includes resolve from the including file's directory.

Keep the `SLOSH_*` environment variable names when migrating existing workflows;
the executable and Rust crate are named `slush`.
