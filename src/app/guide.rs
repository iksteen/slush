//! Hover feedback reads the same hit regions that route the gesture.
use super::{App, floating::*, gesture::Kind as DragKind};
use crate::{
    layout::{Direction, Id},
    screen::{BOLD, Color, INVERSE, Screen},
    shader::{self, Context, Kind, Rect, Shader},
};
const NONE: Color = Color {
    set: false,
    r: 0,
    g: 0,
    b: 0,
};

impl App {
    pub(super) fn hint_for(&self, hit: &str) -> Option<&'static str> {
        let mut parts = hit.split(':');
        let verb = parts.next()?;
        let id = parts.next().and_then(|s| s.parse::<Id>().ok()).unwrap_or(0);
        Some(match verb {
            "minimize" => "minimise",
            "zoom" => {
                if self.layout.tabs[self.layout.current].zoom == Some(id) {
                    "back to the layout"
                } else {
                    "fill the tab"
                }
            }
            "close" => "close this pane",
            "rerun" => "run the command again",
            "scrollbottom" => "back to the bottom",
            "panetitle" => "double-click to rename · drag to move",
            "title" => {
                if self.layout.nodes.get(&id).is_some_and(|n| n.floating) || parts.next().is_none()
                {
                    "drag to move"
                } else {
                    "drag to move · click to split up"
                }
            }
            "border" | "brim" => {
                if let Some(n) = self.layout.nodes.get(&id).filter(|n| n.floating) {
                    let e = self.pointer.map_or(0, |(x, y)| edges(n.rect, x, y));
                    if e & (LEFT | RIGHT) != 0 && e & (TOP | BOTTOM) != 0 {
                        "drag to resize both ways"
                    } else {
                        "drag to resize"
                    }
                } else if verb == "brim" {
                    return None;
                } else {
                    match parts.next() {
                        Some("l") => "click to split left",
                        Some("r") => "click to split right",
                        Some("t") => "click to split up",
                        _ => "click to split down",
                    }
                }
            }
            "edge" => "drag to resize",
            "corner" => "drag to resize both ways",
            "focus" => "open this pane",
            "find" => "go to this pane",
            "open" => "go to this project",
            "tab" => "double-click to rename · drag to reorder",
            "newtab" => "new tab",
            _ => return None,
        })
    }
    pub(super) fn cast_shadow(&self, s: &mut Screen, r: Rect) {
        let amount = self.config.number("float_shadow");
        if amount == 0 || r.h == 0 {
            return;
        }
        let off = self.config.number("gap_aspect").max(1) as u16;
        let pass = Shader::new(Kind::Dim, NONE, amount as u8);
        let context = Context {
            default_fg: self.config.color("default_fg"),
            default_bg: self.config.color("default_bg"),
            ..Context::default()
        };
        for rect in [
            Rect {
                x: r.x + r.w,
                y: r.y + 1,
                w: off,
                h: r.h - 1,
            },
            Rect {
                x: r.x + off,
                y: r.y + r.h,
                w: r.w,
                h: 1,
            },
        ] {
            shader::apply(s, std::slice::from_ref(&pass), rect, None, &context);
        }
    }
    pub(super) fn draw_guides(&self, s: &mut Screen, id: Id) {
        let Some((px, py)) = self.pointer else { return };
        let n = &self.layout.nodes[&id];
        if n.hidden || n.collapsed || n.minimized {
            return;
        }
        let r = n.rect;
        let hit = s.hits.test(px, py).unwrap_or("").to_owned();
        let mut p = hit.split(':');
        let verb = p.next().unwrap_or("");
        let owner = p.next().and_then(|v| v.parse::<Id>().ok());
        let side = p.next().and_then(|s| s.chars().next());
        let held = self.drag.as_ref().filter(|d| d.source == id);
        let dwell = self.pointer_since.elapsed().as_millis()
            >= u128::from(self.config.number("hover_delay_ms"));
        if n.floating {
            if r.w < 3 || r.h < 3 {
                return;
            }
            let active = held.and_then(|d| {
                if let DragKind::FloatResize(e) = d.kind {
                    Some(e)
                } else {
                    None
                }
            });
            let e = if let Some(e) = active {
                e
            } else if self.drag.is_none()
                && dwell
                && owner == Some(id)
                && matches!(verb, "border" | "brim")
            {
                edges(r, px, py)
            } else {
                return;
            };
            let color = self.config.color("resize");
            for (side, bit) in [('l', LEFT), ('r', RIGHT), ('b', BOTTOM)] {
                if e & bit == 0 {
                    continue;
                }
                let rim = format!("brim:{id}:{side}");
                let handle = format!("border:{id}:{side}");
                let cells: Vec<_> = if side == 'b' {
                    (r.x..r.x + r.w).map(|x| (x, r.y + r.h - 1)).collect()
                } else {
                    (r.y + 1..r.y + r.h - 1)
                        .map(|y| (if side == 'l' { r.x } else { r.x + r.w - 1 }, y))
                        .collect()
                };
                for (x, y) in cells {
                    if s.hits.test(x, y).is_some_and(|h| h == rim || h == handle) {
                        s.text(x, y, if side == 'b' { "━" } else { "┃" }, color, NONE, BOLD);
                    }
                }
            }
            let corner = e & (LEFT | RIGHT) != 0 && e & (TOP | BOTTOM) != 0;
            if active.is_none() && !corner {
                s.text(
                    px,
                    py,
                    if e & (LEFT | RIGHT) != 0 {
                        "⇔"
                    } else {
                        "⇕"
                    },
                    color,
                    NONE,
                    BOLD,
                );
            }
            return;
        }
        let border_held = held.is_some_and(|d| matches!(d.kind, DragKind::Border(_)));
        if self.select_drag.is_some()
            || self.drag.is_some() && !border_held
            || !dwell && !border_held
            || owner != Some(id)
        {
            return;
        }
        let (side, on_handle) = match verb {
            "title" => ('t', side.is_some()),
            "border" | "brim" => (side.unwrap_or('r'), verb == "border"),
            _ => return,
        };
        let dir = if matches!(side, 'l' | 'r') {
            Direction::Cols
        } else {
            Direction::Rows
        };
        if !self.layout.split_fits(id, dir, &self.config) {
            return;
        }
        let compact = self.compact_pane(id);
        if r.w < 4 || r.h < if compact { 3 } else { 4 } {
            return;
        }
        let rim = if side == 't' {
            format!("title:{id}")
        } else {
            format!("brim:{id}:{side}")
        };
        let handle = if side == 't' {
            format!("title:{id}:t")
        } else {
            format!("border:{id}:{side}")
        };
        let color = self.config.color("guide");
        let vert = matches!(side, 'l' | 'r');
        let cells: Vec<_> = if vert {
            let x = if side == 'l' {
                r.x - u16::from(compact)
            } else {
                r.x + r.w - u16::from(!compact)
            };
            (r.y + u16::from(!compact)..r.y + r.h - u16::from(!compact))
                .map(|y| (x, y))
                .collect()
        } else {
            let y = if side == 't' {
                r.y - u16::from(compact)
            } else {
                r.y + r.h - u16::from(!compact)
            };
            (r.x + u16::from(!compact)..r.x + r.w - u16::from(!compact))
                .map(|x| (x, y))
                .collect()
        };
        for (x, y) in cells {
            if s.hits.test(x, y).is_some_and(|h| h == rim || h == handle) {
                s.text(x, y, if vert { "┃" } else { "━" }, color, NONE, BOLD);
            }
        }
        if let Some(core) = s
            .hits
            .0
            .iter()
            .find(|h| h.action == handle.as_bytes())
            .cloned()
        {
            let mut h = Rect {
                x: core.x,
                y: core.y,
                w: core.w,
                h: core.h,
            };
            if vert {
                let inner = if side == 'l' {
                    h.x + 1
                } else {
                    h.x.saturating_sub(1)
                };
                s.hits.add(inner, h.y, 1, h.h, &handle);
                h.x = h.x.min(inner);
                h.w = 2;
            }
            let (ax, ay) = if vert {
                (if side == 'l' { h.x } else { h.x + h.w - 1 }, h.y + h.h / 2)
            } else {
                (h.x + h.w / 2, h.y)
            };
            let arrow = match side {
                'l' => "◄",
                'r' => "►",
                't' => "▲",
                _ => "▼",
            };
            for y in h.y..h.y + h.h {
                for x in h.x..h.x + h.w {
                    if s.hits.test(x, y) == Some(handle.as_str()) {
                        let tip = (x, y) == (ax, ay);
                        s.text(
                            x,
                            y,
                            if tip { arrow } else { "█" },
                            color,
                            NONE,
                            BOLD | if tip { INVERSE } else { 0 },
                        );
                    }
                }
            }
        }
        if !on_handle {
            return;
        }
        if vert {
            for y in r.y + 1..r.y + r.h - 1 {
                s.text(r.x + r.w / 2, y, "╎", color, NONE, 0);
            }
        } else {
            for x in r.x + 1..r.x + r.w - 1 {
                s.text(x, r.y + r.h / 2, "╌", color, NONE, 0);
            }
        }
    }
    pub(super) fn draw_resize_guides(&self, s: &mut Screen) {
        let over = self
            .pointer
            .and_then(|(x, y)| s.hits.test(x, y))
            .unwrap_or("")
            .to_owned();
        let dwell = self.pointer_since.elapsed().as_millis()
            >= u128::from(self.config.number("hover_delay_ms"));
        let color = self.config.color("resize");
        let corner = over
            .strip_prefix("corner:")
            .and_then(|v| v.parse::<usize>().ok())
            .and_then(|i| self.corners.get(i));
        for h in s
            .hits
            .0
            .clone()
            .into_iter()
            .filter(|h| h.action.starts_with(b"edge:"))
        {
            let mut p = h.action_text().split(':').skip(1);
            let id = p.next().and_then(|s| s.parse::<Id>().ok()).unwrap_or(0);
            let edge = p.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
            let active = self.drag.as_ref().is_some_and(|d| match d.kind {
                DragKind::Corner(boundaries) => boundaries.contains(&Some((id, edge))),
                DragKind::Edge(index) => d.source == id && index == edge,
                _ => false,
            });
            let corner_hover = corner.is_some_and(|c| c.boundaries.contains(&Some((id, edge))));
            if !active
                && (self.drag.is_some()
                    || self.select_drag.is_some()
                    || !dwell
                    || over.as_bytes() != h.action && !corner_hover)
            {
                continue;
            }
            let vert = self
                .layout
                .nodes
                .get(&id)
                .is_some_and(|n| n.direction() == Some(Direction::Cols));
            let (cx, cy) = (h.x + h.w / 2, h.y + h.h / 2);
            let cells: Vec<_> = if vert {
                (h.y..h.y + h.h).map(|y| (cx, y)).collect()
            } else {
                (h.x..h.x + h.w).map(|x| (x, cy)).collect()
            };
            for (x, y) in cells {
                if s.hits.test_bytes(x, y) != Some(h.action.as_slice()) {
                    continue;
                }
                let middle = (x, y) == (cx, cy) && !corner_hover;
                let glyph = if middle {
                    if vert { "⇔" } else { "⇕" }
                } else if vert {
                    if active { "║" } else { "┊" }
                } else if active {
                    "═"
                } else {
                    "┈"
                };
                s.text(
                    x,
                    y,
                    glyph,
                    color,
                    NONE,
                    if active || middle { BOLD } else { 0 },
                );
            }
        }
    }
    pub(super) fn draw_drop_zones(&self, s: &mut Screen) {
        let Some(drag) = self.drag.as_ref().filter(|d| d.tiled()) else {
            return;
        };
        let Some((x, y)) = self.pointer else { return };
        let Some(id) = s
            .hits
            .test(x, y)
            .and_then(|h| h.split(':').nth(1))
            .and_then(|v| v.parse::<Id>().ok())
        else {
            return;
        };
        let Some(n) = self.layout.nodes.get(&id).filter(|n| {
            n.kind == crate::layout::Kind::Leaf
                && !n.floating
                && !n.collapsed
                && !n.hidden
                && n.id != drag.source
        }) else {
            return;
        };
        let r = n.rect;
        if r.w < 3 || r.h < 3 || x < r.x || x >= r.x + r.w || y < r.y || y >= r.y + r.h {
            return;
        }
        let sibling = self
            .layout
            .nodes
            .get(&drag.source)
            .and_then(|n| n.parent)
            .filter(|p| Some(*p) == n.parent);
        let same_axis = sibling.and_then(|p| self.layout.nodes[&p].direction());
        let bh = (r.h / 3).max(1);
        let bw = (r.w / 3).max(1);
        for (side, dir, zone) in [
            (
                't',
                Direction::Rows,
                Rect {
                    x: r.x,
                    y: r.y,
                    w: r.w,
                    h: bh,
                },
            ),
            (
                'b',
                Direction::Rows,
                Rect {
                    x: r.x,
                    y: r.y + r.h - bh,
                    w: r.w,
                    h: bh,
                },
            ),
            (
                'l',
                Direction::Cols,
                Rect {
                    x: r.x,
                    y: r.y,
                    w: bw,
                    h: r.h,
                },
            ),
            (
                'r',
                Direction::Cols,
                Rect {
                    x: r.x + r.w - bw,
                    y: r.y,
                    w: bw,
                    h: r.h,
                },
            ),
        ] {
            if same_axis == Some(dir) || self.layout.split_fits(id, dir, &self.config) {
                s.hits
                    .add(zone.x, zone.y, zone.w, zone.h, &format!("drop:{id}:{side}"));
            }
        }
        let prefix = format!("drop:{id}:");
        let Some(side) = s
            .hits
            .test(x, y)
            .and_then(|h| h.strip_prefix(&prefix))
            .and_then(|s| s.chars().next())
        else {
            return;
        };
        let mut half = r;
        match side {
            'l' => half.w = r.w / 2,
            'r' => {
                half.w = r.w / 2;
                half.x = r.x + r.w - half.w;
            }
            't' => half.h = r.h / 2,
            _ => {
                half.h = r.h / 2;
                half.y = r.y + r.h - half.h;
            }
        }
        shader::apply(
            s,
            &[Shader::new(
                Kind::Tint,
                self.config.color("drop_target"),
                110,
            )],
            half,
            None,
            &Context {
                default_fg: self.config.color("default_fg"),
                default_bg: self.config.color("default_bg"),
                ..Context::default()
            },
        );
    }
}
