//! Document-level compare — the `hwp compare` IR transform (GM-8, FLOW-03).
//!
//! Two documents in, a [`DocumentDiff`] out. This module performs no I/O of any
//! kind: every function here takes `&Document` values already loaded by the
//! caller and returns pure data, which is what lets `hwp compare` leave both
//! inputs untouched (success criterion 3) — nothing on this path can write.
//!
//! **Text differences (D-11).** Paragraphs are flattened across sections into
//! one ordered sequence per document — top-level paragraphs plus the
//! paragraphs nested in table cells/captions, picture captions, and generic
//! control paragraph lists, visited depth-first in document order — and
//! compared on their `chars` sequence
//! with derived [`HwpChar`] equality — structural, not normalized (assumption
//! A-09: no NFC/NFD folding, so two paragraphs differing only by Unicode
//! normalization form report as changed). A hand-rolled paragraph-level LCS
//! (no diff crate exists in the workspace and project invariant 4 forbids
//! adding one) classifies each paragraph as equal, inserted, deleted or
//! replaced. Only replaced pairs get a second, character-level LCS, whose
//! [`CharRun`] offsets and lengths are WCHAR quantities accumulated through
//! [`HwpChar::wchar_width`] — never element indices, since an inline or
//! extended control is 8 WCHAR wide but one array element.
//!
//! **Backtrack tie-break order (determinism).** Both LCS backtracks resolve a
//! tie between "diagonal equal", "a-side deletion" and "b-side insertion" in
//! that fixed order: prefer the diagonal match; when no match applies, prefer
//! consuming the a-side element over the b-side element. Adjacent a-side
//! deletion / b-side insertion pairs are folded into one `Replace` entry. The
//! same input pair therefore always produces the same report.
//!
//! **Bounded allocation (FLOW-03 `precision`).** Both the paragraph-level and
//! the character-level DP tables have `a_len * b_len` cells. Each product is
//! computed with a checked multiply and refused above [`MAX_LCS_CELLS`] rather
//! than allocated unbounded or silently degraded to a weaker comparison.
//!
//! **Structure (D-12).** [`StructureDiff`] covers section counts, flattened
//! paragraph counts, a per-kind control inventory (the same multiset-counting
//! shape `hwp-cli`'s `commands/preservation.rs` uses for its own control
//! comparison), and table row/column geometry deltas, paired positionally.
//! Char shape and para shape formatting differences are deliberately excluded
//! — palette indices differ between independently produced documents, so
//! comparing them would report noise rather than a real difference.

use std::collections::BTreeMap;

use hwp_model::{Control, Document, HwpChar, Paragraph};

/// The paragraph-level LCS DP table is refused above this many cells
/// (`a_len * b_len`), rather than allocated unbounded or silently degraded to
/// a weaker comparison (FLOW-03 `precision` probe, planner decision A-11).
pub const MAX_LCS_CELLS: usize = 25_000_000;

/// How much of a paragraph's text a [`ParagraphEntry`] carries, counted in
/// `char`s (Unicode scalar values), never bytes — a Korean paragraph yields
/// this many Hangul syllables and never a split UTF-8 sequence. The cap also
/// stops one pathologically long paragraph from inflating the whole report.
pub const TEXT_EXCERPT_CHARS: usize = 60;

/// The full result of comparing two documents.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentDiff {
    /// Paragraph-level operations, in document order (a-side, then b-side for
    /// pure insertions), covering every paragraph on both sides.
    pub paragraphs: Vec<ParagraphEntry>,
    pub structure: StructureDiff,
    /// True only when every paragraph operation is `Equal` and every
    /// structural count matches on both sides.
    pub identical: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParagraphOp {
    Equal,
    Insert,
    Delete,
    Replace,
}

/// Where a paragraph sits in its document (D-16). Carried on every
/// [`ParagraphEntry`] so a report can name the paragraph without walking the
/// source `Document` a second time — the divergent second index space that
/// made cell paragraphs print blank (#223).
///
/// `table` is the 0-based document-order table number `hwp_convert::edit`'s
/// `with_nth_table` uses, so a printed cell location is a valid
/// `hwp edit --set-cell "table:row:col=..."` address. The two counters are
/// kept in lockstep by construction: only tables that walker can reach get a
/// number at all (see [`walk_nested_paragraphs`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParagraphLocation {
    /// A section's own paragraph: the 0-based section index and the 0-based
    /// index within that section's paragraph list.
    Body { section: usize, index: usize },
    /// A paragraph inside the cell of an addressable table: the section, the
    /// document-order table number, the cell's `row` and `col`, and the
    /// 0-based index within the cell's paragraph list.
    Cell {
        section: usize,
        table: usize,
        row: u16,
        col: u16,
        index: usize,
    },
    /// A paragraph inside the caption of an addressable table: the section,
    /// the table number, and the 0-based index within the caption's
    /// paragraph list.
    Caption {
        section: usize,
        table: usize,
        index: usize,
    },
    /// A paragraph inside any other nested list: a picture caption, a generic
    /// control's paragraph list or caption, or anything below a caption —
    /// including the cells of a table nested inside one, which `--set-cell`
    /// cannot address and which therefore carries no table number. The
    /// section, the owning control's `ctrl_id`, and the 0-based index within
    /// that list.
    Nested {
        section: usize,
        ctrl_id: [u8; 4],
        index: usize,
    },
}

/// One paragraph-level operation. `a_index`/`b_index` are indices into the
/// flattened paragraph sequence of each document (top-level plus nested
/// cell/caption paragraphs; not WCHAR offsets). `chars` is populated only for
/// `Replace`.
#[derive(Debug, Clone, PartialEq)]
pub struct ParagraphEntry {
    pub op: ParagraphOp,
    pub a_index: Option<usize>,
    pub b_index: Option<usize>,
    pub chars: Option<Vec<CharRun>>,
    /// The paragraph's own text, the first [`TEXT_EXCERPT_CHARS`] `char`s of
    /// [`para_text`]. Taken from the a-side, except for a pure `Insert`,
    /// which has no a-side and therefore reports the b-side.
    pub text: String,
    /// Where [`text`](Self::text) lives — same side as `text`.
    pub location: ParagraphLocation,
    /// The b-side text of a `Replace`, which is the one operation with a
    /// paragraph on both sides; `None` for every other operation, mirroring
    /// how `chars` is populated only for `Replace`.
    pub b_text: Option<String>,
    /// Where [`b_text`](Self::b_text) lives; `None` whenever `b_text` is.
    pub b_location: Option<ParagraphLocation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharOp {
    Equal,
    Insert,
    Delete,
}

/// One character-level run inside a replaced paragraph pair. `a_wchar` and
/// `b_wchar` are WCHAR offsets from the start of the paragraph (via
/// [`HwpChar::wchar_width`]), not element indices; `len_wchar` is the WCHAR
/// width of the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharRun {
    pub op: CharOp,
    pub a_wchar: Option<u32>,
    pub b_wchar: Option<u32>,
    pub len_wchar: u32,
}

/// A row/column geometry change for one positionally-paired table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableGeometryDelta {
    /// Position of this table in the positional pairing of both documents'
    /// flattened table lists.
    pub index: usize,
    pub rows: (u16, u16),
    pub cols: (u16, u16),
}

/// Structural differences per D-12: section counts, flattened paragraph
/// counts (nested cell/caption paragraphs included), a per-kind control
/// inventory, and table geometry deltas. Char shape / para shape formatting
/// is deliberately out of scope for v1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureDiff {
    pub sections: (usize, usize),
    pub paragraphs: (usize, usize),
    /// Per-kind control counts on both sides, keyed by `ctrl_id` (e.g.
    /// `b"tbl "`). Only kinds present on at least one side appear.
    pub controls: BTreeMap<[u8; 4], (usize, usize)>,
    pub tables: Vec<TableGeometryDelta>,
    /// Paragraphs inserted and deleted inside table cells (D-18). Counted
    /// from the paragraph entries whose location is the cell variant, so a
    /// difference that lives only inside a table is a reported fact rather
    /// than something the summary has to be read between the lines for.
    pub cell_paragraphs: (usize, usize),
    /// How many tables each document has. `tables` above only pairs the
    /// tables both sides have; before this field the surplus was dropped by a
    /// positional `zip` without a word (D-18).
    pub table_counts: (usize, usize),
}

impl StructureDiff {
    fn is_identical(&self) -> bool {
        self.sections.0 == self.sections.1
            && self.paragraphs.0 == self.paragraphs.1
            && self.controls.values().all(|(a, b)| a == b)
            && self.tables.is_empty()
            && self.cell_paragraphs == (0, 0)
            && self.table_counts.0 == self.table_counts.1
    }
}

/// Compares two documents. Pure function, no I/O: the caller owns loading and
/// rendering. Returns `Err` naming the [`MAX_LCS_CELLS`] ceiling when either
/// the paragraph-level or a character-level LCS table would exceed it.
pub fn compare_documents(a: &Document, b: &Document) -> Result<DocumentDiff, String> {
    let (a_paras, a_meta) = flatten_paragraphs(a);
    let (b_paras, b_meta) = flatten_paragraphs(b);

    let ops = paragraph_lcs(&a_paras, &b_paras)?;
    let paragraphs = fold_paragraph_ops(ops, &a_paras, &b_paras, &a_meta, &b_meta)?;

    let structure = structure_diff(a, b, a_paras.len(), b_paras.len(), &paragraphs);

    let identical =
        paragraphs.iter().all(|e| e.op == ParagraphOp::Equal) && structure.is_identical();

    Ok(DocumentDiff {
        paragraphs,
        structure,
        identical,
    })
}

/// Plain text of one paragraph: text characters kept, the `LINE_BREAK`
/// control character mapped to a space, every other element dropped, then
/// trimmed. Moved here from `hwp-cli`'s `commands/compare.rs` so the compare
/// report can read a paragraph's text off its [`ParagraphEntry`] instead of
/// indexing back into the source document (D-16). The near-identical copies
/// in `commands/grep.rs` and `commands/fill.rs` are deliberately left alone.
pub fn para_text(para: &Paragraph) -> String {
    let mut text = String::new();
    for ch in &para.chars {
        match ch {
            HwpChar::Text(c) => text.push(*c),
            HwpChar::CharCtrl(hwp_model::ctrl_char::LINE_BREAK) => text.push(' '),
            _ => {}
        }
    }
    text.trim().to_string()
}

/// One flattened paragraph's reportable identity: its excerpt and where it
/// lives. Collected in the same single walk that flattens the `chars`
/// sequences, so the two can never drift out of index alignment.
struct ParagraphMeta {
    text: String,
    location: ParagraphLocation,
}

/// Flattens every section's paragraphs — top-level plus the paragraphs
/// nested in table cells/captions, picture captions, and generic control
/// paragraph lists/captions — into one ordered `chars` sequence per
/// paragraph plus its [`ParagraphMeta`], in depth-first document order.
fn flatten_paragraphs(document: &Document) -> (Vec<Vec<HwpChar>>, Vec<ParagraphMeta>) {
    let mut chars = Vec::new();
    let mut meta = Vec::new();
    walk_paragraphs(document, &mut |paragraph, location| {
        chars.push(paragraph.chars.clone());
        meta.push(ParagraphMeta {
            text: para_text(paragraph)
                .chars()
                .take(TEXT_EXCERPT_CHARS)
                .collect(),
            location,
        });
    });
    (chars, meta)
}

/// Visits every paragraph of the document in depth-first document order —
/// each top-level paragraph, then the paragraphs nested in its controls —
/// handing the visitor the paragraph's [`ParagraphLocation`].
/// The nested traversal mirrors hwp-cli's
/// `commands/preservation.rs::collect_paragraph_controls` (this crate cannot
/// depend on that binary crate, so the traversal shape is mirrored rather
/// than shared).
///
/// The table counter is document-global; see [`walk_nested_paragraphs`] for
/// how it is kept identical to `edit.rs`'s `with_nth_table` numbering.
fn walk_paragraphs(document: &Document, visit: &mut impl FnMut(&Paragraph, ParagraphLocation)) {
    let mut tables = 0usize;
    for (section, sec) in document.sections.iter().enumerate() {
        for (index, paragraph) in sec.paragraphs.iter().enumerate() {
            visit(paragraph, ParagraphLocation::Body { section, index });
            let scope = Scope {
                section,
                addressable: true,
            };
            walk_nested_paragraphs(paragraph, scope, &mut tables, visit);
        }
    }
}

/// What a nested paragraph inherits from the list it lives in.
#[derive(Debug, Clone, Copy)]
struct Scope {
    section: usize,
    /// Whether `edit.rs`'s `walk_nth_table` can reach this list at all.
    addressable: bool,
}

/// Visits the paragraphs nested in `paragraph`'s controls: table cells and
/// caption, picture caption, and generic paragraph lists and caption.
///
/// **Table numbering.** The counter must produce exactly the index
/// `edit.rs`'s `with_nth_table` takes, or a printed `--set-cell` address
/// edits the wrong table. That walker increments on a `Control::Table`
/// **before** descending into its cells, and it descends only through
/// section paragraphs, table cells and generic paragraph lists — never
/// through a caption. So the counter here increments in the same pre-order
/// and only while `scope.addressable` holds; every caption clears the flag
/// for its whole subtree, and a table found there gets no number and
/// consumes none, keeping later tables correctly numbered.
fn walk_nested_paragraphs(
    paragraph: &Paragraph,
    scope: Scope,
    tables: &mut usize,
    visit: &mut impl FnMut(&Paragraph, ParagraphLocation),
) {
    let section = scope.section;
    let caption_scope = Scope {
        addressable: false,
        ..scope
    };
    for control in &paragraph.controls {
        let ctrl_id = control.ctrl_id();
        let nested = move |index| ParagraphLocation::Nested {
            section,
            ctrl_id,
            index,
        };
        match control {
            Control::Table(table) => {
                let number = scope.addressable.then(|| {
                    let number = *tables;
                    *tables += 1;
                    number
                });
                for cell in &table.cells {
                    walk_list(
                        &cell.paragraphs,
                        scope,
                        tables,
                        visit,
                        |index| match number {
                            Some(table) => ParagraphLocation::Cell {
                                section,
                                table,
                                row: cell.row,
                                col: cell.col,
                                index,
                            },
                            None => nested(index),
                        },
                    );
                }
                if let Some(caption) = &table.caption {
                    walk_list(&caption.paragraphs, caption_scope, tables, visit, |index| {
                        match number {
                            Some(table) => ParagraphLocation::Caption {
                                section,
                                table,
                                index,
                            },
                            None => nested(index),
                        }
                    });
                }
            }
            Control::Picture(picture) => {
                if let Some(caption) = &picture.caption {
                    walk_list(&caption.paragraphs, caption_scope, tables, visit, nested);
                }
            }
            Control::Generic(generic) => {
                for list in &generic.paragraph_lists {
                    walk_list(&list.paragraphs, scope, tables, visit, nested);
                }
                if let Some(caption) = &generic.caption {
                    walk_list(&caption.paragraphs, caption_scope, tables, visit, nested);
                }
            }
            Control::SectionDef(_) => {}
        }
    }
}

/// Visits one nested paragraph list: each paragraph with the location
/// `locate` builds for it, then that paragraph's own nested lists.
fn walk_list(
    paragraphs: &[Paragraph],
    scope: Scope,
    tables: &mut usize,
    visit: &mut impl FnMut(&Paragraph, ParagraphLocation),
    mut locate: impl FnMut(usize) -> ParagraphLocation,
) {
    for (index, nested) in paragraphs.iter().enumerate() {
        visit(nested, locate(index));
        walk_nested_paragraphs(nested, scope, tables, visit);
    }
}

// ---- Paragraph-level LCS ----------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AtomicOp {
    Equal(usize, usize),
    Delete(usize),
    Insert(usize),
}

/// Hand-rolled O(a_len * b_len) paragraph-level LCS. See the module doc
/// comment for the tie-break order and the cell ceiling.
fn paragraph_lcs(a: &[Vec<HwpChar>], b: &[Vec<HwpChar>]) -> Result<Vec<AtomicOp>, String> {
    let a_len = a.len();
    let b_len = b.len();

    let cells = a_len
        .checked_mul(b_len)
        .filter(|cells| *cells <= MAX_LCS_CELLS);
    if a_len > 0 && b_len > 0 && cells.is_none() {
        return Err(format!(
            "문단 수 조합이 비교 가능한 한도를 초과했습니다: {a_len} x {b_len} 문단 \
             (상한 MAX_LCS_CELLS = {MAX_LCS_CELLS}칸)"
        ));
    }

    // dp[i][j] = LCS length of a[..i], b[..j].
    let mut dp = vec![vec![0u32; b_len + 1]; a_len + 1];
    for (i, a_para) in a.iter().enumerate() {
        for (j, b_para) in b.iter().enumerate() {
            dp[i + 1][j + 1] = if a_para == b_para {
                dp[i][j] + 1
            } else {
                dp[i][j + 1].max(dp[i + 1][j])
            };
        }
    }

    let mut ops = Vec::new();
    let (mut i, mut j) = (a_len, b_len);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && a[i - 1] == b[j - 1] {
            ops.push(AtomicOp::Equal(i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if i > 0 && (j == 0 || dp[i - 1][j] >= dp[i][j - 1]) {
            ops.push(AtomicOp::Delete(i - 1));
            i -= 1;
        } else {
            ops.push(AtomicOp::Insert(j - 1));
            j -= 1;
        }
    }
    ops.reverse();
    Ok(ops)
}

/// Folds an adjacent a-side-deletion / b-side-insertion pair into one
/// `Replace` entry and attaches its character sub-diff. Returns `Err` when a
/// replaced pair's character-level LCS table would exceed [`MAX_LCS_CELLS`].
fn fold_paragraph_ops(
    ops: Vec<AtomicOp>,
    a: &[Vec<HwpChar>],
    b: &[Vec<HwpChar>],
    a_meta: &[ParagraphMeta],
    b_meta: &[ParagraphMeta],
) -> Result<Vec<ParagraphEntry>, String> {
    // Text and location come from the a-side, except for a pure Insert, which
    // has no a-side; a Replace additionally carries its b-side explicitly.
    let entry = |op, a_index: Option<usize>, b_index: Option<usize>, chars| {
        let primary = a_index
            .map(|i| &a_meta[i])
            .unwrap_or_else(|| &b_meta[b_index.expect("an entry has at least one side")]);
        let b_side = (op == ParagraphOp::Replace).then(|| &b_meta[b_index.unwrap()]);
        ParagraphEntry {
            op,
            a_index,
            b_index,
            chars,
            text: primary.text.clone(),
            location: primary.location.clone(),
            b_text: b_side.map(|meta| meta.text.clone()),
            b_location: b_side.map(|meta| meta.location.clone()),
        }
    };

    let mut out = Vec::with_capacity(ops.len());
    let mut iter = ops.into_iter().peekable();
    while let Some(op) = iter.next() {
        match op {
            AtomicOp::Equal(ai, bi) => {
                out.push(entry(ParagraphOp::Equal, Some(ai), Some(bi), None))
            }
            AtomicOp::Delete(ai) => {
                if let Some(AtomicOp::Insert(bi)) = iter.peek().copied() {
                    iter.next();
                    let chars = Some(char_lcs_runs(&a[ai], &b[bi])?);
                    out.push(entry(ParagraphOp::Replace, Some(ai), Some(bi), chars));
                } else {
                    out.push(entry(ParagraphOp::Delete, Some(ai), None, None));
                }
            }
            AtomicOp::Insert(bi) => {
                if let Some(AtomicOp::Delete(ai)) = iter.peek().copied() {
                    iter.next();
                    let chars = Some(char_lcs_runs(&a[ai], &b[bi])?);
                    out.push(entry(ParagraphOp::Replace, Some(ai), Some(bi), chars));
                } else {
                    out.push(entry(ParagraphOp::Insert, None, Some(bi), None));
                }
            }
        }
    }
    Ok(out)
}

// ---- Character-level LCS (only inside a Replace pair) -----------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AtomicCharOp {
    Equal(usize, usize),
    Delete(usize),
    Insert(usize),
}

/// Same DP shape and cell ceiling as [`paragraph_lcs`], one level down: over
/// `HwpChar` elements instead of paragraphs. The ceiling applies here too —
/// a single paragraph can hold tens of millions of WCHARs under hwp5's
/// default `max_stream_bytes`, so an unbounded table would abort on
/// allocation failure rather than return a catchable error.
fn char_lcs(a: &[HwpChar], b: &[HwpChar]) -> Result<Vec<AtomicCharOp>, String> {
    let a_len = a.len();
    let b_len = b.len();

    let cells = a_len
        .checked_mul(b_len)
        .filter(|cells| *cells <= MAX_LCS_CELLS);
    if a_len > 0 && b_len > 0 && cells.is_none() {
        return Err(format!(
            "문자 수 조합이 비교 가능한 한도를 초과했습니다: {a_len} x {b_len} 문자 \
             (상한 MAX_LCS_CELLS = {MAX_LCS_CELLS}칸)"
        ));
    }

    let mut dp = vec![vec![0u32; b_len + 1]; a_len + 1];
    for (i, a_char) in a.iter().enumerate() {
        for (j, b_char) in b.iter().enumerate() {
            dp[i + 1][j + 1] = if a_char == b_char {
                dp[i][j] + 1
            } else {
                dp[i][j + 1].max(dp[i + 1][j])
            };
        }
    }

    let mut ops = Vec::new();
    let (mut i, mut j) = (a_len, b_len);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && a[i - 1] == b[j - 1] {
            ops.push(AtomicCharOp::Equal(i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if i > 0 && (j == 0 || dp[i - 1][j] >= dp[i][j - 1]) {
            ops.push(AtomicCharOp::Delete(i - 1));
            i -= 1;
        } else {
            ops.push(AtomicCharOp::Insert(j - 1));
            j -= 1;
        }
    }
    ops.reverse();
    Ok(ops)
}

/// Prefix sums of `wchar_width`, so element index `i` maps to its WCHAR
/// offset in O(1) without recomputing widths of everything before it.
fn wchar_prefix(chars: &[HwpChar]) -> Vec<u32> {
    let mut prefix = Vec::with_capacity(chars.len() + 1);
    prefix.push(0);
    let mut running = 0u32;
    for c in chars {
        running += c.wchar_width();
        prefix.push(running);
    }
    prefix
}

/// Runs a character-level LCS over a replaced paragraph pair and coalesces
/// the atomic ops into WCHAR-offset [`CharRun`]s. Returns `Err` when the
/// pair's LCS table would exceed [`MAX_LCS_CELLS`].
fn char_lcs_runs(a: &[HwpChar], b: &[HwpChar]) -> Result<Vec<CharRun>, String> {
    let ops = char_lcs(a, b)?;
    let a_prefix = wchar_prefix(a);
    let b_prefix = wchar_prefix(b);

    let mut runs: Vec<CharRun> = Vec::new();
    for op in ops {
        let (kind, a_idx, b_idx) = match op {
            AtomicCharOp::Equal(ai, bi) => (CharOp::Equal, Some(ai), Some(bi)),
            AtomicCharOp::Delete(ai) => (CharOp::Delete, Some(ai), None),
            AtomicCharOp::Insert(bi) => (CharOp::Insert, None, Some(bi)),
        };
        let a_wchar = a_idx.map(|i| a_prefix[i]);
        let b_wchar = b_idx.map(|i| b_prefix[i]);
        let width = match (a_idx, b_idx) {
            (Some(i), _) => a_prefix[i + 1] - a_prefix[i],
            (_, Some(i)) => b_prefix[i + 1] - b_prefix[i],
            (None, None) => 0,
        };

        let extends_last = runs.last().is_some_and(|last: &CharRun| {
            last.op == kind
                && a_wchar == last.a_wchar.map(|w| w + last.len_wchar)
                && b_wchar == last.b_wchar.map(|w| w + last.len_wchar)
        });

        if extends_last {
            runs.last_mut().unwrap().len_wchar += width;
        } else {
            runs.push(CharRun {
                op: kind,
                a_wchar,
                b_wchar,
                len_wchar: width,
            });
        }
    }
    Ok(runs)
}

// ---- Structural inventory (D-12) ---------------------------------------

fn structure_diff(
    a: &Document,
    b: &Document,
    a_paragraph_count: usize,
    b_paragraph_count: usize,
    paragraphs: &[ParagraphEntry],
) -> StructureDiff {
    let a_controls = control_inventory(a);
    let b_controls = control_inventory(b);
    let mut controls: BTreeMap<[u8; 4], (usize, usize)> = BTreeMap::new();
    for (kind, count) in &a_controls {
        controls.entry(*kind).or_insert((0, 0)).0 = *count;
    }
    for (kind, count) in &b_controls {
        controls.entry(*kind).or_insert((0, 0)).1 = *count;
    }

    let a_tables = table_geometries(a);
    let b_tables = table_geometries(b);
    let tables = a_tables
        .iter()
        .zip(b_tables.iter())
        .enumerate()
        .filter(|(_, (a_dims, b_dims))| a_dims != b_dims)
        .map(|(index, (a_dims, b_dims))| TableGeometryDelta {
            index,
            rows: (a_dims.0, b_dims.0),
            cols: (a_dims.1, b_dims.1),
        })
        .collect();

    let is_cell = |location: &ParagraphLocation| matches!(location, ParagraphLocation::Cell { .. });
    let count_cell = |op| {
        paragraphs
            .iter()
            .filter(|entry| entry.op == op && is_cell(&entry.location))
            .count()
    };

    StructureDiff {
        sections: (a.sections.len(), b.sections.len()),
        paragraphs: (a_paragraph_count, b_paragraph_count),
        controls,
        tables,
        cell_paragraphs: (
            count_cell(ParagraphOp::Insert),
            count_cell(ParagraphOp::Delete),
        ),
        table_counts: (a_tables.len(), b_tables.len()),
    }
}

/// Per-kind control counts, recursing into table cells/captions, picture
/// captions, and generic paragraph lists/captions via [`walk_paragraphs`] —
/// the same nested coverage as `hwp-cli`'s
/// `commands/preservation.rs::collect_paragraph_controls` (this crate cannot
/// depend on that binary crate, so the counting shape is mirrored rather
/// than shared).
fn control_inventory(document: &Document) -> BTreeMap<[u8; 4], usize> {
    let mut counts = BTreeMap::new();
    walk_paragraphs(document, &mut |paragraph, _location| {
        for control in &paragraph.controls {
            *counts.entry(control.ctrl_id()).or_default() += 1;
        }
    });
    counts
}

/// Flattened, ordered (rows, cols) for every table in the document,
/// including tables nested in cells and captions.
fn table_geometries(document: &Document) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    walk_paragraphs(document, &mut |paragraph, _location| {
        for control in &paragraph.controls {
            if let Control::Table(table) = control {
                out.push((table.rows, table.cols));
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(md: &str) -> Document {
        crate::from_markdown::from_markdown(md)
    }

    /// Appends a paragraph inside the first table's `(row, col)` cell — the
    /// #223 shape, where both documents' top-level paragraphs are identical
    /// and the only difference lives inside a cell.
    fn push_cell_paragraph(document: &mut Document, row: u16, col: u16, text: &str) {
        for para in document
            .sections
            .iter_mut()
            .flat_map(|s| s.paragraphs.iter_mut())
        {
            for control in &mut para.controls {
                if let Control::Table(table) = control {
                    let cell = table
                        .cells
                        .iter_mut()
                        .find(|c| c.row == row && c.col == col)
                        .expect("the cell exists");
                    let mut added = cell.paragraphs[0].clone();
                    added.chars = text.chars().map(HwpChar::Text).collect();
                    cell.paragraphs.push(added);
                    return;
                }
            }
        }
        panic!("the document has no table");
    }

    /// The `n`th table directly under a section paragraph. Test-only: it does
    /// not recurse, so it stays independent of the walker under test.
    fn nth_top_level_table(document: &mut Document, n: usize) -> &mut hwp_model::Table {
        document.sections[0]
            .paragraphs
            .iter_mut()
            .flat_map(|para| para.controls.iter_mut())
            .filter_map(|control| match control {
                Control::Table(table) => Some(table),
                _ => None,
            })
            .nth(n)
            .expect("the table exists")
    }

    /// Gives table `n` a caption holding a copy of itself, so the document has
    /// a table `edit`'s `with_nth_table` can never reach.
    fn add_caption_table(document: &mut Document, n: usize) {
        let inner = nth_top_level_table(document, n).clone();
        nth_top_level_table(document, n).caption = Some(hwp_model::Caption {
            side: hwp_model::CaptionSide::Bottom,
            direction: hwp_model::CaptionDirection::Horizontal,
            gap: 283,
            width: None,
            last_width: 0,
            paragraphs: vec![Paragraph {
                controls: vec![Control::Table(inner)],
                ..Paragraph::default()
            }],
        });
    }

    /// #227 finding A: `with_nth_table` never descends into a caption, so a
    /// table nested in one must not consume a table number — otherwise every
    /// later table is off by one and the printed cell address edits the wrong
    /// table.
    #[test]
    fn a_caption_nested_table_does_not_shift_the_set_cell_numbering() {
        let one = "| a | b |\n| - | - |\n| 1 | 2 |\n";
        let mut a = doc(&format!("{one}\n첫 본문\n\n{one}\n둘째 본문\n\n{one}"));
        add_caption_table(&mut a, 0);

        let mut b = a.clone();
        let cell = nth_top_level_table(&mut b, 2)
            .cells
            .iter_mut()
            .find(|c| c.row == 1 && c.col == 1)
            .expect("the cell exists");
        let mut added = cell.paragraphs[0].clone();
        added.chars = "셋째 표 셀 추가".chars().map(HwpChar::Text).collect();
        cell.paragraphs.push(added);

        let diff = compare_documents(&a, &b).unwrap();
        let inserted = diff
            .paragraphs
            .iter()
            .find(|e| e.op == ParagraphOp::Insert)
            .expect("the added cell paragraph is one insertion");
        let ParagraphLocation::Cell {
            table: reported,
            row,
            col,
            ..
        } = inserted.location
        else {
            panic!("a cell paragraph reports a cell location: {:?}", inserted);
        };
        assert_eq!(reported, 2, "the caption table must not take a number");

        // The reported number is a real `--set-cell` address: driving the
        // editor with it lands in that same table.
        let mut edited = a.clone();
        crate::edit::set_cell(&mut edited, reported, row, col, "찍었다").unwrap();
        let landed = nth_top_level_table(&mut edited, 2)
            .cells
            .iter()
            .find(|c| c.row == row && c.col == col)
            .expect("the cell exists");
        assert_eq!(para_text(&landed.paragraphs[0]), "찍었다");
    }

    /// Everything below a caption is unaddressable, cells included.
    #[test]
    fn a_caption_nested_table_cell_reports_a_nested_location() {
        let mut a = doc("| a | b |\n| - | - |\n| 1 | 2 |\n");
        add_caption_table(&mut a, 0);
        let mut locations = Vec::new();
        walk_paragraphs(&a, &mut |_, location| locations.push(location));
        assert!(
            locations.iter().any(
                |l| matches!(l, ParagraphLocation::Nested { ctrl_id, .. } if ctrl_id == b"tbl ")
            ),
            "a caption table's cells are nested, not addressable: {locations:?}"
        );
        assert!(
            !locations
                .iter()
                .any(|l| matches!(l, ParagraphLocation::Cell { table, .. } if *table != 0)),
            "only table 0 is addressable here: {locations:?}"
        );
    }

    #[test]
    fn inserted_cell_paragraph_carries_its_cell_location_and_text() {
        let a = doc("| a | b |\n| - | - |\n| 1 | 2 |\n");
        let mut b = a.clone();
        push_cell_paragraph(&mut b, 1, 1, "○ 둘째 항목 BBB");

        let diff = compare_documents(&a, &b).unwrap();
        let inserted = diff
            .paragraphs
            .iter()
            .find(|e| e.op == ParagraphOp::Insert)
            .expect("the added cell paragraph is one insertion");
        assert_eq!(inserted.text, "○ 둘째 항목 BBB");
        assert_eq!(
            inserted.location,
            ParagraphLocation::Cell {
                section: 0,
                table: 0,
                row: 1,
                col: 1,
                index: 1,
            }
        );
    }

    #[test]
    fn two_equal_adjacent_cell_paragraphs_stay_two_entries() {
        // The diff is index-based over the deep walk, so byte-equal
        // neighbours never merge into one entry or collide on one location.
        let a = doc("| a | b |\n| - | - |\n| 1 | 2 |\n");
        let mut b = a.clone();
        push_cell_paragraph(&mut b, 1, 1, "같은 문단");
        push_cell_paragraph(&mut b, 1, 1, "같은 문단");

        let diff = compare_documents(&a, &b).unwrap();
        let inserted: Vec<_> = diff
            .paragraphs
            .iter()
            .filter(|e| e.op == ParagraphOp::Insert)
            .collect();
        assert_eq!(inserted.len(), 2);
        assert_ne!(inserted[0].location, inserted[1].location);
        assert_ne!(inserted[0].b_index, inserted[1].b_index);
    }

    #[test]
    fn empty_cell_paragraph_still_carries_a_full_location() {
        let a = doc("| a | b |\n| - | - |\n| 1 | 2 |\n");
        let mut b = a.clone();
        push_cell_paragraph(&mut b, 1, 1, "");

        let diff = compare_documents(&a, &b).unwrap();
        let inserted = diff
            .paragraphs
            .iter()
            .find(|e| e.op == ParagraphOp::Insert)
            .expect("an empty added paragraph is still an insertion");
        assert_eq!(inserted.text, "");
        assert!(matches!(
            inserted.location,
            ParagraphLocation::Cell { row: 1, col: 1, .. }
        ));
    }

    #[test]
    fn excerpt_is_capped_in_chars_not_bytes() {
        let a = doc("머리\n");
        let mut b = doc("머리\n\n{}\n");
        b.sections[0]
            .paragraphs
            .last_mut()
            .unwrap()
            .chars
            .splice(.., "가".repeat(80).chars().map(HwpChar::Text));

        let diff = compare_documents(&a, &b).unwrap();
        let changed = diff
            .paragraphs
            .iter()
            .find(|e| e.op != ParagraphOp::Equal)
            .expect("the long paragraph differs");
        let excerpt = changed.b_text.as_deref().unwrap_or(&changed.text);
        assert_eq!(excerpt.chars().count(), TEXT_EXCERPT_CHARS);
        assert_eq!(excerpt, "가".repeat(TEXT_EXCERPT_CHARS));
    }

    #[test]
    fn identical_documents_have_empty_diff() {
        let a = doc("첫 문단\n\n둘째 문단\n");
        let b = doc("첫 문단\n\n둘째 문단\n");
        let diff = compare_documents(&a, &b).unwrap();
        assert!(diff.identical);
        assert!(diff.paragraphs.iter().all(|e| e.op == ParagraphOp::Equal));
    }

    #[test]
    fn appended_paragraph_is_one_insertion() {
        let a = doc("첫 문단\n");
        let b = doc("첫 문단\n\n둘째 문단\n");
        let diff = compare_documents(&a, &b).unwrap();
        assert!(!diff.identical);
        let inserts: Vec<_> = diff
            .paragraphs
            .iter()
            .filter(|e| e.op == ParagraphOp::Insert)
            .collect();
        assert_eq!(inserts.len(), 1);
        assert!(diff.paragraphs.iter().all(|e| e.op != ParagraphOp::Delete));
    }

    #[test]
    fn removed_paragraph_is_one_deletion() {
        let a = doc("첫 문단\n\n둘째 문단\n");
        let b = doc("첫 문단\n");
        let diff = compare_documents(&a, &b).unwrap();
        let deletes: Vec<_> = diff
            .paragraphs
            .iter()
            .filter(|e| e.op == ParagraphOp::Delete)
            .collect();
        assert_eq!(deletes.len(), 1);
        assert!(diff.paragraphs.iter().all(|e| e.op != ParagraphOp::Insert));
    }

    #[test]
    fn changed_paragraph_reports_wchar_offset_char_run() {
        // A tab is modeled as an 8-WCHAR InlineCtrl (invariant, paragraph.rs).
        // The changed text after it must report an offset 8 greater than a
        // text-only count would give. "머리" is a throwaway leading paragraph
        // so this one is not the document's first (which would otherwise
        // carry the section/column-def ExtCtrls added ahead of the text).
        let a = doc("머리\n\n가\t나\n");
        let b = doc("머리\n\n가\t다\n");
        let diff = compare_documents(&a, &b).unwrap();
        let replace = diff
            .paragraphs
            .iter()
            .find(|e| e.op == ParagraphOp::Replace)
            .expect("one replace entry");
        let runs = replace.chars.as_ref().expect("char sub-diff");
        assert!(
            runs.iter()
                .any(|r| r.op == CharOp::Equal && r.a_wchar == Some(0))
        );
        // 1 (가, BMP text) + 8 (tab InlineCtrl) = 9 WCHAR before the changed character.
        let changed = runs
            .iter()
            .find(|r| r.op == CharOp::Delete || r.op == CharOp::Insert)
            .expect("a changed run");
        let offset = changed.a_wchar.or(changed.b_wchar).unwrap();
        assert_eq!(offset, 9);

        // Unchanged paragraphs carry no sub-diff.
        let equal_entry = diff
            .paragraphs
            .iter()
            .find(|e| e.op == ParagraphOp::Equal)
            .expect("an unchanged paragraph exists (blank trailing paragraph or none)");
        assert!(equal_entry.chars.is_none());
    }

    #[test]
    fn normalization_form_difference_is_reported_as_changed() {
        // "가" (U+AC00, precomposed) vs "ᄀ ᅡ" (U+1100 U+1161, decomposed) —
        // same rendered text, different code points. No normalization in v1.
        let nfc = doc("\u{AC00}\n");
        let nfd = doc("\u{1100}\u{1161}\n");
        let diff = compare_documents(&nfc, &nfd).unwrap();
        assert!(!diff.identical);
    }

    #[test]
    fn table_row_gain_is_a_geometry_delta() {
        let a = doc("| a | b |\n| - | - |\n| 1 | 2 |\n");
        let b = doc("| a | b |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |\n");
        let diff = compare_documents(&a, &b).unwrap();
        assert_eq!(diff.structure.tables.len(), 1);
        let delta = diff.structure.tables[0];
        assert_ne!(delta.rows.0, delta.rows.1);
        assert_eq!(delta.cols.0, delta.cols.1);
    }

    #[test]
    fn no_formatting_difference_appears_anywhere() {
        // StructureDiff has no char-shape/para-shape field at all — this test
        // pins that the type itself cannot carry one.
        let a = doc("문단\n");
        let b = doc("문단\n");
        let diff = compare_documents(&a, &b).unwrap();
        let _: (usize, usize) = diff.structure.sections;
        let _: (usize, usize) = diff.structure.paragraphs;
    }

    #[test]
    fn paragraph_count_product_above_ceiling_is_refused() {
        let big = MAX_LCS_CELLS / 4000 + 1;
        let a: Vec<Vec<HwpChar>> = (0..big).map(|_| vec![HwpChar::Text('a')]).collect();
        let b: Vec<Vec<HwpChar>> = (0..4001).map(|_| vec![HwpChar::Text('b')]).collect();
        let err = paragraph_lcs(&a, &b).unwrap_err();
        assert!(err.contains("MAX_LCS_CELLS"));
    }

    #[test]
    fn cell_only_text_difference_is_not_identical() {
        // The top-level paragraphs are identical on both sides; only the text
        // inside one table cell differs.
        let a = doc("| a | b |\n| - | - |\n| 1 | 2 |\n");
        let b = doc("| a | b |\n| - | - |\n| 1 | 3 |\n");
        let diff = compare_documents(&a, &b).unwrap();
        assert!(!diff.identical);
        assert!(diff.paragraphs.iter().any(|e| e.op != ParagraphOp::Equal));
    }

    #[test]
    fn cell_paragraph_changes_are_counted_and_never_look_identical() {
        // One paragraph added inside a cell on each side, in different cells,
        // so the pair reports one insertion and one deletion inside tables.
        let table = "| 항목 | 내용 |\n| - | - |\n| 하나 | 둘 |\n";
        let mut a = doc(table);
        let mut b = doc(table);
        push_cell_paragraph(&mut a, 0, 0, "a쪽 추가");
        push_cell_paragraph(&mut b, 1, 1, "b쪽 추가");

        let diff = compare_documents(&a, &b).unwrap();
        assert_eq!(diff.structure.cell_paragraphs, (1, 1));
        assert!(!diff.structure.is_identical());
        assert!(!diff.identical);
    }

    #[test]
    fn differing_table_counts_are_reported_not_truncated() {
        let a = doc("| a | b |\n| - | - |\n| 1 | 2 |\n");
        let b = doc("| a | b |\n| - | - |\n| 1 | 2 |\n\n본문\n\n| c | d |\n| - | - |\n| 3 | 4 |\n");
        let diff = compare_documents(&a, &b).unwrap();
        assert_eq!(diff.structure.table_counts, (1, 2));
        assert!(!diff.structure.is_identical());
    }

    #[test]
    fn identical_structure_reports_zero_for_the_new_counts() {
        let a = doc("| a | b |\n| - | - |\n| 1 | 2 |\n");
        let b = doc("| a | b |\n| - | - |\n| 1 | 2 |\n");
        let diff = compare_documents(&a, &b).unwrap();
        assert_eq!(diff.structure.cell_paragraphs, (0, 0));
        assert_eq!(diff.structure.table_counts, (1, 1));
        assert!(diff.structure.is_identical());
        assert!(diff.identical);
    }

    #[test]
    fn char_count_product_above_ceiling_is_refused() {
        // Same ceiling shape as the paragraph-level test: the DP matrix is
        // refused before it is allocated.
        let big = MAX_LCS_CELLS / 4000 + 1;
        let a: Vec<HwpChar> = (0..big).map(|_| HwpChar::Text('a')).collect();
        let b: Vec<HwpChar> = (0..4001).map(|_| HwpChar::Text('b')).collect();
        let err = char_lcs(&a, &b).unwrap_err();
        assert!(err.contains("MAX_LCS_CELLS"));
    }

    #[test]
    fn oversized_replaced_pair_surfaces_as_err() {
        // End to end: two paragraphs are few enough for the paragraph-level
        // LCS, but the replaced pair's char product exceeds the ceiling, so
        // the compare returns Err instead of aborting on allocation failure.
        let big = MAX_LCS_CELLS / 4000 + 1;
        let a = doc(&format!("머리\n\n{}\n", "가".repeat(big)));
        let b = doc(&format!("머리\n\n{}\n", "나".repeat(4001)));
        let err = compare_documents(&a, &b).unwrap_err();
        assert!(err.contains("MAX_LCS_CELLS"));
    }

    #[test]
    fn empty_documents_compare_as_identical() {
        let a = Document::default();
        let b = Document::default();
        let diff = compare_documents(&a, &b).unwrap();
        assert!(diff.identical);
        assert!(diff.paragraphs.is_empty());
    }

    #[test]
    fn comparison_is_deterministic_across_runs() {
        let a = doc("첫 문단\n\n둘째 문단\n");
        let b = doc("첫 문단 수정\n\n셋째 문단\n");
        let first = compare_documents(&a, &b).unwrap();
        let second = compare_documents(&a, &b).unwrap();
        assert_eq!(first, second);
    }
}
