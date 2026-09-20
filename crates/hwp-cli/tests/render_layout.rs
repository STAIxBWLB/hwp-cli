//! The published render layout contract, `schemas/render-layout-v1.schema.json`.
//!
//! The schema is compiled into this file with `include_str!` and a Draft 2020-12 validator, the
//! shape `render-report-v1` and `segment-envelope-v2` already use. It is deliberately NOT
//! registered in `scripts/check-structured-corpus.sh`: an integration test runs on every
//! `cargo test`, and a gate only a human can trigger is not a gate.
//!
//! Every assertion here is font-independent. CI bundles no fonts, so a page count, a glyph or a
//! coordinate literal would be asserting on this host's font set rather than on the renderer.
//! The two identity tests compare our own output to our own output on one host, so whatever
//! fonts that host has, all sides see the same ones.

use std::path::PathBuf;

/// The published v1 contract, compiled into the test.
fn validator() -> jsonschema::Validator {
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../schemas/render-layout-v1.schema.json"
    ))
    .expect("the layout schema is valid JSON");
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .expect("the layout schema compiles as a Draft 2020-12 schema")
}

fn schema_text() -> &'static str {
    include_str!("../../../schemas/render-layout-v1.schema.json")
}

fn sample() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/samples/report-tables.hwpx")
}

/// A live layout artifact, built through the real layout pass and the real serializer.
///
/// Driven from markdown rather than from a fixture so the cases that matter - a table, a
/// paragraph, an empty selection - are chosen here instead of being whatever some committed
/// file happens to contain.
fn layout_of(markdown: &str) -> serde_json::Value {
    let doc = hwp_convert::from_markdown(markdown);
    let mut store = hwp_render::FontStore::new();
    let mut warnings = hwp_render::RenderIssueAccumulator::new();
    let (list, map) =
        hwp_render::layout::layout_document_with_segments(&doc, &mut store, &mut warnings);
    let selected: Vec<usize> = (1..=list.pages.len()).collect();
    hwp_cli::render_layout::layout_json(&list, &map, &selected)
}

const TABLE_MARKDOWN: &str =
    "# 표 문서\n\n본문 문단입니다.\n\n| 가 | 나 |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |\n";

// ---------------------------------------------------------------------------
// The schema gate
// ---------------------------------------------------------------------------

#[test]
fn a_live_layout_artifact_validates_against_the_published_schema() {
    let value = layout_of(TABLE_MARKDOWN);
    assert_eq!(value["contract"], "hwp-render-layout-v1");
    assert_eq!(value["schema_version"], "1.0");
    if let Err(error) = validator().validate(&value) {
        panic!("the published schema rejected a live layout artifact: {error}");
    }
}

/// The schema is closed, so a field-name typo fails the gate instead of disappearing silently.
///
/// Walked by the test rather than read by a person, and run by `cargo test` rather than by a
/// human typing a one-liner. Subschema keywords are skipped: an `anyOf` / `oneOf` / `if` body
/// legitimately carries a bare fragment that constrains a few keys without being an object
/// definition of its own.
#[test]
fn the_schema_is_closed_at_every_object_level() {
    const FRAGMENT_KEYWORDS: [&str; 7] = [
        "if",
        "then",
        "else",
        "dependentSchemas",
        "allOf",
        "anyOf",
        "oneOf",
    ];

    fn walk(node: &serde_json::Value, path: &str, bad: &mut Vec<String>, levels: &mut usize) {
        if let Some(map) = node.as_object() {
            let is_object_definition = map.get("type") == Some(&serde_json::json!("object"))
                || map.contains_key("properties");
            if is_object_definition {
                *levels += 1;
                if map.get("additionalProperties") != Some(&serde_json::json!(false)) {
                    bad.push(format!("{path}: not closed"));
                }
                match map.get("required").and_then(|r| r.as_array()) {
                    None => bad.push(format!("{path}: no required array")),
                    Some(required) if required.is_empty() => {
                        bad.push(format!("{path}: empty required array"))
                    }
                    Some(required) => {
                        for name in required {
                            let name = name.as_str().unwrap_or_default();
                            if map.get("properties").and_then(|p| p.get(name)).is_none() {
                                bad.push(format!("{path}: required {name:?} is not a property"));
                            }
                        }
                    }
                }
            }
            for (k, v) in map {
                if FRAGMENT_KEYWORDS.contains(&k.as_str()) {
                    continue;
                }
                walk(v, &format!("{path}.{k}"), bad, levels);
            }
        } else if let Some(items) = node.as_array() {
            for (i, v) in items.iter().enumerate() {
                walk(v, &format!("{path}[{i}]"), bad, levels);
            }
        }
    }

    let schema: serde_json::Value = serde_json::from_str(schema_text()).unwrap();
    let (mut bad, mut levels) = (Vec::new(), 0usize);
    walk(&schema, "$", &mut bad, &mut levels);
    assert!(
        bad.is_empty(),
        "the schema is not closed:\n{}",
        bad.join("\n")
    );
    assert!(
        levels >= 5,
        "expected every object level to be walked, saw only {levels}"
    );
}

/// No uniqueness constraint may reach the row array, at any nesting depth.
///
/// Asserted structurally rather than by grepping the file, because the word `uniqueItems`
/// legitimately appears twice in this schema: once as a real constraint on `selected_pages`,
/// whose page numbers genuinely are distinct, and once inside the `rows` description saying
/// the opposite about rows. A grep cannot tell those apart; this can.
#[test]
fn no_uniqueness_constraint_applies_to_rows_or_to_a_row() {
    let schema: serde_json::Value = serde_json::from_str(schema_text()).unwrap();
    for pointer in [
        "/$defs/page/properties/rows",
        "/$defs/row",
        "/$defs/box_pt",
        "/$defs/source_chars",
    ] {
        let node = schema
            .pointer(pointer)
            .unwrap_or_else(|| panic!("{pointer} must exist in the schema"));
        assert!(
            node.get("uniqueItems").is_none(),
            "{pointer} carries a uniqueItems constraint; an id repeats across pages (D-09), \
             across a split table's replayed header rows, and across two column fragments of \
             one cell on ONE page - so neither the id nor (page, id) is a key"
        );
    }
}

/// The published `kind` enum is the four kinds that produce geometry, and no more.
///
/// A `run`, an `image` or a `field` produces no row: an image's and a field's display items
/// fold into the row of the paragraph carrying them. A test demanding one of those would go
/// red on correct output, which is why the closed enum is pinned here rather than left to
/// prose.
#[test]
fn the_kind_enum_is_the_four_kinds_that_produce_geometry() {
    let schema: serde_json::Value = serde_json::from_str(schema_text()).unwrap();
    let kinds = schema["$defs"]["row"]["properties"]["kind"]["enum"]
        .as_array()
        .expect("the row kind is a closed enum");
    assert_eq!(
        kinds,
        &vec![
            serde_json::json!("para"),
            serde_json::json!("table"),
            serde_json::json!("cell"),
            serde_json::json!("bookmark"),
        ],
        "the layout artifact is a deliberate subset of the envelope's seven kinds"
    );
}

/// The load-bearing descriptions are published text, not code comments.
///
/// Each of these is a sentence a consumer has to read to use the artifact correctly, and each
/// one corresponds to a way the artifact is misread when it is absent.
#[test]
fn the_load_bearing_descriptions_are_present_in_the_published_text() {
    let text = schema_text();
    for (what, needle) in [
        (
            "the unit and origin",
            "POINTS, WITH THE PAGE ORIGIN AT TOP-LEFT AND Y INCREASING DOWNWARD, AND NO DPI APPLIED",
        ),
        (
            "the coordinate-space contrast",
            "IN UTF-16 CODE UNITS INTO THE SOURCE PARAGRAPH",
        ),
        ("the only join key", "THIS IS THE ONLY JOIN KEY"),
        (
            "the null box",
            "NULL means this segment produced no display item",
        ),
        (
            "the glyph-box approximation",
            "come from the run's `size_pt`, not from the loaded font's ascent and descent",
        ),
        (
            "the four kinds are a subset",
            "THE LAYOUT ARTIFACT IS A DELIBERATE SUBSET OF THE ENVELOPE",
        ),
        (
            "neither the id nor (page, id) is a key",
            "THE ROW ID IS NOT A KEY, AND NEITHER IS THE PAIR (page, id)",
        ),
        (
            "the truncation flag",
            "an incomplete prefix of the document's geometry",
        ),
    ] {
        assert!(
            text.contains(needle),
            "the schema no longer publishes {what}; expected to find {needle:?}"
        );
    }
}

/// `truncated` is published, so a consumer can tell a capped row set from a complete one.
#[test]
fn the_truncated_flag_is_required_and_published() {
    let schema: serde_json::Value = serde_json::from_str(schema_text()).unwrap();
    assert!(
        schema["required"]
            .as_array()
            .expect("required")
            .contains(&serde_json::json!("truncated")),
        "truncated must be required: an absent row means nothing when the set was capped"
    );
    let value = layout_of(TABLE_MARKDOWN);
    assert_eq!(value["truncated"], serde_json::Value::Bool(false));
}

/// A closed schema must actually reject what it does not admit, or `additionalProperties`
/// is decoration. Both halves matter: an unknown field, and a kind outside the four.
#[test]
fn the_schema_rejects_an_unknown_field_and_an_unpublished_kind() {
    let validator = validator();

    let mut extra = layout_of(TABLE_MARKDOWN);
    extra["surprise"] = serde_json::json!(1);
    assert!(
        !validator.is_valid(&extra),
        "a closed top level must reject an unknown field"
    );

    let mut bad_kind = layout_of(TABLE_MARKDOWN);
    let row = bad_kind["pages"][0]["rows"]
        .as_array_mut()
        .expect("rows")
        .first_mut()
        .expect("the table document produces at least one row");
    row["kind"] = serde_json::json!("run");
    assert!(
        !validator.is_valid(&bad_kind),
        "`run` produces no geometry row, so the kind enum must reject it"
    );
}

/// The sample the cross-artifact join-key test uses is committed, so these tests fail loudly
/// rather than skipping the way the gitignored-corpus tests do.
#[test]
fn the_committed_sample_is_present() {
    assert!(
        sample().is_file(),
        "fixtures/samples/report-tables.hwpx is committed; it must be present"
    );
}

// ---------------------------------------------------------------------------
// The flag, and the three identity proofs
// ---------------------------------------------------------------------------

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "hwp-render-layout-{label}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&path).unwrap();
    path
}

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

/// Runs one render with `--layout-json` and returns the layout file's bytes.
fn render_layout_bytes(dir: &std::path::Path, format: &str, dpi: &str, pages: &str) -> Vec<u8> {
    let out = dir.join(format!("out-{format}-{dpi}-{pages}.{format}"));
    let layout = dir.join(format!("layout-{format}-{dpi}-{pages}.json"));
    let result = hwp()
        .arg("render")
        .arg(sample())
        .arg("-o")
        .arg(&out)
        .args(["--format", format, "--dpi", dpi, "--pages", pages])
        .arg("--layout-json")
        .arg(&layout)
        .output()
        .expect("run hwp render --layout-json");
    assert!(
        result.status.success(),
        "hwp render --format {format} failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    std::fs::read(&layout).expect("the layout file was written")
}

/// ROADMAP criterion 2's "for PNG, SVG and PDF renders alike", as one byte comparison.
///
/// It is one comparison rather than a directory walk because the artifact is one file per
/// invocation. A difference here is a backend transform - the PDF backend's y-flip, the raster
/// backend's dpi scale - having reached a published coordinate.
#[test]
fn the_layout_file_is_byte_identical_across_png_svg_and_pdf() {
    let dir = temp_dir("backends");
    let png = render_layout_bytes(&dir, "png", "96", "all");
    let svg = render_layout_bytes(&dir, "svg", "96", "all");
    let pdf = render_layout_bytes(&dir, "pdf", "96", "all");
    assert!(!png.is_empty(), "the layout file must not be empty");
    assert_eq!(
        png, svg,
        "png and svg layout files differ; a backend transform reached a published coordinate"
    );
    assert_eq!(
        png, pdf,
        "png and pdf layout files differ; the pdf y-flip reached a published coordinate"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// D-07 as a test rather than as a claim: no dpi is baked into a coordinate.
#[test]
fn the_layout_file_is_byte_identical_across_two_dpi_values() {
    let dir = temp_dir("dpi");
    let low = render_layout_bytes(&dir, "png", "72", "all");
    let high = render_layout_bytes(&dir, "png", "300", "all");
    assert_eq!(
        low, high,
        "layout files differ across dpi; a coordinate is in pixels under a schema that says points"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The layout file holds the same line the render report holds: no filesystem paths.
#[test]
fn the_layout_file_contains_no_input_or_output_path() {
    let dir = temp_dir("paths");
    let bytes = render_layout_bytes(&dir, "png", "96", "all");
    let text = String::from_utf8(bytes).expect("the layout file is UTF-8");
    for needle in [
        "report-tables",
        "fixtures",
        ".hwpx",
        ".png",
        "out-png",
        "layout-png",
    ] {
        assert!(
            !text.contains(needle),
            "the layout file leaks {needle:?}; it must carry no input path, output path or filename"
        );
    }
    assert!(
        !text.contains(dir.to_str().expect("utf-8 temp dir")),
        "the layout file leaks its own directory path"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// A `--pages` subset emits rows for the selected pages only, and names them.
///
/// No page count is asserted: page 1 exists in any document that renders at all, and the
/// property under test is that the selection reaches both `selected_pages` and `pages`.
#[test]
fn a_pages_subset_emits_rows_for_the_selected_pages_only_and_names_them() {
    let dir = temp_dir("subset");
    let bytes = render_layout_bytes(&dir, "png", "96", "1");
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON");
    if let Err(error) = validator().validate(&value) {
        panic!("a subset layout file failed the published schema: {error}");
    }
    assert_eq!(value["selected_pages"], serde_json::json!([1]));
    let pages = value["pages"].as_array().expect("pages");
    assert_eq!(pages.len(), 1, "only the selected page may appear");
    assert_eq!(pages[0]["page"], 1);
    std::fs::remove_dir_all(&dir).ok();
}

/// The destination vetting `--report` uses covers `--layout-json` too: it may not clobber the
/// input, a render output, or the report beside it. A guard that does not guard is worse than
/// no guard, so each of the three is driven rather than assumed.
#[test]
fn the_layout_destination_cannot_clobber_the_input_an_output_or_the_report() {
    let dir = temp_dir("clobber");
    let out = dir.join("out.pdf");
    let report = dir.join("report.json");

    let refuses = |layout: &std::path::Path, extra: &[&str]| {
        let result = hwp()
            .arg("render")
            .arg(sample())
            .arg("-o")
            .arg(&out)
            .args(["--format", "pdf"])
            .arg("--layout-json")
            .arg(layout)
            .args(extra)
            .output()
            .expect("run hwp render");
        assert!(
            !result.status.success(),
            "render accepted a layout destination it must refuse: {}",
            layout.display()
        );
    };

    refuses(&sample(), &[]);
    refuses(&out, &[]);
    let report_arg = report.to_str().expect("utf-8").to_string();
    refuses(&report, &["--report", &report_arg]);

    // The input must still be intact after every refusal.
    assert!(sample().is_file(), "the input document must be untouched");
    std::fs::remove_dir_all(&dir).ok();
}

/// Recording is opt-in, and a render without the flag must not produce a layout file anywhere.
#[test]
fn a_render_without_the_flag_writes_no_layout_file() {
    let dir = temp_dir("optin");
    let out = dir.join("out.pdf");
    let result = hwp()
        .arg("render")
        .arg(sample())
        .arg("-o")
        .arg(&out)
        .args(["--format", "pdf"])
        .output()
        .expect("run hwp render");
    assert!(result.status.success());
    let strays: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name()))
        .filter(|n| n.to_string_lossy().ends_with(".json"))
        .collect();
    assert!(
        strays.is_empty(),
        "a render without --layout-json wrote {strays:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
