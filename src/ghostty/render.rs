use super::{Error, check, ffi};
use crate::screen::{self, Cell, Color, Cursor, Screen};
use std::ptr;

pub(super) struct Renderer {
    state: ffi::GhosttyRenderState,
    rows: ffi::GhosttyRenderStateRowIterator,
    cells: ffi::GhosttyRenderStateRowCells,
}
impl Renderer {
    pub(super) fn new() -> Result<Self, Error> {
        let mut render = Self {
            state: ptr::null_mut(),
            rows: ptr::null_mut(),
            cells: ptr::null_mut(),
        };
        // SAFETY: each output handle starts NULL and Drop cleans partially initialized state.
        unsafe {
            check(ffi::ghostty_render_state_new(
                ptr::null(),
                &mut render.state,
            ))?;
            check(ffi::ghostty_render_state_row_iterator_new(
                ptr::null(),
                &mut render.rows,
            ))?;
            check(ffi::ghostty_render_state_row_cells_new(
                ptr::null(),
                &mut render.cells,
            ))?;
        }
        Ok(render)
    }

    pub(super) fn compose(
        &mut self,
        terminal: ffi::GhosttyTerminal,
        screen: &mut Screen,
        x0: u16,
        y0: u16,
        focused: bool,
    ) -> Result<(), Error> {
        // SAFETY: the terminal and renderer handles are live and exclusively used
        // throughout this synchronous walk. Every selector's output type matches
        // render.h, cell.h, or terminal.h. No borrowed Ghostty data escapes.
        unsafe {
            check(ffi::ghostty_render_state_update(self.state, terminal))?;
            check(ffi::ghostty_render_state_get(
                self.state,
                ffi::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                (&mut self.rows as *mut ffi::GhosttyRenderStateRowIterator).cast(),
            ))?;
            let mut y = 0u16;
            while ffi::ghostty_render_state_row_iterator_next(self.rows) {
                if y as u32 + y0 as u32 >= screen.rows as u32 {
                    break;
                }
                check(ffi::ghostty_render_state_row_get(
                    self.rows,
                    ffi::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                    (&mut self.cells as *mut ffi::GhosttyRenderStateRowCells).cast(),
                ))?;
                let mut selection = ffi::GhosttyRenderStateRowSelection::default();
                selection.size = std::mem::size_of_val(&selection);
                let selected = ffi::ghostty_render_state_row_get(
                    self.rows,
                    ffi::GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION,
                    (&mut selection as *mut ffi::GhosttyRenderStateRowSelection).cast(),
                ) == ffi::GHOSTTY_SUCCESS;
                let mut x = 0u16;
                while ffi::ghostty_render_state_row_cells_next(self.cells) {
                    if x as u32 + x0 as u32 >= screen.cols as u32 {
                        break;
                    }
                    let index = screen.index(x0 + x, y0 + y).unwrap();
                    let mut raw = ffi::GhosttyCell::default();
                    let mut wide = ffi::GHOSTTY_CELL_WIDE_NARROW;
                    let mut linked = false;
                    if ffi::ghostty_render_state_row_cells_get(
                        self.cells,
                        ffi::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
                        (&mut raw as *mut ffi::GhosttyCell).cast(),
                    ) == ffi::GHOSTTY_SUCCESS
                    {
                        ffi::ghostty_cell_get(
                            raw,
                            ffi::GHOSTTY_CELL_DATA_WIDE,
                            (&mut wide as *mut ffi::GhosttyCellWide).cast(),
                        );
                        ffi::ghostty_cell_get(
                            raw,
                            ffi::GHOSTTY_CELL_DATA_HAS_HYPERLINK,
                            (&mut linked as *mut bool).cast(),
                        );
                    }
                    if wide == ffi::GHOSTTY_CELL_WIDE_SPACER_TAIL {
                        screen.cells[index].width = 0;
                        screen.cells[index].len = 0;
                        screen.cell_links[index] = 0;
                        x += 1;
                        continue;
                    }
                    let mut cell = Cell::default();
                    let mut buffer = ffi::GhosttyBuffer {
                        ptr: cell.text.as_mut_ptr(),
                        cap: 16,
                        len: 0,
                    };
                    let result = ffi::ghostty_render_state_row_cells_get(
                        self.cells,
                        ffi::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
                        (&mut buffer as *mut ffi::GhosttyBuffer).cast(),
                    );
                    if result == ffi::GHOSTTY_SUCCESS && buffer.len != 0 {
                        cell.len = buffer.len.min(16) as u8;
                    } else {
                        cell.text[0] = b' ';
                        cell.len = 1;
                    }
                    cell.width = if wide == ffi::GHOSTTY_CELL_WIDE_WIDE {
                        2
                    } else {
                        1
                    };
                    for (selector, color) in [
                        (
                            ffi::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
                            &mut cell.fg,
                        ),
                        (
                            ffi::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
                            &mut cell.bg,
                        ),
                    ] {
                        let mut rgb = ffi::GhosttyColorRgb::default();
                        if ffi::ghostty_render_state_row_cells_get(
                            self.cells,
                            selector,
                            (&mut rgb as *mut ffi::GhosttyColorRgb).cast(),
                        ) == ffi::GHOSTTY_SUCCESS
                        {
                            *color = Color::rgb(rgb.r, rgb.g, rgb.b);
                        }
                    }
                    let mut styled = false;
                    if ffi::ghostty_render_state_row_cells_get(
                        self.cells,
                        ffi::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING,
                        (&mut styled as *mut bool).cast(),
                    ) == ffi::GHOSTTY_SUCCESS
                        && styled
                    {
                        let mut style = ffi::GhosttyStyle::default();
                        style.size = std::mem::size_of_val(&style);
                        if ffi::ghostty_render_state_row_cells_get(
                            self.cells,
                            ffi::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                            (&mut style as *mut ffi::GhosttyStyle).cast(),
                        ) == ffi::GHOSTTY_SUCCESS
                        {
                            for (on, flag) in [
                                (style.bold, screen::BOLD),
                                (style.faint, screen::DIM),
                                (style.italic, screen::ITALIC),
                                (style.underline != 0, screen::UNDERLINE),
                                (style.blink, screen::BLINK),
                                (style.inverse, screen::INVERSE),
                                (style.invisible, screen::INVISIBLE),
                                (style.strikethrough, screen::STRIKE),
                            ] {
                                if on {
                                    cell.attrs |= flag;
                                }
                            }
                        }
                    }
                    if selected && x >= selection.start_x && x <= selection.end_x {
                        cell.attrs ^= screen::INVERSE;
                    }
                    screen.cells[index] = cell;
                    screen.cell_links[index] = if linked {
                        Self::link(terminal, screen, x, y)
                    } else {
                        0
                    };
                    x += 1;
                }
                y += 1;
            }
            if focused {
                let mut cursor = ffi::GhosttyRenderStateCursor::default();
                cursor.size = std::mem::size_of_val(&cursor);
                check(ffi::ghostty_render_state_get(
                    self.state,
                    ffi::GHOSTTY_RENDER_STATE_DATA_CURSOR,
                    (&mut cursor as *mut ffi::GhosttyRenderStateCursor).cast(),
                ))?;
                screen.cursor.visible = cursor.visible && cursor.viewport_has_value;
                if screen.cursor.visible {
                    screen.cursor = Cursor {
                        visible: true,
                        x: x0 + cursor.viewport_x,
                        y: y0 + cursor.viewport_y,
                    };
                }
            }
            let clean = ffi::GHOSTTY_RENDER_STATE_DIRTY_FALSE;
            check(ffi::ghostty_render_state_set(
                self.state,
                ffi::GHOSTTY_RENDER_STATE_OPTION_DIRTY,
                (&clean as *const ffi::GhosttyRenderStateDirty).cast(),
            ))?;
        }
        Ok(())
    }

    unsafe fn link(terminal: ffi::GhosttyTerminal, screen: &mut Screen, x: u16, y: u16) -> u16 {
        let point = ffi::GhosttyPoint {
            tag: ffi::GHOSTTY_POINT_TAG_VIEWPORT,
            value: ffi::GhosttyPointValue {
                coordinate: ffi::GhosttyPointCoordinate { x, y: y as u32 },
            },
        };
        let mut reference = ffi::GhosttyGridRef::default();
        let mut uri = [0u8; 1024];
        let mut len = 0;
        // SAFETY: terminal is live, the point uses the coordinate union tag, and
        // the reference and output buffer have the ABI's requested shapes.
        unsafe {
            if ffi::ghostty_terminal_grid_ref(terminal, point, &mut reference)
                != ffi::GHOSTTY_SUCCESS
            {
                return 0;
            }
            if ffi::ghostty_grid_ref_hyperlink_uri(
                &reference,
                uri.as_mut_ptr(),
                uri.len(),
                &mut len,
            ) != ffi::GHOSTTY_SUCCESS
            {
                return 0;
            }
        }
        screen.link_id(&String::from_utf8_lossy(&uri[..len.min(1024)]))
    }
}
impl Drop for Renderer {
    fn drop(&mut self) {
        // SAFETY: these handles are owned, and the free APIs accept NULL.
        unsafe {
            ffi::ghostty_render_state_row_cells_free(self.cells);
            ffi::ghostty_render_state_row_iterator_free(self.rows);
            ffi::ghostty_render_state_free(self.state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_styles_wide_cells_and_hyperlinks_reach_compositor() {
        let mut terminal = super::super::Terminal::new(12, 3).unwrap();
        terminal.write(b"\x1b[1;38;2;255;0;0mred\x1b[0m \x1b]8;;https://example.test\x1b\\link\x1b]8;;\x1b\\\r\n");
        terminal.write("日".as_bytes());
        let mut screen = Screen::new(12, 3);
        terminal.compose(&mut screen, 0, 0, true).unwrap();
        assert_eq!(screen.cells[0].fg, Color::rgb(255, 0, 0));
        assert_eq!(screen.cells[0].attrs, screen::BOLD);
        assert_eq!(screen.cells[12].width, 2);
        assert_eq!(screen.cells[13].width, 0);
        assert_eq!(
            screen.links[screen.cell_links[4] as usize - 1],
            "https://example.test"
        );
        assert_eq!(
            screen.cursor,
            Cursor {
                visible: true,
                x: 2,
                y: 1
            }
        );
        terminal.drop_render_cache();
        terminal.compose(&mut screen, 0, 0, true).unwrap();
        assert!(screen.lines()[0].starts_with("red link"));
    }
}
