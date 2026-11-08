//! The named-style listing behind `hwp styles` (#427 T2): every entry of the header's style
//! table with its resolved attributes and the paragraphs that use it.
//!
//! Nothing here re-derives an id or an attribute. The attributes come from the same
//! [`level`] the segment envelope's `style` level uses, and `used_by` is read off the `para`
//! segments [`crate::to_markdown_with_segments_v2`] already produced, so a listed id is by
//! construction an id `hwp cat --with-segments --segments v2` publishes.

use hwp_model::Document;

use crate::segment::{Segment, SegmentKind, StyleLevel, level};

/// Whether a named style applies to paragraphs or to characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleKind {
    Paragraph,
    Character,
}

impl StyleKind {
    /// The name the listing publishes.
    pub fn as_str(self) -> &'static str {
        match self {
            StyleKind::Paragraph => "paragraph",
            StyleKind::Character => "character",
        }
    }
}

/// One entry of the header's style table.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedStyle {
    /// Index into `DocHeader::styles`, which is what a paragraph's `StyleId` holds.
    pub id: u16,
    pub name: String,
    pub english_name: String,
    /// `None` when the document does not answer: see [`style_kind`].
    pub kind: Option<StyleKind>,
    /// The style id applied to the next paragraph, as the document states it (not validated
    /// against the table).
    pub next_style: u8,
    /// The style's own char and para shapes with the attributes they resolve to, in the shapes
    /// the segment envelope publishes for its `style` level.
    pub attrs: StyleLevel,
    /// Ids of the `para` segments whose paragraph names this style, in segment order. Only
    /// paragraphs `hwp cat --with-segments --segments v2` publishes with default text options
    /// are listed, so a paragraph in a header, a footer or a hidden comment is not.
    pub used_by: Vec<String>,
}

/// The style kind the document states, or `None` where it does not.
///
/// The low three bits of `STYLE.attr` carry the kind: 0 paragraph, 1 character. HWP5 stores them
/// there (한글문서 파일 형식 5.0 §4.2.11 table 48; the genuine saved `fixtures/hwp5/bookmark.hwp` has
/// `쪽 번호` at attr 1 and every other style at 0), and the HWPX reader maps `hh:style@type`
/// `PARA`/`CHAR` onto the same bits (genuine `fixtures/hwpx/seal-groundtruth.hwpx` has `쪽 번호`
/// as `CHAR`). A reserved value is unknown, not a paragraph style.
fn style_kind(attr: u8) -> Option<StyleKind> {
    match attr & 0x7 {
        0 => Some(StyleKind::Paragraph),
        1 => Some(StyleKind::Character),
        _ => None,
    }
}

/// The style table has a row no paragraph's `StyleId` can name.
///
/// `StyleId` is a `u16`, so a table holds at most 65,536 rows (ids 0..=65535). The JSON IR loader
/// does not bound `header.styles`, and listing a longer table would wrap its ids, publishing two
/// rows under one id and breaking the contract that a row's array index is its `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleTableTooLarge {
    /// Number of rows in the table.
    pub rows: usize,
}

impl std::fmt::Display for StyleTableTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "스타일 표가 {}행이라 스타일 번호가 가리킬 수 있는 최대 {}행을 넘습니다. 문서가 손상됐거나 JSON IR의 header.styles가 잘못됐습니다. 원본 HWP/HWPX 파일에서 다시 변환해 보세요",
            self.rows,
            usize::from(u16::MAX) + 1
        )
    }
}

impl std::error::Error for StyleTableTooLarge {}

/// Lists every named style of `doc`, joined with the `para` segments of `segments`.
///
/// `segments` must come from [`crate::to_markdown_with_segments_v2`] over the same document.
/// A paragraph naming a style id outside the table is attributed to no style. A table whose rows
/// outnumber the `StyleId` range is refused ([`StyleTableTooLarge`]) rather than listed with
/// wrapped ids.
pub fn list_named_styles(
    doc: &Document,
    segments: &[Segment],
) -> Result<Vec<NamedStyle>, StyleTableTooLarge> {
    let mut styles = doc
        .header
        .styles
        .iter()
        .enumerate()
        .map(|(index, style)| {
            Ok(NamedStyle {
                id: u16::try_from(index).map_err(|_| StyleTableTooLarge {
                    rows: doc.header.styles.len(),
                })?,
                name: style.name.clone(),
                english_name: style.english_name.clone(),
                kind: style_kind(style.attr),
                next_style: style.next_style,
                attrs: level(doc, Some(style.char_shape.0), Some(style.para_shape.0)),
                used_by: Vec::new(),
            })
        })
        .collect::<Result<Vec<_>, StyleTableTooLarge>>()?;
    for segment in segments {
        if segment.kind != SegmentKind::Para {
            continue;
        }
        if let Some(style) = styles.get_mut(segment.style.style_id as usize) {
            style.used_by.push(segment.id.clone());
        }
    }
    Ok(styles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwp_model::header::Style;

    fn doc_with_styles(source_format: &str, attrs: &[u8]) -> Document {
        let mut doc = Document::default();
        doc.meta.source_format = source_format.to_string();
        doc.header.styles = attrs
            .iter()
            .map(|&attr| Style {
                attr,
                ..Style::default()
            })
            .collect();
        doc
    }

    #[test]
    fn hwp5_attr_low_bits_name_the_kind() {
        let doc = doc_with_styles("hwp5", &[0, 1, 2, 0x81]);
        let kinds: Vec<_> = list_named_styles(&doc, &[])
            .unwrap()
            .iter()
            .map(|s| s.kind.map(StyleKind::as_str))
            .collect();
        assert_eq!(
            kinds,
            [
                Some("paragraph"),
                Some("character"),
                None,
                Some("character")
            ],
            "bits above the low three are not part of the kind, and 2 is reserved"
        );
    }

    /// The kind is a property of the style record, not of the format that carried it: the same
    /// attr reads the same from a document read out of HWPX, which used to answer `None` for
    /// every style because its reader dropped `hh:style@type`.
    #[test]
    fn the_kind_does_not_depend_on_the_source_format() {
        for source in ["hwp5", "hwpx", "markdown"] {
            let doc = doc_with_styles(source, &[0, 1]);
            let kinds: Vec<_> = list_named_styles(&doc, &[])
                .unwrap()
                .iter()
                .map(|s| s.kind.map(StyleKind::as_str))
                .collect();
            assert_eq!(kinds, [Some("paragraph"), Some("character")], "{source}");
        }
    }

    /// The largest table `StyleId` can address lists with distinct, in-order ids; one row more
    /// is refused instead of wrapping row 65,536 back to id 0.
    #[test]
    fn a_table_past_the_style_id_range_is_refused_not_wrapped() {
        let max_rows = usize::from(u16::MAX) + 1;
        let doc = doc_with_styles("hwp5", &vec![0; max_rows]);
        let listed = list_named_styles(&doc, &[]).expect("65,536 rows is the full StyleId range");
        assert_eq!(listed.len(), max_rows);
        assert_eq!(listed[0].id, 0);
        assert_eq!(listed[max_rows - 1].id, u16::MAX);

        let doc = doc_with_styles("hwp5", &vec![0; max_rows + 1]);
        let error = list_named_styles(&doc, &[]).expect_err("65,537 rows has no id for row 65,536");
        assert_eq!(error, StyleTableTooLarge { rows: max_rows + 1 });
        let message = error.to_string();
        assert!(
            message.contains("65537") && message.contains("65536"),
            "{message}"
        );
    }
}
