//! Binding-driven help and pickers sharing one modal surface and input policy.
use super::App;
use crate::{
    bindings::Action,
    ghostty::ffi::*,
    input::{CTRL, Event, SHIFT},
    layout::Id,
    screen::{self, BOLD, Color, Screen},
    shader::{self, Context, Kind, Rect, Shader},
};
const NONE: Color = Color {
    set: false,
    r: 0,
    g: 0,
    b: 0,
};
fn clip(text: &str, width: u16) -> String {
    let mut text = text.to_owned();
    while screen::cells(&text) > width {
        text.pop();
    }
    text
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Picker {
    Commands,
    Panes,
    Themes,
    Projects,
}
pub(super) struct Edit {
    pub target: Action,
    pub id: Id,
    pub text: String,
}
struct Description {
    id: u32,
    action: Action,
    group: String,
    label: String,
}
fn descriptions() -> Vec<Description> {
    crate::kdl::parse(include_str!("../../assets/actions.kdl"))
        .unwrap()
        .kids
        .into_iter()
        .map(|n| Description {
            id: n.prop_int("id", 0) as u32,
            action: Action::from_name(n.arg(0, "")).unwrap(),
            group: n.prop("group", "").into(),
            label: n.prop("label", "").into(),
        })
        .collect()
}
pub(super) struct Row {
    left: String,
    mid: String,
    right: String,
    here: bool,
    action: Vec<u8>,
}
struct HelpRow {
    heading: bool,
    chord: String,
    label: Option<String>,
}
impl App {
    pub(super) fn open_picker(&mut self, kind: Picker) {
        if kind == Picker::Projects {
            if self.config.project_roots.is_empty() {
                self.toast("no project roots: set project_roots in your config");
                return;
            }
            self.projects = self.projects();
            if self.projects.is_empty() {
                self.toast("no projects under your project_roots");
                return;
            }
        }
        if kind == Picker::Themes {
            self.theme_names = self.config.themes();
            if self.theme_names.is_empty() {
                self.toast(&format!(
                    "no themes in {}",
                    crate::path::display_dirs(&self.config.theme_dirs())
                ));
                return;
            }
            self.theme_was = self.config.theme_name.clone().unwrap_or_default();
        }
        self.picker = Some(kind);
        self.query.clear();
        self.selection = if kind == Picker::Themes {
            self.theme_names
                .iter()
                .position(|n| *n == self.theme_was)
                .unwrap_or(0)
        } else {
            0
        };
    }
    fn cancel_picker(&mut self) {
        if self.picker == Some(Picker::Themes) {
            let name = self.theme_was.clone();
            if let Err(e) = self.set_theme(&name, false) {
                self.toast(&e);
            }
        }
        self.picker = None;
    }
    fn preview_theme(&mut self) {
        if self.picker != Some(Picker::Themes) {
            return;
        }
        if let Some(name) = self
            .picker_rows()
            .get(self.selection)
            .and_then(|r| r.action.strip_prefix(b"theme:").map(<[u8]>::to_vec))
            && let Err(e) = self.set_theme(&name, false)
        {
            self.toast(&e);
        }
    }
    pub(super) fn begin_edit(&mut self, target: Action) {
        let Some(id) = self.layout.focus() else {
            return;
        };
        let id = if target == Action::RenameTab {
            self.layout.tabs[self.layout.current].id
        } else {
            id
        };
        self.begin_edit_id(target, id);
    }
    pub(super) fn begin_edit_id(&mut self, target: Action, id: Id) {
        let text = match target {
            Action::RenameTab => {
                if !self.config.boolean("status_bar") {
                    self.toast("no tab strip to rename in");
                    return;
                }
                let Some(index) = self.layout.tabs.iter().position(|t| t.id == id) else {
                    return;
                };
                self.select_tab(index);
                String::from_utf8_lossy(&self.layout.tabs[index].name).into_owned()
            }
            _ => {
                if !self.panes.contains_key(&id) {
                    return;
                }
                self.layout.focus_pane(id);
                self.sync_layout();
                if target == Action::SetPurpose {
                    self.layout.nodes[&id].purpose.clone()
                } else {
                    self.panes[&id].title().into()
                }
            }
        };
        self.name_click = None;
        self.drag = None;
        self.pressed = None;
        self.edit = Some(Edit { target, id, text });
    }
    fn commit_edit(&mut self) {
        let Some(edit) = self.edit.take() else { return };
        let done = match edit.target {
            Action::RenameTab => self
                .layout
                .tabs
                .iter_mut()
                .find(|t| t.id == edit.id)
                .map(|t| {
                    t.name = edit.text.as_bytes()[..edit.text.len().min(63)].to_vec();
                })
                .is_some(),
            Action::SetPurpose => self
                .layout
                .nodes
                .get_mut(&edit.id)
                .map(|n| {
                    n.purpose = super::commands::purpose(&edit.text);
                    n.purpose_locked = !n.purpose.is_empty();
                })
                .is_some(),
            _ => self
                .panes
                .get_mut(&edit.id)
                .map(|p| p.set_name(&edit.text))
                .is_some(),
        };
        if done {
            self.toast(if edit.target == Action::SetPurpose {
                if edit.text.is_empty() {
                    "purpose cleared"
                } else {
                    "purpose set"
                }
            } else if edit.text.is_empty() {
                "name cleared"
            } else {
                "renamed"
            });
        }
    }
    pub(super) fn ui_event(&mut self, event: &Event, s: &Screen) -> bool {
        if let Some(edit) = &self.edit
            && let Event::Mouse {
                action: 0, x, y, ..
            } = event
        {
            let own = format!(
                "{}:{}",
                if edit.target == Action::RenameTab {
                    "tab"
                } else {
                    "panetitle"
                },
                edit.id
            );
            if s.hits.test(*x, *y) != Some(own.as_str()) {
                self.commit_edit();
                // The click also reaches the thing the user left the field for.
                return false;
            }
        }
        if self.picker.is_none() && self.edit.is_none() {
            return false;
        }
        if let Some(edit) = &mut self.edit {
            let Event::Key {
                key,
                mods,
                action,
                text,
                ..
            } = event
            else {
                return true;
            };
            if *action == 2 {
                return true;
            }
            if *key == GHOSTTY_KEY_ESCAPE {
                self.edit = None;
                return true;
            }
            if *key == GHOSTTY_KEY_ENTER {
                self.commit_edit();
                return true;
            }
            if *mods & CTRL != 0 && *key == GHOSTTY_KEY_U {
                edit.text.clear();
            } else if *key == GHOSTTY_KEY_BACKSPACE {
                edit.text.pop();
            } else if *mods & CTRL == 0
                && text.first().is_some_and(|c| *c >= 32)
                && edit.text.len() + text.len() < 256
            {
                edit.text.push_str(&String::from_utf8_lossy(text));
            }
            return true;
        }
        match event {
            Event::Key {
                key,
                mods,
                action,
                text,
                ..
            } => {
                if *action == 2 {
                    return true;
                }
                if *key == GHOSTTY_KEY_ESCAPE {
                    self.cancel_picker();
                    return true;
                }
                if *key == GHOSTTY_KEY_ENTER {
                    let rows = self.picker_rows();
                    let action = rows.get(self.selection).map(|r| r.action.clone());
                    self.picker = None;
                    if let Some(action) = action {
                        self.pick(&action);
                    }
                    return true;
                }
                let ctrl = *mods & CTRL != 0;
                if ctrl && *key == GHOSTTY_KEY_S && self.picker == Some(Picker::Themes) {
                    let name = self.config.theme_name.clone().unwrap_or_default();
                    if !name.is_empty() {
                        self.picker = None;
                        match self.set_theme(&name, true) {
                            Ok(()) => self.toast(&format!(
                                "theme_name \"{}\" written to your config",
                                String::from_utf8_lossy(&name)
                            )),
                            Err(e) => self.toast(&e),
                        }
                    }
                    return true;
                }
                let step = match *key {
                    GHOSTTY_KEY_ARROW_DOWN => 1,
                    GHOSTTY_KEY_ARROW_UP => -1,
                    GHOSTTY_KEY_TAB => {
                        if *mods & SHIFT != 0 {
                            -1
                        } else {
                            1
                        }
                    }
                    GHOSTTY_KEY_PAGE_DOWN => 10,
                    GHOSTTY_KEY_PAGE_UP => -10,
                    GHOSTTY_KEY_N if ctrl => 1,
                    GHOSTTY_KEY_P if ctrl => -1,
                    _ => 0,
                };
                let n = self.picker_rows().len();
                if step != 0 && n > 0 {
                    let next = self.selection as i64 + step;
                    self.selection = if next < 0 {
                        if step == -1 { n - 1 } else { 0 }
                    } else if next >= n as i64 {
                        if step == 1 { 0 } else { n - 1 }
                    } else {
                        next as usize
                    };
                    self.preview_theme();
                } else if *key == GHOSTTY_KEY_HOME {
                    self.selection = 0;
                } else if *key == GHOSTTY_KEY_END {
                    self.selection = n.saturating_sub(1);
                } else if ctrl && *key == GHOSTTY_KEY_U {
                    self.query.clear();
                    self.selection = 0;
                    self.preview_theme();
                } else if *key == GHOSTTY_KEY_BACKSPACE {
                    self.query.pop();
                    self.selection = 0;
                    self.preview_theme();
                } else if !ctrl
                    && text.first().is_some_and(|c| *c >= 32)
                    && self.query.len() + text.len() < 128
                {
                    self.query.push_str(&String::from_utf8_lossy(text));
                    self.selection = 0;
                    self.preview_theme();
                }
            }
            Event::Mouse {
                button,
                action,
                x,
                y,
                ..
            } => {
                self.pointer = Some((*x, *y));
                if *action == 0
                    && self
                        .modal_rect
                        .is_some_and(|r| *x < r.x || *x >= r.x + r.w || *y < r.y || *y >= r.y + r.h)
                {
                    self.cancel_picker();
                    self.pressed = None;
                    return true;
                }
                let hit = s.hits.test_bytes(*x, *y).map(<[u8]>::to_vec);
                if *action == 0 && matches!(button, 4 | 5) {
                    let n = self.picker_rows().len();
                    if n > 0 {
                        self.selection = if *button == 4 {
                            self.selection.saturating_sub(1)
                        } else {
                            (self.selection + 1).min(n - 1)
                        };
                        self.preview_theme();
                    }
                } else if *action == 0 {
                    self.pressed = hit;
                } else if *action == 1 {
                    if let Some(hit) = hit
                        && self.pressed.as_ref() == Some(&hit)
                    {
                        if hit.starts_with(b"close") {
                            self.cancel_picker();
                        } else if hit.starts_with(b"run:")
                            || hit.starts_with(b"find:")
                            || hit.starts_with(b"theme:")
                            || hit.starts_with(b"open:")
                        {
                            self.picker = None;
                            self.pick(&hit);
                        }
                    }
                    self.pressed = None;
                }
            }
            _ => {}
        }
        true
    }
    pub(super) fn pick(&mut self, hit: impl AsRef<[u8]>) {
        let hit = hit.as_ref();
        if let Some(name) = hit.strip_prefix(b"theme:") {
            match self.set_theme(name, false) {
                Ok(()) => self.toast(&format!(
                    "{} · this session only: C-s in the picker keeps it",
                    String::from_utf8_lossy(name)
                )),
                Err(e) => self.toast(&e),
            }
            return;
        }
        let hit = std::str::from_utf8(hit).unwrap_or_default();
        if let Some(index) = hit
            .strip_prefix("open:")
            .and_then(|s| s.parse::<usize>().ok())
        {
            if let Some(project) = self.projects.get(index).cloned() {
                match self.open_workspace(&project.path, false) {
                    Ok(opened) => self.toast(if opened.created {
                        "opened"
                    } else {
                        "already open"
                    }),
                    Err(e) => self.toast(&e),
                }
            }
            return;
        }
        if let Some(id) = hit.strip_prefix("find:").and_then(|s| s.parse().ok()) {
            self.layout.focus_pane(id);
            self.sync_layout();
        }
        if let Some(id) = hit.strip_prefix("run:").and_then(|s| s.parse::<u32>().ok())
            && let Some(d) = descriptions().into_iter().find(|d| d.id == id)
        {
            self.action(d.action);
        }
    }
    fn picker_rows(&self) -> Vec<Row> {
        let query = self.query.to_ascii_lowercase();
        if self.picker == Some(Picker::Commands) {
            descriptions()
                .into_iter()
                .filter(|d| !matches!(d.action, Action::LiteralPrefix | Action::SelectTab(_)))
                .filter_map(|d| {
                    let mut chord = String::new();
                    for b in self
                        .config
                        .bindings
                        .entries
                        .iter()
                        .filter(|b| b.action == d.action)
                    {
                        if chord.len() >= 36 {
                            break;
                        }
                        if !chord.is_empty() {
                            chord.push_str(" · ");
                        }
                        if !b.direct {
                            chord.push_str(&self.config.bindings.prefix.display());
                            chord.push(' ');
                        }
                        chord.push_str(&b.chord.display());
                    }
                    let hay = format!("{} {} {} {}", d.label, d.group, d.action.name(), chord)
                        .to_ascii_lowercase();
                    hay.contains(&query).then(|| Row {
                        left: d.group,
                        mid: d.label,
                        right: chord,
                        here: false,
                        action: format!("run:{}", d.id).into_bytes(),
                    })
                })
                .take(128)
                .collect()
        } else if self.picker == Some(Picker::Projects) {
            self.projects
                .iter()
                .enumerate()
                .filter_map(|(i, p)| {
                    let marker = if p.layout.is_none() {
                        ".git"
                    } else {
                        crate::project::LAYOUT_FILE
                    };
                    if !format!(
                        "{} {} {marker}",
                        String::from_utf8_lossy(&p.name),
                        p.path.display()
                    )
                    .to_ascii_lowercase()
                    .contains(&query)
                    {
                        return None;
                    }
                    let tab = self.workspace_tab(&p.slug);
                    Some(Row {
                        left: p
                            .path
                            .parent()
                            .and_then(|p| p.file_name())
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        mid: String::from_utf8_lossy(&p.name).into_owned(),
                        right: if tab.is_some() {
                            String::from_utf8_lossy(&p.slug).into_owned()
                        } else if p.layout.is_none() {
                            ".git · no layout".into()
                        } else {
                            crate::project::LAYOUT_FILE.into()
                        },
                        here: tab.is_some_and(|id| {
                            self.layout
                                .tabs
                                .get(self.layout.current)
                                .is_some_and(|t| t.id == id)
                        }),
                        action: format!("open:{i}").into_bytes(),
                    })
                })
                .collect()
        } else if self.picker == Some(Picker::Themes) {
            self.theme_names
                .iter()
                .filter(|n| {
                    query.is_empty()
                        || n.windows(query.len())
                            .any(|part| part.eq_ignore_ascii_case(query.as_bytes()))
                })
                .map(|n| Row {
                    left: String::new(),
                    mid: String::from_utf8_lossy(n).into_owned(),
                    right: if *n == self.theme_was {
                        "in your config".into()
                    } else {
                        String::new()
                    },
                    here: self.config.theme_name.as_deref() == Some(n.as_slice()),
                    action: [b"theme:", n.as_slice()].concat(),
                })
                .collect()
        } else {
            self.layout
                .tabs
                .iter()
                .enumerate()
                .flat_map(|(i, t)| {
                    let query = &query;
                    self.layout
                        .leaves(t.root)
                        .into_iter()
                        .filter_map(move |id| {
                            let p = &self.panes[&id];
                            let n = &self.layout.nodes[&id];
                            let hay = format!(
                                "{} {} {}",
                                p.title(),
                                n.purpose,
                                String::from_utf8_lossy(&t.name)
                            )
                            .to_ascii_lowercase();
                            hay.contains(query).then(|| Row {
                                left: if t.name.is_empty() {
                                    format!("tab {}", i + 1)
                                } else {
                                    String::from_utf8_lossy(&t.name).into_owned()
                                },
                                mid: if p.title().is_empty() {
                                    "pane".into()
                                } else {
                                    p.title().into()
                                },
                                right: n.purpose.clone(),
                                here: self.layout.focus() == Some(id),
                                action: format!("find:{id}").into_bytes(),
                            })
                        })
                })
                .take(64)
                .collect()
        }
    }
    fn help_rows(&self) -> Vec<HelpRow> {
        let mut rows = Vec::new();
        for group in [
            "panes", "focus", "size", "tabs", "projects", "scroll", "session",
        ] {
            let mut titled = false;
            for d in descriptions().into_iter().filter(|d| d.group == group) {
                let chords = self
                    .config
                    .bindings
                    .entries
                    .iter()
                    .filter(|b| {
                        !b.direct
                            && (b.action == d.action
                                || matches!(
                                    (d.action, b.action),
                                    (Action::SelectTab(1), Action::SelectTab(9))
                                ))
                    })
                    .map(|b| b.chord.display())
                    .collect::<Vec<_>>()
                    .join(if matches!(d.action, Action::SelectTab(_)) {
                        "…"
                    } else {
                        " "
                    });
                if chords.is_empty() {
                    continue;
                }
                if !titled {
                    if !rows.is_empty() {
                        rows.push(HelpRow {
                            heading: true,
                            chord: String::new(),
                            label: None,
                        });
                    }
                    rows.push(HelpRow {
                        heading: true,
                        chord: String::new(),
                        label: Some(group.into()),
                    });
                    titled = true;
                }
                rows.push(HelpRow {
                    heading: false,
                    chord: chords,
                    label: Some(d.label),
                });
            }
        }
        if self.config.bindings.has_direct() {
            rows.push(HelpRow {
                heading: true,
                chord: String::new(),
                label: None,
            });
            rows.push(HelpRow {
                heading: true,
                chord: String::new(),
                label: Some("without the leader".into()),
            });
            for b in self.config.bindings.entries.iter().filter(|b| b.direct) {
                if let Some(d) = descriptions().into_iter().find(|d| d.action == b.action) {
                    rows.push(HelpRow {
                        heading: false,
                        chord: b.chord.display(),
                        label: Some(d.label),
                    });
                }
            }
        }
        rows.truncate(64);
        rows
    }
    fn modal(&mut self, s: &mut Screen, w: u16, h: u16, title: &str, close: &str) -> Rect {
        if self.config.number("modal_scrim") != 0 {
            shader::apply(
                s,
                &[Shader::new(
                    Kind::Dim,
                    NONE,
                    self.config.number("modal_scrim") as u8,
                )],
                Rect {
                    x: 0,
                    y: 0,
                    w: s.cols,
                    h: s.rows,
                },
                None,
                &Context {
                    default_fg: self.config.color("default_fg"),
                    default_bg: self.config.color("default_bg"),
                    ..Context::default()
                },
            );
        }
        let w = w.min(s.cols).max(2);
        let h = h.min(s.rows).max(2);
        let x = s.cols.saturating_sub(w) / 2;
        let y = s.rows.saturating_sub(h) / 2;
        self.modal_rect = Some(Rect { x, y, w, h });
        self.overlays.push(Rect { x, y, w, h });
        let x1 = x + w - 1;
        let y1 = y + h - 1;
        let bg = self.config.color("modal_bg");
        let fg = self.config.color("modal_border");
        for yy in y..=y1 {
            s.text(x, yy, &" ".repeat(w as usize), NONE, bg, 0);
        }
        let corners = if self.config.boolean("rounded") {
            ["╭", "╮", "╰", "╯"]
        } else {
            ["┌", "┐", "└", "┘"]
        };
        for ((xx, yy), c) in [(x, y), (x1, y), (x, y1), (x1, y1)]
            .into_iter()
            .zip(corners)
        {
            s.text(xx, yy, c, fg, bg, 0);
        }
        for xx in x + 1..x1 {
            s.text(xx, y, "─", fg, bg, 0);
            s.text(xx, y1, "─", fg, bg, 0);
        }
        for yy in y + 1..y1 {
            s.text(x, yy, "│", fg, bg, 0);
            s.text(x1, yy, "│", fg, bg, 0);
        }
        let label = format!(" {title} ");
        let tw = screen::cells(&label);
        if w > tw + 2 {
            s.text(
                x + (w - tw) / 2,
                y,
                &label,
                self.config.color("modal_title"),
                bg,
                BOLD,
            );
        }
        if w > 10 {
            let mark = self
                .config
                .text_bytes("close_mark")
                .unwrap_or("✕".as_bytes());
            let bw = screen::cells_bytes(mark) + 1;
            let bx = x1 - bw;
            let hot = self
                .pointer
                .is_some_and(|(px, py)| px >= bx && px < bx + bw && py == y);
            s.text_bytes(
                bx,
                y,
                &[mark, b" "].concat(),
                self.config.color(if hot {
                    "modal_button_hover"
                } else {
                    "modal_button"
                }),
                bg,
                if hot { BOLD } else { 0 },
            );
            s.hits.add(bx, y, bw, 1, close);
        }
        Rect {
            x: x + 1,
            y: y + 1,
            w: w - 2,
            h: h - 2,
        }
    }
    pub(super) fn draw_help(&mut self, s: &mut Screen) {
        let rows = self.help_rows();
        let n = rows.len();
        if n == 0 {
            return;
        }
        let two = s.cols >= 78;
        let mut split = n;
        if two {
            let half = n.div_ceil(2);
            split = half;
            while split < n && !rows[split].heading {
                split += 1;
            }
            if split >= n {
                split = half;
                while split > 1 && !rows[split].heading {
                    split -= 1;
                }
            }
            if split < n && rows[split].label.is_none() {
                split += 1;
            }
        }
        let mut kw = [0u16; 2];
        let mut cw = [0u16; 2];
        for (i, row) in rows.iter().enumerate() {
            let c = usize::from(two && i >= split);
            if !row.heading {
                kw[c] = kw[c].max(screen::cells(&row.chord));
            }
        }
        for (i, row) in rows.iter().enumerate() {
            if let Some(label) = &row.label {
                let c = usize::from(two && i >= split);
                let w = if row.heading {
                    screen::cells(label)
                } else {
                    kw[c] + 2 + screen::cells(label)
                };
                cw[c] = cw[c].max(w);
            }
        }
        let left_n = if split > 0 && rows[split - 1].label.is_none() {
            split - 1
        } else {
            split
        };
        let mut right_n = if two { n - split } else { 0 };
        if right_n > 0 && rows[n - 1].label.is_none() {
            right_n -= 1;
        }
        let lines = left_n.max(right_n);
        let shown = lines.min(s.rows.saturating_sub(7).max(1) as usize);
        let hidden = rows
            .iter()
            .enumerate()
            .filter(|(i, r)| {
                r.label.is_some() && if two && *i >= split { *i - split } else { *i } >= shown
            })
            .count();
        let r = self.modal(
            s,
            cw[0] + if two { cw[1] + 4 } else { 0 } + 6,
            shown as u16 + 6 + u16::from(hidden > 0),
            "keys",
            "closehelp",
        );
        let bg = self.config.color("modal_bg");
        s.text(
            r.x + 2,
            r.y + 1,
            &clip(
                &format!("{} then:", self.config.bindings.prefix.display()),
                r.w.saturating_sub(2),
            ),
            self.config.color("hint"),
            bg,
            0,
        );
        for (i, row) in rows.iter().enumerate() {
            let right = two && i >= split;
            let ry = if right { i - split } else { i };
            if ry >= shown || r.y + 3 + ry as u16 >= r.y + r.h {
                continue;
            }
            let Some(label) = &row.label else {
                continue;
            };
            let x = r.x + 2 + if right { cw[0] + 4 } else { 0 };
            let y = r.y + 3 + ry as u16;
            let mut room = (r.x + r.w).saturating_sub(x);
            if two && !right {
                room = room.min(cw[0] + 2);
            }
            if row.heading {
                s.text(
                    x,
                    y,
                    &clip(label, room),
                    self.config.color("modal_title"),
                    bg,
                    BOLD,
                );
            } else {
                s.text(
                    x,
                    y,
                    &clip(&row.chord, room),
                    self.config.color("modal_title"),
                    bg,
                    0,
                );
                let lx = x + kw[usize::from(right)] + 2;
                s.text(
                    lx,
                    y,
                    &clip(label, (r.x + r.w).saturating_sub(lx)),
                    self.config.color("modal_fg"),
                    bg,
                    0,
                );
            }
        }
        if hidden > 0 {
            s.text(
                r.x + 2,
                r.y + 3 + shown as u16,
                &clip(
                    &format!("+{hidden} more, in docs/keys.md"),
                    r.w.saturating_sub(2),
                ),
                self.config.color("hint"),
                bg,
                0,
            );
        }
        let foot = " any key closes this ";
        let fw = screen::cells(foot);
        if r.w > fw {
            s.text(
                r.x + (r.w - fw) / 2,
                r.y + r.h,
                foot,
                self.config.color("hint"),
                bg,
                0,
            );
        }
    }
    pub(super) fn draw_picker(&mut self, s: &mut Screen) {
        let rows = self.picker_rows();
        let n = rows.len();
        self.selection = self.selection.min(n.saturating_sub(1));
        let shown = n.min(10);
        let first = (self.selection + 1).saturating_sub(shown);
        let palette = self.picker == Some(Picker::Commands);
        let themes = self.picker == Some(Picker::Themes);
        let projects = self.picker == Some(Picker::Projects);
        let w = 64.min(if s.cols > 24 { s.cols - 4 } else { s.cols });
        let r = self.modal(
            s,
            w,
            shown.max(1) as u16 + 5,
            if palette {
                "commands"
            } else if themes {
                "themes"
            } else if projects {
                "projects"
            } else {
                "find"
            },
            if palette {
                "closepalette"
            } else if themes {
                "closethemes"
            } else if projects {
                "closeprojects"
            } else {
                "closefind"
            },
        );
        let bg = self.config.color("finder_bg");
        let fy = r.y + 1;
        s.text(r.x, fy, &" ".repeat(r.w as usize), NONE, bg, 0);
        let q = format!("› {}█", self.query);
        s.text(
            r.x + 1,
            fy,
            &clip(&q, r.w.saturating_sub(1)),
            self.config.color("finder_fg"),
            bg,
            BOLD,
        );
        let ly = r.y + 3;
        if n == 0 {
            s.text(
                r.x + 2,
                ly,
                &clip(
                    if palette {
                        "no command matches that"
                    } else if themes {
                        "no theme matches that"
                    } else if projects {
                        "no project matches that"
                    } else {
                        "no pane matches that"
                    },
                    r.w.saturating_sub(2),
                ),
                self.config.color("hint"),
                self.config.color("modal_bg"),
                0,
            );
            return;
        }
        for (i, row) in rows.iter().skip(first).take(shown).enumerate() {
            let y = ly + i as u16;
            if y >= r.y + r.h {
                break;
            }
            s.hits.add_bytes(r.x, y, r.w, 1, &row.action);
            let selected = first + i == self.selection;
            let hot = self
                .pointer
                .is_some_and(|(px, py)| px >= r.x && px < r.x + r.w && py == y);
            let bg = self.config.color(if selected {
                "finder_sel_bg"
            } else if hot {
                "finder_bg"
            } else {
                "modal_bg"
            });
            let fg = self.config.color(if selected {
                "finder_sel_fg"
            } else {
                "finder_fg"
            });
            let quiet = if selected {
                fg
            } else {
                self.config.color("hint")
            };
            s.text(r.x, y, &" ".repeat(r.w as usize), NONE, bg, 0);
            s.text(r.x + 1, y, if row.here { "•" } else { " " }, fg, bg, 0);
            let mut cx = r.x + 3;
            if r.w > 40 && !themes {
                s.text(cx, y, &clip(&row.left, 13), quiet, bg, 0);
                cx += 13;
            }
            let room = (r.x + r.w).saturating_sub(cx + 1);
            let mw = if room > 20 {
                if r.w > 56 { 26 } else { 18 }
            } else {
                room
            };
            let drawn = s.text(cx, y, &clip(&row.mid, mw), fg, bg, BOLD);
            cx += drawn.max(mw) + 1;
            if cx < r.x + r.w {
                s.text(cx, y, &clip(&row.right, r.x + r.w - cx - 1), quiet, bg, 0);
            }
        }
        if first > 0 {
            s.text(
                r.x + r.w - 1,
                ly,
                "↑",
                self.config.color("hint"),
                self.config.color("modal_bg"),
                0,
            );
        }
        if first + shown < n {
            s.text(
                r.x + r.w - 1,
                ly + shown as u16 - 1,
                "↓",
                self.config.color("hint"),
                self.config.color("modal_bg"),
                0,
            );
        }
        let foot = if themes {
            " ↑↓ · C-s keeps it · esc undoes ".into()
        } else {
            format!(" {} of {n} · ↑↓ enter ", self.selection + 1)
        };
        let fw = screen::cells(&foot);
        if r.w > fw {
            s.text(
                r.x + (r.w - fw) / 2,
                r.y + r.h,
                &foot,
                self.config.color("hint"),
                self.config.color("modal_bg"),
                0,
            );
        }
    }
}
