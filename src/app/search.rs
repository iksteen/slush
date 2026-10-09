//! Inline scrollback query: the terminal owns matches and viewport tracking.
use super::App;
use crate::{
    ghostty::ffi::*,
    input::{CTRL, Event},
    layout::Id,
    screen::{self, BOLD, Color, INVERSE, Screen},
};

pub(super) struct Search {
    id: Id,
    query: String,
}

impl App {
    pub(super) fn search_begin(&mut self) {
        if let Some(id) = self.layout.focus() {
            self.search = Some(Search {
                id,
                query: String::new(),
            });
        }
    }
    fn search_end(&mut self, keep: bool) {
        if let Some(search) = self.search.take()
            && let Some(pane) = self.panes.get_mut(&search.id)
        {
            let _ = pane.terminal.search_set("");
            if !keep {
                pane.terminal.scroll_edge(false);
            }
        }
    }
    pub(super) fn search_refresh(&mut self) {
        if let Some(search) = &self.search {
            if let Some(pane) = self.panes.get_mut(&search.id) {
                let _ = pane.terminal.search_refresh();
            } else {
                self.search = None;
            }
        }
    }
    pub(super) fn search_event(&mut self, event: &Event, screen: &Screen) -> bool {
        let Some(search) = &mut self.search else {
            return false;
        };
        let Some(pane) = self.panes.get_mut(&search.id) else {
            self.search = None;
            return matches!(event, Event::Key { .. });
        };
        let Event::Key {
            key,
            mods,
            action,
            text,
            ..
        } = event
        else {
            if let Event::Mouse {
                button,
                action: 0,
                x,
                y,
                ..
            } = event
                && !matches!(button, 4 | 5)
                && screen.hits.test(*x, *y) != Some(format!("search:{}", search.id).as_str())
            {
                self.search_end(true);
            }
            return false;
        };
        if *action == 2 {
            return true;
        }
        let ctrl = *mods & CTRL != 0;
        match *key {
            GHOSTTY_KEY_ESCAPE => self.search_end(false),
            GHOSTTY_KEY_ENTER => self.search_end(true),
            GHOSTTY_KEY_ARROW_UP | GHOSTTY_KEY_N if *key == GHOSTTY_KEY_ARROW_UP || ctrl => {
                let _ = pane.terminal.search_step(true);
            }
            GHOSTTY_KEY_ARROW_DOWN | GHOSTTY_KEY_P if *key == GHOSTTY_KEY_ARROW_DOWN || ctrl => {
                let _ = pane.terminal.search_step(false);
            }
            _ => {
                if ctrl && *key == GHOSTTY_KEY_U {
                    search.query.clear();
                } else if *key == GHOSTTY_KEY_BACKSPACE {
                    search.query.pop();
                } else if !ctrl
                    && text.first().is_some_and(|c| *c >= 32)
                    && search.query.len() + text.len() < 128
                {
                    search.query.push_str(&String::from_utf8_lossy(text));
                } else {
                    return true;
                }
                let _ = pane.terminal.search_set(&search.query);
            }
        }
        true
    }
    pub(super) fn draw_search_bar(&self, s: &mut Screen, id: Id) -> bool {
        let Some(search) = self.search.as_ref().filter(|q| q.id == id) else {
            return false;
        };
        let r = self.layout.nodes[&id].rect;
        if r.w < 8 || r.h < 3 {
            return true;
        }
        let y = r.y + r.h - 1;
        let left = r.x + 1;
        let right = r.x + r.w - 1;
        let mut cx = right;
        let none = Color::default();
        if !search.query.is_empty() {
            let (index, total) = self.panes[&id].terminal.search_position();
            let count = if let Some(index) = index {
                format!(" {} of {total} ", index + 1)
            } else if total > 0 {
                format!(" {total} ")
            } else {
                " no matches ".into()
            };
            let width = screen::cells(&count);
            if left + width + 6 < right {
                cx = right - width;
                s.text(
                    cx,
                    y,
                    &count,
                    self.config.color(if total > 0 { "hint" } else { "dead" }),
                    none,
                    if total > 0 { 0 } else { BOLD },
                );
            }
        }
        let mut field = format!(" / {}█ ", search.query);
        let avail = cx.saturating_sub(left);
        while screen::cells(&field) > avail && !field.is_empty() {
            field.remove(0);
        }
        let width = s.text(
            left,
            y,
            &field,
            self.config.color("rename_fg"),
            self.config.color("rename_bg"),
            0,
        );
        s.hits.add(left, y, width, 1, &format!("search:{id}"));
        true
    }
    pub(super) fn draw_search_marks(&self, s: &mut Screen, id: Id) {
        if !self.search.as_ref().is_some_and(|q| q.id == id) {
            return;
        }
        let r = self.layout.nodes[&id].content;
        for mark in self.panes[&id].terminal.search_marks(r.w, r.h) {
            let fg = self.config.color(if mark.current {
                "search_cur_fg"
            } else {
                "search_fg"
            });
            let bg = self.config.color(if mark.current {
                "search_cur_bg"
            } else {
                "search_bg"
            });
            for x in mark.x0..=mark.x1.min(r.w.saturating_sub(1)) {
                if let Some(cell) = s.at_mut(r.x + x, r.y + mark.row) {
                    cell.fg = fg;
                    cell.bg = bg;
                    cell.attrs &= !INVERSE;
                    if mark.current {
                        cell.attrs |= BOLD;
                    }
                }
            }
        }
    }
}
