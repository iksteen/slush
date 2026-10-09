//! Pollable sockets on Windows and file descriptors on Unix.
#[cfg(unix)]
pub use libc::{
    POLLERR as ERROR, POLLHUP as HUP, POLLIN as READ, POLLOUT as WRITE, pollfd as PollFd,
};
use std::io;
#[cfg(unix)]
pub type Descriptor = i32;
#[cfg(windows)]
pub use windows_sys::Win32::Networking::WinSock::{
    POLLERR as ERROR, POLLHUP as HUP, POLLRDNORM as READ, POLLWRNORM as WRITE, WSAPOLLFD as PollFd,
};
#[cfg(windows)]
pub type Descriptor = usize;
pub fn poll(fds: &mut [PollFd], timeout_ms: i32) -> io::Result<usize> {
    #[cfg(unix)]
    // SAFETY: the slice provides valid pollfd storage for the call's duration.
    let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) };
    #[cfg(windows)]
    // SAFETY: the slice provides valid WSAPOLLFD storage; handles stay borrowed.
    let result = unsafe {
        windows_sys::Win32::Networking::WinSock::WSAPoll(
            fds.as_mut_ptr(),
            fds.len() as u32,
            timeout_ms,
        )
    };
    if result >= 0 {
        Ok(result as usize)
    } else {
        #[cfg(unix)]
        let error = io::Error::last_os_error();
        #[cfg(windows)]
        // SAFETY: querying the thread-local Winsock error has no preconditions.
        let error = io::Error::from_raw_os_error(unsafe {
            windows_sys::Win32::Networking::WinSock::WSAGetLastError()
        });
        Err(error)
    }
}
