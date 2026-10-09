//! Edits use the visible floating rectangle and spend motion at a wall.
use super::App;
use crate::{
    layout::{Id, size},
    shader::Rect,
};

pub(super) const LEFT: u8 = 1;
pub(super) const RIGHT: u8 = 2;
pub(super) const TOP: u8 = 4;
pub(super) const BOTTOM: u8 = 8;
pub(super) fn edges(r: Rect, x: u16, y: u16) -> u8 {
    let mut e = 0;
    if x <= r.x {
        e |= LEFT;
    } else if x >= r.x + r.w.saturating_sub(1) {
        e |= RIGHT;
    }
    if y <= r.y {
        e |= TOP;
    } else if y >= r.y + r.h.saturating_sub(1) {
        e |= BOTTOM;
    }
    e
}
impl App {
    pub(super) fn float_editable(&self, id: Id) -> bool {
        self.layout
            .nodes
            .get(&id)
            .is_some_and(|n| n.floating && !n.minimized && !n.hidden && !n.collapsed)
            && self
                .layout
                .tabs
                .get(self.layout.current)
                .is_some_and(|t| !t.flattened && t.zoom.is_none())
    }
    fn float_apply(&mut self, id: Id, rect: Rect) {
        self.layout.nodes.get_mut(&id).unwrap().float_rect = rect;
        self.sync_layout();
        if self.float_editable(id) {
            let n = self.layout.nodes.get_mut(&id).unwrap();
            n.float_rect = n.rect;
        }
    }
    pub(super) fn float_move(&mut self, id: Id, dx: i32, dy: i32) {
        if !self.float_editable(id) || (dx == 0 && dy == 0) {
            return;
        }
        let mut r = self.layout.nodes[&id].rect;
        r.x = (i32::from(r.x) + dx).clamp(0, u16::MAX as i32) as u16;
        r.y = (i32::from(r.y) + dy).clamp(0, u16::MAX as i32) as u16;
        self.float_apply(id, r);
    }
    pub(super) fn float_resize(&mut self, id: Id, edges: u8, dx: i32, dy: i32) {
        if !self.float_editable(id) || edges == 0 || (dx == 0 && dy == 0) {
            return;
        }
        let r = self.layout.nodes[&id].rect;
        let area = self.layout.area(self.cols, self.rows, &self.config);
        let (mw, mh) = size(&self.config, "min_pane");
        let (mut l, mut t, mut right, mut b) = (
            i32::from(r.x),
            i32::from(r.y),
            i32::from(r.x) + i32::from(r.w),
            i32::from(r.y) + i32::from(r.h),
        );
        let mw = i32::from(mw.min(area.w));
        let mh = i32::from(mh.min(area.h));
        if edges & LEFT != 0 {
            l = (l + dx).clamp(i32::from(area.x), right - mw);
        }
        if edges & RIGHT != 0 {
            right = (right + dx).clamp(l + mw, i32::from(area.x) + i32::from(area.w));
        }
        if edges & TOP != 0 {
            t = (t + dy).clamp(i32::from(area.y), b - mh);
        }
        if edges & BOTTOM != 0 {
            b = (b + dy).clamp(t + mh, i32::from(area.y) + i32::from(area.h));
        }
        self.float_apply(
            id,
            Rect {
                x: l as u16,
                y: t as u16,
                w: (right - l) as u16,
                h: (b - t) as u16,
            },
        );
    }
    pub(super) fn focus_float_move(&mut self, dx: i32, dy: i32) -> bool {
        let Some(id) = self
            .layout
            .focus()
            .filter(|id| self.layout.nodes[id].floating)
        else {
            return false;
        };
        self.float_move(id, dx * self.config.number("gap_aspect").max(1) as i32, dy);
        true
    }
    pub(super) fn focus_float_grow(&mut self, sign: i32) -> bool {
        let Some(id) = self
            .layout
            .focus()
            .filter(|id| self.layout.nodes[id].floating)
        else {
            return false;
        };
        if self.float_editable(id) {
            let r = self.layout.nodes[&id].rect;
            let a = self.layout.area(self.cols, self.rows, &self.config);
            let (mw, mh) = size(&self.config, "min_pane");
            let w = (i32::from(r.w) + sign * 2 * self.config.number("gap_aspect").max(1) as i32)
                .clamp(i32::from(mw.min(a.w)), i32::from(a.w));
            let h = (i32::from(r.h) + sign * 2).clamp(i32::from(mh.min(a.h)), i32::from(a.h));
            let x = (i32::from(r.x) + (i32::from(r.w) - w) / 2)
                .clamp(i32::from(a.x), i32::from(a.x) + i32::from(a.w) - w);
            let y = (i32::from(r.y) + (i32::from(r.h) - h) / 2)
                .clamp(i32::from(a.y), i32::from(a.y) + i32::from(a.h) - h);
            self.float_apply(
                id,
                Rect {
                    x: x as u16,
                    y: y as u16,
                    w: w as u16,
                    h: h as u16,
                },
            );
        }
        true
    }
}
