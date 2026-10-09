//! Generated bindings to the pinned Zig terminal core and owning Rust handles.
use std::{fmt, ptr::NonNull};
mod graphics;
mod input;
mod png;
mod render;
mod search;
mod selection;

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    clippy::all
)]
pub mod ffi {
    include!(concat!(env!("OUT_DIR"), "/ghostty.rs"));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error(pub ffi::GhosttyResult);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Ghostty error {}", self.0)
    }
}
impl std::error::Error for Error {}

fn check(result: ffi::GhosttyResult) -> Result<(), Error> {
    if result == ffi::GHOSTTY_SUCCESS {
        Ok(())
    } else {
        Err(Error(result))
    }
}

/// A terminal is uniquely owned and used on the thread that created it.
pub struct Terminal {
    handle: NonNull<ffi::GhosttyTerminalImpl>,
    effects: Box<Effects>,
    renderer: Option<render::Renderer>,
    input: Option<input::Encoder>,
    search: Option<NonNull<ffi::GhosttySearchImpl>>,
    anchor: Option<NonNull<ffi::GhosttyTrackedGridRefImpl>>,
    graphics_cache: graphics::Cache,
}

#[derive(Default)]
struct Effects {
    replies: Vec<u8>,
    bell: bool,
    size: ffi::GhosttySizeReportSize,
    clipboard: Option<String>,
    notifications: Vec<String>,
}

unsafe extern "C" fn write_reply(
    _: ffi::GhosttyTerminal,
    userdata: *mut std::ffi::c_void,
    data: *const u8,
    length: usize,
) {
    if length == 0 {
        return;
    }
    // SAFETY: Ghostty calls synchronously, with live userdata and a borrowed byte slice.
    unsafe {
        (&mut *userdata.cast::<Effects>())
            .replies
            .extend_from_slice(std::slice::from_raw_parts(data, length));
    }
}
unsafe extern "C" fn ring_bell(_: ffi::GhosttyTerminal, userdata: *mut std::ffi::c_void) {
    // SAFETY: userdata points to our stable boxed effects for the terminal's lifetime.
    unsafe {
        (*userdata.cast::<Effects>()).bell = true;
    }
}

unsafe extern "C" fn copy_clipboard(
    _: ffi::GhosttyTerminal,
    userdata: *mut std::ffi::c_void,
    request: *const ffi::GhosttyClipboardWrite,
) {
    if request.is_null() {
        return;
    }
    // SAFETY: Ghostty supplies the live request and userdata synchronously. The
    // content is copied before return, and the request is answered in its lifetime.
    unsafe {
        let request_ref = &*request;
        if request_ref.contents_len > 0 && !request_ref.contents.is_null() {
            let data = (*request_ref.contents).data;
            if data.len > 0 && !data.ptr.is_null() {
                (*userdata.cast::<Effects>()).clipboard = Some(
                    String::from_utf8_lossy(std::slice::from_raw_parts(data.ptr, data.len))
                        .into_owned(),
                );
            }
        }
        if let Some(reply) = request_ref.reply {
            let response = ffi::GhosttyClipboardWriteReply {
                size: std::mem::size_of::<ffi::GhosttyClipboardWriteReply>(),
                result: ffi::GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS,
                remember: false,
            };
            reply(request, &response);
        }
    }
}

unsafe extern "C" fn notify(
    _: ffi::GhosttyTerminal,
    userdata: *mut std::ffi::c_void,
    notification: *const ffi::GhosttyTerminalDesktopNotification,
) {
    if notification.is_null() {
        return;
    }
    // SAFETY: Ghostty owns these strings throughout the synchronous callback.
    unsafe {
        let n = &*notification;
        let string = |s: ffi::GhosttyString| {
            if s.len == 0 || s.ptr.is_null() {
                String::new()
            } else {
                crate::osc::shorten(
                    &String::from_utf8_lossy(std::slice::from_raw_parts(s.ptr, s.len)),
                    95,
                )
            }
        };
        let (title, body) = (string(n.title), string(n.body));
        let message = if title.is_empty() {
            body
        } else if body.is_empty() {
            title
        } else {
            format!("{title}: {body}")
        };
        if !message.is_empty() {
            (*userdata.cast::<Effects>())
                .notifications
                .push(crate::osc::shorten(&message, 127));
        }
    }
}

unsafe extern "C" fn report_size(
    _: ffi::GhosttyTerminal,
    userdata: *mut std::ffi::c_void,
    out: *mut ffi::GhosttySizeReportSize,
) -> bool {
    if out.is_null() {
        return false;
    }
    // SAFETY: Ghostty supplies the live boxed userdata and a writable size
    // output for this synchronous callback. No terminal function is reentered.
    unsafe {
        *out = (*userdata.cast::<Effects>()).size;
    }
    true
}

impl Terminal {
    pub fn new(cols: u16, rows: u16) -> Result<Self, Error> {
        png::init()?;
        let mut handle = std::ptr::null_mut();
        // SAFETY: the default allocator is selected and the output is valid.
        unsafe {
            check(ffi::ghostty_terminal_new(
                std::ptr::null(),
                &mut handle,
                cols,
                rows,
            ))?;
        }
        let mut terminal = Self {
            handle: NonNull::new(handle).ok_or(Error(ffi::GHOSTTY_OUT_OF_MEMORY))?,
            effects: Box::default(),
            renderer: None,
            input: None,
            search: None,
            anchor: None,
            graphics_cache: graphics::Cache::default(),
        };
        terminal.effects.size = ffi::GhosttySizeReportSize {
            rows,
            columns: cols,
            cell_width: 0,
            cell_height: 0,
        };
        // SAFETY: boxed userdata stays at a stable address, and both callback
        // signatures match their selectors in terminal.h. They never reenter VT writes.
        unsafe {
            check(ffi::ghostty_terminal_set(
                handle,
                ffi::GHOSTTY_TERMINAL_OPT_USERDATA,
                (&mut *terminal.effects as *mut Effects).cast(),
            ))?;
            check(ffi::ghostty_terminal_set(
                handle,
                ffi::GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                (write_reply as *const ()).cast(),
            ))?;
            check(ffi::ghostty_terminal_set(
                handle,
                ffi::GHOSTTY_TERMINAL_OPT_SIZE,
                (report_size as *const ()).cast(),
            ))?;
            check(ffi::ghostty_terminal_set(
                handle,
                ffi::GHOSTTY_TERMINAL_OPT_BELL,
                (ring_bell as *const ()).cast(),
            ))?;
            check(ffi::ghostty_terminal_set(
                handle,
                ffi::GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE,
                (copy_clipboard as *const ()).cast(),
            ))?;
            check(ffi::ghostty_terminal_set(
                handle,
                ffi::GHOSTTY_TERMINAL_OPT_DESKTOP_NOTIFICATION,
                (notify as *const ()).cast(),
            ))?;
        }
        Ok(terminal)
    }

    pub fn write(&mut self, bytes: &[u8]) {
        // SAFETY: the handle is owned, and the slice lives throughout this synchronous call.
        unsafe {
            ffi::ghostty_terminal_vt_write(self.handle.as_ptr(), bytes.as_ptr(), bytes.len());
        }
    }

    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width: u32,
        cell_height: u32,
    ) -> Result<(), Error> {
        self.effects.size = ffi::GhosttySizeReportSize {
            rows,
            columns: cols,
            cell_width,
            cell_height,
        };
        // SAFETY: the handle is live and exclusively borrowed.
        unsafe {
            check(ffi::ghostty_terminal_resize(
                self.handle.as_ptr(),
                cols,
                rows,
                cell_width,
                cell_height,
            ))
        }
    }

    pub fn cursor(&self) -> Result<(u16, u16), Error> {
        let (mut x, mut y) = (0u16, 0u16);
        // SAFETY: these data selectors write uint16_t values, as declared by terminal.h.
        unsafe {
            check(ffi::ghostty_terminal_get(
                self.handle.as_ptr(),
                ffi::GHOSTTY_TERMINAL_DATA_CURSOR_X,
                (&mut x as *mut u16).cast(),
            ))?;
            check(ffi::ghostty_terminal_get(
                self.handle.as_ptr(),
                ffi::GHOSTTY_TERMINAL_DATA_CURSOR_Y,
                (&mut y as *mut u16).cast(),
            ))?;
        }
        Ok((x, y))
    }

    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.effects.replies)
    }
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.effects.bell)
    }
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.effects.clipboard.take()
    }
    pub fn take_notifications(&mut self) -> Vec<String> {
        std::mem::take(&mut self.effects.notifications)
    }
    pub fn title(&self) -> Result<String, Error> {
        self.title_bytes()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }
    pub fn title_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut title = ffi::GhosttyString::default();
        // SAFETY: this selector writes a borrowed GhosttyString; copy it before any mutation.
        unsafe {
            check(ffi::ghostty_terminal_get(
                self.handle.as_ptr(),
                ffi::GHOSTTY_TERMINAL_DATA_TITLE,
                (&mut title as *mut ffi::GhosttyString).cast(),
            ))?;
            if title.len == 0 {
                return Ok(Vec::new());
            }
            Ok(std::slice::from_raw_parts(title.ptr, title.len).to_vec())
        }
    }

    pub fn compose(
        &mut self,
        screen: &mut crate::screen::Screen,
        x: u16,
        y: u16,
        focused: bool,
    ) -> Result<(), Error> {
        if self.renderer.is_none() {
            self.renderer = Some(render::Renderer::new()?);
        }
        self.renderer
            .as_mut()
            .unwrap()
            .compose(self.handle.as_ptr(), screen, x, y, focused)
    }
    pub fn drop_render_cache(&mut self) {
        self.renderer = None;
    }

    pub fn set_mode_default(&mut self, mode: u16, value: bool) -> Result<(), Error> {
        // SAFETY: MODE_DEFAULT consumes a borrowed mode configuration synchronously.
        unsafe {
            let config = ffi::GhosttyTerminalModeConfig {
                mode: mode & 0x7fff,
                value,
            };
            check(ffi::ghostty_terminal_set(
                self.handle.as_ptr(),
                ffi::GHOSTTY_TERMINAL_OPT_MODE_DEFAULT,
                (&config as *const ffi::GhosttyTerminalModeConfig).cast(),
            ))
        }
    }
    pub fn alt_screen(&self) -> bool {
        let mut value = ffi::GHOSTTY_TERMINAL_SCREEN_PRIMARY;
        // SAFETY: ACTIVE_SCREEN writes one terminal-screen enum to a live output.
        unsafe {
            ffi::ghostty_terminal_get(
                self.handle.as_ptr(),
                ffi::GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN,
                (&mut value as *mut ffi::GhosttyTerminalScreen).cast(),
            );
        }
        value == ffi::GHOSTTY_TERMINAL_SCREEN_ALTERNATE
    }
    pub fn wants_mouse(&self) -> bool {
        let mut value = ffi::GHOSTTY_MOUSE_TRACKING_NONE;
        // SAFETY: MOUSE_TRACKING writes one tracking-mode enum to a live output.
        unsafe {
            ffi::ghostty_terminal_get(
                self.handle.as_ptr(),
                ffi::GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING,
                (&mut value as *mut ffi::GhosttyMouseTrackingMode).cast(),
            );
        }
        value != ffi::GHOSTTY_MOUSE_TRACKING_NONE
    }
    pub fn mode(&self, mode: u16) -> Result<bool, Error> {
        // SAFETY: MODE reads/writes one correctly shaped configuration.
        unsafe {
            let mut config = ffi::GhosttyTerminalModeConfig {
                mode: mode & 0x7fff,
                value: false,
            };
            check(ffi::ghostty_terminal_get(
                self.handle.as_ptr(),
                ffi::GHOSTTY_TERMINAL_DATA_MODE,
                (&mut config as *mut ffi::GhosttyTerminalModeConfig).cast(),
            ))?;
            Ok(config.value)
        }
    }
    pub fn set_scrollback(&mut self, lines: usize, bytes: usize) -> Result<(), Error> {
        let bytes = if lines == 0 { 0 } else { bytes };
        // SAFETY: these selectors borrow size_t pointers; NULL removes a byte ceiling.
        unsafe {
            check(ffi::ghostty_terminal_set(
                self.handle.as_ptr(),
                ffi::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES,
                (&lines as *const usize).cast(),
            ))?;
            check(ffi::ghostty_terminal_set(
                self.handle.as_ptr(),
                ffi::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,
                if bytes == 0 && lines != 0 {
                    std::ptr::null()
                } else {
                    (&bytes as *const usize).cast()
                },
            ))
        }
    }
    pub fn scroll(&mut self, delta: i64) {
        let scroll = ffi::GhosttyTerminalScrollViewport {
            tag: ffi::GHOSTTY_SCROLL_VIEWPORT_DELTA,
            value: ffi::GhosttyTerminalScrollViewportValue {
                delta: delta as isize,
            },
        };
        // SAFETY: live, exclusively borrowed terminal; active union member matches the tag.
        unsafe {
            ffi::ghostty_terminal_scroll_viewport(self.handle.as_ptr(), scroll);
        }
    }
    pub fn scroll_edge(&mut self, top: bool) {
        let scroll = ffi::GhosttyTerminalScrollViewport {
            tag: if top {
                ffi::GHOSTTY_SCROLL_VIEWPORT_TOP
            } else {
                ffi::GHOSTTY_SCROLL_VIEWPORT_BOTTOM
            },
            value: Default::default(),
        };
        // SAFETY: these tags do not read a union payload.
        unsafe {
            ffi::ghostty_terminal_scroll_viewport(self.handle.as_ptr(), scroll);
        }
    }
    pub fn scroll_position(&self) -> Result<(u32, u32), Error> {
        let mut bar = ffi::GhosttyTerminalScrollbar::default();
        // SAFETY: the selector writes one scrollbar value.
        unsafe {
            check(ffi::ghostty_terminal_get(
                self.handle.as_ptr(),
                ffi::GHOSTTY_TERMINAL_DATA_SCROLLBAR,
                (&mut bar as *mut ffi::GhosttyTerminalScrollbar).cast(),
            ))?;
        }
        Ok((bar.offset as u32, bar.total.saturating_sub(bar.len) as u32))
    }
    pub fn scrolled(&self) -> Result<bool, Error> {
        let mut active = true;
        // SAFETY: VIEWPORT_ACTIVE writes one bool.
        unsafe {
            check(ffi::ghostty_terminal_get(
                self.handle.as_ptr(),
                ffi::GHOSTTY_TERMINAL_DATA_VIEWPORT_ACTIVE,
                (&mut active as *mut bool).cast(),
            ))?;
        }
        Ok(!active)
    }
    pub fn encode_input(
        &mut self,
        event: &crate::input::Event,
        cols: u16,
        rows: u16,
        cell_w: u16,
        cell_h: u16,
    ) -> Result<Vec<u8>, Error> {
        if self.input.is_none() {
            self.input = Some(input::Encoder::new()?);
        }
        self.input
            .as_mut()
            .unwrap()
            .encode(self.handle.as_ptr(), event, cols, rows, cell_w, cell_h)
    }

    pub fn plain_text(&self) -> Result<String, Error> {
        let mut options = ffi::GhosttyFormatterTerminalOptions::default();
        options.size = std::mem::size_of_val(&options);
        options.emit = ffi::GHOSTTY_FORMATTER_FORMAT_PLAIN;
        options.trim = true;
        let mut handle = std::ptr::null_mut();
        // SAFETY: the formatter borrows this terminal only within this method.
        unsafe {
            check(ffi::ghostty_formatter_terminal_new(
                std::ptr::null(),
                &mut handle,
                self.handle.as_ptr(),
                options,
            ))?;
        }
        struct Formatter(ffi::GhosttyFormatter);
        impl Drop for Formatter {
            fn drop(&mut self) {
                // SAFETY: this wrapper owns the formatter.
                unsafe {
                    ffi::ghostty_formatter_free(self.0);
                }
            }
        }
        let formatter = Formatter(handle);
        let mut needed = 0;
        // SAFETY: NULL is the documented size-query operation.
        let status = unsafe {
            ffi::ghostty_formatter_format_buf(formatter.0, std::ptr::null_mut(), 0, &mut needed)
        };
        if status != ffi::GHOSTTY_OUT_OF_SPACE {
            check(status)?;
        }
        let mut bytes = vec![0; needed];
        // SAFETY: the allocated slice has the requested size.
        unsafe {
            check(ffi::ghostty_formatter_format_buf(
                formatter.0,
                bytes.as_mut_ptr(),
                bytes.len(),
                &mut needed,
            ))?;
        }
        bytes.truncate(needed);
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.renderer = None;
        self.input = None;
        if let Some(anchor) = self.anchor.take() {
            // SAFETY: the owned anchor is released before its terminal.
            unsafe { ffi::ghostty_tracked_grid_ref_free(anchor.as_ptr()) };
        }
        if let Some(search) = self.search.take() {
            // SAFETY: the search is uniquely owned and its terminal is still live.
            unsafe { ffi::ghostty_search_free(search.as_ptr()) };
        }
        // SAFETY: this is the unique owner; no borrowed formatter can escape.
        unsafe {
            ffi::ghostty_terminal_free(self.handle.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_and_notification_callbacks_copy_borrowed_strings() {
        let mut terminal = Terminal::new(20, 4).unwrap();
        terminal
            .write(b"\x1b]52;c;aGVsbG8=\x07\x1b]9;done\x07\x1b]777;notify;build;finished\x1b\\");
        assert_eq!(terminal.take_clipboard().as_deref(), Some("hello"));
        assert!(terminal.take_clipboard().is_none());
        assert_eq!(terminal.take_notifications(), ["done", "build: finished"]);
        assert!(terminal.take_notifications().is_empty());
    }

    #[test]
    fn vt_sequences_are_processed_by_ghostty() {
        let mut terminal = Terminal::new(20, 4).unwrap();
        terminal.write(b"hello\r\n\x1b[32mworld\x1b[0m");
        assert_eq!(terminal.cursor().unwrap(), (5, 1));
        let text = terminal.plain_text().unwrap();
        assert!(text.contains("hello\nworld"), "{text:?}");
        terminal.resize(10, 4, 8, 16).unwrap();
        assert!(terminal.plain_text().unwrap().contains("world"));
    }

    #[test]
    fn size_queries_and_resize_reports_share_cell_metrics() {
        let mut terminal = Terminal::new(20, 4).unwrap();
        terminal.resize(20, 4, 9, 22).unwrap();
        terminal.write(b"\x1b[14t\x1b[16t\x1b[18t\x1b[?2048h");
        assert_eq!(
            terminal.take_replies(),
            b"\x1b[4;88;180t\x1b[6;22;9t\x1b[8;4;20t\x1b[48;4;20;88;180t"
        );
        terminal.resize(30, 6, 8, 16).unwrap();
        assert_eq!(terminal.take_replies(), b"\x1b[48;6;30;96;240t");
    }

    #[test]
    fn input_encoding_tracks_live_terminal_modes() {
        use crate::input::Event;
        let mut terminal = Terminal::new(20, 4).unwrap();
        let up = Event::Key {
            key: ffi::GHOSTTY_KEY_ARROW_UP,
            mods: 0,
            action: 1,
            text: Vec::new(),
            unshifted: 0,
        };
        assert_eq!(terminal.encode_input(&up, 20, 4, 8, 16).unwrap(), b"\x1b[A");
        terminal.write(b"\x1b[?1h");
        assert_eq!(terminal.encode_input(&up, 20, 4, 8, 16).unwrap(), b"\x1bOA");
        let focus = Event::Focus(true);
        assert!(
            terminal
                .encode_input(&focus, 20, 4, 8, 16)
                .unwrap()
                .is_empty()
        );
        terminal.write(b"\x1b[?1004h\x1b[?2004h");
        assert_eq!(
            terminal.encode_input(&focus, 20, 4, 8, 16).unwrap(),
            b"\x1b[I"
        );
        assert_eq!(
            terminal
                .encode_input(&Event::Paste(b"hello".to_vec()), 20, 4, 8, 16)
                .unwrap(),
            b"\x1b[200~hello\x1b[201~"
        );
        terminal.write(b"\x1b[?1000h\x1b[?1006h");
        let click = Event::Mouse {
            button: 1,
            action: 0,
            x: 3,
            y: 2,
            mods: 0,
        };
        assert_eq!(
            terminal.encode_input(&click, 20, 4, 8, 16).unwrap(),
            b"\x1b[<0;4;3M"
        );
        terminal.write(b"\x1b[?1000l");
        assert!(
            terminal
                .encode_input(&click, 20, 4, 8, 16)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn rejects_zero_dimensions() {
        assert!(Terminal::new(0, 4).is_err());
        assert!(Terminal::new(4, 0).is_err());
    }

    #[test]
    fn terminal_callbacks_reply_without_reentering() {
        let mut terminal = Terminal::new(20, 4).unwrap();
        terminal.write(b"abc\x1b[6n\x07\x1b]2;test title\x07");
        assert_eq!(terminal.take_replies(), b"\x1b[1;4R");
        assert!(terminal.take_bell());
        assert!(!terminal.take_bell());
        assert_eq!(terminal.title().unwrap(), "test title");
    }
}
