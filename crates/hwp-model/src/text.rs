//! Text extraction.
//!
//! Extended-control inclusion is based on the **character code**, which is more stable
//! than ctrl_id: tables/objects (11) and footnotes/endnotes (17) are included by default;
//! headers/footers (16) and hidden comments (15) are excluded by default.

use crate::control::{Caption, CaptionSide, Control};
use crate::document::{Document, Section};
use crate::paragraph::{HwpChar, Paragraph, ctrl_char};

#[derive(Debug, Clone, Default)]
pub struct TextOptions {
    /// Whether to include headers and footers.
    pub include_header_footer: bool,
    /// Whether to include hidden comments.
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

/// The text one character of a paragraph contributes by itself, which is the mapping every
/// text projection shares (`hwp cat`, [`Paragraph::own_text`], heading anchors): a text
/// character is itself, a line break is `\n`, a hyphen is `-`, a non-breaking and a fixed-width
/// space are a space, a tab is `\t`. Anything else, and every extended control, contributes
/// nothing HERE (an extended control's content is the caller's business).
pub fn char_text(ch: &HwpChar, out: &mut String) {
    match ch {
        HwpChar::Text(c) => out.push(*c),
        HwpChar::CharCtrl(code) => match *code {
            ctrl_char::LINE_BREAK => out.push('\n'),
            ctrl_char::HYPHEN => out.push('-'),
            ctrl_char::NB_SPACE | ctrl_char::FW_SPACE => out.push(' '),
            _ => {} // Paragraph ends (13) and similar controls are handled at paragraph boundaries.
        },
        HwpChar::InlineCtrl { code, .. } => {
            if *code == ctrl_char::TAB {
                out.push('\t');
            }
        }
        HwpChar::ExtCtrl { .. } => {}
    }
}

impl Paragraph {
    /// The paragraph's OWN text, exactly as `hwp cat` prints those characters, without the
    /// content of any control in it (a table's cells, a picture's caption, a header). A heading
    /// anchor is compared against this: it is what the paragraph reads like on its own line.
    pub fn own_text(&self) -> String {
        let mut out = String::new();
        for ch in &self.chars {
            char_text(ch, &mut out);
        }
        out
    }

    /// Appends this paragraph's text to out; the caller supplies the paragraph-ending newline.
    pub fn extract_into(&self, out: &mut String, opts: &TextOptions) {
        for ch in &self.chars {
            match ch {
                HwpChar::Text(_) | HwpChar::CharCtrl(_) | HwpChar::InlineCtrl { .. } => {
                    char_text(ch, out);
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

    /// Plain text of one paragraph, for tests and debugging.
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

/// Appends a newline without duplicating one already present.
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
            hwpx_raw_xml: None,
            container_box: None,
        });
        let mut out = String::new();
        extract_control(&picture, &mut out, &TextOptions::default());
        extract_control(&generic, &mut out, &TextOptions::default());
        assert!(out.contains("picture caption"), "{out:?}");
        assert!(out.contains("shape caption"), "{out:?}");
    }
}
