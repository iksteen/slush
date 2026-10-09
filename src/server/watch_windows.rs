//! Overlapped directory notifications bridged to the shared socket poll loop.
use super::*;
use crate::local_socket::AsDescriptor;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
};
use windows_sys::Win32::{
    Foundation::{ERROR_IO_PENDING, HANDLE, WAIT_OBJECT_0},
    Storage::FileSystem::*,
    System::{IO::*, Threading::*},
};

struct Wanted {
    generation: u64,
    files: Vec<std::path::PathBuf>,
}
struct Shared {
    change: OwnedHandle,
    wanted: Mutex<Wanted>,
    stopping: AtomicBool,
    armed: AtomicU64,
}
struct Armed {
    file: fs::File,
    event: OwnedHandle,
    overlapped: OVERLAPPED,
    buffer: [u8; 8192],
    names: BTreeSet<String>,
    pending: bool,
}
fn event(manual: bool) -> io::Result<OwnedHandle> {
    // SAFETY: create an unnamed, non-inheritable event owned by the caller.
    let handle = unsafe { CreateEventW(std::ptr::null(), i32::from(manual), 0, std::ptr::null()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}
impl Armed {
    fn open(dir: &Path, names: BTreeSet<String>) -> io::Result<Box<Self>> {
        let file = fs::OpenOptions::new()
            .read(true)
            .access_mode(FILE_LIST_DIRECTORY)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED)
            .open(dir)?;
        let event = event(true)?;
        let mut watch = Box::new(Self {
            file,
            event,
            overlapped: OVERLAPPED::default(),
            buffer: [0; 8192],
            names,
            pending: false,
        });
        watch.overlapped.hEvent = watch.event.as_raw_handle();
        watch.arm()?;
        Ok(watch)
    }
    fn arm(&mut self) -> io::Result<()> {
        // SAFETY: the event is live, and this boxed watch keeps the buffer and
        // OVERLAPPED at fixed addresses until completion/cancellation in Drop.
        unsafe {
            ResetEvent(self.event.as_raw_handle());
        }
        let ok = unsafe {
            ReadDirectoryChangesW(
                self.file.as_raw_handle(),
                self.buffer.as_mut_ptr().cast(),
                self.buffer.len() as u32,
                0,
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_LAST_WRITE
                    | FILE_NOTIFY_CHANGE_SIZE,
                std::ptr::null_mut(),
                &mut self.overlapped,
                None,
            )
        };
        if ok == 0 && io::Error::last_os_error().raw_os_error() != Some(ERROR_IO_PENDING as i32) {
            return Err(io::Error::last_os_error());
        }
        self.pending = true;
        Ok(())
    }
    fn complete(&mut self) -> io::Result<bool> {
        let mut got = 0;
        // SAFETY: the signalled event belongs to this pending operation.
        let ok = unsafe {
            GetOverlappedResult(self.file.as_raw_handle(), &self.overlapped, &mut got, 0)
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        self.pending = false;
        // A zero-length completion means notification overflow; reload rather
        // than silently losing a changed config.
        let mut touched = got == 0;
        let mut offset = 0;
        while offset + 12 <= got as usize {
            let record = &self.buffer[offset..got as usize];
            let next = u32::from_le_bytes(record[..4].try_into().unwrap()) as usize;
            let length = u32::from_le_bytes(record[8..12].try_into().unwrap()) as usize;
            if !length.is_multiple_of(2) || length > record.len() - 12 {
                touched = true;
                break;
            }
            let name: Vec<_> = record[12..12 + length]
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            touched |= self
                .names
                .contains(&String::from_utf16_lossy(&name).to_lowercase());
            if next == 0 {
                break;
            }
            if next < 12 || next > record.len() {
                touched = true;
                break;
            }
            offset += next;
        }
        Ok(touched)
    }
}
impl Drop for Armed {
    fn drop(&mut self) {
        if self.pending {
            // SAFETY: cancel and await the exact operation before freeing its
            // buffer/OVERLAPPED, including when the directory disappears.
            unsafe {
                CancelIoEx(self.file.as_raw_handle(), &self.overlapped);
                let mut got = 0;
                GetOverlappedResult(self.file.as_raw_handle(), &self.overlapped, &mut got, 1);
            }
        }
    }
}

pub struct Watcher {
    socket: TcpStream,
    shared: Arc<Shared>,
    pump: Option<JoinHandle<()>>,
}
impl Watcher {
    pub fn new() -> io::Result<Option<Self>> {
        let (socket, mut inner) = crate::local_socket::pump_pair()?;
        socket.set_nonblocking(true)?;
        inner.set_nonblocking(true)?;
        let shared = Arc::new(Shared {
            change: event(false)?,
            wanted: Mutex::new(Wanted {
                generation: 0,
                files: Vec::new(),
            }),
            stopping: AtomicBool::new(false),
            armed: AtomicU64::new(0),
        });
        let state = shared.clone();
        let pump = std::thread::Builder::new()
            .name("config-watch".into())
            .spawn(move || {
                let mut generation = u64::MAX;
                let mut watches: Vec<Box<Armed>> = Vec::new();
                while !state.stopping.load(Ordering::Acquire) {
                    let wanted = state.wanted.lock().unwrap();
                    let (fresh, files) = (wanted.generation, wanted.files.clone());
                    drop(wanted);
                    if fresh != generation {
                        watches.clear();
                        let mut directories = BTreeMap::<PathBuf, BTreeSet<String>>::new();
                        for file in files {
                            let path = Path::new(&file);
                            let dir = path
                                .parent()
                                .filter(|p| !p.as_os_str().is_empty())
                                .unwrap_or(Path::new("."));
                            if let Some(name) = path.file_name() {
                                directories
                                    .entry(dir.into())
                                    .or_default()
                                    .insert(name.to_string_lossy().to_lowercase());
                            }
                        }
                        for (dir, names) in directories.into_iter().take(63) {
                            if let Ok(watch) = Armed::open(&dir, names) {
                                watches.push(watch);
                            }
                        }
                        generation = fresh;
                        state.armed.store(fresh, Ordering::Release);
                    }
                    let mut waits: Vec<HANDLE> = vec![state.change.as_raw_handle()];
                    waits.extend(watches.iter().map(|w| w.event.as_raw_handle()));
                    // SAFETY: all events remain owned throughout this bounded wait.
                    let result = unsafe {
                        WaitForMultipleObjects(waits.len() as u32, waits.as_ptr(), 0, 200)
                    };
                    if result == WAIT_OBJECT_0 {
                        continue;
                    }
                    let index = result.wrapping_sub(WAIT_OBJECT_0 + 1) as usize;
                    if index >= watches.len() {
                        continue;
                    }
                    match watches[index].complete() {
                        Ok(true) => {
                            let _ = inner.write(&[1]);
                        }
                        Ok(false) => {}
                        Err(_) => {
                            watches.swap_remove(index);
                            let _ = inner.write(&[1]);
                            continue;
                        }
                    }
                    if watches[index].arm().is_err() {
                        watches.swap_remove(index);
                    }
                }
            })?;
        Ok(Some(Self {
            socket,
            shared,
            pump: Some(pump),
        }))
    }
    pub fn fd(&self) -> crate::poller::Descriptor {
        self.socket.descriptor()
    }
    pub fn update(&mut self, files: &[std::path::PathBuf]) {
        if let Some(file) = files.first() {
            let dir = Path::new(file).parent().unwrap_or(Path::new("."));
            let _ = fs::create_dir_all(dir);
        }
        let mut wanted = self.shared.wanted.lock().unwrap();
        if wanted.files == files {
            return;
        }
        wanted.files = files.to_owned();
        wanted.generation = wanted.generation.wrapping_add(1);
        drop(wanted);
        // SAFETY: wake the pump's live rebuild event.
        unsafe {
            SetEvent(self.shared.change.as_raw_handle());
        }
    }
    pub fn drain(&mut self) -> io::Result<bool> {
        let mut touched = false;
        let mut bytes = [0; 256];
        loop {
            match self.socket.read(&mut bytes) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "config watcher stopped",
                    ));
                }
                Ok(_) => touched = true,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(touched),
                Err(e) => return Err(e),
            }
        }
    }
}
impl Drop for Watcher {
    fn drop(&mut self) {
        self.shared.stopping.store(true, Ordering::Release);
        // SAFETY: the pump shares ownership of this live event.
        unsafe {
            SetEvent(self.shared.change.as_raw_handle());
        }
        let _ = self.socket.shutdown(Shutdown::Both);
        if let Some(pump) = self.pump.take() {
            let _ = pump.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_notifications_survive_rename_and_rebuild() {
        let base = std::env::temp_dir().join(format!("slush-watch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let first = base.join("config.kdl");
        let second = base.join("include.kdl");
        fs::write(&first, "gap 1").unwrap();
        fs::write(&second, "gap 2").unwrap();
        let mut watcher = Watcher::new().unwrap().unwrap();
        let arm = |watcher: &mut Watcher, path: &Path| {
            watcher.update(&[path.to_owned()]);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let wanted = watcher.shared.wanted.lock().unwrap().generation;
            while watcher.shared.armed.load(Ordering::Acquire) != wanted {
                assert!(std::time::Instant::now() < deadline, "watcher did not arm");
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        };
        let changed = |watcher: &mut Watcher| {
            let mut fd = crate::poller::PollFd {
                fd: watcher.fd(),
                events: crate::poller::READ,
                revents: 0,
            };
            assert_eq!(
                crate::poller::poll(std::slice::from_mut(&mut fd), 2000).unwrap(),
                1
            );
            assert!(watcher.drain().unwrap());
        };
        arm(&mut watcher, &first);
        fs::write(&first, "gap 3").unwrap();
        changed(&mut watcher);
        let replacement = base.join("new.kdl");
        fs::write(&replacement, "gap 4").unwrap();
        fs::rename(&replacement, &first).unwrap();
        changed(&mut watcher);
        arm(&mut watcher, &second);
        fs::write(&second, "gap 5").unwrap();
        changed(&mut watcher);
        drop(watcher);
        fs::remove_dir_all(base).unwrap();
    }
}
