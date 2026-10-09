//! Protocol-aware input encoding is provided by Ghostty, as in slosh.
use super::{Error, check, ffi::*};
use crate::input::{Event, SHIFT};

pub(super) struct Encoder {
    key: GhosttyKeyEncoder,
    key_event: GhosttyKeyEvent,
    mouse: GhosttyMouseEncoder,
    mouse_event: GhosttyMouseEvent,
}
impl Encoder {
    pub fn new() -> Result<Self, Error> {
        let mut encoder = Self {
            key: std::ptr::null_mut(),
            key_event: std::ptr::null_mut(),
            mouse: std::ptr::null_mut(),
            mouse_event: std::ptr::null_mut(),
        };
        // SAFETY: each constructor initializes its output handle; Drop cleans partial construction.
        unsafe {
            check(ghostty_key_encoder_new(std::ptr::null(), &mut encoder.key))?;
            check(ghostty_key_event_new(
                std::ptr::null(),
                &mut encoder.key_event,
            ))?;
            check(ghostty_mouse_encoder_new(
                std::ptr::null(),
                &mut encoder.mouse,
            ))?;
            check(ghostty_mouse_event_new(
                std::ptr::null(),
                &mut encoder.mouse_event,
            ))?;
        }
        Ok(encoder)
    }
    pub fn encode(
        &mut self,
        terminal: GhosttyTerminal,
        event: &Event,
        cols: u16,
        rows: u16,
        cw: u16,
        ch: u16,
    ) -> Result<Vec<u8>, Error> {
        let mut out = vec![0; 128];
        let mut n = 0;
        // SAFETY: this wrapper owns live encoders/events; terminal is borrowed
        // synchronously from its owning Terminal and all buffers remain live.
        unsafe {
            match event {
                Event::Key {
                    key,
                    mods,
                    action,
                    text,
                    unshifted,
                } => {
                    ghostty_key_encoder_setopt_from_terminal(self.key, terminal);
                    ghostty_key_event_set_key(self.key_event, *key);
                    ghostty_key_event_set_mods(self.key_event, *mods);
                    ghostty_key_event_set_action(self.key_event, *action as GhosttyKeyAction);
                    ghostty_key_event_set_utf8(
                        self.key_event,
                        if text.is_empty() {
                            std::ptr::null()
                        } else {
                            text.as_ptr().cast()
                        },
                        text.len(),
                    );
                    ghostty_key_event_set_unshifted_codepoint(self.key_event, *unshifted);
                    let cp = std::str::from_utf8(text)
                        .ok()
                        .and_then(|s| s.chars().next())
                        .map(|c| c as u32);
                    let consumed = if *mods & SHIFT != 0 && cp.is_some_and(|cp| cp != *unshifted) {
                        SHIFT
                    } else {
                        0
                    };
                    ghostty_key_event_set_consumed_mods(self.key_event, consumed);
                    check(ghostty_key_encoder_encode(
                        self.key,
                        self.key_event,
                        out.as_mut_ptr().cast(),
                        out.len(),
                        &mut n,
                    ))?;
                }
                Event::Mouse {
                    button,
                    action,
                    x,
                    y,
                    mods,
                } => {
                    ghostty_mouse_encoder_setopt_from_terminal(self.mouse, terminal);
                    let (cw, ch) = (cw.max(1), ch.max(1));
                    let size = GhosttyMouseEncoderSize {
                        size: std::mem::size_of::<GhosttyMouseEncoderSize>(),
                        screen_width: cols as u32 * cw as u32,
                        screen_height: rows as u32 * ch as u32,
                        cell_width: cw as u32,
                        cell_height: ch as u32,
                        ..Default::default()
                    };
                    ghostty_mouse_encoder_setopt(
                        self.mouse,
                        GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                        (&size as *const GhosttyMouseEncoderSize).cast(),
                    );
                    let position = GhosttyMousePosition {
                        x: *x as f32 * cw as f32 + cw as f32 / 2.0,
                        y: *y as f32 * ch as f32 + ch as f32 / 2.0,
                    };
                    ghostty_mouse_event_set_position(self.mouse_event, position);
                    if *button == 0 {
                        ghostty_mouse_event_clear_button(self.mouse_event);
                    } else {
                        ghostty_mouse_event_set_button(
                            self.mouse_event,
                            *button as GhosttyMouseButton,
                        );
                    }
                    ghostty_mouse_event_set_action(self.mouse_event, *action as GhosttyMouseAction);
                    ghostty_mouse_event_set_mods(self.mouse_event, *mods);
                    check(ghostty_mouse_encoder_encode(
                        self.mouse,
                        self.mouse_event,
                        out.as_mut_ptr().cast(),
                        out.len(),
                        &mut n,
                    ))?;
                }
                Event::Paste(text) => {
                    let mut text = text.clone();
                    let mut mode = GhosttyTerminalModeConfig {
                        mode: 2004,
                        value: false,
                    };
                    check(ghostty_terminal_get(
                        terminal,
                        GHOSTTY_TERMINAL_DATA_MODE,
                        (&mut mode as *mut GhosttyTerminalModeConfig).cast(),
                    ))?;
                    let result = ghostty_paste_encode(
                        text.as_mut_ptr().cast(),
                        text.len(),
                        mode.value,
                        std::ptr::null_mut(),
                        0,
                        &mut n,
                    );
                    if result != GHOSTTY_OUT_OF_SPACE {
                        check(result)?;
                    }
                    out.resize(n.max(text.len() + 16), 0);
                    check(ghostty_paste_encode(
                        text.as_mut_ptr().cast(),
                        text.len(),
                        mode.value,
                        out.as_mut_ptr().cast(),
                        out.len(),
                        &mut n,
                    ))?;
                }
                Event::Focus(focused) => {
                    let mut mode = GhosttyTerminalModeConfig {
                        mode: 1004,
                        value: false,
                    };
                    check(ghostty_terminal_get(
                        terminal,
                        GHOSTTY_TERMINAL_DATA_MODE,
                        (&mut mode as *mut GhosttyTerminalModeConfig).cast(),
                    ))?;
                    if mode.value {
                        out[..3].copy_from_slice(if *focused { b"\x1b[I" } else { b"\x1b[O" });
                        n = 3;
                    }
                }
            }
        }
        out.truncate(n);
        Ok(out)
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: each non-NULL handle is uniquely owned by this wrapper.
        unsafe {
            if !self.key.is_null() {
                ghostty_key_encoder_free(self.key);
            }
            if !self.key_event.is_null() {
                ghostty_key_event_free(self.key_event);
            }
            if !self.mouse.is_null() {
                ghostty_mouse_encoder_free(self.mouse);
            }
            if !self.mouse_event.is_null() {
                ghostty_mouse_event_free(self.mouse_event);
            }
        }
    }
}
