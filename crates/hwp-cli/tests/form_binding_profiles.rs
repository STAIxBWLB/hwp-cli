//! Owner-authored P0 acceptance through real extraction, discovery and compound native editing.
//! Structural/value evidence only; no fonts, page-count goldens or Hancom claims.
use hwp_model::Document;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
#[allow(dead_code)]
#[path = "common/original_form_contract.rs"]
mod contract;
#[path = "common/fixture_skip.rs"]
mod fixture_skip;
#[path = "common/temp_path.rs"]
mod temp_path;

struct Workspace {
    dir: temp_path::TempPath,
}
fn hwp(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}
fn success(output: Output, purpose: &str) -> Output {
    assert!(
        output.status.success(),
        "{purpose} must succeed (including native --verify where requested): {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
fn read(path: &Path) -> Document {
    if path.extension().unwrap() == "hwp" {
        hwp5::read_document(path).unwrap().document
    } else {
        hwpx::read_document(path).unwrap().document
    }
}
impl Workspace {
    fn new(label: &str) -> Option<Self> {
        let dir = temp_path::TempPath::new("hwp418-profiles", label);
        std::fs::create_dir_all(&*dir).unwrap();
        let python = std::env::var_os("PYTHON").unwrap_or_else(|| {
            if cfg!(windows) {
                "python".into()
            } else {
                "python3".into()
            }
        });
        let generator = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tools/generate_original_form_fixture.py");
        match Command::new(python).arg(generator).arg(&*dir).output() {
            Ok(result) => {
                success(result, "owner-authored P0 generator");
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fixture_skip::resource_unavailable(
                    "Python for original-form binding fixture",
                    "install Python 3 or set PYTHON to its executable",
                );
                return None;
            }
            Err(error) => panic!("P0 generator could not start: {error}"),
        }
        let spec = dir.join("original-form.spec.json").canonicalize().unwrap();
        success(
            hwp(&dir, &["compose", spec.to_str().unwrap(), "-o", "p0.hwpx"]),
            "P0 compose",
        );
        Some(Self { dir })
    }
    fn discover(&self, source: &str) -> Value {
        let result = success(
            hwp(
                &self.dir,
                &["slots", source, "--forms", "--json", "--occurrences"],
            ),
            "source-bound occurrence discovery",
        );
        let result: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(
            result["source_sha256"],
            contract::digest(&std::fs::read(self.dir.join(source)).unwrap())
        );
        result
    }
    fn profile_source(&self, extension: &str) -> String {
        success(
            hwp(
                &self.dir,
                &[
                    "split",
                    "p0.hwpx",
                    "--out-dir",
                    "one-profile",
                    "--from",
                    "heading:FORM_START_PROFILE_1",
                    "--to",
                    "heading:FORM_START_PROFILE_2",
                    "--end-exclusive",
                    "--range-report",
                    "one-profile.range.json",
                ],
            ),
            "real #417 heading-range extraction",
        );
        let report: Value = serde_json::from_slice(
            &std::fs::read(self.dir.join("one-profile.range.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(report["contract"], "hwp-range-extraction-v1");
        assert_eq!(
            report["source_sha256"],
            contract::digest(&std::fs::read(self.dir.join("p0.hwpx")).unwrap())
        );
        assert!(!report["addresses"].as_array().unwrap().is_empty());
        assert!(
            report["addresses"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["kind"] == "table"
                    && entry["source"]["table"] == 2
                    && entry["output"]["table"] == 0),
            "the real range map must carry profile1 table2 to output table0"
        );
        let fragment = "one-profile/p0-001.hwpx";
        assert_eq!(
            report["fragment_sha256"],
            contract::digest(&std::fs::read(self.dir.join(fragment)).unwrap())
        );
        assert_eq!(
            contract::project_form(&read(&self.dir.join(fragment)))
                .tables
                .len(),
            1
        );
        if extension == "hwp" {
            success(
                hwp(&self.dir, &["convert", fragment, "-o", "profile.hwp"]),
                "extracted HWPX to native HWP",
            );
            "profile.hwp".into()
        } else {
            fragment.into()
        }
    }
    fn apply(
        &self,
        source: &str,
        extension: &str,
        plan: Value,
        values: Value,
    ) -> (Document, Value, Value) {
        let receipt = self.dir.join("binding.receipt.json");
        let preview = self.dir.join("binding.preview.json");
        let ops = json!([{"op":"apply_form_bindings","plan":plan,"values":values,"receipt":receipt,"preview":preview}]);
        std::fs::write(
            self.dir.join("binding.ops.json"),
            serde_json::to_vec_pretty(&ops).unwrap(),
        )
        .unwrap();
        let original = std::fs::read(self.dir.join(source)).unwrap();
        let output = format!("bound.{extension}");
        success(
            hwp(
                &self.dir,
                &[
                    "edit",
                    source,
                    "-o",
                    &output,
                    "--ops",
                    "binding.ops.json",
                    "--verify",
                ],
            ),
            "compound binding with native verification",
        );
        assert!(std::fs::read(self.dir.join(source)).unwrap() == original);
        let receipt: Value = serde_json::from_slice(&std::fs::read(receipt).unwrap()).unwrap();
        let preview: Value = serde_json::from_slice(&std::fs::read(preview).unwrap()).unwrap();
        assert_eq!(receipt["published"], true);
        assert_eq!(receipt["render_font_native"], "unverified");
        assert_eq!(
            receipt["output_sha256"],
            contract::digest(&std::fs::read(self.dir.join(&output)).unwrap())
        );
        (read(&self.dir.join(output)), preview, receipt)
    }
}
fn empty_plan(discovery: &Value) -> Value {
    json!({"version":1,"source_sha256":discovery["source_sha256"],"structural_sha256":discovery["structural_sha256"],"bindings":[],"repeats":[],"constraints":[]})
}
fn selected_cell<'a>(
    discovery: &'a Value,
    table: usize,
    row: u16,
    col: u16,
    old: &str,
) -> &'a Value {
    let found: Vec<_> = discovery["occurrences"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| {
            item["kind"] == "cell_text"
                && item["bindable"] == true
                && item["table_index"] == table
                && item["row"] == row
                && item["col"] == col
                && item["old_text"] == old
        })
        .collect();
    assert_eq!(
        found.len(),
        1,
        "authored table/cell/prior-value tuple must select exactly one bindable paragraph"
    );
    found[0]
}
fn binding(key: &str, ids: &[&Value], copies: &[usize]) -> Value {
    json!({"key":key,"targets":ids.iter().map(|item|json!({"occurrence_id":item["id"],"copies":copies,"max_chars":100,"allow_reflow":true})).collect::<Vec<_>>()})
}
fn four_profiles_case(extension: &str) {
    let Some(ws) = Workspace::new(&format!("clone-four-{extension}")) else {
        return;
    };
    let source = ws.profile_source(extension);
    let before = read(&ws.dir.join(&source));
    let discovery = ws.discover(&source);
    let target = selected_cell(&discovery, 0, 2, 1, "Synthetic profile 1");
    let heading = before.sections[0]
        .paragraphs
        .iter()
        .position(|paragraph| contract::direct_text(paragraph) == "FORM_START_PROFILE_1")
        .expect("extracted profile heading");
    let path = hwp_convert::SegmentPath {
        section: 0,
        indices: vec![heading],
    };
    let heading_id = hwp_convert::paragraph_id(&path, &before.sections[0].paragraphs[heading]);
    let mut plan = empty_plan(&discovery);
    plan["bindings"] = json!([binding("profile.name", &[target], &[0, 1, 2, 3])]);
    plan["repeats"] = json!([{"table_id":target["table_id"],"after_paragraph_id":heading_id,"copies":4,"profile_ids":["profile-original","profile-copy1","profile-copy2","profile-copy3"]}]);
    let before_projection = contract::project_form(&before);
    let expected = contract::expect_cell_values(
        &before_projection,
        &[contract::ExpectedCell {
            table: 0,
            row: 2,
            col: 1,
            paragraphs: vec!["Revised profile".into()],
        }],
    )
    .unwrap();
    let (after, preview, receipt) = ws.apply(
        &source,
        extension,
        plan,
        json!({"profile.name":"Revised profile"}),
    );
    let after_projection = contract::project_form(&after);
    assert_eq!(after_projection.tables.len(), 4);
    for (index, table) in after_projection.tables.iter().enumerate() {
        contract::compare_table_payload(index, &expected.tables[0], table).unwrap();
    }
    assert_eq!(
        after.sections[0].paragraphs.len(),
        before.sections[0].paragraphs.len() + 3
    );
    for index in 0..before.sections[0].paragraphs.len() {
        let after_index = index + if index > heading { 3 } else { 0 };
        contract::compare_body_run(
            &before_projection,
            index..index + 1,
            &after_projection,
            after_index,
        )
        .unwrap();
    }
    assert_eq!(preview["changes"].as_array().unwrap().len(), 4);
    assert_eq!(preview["copies"].as_array().unwrap().len(), 4);
    let copies: BTreeSet<_> = preview["copies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|copy| copy["copy"].as_u64().unwrap())
        .collect();
    assert_eq!(copies, [0, 1, 2, 3].into_iter().collect());
    let ids: BTreeSet<_> = preview["copies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|copy| copy["table_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 4);
    for change in preview["changes"].as_array().unwrap() {
        assert_eq!(change["occurrence_id"], target["id"]);
        assert_eq!(change["old_text"], "Synthetic profile 1");
        assert_eq!(change["new_text"], "Revised profile");
        assert!(change["profile_id"].is_string());
    }
    assert_eq!(receipt["targets"], 4);
    assert_eq!(receipt["cloned_profiles"], 3);
}
#[test]
fn extracted_profile_clones_to_four_and_preserves_hwpx_contract() {
    four_profiles_case("hwpx");
}
#[test]
fn extracted_profile_clones_to_four_and_preserves_hwp_contract() {
    four_profiles_case("hwp");
}

fn allocation_revision_case(extension: &str) {
    let Some(ws) = Workspace::new(&format!("allocations-{extension}")) else {
        return;
    };
    let source = if extension == "hwp" {
        success(
            hwp(&ws.dir, &["convert", "p0.hwpx", "-o", "p0.hwp"]),
            "full P0 native HWP source",
        );
        "p0.hwp"
    } else {
        "p0.hwpx"
    };
    let before = read(&ws.dir.join(source));
    let before_projection = contract::project_form(&before);
    assert_eq!(before_projection.tables.len(), 6);
    let discovery = ws.discover(source);
    let mut plan = empty_plan(&discovery);
    let mut bindings = vec![];
    let mut values = serde_json::Map::new();
    let mut expected_cells = vec![];
    let mut constraints = vec![];
    for (row, old_lecture, lecture, old_practice, practice) in [
        (3, "18", "20", "12", "12"),
        (4, "24", "26", "6", "6"),
        (5, "20", "22", "10", "10"),
    ] {
        for (col, suffix, old, value) in [
            (1, "lecture", old_lecture, lecture),
            (2, "practice", old_practice, practice),
            (3, "subtotal", "30", "32"),
        ] {
            let key = format!("module-{row}.{suffix}");
            let item = selected_cell(&discovery, 1, row, col, old);
            bindings.push(binding(&key, &[item], &[0]));
            values.insert(key, json!(value));
            expected_cells.push(contract::ExpectedCell {
                table: 1,
                row,
                col,
                paragraphs: vec![value.into()],
            });
        }
        constraints.push(json!({"kind":"sum","keys":[format!("module-{row}.lecture"),format!("module-{row}.practice")],"total":format!("module-{row}.subtotal")}));
    }
    for (key, col, old, value, suffix) in [
        ("lecture-total", 1, "62", "68", "lecture"),
        ("practice-total", 2, "28", "28", "practice"),
    ] {
        bindings.push(binding(
            key,
            &[selected_cell(&discovery, 1, 6, col, old)],
            &[0],
        ));
        values.insert(key.into(), json!(value));
        expected_cells.push(contract::ExpectedCell {
            table: 1,
            row: 6,
            col,
            paragraphs: vec![value.into()],
        });
        constraints.push(json!({"kind":"sum","keys":(3..=5).map(|row|format!("module-{row}.{suffix}")).collect::<Vec<_>>(),"total":key}));
    }
    let total_targets: Vec<_> = (0..6)
        .map(|table| selected_cell(&discovery, table, if table == 1 { 6 } else { 2 }, 3, "90"))
        .collect();
    bindings.push(binding("total-hours", &total_targets, &[0]));
    values.insert("total-hours".into(), json!("96"));
    for table in 0..6 {
        expected_cells.push(contract::ExpectedCell {
            table,
            row: if table == 1 { 6 } else { 2 },
            col: 3,
            paragraphs: vec!["96".into()],
        });
    }
    constraints.push(
        json!({"kind":"sum","keys":["lecture-total","practice-total"],"total":"total-hours"}),
    );
    plan["bindings"] = json!(bindings);
    plan["constraints"] = json!(constraints);
    let expected = contract::expect_cell_values(&before_projection, &expected_cells).unwrap();
    let (after, preview, receipt) = ws.apply(source, extension, plan, Value::Object(values));
    let after_projection = contract::project_form(&after);
    contract::compare_form_contract(&expected, &after_projection).unwrap();
    assert_eq!(
        preview["changes"].as_array().unwrap().len(),
        expected_cells.len()
    );
    assert!(preview["copies"].as_array().unwrap().is_empty());
    assert_eq!(receipt["targets"], expected_cells.len());
    assert_eq!(receipt["cloned_profiles"], 0);
}
#[test]
fn p0_allocation_revision_updates_every_bound_hwpx_total_and_preserves_rest() {
    allocation_revision_case("hwpx");
}
#[test]
fn p0_allocation_revision_updates_every_bound_hwp_total_and_preserves_rest() {
    allocation_revision_case("hwp");
}
