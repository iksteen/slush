//! Fixed configuration text buffers compared with the unchanged C loader.
#![cfg(unix)]
use slush::config::Config;
use std::{fs, path::PathBuf, process::Command};

fn quote(value: &[u8]) -> Vec<u8> {
    let mut out = vec![b'"'];
    for b in value {
        if matches!(b, b'"' | b'\\') {
            out.push(b'\\');
        }
        out.push(*b);
    }
    out.push(b'"');
    out
}

#[test]
fn configured_bytes_and_buffer_limits_match_original_loader() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let slosh = root.join("tests/reference/slosh");
    let out = root.join("target/compat/config-text");
    fs::create_dir_all(&out).unwrap();
    let oracle = out.join("oracle");
    let mut compile = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()));
    compile
        .arg("-I")
        .arg(slosh.join("include"))
        .arg("-I")
        .arg(PathBuf::from(env!("SLUSH_GHOSTTY_INCLUDE_DIR")))
        .arg(root.join("tests/config_text_oracle.c"));
    for module in ["config", "kdl", "path", "expr", "shader", "screen", "json"] {
        compile.arg(slosh.join(format!("src/{module}.c")));
    }
    let result = compile
        .arg(PathBuf::from(env!("SLUSH_GHOSTTY_LIB_DIR")).join("libghostty-vt.a"))
        .args(["-lm", "-lpthread", "-o"])
        .arg(&oracle)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let names = [
        "zoom_mark",
        "zoom_on_mark",
        "close_mark",
        "min_mark",
        "newtab_mark",
        "bell_mark",
        "busy_mark",
        "word_separators",
    ];
    let mut cases = vec![
        vec![],
        b"plain".to_vec(),
        b"\xff\xfe marks".to_vec(),
        b"quotes\"and\\slashes".to_vec(),
    ];
    for length in [30, 31, 32, 62, 63, 64, 96] {
        cases.push(vec![b'x'; length]);
        let mut value = vec![b'x'; length];
        value.extend_from_slice("é🚀".as_bytes());
        cases.push(value);
    }
    cases.push(["é".repeat(40).as_bytes(), b"\xff"].concat());
    cases.push([vec![b'x'; 31], vec![0x80, 0x81, b'y']].concat());
    for (i, value) in cases.iter().enumerate() {
        let file = out.join(format!("{i}.kdl"));
        let source: Vec<_> = names
            .iter()
            .flat_map(|name| [name.as_bytes(), b" ", &quote(value), b"\n"].concat())
            .collect();
        fs::write(&file, source).unwrap();
        let expected = Command::new(&oracle).arg(&file).output().unwrap();
        assert!(expected.status.success(), "{i}: {:?}", expected.stderr);
        let mut config = Config::default();
        assert!(config.load(&file));
        assert!(config.messages.is_empty(), "{i}: {:?}", config.messages);
        let mut actual = String::new();
        for name in names {
            for byte in config.text_bytes(name).unwrap() {
                use std::fmt::Write;
                write!(actual, "{byte:02x}").unwrap();
            }
            actual.push('\n');
        }
        assert_eq!(actual.as_bytes(), expected.stdout, "case {i}: {value:?}");
        let saved = out.join(format!("{i}-saved.kdl"));
        fs::write(&saved, config.render_bytes()).unwrap();
        let reread = Command::new(&oracle).arg(&saved).output().unwrap();
        assert!(reread.status.success(), "saved {i}: {:?}", reread.stderr);
        assert_eq!(reread.stdout, expected.stdout, "saved case {i}");
    }
}
