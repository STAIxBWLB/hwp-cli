//! `hwp grep` — 문단 텍스트 검색 (GM-5).
//!
//! 본문·표 셀·글상자 문단을 재귀 순회하며 패턴을 가진 문단의 텍스트를 한 줄씩
//! 출력한다. grep 관례로 일치가 없으면 비정상 종료(종료 코드 1)다.

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
        // grep(1) 관례: 일치 없음은 종료 코드 1 (오류 메시지는 내지 않는다).
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

/// 문단의 표시 텍스트(문자 + 탭은 공백, 줄바꿈은 공백).
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
