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
/// HWP5 `STYLE.attr` carries the kind in its low three bits: 0 paragraph, 1 character (the
/// genuine saved `fixtures/hwp5/bookmark.hwp` has `쪽 번호` at attr 1 and every other style at
/// 0). The HWPX reader never reads `hh:style@type` (gap GE-13), so every HWPX style arrives with
/// attr 0 and would read as a paragraph style even when the file says `CHAR`; for a document
/// read from HWPX the kind is unknown rather than a guess. A reserved value is unknown too.
fn style_kind(doc: &Document, attr: u8) -> Option<StyleKind> {
    if doc.meta.source_format == "hwpx" {
        return None;
    }
    match attr & 0x7 {
        0 => Some(StyleKind::Paragraph),
        1 => Some(StyleKind::Character),
        _ => None,
    }
}

/// Lists every named style of `doc`, joined with the `para` segments of `segments`.
///
/// `segments` must come from [`crate::to_markdown_with_segments_v2`] over the same document.
/// A paragraph naming a style id outside the table is attributed to no style.
pub fn list_named_styles(doc: &Document, segments: &[Segment]) -> Vec<NamedStyle> {
    let mut styles: Vec<NamedStyle> = doc
        .header
        .styles
        .iter()
        .enumerate()
        .map(|(index, style)| NamedStyle {
            id: index as u16,
            name: style.name.clone(),
            english_name: style.english_name.clone(),
            kind: style_kind(doc, style.attr),
            next_style: style.next_style,
            attrs: level(doc, Some(style.char_shape.0), Some(style.para_shape.0)),
            used_by: Vec::new(),
        })
        .collect();
    for segment in segments {
        if segment.kind != SegmentKind::Para {
            continue;
        }
        if let Some(style) = styles.get_mut(segment.style.style_id as usize) {
            style.used_by.push(segment.id.clone());
        }
    }
    styles
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

    #[test]
    fn a_hwpx_document_does_not_claim_a_kind_the_reader_never_read() {
        let doc = doc_with_styles("hwpx", &[0, 0]);
        assert!(
            list_named_styles(&doc, &[])
                .iter()
                .all(|s| s.kind.is_none())
        );
    }
}
