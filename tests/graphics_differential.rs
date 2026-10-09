//! Compare transactional wire output with slosh's unchanged C graphics emitter.
use slush::graphics::{Geometry, Graphics, Request};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
};

#[test]
fn frames_match_reference_across_drops_replacements_and_repaints() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out = root.join("target/compat");
    fs::create_dir_all(&out).unwrap();
    let oracle = out.join("graphics_oracle");
    let result = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()))
        .arg("-I")
        .arg(root.join("tests/reference/slosh/include"))
        .arg(root.join("tests/graphics_oracle.c"))
        .arg(root.join("tests/reference/slosh/src/graphics.c"))
        .arg("-o")
        .arg(&oracle)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut state = Graphics::default();
    let mut trace = String::new();
    let mut expected = Vec::new();
    let mut seed = 0xb107cafeu32;
    for frame in 0..500 {
        if frame % 29 == 0 {
            state.reset();
            trace += "reset\n";
        }
        if frame % 11 == 0 {
            state.repaint();
            trace += "repaint\n";
        }
        state.begin();
        trace += "begin\n";
        // Repeated identities, independent panes, cut pieces, zero extents,
        // missing payloads, and multipart transmissions in one stream.
        for slot in 0..7 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            if seed & 3 == 0 {
                continue;
            }
            let length = [0, 1, 2, 3, 2304, 2305, 7687][(seed as usize >> 12) % 7];
            let g = Geometry {
                col: ((seed >> 5) % 90) as u16,
                row: ((seed >> 12) % 30) as u16,
                cols: ((seed >> 19) % 50 + 1) as u16,
                rows: ((seed >> 25) % 12 + 1) as u16,
                scale_cols: if seed & 8 != 0 { 15 } else { 0 },
                scale_rows: if seed & 16 != 0 { 7 } else { 0 },
                x_off: (seed >> 4) % 8,
                y_off: (seed >> 16) % 16,
                sx: seed % 4,
                sy: (seed >> 12) % 4,
                sw: (seed >> 16) % 100,
                sh: (seed >> 25) % 20,
            };
            let r = Request {
                pane: slot % 3 + 1,
                src_id: slot % 2 + 7,
                generation: (frame / 9) as u64,
                place_id: slot % 3,
                piece: (slot % 2) as u16,
                geometry: g,
                px_w: 32,
                px_h: 12,
                format: (seed % 3) as i32,
                compression: ((seed >> 8) & 1) as i32,
                data: (0..length)
                    .map(|i| seed.wrapping_add(i * 17) as u8)
                    .collect::<Vec<_>>()
                    .into(),
                ..Default::default()
            };
            trace += &format!(
                "place {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}\n",
                r.pane,
                r.src_id,
                r.generation,
                r.place_id,
                r.piece,
                g.col,
                g.row,
                g.cols,
                g.rows,
                g.scale_cols,
                g.scale_rows,
                g.x_off,
                g.y_off,
                g.sx,
                g.sy,
                g.sw,
                g.sh,
                r.px_w,
                r.px_h,
                r.format,
                r.compression,
                seed,
                length
            );
            state.place(&r);
        }
        trace += "flush\n";
        expected.push(
            state
                .flush()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
        );
        let model = state.json();
        expected.push(
            model
                .as_array()
                .unwrap()
                .iter()
                .map(|p| {
                    format!(
                        "{},{},{},{},{},{},{},{},{};",
                        p["image"],
                        p["placement"],
                        p["piece"],
                        p["x"],
                        p["y"],
                        p["cols"],
                        p["rows"],
                        p["x_off"],
                        p["y_off"]
                    )
                })
                .collect::<String>(),
        );
        if frame % 4 != 0 {
            let delivered = frame % 5 != 0;
            state.commit(delivered);
            trace += &format!("commit {}\n", u8::from(delivered));
            if delivered && frame % 17 == 0 {
                state.forget_pane(2);
                trace += "forget 2\n";
            }
        }
    }
    let input = out.join("graphics-trace.txt");
    fs::write(&input, trace).unwrap();
    let result = Command::new(oracle)
        .stdin(Stdio::from(fs::File::open(input).unwrap()))
        .output()
        .unwrap();
    assert!(result.status.success(), "oracle exit: {:?}", result.status);
    let output = String::from_utf8(result.stdout).unwrap();
    let actual: Vec<_> = output.lines().collect();
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(
            *actual,
            expected,
            "{} at frame {}",
            if index % 2 == 0 {
                "wire bytes"
            } else {
                "placement model"
            },
            index / 2
        );
    }
}
