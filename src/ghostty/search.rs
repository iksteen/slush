//! Search handles stay with their terminal; grid snapshots are converted before mutation.
use super::{Error, Terminal, check, ffi};
use std::ptr::NonNull;

pub struct Mark {
    pub row: u16,
    pub x0: u16,
    pub x1: u16,
    pub current: bool,
}

impl Terminal {
    fn ensure_search(&mut self) -> Result<ffi::GhosttySearch, Error> {
        if let Some(search) = self.search {
            return Ok(search.as_ptr());
        }
        let mut handle = std::ptr::null_mut();
        // SAFETY: allocator is default, terminal is owned, output is writable.
        unsafe {
            check(ffi::ghostty_search_new(
                std::ptr::null(),
                &mut handle,
                self.handle.as_ptr(),
            ))?
        };
        self.search = Some(NonNull::new(handle).ok_or(Error(ffi::GHOSTTY_OUT_OF_MEMORY))?);
        Ok(handle)
    }

    pub fn search_set(&mut self, needle: &str) -> Result<(), Error> {
        let search = self.ensure_search()?;
        let string = ffi::GhosttyString {
            ptr: needle.as_ptr(),
            len: needle.len(),
        };
        // SAFETY: setters synchronously copy the borrowed string. Search and terminal
        // are exclusively borrowed; a null needle clears the search.
        unsafe {
            check(ffi::ghostty_search_set(
                search,
                ffi::GHOSTTY_SEARCH_OPT_NEEDLE,
                if needle.is_empty() {
                    std::ptr::null()
                } else {
                    (&string as *const ffi::GhosttyString).cast()
                },
            ))?;
            if !needle.is_empty() {
                check(ffi::ghostty_search_run(search))?;
                if self.search_position().0.is_none() {
                    let result = ffi::ghostty_search_set(
                        search,
                        ffi::GHOSTTY_SEARCH_OPT_SELECT_NEXT,
                        std::ptr::null(),
                    );
                    if result != ffi::GHOSTTY_NO_VALUE {
                        check(result)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn search_step(&mut self, older: bool) -> Result<bool, Error> {
        let Some(search) = self.search else {
            return Ok(false);
        };
        // SAFETY: selecting is serialized with all terminal operations.
        let result = unsafe {
            ffi::ghostty_search_set(
                search.as_ptr(),
                if older {
                    ffi::GHOSTTY_SEARCH_OPT_SELECT_NEXT
                } else {
                    ffi::GHOSTTY_SEARCH_OPT_SELECT_PREV
                },
                std::ptr::null(),
            )
        };
        if result == ffi::GHOSTTY_NO_VALUE {
            return Ok(false);
        }
        check(result)?;
        Ok(true)
    }

    pub fn search_refresh(&mut self) -> Result<(), Error> {
        if let Some(search) = self.search {
            // SAFETY: both handles are owned and used on the same thread.
            unsafe { check(ffi::ghostty_search_run(search.as_ptr()))? };
        }
        Ok(())
    }

    pub fn search_position(&self) -> (Option<usize>, usize) {
        let Some(search) = self.search else {
            return (None, 0);
        };
        let (mut index, mut total) = (0usize, 0usize);
        // SAFETY: these two data selectors write size_t scalars.
        unsafe {
            ffi::ghostty_search_get(
                search.as_ptr(),
                ffi::GHOSTTY_SEARCH_DATA_TOTAL_MATCHES,
                (&mut total as *mut usize).cast(),
            );
            let result = ffi::ghostty_search_get(
                search.as_ptr(),
                ffi::GHOSTTY_SEARCH_DATA_SELECTED_INDEX,
                (&mut index as *mut usize).cast(),
            );
            ((result == ffi::GHOSTTY_SUCCESS).then_some(index), total)
        }
    }

    pub fn search_marks(&self, cols: u16, rows: u16) -> Vec<Mark> {
        if cols == 0 || rows == 0 {
            return Vec::new();
        }
        let Some(search) = self.search else {
            return Vec::new();
        };
        let mut selected = ffi::GhosttySelection {
            size: std::mem::size_of::<ffi::GhosttySelection>(),
            ..Default::default()
        };
        let point = |reference: &ffi::GhosttyGridRef| {
            let mut out = ffi::GhosttyPointCoordinate::default();
            // SAFETY: the reference is a fresh snapshot, converted before any mutation.
            let result = unsafe {
                ffi::ghostty_terminal_point_from_grid_ref(
                    self.handle.as_ptr(),
                    reference,
                    ffi::GHOSTTY_POINT_TAG_VIEWPORT,
                    &mut out,
                )
            };
            (result == ffi::GHOSTTY_SUCCESS).then_some(out)
        };
        // SAFETY: selected is sized and writable for this selector.
        let current = if unsafe {
            ffi::ghostty_search_get(
                search.as_ptr(),
                ffi::GHOSTTY_SEARCH_DATA_SELECTED_MATCH,
                (&mut selected as *mut ffi::GhosttySelection).cast(),
            )
        } == ffi::GHOSTTY_SUCCESS
        {
            point(&selected.start)
        } else {
            None
        };
        let mut selections = vec![ffi::GhosttySelection::default(); 256];
        let mut buffer = ffi::GhosttySelectionBuffer {
            ptr: selections.as_mut_ptr(),
            cap: selections.len(),
            len: 0,
        };
        // SAFETY: buffer owns cap writable entries, and search_get does not retain it.
        let result = unsafe {
            ffi::ghostty_search_get(
                search.as_ptr(),
                ffi::GHOSTTY_SEARCH_DATA_VIEWPORT_MATCHES,
                (&mut buffer as *mut ffi::GhosttySelectionBuffer).cast(),
            )
        };
        if result != ffi::GHOSTTY_SUCCESS {
            return Vec::new();
        }
        let mut marks = Vec::new();
        for selection in selections.iter().take(buffer.len.min(selections.len())) {
            let (Some(mut a), Some(mut b)) = (point(&selection.start), point(&selection.end))
            else {
                continue;
            };
            if (b.y, b.x) < (a.y, a.x) {
                std::mem::swap(&mut a, &mut b);
            }
            let is_current = current.is_some_and(|p| (p.x, p.y) == (a.x, a.y));
            for row in a.y..=b.y.min(rows.saturating_sub(1) as u32) {
                if marks.len() >= 256 {
                    return marks;
                }
                marks.push(Mark {
                    row: row as u16,
                    x0: if row == a.y { a.x } else { 0 },
                    x1: if row == b.y {
                        b.x
                    } else {
                        cols.saturating_sub(1)
                    },
                    current: is_current,
                });
            }
        }
        marks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_tracks_output_and_recovers_after_reflow_and_screen_switches() {
        let mut terminal = Terminal::new(20, 4).unwrap();
        terminal.write(b"one needle\r\ntwo needle\r\n");
        terminal.search_set("NEEDLE").unwrap();
        assert_eq!(terminal.search_position(), (Some(0), 2));
        terminal.search_step(true).unwrap();
        assert_eq!(terminal.search_position(), (Some(1), 2));
        terminal.write(b"three needle\r\n");
        terminal.search_refresh().unwrap();
        assert_eq!(terminal.search_position(), (Some(2), 3));
        terminal.resize(8, 4, 8, 16).unwrap();
        terminal.search_refresh().unwrap();
        // Ghostty deliberately invalidates flattened selections on reflow;
        // fresh stepping must recover without using a pre-resize grid ref.
        assert_eq!(terminal.search_position(), (None, 3));
        for _ in 0..3 {
            terminal.search_step(true).unwrap();
        }
        assert_eq!(terminal.search_position(), (Some(2), 3));
        terminal.write(b"\x1b[?1049halt needle");
        terminal.search_refresh().unwrap();
        assert_eq!(terminal.search_position().1, 1);
        terminal.write(b"\x1b[?1049l");
        terminal.search_refresh().unwrap();
        assert_eq!(terminal.search_position(), (Some(2), 3));
        terminal.search_set("").unwrap();
        assert_eq!(terminal.search_position(), (None, 0));
    }
}
