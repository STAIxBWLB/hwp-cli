//! Stable identifiers for the segments `--with-segments` emits.
//!
//! # What an id is for
//!
//! An editor keys its boxes by segment id and persists those keys across sessions. The id has
//! to survive a reload, agree between a `.hwp` and a `.hwpx` carrying the same content (D-02),
//! and be identical under `--format markdown` and `--format json` (D-03). So it is *derived*
//! from the IR, never read out of the source file: HWPX's OWPML element ids are not preserved
//! through the IR, and using them would make the two readers disagree.
//!
//! # Shape
//!
//! `<checksum>.<section>.<paragraph>[.<child index>...]` — a content checksum joined to a
//! positional path, checksum first (D-01). The checksum covers the segment's own source
//! content: its text and the shape ids it names, not the emitted markdown, which is what makes
//! the id the same across output formats. The path is positional and carries no hash.
//!
//! The separator is `.` and **never** `:`. hwp-editor's `ops.ts` already documents a
//! `:`-separator ambiguity that Phase 6 exists to remove; the new id format must not re-import
//! it. Path components are decimal indices and the checksum is lowercase hex, so `.` never
//! appears inside a component.
//!
//! # Checksum width
//!
//! SHA-256 truncated to 16 hex characters = 64 bits. Birthday arithmetic: with `n` segments the
//! collision probability is about `n^2 / 2^65`, so at 10,000 segments it is roughly
//! `10^8 / 3.7e19` ≈ `2.7e-12`, comfortably below 10^-6. Eight hex characters (32 bits) would
//! sit near 1 percent at the same size, which is why the width is 16 rather than chosen by
//! preference.
//!
//! # What the checksum is not
//!
//! The checksum detects *accidental* drift — an insertion above a segment renames its path
//! while its content checksum stays put, which is how a stale id is recognised. It is **not**
//! tamper-evidence. A truncated hash over public content is not a MAC and must never be
//! presented as one; anyone who can change the content can recompute the id.
//!
//! # The mirror implementation
//!
//! `crates/hwp-render/src/segment_id.rs` carries a second, independent copy of this rule with
//! the identical signatures, because `hwp-render` and `hwp-convert` may not depend on each
//! other (AGENTS.md invariant 1). The two are kept in step by the cross-crate per-kind equality
//! test in `crates/hwp-cli/tests/` — that test is the *only* thing standing between the two
//! crates and a silent divergence, so it is the one to look at first if a reader wonders why
//! one rule exists twice.
//!
//! # The rule: hash the semantic content, canonicalized; never the storage form
//!
//! This is the one sentence to carry over to the mirror. Everything below follows from it.
//!
//! Nothing that differs between the hwp5 and hwpx readers for the same content is hashed:
//! `common_data`, `table_tail`, `header_tail`, control payload bytes and `BinRef` (an hwp5
//! table index on one side, an hwpx manifest string on the other) all stay out, or the same
//! document saved in the two formats would produce two different ids.
//!
//! Excluding fields is not enough on its own, because the two readers also store *equivalent*
//! content in different shapes. Two such shapes are normalized before hashing:
//!
//! - [`canonical_char_shape_runs`] collapses a run whose shape id repeats the previous one and
//!   keeps only the last run at any one position. HWPX already stores that form; HWP5 keeps
//!   every `PARA_CHAR_SHAPE` entry.
//! - `canonical_chars` drops the trailing `CharCtrl(13)` paragraph-break terminator. HWP5's
//!   `PARA_TEXT` stores one; HWPX's `<hp:t>` does not.
//!
//! Neither reader is wrong - each is faithful to its own format - so the normalization lives
//! here, next to the hashing. Changing a reader to suit the id rule would move a
//! format-fidelity decision into a naming concern.
//!
//! The same rule cuts the other way for images: `picture_id` hashes the resolved image
//! *payload* rather than finalizing from metadata, because the payload is the content and the
//! metadata is only a description of it. Without it, an image replaced in place at the same
//! size keeps its id.

use hwp_model::control::{Cell, GenericControl, Picture, Table};
use hwp_model::ids::CharShapeId;
use hwp_model::paragraph::{HwpChar, Paragraph, ctrl_char};
use sha2::{Digest as _, Sha256};

/// Hex characters kept from the SHA-256 digest. See the module doc for the arithmetic.
const CHECKSUM_HEX_LEN: usize = 16;

/// The positional half of a segment id: a section plus the chain of child indices that reaches
/// the segment, starting with the paragraph index.
///
/// The fields are public and there is no constructor, so a caller builds a path as it walks the
/// document without going through this module. That is safe: both this crate and the
/// `hwp-render` mirror key the path off the same raw IR indices. The checksum *input* is the
/// half that must not be left to callers, which is why every entry point below takes the IR
/// value itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SegmentPath {
    /// Section index in `Document::sections`.
    pub section: usize,
    /// Child indices from the section down to the segment; `indices[0]` is the paragraph.
    pub indices: Vec<usize>,
}

impl std::fmt::Display for SegmentPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.section)?;
        for index in &self.indices {
            write!(f, ".{index}")?;
        }
        Ok(())
    }
}

/// The paragraph's character-shape runs in canonical form: the run boundary list both readers
/// agree on semantically.
///
/// The HWPX reader already stores this form - it suppresses a run whose shape id repeats the
/// previous one and overwrites a run at the same WCHAR position
/// (`crates/hwpx/src/read/section.rs`). The HWP5 reader preserves every `PARA_CHAR_SHAPE`
/// entry verbatim (`crates/hwp5/src/body_text.rs`), redundant ones included. Both readers are
/// right about their own format; normalizing here is what lets one document derive one set of
/// ids whichever reader produced the IR.
///
/// Callers that emit run segments must number their runs off **this** list, not off
/// `Paragraph::char_shape_runs`, or the run index in the path would itself be format-dependent.
pub fn canonical_char_shape_runs(paragraph: &Paragraph) -> Vec<(u32, CharShapeId)> {
    let mut out: Vec<(u32, CharShapeId)> = Vec::with_capacity(paragraph.char_shape_runs.len());
    for &(at, shape) in &paragraph.char_shape_runs {
        if out.last().is_some_and(|&(_, last)| last == shape) {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.0 == at => last.1 = shape,
            _ => out.push((at, shape)),
        }
    }
    out
}

/// The paragraph's characters in canonical form: without the trailing paragraph-break
/// terminator.
///
/// HWP5's `PARA_TEXT` ends a paragraph with `CharCtrl(13)`; HWPX's `<hp:t>` does not store one.
/// That terminator is storage framing, not content, so the same document read from the two
/// formats would otherwise hash different character lists - and it sits at the end, so it would
/// also lengthen the last run's slice on one side only.
fn canonical_chars(paragraph: &Paragraph) -> &[HwpChar] {
    match paragraph.chars.last() {
        Some(HwpChar::CharCtrl(code)) if *code == ctrl_char::PARA_BREAK => {
            &paragraph.chars[..paragraph.chars.len() - 1]
        }
        _ => &paragraph.chars,
    }
}

/// The id of a paragraph segment (`kind: "para"`).
pub fn paragraph_id(path: &SegmentPath, paragraph: &Paragraph) -> String {
    let mut hasher = new_hasher("para");
    hash_paragraph(&mut hasher, paragraph);
    join(hasher, path, None)
}

/// The id of one character-shape run inside `paragraph` (`kind: "run"`).
///
/// `run_index` indexes `Paragraph::char_shape_runs`, which *is* the run boundary list. The
/// path is extended with that index, so two runs of one paragraph never collide even when they
/// carry identical text under identical shapes. An out-of-range index hashes an empty slice
/// rather than panicking, because the walk that produces it is the caller's.
pub fn run_id(path: &SegmentPath, paragraph: &Paragraph, run_index: usize) -> String {
    let mut hasher = new_hasher("run");
    hasher.update((paragraph.para_shape.0).to_le_bytes());
    hasher.update((paragraph.style.0).to_le_bytes());
    let runs = canonical_char_shape_runs(paragraph);
    match runs.get(run_index) {
        Some(&(start, shape)) => {
            let end = runs.get(run_index + 1).map_or(u32::MAX, |&(next, _)| next);
            hasher.update(shape.0.to_le_bytes());
            hash_chars(&mut hasher, run_chars(paragraph, start, end));
        }
        None => {
            hasher.update(u16::MAX.to_le_bytes());
            hash_chars(&mut hasher, std::iter::empty());
        }
    }
    join(hasher, path, Some(run_index))
}

/// The id of a table segment (`kind: "table"`).
pub fn table_id(path: &SegmentPath, table: &Table) -> String {
    let mut hasher = new_hasher("table");
    hasher.update(table.rows.to_le_bytes());
    hasher.update(table.cols.to_le_bytes());
    hasher.update((table.row_cell_counts.len() as u64).to_le_bytes());
    for count in &table.row_cell_counts {
        hasher.update(count.to_le_bytes());
    }
    hasher.update((table.cells.len() as u64).to_le_bytes());
    for cell in &table.cells {
        hash_cell(&mut hasher, cell);
    }
    join(hasher, path, None)
}

/// The id of a table cell segment (`kind: "cell"`).
pub fn cell_id(path: &SegmentPath, cell: &Cell) -> String {
    let mut hasher = new_hasher("cell");
    hash_cell(&mut hasher, cell);
    join(hasher, path, None)
}

/// The id of an image segment (`kind: "image"`).
///
/// `image_data` is the picture's resolved payload, which the caller obtains with
/// `Document::resolve_bin(&picture.bin_ref)`; pass `None` when it cannot be resolved. The bytes
/// have to come in from outside because `Picture` holds only a *reference* to them, and
/// resolving it here would mean traversing the `Document` - which these entry points must not
/// do. This is the one place a caller hands over content, and even here the module decides the
/// framing and everything else that is hashed.
///
/// The payload is hashed and `BinRef` is not, and that ordering is the point: `BinRef` is an
/// hwp5 BinData index on one side and an hwpx manifest item string on the other, so hashing it
/// would split the id by source format, while the bytes themselves are the same in both. And
/// without the bytes an image swapped in place at the same size and placement would keep its
/// id, so a persisted editor key would silently be accepted for a different image - exactly the
/// drift the checksum exists to catch.
pub fn picture_id(path: &SegmentPath, picture: &Picture, image_data: Option<&[u8]>) -> String {
    let mut hasher = new_hasher("image");
    hasher.update(picture.width.0.to_le_bytes());
    hasher.update(picture.height.0.to_le_bytes());
    hasher.update([u8::from(picture.treat_as_char)]);
    hasher.update(picture.z_order.to_le_bytes());
    hasher.update(picture.vert_offset.to_le_bytes());
    hasher.update(picture.horz_offset.to_le_bytes());
    // Semantic transforms: two segments showing the same bytes differently are not the same
    // segment. All format-neutral - the readers agree on them.
    hasher.update([picture.flip]);
    hasher.update(picture.brightness.to_le_bytes());
    hasher.update(picture.contrast.to_le_bytes());
    hasher.update(picture.rotation.unwrap_or(0.0).to_bits().to_le_bytes());
    match picture.crop {
        Some(crop) => {
            hasher.update([1u8]);
            for edge in crop {
                hasher.update(edge.to_bits().to_le_bytes());
            }
        }
        None => hasher.update([0u8]),
    }
    hash_field(
        &mut hasher,
        picture.description.as_deref().unwrap_or("").as_bytes(),
    );
    match image_data {
        Some(data) => {
            hasher.update([1u8]);
            hash_field(&mut hasher, data);
        }
        None => hasher.update([0u8]),
    }
    join(hasher, path, None)
}

/// The id of a segment backed by a generic control: the `kind: "field"` and `kind: "bookmark"`
/// segments, which the envelope discriminates by `ctrl_id`.
///
/// The control's raw `data` payload is excluded — it is the hwp5 CTRL_HEADER byte block and is
/// empty for hwpx-sourced controls. The `ctrl_id` plus the control's own paragraph text is what
/// both readers agree on.
pub fn control_id(path: &SegmentPath, control: &GenericControl) -> String {
    let mut hasher = new_hasher("ctrl");
    hash_field(&mut hasher, &control.ctrl_id);
    hasher.update((control.paragraph_lists.len() as u64).to_le_bytes());
    for list in &control.paragraph_lists {
        hasher.update((list.paragraphs.len() as u64).to_le_bytes());
        for paragraph in &list.paragraphs {
            hash_paragraph(&mut hasher, paragraph);
        }
    }
    join(hasher, path, None)
}

fn new_hasher(kind: &str) -> Sha256 {
    let mut hasher = Sha256::new();
    hasher.update(b"hwp-segment-id-v1\0");
    hash_field(&mut hasher, kind.as_bytes());
    hasher
}

/// Finish the checksum and join it to the path: `<checksum>.<section>.<paragraph>[...]`.
fn join(hasher: Sha256, path: &SegmentPath, extra: Option<usize>) -> String {
    let digest = hasher.finalize();
    let mut id = String::with_capacity(CHECKSUM_HEX_LEN + 8);
    for byte in digest.iter().take(CHECKSUM_HEX_LEN / 2) {
        id.push_str(&format!("{byte:02x}"));
    }
    id.push('.');
    id.push_str(&path.to_string());
    if let Some(index) = extra {
        id.push('.');
        id.push_str(&index.to_string());
    }
    id
}

/// Length-prefixed so that concatenating two fields can never look like a third.
fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value);
}

fn hash_paragraph(hasher: &mut Sha256, paragraph: &Paragraph) {
    hasher.update(paragraph.para_shape.0.to_le_bytes());
    hasher.update(paragraph.style.0.to_le_bytes());
    let runs = canonical_char_shape_runs(paragraph);
    hasher.update((runs.len() as u64).to_le_bytes());
    for &(start, shape) in &runs {
        hasher.update(start.to_le_bytes());
        hasher.update(shape.0.to_le_bytes());
    }
    hash_chars(hasher, canonical_chars(paragraph).iter());
}

fn hash_cell(hasher: &mut Sha256, cell: &Cell) {
    hasher.update(cell.col.to_le_bytes());
    hasher.update(cell.row.to_le_bytes());
    hasher.update(cell.col_span.to_le_bytes());
    hasher.update(cell.row_span.to_le_bytes());
    hasher.update((cell.paragraphs.len() as u64).to_le_bytes());
    for paragraph in &cell.paragraphs {
        hash_paragraph(hasher, paragraph);
    }
}

/// Hash the characters themselves. Control payload bytes are skipped: they are hwp5 round-trip
/// material and are empty on the hwpx side for the same content.
fn hash_chars<'a>(hasher: &mut Sha256, chars: impl Iterator<Item = &'a HwpChar>) {
    for ch in chars {
        match ch {
            HwpChar::Text(c) => {
                hasher.update([0u8]);
                hasher.update((*c as u32).to_le_bytes());
            }
            HwpChar::CharCtrl(code) => {
                hasher.update([1u8]);
                hasher.update(code.to_le_bytes());
            }
            HwpChar::InlineCtrl { code, .. } => {
                hasher.update([2u8]);
                hasher.update(code.to_le_bytes());
            }
            HwpChar::ExtCtrl { code, ctrl_id, .. } => {
                hasher.update([3u8]);
                hasher.update(code.to_le_bytes());
                hasher.update(ctrl_id);
            }
        }
    }
    hasher.update([0xffu8]);
}

/// The characters of `paragraph` whose WCHAR offsets fall in `[start, end)`.
fn run_chars(paragraph: &Paragraph, start: u32, end: u32) -> impl Iterator<Item = &HwpChar> {
    let mut offset = 0u32;
    canonical_chars(paragraph).iter().filter(move |ch| {
        let at = offset;
        offset += ch.wchar_width();
        at >= start && at < end
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwp_model::ids::{CharShapeId, ParaShapeId};

    fn path(section: usize, indices: &[usize]) -> SegmentPath {
        SegmentPath {
            section,
            indices: indices.to_vec(),
        }
    }

    fn sample_picture() -> Picture {
        Picture {
            common_data: Vec::new(),
            width: hwp_model::units::HwpUnit(100),
            height: hwp_model::units::HwpUnit(200),
            treat_as_char: true,
            z_order: 0,
            vert_offset: 0,
            horz_offset: 0,
            description: Some("그림".into()),
            crop: None,
            flip: 0,
            rotation: None,
            brightness: 0,
            contrast: 0,
            effect_flags: 0,
            effects_raw: Vec::new(),
            caption: None,
            bin_ref: hwp_model::control::BinRef::Id(Default::default()),
            extras: Vec::new(),
        }
    }

    fn para(text: &str, shape: u16) -> Paragraph {
        Paragraph {
            chars: text.chars().map(HwpChar::Text).collect(),
            char_shape_runs: vec![(0, CharShapeId(shape))],
            ..Default::default()
        }
    }

    /// Identical content at the same path derives the same id, twice over.
    #[test]
    fn same_content_at_same_path_is_stable_across_derivations() {
        let p = path(0, &[3]);
        let a = para("예시 문단", 1);
        let b = para("예시 문단", 1);
        assert_eq!(paragraph_id(&p, &a), paragraph_id(&p, &a));
        assert_eq!(paragraph_id(&p, &a), paragraph_id(&p, &b));
    }

    /// The same text under a different CharShapeId is a different segment.
    #[test]
    fn different_char_shape_changes_the_id() {
        let p = path(0, &[3]);
        assert_ne!(
            paragraph_id(&p, &para("예시 문단", 1)),
            paragraph_id(&p, &para("예시 문단", 2)),
        );
    }

    /// The path is part of the id, so the same paragraph elsewhere is a different segment.
    #[test]
    fn same_paragraph_at_a_different_path_is_a_different_id() {
        let p = para("예시 문단", 1);
        assert_ne!(
            paragraph_id(&path(0, &[3]), &p),
            paragraph_id(&path(0, &[4]), &p)
        );
        assert_ne!(
            paragraph_id(&path(0, &[3]), &p),
            paragraph_id(&path(1, &[3]), &p)
        );
    }

    /// A run extends the paragraph path with its index and hashes its own slice, so two runs
    /// of one paragraph never collide — including two runs carrying the same characters.
    #[test]
    fn runs_of_one_paragraph_do_not_collide() {
        let mut p = para("가나다라", 1);
        // "가나" under shape 1, "다라" under shape 2.
        p.char_shape_runs = vec![(0, CharShapeId(1)), (2, CharShapeId(2))];
        let at = path(0, &[3]);
        let first = run_id(&at, &p, 0);
        let second = run_id(&at, &p, 1);
        assert_ne!(first, second);
        assert_ne!(first, paragraph_id(&at, &p));

        // Same characters in both runs, same shape: still distinct, because the path differs.
        let mut same = para("가가", 1);
        same.char_shape_runs = vec![(0, CharShapeId(1)), (1, CharShapeId(1))];
        assert_ne!(run_id(&at, &same, 0), run_id(&at, &same, 1));

        // The slice really is the run's own: changing only the second run moves only its id.
        let mut edited = p.clone();
        edited.chars[3] = HwpChar::Text('마');
        assert_eq!(run_id(&at, &p, 0), run_id(&at, &edited, 0));
        assert_ne!(run_id(&at, &p, 1), run_id(&at, &edited, 1));
    }

    /// All seven envelope kinds are reachable and no two of them agree at the same path.
    #[test]
    fn every_envelope_kind_has_an_entry_point() {
        let at = path(0, &[0]);
        let cell = Cell {
            list_attr: 0,
            col: 0,
            row: 0,
            col_span: 1,
            row_span: 1,
            width: hwp_model::units::HwpUnit(100),
            height: hwp_model::units::HwpUnit(100),
            margins: [0; 4],
            border_fill: Default::default(),
            header_tail: Vec::new(),
            paragraphs: vec![para("셀", 1)],
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
            border_fill: Default::default(),
            table_tail: Vec::new(),
            cells: vec![cell.clone()],
            caption: None,
            extras: Vec::new(),
        };
        let picture = sample_picture();
        let generic = |ctrl_id: [u8; 4]| GenericControl {
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
        };
        let field = generic(*b"%clk");
        let bookmark = generic(*b"bokm");

        let ids = vec![
            paragraph_id(&at, &para("문단", 1)),
            run_id(&at, &para("문단", 1), 0),
            table_id(&at, &table),
            cell_id(&at, &cell),
            picture_id(&at, &picture, Some(b"image-bytes")),
            control_id(&at, &field),
            control_id(&at, &bookmark),
        ];
        assert_eq!(ids.len(), 7);
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            7,
            "kinds must not collide at one path: {ids:?}"
        );
    }

    /// Redundant runs are storage form, not content: the HWP5 reader keeps them and the HWPX
    /// reader does not, so hashing the raw list would split the id by source format.
    #[test]
    fn redundant_and_same_position_runs_are_canonicalized_away() {
        let at = path(0, &[0]);
        let mut plain = para("가나다라", 1);
        plain.char_shape_runs = vec![(0, CharShapeId(1)), (2, CharShapeId(2))];

        // A repeated shape id, as the HWP5 reader would store it.
        let mut redundant = plain.clone();
        redundant.char_shape_runs = vec![
            (0, CharShapeId(1)),
            (1, CharShapeId(1)),
            (2, CharShapeId(2)),
        ];
        // Two runs at one position: the last one wins.
        let mut overwritten = plain.clone();
        overwritten.char_shape_runs = vec![
            (0, CharShapeId(9)),
            (0, CharShapeId(1)),
            (2, CharShapeId(2)),
        ];

        assert_eq!(canonical_char_shape_runs(&plain).len(), 2);
        assert_eq!(
            canonical_char_shape_runs(&redundant),
            canonical_char_shape_runs(&plain)
        );
        assert_eq!(
            canonical_char_shape_runs(&overwritten),
            canonical_char_shape_runs(&plain)
        );
        assert_eq!(paragraph_id(&at, &redundant), paragraph_id(&at, &plain));
        assert_eq!(paragraph_id(&at, &overwritten), paragraph_id(&at, &plain));
        // Run numbering follows the canonical list, so run 1 means the same run on both sides.
        assert_eq!(run_id(&at, &redundant, 1), run_id(&at, &plain, 1));
    }

    /// HWP5's PARA_TEXT ends a paragraph with CharCtrl(13); HWPX stores no such terminator.
    /// It is framing, not content.
    #[test]
    fn the_trailing_paragraph_break_is_not_content() {
        let at = path(0, &[0]);
        let without = para("문단", 1);
        let mut with = without.clone();
        with.chars.push(HwpChar::CharCtrl(ctrl_char::PARA_BREAK));
        assert_eq!(paragraph_id(&at, &with), paragraph_id(&at, &without));
        // It is only the *trailing* one: a line break inside the text is real content.
        let mut inner = without.clone();
        inner
            .chars
            .insert(1, HwpChar::CharCtrl(ctrl_char::LINE_BREAK));
        assert_ne!(paragraph_id(&at, &inner), paragraph_id(&at, &without));
    }

    /// An image swapped in place must not keep its id. Metadata alone cannot tell the two
    /// apart, which is why the resolved payload is hashed.
    #[test]
    fn an_image_id_follows_the_image_bytes_not_only_its_metadata() {
        let at = path(0, &[0]);
        let picture = sample_picture();
        let red = b"\x89PNG-red-pixels".as_slice();
        let blue = b"\x89PNG-blue-pixel".as_slice();
        assert_eq!(red.len(), blue.len(), "same size, different bytes");

        assert_ne!(
            picture_id(&at, &picture, Some(red)),
            picture_id(&at, &picture, Some(blue)),
            "same dimensions and placement, different bytes: ids must differ"
        );
        assert_eq!(
            picture_id(&at, &picture, Some(red)),
            picture_id(&at, &picture, Some(red)),
        );
        assert_ne!(
            picture_id(&at, &picture, Some(red)),
            picture_id(&at, &picture, None),
            "an unresolvable payload is not the same as a resolved one"
        );

        // Changes confined to the semantic transforms move the id too.
        for tweak in [
            |p: &mut Picture| p.flip = 1,
            |p: &mut Picture| p.rotation = Some(90.0),
            |p: &mut Picture| p.brightness = 20,
            |p: &mut Picture| p.contrast = -20,
            |p: &mut Picture| p.crop = Some([0.0, 0.0, 0.5, 0.5]),
        ] {
            let mut other = picture.clone();
            tweak(&mut other);
            assert_ne!(
                picture_id(&at, &picture, Some(red)),
                picture_id(&at, &other, Some(red)),
            );
        }
    }

    /// The separator is `.`, never `:`, and the checksum is 16 hex characters. Asserted on a
    /// derived id's shape rather than on a frozen literal, so the width can change here
    /// without rewriting every test.
    #[test]
    fn id_shape_is_hex_checksum_then_dotted_path_and_never_colon() {
        let id = paragraph_id(&path(2, &[7]), &para("문단", 1));
        assert!(!id.contains(':'), "the separator must not be ':': {id}");
        let (checksum, rest) = id.split_once('.').expect("checksum then path");
        assert_eq!(checksum.len(), CHECKSUM_HEX_LEN, "checksum width: {id}");
        assert!(
            checksum
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "checksum must be lowercase hex: {id}"
        );
        assert_eq!(rest, "2.7", "path is section then child indices: {id}");
    }

    /// The same-IR property behind D-02: two `Document` values built to be equal derive equal
    /// ids, whichever reader produced them. The end-to-end hwp5-vs-hwpx assertion belongs to
    /// 05-05, which has both readers in scope.
    #[test]
    fn equal_ir_derives_equal_ids_whatever_the_source_format() {
        let build = || {
            let mut p = para("동일 내용", 4);
            p.para_shape = ParaShapeId(2);
            p
        };
        let at = path(1, &[5]);
        // Two independently built, equal IR values stand in for the two readers' output.
        let (a, b) = (build(), build());
        assert_eq!(a, b);
        assert_eq!(paragraph_id(&at, &a), paragraph_id(&at, &b));
        assert_eq!(run_id(&at, &a, 0), run_id(&at, &b, 0));
    }
}
