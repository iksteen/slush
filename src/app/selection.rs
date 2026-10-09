//! Primary selection and pointer-driven copy, including held edge scrolling.
use super::App;
use crate::{
    input::{Event, SHIFT},
    layout::Id,
    screen::Screen,
};
use std::time::{Duration, Instant};

pub(super) struct Drag {
    id: Id,
    due: Instant,
}

impl App {
    pub(super) fn set_clipboard(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        self.toast(&format!(
            "copied {} char{}",
            text.len(),
            if text.len() == 1 { "" } else { "s" }
        ));
        self.clipboard_pending = Some(text.clone());
        self.clipboard = text;
    }
    fn selection_extend(&mut self, id: Id, x: u16, y: u16) {
        if let Some(n) = self.layout.nodes.get(&id) {
            let r = n.content;
            if r.w > 0
                && r.h > 0
                && let Some(p) = self.panes.get_mut(&id)
            {
                let _ = p.terminal.selection_extend(
                    x.saturating_sub(r.x).min(r.w - 1),
                    y.saturating_sub(r.y).min(r.h - 1),
                );
            }
        }
    }
    pub(super) fn selection_event(&mut self, event: &Event, s: &Screen) -> bool {
        if self.help {
            return false;
        }
        let Event::Mouse {
            button,
            action,
            x,
            y,
            mods,
        } = event
        else {
            return false;
        };
        self.pointer = Some((*x, *y));
        if let Some(drag) = &self.select_drag {
            let id = drag.id;
            if *action == 2 || *action == 3 {
                self.selection_extend(id, *x, *y);
                return true;
            }
            if *action == 1 {
                // A press alone leaves no selection; a release does not extend it.
                self.select_drag = None;
                if let Some(p) = self.panes.get(&id)
                    && let Ok(text) = p.terminal.selection_text()
                {
                    self.set_clipboard(text);
                }
                self.pressed = None;
                return true;
            }
        }
        if *action != 0 {
            return false;
        }
        let Some(id) = s
            .hits
            .test(*x, *y)
            .and_then(|h| h.strip_prefix("pane:"))
            .and_then(|id| id.parse::<Id>().ok())
        else {
            return false;
        };
        let Some(p) = self.panes.get_mut(&id) else {
            return false;
        };
        if *button == 3 && !p.terminal.wants_mouse() && !self.clipboard.is_empty() {
            let _ = p.input(&Event::Paste(self.clipboard.as_bytes().to_vec()));
            return true;
        }
        if *button != 1 || (p.terminal.wants_mouse() && *mods & SHIFT == 0) {
            return false;
        }
        let r = self.layout.nodes[&id].content;
        let (lx, ly) = (x.saturating_sub(r.x), y.saturating_sub(r.y));
        let now = Instant::now();
        let double = self.cell_click.is_some_and(|(old, cx, cy, at)| {
            old == id
                && cx == lx
                && cy == ly
                && now.duration_since(at)
                    <= Duration::from_millis(self.config.number("double_click_ms"))
        });
        self.cell_click = Some((id, lx, ly, now));
        if p.terminal.selection_start(lx, ly).is_ok() {
            if double {
                let _ = p.terminal.selection_word(
                    lx,
                    ly,
                    p.size.cols,
                    p.size.rows,
                    self.config
                        .text_bytes("word_separators")
                        .unwrap_or_default(),
                );
            }
            self.select_drag = Some(Drag { id, due: now });
        }
        self.layout.focus_pane(id);
        self.sync_layout();
        true
    }
    fn selection_over(&self) -> Option<(Id, i64)> {
        let drag = self.select_drag.as_ref()?;
        let (_, y) = self.pointer?;
        let r = self.layout.nodes.get(&drag.id)?.content;
        if r.h == 0 {
            return None;
        }
        let local = y as i64 - r.y as i64;
        let over = if local < 0 {
            local
        } else if local >= r.h as i64 {
            local - r.h as i64 + 1
        } else {
            0
        };
        (over != 0).then_some((drag.id, over))
    }
    pub(super) fn selection_deadline(&self) -> Option<i64> {
        self.selection_over()?;
        Some(
            self.select_drag
                .as_ref()?
                .due
                .saturating_duration_since(Instant::now())
                .as_millis() as i64,
        )
    }
    pub(super) fn selection_tick(&mut self) {
        let Some((id, over)) = self.selection_over() else {
            return;
        };
        let drag = self.select_drag.as_mut().unwrap();
        let now = Instant::now();
        if now < drag.due {
            return;
        }
        drag.due = now + Duration::from_millis(self.config.number("select_scroll_ms").max(1));
        if let Some(p) = self.panes.get_mut(&id) {
            p.terminal.scroll(over);
        }
        if let Some((x, y)) = self.pointer {
            self.selection_extend(id, x, y);
        }
    }
}
