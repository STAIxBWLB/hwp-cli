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

const KIND_COVERAGE_MD: &str = "# T\n\nintro para\n\n| 항목 | 수량 |\n|---|---|\n| 가 | 1 |\n";

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

/// All 30 typed edit kinds through one flat `edit --ops` run (plan 06-02 W2-T1b).
/// The kind-coverage fixture chains the paragraph-anchored kinds off each other,
/// exercises the table kinds on the inserted 3x2 table, and deletes what it created
/// (clone table, image, field, bookmark, doomed para). The run must exit success:
/// every kind that fails to apply either aborts or pushes an unapplied entry, and a
/// non-empty unapplied list exits nonzero before an output is published — that is
/// the all-38-applied proxy for the kinds with no text-observable effect (set_meta,
/// set_page, set_format/set_align/set_para, row/col surgery, set_cell_para,
/// style_tables, delete_field, delete_bookmark). The text- and model-level
/// assertions mirror the verified binary drive: replace payloads, field/hyperlink
/// display text, the 라벨값/수정값 row on the first table, the untouched input form
/// table as the second table, the clone removed, the doomed para gone, the inserted
/// picture deleted from its anchor paragraph, and the seal left as a floating
/// Picture with both image parts shipped in the package.
#[test]
fn kind_coverage_all_30() {
    // Future renames of a typed edit kind fail loudly here.
    const ALL_KINDS: [&str; 30] = [
        "set_meta",
        "set_page",
        "insert_para",
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
        ops.len() >= 30,
        "fixture must carry at least 30 ops, got {}",
        ops.len()
    );
    let mut seen = ops;
    seen.sort_unstable();
    seen.dedup();
    let mut expected = ALL_KINDS.to_vec();
    expected.sort_unstable();
    assert_eq!(
        seen, expected,
        "fixture op set must equal the 30 typed edit kinds"
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
        actual, "88958d4e3c64b32ec838ad51ae061542b57b5170c2b2300f0a78c7d84863ec13",
        "edit-ops-v1.schema.json changed — update the pinned contract hash consciously"
    );
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
