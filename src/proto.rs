//! Slosh's client/server framing: u8 type, u32 big-endian length, payload.
use std::{
    ffi::{OsStr, OsString},
    io::{self, Read, Write},
    path::PathBuf,
};

pub const HELLO: u8 = 1;
pub const INPUT: u8 = 2;
pub const RESIZE: u8 = 3;
pub const DETACH: u8 = 4;
pub const OUTPUT: u8 = 5;
pub const EXIT: u8 = 6;
pub const CMD: u8 = 7;
pub const REPLY: u8 = 8;
pub const HEADER: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub kind: u8,
    pub payload: Vec<u8>,
}

#[derive(Default)]
pub struct Reader {
    buffer: Vec<u8>,
    offset: usize,
}
impl Reader {
    pub fn feed(&mut self, bytes: &[u8]) {
        if self.offset != 0 {
            self.buffer.drain(..self.offset);
            self.offset = 0;
        }
        self.buffer.extend_from_slice(bytes);
    }
    pub fn pending(&self) -> &[u8] {
        &self.buffer[self.offset..]
    }
}
impl Iterator for Reader {
    type Item = Message;
    fn next(&mut self) -> Option<Message> {
        let bytes = &self.buffer[self.offset..];
        if bytes.len() < HEADER {
            return None;
        }
        let length = u32::from_be_bytes(bytes[1..5].try_into().unwrap()) as usize;
        let total = HEADER.checked_add(length)?;
        if bytes.len() < total {
            return None;
        }
        let message = Message {
            kind: bytes[0],
            payload: bytes[HEADER..total].to_vec(),
        };
        self.offset += total;
        Some(message)
    }
}

pub fn send(writer: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let length = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame exceeds u32 length"))?;
    let length = length.to_be_bytes();
    writer.write_all(&[kind, length[0], length[1], length[2], length[3]])?;
    writer.write_all(payload)
}

/// Read a one-shot control reply, skipping other message types as the C client does.
pub fn control(stream: &mut (impl Read + Write), command: &[u8]) -> io::Result<Vec<u8>> {
    send(stream, CMD, command)?;
    let mut reader = Reader::default();
    let mut buffer = [0; 65536];
    loop {
        let count = match stream.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "session closed without replying",
            ));
        }
        reader.feed(&buffer[..count]);
        for message in reader.by_ref() {
            if message.kind == REPLY {
                return Ok(message.payload);
            }
        }
    }
}

pub fn session_dir() -> io::Result<PathBuf> {
    #[cfg(unix)]
    let directory = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|dir| !dir.is_empty())
        .map(|dir| PathBuf::from(dir).join("slosh"))
        .unwrap_or_else(|| {
            // SAFETY: getuid takes no arguments and has no failure mode.
            PathBuf::from(format!("/tmp/slosh-{}", unsafe { libc::getuid() }))
        });
    #[cfg(windows)]
    let directory = std::env::var_os("LOCALAPPDATA")
        .filter(|dir| !dir.is_empty())
        .or_else(|| std::env::var_os("TEMP").filter(|dir| !dir.is_empty()))
        .map(|dir| PathBuf::from(dir).join("slosh"))
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA and TEMP are unset")
        })?;
    let builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    let mut builder = builder;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    Ok(directory)
}
pub fn socket_path(name: impl AsRef<OsStr>) -> io::Result<PathBuf> {
    session_path(name.as_ref(), ".sock")
}
pub fn log_path(name: impl AsRef<OsStr>) -> io::Result<PathBuf> {
    session_path(name.as_ref(), ".log")
}
fn session_path(name: &OsStr, extension: &str) -> io::Result<PathBuf> {
    // The C protocol appends the name verbatim. Path::join would interpret a
    // leading slash as a new root and discard the session directory.
    let mut path = session_dir()?.into_os_string();
    path.push("/");
    path.push(name);
    path.push(extension);
    Ok(path.into())
}
pub fn sessions() -> io::Result<Vec<OsString>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(session_dir()?)? {
        let entry = entry?;
        if let Some(name) = session_name(entry.file_name()) {
            names.push(name);
        }
    }
    Ok(names)
}

fn session_name(filename: OsString) -> Option<OsString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        filename
            .as_bytes()
            .strip_suffix(b".sock")
            .filter(|name| !name.is_empty())
            .map(|name| OsString::from_vec(name.to_vec()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let wide: Vec<_> = filename.encode_wide().collect();
        let suffix: Vec<_> = ".sock".encode_utf16().collect();
        wide.strip_suffix(suffix.as_slice())
            .filter(|name| !name.is_empty())
            .map(OsString::from_wide)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_survive_every_read_boundary() {
        let expected = [
            Message {
                kind: CMD,
                payload: b"{\"cmd\":\"alive\"}".to_vec(),
            },
            Message {
                kind: DETACH,
                payload: Vec::new(),
            },
            Message {
                kind: OUTPUT,
                payload: vec![0, 1, 255],
            },
        ];
        let mut bytes = Vec::new();
        for message in &expected {
            send(&mut bytes, message.kind, &message.payload).unwrap();
        }
        for boundary in 0..=bytes.len() {
            let mut reader = Reader::default();
            reader.feed(&bytes[..boundary]);
            let mut messages: Vec<_> = reader.by_ref().collect();
            reader.feed(&bytes[boundary..]);
            messages.extend(reader.by_ref());
            assert_eq!(messages, expected, "boundary {boundary}");
        }
    }

    #[test]
    fn lying_lengths_wait_without_allocating_declared_payload() {
        let mut reader = Reader::default();
        reader.feed(&[CMD, 255, 255, 255, 255, 1, 2, 3]);
        assert!(reader.next().is_none());
        assert_eq!(reader.pending().len(), 8);
    }

    #[test]
    fn original_protocol_seed_corpus_survives_chunked_reads() {
        let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/reference/slosh/tests/fuzz/corpus/proto");
        for path in std::fs::read_dir(corpus).unwrap() {
            let bytes = std::fs::read(path.unwrap().path()).unwrap();
            let mut reader = Reader::default();
            let mut offset = 0;
            while offset < bytes.len() {
                let count = (bytes[offset] as usize % 37 + 1).min(bytes.len() - offset);
                reader.feed(&bytes[offset..offset + count]);
                offset += count;
                for message in reader.by_ref() {
                    assert!(message.payload.len() <= bytes.len());
                }
            }
        }
    }
}
