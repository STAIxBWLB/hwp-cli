//! The v2 segment model: seven kinds, nested character ranges, two-level style (D-05, D-06a).
//!
//! # What a segment is
//!
//! One segment ties a range of the emitted markdown to the IR node that produced it. The kinds
//! are `para`, `run`, `table`, `cell`, `image`, `field` and `bookmark`. Ranges nest: a `run`
//! range lies inside its `para` range, a `cell` range inside its `table` range, and siblings at
//! one level do not overlap. The vector is ordered by `start`, ties broken so the containing
//! segment precedes the contained one.
//!
//! Offsets are **Unicode scalar** offsets into the cleaned markdown, not bytes — the same
//! discipline [`crate::markdown::to_markdown_with_segments`] (v1) already uses: byte spans are
//! recorded during emission, remapped through the cleanup deletion map, and converted to scalars
//! in one pass. A consumer can slice `md[start:end]` in Python directly.
//!
//! # One emission core
//!
//! There is no second emitter. The v2 vector is produced by the same `emit_markdown` the default
//! path uses, with span recording switched on, so the markdown string is byte-identical with and
//! without segments as a structural property rather than as a tested coincidence.
//!
//! # Ids
//!
//! Every id comes from [`crate::segment_id`]'s typed per-kind entry points. This module hashes
//! nothing itself. `crates/hwp-render/src/segment_id.rs` carries a deliberate second copy of that
//! rule, so changing what a kind hashes is a two-crate change.
//!
//! A run's range covers everything the run emitted, its own emphasis markers included. That is a
//! property, not an accident of the walk: excluding the markers would leave them belonging to no
//! run at all, and the boundary the walk reads — after `close_span` has finished moving the
//! previous span's whitespace outside its markers, and before the next span's opening marker is
//! pushed — is the only offset in that sequence that does not shift under the move.
//!
//! Run segments are numbered off [`crate::segment_id::canonical_char_shape_runs`], never off
//! `Paragraph::char_shape_runs`: `run_id` indexes the canonical list, so walking the raw list
//! would give a run segment its text from one list and its id from another — and the two lists
//! differ only on hwp5 input carrying a redundant or same-position shape run.
//!
//! # Style: two levels, side by side
//!
//! Each segment reports the paragraph style's shapes and the shapes the segment itself names,
//! each with its ids, and never a resolved or flattened value (D-06, D-06a). HWP5 stores whole
//! shapes keyed by id and records no per-attribute override flag, so a flattened value would
//! destroy information and a synthesized "explicit" flag would invent it. Equal ids across the
//! two levels mean inherited; different ids mean the segment sets it. The comparison inputs are
//! published, not the comparison.
//!
//! The same rule decides the font face. A character shape names a font *id* per language slot;
//! [`CharAttrs`] publishes those ids and the names they resolve to, side by side. D-05 exists so
//! an editor can draw a toolbar state without a second query, and what a toolbar shows is a font
//! name — an id alone would leave the editor resolving `DocHeader::fonts[slot][id]` itself, which
//! is exactly the second query D-05 was written to avoid. Neither stands in for the other: the id
//! is the document's own statement, the name is the convenience.
//!
//! # Declared limitations
//!
//! - **Unrecognized field kinds (gap catalog GF-1, GF-4).** `field` segments are discriminated by
//!   [`crate::field::is_field_ctrl_id`], which today recognizes 33 of the 34 kinds in
//!   specification table 128 — every kind but memo (`%%me`). **GF-4's own text is stale**: it
//!   still says "22 of 34 field kinds unrecognized", a count from before the recognition list was
//!   extended. What remains true is GF-1: only 12 kinds map to an OWPML type and 13 carry a kind
//!   label, and the rest fall back to `%unk`/`UNKNOWN`. Both gaps are pre-existing and inherited
//!   here, not fixed: a consumer is not entitled to read zero `field` segments as "this document
//!   has none", and a reported `ctrl_id` may be the `%unk` fallback rather than the document's own
//!   kind.
//! - **Bookmarks are invisible by design (GG-25).** A `bokm` control emits no markdown, so a
//!   `bookmark` segment is a point segment with `start == end`.
//! - Bookmarks are `bokm` controls and not `%bmk` fields, which is why they are discriminated by
//!   [`crate::bookmark`] rather than by widening the field predicate.

use hwp_model::header::{CharShape, ParaShape};
use hwp_model::{Document, Paragraph};

use crate::markdown::MarkdownOptions;
use crate::segment_id::{SegmentPath, canonical_char_shape_runs};

/// Language slots of `CharShape::face_ids`, in IR order.
pub const FACE_SLOTS: [&str; 7] = [
    "hangul", "latin", "hanja", "japanese", "other", "symbol", "user",
];

/// What an emitted range is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentKind {
    /// A top-level or nested paragraph.
    Para,
    /// One character-shape run of a paragraph, numbered off the canonical run list.
    Run,
    /// A table control.
    Table,
    /// One cell of a table.
    Cell,
    /// An embedded picture's emitted reference.
    Image,
    /// A field control (`%clk`, `%hlk`, …) from `FIELD_START` to `FIELD_END`.
    Field,
    /// A `bokm` control: a point marker that emits nothing.
    Bookmark,
}

impl SegmentKind {
    /// The wire name of the kind, which 05-05's envelope publishes verbatim.
    pub fn as_str(self) -> &'static str {
        match self {
            SegmentKind::Para => "para",
            SegmentKind::Run => "run",
            SegmentKind::Table => "table",
            SegmentKind::Cell => "cell",
            SegmentKind::Image => "image",
            SegmentKind::Field => "field",
            SegmentKind::Bookmark => "bookmark",
        }
    }

    /// Whether a zero-width range is meaningful for this kind.
    ///
    /// A bookmark is a point marker and emits nothing; an empty table cell still exists as a
    /// cell. Every other kind is dropped when its range collapses to nothing.
    fn may_be_empty(self) -> bool {
        matches!(self, SegmentKind::Bookmark | SegmentKind::Cell)
    }
}

/// The character attributes D-05 names, in units a consumer cannot misread.
#[derive(Debug, Clone, PartialEq)]
pub struct CharAttrs {
    /// Font id per language slot, in [`FACE_SLOTS`] order. Seven slots are published rather than
    /// one, because picking one would silently pick a language for the consumer.
    pub face_ids: [u16; 7],
    /// The face name each slot's id resolves to in `DocHeader::fonts`, in the same slot order.
    /// A toolbar draws this without the second query an id alone would force (D-05).
    ///
    /// `None` means **this document's font table does not answer for that slot** — the id is out
    /// of range. It does not mean "no font", and a consumer must not read it as a default face.
    pub faces: [Option<String>; 7],
    /// Size in **points**. `CharShape::base_size` is HWPUNIT, where 1000 = 10pt.
    pub size_pt: f32,
    /// From `CharShape::is_bold()`, not from a re-derived bit test.
    pub bold: bool,
    /// From `CharShape::is_italic()`.
    pub italic: bool,
    /// `#RRGGBB`. `CharShape::text_color` is a COLORREF (`0x00BBGGRR`), whose byte order is a
    /// trap for the web consumers of this envelope, so it is decoded here exactly once.
    pub color: String,
}

/// The paragraph attributes D-05 names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParaAttrs {
    /// The decoded `ParaShape::alignment()` value as a name, never a bare integer:
    /// `justify | left | right | center | distribute | divide`.
    pub alignment: &'static str,
    /// `ParaShape::indent` in HWPUNIT (1/7200 inch); negative means an outdent.
    pub indent: i32,
    /// `ParaShape::line_spacing_type`: 0 percent, 1 fixed, 2 margin-only, 3 at-least.
    pub line_spacing_type: u8,
    /// `ParaShape::line_spacing`, whose unit follows `line_spacing_type` — percent for type 0,
    /// HWPUNIT for type 1. The pair is published rather than a resolved number, because
    /// collapsing them loses the distinction.
    pub line_spacing: i32,
}

/// One of the two style levels: the ids it names and what they resolve to.
///
/// An id that does not resolve in the header tables leaves the corresponding attributes `None`
/// — an explicitly absent block, never a fabricated default a consumer would read as the
/// document's own value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StyleLevel {
    /// `None` when the level itself is unreachable (an unresolvable `StyleId`).
    pub char_shape_id: Option<u16>,
    pub para_shape_id: Option<u16>,
    pub char: Option<CharAttrs>,
    pub para: Option<ParaAttrs>,
}

/// The two style levels, side by side and never merged (D-06a).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SegmentStyle {
    /// Reached through the paragraph's `StyleId` to the style's own char and para shapes.
    pub style: StyleLevel,
    /// Reached through the ids the segment itself names.
    pub direct: StyleLevel,
}

/// One emitted range, its IR coordinates, its id and its style.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub kind: SegmentKind,
    /// Derived by [`crate::segment_id`]; never read out of the source file.
    pub id: String,
    /// The positional half of the id: section plus child indices.
    pub path: SegmentPath,
    /// Unicode scalar offset into the markdown, inclusive.
    pub start: usize,
    /// Unicode scalar offset into the markdown, exclusive.
    pub end: usize,
    pub style: SegmentStyle,
    /// The four-byte control id of a `field` or `bookmark` segment (`%clk`, `bokm`), which is how
    /// a consumer tells one generic control from another without re-reading the document. See the
    /// module doc on GF-1: a field's `ctrl_id` may be the `%unk` fallback.
    pub ctrl_id: Option<String>,
    /// A bookmark's name, read by [`crate::bookmark::bookmark_name`]. A bookmark segment without
    /// it is not usable by an editor, which is why it is published rather than left to a second
    /// query.
    pub name: Option<String>,
}

/// A span recorded during emission, in the byte coordinates of the buffer being written.
///
/// Crate-private: it is the wire between `markdown.rs`'s instrumentation and [`finalize`].
pub(crate) struct RawSeg {
    pub kind: SegmentKind,
    pub id: String,
    pub path: SegmentPath,
    pub style: SegmentStyle,
    pub ctrl_id: Option<String>,
    pub name: Option<String>,
    pub start: usize,
    pub end: usize,
}

/// The markdown of [`crate::markdown::to_markdown_with`], plus the v2 segment vector.
///
/// The string is byte-identical to the one the default path returns: both come from the same
/// emission core, which this entry point only asks to record spans.
pub fn to_markdown_with_segments_v2(
    doc: &Document,
    opts: &MarkdownOptions,
) -> std::io::Result<(String, Vec<Segment>)> {
    let (markdown, _, segments) = crate::markdown::emit_markdown(doc, opts, true)?;
    Ok((markdown, segments))
}

/// Turns recorded byte spans into the published scalar-offset vector.
///
/// `remap` carries each raw byte offset through the cleanup deletion map and on to a Unicode
/// scalar offset — the same two steps the v1 `build_segments` performs.
pub(crate) fn finalize(raw: Vec<RawSeg>, mut remap: impl FnMut(usize) -> usize) -> Vec<Segment> {
    let mut out: Vec<Segment> = raw
        .into_iter()
        .filter_map(|seg| {
            let start = remap(seg.start);
            let end = remap(seg.end).max(start);
            (end > start || seg.kind.may_be_empty()).then_some(Segment {
                kind: seg.kind,
                id: seg.id,
                path: seg.path,
                start,
                end,
                style: seg.style,
                ctrl_id: seg.ctrl_id,
                name: seg.name,
            })
        })
        .collect();
    // Document order, with the container ahead of what it contains.
    out.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    out
}

/// The two style levels for a segment sitting in `para` and naming `char_shape_id`.
///
/// `char_shape_id` is the segment's own character shape — the run's for a `run`, the shape at the
/// control's character position for a `table`, `image`, `field` or `bookmark`, the first
/// canonical run's for a `para`.
pub(crate) fn summarize(
    doc: &Document,
    para: &Paragraph,
    char_shape_id: Option<u16>,
) -> SegmentStyle {
    let style = doc.header.styles.get(para.style.0 as usize);
    SegmentStyle {
        style: level(
            doc,
            style.map(|s| s.char_shape.0),
            style.map(|s| s.para_shape.0),
        ),
        direct: level(doc, char_shape_id, Some(para.para_shape.0)),
    }
}

/// The character shape id in force at WCHAR position `pos`, off the canonical run list.
pub(crate) fn char_shape_id_at(para: &Paragraph, pos: u32) -> Option<u16> {
    canonical_char_shape_runs(para)
        .iter()
        .rev()
        .find(|(start, _)| *start <= pos)
        .map(|(_, id)| id.0)
}

fn level(doc: &Document, char_shape_id: Option<u16>, para_shape_id: Option<u16>) -> StyleLevel {
    StyleLevel {
        char_shape_id,
        para_shape_id,
        char: char_shape_id
            .and_then(|id| doc.header.char_shapes.get(id as usize))
            .map(|shape| char_attrs(doc, shape)),
        para: para_shape_id
            .and_then(|id| doc.header.para_shapes.get(id as usize))
            .map(para_attrs),
    }
}

fn char_attrs(doc: &Document, shape: &CharShape) -> CharAttrs {
    CharAttrs {
        face_ids: shape.face_ids,
        faces: std::array::from_fn(|slot| {
            doc.header.fonts[slot]
                .get(shape.face_ids[slot] as usize)
                .map(|face| face.name.clone())
        }),
        size_pt: shape.base_size as f32 / 100.0,
        bold: shape.is_bold(),
        italic: shape.is_italic(),
        color: colorref_to_hex(shape.text_color),
    }
}

fn para_attrs(shape: &ParaShape) -> ParaAttrs {
    ParaAttrs {
        alignment: match shape.alignment() {
            0 => "justify",
            1 => "left",
            2 => "right",
            3 => "center",
            4 => "distribute",
            5 => "divide",
            // The field is three bits wide but the specification defines only 0..=5.
            _ => "unknown",
        },
        indent: shape.indent,
        line_spacing_type: shape.line_spacing_type,
        line_spacing: shape.line_spacing,
    }
}

/// COLORREF `0x00BBGGRR` → `#RRGGBB`.
fn colorref_to_hex(color: u32) -> String {
    format!(
        "#{:02X}{:02X}{:02X}",
        color & 0xFF,
        (color >> 8) & 0xFF,
        (color >> 16) & 0xFF
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::to_markdown_with;
    use hwp_model::control::{BinRef, Cell, GenericControl, Picture, Table};
    use hwp_model::document::BinStream;
    use hwp_model::header::{BinDataItem, FaceName, Style};
    use hwp_model::ids::{BinDataId, BorderFillId, CharShapeId, ParaShapeId, StyleId};
    use hwp_model::units::HwpUnit;
    use hwp_model::{Control, HwpChar, Section, ctrl_char};

    /// The scalar slice a segment points at, so an assertion reads the markdown the way a
    /// consumer would.
    fn slice(md: &str, seg: &Segment) -> String {
        md.chars().take(seg.end).skip(seg.start).collect()
    }

    fn of(segs: &[Segment], kind: SegmentKind) -> Vec<&Segment> {
        segs.iter().filter(|s| s.kind == kind).collect()
    }

    fn only(segs: &[Segment], kind: SegmentKind) -> Segment {
        let found = of(segs, kind);
        assert_eq!(found.len(), 1, "expected one {:?}: {segs:#?}", kind);
        found[0].clone()
    }

    fn emit(doc: &Document) -> (String, Vec<Segment>) {
        to_markdown_with_segments_v2(doc, &MarkdownOptions::default()).expect("no media_dir")
    }

    fn char_shape(base_size: i32, color: u32, attr: u32) -> CharShape {
        CharShape {
            base_size,
            text_color: color,
            attr,
            ..Default::default()
        }
    }

    fn para(text: &str) -> Paragraph {
        Paragraph {
            chars: text.chars().map(HwpChar::Text).collect(),
            char_shape_runs: vec![(0, CharShapeId(0))],
            ..Default::default()
        }
    }

    /// Shapes 0 and 1 differ in every attribute the summary publishes, and style 0 points at
    /// shape 0, so "inherited" and "set by the segment" are both reachable.
    fn doc_of(paragraphs: Vec<Paragraph>) -> Document {
        let mut doc = Document {
            sections: vec![Section {
                paragraphs,
                ..Default::default()
            }],
            ..Default::default()
        };
        doc.header.char_shapes = vec![
            char_shape(1000, 0x0000_0000, 0),
            // COLORREF 0x00BBGGRR: red 0x11, green 0x22, blue 0xEE - asymmetric, so a byte-order
            // mistake cannot pass by symmetry. Bold + italic bits set.
            char_shape(1200, 0x00EE_2211, 0b11),
        ];
        doc.header.para_shapes = vec![ParaShape::default()];
        doc.header.styles = vec![Style {
            name: "바탕글".into(),
            char_shape: CharShapeId(0),
            para_shape: ParaShapeId(0),
            ..Default::default()
        }];
        doc.header.fonts[0] = vec![FaceName {
            name: "함초롬바탕".into(),
            ..Default::default()
        }];
        doc
    }

    fn cell_of(col: u16, row: u16, text: &str) -> Cell {
        Cell {
            list_attr: 0,
            col,
            row,
            col_span: 1,
            row_span: 1,
            width: HwpUnit(1000),
            height: HwpUnit(1000),
            margins: [0; 4],
            border_fill: BorderFillId::default(),
            header_tail: Vec::new(),
            paragraphs: vec![para(text)],
        }
    }

    fn table_of(cells: Vec<Cell>, rows: u16, cols: u16) -> Table {
        Table {
            common_data: Vec::new(),
            placement: None,
            attr: 0,
            rows,
            cols,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: vec![cols; rows as usize],
            border_fill: BorderFillId::default(),
            table_tail: Vec::new(),
            cells,
            caption: None,
            extras: Vec::new(),
        }
    }

    fn picture_of() -> Picture {
        Picture {
            common_data: Vec::new(),
            width: HwpUnit(1000),
            height: HwpUnit(1000),
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
            caption: None,
            bin_ref: BinRef::Id(BinDataId(0)),
            extras: Vec::new(),
        }
    }

    fn generic(ctrl_id: [u8; 4]) -> GenericControl {
        GenericControl {
            ctrl_id,
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        }
    }

    /// Appends an extended-control character bound to `control`.
    fn attach(para: &mut Paragraph, code: u16, ctrl_id: [u8; 4], control: Control) {
        let index = para.controls.len() as u32;
        para.controls.push(control);
        para.chars.push(HwpChar::ExtCtrl {
            code,
            ctrl_id,
            payload: Vec::new(),
            ctrl_index: Some(index),
        });
    }

    // ---------------------------------------------------------------- task 1

    /// One plain paragraph yields one `para` plus one `run` per canonical run, nested.
    #[test]
    fn plain_paragraph_yields_one_para_and_one_run_per_canonical_run() {
        let doc = doc_of(vec![para("한 문단")]);
        let (md, segs) = emit(&doc);
        let para_seg = only(&segs, SegmentKind::Para);
        let runs = of(&segs, SegmentKind::Run);
        assert_eq!(
            runs.len(),
            canonical_char_shape_runs(&doc.sections[0].paragraphs[0]).len()
        );
        assert!(runs[0].start >= para_seg.start && runs[0].end <= para_seg.end);
        assert_eq!(slice(&md, runs[0]), "한 문단");
        assert_ne!(para_seg.id, runs[0].id, "two kinds must not share an id");
    }

    /// A redundant character-shape run is the form an hwp5 read produces; both readers must
    /// yield the same run segments, because both enumerate the canonical list.
    #[test]
    fn a_redundant_run_yields_the_same_run_segments_as_the_hwpx_form() {
        let mut hwpx = para("가나다라");
        hwpx.char_shape_runs = vec![(0, CharShapeId(0)), (2, CharShapeId(1))];
        let mut hwp5 = hwpx.clone();
        // The HWP5 reader keeps every PARA_CHAR_SHAPE entry, redundant ones included.
        hwp5.char_shape_runs = vec![
            (0, CharShapeId(0)),
            (1, CharShapeId(0)),
            (2, CharShapeId(1)),
        ];
        let (md_x, segs_x) = emit(&doc_of(vec![hwpx]));
        let (md_5, segs_5) = emit(&doc_of(vec![hwp5]));
        assert_eq!(md_x, md_5);
        let (runs_x, runs_5) = (of(&segs_x, SegmentKind::Run), of(&segs_5, SegmentKind::Run));
        assert_eq!(runs_x.len(), 2, "{segs_x:#?}");
        assert_eq!(runs_5.len(), runs_x.len());
        for (a, b) in runs_5.iter().zip(&runs_x) {
            assert_eq!((a.start, a.end, &a.id), (b.start, b.end, &b.id));
        }
        // The slice and the id describe the same run: run 1 is "다라" on both sides, wrapped in
        // the emphasis markers its own shape produced.
        assert_eq!(slice(&md_5, runs_5[1]), "***다라***");
    }

    /// A table yields one `table` holding one `cell` per cell, with no two cells overlapping.
    #[test]
    fn a_table_yields_one_table_holding_one_cell_per_cell() {
        let mut p = para("표 앞");
        attach(
            &mut p,
            ctrl_char::OBJECT,
            *b"tbl ",
            Control::Table(table_of(
                vec![
                    cell_of(0, 0, "머리1"),
                    cell_of(1, 0, "머리2"),
                    cell_of(0, 1, "값1"),
                    cell_of(1, 1, "값2"),
                ],
                2,
                2,
            )),
        );
        let (md, segs) = emit(&doc_of(vec![p]));
        let table = only(&segs, SegmentKind::Table);
        let cells = of(&segs, SegmentKind::Cell);
        assert_eq!(cells.len(), 4, "{segs:#?}");
        for cell in &cells {
            assert!(
                cell.start >= table.start && cell.end <= table.end,
                "cell {cell:?} outside table {table:?}"
            );
        }
        for pair in cells.windows(2) {
            assert!(
                pair[0].end <= pair[1].start,
                "cells overlap: {:?} {:?}",
                pair[0],
                pair[1]
            );
        }
        assert_eq!(slice(&md, cells[0]), "머리1");
        assert_eq!(slice(&md, cells[3]), "값2");
    }

    /// An embedded picture yields an `image` segment covering the emitted reference.
    #[test]
    fn a_picture_yields_an_image_segment_over_its_reference() {
        let mut doc = doc_of(vec![para("그림 앞")]);
        doc.header.bin_data = vec![BinDataItem {
            attr: 1,
            storage_id: Some(1),
            extension: Some("png".into()),
            ..Default::default()
        }];
        doc.bin_streams = vec![BinStream {
            name: "BIN0001.png".into(),
            data: b"\x89PNG\r\n\x1a\n".to_vec(),
        }];
        let picture = Picture {
            bin_ref: BinRef::Id(BinDataId(1)),
            ..picture_of()
        };
        attach(
            &mut doc.sections[0].paragraphs[0],
            ctrl_char::OBJECT,
            *b"gso ",
            Control::Picture(picture),
        );
        let (md, segs) = emit(&doc);
        let image = only(&segs, SegmentKind::Image);
        assert_eq!(slice(&md, &image), "![image]()");
    }

    /// A field control yields a `field` segment spanning its display text; a bookmark yields a
    /// point `bookmark` segment - built on `bookmark.rs`, which owns `bokm` controls, not on a
    /// widened field predicate.
    #[test]
    fn a_field_and_a_bookmark_each_yield_their_own_kind() {
        let mut p = Paragraph {
            char_shape_runs: vec![(0, CharShapeId(0))],
            ..Default::default()
        };
        attach(
            &mut p,
            ctrl_char::FIELD_START,
            *b"%clk",
            Control::Generic(generic(*b"%clk")),
        );
        p.chars.extend("누름틀 값".chars().map(HwpChar::Text));
        p.chars.push(HwpChar::InlineCtrl {
            code: ctrl_char::FIELD_END,
            payload: Vec::new(),
        });
        let mut bokm = generic(*b"bokm");
        bokm.raw_children = vec![hwp_model::opaque::OpaqueRecord {
            tag: 0x0010 + 71,
            data: crate::bookmark::make_bokm_ctrl_data("표식"),
            children: Vec::new(),
        }];
        attach(
            &mut p,
            ctrl_char::BOOKMARK,
            *b"bokm",
            Control::Generic(bokm),
        );
        let (md, segs) = emit(&doc_of(vec![p]));

        let field = only(&segs, SegmentKind::Field);
        assert_eq!(field.ctrl_id.as_deref(), Some("%clk"));
        assert_eq!(slice(&md, &field), "누름틀 값");

        let bookmark = only(&segs, SegmentKind::Bookmark);
        assert_eq!(bookmark.ctrl_id.as_deref(), Some("bokm"));
        assert_eq!(bookmark.name.as_deref(), Some("표식"));
        // A bookmark emits nothing (GG-25), so it is a point segment.
        assert_eq!(bookmark.start, bookmark.end);
        assert_ne!(field.id, bookmark.id);
    }

    /// The v2 vector never moves the default path: one emission core, so the string is
    /// byte-identical with and without segments.
    #[test]
    fn the_markdown_is_byte_identical_with_and_without_segments() {
        let doc = seven_kind_document();
        let plain = to_markdown_with(&doc, &MarkdownOptions::default()).expect("no media_dir");
        let (with_segments, _) = emit(&doc);
        assert_eq!(plain, with_segments);
    }

    /// One document, all seven kinds, in document order with the container ahead of what it
    /// contains and every range inside the markdown.
    #[test]
    fn all_seven_kinds_are_ordered_nested_and_in_range() {
        let doc = seven_kind_document();
        let (md, segs) = emit(&doc);
        let scalars = md.chars().count();
        let kinds: Vec<&'static str> = {
            let mut k: Vec<&'static str> = segs.iter().map(|s| s.kind.as_str()).collect();
            k.sort_unstable();
            k.dedup();
            k
        };
        assert_eq!(
            kinds,
            vec!["bookmark", "cell", "field", "image", "para", "run", "table"],
            "{segs:#?}"
        );
        for seg in &segs {
            assert!(seg.start <= seg.end, "{seg:?}");
            assert!(seg.end <= scalars, "{seg:?} past {scalars} scalars");
            if seg.kind != SegmentKind::Bookmark {
                assert!(
                    seg.start < seg.end,
                    "only a point segment may be empty: {seg:?}"
                );
            }
        }
        for pair in segs.windows(2) {
            assert!(pair[0].start <= pair[1].start, "not in document order");
            if pair[0].start == pair[1].start {
                assert!(
                    pair[0].end >= pair[1].end,
                    "container must precede contained"
                );
            }
        }
        // Every run sits inside some paragraph.
        for run in of(&segs, SegmentKind::Run) {
            assert!(
                of(&segs, SegmentKind::Para)
                    .iter()
                    .any(|p| run.start >= p.start && run.end <= p.end),
                "run outside every paragraph: {run:?}"
            );
        }
    }

    /// A document carrying every kind: text runs, a picture, a field, a bookmark and a table.
    fn seven_kind_document() -> Document {
        let mut first = para("첫 문단");
        first.char_shape_runs = vec![(0, CharShapeId(0)), (2, CharShapeId(1))];
        let mut second = Paragraph {
            char_shape_runs: vec![(0, CharShapeId(0))],
            ..Default::default()
        };
        attach(
            &mut second,
            ctrl_char::FIELD_START,
            *b"%clk",
            Control::Generic(generic(*b"%clk")),
        );
        second.chars.extend("필드 값".chars().map(HwpChar::Text));
        second.chars.push(HwpChar::InlineCtrl {
            code: ctrl_char::FIELD_END,
            payload: Vec::new(),
        });
        attach(
            &mut second,
            ctrl_char::BOOKMARK,
            *b"bokm",
            Control::Generic(generic(*b"bokm")),
        );
        attach(
            &mut second,
            ctrl_char::OBJECT,
            *b"gso ",
            Control::Picture(picture_of()),
        );
        let mut third = para("표 앞");
        attach(
            &mut third,
            ctrl_char::OBJECT,
            *b"tbl ",
            Control::Table(table_of(
                vec![cell_of(0, 0, "머리"), cell_of(0, 1, "값")],
                2,
                1,
            )),
        );
        doc_of(vec![first, second, third])
    }

    // ---------------------------------------------------------------- task 3

    /// Equal ids mean inherited, different ids mean the segment sets it - and the envelope
    /// publishes the comparison inputs, never the comparison.
    #[test]
    fn the_two_style_levels_report_their_own_ids_and_are_never_merged() {
        let mut p = para("가나다라");
        p.char_shape_runs = vec![(0, CharShapeId(0)), (2, CharShapeId(1))];
        let (_, segs) = emit(&doc_of(vec![p]));
        let runs = of(&segs, SegmentKind::Run);
        assert_eq!(runs.len(), 2);

        let inherited = &runs[0].style;
        assert_eq!(inherited.style.char_shape_id, Some(0));
        assert_eq!(inherited.direct.char_shape_id, Some(0));
        assert_eq!(inherited.style.char, inherited.direct.char);

        let overriding = &runs[1].style;
        assert_eq!(overriding.style.char_shape_id, Some(0));
        assert_eq!(overriding.direct.char_shape_id, Some(1));
        assert_ne!(overriding.style.char, overriding.direct.char);

        // Both blocks are always present, including where they are identical.
        for seg in &segs {
            assert!(seg.style.style.char.is_some(), "{seg:?}");
            assert!(seg.style.direct.char.is_some(), "{seg:?}");
            assert!(seg.style.style.para.is_some(), "{seg:?}");
            assert!(seg.style.direct.para.is_some(), "{seg:?}");
        }
    }

    /// Bold and italic come from the shape's accessors; size is points; colour decodes the
    /// COLORREF byte order.
    #[test]
    fn character_attributes_are_published_in_units_a_consumer_cannot_misread() {
        let mut p = para("가나");
        p.char_shape_runs = vec![(0, CharShapeId(1))];
        let doc = doc_of(vec![p]);
        let shape = &doc.header.char_shapes[1];
        assert_ne!(
            shape.text_color & 0xFF,
            (shape.text_color >> 16) & 0xFF,
            "red and blue must differ so a byte swap cannot pass"
        );
        let (_, segs) = emit(&doc);
        let direct = of(&segs, SegmentKind::Run)[0]
            .style
            .direct
            .char
            .clone()
            .expect("shape 1 resolves");
        assert_eq!(direct.bold, shape.is_bold());
        assert_eq!(direct.italic, shape.is_italic());
        assert!(direct.bold && direct.italic);
        assert_eq!(direct.size_pt, 12.0, "base_size 1200 HWPUNIT is 12pt");
        assert_eq!(direct.color, "#1122EE");
        assert_eq!(direct.faces[0].as_deref(), Some("함초롬바탕"));
        // The latin slot's id resolves in no font table here: null says "this document does not
        // answer for that slot", never "the default face".
        assert_eq!(direct.faces[1], None);

        // The plain shape: base_size 1000 reports exactly 10.
        let style = of(&segs, SegmentKind::Run)[0]
            .style
            .style
            .char
            .clone()
            .expect("shape 0 resolves");
        assert_eq!(style.size_pt, 10.0);
        assert!(!style.bold && !style.italic);
    }

    /// Alignment is a name, and line spacing keeps the type next to the value.
    #[test]
    fn paragraph_attributes_name_the_alignment_and_keep_the_line_spacing_pair() {
        let mut doc = doc_of(vec![para("가운데")]);
        doc.header.para_shapes = vec![ParaShape {
            attr1: 3 << 2, // alignment 3 = center
            indent: 200,
            line_spacing_type: 0,
            line_spacing: 160,
            ..Default::default()
        }];
        let (_, segs) = emit(&doc);
        let attrs = only(&segs, SegmentKind::Para)
            .style
            .direct
            .para
            .clone()
            .expect("shape 0 resolves");
        assert_eq!(attrs.alignment, "center");
        assert_eq!(attrs.indent, 200);
        assert_eq!((attrs.line_spacing_type, attrs.line_spacing), (0, 160));

        // A fixed-length spacing of the same number is a different thing, and stays distinct.
        let mut fixed = doc_of(vec![para("가운데")]);
        fixed.header.para_shapes = vec![ParaShape {
            attr1: 3 << 2,
            indent: 200,
            line_spacing_type: 1,
            line_spacing: 160,
            ..Default::default()
        }];
        let (_, fixed_segs) = emit(&fixed);
        let fixed_attrs = only(&fixed_segs, SegmentKind::Para)
            .style
            .direct
            .para
            .clone()
            .expect("shape 0 resolves");
        assert_ne!(attrs, fixed_attrs);
        assert_eq!(fixed_attrs.line_spacing_type, 1);
    }

    /// A malformed document is untrusted input: an out-of-range shape id must leave the block
    /// explicitly absent rather than panic or fabricate a default.
    #[test]
    fn an_unresolvable_shape_id_yields_an_absent_block_and_no_panic() {
        let mut p = para("잘못된 참조");
        p.char_shape_runs = vec![(0, CharShapeId(99))];
        p.para_shape = ParaShapeId(99);
        p.style = StyleId(99);
        let (_, segs) = emit(&doc_of(vec![p]));
        let seg = only(&segs, SegmentKind::Para);
        // The style level is unreachable at all: the StyleId itself does not resolve.
        assert_eq!(seg.style.style.char_shape_id, None);
        assert_eq!(seg.style.style.char, None);
        assert_eq!(seg.style.style.para, None);
        // The direct level names its ids and reports no attributes for them.
        assert_eq!(seg.style.direct.char_shape_id, Some(99));
        assert_eq!(seg.style.direct.para_shape_id, Some(99));
        assert_eq!(seg.style.direct.char, None);
        assert_eq!(seg.style.direct.para, None);
    }

    /// COLORREF is 0x00BBGGRR, not 0x00RRGGBB.
    #[test]
    fn colorref_decodes_blue_green_red() {
        assert_eq!(colorref_to_hex(0x0000_0000), "#000000");
        assert_eq!(
            colorref_to_hex(0x0000_00FF),
            "#FF0000",
            "red is the low byte"
        );
        assert_eq!(
            colorref_to_hex(0x00FF_0000),
            "#0000FF",
            "blue is the high byte"
        );
        assert_eq!(colorref_to_hex(0x00EE_2211), "#1122EE");
    }
}
