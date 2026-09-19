//! `hwp edit` twice-run byte-equality gate (#253).
//!
//! Each test here runs the identical `hwp edit` invocation twice inside one test session and
//! asserts the two outputs are byte-identical. That proves determinism within a session; it is
//! a necessary but not sufficient proxy for the across-days drift issue #253 actually measured
//! (two regenerations from the same commit, separated by real wall-clock time, produced
//! different bytes with zero content difference), because a calendar-driven field cannot be
//! varied inside a single process run. The real proof is the double regeneration with a process
//! restart between runs, recorded in `04.1-01-determinism-evidence.md`.
//!
//! Deliberately does NOT assert on page counts or glyph metrics (CI render fonts differ from
//! local ones) and deliberately does NOT skip when a fixture is missing — every input here is
//! built from an in-test markdown source and an in-test generated PNG, so there is no fixture to
//! be missing.

use std::path::{Path, PathBuf};
use std::process::Command;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn test_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "hwp-cli-edit-determinism-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn write_md(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

/// Byte-for-byte comparison with a diagnosable failure: on a mismatch, report both lengths and
/// the offset of the first differing byte, so a future regression here is legible rather than
/// just red.
fn assert_bytes_eq(a_path: &Path, b_path: &Path, context: &str) {
    let a = std::fs::read(a_path).unwrap_or_else(|e| panic!("{context}: read {a_path:?}: {e}"));
    let b = std::fs::read(b_path).unwrap_or_else(|e| panic!("{context}: read {b_path:?}: {e}"));
    if a == b {
        return;
    }
    let first_diff = a
        .iter()
        .zip(b.iter())
        .position(|(x, y)| x != y)
        .unwrap_or_else(|| a.len().min(b.len()));
    panic!(
        "{context}: {a_path:?} ({} bytes) != {b_path:?} ({} bytes) — first differing byte at offset {first_diff}",
        a.len(),
        b.len()
    );
}

fn new_from(md: &Path, out: &Path, extra_args: &[&str]) {
    let mut cmd = hwp();
    cmd.args(["new", "--from"]).arg(md).arg("-o").arg(out);
    cmd.args(extra_args);
    let status = cmd.status().unwrap();
    assert!(status.success(), "hwp new --from {md:?} -o {out:?} failed");
}

/// A small opaque-red-circle-ish RGBA PNG, written directly (no shell-out to the Python
/// generator) — same shape as D1's seal fixture, dimensions kept small for test speed.
fn write_seal_png(path: &Path) {
    use image::ImageEncoder as _;
    let size: u32 = 16;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            rgba[idx] = 218;
            rgba[idx + 1] = 32;
            rgba[idx + 2] = 32;
            rgba[idx + 3] = 255;
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&rgba, size, size, image::ExtendedColorType::Rgba8)
        .expect("PNG 인코딩 실패");
    std::fs::write(path, out.into_inner()).unwrap();
}

const APPROVAL_MD: &str = "# 결재 문서\n\n결재란: (인)\n";

/// The hwpx appended-entry path (`crates/hwpx/src/patch.rs::process_package_with_appends`'s
/// `appends` loop): `hwp edit --seal` appends a new BinData entry for the seal image. Before the
/// fix, that loop's zip write options omitted the fixed DOS ZIP timestamp the sibling
/// transformed-entry branch already pins, so two identical invocations produced different bytes.
#[test]
fn seal_edit_byte_stable_across_two_runs_hwpx() {
    let dir = test_dir("hwpx-seal");
    let md = write_md(&dir, "doc.md", APPROVAL_MD);
    let base = dir.join("base.hwpx");
    new_from(&md, &base, &[]);

    let png = dir.join("seal.png");
    write_seal_png(&png);
    let seal_arg = format!("(인)=>{}@18mm", png.display());

    let out_a = dir.join("out-a.hwpx");
    let out_b = dir.join("out-b.hwpx");

    let mut runs = [&out_a, &out_b].into_iter();
    let first = runs.next().unwrap();
    let status = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(first)
        .args(["--seal", &seal_arg])
        .status()
        .unwrap();
    assert!(status.success(), "hwp edit --seal -> {first:?} failed");

    // The bug this test targets is a DOS ZIP timestamp (2-second granularity) leaking the wall
    // clock into a freshly appended entry. Two invocations issued back-to-back can land in the
    // same 2-second bucket even with the bug present, which would make this test pass vacuously
    // regardless of the fix. Sleep past that granularity so an unfixed appends loop is actually
    // exercised with a different "now".
    std::thread::sleep(std::time::Duration::from_millis(2_500));

    let second = runs.next().unwrap();
    let status = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(second)
        .args(["--seal", &seal_arg])
        .status()
        .unwrap();
    assert!(status.success(), "hwp edit --seal -> {second:?} failed");

    assert_bytes_eq(&out_a, &out_b, "hwpx --seal: run A vs run B");
}
