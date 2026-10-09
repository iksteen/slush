//! Machine-local session sockets, with the same AF_UNIX protocol on both OSes.
use crate::poller::Descriptor;
#[cfg(unix)]
pub use std::os::unix::net::{UnixListener as LocalListener, UnixStream as LocalStream};

pub trait AsDescriptor {
    fn descriptor(&self) -> Descriptor;
}
#[cfg(unix)]
impl<T: std::os::fd::AsRawFd> AsDescriptor for T {
    fn descriptor(&self) -> Descriptor {
        self.as_raw_fd()
    }
}
#[cfg(windows)]
impl<T: std::os::windows::io::AsRawSocket> AsDescriptor for T {
    fn descriptor(&self) -> Descriptor {
        self.as_raw_socket() as usize
    }
}

#[cfg(windows)]
mod windows {
    use std::{
        io::{self, Read, Write},
        os::windows::io::{AsRawSocket, FromRawSocket, OwnedSocket, RawSocket},
        path::Path,
        sync::OnceLock,
    };
    use windows_sys::Win32::{
        Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation},
        Networking::WinSock::*,
    };

    fn last_error() -> io::Error {
        // SAFETY: querying the calling thread's Winsock error.
        io::Error::from_raw_os_error(unsafe { WSAGetLastError() })
    }
    fn owned(socket: SOCKET) -> io::Result<OwnedSocket> {
        if socket == INVALID_SOCKET {
            return Err(last_error());
        }
        // SAFETY: successful socket/accept transfers a unique socket to us.
        let socket = unsafe { OwnedSocket::from_raw_socket(socket as RawSocket) };
        // SAFETY: SOCKETs are kernel handles. Prevent inheritance by daemon
        // subprocesses, including accepted sockets which have no WSA flags.
        if unsafe { SetHandleInformation(socket.as_raw_socket() as _, HANDLE_FLAG_INHERIT, 0) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(socket)
    }
    fn socket() -> io::Result<OwnedSocket> {
        static START: OnceLock<i32> = OnceLock::new();
        let error = *START.get_or_init(|| {
            let mut data = WSADATA::default();
            // SAFETY: the data output lives through this call. Keep Winsock
            // initialized for this process, independently of std's own sockets.
            unsafe { WSAStartup(0x202, &mut data) }
        });
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }
        // SAFETY: creating a non-inheritable stream socket with no protocol info.
        owned(unsafe {
            WSASocketW(
                AF_UNIX as i32,
                SOCK_STREAM,
                0,
                std::ptr::null(),
                0,
                WSA_FLAG_NO_HANDLE_INHERIT,
            )
        })
    }
    fn address(path: &Path) -> io::Result<SOCKADDR_UN> {
        let path = std::path::absolute(path)?;
        let name = path.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "session socket path is not Unicode",
            )
        })?;
        let mut address = SOCKADDR_UN::default();
        if name.len() >= address.sun_path.len() || name.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "session socket path exceeds AF_UNIX limits",
            ));
        }
        address.sun_family = AF_UNIX;
        for (dest, source) in address.sun_path.iter_mut().zip(name.bytes()) {
            *dest = source as i8;
        }
        Ok(address)
    }
    fn nonblocking(socket: &OwnedSocket, value: bool) -> io::Result<()> {
        let mut mode = u32::from(value);
        // SAFETY: a live socket and writable mode value for FIONBIO.
        if unsafe { ioctlsocket(socket.as_raw_socket() as usize, FIONBIO, &mut mode) }
            == SOCKET_ERROR
        {
            Err(last_error())
        } else {
            Ok(())
        }
    }
    pub struct LocalStream(OwnedSocket);
    impl LocalStream {
        pub fn connect(path: impl AsRef<Path>) -> io::Result<Self> {
            let address = address(path.as_ref())?;
            let socket = socket()?;
            // SAFETY: address is a complete AF_UNIX structure and the socket is live.
            if unsafe {
                connect(
                    socket.as_raw_socket() as usize,
                    (&address as *const SOCKADDR_UN).cast(),
                    std::mem::size_of::<SOCKADDR_UN>() as i32,
                )
            } == SOCKET_ERROR
            {
                return Err(last_error());
            }
            Ok(Self(socket))
        }
        pub fn set_nonblocking(&self, value: bool) -> io::Result<()> {
            nonblocking(&self.0, value)
        }
    }
    impl AsRawSocket for LocalStream {
        fn as_raw_socket(&self) -> RawSocket {
            self.0.as_raw_socket()
        }
    }
    impl Read for LocalStream {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if bytes.is_empty() {
                return Ok(0);
            }
            // SAFETY: the socket and output slice are live for the bounded read.
            let n = unsafe {
                recv(
                    self.0.as_raw_socket() as usize,
                    bytes.as_mut_ptr(),
                    bytes.len().min(i32::MAX as usize) as i32,
                    0,
                )
            };
            if n == SOCKET_ERROR {
                Err(last_error())
            } else {
                Ok(n as usize)
            }
        }
    }
    impl Write for LocalStream {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            // SAFETY: the socket and input slice are live for the bounded send.
            let n = unsafe {
                send(
                    self.0.as_raw_socket() as usize,
                    bytes.as_ptr(),
                    bytes.len().min(i32::MAX as usize) as i32,
                    0,
                )
            };
            if n == SOCKET_ERROR {
                Err(last_error())
            } else {
                Ok(n as usize)
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    pub struct LocalListener(OwnedSocket);
    impl LocalListener {
        pub fn bind(path: impl AsRef<Path>) -> io::Result<Self> {
            let address = address(path.as_ref())?;
            let socket = socket()?;
            // SAFETY: the address is correctly sized, and both operations use a live socket.
            if unsafe {
                bind(
                    socket.as_raw_socket() as usize,
                    (&address as *const SOCKADDR_UN).cast(),
                    std::mem::size_of::<SOCKADDR_UN>() as i32,
                )
            } == SOCKET_ERROR
                || unsafe { listen(socket.as_raw_socket() as usize, SOMAXCONN as i32) }
                    == SOCKET_ERROR
            {
                return Err(last_error());
            }
            Ok(Self(socket))
        }
        pub fn accept(&self) -> io::Result<(LocalStream, ())> {
            // SAFETY: a live listening socket; no peer address requested.
            owned(unsafe {
                accept(
                    self.0.as_raw_socket() as usize,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            })
            .map(|s| (LocalStream(s), ()))
        }
        pub fn set_nonblocking(&self, value: bool) -> io::Result<()> {
            nonblocking(&self.0, value)
        }
    }
    impl AsRawSocket for LocalListener {
        fn as_raw_socket(&self) -> RawSocket {
            self.0.as_raw_socket()
        }
    }
}
#[cfg(windows)]
pub use windows::{LocalListener, LocalStream};

/// A pollable, full-duplex channel for pipe and notification pumps on Windows.
#[cfg(windows)]
pub fn pump_pair() -> std::io::Result<(std::net::TcpStream, std::net::TcpStream)> {
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let outer = TcpStream::connect(listener.local_addr()?)?;
    let (inner, _) = listener.accept()?;
    for stream in [&outer, &inner] {
        stream.set_nodelay(true)?;
    }
    Ok((outer, inner))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    #[test]
    fn local_socket_protocol_and_nonblocking_poll() {
        let path = std::env::temp_dir().join(format!("slush-socket-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = LocalListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().err().unwrap().kind(),
            std::io::ErrorKind::WouldBlock
        );
        let mut client = LocalStream::connect(&path).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        server.set_nonblocking(true).unwrap();
        let mut buffer = [0; 6];
        assert_eq!(
            server.read(&mut buffer).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        client.write_all(b"native").unwrap();
        let mut fd = crate::poller::PollFd {
            fd: server.descriptor(),
            events: crate::poller::READ,
            revents: 0,
        };
        assert_eq!(
            crate::poller::poll(std::slice::from_mut(&mut fd), 1000).unwrap(),
            1
        );
        server.read_exact(&mut buffer).unwrap();
        assert_eq!(&buffer, b"native");
        server.write_all(b"reply").unwrap();
        let mut reply = [0; 5];
        client.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"reply");
        drop(client);
        crate::poller::poll(std::slice::from_mut(&mut fd), 1000).unwrap();
        assert_eq!(server.read(&mut buffer).unwrap(), 0);
        drop(server);
        drop(listener);
        std::fs::remove_file(path).unwrap();
    }
}
