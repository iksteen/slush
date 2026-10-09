//! Owning C-shaped test buffers for running the unchanged screen suite.
#![allow(clippy::missing_safety_doc)]
use crate::screen::{self, Cell, Color, Cursor, Hit, Hits, Screen};
use std::{
    ffi::{CStr, CString, c_char},
    ptr,
};

#[repr(C)]
pub struct RawHit {
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    action: [c_char; 48],
}
#[repr(C)]
pub struct RawHits {
    items: *mut RawHit,
    len: usize,
    cap: usize,
}
#[repr(C)]
pub struct RawScreen {
    cols: u16,
    rows: u16,
    cur: *mut Cell,
    prev: *mut Cell,
    cur_link: *mut u16,
    prev_link: *mut u16,
    links: *mut *mut c_char,
    nlinks: usize,
    force_full: bool,
    hits: RawHits,
    cursor_visible: bool,
    cursor_x: u16,
    cursor_y: u16,
    shown_cursor_visible: bool,
    shown_cursor_x: u16,
    shown_cursor_y: u16,
    out: *mut c_char,
    out_len: usize,
    out_cap: usize,
}
fn array<T>(values: Vec<T>) -> *mut T {
    Box::into_raw(values.into_boxed_slice()).cast()
}
unsafe fn take_array<T>(data: *mut T, len: usize) -> Vec<T> {
    if data.is_null() {
        return Vec::new();
    }
    // SAFETY: every array is allocated by array with its exact length.
    unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(data, len)).into_vec() }
}
fn raw_hits(hits: Hits) -> RawHits {
    let len = hits.0.len();
    let items = hits
        .0
        .into_iter()
        .map(|hit| {
            let mut action = [0; 48];
            for (slot, byte) in action.iter_mut().zip(hit.action.into_iter().take(47)) {
                *slot = byte as c_char;
            }
            RawHit {
                x: hit.x,
                y: hit.y,
                w: hit.w,
                h: hit.h,
                action,
            }
        })
        .collect();
    RawHits {
        items: array(items),
        len,
        cap: len,
    }
}
impl From<Screen> for RawScreen {
    fn from(screen: Screen) -> Self {
        Self {
            cols: screen.cols,
            rows: screen.rows,
            cur: array(screen.cells),
            prev: array(screen.previous),
            cur_link: array(screen.cell_links),
            prev_link: array(screen.previous_links),
            nlinks: screen.links.len(),
            links: array(
                screen
                    .links
                    .into_iter()
                    .map(|uri| CString::new(uri).unwrap().into_raw())
                    .collect(),
            ),
            force_full: screen.force_full,
            hits: raw_hits(screen.hits),
            cursor_visible: screen.cursor.visible,
            cursor_x: screen.cursor.x,
            cursor_y: screen.cursor.y,
            shown_cursor_visible: screen.shown_cursor.visible,
            shown_cursor_x: screen.shown_cursor.x,
            shown_cursor_y: screen.shown_cursor.y,
            out_len: screen.output.len(),
            out_cap: screen.output.len(),
            out: array(screen.output).cast(),
        }
    }
}
impl RawScreen {
    unsafe fn take(self) -> Screen {
        let count = self.cols as usize * self.rows as usize;
        // SAFETY: self owns allocations produced by From<Screen>.
        unsafe {
            Screen {
                cols: self.cols,
                rows: self.rows,
                cells: take_array(self.cur, count),
                previous: take_array(self.prev, count),
                cell_links: take_array(self.cur_link, count),
                previous_links: take_array(self.prev_link, count),
                links: take_array(self.links, self.nlinks)
                    .into_iter()
                    .map(|uri| CString::from_raw(uri).into_string().unwrap())
                    .collect(),
                force_full: self.force_full,
                hits: Hits(
                    take_array(self.hits.items, self.hits.len)
                        .into_iter()
                        .map(|hit| Hit {
                            x: hit.x,
                            y: hit.y,
                            w: hit.w,
                            h: hit.h,
                            action: CStr::from_ptr(hit.action.as_ptr()).to_bytes().to_vec(),
                        })
                        .collect(),
                ),
                cursor: Cursor {
                    visible: self.cursor_visible,
                    x: self.cursor_x,
                    y: self.cursor_y,
                },
                shown_cursor: Cursor {
                    visible: self.shown_cursor_visible,
                    x: self.shown_cursor_x,
                    y: self.shown_cursor_y,
                },
                output: take_array(self.out.cast(), self.out_len),
            }
        }
    }
}
pub(crate) unsafe fn update<T>(raw: *mut RawScreen, op: impl FnOnce(&mut Screen) -> T) -> T {
    // SAFETY: raw is uniquely borrowed and its state is replaced before returning.
    unsafe {
        let mut screen = raw.read().take();
        let result = op(&mut screen);
        raw.write(screen.into());
        result
    }
}
unsafe fn clone_source(raw: &RawScreen) -> Screen {
    let mut screen = Screen::new(raw.cols, raw.rows);
    let count = raw.cols as usize * raw.rows as usize;
    // SAFETY: source arrays are borrowed only while the live screen is unchanged.
    unsafe {
        screen
            .cells
            .copy_from_slice(std::slice::from_raw_parts(raw.cur, count));
        screen
            .cell_links
            .copy_from_slice(std::slice::from_raw_parts(raw.cur_link, count));
        screen.links = std::slice::from_raw_parts(raw.links, raw.nlinks)
            .iter()
            .map(|uri| CStr::from_ptr(*uri).to_string_lossy().into_owned())
            .collect();
    }
    screen.cursor = Cursor {
        visible: raw.cursor_visible,
        x: raw.cursor_x,
        y: raw.cursor_y,
    };
    screen
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn screen_init(raw: *mut RawScreen, cols: u16, rows: u16) {
    // SAFETY: caller provides storage for one screen_t, initially uninitialized.
    unsafe {
        raw.write(Screen::new(cols, rows).into());
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn screen_free(raw: *mut RawScreen) {
    // SAFETY: the caller returns ownership of a live screen's fields once.
    unsafe {
        drop(raw.read().take());
        raw.write(std::mem::zeroed());
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn screen_resize(raw: *mut RawScreen, cols: u16, rows: u16) {
    // SAFETY: the live screen is exclusively borrowed.
    unsafe {
        update(raw, |screen| screen.resize(cols, rows));
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn screen_clear(raw: *mut RawScreen) {
    // SAFETY: the live screen is exclusively borrowed.
    unsafe {
        update(raw, Screen::clear);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn screen_at(raw: *mut RawScreen, x: u16, y: u16) -> *mut Cell {
    // SAFETY: the live screen's in-bounds allocation is returned as a borrowed pointer.
    unsafe {
        if x >= (*raw).cols || y >= (*raw).rows {
            return ptr::null_mut();
        }
        (*raw)
            .cur
            .add(y as usize * (*raw).cols as usize + x as usize)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn screen_put_utf8(
    raw: *mut RawScreen,
    x: u16,
    y: u16,
    text: *const c_char,
    len: usize,
    fg: Color,
    bg: Color,
    attrs: u16,
) {
    // SAFETY: text spans len bytes and raw is exclusively borrowed.
    unsafe {
        let bytes = std::slice::from_raw_parts(text.cast(), len);
        update(raw, |screen| screen.put(x, y, bytes, fg, bg, attrs));
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn screen_text(
    raw: *mut RawScreen,
    x: u16,
    y: u16,
    text: *const c_char,
    fg: Color,
    bg: Color,
    attrs: u16,
) -> u16 {
    // SAFETY: text is NUL-terminated and raw is exclusively borrowed.
    unsafe {
        let text = CStr::from_ptr(text).to_string_lossy();
        update(raw, |screen| screen.text(x, y, &text, fg, bg, attrs))
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn screen_width_of(text: *const c_char, len: usize) -> u8 {
    // SAFETY: text spans len bytes, per the C API.
    screen::width_of(unsafe { std::slice::from_raw_parts(text.cast(), len) })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn screen_cells(text: *const c_char) -> u16 {
    if text.is_null() {
        return 0;
    }
    // SAFETY: non-NULL text is NUL-terminated.
    screen::cells(&unsafe { CStr::from_ptr(text) }.to_string_lossy())
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn screen_link_id(
    raw: *mut RawScreen,
    uri: *const c_char,
    len: usize,
) -> u16 {
    if uri.is_null() || len == 0 {
        return 0;
    }
    // SAFETY: uri spans len bytes and raw is exclusively borrowed.
    unsafe {
        let text = String::from_utf8_lossy(std::slice::from_raw_parts(uri.cast(), len));
        update(raw, |screen| screen.link_id(&text))
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn screen_set_link(raw: *mut RawScreen, x: u16, y: u16, id: u16) {
    // SAFETY: raw is a uniquely borrowed live screen.
    unsafe {
        update(raw, |screen| screen.set_link(x, y, id));
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn screen_project(
    dst: *mut RawScreen,
    src: *const RawScreen,
    x: u16,
    y: u16,
) {
    // SAFETY: source is borrowed and the destination uniquely owned by the caller.
    unsafe {
        let source = clone_source(&*src);
        update(dst, |screen| screen.project(&source, x, y));
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn screen_follow_cursor(
    src: *const RawScreen,
    cols: u16,
    rows: u16,
    x: *mut u16,
    y: *mut u16,
) {
    // SAFETY: source is borrowed and any non-NULL coordinate pointers are writable.
    unsafe {
        let source = clone_source(&*src);
        let (nx, ny) = source.follow_cursor(
            cols,
            rows,
            x.as_ref().copied().unwrap_or(0),
            y.as_ref().copied().unwrap_or(0),
        );
        if !x.is_null() {
            *x = nx;
        }
        if !y.is_null() {
            *y = ny;
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn screen_render(raw: *mut RawScreen) {
    // SAFETY: raw is a uniquely borrowed live screen.
    unsafe {
        update(raw, |screen| {
            screen.render();
        });
    }
}
