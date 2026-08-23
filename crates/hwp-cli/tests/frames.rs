//! `hwp new --doc-head`/`--doc-foot` frame tracer (GONG-03 plan 01, D-01/D-02/D-03/D-04).
//!
//! Drives the real binary via `CARGO_BIN_EXE_hwp` against temp dirs only — no fixtures, no fonts,
//! no network, mirroring `tests/skill_templates.rs`'s harness style.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use hwp_model::{Control, Document, HwpChar, Paragraph};

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn temp_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "hwp-cli-frames-{label}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn command_output(command: &mut Command, label: &str) -> Output {
    command
        .output()
        .unwrap_or_else(|error| panic!("{label}: {error}"))
}

fn assert_success(output: &Output, label: &str) {
    assert!(
        output.status.success(),
        "{label} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn reread(path: &Path) -> Document {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("hwp") => hwp5::read_document(path).unwrap().document,
        Some("hwpx") => hwpx::read_document(path).unwrap().document,
        other => panic!("unsupported test extension: {other:?}"),
    }
}

/// Text of a paragraph's own characters only (no recursion).
fn paragraph_text(paragraph: &Paragraph) -> String {
    paragraph
        .chars
        .iter()
        .filter_map(|c| match c {
            HwpChar::Text(ch) => Some(*ch),
            _ => None,
        })
        .collect()
}

/// Flattens paragraph text and table-cell text into one document-order sequence of non-empty
/// blocks (D-03: block order over paragraphs and table controls together, not paragraph order).
/// Recurses into `Control::Table` cells and `Control::Generic` paragraph lists.
fn collect_blocks(paragraphs: &[Paragraph], out: &mut Vec<String>) {
    for paragraph in paragraphs {
        let text = paragraph_text(paragraph);
        if !text.is_empty() {
            out.push(text);
        }
        for control in &paragraph.controls {
            match control {
                Control::Table(table) => {
                    for cell in &table.cells {
                        collect_blocks(&cell.paragraphs, out);
                    }
                }
                Control::Generic(generic) => {
                    for list in &generic.paragraph_lists {
                        collect_blocks(&list.paragraphs, out);
                    }
                }
                _ => {}
            }
        }
    }
}

/// The document's block text sequence in document order (D-03 helper, reused by later plans in
/// this phase for the full criterion-1 ordering assertion).
fn block_text_sequence(document: &Document) -> Vec<String> {
    let mut out = Vec::new();
    for section in &document.sections {
        collect_blocks(&section.paragraphs, &mut out);
    }
    out
}

/// Index of the first block whose text contains `needle`, panicking with the full sequence on a
/// miss (clearer failure than a bare `None`).
fn index_of(blocks: &[String], needle: &str) -> usize {
    blocks
        .iter()
        .position(|block| block.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not found in block sequence: {blocks:?}"))
}

#[test]
fn tracer_doc_head_becomes_paragraph_zero_and_reopens() {
    let dir = temp_dir("tracer");
    let body = dir.join("body.md");
    std::fs::write(&body, "본문 첫 줄입니다.\n").unwrap();
    let out = dir.join("out.hwpx");

    let output = command_output(
        hwp().args([
            "new",
            "--preset",
            "official",
            "--from",
        ])
        .arg(&body)
        .args(["--doc-head", "기관명=예시대학교"])
        .args(["--doc-head", "수신=총장"])
        .args(["-o"])
        .arg(&out),
        "hwp new --doc-head",
    );
    assert_success(&output, "hwp new --doc-head");

    let document = reread(&out);
    let paragraph_zero = &document.sections[0].paragraphs[0];
    // The 두문 table is paragraph 0 (Pattern 5 / D-02).
    assert!(
        paragraph_zero
            .controls
            .iter()
            .any(|control| matches!(control, Control::Table(_))),
        "paragraph 0 must carry the 두문 table control"
    );
    // ...and it still received the section/page-margin controls (secd/cold), proving the
    // architectural bet this tracer exists to test.
    assert!(
        paragraph_zero
            .controls
            .iter()
            .any(|control| matches!(control, Control::SectionDef(_))),
        "paragraph 0 must still carry the section-def control after a 두문 table is prepended"
    );

    let blocks = block_text_sequence(&document);
    let agency = index_of(&blocks, "예시대학교");
    let recipient = index_of(&blocks, "수신  총장");
    let body_idx = index_of(&blocks, "본문 첫 줄입니다.");
    assert!(agency < recipient, "blocks: {blocks:?}");
    assert!(recipient < body_idx, "blocks: {blocks:?}");

    // The file reopens cleanly (hwp validate exit 0), confirming Hancom-shaped bytes.
    let validate = command_output(hwp().arg("validate").arg(&out), "hwp validate");
    assert_success(&validate, "hwp validate");
}

#[test]
fn doc_head_value_containing_equals_is_preserved() {
    let dir = temp_dir("equals-in-value");
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "official"])
            .args(["--doc-head", "수신=총장=대리"])
            .args(["-o"])
            .arg(&out),
        "hwp new --doc-head with = in value",
    );
    assert_success(&output, "hwp new --doc-head with = in value");

    let document = reread(&out);
    let blocks = block_text_sequence(&document);
    assert!(
        blocks.iter().any(|block| block == "수신  총장=대리"),
        "blocks: {blocks:?}"
    );
}

#[test]
fn malformed_doc_head_spec_without_equals_fails_closed() {
    let dir = temp_dir("malformed");
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "official"])
            .args(["--doc-head", "기관명"])
            .args(["-o"])
            .arg(&out),
        "hwp new --doc-head (no =)",
    );
    assert!(!output.status.success(), "malformed spec must fail closed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("기관명"), "stderr: {stderr}");
    assert!(!out.exists(), "no file should be written on a hard error");
}

#[test]
fn unknown_doc_head_key_fails_closed() {
    let dir = temp_dir("unknown-key");
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "official"])
            .args(["--doc-head", "없는키=x"])
            .args(["-o"])
            .arg(&out),
        "hwp new --doc-head with unknown key",
    );
    assert!(!output.status.success(), "unknown key must fail closed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("없는키=x"), "stderr: {stderr}");
}

#[test]
fn no_frame_flag_leaves_paragraph_zero_untouched() {
    let dir = temp_dir("no-frame");
    let out = dir.join("out.hwpx");
    let output = command_output(
        hwp()
            .args(["new", "--preset", "official"])
            .args(["-o"])
            .arg(&out),
        "hwp new (no frame flag)",
    );
    assert_success(&output, "hwp new (no frame flag)");

    let document = reread(&out);
    let paragraph_zero = &document.sections[0].paragraphs[0];
    assert!(
        !paragraph_zero
            .controls
            .iter()
            .any(|control| matches!(control, Control::Table(_))),
        "no frame flag must not add a table to paragraph 0"
    );
    let blocks = block_text_sequence(&document);
    assert!(
        !blocks.iter().any(|block| block.contains("예시대학교")),
        "blocks: {blocks:?}"
    );
}

#[test]
fn all_five_frame_flags_appear_in_help() {
    let output = command_output(hwp().args(["new", "--help"]), "hwp new --help");
    assert_success(&output, "hwp new --help");
    let help = String::from_utf8_lossy(&output.stdout);
    for flag in [
        "--doc-head",
        "--doc-foot",
        "--notice-head",
        "--notice-foot",
        "--press-head",
    ] {
        assert!(help.contains(flag), "help missing {flag}:\n{help}");
    }
}
