//! Kernel process queries used when saving pane commands and working directories.
use std::path::PathBuf;

pub(crate) fn command(pid: libc::pid_t) -> Option<Vec<u8>> {
    #[cfg(target_os = "macos")]
    {
        let bytes = darwin_arguments(pid)?;
        join_arguments(parse_darwin_arguments(&bytes)?)
    }
    #[cfg(not(target_os = "macos"))]
    {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(format!("/proc/{pid}/cmdline"))
            .ok()?
            .take(4095)
            .read_to_end(&mut bytes)
            .ok()?;
        join_arguments(bytes.split(|c| *c == 0).take_while(|arg| !arg.is_empty()))
    }
}

fn join_arguments<'a>(args: impl IntoIterator<Item = &'a [u8]>) -> Option<Vec<u8>> {
    // Match pane_foreground's 4096-byte destination, including the room
    // reserved for an escaped quote and the terminating NUL.
    const CAP: usize = 4096;
    let mut out = Vec::new();
    for arg in args.into_iter().take_while(|arg| !arg.is_empty()) {
        if !out.is_empty() && out.len() + 1 < CAP {
            out.push(b' ');
        }
        if arg
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || b"_./:+,=@%-".contains(c))
        {
            let room = (CAP - 1).saturating_sub(out.len());
            out.extend_from_slice(&arg[..arg.len().min(room)]);
            if arg.len() > room {
                break;
            }
        } else {
            if out.len() + 1 < CAP {
                out.push(b'\'');
            }
            for b in arg {
                if out.len() + 5 >= CAP {
                    break;
                }
                if *b == b'\'' {
                    out.extend_from_slice(b"'\\''");
                } else {
                    out.push(*b);
                }
            }
            if out.len() + 1 < CAP {
                out.push(b'\'');
            }
        }
        if out.len() + 2 >= CAP {
            break;
        }
    }
    (!out.is_empty()).then_some(out)
}

pub(crate) fn cwd(pid: libc::pid_t) -> Option<PathBuf> {
    if pid <= 0 {
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStringExt;
        let mut info = std::mem::MaybeUninit::<libc::proc_vnodepathinfo>::uninit();
        let size = std::mem::size_of_val(&info) as libc::c_int;
        // SAFETY: info has the ABI layout and full size expected for this flavor.
        // Only a complete successful result is subsequently read.
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if written != size {
            return None;
        }
        // SAFETY: proc_pidinfo initialized the complete structure above.
        let info = unsafe { info.assume_init() };
        let path = info
            .pvi_cdir
            .vip_path
            .into_iter()
            .flatten()
            .take_while(|c| *c != 0)
            .map(|c| c as u8)
            .collect::<Vec<_>>();
        (!path.is_empty()).then(|| std::ffi::OsString::from_vec(path).into())
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }
}

#[cfg(target_os = "macos")]
fn darwin_arguments(pid: libc::pid_t) -> Option<Vec<u8>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size = 0;
    // SAFETY: a null output buffer requests the size; mib and size are writable.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || size < std::mem::size_of::<i32>()
    {
        return None;
    }
    // Ask for the complete block: Darwin rearranges data in an undersized buffer.
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(size).ok()?;
    bytes.resize(size, 0);
    // SAFETY: the output buffer has the queried capacity; sysctl reports the
    // bytes written in size. A raced exit/exec fails without parsing the output.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            bytes.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || size > bytes.len()
    {
        return None;
    }
    bytes.truncate(size);
    Some(bytes)
}

#[cfg(any(target_os = "macos", test))]
fn parse_darwin_arguments(bytes: &[u8]) -> Option<Vec<&[u8]>> {
    let argc = i32::from_ne_bytes(bytes.get(..4)?.try_into().ok()?);
    if argc <= 0 {
        return None;
    }
    let mut remaining = bytes.get(4..)?;
    let executable_end = remaining.iter().position(|c| *c == 0)?;
    remaining = &remaining[executable_end + 1..];
    let argv_start = remaining.iter().position(|c| *c != 0)?;
    remaining = &remaining[argv_start..];
    let mut args = Vec::new();
    let mut copied = 0;
    for _ in 0..argc {
        let end = remaining.iter().position(|c| *c == 0)?;
        let arg = &remaining[..end];
        // Match slosh's 4096-byte command block, stopping at an argument boundary.
        if copied + end + 1 >= 4096 {
            break;
        }
        copied += end + 1;
        args.push(arg);
        remaining = &remaining[end + 1..];
    }
    (!args.is_empty()).then_some(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(argc: i32, contents: &[u8]) -> Vec<u8> {
        let mut bytes = argc.to_ne_bytes().to_vec();
        bytes.extend_from_slice(contents);
        bytes
    }

    #[test]
    fn darwin_command_excludes_environment_and_quotes_shell_arguments() {
        let bytes = block(
            4,
            b"/usr/bin/tool\0\0\0tool\0a b\0it's\0$HOME\0SECRET=value\0",
        );
        assert_eq!(
            join_arguments(parse_darwin_arguments(&bytes).unwrap()).unwrap(),
            b"tool 'a b' 'it'\\''s' '$HOME'"
        );
        let linux = b"tool\0a b\0it's\0$HOME\0";
        assert_eq!(
            command_block(linux),
            join_arguments(parse_darwin_arguments(&bytes).unwrap())
        );
    }

    fn command_block(bytes: &[u8]) -> Option<Vec<u8>> {
        join_arguments(bytes.split(|c| *c == 0))
    }

    #[test]
    fn darwin_command_rejects_missing_headers_and_unterminated_arguments() {
        for bytes in [
            vec![],
            vec![0, 0, 0],
            block(-1, b"/bin/x\0x\0"),
            block(0, b"/bin/x\0x\0"),
            block(1, b"/bin/x"),
            block(2, b"/bin/x\0\0x\0unterminated"),
        ] {
            assert!(parse_darwin_arguments(&bytes).is_none(), "{bytes:?}");
        }
    }

    #[test]
    fn darwin_command_limit_preserves_complete_arguments() {
        let mut bytes = block(3, b"/bin/x\0\0x\0");
        bytes.extend(std::iter::repeat_n(b'a', 4093));
        bytes.extend_from_slice(b"\0ignored\0");
        assert_eq!(
            join_arguments(parse_darwin_arguments(&bytes).unwrap()).as_deref(),
            Some(b"x".as_slice())
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn kernel_reports_current_process_command_and_cwd() {
        let pid = std::process::id() as libc::pid_t;
        assert!(
            command(pid)
                .unwrap()
                .windows(b"slush".len())
                .any(|s| s == b"slush")
        );
        assert_eq!(
            cwd(pid).unwrap().canonicalize().unwrap(),
            std::env::current_dir().unwrap().canonicalize().unwrap()
        );
        assert!(cwd(-1).is_none());
        assert!(command(-1).is_none());
    }
}
