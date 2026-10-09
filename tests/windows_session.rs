//! Native Windows session coverage. Run on Windows 10/11: Wine lacks AF_UNIX
//! and currently stubs ResizePseudoConsole, so it cannot validate this test.
#![cfg(windows)]
use slush::{
    ghostty::Terminal,
    local_socket::{AsDescriptor, LocalStream},
    poller, proto,
    pty::{Pty, Size},
};
use std::{
    fs,
    io::{self, Read},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
const BINARY: &str = env!("CARGO_BIN_EXE_slush");
const NAME: &str = "native-session";

struct Fixture {
    directory: PathBuf,
    config: PathBuf,
    server: Child,
}
impl Fixture {
    fn start() -> Self {
        let directory = std::env::temp_dir().join(format!("rs-session-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let config = directory.join("config.kdl");
        fs::write(&config, "splash_ms 0\ntheme {frame_focus \"#00ff00\"}\n").unwrap();
        let log = fs::File::create(directory.join("server.log")).unwrap();
        let server = Command::new(BINARY)
            .args([
                "--server", "-s", NAME, "--cols", "70", "--rows", "16", "--", "cmd.exe", "/q",
            ])
            .env("LOCALAPPDATA", &directory)
            .env("SLOSH_CONFIG", &config)
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        let mut fixture = Self {
            directory,
            config,
            server,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while fixture.control("alive").is_err() {
            assert!(
                fixture.server.try_wait().unwrap().is_none(),
                "server exited: {}",
                fs::read_to_string(fixture.directory.join("server.log")).unwrap()
            );
            assert!(Instant::now() < deadline, "server did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
        fixture
    }
    fn socket(&self) -> PathBuf {
        self.directory.join("slosh").join(format!("{NAME}.sock"))
    }
    fn control(&self, command: &str) -> io::Result<String> {
        let mut stream = LocalStream::connect(self.socket())?;
        proto::send(&mut stream, proto::CMD, command.as_bytes())?;
        stream.set_nonblocking(true)?;
        let mut reader = proto::Reader::default();
        let mut fd = poller::PollFd {
            fd: stream.descriptor(),
            events: poller::READ,
            revents: 0,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut buffer = [0; 65536];
        while Instant::now() < deadline {
            if poller::poll(std::slice::from_mut(&mut fd), 100)? == 0 {
                continue;
            }
            match stream.read(&mut buffer) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "session disconnected",
                    ));
                }
                Ok(n) => reader.feed(&buffer[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) => return Err(e),
            }
            for frame in reader.by_ref() {
                if frame.kind == proto::REPLY {
                    return String::from_utf8(frame.payload).map_err(io::Error::other);
                }
            }
        }
        Err(io::Error::new(io::ErrorKind::TimedOut, "no control reply"))
    }
    fn wait_for(&self, mut predicate: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !predicate(self) {
            assert!(
                Instant::now() < deadline,
                "session did not reach expected state"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn attach(&self, cols: u16, rows: u16) -> Display {
        // Set environment in the child shell, without mutating the test process.
        let command = format!(
            "set \"LOCALAPPDATA={}\" & set \"SLOSH_CONFIG={}\" & \"{BINARY}\" -s {NAME}",
            self.directory.display(),
            self.config.display()
        );
        let pty = Pty::spawn(
            ["cmd.exe", "/c", &command],
            Size {
                cols,
                rows,
                cell_width: 8,
                cell_height: 16,
            },
            None,
            1,
            None,
        )
        .unwrap();
        Display {
            pty,
            terminal: Terminal::new(cols, rows).unwrap(),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.control("quit");
        let _ = self.server.kill();
        let _ = self.server.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}
struct Display {
    pty: Pty,
    terminal: Terminal,
}
impl Display {
    fn read(&mut self) {
        self.pty.poll(Duration::from_millis(20)).unwrap();
        self.terminal.write(&self.pty.read_available().unwrap());
        self.pty.flush_pending().unwrap();
    }
    fn text(&mut self, marker: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.read();
            let text = self.terminal.plain_text().unwrap();
            if text.contains(marker) {
                return;
            }
            assert!(
                !self.pty.eof && Instant::now() < deadline,
                "missing {marker}: {text}"
            );
        }
    }
    fn detach(&mut self) {
        self.pty.send(b"\x01d").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.pty.eof && Instant::now() < deadline {
            self.read();
        }
        assert!(self.pty.eof, "client failed to detach");
        assert!(self.pty.exit_status.unwrap().success());
    }
}

#[test]
fn persistent_session_survives_native_console_detach_resize_and_reload() {
    let mut fixture = Fixture::start();
    let mut first = fixture.attach(70, 16);
    first.pty.send(b"echo marker-before-detach\r").unwrap();
    first.text("marker-before-detach");
    fixture.wait_for(|s| {
        s.control("snapshot text")
            .unwrap()
            .contains("marker-before-detach")
    });
    let before: serde_json::Value =
        serde_json::from_str(&fixture.control("panes").unwrap()).unwrap();
    assert!(before[0]["pid"].as_u64().unwrap() > 0);
    let listing = Command::new(BINARY)
        .arg("ls")
        .env("LOCALAPPDATA", &fixture.directory)
        .output()
        .unwrap();
    assert!(listing.status.success());
    assert!(String::from_utf8(listing.stdout).unwrap().contains(NAME));
    first.detach();
    let after: serde_json::Value =
        serde_json::from_str(&fixture.control("panes").unwrap()).unwrap();
    assert_eq!(before[0]["id"], after[0]["id"]);
    assert_eq!(before[0]["pid"], after[0]["pid"]);
    fixture
        .control("raw echo marker-while-detached\\r")
        .unwrap();
    fixture.wait_for(|s| {
        s.control("snapshot text")
            .unwrap()
            .contains("marker-while-detached")
    });
    let mut second = fixture.attach(90, 20);
    second.text("marker-while-detached");
    second
        .pty
        .resize(Size {
            cols: 80,
            rows: 18,
            cell_width: 8,
            cell_height: 16,
        })
        .unwrap();
    fixture.wait_for(|s| {
        let snap: serde_json::Value =
            serde_json::from_str(&s.control("snapshot json").unwrap()).unwrap();
        snap["cols"] == 80 && snap["rows"] == 18
    });
    let replacement = fixture.directory.join("new.kdl");
    fs::write(
        &replacement,
        "splash_ms 0\ntheme {frame_focus \"#ff0000\"}\n",
    )
    .unwrap();
    fs::rename(&replacement, &fixture.config).unwrap();
    fixture.wait_for(|s| {
        let snap: serde_json::Value =
            serde_json::from_str(&s.control("snapshot json").unwrap()).unwrap();
        snap["styles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|run| run["y"] == 2 && run["fg"] == "#ff0000")
    });
    second.detach();
    fixture.control("quit").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture.server.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "server failed to stop");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        fs::symlink_metadata(fixture.socket()).is_err_and(|e| e.kind() == io::ErrorKind::NotFound),
        "session socket was not cleaned up"
    );
}
