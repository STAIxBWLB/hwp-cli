//! v1 baseline for `hwp cat --with-segments`.
//!
//! What this pins: the exact bytes today's default `--with-segments` run emits, plus the
//! behaviours around it — the format rejection matrix, the `markdown` field equalling the
//! plain markdown output, and the span invariants (sorted, non-overlapping, offsets as
//! Unicode scalars).
//!
//! Why the pin had to be generated first: D-04 promises the v0.8.x default output stays
//! byte-identical for one release after the segment model v2 lands. A golden taken from a
//! build that already carries v2 code records whatever that build emits and proves nothing.
//! `tests/golden/segment-envelope-v1.json` was therefore generated from
//! `fixtures/samples/report-tables.hwpx` at commit ac4b019 (origin/main), before a single
//! line of v2 code existed, and committed on its own ahead of every other change in phase 05.
//! It is never regenerated: a later regeneration would silently weaken D-04 rather than fail.
//!
//! The source sample is `fixtures/samples/report-tables.hwpx`, which is committed (the
//! `fixtures/samples/` exception in CLAUDE.md's data policy). So these tests fail loudly when
//! something is wrong instead of skipping the way the gitignored-corpus tests do. It carries
//! Korean text and tables, so the pin is not ASCII-only.
//!
//! Plan 05-05 extends this file with the v2 assertions and flips the `json` row of the
//! rejection matrix to accepted per D-03. Capturing the current matrix here makes that flip a
//! visible one-line diff rather than an invisible behaviour change.

use std::path::PathBuf;
use std::process::Command;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn sample() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/samples/report-tables.hwpx")
}

/// Raw stdout of the default `--with-segments` run.
fn segments_stdout() -> Vec<u8> {
    let out = hwp()
        .arg("cat")
        .arg(sample())
        .args(["--format", "markdown", "--with-segments"])
        .output()
        .expect("run hwp cat --with-segments");
    assert!(
        out.status.success(),
        "hwp cat --with-segments failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// The D-04 pin: a fresh run must be byte-identical to the committed golden.
#[test]
fn envelope_matches_the_v1_golden_byte_for_byte() {
    let golden = include_bytes!("golden/segment-envelope-v1.json");
    let fresh = segments_stdout();
    assert!(
        !golden.is_empty(),
        "the golden is empty; it must hold the pre-v2 envelope bytes"
    );
    if fresh != golden {
        // Show where they diverge rather than dumping 26 KB of JSON twice.
        let at = fresh
            .iter()
            .zip(golden.iter())
            .position(|(a, b)| a != b)
            .unwrap_or_else(|| fresh.len().min(golden.len()));
        let from = at.saturating_sub(60);
        panic!(
            "default --with-segments output moved; D-04 pins it byte-identical.\n\
             first difference at byte {at} (fresh {} bytes, golden {} bytes)\n\
             fresh : ...{}\n\
             golden: ...{}",
            fresh.len(),
            golden.len(),
            String::from_utf8_lossy(&fresh[from..(at + 60).min(fresh.len())]),
            String::from_utf8_lossy(&golden[from..(at + 60).min(golden.len())]),
        );
    }
}

/// The envelope's `markdown` field equals the plain `--format markdown` output. This is the
/// structural guarantee the single-emission-core design gives: the segment map is derived from
/// the same emission run, not from a second one.
#[test]
fn envelope_markdown_field_equals_plain_markdown_output() {
    let env: serde_json::Value =
        serde_json::from_slice(&segments_stdout()).expect("parse the envelope");
    let md = env["markdown"].as_str().expect("markdown field");

    let plain = hwp()
        .arg("cat")
        .arg(sample())
        .args(["--format", "markdown"])
        .output()
        .expect("run hwp cat --format markdown");
    assert!(plain.status.success(), "hwp cat --format markdown failed");
    let plain_md = String::from_utf8(plain.stdout).expect("markdown output is utf-8");

    assert_eq!(
        md, plain_md,
        "the markdown field must equal the plain output"
    );
}

/// The v1 rejection matrix. `--with-segments` is markdown-only and incompatible with
/// `--preview`. 05-05 flips the `json` row per D-03; that flip should show up here as a diff.
#[test]
fn with_segments_is_rejected_outside_markdown_and_with_preview() {
    for format in ["plain", "html", "csv", "json"] {
        let out = hwp()
            .arg("cat")
            .arg(sample())
            .args(["--format", format, "--with-segments"])
            .output()
            .expect("run hwp cat");
        assert!(
            !out.status.success(),
            "--with-segments --format {format} must be rejected in v1"
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("--with-segments는 --format markdown 전용입니다"),
            "--format {format} error text changed: {err}"
        );
    }

    let out = hwp()
        .arg("cat")
        .arg(sample())
        .args(["--format", "markdown", "--with-segments", "--preview"])
        .output()
        .expect("run hwp cat --preview");
    assert!(
        !out.status.success(),
        "--with-segments --preview must be rejected"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("--preview와 함께 쓸 수 없습니다"),
        "--preview error text changed: {err}"
    );
}

/// Offsets are Unicode scalar positions, not byte positions, and the spans are sorted and
/// non-overlapping. The sample is Korean, so a byte-offset regression fails here rather than
/// passing silently the way it would on ASCII.
#[test]
fn spans_are_sorted_non_overlapping_scalar_offsets() {
    let env: serde_json::Value =
        serde_json::from_slice(&segments_stdout()).expect("parse the envelope");
    let md = env["markdown"].as_str().expect("markdown field");
    assert_ne!(
        md.len(),
        md.chars().count(),
        "the sample must contain non-ASCII text, otherwise scalar-vs-byte offsets are \
         indistinguishable here"
    );

    let scalars = md.chars().count();
    let segments = env["segments"].as_array().expect("segments array");
    assert!(!segments.is_empty(), "the sample must produce segments");

    let mut prev_end = 0usize;
    for s in segments {
        let start = s["start"].as_u64().expect("start is a scalar") as usize;
        let end = s["end"].as_u64().expect("end is a scalar") as usize;
        assert!(start < end, "empty or inverted span [{start},{end})");
        assert!(
            end <= scalars,
            "span end {end} past the markdown's {scalars} scalars (byte offsets?)"
        );
        assert!(
            start >= prev_end,
            "spans must be sorted and non-overlapping: {start} < {prev_end}"
        );
        prev_end = end;
    }
}
