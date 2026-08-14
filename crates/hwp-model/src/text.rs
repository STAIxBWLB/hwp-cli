//! 텍스트 추출.
//!
//! 컨트롤 포함 정책은 확장 컨트롤의 **문자 코드** 기준으로 정한다
//! (ctrl_id보다 안정적): 표/개체(11)·각주/미주(17)는 포함,
//! 머리말/꼬리말(16)·숨은 설명(15)은 제외가 기본값.

use crate::control::Control;
use crate::document::{Document, Section};
use crate::paragraph::{HwpChar, Paragraph, ctrl_char};

#[derive(Debug, Clone, Default)]
pub struct TextOptions {
    /// 머리말/꼬리말 포함 여부
    pub include_header_footer: bool,
    /// 숨은 설명 포함 여부
    pub include_hidden: bool,
}

impl Document {
    pub fn plain_text(&self) -> String {
        self.plain_text_with(&TextOptions::default())
    }

    pub fn plain_text_with(&self, opts: &TextOptions) -> String {
        let mut out = String::new();
        for section in &self.sections {
            section.extract_into(&mut out, opts);
        }
        out
    }
}

impl Section {
    fn extract_into(&self, out: &mut String, opts: &TextOptions) {
        for para in &self.paragraphs {
            para.extract_into(out, opts);
            push_newline(out);
        }
    }
}

impl Paragraph {
    /// 이 문단의 텍스트를 out에 덧붙인다 (문단 끝 개행은 호출자 책임).
    pub fn extract_into(&self, out: &mut String, opts: &TextOptions) {
        for ch in &self.chars {
            match ch {
                HwpChar::Text(c) => out.push(*c),
                HwpChar::CharCtrl(code) => match *code {
                    ctrl_char::LINE_BREAK => out.push('\n'),
                    ctrl_char::HYPHEN => out.push('-'),
                    ctrl_char::NB_SPACE | ctrl_char::FW_SPACE => out.push(' '),
                    _ => {} // 문단 끝(13) 등은 문단 경계에서 처리
                },
                HwpChar::InlineCtrl { code, .. } => {
                    if *code == ctrl_char::TAB {
                        out.push('\t');
                    }
                }
                HwpChar::ExtCtrl {
                    code, ctrl_index, ..
                } => {
                    let included = match *code {
                        ctrl_char::HEADER_FOOTER => opts.include_header_footer,
                        ctrl_char::HIDDEN_COMMENT => opts.include_hidden,
                        _ => true,
                    };
                    if !included {
                        continue;
                    }
                    if let Some(idx) = ctrl_index
                        && let Some(control) = self.controls.get(*idx as usize)
                    {
                        extract_control(control, out, opts);
                    }
                }
            }
        }
    }

    /// 단독 문단의 평문 (테스트/디버깅 편의).
    pub fn plain_text(&self) -> String {
        let mut s = String::new();
        self.extract_into(&mut s, &TextOptions::default());
        s
    }
}

fn extract_control(control: &Control, out: &mut String, opts: &TextOptions) {
    match control {
        Control::SectionDef(_) | Control::Picture(_) => {}
        Control::Table(table) => {
            // 셀 사이는 탭, 행 사이는 개행 — hwp5txt와 유사한 평문 표현
            push_newline(out);
            let mut current_row = u16::MAX;
            for cell in &table.cells {
                if cell.row != current_row {
                    if current_row != u16::MAX {
                        push_newline(out);
                    }
                    current_row = cell.row;
                } else {
                    out.push('\t');
                }
                let mut cell_text = String::new();
                for para in &cell.paragraphs {
                    para.extract_into(&mut cell_text, opts);
                    cell_text.push('\n');
                }
                // 셀 내부 개행은 공백으로 평탄화
                out.push_str(cell_text.trim_end().replace('\n', " ").as_str());
            }
            // 캡션 텍스트는 표 뒤에 덧붙인다.
            if let Some(caption) = &table.caption {
                for para in &caption.paragraphs {
                    push_newline(out);
                    para.extract_into(out, opts);
                }
            }
            push_newline(out);
        }
        Control::Generic(g) => {
            for list in &g.paragraph_lists {
                for para in &list.paragraphs {
                    if !out.is_empty() && !out.ends_with(['\n', ' ', '\t']) {
                        out.push(' ');
                    }
                    para.extract_into(out, opts);
                }
            }
        }
    }
}

/// 중복 개행을 만들지 않으면서 개행 추가.
fn push_newline(out: &mut String) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}
