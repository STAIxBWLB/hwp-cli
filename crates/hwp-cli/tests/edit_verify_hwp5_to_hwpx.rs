//! #583: `edit` from an HWP5 input to `.hwpx`.
//!
//! Before this change such an edit ran no loss accounting at all, so it published losses
//! silently (`convert` to the same output reports them), and `--verify` refused even a
//! `--set-meta` because the HWP5 IR and the HWPX re-read differ in format-specific fields.
//! The committed input is derived at run time from `fixtures/samples/report-tables.hwpx`, so
//! the tests always run. The local-only samples under `fixtures/hwp5/` are guarded with
//! `fixture_missing`. Every assertion is on structure, codes and counts, never on glyphs.
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
    let dir = temp_path::TempPath::new("hwp583", name);
    let _ = std::fs::remove_dir_all(&*dir);
    std::fs::create_dir_all(&*dir).unwrap();
    dir
}

/// An HWP5 document whose metadata carries sub-second timestamps, built from the committed
/// sample: HWPX timestamps are second-precision, so writing it as HWPX always reports
/// `metadata_value_removed` (the one event a plain cross-format save produces).
fn hwp5_with_subsecond_times(dir: &Path) -> PathBuf {
    let sample = repo().join("fixtures/samples/report-tables.hwpx");
    // HWPX to HWP5 first: the JSON IR of an HWPX source carries HWPX-only data that `new`
    // refuses, while the JSON of an HWP5 document round-trips.
    let native = dir.join("native.hwp");
    let out = run(&[
        "convert".as_ref(),
        sample.as_os_str(),
        "-o".as_ref(),
        native.as_os_str(),
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let json_path = dir.join("doc.json");
    let out = run(&[
        "convert".as_ref(),
        native.as_os_str(),
        "-o".as_ref(),
        json_path.as_os_str(),
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let mut doc: Value = serde_json::from_slice(&std::fs::read(&json_path).unwrap()).unwrap();
    doc["metadata"]["create_time"] = json!(133_087_540_838_260_000_u64);
    doc["metadata"]["modify_time"] = json!(133_089_921_037_570_000_u64);
    std::fs::write(&json_path, serde_json::to_vec(&doc).unwrap()).unwrap();
    let hwp5 = dir.join("input.hwp");
    let out = run(&[
        "new".as_ref(),
        "--from".as_ref(),
        json_path.as_os_str(),
        "-o".as_ref(),
        hwp5.as_os_str(),
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    hwp5
}

fn schema_validator() -> jsonschema::Validator {
    let schema: Value =
        serde_json::from_str(include_str!("../../../schemas/edit-report-v1.schema.json")).unwrap();
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .unwrap()
}

fn read_report(path: &Path) -> Value {
    let report: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(schema_validator().is_valid(&report), "{report}");
    report
}

fn event_codes(report: &Value) -> Vec<String> {
    report["preservation"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .map(|event| event["code"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn edit_set_meta(input: &Path, output: &Path, report: &Path, extra: &[&str]) -> Output {
    let mut args: Vec<&OsStr> = vec![
        "edit".as_ref(),
        input.as_os_str(),
        "-o".as_ref(),
        output.as_os_str(),
        "--set-meta".as_ref(),
        "title=probe".as_ref(),
        "--report".as_ref(),
        report.as_os_str(),
    ];
    args.extend(extra.iter().map(OsStr::new));
    run(&args)
}

/// The silent loss: without `--verify`, an HWP5 to HWPX edit now names its losses in the
/// report, the same event `convert --loss-report` names for the same pair. It publishes: a
/// cross-format edit follows non-strict `convert` and never refuses for these events.
#[test]
fn hwp5_to_hwpx_edit_without_verify_reports_and_publishes_its_losses() {
    let dir = workdir("silent");
    let input = hwp5_with_subsecond_times(&dir);
    let output = dir.join("out.hwpx");
    let report = dir.join("report.json");
    let edited = edit_set_meta(&input, &output, &report, &[]);
    assert!(edited.status.success(), "{}", stderr(&edited));
    assert!(
        output.exists(),
        "metadata_value_removed alone must not block publication"
    );
    let report = read_report(&report);
    assert_eq!(report["schema_version"], "1.4", "{report}");
    assert_eq!(event_codes(&report), ["metadata_value_removed"], "{report}");
    assert_eq!(
        report["preservation"][0],
        json!({"code": "metadata_value_removed", "resource": "metadata",
               "disposition": "removed", "count": 2})
    );
    // The same events reach stderr, as for convert.
    assert!(
        stderr(&edited).contains("metadata_value_removed"),
        "{}",
        stderr(&edited)
    );
    // And they agree with what `convert` reports for the same pair.
    let loss = dir.join("loss.json");
    let converted = run(&[
        "convert".as_ref(),
        input.as_os_str(),
        "-o".as_ref(),
        dir.join("conv.hwpx").as_os_str(),
        "--loss-report".as_ref(),
        loss.as_os_str(),
    ]);
    assert!(converted.status.success(), "{}", stderr(&converted));
    let loss: Value = serde_json::from_slice(&std::fs::read(&loss).unwrap()).unwrap();
    assert_eq!(
        report["preservation"], loss["events"],
        "edit and convert account alike"
    );
}

/// `--verify` from HWP5 to HWPX passes when every difference is a format projection or is
/// covered by a reported event, and the run reports those events.
#[test]
fn hwp5_to_hwpx_set_meta_verifies_and_reports() {
    let dir = workdir("verify");
    let input = hwp5_with_subsecond_times(&dir);
    let output = dir.join("out.hwpx");
    let report = dir.join("report.json");
    let edited = edit_set_meta(&input, &output, &report, &["--verify"]);
    assert!(edited.status.success(), "{}", stderr(&edited));
    assert!(output.exists());
    assert_eq!(
        event_codes(&read_report(&report)),
        ["metadata_value_removed"]
    );
}

/// A top-level `--replace` verifies too, and its text is in the output.
#[test]
fn hwp5_to_hwpx_replace_verifies() {
    let dir = workdir("replace");
    let input = hwp5_with_subsecond_times(&dir);
    let output = dir.join("out.hwpx");
    let edited = run(&[
        "edit".as_ref(),
        input.as_os_str(),
        "-o".as_ref(),
        output.as_os_str(),
        "--replace".as_ref(),
        "예시 문장=>예시 문장X".as_ref(),
        "--verify".as_ref(),
    ]);
    assert!(edited.status.success(), "{}", stderr(&edited));
    let text = run(&["cat".as_ref(), output.as_os_str()]);
    assert!(String::from_utf8_lossy(&text.stdout).contains("예시 문장X"));
}

/// The HWP-output path is unchanged: refuse-on-event stays for `.hwp` output, and `--verify`
/// still passes with no event. (Same input, same edit, `.hwp` output.)
#[test]
fn hwp5_to_hwp_verify_path_is_unchanged() {
    let dir = workdir("hwp-out");
    let input = hwp5_with_subsecond_times(&dir);
    let output = dir.join("out.hwp");
    let report = dir.join("report.json");
    let edited = edit_set_meta(&input, &output, &report, &["--verify"]);
    assert!(edited.status.success(), "{}", stderr(&edited));
    assert!(event_codes(&read_report(&report)).is_empty());
}

/// An HWP5 document with one control the HWPX writer cannot represent (an unknown `xxxx`
/// control), so the writer itself reports `opaque_control_unrepresentable` (#583 review F1).
fn hwp5_with_an_unrepresentable_control(dir: &Path) -> PathBuf {
    let base = hwp5_with_subsecond_times(dir);
    let json_path = dir.join("opaque.json");
    let out = run(&[
        "convert".as_ref(),
        base.as_os_str(),
        "-o".as_ref(),
        json_path.as_os_str(),
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let mut doc: Value = serde_json::from_slice(&std::fs::read(&json_path).unwrap()).unwrap();
    let paragraph = &mut doc["sections"][0]["paragraphs"][3];
    let index = paragraph["controls"].as_array().unwrap().len();
    paragraph["controls"]
        .as_array_mut()
        .unwrap()
        .push(json!({"Generic": {
            "ctrl_id": [120, 120, 120, 120], "data": "0a0b0c0d", "paragraph_lists": [],
            "extras": [], "raw_children": [], "gso_shapes": [], "equation": null,
            "column_def": null, "caption": null, "hwpx_raw_xml": null, "container_box": null
        }}));
    paragraph["chars"]
        .as_array_mut()
        .unwrap()
        .push(json!({"ExtCtrl": {
            "code": 11, "ctrl_id": [120, 120, 120, 120],
            "payload": "787878780000000000000000", "ctrl_index": index
        }}));
    std::fs::write(&json_path, serde_json::to_vec(&doc).unwrap()).unwrap();
    let hwp5 = dir.join("opaque.hwp");
    let out = run(&[
        "new".as_ref(),
        "--from".as_ref(),
        json_path.as_os_str(),
        "-o".as_ref(),
        hwp5.as_os_str(),
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    hwp5
}

/// F1: only what the cross-format accounting ADDS may publish. An event the HWPX writer itself
/// reports refuses an HWP5 to `.hwpx` edit, exactly as it did before this change, so the new
/// reporting does not widen what publishes.
#[test]
fn hwp5_to_hwpx_edit_still_refuses_an_event_the_writer_itself_reports() {
    let dir = workdir("writer-own");
    let input = hwp5_with_an_unrepresentable_control(&dir);
    // The convert accounting for the same input names the writer's event, so the premise holds.
    let loss = dir.join("loss.json");
    let converted = run(&[
        "convert".as_ref(),
        input.as_os_str(),
        "-o".as_ref(),
        dir.join("conv.hwpx").as_os_str(),
        "--loss-report".as_ref(),
        loss.as_os_str(),
    ]);
    assert!(converted.status.success(), "{}", stderr(&converted));
    let ledger: Value = serde_json::from_slice(&std::fs::read(&loss).unwrap()).unwrap();
    assert!(
        ledger["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["code"] == "opaque_control_unrepresentable"),
        "the writer reports the unknown control: {ledger}"
    );
    for verify in [false, true] {
        let output = dir.join(format!("out-{verify}.hwpx"));
        let report = dir.join(format!("report-{verify}.json"));
        let extra: &[&str] = if verify { &["--verify"] } else { &[] };
        let edited = edit_set_meta(&input, &output, &report, extra);
        assert!(!edited.status.success(), "verify={verify}: must refuse");
        assert!(
            !output.exists(),
            "verify={verify}: a refused edit publishes nothing"
        );
        assert!(
            stderr(&edited).contains("opaque_control_unrepresentable"),
            "{}",
            stderr(&edited)
        );
        let refusal = read_report(&report);
        assert_eq!(refusal["refusal"]["code"], "preservation_loss", "{refusal}");
        let events = refusal["refusal"]["values"]["events"].as_array().unwrap();
        assert!(
            events
                .iter()
                .any(|e| e["code"] == "opaque_control_unrepresentable"),
            "{refusal}"
        );
    }
}

/// The local-only samples, from the issue's AC1: `--set-meta` and a top-level `--replace`
/// with `--verify` to `.hwpx`.
fn assert_sample_verifies(sample: &str, replace: &str) {
    let input = repo().join("fixtures/hwp5").join(sample);
    if fixture_skip::fixture_missing(&input) {
        return;
    }
    let dir = workdir(sample);
    for (label, args) in [
        ("meta", vec!["--set-meta", "title=probe"]),
        ("replace", vec!["--replace", replace]),
    ] {
        let output = dir.join(format!("{label}.hwpx"));
        let report = dir.join(format!("{label}.report.json"));
        let mut command: Vec<&OsStr> = vec![
            "edit".as_ref(),
            input.as_os_str(),
            "-o".as_ref(),
            output.as_os_str(),
        ];
        command.extend(args.iter().map(OsStr::new));
        command.extend(["--verify".as_ref(), "--report".as_ref(), report.as_os_str()]);
        let edited = run(&command);
        assert!(
            edited.status.success(),
            "{sample} {label}: {}",
            stderr(&edited)
        );
        let report = read_report(&report);
        assert!(
            event_codes(&report).contains(&"metadata_value_removed".to_string()),
            "{sample} {label}: {report}"
        );
    }
}

#[test]
fn hello_world_verifies_to_hwpx() {
    assert_sample_verifies("hello_world.hwp", "Hello=>Hi");
}

#[test]
fn work_report_verifies_to_hwpx() {
    assert_sample_verifies("work_report.hwp", "주간업무보고서=>주간업무보고서X");
}

#[test]
fn annual_report_verifies_to_hwpx() {
    assert_sample_verifies("annual_report.hwp", "Vision=>VisionX");
}

/// The `preservation` event shape in `edit-report-v1` 1.4 is a copy of the
/// `hwp-preservation-report-v1` event. A new code added to that schema must reach this one too,
/// or a run reporting it would write a report its own schema rejects.
#[test]
fn edit_report_preservation_events_match_the_preservation_report_schema() {
    let edit: Value =
        serde_json::from_str(include_str!("../../../schemas/edit-report-v1.schema.json")).unwrap();
    let preservation: Value = serde_json::from_str(include_str!(
        "../../../schemas/preservation-report-v1.schema.json"
    ))
    .unwrap();
    let ours = &edit["properties"]["preservation"]["items"]["properties"];
    let theirs = &preservation["$defs"]["event"]["properties"];
    for field in ["code", "resource", "disposition", "count"] {
        assert_eq!(
            ours[field], theirs[field],
            "{field} drifted from the preservation schema"
        );
    }
}
