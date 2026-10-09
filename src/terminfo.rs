//! Embedded Ghostty terminfo, installed only when no readable entry exists.
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::OnceLock,
};
const ENTRY: &[u8] = include_bytes!("../assets/xterm-ghostty.terminfo");

pub fn install() -> io::Result<()> {
    let home = std::env::var_os("HOME")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is unset"))?;
    install_at(Path::new(&home))
}
fn install_at(home: &Path) -> io::Result<()> {
    for (directory, name) in [
        ("x", "xterm-ghostty"),
        ("78", "xterm-ghostty"),
        ("g", "ghostty"),
        ("67", "ghostty"),
    ] {
        let directory = home.join(".terminfo").join(directory);
        fs::create_dir_all(&directory)?;
        fs::write(directory.join(name), ENTRY)?;
    }
    Ok(())
}
fn choose(dirs: &[PathBuf], home: Option<&Path>) -> &'static str {
    if dirs.iter().any(|dir| {
        ["x/xterm-ghostty", "78/xterm-ghostty"]
            .iter()
            .any(|name| fs::File::open(dir.join(name)).is_ok())
    }) || home.is_some_and(|home| install_at(home).is_ok())
    {
        "xterm-ghostty"
    } else {
        "xterm-256color"
    }
}
pub fn terminal_name() -> &'static str {
    static NAME: OnceLock<&'static str> = OnceLock::new();
    NAME.get_or_init(|| {
        let home = std::env::var_os("HOME")
            .filter(|p| !p.is_empty())
            .map(PathBuf::from);
        let mut dirs = Vec::new();
        if let Some(value) = std::env::var_os("TERMINFO").filter(|p| !p.is_empty()) {
            dirs.extend(std::env::split_paths(&value).filter(|p| !p.as_os_str().is_empty()));
        }
        if let Some(home) = &home {
            dirs.push(home.join(".terminfo"));
        }
        if let Some(value) = std::env::var_os("TERMINFO_DIRS") {
            dirs.extend(std::env::split_paths(&value).filter(|p| !p.as_os_str().is_empty()));
        }
        dirs.extend(
            [
                "/etc/terminfo",
                "/lib/terminfo",
                "/usr/share/terminfo",
                "/usr/local/share/terminfo",
            ]
            .map(PathBuf::from),
        );
        choose(&dirs, home.as_deref())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "slush-terminfo-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn missing_database_is_installed_and_ncurses_can_read_all_aliases() {
        let f = Fixture::new();
        assert_eq!(choose(&[], Some(&f.0)), "xterm-ghostty");
        for name in [
            "x/xterm-ghostty",
            "78/xterm-ghostty",
            "g/ghostty",
            "67/ghostty",
        ] {
            assert_eq!(fs::read(f.0.join(".terminfo").join(name)).unwrap(), ENTRY);
        }
        let result = std::process::Command::new("infocmp")
            .args(["-A"])
            .arg(f.0.join(".terminfo"))
            .arg("xterm-ghostty")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let text = String::from_utf8(result.stdout).unwrap();
        assert!(
            text.contains("colors#0x100") || text.contains("colors#256"),
            "{text}"
        );
        assert!(text.contains("xterm-ghostty|ghostty"), "{text}");
    }
    #[test]
    fn existing_entry_is_preserved_and_an_unwritable_home_keeps_the_fallback() {
        let f = Fixture::new();
        let dir = f.0.join("database/78");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("xterm-ghostty"), b"newer external entry").unwrap();
        let home = f.0.join("home");
        assert_eq!(
            choose(&[f.0.join("database")], Some(&home)),
            "xterm-ghostty"
        );
        assert!(!home.exists());
        assert_eq!(
            fs::read(dir.join("xterm-ghostty")).unwrap(),
            b"newer external entry"
        );
        fs::write(&home, b"not a directory").unwrap();
        assert_eq!(choose(&[], Some(&home)), "xterm-256color");
        assert_eq!(choose(&[], None), "xterm-256color");
    }
}
