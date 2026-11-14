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

use serde_json::Value;

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
