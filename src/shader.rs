//! Ordered colour-only passes over pane contents and chrome.
use crate::{
    expr::{Environment, Program},
    screen::{Cell, Color, Screen},
};
use std::{cell::RefCell, rc::Rc};

pub const MAX_ATTACHED: usize = 4;
pub const MAX_CHAIN: usize = MAX_ATTACHED * 2 + 2;
pub const FG: u8 = 1;
pub const BG: u8 = 2;
pub const BOTH: u8 = FG | BG;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Context {
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
    pub now_ms: i64,
    pub state_ms: i64,
    pub focused: bool,
    pub has_cursor: bool,
    pub cursor_x: u16,
    pub cursor_y: u16,
    pub above: u32,
    pub below: u32,
    pub default_fg: Color,
    pub default_bg: Color,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dim,
    Grayscale,
    Tint,
    Vignette,
    Gradient,
    Zebra,
    Ruler,
    Margin,
    Spotlight,
}
impl Kind {
    pub const ALL: [Self; 9] = [
        Self::Dim,
        Self::Grayscale,
        Self::Tint,
        Self::Vignette,
        Self::Gradient,
        Self::Zebra,
        Self::Ruler,
        Self::Margin,
        Self::Spotlight,
    ];
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dim => "dim",
            Self::Grayscale => "grayscale",
            Self::Tint => "tint",
            Self::Vignette => "vignette",
            Self::Gradient => "gradient",
            Self::Zebra => "zebra",
            Self::Ruler => "ruler",
            Self::Margin => "margin",
            Self::Spotlight => "spotlight",
        }
    }
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

#[derive(Clone, Debug)]
pub struct Shader {
    pub kind: Kind,
    pub expression: Option<Rc<RefCell<Program>>>,
    pub color: Color,
    pub amount: u8,
    pub param: u16,
    pub channels: u8,
}
impl Shader {
    pub fn new(kind: Kind, color: Color, amount: u8) -> Self {
        Self {
            kind,
            color,
            amount,
            expression: None,
            param: 0,
            channels: 0,
        }
    }
}

fn mix(from: Color, to: Color, amount: u8) -> Color {
    let channel =
        |a, b| ((a as u32 * (255 - amount as u32) + b as u32 * amount as u32) / 255) as u8;
    Color::rgb(
        channel(from.r, to.r),
        channel(from.g, to.g),
        channel(from.b, to.b),
    )
}
fn dim(cell: &mut Cell, amount: u8) {
    cell.fg = mix(cell.fg, Color::rgb(0, 0, 0), amount);
    cell.bg = mix(cell.bg, Color::rgb(0, 0, 0), amount);
}
fn distance(dx: i64, dy: i64) -> i64 {
    dx * dx + dy * dy * 4
}

pub(crate) fn shade_cell(
    kind: Kind,
    color: Color,
    param: u16,
    amount: u8,
    ctx: &Context,
    cell: &mut Cell,
) {
    match kind {
        Kind::Dim => dim(cell, amount),
        Kind::Grayscale => {
            let gray = |c: Color| {
                let v = ((c.r as u32 * 77 + c.g as u32 * 150 + c.b as u32 * 29) >> 8) as u8;
                Color::rgb(v, v, v)
            };
            cell.fg = mix(cell.fg, gray(cell.fg), amount);
            cell.bg = mix(cell.bg, gray(cell.bg), amount);
        }
        Kind::Tint => {
            cell.fg = mix(cell.fg, color, amount);
            cell.bg = mix(cell.bg, color, amount);
        }
        Kind::Vignette => {
            let (cx, cy) = (ctx.cols as i64 / 2, ctx.rows as i64 / 2);
            let far = distance(cx, cy);
            if far > 0 {
                let d = distance(ctx.x as i64 - cx, ctx.y as i64 - cy).min(far);
                dim(cell, (amount as i64 * d / far) as u8);
            }
        }
        Kind::Gradient => {
            let (span, pos) = match param {
                1 => (ctx.rows, ctx.rows - 1 - ctx.y),
                2 => (ctx.cols, ctx.x),
                3 => (ctx.cols, ctx.cols - 1 - ctx.x),
                _ => (ctx.rows, ctx.y),
            };
            if span > 1 {
                let a = (amount as u32 * pos as u32 / (span - 1) as u32) as u8;
                cell.fg = mix(cell.fg, ctx.default_bg, a);
                cell.bg = mix(cell.bg, ctx.default_bg, a);
            }
        }
        Kind::Zebra => {
            if (ctx.y / param.max(1)) & 1 != 0 {
                dim(cell, amount);
            }
        }
        Kind::Ruler => {
            if ctx.x == param {
                cell.bg = mix(cell.bg, color, amount);
            }
        }
        Kind::Margin => {
            if ctx.x >= param {
                dim(cell, amount);
            }
        }
        Kind::Spotlight if ctx.has_cursor => {
            let r = if param == 0 { 10 } else { param as i64 };
            let inner = r * r;
            let d = distance(
                ctx.x as i64 - ctx.cursor_x as i64,
                ctx.y as i64 - ctx.cursor_y as i64,
            );
            if d > inner {
                let k = ((d - inner) * 255 / (inner * 3)).min(255);
                dim(cell, (amount as i64 * k / 255) as u8);
            }
        }
        Kind::Spotlight => {}
    }
}

pub fn apply(
    screen: &mut Screen,
    shaders: &[Shader],
    rect: Rect,
    hole: Option<Rect>,
    base: &Context,
) {
    for shader in shaders {
        let mut program = shader.expression.as_ref().map(|p| p.borrow_mut());
        apply_pass(
            screen,
            rect,
            hole,
            base,
            shader.amount,
            program.as_deref_mut(),
            shader.channels,
            |ctx, cell, amount| {
                shade_cell(shader.kind, shader.color, shader.param, amount, ctx, cell)
            },
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_pass(
    screen: &mut Screen,
    rect: Rect,
    hole: Option<Rect>,
    base: &Context,
    amount: u8,
    mut program: Option<&mut Program>,
    channels: u8,
    mut shade: impl FnMut(&Context, &mut Cell, u8),
) {
    if rect.x >= screen.cols || rect.y >= screen.rows || (program.is_none() && amount == 0) {
        return;
    }
    let w = rect.w.min(screen.cols - rect.x);
    let h = rect.h.min(screen.rows - rect.y);
    if w == 0 || h == 0 {
        return;
    }
    let mut ctx = Context {
        cols: w,
        rows: h,
        ..*base
    };
    let mut env = Environment {
        cols: w as i32,
        rows: h as i32,
        curx: if ctx.has_cursor {
            ctx.cursor_x as i32
        } else {
            0
        },
        cury: if ctx.has_cursor {
            ctx.cursor_y as i32
        } else {
            0
        },
        cursor: ctx.has_cursor as i32,
        focused: ctx.focused as i32,
        t: ctx.now_ms,
        since: ctx.state_ms,
        above: ctx.above as i32,
        below: ctx.below as i32,
        ..Environment::default()
    };
    let constant = program
        .as_ref()
        .and_then(|p| p.constant())
        .map(|v| v.clamp(0, 255) as u8);
    if constant == Some(0) {
        return;
    }
    let mapped = constant.is_none()
        && program
            .as_mut()
            .is_some_and(|p| p.amount_map(&env).is_some());
    for y in 0..h {
        ctx.y = y;
        env.y = y as i32;
        for x in 0..w {
            let gx = rect.x as u32 + x as u32;
            let gy = rect.y as u32 + y as u32;
            if hole.is_some_and(|r| {
                gx >= r.x as u32
                    && gx < r.x as u32 + r.w as u32
                    && gy >= r.y as u32
                    && gy < r.y as u32 + r.h as u32
            }) {
                continue;
            }
            let cell = &mut screen.cells[(rect.y as usize + y as usize) * screen.cols as usize
                + rect.x as usize
                + x as usize];
            if cell.width == 0 {
                continue;
            }
            ctx.x = x;
            env.x = x as i32;
            let a = match (constant, program.as_ref()) {
                (Some(a), _) => a,
                (_, Some(p)) if mapped => {
                    p.cached_amount_map()[y as usize * w as usize + x as usize]
                }
                (_, Some(p)) => p.eval(&env).clamp(0, 255) as u8,
                _ => amount,
            };
            if a == 0 {
                continue;
            }
            let (fg, bg) = (cell.fg, cell.bg);
            if !cell.fg.set {
                cell.fg = ctx.default_fg;
            }
            if !cell.bg.set {
                cell.bg = ctx.default_bg;
            }
            shade(&ctx, cell, a);
            if channels != 0 && channels & FG == 0 {
                cell.fg = fg;
            }
            if channels != 0 && channels & BG == 0 {
                cell.bg = bg;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        Context {
            default_fg: Color::rgb(255, 255, 255),
            default_bg: Color::rgb(0, 0, 0),
            ..Context::default()
        }
    }

    #[test]
    fn expression_strengths_preserve_zero_cells_and_invalidated_maps() {
        let mut screen = Screen::new(4, 2);
        let mut shader = Shader::new(Kind::Tint, Color::rgb(255, 0, 0), 128);
        shader.channels = FG;
        shader.expression = Some(Rc::new(RefCell::new(
            Program::compile("focused * (x % 2) * 255").unwrap(),
        )));
        let rect = Rect {
            w: 4,
            h: 2,
            ..Rect::default()
        };
        let idle = context();
        apply(&mut screen, &[shader.clone()], rect, None, &idle);
        assert!(screen.cells.iter().all(|c| !c.fg.set && !c.bg.set));
        let focused = Context {
            focused: true,
            ..idle
        };
        apply(&mut screen, &[shader], rect, None, &focused);
        for (i, cell) in screen.cells.iter().enumerate() {
            assert_eq!(cell.fg.set, i % 2 == 1);
            if cell.fg.set {
                assert_eq!(cell.fg, Color::rgb(255, 0, 0));
            }
            assert!(!cell.bg.set);
        }
    }

    #[test]
    fn animation_skips_holes_wide_tails_and_keeps_cell_identity() {
        let mut screen = Screen::new(4, 2);
        let original = screen.cells.clone();
        screen.cells[1].width = 0;
        let mut shader = Shader::new(Kind::Tint, Color::rgb(0, 255, 0), 128);
        shader.expression = Some(Rc::new(RefCell::new(
            Program::compile("(since < 900) * 255").unwrap(),
        )));
        let rect = Rect {
            w: u16::MAX,
            h: u16::MAX,
            ..Rect::default()
        };
        let hole = Rect {
            x: 2,
            y: 0,
            w: 2,
            h: 1,
        };
        let late = Context {
            state_ms: 900,
            ..context()
        };
        apply(&mut screen, &[shader.clone()], rect, Some(hole), &late);
        assert!(screen.cells.iter().all(|c| !c.fg.set));
        let early = Context {
            state_ms: 899,
            ..late
        };
        apply(&mut screen, &[shader], rect, Some(hole), &early);
        for (i, cell) in screen.cells.iter().enumerate() {
            assert_eq!(cell.fg.set, !matches!(i, 1..=3));
            assert_eq!(cell.text, original[i].text);
            assert_eq!(cell.len, original[i].len);
            assert_eq!(cell.attrs, original[i].attrs);
            assert_eq!(cell.width, if i == 1 { 0 } else { original[i].width });
        }
    }
}
