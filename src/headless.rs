//! Scriptable front end with the same compositor and PTYs as an attached session.
use crate::{app::App, input::Parser, screen::Screen};
use std::{
    ffi::OsString,
    io::{self, BufRead, Write},
};
pub fn run(
    argv: Vec<OsString>,
    cols: u16,
    rows: u16,
    idle_ms: u64,
    script: bool,
    layout: Option<&std::path::Path>,
) -> io::Result<()> {
    let mut app = App::new(argv, cols, rows)?;
    if let Some(file) = layout
        && let Err(e) = app.apply_layout_file(file, true, true)
    {
        eprintln!("slush: {}: {e}", file.display());
    }
    let mut screen = Screen::new(cols, rows);
    let mut input = Parser::default();
    let mut out = io::stdout().lock();
    if !script {
        app.settle(idle_ms, Some(&mut screen));
        if !app.should_quit() {
            app.compose(&mut screen);
        }
        out.write_all(screen.dump().as_bytes())?;
        return Ok(());
    }
    app.compose(&mut screen);
    let mut stdin = io::stdin().lock();
    let mut frame = Vec::with_capacity(4095);
    while read_frame(&mut stdin, &mut frame)? {
        let line = &frame;
        let end = line
            .iter()
            .position(|b| matches!(b, 0 | b'\r' | b'\n'))
            .unwrap_or(line.len());
        let line = &line[..end];
        if line.is_empty() {
            continue;
        }
        if let Some(ms) = settle_ms(line, idle_ms) {
            app.settle(ms, None);
            continue;
        }
        match app.command(&mut screen, &mut input, line) {
            Some(reply) if !reply.is_empty() => {
                out.write_all(&reply)?;
                out.write_all(b"\n")?;
                out.flush()?;
            }
            None => {
                let mut stderr = io::stderr().lock();
                stderr.write_all(b"slush: unknown command: ")?;
                stderr.write_all(line)?;
                stderr.write_all(b"\n")?;
            }
            _ => {}
        }
        if app.quit {
            break;
        }
    }
    Ok(())
}

/// Match fgets(line, 4096): dispatch a full frame without waiting for a newline.
fn read_frame(reader: &mut impl BufRead, frame: &mut Vec<u8>) -> io::Result<bool> {
    frame.clear();
    while frame.len() < 4095 {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        let chunk = &available[..available.len().min(4095 - frame.len())];
        let newline = chunk.iter().position(|b| *b == b'\n');
        let count = newline.map_or(chunk.len(), |n| n + 1);
        frame.extend_from_slice(&chunk[..count]);
        reader.consume(count);
        if newline.is_some() {
            break;
        }
    }
    Ok(!frame.is_empty())
}

fn settle_ms(line: &[u8], idle_ms: u64) -> Option<u64> {
    line.strip_prefix(b"settle")?;
    if line.get(6) != Some(&b' ') {
        return Some(idle_ms);
    }
    let mut argument = line[7..].to_vec();
    argument.push(0);
    // SAFETY: argument owns a NUL-terminated byte string for the duration of atoi.
    let quiet = unsafe { libc::atoi(argument.as_ptr().cast()) };
    // Negative quiet times and zero both break after the first polling cycle.
    Some(quiet.max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn bounded_frames_survive_short_reads_native_bytes_and_eof() {
        let mut source = vec![b'x'; 4094];
        source.extend_from_slice(b"\xff\xfe\n\0alive\nalive");
        let mut reader = BufReader::with_capacity(7, Cursor::new(source));
        let mut frame = Vec::new();
        assert!(read_frame(&mut reader, &mut frame).unwrap());
        assert_eq!(frame.len(), 4095);
        assert_eq!(frame.last(), Some(&0xff));
        assert!(read_frame(&mut reader, &mut frame).unwrap());
        assert_eq!(frame, b"\xfe\n");
        assert!(read_frame(&mut reader, &mut frame).unwrap());
        assert_eq!(frame, b"\0alive\n");
        assert!(read_frame(&mut reader, &mut frame).unwrap());
        assert_eq!(frame, b"alive");
        assert!(!read_frame(&mut reader, &mut frame).unwrap());
    }

    #[test]
    fn settling_uses_the_c_prefix_delimiter_and_decimal_prefix() {
        for (line, expected) in [
            (b"settle".as_slice(), 300),
            (b"settled 12", 300),
            (b"settle12", 300),
            (b"settle\t12", 300),
            (b"settle 12tail", 12),
            (b"settle \x0b+12\xff", 12),
            (b"settle -12", 0),
            (b"settle invalid", 0),
        ] {
            assert_eq!(settle_ms(line, 300), Some(expected));
        }
        assert_eq!(settle_ms(b"alive", 300), None);
    }
}
