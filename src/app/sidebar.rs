//! Sidebar tab navigation and per-pane program status annotations.
use super::App;
use crate::{
    config::Setting,
    screen::{self, BOLD, Color, ITALIC, Screen},
};

fn clip(text: &str, width: u16) -> String {
    let mut s = text.to_owned();
    while screen::cells(&s) > width {
        s.pop();
    }
    s
}
fn pad(text: &str, width: u16) -> String {
    let mut s = clip(text, width);
    s.push_str(&" ".repeat(width.saturating_sub(screen::cells(&s)) as usize));
    s
}
fn pad_bytes(text: &[u8], width: u16) -> Vec<u8> {
    let mut text = screen::clip_bytes(text, width);
    text.extend(std::iter::repeat_n(
        b' ',
        width.saturating_sub(screen::cells_bytes(&text)) as usize,
    ));
    text
}
fn cut(text: &[u8], width: u16) -> Vec<u8> {
    if screen::cells_bytes(text) <= width {
        text.into()
    } else {
        [
            screen::clip_bytes(text, width.saturating_sub(1)).as_slice(),
            "…".as_bytes(),
        ]
        .concat()
    }
}
fn wrap(text: &[u8], width: u16, max: usize) -> Vec<Vec<u8>> {
    let mut rows: Vec<Vec<u8>> = Vec::new();
    let mut rest = text;
    while rows.len() < max && width > 0 {
        rest = &rest[rest.iter().position(|b| *b != b' ').unwrap_or(rest.len())..];
        if rest.is_empty() {
            break;
        }
        let prefix = screen::clip_bytes(rest, width);
        if prefix.is_empty() {
            break;
        }
        let n = if prefix.len() == rest.len() || rest.get(prefix.len()) == Some(&b' ') {
            prefix.len()
        } else {
            prefix
                .iter()
                .rposition(|b| *b == b' ')
                .unwrap_or(prefix.len())
        };
        rows.push(rest[..n].into());
        rest = &rest[n..];
    }
    if rest.iter().any(|b| *b != b' ')
        && let Some(last) = rows.last_mut()
    {
        *last = [
            screen::clip_bytes(last, width.saturating_sub(1)).as_slice(),
            "…".as_bytes(),
        ]
        .concat();
    }
    rows
}
impl App {
    fn tab_name(&self, i: usize) -> String {
        let t = &self.layout.tabs[i];
        if !t.name.is_empty() {
            return String::from_utf8_lossy(&t.name).into_owned();
        }
        if !t.purpose.is_empty() {
            return t.purpose.clone();
        }
        let cwd = self.panes.get(&t.focus).and_then(|p| p.live_cwd());
        cwd.map(|path| {
            if std::env::var_os("HOME").is_some_and(|home| home == path.as_os_str()) {
                "~".into()
            } else {
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned()
            }
        })
        .unwrap_or_default()
    }
    fn sidebar_tab(
        &self,
        s: &mut Screen,
        i: usize,
        x: u16,
        y: u16,
        width: u16,
        padding: (u16, u16),
    ) {
        let (left, right) = padding;
        let t = &self.layout.tabs[i];
        let name = self.tab_name(i);
        let edit = self
            .edit
            .as_ref()
            .filter(|e| e.target == crate::bindings::Action::RenameTab && e.id == t.id);
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
        let idx = format!("{}{}", i + 1, " ".repeat(right as usize));
        let right_idx = edit.is_none()
            && self.config.text("tab_bar_index") == Some("right")
            && width >= screen::cells(&idx) + 5;
        let label = if let Some(e) = edit {
            format!(
                "{}{}█{}",
                " ".repeat(left as usize),
                e.text,
                " ".repeat(right as usize)
            )
        } else if right_idx {
            format!("{}{name}", " ".repeat(left as usize))
        } else if name.is_empty() {
            format!(
                "{}{}{}",
                " ".repeat(left as usize),
                i + 1,
                " ".repeat(right as usize)
            )
        } else {
            format!(
                "{}{}:{name}{}",
                " ".repeat(left as usize),
                i + 1,
                " ".repeat(right as usize)
            )
        };
        let budget = width
            .saturating_sub(screen::cells_bytes(&mark))
            .saturating_sub(if right_idx { screen::cells(&idx) } else { 0 });
        let label = pad(
            &clip(&label, budget.saturating_sub(u16::from(right_idx))),
            budget,
        );
        let hot = self
            .pointer
            .is_some_and(|(px, py)| px >= x && px < x + width && py == y);
        let active = i == self.layout.current;
        let fg = self.config.color(if edit.is_some() {
            "rename_fg"
        } else if active {
            if hot {
                "tab_active_hover_fg"
            } else {
                "tab_active_fg"
            }
        } else if hot {
            "tab_hover"
        } else {
            "tab_idle"
        });
        let bg = self.config.color(if edit.is_some() {
            "rename_bg"
        } else if active {
            "tab_active_bg"
        } else {
            "tab_idle_bg"
        });
        let attrs = if active || hot || edit.is_some() {
            BOLD
        } else {
            0
        };
        s.text(x, y, &label, fg, bg, attrs);
        s.text_bytes(x + budget, y, &mark, self.config.color("bell"), bg, BOLD);
        if right_idx {
            s.text(x + width - screen::cells(&idx), y, &idx, fg, bg, attrs);
        }
        s.hits.add(x, y, width, 1, &format!("tab:{}", t.id));
    }
    pub(super) fn draw_sidebar(&mut self, s: &mut Screen) {
        let config = &self.config;
        let sw = self.layout.sidebar_width(self.cols, config);
        let left = self.layout.sidebar(self.cols, self.rows, config) == "left";
        let x = if left {
            0
        } else {
            self.cols.saturating_sub(sw)
        };
        let compact = config.boolean("compact");
        let top = if compact {
            0
        } else {
            config.number("gap") as u16
        };
        let mut limit = self
            .rows
            .saturating_sub(u16::from(config.boolean("status_line")));
        let chrome = config.boolean("tab_bar_chrome") && limit > top + 3 && sw >= 6;
        let (cx, cw) = if chrome {
            let (cx, cw) = if compact {
                (if left { 1 } else { x }, sw - 1)
            } else {
                (x + 1, sw - 2)
            };
            sidebar_ring(
                s,
                cx - 1,
                top,
                cx + cw,
                limit - 1,
                config.color("frame_idle"),
                config.boolean("rounded"),
            );
            limit -= 1;
            (cx, cw)
        } else {
            (x, sw)
        };
        let [pt, pr, pb, pl] = match config.setting("tab_bar_padding") {
            Some(Setting::Sides(s)) => *s,
            _ => [0, 1, 0, 2],
        };
        let mut y = top + u16::from(chrome) + pt;
        let ix = cx + if cw > pl { pl } else { 0 };
        limit = limit.saturating_sub(pb.min(limit.saturating_sub(y + 1)));
        let count = self.panes.len();
        let info = format!("{count} pane{}", if count == 1 { "" } else { "s" });
        if limit > y && screen::cells(&info) < cw {
            limit -= 1;
            s.text(
                ix,
                limit,
                &info,
                config.color("tab_count"),
                Color::default(),
                0,
            );
        }
        if self.prefix {
            let prefix = config.bindings.prefix.display();
            if limit > y && screen::cells(&prefix) < cw {
                limit -= 1;
                s.text(
                    ix,
                    limit,
                    &prefix,
                    config.color("prefix_fg"),
                    config.color("prefix_bg"),
                    BOLD,
                );
            }
        }
        let cap = config.number("tab_bar_status").min(16) as usize;
        let mark = config.text_bytes("newtab_mark").unwrap_or_default();
        let button = config.boolean("newtab_button") && !mark.is_empty();
        for (i, t) in self.layout.tabs.iter().enumerate() {
            if y >= limit {
                break;
            }
            if i > 0 {
                let need = (self.layout.tabs.len() - i) as u16 + u16::from(button);
                y += (config.number("tab_gap") as u16).min(limit.saturating_sub(y + need));
            }
            if y >= limit {
                break;
            }
            self.sidebar_tab(s, i, cx, y, cw, (pl, pr));
            y += 1;
            if cap == 0 {
                continue;
            }
            let keep = (self.layout.tabs.len() - i - 1) as u16 + u16::from(button);
            let allow = cap.min(limit.saturating_sub(keep).saturating_sub(y) as usize);
            let lines = (config.number("tab_bar_status_lines") as usize)
                .clamp(1, 8)
                .min(allow);
            let tw = cw.saturating_sub(pl + pr).max(1);
            let all: Vec<_> = self
                .layout
                .leaves(t.root)
                .into_iter()
                .filter_map(|id| {
                    let p = &self.panes[&id];
                    let text = if !p.alive && !p.suspended {
                        p.exit_words().into_bytes()
                    } else {
                        p.status.clone()
                    };
                    (!text.is_empty()).then_some((id, text, p.busy && p.alive))
                })
                .collect();
            let statuses: Vec<_> = all
                .iter()
                .take(16)
                .map(|(id, text, busy)| {
                    let rows = if lines <= 1 {
                        vec![cut(text, tw)]
                    } else {
                        wrap(text, tw, lines)
                    };
                    (
                        *id,
                        if rows.is_empty() {
                            vec![Vec::new()]
                        } else {
                            rows
                        },
                        *busy,
                    )
                })
                .collect();
            let want = statuses
                .iter()
                .map(|(_, rows, _)| rows.len())
                .sum::<usize>();
            let cut = all.len() > statuses.len() || want > allow;
            let budget = allow.saturating_sub(usize::from(cut));
            let mut used = 0;
            let mut shown = 0;
            for (j, (id, rows, busy)) in statuses.iter().enumerate() {
                if used + rows.len() > budget {
                    break;
                }
                let hot = self.pointer.is_some_and(|(px, py)| {
                    px >= cx && px < cx + cw && py >= y && py < y + rows.len() as u16
                });
                let band = config.color(if lines > 1 && j % 2 == 1 {
                    "tab_status_stripe"
                } else {
                    "tab_status_bg"
                });
                let frames: Vec<_> = config
                    .text_bytes("busy_mark")
                    .unwrap_or_default()
                    .split(|b| *b == b' ')
                    .filter(|s| !s.is_empty())
                    .take(16)
                    .collect();
                let marked = *busy && pl > 0 && !frames.is_empty();
                if marked && frames.len() > 1 {
                    self.sidebar_spins = true;
                }
                for (k, row) in rows.iter().enumerate() {
                    let text = pad_bytes(&[" ".repeat(pl as usize).as_bytes(), row].concat(), cw);
                    s.text_bytes(
                        cx,
                        y,
                        &text,
                        config.color(if hot {
                            "tab_hover"
                        } else if *busy {
                            "tab_status_busy"
                        } else {
                            "tab_status_fg"
                        }),
                        band,
                        if hot { BOLD } else { ITALIC },
                    );
                    if marked && k == 0 {
                        let step = config.number("busy_ms").max(1);
                        let frame = (self.started.elapsed().as_millis() as u64 / step) as usize
                            % frames.len();
                        s.text_bytes(
                            cx,
                            y,
                            &frames[frame][..frames[frame].len().min(15)],
                            config.color(if hot {
                                "tab_hover"
                            } else {
                                "tab_status_spinner"
                            }),
                            band,
                            0,
                        );
                    }
                    s.hits.add(cx, y, cw, 1, &format!("find:{id}"));
                    y += 1;
                }
                used += rows.len();
                shown += 1;
            }
            if shown < all.len() && y < limit && used < allow {
                s.text(
                    cx,
                    y,
                    &pad(&format!("{}…", " ".repeat(pl as usize)), cw),
                    config.color("tab_status_fg"),
                    config.color("tab_status_bg"),
                    ITALIC,
                );
                y += 1;
            }
        }
        if button {
            y += (config.number("newtab_pad") as u16).min(limit.saturating_sub(y + 1));
            if y < limit {
                let text = if screen::cells_bytes(mark) + 10 <= cw {
                    [" ".repeat(pl as usize).as_bytes(), mark, b" new tab "].concat()
                } else {
                    [" ".repeat(pl as usize).as_bytes(), mark, b" "].concat()
                };
                let hot = self
                    .pointer
                    .is_some_and(|(px, py)| px >= cx && px < cx + cw && py == y);
                s.text_bytes(
                    cx,
                    y,
                    &pad_bytes(&text, cw),
                    config.color(if hot { "tab_hover" } else { "tab_idle" }),
                    Color::default(),
                    if hot { BOLD } else { 0 },
                );
                s.hits.add(cx, y, cw, 1, "newtab");
            }
        }
    }
}

fn sidebar_ring(s: &mut Screen, x0: u16, y0: u16, x1: u16, y1: u16, fg: Color, rounded: bool) {
    let glyphs = [
        " ",
        "╵",
        "╷",
        "│",
        "╴",
        if rounded { "╰" } else { "└" },
        if rounded { "╭" } else { "┌" },
        "├",
        "╶",
        if rounded { "╯" } else { "┘" },
        if rounded { "╮" } else { "┐" },
        "┤",
        "─",
        "┴",
        "┬",
        "┼",
    ];
    let mut stroke = |x, y, bits: usize| {
        if let Some(i) = s.index(x, y) {
            let c = &s.cells[i];
            let text = std::str::from_utf8(&c.text[..c.len as usize]).unwrap_or("");
            let have = glyphs.iter().position(|g| *g == text).unwrap_or(0);
            s.text(x, y, glyphs[have | bits], fg, Color::default(), 0);
        }
    };
    stroke(x0, y0, 6);
    stroke(x1, y0, 10);
    stroke(x0, y1, 5);
    stroke(x1, y1, 9);
    for x in x0 + 1..x1 {
        stroke(x, y0, 12);
        stroke(x, y1, 12);
    }
    for y in y0 + 1..y1 {
        stroke(x0, y, 3);
        stroke(x1, y, 3);
    }
}
