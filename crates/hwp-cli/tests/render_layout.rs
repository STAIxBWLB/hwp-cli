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
        ("the only join key", "THE ONLY JOIN KEY"),
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
        ("the nesting hole", "BY NESTING, WHICH IS THE LARGER HOLE"),
        (
            "the scale of the nesting hole",
            "222 paragraphs against this artifact's 40",
        ),
        (
            "the unbounded page numbering",
            "DELIBERATELY UNBOUNDED ABOVE",
        ),
        (
            "that the join is not guaranteed to resolve",
            "THE JOIN IS NOT GUARANTEED TO RESOLVE",
        ),
        (
            "what a consumer does with an unresolved id",
            "MUST THEREFORE TREAT AN UNRESOLVED ID AS GEOMETRY WITH NO SOURCE RANGE, NOT AS A DEFECT",
        ),
        (
            "that a para row can carry a null character range",
            "TWO DIFFERENT THINGS PRODUCE A NULL HERE",
        ),
        (
            "that null source_chars must be checked on para too",
            "A consumer must null-check this field on EVERY kind, `para` included.",
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

/// No upper bound may be placed on a page number or a page count.
///
/// The ordinary render path applies no page budget - `LayoutBudget`'s 4096-page cap is on the
/// certification path only - so any `maximum` here is a number the emitter can exceed, and a
/// long enough document would emit a file that fails this very schema. Clamping the emitter
/// instead was rejected: it would drop pages with no signal, which is the exact failure
/// `truncated` exists to prevent for rows.
#[test]
fn no_upper_bound_is_placed_on_a_page_number_or_page_count() {
    let schema: serde_json::Value = serde_json::from_str(schema_text()).unwrap();
    for (pointer, keyword) in [
        ("/properties/selected_pages", "maxItems"),
        ("/properties/selected_pages/items", "maximum"),
        ("/properties/pages", "maxItems"),
        ("/$defs/page/properties/page", "maximum"),
    ] {
        let node = schema
            .pointer(pointer)
            .unwrap_or_else(|| panic!("{pointer} must exist in the schema"));
        assert!(
            node.get(keyword).is_none(),
            "{pointer} carries {keyword}; the ordinary render path has no page budget, so a \
             bound here is one the emitter can exceed - the file would then fail its own schema"
        );
    }
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

// ---------------------------------------------------------------------------
// D-08 and D-09, as structural properties of the published artifact
// ---------------------------------------------------------------------------

fn rows_of<'a>(value: &'a serde_json::Value, kind: &str) -> Vec<&'a serde_json::Value> {
    value["pages"]
        .as_array()
        .expect("pages")
        .iter()
        .flat_map(|p| p["rows"].as_array().expect("rows"))
        .filter(|r| r["kind"] == kind)
        .collect()
}

fn bbox(row: &serde_json::Value) -> Option<[f64; 4]> {
    let b = row["box"].as_object()?;
    Some([
        b["x0"].as_f64()?,
        b["y0"].as_f64()?,
        b["x1"].as_f64()?,
        b["y1"].as_f64()?,
    ])
}

/// D-08: a table document publishes `table` and `cell` rows, and every cell box on a page lies
/// inside a table box on that page, so an editor hit-tests them directly.
///
/// The tolerance is a point, not zero: `item_bounds` inflates a stroked path by half its
/// width, so a cell's border and its table's border touch by a fraction of a point by
/// construction. Only a real escape is a misattribution.
#[test]
fn a_table_document_publishes_table_and_cell_rows_with_cells_inside_their_table() {
    const SHARED_EDGE_PT: f64 = 1.0;
    let value = layout_of(TABLE_MARKDOWN);
    assert!(
        !rows_of(&value, "table").is_empty(),
        "a table document must publish a table row (D-08)"
    );
    assert!(
        !rows_of(&value, "cell").is_empty(),
        "a table document must publish cell rows (D-08)"
    );

    for page in value["pages"].as_array().expect("pages") {
        let rows = page["rows"].as_array().expect("rows");
        let tables: Vec<[f64; 4]> = rows
            .iter()
            .filter(|r| r["kind"] == "table")
            .filter_map(bbox)
            .collect();
        for cell in rows.iter().filter(|r| r["kind"] == "cell") {
            let Some(c) = bbox(cell) else { continue };
            assert!(
                tables.iter().any(|t| {
                    t[0] <= c[0] + SHARED_EDGE_PT
                        && t[1] <= c[1] + SHARED_EDGE_PT
                        && t[2] + SHARED_EDGE_PT >= c[2]
                        && t[3] + SHARED_EDGE_PT >= c[3]
                }),
                "cell {} at {c:?} lies inside no table box on its page",
                cell["id"]
            );
        }
    }
}

/// D-08a: an invisible segment publishes `box: null` and a character range, and NO row of a
/// point-segment kind carries a zero-extent box.
///
/// The assertion is scoped to the point kinds rather than applied to every row on purpose.
/// Real geometry is legitimately flat in places - a zero-height rule or divider, a degenerate
/// empty line box - so a blanket "no zero-extent box anywhere" rule would go red on correct
/// output. That corner is exactly where the D-08a guard gets quietly relaxed instead of the
/// code being fixed; scoping it keeps the guard true and keeps it a guard.
#[test]
fn a_point_segment_has_a_range_and_no_fabricated_rectangle() {
    let mut doc = hwp_convert::from_markdown("책갈피 문단.\n");
    let para = &mut doc.sections[0].paragraphs[0];
    let control_index = para.controls.len();
    para.controls
        .push(hwp_model::Control::Generic(hwp_model::GenericControl {
            ctrl_id: *b"bokm",
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        }));
    para.chars.insert(
        1,
        hwp_model::HwpChar::ExtCtrl {
            code: hwp_model::paragraph::ctrl_char::BOOKMARK,
            ctrl_id: *b"bokm",
            payload: Vec::new(),
            ctrl_index: Some(control_index as u32),
        },
    );

    let mut store = hwp_render::FontStore::new();
    let mut warnings = hwp_render::RenderIssueAccumulator::new();
    let (list, map) =
        hwp_render::layout::layout_document_with_segments(&doc, &mut store, &mut warnings);
    let selected: Vec<usize> = (1..=list.pages.len()).collect();
    let value = hwp_cli::render_layout::layout_json(&list, &map, &selected);

    if let Err(error) = validator().validate(&value) {
        panic!("a layout artifact carrying a point segment failed the schema: {error}");
    }

    let bookmarks = rows_of(&value, "bookmark");
    assert_eq!(bookmarks.len(), 1, "one anchored bookmark, one row");
    assert_eq!(
        bookmarks[0]["box"],
        serde_json::Value::Null,
        "an invisible segment publishes box: null, never a rectangle"
    );
    assert!(
        bookmarks[0]["source_chars"].is_object(),
        "and it keeps its character range"
    );

    // Point kinds only. A flat box elsewhere is real geometry, not a fabrication.
    for row in rows_of(&value, "bookmark") {
        if let Some(b) = bbox(row) {
            assert!(
                b[0] != b[2] || b[1] != b[3],
                "row {} carries a fabricated zero-extent box (D-08a)",
                row["id"]
            );
        }
    }
}

/// D-09: a segment crossing a page boundary appears once per (segment, page), each row with
/// its own character range; those ranges are pairwise disjoint and their union is contiguous.
///
/// Driven over the committed sample, which does split segments across pages on this host, and
/// asserted as a structural property rather than as a page count - it holds wherever the break
/// lands, so no font decides it.
///
/// THIS TEST DELIBERATELY DOES NOT ASSERT THAT THE CASE WAS REACHED. Where a page break falls
/// is font-dependent, and CI bundles no fonts, so a non-vacuity assertion here would be an
/// assertion about the host's font set. What guarantees the code path is exercised on every
/// host is its synthetic sibling below,
/// `the_artifact_carries_one_row_per_segment_and_page_for_a_page_crossing_segment`, which
/// builds the crossing directly. Neither test substitutes for the other: this one reads real
/// pagination, that one guarantees the shape.
#[test]
fn a_page_crossing_segments_ranges_are_disjoint_and_contiguous() {
    let dir = temp_dir("d09");
    let bytes = render_layout_bytes(&dir, "png", "96", "all");
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON");

    let mut by_id: std::collections::BTreeMap<String, Vec<(u64, u64)>> = Default::default();
    for page in value["pages"].as_array().expect("pages") {
        for row in page["rows"].as_array().expect("rows") {
            if let Some(range) = row["source_chars"].as_object() {
                by_id
                    .entry(row["id"].as_str().expect("id").to_string())
                    .or_default()
                    .push((
                        range["start"].as_u64().expect("start"),
                        range["end"].as_u64().expect("end"),
                    ));
            }
        }
    }

    for (id, ranges) in by_id.iter().filter(|(_, r)| r.len() > 1) {
        let mut sorted = ranges.clone();
        sorted.sort_unstable();
        for pair in sorted.windows(2) {
            assert!(
                pair[0].1 <= pair[1].0,
                "ranges of {id} overlap: {:?} and {:?}",
                pair[0],
                pair[1]
            );
            assert_eq!(
                pair[0].1, pair[1].0,
                "ranges of {id} have a hole between {:?} and {:?}",
                pair[0], pair[1]
            );
        }
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// D-09 end to end and non-vacuously on any host: a paragraph really is split across a page
/// break, and the PUBLISHED artifact carries one row per (segment, page), each with its own box
/// and its own character range.
///
/// THIS TEST EXISTS BECAUSE THE FIXTURE PATH IS FONT-DEPENDENT. Its sibling above reads real
/// pagination on the committed sample and cannot assert that any segment actually crossed a
/// page, because where a break falls depends on shaping and CI bundles no fonts. Do not
/// "simplify" this one back onto the fixture: that would make both tests vacuous together on
/// exactly the host that runs them.
///
/// The split is forced the way 05-04 forces it - by the paragraph's own cached `LineSeg`
/// geometry, where flag bit 0 marks a page-first line. That comes from the MODEL, not from
/// shaping, so it happens whatever fonts the host has. Synthesizing a very long paragraph
/// instead would have inherited the problem it solves: line count is shaping.
#[test]
fn the_published_artifact_carries_one_row_per_segment_and_page_for_a_split_paragraph() {
    let mut doc = hwp_convert::from_markdown("문단 하나.\n");
    let para = &mut doc.sections[0].paragraphs[0];
    para.chars.extend(
        "가나다라마바사아자차카타파하"
            .chars()
            .map(hwp_model::HwpChar::Text),
    );
    let text_len = para.wchar_len();
    let seg = |text_start: u32| hwp_model::paragraph::LineSeg {
        text_start,
        v_pos: 0,
        line_height: 1_600,
        text_height: 1_600,
        baseline_gap: 1_300,
        line_spacing: 0,
        col_start: 0,
        seg_width: 40_000,
        // Both lines are flagged page-first, so the second is a hard break mid-paragraph.
        flags: 0x1,
    };
    para.line_segs = vec![seg(0), seg(text_len / 2)];

    let mut store = hwp_render::FontStore::new();
    let mut warnings = hwp_render::RenderIssueAccumulator::new();
    let (list, map) =
        hwp_render::layout::layout_document_with_segments(&doc, &mut store, &mut warnings);
    let selected: Vec<usize> = (1..=list.pages.len()).collect();
    let value = hwp_cli::render_layout::layout_json(&list, &map, &selected);

    if let Err(error) = validator().validate(&value) {
        panic!("a split paragraph's layout artifact failed the published schema: {error}");
    }

    let mut by_id: std::collections::BTreeMap<String, Vec<(u64, u64, serde_json::Value)>> =
        Default::default();
    for page in value["pages"].as_array().expect("pages") {
        for row in page["rows"].as_array().expect("rows") {
            if row["kind"] != "para" {
                continue;
            }
            if let Some(range) = row["source_chars"].as_object() {
                by_id
                    .entry(row["id"].as_str().expect("id").to_string())
                    .or_default()
                    .push((
                        range["start"].as_u64().expect("start"),
                        range["end"].as_u64().expect("end"),
                        row["box"].clone(),
                    ));
            }
        }
    }

    let split: Vec<_> = by_id.iter().filter(|(_, r)| r.len() > 1).collect();
    assert!(
        !split.is_empty(),
        "no paragraph was split, so this test asserted nothing. The break is driven by \
         LineSeg flags from the model, not by shaping, so this failing means the model-driven \
         break band in layout.rs moved - not that the host lacks fonts. Rows seen: {:?}",
        by_id.keys().collect::<Vec<_>>()
    );

    for (id, rows) in split {
        let mut sorted = rows.clone();
        sorted.sort_by_key(|(start, end, _)| (*start, *end));
        for pair in sorted.windows(2) {
            assert!(
                pair[0].1 <= pair[1].0,
                "ranges of {id} overlap: {:?} and {:?}",
                (pair[0].0, pair[0].1),
                (pair[1].0, pair[1].1)
            );
            assert_eq!(
                pair[0].1,
                pair[1].0,
                "ranges of {id} have a hole between {:?} and {:?}",
                (pair[0].0, pair[0].1),
                (pair[1].0, pair[1].1)
            );
        }
        assert_ne!(
            sorted[0].2, sorted[1].2,
            "each of {id}'s page rows must carry its own box, not a shared one"
        );
    }
}

/// The published artifact must survive an id repeating on ONE page, because two fragments of
/// one cell in different columns of a multi-column section stay two rows rather than being
/// unioned across the gutter.
///
/// This drives the real serializer and the real schema rather than leaving the rule as a
/// comment. NO TEST IN THIS FILE MAY ASSUME ONE ROW PER (page, id): if you are about to add
/// an assertion that groups rows by that pair, this is the case it silently drops.
#[test]
fn two_rows_of_one_id_on_one_page_survive_serialization_and_the_schema() {
    let list = hwp_render::display::DisplayList {
        pages: vec![hwp_render::display::PageList {
            width_pt: 595.0,
            height_pt: 842.0,
            items: Vec::new(),
        }],
    };
    let fragment = |x0: f32, x1: f32| hwp_render::segment_map::SegmentRow {
        page: 0,
        kind: hwp_render::segment_map::kind::CELL,
        id: "abc.0.1.2".into(),
        bbox: Some(hwp_render::segment_map::BoxPt {
            x0,
            y0: 10.0,
            x1,
            y1: 20.0,
        }),
        chars: None,
        item_count: 1,
    };
    let map = hwp_render::segment_map::SegmentMap {
        // One cell, two columns, a gutter between them.
        rows: vec![fragment(10.0, 150.0), fragment(300.0, 400.0)],
        truncated: false,
    };
    let value = hwp_cli::render_layout::layout_json(&list, &map, &[1]);
    if let Err(error) = validator().validate(&value) {
        panic!("the schema rejected two rows of one id on one page: {error}");
    }
    let rows = value["pages"][0]["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 2, "both column fragments must survive");
    assert_eq!(rows[0]["id"], rows[1]["id"]);
    assert_ne!(
        rows[0]["box"], rows[1]["box"],
        "the two fragments keep their own boxes rather than being unioned across the gutter"
    );
}

// ---------------------------------------------------------------------------
// The cross-artifact join key
// ---------------------------------------------------------------------------

/// The cross-artifact join key, over every fixture this checkout has.
///
/// This is the only check that reads the two PUBLISHED artifacts. `hwp-convert` and
/// `hwp-render` derive ids independently by design - that is what keeps them off each other's
/// dependency graph, and `scripts/check-crate-edges.sh` enforces it - so each crate carries its
/// own copy of the id rule and the agreement is made by a test rather than by a shared code
/// path. `segment_id_parity` is the inner guard, comparing the two FUNCTIONS directly; it can
/// pass while a serialization or path-building mistake still makes the published ids disagree.
///
/// # What this asserts, and why it is not "no orphans"
///
/// A layout row id is NOT guaranteed to resolve in the envelope. A paragraph holding only a
/// drawing control produces no envelope segment while the renderer draws it and records a row
/// (issue #285). So the assertion here is the shape of an unresolved id, not its absence:
/// every unresolved id must be a `para` row carrying `source_chars: null`, which is the known
/// case. A `cell` orphan, a `table` orphan, or a `para` orphan that does carry a character
/// range is a NEW divergence and fails.
///
/// Asserting "no orphans" would pin the defect in place and go red the day #285 closes.
/// Asserting "orphans exist" would do the same in the other direction. This assertion is
/// vacuously satisfied once #285 lands, which is the correct behaviour for a guard about a
/// shape rather than a count.
///
/// # Coverage, and the trap in reading a green run
///
/// `fixtures/hwp5/` and `fixtures/hwpx/` are gitignored (CLAUDE.md's data policy), so ON CI
/// THIS TEST SEES ONLY THE COMMITTED FIXTURES and the rest silently do not run - the #275
/// shape, where a skipped case reports `ok`. The two documents that actually exhibit an
/// unresolved id, `annual_report.hwp` and `outline.hwp`, are among the ones CI does not have.
/// A green run here therefore means "the shape held wherever it could be checked on this
/// host", never "the property holds for every document". The committed fixtures are asserted
/// present rather than skipped, so the test cannot degrade to checking nothing at all, and the
/// coverage it achieved is printed.
#[test]
fn an_unresolved_layout_row_id_is_always_a_para_row_with_no_character_range() {
    // (path relative to the repo root, committed and therefore required)
    const FIXTURES: [(&str, bool); 9] = [
        ("fixtures/samples/report-tables.hwpx", true),
        (
            "fixtures/pdf-parity/public/source/public-safety-rfp-p1.hwp",
            true,
        ),
        (
            "fixtures/pdf-parity/public/source/public-safety-rfp-p1.hwpx",
            true,
        ),
        ("fixtures/hwp5/annual_report.hwp", false),
        ("fixtures/hwp5/outline.hwp", false),
        ("fixtures/hwp5/work_report.hwp", false),
        ("fixtures/hwp5/bookmark.hwp", false),
        ("fixtures/hwp5/hello_world.hwp", false),
        ("fixtures/hwpx/minimal.hwpx", false),
    ];

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = temp_dir("joinkey");
    let mut checked = Vec::new();
    let mut absent = Vec::new();

    for (relative, committed) in FIXTURES {
        let input = root.join(relative);
        if !input.is_file() {
            assert!(
                !committed,
                "{relative} is committed and must be present; a skip here would make this \
                 test report ok while checking nothing"
            );
            absent.push(relative);
            continue;
        }

        let layout_path = dir.join("layout.json");
        let out = dir.join("out.png");
        let render = hwp()
            .arg("render")
            .arg(&input)
            .arg("-o")
            .arg(&out)
            .args(["--format", "png"])
            .arg("--layout-json")
            .arg(&layout_path)
            .output()
            .expect("run hwp render --layout-json");
        assert!(
            render.status.success(),
            "hwp render failed on {relative}: {}",
            String::from_utf8_lossy(&render.stderr)
        );

        let envelope_out = hwp()
            .arg("cat")
            .arg(&input)
            .args([
                "--format",
                "markdown",
                "--with-segments",
                "--segments",
                "v2",
            ])
            .output()
            .expect("run hwp cat --with-segments --segments v2");
        assert!(
            envelope_out.status.success(),
            "hwp cat --segments v2 failed on {relative}: {}",
            String::from_utf8_lossy(&envelope_out.stderr)
        );

        let envelope: serde_json::Value =
            serde_json::from_slice(&envelope_out.stdout).expect("the envelope is JSON");
        let envelope_ids: std::collections::BTreeSet<&str> = envelope["segments"]
            .as_array()
            .expect("segments")
            .iter()
            .map(|s| s["id"].as_str().expect("segment id"))
            .collect();

        let layout: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&layout_path).expect("layout file"))
                .expect("JSON");
        let rows: Vec<&serde_json::Value> = layout["pages"]
            .as_array()
            .expect("pages")
            .iter()
            .flat_map(|p| p["rows"].as_array().expect("rows"))
            .collect();

        let mut unresolved = 0;
        for row in &rows {
            let id = row["id"].as_str().expect("row id");
            if envelope_ids.contains(id) {
                continue;
            }
            unresolved += 1;
            assert_eq!(
                row["kind"], "para",
                "{relative}: an unresolved id must be a para row; {id} is a {} - that is a new \
                 divergence, not the documented #285 case",
                row["kind"]
            );
            assert_eq!(
                row["source_chars"],
                serde_json::Value::Null,
                "{relative}: the unresolved para id {id} carries a character range, so it \
                 shaped text and should have an envelope segment - a new divergence, not #285"
            );
        }
        checked.push((relative, rows.len(), envelope_ids.len(), unresolved));
        std::fs::remove_file(&layout_path).ok();
    }

    // Printed, not asserted: the counts move with the fixtures a checkout happens to have.
    for (name, rows, envelope, unresolved) in &checked {
        println!("join key: {name} rows={rows} envelope={envelope} unresolved={unresolved}");
    }
    if !absent.is_empty() {
        println!(
            "join key: NOT CHECKED (gitignored, absent here): {}",
            absent.join(", ")
        );
    }
    assert!(
        checked.len() >= 3,
        "the three committed fixtures must always be checked, only checked {}",
        checked.len()
    );
    std::fs::remove_dir_all(&dir).ok();
}
