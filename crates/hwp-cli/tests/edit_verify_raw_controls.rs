//! #578: `edit --verify` on HWP5 documents whose body holds raw-backed controls (text boxes,
//! headers, shapes). The writer gives every paragraph it re-emits a non-zero PARA_HEADER
//! instance id, including paragraphs inside those controls whose source bytes carry 0; the
//! verifier compared the raw bytes and refused a correct edit. Every assertion here is on
//! structure or text, never on glyphs, fonts or page counts.
//!
//! `fixtures/hwp5/` is local-only (data policy), so the sample tests skip through
//! `fixture_missing` without it. The first test builds its own raw-backed text box and always runs.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

#[path = "common/fixture_skip.rs"]
mod fixture_skip;
#[path = "common/temp_path.rs"]
mod temp_path;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ahwp"))
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run(args: &[&std::ffi::OsStr]) -> Output {
    hwp().args(args).output().unwrap()
}

fn ops_file(dir: &Path) -> PathBuf {
    let path = dir.join("ops.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&json!([{
            "op": "insert_blocks",
            "address": {"at": {"section": 0, "paragraph": 3}},
            "position": "after",
            "blocks": [{"type": "paragraph", "runs": [{"type": "text", "text": "probe"}]}]
        }]))
        .unwrap(),
    )
    .unwrap();
    path
}

fn plain_text(path: &Path) -> String {
    let out = hwp().arg("cat").arg(path).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// `insert_blocks` with `--verify` succeeds on the sample, publishes the probe paragraph, and
/// the output re-reads and validates.
fn assert_insert_verifies(sample: &str) {
    let input = repo().join("fixtures/hwp5").join(sample);
    if fixture_skip::fixture_missing(&input) {
        return;
    }
    let dir = temp_path::TempPath::new("hwp578", sample);
    let _ = std::fs::remove_dir_all(&*dir);
    std::fs::create_dir_all(&*dir).unwrap();
    let ops = ops_file(&dir);
    let output = dir.join("out.hwp");
    let report = dir.join("report.json");
    let edited = run(&[
        "edit".as_ref(),
        input.as_os_str(),
        "-o".as_ref(),
        output.as_os_str(),
        "--ops".as_ref(),
        ops.as_os_str(),
        "--verify".as_ref(),
        "--report".as_ref(),
        report.as_os_str(),
    ]);
    assert!(
        edited.status.success(),
        "{sample}: {}",
        String::from_utf8_lossy(&edited.stderr)
    );
    let report: Value = serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    assert_eq!(report["applied_count"], 1, "{report}");
    assert!(
        report.get("refusal").is_none() || report["refusal"].is_null(),
        "{report}"
    );
    assert!(
        plain_text(&output).contains("probe"),
        "{sample}: the inserted paragraph is published"
    );
    let valid = run(&["validate".as_ref(), output.as_os_str()]);
    assert!(
        valid.status.success(),
        "{}",
        String::from_utf8_lossy(&valid.stderr)
    );
}

#[test]
fn insert_blocks_verifies_on_annual_report() {
    assert_insert_verifies("annual_report.hwp");
}

#[test]
fn insert_blocks_verifies_on_work_report() {
    assert_insert_verifies("work_report.hwp");
}

/// `--replace` of text that lives in a top-level paragraph verifies and lands in the output,
/// on a document that also holds raw-backed controls.
#[test]
fn replace_in_a_top_level_paragraph_verifies_on_work_report() {
    let input = repo().join("fixtures/hwp5/work_report.hwp");
    if fixture_skip::fixture_missing(&input) {
        return;
    }
    let dir = temp_path::TempPath::new("hwp578", "replace");
    let _ = std::fs::remove_dir_all(&*dir);
    std::fs::create_dir_all(&*dir).unwrap();
    let output = dir.join("out.hwp");
    let edited = run(&[
        "edit".as_ref(),
        input.as_os_str(),
        "-o".as_ref(),
        output.as_os_str(),
        "--replace".as_ref(),
        "주간업무보고서=>주간업무보고서X".as_ref(),
        "--verify".as_ref(),
    ]);
    assert!(
        edited.status.success(),
        "{}",
        String::from_utf8_lossy(&edited.stderr)
    );
    assert!(plain_text(&output).contains("주간업무보고서X"));
}

/// The text of the issue's `--replace` example lives only inside a raw-backed text box. The
/// HWP5 writer emits that subtree verbatim, so the edit is not in the output file: `--verify`
/// must refuse it with a structured code and the invariant that differs, and publish nothing.
/// (Without `--verify` this edit is silently dropped; that is a separate writer gap.)
#[test]
fn replace_confined_to_a_raw_text_box_is_refused_with_verify_mismatch() {
    let input = repo().join("fixtures/hwp5/annual_report.hwp");
    if fixture_skip::fixture_missing(&input) {
        return;
    }
    let dir = temp_path::TempPath::new("hwp578", "rawbox");
    let _ = std::fs::remove_dir_all(&*dir);
    std::fs::create_dir_all(&*dir).unwrap();
    let output = dir.join("out.hwp");
    let report = dir.join("report.json");
    let edited = run(&[
        "edit".as_ref(),
        input.as_os_str(),
        "-o".as_ref(),
        output.as_os_str(),
        "--replace".as_ref(),
        "문서는=>문서는X".as_ref(),
        "--verify".as_ref(),
        "--report".as_ref(),
        report.as_os_str(),
    ]);
    assert!(
        !edited.status.success(),
        "the edit is not in the output, so verify must refuse"
    );
    assert!(!output.exists(), "a refused run publishes nothing");
    let report: Value = serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    let schema: Value =
        serde_json::from_str(include_str!("../../../schemas/edit-report-v1.schema.json")).unwrap();
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .unwrap();
    assert!(validator.is_valid(&report), "{report}");
    assert_eq!(report["refusal"]["code"], "verify_mismatch", "{report}");
    let counts = report["refusal"]["values"]["counts"].as_array().unwrap();
    assert!(
        counts.iter().any(|count| count["name"] == "text_chars"
            && count["expected"].as_u64() > count["actual"].as_u64()),
        "the differing invariant is named: {report}"
    );
}
