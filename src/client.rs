//! A terminal and a local socket: the client never owns the application model.
use crate::{
    local_socket::{AsDescriptor, LocalStream},
    poller, proto,
    pty::Size,
};
use std::io::{self, Read, Write};
#[cfg(unix)]
#[path = "client/unix.rs"]
mod native;
#[cfg(windows)]
#[path = "client/windows.rs"]
mod native;
pub use native::{is_terminal, terminal_size};
const ENTER: &[u8] =
    b"\x1b[?1049h\x1b[0m\x1b[2J\x1b[?1002h\x1b[?1003h\x1b[?1006h\x1b[?1004h\x1b[?2004h\x1b[>1u";
const LEAVE: &[u8] =
    b"\x1b[<u\x1b[?2004l\x1b[?1004l\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?25h\x1b[0m\x1b[?1049l";
pub fn size_bytes(size: Size) -> [u8; 8] {
    let mut out = [0; 8];
    for (i, n) in [size.cols, size.rows, size.cell_width, size.cell_height]
        .into_iter()
        .enumerate()
    {
        out[i * 2..i * 2 + 2].copy_from_slice(&n.to_be_bytes());
    }
    out
}
pub fn run(mut stream: LocalStream) -> io::Result<()> {
    let mut guard = native::TerminalGuard::enter()?;
    proto::send(&mut stream, proto::HELLO, &size_bytes(terminal_size()))?;
    let mut reader = proto::Reader::default();
    let mut reason = None;
    let mut bytes = [0; 65536];
    let mut stdout = io::stdout().lock();
    while !native::quit() {
        let mut fds = [
            poller::PollFd {
                fd: guard.descriptor(),
                events: poller::READ,
                revents: 0,
            },
            poller::PollFd {
                fd: stream.descriptor(),
                events: poller::READ,
                revents: 0,
            },
        ];
        let ready = poller::poll(&mut fds, native::POLL_TIMEOUT);
        if native::take_resize(&mut guard) {
            proto::send(&mut stream, proto::RESIZE, &size_bytes(terminal_size()))?;
        }
        if let Err(error) = ready {
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if fds[0].revents & (poller::READ | poller::HUP) != 0 {
            let n = match guard.read(&mut bytes) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                r => r?,
            };
            if n == 0 {
                break;
            }
            proto::send(&mut stream, proto::INPUT, &bytes[..n])?;
        }
        if fds[1].revents & (poller::READ | poller::HUP | poller::ERROR) != 0 {
            let n = match stream.read(&mut bytes) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                r => r?,
            };
            if n == 0 {
                break;
            }
            reader.feed(&bytes[..n]);
            for message in reader.by_ref() {
                match message.kind {
                    proto::OUTPUT => {
                        stdout.write_all(&message.payload)?;
                        stdout.flush()?;
                    }
                    proto::EXIT => {
                        reason = Some(message.payload.first().copied().unwrap_or(0));
                        native::request_quit();
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
    drop(stdout);
    drop(guard);
    if reason == Some(1) {
        eprintln!("[detached]");
    } else if reason == Some(2) {
        eprintln!("[replaced by another client]");
    }
    Ok(())
}
