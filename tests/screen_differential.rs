//! The C application is linked only into this independent reference oracle.
#![cfg(unix)]
use slush::screen::{Color, Cursor, Screen};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn compositor_matches_original_bytes_and_snapshots() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let slosh = root.join("tests/reference/slosh");
    let out = root.join("target/compat");
    fs::create_dir_all(&out).unwrap();
    let oracle = out.join("screen_oracle");
    let result = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()))
        .arg("-I")
        .arg(slosh.join("include"))
        .arg("-I")
        .arg(PathBuf::from(env!("SLUSH_GHOSTTY_INCLUDE_DIR")))
        .arg(root.join("tests/screen_oracle.c"))
        .arg(slosh.join("src/screen.c"))
        .arg(slosh.join("src/json.c"))
        .arg(PathBuf::from(env!("SLUSH_GHOSTTY_LIB_DIR")).join("libghostty-vt.a"))
        .args(["-lm", "-lpthread", "-o"])
        .arg(&oracle)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "oracle compilation failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );

    let mut screen = Screen::new(20, 8);
    let mut trace = String::new();
    let mut expected = Vec::new();
    let mut seed = 0x197fcafeu32;
    let texts = ["a", "日", "🔔", "e\u{301}", "─", "👨‍👩‍👧‍👦"];
    for frame in 0..150 {
        if frame % 23 == 0 {
            let cols = 12 + (frame % 10) as u16;
            let rows = 4 + (frame % 5) as u16;
            trace += &format!("size {cols} {rows}\n");
            screen.resize(cols, rows);
        }
        if frame % 17 == 0 {
            trace += "clear\n";
            screen.clear();
        }
        for _ in 0..5 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let x = (seed % screen.cols as u32) as u16;
            let y = ((seed >> 8) % screen.rows as u32) as u16;
            let text = texts[((seed >> 16) as usize) % texts.len()];
            let attrs = ((seed >> 24) & 255) as u16;
            let rgb = if seed & 1 == 0 {
                -1
            } else {
                (seed & 0xffffff) as i32
            };
            let color = if rgb < 0 {
                Color::default()
            } else {
                Color::rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
            };
            trace += &format!("put {x} {y} {attrs} {rgb} {}\n", hex(text.as_bytes()));
            screen.put(x, y, text.as_bytes(), color, Color::default(), attrs);
            if seed.is_multiple_of(3) {
                trace += &format!("link {x} {y}\n");
                let id = screen.link_id("https://example.test");
                screen.set_link(x, y, id);
            }
        }
        screen.cursor = Cursor {
            visible: frame % 3 != 0,
            x: (frame % screen.cols as usize) as u16,
            y: (frame % screen.rows as usize) as u16,
        };
        trace += &format!(
            "cursor {} {} {}\n",
            u8::from(screen.cursor.visible),
            screen.cursor.x,
            screen.cursor.y
        );
        if frame % 7 == 0 {
            trace += "hit 1 1 2 2\n";
            screen.hits.add(1, 1, 2, 2, "test:action");
        }
        trace += "render\nsnapshot\n";
        expected.push(hex(screen.render()));
        expected.push(screen.snapshot().to_string());
    }
    let input = out.join("screen-trace.txt");
    fs::write(&input, trace).unwrap();
    let result = Command::new(oracle)
        .stdin(Stdio::from(fs::File::open(input).unwrap()))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "oracle failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output = String::from_utf8(result.stdout).unwrap();
    let actual: Vec<_> = output.lines().collect();
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        if index % 2 == 0 {
            assert_eq!(*actual, expected, "terminal diff at frame {}", index / 2);
        } else {
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(actual).unwrap(),
                serde_json::from_str::<serde_json::Value>(&expected).unwrap(),
                "snapshot at frame {}",
                index / 2
            );
        }
    }
}
