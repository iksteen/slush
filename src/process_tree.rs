//! Foreground approximation on Windows: the latest descendant's image name.
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::{Foundation::INVALID_HANDLE_VALUE, System::Diagnostics::ToolHelp::*};

pub fn foreground(root: u32) -> Option<String> {
    if root == 0 {
        return None;
    }
    // SAFETY: snapshot creation returns a new owned process snapshot handle.
    let handle = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let snapshot = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut processes = Vec::new();
    // SAFETY: the handle remains owned, and the structure advertises its size.
    let mut more = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0;
    while more {
        let end = entry
            .szExeFile
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(entry.szExeFile.len());
        processes.push((
            entry.th32ProcessID,
            entry.th32ParentProcessID,
            String::from_utf16_lossy(&entry.szExeFile[..end]),
        ));
        more = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } != 0;
    }
    let mut current = root;
    for _ in 0..16 {
        let Some((child, _, _)) = processes
            .iter()
            .rev()
            .find(|(pid, parent, _)| *parent == current && *pid != current)
        else {
            break;
        };
        current = *child;
    }
    if current == root {
        None
    } else {
        processes
            .into_iter()
            .find(|(pid, _, _)| *pid == current)
            .map(|(_, _, name)| name)
    }
}
