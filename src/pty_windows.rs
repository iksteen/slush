//! Native ConPTYs, bridged to pollable sockets by blocking pipe pumps.
use std::{
    collections::VecDeque,
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, AsRawSocket, FromRawHandle, OwnedHandle},
        process::ExitStatusExt,
    },
    path::Path,
    process::ExitStatus,
    sync::{Arc, Mutex},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::{Console::*, Pipes::CreatePipe, Threading::*},
};

#[derive(Debug, Clone, Copy)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}
impl Size {
    fn coord(self) -> io::Result<COORD> {
        if self.cols == 0
            || self.rows == 0
            || self.cols > i16::MAX as u16
            || self.rows > i16::MAX as u16
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid ConPTY size",
            ));
        }
        Ok(COORD {
            X: self.cols as i16,
            Y: self.rows as i16,
        })
    }
}

struct Console(Mutex<Option<HPCON>>);
impl Console {
    fn close(&self) {
        // Release the lock before closing: draining a console may block, and
        // Drop must still be able to terminate the child and shut down pumps.
        let handle = self.0.lock().unwrap().take();
        if let Some(handle) = handle {
            // SAFETY: taking the handle ensures exactly one close.
            unsafe { ClosePseudoConsole(handle) };
        }
    }
}
impl Drop for Console {
    fn drop(&mut self) {
        self.close();
    }
}

struct Process {
    handle: OwnedHandle,
    pid: u32,
}
impl Process {
    fn status(&self) -> io::Result<Option<ExitStatus>> {
        // SAFETY: the process handle remains owned by this object.
        match unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                // SAFETY: code is a valid output pointer.
                if unsafe { GetExitCodeProcess(self.handle.as_raw_handle(), &mut code) } == 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(Some(ExitStatus::from_raw(code)))
                }
            }
            _ => Err(io::Error::last_os_error()),
        }
    }
}

struct Attributes {
    storage: Vec<usize>,
}
impl Attributes {
    fn new(console: HPCON) -> io::Result<Self> {
        let mut size = 0;
        // SAFETY: querying the allocation size with a NULL attribute list.
        unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size) };
        if size == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
        let ptr = storage.as_mut_ptr().cast();
        // SAFETY: storage is aligned and has the queried capacity.
        if unsafe { InitializeProcThreadAttributeList(ptr, 1, 0, &mut size) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let attributes = Self { storage };
        // SAFETY: the pseudoconsole attribute takes the handle value, rather
        // than a pointer to it, and the console stays alive through spawn.
        if unsafe {
            UpdateProcThreadAttribute(
                ptr,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                console as *const _,
                std::mem::size_of::<HPCON>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(attributes)
    }
    fn ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: the list was initialized and has not been deleted.
        unsafe { DeleteProcThreadAttributeList(self.ptr()) };
    }
}

fn pipe() -> io::Result<(File, File)> {
    let (mut read, mut write): (HANDLE, HANDLE) = (std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: both output pointers are valid; NULL attributes disable inheritance.
    if unsafe { CreatePipe(&mut read, &mut write, std::ptr::null(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreatePipe transferred two distinct, owned handles.
    Ok(unsafe { (File::from_raw_handle(read), File::from_raw_handle(write)) })
}

fn wide(text: &OsStr) -> io::Result<Vec<u16>> {
    let mut result: Vec<_> = text.encode_wide().collect();
    if result.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in process argument",
        ));
    }
    result.push(0);
    Ok(result)
}

// Quote for the Windows CRT/CommandLineToArgvW parser. Backslashes before
// quotes and at the end of a quoted argument need doubling.
fn quote(arg: &OsStr) -> Vec<u16> {
    let mut out = vec![b'"' as u16];
    let mut slashes = 0;
    for c in arg.encode_wide() {
        if c == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        out.extend(std::iter::repeat_n(
            b'\\' as u16,
            if c == b'"' as u16 {
                slashes * 2 + 1
            } else {
                slashes
            },
        ));
        out.push(c);
        slashes = 0;
    }
    out.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    out.push(b'"' as u16);
    out
}
fn command_line(argv: &[OsString]) -> io::Result<Vec<u16>> {
    let shell = argv.first().map(|s| {
        s.to_string_lossy()
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
    });
    let verbatim = shell.is_some_and(|s| s == "cmd.exe" || s == "cmd")
        && argv
            .get(1)
            .is_some_and(|s| s.eq_ignore_ascii_case("/c") || s.eq_ignore_ascii_case("/k"));
    let mut result = Vec::new();
    for (i, arg) in argv.iter().enumerate() {
        if i > 0 {
            result.push(b' ' as u16);
        }
        if verbatim && i > 0 {
            result.extend(arg.encode_wide());
        } else {
            result.extend(quote(arg));
        }
    }
    if result.contains(&0) || result.len() >= 32767 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid Windows command line",
        ));
    }
    result.push(0);
    Ok(result)
}
fn environment(pane_id: u32, session: Option<&OsStr>) -> io::Result<Vec<u16>> {
    let reserved = [
        "TERM",
        "SLOSH",
        "SLOSH_BIN",
        "SLOSH_SESSION",
        "SLOSH_PANE",
        "ZELLIJ",
    ];
    let mut vars: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(k, _)| !reserved.iter().any(|name| k.eq_ignore_ascii_case(name)))
        .collect();
    vars.extend([
        ("TERM".into(), "xterm-ghostty".into()),
        ("SLOSH".into(), "1".into()),
        (
            "SLOSH_BIN".into(),
            std::env::current_exe()?.into_os_string(),
        ),
        (
            "SLOSH_SESSION".into(),
            session.unwrap_or_else(|| OsStr::new("")).to_owned(),
        ),
    ]);
    if pane_id != 0 {
        vars.push(("SLOSH_PANE".into(), pane_id.to_string().into()));
    }
    vars.sort_by_cached_key(|(key, _)| key.to_string_lossy().to_uppercase());
    let mut block = Vec::new();
    for (key, value) in vars {
        let mut entry = key;
        entry.push("=");
        entry.push(value);
        block.extend(wide(&entry)?);
    }
    block.push(0);
    Ok(block)
}

// CreateProcess otherwise copies redirected parent standard handles into a
// ConPTY child. Clear them for spawn, restoring even when process creation fails.
static SPAWN_LOCK: Mutex<()> = Mutex::new(());
struct StandardHandles([HANDLE; 3]);
impl StandardHandles {
    fn clear() -> Self {
        let mut saved = [std::ptr::null_mut(); 3];
        for (i, kind) in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
            .into_iter()
            .enumerate()
        {
            // SAFETY: these calls query/set process standard handles, under SPAWN_LOCK.
            unsafe {
                saved[i] = GetStdHandle(kind);
                SetStdHandle(kind, std::ptr::null_mut());
            }
        }
        Self(saved)
    }
}
impl Drop for StandardHandles {
    fn drop(&mut self) {
        for (i, kind) in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
            .into_iter()
            .enumerate()
        {
            // SAFETY: restore the borrowed handles, which were never closed.
            unsafe {
                SetStdHandle(kind, self.0[i]);
            }
        }
    }
}

pub struct Pty {
    master: TcpStream,
    process: Arc<Process>,
    console: Arc<Console>,
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
        let argv: Vec<_> = argv.into_iter().map(|s| s.as_ref().to_owned()).collect();
        if argv.is_empty() || argv[0].is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty pane command",
            ));
        }
        let mut cmd = command_line(&argv)?;
        let env = environment(pane_id, session)?;
        let cwd = cwd
            .filter(|p| p.is_dir())
            .map(|p| wide(p.as_os_str()))
            .transpose()?;
        let (input, mut to_child) = pipe()?;
        let (mut from_child, output) = pipe()?;
        let mut hpc = 0;
        // SAFETY: valid pipe handles and an output handle, sized before spawn.
        let hr = unsafe {
            CreatePseudoConsole(
                size.coord()?,
                input.as_raw_handle(),
                output.as_raw_handle(),
                0,
                &mut hpc,
            )
        };
        if hr < 0 {
            return Err(io::Error::from_raw_os_error(hr & 0xffff));
        }
        let console = Arc::new(Console(Mutex::new(Some(hpc))));
        drop(input);
        drop(output);
        let mut attributes = Attributes::new(hpc)?;
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        startup.lpAttributeList = attributes.ptr();
        let mut process = PROCESS_INFORMATION::default();
        {
            let _guard = SPAWN_LOCK.lock().unwrap();
            let _restore = StandardHandles::clear();
            // SAFETY: command is mutable/NUL-terminated; environment and cwd
            // live through this call; the extended startup attributes are valid.
            if unsafe {
                CreateProcessW(
                    std::ptr::null(),
                    cmd.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                    env.as_ptr().cast(),
                    cwd.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                    &startup.StartupInfo,
                    &mut process,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        // SAFETY: CreateProcess returned two independently owned handles.
        let process_handle = unsafe { OwnedHandle::from_raw_handle(process.hProcess) };
        drop(unsafe { OwnedHandle::from_raw_handle(process.hThread) });
        let process = Arc::new(Process {
            handle: process_handle,
            pid: process.dwProcessId,
        });
        // Prepare socket endpoints before pumps; the owning Pty guard ensures
        // every subsequent failure terminates the child and releases its console.
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0));
        let sockets = (|| {
            let listener = listener?;
            let master = TcpStream::connect(listener.local_addr()?)?;
            let (inner, _) = listener.accept()?;
            master.set_nonblocking(true)?;
            master.set_nodelay(true)?;
            inner.set_nodelay(true)?;
            Ok::<_, io::Error>((master, inner))
        })();
        let (master, mut inner) = match sockets {
            Ok(pair) => pair,
            Err(e) => {
                // SAFETY: the process handle is owned and still live.
                unsafe {
                    TerminateProcess(process.handle.as_raw_handle(), 1);
                }
                console.close();
                return Err(e);
            }
        };
        let pty = Self {
            master,
            process,
            console,
            output: VecDeque::new(),
            eof: false,
            exit_status: None,
        };
        let mut input_socket = inner.try_clone()?;
        std::thread::Builder::new()
            .name("conpty-output".into())
            .spawn(move || {
                let _ = io::copy(&mut from_child, &mut inner);
                let _ = inner.shutdown(Shutdown::Write);
            })?;
        std::thread::Builder::new()
            .name("conpty-input".into())
            .spawn(move || {
                let _ = io::copy(&mut input_socket, &mut to_child);
            })?;
        let (process, console) = (pty.process.clone(), pty.console.clone());
        std::thread::Builder::new()
            .name("conpty-exit".into())
            .spawn(move || {
                // SAFETY: the Arc retains ownership throughout the wait.
                unsafe {
                    WaitForSingleObject(process.handle.as_raw_handle(), INFINITE);
                }
                console.close();
            })?;
        Ok(pty)
    }
    pub fn fd(&self) -> usize {
        self.master.as_raw_socket() as usize
    }
    pub fn pid(&self) -> u32 {
        self.process.pid
    }
    pub fn resize(&self, size: Size) -> io::Result<()> {
        let coord = size.coord()?;
        let console = self.console.0.lock().unwrap();
        let Some(handle) = *console else {
            return Ok(());
        };
        // SAFETY: holding the lock keeps the console from being closed.
        let hr = unsafe { ResizePseudoConsole(handle, coord) };
        if hr < 0 {
            Err(io::Error::from_raw_os_error(hr & 0xffff))
        } else {
            Ok(())
        }
    }
    pub fn read_available(&mut self) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        let mut buf = [0; 65536];
        loop {
            match self.master.read(&mut buf) {
                Ok(0) => {
                    self.eof = true;
                    break;
                }
                Ok(n) => bytes.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        self.exit_status = self.process.status()?;
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
            match self.master.write(front) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "ConPTY write returned zero",
                    ));
                }
                Ok(n) => {
                    self.output.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    pub fn poll(&self, timeout: Duration) -> io::Result<bool> {
        let mut fd = crate::poller::PollFd {
            fd: self.fd(),
            events: crate::poller::READ
                | if self.wants_write() {
                    crate::poller::WRITE
                } else {
                    0
                },
            revents: 0,
        };
        crate::poller::poll(
            std::slice::from_mut(&mut fd),
            timeout.as_millis().min(i32::MAX as u128) as i32,
        )
        .map(|n| n > 0)
    }
}
impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.master.shutdown(Shutdown::Both);
        if !matches!(self.process.status(), Ok(Some(_))) {
            // SAFETY: the process handle identifies our child, even if its PID
            // has since been reused; termination releases blocked pipe pumps.
            unsafe {
                TerminateProcess(self.process.handle.as_raw_handle(), 1);
            }
        }
        self.console.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoting_preserves_shell_syntax_and_crt_backslashes() {
        let decode = |s: Vec<u16>| String::from_utf16(&s[..s.len() - 1]).unwrap();
        assert_eq!(
            decode(
                command_line(&["app.exe".into(), "".into(), "a b\\".into(), "a\"b".into()])
                    .unwrap()
            ),
            "\"app.exe\" \"\" \"a b\\\\\" \"a\\\"b\""
        );
        assert_eq!(
            decode(
                command_line(&[
                    "C:\\Windows\\cmd.exe".into(),
                    "/c".into(),
                    "echo \"hello world\" > out".into()
                ])
                .unwrap()
            ),
            "\"C:\\Windows\\cmd.exe\" /c echo \"hello world\" > out"
        );
        assert!(command_line(&[OsString::from("bad\0arg")]).is_err());
    }
    #[test]
    fn conpty_drains_output_and_reports_exit() {
        let mut pty = Pty::spawn(
            [
                "cmd.exe",
                "/c",
                "echo native-conpty & echo %SLOSH%:%SLOSH_PANE%:%SLOSH_SESSION% & exit /b 7",
            ],
            Size {
                cols: 40,
                rows: 12,
                cell_width: 8,
                cell_height: 16,
            },
            None,
            42,
            Some(OsStr::new("windows-test")),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut terminal = crate::ghostty::Terminal::new(40, 12).unwrap();
        while !pty.eof && std::time::Instant::now() < deadline {
            pty.poll(Duration::from_millis(20)).unwrap();
            terminal.write(&pty.read_available().unwrap());
        }
        assert!(pty.eof, "ConPTY did not close after process exit");
        let text = terminal.plain_text().unwrap();
        assert!(text.contains("native-conpty"), "{text}");
        assert!(text.contains("1:42:windows-test"), "{text}");
        assert_eq!(pty.exit_status.unwrap().code(), Some(7));
    }

    #[test]
    fn conpty_accepts_input_after_resize_and_drop_terminates_its_child() {
        let size = Size {
            cols: 60,
            rows: 12,
            cell_width: 8,
            cell_height: 16,
        };
        let mut pty = Pty::spawn(["cmd.exe", "/q"], size, None, 3, None).unwrap();
        pty.resize(Size {
            cols: 70,
            rows: 14,
            ..size
        })
        .unwrap();
        pty.send(b"echo bidirectional-conpty\r\n").unwrap();
        let mut terminal = crate::ghostty::Terminal::new(70, 14).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            pty.poll(Duration::from_millis(20)).unwrap();
            pty.flush_pending().unwrap();
            terminal.write(&pty.read_available().unwrap());
            // A standalone output line distinguishes the reply from command echo.
            if terminal
                .plain_text()
                .unwrap()
                .lines()
                .any(|s| s.trim() == "bidirectional-conpty")
            {
                break;
            }
        }
        assert!(
            terminal
                .plain_text()
                .unwrap()
                .lines()
                .any(|s| s.trim() == "bidirectional-conpty")
        );
        let process = pty.process.clone();
        assert!(process.status().unwrap().is_none());
        drop(pty);
        // SAFETY: this Arc keeps the original process handle alive after Pty drop.
        assert_eq!(
            unsafe { WaitForSingleObject(process.handle.as_raw_handle(), 1000) },
            WAIT_OBJECT_0
        );
        assert!(process.status().unwrap().is_some());
    }
}
