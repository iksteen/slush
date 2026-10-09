//! Shared command vocabulary for headless scripts and control clients.
use super::App;
use crate::{
    bindings::Chord,
    config::{Config, Diagnostic},
    input::{Event, Parser},
    json_control::{Reply, Request, native_bytes, os_string},
    layout::{Direction, Id},
    screen::Screen,
};
use serde_json::{Value, json};
fn error(message: &str) -> Value {
    json!({"ok":false,"error":message})
}
fn ok() -> Value {
    json!({"ok":true})
}
fn num(req: &Value, key: &str, default: i64) -> i64 {
    // The reference JSON parser stores every number as a double, then converts
    // it to long for numeric control fields (including fractional/exponent forms).
    let (min, max) = if size_of::<libc::c_long>() == 8 {
        (i64::MIN, i64::MAX)
    } else {
        (i64::from(i32::MIN), i64::from(i32::MAX))
    };
    let number = match req.get(key) {
        Some(Value::Number(number)) => {
            let number = number.as_f64().unwrap_or_else(|| {
                if number.to_string().starts_with('-') {
                    f64::NEG_INFINITY
                } else {
                    f64::INFINITY
                }
            });
            number as i64
        }
        Some(Value::String(text)) => crate::kdl::integer(text).unwrap_or(0),
        _ => default,
    };
    number.clamp(min, max)
}
fn word<'a>(req: &'a Value, key: &str, default: &'a str) -> &'a str {
    req.get(key).and_then(Value::as_str).unwrap_or(default)
}
fn flag(req: &Value, key: &str, default: bool) -> bool {
    req.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// The script vocabulary's byte escapes, bounded like the original command reader.
pub fn unescape(text: impl AsRef<[u8]>) -> Vec<u8> {
    let bytes = text.as_ref();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() && out.len() < 4096 {
        let mut c = bytes[i];
        i += 1;
        if c == b'\\' && i < bytes.len() {
            c = bytes[i];
            i += 1;
            c = match c {
                b'e' => 27,
                b'n' => b'\n',
                b'r' => b'\r',
                b't' => b'\t',
                b'0' => 0,
                b'x' if i + 1 < bytes.len() => {
                    let a = (bytes[i] as char).to_digit(16);
                    let b = (bytes[i + 1] as char).to_digit(16);
                    if let (Some(a), Some(b)) = (a, b) {
                        i += 2;
                        (a * 16 + b) as u8
                    } else {
                        b'x'
                    }
                }
                c => c,
            };
        }
        out.push(c);
    }
    out
}
pub fn prefix_event(chord: Chord) -> Event {
    use crate::ghostty::ffi::*;
    let ascii = match chord.key {
        GHOSTTY_KEY_A..=GHOSTTY_KEY_Z => Some(b'a' + (chord.key - GHOSTTY_KEY_A) as u8),
        GHOSTTY_KEY_DIGIT_0..=GHOSTTY_KEY_DIGIT_9 => {
            Some(b'0' + (chord.key - GHOSTTY_KEY_DIGIT_0) as u8)
        }
        GHOSTTY_KEY_SPACE => Some(b' '),
        GHOSTTY_KEY_BACKSLASH => Some(b'\\'),
        GHOSTTY_KEY_SLASH => Some(b'/'),
        GHOSTTY_KEY_MINUS => Some(b'-'),
        _ => None,
    };
    let text = ascii
        .filter(|_| chord.mods & crate::input::CTRL == 0)
        .map(|c| {
            vec![if chord.mods & crate::input::SHIFT != 0 {
                c.to_ascii_uppercase()
            } else {
                c
            }]
        })
        .unwrap_or_default();
    Event::Key {
        key: chord.key,
        mods: chord.mods,
        action: 0,
        text,
        unshifted: ascii.map_or(0, u32::from),
    }
}
impl App {
    fn feed(&mut self, in_: &mut Parser, s: &Screen, bytes: &[u8]) {
        for event in in_.feed(bytes) {
            self.event(&event, s);
        }
        if in_.pending() {
            for event in in_.timeout() {
                self.event(&event, s);
            }
        }
    }
    pub fn command(
        &mut self,
        s: &mut Screen,
        in_: &mut Parser,
        line: impl AsRef<[u8]>,
    ) -> Option<Vec<u8>> {
        let line = line.as_ref().split(|b| *b == 0).next().unwrap_or_default();
        let start = line.iter().position(|b| *b != b' ').unwrap_or(line.len());
        let line = &line[start..];
        if line.starts_with(b"{") {
            let reply = match crate::json_control::parse_request(line) {
                Ok(req) => self.command_json(s, in_, &req),
                Err(_) => Reply::from(error("malformed json")).bytes(),
            };
            return Some(reply);
        }
        let (verb, arg_bytes) = line
            .iter()
            .position(|b| *b == b' ')
            .map_or((line, b"".as_slice()), |n| (&line[..n], &line[n + 1..]));
        let verb = std::str::from_utf8(verb).ok()?;
        let display = String::from_utf8_lossy(arg_bytes);
        let arg = display.as_ref();
        let native_arg = os_string(arg_bytes);
        let reply: String = match verb {
            "workspaces" => return Some(self.workspaces_text()),
            "open-workspace" => match self.open_workspace(&*native_arg, false) {
                Ok(opened) => {
                    s.force_full = true;
                    format!(
                        "{} tab {}",
                        if opened.created { "opened" } else { "focused" },
                        opened.tab
                    )
                }
                Err(e) => e,
            },
            "close-workspace" => {
                if let Some(project) = self.find_project(&*native_arg) {
                    let closed = self.close_workspace(&project.slug);
                    s.force_full = true;
                    format!("closed {closed} tab{}", if closed == 1 { "" } else { "s" })
                } else {
                    "no such project".into()
                }
            }
            "save-workspace" => {
                match self.save_workspace(
                    0,
                    (!arg_bytes.is_empty()).then_some(native_arg.as_ref()),
                    "commands",
                    false,
                ) {
                    Ok(saved) => {
                        let mut out = native_bytes(saved.path.as_os_str()).into_owned();
                        out.extend_from_slice(
                            format!(" ({} panes, {} suspended)", saved.panes, saved.suspended)
                                .as_bytes(),
                        );
                        return Some(out);
                    }
                    Err(e) => e,
                }
            }
            "send" => {
                self.feed(in_, s, &unescape(arg_bytes));
                String::new()
            }
            "raw" => {
                self.write(0, &unescape(arg_bytes));
                String::new()
            }
            "resize" => {
                let mut argument = arg_bytes.to_vec();
                argument.push(0);
                let mut cols = libc::c_int::from(s.cols);
                let mut rows = libc::c_int::from(s.rows);
                // SAFETY: the terminated input and fixed format are valid for
                // this call; both %d destinations point to writable C integers.
                unsafe {
                    libc::sscanf(
                        argument.as_ptr().cast(),
                        c"%d %d".as_ptr(),
                        &mut cols,
                        &mut rows,
                    );
                }
                let (cols, rows) = (cols as u16, rows as u16);
                s.resize(cols, rows);
                self.resize(cols, rows);
                String::new()
            }
            "snapshot" => {
                self.compose(s);
                if arg == "text" {
                    return Some(s.dump_bytes());
                } else {
                    return Some(s.snapshot_reply().bytes());
                }
            }
            "panes" => {
                self.compose(s);
                return Some(self.panes_json().bytes());
            }
            "tabs" => {
                self.compose(s);
                return Some(self.tabs_json().bytes());
            }
            "dump-layout" => {
                self.compose(s);
                return Some(
                    self.dump_layout(0, None, "as-is")
                        .map(|d| d.0)
                        .unwrap_or_default(),
                );
            }
            "deadline" => self.next_deadline_ms().to_string(),
            "alive" => (!self.should_quit()).to_string(),
            "quit" => {
                self.quit = true;
                String::new()
            }
            "reload" => match self.reload_config(s) {
                Ok(()) => "ok".into(),
                Err(e) => return Some(e.bytes().to_vec()),
            },
            "theme" => {
                if arg_bytes.is_empty() {
                    let current = self.config.theme_name.as_deref().unwrap_or_default();
                    let mut out = Vec::new();
                    for name in self.config.themes() {
                        if !out.is_empty() {
                            out.push(b' ');
                        }
                        out.extend_from_slice(&name);
                        if name == current {
                            out.push(b'*');
                        }
                    }
                    return Some(out);
                } else {
                    return Some(match self.theme(s, arg_bytes, false) {
                        Ok(()) => self.config.theme_name.clone().unwrap_or_default(),
                        Err(e) => e.bytes().to_vec(),
                    });
                }
            }
            _ => return None,
        };
        Some(reply.into_bytes())
    }
    pub(crate) fn command_json(
        &mut self,
        s: &mut Screen,
        in_: &mut Parser,
        native_req: &Request,
    ) -> Vec<u8> {
        let value = native_req.value();
        let req = &value;
        let reply = match req.get("cmd").and_then(Value::as_str).unwrap_or("") {
            "theme" => {
                let save = flag(req, "save", false);
                let name = native_req.get("name").and_then(Request::as_bytes);
                if let Some(name) = name {
                    if let Err(e) = self.theme(s, name, save && !name.is_empty()) {
                        return Reply::from(json!({"ok":false})).field("error", Reply::string_bytes(e.bytes())).bytes();
                    }
                    let worn = self.config.theme_name.as_deref().unwrap_or_default();
                    self.toast(&format!("theme: {}{}", if worn.is_empty() { "the config's own".into() } else { String::from_utf8_lossy(worn) }, if save { " (saved)" } else { "" }));
                }
                let mut reply = Reply::from(ok())
                    .field("theme", Reply::string_bytes(self.config.theme_name.as_deref().unwrap_or_default()))
                    .field("themes", Reply::Array(self.config.themes().iter().map(Reply::string_bytes).collect()))
                    .field("dirs", Reply::Array(self.config.theme_dirs().iter().map(Reply::native).collect()));
                if name.is_some() { reply = reply.field("saved", json!(save).into()); }
                reply
            }
            "snapshot" => {
                self.compose(s);
                match word(req, "format", "json") {
                    "text" => Reply::from(ok()).field("text", Reply::string_bytes(s.dump_bytes())),
                    "bytes" => Reply::from(ok()).field("bytes", Reply::string_bytes(s.render())),
                    _ => Reply::from(ok()).field("screen", s.snapshot_reply()),
                }
            }
            "reload" => match self.reload_config(s) {
                Ok(()) => {
                    if let Some(warning) = self.config.messages.first().cloned() {
                        self.toast(&warning);
                        Reply::from(ok()).field("warning", Reply::string_bytes(warning.bytes()))
                    } else {
                        self.toast("config reloaded");
                        Reply::from(ok())
                    }
                }
                Err(error) => {
                    self.toast(&error);
                    Reply::from(json!({"ok":false})).field("error", Reply::string_bytes(error.bytes()))
                }
            },
            "panes" => {
                self.compose(s);
                Reply::from(ok()).field("panes", self.panes_json())
            }
            "move-pane" if num(req, "tab", 0) == 0 => {
                match self.move_pane_to_new_tab(num(req, "id", 0) as Id, native_req.get("name").and_then(Request::as_bytes).unwrap_or_default()) {
                    Some(tab) => Reply::from(json!({"ok":true,"tab":tab})),
                    None => Reply::from(error("cannot move it to a tab of its own")),
                }
            }
            "tabs" => {
                self.compose(s);
                Reply::from(ok()).field("tabs", self.tabs_json())
            }
            "new-tab" => match self.new_tab(native_req.get("name").and_then(Request::as_bytes).unwrap_or_default()) {
                Ok(id) => {
                    if let Some(p) = req.get("purpose").and_then(Value::as_str) {
                        self.set_tab_purpose(id, p, true);
                    }
                    Reply::from(json!({"ok":true,"id":id}))
                }
                Err(_) => Reply::from(error("cannot create tab")),
            },
            "move-tab" => {
                let index = num(req, "index", 0);
                let id = num(req, "id", 0) as Id;
                if index < 1 {
                    Reply::from(error("index is 1-based"))
                } else if self.move_tab(id, index as usize - 1) {
                    Reply::from(ok()).field("tabs", self.tabs_json())
                } else {
                    Reply::from(error("no such tab"))
                }
            }
            "set-name" => {
                let id = num(req, "id", 0) as Id;
                let name = native_req.get("name").and_then(Request::as_bytes).unwrap_or_default();
                if word(req, "target", "tab") == "pane" {
                    if let Some(id) = self.resolve(id) {
                        self.panes.get_mut(&id).unwrap().set_name(name);
                        Reply::from(ok())
                    } else {
                        Reply::from(error("no such pane"))
                    }
                } else if let Some(tab) = self.layout.tabs.iter_mut().find(|t| t.id == id) {
                    tab.name = name[..name.len().min(63)].to_vec();
                    Reply::from(ok())
                } else {
                    Reply::from(error("no such tab"))
                }
            }
            "workspaces" => self.workspaces_json(),
            "open-workspace" => {
                let Some(name) = native_req
                    .get("name")
                    .or_else(|| native_req.get("path"))
                    .and_then(Request::as_os)
                else {
                    return Reply::from(error("need a name or a path")).bytes();
                };
                match self.open_workspace(&*name, flag(req, "suspended", false)) {
                    Ok(opened) => {
                        s.force_full = true;
                        Reply::from(json!({"ok":true,"tab":opened.tab,"created":opened.created,"tabs":opened.tabs,"honoured":opened.honoured})).field("purpose", Reply::string_bytes(&opened.purpose)).field("path", Reply::native(&opened.path))
                    }
                    Err(e) => error(&e).into(),
                }
            }
            "save-workspace" => match self.save_workspace(
                num(req, "tab", 0) as Id,
                native_req.get("path").and_then(Request::as_os).as_deref(),
                word(req, "suspend", "commands"),
                flag(req, "force", false),
            ) {
                Ok(saved) => {
                    Reply::from(json!({"ok":true,"purpose":saved.purpose,"panes":saved.panes,"suspended":saved.suspended,"replaced":saved.replaced})).field("path", Reply::native(&saved.path))
                }
                Err(e) => error(&e).into(),
            },
            "close-workspace" => {
                let slug = if let Some(slug) = native_req.get("purpose").and_then(Request::as_bytes) {
                    slug.to_vec()
                } else {
                    let Some(name) = native_req.get("name").and_then(Request::as_os) else {
                        return Reply::from(error("need a name or a purpose")).bytes();
                    };
                    let Some(project) = self.find_project(&*name) else {
                        return Reply::from(error("no such project")).bytes();
                    };
                    project.slug
                };
                let closed = self.close_workspace(&slug);
                s.force_full = true;
                Reply::from(json!({"ok":true,"closed":closed}))
            }
            "edit-config" => Reply::from(match self.edit_config() {
                Some(id) => json!({"ok":true,"id":id}),
                None => error("no room for another pane"),
            }),
            "dump-layout" => {
                self.compose(s);
                match self.dump_layout_for(
                    num(req, "tab", 0) as Id,
                    native_req.get("relative_to").and_then(Request::as_os).as_deref().map(std::path::Path::new),
                    word(req, "suspend", "as-is"), false,
                ) {
                    Ok((kdl, panes, suspended)) => {
                        Reply::from(json!({"ok":true,"panes":panes,"suspended":suspended})).field("kdl", Reply::string_bytes(kdl))
                    }
                    Err(e) => error(&e).into(),
                }
            }
            "apply-layout" => {
                let replace = flag(req, "replace", false);
                let focus = flag(req, "focus", true);
                let result = if let Some(kdl) = native_req.get("kdl").and_then(Request::as_bytes) {
                    self.apply_layout_text(kdl, replace, focus, None)
                } else if let Some(path) = native_req.get("path").and_then(Request::as_os) {
                    self.apply_layout_file(std::path::Path::new(&*path), replace, focus)
                } else {
                    Err("need a path or kdl".into())
                };
                match result {
                    Ok(()) => {
                        s.force_full = true;
                        Reply::from(ok()).field("tabs", self.tabs_json())
                    }
                    Err(e) => error(&e).into(),
                }
            }

            "send" => {
                self.feed(in_, s, native_req.get("data").and_then(Request::as_bytes).unwrap_or_default());
                Reply::from(ok())
            }
            "raw" => {
                if self.write(num(req, "id", 0) as Id, native_req.get("data").and_then(Request::as_bytes).unwrap_or_default()) {
                    Reply::from(ok())
                } else {
                    Reply::from(error("no such pane"))
                }
            }
            _ => self.command_json_value(s, req).into(),
        };
        reply.bytes()
    }
    fn command_json_value(&mut self, s: &mut Screen, req: &Value) -> Value {
        let Some(cmd) = req.get("cmd").and_then(Value::as_str) else {
            return error("missing cmd");
        };
        let id = num(req, "id", 0) as Id;
        match cmd {
            "move-pane" => {
                let tab = num(req, "tab", 0) as Id;
                if tab == 0 {
                    match self.move_pane_to_new_tab(id, word(req, "name", "")) {
                        Some(tab) => json!({"ok":true,"tab":tab}),
                        None => error("cannot move it to a tab of its own"),
                    }
                } else {
                    let beside = num(req, "beside", 0) as Id;
                    let direction = if word(req, "dir", "cols") == "rows" {
                        Direction::Rows
                    } else {
                        Direction::Cols
                    };
                    if self.move_pane(id, tab, beside, direction, flag(req, "focus", true)) {
                        json!({"ok":true,"tab":tab})
                    } else {
                        error(if beside == 0 {
                            "cannot move it there"
                        } else {
                            "cannot move it there (is `beside` in that tab?)"
                        })
                    }
                }
            }
            "capture" => {
                self.compose(s);
                if let Some(id) = self.resolve(id) {
                    let text = self.panes.get_mut(&id).unwrap().text().unwrap_or_default();
                    json!({"ok":true,"id":id,"text":text})
                } else if id == 0 {
                    json!({"ok":true,"id":0,"text":""})
                } else {
                    error("no such pane")
                }
            }
            "resize" => {
                let cols = num(req, "cols", s.cols as i64) as u16;
                let rows = num(req, "rows", s.rows as i64) as u16;
                if cols == 0 || rows == 0 {
                    return error("bad size");
                }
                s.resize(cols, rows);
                self.resize(cols, rows);
                self.cell_size(num(req, "cell_w", 0) as u16, num(req, "cell_h", 0) as u16);
                ok()
            }
            "split" => {
                let direction = if word(req, "dir", "cols") == "rows" {
                    Direction::Rows
                } else {
                    Direction::Cols
                };
                if self
                    .resolve(id)
                    .is_some_and(|id| self.layout.nodes[&id].floating)
                {
                    return error("cannot split a floating pane");
                }
                match self.split(id, direction, false) {
                    Ok(Some(id)) => json!({"ok":true,"id":id}),
                    _ => error("no such pane"),
                }
            }
            "focus" => {
                if self.layout.focus_pane(id) {
                    self.sync_layout();
                    json!({"ok":true,"id":id})
                } else {
                    error("no such pane")
                }
            }
            "close" => {
                if self.close(id) {
                    ok()
                } else {
                    error("no such pane")
                }
            }
            "rerun" => {
                if let Some(id) = self.resolve(id)
                    && self
                        .panes
                        .get_mut(&id)
                        .unwrap()
                        .start(self.session.as_deref())
                        .unwrap_or(false)
                {
                    json!({"ok":true,"id":self.layout.focus().unwrap_or(0)})
                } else {
                    error("cannot run it again")
                }
            }
            "new-float" => match self.new_float() {
                Ok(Some(id)) => json!({"ok":true,"id":id}),
                _ => error("cannot open a pane"),
            },
            "float" => {
                let Some(id) = self.resolve(id) else {
                    return error("cannot float that");
                };
                let place = ["x", "y", "w", "h"].iter().any(|key| {
                    num(req, key, if matches!(*key, "x" | "y") { -1 } else { 0 })
                        >= if matches!(*key, "x" | "y") { 0 } else { 1 }
                });
                if (!place || !self.layout.nodes[&id].floating)
                    && !self
                        .layout
                        .toggle_float(id, self.cols, self.rows, &self.config)
                {
                    return error("cannot float that");
                }
                if place {
                    let n = self.layout.nodes.get_mut(&id).unwrap();
                    for (key, value) in [
                        ("x", &mut n.float_rect.x),
                        ("y", &mut n.float_rect.y),
                        ("w", &mut n.float_rect.w),
                        ("h", &mut n.float_rect.h),
                    ] {
                        let v = num(req, key, -1);
                        if v >= if matches!(key, "w" | "h") { 1 } else { 0 } {
                            *value = v.min(u16::MAX as i64) as u16;
                        }
                    }
                }
                self.sync_layout();
                json!({"ok":true,"floating":i32::from(self.layout.nodes[&id].floating)})
            }
            "clear-shaders" => {
                let cleared = self
                    .resolve(id)
                    .is_some_and(|id| self.chains.remove(&id).is_some());
                json!({"ok":true,"cleared":i32::from(cleared)})
            }
            "select-tab" => {
                let index = if id == 0 {
                    Some(num(req, "index", 1).wrapping_sub(1) as usize)
                } else {
                    self.layout.tabs.iter().position(|t| t.id == id)
                };
                if index.is_some_and(|i| self.select_tab(i)) {
                    json!({"ok":true,"id":self.layout.tabs[self.layout.current].id})
                } else {
                    error("no such tab")
                }
            }
            "close-tab" => {
                if self.close_tab(id) {
                    ok()
                } else {
                    error("no such tab")
                }
            }
            "set-purpose" => {
                let declared = flag(req, "declared", true);
                let purpose = word(req, "purpose", "");
                let success = if word(req, "target", "pane") == "tab" {
                    self.set_tab_purpose(id, purpose, declared)
                } else if let Some(id) = self.resolve(id) {
                    let n = self.layout.nodes.get_mut(&id).unwrap();
                    if n.purpose_locked && !declared {
                        false
                    } else {
                        n.purpose = self::purpose(purpose);
                        n.purpose_locked = declared && !n.purpose.is_empty();
                        true
                    }
                } else {
                    false
                };
                if success { ok() } else { error("refused") }
            }
            "notify" => {
                if let Some(text) = req.get("text").and_then(Value::as_str) {
                    self.toast(text);
                    ok()
                } else {
                    error("need text")
                }
            }
            "clipboard" => json!({"ok":true,"text":self.clipboard}),
            "graphics" => {
                self.compose(s);
                if word(req, "format", "placements") == "bytes" {
                    json!({"ok":true,"bytes":String::from_utf8_lossy(&self.graphics_bytes())})
                } else {
                    self.graphics_bytes();
                    json!({"ok":true,"placements":self.graphics.json()})
                }
            }
            "deadline" => json!({"ok":true,"ms":self.next_deadline_ms()}),
            "splash" => {
                self.splash(
                    i64::from(num(req, "fx", -1) as libc::c_int),
                    i64::from(num(req, "motion", -1) as libc::c_int),
                );
                ok()
            }
            "alive" => {
                json!({"ok":true,"alive":!self.should_quit(),"panes":self.panes.len(),"tabs":self.layout.tabs.len()})
            }
            "quit" => {
                self.quit = true;
                ok()
            }
            _ => error("unknown cmd"),
        }
    }
    fn tab_index(&self, id: Id) -> Option<usize> {
        if id == 0 {
            self.layout
                .tabs
                .get(self.layout.current)
                .map(|_| self.layout.current)
        } else {
            self.layout.tabs.iter().position(|t| t.id == id)
        }
    }
    fn set_tab_purpose(&mut self, id: Id, purpose: &str, declared: bool) -> bool {
        let Some(index) = self.tab_index(id) else {
            return false;
        };
        let t = &mut self.layout.tabs[index];
        if t.purpose_locked && !declared {
            return false;
        }
        t.purpose = self::purpose(purpose);
        t.purpose_locked = declared && !t.purpose.is_empty();
        true
    }
    pub(crate) fn reload_config(&mut self, s: &mut Screen) -> Result<(), Diagnostic> {
        self.load_config()?;
        s.force_full = true;
        Ok(())
    }
    fn load_config(&mut self) -> Result<(), Diagnostic> {
        let mut config = Config::default();
        let path = Config::default_path();
        if !config.load(&path) {
            return Err(config
                .messages
                .first()
                .cloned()
                .unwrap_or_else(|| Diagnostic::new(format!("{}: cannot read", path.display()))));
        }
        if !self.theme_override.is_empty() {
            config.apply_theme(&self.theme_override);
        }
        self.config = config;
        self.sync_layout();
        Ok(())
    }
    fn theme(&mut self, s: &mut Screen, name: &[u8], save: bool) -> Result<(), Diagnostic> {
        self.set_theme(name, save)?;
        s.force_full = true;
        Ok(())
    }
    pub(super) fn set_theme(
        &mut self,
        name: impl AsRef<[u8]>,
        save: bool,
    ) -> Result<(), Diagnostic> {
        let name = name.as_ref().split(|b| *b == 0).next().unwrap_or_default();
        if name.is_empty() {
            self.theme_override.clear();
            return self.load_config();
        }
        // C probes the default search directories before changing the session.
        let mut probe = Config::default();
        if !probe.apply_theme(name) {
            return Err(probe
                .messages
                .first()
                .cloned()
                .unwrap_or_else(|| Diagnostic::new(b"cannot use that theme")));
        }
        if save {
            let file = Config::default_path();
            crate::config::write_theme_name(&file, name).map_err(|_| {
                Diagnostic::new(
                    [b"cannot write ", native_bytes(file.as_os_str()).as_ref()].concat(),
                )
            })?;
        }
        self.theme_override = name.into();
        if self.load_config().is_err() {
            let before = self.config.messages.len();
            if !self.config.apply_theme(name) {
                return Err(self
                    .config
                    .messages
                    .get(before)
                    .cloned()
                    .unwrap_or_else(|| Diagnostic::new(b"cannot use that theme")));
            }
            self.sync_layout();
        }
        Ok(())
    }
    pub fn next_deadline_ms(&self) -> i64 {
        let now = std::time::Instant::now();
        let mut deadline = self
            .toasts
            .iter()
            .map(|(_, until)| until.saturating_duration_since(now).as_millis() as i64)
            .min()
            .unwrap_or(-1);
        if let Some(splash) = self.splash_deadline() {
            deadline = if deadline < 0 {
                splash
            } else {
                deadline.min(splash)
            };
        }
        if self.hover_pending {
            let due = self.pointer_since
                + std::time::Duration::from_millis(self.config.number("hover_delay_ms"));
            if due > now {
                let ms = due.duration_since(now).as_millis() as i64;
                deadline = if deadline < 0 { ms } else { deadline.min(ms) };
            }
        }
        if let Some(select) = self.selection_deadline() {
            deadline = if deadline < 0 {
                select
            } else {
                deadline.min(select)
            };
        }
        if self.sidebar_spins {
            let anim = self.config.number("anim_ms") as i64;
            deadline = if deadline < 0 {
                anim
            } else {
                deadline.min(anim)
            };
        }
        for (id, _) in self
            .states
            .iter()
            .filter(|(id, _)| self.layout.tab_of(**id) == Some(self.layout.current))
        {
            let state = self.state(*id);
            let own = self.chains.get(id);
            let chains = [
                Some(&self.config.shaders),
                own,
                state.map(|i| &self.config.states[i]),
            ];
            if chains
                .into_iter()
                .flatten()
                .flat_map(|c| c.content.iter().chain(&c.chrome))
                .any(|s| {
                    s.expression
                        .as_ref()
                        .is_some_and(|e| e.borrow().deps() & crate::expr::DEP_TIME != 0)
                })
            {
                let anim = self.config.number("anim_ms") as i64;
                deadline = if deadline < 0 {
                    anim
                } else {
                    deadline.min(anim)
                };
            }
        }
        deadline
    }
}
pub(super) fn purpose(text: impl AsRef<[u8]>) -> String {
    text.as_ref()
        .iter()
        .copied()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b':' | b'/' | b'-'))
        .take(63)
        .map(char::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn script_escapes_keep_binary_bytes() {
        assert_eq!(unescape(r"\e[2J\x00\n\\"), b"\x1b[2J\0\n\\");
        assert_eq!(unescape(r"\xzz"), b"xzz");
    }
}
