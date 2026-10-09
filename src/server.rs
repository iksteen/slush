//! Persistent single-threaded session with independently projected displays.
use crate::{
    app::App,
    input::{Event, Parser},
    local_socket::{AsDescriptor, LocalListener, LocalStream},
    poller::{self, Descriptor},
    proto,
    screen::Screen,
};
mod watch;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::{self, Read, Write},
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const MAX_CONNS: usize = 16;
const MAX_OUTBOX: usize = 4 * 1024 * 1024;
const FRAME_MS: u64 = 8;
const ESC_MS: u64 = 40;
static STOP: AtomicBool = AtomicBool::new(false);
#[cfg(unix)]
extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

struct SocketGuard {
    path: PathBuf,
    identity: (u64, u64),
}
fn socket_identity(path: &std::path::Path) -> io::Result<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::symlink_metadata(path)?;
        Ok((metadata.dev(), metadata.ino()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::*;
        // Open the reparse point itself, without requiring read access to a
        // socket. File IDs prevent cleanup from deleting a replacement socket.
        let file = fs::OpenOptions::new()
            .access_mode(0)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: a live file handle and correctly sized output structure.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((
            info.dwVolumeSerialNumber as u64,
            ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
        ))
    }
}
impl Drop for SocketGuard {
    fn drop(&mut self) {
        if socket_identity(&self.path).is_ok_and(|id| id == self.identity) {
            let _ = fs::remove_file(&self.path);
        }
    }
}
struct Connection {
    stream: LocalStream,
    reader: proto::Reader,
    display: bool,
    cols: u16,
    rows: u16,
    cell_w: u16,
    cell_h: u16,
    view_x: u16,
    view_y: u16,
    shown_cols: u16,
    shown_rows: u16,
    activity: u64,
    input: Parser,
    esc_due: Option<Instant>,
    view: Screen,
    graphics: crate::graphics::Graphics,
    out: Vec<u8>,
    offset: usize,
}
impl Connection {
    fn new(stream: LocalStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            reader: proto::Reader::default(),
            display: false,
            cols: 0,
            rows: 0,
            cell_w: 0,
            cell_h: 0,
            view_x: 0,
            view_y: 0,
            shown_cols: 0,
            shown_rows: 0,
            activity: 0,
            input: Parser::default(),
            esc_due: None,
            view: Screen::new(1, 1),
            graphics: crate::graphics::Graphics::default(),
            out: Vec::new(),
            offset: 0,
        })
    }
    fn flush(&mut self) -> io::Result<()> {
        while self.offset < self.out.len() {
            match self.stream.write(&self.out[self.offset..]) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "client closed")),
                Ok(n) => self.offset += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(e) => return Err(e),
            }
        }
        self.out.clear();
        self.offset = 0;
        Ok(())
    }
    fn send(&mut self, kind: u8, payload: &[u8]) -> io::Result<bool> {
        if self.out.len() - self.offset > MAX_OUTBOX {
            return Ok(false);
        }
        if self.offset > 0 {
            self.out.drain(..self.offset);
            self.offset = 0;
        }
        proto::send(&mut self.out, kind, payload)?;
        self.flush()?;
        Ok(true)
    }
    fn viewport(&mut self, screen: &Screen) {
        (self.view_x, self.view_y) =
            screen.follow_cursor(self.cols, self.rows, self.view_x, self.view_y);
        self.shown_cols = screen.cols.saturating_sub(self.view_x).min(self.cols);
        self.shown_rows = screen.rows.saturating_sub(self.view_y).min(self.rows);
    }
}
struct Server {
    app: App,
    screen: Screen,
    command_input: Parser,
    conns: BTreeMap<Descriptor, Connection>,
    active: Option<Descriptor>,
    activity: u64,
    fallback_pixels: (u16, u16),
    paint: Option<Instant>,
}
impl Server {
    fn request_paint(&mut self, immediate: bool) {
        if immediate || self.paint.is_none() {
            self.paint = Some(Instant::now());
        }
    }
    fn canvas(&mut self) {
        let active = self.active.and_then(|fd| self.conns.get(&fd));
        let policy = self.app.config.text("size_follows").unwrap_or("active");
        let size = if policy == "active" {
            active.map(|c| (c.cols, c.rows))
        } else {
            self.conns
                .values()
                .filter(|c| c.display)
                .map(|c| (c.cols, c.rows))
                .reduce(|(w, h), (cw, ch)| {
                    if policy == "smallest" {
                        (w.min(cw), h.min(ch))
                    } else {
                        (w.max(cw), h.max(ch))
                    }
                })
        };
        if let Some((cols, rows)) = size.filter(|(w, h)| *w > 0 && *h > 0) {
            let (fw, fh) = self.fallback_pixels;
            let pixels = active.map_or((fw, fh), |c| {
                (
                    if c.cell_w > 0 { c.cell_w } else { fw },
                    if c.cell_h > 0 { c.cell_h } else { fh },
                )
            });
            if (cols, rows) != (self.screen.cols, self.screen.rows) {
                self.screen.resize(cols, rows);
                self.app.resize(cols, rows);
            }
            self.app.cell_size(pixels.0, pixels.1);
            self.app.compose(&mut self.screen);
        }
        self.request_paint(true);
    }
    fn activate(&mut self, fd: Descriptor) {
        if self.active != Some(fd) {
            self.app.cancel_client_interaction();
        }
        self.active = Some(fd);
        self.activity += 1;
        if let Some(c) = self.conns.get_mut(&fd) {
            c.activity = self.activity;
        }
    }
    fn close(&mut self, fd: Descriptor, reason: Option<u8>) {
        if let Some(mut c) = self.conns.remove(&fd)
            && let Some(reason) = reason
        {
            let _ = c.send(proto::EXIT, &[reason]);
        }
        if self.active == Some(fd) {
            self.active = self
                .conns
                .iter()
                .filter(|(_, c)| c.display)
                .max_by_key(|(_, c)| c.activity)
                .map(|(fd, _)| *fd);
            self.app.cancel_client_interaction();
        }
        self.canvas();
    }
    fn size_message(&mut self, fd: Descriptor, kind: u8, payload: &[u8]) {
        let fresh = self
            .conns
            .get(&fd)
            .is_some_and(|c| kind == proto::HELLO && !c.display);
        if !fresh && !self.conns.get(&fd).is_some_and(|c| c.display) {
            return;
        }
        if fresh && !self.app.config.boolean("multi_attach") {
            let old: Vec<_> = self
                .conns
                .iter()
                .filter(|(other, c)| **other != fd && c.display)
                .map(|(fd, _)| *fd)
                .collect();
            for old in old {
                self.close(old, Some(2));
            }
        }
        let Some(c) = self.conns.get_mut(&fd) else {
            return;
        };
        let mut cols = if c.cols > 0 { c.cols } else { self.screen.cols };
        let mut rows = if c.rows > 0 { c.rows } else { self.screen.rows };
        if payload.len() >= 4 {
            let w = u16::from_be_bytes(payload[..2].try_into().unwrap());
            let h = u16::from_be_bytes(payload[2..4].try_into().unwrap());
            if w > 0 && h > 0 {
                cols = w;
                rows = h;
            }
        }
        if payload.len() >= 8 {
            c.cell_w = u16::from_be_bytes(payload[4..6].try_into().unwrap());
            c.cell_h = u16::from_be_bytes(payload[6..8].try_into().unwrap());
        }
        c.cols = cols;
        c.rows = rows;
        c.view.resize(cols, rows);
        if fresh {
            c.display = true;
            self.activate(fd);
            self.app.splash(-1, -1);
        }
        self.canvas();
        if let Some(c) = self.conns.get_mut(&fd) {
            c.viewport(&self.screen);
        }
    }
    fn input(&mut self, fd: Descriptor, bytes: &[u8], timeout: bool) {
        let Some(c) = self.conns.get_mut(&fd).filter(|c| c.display) else {
            return;
        };
        let events = if timeout {
            c.input.timeout()
        } else {
            c.input.feed(bytes)
        };
        c.esc_due = c
            .input
            .pending()
            .then(|| Instant::now() + Duration::from_millis(ESC_MS));
        let mut activity = false;
        for event in events {
            if !activity
                && matches!(
                    event,
                    Event::Key { .. } | Event::Mouse { .. } | Event::Paste(_)
                )
            {
                self.activate(fd);
                activity = true;
            }
            if let Event::Mouse {
                button,
                action,
                mods,
                x,
                y,
            } = event
            {
                let Some(c) = self.conns.get(&fd) else {
                    break;
                };
                if x >= c.shown_cols || y >= c.shown_rows {
                    if action == 1 {
                        self.app.cancel_client_pointer();
                    }
                    continue;
                }
                self.app.event(
                    &Event::Mouse {
                        button,
                        action,
                        mods,
                        x: x + c.view_x,
                        y: y + c.view_y,
                    },
                    &self.screen,
                );
            } else {
                self.app.event(&event, &self.screen);
            }
        }
        if activity {
            self.canvas();
        }
        self.push_clipboard(Some(fd));
        if self.app.detach {
            self.app.detach = false;
            self.close(fd, Some(1));
        }
        self.request_paint(true);
    }
    fn message(&mut self, fd: Descriptor, message: proto::Message) {
        match message.kind {
            proto::HELLO | proto::RESIZE => self.size_message(fd, message.kind, &message.payload),
            proto::INPUT => self.input(fd, &message.payload, false),
            proto::DETACH => self.close(fd, Some(1)),
            proto::CMD => {
                let reply = self
                    .app
                    .command(&mut self.screen, &mut self.command_input, &message.payload)
                    .unwrap_or_else(|| b"{\"error\":\"unknown command\"}".to_vec());
                let sent = self
                    .conns
                    .get_mut(&fd)
                    .map(|c| c.send(proto::REPLY, &reply));
                if sent.is_some_and(|r| r.is_err()) {
                    self.close(fd, None);
                }
                self.screen.force_full = true;
                if self.app.detach {
                    self.app.detach = false;
                    if let Some(active) = self.active {
                        self.close(active, Some(1));
                    }
                }
                self.request_paint(true);
            }
            _ => {}
        }
    }
    fn push(&mut self) {
        self.push_clipboard(self.active);
        self.app.compose(&mut self.screen);
        let reset = std::mem::take(&mut self.screen.force_full);
        let displays = self.conns.values().filter(|c| c.display).count();
        let mut gone = Vec::new();
        let mut retry = false;
        for (fd, c) in self.conns.iter_mut().filter(|(_, c)| c.display) {
            if reset {
                c.view.force_full = true;
            }
            let previous = (c.view_x, c.view_y, c.shown_cols, c.shown_rows);
            c.viewport(&self.screen);
            c.view.project(&self.screen, c.view_x, c.view_y);
            if displays > 1 && self.app.config.boolean("attach_indicator") {
                let tag = format!(" {displays} clients ");
                if tag.len() < c.view.cols as usize {
                    c.view.text(
                        c.view.cols - tag.len() as u16,
                        0,
                        &tag,
                        self.app.config.color("attach_fg"),
                        self.app.config.color("attach_bg"),
                        0,
                    );
                }
            }
            if c.view.force_full {
                c.graphics.repaint();
            }
            let bytes = c.view.render();
            let graphics = self.app.graphics_view(
                &mut c.graphics,
                crate::shader::Rect {
                    x: c.view_x,
                    y: c.view_y,
                    w: c.cols,
                    h: c.rows,
                },
            );
            if bytes.is_empty() && graphics.is_empty() {
                continue;
            }
            let mut frame = b"\x1b[?2026h".to_vec();
            frame.extend_from_slice(bytes);
            frame.extend_from_slice(graphics);
            frame.extend_from_slice(b"\x1b[?2026l");
            match c.send(proto::OUTPUT, &frame) {
                Ok(true) => c.graphics.commit(true),
                Ok(false) => {
                    (c.view_x, c.view_y, c.shown_cols, c.shown_rows) = previous;
                    c.view.force_full = true;
                    c.graphics.commit(false);
                    retry = true;
                }
                Err(_) => {
                    c.graphics.commit(false);
                    gone.push(*fd);
                }
            }
        }
        self.paint = retry.then(|| Instant::now() + Duration::from_millis(FRAME_MS));
        for fd in gone {
            self.close(fd, None);
        }
    }
    fn push_clipboard(&mut self, fd: Option<Descriptor>) {
        let Some(text) = self.app.clipboard_pending.take() else {
            return;
        };
        let Some(c) = fd
            .and_then(|fd| self.conns.get_mut(&fd))
            .filter(|c| c.display)
        else {
            return;
        };
        let message = format!(
            "\x1b]52;c;{}\x1b\\",
            crate::osc::encode_base64(text.as_bytes())
        );
        let _ = c.send(proto::OUTPUT, message.as_bytes());
    }
}

pub fn run(
    name: &std::ffi::OsStr,
    argv: Vec<OsString>,
    cols: u16,
    rows: u16,
    layout: Option<&std::path::Path>,
    watch: bool,
) -> io::Result<()> {
    let path = proto::socket_path(name)?;
    if fs::symlink_metadata(&path).is_ok() {
        if LocalStream::connect(&path).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "session is already running",
            ));
        }
        fs::remove_file(&path)?;
    }
    let listener = LocalListener::bind(&path)?;
    let _socket = SocketGuard {
        identity: socket_identity(&path)?,
        path: path.clone(),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    listener.set_nonblocking(true)?;
    STOP.store(false, Ordering::Relaxed);
    // SAFETY: handlers only set a lock-free atomic flag and have static lifetime.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGTERM, stop as *const () as usize);
        libc::signal(libc::SIGINT, stop as *const () as usize);
    }
    let app = App::with_session(argv, cols, rows, Some(name.into()))?;
    let fallback_pixels = (app.cell_w, app.cell_h);
    let mut server = Server {
        app,
        screen: Screen::new(cols, rows),
        command_input: Parser::default(),
        conns: BTreeMap::new(),
        active: None,
        activity: 0,
        fallback_pixels,
        paint: Some(Instant::now()),
    };
    if let Some(layout) = layout
        && let Err(e) = server.app.apply_layout_file(layout, true, true)
    {
        eprintln!("slush: {}: {e}", layout.display());
    }
    server.app.compose(&mut server.screen);
    let mut watcher = watch.then(watch::Watcher::new).transpose()?.flatten();
    if let Some(w) = &mut watcher {
        w.update(&server.app.config.files);
    }
    let mut reload_due: Option<Instant> = None;
    while !STOP.load(Ordering::Relaxed) && !server.app.should_quit() {
        let mut fds = vec![poller::PollFd {
            fd: listener.descriptor(),
            events: poller::READ,
            revents: 0,
        }];
        let client_fds: Vec<_> = server.conns.keys().copied().collect();
        for fd in &client_fds {
            let c = &server.conns[fd];
            fds.push(poller::PollFd {
                fd: *fd,
                events: poller::READ
                    | if c.offset < c.out.len() {
                        poller::WRITE
                    } else {
                        0
                    },
                revents: 0,
            });
        }
        let pane_slot = fds.len();
        for pane in server.app.panes.values().filter_map(|p| p.pty.as_ref()) {
            fds.push(poller::PollFd {
                fd: pane.fd(),
                events: poller::READ | if pane.wants_write() { poller::WRITE } else { 0 },
                revents: 0,
            });
        }
        let watch_slot = fds.len();
        if let Some(w) = &watcher {
            fds.push(poller::PollFd {
                fd: w.fd(),
                events: poller::READ,
                revents: 0,
            });
        }
        let now = Instant::now();
        let mut due = server.paint;
        if let Some(reload) = reload_due {
            due = Some(due.map_or(reload, |d| d.min(reload)));
        }
        for c in server.conns.values() {
            if let Some(esc) = c.esc_due {
                due = Some(due.map_or(esc, |d| d.min(esc)));
            }
        }
        let self_due = server.app.next_deadline_ms();
        if self_due >= 0 {
            let deadline = now + Duration::from_millis(self_due as u64);
            due = Some(due.map_or(deadline, |d| d.min(deadline)));
        }
        let timeout = due.map_or(-1, |d| {
            d.saturating_duration_since(now)
                .as_millis()
                .min(i32::MAX as u128) as i32
        });
        let result = match poller::poll(&mut fds, timeout) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => 0,
            Err(e) => return Err(e),
        };
        if fds[0].revents & poller::READ != 0 {
            loop {
                match listener.accept() {
                    Ok((stream, _)) if server.conns.len() < MAX_CONNS => {
                        let fd = stream.descriptor();
                        server.conns.insert(fd, Connection::new(stream)?);
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
        }
        for (i, fd) in client_fds.iter().enumerate() {
            let ready = fds[1 + i].revents;
            if ready == 0 || !server.conns.contains_key(fd) {
                continue;
            }
            if ready & poller::WRITE != 0 && server.conns.get_mut(fd).unwrap().flush().is_err() {
                server.close(*fd, None);
                continue;
            }
            if ready & (poller::READ | poller::HUP | poller::ERROR) != 0 {
                let mut bytes = [0; 65536];
                match server.conns.get_mut(fd).unwrap().stream.read(&mut bytes) {
                    Ok(0) => server.close(*fd, None),
                    Ok(n) => {
                        let c = server.conns.get_mut(fd).unwrap();
                        c.reader.feed(&bytes[..n]);
                        let messages: Vec<_> = c.reader.by_ref().collect();
                        for message in messages {
                            if server.conns.contains_key(fd) {
                                server.message(*fd, message);
                            }
                        }
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                        ) => {}
                    Err(_) => server.close(*fd, None),
                }
            }
        }
        let now = Instant::now();
        let pending: Vec<_> = server
            .conns
            .iter()
            .filter(|(_, c)| c.esc_due.is_some_and(|at| at <= now))
            .map(|(fd, _)| *fd)
            .collect();
        for fd in pending {
            server.input(fd, &[], true);
        }
        for pane in &fds[pane_slot..watch_slot] {
            if pane.revents != 0 {
                let _ = server.app.pump_fd(pane.fd);
                if server.paint.is_none() {
                    server.paint = Some(Instant::now() + Duration::from_millis(FRAME_MS));
                }
            }
        }
        if let Some(w) = &mut watcher
            && fds[watch_slot].revents & poller::READ != 0
            && w.drain()?
        {
            reload_due = Some(Instant::now() + Duration::from_millis(80));
        }
        if reload_due.is_some_and(|due| due <= Instant::now()) {
            reload_due = None;
            match server.app.reload_config(&mut server.screen) {
                Ok(()) => {
                    if let Some(w) = &mut watcher {
                        w.update(&server.app.config.files);
                    }
                    let text = server
                        .app
                        .config
                        .messages
                        .first()
                        .map_or("config reloaded", |diagnostic| &**diagnostic)
                        .to_owned();
                    server.app.toast(&text);
                    if !server.app.config.boolean("multi_attach") {
                        let old: Vec<_> = server
                            .conns
                            .iter()
                            .filter(|(fd, c)| c.display && Some(**fd) != server.active)
                            .map(|(fd, _)| *fd)
                            .collect();
                        for fd in old {
                            server.close(fd, Some(2));
                        }
                    }
                    server.canvas();
                }
                Err(e) => server.app.toast(&e),
            }
            server.screen.force_full = true;
            server.request_paint(true);
        }
        server.app.reap();
        if self_due >= 0 && result == 0 {
            server.request_paint(true);
        }
        if server.paint.is_some_and(|at| at <= Instant::now()) {
            server.push();
        }
    }
    let clients: Vec<_> = server.conns.keys().copied().collect();
    for fd in clients {
        server.close(fd, Some(0));
    }
    Ok(())
}
