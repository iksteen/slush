//! Native console modes and a cancellable console-input pump.
use super::{ENTER, LEAVE};
use crate::{local_socket::AsDescriptor, pty::Size};
use std::{
    fs::File,
    io::{self, Read, Write},
    net::{Shutdown, TcpStream},
    os::windows::io::{AsRawHandle, FromRawHandle},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};
use windows_sys::Win32::{
    Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, INVALID_HANDLE_VALUE},
    System::{Console::*, IO::CancelSynchronousIo, Threading::GetCurrentProcess},
};

static QUIT: AtomicBool = AtomicBool::new(false);
unsafe extern "system" fn control(_: u32) -> i32 {
    QUIT.store(true, Ordering::Relaxed);
    1
}
pub fn quit() -> bool {
    QUIT.load(Ordering::Relaxed)
}
pub fn request_quit() {
    QUIT.store(true, Ordering::Relaxed);
}
pub const POLL_TIMEOUT: i32 = 100;
pub fn is_terminal() -> bool {
    let mut mode = 0;
    // SAFETY: querying the borrowed standard output handle with a valid output pointer.
    unsafe { GetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), &mut mode) != 0 }
}
pub fn terminal_size() -> Size {
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    // SAFETY: the console query writes only the provided structure.
    if unsafe { GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &mut info) } != 0 {
        Size {
            cols: (info.srWindow.Right - info.srWindow.Left + 1).max(1) as u16,
            rows: (info.srWindow.Bottom - info.srWindow.Top + 1).max(1) as u16,
            cell_width: 0,
            cell_height: 0,
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
    input: HANDLE,
    output: HANDLE,
    modes: (u32, u32),
    codepages: (u32, u32),
    socket: TcpStream,
    pump: Option<JoinHandle<()>>,
    stopping: Arc<AtomicBool>,
    handler: bool,
    size: (u16, u16),
}
impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        QUIT.store(false, Ordering::Relaxed);
        // SAFETY: handles remain owned by the process, and mode pointers are valid.
        let (input, output) = unsafe {
            (
                GetStdHandle(STD_INPUT_HANDLE),
                GetStdHandle(STD_OUTPUT_HANDLE),
            )
        };
        let (mut in_mode, mut out_mode) = (0, 0);
        if input.is_null()
            || output.is_null()
            || input == INVALID_HANDLE_VALUE
            || output == INVALID_HANDLE_VALUE
            || unsafe { GetConsoleMode(input, &mut in_mode) } == 0
            || unsafe { GetConsoleMode(output, &mut out_mode) } == 0
        {
            return Err(io::Error::other("not a terminal"));
        }
        let (socket, mut inner) = crate::local_socket::pump_pair()?;
        let mut guard = Self {
            input,
            output,
            modes: (in_mode, out_mode),
            // SAFETY: querying the current console code pages.
            codepages: unsafe { (GetConsoleCP(), GetConsoleOutputCP()) },
            socket,
            pump: None,
            stopping: Arc::new(AtomicBool::new(false)),
            handler: false,
            size: (0, 0),
        };
        let in_mode = (in_mode & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT))
            | ENABLE_VIRTUAL_TERMINAL_INPUT
            | ENABLE_WINDOW_INPUT;
        // SAFETY: these handles name our console; guard restores all changes on failure.
        if unsafe { SetConsoleMode(input, in_mode) } == 0
            || unsafe {
                SetConsoleMode(
                    output,
                    out_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING | DISABLE_NEWLINE_AUTO_RETURN,
                )
            } == 0
            || unsafe { SetConsoleCP(65001) } == 0
            || unsafe { SetConsoleOutputCP(65001) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the callback only sets a static atomic flag.
        if unsafe { SetConsoleCtrlHandler(Some(control), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        guard.handler = true;
        let mut duplicate = std::ptr::null_mut();
        // SAFETY: duplicate the console handle without inheriting it. Only the
        // input pump owns the duplicate, so it never closes the process's stdin.
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                input,
                GetCurrentProcess(),
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut source = unsafe { File::from_raw_handle(duplicate) };
        let stopping = guard.stopping.clone();
        guard.pump = Some(
            std::thread::Builder::new()
                .name("console-input".into())
                .spawn(move || {
                    let mut buffer = [0; 4096];
                    while !stopping.load(Ordering::Acquire) {
                        match source.read(&mut buffer) {
                            Ok(0) => break,
                            Ok(n) => {
                                if inner.write_all(&buffer[..n]).is_err() {
                                    break;
                                }
                            }
                            Err(e)
                                if e.kind() == io::ErrorKind::Interrupted
                                    && !stopping.load(Ordering::Acquire) =>
                            {
                                continue;
                            }
                            Err(_) => break,
                        }
                    }
                    let _ = inner.shutdown(Shutdown::Write);
                })?,
        );
        let size = terminal_size();
        guard.size = (size.cols, size.rows);
        io::stdout().write_all(ENTER)?;
        Ok(guard)
    }
    pub fn descriptor(&self) -> crate::poller::Descriptor {
        self.socket.descriptor()
    }
    pub fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.socket.read(bytes)
    }
}
pub fn take_resize(guard: &mut TerminalGuard) -> bool {
    let size = terminal_size();
    let fresh = (size.cols, size.rows);
    let changed = fresh != guard.size;
    guard.size = fresh;
    changed
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = self.socket.shutdown(Shutdown::Both);
        if let Some(pump) = self.pump.take() {
            // Cancellation can race the thread beginning its read. Retry until
            // it has stopped, with a bound so detach never hangs the client.
            for _ in 0..100 {
                if pump.is_finished() {
                    break;
                }
                // SAFETY: JoinHandle retains the native thread handle.
                unsafe {
                    CancelSynchronousIo(pump.as_raw_handle());
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            if pump.is_finished() {
                let _ = pump.join();
            }
        }
        // SAFETY: restore the saved modes/code pages on the borrowed handles.
        unsafe {
            SetConsoleMode(self.input, self.modes.0);
            SetConsoleMode(self.output, self.modes.1);
            SetConsoleCP(self.codepages.0);
            SetConsoleOutputCP(self.codepages.1);
            if self.handler {
                SetConsoleCtrlHandler(Some(control), 0);
            }
        }
        let _ = io::stdout().write_all(LEAVE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ghostty::Terminal, pty::Pty};
    use std::time::{Duration, Instant};

    #[test]
    fn console_input_and_restore() {
        console_driver(false, "console_input_and_restore");
    }
    #[test]
    fn console_resize_detection() {
        console_driver(true, "console_resize_detection");
    }
    fn console_driver(resize: bool, name: &str) {
        // Run the same test inside an actual ConPTY, so GetConsoleMode and
        // ReadFile exercise real console input rather than redirected stdin.
        if std::env::var("SLUSH_CONSOLE_DRIVER").as_deref() == Ok("child") {
            let (mut before_in, mut before_out) = (0, 0);
            unsafe {
                assert_ne!(
                    GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &mut before_in),
                    0
                );
                assert_ne!(
                    GetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), &mut before_out),
                    0
                );
            }
            let pages = unsafe { (GetConsoleCP(), GetConsoleOutputCP()) };
            let mut guard = TerminalGuard::enter().unwrap();
            let initial = terminal_size();
            assert_eq!((initial.cols, initial.rows), (60, 12));
            print!("\r\nconsole-ready\r\n");
            io::stdout().flush().unwrap();
            let mut fd = crate::poller::PollFd {
                fd: guard.descriptor(),
                events: crate::poller::READ,
                revents: 0,
            };
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut input = Vec::new();
            let mut bytes = [0; 128];
            while !input.contains(&b'\r') {
                assert!(Instant::now() < deadline, "console received no input");
                if crate::poller::poll(std::slice::from_mut(&mut fd), 100).unwrap() > 0 {
                    let n = guard.read(&mut bytes).unwrap();
                    assert!(n > 0);
                    input.extend_from_slice(&bytes[..n]);
                }
            }
            assert_eq!(String::from_utf8(input).unwrap(), "typed-é\r");
            // ResizePseudoConsole is delivered asynchronously by conhost.
            // Match the client loop's bounded polling, rather than requiring
            // the resize to have arrived before its next input byte.
            let resize_deadline = Instant::now() + Duration::from_secs(5);
            while resize && !take_resize(&mut guard) {
                assert!(Instant::now() < resize_deadline, "console did not resize");
                std::thread::sleep(Duration::from_millis(10));
            }
            let fresh = terminal_size();
            assert_eq!(
                (fresh.cols, fresh.rows),
                if resize { (70, 14) } else { (60, 12) }
            );
            drop(guard);
            let (mut after_in, mut after_out) = (0, 0);
            unsafe {
                GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &mut after_in);
                GetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), &mut after_out);
            }
            assert_eq!((after_in, after_out), (before_in, before_out));
            assert_eq!(unsafe { (GetConsoleCP(), GetConsoleOutputCP()) }, pages);
            print!("\r\nconsole-restored-ok\r\n");
            io::stdout().flush().unwrap();
            return;
        }
        let executable = std::env::current_exe().unwrap();
        let command = format!(
            "set \"SLUSH_CONSOLE_DRIVER=child\" & \"{}\" --exact client::native::tests::{name} --nocapture",
            executable.display()
        );
        let mut pty = Pty::spawn(
            [
                std::ffi::OsStr::new("cmd.exe"),
                std::ffi::OsStr::new("/c"),
                std::ffi::OsStr::new(&command),
            ],
            Size {
                cols: 60,
                rows: 12,
                cell_width: 8,
                cell_height: 16,
            },
            None,
            1,
            None,
        )
        .unwrap();
        let mut terminal = Terminal::new(60, 12).unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut sent = false;
        let mut bytes = Vec::new();
        while !pty.eof && Instant::now() < deadline {
            pty.poll(Duration::from_millis(20)).unwrap();
            let output = pty.read_available().unwrap();
            terminal.write(&output);
            bytes.extend_from_slice(&output);
            if !sent && terminal.plain_text().unwrap().contains("console-ready") {
                let foreground = crate::process_tree::foreground(pty.pid()).unwrap();
                assert!(foreground.contains("slush"), "{foreground}");
                if resize {
                    pty.resize(Size {
                        cols: 70,
                        rows: 14,
                        cell_width: 8,
                        cell_height: 16,
                    })
                    .unwrap();
                }
                pty.send("typed-é\r".as_bytes()).unwrap();
                sent = true;
            }
            pty.flush_pending().unwrap();
        }
        let output = String::from_utf8_lossy(&bytes);
        assert!(sent && pty.eof, "console driver did not finish: {output}");
        assert!(output.contains("console-restored-ok"), "{output}");
        assert!(pty.exit_status.unwrap().success(), "{output}");
    }
}
