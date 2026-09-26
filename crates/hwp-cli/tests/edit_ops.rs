//! `hwp edit --ops` typed edit operations end-to-end tests (D-10, plan 06-01).
//!
//! `--ops` feeds an already-structured edit-ops-v1 JSON array to the edit path: the
//! file's string payloads are data, not the CLI mini-language, so "=>", "=" and ":"
//! inside them must survive verbatim. The committed tracer fixture exercises exactly
//! those three separator characters (replace from "a=>b" to "c=d:e") plus set_cell
//! and a before:true insert_para, and the test asserts the payloads land in the
//! output document byte for byte and that the two runs are byte-identical across a
//! real wall-clock gap (the #253 convention in edit_determinism.rs).

use std::path::{Path, PathBuf};
use std::process::Command;

use hwp_model::{Control, Table};

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn test_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hwp-cli-edit-ops-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn tracer_fixture() -> PathBuf {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/edit-ops/tracer.json");
    assert!(path.exists(), "tracer fixture missing: {}", path.display());
    path
}

fn new_from(md: &Path, out: &Path) {
    let status = hwp()
        .args(["new", "--from"])
        .arg(md)
        .arg("-o")
        .arg(out)
        .status()
        .unwrap();
    assert!(status.success(), "hwp new --from {md:?} -o {out:?} failed");
}

/// Byte-for-byte comparison with a diagnosable failure, as edit_determinism.rs.
fn assert_bytes_eq(a_path: &Path, b_path: &Path, context: &str) {
    let a = std::fs::read(a_path).unwrap_or_else(|e| panic!("{context}: read {a_path:?}: {e}"));
    let b = std::fs::read(b_path).unwrap_or_else(|e| panic!("{context}: read {b_path:?}: {e}"));
    if a == b {
        return;
    }
    let first_diff = a
        .iter()
        .zip(b.iter())
        .position(|(x, y)| x != y)
        .unwrap_or_else(|| a.len().min(b.len()));
    panic!(
        "{context}: {a_path:?} ({} bytes) != {b_path:?} ({} bytes) — first differing byte at offset {first_diff}",
        a.len(),
        b.len()
    );
}

fn first_table(doc: &hwp_model::Document) -> Table {
    doc.sections
        .iter()
        .flat_map(|section| &section.paragraphs)
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            Control::Table(table) => Some(table.clone()),
            _ => None,
        })
        .expect("generated markdown has a table")
}

fn table_has_text(table: &Table, text: &str) -> bool {
    table.cells.iter().any(|cell| {
        cell.paragraphs
            .iter()
            .any(|paragraph| paragraph.plain_text() == text)
    })
}

const TRACER_MD: &str =
    "generated edit ops tracer fixture\n\na=>b\n\n| 가 | 나 |\n|---|---|\n| 1 | 2 |\n";

/// The tracer fixture through `edit --ops` is lossless end to end: the "=>", "="
/// and ":" payloads inside the replace strings are data and land verbatim ("a=>b"
/// becomes exactly "c=d:e"), set_cell fills the addressed cell, and the before:true
/// insert_para lands immediately before its anchor paragraph. Two runs of the same
/// invocation, separated past the DOS ZIP timestamp granularity (2s), are
/// byte-identical (#253 convention).
#[test]
fn tracer_ops_end_to_end_lossless() {
    let dir = test_dir("lossless");
    let md = dir.join("doc.md");
    std::fs::write(&md, TRACER_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let run = |out: &Path| {
        let status = hwp()
            .arg("edit")
            .arg(&base)
            .arg("-o")
            .arg(out)
            .arg("--ops")
            .arg(tracer_fixture())
            .status()
            .unwrap();
        assert!(status.success(), "hwp edit --ops -> {out:?} failed");
    };

    let out_a = dir.join("out-a.hwpx");
    run(&out_a);
    // Past the 2-second DOS ZIP timestamp granularity, so a wall-clock leak into the
    // freshly written container would actually surface (edit_determinism.rs #253).
    std::thread::sleep(std::time::Duration::from_millis(2_500));
    let out_b = dir.join("out-b.hwpx");
    run(&out_b);
    assert_bytes_eq(&out_a, &out_b, "--ops tracer: run A vs run B");

    let doc = hwpx::read_document(&out_a).unwrap().document;
    let texts: Vec<String> = doc.sections[0]
        .paragraphs
        .iter()
        .map(|paragraph| paragraph.plain_text())
        .collect();
    let anchor = texts
        .iter()
        .position(|text| text == "generated edit ops tracer fixture")
        .expect("the anchor paragraph must survive");
    let inserted = texts
        .iter()
        .position(|text| text == "inserted before the anchor: tracer")
        .expect("the before:true inserted paragraph must survive");
    assert_eq!(
        inserted + 1,
        anchor,
        "before:true must insert immediately before the anchor"
    );
    assert!(
        texts.contains(&"c=d:e".to_string()),
        "the replace payload with =>, = and : must land verbatim, got {texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text.contains("a=>b")),
        "the source payload must be fully replaced"
    );
    let table = first_table(&doc);
    assert!(
        table_has_text(&table, "111"),
        "set_cell must fill the addressed cell with the payload text"
    );
}

/// A schema-violating ops file (`from` is a number where edit-ops-v1 demands a
/// string) exits nonzero, names the schema on stderr, and leaves no output file.
#[test]
fn tracer_ops_rejects_malformed() {
    let dir = test_dir("malformed");
    let md = dir.join("doc.md");
    std::fs::write(&md, TRACER_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let ops = dir.join("bad.json");
    std::fs::write(&ops, r#"[{"op":"replace","from":1}]"#).unwrap();
    let output = dir.join("out.hwpx");
    let report = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        !report.status.success(),
        "a schema-violating ops file must exit nonzero"
    );
    let stderr = String::from_utf8_lossy(&report.stderr);
    assert!(
        stderr.contains("edit-ops-v1"),
        "stderr must name the edit-ops-v1 schema: {stderr}"
    );
    assert!(
        !output.exists(),
        "no output file may be created when the ops file is rejected"
    );
}

// Paragraph 3 (0-based: T, intro para, table anchor, list para) is a numbered-list item at
// head_level 1 — indent_para/outdent_para (07-04) need a real list item, which no other kind in
// this fixture produces (insert_para's inline style/char cannot express head_type/numbering).
const KIND_COVERAGE_MD: &str =
    "# T\n\nintro para\n\n| 항목 | 수량 |\n|---|---|\n| 가 | 1 |\n\n1. list para\n";

/// 최소 유효 PNG(시그니처+IHDR) — image_pixel_size가 치수를 읽고 writer가 바이트를
/// 그대로 임베드한다(디코딩은 하지 않음; cli.rs write_min_png와 동일).
fn write_min_png(path: &Path, w: u32, h: u32) {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend([0, 0, 0, 13]);
    png.extend(b"IHDR");
    png.extend(w.to_be_bytes());
    png.extend(h.to_be_bytes());
    png.extend([8, 6, 0, 0, 0]); // bit depth/color type 등
    png.extend([0, 0, 0, 0]); // CRC 자리(검증 안 함)
    std::fs::write(path, &png).unwrap();
}

/// All 33 typed edit kinds through one flat `edit --ops` run (plan 06-02 W2-T1b).
/// The kind-coverage fixture chains the paragraph-anchored kinds off each other,
/// exercises the table kinds on the inserted 3x2 table, and deletes what it created
/// (clone table, image, field, bookmark, doomed para). The run must exit success:
/// every kind that fails to apply either aborts or pushes an unapplied entry, and a
/// non-empty unapplied list exits nonzero before an output is published — that is
/// the all-applied proxy for the kinds with no text-observable effect (set_meta,
/// set_page, set_format/set_align/set_para, indent_para/outdent_para, row/col
/// surgery, set_cell_para, style_tables, delete_field, delete_bookmark). The text-
/// and model-level assertions mirror the verified binary drive: replace payloads,
/// field/hyperlink display text, the 라벨값/수정값 row on the first table, the
/// untouched input form table as the second table, the clone removed, the doomed
/// para gone, the inserted picture deleted from its anchor paragraph, the seal left
/// as a floating Picture with both image parts shipped in the package, and the
/// indent/outdent round trip leaving the list paragraph back at its original level.
#[test]
fn kind_coverage_all_33() {
    // Future renames of a typed edit kind fail loudly here.
    const ALL_KINDS: [&str; 33] = [
        "set_meta",
        "set_page",
        "insert_para",
        "move_para",
        "indent_para",
        "outdent_para",
        "replace",
        "create_field",
        "set_field",
        "create_bookmark",
        "create_hyperlink",
        "insert_image",
        "seal",
        "add_table",
        "set_cell",
        "set_cell_by_label",
        "set_cell_para",
        "add_row",
        "add_col",
        "merge_cells",
        "split_cell",
        "delete_row",
        "delete_col",
        "clone_table",
        "delete_table",
        "style_tables",
        "set_format",
        "set_align",
        "set_para",
        "delete_image",
        "delete_field",
        "delete_bookmark",
        "delete_para",
    ];

    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/edit-ops/kind-coverage.json");
    assert!(
        fixture_path.exists(),
        "kind-coverage fixture missing: {}",
        fixture_path.display()
    );
    let fixture_value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fixture_path).unwrap())
            .expect("kind-coverage fixture must parse as JSON");
    let ops: Vec<&str> = fixture_value
        .as_array()
        .expect("kind-coverage fixture must be an array")
        .iter()
        .map(|entry| {
            entry["op"]
                .as_str()
                .expect("every op must carry a string tag")
        })
        .collect();
    assert!(
        ops.len() >= 33,
        "fixture must carry at least 33 ops, got {}",
        ops.len()
    );
    let mut seen = ops;
    seen.sort_unstable();
    seen.dedup();
    let mut expected = ALL_KINDS.to_vec();
    expected.sort_unstable();
    assert_eq!(
        seen, expected,
        "fixture op set must equal the 33 typed edit kinds"
    );

    // Schema gate mirrors load_ops: Draft 2020-12 against the committed schema.
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../schemas/edit-ops-v1.schema.json")).unwrap();
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .unwrap();
    assert!(
        validator.is_valid(&fixture_value),
        "kind-coverage fixture must satisfy edit-ops-v1"
    );

    let dir = test_dir("kind-coverage");
    // The verified input document: the markdown form table feeds the label preflight
    // (set_cell_by_label resolves against the ORIGINAL input document) and survives
    // untouched as the second table.
    let md = dir.join("doc.md");
    std::fs::write(&md, KIND_COVERAGE_MD).unwrap();
    // insert_image/seal read their relative paths from the subprocess cwd.
    write_min_png(&dir.join("logo.png"), 32, 24);
    write_min_png(&dir.join("seal.png"), 24, 24);
    let status = hwp()
        .current_dir(&dir)
        .args(["new", "--from"])
        .arg("doc.md")
        .arg("-o")
        .arg("doc.hwpx")
        .status()
        .unwrap();
    assert!(status.success(), "hwp new --from doc.md -o doc.hwpx failed");
    // The fixture rides in the cwd like the image paths (harness path handling).
    std::fs::copy(&fixture_path, dir.join("kind-coverage.json")).unwrap();

    let output = dir.join("out.hwpx");
    let run = hwp()
        .current_dir(&dir)
        .args(["edit", "doc.hwpx", "-o"])
        .arg(&output)
        .args(["--ops", "kind-coverage.json"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        run.status.success(),
        "hwp edit --ops kind-coverage.json failed: {stderr}"
    );
    // Success itself is the all-applied guarantee (unapplied ops exit nonzero); these
    // guards double-check no partial/publish-guard path let the run slip through.
    assert!(
        !stderr.contains("적용되지 않은") && !stderr.contains("게시하지 않습니다"),
        "unapplied or unpublished edits must fail the run: {stderr}"
    );
    assert!(output.exists(), "the run must publish an output document");

    let doc = hwpx::read_document(&output).unwrap().document;
    let texts: Vec<String> = doc
        .sections
        .iter()
        .flat_map(|section| &section.paragraphs)
        .map(|paragraph| paragraph.plain_text())
        .collect();
    assert!(
        texts.iter().any(|text| text.contains("seed one c=d:e")),
        "replace must rewrite the seeded payload verbatim, got {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|text| text.contains("field anchor para변경값")),
        "create_field + set_field must leave the field display text, got {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|text| text.contains("hyperlink anchor para예시 링크")),
        "create_hyperlink must leave its display text, got {texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text.contains("doomed para")),
        "delete_para must remove the doomed paragraph, got {texts:?}"
    );

    let tables: Vec<Table> = doc
        .sections
        .iter()
        .flat_map(|section| &section.paragraphs)
        .flat_map(|paragraph| &paragraph.controls)
        .filter_map(|control| match control {
            Control::Table(table) => Some(table.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        tables.len(),
        2,
        "add_table + clone_table + delete_table(index 1) must leave exactly two tables"
    );
    assert!(
        table_has_text(&tables[0], "라벨값") && table_has_text(&tables[0], "수정값"),
        "set_cell + set_cell_by_label must land on the first table"
    );
    let label_hits: usize = tables
        .iter()
        .map(|table| {
            table
                .cells
                .iter()
                .filter(|cell| {
                    cell.paragraphs
                        .iter()
                        .any(|paragraph| paragraph.plain_text() == "라벨값")
                })
                .count()
        })
        .sum();
    assert_eq!(
        label_hits, 1,
        "the cloned table must be gone — 라벨값 may appear in exactly one cell"
    );
    assert!(
        table_has_text(&tables[1], "가") && table_has_text(&tables[1], "1"),
        "the input form table must survive untouched as the second table"
    );

    // Model-level proxies for the image kinds: the inserted inline picture was
    // deleted (no Picture left in the image-anchor paragraph), the seal floats as a
    // Picture in its own anchor paragraph, and both image parts shipped in the
    // package (read_document loads BinData/* into bin_streams).
    let pictures_in = |needle: &str| -> usize {
        doc.sections
            .iter()
            .flat_map(|section| &section.paragraphs)
            .filter(|paragraph| paragraph.plain_text().contains(needle))
            .map(|paragraph| {
                paragraph
                    .controls
                    .iter()
                    .filter(|control| matches!(control, Control::Picture(_)))
                    .count()
            })
            .sum()
    };
    assert_eq!(
        pictures_in("image anchor para"),
        0,
        "delete_image must remove the inserted picture from its anchor paragraph"
    );
    assert!(
        pictures_in("seal anchor para") >= 1,
        "seal must leave a floating Picture in its anchor paragraph"
    );
    assert!(
        doc.bin_streams.len() >= 2,
        "insert_image and seal must ship their image parts in the package"
    );

    // Model-level proxy for indent_para/outdent_para: the round trip (indent then outdent on
    // the same paragraph, near the top of the fixture) must leave "list para" back at its
    // original head_level, and still a numbered list item (head_type unchanged).
    let list_para = doc
        .sections
        .iter()
        .flat_map(|section| &section.paragraphs)
        .find(|paragraph| paragraph.plain_text() == "list para")
        .expect("list para must survive the batch");
    let list_shape = &doc.header.para_shapes[list_para.para_shape.0 as usize];
    assert_eq!(
        list_shape.head_type(),
        2,
        "indent_para/outdent_para must not change head_type"
    );
    assert_eq!(
        list_shape.head_level(),
        1,
        "an indent immediately followed by an outdent must round-trip to the original level"
    );
}

// ── Phase 7 plan 07-01: addressed `set_format` tracer (EDT-05) ─────────────────────────

/// Paragraphs 2 and 4 (0-based: 제목, 첫 문단, 같은 문단, 셋째 문단, 같은 문단) are identical
/// text — the anchor-collision fixture success criterion 1 requires.
const DUPLICATE_TEXT_MD: &str = "# 제목\n\n첫 문단\n\n같은 문단\n\n셋째 문단\n\n같은 문단\n";

/// A single paragraph importing as three char-shape runs: "plain " (plain), "bold" (bold, from
/// GFM `**bold**`), " tail" (plain).
const RUN_RANGE_MD: &str = "# T\n\nplain **bold** tail\n";

/// An addressed `set_format` targeting the SECOND of two identical-text paragraphs restyles only
/// that paragraph; the first occurrence's `char_shape_runs` and text are unchanged (EDT-05
/// success criterion 1, the anchor-collision proof — a first-match pattern search would hit the
/// first occurrence instead and fail this test).
#[test]
fn addressed_set_format_hits_only_the_named_duplicate() {
    let dir = test_dir("addr-duplicate");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let before_doc = hwpx::read_document(&base).unwrap().document;
    let first_before = before_doc.sections[0].paragraphs[2].clone();
    assert_eq!(first_before.plain_text(), "같은 문단");
    assert_eq!(
        before_doc.sections[0].paragraphs[4].plain_text(),
        "같은 문단"
    );

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_format","address":{"at":{"section":0,"paragraph":4,"run":0}},"bold":"on"}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed set_format must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let after = hwpx::read_document(&output).unwrap().document;
    let first_after = &after.sections[0].paragraphs[2];
    assert_eq!(
        first_after.char_shape_runs, first_before.char_shape_runs,
        "the first (untouched) occurrence's runs must be unchanged"
    );
    assert_eq!(
        first_after.plain_text(),
        "같은 문단",
        "the first occurrence's text must be unchanged"
    );
    let second_after = &after.sections[0].paragraphs[4];
    let styled = second_after
        .char_shape_runs
        .iter()
        .any(|(_, id)| after.header.char_shapes[id.0 as usize].is_bold());
    assert!(styled, "the addressed (second) occurrence must be bold");
}

// ── Phase 7 plan 07-04: font face char property (EDT-05) ───────────────────────

/// A `set_format` carrying `font` resolves through `find_or_insert_face`: a NEW name appends
/// exactly one `FaceName` per language slot, and setting the SAME new name again later in the
/// SAME batch (a different paragraph) reuses the entry instead of growing the table again.
#[test]
fn addressed_set_format_font_appends_once_per_batch_on_repeat() {
    let dir = test_dir("addr-font-repeat");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let before_doc = hwpx::read_document(&base).unwrap().document;
    let before_lens: Vec<usize> = before_doc
        .header
        .fonts
        .iter()
        .map(|slot| slot.len())
        .collect();

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[
          {"op":"set_format","address":{"at":{"section":0,"paragraph":1,"run":0}},"font":"새글꼴"},
          {"op":"set_format","address":{"at":{"section":0,"paragraph":3,"run":0}},"font":"새글꼴"}
        ]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed set_format font must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let after = hwpx::read_document(&output).unwrap().document;
    for (slot, before_len) in before_lens.into_iter().enumerate() {
        assert_eq!(
            after.header.fonts[slot].len(),
            before_len + 1,
            "slot {slot}: a repeated new name within one batch must append exactly once"
        );
        assert_eq!(
            after.header.fonts[slot].last().unwrap().name,
            "새글꼴",
            "slot {slot}: the appended entry must carry the requested name"
        );
    }
}

/// The argv `--set-format` mini-language and the ops-file `set_format` kind accept the same
/// `font` property and produce a byte-identical resulting `CharShape` sequence — one value
/// vocabulary across the two surfaces.
#[test]
fn set_format_font_argv_and_ops_surfaces_agree() {
    let dir = test_dir("font-two-surfaces");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let via_argv = dir.join("via-argv.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&via_argv)
        .args(["--set-format", "첫 문단:font=맑은 고딕"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "argv --set-format font must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_format","pattern":"첫 문단","font":"맑은 고딕"}]"#,
    )
    .unwrap();
    let via_ops = dir.join("via-ops.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&via_ops)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "ops-file set_format font must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let argv_doc = hwpx::read_document(&via_argv).unwrap().document;
    let ops_doc = hwpx::read_document(&via_ops).unwrap().document;
    let argv_shapes: Vec<_> = argv_doc.sections[0].paragraphs[1]
        .char_shape_runs
        .iter()
        .map(|(_, id)| argv_doc.header.char_shapes[id.0 as usize].clone())
        .collect();
    let ops_shapes: Vec<_> = ops_doc.sections[0].paragraphs[1]
        .char_shape_runs
        .iter()
        .map(|(_, id)| ops_doc.header.char_shapes[id.0 as usize].clone())
        .collect();
    assert_eq!(
        argv_shapes, ops_shapes,
        "argv and ops-file font surfaces must agree on the resulting CharShape"
    );
}

/// An empty font name is rejected at the schema layer (`fontName` `minLength: 1`) before
/// anything applies — no output file is created.
#[test]
fn set_format_font_empty_name_rejected_no_output() {
    let dir = test_dir("font-empty-name");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_format","pattern":"첫 문단","font":""}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(!run.status.success(), "an empty font name must be rejected");
    assert!(
        !output.exists(),
        "no output file may be created when font is empty"
    );
}

/// An addressed font set on the SECOND of two identical-text paragraphs leaves the first
/// occurrence's `char_shape_runs` unchanged (EDT-05 success criterion 1, mirrors the bold proof
/// above).
#[test]
fn addressed_set_format_font_hits_only_the_named_duplicate() {
    let dir = test_dir("addr-font-duplicate");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let before_doc = hwpx::read_document(&base).unwrap().document;
    let first_before = before_doc.sections[0].paragraphs[2].clone();
    assert_eq!(first_before.plain_text(), "같은 문단");

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_format","address":{"at":{"section":0,"paragraph":4,"run":0}},"font":"고딕체"}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed font set must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let after = hwpx::read_document(&output).unwrap().document;
    let first_after = &after.sections[0].paragraphs[2];
    assert_eq!(
        first_after.char_shape_runs, first_before.char_shape_runs,
        "the first (untouched) occurrence's runs must be unchanged"
    );
    let second_after = &after.sections[0].paragraphs[4];
    let font_applied = second_after.char_shape_runs.iter().any(|(_, id)| {
        let cs = &after.header.char_shapes[id.0 as usize];
        after.header.fonts[0][cs.face_ids[0] as usize].name == "고딕체"
    });
    assert!(
        font_applied,
        "the addressed (second) occurrence must carry the new font"
    );
}

/// An addressed `set_format` naming a run with the run's own WCHAR sub-range restyles exactly
/// that sub-range; the paragraph's text is byte-identical afterwards (formatting never touches
/// `chars`), and only the addressed wchar positions carry the new attribute.
#[test]
fn addressed_run_range_restyles_the_named_sub_range_only() {
    let dir = test_dir("addr-run-range");
    let md = dir.join("doc.md");
    std::fs::write(&md, RUN_RANGE_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let before = hwpx::read_document(&base).unwrap().document;
    let para = &before.sections[0].paragraphs[1];
    assert_eq!(para.plain_text(), "plain bold tail");
    let runs = hwp_convert::canonical_char_shape_runs(para);
    assert_eq!(
        runs.len(),
        3,
        "plain/bold/tail must import as three runs: {runs:?}"
    );
    assert_eq!(runs[1].0, 6, "run 1 (bold) must start at wchar 6: {runs:?}");

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_format","address":{"at":{"section":0,"paragraph":1,"run":1},"chars":[7,9]},"italic":"on"}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed run-range set_format must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let after = hwpx::read_document(&output).unwrap().document;
    let para = &after.sections[0].paragraphs[1];
    assert_eq!(
        para.plain_text(),
        "plain bold tail",
        "formatting must never change the text"
    );
    let shapes = &after.header.char_shapes;
    let mut pos = 0u32;
    for ch in &para.chars {
        let width = ch.wchar_width();
        let id = para
            .char_shape_runs
            .iter()
            .rev()
            .find(|(p, _)| *p <= pos)
            .map(|(_, id)| *id)
            .unwrap();
        let italic = shapes[id.0 as usize].is_italic();
        if (7..9).contains(&pos) {
            assert!(
                italic,
                "wchar {pos} inside the addressed [7,9) must be italic"
            );
        } else {
            assert!(
                !italic,
                "wchar {pos} outside the addressed [7,9) must not be italic"
            );
        }
        pos += width;
    }
}

/// An address naming a run with no `chars` range applies to the whole run (D-01): the id
/// identifies the run, not a range, so the op applies to the run and leaves it visually uniform.
#[test]
fn addressed_run_without_chars_covers_the_whole_run() {
    let dir = test_dir("addr-whole-run");
    let md = dir.join("doc.md");
    std::fs::write(&md, RUN_RANGE_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r##"[{"op":"set_format","address":{"at":{"section":0,"paragraph":1,"run":1}},"color":"#0000ff"}]"##,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed whole-run set_format must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let after = hwpx::read_document(&output).unwrap().document;
    let para = &after.sections[0].paragraphs[1];
    let shapes = &after.header.char_shapes;
    let mut pos = 0u32;
    for ch in &para.chars {
        let width = ch.wchar_width();
        let id = para
            .char_shape_runs
            .iter()
            .rev()
            .find(|(p, _)| *p <= pos)
            .map(|(_, id)| *id)
            .unwrap();
        let colored = shapes[id.0 as usize].text_color == 0x00ff_0000;
        if (6..10).contains(&pos) {
            assert!(
                colored,
                "wchar {pos} inside run 1 [6,10) must carry the new color"
            );
        } else {
            assert!(
                !colored,
                "wchar {pos} outside run 1 must not carry the new color"
            );
        }
        pos += width;
    }
}

/// After a successful addressed restyle, `run_id` re-derived at the same path differs from the
/// checksum the preflight recorded before the edit — the before/after id-change mechanism 07-05
/// will publish in the report, proven here first.
#[test]
fn addressed_edit_changes_the_run_id() {
    let dir = test_dir("addr-id-change");
    let md = dir.join("doc.md");
    std::fs::write(&md, RUN_RANGE_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let before_doc = hwpx::read_document(&base).unwrap().document;
    let path = hwp_convert::SegmentPath {
        section: 0,
        indices: vec![1],
    };
    let before_id = hwp_convert::run_id(&path, &before_doc.sections[0].paragraphs[1], 1);

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_format","address":{"at":{"section":0,"paragraph":1,"run":1}},"strike":"on"}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed set_format must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let after_doc = hwpx::read_document(&output).unwrap().document;
    let after_id = hwp_convert::run_id(&path, &after_doc.sections[0].paragraphs[1], 1);
    assert_ne!(
        before_id, after_id,
        "the run id must change after the addressed edit"
    );
}

// ── Phase 7 plan 07-01 Task 3: address preflight failure modes ─────────────────────────

/// A batch whose op carries a stale checksum exits non-zero, writes no output file, and its
/// error names the op index, the id, the expected checksum and the found checksum (D-03/D-04,
/// CONTEXT.md's Specific Ideas wording: `op[2] id a1b2.0.3.1, expected checksum a1b2, found
/// c9d4`).
#[test]
fn addressed_stale_checksum_aborts_the_whole_batch() {
    let dir = test_dir("addr-stale");
    let md = dir.join("doc.md");
    std::fs::write(&md, RUN_RANGE_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let before_doc = hwpx::read_document(&base).unwrap().document;
    let path = hwp_convert::SegmentPath {
        section: 0,
        indices: vec![1],
    };
    let real_id = hwp_convert::run_id(&path, &before_doc.sections[0].paragraphs[1], 1);
    let (real_checksum, rest) = real_id.split_once('.').unwrap();
    let stale_checksum = "0000000000000000";
    assert_ne!(real_checksum, stale_checksum, "fixture sanity");
    let stale_id = format!("{stale_checksum}.{rest}");

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        format!(r#"[{{"op":"set_format","address":{{"id":"{stale_id}"}},"bold":"on"}}]"#),
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "a stale checksum must abort the batch"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("op[0]"),
        "stderr must name the op index: {stderr}"
    );
    assert!(
        stderr.contains(&stale_id),
        "stderr must name the id the caller wrote: {stderr}"
    );
    assert!(
        stderr.contains(stale_checksum),
        "stderr must name the expected (caller's) checksum: {stderr}"
    );
    assert!(
        stderr.contains(real_checksum),
        "stderr must name the found (actual) checksum: {stderr}"
    );
    assert!(
        !output.exists(),
        "no output file may be created when an address is stale"
    );
}

/// A batch with three bad addresses reports all three in one error block, each with its op
/// index, rather than failing on the first (D-04).
#[test]
fn addressed_preflight_names_every_failure_at_once() {
    let (dir, base) = new_base_for("addr-multi-fail");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[
          {"op":"set_format","address":{"at":{"section":0,"paragraph":90,"run":0}},"bold":"on"},
          {"op":"set_format","address":{"at":{"section":0,"paragraph":91,"run":0}},"bold":"on"},
          {"op":"set_format","address":{"at":{"section":0,"paragraph":92,"run":0}},"bold":"on"}
        ]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(!run.status.success(), "three bad addresses must abort");
    let stderr = String::from_utf8_lossy(&run.stderr);
    for index in 0..3 {
        assert!(
            stderr.contains(&format!("op[{index}]")),
            "stderr must name op[{index}] among all three failures: {stderr}"
        );
    }
    assert!(!output.exists());
}

/// Every address failure mode behaves identically with `--allow-partial` present — address
/// preflight failures are Phase 6 D-09's structural layer, never softened by it (Task 1
/// decision 6, A5).
#[test]
fn addressed_preflight_ignores_allow_partial() {
    let (dir, base) = new_base_for("addr-allow-partial");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_format","address":{"at":{"section":0,"paragraph":99,"run":0}},"bold":"on"}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .arg("--allow-partial")
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "--allow-partial must not rescue an address preflight failure"
    );
    assert!(!output.exists());
}

/// A `chars` pair whose end exceeds the named run's boundary is rejected with the run's own
/// boundary in the message.
#[test]
fn addressed_chars_outside_the_run_is_rejected() {
    let dir = test_dir("addr-chars-oob");
    let md = dir.join("doc.md");
    std::fs::write(&md, RUN_RANGE_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    // run 1 ("bold") spans wchar [6, 10) — see addressed_run_range_restyles_the_named_sub_range_only.
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_format","address":{"at":{"section":0,"paragraph":1,"run":1},"chars":[6,11]},"italic":"on"}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "chars past the run's own boundary must be rejected"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains('6') && stderr.contains("10"),
        "stderr must name the run's own [6, 10) boundary: {stderr}"
    );
    assert!(!output.exists());
}

/// A `set_para`-shaped entry (paragraph granularity) is not available until 07-02, so this
/// asserts the granularity rule at the schema layer directly: a `paragraphAddress` instance
/// carrying `chars` must fail schema validation (D-11).
#[test]
fn paragraph_granularity_rejects_a_char_range() {
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../schemas/edit-ops-v1.schema.json")).unwrap();
    let defs = schema
        .get("$defs")
        .expect("schema must carry $defs")
        .clone();
    // A wrapper document whose root schema is a $ref straight into the original $defs bag —
    // the standard way to validate an instance against one $defs entry directly.
    let wrapped = serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$ref": "#/$defs/paragraphAddress",
        "$defs": defs,
    });
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&wrapped)
        .unwrap();

    let with_chars: serde_json::Value =
        serde_json::from_str(r#"{"at":{"section":0,"paragraph":3},"chars":[0,1]}"#).unwrap();
    assert!(
        !validator.is_valid(&with_chars),
        "a paragraphAddress instance carrying chars must fail schema validation"
    );

    let without_chars: serde_json::Value =
        serde_json::from_str(r#"{"at":{"section":0,"paragraph":3}}"#).unwrap();
    assert!(
        validator.is_valid(&without_chars),
        "sanity: a plain paragraphAddress without chars must validate"
    );
}

// ── Phase 7 plan 07-02: paragraph-granularity addressed ops (Task 2) ───────────────────

/// An ops file with `{"op":"set_para","address":{...}}` validates and applies — basic
/// schema/parser/apply-arm plumbing proof. The duplicate-text anchor-collision proof for
/// `set_para` lives in Task 3.
#[test]
fn addressed_set_para_applies_via_address() {
    let (dir, base) = new_base_for("addr-set-para-basic");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_para","address":{"at":{"section":0,"paragraph":1}},"align":"center"}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed set_para must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let after = hwpx::read_document(&output).unwrap().document;
    let para = &after.sections[0].paragraphs[1];
    let ps = &after.header.para_shapes[para.para_shape.0 as usize];
    assert_eq!(ps.alignment(), 3, "가운데 정렬(3)이 적용되어야 함");
}

/// Same plumbing proof for `set_align`.
#[test]
fn addressed_set_align_applies_via_address() {
    let (dir, base) = new_base_for("addr-set-align-basic");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_align","address":{"at":{"section":0,"paragraph":1}},"align":"right"}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed set_align must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let after = hwpx::read_document(&output).unwrap().document;
    let para = &after.sections[0].paragraphs[1];
    let ps = &after.header.para_shapes[para.para_shape.0 as usize];
    assert_eq!(ps.alignment(), 2, "오른쪽 정렬(2)이 적용되어야 함");
}

/// Same plumbing proof for `replace`.
#[test]
fn addressed_replace_applies_via_address() {
    let (dir, base) = new_base_for("addr-replace-basic");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"replace","address":{"at":{"section":0,"paragraph":1}},"from":"probe","to":"REPLACED"}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed replace must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let after = hwpx::read_document(&output).unwrap().document;
    assert_eq!(
        after.sections[0].paragraphs[1].plain_text(),
        "shape rejection REPLACED"
    );
}

/// `replace` requires `from` even when `address` narrows the search to one paragraph — unlike
/// `pattern` on `set_para`/`set_align`, `from` is always the matched substring here, never purely
/// a selector `address` can fully replace (rejected at the schema layer: `from` stays required).
#[test]
fn replace_requires_from_even_with_address() {
    let (dir, base) = new_base_for("addr-replace-no-from");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"replace","address":{"at":{"section":0,"paragraph":1}},"to":"REPLACED"}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "replace with address but no from must be rejected"
    );
    assert!(!output.exists());
}

/// `set_para` with both `pattern` and `address` is rejected before anything applies (D-12).
#[test]
fn set_para_rejects_pattern_and_address_together() {
    let (dir, base) = new_base_for("addr-set-para-both");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_para","pattern":"shape","address":{"at":{"section":0,"paragraph":1}},"align":"center"}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "set_para with both pattern and address must be rejected"
    );
    assert!(!output.exists());
}

/// `set_para` with neither `pattern` nor `address` is rejected the same way.
#[test]
fn set_para_rejects_neither_pattern_nor_address() {
    let (dir, base) = new_base_for("addr-set-para-neither");
    let ops = dir.join("ops.json");
    std::fs::write(&ops, r#"[{"op":"set_para","align":"center"}]"#).unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "set_para with neither pattern nor address must be rejected"
    );
    assert!(!output.exists());
}

/// The core `detect_conflicts` proof: an addressed `replace` that shortens paragraph 0 followed
/// by an addressed run-range `set_format` inside that same paragraph is rejected during
/// preflight, with no output file. Task 3 adds the char_shape_runs-untouched assertion,
/// `--allow-partial` variant, pattern-form variant and the different-paragraph negative control.
#[test]
fn detect_conflicts_rejects_length_change_before_run_range() {
    let dir = test_dir("addr-conflict-basic");
    let md = dir.join("doc.md");
    // A run split at "bold" gives set_format a run-range target inside paragraph 0; replacing
    // "plain" (5 wchars) with "x" (1 wchar) shortens the paragraph before that op would run.
    std::fs::write(&md, "# T\n\nplain **bold** tail\n").unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[
          {"op":"replace","address":{"at":{"section":0,"paragraph":1}},"from":"plain","to":"x"},
          {"op":"set_format","address":{"at":{"section":0,"paragraph":1,"run":1}},"bold":"on"}
        ]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "a length-changing replace before a same-paragraph run-range op must be rejected"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("op[0]") && stderr.contains("op[1]"),
        "stderr must name both op indices: {stderr}"
    );
    assert!(!output.exists());
}

// ── Phase 7 plan 07-02 Task 3: duplicate-text proofs and WCHAR-drift aborts ────────────

/// Addressed `set_para` on the SECOND of two identical paragraphs changes that paragraph's
/// `ParaShapeId` only; the first occurrence is untouched. Targets the second occurrence
/// deliberately, so a first-match implementation fails this test.
#[test]
fn addressed_set_para_hits_only_the_named_duplicate() {
    let dir = test_dir("addr-set-para-duplicate");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let before_doc = hwpx::read_document(&base).unwrap().document;
    let first_before = before_doc.sections[0].paragraphs[2].clone();
    let second_before_shape = before_doc.sections[0].paragraphs[4].para_shape;

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_para","address":{"at":{"section":0,"paragraph":4}},"align":"center"}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed set_para must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let after = hwpx::read_document(&output).unwrap().document;
    let first_after = &after.sections[0].paragraphs[2];
    assert_eq!(
        first_after.para_shape, first_before.para_shape,
        "the first (untouched) occurrence's ParaShapeId must be unchanged"
    );
    assert_eq!(first_after.plain_text(), "같은 문단");
    let second_after = &after.sections[0].paragraphs[4];
    assert_ne!(
        second_after.para_shape, second_before_shape,
        "the addressed (second) occurrence's ParaShapeId must change"
    );
    let ps = &after.header.para_shapes[second_after.para_shape.0 as usize];
    assert_eq!(ps.alignment(), 3, "가운데 정렬이 적용되어야 함");
}

/// Same anchor-collision proof for `set_align`.
#[test]
fn addressed_set_align_hits_only_the_named_duplicate() {
    let dir = test_dir("addr-set-align-duplicate");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let before_doc = hwpx::read_document(&base).unwrap().document;
    let first_before_shape = before_doc.sections[0].paragraphs[2].para_shape;

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_align","address":{"at":{"section":0,"paragraph":4}},"align":"right"}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed set_align must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let after = hwpx::read_document(&output).unwrap().document;
    let first_after = &after.sections[0].paragraphs[2];
    assert_eq!(
        first_after.para_shape, first_before_shape,
        "the first (untouched) occurrence's ParaShapeId must be unchanged"
    );
    let second_after = &after.sections[0].paragraphs[4];
    let ps = &after.header.para_shapes[second_after.para_shape.0 as usize];
    assert_eq!(ps.alignment(), 2, "오른쪽 정렬이 적용되어야 함");
}

/// Same anchor-collision proof for `replace`: rewrites text inside the SECOND occurrence only;
/// the first still reads the original string. Also the T-07-06 proof that the addressed replace
/// never took the replace-only package-preserving fast path — that path replaces every match
/// document-wide, so it would have changed the first occurrence too.
#[test]
fn addressed_replace_hits_only_the_named_duplicate() {
    let dir = test_dir("addr-replace-duplicate");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"replace","address":{"at":{"section":0,"paragraph":4}},"from":"같은","to":"바뀐"}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed replace must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let after = hwpx::read_document(&output).unwrap().document;
    assert_eq!(
        after.sections[0].paragraphs[2].plain_text(),
        "같은 문단",
        "the first (untouched) occurrence's text must be unchanged"
    );
    assert_eq!(
        after.sections[0].paragraphs[4].plain_text(),
        "바뀐 문단",
        "the addressed (second) occurrence must be rewritten"
    );
}

/// A `set_para` entry whose `address` carries a `chars` range fails schema validation (D-11) —
/// `paragraphAddress` forbids `chars`, and the reader must never silently ignore the extra depth.
#[test]
fn set_para_rejects_a_char_range_address() {
    let (dir, base) = new_base_for("addr-set-para-chars");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_para","address":{"at":{"section":0,"paragraph":1},"chars":[0,1]},"align":"center"}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "a paragraphAddress carrying chars must fail schema validation"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains(OPS_SCHEMA_MARKER),
        "stderr must name the schema-violation marker: {stderr}"
    );
    assert!(!output.exists());
}

/// A single paragraph importing as three char-shape runs: "plain " (plain), "bold" (bold), " tail"
/// (plain) — run 1 ("bold") spans wchar [6, 10). Two such paragraphs, so the negative control has
/// a second, untouched target for the run-range op.
const WCHAR_DRIFT_MD: &str = "# T\n\nplain **bold** tail\n\nanother plain **bold** tail\n";

/// Runs `hwp edit --ops` and returns (exit success, stderr). Shared by the four WCHAR-drift
/// scenarios below; `extra_args` carries `--allow-partial` for the variant that proves the
/// rejection ignores it.
fn run_wchar_drift_batch(
    base: &Path,
    output: &Path,
    ops_json: &str,
    extra_args: &[&str],
) -> (bool, String) {
    let ops_path = output.with_extension("ops.json");
    std::fs::write(&ops_path, ops_json).unwrap();
    let mut cmd = hwp();
    cmd.arg("edit")
        .arg(base)
        .arg("-o")
        .arg(output)
        .arg("--ops")
        .arg(&ops_path)
        .args(extra_args);
    let run = cmd.output().unwrap();
    (
        run.status.success(),
        String::from_utf8_lossy(&run.stderr).into_owned(),
    )
}

/// The core WCHAR-drift proof (T-07-26): an addressed `replace` on paragraph 1 shrinks it (5
/// wchars "plain" -> 1 wchar "x"), followed by an addressed run-range `set_format` naming run 1
/// ("bold") in that SAME paragraph, whose `[6, 10)` was resolved against the pre-batch paragraph.
/// Rejected during preflight: no output file, and the INPUT document's `char_shape_runs` for
/// paragraph 1 is byte-identical to what it was before the run — asserting only on the error
/// would also pass an implementation that mutates before erroring, which this must catch.
#[test]
fn wchar_drift_batch_is_rejected_with_the_run_table_untouched() {
    let dir = test_dir("addr-wchar-drift-base");
    let md = dir.join("doc.md");
    std::fs::write(&md, WCHAR_DRIFT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let before_runs = hwpx::read_document(&base).unwrap().document.sections[0].paragraphs[1]
        .char_shape_runs
        .clone();

    let output = dir.join("out.hwpx");
    let (success, stderr) = run_wchar_drift_batch(
        &base,
        &output,
        r#"[
          {"op":"replace","address":{"at":{"section":0,"paragraph":1}},"from":"plain","to":"x"},
          {"op":"set_format","address":{"at":{"section":0,"paragraph":1,"run":1}},"italic":"on"}
        ]"#,
        &[],
    );
    assert!(
        !success,
        "a length-changing replace before a same-paragraph run-range op must be rejected: {stderr}"
    );
    assert!(!output.exists(), "no output file may be written");

    let after_runs = hwpx::read_document(&base).unwrap().document.sections[0].paragraphs[1]
        .char_shape_runs
        .clone();
    assert_eq!(
        after_runs, before_runs,
        "the INPUT document's char_shape_runs must be byte-identical after the rejected run"
    );
}

/// The identical batch with `--allow-partial`: address preflight failures (staleness, conflict)
/// are Phase 6 D-09's structural layer, never softened by it.
#[test]
fn wchar_drift_batch_ignores_allow_partial() {
    let dir = test_dir("addr-wchar-drift-partial");
    let md = dir.join("doc.md");
    std::fs::write(&md, WCHAR_DRIFT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let before_runs = hwpx::read_document(&base).unwrap().document.sections[0].paragraphs[1]
        .char_shape_runs
        .clone();

    let output = dir.join("out.hwpx");
    let (success, stderr) = run_wchar_drift_batch(
        &base,
        &output,
        r#"[
          {"op":"replace","address":{"at":{"section":0,"paragraph":1}},"from":"plain","to":"x"},
          {"op":"set_format","address":{"at":{"section":0,"paragraph":1,"run":1}},"italic":"on"}
        ]"#,
        &["--allow-partial"],
    );
    assert!(
        !success,
        "--allow-partial must not rescue a length-change conflict: {stderr}"
    );
    assert!(!output.exists());

    let after_runs = hwpx::read_document(&base).unwrap().document.sections[0].paragraphs[1]
        .char_shape_runs
        .clone();
    assert_eq!(after_runs, before_runs);
}

/// The pattern-form variant: the earlier `replace` carries no address at all (whole-document
/// search), but its `from` occurs in the same paragraph the later run-range op addresses — the
/// over-approximated arm of the predicate (planner decision 2) must reject this too, not only the
/// addressed form.
#[test]
fn wchar_drift_batch_rejects_the_pattern_form_replace_too() {
    let dir = test_dir("addr-wchar-drift-pattern");
    let md = dir.join("doc.md");
    std::fs::write(&md, WCHAR_DRIFT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let output = dir.join("out.hwpx");
    let (success, stderr) = run_wchar_drift_batch(
        &base,
        &output,
        r#"[
          {"op":"replace","from":"plain","to":"x"},
          {"op":"set_format","address":{"at":{"section":0,"paragraph":1,"run":1}},"italic":"on"}
        ]"#,
        &[],
    );
    assert!(
        !success,
        "a pattern-form replace whose from occurs in the run-range op's paragraph must be rejected: {stderr}"
    );
    assert!(!output.exists());
}

/// The negative control: a length-changing replace on paragraph 1 and a run-range op inside a
/// DIFFERENT paragraph (3) are accepted, and both effects apply — proving the predicate is not a
/// blanket rejection of every batch containing a replace.
#[test]
fn wchar_drift_negative_control_different_paragraphs_are_accepted() {
    let dir = test_dir("addr-wchar-drift-negative");
    let md = dir.join("doc.md");
    std::fs::write(&md, WCHAR_DRIFT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let output = dir.join("out.hwpx");
    let (success, stderr) = run_wchar_drift_batch(
        &base,
        &output,
        r#"[
          {"op":"replace","address":{"at":{"section":0,"paragraph":1}},"from":"plain","to":"x"},
          {"op":"set_format","address":{"at":{"section":0,"paragraph":2,"run":1}},"italic":"on"}
        ]"#,
        &[],
    );
    assert!(
        success,
        "a replace and a run-range op on DIFFERENT paragraphs must both apply: {stderr}"
    );

    let after = hwpx::read_document(&output).unwrap().document;
    assert_eq!(
        after.sections[0].paragraphs[1].plain_text(),
        "x bold tail",
        "paragraph 1's replace must have applied"
    );
    let para2 = &after.sections[0].paragraphs[2];
    let shapes = &after.header.char_shapes;
    let styled = para2
        .char_shape_runs
        .iter()
        .any(|(_, id)| shapes[id.0 as usize].is_italic());
    assert!(
        styled,
        "paragraph 2's run-range set_format must have applied"
    );
}

/// Two NON-destructive addressed ops on ONE paragraph (`set_align` then a run-range
/// `set_format`) still compose in array order — `detect_conflicts` only ever treats a `replace`
/// as length-changing, so an earlier `set_align`/`set_para` on the same paragraph must never
/// trip the widened predicate.
#[test]
fn non_destructive_ops_on_one_paragraph_still_compose() {
    let dir = test_dir("addr-non-destructive-compose");
    let md = dir.join("doc.md");
    std::fs::write(&md, WCHAR_DRIFT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let output = dir.join("out.hwpx");
    let (success, stderr) = run_wchar_drift_batch(
        &base,
        &output,
        r#"[
          {"op":"set_align","address":{"at":{"section":0,"paragraph":1}},"align":"center"},
          {"op":"set_format","address":{"at":{"section":0,"paragraph":1,"run":1}},"italic":"on"}
        ]"#,
        &[],
    );
    assert!(
        success,
        "two non-destructive addressed ops on one paragraph must both apply: {stderr}"
    );

    let after = hwpx::read_document(&output).unwrap().document;
    let para = &after.sections[0].paragraphs[1];
    let ps = &after.header.para_shapes[para.para_shape.0 as usize];
    assert_eq!(ps.alignment(), 3, "set_align must have applied");
    let shapes = &after.header.char_shapes;
    let styled = para
        .char_shape_runs
        .iter()
        .any(|(_, id)| shapes[id.0 as usize].is_italic());
    assert!(styled, "run-range set_format must have applied");
}

/// load_ops가 스키마 위반을 만날 때 내보내는 bail 마커 접두어
/// ("편집 연산이 edit-ops-v1 스키마를 벗어났습니다: {instance_path}: {error}").
const OPS_SCHEMA_MARKER: &str = "편집 연산이 edit-ops-v1 스키마를 벗어났습니다";

const SHAPE_REJECT_MD: &str = "# T\n\nshape rejection probe\n";

/// Rejection tests never reach mutation, so a one-paragraph doc is enough; each
/// test synthesizes its own base document via `hwp new --from`.
fn new_base_for(name: &str) -> (PathBuf, PathBuf) {
    let dir = test_dir(name);
    let md = dir.join("doc.md");
    std::fs::write(&md, SHAPE_REJECT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);
    (dir, base)
}

/// Every named malformed fragment in the committed malformed-variants fixture must
/// bail through the edit-ops-v1 schema gate: exit nonzero, the schema-violation
/// marker on stderr, and no output file (D-01/D-02/D-09, D-10 MCP parity: the MCP
/// edit tool feeds the same ops channel, so argv-only shapes must never slip
/// through). The trailing variant pins the channel boundary itself: set_format's
/// switch fields take structured values, not the CLI mini-language, so an ops file
/// carrying an argv string like "bold=on,size=16" is a schema violation, never a
/// silently applied format.
#[test]
fn ops_shape_rejections() {
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/edit-ops/malformed-variants.json");
    assert!(
        fixture_path.exists(),
        "malformed-variants fixture missing: {}",
        fixture_path.display()
    );
    let variants: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fixture_path).unwrap())
            .expect("malformed-variants fixture must parse as JSON");
    let variants = variants
        .as_object()
        .expect("malformed-variants fixture must be an object keyed by variant name");
    assert!(
        !variants.is_empty(),
        "malformed-variants fixture must name at least one fragment"
    );

    let (dir, base) = new_base_for("shape-reject");
    for (name, fragment) in variants {
        let ops = dir.join(format!("{name}.json"));
        std::fs::write(&ops, serde_json::to_string(fragment).unwrap()).unwrap();
        let output = dir.join(format!("{name}.out.hwpx"));
        let report = hwp()
            .arg("edit")
            .arg(&base)
            .arg("-o")
            .arg(&output)
            .arg("--ops")
            .arg(&ops)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&report.stderr);
        assert!(
            !report.status.success(),
            "malformed variant {name} must exit nonzero: {stderr}"
        );
        assert!(
            stderr.contains(OPS_SCHEMA_MARKER),
            "malformed variant {name} must bail with the schema-violation marker: {stderr}"
        );
        assert!(
            !output.exists(),
            "malformed variant {name} must not produce an output file"
        );
    }

    // The ops channel takes structured objects, not argv strings: a mini-language
    // value in a typed switch field is a schema violation and must never be applied.
    let ops = dir.join("mini-language.json");
    std::fs::write(
        &ops,
        r#"{"op":"set_format","pattern":"p","bold":"bold=on,size=16"}"#,
    )
    .unwrap();
    let output = dir.join("mini-language.out.hwpx");
    let report = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&report.stderr);
    assert!(
        !report.status.success(),
        "mini-language ops value must exit nonzero: {stderr}"
    );
    assert!(
        stderr.contains(OPS_SCHEMA_MARKER),
        "mini-language ops value must trip the schema gate (switch fields are typed): {stderr}"
    );
    assert!(
        !output.exists(),
        "mini-language ops value must not produce an output file"
    );
}

/// `--ops` and `--replace` are exclusive invocation modes (D-07/D-08): passing both
/// must exit nonzero with an error that names both flags and must not write an
/// output document, and `hwp edit --help` must document `--ops` so the exclusive
/// modes are discoverable.
#[test]
fn mixed_invocation_conflict() {
    let (dir, base) = new_base_for("mixed-conflict");
    let ops = dir.join("ops.json");
    std::fs::write(&ops, r#"[{"op":"replace","from":"probe","to":"probe2"}]"#).unwrap();
    let output = dir.join("out.hwpx");
    let report = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .arg("--replace")
        .arg("a=>b")
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&report.stdout),
        String::from_utf8_lossy(&report.stderr)
    );
    assert!(
        !report.status.success(),
        "--ops plus --replace must conflict and exit nonzero: {combined}"
    );
    assert!(
        combined.contains("--ops") && combined.contains("--replace"),
        "the conflict error must name both flags: {combined}"
    );
    assert!(
        !output.exists(),
        "a conflicting invocation must not produce an output file"
    );

    let help = hwp().args(["edit", "--help"]).output().unwrap();
    let help_text = format!(
        "{}{}",
        String::from_utf8_lossy(&help.stdout),
        String::from_utf8_lossy(&help.stderr)
    );
    assert!(
        help_text.contains("--ops"),
        "hwp edit --help must document --ops: {help_text}"
    );
}

/// An empty ops array is a schema violation, not a silent no-op (D-13/D-14): the
/// root schema demands minItems 1, so `--ops []` must exit nonzero with the
/// schema-violation marker and write nothing.
#[test]
fn empty_array_rejected() {
    let (dir, base) = new_base_for("empty-array");
    let ops = dir.join("empty.json");
    std::fs::write(&ops, "[]").unwrap();
    let output = dir.join("out.hwpx");
    let report = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&report.stderr);
    assert!(
        !report.status.success(),
        "an empty ops array must exit nonzero: {stderr}"
    );
    assert!(
        stderr.contains(OPS_SCHEMA_MARKER),
        "an empty ops array must trip the minItems schema gate: {stderr}"
    );
    assert!(
        !output.exists(),
        "an empty ops array must not produce an output file"
    );
}

/// Unknown op kinds and entries without an op tag are schema violations
/// (D-15/D-16): the per-kind oneOf matches no variant for "foo" and nothing at all
/// for an untagged {}, so both must exit nonzero with the schema-violation marker
/// and write nothing.
#[test]
fn unknown_kind_and_missing_tag() {
    let (dir, base) = new_base_for("unknown-missing");
    for (name, body) in [
        ("unknown-kind", r#"[{"op":"foo"}]"#),
        ("missing-tag", r#"[{}]"#),
    ] {
        let ops = dir.join(format!("{name}.json"));
        std::fs::write(&ops, body).unwrap();
        let output = dir.join(format!("{name}.out.hwpx"));
        let report = hwp()
            .arg("edit")
            .arg(&base)
            .arg("-o")
            .arg(&output)
            .arg("--ops")
            .arg(&ops)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&report.stderr);
        assert!(
            !report.status.success(),
            "{name} ops fragment must exit nonzero: {stderr}"
        );
        assert!(
            stderr.contains(OPS_SCHEMA_MARKER),
            "{name} ops fragment must bail with the schema-violation marker: {stderr}"
        );
        assert!(
            !output.exists(),
            "{name} ops fragment must not produce an output file"
        );
    }
}

/// 미적용 bail 마커 접두어 (edit.rs: "적용되지 않은 편집 요청이 있습니다: {}
/// (--allow-partial로 일치한 요청만 적용 가능)").
const OPS_UNAPPLIED_MARKER: &str = "적용되지 않은 편집 요청이 있습니다";

/// --allow-partial이 출력 전에 내보내는 미적용 경고 접두어 ("경고: 미적용 편집 요청: {request}").
const OPS_UNAPPLIED_WARNING: &str = "미적용 편집 요청";

/// set_cell 같은 표 종류 op의 apply-time 하드 abort 문구 — Err 반환으로 run 전체가
/// 즉시 중단되며, unapplied push로 기록되지 않는다.
fn table_missing_phrase(index: usize) -> String {
    format!("표 #{index}를 찾을 수 없습니다")
}

const TABLE_MD: &str = "# T\n\ntable probe\n\n| 가 | 나 |\n|---|---|\n| 1 | 2 |\n";

/// A base document carrying one markdown table (GFM import styles it at import
/// time — the publish-guard re-styling probes below rely on that).
fn table_base_for(name: &str) -> (PathBuf, PathBuf) {
    let dir = test_dir(name);
    let md = dir.join("doc.md");
    std::fs::write(&md, TABLE_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);
    (dir, base)
}

/// Layered failure semantics, layer 1 (D-03/D-04): a set_cell whose table index
/// matches no table is an apply-time hard abort, not an unapplied entry. The
/// default run exits nonzero with the 표 #N를 찾을 수 없습니다 abort and writes
/// nothing; --allow-partial does NOT rescue it — the flag only downgrades unapplied
/// pushes, and a table-kind target miss never becomes one. Verified deviation from
/// the assigned expectation (unapplied bail marker / per-op warning): the bail layer
/// sits after apply, and set_cell errors through `?` before ever reaching it. The
/// unapplied layers themselves are exercised by unapplied_partial_semantics.
#[test]
fn layered_failure_semantics() {
    let (dir, base) = new_base_for("layered-failure");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_cell","table":0,"row":0,"col":0,"text":"x"}]"#,
    )
    .unwrap();

    let modes: [(&str, Vec<&str>); 2] = [
        ("default", Vec::new()),
        ("allow-partial", vec!["--allow-partial"]),
    ];
    for (name, extra) in modes {
        let output = dir.join(format!("{name}.out.hwpx"));
        let report = hwp()
            .arg("edit")
            .arg(&base)
            .arg("-o")
            .arg(&output)
            .arg("--ops")
            .arg(&ops)
            .args(&extra)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&report.stderr);
        assert!(
            !report.status.success(),
            "layer-1 target-miss ({name}) must exit nonzero: {stderr}"
        );
        assert!(
            stderr.contains(table_missing_phrase(0).as_str()),
            "layer-1 target-miss ({name}) must abort with the table-missing phrase: {stderr}"
        );
        assert!(
            !stderr.contains(OPS_UNAPPLIED_MARKER),
            "the unapplied bail layer must not fire for an apply-time abort ({name}): {stderr}"
        );
        assert!(
            !output.exists(),
            "layer-1 target-miss ({name}) must not produce an output file"
        );
    }
}

/// Layered failure semantics, layers 2-3 (D-05/D-06): the unapplied bail names the
/// failing request; --allow-partial publishes the edits that DID apply, in array
/// order, with a warning about the skipped one. The assigned fragment
/// set_cell(table=5) actually aborts in BOTH modes (layer-1 hard abort, see
/// layered_failure_semantics — verified deviation), so the order-holding demo pairs
/// set_meta(title) with a second op whose target miss is an unapplied push (replace
/// of an absent string): default exits nonzero naming that second op, and
/// --allow-partial publishes with set_meta's title reread from the output, proving
/// the first op applied even though the second failed.
#[test]
fn unapplied_partial_semantics() {
    let (dir, base) = table_base_for("unapplied-partial");

    // The assigned fragment: set_cell on a missing table aborts in both modes.
    let cell_ops = dir.join("cell-ops.json");
    std::fs::write(
        &cell_ops,
        r#"[{"op":"set_meta","key":"title","value":"partial-order"},{"op":"set_cell","table":5,"row":0,"col":0,"text":"x"}]"#,
    )
    .unwrap();
    let modes: [(&str, Vec<&str>); 2] = [
        ("default", Vec::new()),
        ("allow-partial", vec!["--allow-partial"]),
    ];
    for (name, extra) in modes {
        let output = dir.join(format!("cell-{name}.out.hwpx"));
        let report = hwp()
            .arg("edit")
            .arg(&base)
            .arg("-o")
            .arg(&output)
            .arg("--ops")
            .arg(&cell_ops)
            .args(&extra)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&report.stderr);
        assert!(
            !report.status.success(),
            "set_cell(table=5) ({name}) must exit nonzero even with --allow-partial: {stderr}"
        );
        assert!(
            stderr.contains(table_missing_phrase(5).as_str()),
            "set_cell(table=5) ({name}) must abort at apply time: {stderr}"
        );
        assert!(
            !output.exists(),
            "set_cell(table=5) ({name}) must not produce an output file"
        );
    }

    // The unapplied-push demo: a replace whose target string is absent is recorded
    // unapplied instead of aborting.
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_meta","key":"title","value":"partial-order"},{"op":"replace","from":"absent-string","to":"replacement"}]"#,
    )
    .unwrap();

    let default_out = dir.join("default.out.hwpx");
    let report = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&default_out)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&report.stderr);
    assert!(
        !report.status.success(),
        "the default run must bail on the unapplied second op: {stderr}"
    );
    assert!(
        stderr.contains(OPS_UNAPPLIED_MARKER),
        "the bail must carry the unapplied marker: {stderr}"
    );
    assert!(
        stderr.contains(r#"replace from="absent-string" to="replacement""#),
        "the bail must name the second op: {stderr}"
    );
    assert!(
        !default_out.exists(),
        "the unapplied bail must not produce an output file"
    );

    let partial_out = dir.join("partial.out.hwpx");
    let report = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&partial_out)
        .arg("--ops")
        .arg(&ops)
        .arg("--allow-partial")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&report.stderr);
    assert!(
        report.status.success(),
        "--allow-partial must publish the matched edits: {stderr}"
    );
    assert!(
        stderr.contains(OPS_UNAPPLIED_WARNING) && stderr.contains("absent-string"),
        "the run must warn about the skipped second op: {stderr}"
    );
    let info = hwp()
        .args(["info", "--json"])
        .arg(&partial_out)
        .output()
        .unwrap();
    let info: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&info.stdout))
        .expect("hwp info --json must parse");
    assert_eq!(
        info["metadata"]["title"], "partial-order",
        "array order must hold: set_meta applied even though the following op failed"
    );
}

/// D-08's re-styling exemption through the typed channel: style_tables-only ops on
/// a document whose GFM table is styled at import time produce zero edits and are
/// still published — the generic "no visible effect" publish guard exempts
/// style_tables, so the run exits 0 with the 이미 적용되어 있습니다 no-op note.
/// Re-running the same ops on that output stays exit 0 and byte-identical (D-08's
/// byte-stability on a second run). The exemption is style_tables-specific: the
/// same --allow-partial with a replace that matched nothing is still refused by the
/// publish guard (verified probe).
#[test]
fn publish_guard_styletables_noop() {
    let (dir, base) = table_base_for("styletables-noop");
    let ops = dir.join("ops.json");
    std::fs::write(&ops, r#"[{"op":"style_tables","preset":"official"}]"#).unwrap();

    let run = |input: &Path, out: &Path| {
        hwp()
            .arg("edit")
            .arg(input)
            .arg("-o")
            .arg(out)
            .arg("--ops")
            .arg(&ops)
            .output()
            .unwrap()
    };
    let run1_out = dir.join("run1.hwpx");
    let run1 = run(&base, &run1_out);
    let stderr1 = String::from_utf8_lossy(&run1.stderr);
    assert!(
        run1.status.success(),
        "style_tables-only ops must publish despite zero non-style edits: {stderr1}"
    );
    assert!(
        stderr1.contains("이미 적용되어 있습니다"),
        "the GFM table is styled at import time, so run 1 is a recorded no-op: {stderr1}"
    );
    assert!(run1_out.exists(), "run 1 must publish an output document");

    let run2_out = dir.join("run2.hwpx");
    let run2 = run(&run1_out, &run2_out);
    let stderr2 = String::from_utf8_lossy(&run2.stderr);
    assert!(
        run2.status.success(),
        "the second style_tables run must stay exit 0: {stderr2}"
    );
    assert!(
        stderr2.contains("이미 적용되어 있습니다"),
        "the second run must record the same no-op note: {stderr2}"
    );
    assert_bytes_eq(
        &run1_out,
        &run2_out,
        "style_tables re-run must be byte-stable",
    );

    // The exemption is style_tables-specific: the same flag with a replace-only,
    // nothing-matched plan is still refused by the publish guard.
    let replace_ops = dir.join("replace-ops.json");
    std::fs::write(
        &replace_ops,
        r#"[{"op":"replace","from":"absent-string","to":"replacement"}]"#,
    )
    .unwrap();
    let refused = dir.join("refused.out.hwpx");
    let report = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&refused)
        .arg("--ops")
        .arg(&replace_ops)
        .arg("--allow-partial")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&report.stderr);
    assert!(
        !report.status.success(),
        "a zero-edit replace plan must not be rescued by --allow-partial: {stderr}"
    );
    assert!(
        stderr.contains("게시하지 않습니다"),
        "the publish guard must refuse to publish a zero-edit replace plan: {stderr}"
    );
    assert!(
        !refused.exists(),
        "a refused plan must not produce an output file"
    );
}

/// The verified error-phrase vocabulary for value-level constraints (D-09 fold-in):
/// each malformed value exits nonzero with its Korean phrase, and which gate fired
/// was verified by trial drive. (a) insert_image's paired-mm rule and (b) add_row's
/// count lower bound are parser-level (the schema cannot encode them — usize min is
/// 0 and units are free strings), so the into_typed phrases fire. (c) set_format's
/// color and (d) clone_table's text_mode are SCHEMA enums, so the schema gate fires
/// before the parser phrases ever could. (e) a % size passes the schema's free-form
/// unit string and is rejected by the parser's pt/mm rule. Rejection tests never
/// mutate: the one-paragraph base is enough and no output file may appear.
#[test]
fn value_vocabulary() {
    let (dir, base) = new_base_for("value-vocabulary");
    for (name, body, expected) in [
        (
            "insert-image-paired-mm",
            r#"[{"op":"insert_image","anchor":"plain","path":"x.png","width_mm":"20mm"}]"#,
            "insert_image는 유한한 width_mm와 height_mm를 함께 지정해야 합니다",
        ),
        (
            "add-row-count-zero",
            r#"[{"op":"add_row","table":0,"count":0}]"#,
            "add_row: count는 1 이상이어야 합니다",
        ),
        (
            "set-format-color",
            r#"[{"op":"set_format","pattern":"plain","color":"not-a-color"}]"#,
            OPS_SCHEMA_MARKER,
        ),
        (
            "clone-table-text-mode",
            r#"[{"op":"clone_table","source_table":0,"anchor":"plain","text_mode":"bogus"}]"#,
            OPS_SCHEMA_MARKER,
        ),
        (
            "set-format-size-percent",
            r#"[{"op":"set_format","pattern":"plain","size":"50%"}]"#,
            "크기 값은 pt 또는 mm 단위여야 합니다: \"50%\" (%는 절대 pt 기준이 없습니다)",
        ),
        (
            "insert-image-width-overflow",
            r#"[{"op":"insert_image","anchor":"plain","path":"x.png","width_mm":"999999999999999999999999999999mm","height_mm":"10mm"}]"#,
            "mm 값은 유한한 0..=5000 범위여야 합니다",
        ),
        (
            "set-format-size-pt-overflow",
            r#"[{"op":"set_format","pattern":"plain","size":"99999pt"}]"#,
            "pt 값은 유한한 0..=1000 범위여야 합니다",
        ),
        (
            "set-para-line-spacing-pct-overflow",
            r#"[{"op":"set_para","pattern":"plain","line_spacing_pct":"99999%"}]"#,
            "백분율 값은 유한한 0..=1000 범위여야 합니다",
        ),
    ] {
        let ops = dir.join(format!("{name}.json"));
        std::fs::write(&ops, body).unwrap();
        let output = dir.join(format!("{name}.out.hwpx"));
        let report = hwp()
            .arg("edit")
            .arg(&base)
            .arg("-o")
            .arg(&output)
            .arg("--ops")
            .arg(&ops)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&report.stderr);
        assert!(
            !report.status.success(),
            "malformed value {name} must exit nonzero: {stderr}"
        );
        assert!(
            stderr.contains(expected),
            "malformed value {name} must carry its verified phrase: {stderr}"
        );
        assert!(
            !output.exists(),
            "malformed value {name} must not produce an output file"
        );
    }
}

/// WR-01 regression: the edit-ops-v1 schema's `align`/`orientation`/`preset` enums
/// are case-sensitive by design, unlike the equivalent CLI flags and the MCP
/// `hwp_edit` tool (both lowercase their input before matching). An uppercase or
/// mixed-case value in an ops file must be rejected at schema validation, not
/// silently normalized — the schema is the single source of the value vocabulary
/// for this channel.
#[test]
fn case_sensitive_enum_values_rejected() {
    let (dir, base) = new_base_for("case-sensitive-enum");
    for (name, body) in [
        (
            "set-align-uppercase",
            r#"[{"op":"set_align","pattern":"plain","align":"LEFT"}]"#,
        ),
        (
            "set-page-orientation-uppercase",
            r#"[{"op":"set_page","orientation":"Landscape"}]"#,
        ),
        (
            "style-tables-preset-uppercase",
            r#"[{"op":"style_tables","preset":"OFFICIAL"}]"#,
        ),
    ] {
        let ops = dir.join(format!("{name}.json"));
        std::fs::write(&ops, body).unwrap();
        let output = dir.join(format!("{name}.out.hwpx"));
        let report = hwp()
            .arg("edit")
            .arg(&base)
            .arg("-o")
            .arg(&output)
            .arg("--ops")
            .arg(&ops)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&report.stderr);
        assert!(
            !report.status.success(),
            "uppercase enum value {name} must exit nonzero: {stderr}"
        );
        assert!(
            stderr.contains(OPS_SCHEMA_MARKER),
            "uppercase enum value {name} must be rejected at schema validation, not normalized: {stderr}"
        );
        assert!(
            !output.exists(),
            "uppercase enum value {name} must not produce an output file"
        );
    }
}

/// The edit-ops-v1 contract is pinned by content hash (D-16), mirroring the
/// document-spec-v1 pin in document_spec.rs: any schema edit — even a description
/// tweak — must consciously update this constant. The schema is the shared contract
/// for `hwp edit --ops` and the MCP edit tool's ops channel.
#[test]
fn schema_hash_frozen() {
    use sha2::{Digest, Sha256};

    let digest: [u8; 32] =
        Sha256::digest(include_bytes!("../../../schemas/edit-ops-v1.schema.json")).into();
    let actual = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        actual, "faa58b2a751428bca61395d087ead98e954ef4d401f1f19ac14e839774ffef0c",
        "edit-ops-v1.schema.json changed — update the pinned contract hash consciously"
    );
}

// ── Phase 7 plan 07-05: edit report and dry-run (EDT-06) ───────────────────────

/// D-15's own frozen-hash pin, beside `schema_hash_frozen` above: any edit to
/// `edit-report-v1.schema.json` — even a description tweak — must consciously update this
/// constant.
#[test]
fn edit_report_schema_hash_frozen() {
    use sha2::{Digest, Sha256};

    let digest: [u8; 32] = Sha256::digest(include_bytes!(
        "../../../schemas/edit-report-v1.schema.json"
    ))
    .into();
    let actual = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        actual, "f081dac397d6d0bc9776e9fa3f6b3a9d76e330b595d5ea9224c037db8d9b6d70",
        "edit-report-v1.schema.json changed — update the pinned contract hash consciously"
    );
}

fn edit_report_v1_validator() -> jsonschema::Validator {
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../schemas/edit-report-v1.schema.json")).unwrap();
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .unwrap()
}

/// A single addressed `set_format` run: the report has exactly one `applied` op entry with a
/// non-empty `changed` array (the paragraph's own before/after id pair), and validates against
/// `edit-report-v1.schema.json`.
#[test]
fn addressed_set_format_report_has_before_after_ids() {
    let dir = test_dir("report-set-format");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_format","address":{"at":{"section":0,"paragraph":4,"run":0}},"bold":"on"}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let report_path = dir.join("report.json");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .arg("--report")
        .arg(&report_path)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed set_format with --report must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    let validator = edit_report_v1_validator();
    assert!(
        validator.is_valid(&report),
        "report must validate against edit-report-v1: {report}"
    );
    assert_eq!(report["contract"], "hwp-edit-report-v1");
    assert_eq!(report["dry_run"], false);
    assert_eq!(report["applied_count"], 1);
    assert_eq!(report["failed_count"], 0);
    let ops = report["ops"].as_array().unwrap();
    assert_eq!(ops.len(), 1, "one op in, one outcome out: {ops:?}");
    assert_eq!(ops[0]["index"], 0);
    assert_eq!(ops[0]["op"], "set_format");
    assert_eq!(ops[0]["status"], "applied");
    let changed = ops[0]["changed"].as_array().unwrap();
    assert!(
        !changed.is_empty(),
        "a single addressed set_format must report a non-empty changed array: {changed:?}"
    );
    for pair in changed {
        assert!(
            pair["before"].is_string() && pair["after"].is_string(),
            "a same-paragraph restyle changes an EXISTING segment, neither side is a creation/removal: {pair}"
        );
        assert_ne!(
            pair["before"], pair["after"],
            "a changed pair must actually differ: {pair}"
        );
    }
}

/// A run-range `set_format` that splits a run reports the touched paragraph's OWN id change AND
/// the id change of every LATER run in the paragraph — not only the run it targeted (Pitfall 2 /
/// the plan's own prohibition 3). Reuses `addressed_run_range_restyles_the_named_sub_range_only`'s
/// fixture: restyling `run:1` (`"bold"`, wchar `[6,10)`) at `chars:[7,9]` splits it into three
/// pieces, shifting the canonical index — and therefore the id — of the trailing `" tail"` run.
#[test]
fn addressed_run_split_report_cascades_later_run_ids() {
    let dir = test_dir("report-run-split");
    let md = dir.join("doc.md");
    std::fs::write(&md, RUN_RANGE_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let before_doc = hwpx::read_document(&base).unwrap().document;
    let before_para = &before_doc.sections[0].paragraphs[1];
    let before_runs = hwp_convert::canonical_char_shape_runs(before_para);
    assert_eq!(
        before_runs.len(),
        3,
        "plain/bold/tail must start as three runs"
    );

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_format","address":{"at":{"section":0,"paragraph":1,"run":1},"chars":[7,9]},"italic":"on"}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let report_path = dir.join("report.json");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .arg("--report")
        .arg(&report_path)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "run-split set_format with --report must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let after_doc = hwpx::read_document(&output).unwrap().document;
    let after_para = &after_doc.sections[0].paragraphs[1];
    let after_runs = hwp_convert::canonical_char_shape_runs(after_para);
    assert!(
        after_runs.len() > before_runs.len(),
        "restyling a sub-range of run 1 must split it into more runs: before={} after={}",
        before_runs.len(),
        after_runs.len()
    );

    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert!(
        edit_report_v1_validator().is_valid(&report),
        "report must validate against edit-report-v1: {report}"
    );
    let changed = report["ops"][0]["changed"].as_array().unwrap();
    // The paragraph's own pair, plus at least one pair per run whose canonical index moved
    // (every run from the split point onward) — never just the one pair a targeted-only report
    // would produce.
    assert!(
        changed.len() >= 3,
        "a run split must cascade to the paragraph AND every later run, not one pair: {changed:?}"
    );
}

/// An addressed `insert_para` reports the newly-created paragraph's own id with a null `before`
/// (D-13's creation case) — it did not exist before the batch.
#[test]
fn addressed_insert_para_report_marks_creation_with_null_before() {
    let dir = test_dir("report-insert-para");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);
    let output = dir.join("out.hwpx");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"insert_para","address":{"at":{"section":0,"paragraph":4}},"before":false,"text":"NEW BESIDE SECOND"}]"#,
    )
    .unwrap();
    let report_path = dir.join("report.json");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .arg("--report")
        .arg(&report_path)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed insert_para with --report must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert!(
        edit_report_v1_validator().is_valid(&report),
        "report must validate against edit-report-v1: {report}"
    );
    let changed = report["ops"][0]["changed"].as_array().unwrap();
    assert_eq!(
        changed.len(),
        1,
        "insert_para creates exactly one new paragraph: {changed:?}"
    );
    assert!(
        changed[0]["before"].is_null(),
        "a creation must report a null before: {}",
        changed[0]
    );
    assert!(
        changed[0]["after"].is_string(),
        "the created paragraph's after id must be present: {}",
        changed[0]
    );
}

/// An addressed `delete_para` reports the removed paragraph's before id with a null `after`
/// (D-13's removal case) — it no longer exists after the batch.
#[test]
fn addressed_delete_para_report_marks_removal_with_null_after() {
    let dir = test_dir("report-delete-para");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);
    let output = dir.join("out.hwpx");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"delete_para","address":{"at":{"section":0,"paragraph":4}}}]"#,
    )
    .unwrap();
    let report_path = dir.join("report.json");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .arg("--report")
        .arg(&report_path)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed delete_para with --report must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert!(
        edit_report_v1_validator().is_valid(&report),
        "report must validate against edit-report-v1: {report}"
    );
    let changed = report["ops"][0]["changed"].as_array().unwrap();
    assert_eq!(
        changed.len(),
        1,
        "delete_para removes exactly one paragraph: {changed:?}"
    );
    assert!(
        changed[0]["before"].is_string(),
        "the removed paragraph's before id must be present: {}",
        changed[0]
    );
    assert!(
        changed[0]["after"].is_null(),
        "a removal must report a null after: {}",
        changed[0]
    );
}

/// A batch pairing one op that applies with one pattern-form op that matches nothing, under
/// `--allow-partial`, reports the first as applied and the second as failed with a populated
/// reason; `failed_count` counts it, matching the stderr summary's own accounting.
#[test]
fn failed_op_under_allow_partial_reports_status_failed() {
    let dir = test_dir("report-failed-op");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);
    let output = dir.join("out.hwpx");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[
          {"op":"set_align","address":{"at":{"section":0,"paragraph":4}},"align":"right"},
          {"op":"set_align","pattern":"NO_SUCH_TEXT_ANYWHERE","align":"left"}
        ]"#,
    )
    .unwrap();
    let report_path = dir.join("report.json");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .arg("--allow-partial")
        .arg("--report")
        .arg(&report_path)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "a partially-matched batch under --allow-partial must still succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert!(
        edit_report_v1_validator().is_valid(&report),
        "report must validate against edit-report-v1: {report}"
    );
    assert_eq!(report["applied_count"], 1);
    assert_eq!(report["failed_count"], 1);
    let ops = report["ops"].as_array().unwrap();
    assert_eq!(ops[0]["status"], "applied");
    assert_eq!(ops[1]["status"], "failed");
    assert!(
        ops[1]["reason"].is_string() && !ops[1]["reason"].as_str().unwrap().is_empty(),
        "a failed op must carry a populated reason: {}",
        ops[1]
    );
    assert_eq!(ops[1]["pieces_touched"], 0);
    assert!(ops[1]["changed"].as_array().unwrap().is_empty());
}

/// #348: a failed op's `reason` is a fixed `<op>: <cause>` label. None of the request's own
/// strings (patterns, replacements, anchors, names, urls, values, text) reach the report, while
/// the operator-facing stderr summary still names the request in full.
#[test]
fn failed_op_reason_carries_no_request_text() {
    let dir = test_dir("report-reason-content-free");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);
    // Every failing op below carries its own "LEAK-" string; the first op applies so the
    // batch publishes under --allow-partial.
    let cases = [
        (r#"{"op":"set_meta","key":"title","value":"applied"}"#, None),
        (
            r#"{"op":"replace","from":"LEAK-replace-from","to":"LEAK-replace-to"}"#,
            Some("replace: no match"),
        ),
        (
            r#"{"op":"replace","from":"LEAK-same","to":"LEAK-same"}"#,
            Some("replace: empty pattern or identical replacement"),
        ),
        (
            r#"{"op":"replace","address":{"at":{"section":0,"paragraph":1}},"from":"LEAK-addr-from","to":"LEAK-addr-to"}"#,
            Some("replace: no match at the address"),
        ),
        (
            r#"{"op":"set_cell_by_label","label":"LEAK-label","text":"LEAK-label-text"}"#,
            Some("적용되지 않음 (사전 검증 단계에서 이미 확인됨)"),
        ),
        (
            r#"{"op":"create_field","anchor":"LEAK-field-anchor","name":"LEAK-field-name","value":"LEAK-field-value"}"#,
            Some("create_field: anchor not found"),
        ),
        (
            r#"{"op":"create_bookmark","anchor":"LEAK-bookmark-anchor","name":"LEAK-bookmark-name"}"#,
            Some("create_bookmark: anchor not found"),
        ),
        (
            r#"{"op":"create_hyperlink","anchor":"LEAK-link-anchor","display":"LEAK-link-display","url":"https://example.com/LEAK-link-url"}"#,
            Some("create_hyperlink: anchor not found"),
        ),
        (
            r#"{"op":"set_field","name":"LEAK-setfield-name","value":"LEAK-setfield-value"}"#,
            Some("set_field: no match or no change"),
        ),
        (
            r#"{"op":"set_format","pattern":"LEAK-format-pattern","bold":"on"}"#,
            Some("set_format: no match or no change"),
        ),
        (
            r#"{"op":"set_align","pattern":"LEAK-align-pattern","align":"left"}"#,
            Some("set_align: no match or no change"),
        ),
        (
            r#"{"op":"set_para","pattern":"LEAK-setpara-pattern","align":"center"}"#,
            Some("set_para: no match or no change"),
        ),
        (
            r#"{"op":"insert_para","anchor":"LEAK-insert-anchor","text":"LEAK-insert-text"}"#,
            Some("insert_para: anchor not found"),
        ),
        (
            r#"{"op":"delete_para","matching":"LEAK-delete-matching"}"#,
            Some("delete_para: no match"),
        ),
        (
            r#"{"op":"delete_image","anchor":"LEAK-image-anchor"}"#,
            Some("delete_image: no match"),
        ),
        (
            r#"{"op":"delete_table","anchor":"LEAK-table-anchor"}"#,
            Some("delete_table: no match"),
        ),
        (
            r#"{"op":"delete_field","name":"LEAK-delfield-name"}"#,
            Some("delete_field: no match"),
        ),
        (
            r#"{"op":"delete_bookmark","name":"LEAK-delbookmark-name"}"#,
            Some("delete_bookmark: no match"),
        ),
    ];
    let ops = dir.join("ops.json");
    let batch = cases.iter().map(|(op, _)| *op).collect::<Vec<_>>();
    std::fs::write(&ops, format!("[{}]", batch.join(","))).unwrap();
    let report_path = dir.join("report.json");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(dir.join("out.hwpx"))
        .arg("--ops")
        .arg(&ops)
        .arg("--allow-partial")
        .arg("--report")
        .arg(&report_path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(
        stderr.contains("LEAK-replace-from"),
        "stderr keeps naming the unapplied request: {stderr}"
    );

    let text = std::fs::read_to_string(&report_path).unwrap();
    assert!(
        !text.contains("LEAK"),
        "request text leaked into the report: {text}"
    );
    let report: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(edit_report_v1_validator().is_valid(&report), "{report}");
    let outcomes = report["ops"].as_array().unwrap();
    assert_eq!(outcomes.len(), cases.len(), "{report}");
    for (outcome, (op, reason)) in outcomes.iter().zip(&cases) {
        match reason {
            None => assert_eq!(outcome["status"], "applied", "{op}: {outcome}"),
            Some(reason) => {
                assert_eq!(outcome["status"], "failed", "{op}: {outcome}");
                assert_eq!(outcome["reason"], *reason, "{op}: {outcome}");
            }
        }
    }
}

/// A run whose only op matches nothing still writes `--report`'s file when given, even though
/// `execute()` itself errors — a caller diagnosing why nothing applied needs the `ops` array, not
/// only the error string.
#[test]
fn zero_edit_run_still_writes_report() {
    let dir = test_dir("report-zero-edit");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);
    let output = dir.join("out.hwpx");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_align","pattern":"NO_SUCH_TEXT_ANYWHERE","align":"left"}]"#,
    )
    .unwrap();
    let report_path = dir.join("report.json");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .arg("--report")
        .arg(&report_path)
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "zero applicable edits must still fail the command overall"
    );
    assert!(
        !output.exists(),
        "a failed run must not publish an output file"
    );
    assert!(
        report_path.exists(),
        "the --report file must exist even though the run aborted: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert!(
        edit_report_v1_validator().is_valid(&report),
        "report must validate against edit-report-v1: {report}"
    );
    assert_eq!(report["applied_count"], 0);
    assert_eq!(report["failed_count"], 1);
    assert_eq!(report["ops"][0]["status"], "failed");
}

/// The SAME zero-edit case, but reached via the second guard: `--allow-partial` clears the
/// unapplied-requests bail, so the run must instead abort on "no applicable edits" — and still
/// write the report.
#[test]
fn zero_edit_run_under_allow_partial_still_writes_report() {
    let dir = test_dir("report-zero-edit-allow-partial");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);
    let output = dir.join("out.hwpx");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_align","pattern":"NO_SUCH_TEXT_ANYWHERE","align":"left"}]"#,
    )
    .unwrap();
    let report_path = dir.join("report.json");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .arg("--allow-partial")
        .arg("--report")
        .arg(&report_path)
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "zero applicable edits must fail even under --allow-partial"
    );
    assert!(!output.exists());
    assert!(report_path.exists());
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["applied_count"], 0);
    assert_eq!(report["failed_count"], 1);
}

/// `--dry-run` writes neither the output file nor anything at the destination path, and prints
/// the `edit-report-v1` report to stdout when no `--report` path was given.
#[test]
fn dry_run_writes_no_output_file_and_prints_report_to_stdout() {
    let dir = test_dir("dry-run-no-output");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);
    let output = dir.join("out.hwpx");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_format","address":{"at":{"section":0,"paragraph":4,"run":0}},"bold":"on"}]"#,
    )
    .unwrap();
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .arg("--dry-run")
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "dry-run of an applicable batch must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        !output.exists(),
        "--dry-run must not write an output file at the destination path"
    );
    let stdout = String::from_utf8_lossy(&run.stdout);
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("--dry-run stdout must be the edit-report-v1 JSON: {e}: {stdout}")
    });
    assert!(edit_report_v1_validator().is_valid(&report));
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["applied_count"], 1);
}

/// A dry-run over an EXISTING destination leaves that file's bytes unchanged (T-07-21: dry-run
/// must never publish its staged output).
#[test]
fn dry_run_over_existing_destination_leaves_bytes_unchanged() {
    let dir = test_dir("dry-run-existing-dest");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);
    let output = dir.join("out.hwpx");
    // A plausible pre-existing destination: a real hwpx file with different bytes than the
    // dry-run would ever have produced (a straight copy of the input, not the edited output).
    std::fs::copy(&base, &output).unwrap();
    let existing_bytes = std::fs::read(&output).unwrap();

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_format","address":{"at":{"section":0,"paragraph":4,"run":0}},"bold":"on"}]"#,
    )
    .unwrap();
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .arg("--dry-run")
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "dry-run over an existing destination must still succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let after_bytes = std::fs::read(&output).unwrap();
    assert_eq!(
        existing_bytes, after_bytes,
        "a dry-run must leave an existing destination's bytes completely unchanged"
    );
}

/// A dry-run report and a real-run report of the SAME batch differ only in `dry_run` — the
/// entire preflight/apply-loop/id-derivation path is shared, so `--dry-run` never has to lie
/// about target misses or resulting ids (D-14).
#[test]
fn dry_run_report_matches_real_run_report_except_dry_run_field() {
    let dir = test_dir("dry-run-vs-real");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_format","address":{"at":{"section":0,"paragraph":4,"run":0}},"bold":"on"}]"#,
    )
    .unwrap();

    let dry_output = dir.join("dry.hwpx");
    let dry_report_path = dir.join("dry-report.json");
    let dry_run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&dry_output)
        .arg("--ops")
        .arg(&ops)
        .arg("--dry-run")
        .arg("--report")
        .arg(&dry_report_path)
        .output()
        .unwrap();
    assert!(
        dry_run.status.success(),
        "{}",
        String::from_utf8_lossy(&dry_run.stderr)
    );

    let real_output = dir.join("real.hwpx");
    let real_report_path = dir.join("real-report.json");
    let real_run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&real_output)
        .arg("--ops")
        .arg(&ops)
        .arg("--report")
        .arg(&real_report_path)
        .output()
        .unwrap();
    assert!(
        real_run.status.success(),
        "{}",
        String::from_utf8_lossy(&real_run.stderr)
    );

    let mut dry_report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&dry_report_path).unwrap()).unwrap();
    let mut real_report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&real_report_path).unwrap()).unwrap();
    assert_eq!(
        dry_report["dry_run"], true,
        "the dry-run report must say so: {dry_report}"
    );
    assert_eq!(
        real_report["dry_run"], false,
        "the real-run report must say so: {real_report}"
    );
    // Normalize the two known-divergent fields (dry_run itself, and output — the two runs wrote
    // to different paths) before comparing the rest of the report byte-for-byte.
    dry_report["dry_run"] = serde_json::Value::Null;
    real_report["dry_run"] = serde_json::Value::Null;
    dry_report["output"] = serde_json::Value::Null;
    real_report["output"] = serde_json::Value::Null;
    assert_eq!(
        dry_report, real_report,
        "a dry-run report and a real-run report of the same batch must differ only in dry_run/output"
    );
}

/// #332: a replace-only batch on hwpx -> hwpx takes the package-preserving fast path, whose
/// report must carry the same per-op outcomes the apply loop reports for the same batch (forced
/// here by an `.hwp` output): on a partial success under `--allow-partial`, and on the abort
/// without it, where the fast path used to write no report at all.
#[test]
fn fast_path_replace_report_matches_the_apply_loop_report() {
    let dir = test_dir("report-fast-path");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[
          {"op":"replace","from":"첫 문단","to":"FIRST"},
          {"op":"replace","from":"NO_SUCH_TEXT_ANYWHERE","to":"X"},
          {"op":"replace","from":"같은 문단","to":"SAME"}
        ]"#,
    )
    .unwrap();
    let run = |output: &str, extra: &[&str]| {
        let report_path = dir.join(format!("{output}.json"));
        let run = hwp()
            .arg("edit")
            .arg(&base)
            .arg("-o")
            .arg(dir.join(output))
            .arg("--ops")
            .arg(&ops)
            .args(extra)
            .arg("--report")
            .arg(&report_path)
            .output()
            .unwrap();
        let mut report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&report_path).unwrap_or_else(|e| {
                panic!(
                    "{output}: --report must be written: {e}: {}",
                    String::from_utf8_lossy(&run.stderr)
                )
            }))
            .unwrap();
        assert!(
            edit_report_v1_validator().is_valid(&report),
            "{output}: report must validate against edit-report-v1: {report}"
        );
        report["output"] = serde_json::Value::Null;
        (run, report)
    };

    // --allow-partial: both paths publish, and op 1 is the one failed op.
    let (fast, fast_report) = run("partial-fast.hwpx", &["--allow-partial"]);
    let (slow, slow_report) = run("partial-slow.hwp", &["--allow-partial"]);
    let fast_stderr = String::from_utf8_lossy(&fast.stderr);
    assert!(fast.status.success(), "{fast_stderr}");
    assert!(
        slow.status.success(),
        "{}",
        String::from_utf8_lossy(&slow.stderr)
    );
    assert!(
        fast_stderr.contains("치환(패키지 보존)"),
        "a replace-only hwpx batch must take the package-preserving fast path: {fast_stderr}"
    );
    assert_eq!(fast_report["applied_count"], 2, "{fast_report}");
    assert_eq!(fast_report["failed_count"], 1, "{fast_report}");
    assert_eq!(fast_report["ops"][1]["status"], "failed", "{fast_report}");
    assert_eq!(
        fast_report["ops"][1]["reason"], "replace: no match",
        "{fast_report}"
    );
    assert!(
        !fast_report.to_string().contains("NO_SUCH_TEXT_ANYWHERE"),
        "the fast path's reason must not echo the pattern (#348): {fast_report}"
    );
    assert_eq!(
        fast_report, slow_report,
        "the fast path must report what the apply loop reports for the same batch"
    );

    // Without --allow-partial both paths abort; the fast path's abort still writes the report.
    let (fast, fast_report) = run("abort-fast.hwpx", &[]);
    let (slow, slow_report) = run("abort-slow.hwp", &[]);
    let fast_stderr = String::from_utf8_lossy(&fast.stderr);
    assert!(!fast.status.success() && !slow.status.success());
    assert!(
        fast_stderr.contains("런 분절 교차 매칭은 미지원"),
        "the abort must come from the fast path: {fast_stderr}"
    );
    assert!(!dir.join("abort-fast.hwpx").exists());
    assert_eq!(fast_report, slow_report);
}

use std::io::Write as _;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// stdin 편집 완료 마커 접두어 (edit.rs: "편집 완료: {input} → {output}").
const STDIN_DONE_MARKER: &str = "편집 완료:";

/// read_bounded가 상한 초과 입력에 내보내는 bail 문구 (edit_ops.rs: cap =
/// MAX_OPS_BYTES = 16 MiB가 십진 포맷으로 그대로 들어간다).
const OPS_SIZE_LIMIT_MARKER: &str = "입력이 크기 제한(16777216바이트)을 초과했습니다";

/// read_bounded가 읽기 실패(non-UTF-8 포함)에 내보내는 오류 접두어
/// ("편집 연산 입력을 읽을 수 없습니다: {error}").
const OPS_READ_ERROR_MARKER: &str = "편집 연산 입력을 읽을 수 없습니다";

/// MAX_OPS_BYTES (edit_ops.rs) — integration tests cannot see pub(crate) consts.
const MAX_OPS_BYTES: usize = 16 * 1024 * 1024;

/// `hwp edit <doc> -o <out> --ops -`를 stdin 파이프로 스폰한다 (CLI argv 표기:
/// stdin은 `-`로만 쓴다 — `--ops-stdin` 플래그는 존재하지 않는다).
fn spawn_edit_stdin(base: &Path, out: &Path) -> std::process::Child {
    hwp()
        .arg("edit")
        .arg(base)
        .arg("-o")
        .arg(out)
        .arg("--ops")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hwp edit --ops -")
}

/// payload를 stdin에 기록하고 자식 종료를 수거한다. 자식이 상한 초과 등으로 조기
/// 종료하면 남은 쓰기는 BrokenPipe — 허용하고 계속한다. 자식이 읽지 않아 부모가
/// 영원히 막히는 회귀는 워치독이 grace 후 자식을 kill해 최악에도 테스트 실패로
/// 전환한다 (D-16 no-hang guard).
fn feed_stdin_and_collect(
    mut child: std::process::Child,
    payload: &[u8],
    grace_secs: u64,
) -> std::process::Output {
    let done = Arc::new(AtomicBool::new(false));
    let pid = child.id();
    let watchdog_flag = Arc::clone(&done);
    std::thread::spawn(move || {
        for _ in 0..grace_secs {
            if watchdog_flag.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        if !watchdog_flag.load(Ordering::Relaxed) {
            let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
        }
    });

    let mut stdin = child.stdin.take().expect("stdin must be piped");
    if let Err(error) = stdin.write_all(payload) {
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::BrokenPipe,
            "stdin write failed unexpectedly: {error}"
        );
    }
    drop(stdin);
    let output = child.wait_with_output().expect("collect hwp edit output");
    done.store(true, Ordering::Relaxed);
    output
}

/// The committed stdin fragment piped through `--ops -` is lossless end to end
/// (D-05): insert_para seeds "seed a=>b" after the "intro" anchor, replace rewrites
/// it to "seed c=d:e", and the run exits 0 with the 편집 완료 marker and no
/// unapplied note. The notation is pinned too: `hwp edit --help` lists `--ops
/// <FILE>` and there is no `--ops-stdin` flag — stdin is spelled `--ops -`.
#[test]
fn stdin_roundtrip_lossless() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/edit-ops/stdin-fragment.json");
    assert!(
        fixture.exists(),
        "stdin fixture missing: {}",
        fixture.display()
    );
    let payload = std::fs::read(&fixture).unwrap();

    let dir = test_dir("stdin-roundtrip");
    let md = dir.join("doc.md");
    std::fs::write(&md, "# T\n\nintro\n\n").unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let out = dir.join("out.hwpx");
    let child = spawn_edit_stdin(&base, &out);
    let report = feed_stdin_and_collect(child, &payload, 60);
    let stderr = String::from_utf8_lossy(&report.stderr);
    assert!(
        report.status.success(),
        "hwp edit --ops - via stdin must succeed: {stderr}"
    );
    assert!(
        stderr.contains(STDIN_DONE_MARKER),
        "the stdin run must carry the success marker: {stderr}"
    );
    assert!(
        !stderr.contains(OPS_UNAPPLIED_MARKER),
        "the stdin run must not report unapplied edits: {stderr}"
    );
    assert!(
        out.exists(),
        "the stdin run must publish an output document"
    );

    let doc = hwpx::read_document(&out).unwrap().document;
    let texts: Vec<String> = doc
        .sections
        .iter()
        .flat_map(|section| &section.paragraphs)
        .map(|paragraph| paragraph.plain_text())
        .collect();
    assert!(
        texts.iter().any(|text| text == "seed c=d:e"),
        "the seeded insert plus replace must land as exactly seed c=d:e, got {texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text.contains("a=>b")),
        "the pre-replace payload must be fully rewritten, got {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text == "intro"),
        "the anchor paragraph must survive, got {texts:?}"
    );

    // CLI 표기 고정: stdin은 `--ops -`로만 쓴다.
    let help = hwp().args(["edit", "--help"]).output().unwrap();
    let help_text = format!(
        "{}{}",
        String::from_utf8_lossy(&help.stdout),
        String::from_utf8_lossy(&help.stderr)
    );
    assert!(
        help_text.contains("--ops <FILE>"),
        "hwp edit --help must list --ops <FILE>: {help_text}"
    );
    assert!(
        !help_text.contains("--ops-stdin"),
        "there is no --ops-stdin flag; stdin is spelled --ops -: {help_text}"
    );
}

/// stdin input limits (D-02/D-16): (a) stdin over the 16 MiB cap terminates the
/// process on its own (never a hang — the watchdog caps the wait and a signal kill
/// fails the assert) with the size-limit phrase and no output file, and (b)
/// non-UTF-8 stdin surfaces the read-error phrase instead of panicking. The
/// oversized payload deliberately exceeds the cap+1 the child reads, so the tail
/// write may hit a broken pipe — tolerated by the helper per the early-exit contract.
#[test]
fn stdin_limits() {
    let dir = test_dir("stdin-limits");
    let md = dir.join("doc.md");
    std::fs::write(&md, "# T\n\nlimits probe\n\n").unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    // (a) 16 MiB 상한 초과 stdin.
    let mut payload = b"[".to_vec();
    payload.extend(vec![b'x'; MAX_OPS_BYTES + 65536 - payload.len() - 1]);
    payload.push(b']');
    assert!(
        payload.len() > MAX_OPS_BYTES,
        "the oversized payload must exceed the cap"
    );
    let out = dir.join("oversized.out.hwpx");
    let child = spawn_edit_stdin(&base, &out);
    let report = feed_stdin_and_collect(child, &payload, 60);
    let stderr = String::from_utf8_lossy(&report.stderr);
    assert!(
        !report.status.success(),
        "oversized stdin must exit nonzero: {stderr}"
    );
    assert!(
        report.status.code().is_some(),
        "the process must terminate on its own, not be killed by a signal: {:?}",
        report.status
    );
    assert!(
        stderr.contains(OPS_SIZE_LIMIT_MARKER),
        "oversized stdin must carry the size-limit phrase: {stderr}"
    );
    assert!(
        !out.exists(),
        "oversized stdin must not produce an output file"
    );

    // (b) non-UTF-8 stdin — 패닉 없이 읽기 오류로 표면화.
    let payload: Vec<u8> = [0xFF, 0xFE, 0x00, 0x01].repeat(1024);
    let out = dir.join("bad-utf8.out.hwpx");
    let child = spawn_edit_stdin(&base, &out);
    let report = feed_stdin_and_collect(child, &payload, 60);
    let stderr = String::from_utf8_lossy(&report.stderr);
    assert!(
        !report.status.success(),
        "non-UTF-8 stdin must exit nonzero: {stderr}"
    );
    assert!(
        stderr.contains(OPS_READ_ERROR_MARKER),
        "non-UTF-8 stdin must surface the read-error phrase: {stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "non-UTF-8 input must never panic the child: {stderr}"
    );
    assert!(
        !out.exists(),
        "non-UTF-8 stdin must not produce an output file"
    );
}

// ── Phase 7 plan 07-03: structural addressed ops and batch index drift (EDT-05) ────────

/// Eight distinct top-level paragraphs (`# T` heading at index 0, seven body paragraphs at
/// indices 1..7) so a drift test can assert on exact text rather than a re-derived index — an
/// implementation that naively re-indexes fails these assertions instead of silently passing.
const DRIFT_MD: &str = "# T\n\nalpha\n\nbravo\n\ncharlie\n\ndelta\n\necho\n\nfoxtrot\n\ngolf\n";

fn drift_base(dir: &Path) -> PathBuf {
    let md = dir.join("doc.md");
    std::fs::write(&md, DRIFT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);
    base
}

fn plain_texts(doc: &hwp_model::Document) -> Vec<String> {
    doc.sections[0]
        .paragraphs
        .iter()
        .map(|p| p.plain_text())
        .collect()
}

/// `[delete_para at original 3 ("charlie"), set_para at original 6 ("foxtrot")]`: the tracker
/// must restyle "foxtrot" (the paragraph whose ORIGINAL index was 6), not "delta" (the paragraph
/// that slides into live index 6 after the delete) — a naive re-index bug asserts on text, so it
/// fails here instead of silently passing (D-06, planner decision 3, Pitfall 1).
#[test]
fn drift_delete_then_set_para_targets_the_original_index() {
    let dir = test_dir("drift-delete-set-para");
    let base = drift_base(&dir);
    let output = dir.join("out.hwpx");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[
          {"op":"delete_para","address":{"at":{"section":0,"paragraph":3}}},
          {"op":"set_para","address":{"at":{"section":0,"paragraph":6}},"align":"center"}
        ]"#,
    )
    .unwrap();
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "drift batch must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let after = hwpx::read_document(&output).unwrap().document;
    assert_eq!(
        plain_texts(&after),
        vec!["1. T", "alpha", "bravo", "delta", "echo", "foxtrot", "golf"],
        "charlie must be gone, every other paragraph must survive in order"
    );
    let foxtrot = after.sections[0]
        .paragraphs
        .iter()
        .find(|p| p.plain_text() == "foxtrot")
        .unwrap();
    let ps = &after.header.para_shapes[foxtrot.para_shape.0 as usize];
    assert_eq!(
        ps.alignment(),
        3,
        "set_para must have restyled the paragraph whose ORIGINAL index was 6 (foxtrot)"
    );
    let delta = after.sections[0]
        .paragraphs
        .iter()
        .find(|p| p.plain_text() == "delta")
        .unwrap();
    let delta_ps = &after.header.para_shapes[delta.para_shape.0 as usize];
    assert_ne!(
        delta_ps.alignment(),
        3,
        "delta (which slid into live index 6) must NOT have been restyled"
    );
}

/// `[insert_para before original 2 ("bravo"), delete_para at original 5 ("echo")]`: the tracker
/// must delete "echo" (the paragraph whose ORIGINAL index was 5), not "delta" (whatever a naive
/// re-index would land on) — asserted on the exact resulting text sequence.
#[test]
fn drift_insert_then_delete_targets_the_original_index() {
    let dir = test_dir("drift-insert-delete");
    let base = drift_base(&dir);
    let output = dir.join("out.hwpx");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[
          {"op":"insert_para","address":{"at":{"section":0,"paragraph":2}},"before":true,"text":"NEW"},
          {"op":"delete_para","address":{"at":{"section":0,"paragraph":5}}}
        ]"#,
    )
    .unwrap();
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "drift batch must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let after = hwpx::read_document(&output).unwrap().document;
    assert_eq!(
        plain_texts(&after),
        vec![
            "1. T", "alpha", "NEW", "bravo", "charlie", "delta", "foxtrot", "golf"
        ],
        "NEW must land before bravo and echo (original index 5) must be the one deleted"
    );
}

/// `[delete_para at X, set_para at X]`: the removal clause (07-03, T-07-12) rejects this during
/// preflight — 0 ops applied, no output file, both op indices named in the error.
#[test]
fn destructive_conflict_delete_then_set_para_same_address_is_rejected() {
    let dir = test_dir("destructive-delete-set-para");
    let base = drift_base(&dir);
    let output = dir.join("out.hwpx");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[
          {"op":"delete_para","address":{"at":{"section":0,"paragraph":3}}},
          {"op":"set_para","address":{"at":{"section":0,"paragraph":3}},"align":"center"}
        ]"#,
    )
    .unwrap();
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "delete then set_para on the same address must be rejected"
    );
    assert!(!output.exists(), "no output file may be written");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("op[0]"), "error must name op 0: {stderr}");
    assert!(stderr.contains("op[1]"), "error must name op 1: {stderr}");
}

/// `[move_para from X, set_format inside X]`: the removal clause treats a `move_para`'s source
/// exactly like `delete_para`'s target — a later op addressing inside the same paragraph is
/// rejected the same way.
#[test]
fn destructive_conflict_move_source_then_set_format_inside_is_rejected() {
    let dir = test_dir("destructive-move-set-format");
    let md = dir.join("doc.md");
    std::fs::write(&md, RUN_RANGE_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);
    let output = dir.join("out.hwpx");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[
          {"op":"move_para","address":{"at":{"section":0,"paragraph":1}},"to":{"address":{"at":{"section":0,"paragraph":0}},"position":"after"}},
          {"op":"set_format","address":{"at":{"section":0,"paragraph":1,"run":1}},"italic":"on"}
        ]"#,
    )
    .unwrap();
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "move source then set_format inside it must be rejected"
    );
    assert!(!output.exists(), "no output file may be written");
}

/// `[set_align at X, set_para at X]`: two non-destructive paragraph-level ops on one address
/// compose in array order (Phase 6 D-01) — the removal clause must not widen to reject this.
#[test]
fn non_destructive_paragraph_ops_on_one_address_still_compose() {
    let dir = test_dir("non-destructive-para-compose");
    let base = drift_base(&dir);
    let output = dir.join("out.hwpx");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[
          {"op":"set_align","address":{"at":{"section":0,"paragraph":1}},"align":"center"},
          {"op":"set_para","address":{"at":{"section":0,"paragraph":1}},"line_spacing_pct":"160%"}
        ]"#,
    )
    .unwrap();
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "two non-destructive addressed ops on one paragraph must both apply: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let after = hwpx::read_document(&output).unwrap().document;
    let para = &after.sections[0].paragraphs[1];
    let ps = &after.header.para_shapes[para.para_shape.0 as usize];
    assert_eq!(ps.alignment(), 3, "set_align must have applied");
    assert_eq!(
        ps.line_spacing, 160,
        "set_para's line_spacing_pct must also have applied"
    );
}

/// An addressed `insert_para` targeting the SECOND of two identical-text paragraphs inserts
/// beside that one; the first occurrence is untouched (EDT-05 success criterion 1). A first-match
/// implementation would insert next to the first occurrence instead and fail this test.
#[test]
fn addressed_insert_para_hits_beside_the_named_duplicate() {
    let dir = test_dir("addr-insert-duplicate");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);
    let output = dir.join("out.hwpx");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"insert_para","address":{"at":{"section":0,"paragraph":4}},"before":false,"text":"NEW BESIDE SECOND"}]"#,
    )
    .unwrap();
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed insert_para must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let after = hwpx::read_document(&output).unwrap().document;
    let texts = plain_texts(&after);
    assert_eq!(texts.len(), 6, "one paragraph inserted: {texts:?}");
    assert_eq!(
        texts[2], "같은 문단",
        "the first occurrence must be untouched at its original index: {texts:?}"
    );
    assert_eq!(
        texts[4], "같은 문단",
        "the second occurrence must still be at its own index: {texts:?}"
    );
    assert_eq!(
        texts[5], "NEW BESIDE SECOND",
        "the new paragraph must land right after the SECOND occurrence, not the first: {texts:?}"
    );
}

/// An addressed `delete_para` targeting the SECOND of two identical-text paragraphs deletes only
/// that one; the first occurrence survives (EDT-05 success criterion 1).
#[test]
fn addressed_delete_para_hits_only_the_named_duplicate() {
    let dir = test_dir("addr-delete-duplicate");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);
    let output = dir.join("out.hwpx");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"delete_para","address":{"at":{"section":0,"paragraph":4}}}]"#,
    )
    .unwrap();
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed delete_para must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let after = hwpx::read_document(&output).unwrap().document;
    let texts = plain_texts(&after);
    assert_eq!(
        texts,
        vec!["1. 제목", "첫 문단", "같은 문단", "셋째 문단"],
        "only the SECOND occurrence must be removed, the first survives at its own index: {texts:?}"
    );
}

/// An addressed `move_para` targeting the SECOND of two identical-text paragraphs moves only
/// that one — verified by `instance_id` identity, not index arithmetic, so the assertion is
/// exact regardless of how the move shifts surrounding indices. A first-match implementation
/// would move the FIRST occurrence's `instance_id` instead and fail these assertions.
#[test]
fn addressed_move_para_hits_only_the_named_duplicate() {
    // hwp5 output specifically: instance_id uniqueness (compat rule A8) is a synthesis-write-path
    // guarantee; hwpx output does not assign the same meaningful non-zero ids, so an hwpx round
    // trip cannot prove this identity claim.
    let dir = test_dir("addr-move-duplicate");
    let md = dir.join("doc.md");
    std::fs::write(&md, DUPLICATE_TEXT_MD).unwrap();
    let base = dir.join("base.hwp");
    new_from(&md, &base);

    let before = hwp5::read_document(&base).unwrap().document;
    let first_id = before.sections[0].paragraphs[2].header.instance_id;
    let second_id = before.sections[0].paragraphs[4].header.instance_id;
    assert_ne!(first_id, second_id, "fixture sanity: distinct instance ids");

    let output = dir.join("out.hwp");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"move_para","address":{"at":{"section":0,"paragraph":4}},"to":{"address":{"at":{"section":0,"paragraph":1}},"position":"after"}}]"#,
    )
    .unwrap();
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed move_para must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let after = hwp5::read_document(&output).unwrap().document;
    let paras = &after.sections[0].paragraphs;
    assert_eq!(paras.len(), 5, "move never changes the paragraph count");

    let first_after = paras
        .iter()
        .find(|p| p.header.instance_id == first_id)
        .expect("the FIRST occurrence must survive with its own instance_id");
    assert_eq!(
        first_after.plain_text(),
        "같은 문단",
        "the first occurrence's text is unchanged"
    );

    let second_index = paras
        .iter()
        .position(|p| p.header.instance_id == second_id)
        .expect("the SECOND occurrence must survive with its own instance_id");
    assert_eq!(
        paras[second_index].plain_text(),
        "같은 문단",
        "the moved paragraph's text is unchanged"
    );
    assert_eq!(
        paras[second_index - 1].plain_text(),
        "첫 문단",
        "the SECOND occurrence must land right after its destination reference (첫 문단)"
    );
}

/// `move_para` to a `to.address` naming a DIFFERENT section is rejected during preflight, never
/// attempted — the rejection names both sections (D-17).
#[test]
fn move_para_cross_section_is_rejected_naming_both_sections() {
    let dir = test_dir("move-cross-section");
    let md = dir.join("doc.md");
    std::fs::write(&md, DRIFT_MD).unwrap();
    let single = dir.join("single.hwpx");
    new_from(&md, &single);
    let mut doc = hwpx::read_document(&single).unwrap().document;
    doc.sections.push(doc.sections[0].clone());
    let json = hwp_convert::to_json(&doc, false, true).unwrap();
    let json_path = dir.join("two-section.json");
    std::fs::write(&json_path, json).unwrap();
    let base = dir.join("base.hwpx");
    let status = hwp()
        .args(["new", "--from"])
        .arg(&json_path)
        .arg("-o")
        .arg(&base)
        .status()
        .unwrap();
    assert!(status.success(), "two-section fixture build must succeed");

    let output = dir.join("out.hwpx");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"move_para","address":{"at":{"section":0,"paragraph":1}},"to":{"address":{"at":{"section":1,"paragraph":1}},"position":"after"}}]"#,
    )
    .unwrap();
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "a cross-section move_para must be rejected"
    );
    assert!(!output.exists(), "no output file may be written");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains('0') && stderr.contains('1'),
        "the rejection must name both sections (0 and 1): {stderr}"
    );
}

/// The nested-list drift proof (07-03 Task 3 action 4): the per-list offset tracker's key
/// derivation for a NESTED list (a table cell's paragraph list) is a distinct code path from a
/// top-level list's — a flat-only test suite cannot exercise it. Built in two invocations because
/// nested lists cannot get more than one paragraph directly from markdown import: the first grows
/// one cell's paragraph list to three (pattern-form inserts, so the setup itself does not depend
/// on address-driven drift tracking), the second runs the same two-op drift shape as the flat
/// tests, addressing the cell's paragraphs by `id` (the `at` form is top-level only, D-05).
#[test]
fn nested_list_drift_targets_the_original_index() {
    let dir = test_dir("nested-drift");
    let md = dir.join("doc.md");
    std::fs::write(&md, "# T\n\n| 가 | 나 |\n|---|---|\n| 셀시작 | 2 |\n").unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    // Invocation 1: grow cell (1,0)'s paragraph list from 1 to 3, pattern-form (unaddressed).
    let grown = dir.join("grown.hwpx");
    let setup_ops = dir.join("setup-ops.json");
    std::fs::write(
        &setup_ops,
        r#"[
          {"op":"insert_para","anchor":"셀시작","text":"셀중간","before":false},
          {"op":"insert_para","anchor":"셀중간","text":"셀끝","before":false}
        ]"#,
    )
    .unwrap();
    let setup_run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&grown)
        .arg("--ops")
        .arg(&setup_ops)
        .output()
        .unwrap();
    assert!(
        setup_run.status.success(),
        "nested-list setup must succeed: {}",
        String::from_utf8_lossy(&setup_run.stderr)
    );

    let grown_doc = hwpx::read_document(&grown).unwrap().document;
    let cell_prefix = cell_path_prefix(&grown_doc, 1, 0);
    let cell = grown_doc.sections[0]
        .paragraphs
        .iter()
        .flat_map(|p| &p.controls)
        .find_map(|c| match c {
            Control::Table(t) => t.cells.iter().find(|c| c.row == 1 && c.col == 0),
            _ => None,
        })
        .expect("cell (1,0) must exist");
    assert_eq!(
        cell.paragraphs
            .iter()
            .map(|p| p.plain_text())
            .collect::<Vec<_>>(),
        vec!["셀시작", "셀중간", "셀끝"],
        "setup must grow the cell to exactly these three paragraphs in order"
    );

    let mut path0 = cell_prefix.clone();
    path0.push(0);
    let id0 = hwp_convert::paragraph_id(
        &hwp_convert::SegmentPath {
            section: 0,
            indices: path0,
        },
        &cell.paragraphs[0],
    );
    let mut path2 = cell_prefix.clone();
    path2.push(2);
    let id2 = hwp_convert::paragraph_id(
        &hwp_convert::SegmentPath {
            section: 0,
            indices: path2,
        },
        &cell.paragraphs[2],
    );

    // Invocation 2: the actual drift proof — delete local index 0, restyle local index 2 (both
    // addressed by id against this invocation's OWN original document, the grown one).
    let output = dir.join("out.hwpx");
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        format!(
            r#"[
              {{"op":"delete_para","address":{{"id":"{id0}"}}}},
              {{"op":"set_para","address":{{"id":"{id2}"}},"align":"right"}}
            ]"#
        ),
    )
    .unwrap();
    let run = hwp()
        .arg("edit")
        .arg(&grown)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "nested-list drift batch must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let after = hwpx::read_document(&output).unwrap().document;
    let after_cell = after.sections[0]
        .paragraphs
        .iter()
        .flat_map(|p| &p.controls)
        .find_map(|c| match c {
            Control::Table(t) => t.cells.iter().find(|c| c.row == 1 && c.col == 0),
            _ => None,
        })
        .expect("cell (1,0) must exist");
    assert_eq!(
        after_cell
            .paragraphs
            .iter()
            .map(|p| p.plain_text())
            .collect::<Vec<_>>(),
        vec!["셀중간", "셀끝"],
        "셀시작 (local index 0) must be gone, 셀끝 (local index 2) survives, both in order"
    );
    // Table cells default to CENTER alignment on import (confirmed empirically): the
    // set_para call below uses "right" specifically so the restyled paragraph is observably
    // different from that shared default, discriminating "was restyled" from "was already
    // this way".
    let ending = &after_cell.paragraphs[1];
    assert_eq!(ending.plain_text(), "셀끝");
    let ps = &after.header.para_shapes[ending.para_shape.0 as usize];
    assert_eq!(
        ps.alignment(),
        2,
        "set_para must have restyled 셀끝 (ORIGINAL local index 2), not 셀중간 (which slid into local index 1)"
    );
    let middle = &after_cell.paragraphs[0];
    let middle_ps = &after.header.para_shapes[middle.para_shape.0 as usize];
    assert_eq!(
        middle_ps.alignment(),
        3,
        "셀중간 (which slid into local index 1) must keep the cell default (center), not be restyled"
    );
}

/// The path to the cell `(row, col)` of the FIRST table in the document's top-level list, as a
/// `SegmentPath` prefix (`[top_para_idx, ctrl_idx, cell_idx]`) — everything above the cell's own
/// paragraph-list index. Mirrors how a real caller would resolve a nested address: locate the
/// structural position once, then address individual paragraphs inside it by id.
fn cell_path_prefix(doc: &hwp_model::Document, row: u16, col: u16) -> Vec<usize> {
    for (top_idx, para) in doc.sections[0].paragraphs.iter().enumerate() {
        for (ctrl_idx, ctrl) in para.controls.iter().enumerate() {
            if let Control::Table(table) = ctrl
                && let Some(cell_idx) = table
                    .cells
                    .iter()
                    .position(|c| c.row == row && c.col == col)
            {
                return vec![top_idx, ctrl_idx, cell_idx];
            }
        }
    }
    panic!("cell ({row},{col}) not found");
}

// ── Phase 7 plan 07-04: list indent/outdent (EDT-05) ────────────────────────────

/// A numbered list of four level-1 items (0-based paragraphs 1..=4); items 2 and 4 carry
/// identical text "같은 항목" for the anchor-collision proof. Paragraph 5 is a plain,
/// non-list paragraph for the rejection case. Paragraph 0 is the "# T" heading — head_type 0,
/// not a list item, but its plain-text render embeds a "1. " outline prefix (an unrelated
/// heading-numbering feature), so paragraph 5 is the unambiguous non-list target instead.
const INDENT_OUTDENT_MD: &str =
    "# T\n\n1. 첫 항목\n\n2. 같은 항목\n\n3. 중간 항목\n\n4. 같은 항목\n\n일반 문단\n";

/// `indent_para` on a numbered paragraph at level 2 produces level 3, and a following
/// `outdent_para` brings it back to level 2 (behavior spec) — with no other attr1 bit disturbed
/// along the way.
#[test]
fn indent_then_outdent_basic_level_shift() {
    let dir = test_dir("indent-basic");
    let md = dir.join("doc.md");
    std::fs::write(&md, INDENT_OUTDENT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let before_doc = hwpx::read_document(&base).unwrap().document;
    let before_ps = before_doc.header.para_shapes
        [before_doc.sections[0].paragraphs[1].para_shape.0 as usize]
        .clone();
    assert_eq!(before_ps.head_level(), 1);

    // Two indents (1 -> 2 -> 3), then one outdent (3 -> 2), in one batch.
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[
          {"op":"indent_para","address":{"at":{"section":0,"paragraph":1}}},
          {"op":"indent_para","address":{"at":{"section":0,"paragraph":1}}},
          {"op":"outdent_para","address":{"at":{"section":0,"paragraph":1}}}
        ]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "indent/outdent batch must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let after = hwpx::read_document(&output).unwrap().document;
    let after_ps = &after.header.para_shapes[after.sections[0].paragraphs[1].para_shape.0 as usize];
    assert_eq!(after_ps.head_level(), 2, "1 -> 2 -> 3 -> 2");
    assert_eq!(
        after_ps.head_type(),
        before_ps.head_type(),
        "head_type must not change"
    );
    let other_bits_before = before_ps.attr1 & !(0x7 << 25);
    let other_bits_after = after_ps.attr1 & !(0x7 << 25);
    assert_eq!(
        other_bits_after, other_bits_before,
        "no other attr1 bit may change"
    );
}

/// Six successive indents from level 1 reach level 7; the seventh must fail the WHOLE batch
/// (A3: fail loudly rather than clamp) with no output file.
#[test]
fn indent_para_at_top_of_range_fails_no_output() {
    let dir = test_dir("indent-boundary");
    let md = dir.join("doc.md");
    std::fs::write(&md, INDENT_OUTDENT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let one_indent = r#"{"op":"indent_para","address":{"at":{"section":0,"paragraph":1}}}"#;
    let ops_list = format!("[{}]", [one_indent; 7].join(","));
    let ops = dir.join("ops.json");
    std::fs::write(&ops, &ops_list).unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "the 7th indent (level 7 -> 8) must fail"
    );
    assert!(
        !output.exists(),
        "no output file may be created when a boundary op fails"
    );
}

/// `outdent_para` at level 1 fails the same way as indent at level 7 — no output file.
#[test]
fn outdent_para_at_bottom_of_range_fails_no_output() {
    let dir = test_dir("outdent-boundary");
    let md = dir.join("doc.md");
    std::fs::write(&md, INDENT_OUTDENT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"outdent_para","address":{"at":{"section":0,"paragraph":1}}}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(!run.status.success(), "outdent at level 1 must fail");
    assert!(
        !output.exists(),
        "no output file may be created when a boundary op fails"
    );
}

/// Either op on a paragraph whose `head_type` is neither numbered nor bullet is rejected —
/// never inventing a list — with no output file.
#[test]
fn indent_para_on_non_list_paragraph_rejected_no_output() {
    let dir = test_dir("indent-non-list");
    let md = dir.join("doc.md");
    std::fs::write(&md, INDENT_OUTDENT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    // Paragraph 5 = "일반 문단", not a list item.
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"indent_para","address":{"at":{"section":0,"paragraph":5}}}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "indent on a non-list paragraph must be rejected"
    );
    assert!(!output.exists());
}

/// An addressed indent on the SECOND of two identical-text list items changes only that one;
/// the first item's `ParaShapeId` and `head_level` are unchanged (EDT-05 success criterion 1).
#[test]
fn addressed_indent_para_hits_only_the_named_duplicate() {
    let dir = test_dir("indent-duplicate");
    let md = dir.join("doc.md");
    std::fs::write(&md, INDENT_OUTDENT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let before_doc = hwpx::read_document(&base).unwrap().document;
    let first_before = before_doc.sections[0].paragraphs[2].clone();
    assert_eq!(first_before.plain_text(), "같은 항목");
    assert_eq!(
        before_doc.sections[0].paragraphs[4].plain_text(),
        "같은 항목"
    );

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"indent_para","address":{"at":{"section":0,"paragraph":4}}}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "addressed indent must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let after = hwpx::read_document(&output).unwrap().document;
    let first_after = &after.sections[0].paragraphs[2];
    assert_eq!(
        first_after.para_shape, first_before.para_shape,
        "the first (untouched) occurrence's ParaShapeId must be unchanged"
    );
    let first_after_ps = &after.header.para_shapes[first_after.para_shape.0 as usize];
    assert_eq!(
        first_after_ps.head_level(),
        1,
        "the first occurrence's level must be unchanged"
    );

    let second_after = &after.sections[0].paragraphs[4];
    let second_after_ps = &after.header.para_shapes[second_after.para_shape.0 as usize];
    assert_eq!(
        second_after_ps.head_level(),
        2,
        "the addressed (second) occurrence must be indented"
    );
}

/// A repeated indent/outdent pair on the same paragraph reuses the same two `ParaShape` entries
/// (level 1, level 2) through `find_or_insert_para` — the table grows by exactly one entry
/// across four ops, not four.
#[test]
fn repeated_indent_outdent_pair_reuses_para_shapes() {
    let dir = test_dir("indent-repeat");
    let md = dir.join("doc.md");
    std::fs::write(&md, INDENT_OUTDENT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let before_doc = hwpx::read_document(&base).unwrap().document;
    let before_len = before_doc.header.para_shapes.len();

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[
          {"op":"indent_para","address":{"at":{"section":0,"paragraph":1}}},
          {"op":"outdent_para","address":{"at":{"section":0,"paragraph":1}}},
          {"op":"indent_para","address":{"at":{"section":0,"paragraph":1}}},
          {"op":"outdent_para","address":{"at":{"section":0,"paragraph":1}}}
        ]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "repeated indent/outdent must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let after = hwpx::read_document(&output).unwrap().document;
    assert_eq!(
        after.header.para_shapes.len(),
        before_len + 1,
        "only ONE new ParaShape (level 2) should be added across four ops"
    );
    let ps = &after.header.para_shapes[after.sections[0].paragraphs[1].para_shape.0 as usize];
    assert_eq!(
        ps.head_level(),
        1,
        "indent-outdent-indent-outdent must round-trip to level 1"
    );
}

/// Neither op ever reads or writes a numbering/bullet definition — `header.numberings` stays
/// byte-identical before and after.
#[test]
fn indent_outdent_never_touches_numbering_definitions() {
    let dir = test_dir("indent-numbering-untouched");
    let md = dir.join("doc.md");
    std::fs::write(&md, INDENT_OUTDENT_MD).unwrap();
    let base = dir.join("base.hwpx");
    new_from(&md, &base);

    let before_doc = hwpx::read_document(&base).unwrap().document;
    let before_numberings = before_doc.header.numberings.clone();

    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"indent_para","address":{"at":{"section":0,"paragraph":1}}}]"#,
    )
    .unwrap();
    let output = dir.join("out.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "indent must succeed: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let after = hwpx::read_document(&output).unwrap().document;
    assert_eq!(
        after.header.numberings, before_numberings,
        "no numbering definition may be created or edited"
    );
}
