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
    std::fs::write(&ops, r#"[{"op":"set_cell","table":0,"row":0,"col":0,"text":"x"}]"#).unwrap();

    let modes: [(&str, Vec<&str>); 2] =
        [("default", Vec::new()), ("allow-partial", vec!["--allow-partial"])];
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
    let modes: [(&str, Vec<&str>); 2] =
        [("default", Vec::new()), ("allow-partial", vec!["--allow-partial"])];
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
    let info: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&info.stdout)).expect("hwp info --json must parse");
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
    assert_bytes_eq(&run1_out, &run2_out, "style_tables re-run must be byte-stable");

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
        actual,
        "8fb42a96f75e1473df1ee20af1f3a65315906f3e3f4c7366530cce474a27c937",
        "edit-ops-v1.schema.json changed — update the pinned contract hash consciously"
    );
}
