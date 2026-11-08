//! Typed range extraction (`hwp split --from/--to`, #417): each form of the generated P0 form
//! set is extracted from the HWPX source and from its HWP5 conversion, and compared with the
//! source through the shared original-form contract. Refusals must leave nothing on disk.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use hwp_model::Document;
use serde_json::Value;

#[allow(dead_code)] // This test reuses the projection and comparison layers only.
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
    // The form set holds no reference the closure cannot read, so nothing is kept on suspicion.
    assert!(
        report["resources"]["kept_unverified"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        report["resources"]["unverified_sites"]
            .as_array()
            .unwrap()
            .is_empty()
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
    // The report may not overwrite the input, and the refusal names the flag the user gave.
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
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("--range-report") && !stderr.contains("--loss-report"),
        "alias refusal must name --range-report: {stderr}"
    );
    assert!(!ws.dir.join("refused").exists());
    // The same holds for the loss report on a range run and on a plain split.
    for extra in [vec!["--from", "heading:FORM_START_APPLICATION"], vec![]] {
        let mut args = vec!["split", "source.hwpx", "--out-dir", "refused"];
        args.extend(extra);
        args.extend(["--loss-report", "source.hwpx"]);
        let result = hwp(&ws.dir, &args);
        assert!(!result.status.success());
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("--loss-report"),
            "{args:?}"
        );
        assert!(!ws.dir.join("refused").exists());
    }
    // One path for both reports would publish the range report over the loss ledger.
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
            "same.json",
            "--loss-report",
            "same.json",
        ],
    );
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("--range-report") && stderr.contains("--loss-report"),
        "{stderr}"
    );
    assert!(!ws.dir.join("refused").exists());
    assert!(!ws.dir.join("same.json").exists());
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
    let mut echoed = good.clone();
    echoed["selection"]["from"] = Value::from("heading:FORM_START_APPLICATION");
    cases.push(("anchor text echoed into the selection", echoed));
    let mut unknown_site = good.clone();
    unknown_site["resources"]["unverified_sites"] =
        serde_json::json!([{"code": "guess", "at": "s0/p0"}]);
    cases.push(("unknown unverified-site code", unknown_site));
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

/// `hwp-convert` copies three HWP5 tags because it cannot depend on `hwp5` (hub and spoke).
/// A drifted copy would silently stop recognizing a raw object, so pin them here.
#[test]
fn the_closure_tags_equal_the_hwp5_tags() {
    assert_eq!(
        hwp_convert::document_split::TAG_SHAPE_COMPONENT,
        hwp5::record::tag::SHAPE_COMPONENT
    );
    assert_eq!(
        hwp_convert::document_split::TAG_SHAPE_COMPONENT_OLE,
        hwp5::record::tag::SHAPE_COMPONENT_OLE
    );
    assert_eq!(
        hwp_convert::document_split::TAG_SHAPE_COMPONENT_PICTURE,
        hwp5::record::tag::SHAPE_COMPONENT_PICTURE
    );
}

/// A reference site the closure cannot read must reach the published report and keep the
/// streams, end to end: a border fill with an image fill is added to a real HWP5 form set.
#[test]
fn an_unreadable_reference_site_keeps_every_stream_and_is_reported() {
    let Some(ws) = Workspace::new("unverified") else {
        return;
    };
    let mut doc = ws.read("source.hwp");
    let total = doc.bin_streams.len();
    assert!(total >= 2, "the form set carries both pictures");
    doc.header.border_fills.push(hwp_model::BorderFill {
        fill_type: 0x2,
        ..Default::default()
    });
    let site = doc.header.border_fills.len() - 1;
    hwp5::write_document(
        &doc,
        &ws.dir.join("filled.hwp"),
        &hwp5::WriteOptions::default(),
    )
    .unwrap();
    let report = extract(&ws, "filled.hwp", "u", "FORM_START_PROFILE_1", None, &[]);
    let resources = &report["resources"];
    assert!(resources["excluded"].as_array().unwrap().is_empty());
    assert_eq!(
        resources["kept_unverified"].as_array().unwrap().len(),
        total
    );
    assert_eq!(
        resources["unverified_sites"][0]["code"],
        "border_fill_image"
    );
    assert_eq!(
        resources["unverified_sites"][0]["at"],
        format!("header/border_fills/{site}").as_str()
    );
    assert_eq!(ws.read("u/filled-001.hwp").bin_streams.len(), total);
}

/// The report is declared content-free. A heading is document text and a bookmark name is
/// author text, so neither may be echoed into the published file or the MCP reply: the report
/// names each anchor by kind and resolved address only.
#[test]
fn the_report_never_echoes_anchor_text() {
    let Some(ws) = Workspace::new("content-free") else {
        return;
    };
    let report = extract(
        &ws,
        "source.hwpx",
        "cf",
        "FORM_START_PROFILE_1",
        Some("FORM_START_PROFILE_3"),
        &[],
    );
    let on_disk = std::fs::read_to_string(ws.dir.join("cf.json")).unwrap();
    for anchor_text in ["FORM_START_PROFILE_1", "FORM_START_PROFILE_3", "heading:"] {
        assert!(
            !on_disk.contains(anchor_text),
            "the range report carries anchor text {anchor_text:?}"
        );
    }
    assert_eq!(report["selection"]["from_kind"], "heading");
    assert_eq!(report["selection"]["to_kind"], "heading");
    // The anchors are still pinned down, by the address they resolved to.
    assert_eq!(report["selection"]["first_paragraph"], 10);
    assert_eq!(report["selection"]["end_paragraph"], 16);
    // An address anchor and an open end are reported as such.
    hwp_ok(
        &ws.dir,
        &[
            "split",
            "source.hwpx",
            "--out-dir",
            "cf2",
            "--from",
            "at:0.10",
            "--range-report",
            "cf2.json",
        ],
    );
    let open: Value =
        serde_json::from_slice(&std::fs::read(ws.dir.join("cf2.json")).unwrap()).unwrap();
    assert_report_valid(&open);
    assert_eq!(open["selection"]["from_kind"], "address");
    assert!(open["selection"]["to_kind"].is_null());
}

/// An HWPX whose manifest id is not the file stem (`id="logo" href="BinData/p1.png"`) is a
/// valid document: its picture must resolve through the manifest, not be refused as lost.
#[test]
fn a_manifest_id_that_differs_from_the_file_name_extracts() {
    let Some(ws) = Workspace::new("manifest-id") else {
        return;
    };
    rewrite_zip(
        &ws.dir.join("source.hwpx"),
        &ws.dir.join("odd.hwpx"),
        |name, data| match name {
            "BinData/image2.png" => Some(("BinData/p1.png".to_string(), data)),
            "Contents/content.hpf" => Some((
                name.to_string(),
                swap_text(
                    data,
                    &[
                        ("id=\"image2\"", "id=\"logo\""),
                        ("BinData/image2.png", "BinData/p1.png"),
                    ],
                ),
            )),
            _ if name.starts_with("Contents/section") && name.ends_with(".xml") => Some((
                name.to_string(),
                swap_text(
                    data,
                    &[("binaryItemIDRef=\"image2\"", "binaryItemIDRef=\"logo\"")],
                ),
            )),
            _ => Some((name.to_string(), data)),
        },
    );
    let report = extract(
        &ws,
        "odd.hwpx",
        "odd-out",
        "FORM_START_APPLICATION",
        Some("FORM_START_DEVELOPMENT"),
        &[],
    );
    assert_eq!(
        report["resources"]["kept"],
        serde_json::json!(["BinData/p1.png"])
    );
}

/// A package part the HWPX writer copies back verbatim is part of the fragment: a reference in
/// it keeps the stream, and a part that cannot be read keeps every stream and is reported.
#[test]
fn a_carried_package_part_is_part_of_the_closure() {
    let Some(ws) = Workspace::new("carried") else {
        return;
    };
    // Add a master page that references the picture no form uses (`image1`, the manifest's
    // "excluded" one). The range below (profile-1) contains no picture, so only the carried
    // part can keep that stream.
    let outside = ws.expected["images"]
        .as_array()
        .unwrap()
        .iter()
        .find(|image| image["role"] == "excluded")
        .unwrap()["sha256"]
        .as_str()
        .unwrap()
        .to_string();
    add_zip_entry(
        &ws.dir.join("source.hwpx"),
        &ws.dir.join("carried.hwpx"),
        "Contents/masterpage0.xml",
        br#"<masterPage><hc:img binaryItemIDRef="image1"/></masterPage>"#,
    );
    let report = extract(
        &ws,
        "carried.hwpx",
        "carried-out",
        "FORM_START_PROFILE_1",
        Some("FORM_START_PROFILE_2"),
        &[],
    );
    let kept = report["resources"]["kept"].as_array().unwrap();
    assert_eq!(kept.len(), 1, "{report}");
    let fragment = ws.read("carried-out/carried-001.hwpx");
    assert_eq!(
        fragment
            .bin_streams
            .iter()
            .map(|b| contract::digest(&b.data))
            .collect::<Vec<_>>(),
        vec![outside],
        "the picture only the carried part references travels with the fragment"
    );
    assert!(
        report["resources"]["unverified_sites"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // The same part, unreadable: nothing is excluded and the part is named.
    add_zip_entry(
        &ws.dir.join("source.hwpx"),
        &ws.dir.join("unread.hwpx"),
        "Scripts/macro.bin",
        &[0xff, 0xfe, 0x00, 0x01],
    );
    let report = extract(
        &ws,
        "unread.hwpx",
        "unread-out",
        "FORM_START_PROFILE_1",
        Some("FORM_START_PROFILE_2"),
        &[],
    );
    assert!(
        report["resources"]["excluded"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        report["resources"]["unverified_sites"][0]["code"],
        "carried_part_unread"
    );
    assert_eq!(
        report["resources"]["unverified_sites"][0]["at"],
        "package/Scripts/macro.bin"
    );
}

/// HWP5 writes only streams its BIN_DATA table names, so a stream the closure keeps on
/// suspicion can still be dropped by the format. The report must say what the published file
/// holds, not what the closure intended.
#[test]
fn the_report_lists_only_streams_the_published_hwp5_holds() {
    let Some(ws) = Workspace::new("orphan") else {
        return;
    };
    let mut doc = ws.read("source.hwp");
    doc.header.border_fills.push(hwp_model::BorderFill {
        fill_type: 0x2,
        ..Default::default()
    });
    hwp5::write_document(
        &doc,
        &ws.dir.join("filled.hwp"),
        &hwp5::WriteOptions::default(),
    )
    .unwrap();
    // An orphan stream: present in the container, named by no BIN_DATA row.
    {
        use std::io::Write as _;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(ws.dir.join("filled.hwp"))
            .unwrap();
        let mut cfb = cfb::CompoundFile::open(file).unwrap();
        cfb.create_new_stream("/BinData/BIN0099.png")
            .unwrap()
            .write_all(b"orphan payload no table row names")
            .unwrap();
        cfb.flush().unwrap();
    }
    let loaded = ws.read("filled.hwp");
    assert!(
        loaded.bin_streams.iter().any(|b| b.name == "BIN0099.png"),
        "the reader loads the orphan"
    );
    let report = extract(
        &ws,
        "filled.hwp",
        "orphan-out",
        "FORM_START_PROFILE_1",
        None,
        &[],
    );
    let resources = &report["resources"];
    let fragment = ws.read("orphan-out/filled-001.hwp");
    // Everything the report calls kept is really in the file...
    let held: Vec<String> = fragment
        .bin_streams
        .iter()
        .map(|b| b.name.clone())
        .collect();
    for list in ["kept", "kept_unverified"] {
        for name in resources[list].as_array().unwrap() {
            assert!(
                held.contains(&name.as_str().unwrap().to_string()),
                "{list} names {name} but the published file holds {held:?}"
            );
        }
    }
    // ...and the orphan the format dropped is reported as such, not as kept.
    assert_eq!(
        resources["dropped_by_format"],
        serde_json::json!(["BIN0099.png"])
    );
    assert!(!held.contains(&"BIN0099.png".to_string()));
    assert_report_valid(&report);
}

/// Rewrites `from` into `to`, passing every entry through `edit`, which returns the (possibly
/// renamed) entry or `None` to drop it.
fn rewrite_zip(
    from: &Path,
    to: &Path,
    mut edit: impl FnMut(&str, Vec<u8>) -> Option<(String, Vec<u8>)>,
) {
    use std::io::{Read as _, Write as _};
    let mut archive = zip::ZipArchive::new(std::fs::File::open(from).unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(std::fs::File::create(to).unwrap());
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().unwrap().into_owned();
        // `mimetype` must stay stored for a valid HWPX package.
        let method = if name == "mimetype" {
            zip::CompressionMethod::Stored
        } else {
            entry.compression()
        };
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        if name.ends_with('/') {
            continue;
        }
        if let Some((new_name, new_data)) = edit(&name, data) {
            writer
                .start_file(
                    new_name,
                    zip::write::SimpleFileOptions::default().compression_method(method),
                )
                .unwrap();
            writer.write_all(&new_data).unwrap();
        }
    }
    writer.finish().unwrap();
}

fn add_zip_entry(from: &Path, to: &Path, name: &str, data: &[u8]) {
    use std::io::Write as _;
    let mut archive = zip::ZipArchive::new(std::fs::File::open(from).unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(std::fs::File::create(to).unwrap());
    for index in 0..archive.len() {
        writer
            .raw_copy_file(archive.by_index_raw(index).unwrap())
            .unwrap();
    }
    writer
        .start_file(name, zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(data).unwrap();
    writer.finish().unwrap();
}

fn swap_text(data: Vec<u8>, replacements: &[(&str, &str)]) -> Vec<u8> {
    let mut text = String::from_utf8(data).unwrap();
    for (from, to) in replacements {
        assert!(text.contains(from), "expected {from:?} in the part");
        text = text.replace(from, to);
    }
    text.into_bytes()
}

/// The fragment and the range report are published in one transaction, so they must be two
/// files. A report path that reaches the fragment through `..` is the same file even though
/// its spelling differs, and publishing both would leave JSON where the fragment should be.
#[test]
fn a_report_path_that_reaches_the_fragment_through_dotdot_is_refused() {
    let Some(ws) = Workspace::new("dotdot") else {
        return;
    };
    std::fs::create_dir_all(ws.dir.join("out")).unwrap();
    for (flag, spelled) in [
        ("--range-report", "out/../out/source-001.hwpx"),
        ("--range-report", "./out/source-001.hwpx"),
        ("--loss-report", "out/../out/source-001.hwpx"),
    ] {
        let result = hwp(
            &ws.dir,
            &[
                "split",
                "source.hwpx",
                "--out-dir",
                "out",
                "--from",
                "heading:FORM_START_PROFILE_1",
                flag,
                spelled,
            ],
        );
        assert!(!result.status.success(), "{flag} {spelled} must be refused");
        assert!(
            !ws.dir.join("out/source-001.hwpx").exists(),
            "{flag} {spelled}: nothing may be published"
        );
    }
    // Control: a report beside the fragment is fine and the fragment stays a package.
    hwp_ok(
        &ws.dir,
        &[
            "split",
            "source.hwpx",
            "--out-dir",
            "out",
            "--from",
            "heading:FORM_START_PROFILE_1",
            "--range-report",
            "out/../report.json",
        ],
    );
    let fragment = std::fs::read(ws.dir.join("out/source-001.hwpx")).unwrap();
    assert_eq!(&fragment[..2], b"PK");
}

/// `--source-sha256` must describe the bytes the range was extracted from.
#[test]
fn the_reported_source_hash_is_the_hash_of_the_extracted_file() {
    let Some(ws) = Workspace::new("hash-once") else {
        return;
    };
    let report = extract(&ws, "source.hwpx", "h", "FORM_START_PROFILE_1", None, &[]);
    assert_eq!(report["source_sha256"], ws.sha256("source.hwpx").as_str());
}
