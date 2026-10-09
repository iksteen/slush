//! Paths from configuration files are resolved lexically, without a shell.
use std::{
    borrow::Cow,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

pub fn is_absolute(path: &str) -> bool {
    if path.starts_with('/') {
        return true;
    }
    #[cfg(windows)]
    {
        let b = path.as_bytes();
        path.starts_with('\\')
            || (b.len() >= 3
                && b[0].is_ascii_alphabetic()
                && b[1] == b':'
                && matches!(b[2], b'/' | b'\\'))
    }
    #[cfg(not(windows))]
    false
}

pub fn expand(path: &str) -> Cow<'_, str> {
    if (path == "~" || path.starts_with("~/") || (cfg!(windows) && path.starts_with("~\\")))
        && let Some(home) = home()
    {
        return Cow::Owned(format!("{home}{}", &path[1..]));
    }
    Cow::Borrowed(path)
}

pub fn home() -> Option<String> {
    let home = std::env::var("HOME").ok().filter(|s| !s.is_empty());
    #[cfg(windows)]
    let home = home.or_else(|| std::env::var("USERPROFILE").ok().filter(|s| !s.is_empty()));
    home
}

pub fn resolve<'a>(path: &'a str, base: Option<&'a str>) -> Cow<'a, str> {
    if path.starts_with('~') {
        return expand(path);
    }
    let Some(base) = base.filter(|b| !b.is_empty()) else {
        return Cow::Borrowed(path);
    };
    if is_absolute(path) {
        Cow::Borrowed(path)
    } else if path == "." {
        Cow::Borrowed(base)
    } else {
        Cow::Owned(format!(
            "{base}/{}",
            path.strip_prefix("./").unwrap_or(path)
        ))
    }
}

fn separator(path: &str) -> Option<usize> {
    path.rfind(|c| c == '/' || (cfg!(windows) && c == '\\'))
}

pub fn dir(path: &str) -> &str {
    match separator(path) {
        None => ".",
        Some(0) => &path[..1],
        Some(2) if cfg!(windows) && path.as_bytes()[1] == b':' => &path[..3],
        Some(n) => &path[..n],
    }
}

pub fn base(path: &str) -> &str {
    separator(path).map_or(path, |n| &path[n + 1..])
}

pub fn relative<'a>(path: &'a str, base: Option<&str>) -> &'a str {
    let Some(base) = base.filter(|b| !b.is_empty()) else {
        return path;
    };
    let base = base.trim_end_matches('/');
    match path.strip_prefix(base) {
        Some("") | Some("/") => ".",
        Some(rest) if rest.starts_with('/') => &rest[1..],
        _ => path,
    }
}

/// Native paths retain OS bytes; textual helpers above serve KDL/JSON rendering.
pub fn home_os() -> Option<OsString> {
    let home = std::env::var_os("HOME").filter(|s| !s.is_empty());
    #[cfg(windows)]
    let home = home.or_else(|| std::env::var_os("USERPROFILE").filter(|s| !s.is_empty()));
    home
}

pub fn expand_os(path: &Path) -> PathBuf {
    let bytes = path.as_os_str().as_encoded_bytes();
    if (bytes == b"~" || bytes.starts_with(b"~/") || (cfg!(windows) && bytes.starts_with(b"~\\")))
        && let Some(mut home) = home_os()
    {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            home.push(std::ffi::OsStr::from_bytes(&bytes[1..]));
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::{OsStrExt, OsStringExt};
            let wide: Vec<_> = path.as_os_str().encode_wide().collect();
            home.push(OsString::from_wide(&wide[1..]));
        }
        return home.into();
    }
    path.to_owned()
}

/// Concatenate literally, preserving dots and separators in tracked filenames.
pub fn join_os(base: &Path, path: &Path) -> PathBuf {
    let mut joined = base.as_os_str().to_owned();
    joined.push("/");
    joined.push(path);
    joined.into()
}

/// Remove a literal native base before rendering a portable layout directory.
pub fn relative_os(path: &Path, base: Option<&Path>) -> PathBuf {
    let Some(base) = base.filter(|p| !p.as_os_str().is_empty()) else {
        return path.to_owned();
    };
    fn suffix<'a, T: PartialEq>(path: &'a [T], base: &[T], slash: &T) -> Option<&'a [T]> {
        let end = base.iter().rposition(|c| c != slash).map_or(0, |n| n + 1);
        let rest = path.strip_prefix(&base[..end])?;
        if rest.is_empty() {
            Some(rest)
        } else {
            rest.strip_prefix(std::slice::from_ref(slash))
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        match suffix(
            path.as_os_str().as_bytes(),
            base.as_os_str().as_bytes(),
            &b'/',
        ) {
            Some([]) => PathBuf::from("."),
            Some(rest) => OsString::from_vec(rest.to_vec()).into(),
            None => path.to_owned(),
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let path_units: Vec<_> = path.as_os_str().encode_wide().collect();
        let base_units: Vec<_> = base.as_os_str().encode_wide().collect();
        match suffix(&path_units, &base_units, &(b'/' as u16)) {
            Some([]) => PathBuf::from("."),
            Some(rest) => OsString::from_wide(rest).into(),
            None => path.to_owned(),
        }
    }
}

pub fn include_os(file: &Path, reference: &Path) -> PathBuf {
    let expanded = expand_os(reference);
    if expanded.has_root() {
        expanded
    } else {
        join_os(dir_os(file), &expanded)
    }
}

pub fn resolve_os(path: &Path, base: Option<&Path>) -> PathBuf {
    let bytes = path.as_os_str().as_encoded_bytes();
    if bytes.starts_with(b"~") {
        return expand_os(path);
    }
    let Some(base) = base.filter(|p| !p.as_os_str().is_empty()) else {
        return path.to_owned();
    };
    if path.has_root() {
        path.to_owned()
    } else if bytes == b"." {
        base.to_owned()
    } else {
        #[cfg(unix)]
        let path = {
            use std::os::unix::ffi::OsStrExt;
            Path::new(std::ffi::OsStr::from_bytes(
                bytes.strip_prefix(b"./").unwrap_or(bytes),
            ))
        };
        #[cfg(windows)]
        let path = path
            .as_os_str()
            .to_str()
            .and_then(|s| s.strip_prefix("./"))
            .map(Path::new)
            .unwrap_or(path);
        join_os(base, path)
    }
}

pub fn dir_os(path: &Path) -> &Path {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        match bytes.iter().rposition(|b| *b == b'/') {
            None => Path::new("."),
            Some(0) => Path::new("/"),
            Some(n) => Path::new(std::ffi::OsStr::from_bytes(&bytes[..n])),
        }
    }
    #[cfg(windows)]
    {
        if let Some(text) = path.to_str() {
            return Path::new(dir(text));
        }
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| {
                if path.has_root() {
                    path
                } else {
                    Path::new(".")
                }
            })
    }
}

pub fn display_dirs(dirs: &[PathBuf]) -> String {
    dirs.iter()
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn mkdirs(path: impl AsRef<Path>) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_resolution_preserves_file_intent() {
        assert_eq!(
            resolve("./api/../db", Some("/project")),
            "/project/api/../db"
        );
        assert_eq!(resolve("~another/repo", Some("/project")), "~another/repo");
        assert_eq!(relative("/dev/api", Some("/dev/api/")), ".");
        assert_eq!(relative("/dev/apiary", Some("/dev/api")), "/dev/apiary");
        assert_eq!(relative("/dev/api/src", Some("/dev/api")), "src");
        assert_eq!(relative("/etc", Some("/")), "etc");
    }
}
