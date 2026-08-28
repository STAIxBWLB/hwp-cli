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
use hwp_convert::document_compare::{CharOp, CharRun, DocumentDiff, ParagraphOp};
use hwp_model::{Document, HwpChar, Paragraph};

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
        CompareFormat::Text => print_text_report(a, b, &doc_a, &doc_b, &diff),
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

/// Plain text extraction identical in shape to `commands/grep.rs`'s `para_text`
/// (this file has no reuse path across binaries, so the small helper is
/// duplicated rather than shared).
fn para_text(para: &Paragraph) -> String {
    let mut text = String::new();
    for ch in &para.chars {
        match ch {
            HwpChar::Text(c) => text.push(*c),
            HwpChar::CharCtrl(10) => text.push(' '),
            _ => {}
        }
    }
    text.trim().to_string()
}

fn flat_paragraphs(doc: &Document) -> Vec<&Paragraph> {
    doc.sections.iter().flat_map(|s| &s.paragraphs).collect()
}

/// Unified-diff-style text report: a header naming both paths, one line per
/// non-equal paragraph operation, then a short structural summary.
fn print_text_report(a: &Path, b: &Path, doc_a: &Document, doc_b: &Document, diff: &DocumentDiff) {
    println!("--- {}", a.display());
    println!("+++ {}", b.display());

    let a_paras = flat_paragraphs(doc_a);
    let b_paras = flat_paragraphs(doc_b);

    for entry in &diff.paragraphs {
        match entry.op {
            ParagraphOp::Equal => {}
            ParagraphOp::Insert => {
                let index = entry.b_index.unwrap_or_default();
                let text = b_paras.get(index).map(|p| para_text(p)).unwrap_or_default();
                println!("+ [{index}] {text}");
            }
            ParagraphOp::Delete => {
                let index = entry.a_index.unwrap_or_default();
                let text = a_paras.get(index).map(|p| para_text(p)).unwrap_or_default();
                println!("- [{index}] {text}");
            }
            ParagraphOp::Replace => {
                let a_index = entry.a_index.unwrap_or_default();
                let b_index = entry.b_index.unwrap_or_default();
                let a_text = a_paras
                    .get(a_index)
                    .map(|p| para_text(p))
                    .unwrap_or_default();
                let b_text = b_paras
                    .get(b_index)
                    .map(|p| para_text(p))
                    .unwrap_or_default();
                println!("- [{a_index}] {a_text}");
                println!("+ [{b_index}] {b_text}");
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
    println!(
        "구조: 구역 {}→{}, 문단 {}→{}, 표 변경 {}건, 컨트롤 종류 변경 {}건",
        diff.structure.sections.0,
        diff.structure.sections.1,
        diff.structure.paragraphs.0,
        diff.structure.paragraphs.1,
        diff.structure.tables.len(),
        changed_controls,
    );
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
fn compare_report_json(a: &Path, b: &Path, diff: &DocumentDiff) -> serde_json::Value {
    let paragraphs: Vec<serde_json::Value> = diff
        .paragraphs
        .iter()
        .map(|entry| {
            let mut value = serde_json::json!({
                "op": paragraph_op_str(entry.op),
                "a_index": entry.a_index,
                "b_index": entry.b_index,
            });
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
