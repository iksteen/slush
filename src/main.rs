use slush::config::Config;
use std::{
    ffi::OsString,
    io::{self, Write},
    path::Path,
    process::ExitCode,
};

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let utility = utility_index(&args);
    let utility_args = utility.map_or(&[][..], |i| &args[i..]);
    match utility_args.first().and_then(|s| s.to_str()) {
        Some("--install-terminfo") => install_terminfo(),
        Some("--dump-config") => {
            print!("{}", Config::default().render());
            ExitCode::SUCCESS
        }
        Some("--dump-theme") => {
            let mut config = Config::default();
            config.load(Config::default_path());
            io::stdout()
                .write_all(&config.render_theme_bytes())
                .expect("write theme");
            ExitCode::SUCCESS
        }
        Some("--check") => {
            let file = utility_args
                .get(1)
                .filter(|s| !s.as_encoded_bytes().starts_with(b"-"))
                .map(std::path::PathBuf::from)
                .unwrap_or_else(Config::default_path);
            if utility_args
                .get(1)
                .is_some_and(|s| !s.as_encoded_bytes().starts_with(b"-"))
                && slush::layout_check::looks_like_layout(&file)
            {
                let report = slush::layout_check::check_file(&file);
                for message in &report.messages {
                    eprintln!("  {message}");
                }
                if report.dropped > 0 {
                    eprintln!("  ...and {} more", report.dropped);
                }
                let problems = report.messages.len() + report.dropped;
                if problems == 0 {
                    path_line(io::stdout(), "", &file, ": ok, a layout");
                    return ExitCode::SUCCESS;
                }
                path_line(
                    io::stderr(),
                    "",
                    &file,
                    &format!(
                        ": {problems} problem{}",
                        if problems == 1 { "" } else { "s" }
                    ),
                );
                return ExitCode::FAILURE;
            }
            let mut config = Config::default();
            let loaded = config.load(&file);
            for message in &config.messages {
                let mut stderr = io::stderr().lock();
                stderr.write_all(b"  ").expect("write diagnostic");
                stderr.write_all(message.bytes()).expect("write diagnostic");
                stderr.write_all(b"\n").expect("write diagnostic");
            }
            if !loaded {
                path_line(
                    io::stderr(),
                    "",
                    &file,
                    ": not loaded (defaults would stand)",
                );
            } else if !config.messages.is_empty() {
                path_line(
                    io::stderr(),
                    "",
                    &file,
                    &format!(
                        ": {} problem{}",
                        config.messages.len(),
                        if config.messages.len() == 1 { "" } else { "s" }
                    ),
                );
            }
            if loaded && config.messages.is_empty() {
                path_line(io::stdout(), "", &file, ": ok");
                for file in &config.files {
                    path_line(io::stdout(), "  read     ", file, "");
                }
                println!("  prefix   {}", config.bindings.prefix.display());
                println!("  bindings {}", config.bindings.entries.len());
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Some("--version") => {
            println!("slush {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") => {
            println!(
                "usage: slush [-s NAME] [--layout FILE] [--no-reload] [-- COMMAND...]\n       slush [-s NAME] cmd LINE | ls\n       slush --server [-s NAME] [--cols N] [--rows N] [-- COMMAND...]\n       slush --script|--headless [--cols N] [--rows N] [--idle-ms N] [--layout FILE] [-- COMMAND...]\n       slush --check [FILE] | --dump-config | --dump-theme | --install-terminfo | --version"
            );
            ExitCode::SUCCESS
        }
        _ => run_runtime(&args),
    }
}

// Unix filenames are byte strings; CLI reports preserve them just as reads do.
fn path_line(mut output: impl Write, prefix: &str, file: &Path, suffix: &str) {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        file.as_os_str().as_bytes()
    };
    #[cfg(not(unix))]
    let display = file.to_string_lossy();
    #[cfg(not(unix))]
    let bytes = display.as_bytes();
    output.write_all(prefix.as_bytes()).unwrap();
    output.write_all(bytes).unwrap();
    writeln!(output, "{suffix}").unwrap();
}

// Utility modes take effect when reached in the argument parser. Values and
// pane commands must not be mistaken for flags belonging to this process.
fn utility_index(args: &[OsString]) -> Option<usize> {
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        match arg.to_str()? {
            "--check" | "--dump-config" | "--dump-theme" | "--install-terminfo" | "--version"
            | "--help" | "-h" => return Some(i),
            "-s" | "--layout" | "cmd" | "--cols" | "--rows" | "--idle-ms" => i += 2,
            "--headless" | "--script" | "--server" | "--no-reload" | "ls" => i += 1,
            _ => return None,
        }
    }
    None
}

#[cfg(unix)]
fn install_terminfo() -> ExitCode {
    match slush::terminfo::install() {
        Ok(()) => {
            println!(
                "installed xterm-ghostty (and the ghostty alias) into ~/.terminfo\nnew sessions use it; a session already running keeps the TERM it\nchose at startup until it is restarted"
            );
            ExitCode::SUCCESS
        }
        Err(_) => {
            eprintln!("slush: could not write ~/.terminfo (no $HOME, or not writable)");
            ExitCode::FAILURE
        }
    }
}
#[cfg(not(unix))]
fn install_terminfo() -> ExitCode {
    eprintln!("slush: terminfo is not a Windows concept; nothing to install");
    ExitCode::FAILURE
}

fn run_runtime(args: &[OsString]) -> ExitCode {
    match slush::runtime::run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("slush: {e}");
            if e.kind() == std::io::ErrorKind::InvalidInput {
                ExitCode::from(2)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utility_flags_respect_argument_values_and_command_boundary() {
        let index =
            |args: &[&str]| utility_index(&args.iter().map(|s| (*s).into()).collect::<Vec<_>>());
        assert_eq!(
            index(&["-s", "session", "--no-reload", "--dump-config"]),
            Some(3)
        );
        assert_eq!(
            index(&["--layout", "--dump-config", "--check", "file.kdl"]),
            Some(2)
        );
        assert_eq!(index(&["--", "echo", "--dump-config"]), None);
        assert_eq!(index(&["cmd", "--dump-config"]), None);
        assert_eq!(index(&["unknown", "--version"]), None);
    }
}
