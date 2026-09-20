//! v1 baseline for `hwp cat --with-segments`.
//!
//! What this pins: the exact bytes today's default `--with-segments` run emits, plus the
//! behaviours around it — the format rejection matrix, the `markdown` field equalling the
//! plain markdown output, and the span invariants (sorted, non-overlapping, offsets as
//! Unicode scalars).
//!
//! Why the pin had to be generated first: D-04 promises the v0.8.x default output stays
//! byte-identical for one release after the segment model v2 lands. A golden taken from a
//! build that already carries v2 code records whatever that build emits and proves nothing.
//! `tests/golden/segment-envelope-v1.json` was therefore generated from
//! `fixtures/samples/report-tables.hwpx` at commit ac4b019 (origin/main), before a single
//! line of v2 code existed, and committed on its own ahead of every other change in phase 05.
//! It is never regenerated: a later regeneration would silently weaken D-04 rather than fail.
//!
//! The source sample is `fixtures/samples/report-tables.hwpx`, which is committed (the
//! `fixtures/samples/` exception in CLAUDE.md's data policy). So these tests fail loudly when
//! something is wrong instead of skipping the way the gitignored-corpus tests do. It carries
//! Korean text and tables, so the pin is not ASCII-only.
//!
//! Plan 05-05 extends this file with the v2 assertions and flips the `json` row of the
//! rejection matrix to accepted per D-03. Capturing the current matrix here makes that flip a
//! visible one-line diff rather than an invisible behaviour change.

use std::path::PathBuf;
use std::process::Command;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn sample() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/samples/report-tables.hwpx")
}

/// Raw stdout of the default `--with-segments` run.
fn segments_stdout() -> Vec<u8> {
    let out = hwp()
        .arg("cat")
        .arg(sample())
        .args(["--format", "markdown", "--with-segments"])
        .output()
        .expect("run hwp cat --with-segments");
    assert!(
        out.status.success(),
        "hwp cat --with-segments failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// The D-04 pin: a fresh run must be byte-identical to the committed golden.
#[test]
fn envelope_matches_the_v1_golden_byte_for_byte() {
    let golden = include_bytes!("golden/segment-envelope-v1.json");
    let fresh = segments_stdout();
    assert!(
        !golden.is_empty(),
        "the golden is empty; it must hold the pre-v2 envelope bytes"
    );
    if fresh != golden {
        // Show where they diverge rather than dumping 26 KB of JSON twice.
        let at = fresh
            .iter()
            .zip(golden.iter())
            .position(|(a, b)| a != b)
            .unwrap_or_else(|| fresh.len().min(golden.len()));
        let from = at.saturating_sub(60);
        panic!(
            "default --with-segments output moved; D-04 pins it byte-identical.\n\
             first difference at byte {at} (fresh {} bytes, golden {} bytes)\n\
             fresh : ...{}\n\
             golden: ...{}",
            fresh.len(),
            golden.len(),
            String::from_utf8_lossy(&fresh[from..(at + 60).min(fresh.len())]),
            String::from_utf8_lossy(&golden[from..(at + 60).min(golden.len())]),
        );
    }
}

/// The envelope's `markdown` field equals the plain `--format markdown` output. This is the
/// structural guarantee the single-emission-core design gives: the segment map is derived from
/// the same emission run, not from a second one.
#[test]
fn envelope_markdown_field_equals_plain_markdown_output() {
    let env: serde_json::Value =
        serde_json::from_slice(&segments_stdout()).expect("parse the envelope");
    let md = env["markdown"].as_str().expect("markdown field");

    let plain = hwp()
        .arg("cat")
        .arg(sample())
        .args(["--format", "markdown"])
        .output()
        .expect("run hwp cat --format markdown");
    assert!(plain.status.success(), "hwp cat --format markdown failed");
    let plain_md = String::from_utf8(plain.stdout).expect("markdown output is utf-8");

    assert_eq!(
        md, plain_md,
        "the markdown field must equal the plain output"
    );
}

/// The v1 rejection matrix. `--with-segments` is markdown-only and incompatible with
/// `--preview`. 05-05 flips the `json` row per D-03; that flip should show up here as a diff.
#[test]
fn with_segments_is_rejected_outside_markdown_and_with_preview() {
    for format in ["plain", "html", "csv", "json"] {
        let out = hwp()
            .arg("cat")
            .arg(sample())
            .args(["--format", format, "--with-segments"])
            .output()
            .expect("run hwp cat");
        assert!(
            !out.status.success(),
            "--with-segments --format {format} must be rejected in v1"
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("--with-segments는 --format markdown 전용입니다"),
            "--format {format} error text changed: {err}"
        );
    }

    let out = hwp()
        .arg("cat")
        .arg(sample())
        .args(["--format", "markdown", "--with-segments", "--preview"])
        .output()
        .expect("run hwp cat --preview");
    assert!(
        !out.status.success(),
        "--with-segments --preview must be rejected"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("--preview와 함께 쓸 수 없습니다"),
        "--preview error text changed: {err}"
    );
}

/// Offsets are Unicode scalar positions, not byte positions, and the spans are sorted and
/// non-overlapping. The sample is Korean, so a byte-offset regression fails here rather than
/// passing silently the way it would on ASCII.
#[test]
fn spans_are_sorted_non_overlapping_scalar_offsets() {
    let env: serde_json::Value =
        serde_json::from_slice(&segments_stdout()).expect("parse the envelope");
    let md = env["markdown"].as_str().expect("markdown field");
    assert_ne!(
        md.len(),
        md.chars().count(),
        "the sample must contain non-ASCII text, otherwise scalar-vs-byte offsets are \
         indistinguishable here"
    );

    let scalars = md.chars().count();
    let segments = env["segments"].as_array().expect("segments array");
    assert!(!segments.is_empty(), "the sample must produce segments");

    let mut prev_end = 0usize;
    for s in segments {
        let start = s["start"].as_u64().expect("start is a scalar") as usize;
        let end = s["end"].as_u64().expect("end is a scalar") as usize;
        assert!(start < end, "empty or inverted span [{start},{end})");
        assert!(
            end <= scalars,
            "span end {end} past the markdown's {scalars} scalars (byte offsets?)"
        );
        assert!(
            start >= prev_end,
            "spans must be sorted and non-overlapping: {start} < {prev_end}"
        );
        prev_end = end;
    }
}

// ---------------------------------------------------------------------------
// v2: the published schema
// ---------------------------------------------------------------------------

/// The published v2 contract, compiled into the test the way `render-report-v1` is.
fn v2_validator() -> jsonschema::Validator {
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../schemas/segment-envelope-v2.schema.json"
    ))
    .expect("the v2 schema is valid JSON");
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .expect("the v2 schema compiles as a Draft 2020-12 schema")
}

/// A style level with everything resolved, for building fixtures below.
fn resolved_level() -> serde_json::Value {
    serde_json::json!({
        "char_shape_id": 3,
        "para_shape_id": 1,
        "char": {
            "face_ids": [1, 1, 1, 1, 1, 1, 1],
            "faces": ["함초롬바탕", "함초롬바탕", null, null, null, null, null],
            "size_pt": 10.0,
            "bold": false,
            "italic": false,
            "color": "#000000"
        },
        "para": {
            "alignment": "justify",
            "indent": 0,
            "line_spacing_type": 0,
            "line_spacing": 160
        }
    })
}

/// A representative envelope: all seven kinds, both style levels, an unresolvable level, a
/// `null` face slot, an omitted `ctrl_id`, and — the point of the fixture — a run a block
/// interrupted, reported as two pieces that SHARE ONE ID.
///
/// This is the shape the CLI is written to in task 2. Keeping it here as well means the schema
/// is pinned against the intended shape even where the CLI happens not to produce one of these
/// cases from the committed sample.
fn representative_envelope() -> serde_json::Value {
    let seg = |kind: &str, id: &str, indices: Vec<usize>, start: usize, end: usize| {
        serde_json::json!({
            "id": id,
            "kind": kind,
            "path": { "section": 0, "indices": indices },
            "char_range": { "start": start, "end": end },
            "style": resolved_level(),
            "direct": resolved_level(),
        })
    };

    let mut segments = vec![
        seg("para", "aaaaaaaa.0.0", vec![0], 0, 40),
        // The fragmented run, first piece.
        seg("run", "bbbbbbbb.0.0.0", vec![0, 0], 0, 10),
        seg("table", "cccccccc.0.0.1", vec![0, 1], 10, 30),
        // A cell whose range is exactly its run: the depth tie-break case.
        seg("cell", "dddddddd.0.0.1.0", vec![0, 1, 0], 12, 20),
        seg("run", "eeeeeeee.0.0.1.0.0", vec![0, 1, 0, 0], 12, 20),
        // The same run after the block: same id, disjoint range.
        seg("run", "bbbbbbbb.0.0.0", vec![0, 0], 30, 40),
        seg("image", "ffffffff.0.1", vec![1], 40, 45),
    ];

    let mut field = seg("field", "11111111.0.2", vec![2], 45, 50);
    field["ctrl_id"] = serde_json::json!("%clk");
    segments.push(field);

    let mut bookmark = seg("bookmark", "22222222.0.3", vec![3], 50, 50);
    bookmark["ctrl_id"] = serde_json::json!("bokm");
    bookmark["name"] = serde_json::json!("표지_기관명");
    // An unresolvable style level: explicitly absent, never a fabricated default.
    bookmark["direct"] = serde_json::json!({
        "char_shape_id": null,
        "para_shape_id": null,
        "char": null,
        "para": null
    });
    segments.push(bookmark);

    serde_json::json!({
        "contract": "hwp-segment-envelope-v2",
        "schema_version": "1.0",
        "markdown": "본문",
        "segments": segments,
    })
}

/// The schema accepts the shape the envelope is specified to have, all seven kinds included.
#[test]
fn the_v2_schema_accepts_the_published_shape() {
    let validator = v2_validator();
    let value = representative_envelope();
    if let Err(e) = validator.validate(&value) {
        panic!("the v2 schema rejected the published shape: {e}");
    }
    assert_eq!(value["contract"], "hwp-segment-envelope-v2");
    assert_eq!(value["schema_version"], "1.0");
}

/// The tripwire for the reflex this contract is most likely to lose to. Segment ids are NOT
/// unique: a run a block control interrupted is reported once per contiguous piece, every piece
/// carrying the run's single id, and the render geometry artifact repeats an id across pages.
/// A later "tidy-up" that adds `uniqueItems` has to fail here, in this repository, rather than
/// in a consumer that has already persisted the ids.
#[test]
fn the_v2_schema_accepts_two_segments_sharing_one_id() {
    let value = representative_envelope();
    let ids: Vec<&str> = value["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    let shared = "bbbbbbbb.0.0.0";
    assert_eq!(
        ids.iter().filter(|id| **id == shared).count(),
        2,
        "the fixture must carry a fragmented run, or this test proves nothing"
    );
    assert!(
        v2_validator().is_valid(&value),
        "the schema must accept a fragmented run whose pieces share one id"
    );
    assert!(
        !include_str!("../../../schemas/segment-envelope-v2.schema.json").contains("uniqueItems"),
        "no uniqueness constraint may exist anywhere in the v2 schema"
    );
}

/// The schema is closed, so a field-name typo fails the gate instead of disappearing silently.
#[test]
fn the_v2_schema_rejects_an_unknown_field() {
    let validator = v2_validator();

    let mut typo = representative_envelope();
    typo["segments"][0]["char_rnage"] = serde_json::json!({ "start": 0, "end": 1 });
    assert!(
        !validator.is_valid(&typo),
        "a misspelt segment field must be rejected, not ignored"
    );

    let mut extra = representative_envelope();
    extra["segmnets"] = serde_json::json!([]);
    assert!(
        !validator.is_valid(&extra),
        "a misspelt envelope field must be rejected, not ignored"
    );

    let mut nested = representative_envelope();
    nested["segments"][0]["style"]["char"]["size_pnt"] = serde_json::json!(10.0);
    assert!(
        !validator.is_valid(&nested),
        "a misspelt field in a nested object must be rejected too"
    );

    let mut wrong_kind = representative_envelope();
    wrong_kind["segments"][0]["kind"] = serde_json::json!("paragraph");
    assert!(
        !validator.is_valid(&wrong_kind),
        "the seven kinds are a closed enum"
    );
}

/// Each of the six load-bearing descriptions is published in the schema, because a consumer
/// reads the schema and not this repository's design docs. A test rather than a review note:
/// a description deleted during a later edit is exactly the kind of silent loss this phase has
/// produced before.
#[test]
fn the_six_load_bearing_descriptions_are_published() {
    let schema = include_str!("../../../schemas/segment-envelope-v2.schema.json");
    for (what, sentence) in [
        (
            "the coordinate space",
            "UNICODE SCALAR OFFSETS INTO THIS ENVELOPE'S OWN `markdown` STRING",
        ),
        ("the checksum is not tamper evidence", "IS NOT TAMPER EVIDENCE"),
        ("the field-kind floor", "A `field` COUNT IS A FLOOR, NEVER A CENSUS"),
        ("the repeating run id", "SEGMENT IDS ARE NOT UNIQUE"),
        (
            "the interrupted paragraph",
            "HAS NO SINGLE `para` SEGMENT",
        ),
        ("the segment order", "`char_range.end` DESCENDING"),
    ] {
        assert!(
            schema.contains(sentence),
            "the schema no longer states {what}: expected to find {sentence:?}"
        );
    }
}
