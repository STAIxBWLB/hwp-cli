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
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../schemas/render-layout-v1.schema.json"))
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
    let (list, map) = hwp_render::layout::layout_document_with_segments(&doc, &mut store, &mut warnings);
    let selected: Vec<usize> = (1..=list.pages.len()).collect();
    hwp_cli::render_layout::layout_json(&list, &map, &selected)
}

const TABLE_MARKDOWN: &str = "# 표 문서\n\n본문 문단입니다.\n\n| 가 | 나 |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |\n";

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
        ("the unit and origin", "POINTS, WITH THE PAGE ORIGIN AT TOP-LEFT AND Y INCREASING DOWNWARD, AND NO DPI APPLIED"),
        ("the coordinate-space contrast", "IN UTF-16 CODE UNITS INTO THE SOURCE PARAGRAPH"),
        ("the only join key", "THIS IS THE ONLY JOIN KEY"),
        ("the null box", "NULL means this segment produced no display item"),
        ("the glyph-box approximation", "come from the run's `size_pt`, not from the loaded font's ascent and descent"),
        ("the four kinds are a subset", "THE LAYOUT ARTIFACT IS A DELIBERATE SUBSET OF THE ENVELOPE"),
        ("neither the id nor (page, id) is a key", "THE ROW ID IS NOT A KEY, AND NEITHER IS THE PAIR (page, id)"),
        ("the truncation flag", "an incomplete prefix of the document's geometry"),
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
