//! Owning POSIX PTYs. Open and size the slave before spawning, avoiding the
//! initial-resize race that affects Darwin when the child opens its own slave.
use std::{
    collections::VecDeque,
    ffi::OsStr,
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}
impl Size {
    fn winsize(self) -> libc::winsize {
        libc::winsize {
            ws_col: self.cols,
            ws_row: self.rows,
            ws_xpixel: self.cols.wrapping_mul(self.cell_width),
            ws_ypixel: self.rows.wrapping_mul(self.cell_height),
        }
    }
}

pub struct Pty {
    master: Option<File>,
    child: Child,
    output: VecDeque<u8>,
    pub eof: bool,
    pub exit_status: Option<ExitStatus>,
}

impl Pty {
    pub fn spawn<I, S>(
        argv: I,
        size: Size,
        cwd: Option<&Path>,
        pane_id: u32,
        session: Option<&OsStr>,
    ) -> io::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut argv = argv.into_iter();
        let executable = argv
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty pane command"))?;
        let (mut master, mut slave) = (-1, -1);
        let mut winsize = size.winsize();
        // SAFETY: output fd pointers and size are valid; NULL selects default termios.
        if unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::addr_of_mut!(winsize),
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openpty returned two owned file descriptors.
        let master = unsafe { File::from_raw_fd(master) };
        // SAFETY: the slave descriptor is distinct and uniquely owned.
        let slave = unsafe { File::from_raw_fd(slave) };
        for file in [&master, &slave] {
            // SAFETY: fcntl operates on an owned fd with a valid descriptor flag.
            if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        // SAFETY: F_GETFL takes no optional argument; fd is live.
        let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
        // SAFETY: F_SETFL accepts the returned flags plus O_NONBLOCK.
        if flags < 0
            || unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                < 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut command = Command::new(executable);
        command
            .args(argv)
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        if let Some(cwd) = cwd.filter(|path| path.is_dir()) {
            command.current_dir(cwd);
        }
        command
            .env("TERM", crate::terminfo::terminal_name())
            .env("SLOSH", "1")
            .env("SLOSH_BIN", std::env::current_exe()?)
            .env("SLOSH_SESSION", session.unwrap_or_else(|| OsStr::new("")))
            .env_remove("ZELLIJ");
        if pane_id == 0 {
            command.env_remove("SLOSH_PANE");
        } else {
            command.env("SLOSH_PANE", pane_id.to_string());
        }
        // SAFETY: the child closure uses only async-signal-safe system calls and
        // error construction, after Rust has configured stdio and before exec.
        unsafe {
            command.pre_exec(|| {
                #[cfg(target_os = "macos")]
                let request = libc::c_ulong::from(libc::TIOCSCTTY);
                #[cfg(not(target_os = "macos"))]
                let request = libc::TIOCSCTTY;
                if libc::setsid() < 0 || libc::ioctl(libc::STDIN_FILENO, request, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                libc::signal(libc::SIGPIPE, libc::SIG_DFL);
                Ok(())
            });
        }
        let child = command.spawn()?;
        // Command retains Stdio handles after spawn; drop them to avoid holding
        // the slave open after the actual pane process exits.
        drop(command);
        Ok(Self {
            master: Some(master),
            child,
            output: VecDeque::new(),
            eof: false,
            exit_status: None,
        })
    }

    pub fn fd(&self) -> i32 {
        self.master.as_ref().unwrap().as_raw_fd()
    }
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
    pub fn resize(&self, size: Size) -> io::Result<()> {
        // SAFETY: a live PTY fd and pointer to the correctly shaped winsize.
        if unsafe { libc::ioctl(self.fd(), libc::TIOCSWINSZ, &size.winsize()) } < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    pub fn read_available(&mut self) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        let mut buffer = [0; 65536];
        loop {
            match self.master.as_mut().unwrap().read(&mut buffer) {
                Ok(0) => {
                    self.eof = true;
                    break;
                }
                Ok(n) => bytes.extend_from_slice(&buffer[..n]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                // Linux's PTY master reports EIO when the final slave closes.
                Err(error) if error.raw_os_error() == Some(libc::EIO) => {
                    self.eof = true;
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        self.exit_status = self.child.try_wait()?;
        if self.eof && self.exit_status.is_none() {
            for _ in 0..5 {
                std::thread::sleep(Duration::from_micros(200));
                self.exit_status = self.child.try_wait()?;
                if self.exit_status.is_some() {
                    break;
                }
            }
        }
        Ok(bytes)
    }
    pub fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.output.extend(bytes);
        self.flush_pending()
    }
    pub fn wants_write(&self) -> bool {
        !self.output.is_empty()
    }
    pub fn flush_pending(&mut self) -> io::Result<()> {
        while !self.output.is_empty() {
            let (front, _) = self.output.as_slices();
            match self.master.as_mut().unwrap().write(front) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "PTY write returned zero",
                    ));
                }
                Ok(n) => {
                    self.output.drain(..n);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
    pub fn poll(&self, timeout: Duration) -> io::Result<bool> {
        let mut fd = libc::pollfd {
            fd: self.fd(),
            events: libc::POLLIN | if self.wants_write() { libc::POLLOUT } else { 0 },
            revents: 0,
        };
        // SAFETY: fd points to one valid pollfd; timeout is bounded to c_int.
        let result =
            unsafe { libc::poll(&mut fd, 1, timeout.as_millis().min(i32::MAX as u128) as i32) };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(result > 0)
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // Close the master before waiting: Darwin can keep a child in terminal
        // setup even after SIGKILL while its master remains open.
        drop(self.master.take());
        if self.exit_status.is_some() {
            return;
        }
        // SAFETY: the unreaped direct child still owns this pid. Match slosh's
        // SIGHUP behavior, then bound cleanup even for programs ignoring HUP.
        unsafe {
            libc::kill(self.child.id() as libc::pid_t, libc::SIGHUP);
        }
        let deadline = Instant::now() + Duration::from_millis(20);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_micros(200));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_a_just_started_shell_does_not_wait_for_terminal_setup() {
        let start = Instant::now();
        for _ in 0..16 {
            let pty = Pty::spawn(
                ["/bin/sh"],
                Size {
                    cols: 80,
                    rows: 24,
                    cell_width: 8,
                    cell_height: 16,
                },
                None,
                1,
                None,
            )
            .unwrap();
            drop(pty);
        }
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn slave_size_and_pane_environment_exist_before_exec() {
        let size = Size {
            cols: 40,
            rows: 12,
            cell_width: 8,
            cell_height: 16,
        };
        let mut pty = Pty::spawn(
            [
                "/bin/sh",
                "-c",
                "stty size; printf '%s:%s:%s' \"$SLOSH\" \"$SLOSH_PANE\" \"$SLOSH_SESSION\"",
            ],
            size,
            None,
            42,
            None,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut bytes = Vec::new();
        while !pty.eof && Instant::now() < deadline {
            pty.poll(Duration::from_millis(20)).unwrap();
            bytes.extend(pty.read_available().unwrap());
        }
        assert!(pty.eof, "pane did not close");
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("12 40\r\n"), "{text:?}");
        assert!(text.ends_with("1:42:"), "{text:?}");
    }

    #[test]
    fn real_pty_output_reaches_ghostty() {
        let size = Size {
            cols: 20,
            rows: 4,
            cell_width: 8,
            cell_height: 16,
        };
        let mut pty = Pty::spawn(
            ["/bin/sh", "-c", "printf '\\033[32mhello\\033[0m\\n'"],
            size,
            None,
            1,
            None,
        )
        .unwrap();
        let mut terminal = crate::ghostty::Terminal::new(20, 4).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !pty.eof && Instant::now() < deadline {
            pty.poll(Duration::from_millis(20)).unwrap();
            terminal.write(&pty.read_available().unwrap());
        }
        assert!(pty.eof);
        assert!(terminal.plain_text().unwrap().contains("hello"));
    }
}
