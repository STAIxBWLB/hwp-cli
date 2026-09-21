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
