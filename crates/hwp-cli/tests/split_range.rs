//! Typed range extraction (`hwp split --from/--to`, #417): each form of the generated P0 form
//! set is extracted from the HWPX source and from its HWP5 conversion, and compared with the
//! source through the shared original-form contract. Refusals must leave nothing on disk.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use hwp_model::Document;
use serde_json::Value;

#[path = "common/original_form_contract.rs"]
mod contract;
#[path = "common/fixture_skip.rs"]
mod fixture_skip;
#[path = "common/temp_path.rs"]
mod temp_path;

struct Workspace {
    dir: temp_path::TempPath,
    expected: Value,
}

fn hwp(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn hwp_ok(dir: &Path, args: &[&str]) {
    let result = hwp(dir, args);
    assert!(
        result.status.success(),
        "hwp {args:?} failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

impl Workspace {
    /// Generates the form set, composes the HWPX source and converts it to HWP5. `None` when
    /// Python is unavailable (recorded as a skip).
    fn new(name: &str) -> Option<Self> {
        let dir = temp_path::TempPath::new("hwp417", name);
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
            Ok(result) => assert!(
                result.status.success(),
                "fixture generator failed: {}",
                String::from_utf8_lossy(&result.stderr)
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
        let spec = dir.join("original-form.spec.json").canonicalize().unwrap();
        hwp_ok(
            &dir,
            &["compose", spec.to_str().unwrap(), "-o", "source.hwpx"],
        );
        hwp_ok(&dir, &["convert", "source.hwpx", "-o", "source.hwp"]);
        let expected =
            serde_json::from_slice(&std::fs::read(dir.join("expected.json")).unwrap()).unwrap();
        Some(Self { dir, expected })
    }

    fn markers(&self) -> Vec<(String, usize)> {
        self.expected["form_starts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|form| {
                (
                    form["marker"].as_str().unwrap().to_string(),
                    form["paragraph"].as_u64().unwrap() as usize,
                )
            })
            .collect()
    }

    fn read(&self, file: &str) -> Document {
        let path = self.dir.join(file);
        if file.ends_with(".hwpx") {
            hwpx::read_document(&path).unwrap().document
        } else {
            hwp5::read_document(&path).unwrap().document
        }
    }

    fn sha256(&self, file: &str) -> String {
        contract::digest(&std::fs::read(self.dir.join(file)).unwrap())
    }
}

fn report_validator() -> jsonschema::Validator {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../schemas/range-extraction-v1.schema.json"
    ))
    .expect("the schema is valid JSON");
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .expect("the schema compiles as Draft 2020-12")
}

fn assert_report_valid(report: &Value) {
    let errors: Vec<String> = report_validator()
        .iter_errors(report)
        .map(|error| error.to_string())
        .collect();
    assert!(errors.is_empty(), "report violates its schema: {errors:?}");
}

fn body_len(doc: &Document) -> usize {
    doc.sections[0].paragraphs.len()
}

fn tables_in(doc: &Document, range: std::ops::Range<usize>) -> usize {
    doc.sections[0].paragraphs[range]
        .iter()
        .flat_map(|para| &para.controls)
        .filter(|control| matches!(control, hwp_model::Control::Table(_)))
        .count()
}

/// Extracts `[start_marker, end_marker)` (to the end of the section when `end_marker` is
/// `None`) from `source` into `out`, with a report, and returns the report.
fn extract(
    ws: &Workspace,
    source: &str,
    out: &str,
    start: &str,
    end: Option<&str>,
    extra: &[&str],
) -> Value {
    let from = format!("heading:{start}");
    let to = end.map(|marker| format!("heading:{marker}"));
    let report = format!("{out}.json");
    let mut args = vec![
        "split",
        source,
        "--out-dir",
        out,
        "--from",
        &from,
        "--range-report",
        &report,
    ];
    if let Some(to) = &to {
        args.extend(["--to", to, "--end-exclusive"]);
    }
    args.extend_from_slice(extra);
    hwp_ok(&ws.dir, &args);
    let parsed: Value =
        serde_json::from_slice(&std::fs::read(ws.dir.join(&report)).unwrap()).unwrap();
    assert_report_valid(&parsed);
    parsed
}

/// Asserts that the fragment `out/<stem>-001.<ext>` carries the source body run
/// `[first, end)` unchanged, the same tables and pictures, and exactly the resources the
/// pictures in that run reference.
fn assert_fragment_matches_source(
    ws: &Workspace,
    source_file: &str,
    fragment_file: &str,
    first: usize,
    end: usize,
    report: &Value,
) {
    let source = ws.read(source_file);
    let fragment = ws.read(fragment_file);
    let projected_source = contract::project_form(&source);
    let projected_fragment = contract::project_form(&fragment);

    // Section setup rides ahead of the run, then the run, in order.
    assert_eq!(body_len(&fragment), 1 + (end - first), "fragment body size");
    contract::compare_body_run(&projected_source, first..end, &projected_fragment, 1).unwrap();

    // Tables: the same content, in order, at the addresses the report names.
    let first_table = tables_in(&source, 0..first);
    let run_tables = tables_in(&source, first..end);
    assert_eq!(projected_fragment.tables.len(), run_tables);
    for offset in 0..run_tables {
        contract::compare_table_payload(
            offset,
            &projected_source.tables[first_table + offset],
            &projected_fragment.tables[offset],
        )
        .unwrap();
    }
    let reported_tables: Vec<(u64, u64)> = report["addresses"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["kind"] == "table")
        .map(|a| {
            (
                a["source"]["table"].as_u64().unwrap(),
                a["output"]["table"].as_u64().unwrap(),
            )
        })
        .collect();
    let want: Vec<(u64, u64)> = (0..run_tables as u64)
        .map(|n| (first_table as u64 + n, n))
        .collect();
    assert_eq!(reported_tables, want, "table addresses");

    // Pictures: exactly those inside the run, with the same payload and geometry.
    let run_images: Vec<_> = projected_source
        .images
        .iter()
        .filter(|image| (first..end).any(|index| image.key.starts_with(&format!("s0/p{index}/"))))
        .collect();
    assert_eq!(projected_fragment.images.len(), run_images.len());
    for (before, after) in run_images.iter().zip(&projected_fragment.images) {
        contract::compare_image_payload(before, after).unwrap();
    }

    // Resources: the closure is exactly the referenced payloads, and the report agrees.
    let mut want_hashes: Vec<String> = run_images
        .iter()
        .filter_map(|image| image.payload_sha256.clone())
        .collect();
    want_hashes.sort();
    want_hashes.dedup();
    assert_eq!(projected_fragment.resource_hashes, want_hashes);
    assert_eq!(
        report["resources"]["kept"].as_array().unwrap().len(),
        want_hashes.len()
    );
    assert_eq!(
        report["resources"]["excluded"].as_array().unwrap().len(),
        source.bin_streams.len() - want_hashes.len()
    );

    // Addresses: every body paragraph of the run moved to its offset position.
    let moved: Vec<(u64, u64)> = report["addresses"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["kind"] == "paragraph")
        .map(|a| {
            (
                a["source"]["paragraph"].as_u64().unwrap(),
                a["output"]["paragraph"].as_u64().unwrap(),
            )
        })
        .collect();
    let want_moved: Vec<(u64, u64)> = (first..end)
        .map(|index| (index as u64, 1 + (index - first) as u64))
        .collect();
    assert_eq!(moved, want_moved, "paragraph addresses");
}

fn every_form(source_file: &str, test_name: &str) {
    let Some(ws) = Workspace::new(test_name) else {
        return;
    };
    let markers = ws.markers();
    let total = body_len(&ws.read(source_file));
    let source_hash = ws.sha256(source_file);
    let extension = source_file.rsplit('.').next().unwrap();
    for (index, (marker, first)) in markers.iter().enumerate() {
        let next = markers.get(index + 1);
        let out = format!("out-{index}");
        let report = extract(
            &ws,
            source_file,
            &out,
            marker,
            next.map(|(next_marker, _)| next_marker.as_str()),
            &["--source-sha256", &source_hash],
        );
        let end = next.map_or(total, |(_, paragraph)| *paragraph);
        assert_eq!(report["contract"], "hwp-range-extraction-v1");
        assert_eq!(report["source_sha256"], source_hash.as_str());
        let stem = source_file.rsplit_once('.').unwrap().0;
        let fragment_file = format!("{out}/{stem}-001.{extension}");
        assert_eq!(
            report["fragment_sha256"],
            ws.sha256(&fragment_file).as_str(),
            "report hash names the published fragment"
        );
        assert_fragment_matches_source(&ws, source_file, &fragment_file, *first, end, &report);
        let validate = hwp(&ws.dir, &["validate", &fragment_file]);
        assert!(
            validate.status.success(),
            "fragment is a valid {extension}: {}",
            String::from_utf8_lossy(&validate.stderr)
        );
    }
}

#[test]
fn every_form_extracts_from_hwpx_with_its_own_resources() {
    every_form("source.hwpx", "hwpx");
}

#[test]
fn every_form_extracts_from_hwp5_with_its_own_resources() {
    every_form("source.hwp", "hwp5");
}

/// The application form is the only one whose table holds a picture; every other form must
/// carry no binary at all, and the application fragment carries exactly the retained one.
#[test]
fn only_the_form_that_uses_a_picture_keeps_its_binary() {
    let Some(ws) = Workspace::new("closure") else {
        return;
    };
    let markers = ws.markers();
    let retained = ws.expected["images"]
        .as_array()
        .unwrap()
        .iter()
        .find(|image| image["role"] == "retained")
        .unwrap()["sha256"]
        .as_str()
        .unwrap()
        .to_string();
    let excluded = ws.expected["images"]
        .as_array()
        .unwrap()
        .iter()
        .find(|image| image["role"] == "excluded")
        .unwrap()["sha256"]
        .as_str()
        .unwrap()
        .to_string();
    for (index, (marker, _)) in markers.iter().enumerate() {
        let next = markers.get(index + 1).map(|(m, _)| m.as_str());
        let out = format!("c-{index}");
        extract(&ws, "source.hwpx", &out, marker, next, &[]);
        let fragment = ws.read(&format!("{out}/source-001.hwpx"));
        let hashes: Vec<String> = fragment
            .bin_streams
            .iter()
            .map(|bin| contract::digest(&bin.data))
            .collect();
        if index == 0 {
            assert_eq!(
                hashes,
                vec![retained.clone()],
                "application keeps its picture"
            );
        } else {
            assert!(
                hashes.is_empty(),
                "form {index} keeps no binary: {hashes:?}"
            );
        }
        assert!(
            !hashes.contains(&excluded),
            "the outside picture never travels"
        );
    }
}

/// A refusal is decided before anything is built, so neither the fragment directory nor the
/// report may exist afterwards.
fn assert_refused(ws: &Workspace, args: &[&str], message_part: &str) {
    let mut full = vec!["split"];
    full.extend_from_slice(args);
    full.extend(["--out-dir", "refused", "--range-report", "refused.json"]);
    let result = hwp(&ws.dir, &full);
    assert!(!result.status.success(), "{args:?} must be refused");
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains(message_part), "{args:?}: {stderr}");
    assert!(
        !ws.dir.join("refused").exists(),
        "{args:?} left a fragment dir"
    );
    assert!(
        !ws.dir.join("refused.json").exists(),
        "{args:?} left a report"
    );
}

#[test]
fn refusals_name_the_cause_and_write_nothing() {
    let Some(ws) = Workspace::new("refusals") else {
        return;
    };
    let wrong_hash = "0".repeat(64);
    assert_refused(
        &ws,
        &[
            "source.hwpx",
            "--from",
            "heading:FORM_START_APPLICATION",
            "--source-sha256",
            &wrong_hash,
        ],
        "다릅니다",
    );
    assert_refused(
        &ws,
        &["source.hwpx", "--from", "heading:NO_SUCH_MARKER"],
        "본문 문단이 없습니다",
    );
    // Cell text is not a body paragraph, so it is not an anchor even though it exists.
    assert_refused(
        &ws,
        &["source.hwpx", "--from", "heading:APPLICATION_FORM"],
        "본문 문단이 없습니다",
    );
    assert_refused(
        &ws,
        &["source.hwpx", "--from", "at:0.9999"],
        "본문 문단이 없습니다",
    );
    assert_refused(
        &ws,
        &[
            "source.hwpx",
            "--from",
            "heading:FORM_START_PROFILE_1",
            "--to",
            "heading:FORM_START_APPLICATION",
        ],
        "앞에 있습니다",
    );
    assert_refused(
        &ws,
        &[
            "source.hwpx",
            "--from",
            "heading:FORM_START_PROFILE_1",
            "--to",
            "heading:FORM_START_PROFILE_1",
            "--end-exclusive",
        ],
        "앞에 있습니다",
    );
    assert_refused(
        &ws,
        &["source.hwpx", "--from", "FORM_START_APPLICATION"],
        "잘못된 범위 지점",
    );
    // The report may not overwrite the input.
    let result = hwp(
        &ws.dir,
        &[
            "split",
            "source.hwpx",
            "--out-dir",
            "refused",
            "--from",
            "heading:FORM_START_APPLICATION",
            "--range-report",
            "source.hwpx",
        ],
    );
    assert!(!result.status.success());
    assert!(!ws.dir.join("refused").exists());
}

#[test]
fn a_duplicated_anchor_is_ambiguous_and_an_address_resolves_it() {
    let Some(ws) = Workspace::new("ambiguous") else {
        return;
    };
    hwp_ok(
        &ws.dir,
        &["merge", "source.hwpx", "source.hwpx", "-o", "twice.hwpx"],
    );
    assert_refused(
        &ws,
        &["twice.hwpx", "--from", "heading:FORM_START_APPLICATION"],
        "곳(",
    );
    // A range cannot span the two sections either.
    assert_refused(
        &ws,
        &["twice.hwpx", "--from", "at:0.4", "--to", "at:1.7"],
        "서로 다른 구역",
    );
    // Addressing the second copy works and reads the second section.
    let report = extract_by_address(&ws, "twice.hwpx", "at:1.4", "at:1.7");
    assert_eq!(report["selection"]["section"], 1);
    assert_eq!(report["selection"]["first_paragraph"], 4);
}

fn extract_by_address(ws: &Workspace, source: &str, from: &str, to: &str) -> Value {
    hwp_ok(
        &ws.dir,
        &[
            "split",
            source,
            "--out-dir",
            "addr",
            "--from",
            from,
            "--to",
            to,
            "--end-exclusive",
            "--range-report",
            "addr.json",
        ],
    );
    serde_json::from_slice(&std::fs::read(ws.dir.join("addr.json")).unwrap()).unwrap()
}

#[test]
fn a_range_cannot_be_combined_with_page_splitting() {
    let Some(ws) = Workspace::new("conflict") else {
        return;
    };
    let result = hwp(
        &ws.dir,
        &[
            "split",
            "source.hwpx",
            "--out-dir",
            "x",
            "--pages",
            "1",
            "--from",
            "at:0.4",
        ],
    );
    assert!(!result.status.success());
    assert!(!ws.dir.join("x").exists());
}

/// Forms hold one table each, so a single form cannot tell a correct table counter from a
/// frozen one. A run over several forms, starting after the first, must number the tables in
/// the source from where the run starts and in the fragment from zero.
#[test]
fn a_run_over_several_forms_numbers_every_table() {
    let Some(ws) = Workspace::new("multi") else {
        return;
    };
    let markers = ws.markers();
    for source_file in ["source.hwpx", "source.hwp"] {
        let first = &markers[2];
        let last = &markers[5];
        let out = format!("multi-{source_file}");
        let report = extract(&ws, source_file, &out, &first.0, Some(&last.0), &[]);
        let stem = source_file.rsplit_once('.').unwrap().0;
        let extension = source_file.rsplit('.').next().unwrap();
        let fragment_file = format!("{out}/{stem}-001.{extension}");
        assert_fragment_matches_source(&ws, source_file, &fragment_file, first.1, last.1, &report);
        // profile-1..profile-3 are source tables 2, 3 and 4.
        let numbered: Vec<(u64, u64)> = report["addresses"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["kind"] == "table")
            .map(|a| {
                (
                    a["source"]["table"].as_u64().unwrap(),
                    a["output"]["table"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(numbered, vec![(2, 0), (3, 1), (4, 2)]);
    }
}

/// The schema is closed: drift in a published report must be caught, not tolerated.
#[test]
fn the_schema_rejects_a_drifted_report() {
    let Some(ws) = Workspace::new("schema") else {
        return;
    };
    let good = extract(&ws, "source.hwpx", "s", "FORM_START_APPLICATION", None, &[]);
    assert_report_valid(&good);
    let mut cases: Vec<(&str, Value)> = Vec::new();
    let mut extra_field = good.clone();
    extra_field["note"] = Value::from("x");
    cases.push(("unknown top-level field", extra_field));
    let mut bad_hash = good.clone();
    bad_hash["source_sha256"] = Value::from("ABC");
    cases.push(("malformed hash", bad_hash));
    let mut renamed = good.clone();
    renamed["resources"]["dropped"] = renamed["resources"]["excluded"].take();
    cases.push(("renamed resource list", renamed));
    let mut wrong_kind = good.clone();
    wrong_kind["addresses"][0]["kind"] = Value::from("cell");
    cases.push(("unknown address kind", wrong_kind));
    let mut table_without_ordinal = good.clone();
    let table = good["addresses"]
        .as_array()
        .unwrap()
        .iter()
        .position(|a| a["kind"] == "table")
        .unwrap();
    table_without_ordinal["addresses"][table]["source"]
        .as_object_mut()
        .unwrap()
        .remove("table");
    cases.push(("table address without ordinal", table_without_ordinal));
    let mut bad_version = good;
    bad_version["schema_version"] = Value::from("2.0");
    cases.push(("other schema version", bad_version));
    for (label, report) in cases {
        assert!(
            !report_validator().is_valid(&report),
            "the schema accepted a drifted report: {label}"
        );
    }
}
