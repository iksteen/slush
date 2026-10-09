//! Session model shared by headless and interactive front ends.
mod commands;
mod corner;
mod draw;
mod editor;
mod floating;
mod gesture;
mod graphics;
mod guide;
mod moving;
mod osc;
mod search;
mod selection;
mod session;
mod sidebar;
mod splash;
mod ui;
mod workspace;
use crate::{
    bindings::{Action, Chord},
    config::{Chains, Config},
    input::Event,
    json_control::Reply,
    layout::{Direction, Id, Layout},
    pane::Pane,
    pty::Size,
    screen::Screen,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io,
    time::{Duration, Instant},
};

pub struct App {
    pub config: Config,
    pub layout: Layout,
    pub panes: BTreeMap<Id, Pane>,
    pub cols: u16,
    pub rows: u16,
    pub cell_w: u16,
    pub cell_h: u16,
    pub prefix: bool,
    pub quit: bool,
    pub detach: bool,
    pub help: bool,
    picker: Option<ui::Picker>,
    theme_override: Vec<u8>,
    theme_was: Vec<u8>,
    theme_names: Vec<Vec<u8>>,
    projects: Vec<crate::project::Project>,
    force_suspend: bool,
    query: String,
    selection: usize,
    edit: Option<ui::Edit>,
    search: Option<search::Search>,
    select_drag: Option<selection::Drag>,
    drag: Option<gesture::Drag>,
    corners: Vec<corner::Corner>,
    splash: Option<splash::Splash>,
    splash_chains: Vec<Vec<crate::shader::Shader>>,
    cell_click: Option<(Id, u16, u16, Instant)>,
    name_click: Option<(Action, Id, Instant)>,
    pub clipboard: String,
    pub clipboard_pending: Option<String>,
    modal_rect: Option<crate::shader::Rect>,
    pub chains: BTreeMap<Id, Chains>,
    pub session: Option<OsString>,
    argv: Vec<OsString>,
    pub toasts: Vec<(String, Instant)>,
    pub pointer: Option<(u16, u16)>,
    pointer_since: Instant,
    hover_pending: bool,
    pressed: Option<Vec<u8>>,
    started: Instant,
    states: BTreeMap<Id, (Option<usize>, Instant)>,
    sidebar_spins: bool,
    graphics: crate::graphics::Graphics,
    graphics_requests: Vec<crate::graphics::Request>,
    overlays: Vec<crate::shader::Rect>,
}
impl App {
    pub fn new(argv: Vec<OsString>, cols: u16, rows: u16) -> io::Result<Self> {
        Self::with_session(argv, cols, rows, None)
    }
    pub fn with_session(
        argv: Vec<OsString>,
        cols: u16,
        rows: u16,
        session: Option<OsString>,
    ) -> io::Result<Self> {
        let mut config = Config::default();
        let file = Config::default_path();
        config.load(&file);
        if !std::path::Path::new(&file).exists() {
            config.messages.clear();
        }
        if let Some(message) = config.messages.first() {
            eprintln!("slush: {}: {message}", file.display());
        }
        let mut app = Self {
            config,
            layout: Layout::default(),
            panes: BTreeMap::new(),
            cols,
            rows,
            cell_w: 8,
            cell_h: 16,
            prefix: false,
            quit: false,
            detach: false,
            help: false,
            picker: None,
            theme_override: Vec::new(),
            theme_was: Vec::new(),
            theme_names: Vec::new(),
            projects: Vec::new(),
            force_suspend: false,
            query: String::new(),
            selection: 0,
            edit: None,
            search: None,
            select_drag: None,
            drag: None,
            corners: Vec::new(),
            splash: None,
            splash_chains: Vec::new(),
            cell_click: None,
            name_click: None,
            clipboard: String::new(),
            clipboard_pending: None,
            modal_rect: None,
            chains: BTreeMap::new(),
            session,
            argv,
            toasts: Vec::new(),
            pointer: None,
            pointer_since: Instant::now(),
            hover_pending: false,
            pressed: None,
            started: Instant::now(),
            states: BTreeMap::new(),
            sidebar_spins: false,
            graphics: crate::graphics::Graphics::default(),
            graphics_requests: Vec::new(),
            overlays: Vec::new(),
        };
        let id = app.layout.leaf();
        app.layout.add_tab(id, "");
        app.sync_layout();
        let content = app.layout.nodes[&id].content;
        let pane = Pane::new(
            id,
            app.default_argv(),
            app.size(content.w, content.h),
            None,
            false,
            "",
            app.session.as_deref(),
            &app.config,
        )?;
        app.panes.insert(id, pane);
        if let Some(message) = app.config.messages.first().cloned() {
            app.toast(&message);
        }
        Ok(app)
    }
    fn size(&self, cols: u16, rows: u16) -> Size {
        Size {
            cols: cols.max(1),
            rows: rows.max(1),
            cell_width: self.cell_w,
            cell_height: self.cell_h,
        }
    }
    pub fn default_argv(&self) -> Vec<OsString> {
        if !self.argv.is_empty() {
            self.argv.clone()
        } else {
            vec![
                self.config
                    .shell
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(OsStr::to_owned)
                    .unwrap_or_else(crate::shell::default_program),
            ]
        }
    }
    pub fn resolve(&self, id: Id) -> Option<Id> {
        if id == 0 {
            self.layout.focus()
        } else {
            self.panes.contains_key(&id).then_some(id)
        }
    }
    pub fn should_quit(&self) -> bool {
        self.quit || self.layout.tabs.is_empty()
    }
    pub fn sync_layout(&mut self) {
        if self.edit.as_ref().is_some_and(|e| {
            if e.target == Action::RenameTab {
                !self.layout.tabs.iter().any(|t| t.id == e.id)
            } else {
                !self.panes.contains_key(&e.id)
            }
        }) {
            self.edit = None;
        }
        self.sync_tab(self.layout.current);
    }
    fn sync_tab(&mut self, ti: usize) {
        for (id, content) in self.layout.arrange(ti, self.cols, self.rows, &self.config) {
            let size = self.size(content.w, content.h);
            if let Some(pane) = self.panes.get_mut(&id) {
                let _ = pane.resize(size);
            }
        }
    }
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.sync_layout();
    }
    pub fn cell_size(&mut self, w: u16, h: u16) {
        if w == 0 || h == 0 {
            return;
        }
        self.cell_w = w;
        self.cell_h = h;
        for pane in self.panes.values_mut() {
            let _ = pane.resize(Size {
                cell_width: w,
                cell_height: h,
                ..pane.size
            });
        }
    }
    pub fn split(&mut self, id: Id, direction: Direction, before: bool) -> io::Result<Option<Id>> {
        self.split_with(id, direction, before, self.default_argv(), "")
    }
    fn split_with(
        &mut self,
        id: Id,
        direction: Direction,
        before: bool,
        argv: Vec<OsString>,
        label: impl AsRef<[u8]>,
    ) -> io::Result<Option<Id>> {
        let Some(id) = self.resolve(id) else {
            return Ok(None);
        };
        if self.layout.nodes[&id].floating {
            return Ok(None);
        }
        let cwd = self.panes[&id].live_cwd();
        let ti = self.layout.tab_of(id).unwrap();
        let fresh = self.layout.leaf();
        self.layout.insert_beside(id, fresh, direction, before);
        self.layout.current = ti;
        self.layout.tabs[ti].focus = fresh;
        self.sync_layout();
        let content = self.layout.nodes[&fresh].content;
        match Pane::new(
            fresh,
            argv,
            self.size(content.w, content.h),
            cwd.as_deref(),
            false,
            label,
            self.session.as_deref(),
            &self.config,
        ) {
            Ok(pane) => {
                self.panes.insert(fresh, pane);
                Ok(Some(fresh))
            }
            Err(error) => {
                self.layout.remove(fresh);
                self.sync_layout();
                Err(error)
            }
        }
    }
    pub fn close(&mut self, id: Id) -> bool {
        let Some(id) = self.resolve(id) else {
            return false;
        };
        for leaf in self.layout.remove(id) {
            self.panes.remove(&leaf);
            self.chains.remove(&leaf);
            self.states.remove(&leaf);
            self.graphics.forget_pane(leaf);
        }
        self.sync_layout();
        true
    }
    pub fn new_float(&mut self) -> io::Result<Option<Id>> {
        let Some(beside) = self.layout.focus() else {
            return Ok(None);
        };
        let cwd = self.panes[&beside].live_cwd();
        let r = self.layout.nodes[&beside].rect;
        let dir = if r.w as u64 >= r.h as u64 * self.config.number("gap_aspect").max(1) {
            Direction::Cols
        } else {
            Direction::Rows
        };
        let fresh = self.layout.leaf();
        self.layout.insert_beside(beside, fresh, dir, false);
        // A float can be opened beside another float: the new leaf belongs to
        // that tree slot, while its geometry comes from its overlay rectangle.
        if self.layout.tab_of(fresh).is_none() {
            let old_float = self.layout.nodes[&beside].floating;
            self.layout.nodes.get_mut(&beside).unwrap().floating = false;
            self.layout.insert_beside(beside, fresh, dir, false);
            self.layout.nodes.get_mut(&beside).unwrap().floating = old_float;
        }
        self.layout.raise_seq += 1;
        let n = self.layout.nodes.get_mut(&fresh).unwrap();
        n.floating = true;
        n.raised = self.layout.raise_seq;
        self.layout.tabs[self.layout.current].focus = fresh;
        self.sync_layout();
        let r = self.layout.nodes[&fresh].content;
        match Pane::new(
            fresh,
            self.default_argv(),
            self.size(r.w, r.h),
            cwd.as_deref(),
            false,
            "",
            self.session.as_deref(),
            &self.config,
        ) {
            Ok(p) => {
                self.panes.insert(fresh, p);
                Ok(Some(fresh))
            }
            Err(e) => {
                self.layout.remove(fresh);
                self.sync_layout();
                Err(e)
            }
        }
    }
    pub fn new_tab(&mut self, name: impl AsRef<[u8]>) -> io::Result<Id> {
        let root = self.layout.leaf();
        let tab = self.layout.add_tab(root, name);
        self.layout.current = self.layout.tabs.len() - 1;
        self.sync_layout();
        let r = self.layout.nodes[&root].content;
        match Pane::new(
            root,
            self.default_argv(),
            self.size(r.w, r.h),
            None,
            false,
            "",
            self.session.as_deref(),
            &self.config,
        ) {
            Ok(pane) => {
                self.panes.insert(root, pane);
                Ok(tab)
            }
            Err(error) => {
                self.layout.remove(root);
                self.sync_layout();
                Err(error)
            }
        }
    }
    pub fn select_tab(&mut self, index: usize) -> bool {
        if index >= self.layout.tabs.len() {
            return false;
        }
        if let Some(tab) = self.layout.tabs.get(self.layout.current) {
            for id in self.layout.leaves(tab.root) {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.terminal.drop_render_cache();
                }
            }
        }
        self.layout.current = index;
        self.prefix = false;
        self.sync_layout();
        true
    }
    pub fn cycle_tab(&mut self, delta: i32) {
        if !self.layout.tabs.is_empty() {
            let index = (self.layout.current as i32 + delta)
                .rem_euclid(self.layout.tabs.len() as i32) as usize;
            self.select_tab(index);
        }
    }
    pub fn move_tab(&mut self, id: Id, to: usize) -> bool {
        let Some(from) = self.layout.tabs.iter().position(|t| t.id == id) else {
            return false;
        };
        if to >= self.layout.tabs.len() {
            return false;
        }
        let current = self.layout.tabs[self.layout.current].id;
        let tab = self.layout.tabs.remove(from);
        self.layout.tabs.insert(to, tab);
        self.layout.current = self
            .layout
            .tabs
            .iter()
            .position(|t| t.id == current)
            .unwrap();
        true
    }
    pub fn close_tab(&mut self, id: Id) -> bool {
        let tab = if id == 0 {
            self.layout.tabs.get(self.layout.current)
        } else {
            self.layout.tabs.iter().find(|t| t.id == id)
        };
        let Some(root) = tab.map(|t| t.root) else {
            return false;
        };
        for leaf in self.layout.remove(root) {
            self.panes.remove(&leaf);
            self.chains.remove(&leaf);
            self.states.remove(&leaf);
        }
        self.sync_layout();
        true
    }
    pub fn write(&mut self, id: Id, data: &[u8]) -> bool {
        if let Some(id) = self.resolve(id) {
            let _ = self.panes.get_mut(&id).unwrap().write(data);
            true
        } else {
            false
        }
    }
    pub fn reap(&mut self) {
        let dead: Vec<_> = self
            .panes
            .iter()
            .filter(|(_, p)| {
                !p.alive
                    && !p.suspended
                    && (p.ephemeral
                        || match self.config.text("keep_dead") {
                            Some("all") => false,
                            Some("commands") => p.label.is_empty(),
                            _ => true,
                        })
            })
            .map(|(id, _)| *id)
            .collect();
        for id in dead {
            self.close(id);
        }
    }
    pub fn settle(&mut self, quiet_ms: u64, mut snapshot: Option<&mut Screen>) {
        let mut last = Instant::now();
        let deadline = last + Duration::from_secs(3);
        while !self.should_quit() && Instant::now() < deadline {
            let mut fds: Vec<_> = self
                .panes
                .values()
                .filter_map(|p| p.pty.as_ref())
                .map(|p| crate::poller::PollFd {
                    fd: p.fd(),
                    events: crate::poller::READ
                        | if p.wants_write() {
                            crate::poller::WRITE
                        } else {
                            0
                        },
                    revents: 0,
                })
                .collect();
            if fds.is_empty() {
                break;
            }
            if crate::poller::poll(&mut fds, 10).is_ok_and(|n| n > 0) {
                for fd in fds.iter().filter(|fd| fd.revents != 0) {
                    let _ = self.pump_fd(fd.fd);
                    last = Instant::now();
                }
            }
            self.handle_pane_osc();
            self.selection_tick();
            if let Some(screen) = snapshot.as_deref_mut() {
                self.compose(screen);
            }
            self.reap();
            self.toasts.retain(|(_, until)| *until > Instant::now());
            if last.elapsed() >= Duration::from_millis(quiet_ms) {
                break;
            }
        }
    }
    pub fn pump_fd(&mut self, fd: crate::poller::Descriptor) -> io::Result<usize> {
        let Some(pane) = self
            .panes
            .values_mut()
            .find(|p| p.pty.as_ref().is_some_and(|p| p.fd() == fd))
        else {
            return Ok(0);
        };
        let was_alive = pane.alive;
        let n = pane.pump()?;
        if was_alive && !pane.alive {
            let words = pane.exit_words();
            let note = if pane.label.is_empty() {
                format!("[process {words}]").into_bytes()
            } else {
                let mut bytes = b"[ran: ".to_vec();
                bytes.extend_from_slice(&pane.label);
                bytes.extend_from_slice(format!("]\r\n[process {words}]").as_bytes());
                bytes
            };
            pane.note(&note, self.config.color("dead"));
        }
        let clipboard = pane.terminal.take_clipboard();
        let notifications = pane.terminal.take_notifications();
        if let Some(text) = clipboard {
            self.set_clipboard(text);
        }
        for text in notifications {
            self.toast(&text);
        }
        self.handle_pane_osc();
        Ok(n)
    }
    pub fn cancel_client_pointer(&mut self) {
        self.pointer = None;
        self.hover_pending = false;
        self.pressed = None;
        self.select_drag = None;
        self.cell_click = None;
        self.name_click = None;
        self.drag = None;
    }
    pub fn cancel_client_interaction(&mut self) {
        self.prefix = false;
        self.cancel_client_pointer();
    }
    pub fn toast(&mut self, text: &str) {
        self.toasts.push((
            text.into(),
            Instant::now() + Duration::from_millis(self.config.number("toast_ms")),
        ));
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
    }
    pub fn event(&mut self, event: &Event, screen: &Screen) {
        if self.should_quit() {
            return;
        }
        if matches!(event, Event::Key { .. } | Event::Mouse { action: 0, .. }) {
            self.splash = None;
        }
        if let Event::Mouse { x, y, .. } = event {
            if self.pointer != Some((*x, *y)) {
                self.pointer_since = Instant::now();
            }
            self.pointer = Some((*x, *y));
        }
        if self.search_event(event, screen) {
            return;
        }
        if self.ui_event(event, screen) {
            return;
        }
        if self.gesture_event(event, screen) {
            return;
        }
        if self.selection_event(event, screen) {
            return;
        }
        match event {
            Event::Key {
                key, mods, action, ..
            } => {
                if *action == 2 {
                    if !self.prefix {
                        self.forward(event);
                    }
                    return;
                }
                if self.help {
                    self.help = false;
                    return;
                }
                let chord = Chord {
                    key: *key,
                    mods: *mods
                        & (crate::input::SHIFT
                            | crate::input::CTRL
                            | crate::input::ALT
                            | crate::input::SUPER),
                };
                if chord == self.config.bindings.prefix {
                    if self.prefix {
                        self.prefix = false;
                        self.forward(event);
                    } else {
                        self.prefix = true;
                    }
                    return;
                }
                if self.prefix {
                    self.prefix = false;
                    if let Some(action) = self
                        .config
                        .bindings
                        .lookup(*key, *mods, false)
                        .or_else(|| self.config.bindings.lookup(*key, *mods, true))
                    {
                        self.action(action);
                    }
                    return;
                }
                if let Some(action) = self
                    .config
                    .bindings
                    .lookup(*key, *mods, true)
                    .filter(|a| *a != Action::None)
                {
                    self.action(action);
                    return;
                }
                if let Some(id) = self.layout.focus()
                    && self.panes[&id].suspended
                {
                    let _ = self
                        .panes
                        .get_mut(&id)
                        .unwrap()
                        .start(self.session.as_deref());
                    return;
                }
                if self.ctrl_d_exit(event) {
                    return;
                }
                self.forward(event);
            }
            Event::Mouse { action, x, y, .. } => {
                self.pointer = Some((*x, *y));
                let hit = screen.hits.test(*x, *y).map(str::to_owned);
                if *action == 0 {
                    self.pressed = hit.as_ref().map(|h| h.as_bytes().to_vec());
                    if self.help {
                        self.help = false;
                        return;
                    }
                }
                if let Some(hit) = hit {
                    let mut parts = hit.split(':');
                    let verb = parts.next().unwrap_or("");
                    let id = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
                    if *action == 2
                        && self.drag.is_none()
                        && !self.prefix
                        && !self.help
                        && self.config.boolean("focus_follows_mouse")
                        && matches!(verb, "pane" | "focus" | "title" | "panetitle" | "header")
                        && self
                            .layout
                            .nodes
                            .get(&id)
                            .is_some_and(|n| !n.hidden && !n.minimized)
                    {
                        self.layout.focus_pane(id);
                        self.sync_layout();
                    }
                    if *action == 0
                        && matches!(verb, "pane" | "focus" | "title" | "panetitle" | "header")
                    {
                        self.layout.focus_pane(id);
                        self.sync_layout();
                    }
                    if *action == 1 && self.pressed.as_deref() == Some(hit.as_bytes()) {
                        match verb {
                            "btn" => {
                                if let Some(button) = parts.next()
                                    && let Some(p) = self.panes.get_mut(&id)
                                    && p.alive
                                    && p.buttons.iter().any(|b| b.id == button)
                                {
                                    let _ = p.write(
                                        format!("\x1b]5577;1;click;{button}\x1b\\").as_bytes(),
                                    );
                                }
                            }
                            "close" => {
                                self.close(id);
                            }
                            "zoom" => {
                                self.layout.zoom(id);
                                self.sync_layout();
                            }
                            "minimize" => {
                                self.layout.minimize(id);
                                self.sync_layout();
                            }
                            "scrollbottom" => {
                                if let Some(p) = self.panes.get_mut(&id) {
                                    p.terminal.scroll_edge(false);
                                }
                            }
                            "rerun" => {
                                if let Some(p) = self.panes.get_mut(&id) {
                                    let _ = p.start(self.session.as_deref());
                                }
                            }
                            "tab" => {
                                if let Some(index) =
                                    self.layout.tabs.iter().position(|t| t.id == id)
                                {
                                    self.select_tab(index);
                                }
                            }
                            "find" => {
                                self.layout.focus_pane(id);
                                self.sync_layout();
                            }
                            "newtab" => {
                                let _ = self.new_tab("");
                            }
                            "border" => {
                                let side = parts.next().unwrap_or("");
                                let direction = if matches!(side, "l" | "r") {
                                    Direction::Cols
                                } else {
                                    Direction::Rows
                                };
                                if self.layout.split_fits(id, direction, &self.config) {
                                    let _ = self.split(id, direction, matches!(side, "l" | "t"));
                                    self.toast(match side {
                                        "l" => "split left",
                                        "r" => "split right",
                                        "t" => "split up",
                                        _ => "split down",
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                    if verb == "pane"
                        && let Some(pane) = self.panes.get_mut(&id)
                    {
                        let r = self.layout.nodes[&id].content;
                        let local = match event {
                            Event::Mouse {
                                button,
                                action,
                                mods,
                                ..
                            } => Event::Mouse {
                                button: *button,
                                action: *action,
                                mods: *mods,
                                x: x.saturating_sub(r.x),
                                y: y.saturating_sub(r.y),
                            },
                            _ => unreachable!(),
                        };
                        if let Event::Mouse {
                            button, action: 0, ..
                        } = event
                            && matches!(button, 4 | 5)
                            && !pane.terminal.wants_mouse()
                        {
                            let up = *button == 4;
                            let lines = self.config.number("scroll_lines") as i64;
                            if pane.terminal.alt_screen() {
                                let key = Event::Key {
                                    key: if up {
                                        crate::ghostty::ffi::GHOSTTY_KEY_ARROW_UP
                                    } else {
                                        crate::ghostty::ffi::GHOSTTY_KEY_ARROW_DOWN
                                    },
                                    mods: 0,
                                    action: 1,
                                    text: Vec::new(),
                                    unshifted: 0,
                                };
                                for _ in 0..lines {
                                    let _ = pane.input(&key);
                                }
                            } else {
                                pane.terminal.scroll(if up { -lines } else { lines });
                            }
                        } else {
                            let _ = pane.input(&local);
                        }
                    }
                }
                if *action == 1 {
                    self.pressed = None;
                }
            }
            _ => self.forward(event),
        }
    }
    fn forward(&mut self, event: &Event) {
        if let Some(id) = self.layout.focus() {
            let _ = self.panes.get_mut(&id).unwrap().input(event);
        }
    }
    fn ctrl_d_exit(&mut self, event: &Event) -> bool {
        let Event::Key {
            key, mods, action, ..
        } = event
        else {
            return false;
        };
        if !self.config.boolean("ctrl_d_exits")
            || *action == 2
            || *key != crate::ghostty::ffi::GHOSTTY_KEY_D
            || *mods
                & (crate::input::CTRL
                    | crate::input::ALT
                    | crate::input::SUPER
                    | crate::input::SHIFT)
                != crate::input::CTRL
        {
            return false;
        }
        let Some(id) = self.layout.focus() else {
            return false;
        };
        let pane = self.panes.get_mut(&id).unwrap();
        if pane.terminal.alt_screen() || pane.line_len != 0 || pane.foreground_command().is_some() {
            return false;
        }
        let _ = pane.write(b"exit\r");
        true
    }
    pub fn action(&mut self, action: Action) {
        use Action::*;
        let focus = self.layout.focus();
        match action {
            Split | SplitCols | SplitRows => {
                if action == SplitRows && self.focus_float_grow(-1) {
                    return;
                }
                if let Some(id) = focus {
                    if self.layout.nodes[&id].floating {
                        self.toast("a floating pane cannot be split");
                        return;
                    }
                    let r = self.layout.nodes[&id].rect;
                    let preferred =
                        if r.w as u64 >= r.h as u64 * self.config.number("gap_aspect").max(1) {
                            Direction::Cols
                        } else {
                            Direction::Rows
                        };
                    let mut dir = match action {
                        SplitCols => Direction::Cols,
                        SplitRows => Direction::Rows,
                        _ => preferred,
                    };
                    if action == Split && !self.layout.split_fits(id, dir, &self.config) {
                        dir = if dir == Direction::Cols {
                            Direction::Rows
                        } else {
                            Direction::Cols
                        };
                    }
                    if self.layout.split_fits(id, dir, &self.config) {
                        let _ = self.split(id, dir, false);
                        if action == Split {
                            self.toast(if dir == Direction::Cols {
                                "split right"
                            } else {
                                "split down"
                            });
                        }
                    } else {
                        self.toast(if action == Split {
                            "no room to split"
                        } else if dir == Direction::Cols {
                            "no room to split across"
                        } else {
                            "no room to split down"
                        });
                    }
                }
            }
            ClosePane => {
                self.close(0);
            }
            NewTab => {
                let _ = self.new_tab("");
            }
            PaneToNextTab => self.push_pane_tab(true),
            PaneToPrevTab => self.push_pane_tab(false),
            PaneToNewTab => {
                let moved = self.move_pane_to_new_tab(0, "").is_some();
                self.toast(if moved {
                    "into a tab of its own"
                } else {
                    "it is the only pane in this tab"
                });
            }
            CloseTab => {
                if self.layout.tabs.len() < 2 {
                    self.toast("last tab: quit the session instead");
                } else {
                    let n = self
                        .layout
                        .leaves(self.layout.tabs[self.layout.current].root)
                        .len();
                    self.close_tab(0);
                    self.toast(&format!(
                        "closed tab · {n} pane{}",
                        if n == 1 { "" } else { "s" }
                    ));
                }
            }
            NextTab => self.cycle_tab(1),
            PrevTab => self.cycle_tab(-1),
            SelectTab(n) => {
                self.select_tab(n as usize - 1);
            }
            FocusLeft => self.layout.focus_direction(-1, 0),
            FocusRight => self.layout.focus_direction(1, 0),
            FocusUp => self.layout.focus_direction(0, -1),
            FocusDown => self.layout.focus_direction(0, 1),
            FocusNext => self.layout.focus_next(),
            ResizeLeft | ResizeRight | ResizeUp | ResizeDown => {
                let (dx, dy) = match action {
                    ResizeLeft => (-1, 0),
                    ResizeRight => (1, 0),
                    ResizeUp => (0, -1),
                    _ => (0, 1),
                };
                if !self.focus_float_move(dx, dy) {
                    self.layout.resize_focus(dx, dy, &self.config);
                }
            }
            FloatGrow | FloatShrink => {
                if !self.focus_float_grow(if action == FloatGrow { 1 } else { -1 }) {
                    self.toast(if action == FloatGrow {
                        "nothing floating to grow"
                    } else {
                        "nothing floating to shrink"
                    });
                }
            }
            Equalize => {
                if !self.layout.equalize() {
                    self.toast("nothing to even out");
                }
            }
            RotateLayout => {
                if !self.layout.rotate(self.cols, self.rows, &self.config) {
                    self.toast(
                        if self.layout.tabs.get(self.layout.current).is_some_and(|t| {
                            self.layout.nodes[&t.root].kind == crate::layout::Kind::Leaf
                        }) {
                            "nothing to turn"
                        } else {
                            "no room to turn it"
                        },
                    );
                }
            }
            Zoom => {
                if let Some(id) = focus {
                    self.layout.zoom(id);
                }
            }
            Minimize => {
                if !focus.is_some_and(|id| self.layout.minimize(id)) {
                    self.toast("nothing else to show");
                }
            }
            Float => {
                if !focus.is_some_and(|id| {
                    self.layout
                        .toggle_float(id, self.cols, self.rows, &self.config)
                }) {
                    self.toast("nothing to float over");
                }
            }
            NewFloat => {
                if self.new_float().ok().flatten().is_none() {
                    self.toast("cannot open a pane");
                }
            }
            Rerun => {
                if !focus.is_some_and(|id| {
                    self.panes
                        .get_mut(&id)
                        .unwrap()
                        .start(self.session.as_deref())
                        .unwrap_or(false)
                }) {
                    self.toast("pane is still running");
                }
            }
            ScrollUp | ScrollDown | ScrollPageUp | ScrollPageDown | ScrollTop | ScrollBottom => {
                if let Some(id) = focus {
                    let p = self.panes.get_mut(&id).unwrap();
                    match action {
                        ScrollTop => p.terminal.scroll_edge(true),
                        ScrollBottom => p.terminal.scroll_edge(false),
                        _ => {
                            let n = if matches!(action, ScrollPageUp | ScrollPageDown) {
                                p.size.rows as i64
                            } else {
                                self.config.number("scroll_lines") as i64
                            };
                            p.terminal
                                .scroll(if matches!(action, ScrollUp | ScrollPageUp) {
                                    -n
                                } else {
                                    n
                                });
                        }
                    }
                }
            }
            Help => self.help = !self.help,
            Search => self.search_begin(),
            EditConfig => {
                let _ = self.edit_config();
            }
            Palette => self.open_picker(ui::Picker::Commands),
            Finder => self.open_picker(ui::Picker::Panes),
            Themes => self.open_picker(ui::Picker::Themes),
            Workspaces => self.open_picker(ui::Picker::Projects),
            SaveWorkspace => match self.save_workspace(0, Option::None, "commands", true) {
                Ok(saved) => self.toast(&format!(
                    "{} slosh.layout · {} pane{}, {} suspended",
                    if saved.replaced { "replaced" } else { "wrote" },
                    saved.panes,
                    if saved.panes == 1 { "" } else { "s" },
                    saved.suspended
                )),
                Err(e) => self.toast(&e),
            },
            RenamePane | RenameTab | SetPurpose => self.begin_edit(action),
            Detach => self.detach = true,
            Quit => self.quit = true,
            ClearShaders => {
                if let Some(id) = focus {
                    let had = self
                        .chains
                        .remove(&id)
                        .is_some_and(|c| !c.chrome.is_empty() || !c.content.is_empty());
                    self.toast(if had {
                        "shaders cleared"
                    } else {
                        "no shaders on this pane"
                    });
                }
            }
            LiteralPrefix => {
                let event = commands::prefix_event(self.config.bindings.prefix);
                self.forward(&event);
            }
            // Keep this match exhaustive so a new action cannot silently do nothing.
            None => {}
        }
        self.sync_layout();
        if let Some(id) = self.layout.focus() {
            self.panes.get_mut(&id).unwrap().bell = false;
        }
    }
    pub(crate) fn panes_json(&self) -> Reply {
        let mut panes = Vec::new();
        for (index, tab) in self.layout.tabs.iter().enumerate() {
            for id in self.layout.leaves(tab.root) {
                let n = &self.layout.nodes[&id];
                let p = &self.panes[&id];
                panes.push(Reply::from(json!({"id":id,"x":n.rect.x,"y":n.rect.y,"w":n.rect.w,"h":n.rect.h,"content_x":n.content.x,"content_y":n.content.y,"content_w":n.content.w,"content_h":n.content.h,
                "focused":self.layout.focus()==Some(id),"hidden":n.hidden,"floating":n.floating,"suspended":p.suspended,"alive":p.alive,"pid":p.pid(),"exit_code":p.exit_code(),"exit_signal":p.exit_signal(),
                "purpose":n.purpose,"purpose_declared":n.purpose_locked,"tab":index+1,"tab_id":tab.id,"busy":p.busy&&p.alive,
                "program_state":p.program.showing.as_ref().map_or("", |r| r.state.name()),
                "program_kind":p.program.showing.as_ref().map_or("", |r| r.kind.as_str()),
                "program_app":p.program.showing.as_ref().map_or("", |r| r.app.as_str()),
                "progress":p.program.showing.as_ref().map_or(-1, |r| r.progress)})).field("title", Reply::string_bytes(p.title_bytes())).field("status", Reply::string_bytes(&p.status)));
            }
        }
        Reply::Array(panes)
    }
    pub(crate) fn tabs_json(&self) -> Reply {
        Reply::Array(self.layout.tabs.iter().enumerate().map(|(i,t)|Reply::from(json!({"id":t.id,"index":i+1,"purpose":t.purpose,"purpose_declared":t.purpose_locked,"active":i==self.layout.current,"panes":self.layout.leaves(t.root).len()})).field("name",Reply::string_bytes(&t.name))).collect())
    }
}
