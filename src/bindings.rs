//! Leader and direct shortcuts, using the terminal core's physical key names.
use crate::{
    ghostty::ffi::*,
    input::{ALT, CTRL, SHIFT, SUPER},
};

macro_rules! actions {
    ($($variant:ident => $name:literal),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Action { $($variant,)* SelectTab(u8) }
        impl Action {
            pub fn from_name(name: &str) -> Option<Self> {
                match name {
                    $($name => Some(Self::$variant),)*
                    name => name.strip_prefix("select-tab-")
                        .and_then(crate::kdl::integer).filter(|n| (1..=9).contains(n))
                        .map(|n| Self::SelectTab(n as u8)),
                }
            }
            pub fn name(self) -> String {
                match self { $(Self::$variant => $name.into(),)* Self::SelectTab(n) => format!("select-tab-{n}") }
            }
        }
    };
}
actions! {
    None => "none", Split => "split", SplitCols => "split-cols", SplitRows => "split-rows",
    ClosePane => "close-pane", Rerun => "rerun", Zoom => "zoom", Minimize => "minimize",
    Float => "float", NewFloat => "new-float", FloatGrow => "float-grow", FloatShrink => "float-shrink",
    SetPurpose => "set-purpose", RenamePane => "rename-pane", RenameTab => "rename-tab",
    RotateLayout => "rotate-layout", FocusLeft => "focus-left", FocusRight => "focus-right",
    FocusUp => "focus-up", FocusDown => "focus-down", FocusNext => "focus-next",
    NewTab => "new-tab", CloseTab => "close-tab", NextTab => "next-tab", PrevTab => "prev-tab",
    Finder => "finder", Palette => "palette", Workspaces => "workspaces", SaveWorkspace => "save-workspace",
    ScrollUp => "scroll-up", ScrollDown => "scroll-down", ScrollPageUp => "scroll-page-up",
    ScrollPageDown => "scroll-page-down", ScrollTop => "scroll-top", ScrollBottom => "scroll-bottom",
    Search => "search", ResizeLeft => "resize-left", ResizeRight => "resize-right",
    ResizeUp => "resize-up", ResizeDown => "resize-down", Equalize => "equalize",
    ClearShaders => "clear-shaders", PaneToNextTab => "pane-to-next-tab", PaneToPrevTab => "pane-to-prev-tab",
    PaneToNewTab => "pane-to-new-tab", Detach => "detach", Quit => "quit", Help => "help",
    EditConfig => "edit-config", Themes => "themes", LiteralPrefix => "literal-prefix",
}

const NAMED: &[(&str, GhosttyKey)] = &[
    ("left", GHOSTTY_KEY_ARROW_LEFT),
    ("right", GHOSTTY_KEY_ARROW_RIGHT),
    ("up", GHOSTTY_KEY_ARROW_UP),
    ("down", GHOSTTY_KEY_ARROW_DOWN),
    ("enter", GHOSTTY_KEY_ENTER),
    ("tab", GHOSTTY_KEY_TAB),
    ("escape", GHOSTTY_KEY_ESCAPE),
    ("space", GHOSTTY_KEY_SPACE),
    ("backspace", GHOSTTY_KEY_BACKSPACE),
    ("home", GHOSTTY_KEY_HOME),
    ("end", GHOSTTY_KEY_END),
    ("pageup", GHOSTTY_KEY_PAGE_UP),
    ("pagedown", GHOSTTY_KEY_PAGE_DOWN),
    ("delete", GHOSTTY_KEY_DELETE),
    ("insert", GHOSTTY_KEY_INSERT),
    ("backslash", GHOSTTY_KEY_BACKSLASH),
    ("minus", GHOSTTY_KEY_MINUS),
    ("slash", GHOSTTY_KEY_SLASH),
    ("comma", GHOSTTY_KEY_COMMA),
    ("period", GHOSTTY_KEY_PERIOD),
    ("f1", GHOSTTY_KEY_F1),
    ("f2", GHOSTTY_KEY_F2),
    ("f3", GHOSTTY_KEY_F3),
    ("f4", GHOSTTY_KEY_F4),
    ("f5", GHOSTTY_KEY_F5),
    ("f6", GHOSTTY_KEY_F6),
    ("f7", GHOSTTY_KEY_F7),
    ("f8", GHOSTTY_KEY_F8),
    ("f9", GHOSTTY_KEY_F9),
    ("f10", GHOSTTY_KEY_F10),
    ("f11", GHOSTTY_KEY_F11),
    ("f12", GHOSTTY_KEY_F12),
];
const PUNCT: &[(GhosttyKey, char, char)] = &[
    (GHOSTTY_KEY_SLASH, '/', '?'),
    (GHOSTTY_KEY_BACKSLASH, '\\', '|'),
    (GHOSTTY_KEY_MINUS, '-', '_'),
    (GHOSTTY_KEY_EQUAL, '=', '+'),
    (GHOSTTY_KEY_COMMA, ',', '<'),
    (GHOSTTY_KEY_PERIOD, '.', '>'),
    (GHOSTTY_KEY_SEMICOLON, ';', ':'),
    (GHOSTTY_KEY_QUOTE, '\'', '"'),
    (GHOSTTY_KEY_BRACKET_LEFT, '[', '{'),
    (GHOSTTY_KEY_BRACKET_RIGHT, ']', '}'),
    (GHOSTTY_KEY_BACKQUOTE, '`', '~'),
];
const ARROWS: &[(GhosttyKey, &str)] = &[
    (GHOSTTY_KEY_ARROW_LEFT, "←"),
    (GHOSTTY_KEY_ARROW_RIGHT, "→"),
    (GHOSTTY_KEY_ARROW_UP, "↑"),
    (GHOSTTY_KEY_ARROW_DOWN, "↓"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    pub key: GhosttyKey,
    pub mods: u16,
}
impl Chord {
    /// The boolean requests a second legacy punctuation binding without Shift.
    pub fn parse(text: &str) -> Option<(Self, bool)> {
        let mut rest = text;
        let mut mods = 0;
        while let Some((prefix, flag)) = [
            ("ctrl+", CTRL),
            ("alt+", ALT),
            ("shift+", SHIFT),
            ("super+", SUPER),
            ("C-", CTRL),
            ("M-", ALT),
            ("S-", SHIFT),
        ]
        .into_iter()
        .find(|(p, _)| rest.starts_with(p))
        {
            mods |= flag;
            rest = &rest[prefix.len()..];
        }
        if let Some((_, key)) = NAMED.iter().find(|(name, _)| *name == rest) {
            return Some((Self { key: *key, mods }, false));
        }
        if let Some((key, _)) = ARROWS.iter().find(|(_, glyph)| *glyph == rest) {
            return Some((Self { key: *key, mods }, false));
        }
        let bytes = rest.as_bytes();
        if bytes.len() != 1 {
            return None;
        }
        let c = bytes[0];
        let (key, shift, implied) = match c {
            b'a'..=b'z' => (GHOSTTY_KEY_A + (c - b'a') as i32, false, false),
            b'A'..=b'Z' => (GHOSTTY_KEY_A + (c - b'A') as i32, true, false),
            b'0'..=b'9' => (GHOSTTY_KEY_DIGIT_0 + (c - b'0') as i32, false, false),
            b' ' => (GHOSTTY_KEY_SPACE, false, false),
            _ => {
                let &(key, _, shifted) = PUNCT
                    .iter()
                    .find(|(_, p, s)| *p == c as char || *s == c as char)?;
                (key, shifted == c as char, shifted == c as char)
            }
        };
        Some((
            Self {
                key,
                mods: mods | if shift { SHIFT } else { 0 },
            },
            implied,
        ))
    }
    pub fn canonical(self) -> String {
        let base = NAMED
            .iter()
            .find(|(_, k)| *k == self.key)
            .map(|(n, _)| (*n).into())
            .or_else(|| {
                ((GHOSTTY_KEY_A..=GHOSTTY_KEY_Z).contains(&self.key))
                    .then(|| ((b'a' as i32 + self.key - GHOSTTY_KEY_A) as u8 as char).to_string())
            })
            .or_else(|| {
                ((GHOSTTY_KEY_DIGIT_0..=GHOSTTY_KEY_DIGIT_9).contains(&self.key)).then(|| {
                    ((b'0' as i32 + self.key - GHOSTTY_KEY_DIGIT_0) as u8 as char).to_string()
                })
            })
            .or_else(|| {
                PUNCT
                    .iter()
                    .find(|(k, _, _)| *k == self.key)
                    .map(|(_, p, _)| p.to_string())
            })
            .unwrap_or_else(|| "?".into());
        let mut out = String::new();
        for (flag, name) in [
            (CTRL, "ctrl+"),
            (ALT, "alt+"),
            (SHIFT, "shift+"),
            (SUPER, "super+"),
        ] {
            if self.mods & flag != 0 {
                out.push_str(name);
            }
        }
        out + &base
    }
    pub fn display(self) -> String {
        let mut mods = self.mods;
        let base = if let Some((_, plain, shifted)) = PUNCT.iter().find(|(k, _, _)| *k == self.key)
        {
            let c = if mods & SHIFT != 0 {
                mods &= !SHIFT;
                *shifted
            } else {
                *plain
            };
            c.to_string()
        } else if let Some((_, glyph)) = ARROWS.iter().find(|(k, _)| *k == self.key) {
            (*glyph).into()
        } else if (GHOSTTY_KEY_A..=GHOSTTY_KEY_Z).contains(&self.key) {
            let upper = mods & SHIFT != 0;
            mods &= !SHIFT;
            ((if upper { b'A' } else { b'a' } as i32 + self.key - GHOSTTY_KEY_A) as u8 as char)
                .to_string()
        } else {
            Self { mods: 0, ..self }.canonical()
        };
        format!(
            "{}{}{}{base}",
            if mods & CTRL != 0 { "C-" } else { "" },
            if mods & ALT != 0 { "M-" } else { "" },
            if mods & SHIFT != 0 { "S-" } else { "" }
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub chord: Chord,
    pub action: Action,
    pub direct: bool,
}
#[derive(Clone, Debug)]
pub struct Bindings {
    pub prefix: Chord,
    pub entries: Vec<Binding>,
}
impl Default for Bindings {
    fn default() -> Self {
        Self {
            prefix: Chord {
                key: GHOSTTY_KEY_A,
                mods: CTRL,
            },
            entries: Vec::new(),
        }
    }
}
impl Bindings {
    pub fn add(&mut self, chord: Chord, action: Action, direct: bool) {
        if let Some(old) = self
            .entries
            .iter_mut()
            .find(|b| b.chord == chord && b.direct == direct)
        {
            old.action = action;
        } else {
            self.entries.push(Binding {
                chord,
                action,
                direct,
            });
        }
    }
    pub fn lookup(&self, key: GhosttyKey, mods: u16, direct: bool) -> Option<Action> {
        let chord = Chord {
            key,
            mods: mods & (SHIFT | CTRL | ALT | SUPER),
        };
        self.entries
            .iter()
            .find(|b| b.chord == chord && b.direct == direct)
            .map(|b| b.action)
    }
    pub fn has_direct(&self) -> bool {
        self.entries
            .iter()
            .any(|b| b.direct && b.action != Action::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{CAPS, NUM};
    #[test]
    fn chords_round_trip_and_punctuation_gets_legacy_aliases() {
        for text in [
            "ctrl+a",
            "M-S-←",
            "?",
            "H",
            "super+f12",
            "shift+tab",
            "ctrl+\\",
        ] {
            let (chord, _) = Chord::parse(text).unwrap();
            assert_eq!(Chord::parse(&chord.canonical()).unwrap().0, chord);
        }
        assert!(Chord::parse("?").unwrap().1);
        assert!(!Chord::parse("H").unwrap().1);
        assert_eq!(Chord::parse("ctrl+b").unwrap().0.display(), "C-b");
        assert_eq!(Chord::parse("C-?").unwrap().0.display(), "C-?");
        assert!(Chord::parse("ctrl+nosuchkey").is_none());
    }
    #[test]
    fn direct_and_leader_bindings_are_independent_and_ignore_locks() {
        let mut bindings = Bindings::default();
        let chord = Chord::parse("x").unwrap().0;
        bindings.add(chord, Action::Zoom, false);
        bindings.add(chord, Action::None, true);
        bindings.add(chord, Action::ClosePane, false);
        assert_eq!(
            bindings.lookup(chord.key, CAPS | NUM, false),
            Some(Action::ClosePane)
        );
        assert_eq!(bindings.lookup(chord.key, 0, true), Some(Action::None));
        assert!(!bindings.has_direct());
        assert_eq!(
            Action::from_name("select-tab-9"),
            Some(Action::SelectTab(9))
        );
        assert_eq!(Action::from_name("select-tab-10"), None);
    }
}
