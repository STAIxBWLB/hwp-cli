//! `hwp compare` exit-code and read-only integration tests (03-04-PLAN.md, Task 4).
//!
//! Unlike the inline module tests in `commands/compare.rs`, these spawn the
//! compiled binary as a subprocess: the exit-code contract (D-13) lives in the
//! process dispatch (`main.rs`'s `Cmd::Compare` arm), and cannot be observed
//! from inside the library.
//!
//! Inputs are `.json` IR documents (`hwp_convert::to_json`), not `.hwp`/`.hwpx`
//! containers — `load_document_with_options` treats `.json` as an IR
//! pass-through, so no document writer is needed to build fixtures for these
//! tests either.

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest as _, Sha256};

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

/// A fresh, test-scoped temp directory — cleared at the start so PID reuse or
/// a previous run's leftovers cannot contaminate byte-count/file-count
/// assertions.
fn tmp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("hwp-cli-compare-exit-codes-{}", std::process::id()))
        .join(test);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_json_doc(path: &Path, markdown: &str) {
    let doc = hwp_convert::from_markdown::from_markdown(markdown);
    let json = hwp_convert::to_json(&doc, true, false).unwrap();
    std::fs::write(path, json).unwrap();
}

/// Same as [`write_json_doc`], plus one extra paragraph appended inside the
/// first table's `(row, col)` cell — the #223 shape, where the two documents'
/// top-level paragraphs are identical and the only difference is in a cell.
fn write_json_doc_with_cell_paragraph(path: &Path, markdown: &str, row: u16, col: u16, text: &str) {
    use hwp_model::{Control, HwpChar};

    let mut doc = hwp_convert::from_markdown::from_markdown(markdown);
    let mut done = false;
    for para in doc
        .sections
        .iter_mut()
        .flat_map(|s| s.paragraphs.iter_mut())
    {
        for control in &mut para.controls {
            if let Control::Table(table) = control {
                let cell = table
                    .cells
                    .iter_mut()
                    .find(|c| c.row == row && c.col == col)
                    .expect("셀이 있어야 함");
                let mut added = cell.paragraphs[0].clone();
                added.chars = text.chars().map(HwpChar::Text).collect();
                cell.paragraphs.push(added);
                done = true;
                break;
            }
        }
        if done {
            break;
        }
    }
    assert!(done, "표가 있는 문서여야 함");
    std::fs::write(path, hwp_convert::to_json(&doc, true, false).unwrap()).unwrap();
}

fn sha256(path: &Path) -> Vec<u8> {
    Sha256::digest(std::fs::read(path).unwrap()).to_vec()
}

#[test]
fn identical_documents_exit_zero() {
    let dir = tmp_dir("identical");
    let a = dir.join("a.json");
    let b = dir.join("b.json");
    write_json_doc(&a, "첫 문단\n\n둘째 문단\n");
    write_json_doc(&b, "첫 문단\n\n둘째 문단\n");

    let status = hwp().arg("compare").arg(&a).arg(&b).status().unwrap();
    assert_eq!(status.code(), Some(0));
}

#[test]
fn differing_documents_exit_one() {
    let dir = tmp_dir("differing");
    let a = dir.join("a.json");
    let b = dir.join("b.json");
    write_json_doc(&a, "첫 문단\n");
    write_json_doc(&b, "고유텍스트마커삽입됨\n");

    let output = hwp().arg("compare").arg(&a).arg(&b).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("고유텍스트마커삽입됨"),
        "stdout에 변경된 문단 텍스트가 있어야 함: {stdout}"
    );
}

/// #223: a paragraph added inside a table cell used to print as a blank line,
/// because the report re-looked-up the text in a flat list of top-level
/// paragraphs while the index came from the engine's deep walk. The entry now
/// carries its own text and its own location, so the line names the cell.
#[test]
fn added_cell_paragraph_prints_its_cell_and_its_text() {
    let dir = tmp_dir("cell_paragraph");
    let a = dir.join("a.json");
    let b = dir.join("b.json");
    let table = "본문 문단\n\n| 항목 | 내용 |\n| - | - |\n| 하나 | 둘 |\n";
    write_json_doc(&a, table);
    write_json_doc_with_cell_paragraph(&b, table, 1, 1, "○ 둘째 항목 BBB");

    let output = hwp().arg("compare").arg(&a).arg(&b).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("표 0 셀 (1,1) 문단 1"),
        "stdout에 셀 위치가 있어야 함: {stdout}"
    );
    assert!(
        stdout.contains("○ 둘째 항목 BBB"),
        "stdout에 추가된 문단 텍스트가 있어야 함: {stdout}"
    );
}

/// Catches a regression where the "differences found" path starts returning
/// an `Err` and collapses exit codes 1 and 2 into one.
#[test]
fn unreadable_input_exits_two() {
    let dir = tmp_dir("unreadable");
    let a = dir.join("a.json");
    write_json_doc(&a, "첫 문단\n");
    let missing = dir.join("does-not-exist.hwpx");

    let output = hwp().arg("compare").arg(&a).arg(&missing).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("동일합니다") && !stdout.contains('+') && !stdout.contains('-'),
        "실행 실패 경로는 차이 발견 리포트를 출력하면 안 됨: {stdout}"
    );
    // The exit-2 diagnostic must read like every other command's failed run —
    // the `Error: ...` line anyhow's `Termination` impl prints for a returned
    // `Err`, not a command-specific prefix.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("Error: "),
        "실행 실패는 다른 명령과 같은 `Error:` 형식이어야 함: {stderr}"
    );
}

#[test]
fn compare_leaves_inputs_byte_identical() {
    let dir = tmp_dir("byte_identical");
    let a = dir.join("a.json");
    let b = dir.join("b.json");
    write_json_doc(&a, "첫 문단\n");
    write_json_doc(&b, "첫 문단\n\n둘째 문단\n");

    let a_before = sha256(&a);
    let b_before = sha256(&b);
    let files_before: std::collections::BTreeSet<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();

    for format in ["text", "json"] {
        let status = hwp()
            .current_dir(&dir)
            .arg("compare")
            .arg(&a)
            .arg(&b)
            .args(["--format", format])
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(1));
    }

    assert_eq!(sha256(&a), a_before, "a.json 바이트가 변경됨");
    assert_eq!(sha256(&b), b_before, "b.json 바이트가 변경됨");
    let files_after: std::collections::BTreeSet<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        files_before, files_after,
        "작업 디렉터리에 새 파일이 생기면 안 됨"
    );
}

#[test]
fn json_format_exit_code_matches_text_format() {
    let dir = tmp_dir("format_parity");
    let a = dir.join("a.json");
    let b = dir.join("b.json");
    write_json_doc(&a, "첫 문단\n");
    write_json_doc(&b, "다른 문단\n");

    let text_status = hwp()
        .arg("compare")
        .arg(&a)
        .arg(&b)
        .args(["--format", "text"])
        .status()
        .unwrap();
    let json_status = hwp()
        .arg("compare")
        .arg(&a)
        .arg(&b)
        .args(["--format", "json"])
        .status()
        .unwrap();
    assert_eq!(text_status.code(), json_status.code());
    assert_eq!(text_status.code(), Some(1));
}

#[test]
fn self_comparison_exits_zero() {
    let dir = tmp_dir("self_comparison");
    let a = dir.join("a.json");
    write_json_doc(&a, "첫 문단\n\n둘째 문단\n");

    let status = hwp().arg("compare").arg(&a).arg(&a).status().unwrap();
    assert_eq!(status.code(), Some(0));
}
