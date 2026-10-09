//! One terminal and its child process, retaining both across layout edits.
use crate::{
    config::Config,
    ghostty::Terminal,
    input::Event,
    osc,
    pty::{Pty, Size},
    screen::Screen,
};
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::{
    borrow::Cow,
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    process::ExitStatus,
    time::{Duration, Instant},
};

pub struct Pane {
    pub id: u32,
    pub terminal: Terminal,
    pub pty: Option<Pty>,
    pub argv: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub size: Size,
    pub alive: bool,
    pub suspended: bool,
    pub label: Vec<u8>,
    pub name: Vec<u8>,
    pub title: Vec<u8>,
    pub status: Vec<u8>,
    pub busy: bool,
    pub buttons: Vec<osc::Button>,
    pub program: osc::Records,
    scanner: osc::Scanner,
    status_query: Option<Instant>,
    pub osc_events: Vec<(Vec<u8>, Vec<u8>)>,
    pub bell: bool,
    pub ephemeral: bool,
    pub exit: Option<ExitStatus>,
    pub line_len: u16,
}
impl Pane {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: u32,
        argv: Vec<OsString>,
        size: Size,
        cwd: Option<&Path>,
        suspended: bool,
        label: impl AsRef<[u8]>,
        session: Option<&OsStr>,
        config: &Config,
    ) -> io::Result<Self> {
        let mut terminal =
            Terminal::new(size.cols.max(1), size.rows.max(1)).map_err(io::Error::other)?;
        for mode in [25, 2027] {
            terminal
                .set_mode_default(mode, true)
                .map_err(io::Error::other)?;
        }
        terminal
            .set_scrollback(
                config.number("scrollback") as usize,
                config.number("scrollback_bytes") as usize,
            )
            .map_err(io::Error::other)?;
        terminal
            .resize(
                size.cols.max(1),
                size.rows.max(1),
                size.cell_width as u32,
                size.cell_height as u32,
            )
            .map_err(io::Error::other)?;
        let cwd = cwd
            .map(|path| {
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    std::env::current_dir().map_or_else(
                        |_| path.to_path_buf(),
                        |base| crate::path::resolve_os(path, Some(&base)),
                    )
                }
            })
            .or_else(|| std::env::current_dir().ok());
        let pty = if suspended {
            None
        } else {
            Some(Pty::spawn(&argv, size, cwd.as_deref(), id, session)?)
        };
        Ok(Self {
            id,
            terminal,
            pty,
            argv,
            cwd,
            size,
            alive: true,
            suspended,
            label: bounded_string(label.as_ref(), 1023),
            name: Vec::new(),
            title: Vec::new(),
            status: Vec::new(),
            busy: false,
            buttons: Vec::new(),
            program: osc::Records::default(),
            scanner: osc::Scanner::default(),
            status_query: None,
            osc_events: Vec::new(),
            bell: false,
            ephemeral: false,
            exit: None,
            line_len: 0,
        })
    }
    pub fn resize(&mut self, size: Size) -> io::Result<()> {
        if (
            self.size.cols,
            self.size.rows,
            self.size.cell_width,
            self.size.cell_height,
        ) == (size.cols, size.rows, size.cell_width, size.cell_height)
        {
            return Ok(());
        }
        self.terminal
            .resize(
                size.cols.max(1),
                size.rows.max(1),
                size.cell_width as u32,
                size.cell_height as u32,
            )
            .map_err(io::Error::other)?;
        if let Some(pty) = &self.pty {
            pty.resize(size)?;
        }
        self.size = size;
        Ok(())
    }
    pub fn pump(&mut self) -> io::Result<usize> {
        let Some(pty) = &mut self.pty else {
            return Ok(0);
        };
        pty.flush_pending()?;
        let data = pty.read_available()?;
        let n = data.len();
        let eof = pty.eof;
        let exit = pty.exit_status;
        for event in self.scanner.feed(&data) {
            self.osc(event)?;
        }
        self.terminal.write(&data);
        self.title = bounded_string(&self.terminal.title_bytes().unwrap_or_default(), 255);
        self.bell |= self.terminal.take_bell();
        let replies = self.terminal.take_replies();
        if !replies.is_empty() && !eof {
            self.write(&replies)?;
        }
        if eof {
            self.alive = false;
            self.exit = exit;
            let showing = self.program.showing.is_some();
            self.program.drop_transient();
            self.program_line(showing);
            self.busy = false;
            self.pty = None;
        }
        Ok(n)
    }
    fn program_line(&mut self, was_showing: bool) {
        if let Some(show) = &self.program.showing {
            self.status = show.line_bytes();
            self.busy = show.state.busy();
        } else if was_showing {
            self.status.clear();
            self.busy = false;
        }
    }
    fn osc(&mut self, event: osc::Event) -> io::Result<()> {
        let showing = self.program.showing.is_some();
        match event {
            osc::Event::Status(report) => {
                self.program.apply(report);
                self.program_line(showing);
            }
            osc::Event::Prompt => {
                self.program.drop_transient();
                self.program_line(showing);
            }
            osc::Event::Reset => {
                self.program.clear();
                self.program_line(showing);
            }
            osc::Event::Query { bel } => {
                if self
                    .status_query
                    .is_none_or(|at| at.elapsed() >= Duration::from_millis(250))
                {
                    self.status_query = Some(Instant::now());
                    self.write(if bel {
                        b"\x1b]7501;?\x07"
                    } else {
                        b"\x1b]7501;?\x1b\\"
                    })?;
                }
            }
            osc::Event::Verb(verb, payload) => {
                if matches!(verb.as_slice(), b"status" | b"busy" | b"clear") {
                    self.program.clear();
                    if showing {
                        self.busy = false;
                    }
                }
                match verb.as_slice() {
                    b"status" => self.status = bounded_string(&payload, 255),
                    b"busy" => {
                        self.busy = !matches!(payload.as_slice(), b"0" | b"false" | b"no" | b"off")
                    }
                    b"buttons" => self.buttons = osc::buttons(&payload),
                    b"clear" => {
                        self.status.clear();
                        self.busy = false;
                        self.buttons.clear();
                    }
                    b"hello" => self.write(b"\x1b]5577;1;hello-reply;slosh;1\x1b\\")?,
                    b"purpose" | b"shader" | b"shader-load" => {
                        self.osc_events.push((verb, payload))
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
    pub fn write(&mut self, data: &[u8]) -> io::Result<()> {
        if let Some(pty) = &mut self.pty {
            pty.send(data)?;
        }
        Ok(())
    }
    pub fn input(&mut self, event: &Event) -> io::Result<()> {
        if let Event::Key {
            key, action, text, ..
        } = event
        {
            if *action != 2 {
                use crate::ghostty::ffi::*;
                if *key == GHOSTTY_KEY_ENTER {
                    self.line_len = 0;
                } else if *key == GHOSTTY_KEY_BACKSPACE {
                    self.line_len = self.line_len.saturating_sub(1);
                } else if text.first().is_some_and(|c| *c >= 0x20) {
                    self.line_len = self.line_len.saturating_add(1);
                }
            }
            if self.terminal.scrolled().unwrap_or(false) {
                self.terminal.scroll_edge(false);
            }
        }
        let data = self
            .terminal
            .encode_input(
                event,
                self.size.cols,
                self.size.rows,
                self.size.cell_width,
                self.size.cell_height,
            )
            .map_err(io::Error::other)?;
        self.write(&data)
    }
    pub fn start(&mut self, session: Option<&OsStr>) -> io::Result<bool> {
        if self.alive && !self.suspended {
            return Ok(false);
        }
        self.pty = Some(Pty::spawn(
            &self.argv,
            self.size,
            self.cwd.as_deref(),
            self.id,
            session,
        )?);
        self.alive = true;
        self.suspended = false;
        self.exit = None;
        self.status.clear();
        self.buttons.clear();
        self.program.clear();
        self.scanner = osc::Scanner::default();
        self.status_query = None;
        self.osc_events.clear();
        self.busy = false;
        self.bell = false;
        Ok(true)
    }
    pub fn title_bytes(&self) -> &[u8] {
        if self.name.is_empty() {
            &self.title
        } else {
            &self.name
        }
    }
    pub fn title(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(self.title_bytes())
    }
    pub fn set_name(&mut self, name: impl AsRef<[u8]>) {
        self.name = bounded_string(name.as_ref(), 255);
    }
    pub fn pid(&self) -> i64 {
        self.pty.as_ref().map_or(-1, |p| p.pid() as i64)
    }
    pub fn exit_words(&self) -> String {
        if self.exit_signal() != 0 {
            format!("exited: signal {}", self.exit_signal())
        } else if self.exit_code() > 0 {
            format!("exited: status {}", self.exit_code())
        } else {
            "exited".into()
        }
    }
    pub fn note(&mut self, text: impl AsRef<[u8]>, fg: crate::screen::Color) {
        let color = if fg.set {
            format!("\x1b[38;2;{};{};{}m", fg.r, fg.g, fg.b)
        } else {
            "\x1b[2m".into()
        };
        let mut bytes = format!("\r\n{color}").into_bytes();
        bytes.extend_from_slice(text.as_ref());
        bytes.extend_from_slice(b"\x1b[0m\r\n");
        self.terminal.write(&bytes);
    }
    pub fn exit_code(&self) -> i32 {
        self.exit.and_then(|s| s.code()).unwrap_or(-1)
    }
    pub fn exit_signal(&self) -> i32 {
        #[cfg(unix)]
        {
            self.exit.and_then(|s| s.signal()).unwrap_or(0)
        }
        #[cfg(windows)]
        {
            0
        }
    }
    #[cfg(unix)]
    pub fn foreground_command(&self) -> Option<Vec<u8>> {
        let pty = self.pty.as_ref()?;
        // SAFETY: tcgetpgrp only queries the live master descriptor.
        let fg = unsafe { libc::tcgetpgrp(pty.fd()) };
        if fg <= 0 || fg == pty.pid() as i32 {
            return None;
        }
        crate::process_unix::command(fg)
    }
    #[cfg(windows)]
    pub fn foreground_command(&self) -> Option<Vec<u8>> {
        crate::process_tree::foreground(self.pty.as_ref()?.pid()).map(String::into_bytes)
    }
    pub fn live_cwd(&self) -> Option<PathBuf> {
        #[cfg(unix)]
        if let Some(path) = crate::process_unix::cwd(self.pid() as libc::pid_t) {
            return Some(path);
        }
        self.cwd.clone()
    }
    pub fn compose(
        &mut self,
        screen: &mut Screen,
        x: u16,
        y: u16,
        focused: bool,
    ) -> io::Result<()> {
        self.terminal
            .compose(screen, x, y, focused)
            .map_err(io::Error::other)
    }
    pub fn text(&mut self) -> io::Result<String> {
        let mut screen = Screen::new(self.size.cols, self.size.rows);
        self.compose(&mut screen, 0, 0, false)?;
        Ok(screen.dump())
    }
}

fn bounded_string(bytes: &[u8], limit: usize) -> Vec<u8> {
    let bytes = bytes.split(|b| *b == 0).next().unwrap_or_default();
    bytes[..bytes.len().min(limit)].to_vec()
}
