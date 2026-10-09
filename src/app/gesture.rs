//! Title drags share the same tree operations as scripted pane moves.
use super::App;
use crate::{
    bindings::Action,
    input::Event,
    layout::{Direction, Id},
    screen::{BOLD, Screen},
};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Title,
    FloatMove,
    FloatResize(u8),
    Tab,
    Border(char),
    Edge(usize),
    Corner(super::corner::Boundaries),
}

pub(super) struct Drag {
    pub source: Id,
    pub target: Option<Id>,
    start: (u16, u16),
    pub moved: bool,
    pub kind: Kind,
}
impl Drag {
    pub(super) fn tiled(&self) -> bool {
        self.moved && self.kind == Kind::Title
    }
}

impl App {
    pub(super) fn gesture_event(&mut self, event: &Event, screen: &Screen) -> bool {
        if matches!(event, Event::Key { .. }) {
            self.drag = None;
            return false;
        }
        if self.help {
            return false;
        }
        let Event::Mouse {
            button,
            action,
            x,
            y,
            ..
        } = event
        else {
            return false;
        };
        self.pointer = Some((*x, *y));
        let hit = screen.hits.test(*x, *y).unwrap_or("");
        let (verb, body) = hit.split_once(':').unwrap_or((hit, ""));
        let id = body.split(':').next().and_then(|s| s.parse::<Id>().ok());
        if *action == 0
            && *button == 1
            && verb == "corner"
            && let Some(corner) = body.parse::<usize>().ok().and_then(|i| self.corners.get(i))
        {
            self.drag = Some(Drag {
                source: 0,
                target: None,
                start: (*x, *y),
                moved: false,
                kind: Kind::Corner(corner.boundaries),
            });
            self.pressed = None;
            return true;
        }
        if *action == 0
            && *button == 1
            && matches!(verb, "edge" | "border" | "brim")
            && let Some(id) = id.filter(|id| self.layout.nodes.contains_key(id))
            && !self.layout.nodes[&id].floating
        {
            let detail = body.split(':').nth(1).unwrap_or("");
            let kind = if verb == "edge" {
                Kind::Edge(detail.parse().unwrap_or(0))
            } else {
                Kind::Border(detail.chars().next().unwrap_or('r'))
            };
            self.drag = Some(Drag {
                source: id,
                target: None,
                start: (*x, *y),
                moved: false,
                kind,
            });
            self.pressed = Some(hit.into());
            return true;
        }
        if *action == 0
            && *button == 1
            && matches!(verb, "border" | "brim")
            && let Some(id) = id.filter(|id| self.float_editable(*id))
        {
            self.layout.focus_pane(id);
            self.drag = Some(Drag {
                source: id,
                target: None,
                start: (*x, *y),
                moved: false,
                kind: Kind::FloatResize(super::floating::edges(
                    self.layout.nodes[&id].rect,
                    *x,
                    *y,
                )),
            });
            self.pressed = None;
            return true;
        }
        if *action == 0
            && *button == 1
            && matches!(verb, "panetitle" | "tab")
            && let Some(id) = id
        {
            let target = if verb == "tab" {
                Action::RenameTab
            } else {
                Action::RenamePane
            };
            let now = Instant::now();
            if self.name_click.is_some_and(|(kind, old, at)| {
                kind == target
                    && old == id
                    && now.saturating_duration_since(at)
                        <= Duration::from_millis(self.config.number("double_click_ms"))
            }) {
                self.begin_edit_id(target, id);
                return true;
            }
            self.name_click = Some((target, id, now));
        }
        if *action == 0
            && *button == 1
            && verb == "tab"
            && let Some(index) = id.and_then(|id| self.layout.tabs.iter().position(|t| t.id == id))
        {
            self.select_tab(index);
            self.drag = Some(Drag {
                source: id.unwrap(),
                target: None,
                start: (*x, *y),
                moved: false,
                kind: Kind::Tab,
            });
            self.pressed = None;
            return true;
        }
        if *action == 0
            && *button == 1
            && matches!(verb, "title" | "panetitle" | "header")
            && let Some(id) = id.filter(|id| self.panes.contains_key(id))
        {
            let node = &self.layout.nodes[&id];
            if node.floating
                && (node.hidden
                    || node.collapsed
                    || node.minimized
                    || self.layout.tabs[self.layout.current].zoom.is_some())
            {
                return false;
            }
            self.drag = Some(Drag {
                source: id,
                target: None,
                start: (*x, *y),
                moved: false,
                kind: if node.floating {
                    Kind::FloatMove
                } else {
                    Kind::Title
                },
            });
        } else if matches!(action, 2 | 3)
            && let Some(drag) = &mut self.drag
        {
            drag.moved |= drag.start != (*x, *y);
            if let Kind::Corner(boundaries) = drag.kind {
                let dx = i32::from(*x) - i32::from(drag.start.0);
                let dy = i32::from(*y) - i32::from(drag.start.1);
                drag.start = (*x, *y);
                for (split, edge) in boundaries.into_iter().flatten() {
                    self.layout.drag_edge(split, edge, dx, dy, &self.config);
                    self.sync_layout();
                }
                self.pressed = None;
                return true;
            }
            if let Kind::Border(side) = drag.kind
                && let Some((split, edge)) = self.layout.boundary_for_side(drag.source, side)
            {
                drag.source = split;
                drag.kind = Kind::Edge(edge);
            }
            if let Kind::Edge(edge) = drag.kind {
                let split = drag.source;
                let dx = i32::from(*x) - i32::from(drag.start.0);
                let dy = i32::from(*y) - i32::from(drag.start.1);
                drag.start = (*x, *y);
                self.layout.drag_edge(split, edge, dx, dy, &self.config);
                self.sync_layout();
                self.pressed = None;
                return true;
            }
            if matches!(drag.kind, Kind::Border(_)) {
                self.pressed = None;
                return true;
            }
            if drag.kind == Kind::Tab {
                let source = drag.source;
                let to = match verb {
                    "tab" => id.and_then(|id| self.layout.tabs.iter().position(|t| t.id == id)),
                    "newtab" => Some(self.layout.tabs.len().saturating_sub(1)),
                    _ => None,
                };
                if let Some(to) = to {
                    self.move_tab(source, to);
                }
                self.pressed = None;
                return true;
            }
            if matches!(drag.kind, Kind::FloatMove | Kind::FloatResize(_)) {
                let source = drag.source;
                let kind = drag.kind;
                let dx = *x as i32 - drag.start.0 as i32;
                let dy = *y as i32 - drag.start.1 as i32;
                // A clamp spends the motion at the wall. The next motion is
                // relative to the clamped position, rather than the press.
                drag.start = (*x, *y);
                if let Kind::FloatResize(edges) = kind {
                    self.float_resize(source, edges, dx, dy);
                } else {
                    self.float_move(source, dx, dy);
                }
                self.pressed = None;
                return true;
            }
            drag.target = if matches!(
                verb,
                "pane" | "focus" | "title" | "panetitle" | "border" | "brim" | "header" | "drop"
            ) {
                id.filter(|id| {
                    *id != drag.source && self.layout.nodes.get(id).is_some_and(|n| !n.floating)
                })
            } else {
                None
            };
            if drag.moved {
                self.pressed = None;
                return true;
            }
        } else if *action == 1
            && let Some(drag) = self.drag.take()
        {
            if !drag.moved {
                if drag.kind == Kind::Title
                    && hit == format!("title:{}:t", drag.source)
                    && self.pressed.as_deref() == Some(hit.as_bytes())
                    && self
                        .layout
                        .split_fits(drag.source, Direction::Rows, &self.config)
                {
                    let _ = self.split(drag.source, Direction::Rows, true);
                    self.toast("split up");
                    self.pressed = None;
                    return true;
                }
                return false;
            }
            self.pressed = None;
            if drag.kind != Kind::Title {
                return true;
            }
            match verb {
                "drop" => {
                    if let Some(target) = id {
                        let side = body.split(':').nth(1).unwrap_or("");
                        let dir = if matches!(side, "l" | "r") {
                            Direction::Cols
                        } else {
                            Direction::Rows
                        };
                        self.layout
                            .drop_leaf(drag.source, target, dir, matches!(side, "l" | "t"));
                        self.sync_layout();
                    }
                }
                "tab" => {
                    if let Some(tab) = id {
                        let name = self
                            .layout
                            .tabs
                            .iter()
                            .find(|t| t.id == tab)
                            .map(|t| t.name.clone())
                            .unwrap_or_default();
                        if self.move_pane(drag.source, tab, 0, Direction::Cols, true) {
                            let index =
                                self.layout.tabs.iter().position(|t| t.id == tab).unwrap() + 1;
                            self.toast(&format!(
                                "moved to tab {index}{}",
                                if name.is_empty() {
                                    String::new()
                                } else {
                                    format!(" ({})", String::from_utf8_lossy(&name))
                                }
                            ));
                        } else {
                            self.toast("it is already in that tab");
                        }
                    }
                }
                "newtab" => {
                    let moved = self.move_pane_to_new_tab(drag.source, "").is_some();
                    self.toast(if moved {
                        "into a tab of its own"
                    } else {
                        "it is the only pane in this tab"
                    });
                }
                _ => {
                    if matches!(
                        verb,
                        "pane" | "focus" | "title" | "panetitle" | "border" | "brim" | "header"
                    ) && let Some(target) = id.filter(|id| *id != drag.source)
                        && self.layout.nodes.get(&target).is_some_and(|n| !n.floating)
                    {
                        self.layout.swap_leaves(drag.source, target);
                        self.sync_layout();
                    }
                }
            }
            return true;
        }
        false
    }
    pub(super) fn draw_drag_strip(&self, screen: &mut Screen) {
        let Some(drag) = self.drag.as_ref().filter(|d| d.tiled()) else {
            return;
        };
        let own = self
            .layout
            .tab_of(drag.source)
            .map(|i| self.layout.tabs[i].id);
        for hit in screen.hits.0.clone() {
            if hit.action != b"newtab" {
                let Some(tab) = hit
                    .action_text()
                    .strip_prefix("tab:")
                    .and_then(|s| s.parse::<Id>().ok())
                else {
                    continue;
                };
                if Some(tab) == own {
                    continue;
                }
            }
            let on = self.pointer.is_some_and(|(x, y)| {
                x >= hit.x && x < hit.x + hit.w && y >= hit.y && y < hit.y + hit.h
            });
            for y in hit.y..hit.y.saturating_add(hit.h) {
                for x in hit.x..hit.x.saturating_add(hit.w) {
                    if let Some(cell) = screen.at_mut(x, y) {
                        cell.fg = self.config.color(if on {
                            "tab_active_hover_fg"
                        } else {
                            "drop_target"
                        });
                        cell.bg = if on {
                            self.config.color("drop_target")
                        } else {
                            Default::default()
                        };
                        cell.attrs |= BOLD;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_float_moves_back_immediately_after_hitting_a_wall() {
        let mut app = App::new(vec![crate::shell::default_program()], 90, 24).unwrap();
        app.config = crate::config::Config::default();
        app.sync_layout();
        let id = app.new_float().unwrap().unwrap();
        let mut screen = Screen::new(90, 24);
        app.compose(&mut screen);
        let rect = app.layout.nodes[&id].rect;
        let mouse = |action, x, y| Event::Mouse {
            button: 1,
            action,
            mods: 0,
            x,
            y,
        };
        assert!(!app.gesture_event(&mouse(0, rect.x + 3, rect.y), &screen));
        assert!(app.gesture_event(&mouse(2, 89, 23), &screen));
        let clamped = app.layout.nodes[&id].rect;
        let area = app.layout.area(90, 24, &app.config);
        assert_eq!(clamped.x + clamped.w, area.x + area.w);
        assert_eq!(clamped.y + clamped.h, area.y + area.h);
        assert!(app.gesture_event(&mouse(2, 88, 22), &screen));
        let back = app.layout.nodes[&id].rect;
        assert_eq!(back.x + 1, clamped.x);
        assert_eq!(back.y + 1, clamped.y);
        assert_eq!((back.w, back.h), (clamped.w, clamped.h));
    }
}
