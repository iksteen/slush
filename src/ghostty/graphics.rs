//! Copy image pixels once per generation; never retain a borrowed Ghostty pointer.
use super::{Error, Terminal, check, ffi};
use crate::graphics::{Geometry, Request};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Default)]
pub(super) struct Cache(BTreeMap<u32, (u64, Arc<[u8]>)>);
struct Iterator(ffi::GhosttyKittyGraphicsPlacementIterator);
impl Drop for Iterator {
    fn drop(&mut self) {
        // SAFETY: the iterator is uniquely owned, and Drop runs before the terminal borrow ends.
        unsafe { ffi::ghostty_kitty_graphics_placement_iterator_free(self.0) };
    }
}

impl Terminal {
    pub fn graphics(&mut self) -> Result<Vec<Request>, Error> {
        let mut graphics: ffi::GhosttyKittyGraphics = std::ptr::null_mut();
        // SAFETY: all selectors below use the types declared in kitty_graphics.h.
        // Access is serialized with VT writes, snapshots are copied before return,
        // and the iterator is freed on every exit, including errors.
        unsafe {
            let result = ffi::ghostty_terminal_get(
                self.handle.as_ptr(),
                ffi::GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS,
                (&mut graphics as *mut ffi::GhosttyKittyGraphics).cast(),
            );
            if result == ffi::GHOSTTY_NO_VALUE || graphics.is_null() {
                return Ok(Vec::new());
            }
            check(result)?;
            let mut it = std::ptr::null_mut();
            check(ffi::ghostty_kitty_graphics_placement_iterator_new(
                std::ptr::null(),
                &mut it,
            ))?;
            let it = Iterator(it);
            let mut handle = it.0;
            check(ffi::ghostty_kitty_graphics_get(
                graphics,
                ffi::GHOSTTY_KITTY_GRAPHICS_DATA_PLACEMENT_ITERATOR,
                (&mut handle as *mut ffi::GhosttyKittyGraphicsPlacementIterator).cast(),
            ))?;
            macro_rules! value {
                ($function:ident, $handle:expr, $selector:ident, $ty:ty) => {{
                    let mut value: $ty = Default::default();
                    ffi::$function($handle, ffi::$selector, (&mut value as *mut $ty).cast());
                    value
                }};
            }
            let mut requests = Vec::new();
            let mut seen = BTreeSet::new();
            while ffi::ghostty_kitty_graphics_placement_next(it.0) {
                let image_id = value!(
                    ghostty_kitty_graphics_placement_get,
                    it.0,
                    GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IMAGE_ID,
                    u32
                );
                let place_id = value!(
                    ghostty_kitty_graphics_placement_get,
                    it.0,
                    GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_PLACEMENT_ID,
                    u32
                );
                let mut x_off = value!(
                    ghostty_kitty_graphics_placement_get,
                    it.0,
                    GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_X_OFFSET,
                    u32
                );
                let mut y_off = value!(
                    ghostty_kitty_graphics_placement_get,
                    it.0,
                    GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_Y_OFFSET,
                    u32
                );
                let mut req_cols = value!(
                    ghostty_kitty_graphics_placement_get,
                    it.0,
                    GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_COLUMNS,
                    u32
                );
                let mut req_rows = value!(
                    ghostty_kitty_graphics_placement_get,
                    it.0,
                    GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_ROWS,
                    u32
                );
                let image = ffi::ghostty_kitty_graphics_image(graphics, image_id);
                seen.insert(image_id);
                let mut info = ffi::GhosttyKittyGraphicsPlacementRenderInfo {
                    size: std::mem::size_of::<ffi::GhosttyKittyGraphicsPlacementRenderInfo>(),
                    ..Default::default()
                };
                if ffi::ghostty_kitty_graphics_placement_render_info(
                    it.0,
                    image,
                    self.handle.as_ptr(),
                    &mut info,
                ) != ffi::GHOSTTY_SUCCESS
                    || !info.viewport_visible
                {
                    continue;
                }
                let cw = info.pixel_width.checked_div(info.grid_cols).unwrap_or(0);
                let ch = info.pixel_height.checked_div(info.grid_rows).unwrap_or(0);
                let (mut sx, mut sy, mut sw, mut sh) = (
                    info.source_x,
                    info.source_y,
                    info.source_width,
                    info.source_height,
                );
                let (mut col, mut row) = (info.viewport_col, info.viewport_row);
                let (mut cols, mut rows) = (info.grid_cols, info.grid_rows);
                if col < 0 {
                    let off = col.unsigned_abs();
                    if off >= cols {
                        continue;
                    }
                    let skip = off.saturating_mul(cw).saturating_sub(x_off);
                    sx = sx.saturating_add(skip);
                    sw = sw.saturating_sub(skip);
                    cols -= off;
                    req_cols = req_cols.saturating_sub(off);
                    col = 0;
                    x_off = 0;
                }
                if row < 0 {
                    let off = row.unsigned_abs();
                    if off >= rows {
                        continue;
                    }
                    let skip = off.saturating_mul(ch).saturating_sub(y_off);
                    sy = sy.saturating_add(skip);
                    sh = sh.saturating_sub(skip);
                    rows -= off;
                    req_rows = req_rows.saturating_sub(off);
                    row = 0;
                    y_off = 0;
                }
                if cols == 0 || rows == 0 {
                    continue;
                }
                let mut request = Request {
                    src_id: image_id,
                    place_id,
                    geometry: Geometry {
                        col: col as u16,
                        row: row as u16,
                        cols: cols as u16,
                        rows: rows as u16,
                        scale_cols: req_cols as u16,
                        scale_rows: req_rows as u16,
                        x_off,
                        y_off,
                        sx,
                        sy,
                        sw,
                        sh,
                    },
                    cell_px_w: cw,
                    cell_px_h: ch,
                    ..Default::default()
                };
                if !image.is_null() {
                    request.px_w = value!(
                        ghostty_kitty_graphics_image_get,
                        image,
                        GHOSTTY_KITTY_IMAGE_DATA_WIDTH,
                        u32
                    );
                    request.px_h = value!(
                        ghostty_kitty_graphics_image_get,
                        image,
                        GHOSTTY_KITTY_IMAGE_DATA_HEIGHT,
                        u32
                    );
                    request.format = value!(
                        ghostty_kitty_graphics_image_get,
                        image,
                        GHOSTTY_KITTY_IMAGE_DATA_FORMAT,
                        ffi::GhosttyKittyImageFormat
                    );
                    request.compression = value!(
                        ghostty_kitty_graphics_image_get,
                        image,
                        GHOSTTY_KITTY_IMAGE_DATA_COMPRESSION,
                        ffi::GhosttyKittyImageCompression
                    );
                    request.generation = value!(
                        ghostty_kitty_graphics_image_get,
                        image,
                        GHOSTTY_KITTY_IMAGE_DATA_GENERATION,
                        u64
                    );
                    if !self
                        .graphics_cache
                        .0
                        .get(&image_id)
                        .is_some_and(|(generation, _)| *generation == request.generation)
                    {
                        let mut ptr: *const u8 = std::ptr::null();
                        ffi::ghostty_kitty_graphics_image_get(
                            image,
                            ffi::GHOSTTY_KITTY_IMAGE_DATA_DATA_PTR,
                            (&mut ptr as *mut *const u8).cast(),
                        );
                        let len = value!(
                            ghostty_kitty_graphics_image_get,
                            image,
                            GHOSTTY_KITTY_IMAGE_DATA_DATA_LEN,
                            usize
                        );
                        let data: Arc<[u8]> = if ptr.is_null() || len == 0 {
                            Arc::from([])
                        } else {
                            Arc::from(std::slice::from_raw_parts(ptr, len))
                        };
                        self.graphics_cache
                            .0
                            .insert(image_id, (request.generation, data));
                    }
                    request.data = self.graphics_cache.0[&image_id].1.clone();
                }
                requests.push(request);
            }
            self.graphics_cache.0.retain(|id, _| seen.contains(id));
            Ok(requests)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_share_pixels_until_image_generation_changes() {
        let mut terminal = Terminal::new(20, 5).unwrap();
        terminal.resize(20, 5, 8, 16).unwrap();
        let image = |byte| {
            format!(
                "\x1b_Ga=T,f=24,q=2,s=4,v=2,i=7,p=1,c=6,r=2;{}\x1b\\",
                crate::osc::encode_base64(&[byte; 24])
            )
        };
        terminal.write(image(128).as_bytes());
        let first = terminal.graphics().unwrap();
        let second = terminal.graphics().unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].data.as_ref(), &[128; 24]);
        assert!(Arc::ptr_eq(&first[0].data, &second[0].data));
        terminal.write(b"\x1b[1;1H");
        terminal.write(image(64).as_bytes());
        let replaced = terminal.graphics().unwrap();
        assert_ne!(first[0].generation, replaced[0].generation);
        assert!(!Arc::ptr_eq(&first[0].data, &replaced[0].data));
        assert_eq!(replaced[0].data.as_ref(), &[64; 24]);
        // Old snapshots own their bytes even after Ghostty replaces the image.
        assert_eq!(first[0].data.as_ref(), &[128; 24]);
    }
}
