//! The v2 segment model: seven kinds, nested character ranges, two-level style (D-05, D-06a).
//!
//! # What a segment is
//!
//! One segment ties a range of the emitted markdown to the IR node that produced it. The kinds
//! are `para`, `run`, `table`, `cell`, `image`, `field` and `bookmark`. Ranges nest: a `run`
//! range lies inside its `para` range (but for the interrupted drawing-object paragraph below),
//! a `cell` range inside its `table` range, and siblings at
//! one level do not overlap. The vector is ordered by `start`, ties broken so the containing
//! segment precedes the contained one — including where the two ranges are *identical*, which
//! `end` alone cannot separate (a single-run cell is exactly its run); see [`finalize`].
//!
//! A paragraph is a paragraph wherever it lives. Paragraphs inside table cells and inside a
//! `Generic` control's paragraph lists carry their own `para` segment, not only the top-level
//! ones, or an editor could not address the paragraph in a cell.
//!
//! A paragraph that emits nothing has no `para` segment - with one exception (#285). A
//! paragraph that emits no markdown but that the renderer still draws, because it holds
//! whitespace text the markdown trims away or an object ([`always_has_para_segment`]: a table,
//! a picture, an equation, a drawing object), is published as a **point** `para` segment
//! (`start == end`) at the position its text would occupy. The render
//! layout artifact records a geometry row for such a paragraph, joined by id; an envelope with
//! no segment for it left that row's id resolving to nothing. The empty range is how the
//! envelope already spells "no text" (a bookmark), so no field changes, and the markdown does
//! not change either. Only the paragraph's OWN emission decides this: a blank line that closes
//! the list block above an empty paragraph is a separator, not that paragraph's content.
//!
//! A drawing object's paragraph that a block interrupts is a point too (#350). A table or a
//! block equation in a text box flushes the ENCLOSING paragraph's output, which resets the
//! coordinate system, so the drawing paragraph's contribution is not one range. It is one point
//! at the start of that contribution, recorded before the flush, and its runs are the one place
//! a `run` does not lie inside its `para` range.
//!
//! Offsets are **Unicode scalar** offsets into the cleaned markdown, not bytes — the same
//! discipline [`crate::markdown::to_markdown_with_segments`] (v1) already uses: byte spans are
//! recorded during emission, remapped through the cleanup deletion map, and converted to scalars
//! in one pass. A consumer can slice `md[start:end]` in Python directly.
//!
//! ## Source offsets
//!
//! A `run` segment carries a second range, [`Segment::source_chars`], and it lives in a
//! different coordinate space than `char_range`: **absolute UTF-16 code units into the source
//! paragraph**, the same unit and origin as `edit-ops-v1` `address.chars` and
//! `render-layout-v1`'s `source_chars` (#517). The two are never interchangeable and nothing
//! converts between them — `char_range` indexes the emitted markdown, emphasis markers included,
//! while `source_chars` indexes the paragraph as the document stores it.
//!
//! The point of publishing it is a round trip with no conversion on the caller's side: a
//! consumer copies `start` and `end` into a run-range op's `address.chars` and the op targets the
//! text that segment emitted. That only holds if both sides compute the bound the same way, so
//! they do: `run_wchar_bounds` is the single rule, [`crate::address`]'s `Granularity::Run`
//! resolves the whole-run bound through it and the walk publishes what it returns for the same
//! run. A second, independent derivation would let the envelope name a range its own resolver
//! then refuses as out of range.
//!
//! A fragmented run publishes **one value per contiguous piece** (see "A fragmented run" below),
//! each paired with that piece's `char_range`. The interrupting control's own characters belong
//! to no piece, so no piece's `source_chars` spans it, and the pieces of one run neither overlap
//! nor leave a gap. Inline controls — a field start/end, a bookmark — sit inside a run's source
//! span and are counted in it, exactly as `address.chars` counts them, because they are characters
//! of the paragraph. A run that emitted no source text at all (a control-only run) is not
//! published as a segment, so no empty range exists to hand to an op. The last piece of a
//! paragraph ends at the paragraph's own length, where the stored paragraph break is the
//! paragraph's final character: the same bound `run_wchar_bounds` gives the last run, so the
//! published range is always one the resolver accepts.
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
//! ## A fragmented run: the one place an id repeats
//!
//! A block control anchored inside a run — a table, a block equation — is emitted *after* the
//! paragraph's text, so the run's own emission is split in two around it. Such a run is reported
//! **once per contiguous piece, every piece carrying the run's single id**. Three properties
//! decide that, and none of the alternatives keeps all three:
//!
//! - *No emitted byte inside a paragraph belongs to no run.* Reporting only the first piece, and
//!   leaving the text after the block unattributed, breaks it — and breaks it silently, which is
//!   the failure mode this model exists to remove.
//! - *A run does not contain a table.* One segment stretched from the first piece's start to the
//!   last piece's end would swallow the block, and the table would be reported as nested inside
//!   the run rather than beside it.
//! - *The id rule does not change here.* Giving each piece its own id means extending the path
//!   with a piece index, which is a change to what a run's id is — and `hwp-render` carries a
//!   mirror of that rule pinned by a cross-crate equality test, so it is a two-crate change, not
//!   a local one.
//!
//! So the id identifies the run, not the range, and a consumer that needs the whole run groups
//! the pieces by id. This is the **only** case in which two segments share an id; 05-05's
//! envelope must say so rather than assert ids are unique.
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
use hwp_model::paragraph::HwpChar;
use hwp_model::{Control, Document, Paragraph};

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
    /// cell. Every other kind is dropped when its range collapses to nothing - except a `para`
    /// deliberately recorded as a point, which carries [`RawSeg::point`] instead (#285).
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
    /// The paragraph's own `StyleId` (an index into `DocHeader::styles`), whether or not it
    /// resolves. Not published by the segment envelope, which is closed; the `hwp styles`
    /// listing joins on it to say which paragraphs use a named style.
    pub style_id: u16,
    /// Reached through the paragraph's `StyleId` to the style's own char and para shapes.
    pub style: StyleLevel,
    /// Reached through the ids the segment itself names.
    pub direct: StyleLevel,
}

/// A half-open range in the source paragraph, in UTF-16 code units.
///
/// The same unit and origin as `edit-ops-v1` `address.chars` and `render-layout-v1`
/// `source_chars`, so a caller copies the two numbers into a run-range op's `[start, end]` with
/// no conversion (issue #517). It is deliberately NOT the same coordinate space as a segment's
/// `start`/`end`, which count Unicode scalars into the emitted markdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceChars {
    /// Absolute WCHAR offset into the source paragraph, inclusive.
    pub start: u32,
    /// Absolute WCHAR offset into the source paragraph, exclusive.
    pub end: u32,
}

/// A character control a `run` piece projected as ordinary text (#543). The projected
/// character alone cannot tell a typed space from a non-breaking one, so the envelope names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunControlKind {
    /// `TAB` (code 9, an 8-unit inline control), projected as a space in markdown.
    Tab,
    /// `HYPHEN` (code 24), projected as `-`.
    Hyphen,
    /// `NB_SPACE` (code 30), projected as a space.
    NbSpace,
    /// `FW_SPACE` (code 31), projected as a space.
    FwSpace,
}

impl RunControlKind {
    /// The wire name the envelope publishes.
    pub fn as_str(self) -> &'static str {
        match self {
            RunControlKind::Tab => "tab",
            RunControlKind::Hyphen => "hyphen",
            RunControlKind::NbSpace => "nb_space",
            RunControlKind::FwSpace => "fw_space",
        }
    }
}

/// One projected control inside a `run` piece, in both coordinate spaces the piece carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunControl {
    pub kind: RunControlKind,
    /// Unicode scalar offset of the projected character in the markdown, the space
    /// [`Segment::start`]/[`Segment::end`] index. Always inside the piece's own range.
    pub at: usize,
    /// The control's WCHAR offset in the source paragraph, the space
    /// [`Segment::source_chars`] and `edit-ops-v1` `address.chars` use.
    pub source: u32,
}

/// One emitted range, its IR coordinates, its id and its style.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub kind: SegmentKind,
    /// Derived by [`crate::segment_id`]; never read out of the source file. Unique per segment
    /// with one documented exception: the pieces of a run a block control interrupted share the
    /// run's id. See the module doc, "A fragmented run".
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
    /// The source characters this `run` piece emitted, in the source paragraph's own UTF-16
    /// coordinates. `None` on every other kind. A piece that emitted no source text is not
    /// published as a segment at all, so a `run` here always carries a non-empty range. See the
    /// module doc, "Source offsets".
    pub source_chars: Option<SourceChars>,
    /// The tab, hyphen, non-breaking and fixed-width space controls this `run` piece projected
    /// as text, in document order (#543). Empty on every other kind and on a run with none.
    pub controls: Vec<RunControl>,
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
    /// Recorded as a point on purpose - a `para` that emitted no text but is still drawn - so
    /// [`finalize`] keeps it even though its kind is dropped when a range collapses.
    pub point: bool,
    /// The source-paragraph WCHAR span this piece emitted, for a `run` piece that emitted text.
    /// Recorded in the same coordinates `address.chars` takes, so no conversion happens anywhere.
    pub source: Option<(u32, u32)>,
    /// `run` only: each projected control as (kind, byte offset of its projected character in
    /// this buffer, source WCHAR offset). The byte offset follows the same rebasing as
    /// `start`/`end`, and an entry whose character did not reach the output is dropped.
    pub controls: Vec<(RunControlKind, usize, u32)>,
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
pub(crate) fn finalize(
    raw: Vec<RawSeg>,
    mut remap: impl FnMut(usize) -> usize,
    mut survives: impl FnMut(usize) -> bool,
) -> Vec<Segment> {
    let mut out: Vec<Segment> = raw
        .into_iter()
        .filter_map(|seg| {
            let start = remap(seg.start);
            let end = remap(seg.end).max(start);
            // A control whose projected character the cleanup deleted, or that fell outside
            // the piece, is not published: `at` must index a character the piece emitted.
            let controls = seg
                .controls
                .iter()
                .filter(|&&(_, byte, _)| survives(byte))
                .map(|&(kind, byte, source)| RunControl {
                    kind,
                    at: remap(byte),
                    source,
                })
                .filter(|c| c.at >= start && c.at < end)
                .collect();
            (end > start || seg.kind.may_be_empty() || seg.point).then_some(Segment {
                kind: seg.kind,
                id: seg.id,
                path: seg.path,
                start,
                end,
                style: seg.style,
                ctrl_id: seg.ctrl_id,
                name: seg.name,
                // A zero-length source span is omitted, never published: the contract says a
                // run with no source text carries no `source_chars`, and an empty range would
                // be a second way to say the same thing.
                source_chars: seg
                    .source
                    .filter(|&(s, e)| e > s)
                    .map(|(s, e)| SourceChars { start: s, end: e }),
                controls,
            })
        })
        .collect();
    // Document order, with the container ahead of what it contains. `end` descending settles
    // the nested case; where the two ranges are *identical* - a single-run cell is exactly its
    // run - it cannot, so the depth of the positional path breaks the tie: a container's path is
    // a prefix of what it contains, so the shorter path is the outer segment.
    out.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then(b.end.cmp(&a.end))
            .then(a.path.indices.len().cmp(&b.path.indices.len()))
    });
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
        style_id: para.style.0,
        style: level(
            doc,
            style.map(|s| s.char_shape.0),
            style.map(|s| s.para_shape.0),
        ),
        direct: level(doc, char_shape_id, Some(para.para_shape.0)),
    }
}

/// Whether a paragraph always has a `para` segment: any text (a range, or a point when it is
/// only whitespace the markdown trims away), or an object the renderer draws without text - a
/// table, a picture, an equation, or a drawing object, HWP5 `gso ` or an HWPX shape or
/// container. Nothing emitted and none of these means no segment.
///
/// `hwp-render`'s `segment_map::always_has_para_segment` is a deliberate second copy of this
/// rule, deciding which geometry rows survive with nothing measured (the two crates may not
/// depend on each other); `crates/hwp-cli/tests/segment_id_parity.rs` pins them together.
pub fn always_has_para_segment(para: &Paragraph) -> bool {
    para.chars.iter().any(|ch| matches!(ch, HwpChar::Text(_)))
        || para.controls.iter().any(|control| match control {
            Control::Table(_) | Control::Picture(_) => true,
            Control::Generic(g) => {
                g.ctrl_id == *b"gso "
                    || g.container_box.is_some()
                    || !g.gso_shapes.is_empty()
                    || g.equation.is_some()
            }
            _ => false,
        })
}

/// The character shape id in force at WCHAR position `pos`, off the canonical run list.
pub(crate) fn char_shape_id_at(para: &Paragraph, pos: u32) -> Option<u16> {
    canonical_char_shape_runs(para)
        .iter()
        .rev()
        .find(|(start, _)| *start <= pos)
        .map(|(_, id)| id.0)
}

/// The `[start, end)` WCHAR bounds of canonical run `index`, or `None` when there is no such run.
///
/// ONE RULE, TWO READERS: this is the boundary `address::resolve` validates an `address.chars`
/// against, and the same boundary a `run` segment publishes as `source_chars`. If they were
/// computed twice they could drift, and a caller handing our own published range back to
/// `set_format` would be refused by the resolver — which is exactly the round trip issue #517
/// exists to make work.
pub(crate) fn run_wchar_bounds(para: &Paragraph, index: usize) -> Option<(u32, u32)> {
    let runs = canonical_char_shape_runs(para);
    let &(start, _) = runs.get(index)?;
    let end = runs
        .get(index + 1)
        .map_or_else(|| para.wchar_len(), |&(next, _)| next);
    Some((start, end))
}

pub(crate) fn level(
    doc: &Document,
    char_shape_id: Option<u16>,
    para_shape_id: Option<u16>,
) -> StyleLevel {
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

    /// One run whose emission a table interrupts, so the run is emitted as two pieces.
    fn fragmented_document() -> Document {
        let mut p = Paragraph {
            chars: "앞".chars().map(HwpChar::Text).collect(),
            char_shape_runs: vec![(0, CharShapeId(0))],
            ..Default::default()
        };
        attach(
            &mut p,
            ctrl_char::OBJECT,
            *b"tbl ",
            Control::Table(table_of(vec![cell_of(0, 0, "칸")], 1, 1)),
        );
        p.chars.extend("뒤".chars().map(HwpChar::Text));
        doc_of(vec![p])
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

    // ------------------------------------------- regressions found in review (PR #276)

    /// A bookmark alone in its paragraph must survive. It is a point marker that emits nothing,
    /// so the paragraph produces no text - and a bookmark that vanishes exactly when it is the
    /// only thing addressable in its paragraph is the case a consumer most needs.
    #[test]
    fn a_bookmark_alone_in_its_paragraph_survives() {
        let mut p = Paragraph {
            char_shape_runs: vec![(0, CharShapeId(0))],
            ..Default::default()
        };
        let mut bokm = generic(*b"bokm");
        bokm.raw_children = vec![hwp_model::opaque::OpaqueRecord {
            tag: 0x0010 + 71,
            data: crate::bookmark::make_bokm_ctrl_data("혼자"),
            children: Vec::new(),
        }];
        attach(
            &mut p,
            ctrl_char::BOOKMARK,
            *b"bokm",
            Control::Generic(bokm),
        );
        let (_, segs) = emit(&doc_of(vec![para("앞 문단"), p, para("뒤 문단")]));
        let bookmark = only(&segs, SegmentKind::Bookmark);
        assert_eq!(bookmark.name.as_deref(), Some("혼자"));
        assert_eq!(bookmark.start, bookmark.end);
    }

    /// A paragraph inside a table cell is a paragraph. Without its own `para` segment an editor
    /// cannot address it, and "seven kinds" would hold only at the top level.
    #[test]
    fn a_paragraph_inside_a_cell_gets_its_own_para_segment() {
        let mut p = para("표 앞");
        attach(
            &mut p,
            ctrl_char::OBJECT,
            *b"tbl ",
            Control::Table(table_of(
                vec![cell_of(0, 0, "머리"), cell_of(0, 1, "값")],
                2,
                1,
            )),
        );
        let (md, segs) = emit(&doc_of(vec![p]));
        let cells = of(&segs, SegmentKind::Cell);
        assert_eq!(cells.len(), 2, "{segs:#?}");
        for cell in &cells {
            let inner: Vec<&Segment> = of(&segs, SegmentKind::Para)
                .into_iter()
                .filter(|p| p.start >= cell.start && p.end <= cell.end)
                .collect();
            assert_eq!(
                inner.len(),
                1,
                "cell {:?} ({:?}) holds no paragraph of its own: {segs:#?}",
                cell,
                slice(&md, cell)
            );
        }
        // Three paragraphs in all: the top-level one plus one per cell.
        assert_eq!(of(&segs, SegmentKind::Para).len(), 3);
    }

    /// One character-shape run whose emission a block interrupts is reported once per contiguous
    /// piece, the pieces sharing the run's id - see the module doc for why the id is shared
    /// rather than split or the block swallowed.
    #[test]
    fn a_run_interrupted_by_a_block_is_reported_in_every_piece() {
        let mut p = Paragraph {
            chars: "앞".chars().map(HwpChar::Text).collect(),
            char_shape_runs: vec![(0, CharShapeId(0))],
            ..Default::default()
        };
        attach(
            &mut p,
            ctrl_char::OBJECT,
            *b"tbl ",
            Control::Table(table_of(vec![cell_of(0, 0, "칸")], 1, 1)),
        );
        p.chars.extend("뒤".chars().map(HwpChar::Text));
        let (md, segs) = emit(&doc_of(vec![p]));
        let runs: Vec<&Segment> = of(&segs, SegmentKind::Run)
            .into_iter()
            .filter(|r| r.path.indices == vec![0, 0])
            .collect();
        assert_eq!(runs.len(), 2, "both pieces of the run: {segs:#?}");
        assert_eq!(runs[0].id, runs[1].id, "one run, so one id");
        assert_eq!(slice(&md, runs[0]), "앞");
        assert_eq!(slice(&md, runs[1]), "뒤");
        // The block is not swallowed by the run that its anchor sits in.
        let table = only(&segs, SegmentKind::Table);
        assert!(runs[0].end <= table.start && runs[1].start >= table.end);
    }

    /// #517: a run publishes the source characters it emitted, in the source paragraph's own
    /// UTF-16 coordinates - the same ones `edit-ops-v1` `address.chars` takes - so a caller's
    /// round trip needs no conversion. The published span is the run's text, not the markdown it
    /// produced: a bold run's `char_range` also covers the emphasis markers.
    #[test]
    fn a_run_publishes_its_source_span() {
        let mut p = para("plain bold tail");
        p.char_shape_runs = vec![
            (0, CharShapeId(0)),
            (6, CharShapeId(1)),
            (10, CharShapeId(0)),
        ];
        let (md, segs) = emit(&doc_of(vec![p]));
        let runs = of(&segs, SegmentKind::Run);
        assert_eq!(runs.len(), 3, "{segs:#?}");
        // The three pieces tile the paragraph with no gap and no overlap.
        assert_eq!(runs[0].source_chars, Some(SourceChars { start: 0, end: 6 }));
        assert_eq!(
            runs[1].source_chars,
            Some(SourceChars { start: 6, end: 10 })
        );
        assert_eq!(
            runs[2].source_chars,
            Some(SourceChars { start: 10, end: 15 })
        );
        // The middle run's markdown is longer than its source span, because the markers belong to
        // the run's emission but to none of its source characters.
        assert_eq!(slice(&md, runs[1]), "***bold***");
    }

    /// A surrogate pair is two source units and one markdown scalar. A span computed from the
    /// markdown's scalars would name the wrong characters on any document with an emoji before
    /// the run.
    #[test]
    fn a_source_span_counts_surrogate_pairs_as_two_units() {
        let mut p = para("😀가나다");
        // 😀 is U+1F600: two UTF-16 code units, so the second run starts at 2, not at 1.
        p.char_shape_runs = vec![(0, CharShapeId(0)), (2, CharShapeId(1))];
        let (md, segs) = emit(&doc_of(vec![p]));
        let runs = of(&segs, SegmentKind::Run);
        assert_eq!(runs.len(), 2, "{segs:#?}");
        assert_eq!(runs[0].source_chars, Some(SourceChars { start: 0, end: 2 }));
        assert_eq!(runs[1].source_chars, Some(SourceChars { start: 2, end: 5 }));
        // The same run in the other space: "😀가나다" is four scalars, and the run holds three.
        assert_eq!(slice(&md, runs[1]), "***가나다***");
    }

    /// A fragmented run publishes one span per piece, and the interrupting control's own
    /// characters belong to no piece: the second piece resumes after the anchor rather than
    /// continuing where the first stopped.
    #[test]
    fn a_fragmented_run_publishes_one_source_span_per_piece() {
        let mut p = Paragraph {
            chars: "앞".chars().map(HwpChar::Text).collect(),
            char_shape_runs: vec![(0, CharShapeId(0))],
            ..Default::default()
        };
        attach(
            &mut p,
            ctrl_char::OBJECT,
            *b"tbl ",
            Control::Table(table_of(vec![cell_of(0, 0, "칸")], 1, 1)),
        );
        p.chars.extend("뒤".chars().map(HwpChar::Text));
        let (_, segs) = emit(&doc_of(vec![p]));
        let runs: Vec<&Segment> = of(&segs, SegmentKind::Run)
            .into_iter()
            .filter(|r| r.path.indices == vec![0, 0])
            .collect();
        assert_eq!(runs.len(), 2, "{segs:#?}");
        assert_eq!(runs[0].source_chars, Some(SourceChars { start: 0, end: 1 }));
        // The table anchor is an ExtCtrl, 8 WCHAR wide: 1 + 8 = 9, and the paragraph ends at 10.
        assert_eq!(
            runs[1].source_chars,
            Some(SourceChars { start: 9, end: 10 })
        );
    }

    /// A run whose piece emitted no source text is not published at all, so there is no empty
    /// range for a caller to hand to an op: `source_chars` is never zero-length anywhere. This
    /// pins the mechanism, which the contract states only as the omission.
    #[test]
    fn a_control_only_run_publishes_nothing_and_no_empty_span() {
        let mut p = Paragraph {
            char_shape_runs: vec![(0, CharShapeId(0))],
            ..Default::default()
        };
        attach(
            &mut p,
            ctrl_char::OBJECT,
            *b"tbl ",
            Control::Table(table_of(vec![cell_of(0, 0, "칸")], 1, 1)),
        );
        let control_only = doc_of(vec![p]);
        let (_, segs) = emit(&control_only);
        let outer: Vec<&Segment> = of(&segs, SegmentKind::Run)
            .into_iter()
            .filter(|r| r.path.indices == vec![0, 0])
            .collect();
        assert!(
            outer.is_empty(),
            "a run with no source text is not a segment: {outer:#?}"
        );

        // The same invariant over a document whose runs do emit text: no published run carries an
        // empty or missing span.
        let (_, rich) = emit(&seven_kind_document());
        let runs = of(&rich, SegmentKind::Run);
        assert!(!runs.is_empty(), "the document must publish runs at all");
        for run in runs {
            let chars = run
                .source_chars
                .unwrap_or_else(|| panic!("a published run must carry its span: {run:#?}"));
            assert!(
                chars.end > chars.start,
                "a published span is never zero-length: {run:#?}"
            );
        }
    }

    /// The resolver and the envelope share one rule, so every span the envelope publishes is a
    /// string `resolve` accepts at run granularity - including each piece of a fragmented run,
    /// which is a sub-range of the run it names. This is the property the published field exists
    /// for; a second derivation would let the envelope name a range its own resolver refuses.
    #[test]
    fn every_published_source_span_resolves_as_an_address_range() {
        for doc in [seven_kind_document(), fragmented_document()] {
            let (_, segs) = emit(&doc);
            let mut checked = 0;
            for seg in of(&segs, SegmentKind::Run) {
                let Some(chars) = seg.source_chars else {
                    continue;
                };
                let addr = crate::address::Address {
                    section: seg.path.section,
                    indices: seg.path.indices.clone(),
                    checksum: None,
                    chars: Some((chars.start, chars.end)),
                };
                let resolved =
                    crate::address::resolve(&doc, &addr, crate::address::Granularity::Run)
                        .unwrap_or_else(|e| {
                            panic!(
                                "the published span {chars:?} of {} was refused: {e}",
                                seg.id
                            )
                        });
                match resolved.kind {
                    crate::address::TargetKind::Run { w_start, w_end, .. } => {
                        assert_eq!((w_start, w_end), (chars.start, chars.end))
                    }
                    other => panic!("expected a run target, got {other:?}"),
                }
                checked += 1;
            }
            assert!(
                checked > 0,
                "no run published a span, so this proves nothing: {segs:#?}"
            );
        }
    }

    /// A control inside a cell must not overwrite the style captured at the outer control.
    #[test]
    fn an_outer_table_keeps_the_style_captured_at_its_own_control() {
        let mut inner = para("그림 칸");
        // The cell's paragraph names shape 1; the table's own paragraph names shape 0.
        inner.char_shape_runs = vec![(0, CharShapeId(1))];
        attach(
            &mut inner,
            ctrl_char::OBJECT,
            *b"gso ",
            Control::Picture(picture_of()),
        );
        let mut cell = cell_of(0, 0, "");
        cell.paragraphs = vec![inner];
        let mut p = para("표 앞");
        attach(
            &mut p,
            ctrl_char::OBJECT,
            *b"tbl ",
            Control::Table(table_of(vec![cell], 1, 1)),
        );
        let (_, segs) = emit(&doc_of(vec![p]));
        let table = only(&segs, SegmentKind::Table);
        assert_eq!(
            table.style.direct.char_shape_id,
            Some(0),
            "the table's style must be the one at the table control, not a nested control's"
        );
    }

    /// Identical ranges still order container first: a single-run cell produces a `cell` and a
    /// `run` over exactly the same range, and `end` descending cannot separate them.
    #[test]
    fn a_container_precedes_a_contained_segment_of_the_same_range() {
        let mut p = para("표 앞");
        attach(
            &mut p,
            ctrl_char::OBJECT,
            *b"tbl ",
            Control::Table(table_of(
                vec![cell_of(0, 0, "머리"), cell_of(0, 1, "값")],
                2,
                1,
            )),
        );
        let (_, segs) = emit(&doc_of(vec![p]));
        for pair in segs.windows(2) {
            if pair[0].start == pair[1].start && pair[0].end == pair[1].end {
                assert!(
                    pair[0].path.indices.len() < pair[1].path.indices.len(),
                    "equal ranges must order outer first: {:?} then {:?}",
                    pair[0],
                    pair[1]
                );
            }
        }
        // The pairing this guards actually occurs here.
        assert!(
            segs.windows(2)
                .any(|w| w[0].start == w[1].start && w[0].end == w[1].end),
            "expected at least one equal-range pair to exercise the tie-break: {segs:#?}"
        );
    }

    /// #285: a paragraph that emits no markdown but is drawn - a drawing object alone, or only
    /// whitespace - is a POINT `para` segment where its text would be, in the body and in a
    /// cell, and the markdown is unchanged by it.
    #[test]
    fn a_drawn_paragraph_without_text_is_a_point_para_segment() {
        let drawing = || {
            let mut p = para("");
            attach(
                &mut p,
                ctrl_char::OBJECT,
                *b"gso ",
                Control::Generic(generic(*b"gso ")),
            );
            p
        };
        let mut cell = cell_of(0, 0, "값");
        cell.paragraphs.push(drawing());
        let mut host = para("표");
        attach(
            &mut host,
            ctrl_char::OBJECT,
            *b"tbl ",
            Control::Table(table_of(vec![cell], 1, 1)),
        );
        let doc = doc_of(vec![
            para("앞 문단"),
            drawing(),
            para("   "),
            host,
            para(""),
        ]);
        let (md, segs) = emit(&doc);
        assert_eq!(
            md,
            to_markdown_with(&doc, &MarkdownOptions::default()).unwrap(),
            "a point segment changes nothing in the markdown"
        );
        let paras = of(&segs, SegmentKind::Para);
        let points: Vec<&[usize]> = paras
            .iter()
            .filter(|s| s.start == s.end)
            .map(|s| s.path.indices.as_slice())
            .collect();
        assert_eq!(
            points,
            vec![&[1][..], &[2], &[3, 0, 0, 1]],
            "the drawing, the whitespace and the drawing in a cell are points; the empty \
             paragraph draws nothing and has no segment: {segs:#?}"
        );
        let first = paras.iter().find(|s| s.path.indices == [0]).unwrap();
        let drawn = paras.iter().find(|s| s.path.indices == [1]).unwrap();
        assert!(
            drawn.start >= first.end,
            "a point sits where its text would be, after the paragraph before it"
        );
    }

    /// A drawing object carrying `lists` as its text.
    fn drawing_with(ctrl_id: [u8; 4], lists: Vec<Vec<Paragraph>>) -> Control {
        let mut g = generic(ctrl_id);
        g.paragraph_lists = lists
            .into_iter()
            .map(|paragraphs| hwp_model::ParagraphList {
                header_data: Vec::new(),
                paragraphs,
            })
            .collect();
        Control::Generic(g)
    }

    /// A paragraph holding one text box, `lists` its text.
    fn text_box_host(lists: Vec<Vec<Paragraph>>) -> Paragraph {
        let mut host = para("앞");
        attach(
            &mut host,
            ctrl_char::OBJECT,
            *b"gso ",
            drawing_with(*b"gso ", lists),
        );
        host
    }

    fn para_paths(segs: &[Segment]) -> Vec<Vec<usize>> {
        of(segs, SegmentKind::Para)
            .iter()
            .map(|s| s.path.indices.clone())
            .collect()
    }

    /// #350: a drawing object's paragraphs are numbered `[para, control, n]` across ALL of its
    /// lists, the scheme the render layout artifact's rows for them follow.
    #[test]
    fn a_drawing_objects_paragraphs_are_numbered_across_all_of_its_lists() {
        let doc = doc_of(vec![text_box_host(vec![
            vec![para("가"), para("나")],
            vec![para("다")],
        ])]);
        let (_, segs) = emit(&doc);
        assert_eq!(
            para_paths(&segs),
            vec![vec![0], vec![0, 0, 0], vec![0, 0, 1], vec![0, 0, 2]],
            "{segs:#?}"
        );
    }

    /// A table in a text box flushes the enclosing paragraph's output, so the drawing paragraph
    /// holding it is not one range. It is ONE point at the start of its contribution - never a
    /// range read off the flushed buffer, which is what the paragraph got when its text after
    /// the table was longer than its start offset.
    #[test]
    fn an_interrupted_drawing_paragraph_is_one_point_at_its_start() {
        let mut inner = para("전");
        attach(
            &mut inner,
            ctrl_char::OBJECT,
            *b"tbl ",
            Control::Table(table_of(vec![cell_of(0, 0, "칸")], 1, 1)),
        );
        inner
            .chars
            .extend("표 뒤에 오는 훨씬 긴 문장입니다".chars().map(HwpChar::Text));
        let doc = doc_of(vec![text_box_host(vec![vec![inner]])]);
        let (md, segs) = emit(&doc);
        let own: Vec<&Segment> = of(&segs, SegmentKind::Para)
            .into_iter()
            .filter(|s| s.path.indices == [0, 0, 0])
            .collect();
        assert_eq!(own.len(), 1, "one segment for the paragraph: {segs:#?}");
        assert_eq!(own[0].start, own[0].end, "a point: {:?}", own[0]);
        let from: String = md.chars().skip(own[0].start).collect();
        assert!(
            from.trim_start().starts_with("전"),
            "the point is where the paragraph's contribution starts: {from:?}"
        );
        assert_eq!(
            md,
            to_markdown_with(&doc, &MarkdownOptions::default()).unwrap(),
            "the markdown is unchanged by the point"
        );
    }

    /// A drawing paragraph whose first object is a table and which has no text before it is
    /// interrupted at once: still one point, and the table nests under its path.
    #[test]
    fn a_table_only_drawing_paragraph_is_a_point() {
        let mut inner = para("");
        attach(
            &mut inner,
            ctrl_char::OBJECT,
            *b"tbl ",
            Control::Table(table_of(vec![cell_of(0, 0, "칸")], 1, 1)),
        );
        let (_, segs) = emit(&doc_of(vec![text_box_host(vec![vec![inner]])]));
        let own: Vec<&Segment> = of(&segs, SegmentKind::Para)
            .into_iter()
            .filter(|s| s.path.indices == [0, 0, 0])
            .collect();
        assert_eq!(own.len(), 1, "{segs:#?}");
        assert_eq!(own[0].start, own[0].end);
        assert_eq!(only(&segs, SegmentKind::Table).path.indices, [0, 0, 0, 0]);
    }

    /// A text-less HWPX shape (`rect`, not `gso `) is drawn, so the paragraph holding it is a
    /// point, in the body and inside a text box alike.
    #[test]
    fn a_textless_hwpx_shape_is_a_point_para_segment() {
        let rect = || {
            let mut g = generic(*b"rect");
            g.gso_shapes.push(hwp_model::ShapeGeom {
                kind: hwp_model::ShapeKind::Rect,
                x: 0,
                y: 0,
                w: 1_000,
                h: 1_000,
                points: Vec::new(),
                fill: 0,
                fill_gradient: None,
                border_color: 0,
                border_width: 10,
                round_ratio: 0,
                border_style: 0,
                arrow_start: 0,
                arrow_end: 0,
                anchored: false,
                description: None,
                image_fill_dropped: false,
            });
            let mut p = para("");
            attach(&mut p, ctrl_char::OBJECT, *b"rect", Control::Generic(g));
            p
        };
        let doc = doc_of(vec![rect(), text_box_host(vec![vec![rect()]])]);
        let (_, segs) = emit(&doc);
        let points: Vec<Vec<usize>> = of(&segs, SegmentKind::Para)
            .iter()
            .filter(|s| s.start == s.end)
            .map(|s| s.path.indices.clone())
            .collect();
        assert_eq!(points, vec![vec![0], vec![1, 0, 0]], "{segs:#?}");
    }

    /// An empty paragraph right after a list has no `para` segment. The blank line emitted at
    /// its position closes the list block - the markdown is the same without the paragraph -
    /// so it is a separator, not the paragraph's content; it used to be published as the
    /// paragraph's whole range.
    #[test]
    fn a_list_closing_blank_line_is_not_an_empty_paragraphs_content() {
        let without = crate::from_markdown("- 항목\n\n뒤 문단\n");
        let mut with = without.clone();
        let para_shape = with.sections[0].paragraphs[1].para_shape;
        with.sections[0].paragraphs.insert(
            1,
            Paragraph {
                para_shape,
                ..Default::default()
            },
        );
        let (md, segs) = emit(&with);
        assert!(
            md.starts_with("- 항목\n\n"),
            "the fixture must open with a list block, or no separator is emitted: {md:?}"
        );
        assert_eq!(
            md,
            emit(&without).0,
            "the blank line is the list block's separator, emitted either way"
        );
        let paths: Vec<&[usize]> = of(&segs, SegmentKind::Para)
            .iter()
            .map(|s| s.path.indices.as_slice())
            .collect();
        assert_eq!(paths, vec![&[0][..], &[2]], "{segs:#?}");
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

#[cfg(test)]
mod run_control_tests {
    //! #543: the tab, hyphen, non-breaking and fixed-width space controls a run projects as
    //! text are published with their offset in both coordinate spaces.
    use super::*;
    use crate::markdown::MarkdownOptions;
    use hwp_model::ids::{BorderFillId, CharShapeId, ParaShapeId};
    use hwp_model::units::HwpUnit;
    use hwp_model::{
        Cell, CharShape, Control, Document, GenericControl, HwpChar, ParaShape, Paragraph, Section,
        Style, Table, ctrl_char,
    };

    /// `a<TAB>b<HYPHEN>c<NB_SPACE>d<FW_SPACE>e`: every control sits between ordinary text.
    fn controls_para(shape: u16) -> Paragraph {
        let mut chars = vec![HwpChar::Text('a')];
        chars.push(HwpChar::InlineCtrl {
            code: ctrl_char::TAB,
            payload: vec![0; 12],
        });
        chars.push(HwpChar::Text('b'));
        chars.push(HwpChar::CharCtrl(ctrl_char::HYPHEN));
        chars.push(HwpChar::Text('c'));
        chars.push(HwpChar::CharCtrl(ctrl_char::NB_SPACE));
        chars.push(HwpChar::Text('d'));
        chars.push(HwpChar::CharCtrl(ctrl_char::FW_SPACE));
        chars.push(HwpChar::Text('e'));
        Paragraph {
            chars,
            char_shape_runs: vec![(0, CharShapeId(shape))],
            ..Default::default()
        }
    }

    fn doc_of(paragraphs: Vec<Paragraph>) -> Document {
        let mut doc = Document {
            sections: vec![Section {
                paragraphs,
                ..Default::default()
            }],
            ..Default::default()
        };
        doc.header.char_shapes = vec![
            CharShape::default(),
            // Bold, so the emphasis close relocates whitespace around the projected spaces.
            CharShape {
                attr: 0b10,
                ..Default::default()
            },
        ];
        doc.header.para_shapes = vec![ParaShape::default()];
        doc.header.styles = vec![Style {
            name: "바탕글".into(),
            char_shape: CharShapeId(0),
            para_shape: ParaShapeId(0),
            ..Default::default()
        }];
        doc
    }

    fn emit(doc: &Document) -> (String, Vec<Segment>) {
        to_markdown_with_segments_v2(doc, &MarkdownOptions::default()).expect("no media_dir")
    }

    /// Asserts the four controls of [`controls_para`] on `run`, each `at` resolving to its
    /// projected character in `md` and each `source` to the control's WCHAR offset.
    fn assert_four(md: &str, run: &Segment) {
        let chars: Vec<char> = md.chars().collect();
        let got: Vec<(&str, char, u32)> = run
            .controls
            .iter()
            .map(|c| (c.kind.as_str(), chars[c.at], c.source))
            .collect();
        // Source offsets: a=0, TAB=1..9 (8 units), b=9, HYPHEN=10, c=11, NB=12, d=13, FW=14.
        assert_eq!(
            got,
            vec![
                ("tab", ' ', 1),
                ("hyphen", '-', 10),
                ("nb_space", ' ', 12),
                ("fw_space", ' ', 14)
            ],
            "{run:#?}\n{md:?}"
        );
        for c in &run.controls {
            assert!(
                run.start <= c.at && c.at < run.end,
                "at inside the piece: {c:?}"
            );
            let span = run.source_chars.expect("run source span");
            assert!(span.start <= c.source && c.source < span.end, "{c:?}");
        }
    }

    fn the_run(segs: &[Segment], indices: &[usize]) -> Segment {
        let runs: Vec<&Segment> = segs
            .iter()
            .filter(|s| s.kind == SegmentKind::Run && s.path.indices == indices)
            .collect();
        assert_eq!(runs.len(), 1, "{segs:#?}");
        runs[0].clone()
    }

    #[test]
    fn a_top_level_run_publishes_each_projected_control() {
        let (md, segs) = emit(&doc_of(vec![controls_para(0)]));
        assert_four(&md, &the_run(&segs, &[0, 0]));
    }

    /// Bold closes by moving whitespace outside its markers; the controls follow it.
    #[test]
    fn a_bold_run_keeps_each_control_on_its_projected_character() {
        let (md, segs) = emit(&doc_of(vec![controls_para(1)]));
        assert!(md.contains("**"), "{md}");
        assert_four(&md, &the_run(&segs, &[0, 0]));
    }

    /// A bold run that starts and ends with a projected space: the emphasis close moves both
    /// spaces outside its markers, and each control follows its own character.
    #[test]
    fn a_control_relocated_outside_bold_markers_keeps_its_offset() {
        let p = Paragraph {
            chars: vec![
                HwpChar::Text('x'),
                HwpChar::CharCtrl(ctrl_char::FW_SPACE),
                HwpChar::Text('a'),
                HwpChar::CharCtrl(ctrl_char::NB_SPACE),
                HwpChar::Text('y'),
            ],
            // x plain, FW_SPACE..NB_SPACE bold, y plain.
            char_shape_runs: vec![
                (0, CharShapeId(0)),
                (1, CharShapeId(1)),
                (4, CharShapeId(0)),
            ],
            ..Default::default()
        };
        let (md, segs) = emit(&doc_of(vec![p]));
        assert!(
            md.contains(" **a** "),
            "whitespace moved outside the markers: {md:?}"
        );
        let chars: Vec<char> = md.chars().collect();
        let bold = the_run(&segs, &[0, 1]);
        let got: Vec<(&str, char, u32)> = bold
            .controls
            .iter()
            .map(|c| (c.kind.as_str(), chars[c.at], c.source))
            .collect();
        assert_eq!(
            got,
            vec![("fw_space", ' ', 1), ("nb_space", ' ', 3)],
            "{bold:#?} {md:?}"
        );
        for c in &bold.controls {
            assert!(bold.start <= c.at && c.at < bold.end, "{c:?} {bold:?}");
        }
    }

    #[test]
    fn a_run_without_controls_publishes_none() {
        let p = Paragraph {
            chars: "a b-c".chars().map(HwpChar::Text).collect(),
            char_shape_runs: vec![(0, CharShapeId(0))],
            ..Default::default()
        };
        let (_, segs) = emit(&doc_of(vec![p]));
        assert!(segs.iter().all(|s| s.controls.is_empty()), "{segs:#?}");
    }

    fn host(control: Control, ctrl_id: [u8; 4]) -> Paragraph {
        let mut host = Paragraph {
            chars: vec![HwpChar::Text('x')],
            char_shape_runs: vec![(0, CharShapeId(0))],
            ..Default::default()
        };
        host.controls.push(control);
        host.chars.push(HwpChar::ExtCtrl {
            code: ctrl_char::OBJECT,
            ctrl_id,
            payload: Vec::new(),
            ctrl_index: Some(0),
        });
        host
    }

    #[test]
    fn a_run_inside_a_table_cell_publishes_its_controls() {
        let cell = Cell {
            list_attr: 0,
            col: 0,
            row: 0,
            col_span: 1,
            row_span: 1,
            width: HwpUnit(1000),
            height: HwpUnit(1000),
            margins: [0; 4],
            border_fill: BorderFillId::default(),
            header_tail: Vec::new(),
            paragraphs: vec![controls_para(0)],
        };
        let table = Table {
            common_data: Vec::new(),
            placement: None,
            attr: 0,
            rows: 1,
            cols: 1,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: vec![1],
            border_fill: BorderFillId::default(),
            table_tail: Vec::new(),
            cells: vec![cell],
            caption: None,
            extras: Vec::new(),
        };
        let (md, segs) = emit(&doc_of(vec![host(Control::Table(table), *b"tbl ")]));
        let run = segs
            .iter()
            .find(|s| s.kind == SegmentKind::Run && !s.controls.is_empty())
            .unwrap_or_else(|| panic!("cell run with controls: {segs:#?}"));
        assert!(run.path.indices.len() > 2, "{run:?}");
        assert_four(&md, run);
    }

    #[test]
    fn a_run_inside_a_text_box_publishes_its_controls() {
        let g = GenericControl {
            ctrl_id: *b"gso ",
            data: Vec::new(),
            paragraph_lists: vec![hwp_model::ParagraphList {
                header_data: Vec::new(),
                paragraphs: vec![controls_para(0)],
            }],
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        };
        let (md, segs) = emit(&doc_of(vec![host(Control::Generic(g), *b"gso ")]));
        let run = segs
            .iter()
            .find(|s| s.kind == SegmentKind::Run && !s.controls.is_empty())
            .unwrap_or_else(|| panic!("text-box run with controls: {segs:#?}"));
        assert!(run.path.indices.len() > 2, "{run:?}");
        assert_four(&md, run);
    }
}
