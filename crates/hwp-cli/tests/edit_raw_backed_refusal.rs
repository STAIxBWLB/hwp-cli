//! #577: an edit the HWP5 writer cannot apply must be refused, never reported as success.
//!
//! The HWP5 writer emits a raw-backed control (a text box, a footnote or endnote, a header or
//! footer read from a saved file) from its source records, while the reader mirrors the same
//! paragraphs into `paragraph_lists` and every edit changes only that mirror. So an edit whose
//! target lies inside one exits 0 and the output keeps the old text. Without `--verify` nothing
//! says so.
//!
//! The committed input is built at run time: `fixtures/samples/report-tables.hwpx` converted to
//! HWP5 holds three raw-backed footnote/endnote controls. The local-only `annual_report.hwp`
//! (text boxes) is guarded with `fixture_missing`. Assertions are on codes, structure and text,
//! never on glyphs.
use std::ffi::OsStr;
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

fn run(args: &[&OsStr]) -> Output {
    hwp().args(args).output().unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn workdir(name: &str) -> temp_path::TempPath {
    let dir = temp_path::TempPath::new("hwp577", name);
    let _ = std::fs::remove_dir_all(&*dir);
    std::fs::create_dir_all(&*dir).unwrap();
    dir
}

/// The committed sample as an HWP5 file. Its footnotes and endnote are raw-backed controls.
fn hwp5_with_raw_backed_notes(dir: &Path) -> PathBuf {
    let sample = repo().join("fixtures/samples/report-tables.hwpx");
    let hwp5 = dir.join("input.hwp");
    let out = run(&[
        "convert".as_ref(),
        sample.as_os_str(),
        "-o".as_ref(),
        hwp5.as_os_str(),
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    hwp5
}

fn validator() -> jsonschema::Validator {
    let schema: Value =
        serde_json::from_str(include_str!("../../../schemas/edit-report-v1.schema.json")).unwrap();
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .unwrap()
}

fn read_report(path: &Path) -> Value {
    let report: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(validator().is_valid(&report), "{report}");
    report
}

fn edit(input: &Path, output: &Path, report: &Path, args: &[&str]) -> Output {
    let mut command: Vec<&OsStr> = vec![
        "edit".as_ref(),
        input.as_os_str(),
        "-o".as_ref(),
        output.as_os_str(),
    ];
    command.extend(args.iter().map(OsStr::new));
    command.extend(["--report".as_ref(), report.as_os_str()]);
    run(&command)
}

fn text(path: &Path) -> String {
    let out = run(&["cat".as_ref(), path.as_os_str()]);
    assert!(out.status.success(), "{}", stderr(&out));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The refusal a dropped edit must carry: code, the control kind and count, and a message that
/// names the cause and a next step.
fn assert_raw_backed_refusal(report: &Value, kind: &str) {
    let refusal = &report["refusal"];
    assert_eq!(refusal["code"], "raw_backed_edit", "{report}");
    let controls = refusal["values"]["controls"].as_array().unwrap();
    assert!(
        controls
            .iter()
            .any(|control| control["kind"] == kind && control["count"].as_u64() >= Some(1)),
        "expected a {kind} control in {report}"
    );
    let message = refusal["message"].as_str().unwrap();
    assert_eq!(message, hwp_cli::refusal::fixed_message("raw_backed_edit"));
    assert!(
        message.contains(".hwpx") && message.contains("한글"),
        "the message names a next step: {message}"
    );
}

/// The core defect: a `--replace` whose only match is inside a footnote used to exit 0 and
/// leave the old text. It is refused now, with and without `--verify`, and publishes nothing.
#[test]
fn replace_inside_a_footnote_is_refused_not_reported_as_success() {
    let dir = workdir("footnote");
    let input = hwp5_with_raw_backed_notes(&dir);
    for verify in [false, true] {
        let output = dir.join(format!("out-{verify}.hwp"));
        let report = dir.join(format!("report-{verify}.json"));
        let mut args = vec!["--replace", "각주 예시 1=>각주 예시 X"];
        if verify {
            args.push("--verify");
        }
        let edited = edit(&input, &output, &report, &args);
        assert!(
            !edited.status.success(),
            "verify={verify}: the edit cannot be written, so it must not succeed: {}",
            stderr(&edited)
        );
        assert!(
            !output.exists(),
            "verify={verify}: a refused edit publishes nothing"
        );
        assert_raw_backed_refusal(&read_report(&report), "footnote");
    }
}

/// The same edit is written when the output is `.hwpx`, which emits from the edited paragraphs,
/// so the refusal is specific to the HWP5 writer and the message's next step is real.
#[test]
fn the_same_footnote_edit_to_hwpx_is_written() {
    let dir = workdir("hwpx");
    let input = hwp5_with_raw_backed_notes(&dir);
    let output = dir.join("out.hwpx");
    let report = dir.join("report.json");
    let edited = edit(
        &input,
        &output,
        &report,
        &["--replace", "각주 예시 1=>각주 예시 X"],
    );
    assert!(edited.status.success(), "{}", stderr(&edited));
    assert!(text(&output).contains("각주 예시 X"));
}

/// An edit that does not touch a raw-backed paragraph is unaffected, on a document that holds
/// raw-backed controls: body text and metadata still publish and the body text lands.
#[test]
fn edits_outside_raw_backed_controls_still_publish() {
    let dir = workdir("outside");
    let input = hwp5_with_raw_backed_notes(&dir);
    let output = dir.join("out.hwp");
    let report = dir.join("report.json");
    let edited = edit(
        &input,
        &output,
        &report,
        &["--replace", "예시 문장 2입니다=>예시 문장 2X입니다"],
    );
    assert!(edited.status.success(), "{}", stderr(&edited));
    assert!(text(&output).contains("예시 문장 2X입니다"));
    let notes_intact = text(&output).contains("각주 예시 1");
    assert!(notes_intact, "the footnote text is unchanged");

    let meta_output = dir.join("meta.hwp");
    let edited = edit(
        &input,
        &meta_output,
        &dir.join("meta.json"),
        &["--set-meta", "title=probe"],
    );
    assert!(edited.status.success(), "{}", stderr(&edited));
}

/// A text box in a real Hancom document (`annual_report.hwp`, local-only): the issue's own
/// kind of target.
#[test]
fn replace_inside_a_text_box_of_a_real_document_is_refused() {
    let input = repo().join("fixtures/hwp5/annual_report.hwp");
    if fixture_skip::fixture_missing(&input) {
        return;
    }
    let dir = workdir("textbox");
    let output = dir.join("out.hwp");
    let report = dir.join("report.json");
    let edited = edit(&input, &output, &report, &["--replace", "Vision=>VisionX"]);
    assert!(!edited.status.success(), "{}", stderr(&edited));
    assert!(!output.exists());
    assert_raw_backed_refusal(&read_report(&report), "shape_or_text_box");
}

/// Runs `edit --ops` with a typed batch against `input` and returns (output, report).
fn edit_ops(input: &Path, dir: &Path, label: &str, ops: Value) -> (Output, PathBuf, PathBuf) {
    let ops_path = dir.join(format!("{label}.ops.json"));
    std::fs::write(&ops_path, serde_json::to_vec(&ops).unwrap()).unwrap();
    let output = dir.join(format!("{label}.hwp"));
    let report = dir.join(format!("{label}.report.json"));
    let edited = edit(
        input,
        &output,
        &report,
        &["--ops", ops_path.to_str().unwrap()],
    );
    (edited, output, report)
}

/// A structural op that moves every later paragraph down by one and is legal: after paragraph
/// 1, which holds plain text. (Before paragraph 0 would refuse as `section_owner_insert`, since
/// paragraph 0 holds the section definition, and never reach the check under test.)
fn insert_at_top() -> Value {
    json!({"op":"insert_blocks","address":{"at":{"section":0,"paragraph":1}},
        "position":"after",
        "blocks":[{"type":"paragraph","runs":[{"type":"text","text":"앞쪽 삽입"}]}]})
}

fn footnote_replace() -> Value {
    json!({"op":"replace","from":"각주 예시 1","to":"각주 예시 X"})
}

/// #577 review (a): a structural op in the same plan moves every later paragraph, so the
/// changed footnote is no longer at the index it had. It must still be refused. Pairing the
/// original and edited paragraphs by position compared it with a paragraph that holds no raw
/// control, found nothing, and let the run exit 0 with the old note text.
#[test]
fn a_structural_op_before_a_footnote_edit_does_not_hide_it() {
    let dir = workdir("mixed");
    let input = hwp5_with_raw_backed_notes(&dir);
    let (edited, output, report) = edit_ops(
        &input,
        &dir,
        "mixed",
        json!([insert_at_top(), footnote_replace()]),
    );
    assert!(
        !edited.status.success(),
        "the footnote edit is dropped, so the run must not succeed: {}",
        stderr(&edited)
    );
    assert!(!output.exists(), "a refused edit publishes nothing");
    assert_raw_backed_refusal(&read_report(&report), "footnote");
}

/// #577 review (b): a body-only structural edit in front of paragraphs that hold footnotes must
/// publish. The sample has a footnote in paragraph 4 and an endnote in paragraph 7; inserting or
/// deleting one paragraph in front of them shifts both by one, and positional pairing compared
/// each note with the other and refused an edit that never touched a note.
#[test]
fn a_body_only_structural_edit_in_front_of_notes_publishes() {
    let dir = workdir("structural");
    let input = hwp5_with_raw_backed_notes(&dir);
    let plans = [
        ("insert", json!([insert_at_top()])),
        (
            "delete",
            json!([{"op":"delete_para","address":{"at":{"section":0,"paragraph":2}}}]),
        ),
    ];
    for (label, ops) in plans {
        let (edited, output, report) = edit_ops(&input, &dir, label, ops);
        assert!(
            edited.status.success(),
            "{label}: a body-only structural edit must publish: {}",
            stderr(&edited)
        );
        assert!(output.exists(), "{label}: published");
        let report = read_report(&report);
        assert!(
            report.get("refusal").is_none() || report["refusal"].is_null(),
            "{label}: {report}"
        );
        let body = text(&output);
        assert!(
            body.contains("각주 예시 1"),
            "{label}: the footnote text is untouched"
        );
        assert!(
            body.contains("미주 예시 1"),
            "{label}: the endnote text is untouched"
        );
    }
}

/// The committed sample with its endnote paragraph moved directly after the footnote paragraph,
/// so two adjacent paragraphs each hold a raw-backed note (paragraphs 4 and 5).
fn hwp5_with_adjacent_notes(dir: &Path) -> PathBuf {
    let base = hwp5_with_raw_backed_notes(dir);
    let json_path = dir.join("adjacent.json");
    let out = run(&[
        "convert".as_ref(),
        base.as_os_str(),
        "-o".as_ref(),
        json_path.as_os_str(),
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let mut doc: Value = serde_json::from_slice(&std::fs::read(&json_path).unwrap()).unwrap();
    let paragraphs = doc["sections"][0]["paragraphs"].as_array_mut().unwrap();
    let endnote = paragraphs.remove(7);
    paragraphs.insert(5, endnote);
    std::fs::write(&json_path, serde_json::to_vec(&doc).unwrap()).unwrap();
    let moved = dir.join("adjacent.hwp");
    let out = run(&[
        "new".as_ref(),
        "--from".as_ref(),
        json_path.as_os_str(),
        "-o".as_ref(),
        moved.as_os_str(),
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    moved
}

/// #577 review (b), the sharper case: with two notes in adjacent paragraphs, one inserted
/// paragraph in front of them moves both by one, so positional pairing compared the footnote
/// with the endnote, saw a difference, and refused an edit that never touched a note.
#[test]
fn a_structural_edit_in_front_of_adjacent_notes_is_not_refused() {
    let dir = workdir("adjacent");
    let input = hwp5_with_adjacent_notes(&dir);
    for (label, ops) in [
        ("insert", json!([insert_at_top()])),
        (
            "delete",
            json!([{"op":"delete_para","address":{"at":{"section":0,"paragraph":2}}}]),
        ),
    ] {
        let (edited, output, report) = edit_ops(&input, &dir, label, ops);
        assert!(
            edited.status.success(),
            "{label}: the notes are untouched, so this must publish: {}",
            stderr(&edited)
        );
        assert!(output.exists(), "{label}: published");
        let report = read_report(&report);
        assert!(
            report.get("refusal").is_none() || report["refusal"].is_null(),
            "{label}: {report}"
        );
    }
}

/// A move reorders paragraphs too. Moving a note's paragraph and editing the other note's text
/// in the same plan is still refused: the edited note is found wherever it now sits.
#[test]
fn a_moved_note_paragraph_does_not_hide_an_edit_to_a_note() {
    let dir = workdir("moved");
    let input = hwp5_with_adjacent_notes(&dir);
    let (edited, output, report) = edit_ops(
        &input,
        &dir,
        "moved",
        json!([
            {"op":"move_para","address":{"at":{"section":0,"paragraph":5}},
                "to":{"address":{"at":{"section":0,"paragraph":2}},"position":"before"}},
            footnote_replace()
        ]),
    );
    assert!(!edited.status.success(), "{}", stderr(&edited));
    assert!(!output.exists());
    assert_raw_backed_refusal(&read_report(&report), "footnote");
}
