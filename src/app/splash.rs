//! The attach greeting uses deterministic particles and ordinary shader passes.
use super::App;
use crate::{
    expr::Program,
    screen::{self, BOLD, Color, Screen},
    shader::{self, Context, Kind, Rect, Shader},
};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const LOGO: &str = include_str!("../../assets/logo.txt");
type Pass = (Kind, u32, &'static str);
const EFFECTS: &[&[Pass]] = &[
    &[
        (Kind::Tint, 0xff2d95, "128 + sin(x * 9 - t / 3) / 2"),
        (Kind::Tint, 0x00e5ff, "128 + sin(x * 9 - t / 3 + 120) / 2"),
        (Kind::Tint, 0xffd400, "128 + sin(x * 9 - t / 3 + 240) / 2"),
    ],
    &[
        (
            Kind::Tint,
            0xff2d95,
            "128 + sin(x * 21 + t / 4) / 4 + sin(y * 47 - t / 6) / 4",
        ),
        (
            Kind::Tint,
            0x2de5ff,
            "128 + sin(x * 17 - t / 5) / 4 + sin(y * 61 + t / 7) / 4",
        ),
    ],
    &[
        (
            Kind::Tint,
            0xff3b00,
            "clamp(y * 34 + sin(x * 53 + t / 3) / 6, 0, 255)",
        ),
        (
            Kind::Tint,
            0xffc400,
            "max(0, (y - rows / 2) * 40 + sin(x * 97 - t / 2) / 5)",
        ),
    ],
    &[
        (
            Kind::Tint,
            0x7a5cff,
            "128 + sin(dist(x, y, cols / 2, rows / 2) * 55 - t / 2) / 2",
        ),
        (
            Kind::Tint,
            0x00ffa8,
            "128 + sin(dist(x, y, cols / 2, rows / 2) * 55 - t / 2 + 120) / 2",
        ),
        (
            Kind::Tint,
            0xff2d95,
            "128 + sin(dist(x, y, cols / 2, rows / 2) * 55 - t / 2 + 240) / 2",
        ),
    ],
    &[
        (
            Kind::Tint,
            0x00e5ff,
            "((x * 31 + y * 83 + (t / 70) * 47) % 89 < 9) * 255",
        ),
        (
            Kind::Tint,
            0xff2d95,
            "((x * 53 + y * 29 + (t / 90) * 31) % 97 < 9) * 255",
        ),
        (Kind::Dim, 0, "(x * 500 > since * cols) * 255"),
    ],
    &[
        (
            Kind::Tint,
            0x19ff8c,
            "128 + sin(x * 13 + sin(y * 40 + t / 9) / 8 + t / 6) / 2",
        ),
        (Kind::Tint, 0x8c5cff, "128 + sin(x * 11 - t / 8 + 90) / 3"),
        (Kind::Tint, 0x00d0ff, "max(0, sin(x * 7 + t / 5) / 2)"),
    ],
];
pub(super) struct Splash {
    since: Instant,
    until: Instant,
    duration: u64,
    seed: u32,
    pub effect: usize,
    motion: usize,
}
struct Particle {
    x: i32,
    y: i32,
    glyph: char,
}
fn particles(x0: i32, y0: i32) -> Vec<Particle> {
    LOGO.lines()
        .enumerate()
        .flat_map(|(y, line)| {
            line.chars().enumerate().filter_map(move |(x, glyph)| {
                (glyph != ' ').then_some(Particle {
                    x: x0 + x as i32,
                    y: y0 + y as i32,
                    glyph,
                })
            })
        })
        .take(512)
        .collect()
}
fn place(
    motion: usize,
    all: &[Particle],
    index: usize,
    seed: u32,
    viewport: (u16, u16),
    since: i64,
    assembly: i64,
) -> (i32, i32) {
    let (cols, rows) = viewport;
    let p = &all[index];
    let hash = (index as u32).wrapping_mul(2654435761) ^ seed.wrapping_mul(2246822519);
    let (mut sx, mut sy, delay) = match motion {
        0 => (p.x, -1 - (hash % 12) as i32, (hash % 160) as i32),
        1 => (
            (hash % u32::from(cols.max(1))) as i32,
            ((hash >> 9) % u32::from(rows.max(1))) as i32,
            ((hash >> 18) % 96) as i32,
        ),
        2 => (
            if p.y % 2 != 0 {
                i32::from(cols) + 2 + (hash % 8) as i32
            } else {
                -3 - (hash % 8) as i32
            },
            p.y,
            (p.y * 31) % 128,
        ),
        4 => {
            let j = (index + 1 + (seed as usize % all.len().saturating_sub(1).max(1))) % all.len();
            (all[j].x, all[j].y, (hash % 112) as i32)
        }
        _ => (p.x, p.y, (hash % 64) as i32),
    };
    let progress = if assembly <= 0 || since >= assembly {
        256
    } else {
        ((since * 256 / assembly - delay.min(224) as i64) * 256 / (256 - delay.min(224)) as i64)
            .clamp(0, 256) as i32
    };
    let q = 256 - progress;
    let e = 256 - q * q / 256 * q / 256;
    sx += (p.x - sx) * e / 256;
    sy += (p.y - sy) * e / 256;
    if motion == 3 && e < 256 {
        let radius = (24 + (hash % 20) as i32) * (256 - e) / 256;
        let angle = (hash % 360) as i32 + e * 2;
        sx += radius * crate::expr::sin(angle + 90) / 255;
        sy += radius * crate::expr::sin(angle) / 510;
    }
    (sx, sy)
}
impl App {
    pub fn splash(&mut self, effect: i64, motion: i64) {
        let duration = self.config.number("splash_ms");
        if duration == 0 {
            return;
        }
        if self.splash_chains.is_empty() {
            self.splash_chains = EFFECTS
                .iter()
                .map(|passes| {
                    passes
                        .iter()
                        .map(|&(kind, rgb, expression)| {
                            let mut pass = Shader::new(
                                kind,
                                Color::rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8),
                                128,
                            );
                            pass.channels = shader::FG;
                            pass.expression = Some(Rc::new(RefCell::new(
                                Program::compile(expression).expect("built-in splash expression"),
                            )));
                            pass
                        })
                        .collect()
                })
                .collect();
        }
        let now = Instant::now();
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .wrapping_add(duration as u128) as u32;
        self.splash = Some(Splash {
            since: now,
            until: now + Duration::from_millis(duration),
            duration,
            seed,
            effect: if effect < 0 {
                seed as usize % EFFECTS.len()
            } else {
                effect as usize % EFFECTS.len()
            },
            motion: if motion < 0 {
                (seed as usize / EFFECTS.len()) % 5
            } else {
                motion as usize % 5
            },
        });
    }
    pub(super) fn splash_deadline(&self) -> Option<i64> {
        let splash = self.splash.as_ref()?;
        let remaining = splash
            .until
            .saturating_duration_since(Instant::now())
            .as_millis() as i64;
        let anim = self.config.number("anim_ms") as i64;
        Some(if anim > 0 {
            remaining.min(anim)
        } else {
            remaining
        })
    }
    pub(super) fn draw_splash(&mut self, s: &mut Screen) {
        let Some(splash) = &self.splash else { return };
        if Instant::now() >= splash.until {
            self.splash = None;
            return;
        }
        let w = LOGO.lines().map(screen::cells).max().unwrap_or(0);
        let bh = LOGO.lines().count() as u16 + 2;
        let bw = w + 4;
        if bw > s.cols || bh > s.rows {
            return;
        }
        let r = Rect {
            x: (s.cols - bw) / 2,
            y: (s.rows - bh) / 2,
            w: bw,
            h: bh,
        };
        self.overlays.push(r);
        for y in r.y..r.y + r.h {
            s.text(
                r.x,
                y,
                &" ".repeat(r.w as usize),
                Default::default(),
                Default::default(),
                0,
            );
        }
        let all = particles(i32::from(r.x) + 2, i32::from(r.y) + 1);
        let age = splash.since.elapsed().as_millis() as i64;
        for (i, p) in all.iter().enumerate() {
            let (x, y) = place(
                splash.motion,
                &all,
                i,
                splash.seed,
                (s.cols, s.rows),
                age,
                (splash.duration * 2 / 5) as i64,
            );
            if x >= 0 && y >= 0 && x < i32::from(s.cols) && y < i32::from(s.rows) {
                s.text(
                    x as u16,
                    y as u16,
                    &p.glyph.to_string(),
                    self.config.color("default_fg"),
                    Default::default(),
                    BOLD,
                );
            }
        }
        let context = Context {
            now_ms: self.started.elapsed().as_millis() as i64,
            state_ms: age,
            default_fg: self.config.color("default_fg"),
            default_bg: self.config.color("default_bg"),
            ..Context::default()
        };
        shader::apply(s, &self.splash_chains[splash.effect], r, None, &context);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn control_indices_follow_c_numeric_conversion() {
        use crate::{config::Config, input::Parser, json_control::parse_request};

        let mut app = App::new(vec!["/bin/cat".into()], 80, 24).unwrap();
        app.config = Config::default();
        let mut screen = Screen::new(80, 24);
        let mut input = Parser::default();
        // Expected values are jv_geti's decimal conversion, narrowed to the
        // C int accepted by app_splash_fx (before effect/motion modulo).
        for (token, index) in [
            (b"0".as_slice(), 0_i32),
            (b"4.9", 4),
            (b"1e1", 10),
            (b"+0x1p2", 4),
            (br#""+12suffix""#, 12),
            (br#""\t2tail""#, 2),
            (br#""0x12""#, 0),
            (br#""bad""#, 0),
            (br#""""#, 0),
            (b"\"\xff4\"", 0),
            (b"\"4\xff\"", 4),
            (br#""4\u0000ignored""#, 4),
            (b"true", -1),
            (b"null", -1),
            (b"[]", -1),
            (b"-1.9", -1),
            (b"4294967300", 4),
            (b"4294967295", -1),
            (b"2147483648", i32::MIN),
        ] {
            let mut command = b"{\"cmd\":\"splash\",\"fx\":".to_vec();
            command.extend_from_slice(token);
            command.extend_from_slice(b",\"motion\":");
            command.extend_from_slice(token);
            command.push(b'}');
            let request = parse_request(&command).unwrap();
            assert_eq!(
                app.command_json(&mut screen, &mut input, &request),
                br#"{"ok":true}"#,
            );
            let splash = app.splash.as_ref().unwrap();
            let expected = if index < 0 {
                (
                    splash.seed as usize % EFFECTS.len(),
                    (splash.seed as usize / EFFECTS.len()) % 5,
                )
            } else {
                (index as usize % EFFECTS.len(), index as usize % 5)
            };
            assert_eq!((splash.effect, splash.motion), expected, "{token:?}");
        }
    }

    #[test]
    fn every_particle_lands_exactly_for_every_motion_and_seed() {
        let all = particles(18, 8);
        assert!(!all.is_empty());
        for motion in 0..5 {
            for seed in [0, 1, 384221, u32::MAX] {
                for (i, p) in all.iter().enumerate() {
                    assert_eq!(place(motion, &all, i, seed, (80, 24), 640, 640), (p.x, p.y));
                    assert_eq!(place(motion, &all, i, seed, (80, 24), 0, 0), (p.x, p.y));
                }
            }
        }
        for passes in EFFECTS {
            for &(_, _, expression) in *passes {
                assert!(Program::compile(expression).is_ok());
            }
        }
    }
}
