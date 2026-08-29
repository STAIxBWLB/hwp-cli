//! `hwp edit --style-tables` twice-applied byte-stability gate (D-08).
//!
//! `style::style_table` (`crates/hwp-convert/src/style.rs`) has NO marker and NO "already
//! styled" probe by design — idempotence is a purity property of the function, not something
//! detected and short-circuited. That means if a future styling rule ever starts reading state
//! outside the table's own content, nothing else in the codebase will catch it: this test,
//! comparing the raw bytes of a file styled once against the same file styled twice, is the
//! ONLY guard. Read that as an instruction to the next person touching `style_table` or
//! `style_tables`: if you are about to delete or weaken this file, you are removing D-08's only
//! detector, not a redundant check.
//!
//! Deliberately does NOT assert on page counts or glyph metrics (CI render fonts differ from
//! local ones) and deliberately does NOT skip when a fixture is missing — every input here is
//! built from committed markdown, so there is no fixture to be missing.

use std::path::{Path, PathBuf};
use std::process::Command;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn test_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "hwp-cli-style-tables-idempotence-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn write_md(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

/// Byte-for-byte comparison with a diagnosable failure: on a mismatch, report both lengths and
/// the offset of the first differing byte, so a future regression here is legible rather than
/// just red.
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

fn new_from(md: &Path, out: &Path, extra_args: &[&str]) {
    let mut cmd = hwp();
    cmd.args(["new", "--from"]).arg(md).arg("-o").arg(out);
    cmd.args(extra_args);
    let status = cmd.status().unwrap();
    assert!(status.success(), "hwp new --from {md:?} -o {out:?} failed");
}

fn style_tables(input: &Path, output: &Path) {
    let r = hwp()
        .arg("edit")
        .arg(input)
        .arg("-o")
        .arg(output)
        .args(["--style-tables", "official", "--verify"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "hwp edit {input:?} -o {output:?} --style-tables official failed: {}",
        String::from_utf8_lossy(&r.stderr)
    );
}

fn validate_ok(path: &Path) {
    let status = hwp().arg("validate").arg(path).status().unwrap();
    assert!(status.success(), "hwp validate {path:?} failed");
}

const GFM_TABLE_MD: &str = "\
| 가나다라마바사아자차카타파하 | 값 |
|---|---|
| 1 | 2 |
";

/// Nested via raw HTML (GFM tables cannot nest): the from_html import path does not call
/// `style_table` at all, so both the outer and the inner table start genuinely unstyled — a
/// real (not already-idempotent) styling target for behavior bullet 4, and simultaneously the
/// dedicated nested-table case for bullet 3.
const NESTED_HTML_MD: &str = "\
<table>
<tr><th>Outer1</th><th>Outer2</th></tr>
<tr><td>

<table>
<tr><th>InnerA</th><th>InnerB</th></tr>
<tr><td>i1</td><td>i2</td></tr>
</table>

</td><td>x</td></tr>
</table>
";

/// Behavior bullet 1/2: write once, style into a second file, style that into a third file — the
/// second and third files are byte-identical. Runs for both the `.hwpx` and the `.hwp` writer
/// (the two writers are independent; a earlier draft of this feature was byte-stable on one and
/// silently duplicated a `BorderFill`/`ParaShape` entry on hwp5's patch-rewrite path on every
/// reapplication — see `style.rs`'s `find_or_insert_border_fill`/`find_or_insert_para_ignoring_tail`
/// doc comments for why the comparison must ignore a hwp5 round trip's materialized `tail`).
#[test]
fn style_tables_byte_stable_on_second_application_both_writers() {
    let dir = test_dir("second-vs-third");
    let md = write_md(&dir, "table.md", GFM_TABLE_MD);

    for ext in ["hwpx", "hwp"] {
        let gen0 = dir.join(format!("gen0.{ext}"));
        new_from(&md, &gen0, &[]);

        let gen1 = dir.join(format!("gen1.{ext}"));
        style_tables(&gen0, &gen1);
        let gen2 = dir.join(format!("gen2.{ext}"));
        style_tables(&gen1, &gen2);

        assert_bytes_eq(&gen1, &gen2, &format!("{ext}: second vs third file"));
        validate_ok(&gen2);

        // And once more, to prove generation 2 -> 3 is a genuine fixed point, not a one-time
        // coincidence of exactly two applications.
        let gen3 = dir.join(format!("gen3.{ext}"));
        style_tables(&gen2, &gen3);
        assert_bytes_eq(&gen2, &gen3, &format!("{ext}: third vs fourth file"));
    }
}

/// Behavior bullet 2 (explicit): a document created by `hwp new` UNDER A PRESET (so it is already
/// styled at import time, D-07 applying to every GFM table regardless of preset) is still
/// byte-stable when `--style-tables` is applied to it and then applied again — proving the
/// import path and the edit path compute the identical values, not just "no visible difference
/// this one time".
#[test]
fn style_tables_stable_on_already_preset_styled_document() {
    let dir = test_dir("preset-styled");
    let md = write_md(&dir, "table.md", GFM_TABLE_MD);

    for ext in ["hwpx", "hwp"] {
        let gen0 = dir.join(format!("gen0.{ext}"));
        new_from(&md, &gen0, &["--preset", "official"]);

        let gen1 = dir.join(format!("gen1.{ext}"));
        style_tables(&gen0, &gen1);
        let gen2 = dir.join(format!("gen2.{ext}"));
        style_tables(&gen1, &gen2);

        assert_bytes_eq(
            &gen1,
            &gen2,
            &format!("{ext}: preset-styled, second vs third"),
        );
        validate_ok(&gen2);
    }
}

/// Behavior bullet 3: a table nested inside a table cell has the nested table styled too, not
/// just the top-level one. `NESTED_HTML_MD` imports through `from_html`, which never calls
/// `style_table` at import time, so both the outer and the inner table are genuinely unstyled
/// going in — a real (not already-idempotent) change on the first `--style-tables` application.
#[test]
fn style_tables_recurses_into_nested_table_in_cell() {
    let dir = test_dir("nested");
    let md = write_md(&dir, "nested.md", NESTED_HTML_MD);

    for ext in ["hwpx", "hwp"] {
        let gen0 = dir.join(format!("gen0.{ext}"));
        new_from(&md, &gen0, &["--strict"]);

        let before = all_tables_with_shapes(&gen0);
        assert_eq!(
            before.len(),
            2,
            "{ext}: expected outer + nested table, got {before:?}"
        );
        let unstyled_header = before[0].header_shapes();
        assert!(
            before.iter().all(|t| t.header_shapes() == unstyled_header),
            "{ext}: both tables must start with the SAME (unstyled) header shape, got {before:?}"
        );

        let gen1 = dir.join(format!("gen1.{ext}"));
        style_tables(&gen0, &gen1);
        let after = all_tables_with_shapes(&gen1);
        assert_eq!(
            after.len(),
            2,
            "{ext}: styling must not add or drop a table, got {after:?}"
        );
        // Both the outer table's header row and the nested table's header row must now carry
        // the SAME new shape (value-deduped, D-08) — and it must differ from the pre-styling one.
        assert_eq!(
            after[0].header_shapes(),
            after[1].header_shapes(),
            "{ext}: outer and nested header rows must share the same styled shape, got {after:?}"
        );
        assert_ne!(
            after[0].header_shapes(),
            unstyled_header,
            "{ext}: the nested table's header row must have actually changed, got {after:?}"
        );

        let gen2 = dir.join(format!("gen2.{ext}"));
        style_tables(&gen1, &gen2);
        assert_bytes_eq(
            &gen1,
            &gen2,
            &format!("{ext}: nested-table case, second vs third"),
        );
        validate_ok(&gen2);
    }
}

/// A single, un-nested, genuinely-unstyled (via `from_html`, D-07 never touches it) 2-column
/// body table, so behavior bullet 4's "body table styled AND frames untouched in the same run"
/// has an actual visible change to check on the body side, not a table that was already styled
/// at import and would look unchanged either way.
const HTML_BODY_TABLE_MD: &str = "\
<table>
<tr><th>Body1</th><th>Body2</th></tr>
<tr><td>1</td><td>2</td></tr>
</table>
";

/// Behavior bullet 4 (D-11): a framed document is left alone. Generates with `--doc-head`/
/// `--doc-foot` (single-column frame tables per plan 01/02) PLUS an unstyled body table, runs
/// `--style-tables`, and asserts every frame table's cells are byte-for-byte unchanged while the
/// body table's header row visibly picks up the shade/centered shapes in the SAME run — proving
/// the predicate discriminates by column count rather than disabling the walker outright.
#[test]
fn style_tables_skips_single_column_frame_tables() {
    let dir = test_dir("framed");
    let md = write_md(&dir, "table.md", HTML_BODY_TABLE_MD);

    for ext in ["hwpx", "hwp"] {
        let framed = dir.join(format!("framed.{ext}"));
        new_from(
            &md,
            &framed,
            &[
                "--strict",
                "--preset",
                "official",
                "--doc-head",
                "기관명=제주한라대학교",
                "--doc-foot",
                "발신명의=총장",
            ],
        );

        let before = tables_with_shapes(&framed);
        let frame_tables_before: Vec<_> = before.iter().filter(|t| t.cols == 1).collect();
        assert!(
            frame_tables_before.len() >= 2,
            "{ext}: expected at least the 두문/결문 frame tables (cols=1), got {before:?}"
        );
        let body_before = before
            .iter()
            .find(|t| t.cols > 1)
            .unwrap_or_else(|| panic!("{ext}: expected a multi-column body table, got {before:?}"));

        let styled = dir.join(format!("framed_styled.{ext}"));
        let report = hwp()
            .arg("edit")
            .arg(&framed)
            .arg("-o")
            .arg(&styled)
            .args(["--style-tables", "official", "--verify"])
            .output()
            .unwrap();
        assert!(
            report.status.success(),
            "{ext}: styling a framed doc must still succeed (the body table is a real target): {}",
            String::from_utf8_lossy(&report.stderr)
        );

        let after = tables_with_shapes(&styled);
        let frame_tables_after: Vec<_> = after.iter().filter(|t| t.cols == 1).collect();
        assert_eq!(
            frame_tables_before, frame_tables_after,
            "{ext}: every frame table's cells must be byte-for-byte unchanged (D-11)"
        );
        let body_after = after.iter().find(|t| t.cols > 1).unwrap();
        assert_ne!(
            body_before.header_shapes(),
            body_after.header_shapes(),
            "{ext}: the body table's header row must visibly change (shade fill + centered \
             para shape) in the SAME run that left every frame table's cells alone"
        );

        validate_ok(&styled);
    }
}

/// `hwp validate` exits 0 on every styled output — already asserted inline above via
/// `validate_ok` after each styling pass; this test exists to name the requirement explicitly so
/// a future refactor that removes a `validate_ok` call still trips a named assertion.
#[test]
fn style_tables_output_always_validates() {
    let dir = test_dir("validate");
    let md = write_md(&dir, "table.md", GFM_TABLE_MD);
    for ext in ["hwpx", "hwp"] {
        let gen0 = dir.join(format!("gen0.{ext}"));
        new_from(&md, &gen0, &[]);
        let gen1 = dir.join(format!("gen1.{ext}"));
        style_tables(&gen0, &gen1);
        validate_ok(&gen1);
    }
}

/// `hwp cat --format json` output as parsed JSON (full document IR — not a page-count/glyph
/// projection, so this is stable across CI/local font differences per this module's contract).
fn cat_json(path: &Path) -> serde_json::Value {
    let out = hwp()
        .args(["cat", "--format", "json"])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "hwp cat --format json {path:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{path:?}: invalid JSON: {e}"))
}

/// One table's column count and every cell's `(row, col, border_fill, para_shape)` —
/// enough to tell a frame table (`cols == 1`) apart from a body table, and to compare a
/// specific table's cells before/after without depending on the exact numeric shape ids a
/// PARTICULAR document's header happens to allocate (those ids shift with how many frame/preset
/// shapes precede them, so asserting against a hardcoded id would be fragile).
#[derive(Debug, Clone, PartialEq, Eq)]
struct TableRecord {
    cols: i64,
    cells: Vec<(i64, i64, i64, i64)>,
}

impl TableRecord {
    /// `(border_fill, para_shape)` of the first row-0 cell — the header row's shape.
    fn header_shapes(&self) -> (i64, i64) {
        self.cells
            .iter()
            .find(|(row, ..)| *row == 0)
            .map(|(_, _, bf, ps)| (*bf, *ps))
            .expect("table has at least one row-0 cell")
    }
}

/// Every table's shape record, TOP-LEVEL only (not recursing into nested tables — the framed-doc
/// test only needs to tell frame tables apart from the one body table at this document's top
/// level; row/col spans don't apply to the single-paragraph frame/GFM tables this suite builds).
fn tables_with_shapes(path: &Path) -> Vec<TableRecord> {
    let doc = cat_json(path);
    let mut out = Vec::new();
    for section in doc["sections"].as_array().unwrap() {
        for p in section["paragraphs"].as_array().unwrap() {
            for c in p
                .get("controls")
                .and_then(|c| c.as_array())
                .unwrap_or(&Vec::new())
            {
                if let Some(t) = table_record(c) {
                    out.push(t);
                }
            }
        }
    }
    out
}

/// Every table's shape record, recursing into nested tables (a cell whose own paragraph list
/// contains another `Table` control) — outer tables first, then their nested tables, matching
/// document order.
fn all_tables_with_shapes(path: &Path) -> Vec<TableRecord> {
    fn walk(paras: &serde_json::Value, out: &mut Vec<TableRecord>) {
        let Some(paras) = paras.as_array() else {
            return;
        };
        for p in paras {
            for c in p
                .get("controls")
                .and_then(|c| c.as_array())
                .unwrap_or(&Vec::new())
            {
                let Some(table) = table_record(c) else {
                    continue;
                };
                let cells = c["Table"]["cells"].as_array().unwrap();
                out.push(table);
                for cell in cells {
                    walk(&cell["paragraphs"], out);
                }
            }
        }
    }
    let doc = cat_json(path);
    let mut out = Vec::new();
    for section in doc["sections"].as_array().unwrap() {
        walk(&section["paragraphs"], &mut out);
    }
    out
}

fn table_record(control: &serde_json::Value) -> Option<TableRecord> {
    let table = control.get("Table")?;
    let cols = table["cols"].as_i64().unwrap_or(-1);
    let cells = table["cells"]
        .as_array()
        .unwrap()
        .iter()
        .map(|cell| {
            (
                cell["row"].as_i64().unwrap_or(-1),
                cell["col"].as_i64().unwrap_or(-1),
                cell["border_fill"].as_i64().unwrap_or(-1),
                cell["paragraphs"][0]["para_shape"].as_i64().unwrap_or(-1),
            )
        })
        .collect();
    Some(TableRecord { cols, cells })
}

/// Regression for the two defects Codex found on #133.
///
/// A real HWPX carrying an `hp:container` (an opaque run-level object whose captured XML is the
/// ONLY lossless serialization source) is the case the earlier tests missed: they covered
/// generated documents and a table nested in a table cell, never a raw-backed generic. Two bugs
/// compounded there. `style_table` returned `true` for every table with at least one column
/// rather than only for tables it changed, so the walker's `Control::Generic` arm cleared
/// `hwpx_raw_xml` on a pure no-op re-application; and once cleared, the HWPX writer cannot
/// reconstruct the container and reports `OpaqueControlUnrepresentable`. Separately, an
/// already-styled document produced zero edits, which the publish gate could not tell apart from
/// "no table matched", so the second of two identical runs failed outright.
///
/// This test runs with `--verify` (the semantic-hash self-check). It was --verify-free while the
/// fixture tripped an unrelated HWPX writer round-trip defect (#135 — explicit no-fill border
/// fills and empty numbering templates were not written back faithfully); with that fixed, the
/// full edit/re-read comparison is asserted here too.
#[test]
fn style_tables_byte_stable_on_a_document_with_an_opaque_container() {
    let source = Path::new(
        "../../fixtures/pdf-parity/private/complex-proposal-body-v2/source/complex-proposal-body-v2.hwpx",
    );
    if !source.exists() {
        eprintln!("skip: private parity fixture absent (gitignored corpus)");
        return;
    }
    let dir = test_dir("opaque-container");
    let first = dir.join("s1.hwpx");
    let second = dir.join("s2.hwpx");
    let third = dir.join("s3.hwpx");

    // Each pass must PUBLISH. Before the fix the second one failed the publish gate outright.
    style_tables(source, &first);
    style_tables(&first, &second);
    style_tables(&second, &third);

    assert_bytes_eq(
        &first,
        &second,
        "opaque container: re-applying must change nothing",
    );
    assert_bytes_eq(&second, &third, "opaque container: and stay a fixed point");
    validate_ok(&second);
    validate_ok(&third);
}
