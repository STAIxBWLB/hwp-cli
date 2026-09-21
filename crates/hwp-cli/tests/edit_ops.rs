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
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/edit-ops/tracer.json");
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
        .map(|entry| entry["op"].as_str().expect("every op must carry a string tag"))
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
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../schemas/edit-ops-v1.schema.json"
    ))
    .unwrap();
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
        texts.iter().any(|text| text.contains("field anchor para변경값")),
        "create_field + set_field must leave the field display text, got {texts:?}"
    );
    assert!(
        texts.iter().any(|text| text.contains("hyperlink anchor para예시 링크")),
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
