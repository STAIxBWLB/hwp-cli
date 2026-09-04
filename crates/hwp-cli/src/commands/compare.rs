//! `hwp compare` — report text and structural differences between two documents,
//! leaving both untouched (GM-8, FLOW-03).
//!
//! Not `diff`, which renders a page and compares it pixel-by-pixel against a
//! Hancom reference PNG (`commands/diff.rs`, `hwp-diff-report-v1`). `compare` never
//! renders and never writes: both inputs are loaded read-only, handed to
//! `hwp_convert::document_compare::compare_documents`, and the report goes to
//! standard output only. That absence of any output path is the structural
//! guarantee behind success criterion 3 — nothing on this file's path can call a
//! document writer or a staged-write helper (see the acceptance gate in
//! `03-04-PLAN.md`).
//!
//! Exit codes follow the diff(1) convention (D-13): 0 identical, 1 differences
//! found, 2 the run itself failed. Differences-found is a **success** outcome —
//! `run` returns a typed [`CompareOutcome`], never an `Err`, for that path. An
//! `Err` return from a `Cmd` arm becomes process exit code 1 through `main.rs`'s
//! default termination behavior, which is the exact code reserved for
//! "differences found"; the two would be indistinguishable to a CI script if this
//! command used the ordinary `?`-propagated-`Err` shape every other command uses.
//! The `Cmd::Compare` dispatch arm in `main.rs` (not this file) is the one place
//! that intercepts this and calls the exit primitive explicitly for all three
//! codes. This deliberately diverges from `commands/lint.rs`'s exit-code
//! convention (advisory, always 0 unless `--strict` finds an error).

use std::path::Path;

use hwp_cli::cli::{CompareFormat, PasswordArgs};
use hwp_convert::document_compare::{
    CharOp, CharRun, DocumentDiff, ParagraphLocation, ParagraphOp,
};

use crate::commands::cat::{LoadOptions, load_document_with_options, resolve_password_args};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOutcome {
    Identical,
    Different,
}

/// Loads both inputs read-only with one resolved password, compares them, and
/// renders the requested report. Never returns `Err` for "differences found" —
/// see the module doc comment.
pub fn run(
    a: &Path,
    b: &Path,
    format: CompareFormat,
    password: PasswordArgs,
) -> anyhow::Result<CompareOutcome> {
    let resolved_password = resolve_password_args(password, a)?;
    let options = LoadOptions {
        password: resolved_password.as_ref(),
    };

    let doc_a = load_document_with_options(a, &options).map_err(anyhow::Error::new)?;
    let doc_b = load_document_with_options(b, &options).map_err(anyhow::Error::new)?;

    let diff = hwp_convert::document_compare::compare_documents(&doc_a, &doc_b)
        .map_err(|error| anyhow::anyhow!("비교 실패: {error}"))?;

    match format {
        CompareFormat::Text => print_text_report(a, b, &diff),
        CompareFormat::Json => {
            let json = compare_report_json(a, b, &diff);
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
    }

    Ok(if diff.identical {
        CompareOutcome::Identical
    } else {
        CompareOutcome::Different
    })
}

/// Loads both inputs read-only with an already-resolved password and returns
/// the typed `hwp-compare-report-v1` body. Split out of [`run`] so the MCP
/// `hwp_compare` tool never reaches `run`'s `println!` reporting — over stdio
/// that stream carries JSON-RPC and a leaked report would corrupt the session.
pub(crate) fn execute(
    a: &Path,
    b: &Path,
    options: &LoadOptions<'_>,
) -> anyhow::Result<serde_json::Value> {
    let doc_a = load_document_with_options(a, options).map_err(anyhow::Error::new)?;
    let doc_b = load_document_with_options(b, options).map_err(anyhow::Error::new)?;
    let diff = hwp_convert::document_compare::compare_documents(&doc_a, &doc_b)
        .map_err(|error| anyhow::anyhow!("비교 실패: {error}"))?;
    Ok(compare_report_json(a, b, &diff))
}

/// The Korean location label for one paragraph. Body paragraphs render as an
/// empty string — they are addressed by their bracketed index alone, which is
/// what `hwp compare` printed before locations existed.
fn location_str(location: &ParagraphLocation) -> String {
    match location {
        ParagraphLocation::Body { .. } => String::new(),
        ParagraphLocation::Cell {
            table,
            row,
            col,
            index,
            ..
        } => format!("표 {table} 셀 ({row},{col}) 문단 {index}"),
        ParagraphLocation::Caption { table, index, .. } => format!("표 {table} 캡션 문단 {index}"),
        ParagraphLocation::Nested { ctrl_id, index, .. } => {
            format!("개체 {} 문단 {index}", ctrl_id_str(ctrl_id))
        }
    }
}

/// One report line: the operation sign, the bracketed flattened index, the
/// location when there is one, then the paragraph's excerpt.
fn entry_line(sign: char, index: usize, location: &ParagraphLocation, text: &str) -> String {
    let location = location_str(location);
    if location.is_empty() {
        format!("{sign} [{index}] {text}")
    } else {
        format!("{sign} [{index}] {location}: {text}")
    }
}

/// Unified-diff-style text report: a header naming both paths, one line per
/// non-equal paragraph operation, then a short structural summary.
///
/// Every printed value comes off the [`ParagraphEntry`], never from a second
/// walk of the source documents: the report's old flat top-level paragraph
/// list and the engine's deep walk indexed different sequences, so past the
/// first table the lookup either missed or named the wrong paragraph (#223).
fn print_text_report(a: &Path, b: &Path, diff: &DocumentDiff) {
    println!("--- {}", a.display());
    println!("+++ {}", b.display());

    for entry in &diff.paragraphs {
        match entry.op {
            ParagraphOp::Equal => {}
            ParagraphOp::Insert => {
                let index = entry.b_index.unwrap_or_default();
                println!("{}", entry_line('+', index, &entry.location, &entry.text));
            }
            ParagraphOp::Delete => {
                let index = entry.a_index.unwrap_or_default();
                println!("{}", entry_line('-', index, &entry.location, &entry.text));
            }
            ParagraphOp::Replace => {
                let a_index = entry.a_index.unwrap_or_default();
                let b_index = entry.b_index.unwrap_or_default();
                println!("{}", entry_line('-', a_index, &entry.location, &entry.text));
                let b_location = entry.b_location.as_ref().unwrap_or(&entry.location);
                let b_text = entry.b_text.as_deref().unwrap_or_default();
                println!("{}", entry_line('+', b_index, b_location, b_text));
            }
        }
    }

    if diff.identical {
        println!("문서가 동일합니다.");
        return;
    }

    let changed_controls = diff
        .structure
        .controls
        .values()
        .filter(|(a, b)| a != b)
        .count();
    // The existing fields keep their order; the D-18 additions are appended,
    // and the table count only when the two sides actually differ. The cell
    // paragraph figure is each side's own count, so it reads like every other
    // a→b pair on the line.
    let (a_cells, b_cells) = diff.structure.cell_paragraphs;
    let cell_delta = b_cells as isize - a_cells as isize;
    let mut summary = format!(
        "구조: 구역 {}→{}, 문단 {}→{}, 표 변경 {}건, 컨트롤 종류 변경 {}건, 표 내부 문단 {a_cells}→{b_cells} ({cell_delta:+})",
        diff.structure.sections.0,
        diff.structure.sections.1,
        diff.structure.paragraphs.0,
        diff.structure.paragraphs.1,
        diff.structure.tables.len(),
        changed_controls,
    );
    let (a_tables, b_tables) = diff.structure.table_counts;
    if a_tables != b_tables {
        summary.push_str(&format!(", 표 개수 {a_tables}→{b_tables}"));
    }
    println!("{summary}");
}

/// A `ctrl_id` as its four printable characters (e.g. `gso `).
fn ctrl_id_str(ctrl_id: &[u8; 4]) -> String {
    String::from_utf8_lossy(ctrl_id).into_owned()
}

fn paragraph_op_str(op: ParagraphOp) -> &'static str {
    match op {
        ParagraphOp::Equal => "equal",
        ParagraphOp::Insert => "insert",
        ParagraphOp::Delete => "delete",
        ParagraphOp::Replace => "replace",
    }
}

fn char_op_str(op: CharOp) -> &'static str {
    match op {
        CharOp::Equal => "equal",
        CharOp::Insert => "insert",
        CharOp::Delete => "delete",
    }
}

/// Serializes a [`ParagraphLocation`] by hand into a `kind`-discriminated
/// object carrying that variant's numbers. Hand-rolled rather than derived so
/// the engine types stay free of a serde dependency and the emitted shape is
/// explicit and reviewable.
fn location_json(location: &ParagraphLocation) -> serde_json::Value {
    match location {
        ParagraphLocation::Body { section, index } => serde_json::json!({
            "kind": "body",
            "section": section,
            "index": index,
        }),
        ParagraphLocation::Cell {
            section,
            table,
            row,
            col,
            index,
        } => serde_json::json!({
            "kind": "cell",
            "section": section,
            "table": table,
            "row": row,
            "col": col,
            "index": index,
        }),
        ParagraphLocation::Caption {
            section,
            table,
            index,
        } => serde_json::json!({
            "kind": "caption",
            "section": section,
            "table": table,
            "index": index,
        }),
        ParagraphLocation::Nested {
            section,
            ctrl_id,
            index,
        } => serde_json::json!({
            "kind": "nested",
            "section": section,
            "ctrl_id": ctrl_id_str(ctrl_id),
            "index": index,
        }),
    }
}

fn char_run_json(run: &CharRun) -> serde_json::Value {
    serde_json::json!({
        "op": char_op_str(run.op),
        "a_wchar": run.a_wchar,
        "b_wchar": run.b_wchar,
        "len_wchar": run.len_wchar,
    })
}

/// Builds the `hwp-compare-report-v1` payload: one flat `serde_json::json!`
/// value, matching `commands/diff.rs`'s `diff_report_json` shape rather than a
/// new `#[derive(Serialize)]` struct (D-10).
pub(crate) fn compare_report_json(a: &Path, b: &Path, diff: &DocumentDiff) -> serde_json::Value {
    let paragraphs: Vec<serde_json::Value> = diff
        .paragraphs
        .iter()
        .map(|entry| {
            // `text` and `location` are additive (D-17): every key below was
            // present before them and keeps its meaning, and the contract
            // string is unchanged, so a consumer that ignores unknown keys
            // needs no change.
            let mut value = serde_json::json!({
                "op": paragraph_op_str(entry.op),
                "a_index": entry.a_index,
                "b_index": entry.b_index,
                "text": entry.text,
                "location": location_json(&entry.location),
            });
            if let Some(b_text) = &entry.b_text {
                value["b_text"] = serde_json::json!(b_text);
            }
            if let Some(b_location) = &entry.b_location {
                value["b_location"] = location_json(b_location);
            }
            if let Some(chars) = &entry.chars {
                value["chars"] =
                    serde_json::json!(chars.iter().map(char_run_json).collect::<Vec<_>>());
            }
            value
        })
        .collect();

    let controls: Vec<serde_json::Value> = diff
        .structure
        .controls
        .iter()
        .map(|(kind, (a_count, b_count))| {
            serde_json::json!({
                "kind": String::from_utf8_lossy(kind),
                "a": a_count,
                "b": b_count,
            })
        })
        .collect();

    let tables: Vec<serde_json::Value> = diff
        .structure
        .tables
        .iter()
        .map(|delta| {
            serde_json::json!({
                "index": delta.index,
                "rows": {"a": delta.rows.0, "b": delta.rows.1},
                "cols": {"a": delta.cols.0, "b": delta.cols.1},
            })
        })
        .collect();

    serde_json::json!({
        "contract": "hwp-compare-report-v1",
        "a": a.to_string_lossy(),
        "b": b.to_string_lossy(),
        "identical": diff.identical,
        "paragraphs": paragraphs,
        "structure": {
            "sections": {"a": diff.structure.sections.0, "b": diff.structure.sections.1},
            "paragraphs": {"a": diff.structure.paragraphs.0, "b": diff.structure.paragraphs.1},
            "controls": controls,
            "tables": tables,
            // Per-side counts, like every other pair in this block: how many
            // paragraphs live in a table cell on each side.
            "cell_paragraphs": {
                "a": diff.structure.cell_paragraphs.0,
                "b": diff.structure.cell_paragraphs.1,
            },
            "table_count": {"a": diff.structure.table_counts.0, "b": diff.structure.table_counts.1},
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest as _, Sha256};

    fn diff_a_vs_a() -> (std::path::PathBuf, std::path::PathBuf, DocumentDiff) {
        let doc = hwp_convert::from_markdown::from_markdown("첫 문단\n\n둘째 문단\n");
        let diff = hwp_convert::document_compare::compare_documents(&doc, &doc).unwrap();
        (
            std::path::PathBuf::from("a.hwpx"),
            std::path::PathBuf::from("b.hwpx"),
            diff,
        )
    }

    /// A pair whose only difference is one paragraph appended inside a table
    /// cell — the #223 shape.
    fn diff_with_cell_insertion() -> (std::path::PathBuf, std::path::PathBuf, DocumentDiff) {
        use hwp_model::{Control, HwpChar};

        let markdown = "본문 문단\n\n| 항목 | 내용 |\n| - | - |\n| 하나 | 둘 |\n";
        let doc_a = hwp_convert::from_markdown::from_markdown(markdown);
        let mut doc_b = doc_a.clone();
        let cell = doc_b
            .sections
            .iter_mut()
            .flat_map(|s| s.paragraphs.iter_mut())
            .flat_map(|p| p.controls.iter_mut())
            .find_map(|control| match control {
                Control::Table(table) => table.cells.iter_mut().find(|c| c.row == 1 && c.col == 1),
                _ => None,
            })
            .expect("셀 (1,1)이 있어야 함");
        let mut added = cell.paragraphs[0].clone();
        added.chars = "○ 둘째 항목 BBB".chars().map(HwpChar::Text).collect();
        cell.paragraphs.push(added);

        let diff = hwp_convert::document_compare::compare_documents(&doc_a, &doc_b).unwrap();
        (
            std::path::PathBuf::from("a.hwpx"),
            std::path::PathBuf::from("b.hwpx"),
            diff,
        )
    }

    #[test]
    fn json_report_contract() {
        let (a, b, diff) = diff_a_vs_a();
        let v = compare_report_json(&a, &b, &diff);
        assert_eq!(v["contract"], "hwp-compare-report-v1");
        assert_eq!(v["identical"], true);
        assert!(v["paragraphs"].is_array());
        assert!(v["structure"].is_object());
        assert!(v["structure"]["sections"].is_object());
        assert!(v["structure"]["controls"].is_array());
        assert!(v["structure"]["tables"].is_array());

        // The `text` and `location` keys are additive: every key the report
        // carried before them is still present with the same meaning, on
        // every entry, so a consumer that ignores unknown keys keeps working.
        for entry in v["paragraphs"].as_array().unwrap() {
            assert!(entry["op"].is_string());
            assert!(entry.get("a_index").is_some());
            assert!(entry.get("b_index").is_some());
            assert!(entry["text"].is_string());
            assert!(entry["location"]["kind"].is_string());
        }
    }

    /// #223: a paragraph inside a table cell must be addressable from the
    /// JSON report, with the same 0-based table number `--set-cell` uses.
    #[test]
    fn json_report_locates_a_cell_paragraph() {
        let (a, b, diff) = diff_with_cell_insertion();
        let v = compare_report_json(&a, &b, &diff);
        assert_eq!(v["contract"], "hwp-compare-report-v1");

        let inserted = v["paragraphs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["op"] == "insert")
            .expect("추가된 셀 문단이 있어야 함");
        assert_eq!(inserted["text"], "○ 둘째 항목 BBB");
        assert_eq!(inserted["location"]["kind"], "cell");
        assert_eq!(inserted["location"]["section"], 0);
        assert_eq!(inserted["location"]["table"], 0);
        assert_eq!(inserted["location"]["row"], 1);
        assert_eq!(inserted["location"]["col"], 1);
        assert_eq!(inserted["location"]["index"], 1);
    }

    /// Both inputs go in as `.json` IR (no hwp5/hwpx container writer touches
    /// this file — `load_document_with_options` reads `.json` as an IR
    /// pass-through), so the SHA-256 hash proof exercises the real `run()`
    /// path without this module depending on any document writer.
    #[test]
    fn run_leaves_both_inputs_byte_identical() {
        let dir = std::env::temp_dir().join(format!(
            "hwp-cli-compare-hash-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let a_path = dir.join("a.json");
        let b_path = dir.join("b.json");

        let doc_a = hwp_convert::from_markdown::from_markdown("첫 문단\n");
        let doc_b = hwp_convert::from_markdown::from_markdown("첫 문단\n\n둘째 문단\n");
        std::fs::write(&a_path, hwp_convert::to_json(&doc_a, true, false).unwrap()).unwrap();
        std::fs::write(&b_path, hwp_convert::to_json(&doc_b, true, false).unwrap()).unwrap();

        let hash = |p: &Path| -> [u8; 32] { Sha256::digest(std::fs::read(p).unwrap()).into() };
        let a_before = hash(&a_path);
        let b_before = hash(&b_path);

        let outcome = run(
            &a_path,
            &b_path,
            CompareFormat::Json,
            PasswordArgs::default(),
        )
        .unwrap();
        assert_eq!(outcome, CompareOutcome::Different);
        assert_eq!(hash(&a_path), a_before);
        assert_eq!(hash(&b_path), b_before);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
