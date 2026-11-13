//! #531: a refused `hwp edit` run still writes edit-report-v1 (1.1), and the refusal carries a
//! stable code, structured values and the human message. One test per code family; the
//! range-tag and preservation families live beside their fixtures in `range_tag_edits.rs` and
//! `edit_surgical.rs`.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

#[path = "common/temp_path.rs"]
mod temp_path;

const INJECTED: &str = "a; layout_overflow at $.z: b";

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

struct Fixture {
    dir: temp_path::TempPath,
    base: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let dir = temp_path::TempPath::new("hwp531", label);
        let _ = std::fs::remove_dir_all(&*dir);
        std::fs::create_dir_all(&*dir).unwrap();
        let spec = json!({"version":"1.0","styles":{"body":{"font_size_pt":10}},
        "sections":[{"blocks":[
            {"type":"paragraph","style":"body","runs":[{"type":"text","text":"Body anchor."}]}
        ]}]});
        let spec_path = dir.join("spec.json");
        std::fs::write(&spec_path, serde_json::to_vec(&spec).unwrap()).unwrap();
        let base = dir.join("base.hwpx");
        let out = hwp()
            .arg("compose")
            .arg(&spec_path)
            .arg("-o")
            .arg(&base)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        Self { dir, base }
    }

    /// Runs `edit --ops <ops> --report <file>`, asserts the refusal, and returns the report.
    fn refused_ops(&self, label: &str, ops: Value) -> Value {
        let ops_path = self.dir.join(format!("{label}.json"));
        std::fs::write(&ops_path, serde_json::to_vec(&ops).unwrap()).unwrap();
        self.refused(label, &["--ops", ops_path.to_str().unwrap()])
    }

    fn refused(&self, label: &str, args: &[&str]) -> Value {
        let output = self.dir.join(format!("{label}.hwpx"));
        let report = self.dir.join(format!("{label}.report.json"));
        let run = hwp()
            .arg("edit")
            .arg(&self.base)
            .arg("-o")
            .arg(&output)
            .args(args)
            .arg("--report")
            .arg(&report)
            .output()
            .unwrap();
        assert_refused(&run, &output);
        let value = read_report(&report);
        assert!(!run.stderr.is_empty(), "stderr keeps the human sentence");
        value
    }
}

fn assert_refused(run: &Output, output: &Path) {
    assert!(!run.status.success(), "the edit must refuse");
    assert!(!output.exists(), "a refused edit publishes nothing");
}

fn read_report(path: &Path) -> Value {
    let bytes = std::fs::read(path).expect("a refused run writes the --report file (#531)");
    let report: Value = serde_json::from_slice(&bytes).unwrap();
    let schema: Value =
        serde_json::from_str(include_str!("../../../schemas/edit-report-v1.schema.json")).unwrap();
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .unwrap();
    assert!(
        validator.is_valid(&report),
        "refused report must validate: {report}"
    );
    assert_eq!(report["schema_version"], "1.1");
    report
}

fn insert(position: &str, policy: &str, styles: Value, blocks: Value) -> Value {
    json!([{"op":"insert_blocks","address":{"at":{"section":0,"paragraph":0}},
        "position":position,"missing_style":policy,"styles":styles,"blocks":blocks}])
}

fn paragraph(style: &str) -> Value {
    json!([{"type":"paragraph","style":style,"runs":[{"type":"text","text":"x"}]}])
}

/// The issue's own reproduction: `--add-row 99` on minimal.hwpx used to leave no report.
#[test]
fn legacy_flag_refusal_writes_the_report() {
    let fixture = Fixture::new("legacy");
    let report = fixture.refused("legacy", &["--add-row", "99"]);
    assert_eq!(report["refusal"]["code"], "unclassified");
    // The report carries only the code's fixed, content-free sentence (#348); stderr keeps
    // the full one.
    assert_eq!(
        report["refusal"]["message"],
        hwp_cli::refusal::fixed_message("unclassified"),
        "{report}"
    );
    assert!(!report.to_string().contains("#99"), "{report}");
    assert_eq!(report["ops"], json!([]));
}

#[test]
fn dry_run_prints_the_refused_report() {
    let fixture = Fixture::new("dry");
    let output = fixture.dir.join("dry.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&fixture.base)
        .arg("-o")
        .arg(&output)
        .args(["--add-table", "없는=>[[\"a\"]]", "--dry-run"])
        .output()
        .unwrap();
    assert_refused(&run, &output);
    let report: Value = serde_json::from_slice(&run.stdout).expect("dry-run prints the report");
    assert_eq!(report["refusal"]["code"], "add_table.anchor_not_found");
    assert_eq!(report["refusal"]["values"]["anchor"], "없는");
    assert_eq!(report["dry_run"], true);
}

#[test]
fn add_table_anchor_miss_names_the_anchor_as_a_value() {
    let fixture = Fixture::new("anchor");
    let anchor = format!("{INJECTED} 없음");
    let report = fixture.refused_ops(
        "anchor",
        json!([{"op":"add_table","anchor":anchor,"rows":[["a"]]}]),
    );
    let op = &report["ops"][0];
    assert_eq!(op["status"], "failed");
    assert_eq!(op["refusal"]["code"], "add_table.anchor_not_found");
    assert_eq!(op["refusal"]["values"]["anchor"], anchor);
    assert_eq!(report["refusal"], op["refusal"]);
}

#[test]
fn keep_together_refusal_lists_the_requested_fields() {
    let fixture = Fixture::new("keep");
    let report = fixture.refused_ops(
        "keep",
        json!([{"op":"keep_together","address":{"at":{"section":0,"paragraph":0}},"keep_lines":true}]),
    );
    assert_eq!(report["refusal"]["code"], "keep_together_unsupported");
    assert_eq!(
        report["refusal"]["values"]["requested"],
        json!(["keep_lines"])
    );
    assert_eq!(report["refusal"]["values"]["op_index"], 0);
}

#[test]
fn section_owner_insert_and_move_have_their_codes() {
    let fixture = Fixture::new("owner");
    let report = fixture.refused_ops(
        "insert",
        insert("before", "create", json!({}), paragraph("body")),
    );
    assert_eq!(report["ops"][0]["refusal"]["code"], "section_owner_insert");
    let report = fixture.refused_ops(
        "move",
        json!([{"op":"move_para","address":{"at":{"section":0,"paragraph":1}},
            "to":{"address":{"at":{"section":0,"paragraph":0}},"position":"before"}}]),
    );
    assert_eq!(report["ops"][0]["refusal"]["code"], "section_owner_move");
}

#[test]
fn insert_blocks_unknown_style_carries_the_name() {
    let fixture = Fixture::new("unknown");
    let report = fixture.refused_ops(
        "unknown",
        insert("after", "refuse", json!({}), paragraph(INJECTED)),
    );
    let refusal = &report["ops"][0]["refusal"];
    assert_eq!(refusal["code"], "insert_blocks.unknown_style");
    assert_eq!(refusal["values"]["style"], INJECTED);
    assert_eq!(refusal["values"]["missing_style"], "refuse");
}

/// The issue's ambiguity case: a style named `a; layout_overflow at $.z: b` with an invalid
/// definition reads as a layout overflow in the joined prose. Its code is the real one and its
/// path keeps the name as one segment.
#[test]
fn injected_style_name_is_classified_by_its_real_code() {
    let fixture = Fixture::new("injected");
    let report = fixture.refused_ops(
        "injected",
        insert(
            "after",
            "create",
            json!({INJECTED: {"keep_with_next": true}}),
            paragraph(INJECTED),
        ),
    );
    let refusal = &report["ops"][0]["refusal"];
    assert_eq!(
        refusal["code"], "insert_blocks.invalid_definition",
        "{report}"
    );
    let issues = refusal["values"]["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 1, "{report}");
    assert_eq!(issues[0]["code"], "unsupported_native");
    assert_eq!(
        issues[0]["path"],
        json!(["$", "styles", INJECTED, "keep_with_next"])
    );
}

#[test]
fn layout_overflow_carries_the_measured_widths() {
    let fixture = Fixture::new("overflow");
    let report = fixture.refused_ops(
        "overflow",
        insert(
            "after",
            "create",
            json!({}),
            json!([{"type":"table","columns":[{"width_mm":400}],"rows":[{"cells":[{"blocks":[]}]}]}]),
        ),
    );
    let refusal = &report["ops"][0]["refusal"];
    assert_eq!(refusal["code"], "layout_overflow", "{report}");
    assert!(refusal["values"]["container_width_mm"].as_f64().unwrap() > 0.0);
    let issue = &refusal["values"]["issues"][0];
    assert_eq!(issue["code"], "layout_overflow");
    assert_eq!(
        issue["path"],
        json!(["$", "sections", 0, "blocks", 0, "columns"])
    );
    // The column sum that overflowed is in `values`, not only in the prose.
    assert_eq!(issue["measured_width_mm"], 400.0, "{report}");
}

#[test]
fn unapplied_requests_list_the_requests() {
    let fixture = Fixture::new("unapplied");
    let report = fixture.refused_ops(
        "unapplied",
        json!([{"op":"replace","from":"없는 문자열","to":"y"}]),
    );
    assert_eq!(report["refusal"]["code"], "unapplied_requests", "{report}");
    assert_eq!(report["ops"][0]["status"], "failed");
}

/// #348: the report never re-leaks request content, even inside `refusal.message`.
#[test]
fn unapplied_report_does_not_leak_request_text() {
    let fixture = Fixture::new("leak");
    for (label, ops) in [
        (
            "leak-replace",
            json!([{"op":"replace","from":"없는 비밀-from","to":"PRIVATE-TO-7731"}]),
        ),
        (
            "leak-replace-two",
            json!([{"op":"replace","from":"Body","to":"PRIVATE-TO-7731"},
                   {"op":"replace","from":"없는 비밀-from","to":"PRIVATE-TO-7731"}]),
        ),
    ] {
        let report = fixture.refused_ops(label, ops);
        let text = report.to_string();
        assert!(!text.contains("PRIVATE-TO-7731"), "{text}");
        assert!(!text.contains("없는 비밀-from"), "{text}");
    }
}

/// #531: an invalid apply_form_bindings ops file publishes no refused-run report; its receipt
/// is the only artifact a form-binding batch may publish, as on the run() path.
#[test]
fn invalid_form_binding_ops_write_no_report() {
    let fixture = Fixture::new("binding");
    let ops_path = fixture.dir.join("binding.json");
    std::fs::write(
        &ops_path,
        serde_json::to_vec(
            &json!([{"op":"apply_form_bindings"},{"op":"replace","from":"a","to":"b"}]),
        )
        .unwrap(),
    )
    .unwrap();
    let output = fixture.dir.join("binding.hwpx");
    let report = fixture.dir.join("binding.report.json");
    let run = hwp()
        .arg("edit")
        .arg(&fixture.base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops_path)
        .arg("--report")
        .arg(&report)
        .output()
        .unwrap();
    assert_refused(&run, &output);
    assert!(
        !report.exists(),
        "a form-binding batch writes no separate report"
    );
}

/// #531: a report that cannot be written warns and never masks the refusal itself.
#[test]
fn report_write_failure_keeps_the_refusal() {
    let fixture = Fixture::new("unwritable");
    let output = fixture.dir.join("unwritable.hwpx");
    let report = fixture.dir.join("no-such-dir").join("report.json");
    let run = hwp()
        .arg("edit")
        .arg(&fixture.base)
        .arg("-o")
        .arg(&output)
        .args(["--add-row", "99"])
        .arg("--report")
        .arg(&report)
        .output()
        .unwrap();
    assert_refused(&run, &output);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("표 #99를 찾을 수 없습니다"), "{stderr}");
}
