//! Canonical visibility, float/overlay subtraction, and per-display projection.
use super::App;
use crate::{graphics::Graphics, shader::Rect};

/// Disjoint strips in the original paint order, bounded to eight pieces.
fn subtract(rect: Rect, occluders: &[Rect]) -> Vec<Rect> {
    if rect.w == 0 || rect.h == 0 {
        return Vec::new();
    }
    let mut pieces = vec![rect];
    for cover in occluders {
        let mut next = Vec::new();
        for r in pieces {
            let (x0, y0, x1, y1) = (
                r.x as u32,
                r.y as u32,
                r.x as u32 + r.w as u32,
                r.y as u32 + r.h as u32,
            );
            let (ox0, oy0, ox1, oy1) = (
                cover.x as u32,
                cover.y as u32,
                cover.x as u32 + cover.w as u32,
                cover.y as u32 + cover.h as u32,
            );
            if ox1 <= x0 || ox0 >= x1 || oy1 <= y0 || oy0 >= y1 {
                next.push(r);
            } else {
                let (cx0, cy0, cx1, cy1) = (ox0.max(x0), oy0.max(y0), ox1.min(x1), oy1.min(y1));
                for (a, b, c, d) in [
                    (x0, y0, x1, cy0),
                    (x0, cy1, x1, y1),
                    (x0, cy0, cx0, cy1),
                    (cx1, cy0, x1, cy1),
                ] {
                    if c > a && d > b {
                        next.push(Rect {
                            x: a as u16,
                            y: b as u16,
                            w: (c - a) as u16,
                            h: (d - b) as u16,
                        });
                    }
                }
            }
            if next.len() > 8 {
                return Vec::new();
            }
        }
        pieces = next;
        if pieces.is_empty() {
            break;
        }
    }
    pieces
}

fn axis_crop(
    scaled: bool,
    cells: (u16, u16, u16),
    cell_px: u32,
    source: (u32, u32, u32),
) -> (u32, u32, u32) {
    let (total, start, count) = cells;
    let (mut source, mut len, mut offset) = source;
    if total == 0 || count == 0 {
        return (source, len, offset);
    }
    if scaled {
        let s0 = (len as u64 * start as u64 / total as u64) as u32;
        let mut s1 = (len as u64 * (start as u64 + count as u64) / total as u64) as u32;
        if s1 == s0 && s0 < len {
            s1 = s0 + 1;
        }
        source += s0;
        len = s1 - s0;
        if start != 0 {
            offset = 0;
        }
    } else {
        let skip = if start == 0 {
            0
        } else {
            (start as u32 * cell_px).saturating_sub(offset).min(len)
        };
        let keep = if start == 0 { offset } else { 0 };
        source += skip;
        len -= skip;
        offset = keep;
        if start as u32 + (count as u32) < total as u32 {
            len = len.min((count as u32 * cell_px).saturating_sub(keep));
        }
    }
    (source, len, offset)
}

impl App {
    pub(super) fn collect_graphics(&mut self) {
        let Some(tab) = self.layout.tabs.get(self.layout.current) else {
            return;
        };
        let leaves = self.layout.leaves(tab.root);
        let floats: Vec<_> = leaves
            .iter()
            .map(|id| &self.layout.nodes[id])
            .filter(|n| n.floating && !n.hidden && !n.collapsed)
            .take(64)
            .map(|n| (n.rect, n.raised))
            .collect();
        for id in leaves {
            let n = &self.layout.nodes[&id];
            if n.hidden || n.collapsed {
                continue;
            }
            let rect = n.content;
            let mut occluders: Vec<_> = floats
                .iter()
                .filter(|(_, raised)| !n.floating || *raised > n.raised)
                .map(|(r, _)| *r)
                .collect();
            occluders.extend(&self.overlays);
            let Some(pane) = self.panes.get_mut(&id) else {
                continue;
            };
            for mut request in pane.terminal.graphics().unwrap_or_default() {
                let g = &mut request.geometry;
                if g.col >= rect.w || g.row >= rect.h {
                    continue;
                }
                if g.col as u32 + g.cols as u32 > rect.w as u32 {
                    let keep = rect.w - g.col;
                    g.sw = if g.scale_cols != 0 {
                        (g.sw as u64 * keep as u64 / g.cols as u64) as u32
                    } else {
                        g.sw.min((keep as u32 * request.cell_px_w).saturating_sub(g.x_off))
                    };
                    g.cols = keep;
                }
                if g.row as u32 + g.rows as u32 > rect.h as u32 {
                    let keep = rect.h - g.row;
                    g.sh = if g.scale_rows != 0 {
                        (g.sh as u64 * keep as u64 / g.rows as u64) as u32
                    } else {
                        g.sh.min((keep as u32 * request.cell_px_h).saturating_sub(g.y_off))
                    };
                    g.rows = keep;
                }
                if g.cols == 0 || g.rows == 0 {
                    continue;
                }
                let whole = Rect {
                    x: rect.x + g.col,
                    y: rect.y + g.row,
                    w: g.cols,
                    h: g.rows,
                };
                for (piece, visible) in subtract(whole, &occluders).into_iter().enumerate() {
                    let mut r = request.clone();
                    let g = &mut r.geometry;
                    (g.sx, g.sw, g.x_off) = axis_crop(
                        g.scale_cols != 0,
                        (whole.w, visible.x - whole.x, visible.w),
                        r.cell_px_w,
                        (g.sx, g.sw, g.x_off),
                    );
                    (g.sy, g.sh, g.y_off) = axis_crop(
                        g.scale_rows != 0,
                        (whole.h, visible.y - whole.y, visible.h),
                        r.cell_px_h,
                        (g.sy, g.sh, g.y_off),
                    );
                    let cut = (visible.x, visible.y, visible.w, visible.h)
                        != (whole.x, whole.y, whole.w, whole.h);
                    if cut && (g.sw == 0 || g.sh == 0) {
                        continue;
                    }
                    r.pane = id;
                    r.piece = piece as u16;
                    g.col = visible.x;
                    g.row = visible.y;
                    g.cols = visible.w;
                    g.rows = visible.h;
                    if g.scale_cols != 0 {
                        g.scale_cols = visible.w;
                    }
                    if g.scale_rows != 0 {
                        g.scale_rows = visible.h;
                    }
                    self.graphics_requests.push(r);
                }
            }
        }
    }
    pub fn graphics_view<'a>(&self, state: &'a mut Graphics, view: Rect) -> &'a [u8] {
        state.begin();
        for request in &self.graphics_requests {
            if let Some(request) = request.crop_view(view) {
                state.place(&request);
            }
        }
        state.flush()
    }
    pub(super) fn graphics_bytes(&mut self) -> Vec<u8> {
        let mut state = std::mem::take(&mut self.graphics);
        let bytes = self
            .graphics_view(
                &mut state,
                Rect {
                    x: 0,
                    y: 0,
                    w: self.cols,
                    h: self.rows,
                },
            )
            .to_vec();
        self.graphics = state;
        bytes
    }
}
