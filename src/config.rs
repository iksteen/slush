//! Fail-open KDL configuration, shared by the app, checker, and renderer.
use crate::{
    bindings::{Action, Bindings, Chord},
    expr::Program,
    input::SHIFT,
    json_control::{native_bytes, os_string},
    kdl::{self, Node},
    path,
    screen::Color,
    shader::{self, Kind, Shader},
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fmt::Write,
    path::{Path, PathBuf},
    rc::Rc,
};

const DEFAULTS: &str = include_str!("../assets/defaults.kdl");
const PALETTE: &str = include_str!("../assets/theme.kdl");
pub const STATE_NAMES: [&str; 9] = [
    "dragging",
    "drop_hover",
    "drop_target",
    "dead",
    "suspended",
    "bell",
    "scrolled",
    "floating",
    "unfocused",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Setting {
    Bool(bool),
    Number(u64),
    Text(Vec<u8>),
    Sides([u16; 4]),
    Size { cols: u16, rows: u16 },
}
/// Configuration diagnostics retain the bytes emitted by the C loader.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    bytes: Vec<u8>,
    display: String,
}
impl Diagnostic {
    pub fn new(bytes: impl AsRef<[u8]>) -> Self {
        let bytes = bytes.as_ref().split(|b| *b == 0).next().unwrap_or_default();
        let bytes = bytes[..bytes.len().min(191)].to_vec();
        Self {
            display: String::from_utf8_lossy(&bytes).into_owned(),
            bytes,
        }
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
impl std::ops::Deref for Diagnostic {
    type Target = str;
    fn deref(&self) -> &str {
        &self.display
    }
}
impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.display.fmt(f)
    }
}
#[derive(Clone, Debug, Default)]
pub struct Chains {
    pub content: Vec<Shader>,
    pub chrome: Vec<Shader>,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub settings: Vec<(String, Setting)>,
    pub bindings: Bindings,
    pub colors: BTreeMap<String, Color>,
    pub shaders: Chains,
    pub states: [Chains; 9],
    pub shell: Option<OsString>,
    pub editor: Option<OsString>,
    pub project_layout: Option<PathBuf>,
    pub project_roots: Vec<PathBuf>,
    pub project_depth: u8,
    pub theme_dir: Option<PathBuf>,
    pub theme_name: Option<Vec<u8>>,
    pub files: Vec<PathBuf>,
    pub messages: Vec<Diagnostic>,
    pub explicit_colors: BTreeSet<String>,
    color_order: Vec<String>,
    scrolled_declared: bool,
    bell_declared: bool,
    in_theme: bool,
}

pub fn parse_color(text: &str) -> Option<Color> {
    if text.len() != 7 || !text.starts_with('#') {
        return None;
    }
    let value = u32::from_str_radix(&text[1..], 16).ok()?;
    Some(Color::rgb(
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
    ))
}
fn blend(a: Color, b: Color, pct: i32) -> Color {
    if !a.set {
        return b;
    }
    if !b.set {
        return a;
    }
    let channel = |a: u8, b: u8| (a as i32 + (b as i32 - a as i32) * pct / 100) as u8;
    Color::rgb(channel(a.r, b.r), channel(a.g, b.g), channel(a.b, b.b))
}
fn quote(text: &str) -> String {
    // JSON string escaping is the subset accepted by the KDL reader.
    serde_json::to_string(text).unwrap()
}

fn quote_bytes(value: &[u8]) -> Vec<u8> {
    let mut out = vec![b'"'];
    for byte in value {
        if matches!(byte, b'"' | b'\\') {
            out.push(b'\\');
        }
        out.push(*byte);
    }
    out.push(b'"');
    out
}

impl Default for Config {
    fn default() -> Self {
        let root = kdl::parse(DEFAULTS).expect("embedded defaults must parse");
        let palette = kdl::parse(PALETTE).unwrap();
        let palette = palette.child("theme").unwrap();
        let color_order = palette.kids.iter().map(|n| n.name.clone()).collect();
        let colors: BTreeMap<String, Color> = palette
            .kids
            .iter()
            .map(|node| {
                (
                    node.name.clone(),
                    parse_color(node.arg(0, "")).unwrap_or_default(),
                )
            })
            .collect();
        let mut config = Self {
            settings: Vec::new(),
            bindings: Bindings::default(),
            colors,
            shaders: Chains::default(),
            states: std::array::from_fn(|_| Chains::default()),
            shell: None,
            editor: None,
            project_layout: None,
            project_roots: Vec::new(),
            project_depth: 2,
            theme_dir: None,
            theme_name: None,
            files: Vec::new(),
            messages: Vec::new(),
            explicit_colors: BTreeSet::new(),
            color_order,
            scrolled_declared: false,
            bell_declared: false,
            in_theme: false,
        };
        for node in &root.kids {
            let value = match node.name.as_str() {
                "keys" | "shaders" | "states" => continue,
                "padding" | "tab_bar_padding" => Setting::Sides(sides(node).unwrap()),
                "min_pane" | "min_split" => Setting::Size {
                    cols: node.prop_int("cols", 0) as u16,
                    rows: node.prop_int("rows", 0) as u16,
                },
                _ => {
                    let arg = node.arg(0, "");
                    if let Some(b) = kdl::boolean(arg) {
                        Setting::Bool(b)
                    } else if let Some(n) = kdl::integer(arg) {
                        Setting::Number(n as u64)
                    } else {
                        Setting::Text(arg.as_bytes().into())
                    }
                }
            };
            config.settings.push((node.name.clone(), value));
        }
        #[cfg(windows)]
        config.set("ctrl_d_exits", Setting::Bool(true));
        config.load_keys(root.child("keys").unwrap(), None);
        // The compiled-in chains have no target colour or channel preference.
        // A user-authored chain instead defaults to the frame accent.
        for state in &root.child("states").unwrap().kids {
            let i = STATE_NAMES
                .iter()
                .position(|name| *name == state.name)
                .unwrap();
            for entry in &state.kids {
                let (made, warning) = parse_shader(entry, Color::default(), false);
                assert!(warning.is_none(), "invalid built-in shader");
                let (mut shader, chrome) = made.unwrap();
                if entry.property("channel").is_none() {
                    shader.channels = 0;
                }
                if chrome {
                    config.states[i].chrome.push(shader);
                } else {
                    config.states[i].content.push(shader);
                }
            }
        }
        config.derive_unfocused(false);
        config.derive_scrolled_and_bell();
        config
    }
}

fn sides(node: &Node) -> Option<[u16; 4]> {
    let v = |i| node.arg_int(i, 0) as u16;
    match node.args.len() {
        1 => Some([v(0); 4]),
        2 => Some([v(0), v(1), v(0), v(1)]),
        4 => Some([v(0), v(1), v(2), v(3)]),
        _ => None,
    }
}

impl Config {
    pub fn setting(&self, name: &str) -> Option<&Setting> {
        self.settings
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, v)| v)
    }
    fn set(&mut self, name: &str, value: Setting) {
        if let Some((_, v)) = self.settings.iter_mut().find(|(key, _)| key == name) {
            *v = value;
        } else {
            self.settings.push((name.into(), value));
        }
    }
    pub fn number(&self, name: &str) -> u64 {
        match self.setting(name) {
            Some(Setting::Number(v)) => *v,
            _ => panic!("not a numeric setting: {name}"),
        }
    }
    pub fn boolean(&self, name: &str) -> bool {
        match self.setting(name) {
            Some(Setting::Bool(v)) => *v,
            _ => panic!("not a boolean setting: {name}"),
        }
    }
    pub fn text(&self, name: &str) -> Option<&str> {
        if matches!(name, "shell" | "editor" | "project_layout") {
            return self.native_text(name).and_then(OsStr::to_str);
        }
        self.text_bytes(name)
            .and_then(|v| std::str::from_utf8(v).ok())
    }
    pub fn text_bytes(&self, name: &str) -> Option<&[u8]> {
        match self.setting(name) {
            Some(Setting::Text(v)) => Some(v),
            _ => None,
        }
    }
    pub fn native_text(&self, name: &str) -> Option<&OsStr> {
        match name {
            "shell" => self.shell.as_deref(),
            "editor" => self.editor.as_deref(),
            "project_layout" => self.project_layout.as_deref().map(Path::as_os_str),
            _ => None,
        }
    }
    pub fn color(&self, name: &str) -> Color {
        self.colors.get(name).copied().unwrap_or_default()
    }
    fn complain(&mut self, file: Option<&Path>, line: usize, message: impl AsRef<str>) {
        self.complain_bytes(file, line, message.as_ref().as_bytes());
    }
    fn complain_bytes(&mut self, file: Option<&Path>, line: usize, message: &[u8]) {
        if self.messages.len() >= 32 {
            return;
        }
        let message = Diagnostic::new(message);
        let mut full = Vec::new();
        if let Some(file) = file {
            full.extend_from_slice(&native_bytes(file.file_name().unwrap_or(file.as_os_str())));
            if line > 0 {
                full.extend_from_slice(format!(":{line}").as_bytes());
            }
            full.extend_from_slice(b": ");
        }
        full.extend_from_slice(message.bytes());
        self.messages.push(Diagnostic::new(full));
    }
    pub fn default_path() -> PathBuf {
        let nonempty = |name| std::env::var_os(name).filter(|v| !v.is_empty());
        nonempty("SLOSH_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                nonempty("XDG_CONFIG_HOME")
                    .map(|dir| path::join_os(Path::new(&dir), Path::new("slosh/config.kdl")))
                    .unwrap_or_else(|| {
                        path::join_os(
                            Path::new(&path::home_os().unwrap_or_else(|| ".".into())),
                            Path::new(".config/slosh/config.kdl"),
                        )
                    })
            })
    }
    pub fn load(&mut self, file: impl AsRef<Path>) -> bool {
        self.messages.clear();
        self.load_into(file.as_ref(), 0)
    }
    /// The same schema is used to distinguish config files from session layouts.
    pub fn is_setting(&self, name: &str) -> bool {
        self.setting(name).is_some()
            || matches!(
                name,
                "include"
                    | "keys"
                    | "shaders"
                    | "states"
                    | "theme"
                    | "theme_name"
                    | "theme_dir"
                    | "tab_bar_pad"
                    | "shell"
                    | "editor"
                    | "project_roots"
                    | "project_layout"
            )
    }
    fn load_into(&mut self, file: &Path, depth: usize) -> bool {
        if depth > 8 {
            self.complain_bytes(
                None,
                0,
                &[
                    native_bytes(file.file_name().unwrap_or(file.as_os_str())).as_ref(),
                    b": includes nested too deep (a cycle?)",
                ]
                .concat(),
            );
            return false;
        }
        if self.files.len() < 16
            && !self.files.iter().any(|f| {
                #[cfg(unix)]
                {
                    f.as_os_str() == file.as_os_str()
                }
                #[cfg(windows)]
                {
                    std::path::Path::new(f) == std::path::Path::new(file)
                }
            })
        {
            self.files.push(file.into());
        }
        let source = match std::fs::read(file) {
            Ok(source) => source,
            Err(_) => {
                self.complain_bytes(
                    None,
                    0,
                    &[b"cannot open ", native_bytes(file.as_os_str()).as_ref()].concat(),
                );
                return false;
            }
        };
        let native = match kdl::parse_native(&source) {
            Ok(root) => root,
            Err(error) => {
                self.complain(None, 0, error.to_string());
                return false;
            }
        };
        for include in native.kids.iter().filter(|n| n.name == b"include") {
            if include.args.is_empty() {
                self.complain(Some(file), include.line, "include needs a file");
            }
            for arg in &include.args {
                self.load_into(
                    &path::include_os(file, Path::new(&*os_string(arg))),
                    depth + 1,
                );
            }
        }
        if let Some(dir) = native.child("theme_dir").and_then(|n| n.args.first()) {
            self.theme_dir = Some(path::include_os(file, Path::new(&*os_string(dir))));
        }
        let root = native.clone().text();
        if let Some(node) = native.child("theme_name") {
            if let Some(name) = node.args.first() {
                self.apply_theme_name(name, Some(file), node.line, depth);
            } else {
                self.complain(Some(file), node.line, "theme_name needs a name");
            }
        }
        for node in &native.kids {
            if !std::str::from_utf8(&node.name).is_ok_and(|name| self.is_setting(name)) {
                self.complain_bytes(
                    Some(file),
                    node.line,
                    &if node.name == b"layout" {
                        b"this is a layout, not a config: `slosh --layout` reads those".to_vec()
                    } else {
                        [b"unknown setting: ", node.name.as_slice()].concat()
                    },
                );
            }
        }
        self.load_scalars(&root, file);
        self.load_native_scalars(&native, file);
        if let Some(node) = root.child("shaders") {
            self.shaders = self.parse_list(node, Some(file), false, false);
        }
        let unfocused = if let Some(node) = root.child("states") {
            self.load_states(node, Some(file))
        } else {
            false
        };
        self.derive_unfocused(unfocused);
        if let Some(theme) = root.child("theme") {
            self.load_theme(theme, file);
        }
        self.derive_scrolled_and_bell();
        if let Some(keys) = root.child("keys") {
            self.load_keys(keys, Some(file));
        }
        true
    }

    fn load_scalars(&mut self, root: &Node, file: &Path) {
        // Resolve each first child in schema order: source order does not alter precedence.
        let mut names: Vec<String> = self.settings.iter().map(|(name, _)| name.clone()).collect();
        // Scalar updates commute. Keep diagnostic order aligned with slosh's loader.
        let checks = [
            "padding",
            "tab_bar_padding",
            "tab_bar_index",
            "tab_bar_side",
            "size_follows",
            "keep_dead",
            "scrollback",
            "scrollback_bytes",
        ];
        names.sort_by_key(|name| {
            checks
                .iter()
                .position(|check| *check == name)
                .unwrap_or(checks.len())
        });
        for name in names {
            let Some(node) = root.child(&name) else {
                continue;
            };
            let old = self.setting(&name).unwrap();
            let value = match old {
                Setting::Bool(v) => Setting::Bool(node.arg_bool(0, *v)),
                Setting::Number(v) => {
                    let n = node.arg_int(0, *v as i64);
                    if matches!(name.as_str(), "scrollback" | "scrollback_bytes") {
                        if n < 0 {
                            let what = if name == "scrollback" {
                                "a number of lines"
                            } else {
                                "a byte ceiling"
                            };
                            self.complain(
                                Some(file),
                                node.line,
                                format!("{name} is {what}, 0 for none, not {n}"),
                            );
                            continue;
                        }
                        Setting::Number(n as u64)
                    } else if matches!(
                        name.as_str(),
                        "dim_unfocused" | "modal_scrim" | "float_shadow"
                    ) {
                        Setting::Number(n.clamp(0, 255) as u64)
                    } else {
                        Setting::Number(n as u16 as u64)
                    }
                }
                Setting::Sides(_) => {
                    let Some(sides) = sides(node) else {
                        self.complain(Some(file),node.line,format!("{name} takes 1, 2 or 4 values (all, vertical horizontal, or top right bottom left), not {}",node.args.len()));
                        continue;
                    };
                    Setting::Sides(sides)
                }
                Setting::Size { cols, rows } => Setting::Size {
                    cols: node.prop_int("cols", *cols as i64) as u16,
                    rows: node.prop_int("rows", *rows as i64) as u16,
                },
                Setting::Text(_) => {
                    let Some(arg) = node.args.first() else {
                        continue;
                    };
                    let text = match name.as_str() {
                        "tab_bar_index" if !matches!(arg.as_str(), "prefix" | "right") => {
                            self.complain(
                                Some(file),
                                node.line,
                                format!("tab_bar_index is \"prefix\" or \"right\", not \"{arg}\""),
                            );
                            continue;
                        }
                        "tab_bar_side" if !matches!(arg.as_str(), "top" | "left" | "right") => {
                            self.complain(
                                Some(file),
                                node.line,
                                format!(
                                    "tab_bar_side is \"top\", \"left\" or \"right\", not \"{arg}\""
                                ),
                            );
                            continue;
                        }
                        "size_follows"
                            if !matches!(arg.as_str(), "active" | "smallest" | "largest") =>
                        {
                            self.complain(
                                Some(file),
                                0,
                                format!(
                                    "size_follows wants active, smallest or largest, not \"{arg}\""
                                ),
                            );
                            continue;
                        }
                        "keep_dead" => match arg.as_str() {
                            "true" => "all".into(),
                            "false" => "none".into(),
                            "all" | "none" | "commands" => arg.clone(),
                            _ => {
                                self.complain(
                                    Some(file),
                                    node.line,
                                    format!("keep_dead: {arg} (want commands, all or none)"),
                                );
                                continue;
                            }
                        },
                        "title_align" => {
                            if matches!(arg.as_str(), "left" | "right") {
                                arg.clone()
                            } else {
                                "center".into()
                            }
                        }
                        // Native text is loaded below, before its bytes can be
                        // replaced by the explicit UTF-8 view of this tree.
                        "busy_mark" | "word_separators" => continue,
                        name if name.ends_with("_mark") => continue,
                        _ => arg.clone(),
                    };
                    Setting::Text(text.into_bytes())
                }
            };
            self.set(&name, value);
        }
        if let Some(node) = root.child("tab_bar_pad")
            && let Some(Setting::Sides(mut sides)) = self.setting("tab_bar_padding").cloned()
        {
            sides[0] = node.arg_int(0, sides[0] as i64) as u16;
            self.set("tab_bar_padding", Setting::Sides(sides));
        }
    }

    fn load_native_scalars(&mut self, root: &kdl::NativeNode, file: &Path) {
        for name in [
            "zoom_mark",
            "zoom_on_mark",
            "close_mark",
            "min_mark",
            "newtab_mark",
            "bell_mark",
            "busy_mark",
            "word_separators",
        ] {
            if let Some(arg) = root.child(name).and_then(|n| n.args.first()) {
                let arg = arg.split(|b| *b == 0).next().unwrap_or_default();
                let cap = if matches!(name, "busy_mark" | "word_separators") {
                    64
                } else {
                    32
                };
                let mut n = arg.len().min(cap - 1);
                if cap == 32 && arg.len() >= cap {
                    // Match set_mark: only back over continuation bytes when
                    // the mark actually exceeds its fixed C buffer.
                    while n > 0 && arg[n] & 0xc0 == 0x80 {
                        n -= 1;
                    }
                }
                self.set(name, Setting::Text(arg[..n].into()));
            }
        }
        for (name, value) in [("shell", &mut self.shell), ("editor", &mut self.editor)] {
            if let Some(arg) = root.child(name).and_then(|n| n.args.first()) {
                *value = Some(os_string(arg).into_owned());
            }
        }
        if let Some(arg) = root.child("project_layout").and_then(|n| n.args.first()) {
            self.project_layout = Some(os_string(arg).into_owned().into());
        }
        if let Some(node) = root.child("project_roots") {
            self.project_roots = node
                .args
                .iter()
                .take(8)
                .map(|arg| PathBuf::from(os_string(arg).into_owned()))
                .collect();
            if node.args.len() > 8 {
                self.complain(
                    Some(file),
                    node.line,
                    "project_roots takes at most 8 directories",
                );
            }
            if node.args.is_empty() {
                self.complain(
                    Some(file),
                    node.line,
                    "project_roots takes one or more directories",
                );
            }
            let d = node.prop_int("depth", self.project_depth as i64);
            if !(1..=8).contains(&d) {
                self.complain(
                    Some(file),
                    node.line,
                    format!("depth is between 1 and 8, not {d}"),
                );
            } else {
                self.project_depth = d as u8;
            }
        }
    }

    fn load_keys(&mut self, keys: &Node, file: Option<&Path>) {
        if let Some(node) = keys.child("prefix")
            && let Some(text) = node.args.first()
        {
            if let Some((chord, _)) = Chord::parse(text) {
                self.bindings.prefix = chord;
            } else {
                self.complain(file, node.line, format!("bad prefix: {text}"));
            }
        }
        for node in &keys.kids {
            let direct = node.name == "direct";
            let entries = if direct {
                node.kids.as_slice()
            } else {
                std::slice::from_ref(node)
            };
            for binding in entries.iter().filter(|n| n.name == "bind") {
                let text = binding.arg(0, "(none)");
                let parsed = if binding.args.len() >= 2 {
                    Chord::parse(text)
                } else {
                    None
                };
                let Some((chord, implied)) = parsed else {
                    self.complain(file, binding.line, format!("bad key: {text}"));
                    continue;
                };
                let action = binding.arg(1, "");
                let Some(action) = Action::from_name(action) else {
                    self.complain(file, binding.line, format!("unknown action: {action}"));
                    continue;
                };
                self.bindings.add(chord, action, direct);
                if implied {
                    self.bindings.add(
                        Chord {
                            mods: chord.mods & !SHIFT,
                            ..chord
                        },
                        action,
                        direct,
                    );
                }
            }
        }
    }
    fn load_states(&mut self, node: &Node, file: Option<&Path>) -> bool {
        let mut unfocused = false;
        for state in &node.kids {
            let Some(i) = STATE_NAMES.iter().position(|n| *n == state.name) else {
                self.complain(
                    file,
                    state.line,
                    format!("unknown pane state: {}", state.name),
                );
                continue;
            };
            unfocused |= state.name == "unfocused";
            self.scrolled_declared |= state.name == "scrolled";
            self.bell_declared |= state.name == "bell";
            self.states[i] = self.parse_list(state, file, false, false);
        }
        unfocused
    }

    fn parse_list(
        &mut self,
        node: &Node,
        file: Option<&Path>,
        default_chrome: bool,
        warn_overflow: bool,
    ) -> Chains {
        let mut chains = Chains::default();
        for entry in &node.kids {
            let (made, warning) = parse_shader(entry, self.color("frame_focus"), default_chrome);
            if let Some(warning) = warning {
                self.complain(file, entry.line, warning);
            }
            let Some((shader, chrome)) = made else {
                continue;
            };
            let chain = if chrome {
                &mut chains.chrome
            } else {
                &mut chains.content
            };
            if chain.len() < shader::MAX_ATTACHED {
                chain.push(shader);
            } else if warn_overflow {
                self.complain(
                    file,
                    0,
                    format!(
                        "more than 4 passes for the {}",
                        if chrome { "frame" } else { "contents" }
                    ),
                );
            }
        }
        chains
    }
    pub fn parse_chain(
        &mut self,
        text: impl AsRef<[u8]>,
        default_chrome: bool,
    ) -> Result<Chains, kdl::Error> {
        let root = kdl::parse_native(text.as_ref())?.text();
        let block = root
            .kids
            .iter()
            .rev()
            .find(|n| n.name == "shaders")
            .unwrap_or(&root);
        Ok(self.parse_list(block, None, default_chrome, true))
    }
    pub fn parse_chain_file(&mut self, file: impl AsRef<Path>) -> Result<Chains, Diagnostic> {
        let file = file.as_ref();
        let source = std::fs::read(file).map_err(|_| {
            Diagnostic::new([b"cannot open ".as_slice(), &native_bytes(file.as_os_str())].concat())
        })?;
        let root = kdl::parse_native(&source)
            .map_err(|e| Diagnostic::new(e.to_string()))?
            .text();
        let mut entries = root.clone();
        entries.kids = root
            .kids
            .iter()
            .filter(|n| n.name == "shaders")
            .flat_map(|n| n.kids.iter().cloned())
            .collect();
        if !root.kids.iter().any(|n| n.name == "shaders") {
            self.complain(None,0,if root.child("states").is_some() {
                "only states { } in it, which is a config's and not a pane's -- paste the pass itself"
            } else {"no shaders { } block in it"});
        }
        Ok(self.parse_list(&entries, None, false, true))
    }

    fn derive_unfocused(&mut self, declared: bool) {
        if declared {
            return;
        }
        let amount = self.number("dim_unfocused") as u8;
        self.states[8].content = if amount == 0 {
            Vec::new()
        } else {
            vec![Shader::new(Kind::Dim, Color::default(), amount)]
        };
    }
    fn derive_scrolled_and_bell(&mut self) {
        let expression = |kind, color, channels, source: &str| {
            let mut sh = Shader::new(kind, color, 128);
            sh.channels = channels;
            sh.expression = Some(Rc::new(RefCell::new(
                Program::compile(source).expect("built-in shader expression"),
            )));
            sh
        };
        if !self.scrolled_declared {
            self.states[6].content = vec![
                Shader::new(Kind::Tint, self.color("scroll_bg"), 22),
                expression(
                    Kind::Dim,
                    Color::default(),
                    0,
                    "(above > 0) * max(0, (min(4, rows / 4) - y) * 60)",
                ),
                expression(
                    Kind::Dim,
                    Color::default(),
                    0,
                    "(below > 0) * max(0, (y - rows + min(4, rows / 4) + 1) * 60)",
                ),
            ];
        }
        if !self.bell_declared {
            self.states[5] = Chains::default();
            if self.boolean("bell_indicator") {
                let bell = self.color("bell");
                let fg = self.color("default_fg");
                let mean = |a: u8, b: u8| ((a as u16 + b as u16) / 2) as u8;
                let sheen = Color::rgb(mean(bell.r, fg.r), mean(bell.g, fg.g), mean(bell.b, fg.b));
                self.states[5].content.push(expression(Kind::Tint,sheen,0,"(since < 900) * max(0, 95 - abs(x + 2 * y - since / 4) * (6 + sin(since / 2) / 96) - since / 64)"));
                self.states[5].chrome.push(expression(
                    Kind::Tint,
                    bell,
                    shader::FG,
                    "since < 900 ? (since / 150 % 2 == 0) * 255 : 120 + sin(since / 8 + 158) / 3",
                ));
            }
        }
    }
    fn load_theme(&mut self, theme: &Node, file: &Path) {
        let names = self.color_order.clone();
        for name in names {
            let Some(node) = theme.child(&name) else {
                continue;
            };
            let Some(value) = node.args.first() else {
                continue;
            };
            if let Some(color) = parse_color(value) {
                self.colors.insert(name.clone(), color);
                if !self.in_theme {
                    self.explicit_colors.insert(name);
                }
            } else {
                self.complain(
                    Some(file),
                    node.line,
                    format!("bad colour for {name}: {value}"),
                );
            }
        }
        for (name, a, b, pct) in [
            ("tab_status_fg", "tab_count", "tab_hover", 40),
            ("tab_idle_bg", "default_bg", "tab_idle", 43),
            ("tab_status_stripe", "default_bg", "tab_hover", 12),
        ] {
            if theme.child(name).is_none() {
                self.colors
                    .insert(name.into(), blend(self.color(a), self.color(b), pct));
            }
        }
        for (name, source) in [
            ("tab_status_busy", "tab_hover"),
            ("tab_status_spinner", "tab_status_busy"),
        ] {
            if theme.child(name).is_none() {
                self.colors.insert(name.into(), self.color(source));
            }
        }
    }
    pub fn theme_dirs(&self) -> Vec<PathBuf> {
        let root = self
            .files
            .first()
            .cloned()
            .unwrap_or_else(Self::default_path);
        let mut dirs = vec![
            self.theme_dir
                .clone()
                .unwrap_or_else(|| path::join_os(path::dir_os(&root), Path::new("themes"))),
        ];
        if let Ok(exe) = std::env::current_exe() {
            let up = path::dir_os(path::dir_os(&exe));
            dirs.extend([
                path::join_os(up, Path::new("share/slosh/themes")),
                path::join_os(up, Path::new("contrib/themes")),
            ]);
        }
        dirs.push("/usr/local/share/slosh/themes".into());
        // Find shipped themes when running directly from this checkout.
        let checkout = format!("{}/contrib/themes", env!("CARGO_MANIFEST_DIR"));
        if std::path::Path::new(&checkout).is_dir() {
            dirs.push(checkout.into());
        }
        let mut unique = Vec::new();
        for dir in dirs {
            if !unique.contains(&dir) {
                unique.push(dir);
            }
        }
        unique
    }
    pub fn themes(&self) -> Vec<Vec<u8>> {
        let mut names = Vec::new();
        for dir in self.theme_dirs() {
            if names.len() == 64 {
                break;
            }
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                if names.len() == 64 {
                    break;
                }
                let file_name = entry.file_name();
                let name = native_bytes(&file_name);
                if !name.starts_with(b".")
                    && let Some(name) = name.strip_suffix(b".kdl")
                    && name.len() < 64
                    && !name.is_empty()
                    && !names.iter().any(|seen| seen == name)
                {
                    names.push(name.to_vec());
                }
            }
        }
        names.sort();
        names
    }
    fn apply_theme_name(&mut self, name: &[u8], file: Option<&Path>, line: usize, depth: usize) {
        let name = name.split(|b| *b == 0).next().unwrap_or_default();
        if !theme_name_ok(name) {
            self.complain_bytes(
                file,
                line,
                &[
                    b"theme_name is a name, not a path: ",
                    name,
                    b" (use include for a file elsewhere)",
                ]
                .concat(),
            );
            return;
        }
        let dirs = self.theme_dirs();
        let filename = os_string(&[name, b".kdl"].concat()).into_owned();
        let Some(theme) = dirs
            .iter()
            .map(|d| path::join_os(d, Path::new(&filename)))
            .find(|f| std::fs::File::open(f).is_ok())
        else {
            let mut tried = Vec::new();
            for dir in &dirs {
                if !tried.is_empty() {
                    tried.extend_from_slice(b", ");
                }
                tried.extend_from_slice(&native_bytes(dir.as_os_str()));
                tried.truncate(511);
            }
            self.complain_bytes(
                file,
                line,
                &[b"no theme named ", name, b" in ", &tried].concat(),
            );
            return;
        };
        self.theme_name = Some(name.into());
        let was = self.in_theme;
        self.in_theme = true;
        self.load_into(&theme, depth + 1);
        self.in_theme = was;
    }
    pub fn apply_theme(&mut self, name: impl AsRef<[u8]>) -> bool {
        let name = name.as_ref().split(|b| *b == 0).next().unwrap_or_default();
        let before = self.messages.len();
        self.apply_theme_name(name, None, 0, 0);
        self.theme_name.as_deref() == Some(name) && self.messages.len() == before
    }

    pub fn render_theme(&self) -> String {
        String::from_utf8_lossy(&self.render_theme_bytes()).into_owned()
    }
    pub fn render_theme_bytes(&self) -> Vec<u8> {
        let mut bytes = b"// Every configured compositor colour.\n".to_vec();
        if let Some(name) = &self.theme_name {
            bytes.extend_from_slice(b"// Started from theme_name ");
            bytes.extend_from_slice(&quote_bytes(name));
            bytes.extend_from_slice(b".\n");
        }
        let mut out = String::new();
        out.push_str("theme {\n");
        for name in &self.color_order {
            let color = self.color(name);
            if let Some(hex) = color.hex() {
                writeln!(out, "    {name} {}", quote(&hex)).unwrap();
            } else {
                writeln!(out, "    // {name} (terminal default)").unwrap();
            }
        }
        out.push_str("}\n");
        bytes.extend_from_slice(out.as_bytes());
        bytes
    }
    pub fn render(&self) -> String {
        String::from_utf8_lossy(&self.render_bytes()).into_owned()
    }
    pub fn render_bytes(&self) -> Vec<u8> {
        let mut bytes = b"// Slosh configuration. Omitted settings use the defaults.\n\n".to_vec();
        for (name, value) in &self.settings {
            bytes.extend_from_slice(name.as_bytes());
            bytes.push(b' ');
            if let Setting::Text(value) = value {
                bytes.extend_from_slice(&quote_bytes(value));
                bytes.push(b'\n');
                continue;
            }
            let mut out = String::new();
            match value {
                Setting::Bool(v) => writeln!(out, "{v}"),
                Setting::Number(v) => writeln!(out, "{v}"),
                Setting::Text(_) => unreachable!(),
                Setting::Sides(v) => writeln!(out, "{} {} {} {}", v[0], v[1], v[2], v[3]),
                Setting::Size { cols, rows } => writeln!(out, "cols={cols} rows={rows}"),
            }
            .unwrap();
            bytes.extend_from_slice(out.as_bytes());
        }
        for name in ["shell", "editor", "project_layout"] {
            if let Some(value) = self.native_text(name) {
                bytes.extend_from_slice(name.as_bytes());
                bytes.push(b' ');
                bytes.extend_from_slice(&quote_bytes(&native_bytes(value)));
                bytes.push(b'\n');
            }
        }
        bytes.extend_from_slice(b"// tab_bar_pad 0\n");
        for (name, example) in [
            ("shell", "/bin/zsh"),
            ("editor", "nvim"),
            ("project_layout", "~/.config/slosh/project.layout"),
        ] {
            if self.native_text(name).is_none() {
                bytes.extend_from_slice(format!("// {name} {}\n", quote(example)).as_bytes());
            }
        }
        if self.project_roots.is_empty() {
            bytes.extend_from_slice(b"// project_roots \"~/dev\" depth=2\n");
        } else {
            bytes.extend_from_slice(b"project_roots");
            for root in &self.project_roots {
                bytes.push(b' ');
                bytes.extend_from_slice(&quote_bytes(&native_bytes(root.as_os_str())));
            }
            bytes.extend_from_slice(format!(" depth={}\n", self.project_depth).as_bytes());
        }
        if let Some(dir) = &self.theme_dir {
            bytes.extend_from_slice(b"theme_dir ");
            bytes.extend_from_slice(&quote_bytes(&native_bytes(dir.as_os_str())));
            bytes.push(b'\n');
        } else {
            bytes.extend_from_slice(b"// theme_dir \"~/.config/slosh/themes\"\n");
        }
        if let Some(name) = &self.theme_name {
            bytes.extend_from_slice(b"theme_name ");
            bytes.extend_from_slice(&quote_bytes(name));
            bytes.push(b'\n');
        } else {
            bytes.extend_from_slice(b"// theme_name \"phosphor\"\n");
        }
        let mut out = String::new();
        if self.explicit_colors.is_empty() {
            out.push_str("// theme { frame_focus \"#00ff88\" }\n");
        } else {
            out.push_str("theme {\n");
            for name in &self.explicit_colors {
                if let Some(hex) = self.color(name).hex() {
                    writeln!(out, "    {name} {}", quote(&hex)).unwrap();
                }
            }
            out.push_str("}\n");
        }
        out.push_str("\nshaders {\n");
        render_chains(&mut out, &self.shaders, "    ");
        out.push_str("}\n\nstates {\n");
        for (i, name) in STATE_NAMES.iter().enumerate() {
            if (*name == "unfocused" && is_derived_unfocused(self))
                || (*name == "scrolled" && !self.scrolled_declared)
                || (*name == "bell" && !self.bell_declared)
            {
                writeln!(
                    out,
                    "    // {name} is derived from its setting and the theme"
                )
                .unwrap();
                continue;
            }
            writeln!(out, "    {name} {{").unwrap();
            render_chains(&mut out, &self.states[i], "        ");
            out.push_str("    }\n");
        }
        out.push_str("}\n\nkeys {\n");
        writeln!(
            out,
            "    prefix {}",
            quote(&self.bindings.prefix.canonical())
        )
        .unwrap();
        for binding in self.bindings.entries.iter().filter(|b| !b.direct) {
            writeln!(
                out,
                "    bind {} {}",
                quote(&binding.chord.canonical()),
                quote(&binding.action.name())
            )
            .unwrap();
        }
        let direct: Vec<_> = self.bindings.entries.iter().filter(|b| b.direct).collect();
        if !direct.is_empty() {
            out.push_str("    direct {\n");
            for binding in direct {
                writeln!(
                    out,
                    "        bind {} {}",
                    quote(&binding.chord.canonical()),
                    quote(&binding.action.name())
                )
                .unwrap();
            }
            out.push_str("    }\n");
        }
        out.push_str("}\n");
        bytes.extend_from_slice(out.as_bytes());
        bytes
    }
}

fn is_derived_unfocused(config: &Config) -> bool {
    let state = &config.states[8].content;
    config.states[8].chrome.is_empty()
        && state.len() == usize::from(config.number("dim_unfocused") != 0)
        && state.first().is_none_or(|sh| {
            sh.kind == Kind::Dim
                && sh.amount == config.number("dim_unfocused") as u8
                && sh.expression.is_none()
                && !sh.color.set
                && sh.param == 0
                && sh.channels == 0
        })
}
fn theme_name_ok(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() < 64
        && name != b"."
        && name != b".."
        && !name
            .iter()
            // Plain C char is signed on x86/macOS and unsigned on ARM Linux.
            .any(|&b| (b as libc::c_char) < 32 || matches!(b, b'/' | b'\\' | b':'))
}
/// Update the top-level theme selection while preserving the rest of the file.
pub fn write_theme_name(file: impl AsRef<Path>, name: impl AsRef<[u8]>) -> std::io::Result<()> {
    use std::{
        fs,
        io::{self, Write},
    };
    let name = name.as_ref().split(|b| *b == 0).next().unwrap_or_default();
    if !theme_name_ok(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a theme name: {}", String::from_utf8_lossy(name)),
        ));
    }
    let file = file.as_ref();
    let source = match fs::read(file) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    let mut text = Vec::new();
    // The C writer scans a NUL-terminated buffer, but uses the original file
    // length when deciding whether an appended selection needs a blank line.
    let mut remaining = source.split(|b| *b == 0).next().unwrap_or_default();
    let mut replaced = false;
    let selection = [b"theme_name \"", name, b"\"\n"].concat();
    while !remaining.is_empty() {
        let newline = remaining.iter().position(|b| *b == b'\n');
        let line = &remaining[..newline.unwrap_or(remaining.len())];
        let rest = line.strip_prefix(b"theme_name");
        if !replaced && rest.is_some_and(|s| matches!(s.first(), Some(b' ' | b'\t' | b'"'))) {
            text.extend_from_slice(&selection);
            replaced = true;
        } else {
            text.extend_from_slice(line);
            text.push(b'\n');
        }
        let Some(newline) = newline else { break };
        remaining = &remaining[newline + 1..];
    }
    if !replaced {
        if !source.is_empty() && source.last() != Some(&b'\n') {
            text.push(b'\n');
        }
        text.extend_from_slice(&selection);
    }
    path::mkdirs(path::dir_os(file))?;
    let mut temp = file.as_os_str().to_owned();
    temp.push(".new");
    let temp = PathBuf::from(temp);
    let result = (|| {
        let mut out = fs::File::create(&temp)?;
        out.write_all(&text)?;
        out.sync_all()?;
        fs::rename(&temp, file)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn render_chains(out: &mut String, chains: &Chains, indent: &str) {
    for (chrome, chain) in [(false, &chains.content), (true, &chains.chrome)] {
        for sh in chain {
            let amount = sh
                .expression
                .as_ref()
                .map_or_else(|| sh.amount.to_string(), |p| quote(p.borrow().source()));
            write!(out, "{indent}{} amount={amount}", sh.kind.name()).unwrap();
            if sh.color.set {
                write!(out, " color={}", quote(&sh.color.hex().unwrap())).unwrap();
            }
            if sh.param != 0 {
                write!(out, " at={}", sh.param).unwrap();
            }
            if chrome {
                out.push_str(" where=\"chrome\"");
            }
            if sh.channels == shader::FG {
                out.push_str(" channel=\"fg\"");
            }
            if sh.channels == shader::BG {
                out.push_str(" channel=\"bg\"");
            }
            out.push('\n');
        }
    }
}

type ParsedShader = (Option<(Shader, bool)>, Option<String>);
fn parse_shader(node: &Node, default_color: Color, default_chrome: bool) -> ParsedShader {
    let error = |message| (None, Some(message));
    if let Some(arg) = node.args.first() {
        return error(format!(
            "unexpected argument for {}: {arg} (separate entries with `;`)",
            node.name
        ));
    }
    let location = node.prop("where", if default_chrome { "chrome" } else { "content" });
    let chrome = match location {
        "chrome" => true,
        "content" => false,
        _ => {
            return error(format!(
                "bad where for {}: {location} (content or chrome)",
                node.name
            ));
        }
    };
    let channel = node.prop("channel", "both");
    let channels = match channel {
        "fg" => shader::FG,
        "bg" => shader::BG,
        "both" => shader::BOTH,
        _ => {
            return error(format!(
                "bad channel for {}: {channel} (fg, bg or both)",
                node.name
            ));
        }
    };
    let mut amount = 128;
    let mut expression = None;
    if let Some(text) = node.property("amount") {
        let trimmed = text.trim_matches([' ', '\t']);
        let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
        if trimmed.is_empty() || (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
        {
            amount = kdl::integer(trimmed).unwrap_or(0).clamp(0, 255) as u8;
        } else {
            match Program::compile(text) {
                Ok(program) => expression = Some(Rc::new(RefCell::new(program))),
                Err(why) => return error(format!("bad amount for {}: {why}", node.name)),
            }
        }
    }
    let param = ["at", "radius", "band", "direction"]
        .iter()
        .map(|name| node.prop_int(name, -1))
        .find(|v| *v >= 0)
        .unwrap_or(0)
        .min(65535) as u16;
    let color = node.property("color").and_then(parse_color);
    let Some(kind) = Kind::from_name(&node.name) else {
        return error(format!("unknown shader: {}", node.name));
    };
    let warning = node
        .property("color")
        .filter(|_| color.is_none())
        .map(|text| format!("bad colour for shader {}: {text}", node.name));
    (
        Some((
            Shader {
                kind,
                expression,
                color: color.unwrap_or(default_color),
                amount,
                param,
                channels,
            },
            chrome,
        )),
        warning,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "slush-config-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }
        fn write(&self, name: &str, text: &str) -> String {
            let file = self.0.join(name);
            std::fs::write(&file, text).unwrap();
            file.to_string_lossy().into_owned()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn default_dump_loads_without_pinning_derived_effects() {
        let fixture = Fixture::new();
        let config = Config::default();
        assert!(config.messages.is_empty());
        assert_eq!(config.color("tab_idle"), Color::rgb(0x8a, 0x8a, 0x95));
        assert_eq!(config.bindings.entries.len(), 63);
        let text = config.render();
        assert!(text.lines().count() > 100);
        let file = fixture.write("config.kdl", &text);
        let mut loaded = Config::default();
        assert!(loaded.load(&file));
        assert!(loaded.messages.is_empty(), "{:?}", loaded.messages);
        assert_eq!(loaded.settings, config.settings);
        assert_eq!(loaded.bindings.entries, config.bindings.entries);
        assert!(!loaded.bell_declared && !loaded.scrolled_declared);
        assert_eq!(loaded.states[5].chrome.len(), 1);
        assert_eq!(loaded.states[6].content.len(), 3);
        assert!(loaded.explicit_colors.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn native_configuration_values_round_trip_without_display_conversion() {
        let fixture = Fixture::new();
        let file = fixture.0.join("native.kdl");
        std::fs::write(&file, b"shell \"shell-\xff\"\neditor \"editor-\xfe 'arg'\"\nproject_layout \"layout-\xfd\"\nproject_roots \"projects-\xfc\" depth=3\ntheme_dir \"themes-\xfb\"\n").unwrap();
        let mut config = Config::default();
        assert!(config.load(&file));
        assert!(config.messages.is_empty(), "{:?}", config.messages);
        assert_eq!(
            native_bytes(config.native_text("editor").unwrap()).as_ref(),
            b"editor-\xfe 'arg'"
        );
        assert_eq!(
            native_bytes(config.project_roots[0].as_os_str()).as_ref(),
            b"projects-\xfc"
        );
        let rendered = fixture.0.join("rendered.kdl");
        std::fs::write(&rendered, config.render_bytes()).unwrap();
        let mut restored = Config::default();
        assert!(restored.load(&rendered));
        assert!(restored.messages.is_empty(), "{:?}", restored.messages);
        assert_eq!(restored.shell, config.shell);
        assert_eq!(restored.editor, config.editor);
        assert_eq!(restored.project_layout, config.project_layout);
        assert_eq!(restored.project_roots, config.project_roots);
        assert_eq!(restored.project_depth, config.project_depth);
        assert_eq!(restored.theme_dir, config.theme_dir);
    }

    #[test]
    fn includes_precede_theme_and_local_settings_regardless_of_line_order() {
        let fixture = Fixture::new();
        fixture.write("base.kdl","gap 3\nstates { bell {} ; scrolled {} ; unfocused {} }\ntheme { frame_focus \"#112233\" }\n");
        fixture.write(
            "custom.kdl",
            "gap 4\ntheme { frame_focus \"#334455\"; scroll_bg \"#ff0000\" }\n",
        );
        let file = fixture.write("config.kdl",&format!("gap 5\ntheme_name \"custom\"\ninclude \"base.kdl\"\ntheme_dir {}\ntheme {{ frame_focus \"#667788\" }}\n",quote(&fixture.0.to_string_lossy())));
        let mut config = Config::default();
        assert!(config.load(&file));
        assert!(config.messages.is_empty(), "{:?}", config.messages);
        assert_eq!(config.number("gap"), 5);
        assert_eq!(config.color("frame_focus"), Color::rgb(0x66, 0x77, 0x88));
        assert_eq!(config.theme_name.as_deref(), Some(b"custom".as_slice()));
        assert!(config.states[5].content.is_empty() && config.states[5].chrome.is_empty());
        assert!(config.states[6].content.is_empty());
        // Unfocused is deliberately derived anew per file; bell/scrolled declarations persist.
        assert_eq!(config.states[8].content.len(), 1);
        assert_eq!(config.files.len(), 3);
    }

    #[test]
    fn invalid_values_leave_prior_values_and_collect_diagnostics() {
        let fixture = Fixture::new();
        let file = fixture.write("bad.kdl","gap -1\nscrollback -2\npadding 1 2 3\ntab_bar_side \"down\"\nkeep_dead \"maybe\"\nmodal_scrim 999\nproject_roots depth=99\nkeys { prefix \"nope\"; bind \"?\" \"zoom\"; direct {bind \"x\" \"none\"} }\n");
        let mut config = Config::default();
        assert!(config.load(&file));
        assert_eq!(config.messages.len(), 7, "{:?}", config.messages);
        assert_eq!(config.number("gap"), 65535);
        assert_eq!(config.number("scrollback"), 10000);
        assert_eq!(config.number("modal_scrim"), 255);
        assert_eq!(config.text("tab_bar_side"), Some("top"));
        assert_eq!(config.project_depth, 2);
        let (chord, _) = Chord::parse("?").unwrap();
        assert_eq!(
            config.bindings.lookup(chord.key, SHIFT, false),
            Some(Action::Zoom)
        );
        assert_eq!(
            config.bindings.lookup(chord.key, 0, false),
            Some(Action::Zoom)
        );
    }

    #[test]
    fn shader_routing_empty_overrides_and_expression_strengths() {
        let fixture = Fixture::new();
        let file = fixture.write("config.kdl","shaders { dim amount=22; tint amount=\"focused * 255\" where=\"chrome\" channel=\"fg\"; bloom amount=200 }\nstates { unfocused {}; bell {} }\n");
        let mut config = Config::default();
        assert!(config.load(&file));
        assert_eq!(config.shaders.content.len(), 1);
        assert_eq!(config.shaders.chrome.len(), 1);
        assert!(config.states[8].content.is_empty());
        assert!(config.states[5].chrome.is_empty());
        assert_eq!(config.shaders.chrome[0].channels, shader::FG);
        assert_eq!(
            config.shaders.chrome[0]
                .expression
                .as_ref()
                .unwrap()
                .borrow()
                .deps(),
            crate::expr::DEP_FOCUS
        );
        assert!(config.messages[0].contains("unknown shader: bloom"));
        let mut reloaded = Config::default();
        assert!(reloaded.load(fixture.write("roundtrip.kdl", &config.render())));
        assert!(reloaded.messages.is_empty());
        assert!(reloaded.states[8].content.is_empty());
        assert!(reloaded.states[5].chrome.is_empty());
    }

    #[test]
    fn cycles_are_bounded_and_presets_route_only_shader_blocks() {
        let fixture = Fixture::new();
        let file = fixture.write("cycle.kdl", "include \"cycle.kdl\"\ngap 7\n");
        let mut config = Config::default();
        assert!(config.load(&file));
        assert_eq!(config.number("gap"), 7);
        assert_eq!(config.files.len(), 1);
        assert!(config.messages[0].contains("cycle"));
        config.messages.clear();
        let preset = fixture.write(
            "preset.kdl",
            "keys { prefix \"C-b\" }\nshaders {dim amount=2}\nshaders {tint where=\"chrome\"}\n",
        );
        let chains = config.parse_chain_file(&preset).unwrap();
        assert_eq!((chains.content.len(), chains.chrome.len()), (1, 1));
        assert_eq!(config.bindings.prefix.display(), "C-a");
        assert!(config.messages.is_empty());
    }

    #[test]
    fn selecting_a_theme_preserves_comments_and_refuses_paths() {
        let fixture = Fixture::new();
        let source = "// mine\ngap 3\ntheme_name \"old\" // selected\nkeys {\n    theme_name \"nested\"\n}\n";
        let file = fixture.write("config.kdl", source);
        write_theme_name(&file, "amber").unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            source.replace("theme_name \"old\" // selected", "theme_name \"amber\"")
        );
        assert!(write_theme_name(&file, "../theme").is_err());
        assert!(!std::path::Path::new(&format!("{file}.new")).exists());
    }

    #[test]
    fn theme_selection_preserves_native_bytes_and_c_line_boundaries() {
        let fixture = Fixture::new();
        let file = fixture.0.join("native-config.kdl");
        let cases: &[(&[u8], &[u8])] = &[
            (
                b"// native-\xff\r\neditor \"arg-\xfe\"\r\ntheme_name \"old\" // mine\r\n",
                b"// native-\xff\r\neditor \"arg-\xfe\"\r\ntheme_name \"amber\"\n",
            ),
            (b"gap 3", b"gap 3\n\ntheme_name \"amber\"\n"),
            (
                b"gap 3\ntheme_name \"old\"",
                b"gap 3\ntheme_name \"amber\"\n",
            ),
            (
                b"  theme_name \"old\"\n",
                b"  theme_name \"old\"\ntheme_name \"amber\"\n",
            ),
            (
                b"theme_name \"old\"\ntheme_name \"other\"\n",
                b"theme_name \"amber\"\ntheme_name \"other\"\n",
            ),
            (
                b"gap 3\n\0// ignored-\xff\n",
                b"gap 3\ntheme_name \"amber\"\n",
            ),
            (b"", b"theme_name \"amber\"\n"),
        ];
        for (source, expected) in cases {
            std::fs::write(&file, source).unwrap();
            write_theme_name(&file, "amber").unwrap();
            assert_eq!(std::fs::read(&file).unwrap(), *expected);
            assert!(!fixture.0.join("native-config.kdl.new").exists());
        }
        // The original writer inserts the accepted name literally, including
        // quotes, rather than adding KDL escapes that change its output.
        std::fs::write(&file, b"theme_name \"old\"\n").unwrap();
        write_theme_name(&file, "say\"hi").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"theme_name \"say\"hi\"\n");
    }
}
