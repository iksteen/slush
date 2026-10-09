use super::{ENTER, LEAVE};
use crate::pty::Size;
use std::{
    io::{self, Read, Write},
    sync::atomic::{AtomicBool, Ordering},
};
static RESIZED: AtomicBool = AtomicBool::new(false);
static QUIT: AtomicBool = AtomicBool::new(false);
extern "C" fn signal(sig: libc::c_int) {
    if sig == libc::SIGWINCH {
        RESIZED.store(true, Ordering::Relaxed);
    } else {
        QUIT.store(true, Ordering::Relaxed);
    }
}
pub fn terminal_size() -> Size {
    // SAFETY: winsize is a plain C output structure; the ioctl only queries stdout.
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } == 0;
    if ok && ws.ws_col > 0 && ws.ws_row > 0 {
        Size {
            cols: ws.ws_col,
            rows: ws.ws_row,
            cell_width: ws.ws_xpixel / ws.ws_col,
            cell_height: ws.ws_ypixel / ws.ws_row,
        }
    } else {
        Size {
            cols: 80,
            rows: 24,
            cell_width: 0,
            cell_height: 0,
        }
    }
}
pub struct TerminalGuard {
    saved: libc::termios,
    signals: Vec<(i32, libc::sigaction)>,
}
impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        QUIT.store(false, Ordering::Relaxed);
        RESIZED.store(false, Ordering::Relaxed);
        // SAFETY: termios and sigaction contain only C scalar fields/pointers.
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut saved) } != 0 {
            return Err(io::Error::other("not a terminal"));
        }
        let mut raw = saved;
        // SAFETY: cfmakeraw mutates a valid termios; tcsetattr applies it to stdin.
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut guard = Self {
            saved,
            signals: Vec::new(),
        };
        for sig in [libc::SIGWINCH, libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: zero initialization and sigemptyset construct a valid action.
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            let mut old = action;
            action.sa_sigaction = signal as *const () as usize;
            unsafe {
                libc::sigemptyset(&mut action.sa_mask);
            }
            if unsafe { libc::sigaction(sig, &action, &mut old) } != 0 {
                return Err(io::Error::last_os_error());
            }
            guard.signals.push((sig, old));
        }
        io::stdout().write_all(ENTER)?;
        Ok(guard)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // SAFETY: saved modes came from this descriptor; actions came from sigaction.
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &self.saved);
        }
        let _ = io::stdout().write_all(LEAVE);
        for (sig, old) in &self.signals {
            unsafe {
                libc::sigaction(*sig, old, std::ptr::null_mut());
            }
        }
    }
}

impl TerminalGuard {
    pub fn descriptor(&self) -> crate::poller::Descriptor {
        libc::STDIN_FILENO
    }
    pub fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        io::stdin().read(bytes)
    }
}
pub fn is_terminal() -> bool {
    // SAFETY: querying whether stdout is a terminal.
    unsafe { libc::isatty(libc::STDOUT_FILENO) != 0 }
}
pub fn take_resize(_: &mut TerminalGuard) -> bool {
    RESIZED.swap(false, Ordering::Relaxed)
}
pub fn quit() -> bool {
    QUIT.load(Ordering::Relaxed)
}
pub fn request_quit() {
    QUIT.store(true, Ordering::Relaxed);
}
pub const POLL_TIMEOUT: i32 = -1;
