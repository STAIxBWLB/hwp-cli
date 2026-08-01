//! `hwp grep` — paragraph text search (GM-5).
//!
//! Recursively walks body, table-cell, and text-box paragraphs and prints the text of each
//! paragraph containing the pattern, one per line. Following grep convention, no matches means
//! an abnormal exit (exit code 1).

use std::path::Path;

use hwp_model::{Control, HwpChar, Paragraph};

use crate::commands::cat::load_document;

pub fn run(pattern: &str, file: &Path, ignore_case: bool) -> anyhow::Result<()> {
    if pattern.is_empty() {
        anyhow::bail!("검색 패턴이 비어 있습니다");
    }
    let doc = load_document(file)?;
    let needle = if ignore_case {
        pattern.to_lowercase()
    } else {
        pattern.to_string()
    };
    let mut found = 0usize;
    let mut out = String::new();
    for section in &doc.sections {
        for para in &section.paragraphs {
            search_para(para, &needle, ignore_case, &mut out, &mut found);
        }
    }
    print!("{out}");
    if found == 0 {
        // grep(1) convention: no match is exit code 1 (no error message is printed).
        std::process::exit(1);
    }
    Ok(())
}

fn search_para(
    para: &Paragraph,
    needle: &str,
    ignore_case: bool,
    out: &mut String,
    found: &mut usize,
) {
    let text = para_text(para);
    let haystack = if ignore_case {
        text.to_lowercase()
    } else {
        text.clone()
    };
    if !text.is_empty() && haystack.contains(needle) {
        out.push_str(&text);
        out.push('\n');
        *found += 1;
    }
    for control in &para.controls {
        match control {
            Control::Table(t) => {
                for cell in &t.cells {
                    for p in &cell.paragraphs {
                        search_para(p, needle, ignore_case, out, found);
                    }
                }
            }
            Control::Generic(g) => {
                for list in &g.paragraph_lists {
                    for p in &list.paragraphs {
                        search_para(p, needle, ignore_case, out, found);
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
