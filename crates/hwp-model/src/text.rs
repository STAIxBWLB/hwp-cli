//! 텍스트 추출.
//!
//! 컨트롤 포함 정책은 확장 컨트롤의 **문자 코드** 기준으로 정한다
//! (ctrl_id보다 안정적): 표/개체(11)·각주/미주(17)는 포함,
//! 머리말/꼬리말(16)·숨은 설명(15)은 제외가 기본값.

use crate::control::{Caption, CaptionSide, Control};
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
        Control::SectionDef(_) => {}
        Control::Picture(picture) => {
            if let Some(caption) = &picture.caption {
                extract_caption(caption, out, opts);
            }
        }
        Control::Table(table) => {
            if let Some(caption) = table
                .caption
                .as_ref()
                .filter(|caption| caption_precedes_object(caption.side))
            {
                extract_caption(caption, out, opts);
            }
            // Cells use tabs and rows use newlines, similar to hwp5txt output.
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
                // Flatten line breaks inside a cell to spaces.
                out.push_str(cell_text.trim_end().replace('\n', " ").as_str());
            }
            if let Some(caption) = table
                .caption
                .as_ref()
                .filter(|caption| !caption_precedes_object(caption.side))
            {
                extract_caption(caption, out, opts);
            }
            push_newline(out);
        }
        Control::Generic(g) => {
            if let Some(caption) = g
                .caption
                .as_ref()
                .filter(|caption| caption_precedes_object(caption.side))
            {
                extract_caption(caption, out, opts);
            }
            for list in &g.paragraph_lists {
                for para in &list.paragraphs {
                    if !out.is_empty() && !out.ends_with(['\n', ' ', '\t']) {
                        out.push(' ');
                    }
                    para.extract_into(out, opts);
                }
            }
            if let Some(caption) = g
                .caption
                .as_ref()
                .filter(|caption| !caption_precedes_object(caption.side))
            {
                extract_caption(caption, out, opts);
            }
        }
    }
}

fn caption_precedes_object(side: CaptionSide) -> bool {
    matches!(side, CaptionSide::Left | CaptionSide::Top)
}

fn extract_caption(caption: &Caption, out: &mut String, opts: &TextOptions) {
    for para in &caption.paragraphs {
        push_newline(out);
        para.extract_into(out, opts);
    }
}

/// 중복 개행을 만들지 않으면서 개행 추가.
fn push_newline(out: &mut String) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BinRef, BorderFillId, CaptionDirection, Cell, GenericControl, HwpChar, HwpUnit, Picture,
        Table,
    };

    fn paragraph(text: &str) -> Paragraph {
        Paragraph {
            chars: text.chars().map(HwpChar::Text).collect(),
            ..Paragraph::default()
        }
    }

    fn caption(side: CaptionSide, text: &str) -> Caption {
        Caption {
            side,
            direction: CaptionDirection::Horizontal,
            gap: 0,
            width: None,
            last_width: 0,
            paragraphs: vec![paragraph(text)],
        }
    }

    fn table_with_caption(side: CaptionSide) -> Control {
        Control::Table(Table {
            common_data: Vec::new(),
            placement: None,
            attr: 0,
            rows: 1,
            cols: 1,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: vec![1],
            border_fill: BorderFillId(0),
            table_tail: Vec::new(),
            cells: vec![Cell {
                list_attr: 0,
                col: 0,
                row: 0,
                col_span: 1,
                row_span: 1,
                width: HwpUnit(100),
                height: HwpUnit(100),
                margins: [0; 4],
                border_fill: BorderFillId(0),
                header_tail: Vec::new(),
                paragraphs: vec![paragraph("cell")],
            }],
            caption: Some(caption(side, "caption")),
            extras: Vec::new(),
        })
    }

    #[test]
    fn table_caption_follows_visual_reading_order() {
        let mut out = String::new();
        extract_control(
            &table_with_caption(CaptionSide::Top),
            &mut out,
            &TextOptions::default(),
        );
        assert!(
            out.find("caption").unwrap() < out.find("cell").unwrap(),
            "{out:?}"
        );

        out.clear();
        extract_control(
            &table_with_caption(CaptionSide::Bottom),
            &mut out,
            &TextOptions::default(),
        );
        assert!(
            out.find("cell").unwrap() < out.find("caption").unwrap(),
            "{out:?}"
        );
    }

    #[test]
    fn picture_and_generic_captions_are_included() {
        let picture = Control::Picture(Picture {
            common_data: Vec::new(),
            width: HwpUnit(100),
            height: HwpUnit(100),
            treat_as_char: true,
            z_order: 0,
            vert_offset: 0,
            horz_offset: 0,
            description: None,
            crop: None,
            flip: 0,
            rotation: None,
            brightness: 0,
            contrast: 0,
            effect_flags: 0,
            effects_raw: Vec::new(),
            caption: Some(caption(CaptionSide::Bottom, "picture caption")),
            bin_ref: BinRef::ItemRef(String::new()),
            extras: Vec::new(),
        });
        let generic = Control::Generic(GenericControl {
            ctrl_id: *b"gso ",
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: Some(caption(CaptionSide::Top, "shape caption")),
        });
        let mut out = String::new();
        extract_control(&picture, &mut out, &TextOptions::default());
        extract_control(&generic, &mut out, &TextOptions::default());
        assert!(out.contains("picture caption"), "{out:?}");
        assert!(out.contains("shape caption"), "{out:?}");
    }
}
