//! Composition into the canonical cell buffer, independent of the front end.
use super::App;
use crate::{
    layout::{Direction, Id, Kind},
    screen::{self, BOLD, Color, Screen},
    shader::{self, Context, Rect},
};
use std::time::Instant;

const NONE: Color = Color {
    set: false,
    r: 0,
    g: 0,
    b: 0,
};
fn clip(text: &str, width: u16) -> String {
    let mut out = text.to_owned();
    while screen::cells(&out) > width {
        out.pop();
    }
    out
}
fn toast_line(text: &str, width: u16) -> String {
    let line = format!(" {text} ");
    if screen::cells(&line) <= width || width < 5 {
        return clip(&line, width);
    }
    let head = clip(&line, (width - 3) / 2);
    let tail_width = width - 3 - screen::cells(&head);
    let mut tail = String::new();
    for ch in line.chars().rev() {
        let candidate = format!("{ch}{tail}");
        if screen::cells(&candidate) > tail_width {
            break;
        }
        tail = candidate;
    }
    format!("{head}...{tail}")
}
impl App {
    pub fn compose(&mut self, s: &mut Screen) {
        if (s.cols, s.rows) != (self.cols, self.rows) {
            s.resize(self.cols, self.rows);
        }
        s.clear();
        s.hits.reset();
        s.cursor.visible = false;
        self.overlays.clear();
        self.graphics_requests.clear();
        self.sidebar_spins = false;
        if self.should_quit() {
            return;
        }
        self.sync_layout();
        self.search_refresh();
        self.selection_tick();
        let focus = self.layout.focus();
        if let Some(id) = focus {
            self.panes.get_mut(&id).unwrap().bell = false;
        }
        for (id, pane) in &mut self.panes {
            if self.layout.tab_of(*id) != Some(self.layout.current) {
                pane.terminal.drop_render_cache();
            }
        }
        let root = self.layout.tabs[self.layout.current].root;
        if self.config.boolean("status_bar") {
            if self.layout.sidebar(self.cols, self.rows, &self.config) == "top" {
                self.draw_tabs(s);
            } else {
                self.draw_sidebar(s);
            }
        }
        if self.config.boolean("compact") {
            self.compact_lines(s, root);
        }
        self.draw_node(s, root);
        self.draw_corners(s);
        let mut floats: Vec<_> = self
            .layout
            .leaves(root)
            .into_iter()
            .filter(|id| self.layout.nodes[id].floating && !self.layout.nodes[id].hidden)
            .collect();
        floats.sort_by_key(|id| self.layout.nodes[id].raised);
        for id in floats {
            if self.layout.nodes[&id].collapsed {
                continue;
            }
            self.cast_shadow(s, self.layout.nodes[&id].rect);
            self.draw_pane(s, id);
        }
        self.draw_minimized(s, root);
        self.draw_drop_zones(s);
        self.draw_resize_guides(s);
        self.draw_drag_strip(s);
        if self.config.boolean("status_line") {
            self.draw_status(s);
        }
        self.toasts.retain(|(_, until)| *until > Instant::now());
        for (i, (text, _)) in self.toasts.iter().rev().enumerate() {
            let text = toast_line(text, s.cols.saturating_sub(1).max(1));
            let w = screen::cells(&text);
            let Some(y) = s.rows.saturating_sub(1).checked_sub(i as u16) else {
                break;
            };
            let gap = self.config.number("gap") * self.config.number("gap_aspect");
            let x = s.cols.saturating_sub(w).saturating_sub(gap as u16);
            self.overlays.push(Rect { x, y, w, h: 1 });
            s.text(
                x,
                y,
                &text,
                self.config.color("toast_fg"),
                self.config.color("toast_bg"),
                BOLD,
            );
        }
        if self.help {
            self.draw_help(s);
        }
        if self.picker.is_some() {
            self.draw_picker(s);
        }
        self.draw_splash(s);
        self.collect_graphics();
        self.hover_pending = self
            .pointer
            .and_then(|(x, y)| s.hits.test(x, y))
            .is_some_and(|hit| {
                matches!(
                    hit.split(':').next().unwrap_or(""),
                    "border" | "brim" | "title" | "edge" | "corner"
                )
            });
    }
    fn draw_node(&mut self, s: &mut Screen, id: Id) {
        let n = self.layout.nodes[&id].clone();
        match n.kind {
            Kind::Leaf => {
                if n.collapsed {
                    if n.rect.w != 0 && n.rect.h != 0 {
                        self.draw_header(s, id);
                    }
                } else if !n.floating {
                    self.draw_pane(s, id);
                }
            }
            Kind::Split {
                direction,
                children,
            } => {
                let live: Vec<_> = children
                    .iter()
                    .copied()
                    .filter(|id| {
                        let n = &self.layout.nodes[id];
                        !n.collapsed && !n.floating
                    })
                    .collect();
                for (i, pair) in live.windows(2).enumerate() {
                    let a = self.layout.nodes[&pair[0]].rect;
                    let b = self.layout.nodes[&pair[1]].rect;
                    let r = match direction {
                        Direction::Cols => Rect {
                            x: a.x + a.w,
                            y: n.rect.y,
                            w: b.x.saturating_sub(a.x + a.w),
                            h: n.rect.h,
                        },
                        Direction::Rows => Rect {
                            x: n.rect.x,
                            y: a.y + a.h,
                            w: n.rect.w,
                            h: b.y.saturating_sub(a.y + a.h),
                        },
                    };
                    if r.w > 0 && r.h > 0 {
                        s.hits.add(r.x, r.y, r.w, r.h, &format!("edge:{id}:{i}"));
                    }
                }
                for child in children {
                    self.draw_node(s, child);
                }
            }
        }
    }
    fn draw_pane(&mut self, s: &mut Screen, id: Id) {
        let n = self.layout.nodes[&id].clone();
        let focused = self.layout.focus() == Some(id);
        if n.rect.w == 0 || n.rect.h == 0 {
            return;
        }
        s.hits.add(
            n.rect.x,
            n.rect.y,
            n.rect.w,
            n.rect.h,
            &format!("focus:{id}"),
        );
        s.hits.add(
            n.content.x,
            n.content.y,
            n.content.w,
            n.content.h,
            &format!("pane:{id}"),
        );
        self.draw_frame(s, id);
        let p = self.panes.get_mut(&id).unwrap();
        if p.suspended {
            for y in n.content.y..n.content.y.saturating_add(n.content.h) {
                for x in n.content.x..n.content.x.saturating_add(n.content.w) {
                    s.text(x, y, " ", NONE, NONE, 0);
                }
            }
            let native_label = String::from_utf8_lossy(&p.label);
            let label = if p.label.is_empty() {
                "shell"
            } else {
                native_label.as_ref()
            };
            let hint = clip(&format!("press a key to run: {label}"), n.content.w);
            s.text(
                n.content.x + (n.content.w - screen::cells(&hint)) / 2,
                n.content.y + n.content.h / 2,
                &hint,
                self.config.color("title_idle"),
                NONE,
                0,
            );
        } else {
            let _ = p.compose(s, n.content.x, n.content.y, focused);
        }
        self.draw_pane_status(s, id);
        self.shade(s, id, false, n.content, None);
        self.draw_search_marks(s, id);
        let mut chrome = n.rect;
        if self.compact_pane(id) && chrome.x > 0 && chrome.y > 0 {
            chrome.x -= 1;
            chrome.y -= 1;
            chrome.w = chrome.w.saturating_add(2);
            chrome.h = chrome.h.saturating_add(2);
        }
        self.shade(s, id, true, chrome, Some(n.content));
        self.draw_guides(s, id);
    }
    fn draw_pane_status(&self, s: &mut Screen, id: Id) {
        if self.draw_search_bar(s, id) {
            return;
        }
        let p = &self.panes[&id];
        let r = self.layout.nodes[&id].rect;
        let dead = !p.alive && !p.suspended;
        let status = if dead {
            p.exit_words().into_bytes()
        } else {
            p.status.clone()
        };
        if r.w < 6 || r.h < 3 || (!dead && status.is_empty() && p.buttons.is_empty()) {
            return;
        }
        let focused = self.layout.focus() == Some(id);
        let fg = self
            .config
            .color(if focused { "frame_focus" } else { "frame_idle" });
        let y = r.y + r.h - 1;
        let left = r.x + 1;
        let mut right = r.x + r.w - 1;
        let buttons: Vec<_> = if dead {
            vec![
                (b"re-run".as_slice(), format!("rerun:{id}")),
                (b"close".as_slice(), format!("close:{id}")),
            ]
        } else {
            p.buttons
                .iter()
                .map(|b| (b.label.as_slice(), format!("btn:{id}:{}", b.id)))
                .collect()
        };
        for (label, action) in buttons.iter().rev() {
            let w = screen::cells_bytes(label) + 2;
            if right < left + w + 1 {
                break;
            }
            right -= w + 1;
            let hot = self
                .pointer
                .is_some_and(|(x, py)| x >= right && x < right + w && py == y);
            s.text_bytes(
                right,
                y,
                &[b"[".as_slice(), label, b"]"].concat(),
                if focused || hot {
                    self.config.color("button_fg")
                } else {
                    fg
                },
                self.config.color(if focused || hot {
                    "button_bg"
                } else {
                    "button_bg_idle"
                }),
                if hot { BOLD } else { 0 },
            );
            s.hits.add(right, y, w, 1, action);
        }
        if !status.is_empty() && right > left + 1 {
            let w = s.text_bytes(
                left,
                y,
                &screen::clip_bytes(&[b" ".as_slice(), &status, b" "].concat(), right - left),
                self.config.color(if dead {
                    "dead"
                } else if focused {
                    "title"
                } else {
                    "title_idle"
                }),
                NONE,
                if dead { BOLD } else { 0 },
            );
            s.hits.add(left, y, w, 1, &format!("panestatus:{id}"));
        }
    }
    pub(super) fn state(&self, id: Id) -> Option<usize> {
        if let Some(drag) = self.drag.as_ref().filter(|d| d.tiled()) {
            return Some(if drag.source == id {
                0
            } else if drag.target == Some(id) {
                1
            } else {
                2
            });
        }
        let p = &self.panes[&id];
        let n = &self.layout.nodes[&id];
        if !p.alive && !p.suspended {
            Some(3)
        } else if p.suspended {
            Some(4)
        } else if p.bell {
            Some(5)
        } else if p.terminal.scrolled().unwrap_or(false) {
            Some(6)
        } else if n.floating {
            Some(7)
        } else if self.layout.focus() != Some(id) {
            Some(8)
        } else {
            None
        }
    }
    fn shade(&mut self, s: &mut Screen, id: Id, chrome: bool, r: Rect, hole: Option<Rect>) {
        let state = self.state(id);
        let now = Instant::now();
        let stamp = self.states.entry(id).or_insert((state, now));
        if stamp.0 != state {
            *stamp = (state, now);
        }
        let state_ms = stamp.1.elapsed().as_millis() as i64;
        let mut chain = if chrome {
            self.config.shaders.chrome.clone()
        } else {
            self.config.shaders.content.clone()
        };
        if let Some(own) = self.chains.get(&id) {
            chain.extend(
                if chrome { &own.chrome } else { &own.content }
                    .iter()
                    .cloned(),
            );
        }
        if let Some(state) = state {
            let policy = &self.config.states[state];
            chain.extend(
                if chrome {
                    &policy.chrome
                } else {
                    &policy.content
                }
                .iter()
                .cloned(),
            );
        }
        chain.truncate(shader::MAX_CHAIN);
        if chain.is_empty() {
            return;
        }
        let focused = self.layout.focus() == Some(id);
        let content = self.layout.nodes[&id].content;
        let (above, total) = self.panes[&id]
            .terminal
            .scroll_position()
            .unwrap_or_default();
        let ctx = Context {
            now_ms: self.started.elapsed().as_millis() as i64,
            state_ms,
            focused,
            has_cursor: !chrome
                && focused
                && s.cursor.visible
                && s.cursor.x >= content.x
                && s.cursor.y >= content.y,
            cursor_x: s.cursor.x.saturating_sub(content.x),
            cursor_y: s.cursor.y.saturating_sub(content.y),
            above,
            below: total.saturating_sub(above),
            default_fg: self.config.color("default_fg"),
            default_bg: self.config.color("default_bg"),
            ..Context::default()
        };
        shader::apply(s, &chain, r, hole, &ctx);
    }
    pub(super) fn compact_pane(&self, id: Id) -> bool {
        let n = &self.layout.nodes[&id];
        let t = &self.layout.tabs[self.layout.current];
        self.config.boolean("compact")
            && !n.floating
            && !n.collapsed
            && !t.flattened
            && t.zoom.is_none()
    }
    fn draw_frame(&self, s: &mut Screen, id: Id) {
        let n = &self.layout.nodes[&id];
        let r = n.rect;
        let compact = self.compact_pane(id);
        if r.w < 3
            || if compact {
                r.h == 0 || r.x == 0 || r.y == 0
            } else {
                r.h < 3
            }
        {
            return;
        }
        let focused = self.layout.focus() == Some(id);
        let fg = self
            .config
            .color(if focused { "frame_focus" } else { "frame_idle" });
        let dragged = self.drag.as_ref().filter(|d| d.tiled());
        let target = dragged.is_some_and(|d| d.target == Some(id));
        let dashed = dragged.is_some_and(|d| d.source != id) && !n.floating;
        let fg = if target {
            self.config.color("drop_target")
        } else {
            fg
        };
        let attrs = if target { BOLD } else { 0 };
        let hbar = if dashed { "┄" } else { "─" };
        let vbar = if dashed { "┆" } else { "│" };
        let x1 = r.x + r.w - 1;
        let y1 = r.y + r.h - 1;
        let ty = if compact { r.y - 1 } else { r.y };
        if !compact {
            let corners = if self.config.boolean("rounded") {
                ["╭", "╮", "╰", "╯"]
            } else {
                ["┌", "┐", "└", "┘"]
            };
            for ((x, y), glyph) in [(r.x, r.y), (x1, r.y), (r.x, y1), (x1, y1)]
                .into_iter()
                .zip(corners)
            {
                s.text(x, y, glyph, fg, NONE, attrs);
            }
            for x in r.x + 1..x1 {
                s.text(x, r.y, hbar, fg, NONE, attrs);
                s.text(x, y1, hbar, fg, NONE, attrs);
            }
            for y in r.y + 1..y1 {
                s.text(r.x, y, vbar, fg, NONE, attrs);
                s.text(x1, y, vbar, fg, NONE, attrs);
                for x in r.x + 1..x1 {
                    if x < n.content.x
                        || x >= n.content.x + n.content.w
                        || y < n.content.y
                        || y >= n.content.y + n.content.h
                    {
                        s.text(x, y, " ", NONE, NONE, 0);
                    }
                }
            }
        }
        let outer = self.layout.area(self.cols, self.rows, &self.config);
        let top_outer = !compact || r.y == outer.y;
        if top_outer {
            s.hits.add(r.x, ty, r.w, 1, &format!("title:{id}"));
        }
        let mut avail = r.w - 2;
        let mut right_lo = x1;
        if self.config.boolean("pane_buttons") && avail > 10 {
            let zoom = self.layout.tabs[self.layout.current].zoom == Some(id);
            let mut bx = x1 - 1;
            let mut first = None;
            for (setting, verb) in [
                ("close_mark", "close"),
                (if zoom { "zoom_on_mark" } else { "zoom_mark" }, "zoom"),
                ("min_mark", "minimize"),
            ] {
                let mark = self.config.text_bytes(setting).unwrap_or_default();
                let bw = screen::cells_bytes(mark).max(1) + 1;
                if bx < r.x + 4 + bw {
                    break;
                }
                let px = bx - bw + 1;
                let hot = self
                    .pointer
                    .is_some_and(|(x, y)| x >= px && x < px + bw && y == ty);
                s.text_bytes(
                    px,
                    ty,
                    &[mark, b" "].concat(),
                    self.config.color(if hot {
                        "pane_button_hover"
                    } else {
                        "pane_button"
                    }),
                    NONE,
                    if hot { BOLD } else { 0 },
                );
                s.hits.add(px, ty, bw, 1, &format!("{verb}:{id}"));
                bx = px - 1;
                first = Some(px);
                avail = avail.saturating_sub(bw);
            }
            if let Some(px) = first {
                right_lo = px;
                if px > r.x + 1 {
                    right_lo -= 1;
                    s.text(right_lo, ty, " ", self.config.color("pane_button"), NONE, 0);
                    avail = avail.saturating_sub(1);
                }
            }
        }
        if self.config.boolean("bell_indicator") && self.panes[&id].bell && r.w > 4 {
            s.text_bytes(
                r.x + 1,
                ty,
                self.config.text_bytes("bell_mark").unwrap_or(b"[!]"),
                self.config.color("bell"),
                NONE,
                BOLD,
            );
        }
        if self.panes[&id].terminal.scrolled().unwrap_or(false) && avail >= 8 {
            let (above, total) = self.panes[&id]
                .terminal
                .scroll_position()
                .unwrap_or_default();
            let ind = format!(" ▲ {} ", total.saturating_sub(above));
            let w = screen::cells(&ind);
            if w + 2 < avail {
                let x = right_lo.saturating_sub(w);
                s.text(
                    x,
                    ty,
                    &ind,
                    self.config.color("scroll_fg"),
                    self.config.color("scroll_bg"),
                    BOLD,
                );
                s.hits.add(x, ty, w, 1, &format!("scrollbottom:{id}"));
                right_lo = x;
                avail -= w;
            }
        }
        let edit = self
            .edit
            .as_ref()
            .filter(|e| e.id == id && e.target != crate::bindings::Action::RenameTab);
        let shown = edit.map(|e| {
            format!(
                "{}{}█",
                if e.target == crate::bindings::Action::SetPurpose {
                    "purpose "
                } else {
                    ""
                },
                e.text
            )
        });
        let native_title = self.panes[&id].title();
        let title = shown.as_deref().unwrap_or(native_title.as_ref());
        let mut title_span = None;
        if !title.is_empty() && avail >= 3 {
            let label = clip(&format!(" {title} "), avail);
            let len = screen::cells(&label);
            let inset = (self.config.number("title_inset") as u16).min(avail.saturating_sub(1));
            let tx = match self.config.text("title_align") {
                Some("left") => r.x + 1 + inset,
                Some("right") => r.x + 1 + avail - len - inset.min(avail - len),
                _ => r.x + 1 + (avail - len) / 2,
            };
            s.text(
                tx,
                ty,
                &label,
                self.config
                    .color(if focused { "title" } else { "title_idle" }),
                NONE,
                if focused { BOLD } else { 0 },
            );
            s.hits.add(tx, ty, len, 1, &format!("panetitle:{id}"));
            title_span = Some((tx, tx + len));
        }
        for side in ['l', 'r', 'b'] {
            let outside = !compact
                || match side {
                    'l' => r.x == outer.x,
                    'r' => r.x + r.w == outer.x + outer.w,
                    _ => r.y + r.h == outer.y + outer.h,
                };
            if !outside {
                continue;
            }
            let brim = match side {
                'l' => Rect {
                    x: if compact { r.x - 1 } else { r.x },
                    y: r.y + u16::from(!compact),
                    w: 1,
                    h: r.h.saturating_sub(if compact { 0 } else { 2 }),
                },
                'r' => Rect {
                    x: if compact { r.x + r.w } else { x1 },
                    y: r.y + u16::from(!compact),
                    w: 1,
                    h: r.h.saturating_sub(if compact { 0 } else { 2 }),
                },
                _ => Rect {
                    x: r.x,
                    y: if compact { r.y + r.h } else { y1 },
                    w: r.w,
                    h: 1,
                },
            };
            s.hits
                .add(brim.x, brim.y, brim.w, brim.h, &format!("brim:{id}:{side}"));
            let span = if side == 'b' { r.w } else { r.h };
            let span = span.saturating_sub(if compact { 0 } else { 2 });
            if r.w >= 4 && r.h >= if compact { 3 } else { 4 } && span >= 3 {
                let len = (span / 3).clamp(3, 7);
                let off = (span - len) / 2;
                let h = if side == 'b' {
                    Rect {
                        x: r.x + u16::from(!compact) + off,
                        y: brim.y,
                        w: len,
                        h: 1,
                    }
                } else {
                    Rect {
                        x: brim.x,
                        y: brim.y + off,
                        w: 1,
                        h: len,
                    }
                };
                s.hits
                    .add(h.x, h.y, h.w, h.h, &format!("border:{id}:{side}"));
            }
        }
        if top_outer {
            let lo = r.x + 1 + u16::from(self.panes[&id].bell);
            let hi = right_lo.saturating_sub(1);
            let mid = r.x + r.w / 2;
            let mut best: Option<(u16, u16, u16)> = None;
            let runs = if let Some((tl, th)) = title_span {
                vec![(lo, hi.min(tl.saturating_sub(1))), (lo.max(th), hi)]
            } else {
                vec![(lo, hi)]
            };
            for (lo, hi) in runs {
                if hi < lo || hi - lo + 1 < 3 {
                    continue;
                }
                let len = (hi - lo + 1).min(7);
                let start = mid.saturating_sub(len / 2).clamp(lo, hi - len + 1);
                let distance = (start + len / 2).abs_diff(mid);
                if best.is_none_or(|(_, old_len, old_d)| {
                    distance < old_d || (distance == old_d && len > old_len)
                }) {
                    best = Some((start, len, distance));
                }
            }
            if let Some((x, w, _)) = best {
                s.hits.add(x, ty, w, 1, &format!("title:{id}:t"));
            }
        }
    }
    fn draw_header(&mut self, s: &mut Screen, id: Id) {
        let r = self.layout.nodes[&id].rect;
        if r.w < 4 {
            return;
        }
        let p = &self.panes[&id];
        let native_title = p.title();
        let title = if native_title.is_empty() {
            "pane"
        } else {
            native_title.as_ref()
        };
        let status = if !p.alive && !p.suspended {
            p.exit_words().into_bytes()
        } else if p.suspended {
            b"not started".to_vec()
        } else {
            p.status.clone()
        };
        let mut label = format!(" {title}").into_bytes();
        if !status.is_empty() {
            label.extend_from_slice(" · ".as_bytes());
            label.extend_from_slice(&status);
        }
        label.push(b' ');
        let hot = self
            .pointer
            .is_some_and(|(x, y)| x >= r.x && x < r.x + r.w && y == r.y);
        let fg = self
            .config
            .color(if hot { "header_hover" } else { "header" });
        for x in r.x..r.x + r.w {
            s.text(x, r.y, "─", fg, NONE, 0);
        }
        s.text(
            r.x,
            r.y,
            if self.config.boolean("rounded") {
                "╭"
            } else {
                "┌"
            },
            fg,
            NONE,
            0,
        );
        s.text(
            r.x + r.w - 1,
            r.y,
            if self.config.boolean("rounded") {
                "╮"
            } else {
                "┐"
            },
            fg,
            NONE,
            0,
        );
        let tx = r.x + 1 + self.config.number("title_inset") as u16;
        if tx < r.x + r.w - 1 {
            s.text_bytes(
                tx,
                r.y,
                &screen::clip_bytes(&label, r.x + r.w - 1 - tx),
                self.config.color(if hot {
                    "header_hover_title"
                } else if !p.alive {
                    "dead"
                } else {
                    "header"
                }),
                NONE,
                if hot { BOLD } else { 0 },
            );
        }
        s.hits.add(r.x, r.y, r.w, 1, &format!("focus:{id}"));
        self.shade(s, id, true, r, None);
    }
    fn draw_minimized(&mut self, s: &mut Screen, root: Id) {
        let bar = self.layout.tabs[self.layout.current].min_bar;
        if bar.w == 0 {
            return;
        }
        let mut x = bar.x;
        let right = bar.x + bar.w;
        let corners = if self.config.boolean("rounded") {
            ["╭", "╮", "╰", "╯"]
        } else {
            ["┌", "┐", "└", "┘"]
        };
        for id in self.layout.leaves(root) {
            if !self.layout.nodes[&id].minimized {
                continue;
            }
            let p = &self.panes[&id];
            let native_title = p.title();
            let title = if native_title.is_empty() {
                "pane"
            } else {
                native_title.as_ref()
            };
            let label = [
                b" ".as_slice(),
                self.config.text_bytes("min_mark").unwrap_or("▬".as_bytes()),
                b" ",
                title.as_bytes(),
            ]
            .concat();
            let bell = if self.config.boolean("bell_indicator") && p.bell {
                [
                    b" ".as_slice(),
                    self.config.text_bytes("bell_mark").unwrap_or(b"[!]"),
                ]
                .concat()
            } else {
                Vec::new()
            };
            let w = screen::cells_bytes(&label) + screen::cells_bytes(&bell) + 3;
            if x + w > right {
                break;
            }
            let hot = self
                .pointer
                .is_some_and(|(px, py)| px >= x && px < x + w && py >= bar.y && py < bar.y + 3);
            let fg = self
                .config
                .color(if hot { "minbar_hover" } else { "minbar" });
            let attrs = if hot { BOLD } else { 0 };
            let x1 = x + w - 1;
            let y1 = bar.y + 2;
            for ((px, py), glyph) in [(x, bar.y), (x1, bar.y), (x, y1), (x1, y1)]
                .into_iter()
                .zip(corners)
            {
                s.text(px, py, glyph, fg, NONE, attrs);
            }
            for px in x + 1..x1 {
                s.text(px, bar.y, "─", fg, NONE, attrs);
                s.text(px, y1, "─", fg, NONE, attrs);
                s.text(px, bar.y + 1, " ", fg, NONE, attrs);
            }
            s.text(x, bar.y + 1, "│", fg, NONE, attrs);
            s.text(x1, bar.y + 1, "│", fg, NONE, attrs);
            let lw = s.text_bytes(x + 1, bar.y + 1, &label, fg, NONE, attrs);
            s.text_bytes(
                x + 1 + lw,
                bar.y + 1,
                &bell,
                self.config.color("bell"),
                NONE,
                BOLD,
            );
            s.hits.add(x, bar.y, w, 3, &format!("focus:{id}"));
            x += w + 1;
        }
    }
    fn draw_tabs(&self, s: &mut Screen) {
        // A top strip is also the fallback when the configured sidebar cannot fit.
        let y = if self.config.boolean("compact") {
            0
        } else {
            self.config.number("gap") as u16
        };
        let mut x = self.config.number("status_pad") as u16;
        let mut right = s.cols.saturating_sub(x);
        let count = self.panes.len();
        let label = format!("{count} pane{}", if count == 1 { "" } else { "s" });
        let w = screen::cells(&label);
        if right > w + 4 {
            s.text(
                right - w,
                y,
                &label,
                self.config.color("tab_count"),
                NONE,
                0,
            );
            right = right.saturating_sub(w + 1);
        }
        if self.prefix {
            let prefix = self.config.bindings.prefix.display();
            let w = screen::cells(&prefix);
            if right > w + 2 {
                s.text(
                    right - w,
                    y,
                    &prefix,
                    self.config.color("prefix_fg"),
                    self.config.color("prefix_bg"),
                    BOLD,
                );
                right -= w + 1;
            }
        }
        for (i, t) in self.layout.tabs.iter().enumerate() {
            let p = &self.panes[&t.focus];
            let cwd = p.live_cwd();
            let derived = cwd
                .as_ref()
                .map(|p| {
                    if std::env::var_os("HOME").is_some_and(|h| p.as_os_str() == h) {
                        "~".into()
                    } else {
                        p.file_name().map_or_else(
                            || p.to_string_lossy().into_owned(),
                            |s| s.to_string_lossy().into_owned(),
                        )
                    }
                })
                .unwrap_or_default();
            let native_name = String::from_utf8_lossy(&t.name);
            let name = if !t.name.is_empty() {
                native_name.as_ref()
            } else if !t.purpose.is_empty() {
                &t.purpose
            } else {
                &derived
            };
            let label = if let Some(e) = self
                .edit
                .as_ref()
                .filter(|e| e.target == crate::bindings::Action::RenameTab && e.id == t.id)
            {
                format!(" {}█ ", e.text)
            } else if name.is_empty() {
                format!(" {} ", i + 1)
            } else {
                format!(" {}:{name} ", i + 1)
            };
            let bell = self.config.boolean("bell_indicator")
                && self
                    .layout
                    .leaves(t.root)
                    .iter()
                    .any(|id| self.panes[id].bell);
            let mark = if bell {
                [
                    self.config.text_bytes("bell_mark").unwrap_or_default(),
                    b" ",
                ]
                .concat()
            } else {
                Vec::new()
            };
            let w = screen::cells(&label) + screen::cells_bytes(&mark);
            if x + w >= right {
                break;
            }
            let active = i == self.layout.current;
            let hot = self
                .pointer
                .is_some_and(|(px, py)| px >= x && px < x + w && py == y);
            let fg = if active {
                if hot {
                    "tab_active_hover_fg"
                } else {
                    "tab_active_fg"
                }
            } else if hot {
                "tab_hover"
            } else {
                "tab_idle"
            };
            s.text(
                x,
                y,
                &label,
                self.config.color(fg),
                self.config.color(if active {
                    "tab_active_bg"
                } else {
                    "tab_idle_bg"
                }),
                if active { BOLD } else { 0 },
            );
            s.hits.add(x, y, w, 1, &format!("tab:{}", t.id));
            if bell {
                s.text_bytes(
                    x + screen::cells(&label),
                    y,
                    &mark,
                    self.config.color("bell"),
                    self.config.color(if active {
                        "tab_active_bg"
                    } else {
                        "tab_idle_bg"
                    }),
                    BOLD,
                );
            }
            x += w;
        }
        if self.config.boolean("newtab_button") {
            let mark = self.config.text_bytes("newtab_mark").unwrap_or_default();
            let text = [b" ".as_slice(), mark, b" "].concat();
            let w = screen::cells_bytes(&text);
            let pad = self.config.number("newtab_pad") as u16;
            if !mark.is_empty() && x + pad + w < right {
                x += pad;
                s.text_bytes(
                    x,
                    y,
                    &text,
                    self.config.color(
                        if self
                            .pointer
                            .is_some_and(|(px, py)| px >= x && px < x + w && py == y)
                        {
                            "tab_hover"
                        } else {
                            "tab_idle"
                        },
                    ),
                    NONE,
                    0,
                );
                s.hits.add(x, y, w, 1, "newtab");
            }
        }
    }
    fn draw_status(&self, s: &mut Screen) {
        if s.rows < 3 {
            return;
        }
        let y = s
            .rows
            .saturating_sub(if self.config.boolean("compact") {
                0
            } else {
                self.config.number("gap") as u16
            })
            .saturating_sub(1);
        let x = self.config.number("status_pad") as u16;
        let mut right = s.cols.saturating_sub(x);
        if right <= x {
            return;
        }
        let tab = &self.layout.tabs[self.layout.current];
        let leaves = self.layout.leaves(tab.root);
        let focus = self.layout.focus();
        let mut used = right;
        let count = if let Some(i) = leaves.iter().position(|id| Some(*id) == focus) {
            format!("pane {}/{}", i + 1, leaves.len())
        } else {
            format!("{} panes", leaves.len())
        };
        let w = screen::cells(&count);
        if right > x + w + 2 {
            s.text(right - w, y, &count, self.config.color("status"), NONE, 0);
            used = right - w;
            right = used.saturating_sub(2);
        }
        let mut labels = Vec::<std::borrow::Cow<'_, str>>::new();
        let session_label = self
            .session
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string_lossy());
        if let Some(name) = session_label.as_deref() {
            labels.push(name.into());
        }
        let native_name = String::from_utf8_lossy(&tab.name);
        if !tab.name.is_empty() {
            labels.push(native_name);
        } else if !tab.purpose.is_empty() {
            labels.push(tab.purpose.as_str().into());
        }
        if let Some(id) = focus {
            let p = &self.panes[&id];
            let n = &self.layout.nodes[&id];
            let status = if p.suspended {
                "not started".into()
            } else if !p.alive {
                p.exit_words()
            } else if tab.zoom == Some(id) {
                "zoomed".into()
            } else if p.terminal.scrolled().unwrap_or(false) {
                let (above, total) = p.terminal.scroll_position().unwrap_or_default();
                format!("▲ {} scrolled", total.saturating_sub(above))
            } else if p.terminal.alt_screen() {
                "alt screen".into()
            } else {
                String::new()
            };
            let w = screen::cells(&status);
            if !status.is_empty() && right > x + w + 2 {
                s.text(
                    right - w,
                    y,
                    &status,
                    self.config.color("status_state"),
                    NONE,
                    BOLD,
                );
                used = right - w;
                right = used.saturating_sub(2);
            }
            if !n.purpose.is_empty() {
                labels.push(n.purpose.as_str().into());
            } else if !p.title().is_empty() {
                labels.push(p.title());
            }
        }
        let left = clip(&labels.join(" · "), right.saturating_sub(x));
        let lw = s.text(x, y, &left, self.config.color("status"), NONE, 0);
        let hint = if self.config.boolean("hints") {
            self.pointer
                .and_then(|(x, y)| s.hits.test(x, y))
                .and_then(|hit| self.hint_for(hit))
        } else {
            None
        };
        let middle = hint.or_else(|| {
            self.config.boolean("version_banner").then_some(concat!(
                "slosh ",
                env!("CARGO_PKG_VERSION"),
                " (slush)"
            ))
        });
        if let Some(middle) = middle {
            let w = screen::cells(middle);
            let from = s.cols.saturating_sub(w) / 2;
            if from > x + lw && from + w < used {
                s.text(
                    from,
                    y,
                    middle,
                    self.config
                        .color(if hint.is_some() { "hint" } else { "status" }),
                    NONE,
                    0,
                );
            }
        }
    }
    fn compact_lines(&self, s: &mut Screen, root: Id) {
        let mut bits = vec![0u8; s.cells.len()];
        let mut focused = vec![false; s.cells.len()];
        let tab = &self.layout.tabs[self.layout.current];
        if tab.flattened || tab.zoom.is_some() {
            return;
        }
        let area = self.layout.area(self.cols, self.rows, &self.config);
        if area.x == 0 || area.y == 0 {
            return;
        }
        let (x0, y0, x1, y1) = (area.x - 1, area.y - 1, area.x + area.w, area.y + area.h);
        let mut add = |x, y, b| {
            if let Some(i) = s.index(x, y) {
                bits[i] |= b;
            }
        };
        add(x0, y0, 6);
        add(x1, y0, 10);
        add(x0, y1, 5);
        add(x1, y1, 9);
        for x in x0 + 1..x1 {
            add(x, y0, 12);
            add(x, y1, 12);
        }
        for y in y0 + 1..y1 {
            add(x0, y, 3);
            add(x1, y, 3);
        }
        for id in self.layout.leaves(root) {
            let n = &self.layout.nodes[&id];
            if n.collapsed || n.floating || n.rect.x == 0 || n.rect.y == 0 {
                continue;
            }
            let r = n.rect;
            let x0 = r.x - 1;
            let x1 = r.x + r.w;
            let y0 = r.y - 1;
            let y1 = r.y + r.h;
            let mut add = |x, y, b| {
                if let Some(i) = s.index(x, y) {
                    bits[i] |= b;
                    focused[i] |= self.layout.focus() == Some(id);
                }
            };
            add(x0, y0, 6);
            add(x1, y0, 10);
            add(x0, y1, 5);
            add(x1, y1, 9);
            for x in x0 + 1..x1 {
                add(x, y0, 12);
                add(x, y1, 12);
            }
            for y in y0 + 1..y1 {
                add(x0, y, 3);
                add(x1, y, 3);
            }
        }
        for (i, b) in bits.into_iter().enumerate() {
            if b == 0 {
                continue;
            }
            let cell = &s.cells[i];
            let have =
                match std::str::from_utf8(&cell.text[..cell.len as usize]).unwrap_or_default() {
                    "│" | "┆" => 3,
                    "─" | "┄" => 12,
                    "┌" | "╭" => 6,
                    "┐" | "╮" => 10,
                    "└" | "╰" => 5,
                    "┘" | "╯" => 9,
                    "├" => 7,
                    "┤" => 11,
                    "┬" => 14,
                    "┴" => 13,
                    "┼" => 15,
                    "╵" => 1,
                    "╷" => 2,
                    "╴" => 4,
                    "╶" => 8,
                    " " => 0,
                    _ => continue,
                };
            let b = b | have;
            let glyph = match b {
                1 => "╵",
                2 => "╷",
                3 => "│",
                4 => "╴",
                8 => "╶",
                12 => "─",
                6 => {
                    if self.config.boolean("rounded") {
                        "╭"
                    } else {
                        "┌"
                    }
                }
                10 => {
                    if self.config.boolean("rounded") {
                        "╮"
                    } else {
                        "┐"
                    }
                }
                5 => {
                    if self.config.boolean("rounded") {
                        "╰"
                    } else {
                        "└"
                    }
                }
                9 => {
                    if self.config.boolean("rounded") {
                        "╯"
                    } else {
                        "┘"
                    }
                }
                7 => "├",
                11 => "┤",
                14 => "┬",
                13 => "┴",
                15 => "┼",
                _ => continue,
            };
            s.text(
                (i % s.cols as usize) as u16,
                (i / s.cols as usize) as u16,
                glyph,
                self.config.color(if focused[i] {
                    "frame_focus"
                } else {
                    "frame_idle"
                }),
                NONE,
                0,
            );
        }
    }
}
