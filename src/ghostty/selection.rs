//! Selection uses tracked anchors so dragging survives scrollback and reflow.
use super::{Error, Terminal, check, ffi};
use std::{mem::size_of, ptr::NonNull};

fn point(x: u16, y: u16) -> ffi::GhosttyPoint {
    ffi::GhosttyPoint {
        tag: ffi::GHOSTTY_POINT_TAG_VIEWPORT,
        value: ffi::GhosttyPointValue {
            coordinate: ffi::GhosttyPointCoordinate { x, y: y as u32 },
        },
    }
}

impl Terminal {
    pub fn selection_clear(&mut self) {
        // SAFETY: null selection clears the exclusively borrowed terminal.
        unsafe {
            ffi::ghostty_terminal_set(
                self.handle.as_ptr(),
                ffi::GHOSTTY_TERMINAL_OPT_SELECTION,
                std::ptr::null(),
            );
        }
    }
    pub fn selection_start(&mut self, x: u16, y: u16) -> Result<(), Error> {
        // SAFETY: outputs are writable; point is a value with matching union tag.
        unsafe {
            if let Some(anchor) = self.anchor {
                check(ffi::ghostty_tracked_grid_ref_set(
                    anchor.as_ptr(),
                    self.handle.as_ptr(),
                    point(x, y),
                ))?;
            } else {
                let mut anchor = std::ptr::null_mut();
                check(ffi::ghostty_terminal_grid_ref_track(
                    self.handle.as_ptr(),
                    point(x, y),
                    &mut anchor,
                ))?;
                self.anchor = Some(NonNull::new(anchor).ok_or(Error(ffi::GHOSTTY_OUT_OF_MEMORY))?);
            }
        }
        self.selection_clear();
        Ok(())
    }
    fn grid_ref(&self, x: u16, y: u16) -> Result<ffi::GhosttyGridRef, Error> {
        let mut reference = ffi::GhosttyGridRef::default();
        // SAFETY: live terminal and writable output; reference does not outlive this operation.
        unsafe {
            check(ffi::ghostty_terminal_grid_ref(
                self.handle.as_ptr(),
                point(x, y),
                &mut reference,
            ))?;
        }
        Ok(reference)
    }
    fn select(&mut self, a: ffi::GhosttyGridRef, b: ffi::GhosttyGridRef) -> Result<(), Error> {
        let selection = ffi::GhosttySelection {
            size: size_of::<ffi::GhosttySelection>(),
            start: a,
            end: b,
            rectangle: false,
        };
        // SAFETY: fresh references belong to this terminal; setter copies them into tracked state.
        unsafe {
            check(ffi::ghostty_terminal_set(
                self.handle.as_ptr(),
                ffi::GHOSTTY_TERMINAL_OPT_SELECTION,
                (&selection as *const ffi::GhosttySelection).cast(),
            ))
        }
    }
    pub fn selection_extend(&mut self, x: u16, y: u16) -> Result<(), Error> {
        let Some(anchor) = self.anchor else {
            return Ok(());
        };
        let mut start = ffi::GhosttyGridRef::default();
        // SAFETY: snapshot writes a fresh ref; an evicted anchor produces an error.
        unsafe {
            check(ffi::ghostty_tracked_grid_ref_snapshot(
                anchor.as_ptr(),
                &mut start,
            ))?;
        }
        self.select(start, self.grid_ref(x, y)?)
    }
    pub fn selection_word(
        &mut self,
        x: u16,
        y: u16,
        cols: u16,
        rows: u16,
        separators: impl AsRef<[u8]>,
    ) -> Result<bool, Error> {
        if x >= cols || y >= rows {
            return Ok(false);
        }
        // Match slosh's row-local words (a wrap is not a word boundary crossing).
        let mut screen = crate::screen::Screen::new(cols, rows);
        self.compose(&mut screen, 0, 0, false)?;
        let (mut start, mut hit, mut in_run) = (0, false, false);
        let mut bounds = None;
        for col in 0..cols {
            let cell = screen.at(col, y).unwrap();
            let bytes = &cell.text[..cell.len as usize];
            let word = if cell.width == 0 {
                in_run
            } else {
                !bytes.is_empty()
                    && !(bytes.len() == 1
                        && (matches!(bytes[0], 0 | b' ' | b'\t')
                            || separators.as_ref().contains(&bytes[0])))
            };
            if word {
                if !in_run {
                    start = col;
                    in_run = true;
                }
                if col == x {
                    hit = true;
                }
            } else {
                if in_run && hit {
                    bounds = Some((start, col - 1));
                    break;
                }
                in_run = false;
                if col == x {
                    return Ok(false);
                }
            }
        }
        let Some((start, end)) = bounds.or_else(|| (hit && in_run).then_some((start, cols - 1)))
        else {
            return Ok(false);
        };
        self.select(self.grid_ref(start, y)?, self.grid_ref(end, y)?)?;
        Ok(true)
    }
    pub fn selection_text(&self) -> Result<String, Error> {
        let options = ffi::GhosttyTerminalSelectionFormatOptions {
            size: size_of::<ffi::GhosttyTerminalSelectionFormatOptions>(),
            emit: ffi::GHOSTTY_FORMATTER_FORMAT_PLAIN,
            unwrap: true,
            trim: true,
            ..Default::default()
        };
        let mut data = std::ptr::null_mut();
        let mut len = 0;
        // SAFETY: formatter uses terminal's selection and allocates with the default
        // allocator. Copy and free before returning; no Ghostty bytes escape.
        unsafe {
            let result = ffi::ghostty_terminal_selection_format_alloc(
                self.handle.as_ptr(),
                std::ptr::null(),
                options,
                &mut data,
                &mut len,
            );
            if result == ffi::GHOSTTY_NO_VALUE {
                return Ok(String::new());
            }
            check(result)?;
            let text = if len == 0 {
                String::new()
            } else {
                String::from_utf8_lossy(std::slice::from_raw_parts(data, len)).into_owned()
            };
            if !data.is_null() {
                ffi::ghostty_free(std::ptr::null(), data.cast(), len);
            }
            Ok(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_format_distinguishes_wraps_from_newlines() {
        let mut terminal = Terminal::new(5, 4).unwrap();
        terminal.write(b"abcdefgh\r\nnext");
        terminal.selection_start(0, 0).unwrap();
        terminal.selection_extend(3, 2).unwrap();
        assert_eq!(terminal.selection_text().unwrap(), "abcdefgh\nnext");
        terminal.selection_clear();
        assert_eq!(terminal.selection_text().unwrap(), "");
    }

    #[test]
    fn tracked_anchor_keeps_far_end_when_viewport_moves() {
        let mut terminal = Terminal::new(12, 3).unwrap();
        for n in 1..=12 {
            terminal.write(format!("line{n:02}\r\n").as_bytes());
        }
        terminal.selection_start(5, 1).unwrap();
        terminal.scroll_edge(true);
        terminal.selection_extend(0, 0).unwrap();
        let text = terminal.selection_text().unwrap();
        assert!(text.starts_with("line01"), "{text:?}");
        assert!(text.ends_with("line12"), "{text:?}");
    }
}
