//! Watch directories as well as files so saves by rename remain observable.
use std::{fs, io, path::Path};

#[cfg(target_os = "linux")]
mod native {
    use super::*;
    use std::{
        collections::BTreeSet,
        ffi::CString,
        fs::File,
        io::Read,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    pub struct Watcher {
        file: File,
        watches: BTreeSet<i32>,
        names: BTreeSet<Vec<u8>>,
    }
    impl Watcher {
        pub fn new() -> io::Result<Option<Self>> {
            // SAFETY: inotify_init1 creates an owned nonblocking descriptor.
            let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
            if fd < 0 {
                return Ok(None);
            }
            Ok(Some(Self {
                file: unsafe { File::from_raw_fd(fd) },
                watches: BTreeSet::new(),
                names: BTreeSet::new(),
            }))
        }
        pub fn fd(&self) -> i32 {
            self.file.as_raw_fd()
        }
        pub fn update(&mut self, files: &[std::path::PathBuf]) {
            for wd in &self.watches {
                // SAFETY: the descriptor and watch ids belong to this watcher.
                unsafe {
                    libc::inotify_rm_watch(self.fd(), *wd);
                }
            }
            self.watches.clear();
            self.names.clear();
            for (i, file) in files.iter().enumerate() {
                let path = Path::new(file);
                let dir = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
                if i == 0 {
                    let _ = fs::create_dir_all(dir);
                }
                if let Some(name) = path.file_name() {
                    self.names.insert(name.as_bytes().to_vec());
                }
                if let Ok(dir) = CString::new(dir.as_os_str().as_bytes()) {
                    // SAFETY: the path is a terminated string, and the mask is valid.
                    let wd = unsafe {
                        libc::inotify_add_watch(
                            self.fd(),
                            dir.as_ptr(),
                            libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO | libc::IN_CREATE,
                        )
                    };
                    if wd >= 0 {
                        self.watches.insert(wd);
                    }
                }
            }
        }
        pub fn drain(&mut self) -> io::Result<bool> {
            let mut touched = false;
            let mut buffer = [0u8; 8192];
            loop {
                let n = match self.file.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) => return Err(e),
                };
                let header = std::mem::size_of::<libc::inotify_event>();
                let mut offset = 0;
                while offset + header <= n {
                    // SAFETY: the complete fixed-size event header fits in the byte buffer.
                    let event = unsafe {
                        std::ptr::read_unaligned(
                            buffer.as_ptr().add(offset).cast::<libc::inotify_event>(),
                        )
                    };
                    let end = offset + header + event.len as usize;
                    if end > n {
                        break;
                    }
                    let name = &buffer[offset + header..end];
                    let len = name.iter().position(|c| *c == 0).unwrap_or(name.len());
                    if self.names.contains(&name[..len]) {
                        touched = true;
                    }
                    offset = end;
                }
            }
            Ok(touched)
        }
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use std::{
        fs::{File, OpenOptions},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::OpenOptionsExt,
        },
    };
    pub struct Watcher {
        queue: File,
        files: Vec<File>,
    }
    impl Watcher {
        pub fn new() -> io::Result<Option<Self>> {
            // SAFETY: kqueue returns a newly owned descriptor.
            let fd = unsafe { libc::kqueue() };
            if fd < 0 {
                return Ok(None);
            }
            unsafe {
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
            Ok(Some(Self {
                queue: unsafe { File::from_raw_fd(fd) },
                files: Vec::new(),
            }))
        }
        pub fn fd(&self) -> i32 {
            self.queue.as_raw_fd()
        }
        pub fn update(&mut self, files: &[std::path::PathBuf]) {
            self.files.clear();
            for (i, file) in files.iter().enumerate() {
                let path = Path::new(file);
                let dir = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
                if i == 0 {
                    let _ = fs::create_dir_all(dir);
                }
                for path in [dir, path] {
                    let Ok(file) = OpenOptions::new()
                        .custom_flags(libc::O_EVTONLY | libc::O_CLOEXEC)
                        .read(true)
                        .open(path)
                    else {
                        continue;
                    };
                    let event = libc::kevent {
                        ident: file.as_raw_fd() as libc::uintptr_t,
                        filter: libc::EVFILT_VNODE,
                        flags: libc::EV_ADD | libc::EV_CLEAR,
                        fflags: libc::NOTE_WRITE
                            | libc::NOTE_EXTEND
                            | libc::NOTE_RENAME
                            | libc::NOTE_DELETE
                            | libc::NOTE_ATTRIB,
                        data: 0,
                        udata: std::ptr::null_mut(),
                    };
                    // SAFETY: all pointers describe valid event and timespec structures.
                    if unsafe {
                        libc::kevent(
                            self.fd(),
                            &event,
                            1,
                            std::ptr::null_mut(),
                            0,
                            std::ptr::null(),
                        )
                    } == 0
                    {
                        self.files.push(file);
                    }
                }
            }
        }
        pub fn drain(&mut self) -> io::Result<bool> {
            let zero = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            let mut touched = false;
            loop {
                let mut events: [libc::kevent; 16] = unsafe { std::mem::zeroed() };
                let n = unsafe {
                    libc::kevent(
                        self.fd(),
                        std::ptr::null(),
                        0,
                        events.as_mut_ptr(),
                        16,
                        &zero,
                    )
                };
                if n < 0 {
                    return Err(io::Error::last_os_error());
                }
                if n == 0 {
                    return Ok(touched);
                }
                touched = true;
            }
        }
    }
}
#[cfg(windows)]
#[path = "watch_windows.rs"]
mod native;
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod native {
    use super::*;
    pub struct Watcher;
    impl Watcher {
        pub fn new() -> io::Result<Option<Self>> {
            Ok(None)
        }
        pub fn fd(&self) -> crate::poller::Descriptor {
            crate::poller::Descriptor::MAX
        }
        pub fn update(&mut self, _: &[std::path::PathBuf]) {}
        pub fn drain(&mut self) -> io::Result<bool> {
            Ok(false)
        }
    }
}
pub use native::Watcher;
