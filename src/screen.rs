//! Canonical cell buffer, client projections, hyperlinks, and terminal diffs.
use crate::ghostty::ffi;
use serde_json::{Value, json};

pub const BOLD: u16 = 1;
pub const DIM: u16 = 2;
pub const ITALIC: u16 = 4;
pub const UNDERLINE: u16 = 8;
pub const BLINK: u16 = 16;
pub const INVERSE: u16 = 32;
pub const INVISIBLE: u16 = 64;
pub const STRIKE: u16 = 128;

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct Color {
    pub set: bool,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}
impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { set: true, r, g, b }
    }
    pub fn hex(self) -> Option<String> {
        self.set
            .then(|| format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b))
    }
}
impl PartialEq for Color {
    fn eq(&self, other: &Self) -> bool {
        self.set == other.set
            && (!self.set || (self.r, self.g, self.b) == (other.r, other.g, other.b))
    }
}
impl Eq for Color {}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct Cell {
    pub text: [u8; 16],
    pub len: u8,
    pub width: u8,
    pub attrs: u16,
    pub fg: Color,
    pub bg: Color,
}
impl Cell {
    pub fn blank() -> Self {
        let mut cell = Self {
            len: 1,
            width: 1,
            ..Self::default()
        };
        cell.text[0] = b' ';
        cell
    }
    fn same_style(self, other: Self) -> bool {
        self.attrs == other.attrs && self.fg == other.fg && self.bg == other.bg
    }
    fn bytes(&self) -> &[u8] {
        &self.text[..self.len as usize]
    }
}
impl PartialEq for Cell {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len
            && self.width == other.width
            && self.same_style(*other)
            && self.bytes() == other.bytes()
    }
}
impl Eq for Cell {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub visible: bool,
    pub x: u16,
    pub y: u16,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub action: Vec<u8>,
}
impl Hit {
    pub fn action_text(&self) -> &str {
        std::str::from_utf8(&self.action).unwrap_or_default()
    }
}
#[derive(Debug, Clone, Default)]
pub struct Hits(pub Vec<Hit>);
impl Hits {
    pub fn reset(&mut self) {
        self.0.clear();
    }
    pub fn add(&mut self, x: u16, y: u16, w: u16, h: u16, action: &str) {
        self.add_bytes(x, y, w, h, action.as_bytes());
    }
    pub fn add_bytes(&mut self, x: u16, y: u16, w: u16, h: u16, action: &[u8]) {
        let action = action.split(|b| *b == 0).next().unwrap_or_default();
        let length = action.len().min(47);
        self.0.push(Hit {
            x,
            y,
            w,
            h,
            action: action[..length].to_vec(),
        });
    }
    /// ASCII action consumers use this view; native picker actions use bytes.
    pub fn test(&self, x: u16, y: u16) -> Option<&str> {
        self.test_bytes(x, y)
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
    }
    pub fn test_bytes(&self, x: u16, y: u16) -> Option<&[u8]> {
        self.0
            .iter()
            .rev()
            .find(|hit| {
                x >= hit.x
                    && y >= hit.y
                    && (x as u32) < hit.x as u32 + hit.w as u32
                    && (y as u32) < hit.y as u32 + hit.h as u32
            })
            .map(|hit| hit.action.as_slice())
    }
    pub fn find(&self, action: &str) -> Option<&Hit> {
        self.0
            .iter()
            .rev()
            .find(|hit| hit.action == action.as_bytes())
    }
}

pub struct Screen {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
    pub(crate) previous: Vec<Cell>,
    pub cell_links: Vec<u16>,
    pub(crate) previous_links: Vec<u16>,
    pub links: Vec<String>,
    pub cursor: Cursor,
    pub(crate) shown_cursor: Cursor,
    pub hits: Hits,
    pub force_full: bool,
    pub(crate) output: Vec<u8>,
}

impl Screen {
    pub fn new(cols: u16, rows: u16) -> Self {
        let count = cols as usize * rows as usize;
        Self {
            cols,
            rows,
            cells: vec![Cell::blank(); count],
            previous: vec![Cell::default(); count],
            cell_links: vec![0; count],
            previous_links: vec![0; count],
            links: Vec::new(),
            cursor: Cursor::default(),
            shown_cursor: Cursor::default(),
            hits: Hits::default(),
            force_full: true,
            output: Vec::new(),
        }
    }
    pub fn resize(&mut self, cols: u16, rows: u16) {
        if self.cols == cols && self.rows == rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        let count = cols as usize * rows as usize;
        self.cells = vec![Cell::blank(); count];
        self.previous = vec![Cell::default(); count];
        self.cell_links = vec![0; count];
        self.previous_links = vec![0; count];
        self.force_full = true;
    }
    pub fn clear(&mut self) {
        self.cells.fill(Cell::blank());
        self.cell_links.fill(0);
    }
    pub fn index(&self, x: u16, y: u16) -> Option<usize> {
        (x < self.cols && y < self.rows).then_some(y as usize * self.cols as usize + x as usize)
    }
    pub fn at(&self, x: u16, y: u16) -> Option<&Cell> {
        self.index(x, y).map(|index| &self.cells[index])
    }
    pub fn at_mut(&mut self, x: u16, y: u16) -> Option<&mut Cell> {
        self.index(x, y).map(|index| &mut self.cells[index])
    }
    pub fn link_id(&mut self, uri: &str) -> u16 {
        if uri.is_empty() || uri.len() > 1024 || uri.bytes().any(|byte| byte < 32 || byte == 127) {
            return 0;
        }
        if let Some(index) = self.links.iter().position(|link| link == uri) {
            return (index + 1) as u16;
        }
        if self.links.len() >= 4096 {
            return 0;
        }
        self.links.push(uri.into());
        self.links.len() as u16
    }
    pub fn set_link(&mut self, x: u16, y: u16, id: u16) {
        if let Some(index) = self.index(x, y) {
            self.cell_links[index] = id;
        }
    }
    pub fn put(&mut self, x: u16, y: u16, text: &[u8], fg: Color, bg: Color, attrs: u16) {
        let Some(index) = self.index(x, y) else {
            return;
        };
        let mut length = text.len().min(16);
        if text.len() > 16 {
            while length > 0 && text[length] & 0xc0 == 0x80 {
                length -= 1;
            }
        }
        let text = &text[..length];
        let mut cell = Cell {
            len: length as u8,
            width: width_of(text),
            fg,
            bg,
            attrs,
            ..Default::default()
        };
        cell.text[..length].copy_from_slice(text);
        self.cell_links[index] = 0;
        if cell.width == 2 {
            if x as u32 + 1 < self.cols as u32 {
                self.cells[index + 1] = Cell {
                    fg,
                    bg,
                    ..Default::default()
                };
                self.cell_links[index + 1] = 0;
            } else {
                cell.width = 1;
            }
        }
        self.cells[index] = cell;
    }
    pub fn text(&mut self, x: u16, y: u16, text: &str, fg: Color, bg: Color, attrs: u16) -> u16 {
        self.text_bytes(x, y, text.as_bytes(), fg, bg, attrs)
    }
    pub fn text_bytes(
        &mut self,
        x: u16,
        y: u16,
        text: &[u8],
        fg: Color,
        bg: Color,
        attrs: u16,
    ) -> u16 {
        let text = text.split(|b| *b == 0).next().unwrap_or_default();
        let mut columns = 0u16;
        let mut at = 0;
        while at < text.len() {
            let col = x as u32 + columns as u32;
            if col >= self.cols as u32 {
                break;
            }
            let length = utf8_len(text[at]).min(text.len() - at);
            self.put(col as u16, y, &text[at..at + length], fg, bg, attrs);
            columns += self.at(col as u16, y).map_or(1, |cell| cell.width.max(1)) as u16;
            at += length;
        }
        columns
    }

    pub fn project(&mut self, source: &Self, x0: u16, y0: u16) {
        self.clear();
        self.hits.reset();
        self.cursor.visible = false;
        for y in 0..self.rows {
            let sy = y0 as u32 + y as u32;
            if sy >= source.rows as u32 {
                break;
            }
            let mut x = 0;
            while x < self.cols {
                let sx = x0 as u32 + x as u32;
                if sx >= source.cols as u32 {
                    break;
                }
                let si = sy as usize * source.cols as usize + sx as usize;
                let di = y as usize * self.cols as usize + x as usize;
                let cell = source.cells[si];
                if cell.width != 0
                    && !(cell.width == 2
                        && (sx + 1 >= source.cols as u32 || x as u32 + 1 >= self.cols as u32))
                {
                    for offset in 0..cell.width.max(1) as usize {
                        self.cells[di + offset] = source.cells[si + offset];
                        let link = source.cell_links[si + offset];
                        if let Some(uri) = link
                            .checked_sub(1)
                            .and_then(|id| source.links.get(id as usize))
                        {
                            self.cell_links[di + offset] = self.link_id(uri);
                        }
                    }
                    x += cell.width.max(1) as u16;
                } else {
                    x += 1;
                }
            }
        }
        if source.cursor.visible
            && source.cursor.x >= x0
            && source.cursor.y >= y0
            && (source.cursor.x as u32) < x0 as u32 + self.cols as u32
            && (source.cursor.y as u32) < y0 as u32 + self.rows as u32
        {
            self.cursor = Cursor {
                visible: true,
                x: source.cursor.x - x0,
                y: source.cursor.y - y0,
            };
        }
    }

    pub fn follow_cursor(&self, cols: u16, rows: u16, x: u16, y: u16) -> (u16, u16) {
        fn follow(position: u16, total: u16, view: u16, cursor: Option<u16>) -> u16 {
            let max = total.saturating_sub(view);
            if max == 0 {
                return 0;
            }
            let position = if let Some(cursor) = cursor {
                if cursor < position {
                    cursor
                } else if cursor as u32 >= position as u32 + view as u32 {
                    cursor.saturating_sub(view).saturating_add(1)
                } else {
                    position
                }
            } else {
                position
            };
            position.min(max)
        }
        (
            follow(
                x,
                self.cols,
                cols,
                self.cursor.visible.then_some(self.cursor.x),
            ),
            follow(
                y,
                self.rows,
                rows,
                self.cursor.visible.then_some(self.cursor.y),
            ),
        )
    }

    pub fn render(&mut self) -> &[u8] {
        self.output.clear();
        let mut painted = false;
        if self.force_full {
            self.output
                .extend_from_slice(b"\x1b[?25l\x1b[0m\x1b[H\x1b[2J");
            self.previous.fill(Cell::default());
            self.previous_links.fill(0);
            painted = true;
        }
        let mut position = None;
        let mut style: Option<Cell> = None;
        let mut open_link = 0;
        for y in 0..self.rows {
            for x in 0..self.cols {
                let index = y as usize * self.cols as usize + x as usize;
                let cell = self.cells[index];
                if cell.width == 0
                    || (cell == self.previous[index]
                        && self.cell_links[index] == self.previous_links[index])
                {
                    continue;
                }
                if !painted {
                    self.output.extend_from_slice(b"\x1b[?25l");
                    painted = true;
                }
                if position != Some((x, y)) {
                    self.output.extend_from_slice(
                        format!("\x1b[{};{}H", y as u32 + 1, x as u32 + 1).as_bytes(),
                    );
                }
                if !style.is_some_and(|style| style.same_style(cell)) {
                    sgr(&mut self.output, cell);
                    style = Some(cell);
                }
                if self.cell_links[index] != open_link {
                    if open_link != 0 {
                        self.output.extend_from_slice(b"\x1b]8;;\x1b\\");
                    }
                    open_link = self.cell_links[index];
                    if let Some(uri) = open_link
                        .checked_sub(1)
                        .and_then(|id| self.links.get(id as usize))
                    {
                        self.output.extend_from_slice(b"\x1b]8;;");
                        self.output.extend_from_slice(uri.as_bytes());
                        self.output.extend_from_slice(b"\x1b\\");
                    }
                }
                self.output
                    .extend_from_slice(if cell.len == 0 { b" " } else { cell.bytes() });
                self.previous[index] = cell;
                self.previous_links[index] = self.cell_links[index];
                if cell.width == 2 && x as u32 + 1 < self.cols as u32 {
                    self.previous[index + 1] = self.cells[index + 1];
                    self.previous_links[index + 1] = self.cell_links[index + 1];
                }
                let next_x = x as u32 + cell.width.max(1) as u32;
                position = (next_x < self.cols as u32).then_some((next_x as u16, y));
            }
        }
        if open_link != 0 {
            self.output.extend_from_slice(b"\x1b]8;;\x1b\\");
        }
        if painted || self.cursor != self.shown_cursor {
            if self.cursor.visible && self.cursor.x < self.cols && self.cursor.y < self.rows {
                self.output.extend_from_slice(
                    format!(
                        "\x1b[{};{}H\x1b[?25h",
                        self.cursor.y as u32 + 1,
                        self.cursor.x as u32 + 1
                    )
                    .as_bytes(),
                );
            } else if painted {
                self.output.extend_from_slice(b"\x1b[?25l");
            }
            self.shown_cursor = self.cursor;
        }
        self.force_full = false;
        &self.output
    }

    pub fn lines(&self) -> Vec<String> {
        self.lines_bytes()
            .iter()
            .map(|line| String::from_utf8_lossy(line).into_owned())
            .collect()
    }
    pub fn lines_bytes(&self) -> Vec<Vec<u8>> {
        (0..self.rows)
            .map(|y| {
                let mut line = Vec::new();
                for x in 0..self.cols {
                    let cell = self.cells[y as usize * self.cols as usize + x as usize];
                    if cell.width != 0 {
                        line.extend_from_slice(if cell.len == 0 { b" " } else { cell.bytes() });
                    }
                }
                line
            })
            .collect()
    }
    pub fn dump(&self) -> String {
        String::from_utf8_lossy(&self.dump_bytes()).into_owned()
    }
    pub fn dump_bytes(&self) -> Vec<u8> {
        self.lines_bytes()
            .into_iter()
            .flat_map(|mut line| {
                line.push(b'\n');
                line
            })
            .collect()
    }
    pub fn snapshot(&self) -> Value {
        let mut styles = Vec::new();
        let mut links = Vec::new();
        for y in 0..self.rows {
            let mut x = 0;
            while x < self.cols {
                let cell = self.cells[y as usize * self.cols as usize + x as usize];
                if cell.attrs == 0 && !cell.fg.set && !cell.bg.set {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < self.cols
                    && cell.same_style(self.cells[y as usize * self.cols as usize + x as usize])
                {
                    x += 1;
                }
                let attrs: Vec<_> = [
                    (BOLD, "bold"),
                    (DIM, "dim"),
                    (ITALIC, "italic"),
                    (UNDERLINE, "underline"),
                    (BLINK, "blink"),
                    (INVERSE, "inverse"),
                    (INVISIBLE, "invisible"),
                    (STRIKE, "strike"),
                ]
                .into_iter()
                .filter(|(bit, _)| cell.attrs & bit != 0)
                .map(|(_, label)| label)
                .collect();
                styles.push(json!({"x": start, "y": y, "w": x - start, "fg": cell.fg.hex(), "bg": cell.bg.hex(), "attrs": attrs}));
            }
            x = 0;
            while x < self.cols {
                let id = self.cell_links[y as usize * self.cols as usize + x as usize];
                if id == 0 {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < self.cols
                    && self.cell_links[y as usize * self.cols as usize + x as usize] == id
                {
                    x += 1;
                }
                if let Some(uri) = self.links.get(id as usize - 1) {
                    links.push(json!({"x": start, "y": y, "w": x - start, "uri": uri}));
                }
            }
        }
        let hits: Vec<_> = self
            .hits
            .0
            .iter()
            .map(|hit| json!({"x":hit.x,"y":hit.y,"w":hit.w,"h":hit.h,"action":String::from_utf8_lossy(&hit.action)}))
            .collect();
        json!({"cols":self.cols,"rows":self.rows,"text":self.lines(),"styles":styles,"links":links,
            "cursor":{"visible":self.cursor.visible,"x":self.cursor.x,"y":self.cursor.y},"hits":hits})
    }
    pub(crate) fn snapshot_reply(&self) -> crate::json_control::Reply {
        use crate::json_control::Reply;
        Reply::from(self.snapshot())
            .field(
                "hits",
                Reply::Array(
                    self.hits
                        .0
                        .iter()
                        .map(|hit| {
                            Reply::from(json!({"x":hit.x,"y":hit.y,"w":hit.w,"h":hit.h}))
                                .field("action", Reply::string_bytes(&hit.action))
                        })
                        .collect(),
                ),
            )
            .field(
                "text",
                Reply::Array(self.lines_bytes().iter().map(Reply::string_bytes).collect()),
            )
    }
}

fn sgr(output: &mut Vec<u8>, cell: Cell) {
    output.extend_from_slice(b"\x1b[0");
    for (bit, code) in [
        (BOLD, 1),
        (DIM, 2),
        (ITALIC, 3),
        (UNDERLINE, 4),
        (BLINK, 5),
        (INVERSE, 7),
        (INVISIBLE, 8),
        (STRIKE, 9),
    ] {
        if cell.attrs & bit != 0 {
            output.extend_from_slice(format!(";{code}").as_bytes());
        }
    }
    for (code, color) in [(38, cell.fg), (48, cell.bg)] {
        if color.set {
            output.extend_from_slice(
                format!(";{code};2;{};{};{}", color.r, color.g, color.b).as_bytes(),
            );
        }
    }
    output.push(b'm');
}

fn utf8_len(first: u8) -> usize {
    if first < 128 {
        1
    } else if first & 0xe0 == 0xc0 {
        2
    } else if first & 0xf0 == 0xe0 {
        3
    } else if first & 0xf8 == 0xf0 {
        4
    } else {
        1
    }
}
fn codepoints(bytes: &[u8], max: usize) -> (Vec<u32>, usize) {
    let mut points = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() && points.len() < max {
        let first = bytes[offset];
        let length = utf8_len(first);
        if offset + length > bytes.len() {
            break;
        }
        let mut cp = match length {
            1 => first as u32,
            2 => (first & 31) as u32,
            3 => (first & 15) as u32,
            _ => (first & 7) as u32,
        };
        for byte in &bytes[offset + 1..offset + length] {
            cp = (cp << 6) | (*byte & 63) as u32;
        }
        points.push(cp);
        offset += length;
    }
    (points, offset)
}
pub fn width_of(bytes: &[u8]) -> u8 {
    let (points, _) = codepoints(bytes, 16);
    if points.is_empty() {
        return 1;
    }
    let mut width = 1;
    // SAFETY: the codepoint slice and output remain valid throughout this call.
    unsafe {
        ffi::ghostty_unicode_grapheme_width(points.as_ptr(), points.len(), &mut width);
    }
    width.max(1)
}
pub fn cells(text: &str) -> u16 {
    cells_bytes(text.as_bytes())
}
pub fn cells_bytes(text: &[u8]) -> u16 {
    let mut bytes = text.split(|b| *b == 0).next().unwrap_or_default();
    let mut total = 0u16;
    while !bytes.is_empty() {
        let (points, used) = codepoints(bytes, 256);
        if used == 0 {
            break;
        }
        let mut at = 0;
        while at < points.len() {
            let mut width = 1;
            // SAFETY: an in-bounds suffix is borrowed with a valid width output.
            let consumed = unsafe {
                ffi::ghostty_unicode_grapheme_width(
                    points[at..].as_ptr(),
                    points.len() - at,
                    &mut width,
                )
            };
            if consumed == 0 {
                break;
            }
            at += consumed;
            total = total.wrapping_add(width as u16);
        }
        bytes = &bytes[used..];
    }
    total
}

pub fn clip_bytes(text: &[u8], width: u16) -> Vec<u8> {
    let mut text = text.split(|b| *b == 0).next().unwrap_or_default().to_vec();
    while cells_bytes(&text) > width {
        let mut at = text.len() - 1;
        while at > 0 && text[at] & 0xc0 == 0x80 {
            at -= 1;
        }
        text.truncate(at);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_screen_projection_cases() {
        let mut src = Screen::new(6, 4);
        for y in 0..4 {
            for x in 0..6 {
                src.put(
                    x,
                    y,
                    &[b'A' + (y * 6 + x) as u8],
                    Color::default(),
                    Color::default(),
                    0,
                );
            }
        }
        src.cursor = Cursor {
            visible: true,
            x: 4,
            y: 2,
        };
        let id = src.link_id("https://example.test");
        src.set_link(3, 2, id);
        src.at_mut(3, 2).unwrap().attrs = BOLD;
        let mut view = Screen::new(8, 5);
        view.project(&src, 0, 0);
        assert_eq!(view.at(5, 3).unwrap().text[0], b'X');
        assert_eq!(view.at(6, 0).unwrap().text[0], b' ');
        assert_eq!(view.at(0, 4).unwrap().text[0], b' ');
        assert_eq!(view.cursor, src.cursor);
        view.resize(3, 2);
        view.project(&src, 2, 1);
        assert_eq!(
            (
                view.at(0, 0).unwrap().text[0],
                view.at(2, 1).unwrap().text[0]
            ),
            (b'I', b'Q')
        );
        assert_eq!(view.at(1, 1).unwrap().attrs, BOLD);
        assert_eq!(
            view.links[view.cell_links[4] as usize - 1],
            "https://example.test"
        );
        assert_eq!(
            view.cursor,
            Cursor {
                visible: true,
                x: 2,
                y: 1
            }
        );
        assert_eq!(src.follow_cursor(3, 2, 0, 0), (2, 1));
        src.cursor.x = 1;
        src.cursor.y = 0;
        assert_eq!(src.follow_cursor(3, 2, 2, 1), (1, 0));
        src.cursor.visible = false;
        assert_eq!(src.follow_cursor(3, 2, 99, 99), (3, 2));
        src.put(4, 0, "日".as_bytes(), Color::default(), Color::default(), 0);
        view.resize(5, 1);
        view.project(&src, 0, 0);
        assert_eq!(view.at(4, 0).unwrap().text[0], b' ');
        view.project(&src, 5, 0);
        assert_eq!(view.at(0, 0).unwrap().text[0], b' ');
    }

    #[test]
    fn wide_tail_repainting_and_unchanged_frames() {
        let mut screen = Screen::new(6, 1);
        screen.text(0, 0, "------", Color::default(), Color::default(), 0);
        screen.render();
        screen.put(2, 0, "日".as_bytes(), Color::default(), Color::default(), 0);
        screen.render();
        screen.text(2, 0, "--", Color::default(), Color::default(), 0);
        assert_eq!(
            screen.render().iter().filter(|byte| **byte == b'-').count(),
            2
        );
        assert!(screen.render().is_empty());
    }

    #[test]
    fn native_hit_actions_keep_byte_caps_and_snapshot_bytes() {
        let mut screen = Screen::new(4, 1);
        let mut action = b"theme:".to_vec();
        action.extend_from_slice(&[b'x'; 40]);
        action.extend_from_slice("é".as_bytes());
        screen.hits.add_bytes(0, 0, 2, 1, &action);
        assert_eq!(screen.hits.test_bytes(1, 0), Some(&action[..47]));
        assert!(screen.hits.test(1, 0).is_none());
        let snapshot = screen.snapshot_reply().bytes();
        assert!(snapshot.windows(47).any(|part| part == &action[..47]));
        assert!(!snapshot.windows(3).any(|part| part == b"\xef\xbf\xbd"));
        screen
            .hits
            .add_bytes(1, 0, 2, 1, b"theme:raw-\xff\0ignored");
        assert_eq!(
            screen.hits.test_bytes(1, 0),
            Some(b"theme:raw-\xff".as_slice())
        );
    }

    #[test]
    fn graphemes_snapshots_and_links() {
        assert_eq!(cells("a日🔔"), 5);
        assert_eq!(cells("e\u{301}"), 1);
        let mut screen = Screen::new(4, 1);
        screen.put(
            0,
            0,
            "👨‍👩‍👧‍👦".as_bytes(),
            Color::rgb(255, 0, 0),
            Color::default(),
            BOLD,
        );
        assert!(std::str::from_utf8(screen.at(0, 0).unwrap().bytes()).is_ok());
        assert_eq!(screen.snapshot()["styles"][0]["fg"], "#ff0000");
        assert_eq!(screen.link_id("https://a\x1b[2J"), 0);
        let id = screen.link_id("https://example.test");
        screen.set_link(0, 0, id);
        assert!(
            screen
                .render()
                .windows(4)
                .any(|window| window == b"\x1b]8;")
        );
        screen.hits.add(0, 0, 2, 1, "first");
        screen.hits.add(1, 0, 2, 1, "second");
        assert_eq!(screen.hits.test(1, 0), Some("second"));
    }
}
