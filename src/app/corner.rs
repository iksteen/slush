//! Crossings group touching dividers; a held crossing keeps stable divider IDs.
use super::{App, gesture::Kind};
use crate::{
    layout::{Direction, Id},
    screen::{BOLD, Screen},
    shader::Rect,
};

pub(super) type Boundaries = [Option<(Id, usize)>; 4];
pub(super) struct Corner {
    pub rect: Rect,
    pub boundaries: Boundaries,
}
impl App {
    pub(super) fn draw_corners(&mut self, screen: &mut Screen) {
        self.corners.clear();
        let gaps: Vec<_> = screen
            .hits
            .0
            .iter()
            .filter_map(|hit| {
                let mut p = hit.action_text().strip_prefix("edge:")?.split(':');
                let id = p.next()?.parse::<Id>().ok()?;
                let edge = p.next()?.parse::<usize>().ok()?;
                Some((
                    id,
                    edge,
                    self.layout.nodes.get(&id)?.direction()?,
                    Rect {
                        x: hit.x,
                        y: hit.y,
                        w: hit.w,
                        h: hit.h,
                    },
                ))
            })
            .take(64)
            .collect();
        for &(hid, hi, _, h) in gaps.iter().filter(|g| g.2 == Direction::Rows) {
            for &(vid, vi, _, v) in gaps.iter().filter(|g| g.2 == Direction::Cols) {
                if v.y > h.y + h.h || h.y > v.y + v.h || h.x > v.x + v.w || v.x > h.x + h.w {
                    continue;
                }
                let rect = Rect {
                    x: v.x,
                    y: h.y,
                    w: v.w,
                    h: h.h,
                };
                let at = self.corners.iter().position(|c| c.rect == rect);
                let at = if let Some(at) = at {
                    at
                } else {
                    if self.corners.len() == 16 {
                        continue;
                    }
                    self.corners.push(Corner {
                        rect,
                        boundaries: [None; 4],
                    });
                    self.corners.len() - 1
                };
                let c = &mut self.corners[at];
                for (range, boundary) in [(0..2, (hid, hi)), (2..4, (vid, vi))] {
                    if !c.boundaries[range.clone()].contains(&Some(boundary))
                        && let Some(slot) = c.boundaries[range].iter_mut().find(|s| s.is_none())
                    {
                        *slot = Some(boundary);
                    }
                }
            }
        }
        for (index, c) in self.corners.iter().enumerate() {
            let r = c.rect;
            screen
                .hits
                .add(r.x, r.y, r.w, r.h, &format!("corner:{index}"));
            let active = self.drag.as_ref().is_some_and(|d| {
                if let Kind::Corner(b) = d.kind {
                    b[0] == c.boundaries[0] && b[2] == c.boundaries[2]
                } else {
                    false
                }
            });
            let over = self.drag.is_none()
                && self
                    .pointer
                    .is_some_and(|(x, y)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h);
            if active || over {
                for x in r.x..r.x + r.w {
                    screen.text(
                        x,
                        r.y,
                        if active { "╬" } else { "┼" },
                        self.config.color("resize"),
                        Default::default(),
                        if active { BOLD } else { 0 },
                    );
                }
            }
        }
    }
}
