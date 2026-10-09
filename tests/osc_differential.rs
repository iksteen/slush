//! Compare report parsing with the original implementation and feed boundaries
//! with its hostile-input corpus. C is compiled only for this test oracle.
use slush::osc::{Report, Scanner};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Stdio},
};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|c| format!("{c:02x}")).collect()
}
#[test]
fn reports_match_reference_and_scanner_preserves_read_boundaries() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let reference = root.join("tests/reference/slosh");
    let out = root.join("target/compat");
    fs::create_dir_all(&out).unwrap();
    let oracle = out.join("osc_oracle");
    let compiled = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()))
        .arg("-I")
        .arg(reference.join("include"))
        .arg(root.join("tests/osc_oracle.c"))
        .arg(reference.join("src/osc5577.c"))
        .arg("-o")
        .arg(&oracle)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let fields = [
        "state=working",
        "state=done",
        "state=blocked",
        "state=clear",
        "state=idle",
        "state=error",
        "state=future",
        "id=build/test",
        "id=a//b",
        "id=",
        "app=cargo",
        "app=bad/name",
        "app=tool+1",
        "kind=permission",
        "kind=future",
        "progress=100",
        "progress=101",
        "progress=01",
        "progress=-1",
        "msg=aGVsbG8=",
        "msg=Y2Fmw6k=",
        "msg=YcKbYg==",
        "msg=YQ",
        "msg=YR",
        "msg=x",
        "msg=abc=def",
        "title=YnVpbGQ=",
        "future=a+b",
        " BAD = value ",
        "nonsense",
        "=empty",
        "keylongerthansixteen=5",
    ];
    let mut cases: Vec<Vec<u8>> = vec![
        Vec::new(),
        b"state=working:future=\xff".to_vec(),
        b"state=working\n".to_vec(),
        b"\nstate=working".to_vec(),
        b"state=done:\t app = cargo \t".to_vec(),
        b"state=blocked:id=build\0test".to_vec(),
        format!("state=working:msg={}", "YQ==".repeat(700)).into_bytes(),
    ];
    let mut seed = 0x75015577u32;
    for _ in 0..1500 {
        let mut pairs = Vec::new();
        for _ in 0..8 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            pairs.push(fields[seed as usize % fields.len()]);
        }
        cases.push(pairs.join(":").into_bytes());
    }
    let corpus = reference.join("tests/fuzz/corpus/osc5577");
    for file in fs::read_dir(corpus).unwrap() {
        let bytes = fs::read(file.unwrap().path()).unwrap();
        let whole = Scanner::default().feed(&bytes);
        for cut in 0..=bytes.len() {
            let mut scanner = Scanner::default();
            let mut parts = scanner.feed(&bytes[..cut]);
            parts.extend(scanner.feed(&bytes[cut..]));
            assert_eq!(parts, whole, "scanner split at byte {cut}");
        }
        cases.push(bytes);
    }
    let input = out.join("osc-trace.txt");
    fs::write(
        &input,
        cases
            .iter()
            .map(|bytes| format!("{}\n", hex(bytes)))
            .collect::<String>(),
    )
    .unwrap();
    let result = Command::new(oracle)
        .stdin(Stdio::from(fs::File::open(input).unwrap()))
        .output()
        .unwrap();
    assert!(result.status.success());
    let output = String::from_utf8(result.stdout).unwrap();
    let actual: Vec<_> = output.lines().collect();
    assert_eq!(actual.len(), cases.len());
    for (case, actual) in cases.iter().zip(actual) {
        let expected = Report::parse(case).map_or_else(
            || "invalid".into(),
            |r| {
                format!(
                    "{}|{}|{}|{}|{}|{}|{}",
                    r.state.name(),
                    r.kind,
                    r.progress,
                    hex(r.id.as_bytes()),
                    hex(r.app.as_bytes()),
                    hex(r.title.as_bytes()),
                    hex(r.msg.as_bytes())
                )
            },
        );
        assert_eq!(
            actual,
            expected,
            "pairs {:?}",
            String::from_utf8_lossy(case)
        );
    }
}
