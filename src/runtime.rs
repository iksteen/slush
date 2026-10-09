//! CLI dispatch and daemon startup for persistent sessions.
use crate::{client, headless, local_socket::LocalStream, proto, server};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    ffi::{OsStr, OsString},
    fs::OpenOptions,
    io::{self, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
#[cfg(unix)]
use std::{
    fs::File,
    os::unix::{fs::OpenOptionsExt, process::CommandExt},
};

pub fn run(args: &[OsString]) -> io::Result<()> {
    let mut name = OsStr::new("main");
    let mut cols = 80;
    let mut rows = 24;
    let mut idle = 300;
    let mut script = false;
    let mut headless_mode = false;
    let mut server_mode = false;
    let mut watch = true;
    let mut list = false;
    let mut control = None;
    let mut layout = None;
    let mut argv = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].to_str().unwrap_or("");
        match arg {
            "--" => {
                argv = args[i + 1..].iter().take(63).cloned().collect();
                break;
            }
            "--script" => {
                script = true;
                headless_mode = true;
            }
            "--headless" => headless_mode = true,
            "--server" => server_mode = true,
            "--no-reload" => watch = false,
            "ls" => list = true,
            "-s" | "--layout" | "cmd" | "--cols" | "--rows" | "--idle-ms" => {
                i += 1;
                let value = args.get(i).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{arg} requires a value"),
                    )
                })?;
                match arg {
                    "-s" => name = value,
                    "--layout" => layout = Some(std::path::Path::new(value)),
                    "cmd" => control = Some(value.as_os_str()),
                    _ => {
                        let n = cli_number(&value.to_string_lossy());
                        match arg {
                            "--cols" => cols = n as u16,
                            "--rows" => rows = n as u16,
                            _ => idle = n.max(0) as u64,
                        }
                    }
                }
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown argument: {}", args[i].to_string_lossy()),
                ));
            }
        }
        i += 1;
    }
    if list {
        for name in proto::sessions().unwrap_or_default() {
            let state = if LocalStream::connect(proto::socket_path(&name)?).is_ok() {
                "running"
            } else {
                "stale"
            };
            let mut output = io::stdout().lock();
            #[cfg(unix)]
            let bytes = {
                use std::os::unix::ffi::OsStrExt;
                name.as_bytes()
            };
            #[cfg(windows)]
            let display = name.to_string_lossy();
            #[cfg(windows)]
            let bytes = display.as_bytes();
            output.write_all(bytes)?;
            writeln!(
                output,
                "{:width$} {state}",
                "",
                width = 20usize.saturating_sub(bytes.len())
            )?;
        }
        return Ok(());
    }
    if let Some(command) = control {
        let mut socket = connect(name).map_err(|_| {
            io::Error::other(format!("no session named {}", name.to_string_lossy()))
        })?;
        let reply = proto::control(&mut socket, &crate::json_control::native_bytes(command))?;
        io::stdout().write_all(&reply)?;
        println!();
        return Ok(());
    }
    if headless_mode {
        return headless::run(argv, cols, rows, idle, script, layout);
    }
    if client::is_terminal() {
        let size = client::terminal_size();
        cols = size.cols;
        rows = size.rows;
    }
    if server_mode {
        return server::run(name, argv, cols, rows, layout, watch);
    }
    if std::env::var_os("SLOSH").is_some()
        && std::env::var_os("SLOSH_SESSION")
            .is_some_and(|inside| !inside.is_empty() && inside == name)
    {
        return Err(io::Error::other(format!(
            "this pane is already inside session {}\n  (C-a d detaches; `slush -s NAME` nests another session;\n   unset SLOSH_SESSION to attach recursively anyway)",
            name.to_string_lossy()
        )));
    }
    let socket = match connect(name) {
        Ok(stream) => stream,
        Err(_) => spawn(name, argv, cols, rows, layout, watch)?,
    };
    client::run(socket)
}
// Match the reference CLI's decimal-prefix parsing, including an optional sign
// and zero for text without digits. Dimensions retain the uint16_t conversion.
fn cli_number(text: &str) -> i32 {
    let bytes = text
        .trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '\u{b}')
        .as_bytes();
    let (negative, digits) = match bytes.first() {
        Some(b'-') => (true, &bytes[1..]),
        Some(b'+') => (false, &bytes[1..]),
        _ => (false, bytes),
    };
    let n = digits
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .fold(0i32, |n, c| {
            n.wrapping_mul(10).wrapping_add(i32::from(c - b'0'))
        });
    if negative { n.wrapping_neg() } else { n }
}
fn connect(name: &OsStr) -> io::Result<LocalStream> {
    LocalStream::connect(proto::socket_path(name)?)
}
fn spawn(
    name: &OsStr,
    argv: Vec<OsString>,
    cols: u16,
    rows: u16,
    layout: Option<&std::path::Path>,
    watch: bool,
) -> io::Result<LocalStream> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    let log = options.open(proto::log_path(name)?)?;
    #[cfg(unix)]
    let null = Stdio::from(File::open("/dev/null")?);
    #[cfg(windows)]
    let null = Stdio::null();
    let mut command = Command::new(std::env::current_exe()?);
    command.args(["--server", "-s"]).arg(name).args([
        "--cols",
        &cols.to_string(),
        "--rows",
        &rows.to_string(),
    ]);
    if let Some(layout) = layout {
        command.arg("--layout").arg(layout);
    }
    if !watch {
        command.arg("--no-reload");
    }
    if !argv.is_empty() {
        command.arg("--").args(argv);
    }
    command
        .stdin(null)
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    // SAFETY: the pre-exec closure uses only async-signal-safe POSIX operations.
    // The intermediate child exits immediately; the grandchild retains stdio
    // and the exec error pipe until Rust's normal exec path closes it.
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            let pid = libc::fork();
            if pid < 0 {
                return Err(io::Error::last_os_error());
            }
            if pid > 0 {
                libc::_exit(0);
            }
            Ok(())
        });
    }
    #[cfg(windows)]
    command.creation_flags(
        windows_sys::Win32::System::Threading::DETACHED_PROCESS
            | windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP,
    );
    let mut intermediate = command.spawn()?;
    #[cfg(unix)]
    intermediate.wait()?;
    #[cfg(windows)]
    {
        let _ = intermediate.try_wait()?;
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match connect(name) {
            Ok(stream) => return Ok(stream),
            Err(e) if Instant::now() >= deadline => {
                return Err(io::Error::other(format!(
                    "cannot start session {}: {e}",
                    name.to_string_lossy()
                )));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}
