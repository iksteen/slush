//! Platform defaults and shell command invocation for panes and layouts.
use std::{
    ffi::{OsStr, OsString},
    path::Path,
};
pub fn default_program() -> OsString {
    #[cfg(unix)]
    let (variable, fallback) = ("SHELL", "/bin/sh");
    #[cfg(windows)]
    let (variable, fallback) = ("ComSpec", "cmd.exe");
    std::env::var_os(variable)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.into())
}
pub fn command(line: impl AsRef<OsStr>) -> Vec<OsString> {
    #[cfg(unix)]
    {
        vec!["/bin/sh".into(), "-c".into(), line.as_ref().to_owned()]
    }
    #[cfg(windows)]
    {
        vec![default_program(), "/c".into(), line.as_ref().to_owned()]
    }
}

/// Quote only the config filename; the editor remains a shell command.
pub fn editor_command(editor: impl AsRef<OsStr>, file: &Path) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let mut bytes = editor.as_ref().as_bytes().to_vec();
        bytes.extend_from_slice(b" '");
        for byte in file.as_os_str().as_bytes() {
            if *byte == b'\'' {
                bytes.extend_from_slice(b"'\\''");
            } else {
                bytes.push(*byte);
            }
        }
        bytes.push(b'\'');
        OsString::from_vec(bytes)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let absolute = std::path::absolute(file).unwrap_or_else(|_| file.to_owned());
        let mut wide: Vec<_> = editor.as_ref().encode_wide().collect();
        wide.extend([b' ' as u16, b'"' as u16]);
        wide.extend(
            absolute.as_os_str().encode_wide().map(
                |c| {
                    if c == b'/' as u16 { b'\\' as u16 } else { c }
                },
            ),
        );
        wide.push(b'"' as u16);
        OsString::from_wide(&wide)
    }
}

/// Preserve an explicitly chosen editor; otherwise prefer installed console
/// editors on Windows before using a windowed fallback.
pub fn editor(requested: Option<OsString>) -> (OsString, bool) {
    #[cfg(unix)]
    {
        (requested.unwrap_or_else(|| "vi".into()), false)
    }
    #[cfg(windows)]
    {
        if let Some(editor) = requested.filter(|s| s != "vi") {
            return (editor, false);
        }
        for name in ["nvim", "vim", "vi", "nano", "micro", "hx", "edit"] {
            if which(name) {
                return (name.into(), false);
            }
        }
        ("notepad".into(), true)
    }
}
#[cfg(windows)]
fn which(name: &str) -> bool {
    use windows_sys::Win32::Storage::FileSystem::SearchPathW;
    let name_wide: Vec<_> = name.encode_utf16().chain(std::iter::once(0)).collect();
    for extension in if name.contains('.') {
        &[""][..]
    } else {
        &[".exe", ".cmd", ".bat", ".com"][..]
    } {
        let extension: Vec<_> = extension.encode_utf16().chain(std::iter::once(0)).collect();
        let mut found = [0u16; 32768];
        // SAFETY: terminated strings, a correctly sized buffer, and no request
        // for a file-part pointer; SearchPath only inspects the filesystem.
        let n = unsafe {
            SearchPathW(
                std::ptr::null(),
                name_wide.as_ptr(),
                extension.as_ptr(),
                found.len() as u32,
                found.as_mut_ptr(),
                std::ptr::null_mut(),
            )
        };
        if n > 0 && (n as usize) < found.len() {
            return true;
        }
    }
    false
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn editor_search_honors_executable_suffixes_and_user_choice() {
        assert!(which("cmd"));
        assert!(which("cmd.exe"));
        assert!(!which("slush-nonexistent-editor-f27d91"));
        assert_eq!(
            editor(Some("custom --flag".into())),
            ("custom --flag".into(), false)
        );
        assert_eq!(editor(None), editor(Some("vi".into())));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{os::unix::ffi::OsStrExt, process::Command};

    #[test]
    fn editor_receives_one_native_filename_with_spaces_and_quotes() {
        let bytes = b"/tmp/config-\xff' space.kdl";
        let file = Path::new(OsStr::from_bytes(bytes));
        let argv = command(editor_command("printf '%s'", file));
        let output = Command::new(&argv[0]).args(&argv[1..]).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, bytes);
        assert!(output.stderr.is_empty());
    }
}
