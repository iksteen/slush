//! Kitty wire state belongs to a display; only accepted frames advance it.
use crate::{osc::encode_base64, shader::Rect};
use serde_json::{Value, json};
use std::{fmt::Write, sync::Arc};

#[derive(Clone, Debug, Default)]
pub struct Request {
    pub pane: u32,
    pub src_id: u32,
    pub generation: u64,
    pub place_id: u32,
    pub piece: u16,
    pub geometry: Geometry,
    pub px_w: u32,
    pub px_h: u32,
    pub cell_px_w: u32,
    pub cell_px_h: u32,
    pub format: i32,
    pub compression: i32,
    pub data: Arc<[u8]>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Geometry {
    pub col: u16,
    pub row: u16,
    pub cols: u16,
    pub rows: u16,
    pub scale_cols: u16,
    pub scale_rows: u16,
    pub x_off: u32,
    pub y_off: u32,
    pub sx: u32,
    pub sy: u32,
    pub sw: u32,
    pub sh: u32,
}
#[derive(Clone, Debug)]
struct Image {
    pane: u32,
    src_id: u32,
    out_id: u32,
    generation: u64,
    sent: bool,
}
#[derive(Clone, Debug)]
pub struct Placement {
    out_id: u32,
    place_id: u32,
    piece: u16,
    wire_id: u32,
    geometry: Geometry,
    live: bool,
    shown: bool,
}
pub struct Graphics {
    images: Vec<Image>,
    places: Vec<Placement>,
    dead: Vec<(u32, u32)>,
    transmitted: Vec<usize>,
    placed: Vec<usize>,
    next_image: u32,
    next_place: u32,
    out: String,
}
impl Default for Graphics {
    fn default() -> Self {
        Self {
            images: Vec::new(),
            places: Vec::new(),
            dead: Vec::new(),
            transmitted: Vec::new(),
            placed: Vec::new(),
            next_image: 0x51000000,
            next_place: 1,
            out: String::new(),
        }
    }
}
impl Graphics {
    pub fn reset(&mut self) {
        for image in &mut self.images {
            image.sent = false;
        }
        self.places.clear();
        self.dead.clear();
        self.transmitted.clear();
        self.placed.clear();
    }
    pub fn repaint(&mut self) {
        for placement in &mut self.places {
            placement.shown = false;
        }
    }
    pub fn forget_pane(&mut self, pane: u32) {
        self.commit(false);
        self.images.retain(|i| i.pane != pane);
    }
    pub fn begin(&mut self) {
        self.commit(false);
        for placement in &mut self.places {
            placement.live = false;
        }
        self.out.clear();
        self.out.push_str("\x1b7");
    }
    pub fn place(&mut self, request: &Request) {
        let index = self
            .images
            .iter()
            .position(|i| (i.pane, i.src_id) == (request.pane, request.src_id))
            .unwrap_or_else(|| {
                let out_id = self.next_image;
                self.next_image = self.next_image.wrapping_add(1);
                self.images.push(Image {
                    pane: request.pane,
                    src_id: request.src_id,
                    out_id,
                    generation: request.generation,
                    sent: false,
                });
                self.images.len() - 1
            });
        let image = &mut self.images[index];
        if image.generation != request.generation {
            image.generation = request.generation;
            image.sent = false;
        }
        let mut transmitted = false;
        if !image.sent && !request.data.is_empty() {
            let format = match request.format {
                0 => 24,
                1 => 32,
                2 => 100,
                _ => 32,
            };
            for (n, chunk) in request.data.chunks(2304).enumerate() {
                let more = u8::from((n + 1) * 2304 < request.data.len());
                self.out.push_str("\x1b_G");
                if n == 0 {
                    write!(
                        self.out,
                        "a=t,q=2,i={},f={format},s={},v={}",
                        image.out_id, request.px_w, request.px_h
                    )
                    .unwrap();
                    if request.compression == 1 {
                        self.out.push_str(",o=z");
                    }
                    write!(self.out, ",m={more};").unwrap();
                } else {
                    write!(self.out, "m={more};").unwrap();
                }
                self.out.push_str(&encode_base64(chunk));
                self.out.push_str("\x1b\\");
            }
            image.sent = true;
            transmitted = true;
            self.transmitted.push(index);
            for placement in &mut self.places {
                if placement.out_id == image.out_id {
                    placement.shown = false;
                }
            }
        }
        let out_id = image.out_id;
        let place_id = request.place_id.max(1);
        let index = self
            .places
            .iter()
            .position(|p| (p.out_id, p.place_id, p.piece) == (out_id, place_id, request.piece))
            .unwrap_or_else(|| {
                let wire_id = self.next_place;
                self.next_place = self.next_place.wrapping_add(1);
                self.places.push(Placement {
                    out_id,
                    place_id,
                    piece: request.piece,
                    wire_id,
                    geometry: Geometry::default(),
                    live: false,
                    shown: false,
                });
                self.places.len() - 1
            });
        let placement = &mut self.places[index];
        placement.shown = placement.shown && !transmitted && placement.geometry == request.geometry;
        placement.geometry = request.geometry;
        placement.live = true;
    }
    pub fn flush(&mut self) -> &[u8] {
        for placement in self.places.iter().filter(|p| !p.live) {
            let dead = (placement.out_id, placement.wire_id);
            if !self.dead.contains(&dead) {
                self.dead.push(dead);
            }
        }
        self.places.retain(|p| p.live);
        for (image, placement) in &self.dead {
            write!(self.out, "\x1b_Ga=d,d=i,q=2,i={image},p={placement}\x1b\\").unwrap();
        }
        for (index, placement) in self.places.iter_mut().enumerate() {
            if placement.shown {
                continue;
            }
            placement.shown = true;
            self.placed.push(index);
            let g = placement.geometry;
            write!(
                self.out,
                "\x1b[{};{}H\x1b_Ga=p,q=2,C=1,z=0,i={},p={}",
                g.row as u32 + 1,
                g.col as u32 + 1,
                placement.out_id,
                placement.wire_id
            )
            .unwrap();
            if g.scale_cols != 0 || g.scale_rows != 0 {
                write!(self.out, ",c={},r={}", g.scale_cols, g.scale_rows).unwrap();
            }
            if g.x_off != 0 {
                write!(self.out, ",X={}", g.x_off).unwrap();
            }
            if g.y_off != 0 {
                write!(self.out, ",Y={}", g.y_off).unwrap();
            }
            if g.sw != 0 || g.sh != 0 {
                write!(self.out, ",x={},y={},w={},h={}", g.sx, g.sy, g.sw, g.sh).unwrap();
            }
            self.out.push_str("\x1b\\");
        }
        if self.out.len() == 2 {
            self.out.clear();
        } else {
            self.out.push_str("\x1b8");
        }
        self.out.as_bytes()
    }
    pub fn commit(&mut self, delivered: bool) {
        if delivered {
            self.dead.clear();
        } else {
            for index in &self.transmitted {
                if let Some(image) = self.images.get_mut(*index) {
                    image.sent = false;
                }
            }
            for index in &self.placed {
                if let Some(placement) = self.places.get_mut(*index) {
                    placement.shown = false;
                }
            }
        }
        self.transmitted.clear();
        self.placed.clear();
    }
    pub fn json(&self) -> Value {
        Value::Array(self.places.iter().map(|p| { let g = p.geometry; json!({"image":p.out_id,"placement":p.place_id,"piece":p.piece,"x":g.col,"y":g.row,"cols":g.cols,"rows":g.rows,"x_off":g.x_off,"y_off":g.y_off}) }).collect())
    }
}

impl Request {
    /// Crop a canonical placement and translate it into a client's coordinates.
    pub fn crop_view(&self, view: Rect) -> Option<Self> {
        let mut out = self.clone();
        let g = &mut out.geometry;
        let (vx1, vy1) = (view.x as u32 + view.w as u32, view.y as u32 + view.h as u32);
        if g.col as u32 + g.cols as u32 <= view.x as u32
            || g.col as u32 >= vx1
            || g.row as u32 + g.rows as u32 <= view.y as u32
            || g.row as u32 >= vy1
        {
            return None;
        }
        if g.col < view.x {
            let cut = view.x - g.col;
            let drop = if g.scale_cols != 0 {
                (g.sw as u64 * cut as u64 / g.cols as u64) as u32
            } else {
                (cut as u32 * out.cell_px_w)
                    .saturating_sub(g.x_off)
                    .min(g.sw)
            };
            g.sx += drop;
            g.sw -= drop;
            if g.scale_cols == 0 {
                g.x_off = 0;
            }
            g.col = view.x;
            g.cols -= cut;
        }
        if g.row < view.y {
            let cut = view.y - g.row;
            let drop = if g.scale_rows != 0 {
                (g.sh as u64 * cut as u64 / g.rows as u64) as u32
            } else {
                (cut as u32 * out.cell_px_h)
                    .saturating_sub(g.y_off)
                    .min(g.sh)
            };
            g.sy += drop;
            g.sh -= drop;
            if g.scale_rows == 0 {
                g.y_off = 0;
            }
            g.row = view.y;
            g.rows -= cut;
        }
        if g.col as u32 + g.cols as u32 > vx1 {
            let keep = (vx1 - g.col as u32) as u16;
            g.sw = if g.scale_cols != 0 {
                (g.sw as u64 * keep as u64 / g.cols as u64) as u32
            } else {
                g.sw.min((keep as u32 * out.cell_px_w).saturating_sub(g.x_off))
            };
            g.cols = keep;
        }
        if g.row as u32 + g.rows as u32 > vy1 {
            let keep = (vy1 - g.row as u32) as u16;
            g.sh = if g.scale_rows != 0 {
                (g.sh as u64 * keep as u64 / g.rows as u64) as u32
            } else {
                g.sh.min((keep as u32 * out.cell_px_h).saturating_sub(g.y_off))
            };
            g.rows = keep;
        }
        if g.cols == 0 || g.rows == 0 {
            return None;
        }
        if g.scale_cols != 0 {
            g.scale_cols = g.cols;
        }
        if g.scale_rows != 0 {
            g.scale_rows = g.rows;
        }
        g.col -= view.x;
        g.row -= view.y;
        Some(out)
    }
}
