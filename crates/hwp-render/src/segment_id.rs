//! The geometry side's copy of the segment id rule — a deliberate second implementation.
//!
//! # Why this file exists twice
//!
//! `crates/hwp-convert/src/segment_id.rs` carries the original. `hwp-render` may not depend on
//! `hwp-convert` and `hwp-convert` may not depend on `hwp-render` (CLAUDE.md invariant 1: the
//! IR is the hub and the two spokes never touch), and the owner chose duplication over moving
//! the rule into a shared crate. So there is no shared code path here to fall back on, and
//! **nothing at compile time makes the two copies agree**.
//!
//! What does: `crates/hwp-cli/tests/segment_id_parity.rs`, the per-kind cross-crate equality
//! test in the only crate that depends on both. Read it before changing anything below. A
//! reader who finds two copies and no explanation deletes one; that test is the explanation.
//!
//! # The rule: hash the semantic content, canonicalized; never the storage form
//!
//! This is the one sentence the mirror inherits. Everything below follows from it.
//!
//! Nothing that differs between the hwp5 and hwpx readers for the same content is hashed:
//! `common_data`, `table_tail`, `header_tail`, control payload bytes and `BinRef` (an hwp5
//! BinData index on one side, an hwpx manifest item string on the other) all stay out, or the
//! same document saved in the two formats would produce two different ids.
//!
//! Excluding fields is not enough on its own, because the two readers also store *equivalent*
//! content in different shapes. Two such shapes are normalized before hashing, and both
//! normalizations live here, next to the hashing, never in a reader — each reader is faithful
//! to its own format and neither is wrong:
//!
//! - [`canonical_char_shape_runs`] collapses a run whose shape id repeats the previous one and
//!   keeps only the last run at any one position. The HWPX reader already stores that form
//!   (`crates/hwpx/src/read/section.rs` suppresses the repeat and overwrites at the same WCHAR
//!   position); the HWP5 reader keeps every `PARA_CHAR_SHAPE` entry, redundant ones included.
//!   [`run_id`] therefore numbers runs off the **canonical** list: indexing the raw list would
//!   make the run index inside the path itself format-dependent, and the canonicalization would
//!   buy nothing.
//! - `canonical_chars` drops the trailing `CharCtrl(13)` paragraph-break terminator. **HWP5's
//!   `PARA_TEXT` stores one; HWPX's `<hp:t>` does not.** It is storage framing, not content, and
//!   it sits at the end, so it would also lengthen the last run's slice on one side only. This
//!   is not dead code: delete it and the same document read from the two formats derives two
//!   different ids.
//!
//! The same rule cuts the other way for images: [`picture_id`] hashes the resolved image
//! *payload* rather than finishing from metadata, because the payload is the content and the
//! metadata is only a description of it. Without it, an image replaced in place at the same size
//! and placement would keep its id. The payload comes in as an argument because `Picture` holds
//! only a `BinRef`; this crate's layout pass already resolves it with
//! `Document::resolve_bin(&picture.bin_ref)` at the picture layout site, and that same
//! `Option<&[u8]>` is what gets passed in. There is no second resolution path and this module
//! never reaches for the `Document`.
//!
//! # Shape
//!
//! `<checksum>.<section>.<paragraph>[.<child index>...]` — a 16-hex-character SHA-256 prefix
//! joined to a positional path. The separator is `.` and never `:`. See the original module's
//! doc comment for the collision arithmetic behind the 16-character width and for why the
//! checksum is drift detection, not tamper evidence.
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
/// `hwp-convert` original key the path off the same raw IR indices. The checksum *input* is the
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
/// The HWPX reader already stores this form — it suppresses a run whose shape id repeats the
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
/// `run_index` indexes [`canonical_char_shape_runs`], **not** `Paragraph::char_shape_runs`:
/// numbering off the raw list would make the run index in the path format-dependent. The
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
