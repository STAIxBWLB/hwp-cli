//! Original-form certification (#419) on the owner-authored P0 form set (#416).
//!
//! The form comes from `tools/generate_original_form_fixture.py`. Page placement needs a pinned
//! font environment, which CI does not bundle, so the pinned cases use one single-face TrueType
//! font found on the host (fonts-nanum on Linux CI, AppleGothic on macOS) and are skipped as a
//! missing required resource elsewhere. Every assertion is on finding codes and statuses, never
//! on glyphs, coordinates or page counts.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

#[path = "common/fixture_skip.rs"]
mod fixture_skip;
#[path = "common/temp_path.rs"]
mod temp_path;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ahwp"))
}

/// One host TrueType font and the family name it declares.
fn host_font() -> Option<(PathBuf, &'static str)> {
    [
        (
            "/usr/share/fonts/truetype/nanum/NanumGothic.ttf",
            "NanumGothic",
        ),
        (
            "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
            "AppleGothic",
        ),
    ]
    .into_iter()
    .map(|(path, family)| (PathBuf::from(path), family))
    .find(|(path, _)| path.is_file())
}

fn sha256(path: &Path) -> String {
    use sha2::{Digest as _, Sha256};
    Sha256::digest(std::fs::read(path).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn run(command: &mut Command, label: &str) {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "{label}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct Forms {
    dir: temp_path::TempPath,
}

impl Forms {
    /// Generates the P0 spec with `family` as the body font, and composes it.
    fn new(name: &str, family: &str) -> Option<Self> {
        let dir = temp_path::TempPath::new("hwp419", name);
        // A panicked earlier run keeps its directory for debugging; never build on it.
        let _ = std::fs::remove_dir_all(&*dir);
        std::fs::create_dir_all(&*dir).unwrap();
        let python = std::env::var_os("PYTHON")
            .unwrap_or_else(|| if cfg!(windows) { "python" } else { "python3" }.into());
        let generator = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tools/generate_original_form_fixture.py");
        match Command::new(python).arg(generator).arg(&*dir).output() {
            Ok(out) => assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fixture_skip::resource_unavailable(
                    "Python for original-form fixture generator",
                    "install Python 3 or set PYTHON to its executable",
                );
                return None;
            }
            Err(error) => panic!("fixture generator could not start: {error}"),
        }
        let forms = Self { dir };
        let mut spec: Value =
            serde_json::from_slice(&std::fs::read(forms.path("original-form.spec.json")).unwrap())
                .unwrap();
        spec["document"]["styles"]["body"]["font_family"] = json!(family);
        forms.compose(&spec, "good.hwpx");
        Some(forms)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn spec(&self) -> Value {
        let mut spec: Value =
            serde_json::from_slice(&std::fs::read(self.path("original-form.spec.json")).unwrap())
                .unwrap();
        let family = self.family();
        spec["document"]["styles"]["body"]["font_family"] = json!(family);
        spec
    }

    fn family(&self) -> String {
        std::fs::read_to_string(self.path("family.txt")).unwrap_or_else(|_| "AppleGothic".into())
    }

    fn compose(&self, spec: &Value, out: &str) {
        let name = format!("{out}.spec.json");
        std::fs::write(self.path(&name), serde_json::to_vec(spec).unwrap()).unwrap();
        run(
            hwp()
                .arg("compose")
                .arg(self.path(&name))
                .arg("-o")
                .arg(self.path(out)),
            "compose",
        );
    }

    fn set_cell(&self, out: &str, value: &str) {
        run(
            hwp()
                .arg("edit")
                .arg(self.path("good.hwpx"))
                .arg("-o")
                .arg(self.path(out))
                .arg("--set-cell")
                .arg(format!("0:2:1={value}")),
            "edit",
        );
    }

    /// Writes the plan and policy, certifies `input` and returns (exit success, report, stderr).
    fn certify(
        &self,
        input: &str,
        font: Option<&Path>,
        address_map: bool,
    ) -> (bool, Value, String) {
        self.certify_with(input, font, address_map, true)
    }

    /// [`Self::certify`] with the policy's `forbid_substitution` set explicitly.
    fn certify_with(
        &self,
        input: &str,
        font: Option<&Path>,
        address_map: bool,
        forbid_substitution: bool,
    ) -> (bool, Value, String) {
        let plan = json!({
            "contract": "hwp-original-form-certification-plan-v1",
            "version": 1,
            "table_count": 6,
            "form_starts": [
                {"id": "application", "text": "FORM_START_APPLICATION"},
                {"id": "development-report", "text": "FORM_START_DEVELOPMENT"},
                {"id": "profile-1", "text": "FORM_START_PROFILE_1"}
            ],
            "tables": [{"id": "application.grid", "table": 0, "rows": 7, "cols": 4}],
            "expected_cells": [
                {"id": "application.course", "table": 0, "row": 1, "col": 1, "text": "Synthetic Methods"},
                {"id": "application.year", "table": 0, "row": 1, "col": 3, "text": "2029", "allow_year_change": true},
                {"id": "application.applicant", "table": 0, "row": 2, "col": 1, "text": "Sample Author"},
                {"id": "development.module-a.subtotal", "table": 1, "row": 3, "col": 3, "text": "30"}
            ]
        });
        std::fs::write(self.path("plan.json"), plan.to_string()).unwrap();
        let mut document =
            json!({"original_form": {"plan": "plan.json", "address_map": address_map}});
        if let Some(font) = font {
            if !self.path("font.ttf").exists() {
                // Copy the bytes: macOS `fs::copy` clones a system font with its compression
                // attributes, and reading such a clone can fail with ENOATTR.
                std::fs::write(self.path("font.ttf"), std::fs::read(font).unwrap()).unwrap();
            }
            let hash = sha256(&self.path("font.ttf"));
            document["fonts"] = json!({
                "manifest": [{"path": "font.ttf", "sha256": hash}],
                "forbid_substitution": forbid_substitution
            });
        }
        let policy = json!({
            "schema_version": "1.0",
            "document": document,
            "render": {"pages": "all", "fail_on_outside_page_bounds": false, "fail_on_potential_collision": false}
        });
        let policy_path = self.path(&format!("{input}.policy.json"));
        std::fs::write(&policy_path, policy.to_string()).unwrap();
        let report = self.path(&format!("{input}.report"));
        let out = hwp()
            .arg("certify")
            .arg(self.path(input))
            .arg("--policy")
            .arg(&policy_path)
            .arg("--report")
            .arg(&report)
            .output()
            .unwrap();
        let value: Value =
            serde_json::from_slice(&std::fs::read(report.join("report.json")).unwrap()).unwrap();
        (
            out.status.success(),
            value,
            String::from_utf8_lossy(&out.stderr).into_owned()
                + &String::from_utf8_lossy(&out.stdout),
        )
    }
}

fn codes(report: &Value) -> Vec<(String, String)> {
    report["checks"]["original_form"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["id"].as_str().unwrap().to_string(),
                f["code"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn has(report: &Value, id: &str, code: &str) -> bool {
    codes(report).iter().any(|(i, c)| i == id && c == code)
}

/// Text the plan and the form hold. None of it may reach a report, a map, or a log.
const PRIVATE_TEXT: [&str; 6] = [
    "Synthetic Methods",
    "Sample Author",
    "FORM_START_APPLICATION",
    "FORM_START_DEVELOPMENT",
    "long value line",
    "APPLICATION_FORM",
];

fn assert_content_free(forms: &Forms, input: &str, log: &str) {
    let report = forms.path(&format!("{input}.report"));
    let mut texts = vec![log.to_string()];
    for name in [
        "report.json",
        "manifest.json",
        "original-form/address-map.json",
    ] {
        if let Ok(bytes) = std::fs::read(report.join(name)) {
            texts.push(String::from_utf8_lossy(&bytes).into_owned());
        }
    }
    assert!(
        !report.join("plan.json").exists(),
        "the plan is never copied"
    );
    for text in &texts {
        for private in PRIVATE_TEXT {
            assert!(
                !text.contains(private),
                "{input}: document text {private:?} leaked"
            );
        }
    }
}

/// `name` keeps each test's directory its own: tests run in parallel.
fn pinned(name: &str) -> Option<(Forms, PathBuf)> {
    let Some((font, family)) = host_font() else {
        fixture_skip::resource_unavailable(
            "single-face TrueType font (NanumGothic or AppleGothic)",
            "install fonts-nanum (Linux) or run on macOS",
        );
        return None;
    };
    let forms = Forms::new(name, family)?;
    std::fs::write(forms.path("family.txt"), family).unwrap();
    Some((forms, font))
}

/// AC: a correct fill certifies; a clipped row, a missing value, a moved form start and a
/// substituted font each fail with their own cause. Refusals and reports stay content-free.
#[test]
fn p0_correct_fill_certifies_and_each_defect_fails_with_its_own_cause() {
    let Some((forms, font)) = pinned("pinned") else {
        return;
    };

    let (ok, report, log) = forms.certify("good.hwpx", Some(&font), true);
    assert!(
        ok,
        "correct fill must certify: {}",
        report["checks"]["original_form"]
    );
    assert_eq!(report["overall"], "passed");
    assert_eq!(report["checks"]["original_form"]["status"], "passed");
    assert_eq!(report["checks"]["original_form"]["limited"], false);
    assert_eq!(
        report["checks"]["original_form"]["address_map_published"],
        true
    );
    assert!(
        forms
            .path("good.hwpx.report/original-form/address-map.json")
            .is_file()
    );
    assert_content_free(&forms, "good.hwpx", &log);

    forms.set_cell("missing.hwpx", "");
    let (ok, report, log) = forms.certify("missing.hwpx", Some(&font), true);
    assert!(!ok);
    assert!(
        has(
            &report,
            "application.applicant",
            "expected_cell_value_missing"
        ),
        "{:?}",
        codes(&report)
    );
    assert_content_free(&forms, "missing.hwpx", &log);
    let map: Value = serde_json::from_slice(
        &std::fs::read(forms.path("missing.hwpx.report/original-form/address-map.json")).unwrap(),
    )
    .unwrap();
    let entry = &map["entries"][0];
    assert_eq!(
        (&entry["id"], &entry["table"], &entry["row"], &entry["col"]),
        (
            &json!("application.applicant"),
            &json!(0),
            &json!(2),
            &json!(1)
        ),
        "the local map locates the cell by address"
    );

    let long = vec!["long value line"; 200].join("\n\n");
    forms.set_cell("clipped.hwpx", &long);
    let (ok, report, log) = forms.certify("clipped.hwpx", Some(&font), true);
    assert!(!ok);
    assert!(
        has(&report, "render", "table_row_clipped"),
        "{:?}",
        codes(&report)
    );
    assert_content_free(&forms, "clipped.hwpx", &log);

    let mut moved = forms.spec();
    let blocks = moved["document"]["sections"][0]["blocks"]
        .as_array_mut()
        .unwrap();
    let before_development = blocks
        .iter()
        .position(|b| b["runs"][0]["text"] == "FORM_START_DEVELOPMENT")
        .unwrap();
    assert_eq!(blocks[before_development - 1]["type"], "break");
    blocks.remove(before_development - 1);
    forms.compose(&moved, "moved.hwpx");
    let (ok, report, log) = forms.certify("moved.hwpx", Some(&font), true);
    assert!(!ok);
    assert!(
        has(
            &report,
            "development-report",
            "form_start_not_at_page_start"
        ),
        "{:?}",
        codes(&report)
    );
    assert!(!has(&report, "application", "form_start_not_at_page_start"));
    assert_content_free(&forms, "moved.hwpx", &log);

    let mut substituted = forms.spec();
    substituted["document"]["styles"]["body"]["font_family"] = json!("No Such Family 419");
    forms.compose(&substituted, "substituted.hwpx");
    let (ok, report, log) = forms.certify("substituted.hwpx", Some(&font), true);
    assert!(!ok);
    assert!(
        has(&report, "fonts", "disallowed_font_substitution"),
        "{:?}",
        codes(&report)
    );
    assert_content_free(&forms, "substituted.hwpx", &log);
}

/// AC: without pinned fonts the result is explicitly limited and names the missing evidence;
/// it never certifies, and placement is not reported as a pass or a defect.
#[test]
fn without_pinned_fonts_the_result_is_limited_and_names_the_missing_evidence() {
    let Some(forms) = Forms::new("unpinned", "Noto Sans CJK KR") else {
        return;
    };
    let (ok, report, log) = forms.certify("good.hwpx", None, false);
    assert!(!ok, "a limited result never certifies");
    assert_ne!(report["overall"], "passed");
    let check = &report["checks"]["original_form"];
    assert_eq!(check["limited"], true);
    assert!(
        check["missing_evidence"]
            .as_array()
            .unwrap()
            .contains(&json!("pinned_font_environment")),
        "{check}"
    );
    assert!(
        !codes(&report)
            .iter()
            .any(|(_, code)| code.starts_with("form_start_not") || code == "table_row_clipped"),
        "placement is missing evidence, not a finding: {:?}",
        codes(&report)
    );
    assert!(
        !forms.path("good.hwpx.report/original-form").exists(),
        "no map without opt-in"
    );
    assert_eq!(check["address_map"], false);
    assert_eq!(check["address_map_published"], false);
    assert_content_free(&forms, "good.hwpx", &log);
}

/// A plan that does not validate is refused content-free, in the report rather than as text.
#[test]
fn an_invalid_plan_is_a_content_free_refusal_with_its_cause_class() {
    let Some(forms) = Forms::new("invalid", "Noto Sans CJK KR") else {
        return;
    };
    let (_, _, _) = forms.certify("good.hwpx", None, false);
    let policy = forms.path("good.hwpx.policy.json");
    // T11-N1: each family of plan failure gets its own content-free cause and next step.
    let ids_plan = json!({"contract": "hwp-original-form-certification-plan-v1", "version": 1,
        "expected_cells": [{"id": "has space", "table": 0, "row": 0, "col": 0, "text": "Sample Author"}]});
    let contract_plan = json!({"contract": "something-else", "version": 1,
        "expected_cells": [{"id": "a", "table": 0, "row": 0, "col": 0, "text": "Sample Author"}]});
    let empty_plan = json!({"contract": "hwp-original-form-certification-plan-v1", "version": 1});
    let cases = [
        (
            "ids",
            ids_plan.to_string(),
            "original_form_plan_ids",
            "make_plan_ids_and_cells_unique",
        ),
        (
            "not-json",
            "Sample Author {".to_string(),
            "original_form_plan_not_json",
            "fix_the_plan_against_its_schema",
        ),
        (
            "contract",
            contract_plan.to_string(),
            "original_form_plan_contract",
            "set_the_plan_contract_and_version",
        ),
        (
            "bounds",
            empty_plan.to_string(),
            "original_form_plan_bounds",
            "bring_the_plan_within_its_bounds",
        ),
    ];
    for (label, plan, code, next_step) in cases {
        std::fs::write(forms.path("plan.json"), plan).unwrap();
        let report = forms.path(&format!("invalid-{label}.report"));
        let out = hwp()
            .arg("certify")
            .arg(forms.path("good.hwpx"))
            .arg("--policy")
            .arg(&policy)
            .arg("--report")
            .arg(&report)
            .output()
            .unwrap();
        assert!(!out.status.success(), "{label}");
        let value: Value =
            serde_json::from_slice(&std::fs::read(report.join("report.json")).unwrap()).unwrap();
        let check = &value["checks"]["original_form"];
        assert_eq!(check["reason_codes"], json!([code]), "{label}");
        assert_eq!(check["findings"][0]["next_step"], next_step, "{label}");
        let text = String::from_utf8_lossy(&out.stderr).into_owned()
            + &String::from_utf8_lossy(&std::fs::read(report.join("report.json")).unwrap());
        assert!(
            !text.contains("Sample Author") && !text.contains("has space"),
            "{label}: plan text leaked"
        );
    }
}

/// T11-F1: a substitution the policy allows (`forbid_substitution: false`) is not a
/// `disallowed_font_substitution` finding, even with a manifest. The environment is then not
/// pinned, so the check is limited and names `pinned_font_environment`.
#[test]
fn an_allowed_substitution_is_limited_evidence_not_a_disallowed_finding() {
    let Some((forms, font)) = pinned("allowed") else {
        return;
    };
    let mut substituted = forms.spec();
    substituted["document"]["styles"]["body"]["font_family"] = json!("No Such Family 419");
    forms.compose(&substituted, "allowed.hwpx");
    let (ok, report, log) = forms.certify_with("allowed.hwpx", Some(&font), false, false);
    assert!(!ok, "a limited result never certifies");
    let check = &report["checks"]["original_form"];
    assert!(
        !has(&report, "fonts", "disallowed_font_substitution"),
        "the policy allows substitution: {:?}",
        codes(&report)
    );
    assert_eq!(check["limited"], true, "{check}");
    assert!(
        check["missing_evidence"]
            .as_array()
            .unwrap()
            .contains(&json!("pinned_font_environment")),
        "{check}"
    );
    assert_content_free(&forms, "allowed.hwpx", &log);
}
