//! `hwp grep` — paragraph text search (GM-5).
//!
//! Recursively walks body, table-cell, and text-box paragraphs and prints the text of each
//! paragraph containing the pattern, one per line. Following grep convention, no matches means
//! an abnormal exit (exit code 1).

use std::path::Path;

use hwp_model::{Control, Document, HwpChar, Paragraph};

use crate::commands::cat::load_document;

pub fn run(pattern: &str, file: &Path, ignore_case: bool) -> anyhow::Result<()> {
    if pattern.is_empty() {
        anyhow::bail!("검색 패턴이 비어 있습니다");
    }
    let doc = load_document(file)?;
    let matches = search(&doc, pattern, ignore_case)?;
    if matches.is_empty() {
        // grep(1) convention: no match is exit code 1 (no error message is printed).
        std::process::exit(1);
    }
    let mut out = matches.join("\n");
    out.push('\n');
    print!("{out}");
    Ok(())
}

/// 패턴이 든 문단 텍스트를 등장 순서로 모은다 (본문·표 셀·글상자 재귀).
///
/// `run`의 순수 코어 — CLI는 exit 코드 계약을, MCP는 그대로 결과를 반환한다.
pub fn search(doc: &Document, pattern: &str, ignore_case: bool) -> anyhow::Result<Vec<String>> {
    if pattern.is_empty() {
        anyhow::bail!("검색 패턴이 비어 있습니다");
    }
    let needle = if ignore_case {
        pattern.to_lowercase()
    } else {
        pattern.to_string()
    };
    let mut matches = Vec::new();
    for section in &doc.sections {
        for para in &section.paragraphs {
            search_para(para, &needle, ignore_case, &mut matches);
        }
    }
    Ok(matches)
}

fn search_para(para: &Paragraph, needle: &str, ignore_case: bool, matches: &mut Vec<String>) {
    let text = para_text(para);
    let haystack = if ignore_case {
        text.to_lowercase()
    } else {
        text.clone()
    };
    if !text.is_empty() && haystack.contains(needle) {
        matches.push(text);
    }
    for control in &para.controls {
        match control {
            Control::Table(t) => {
                for cell in &t.cells {
                    for p in &cell.paragraphs {
                        search_para(p, needle, ignore_case, matches);
                    }
                }
            }
            Control::Generic(g) => {
                for list in &g.paragraph_lists {
                    for p in &list.paragraphs {
                        search_para(p, needle, ignore_case, matches);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Display text of a paragraph (chars; tabs and line breaks become spaces).
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
