//! Document-level compare — the `hwp compare` IR transform (GM-8, FLOW-03).
//!
//! Two documents in, a [`DocumentDiff`] out. This module performs no I/O of any
//! kind: every function here takes `&Document` values already loaded by the
//! caller and returns pure data, which is what lets `hwp compare` leave both
//! inputs untouched (success criterion 3) — nothing on this path can write.
//!
//! **Text differences (D-11).** Paragraphs are flattened across sections into
//! one ordered sequence per document and compared on their `chars` sequence
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
//! **Bounded allocation (FLOW-03 `precision`).** The paragraph-level DP table
//! has `a_len * b_len` cells. That product is computed with a checked
//! multiply and refused above [`MAX_LCS_CELLS`] rather than allocated
//! unbounded or silently degraded to a weaker comparison.
//!
//! **Structure (D-12).** [`StructureDiff`] covers section counts, top-level
//! paragraph counts, a per-kind control inventory (the same multiset-counting
//! shape `hwp-cli`'s `commands/preservation.rs` uses for its own control
//! comparison), and table row/column geometry deltas, paired positionally.
//! Char shape and para shape formatting differences are deliberately excluded
//! — palette indices differ between independently produced documents, so
//! comparing them would report noise rather than a real difference.

use std::collections::BTreeMap;

use hwp_model::{Control, Document, HwpChar};

/// The paragraph-level LCS DP table is refused above this many cells
/// (`a_len * b_len`), rather than allocated unbounded or silently degraded to
/// a weaker comparison (FLOW-03 `precision` probe, planner decision A-11).
pub const MAX_LCS_CELLS: usize = 25_000_000;

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

/// One paragraph-level operation. `a_index`/`b_index` are indices into the
/// flattened top-level paragraph sequence of each document (not WCHAR
/// offsets). `chars` is populated only for `Replace`.
#[derive(Debug, Clone, PartialEq)]
pub struct ParagraphEntry {
    pub op: ParagraphOp,
    pub a_index: Option<usize>,
    pub b_index: Option<usize>,
    pub chars: Option<Vec<CharRun>>,
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

/// Structural differences per D-12: section counts, top-level paragraph
/// counts, a per-kind control inventory, and table geometry deltas. Char
/// shape / para shape formatting is deliberately out of scope for v1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureDiff {
    pub sections: (usize, usize),
    pub paragraphs: (usize, usize),
    /// Per-kind control counts on both sides, keyed by `ctrl_id` (e.g.
    /// `b"tbl "`). Only kinds present on at least one side appear.
    pub controls: BTreeMap<[u8; 4], (usize, usize)>,
    pub tables: Vec<TableGeometryDelta>,
}

impl StructureDiff {
    fn is_identical(&self) -> bool {
        self.sections.0 == self.sections.1
            && self.paragraphs.0 == self.paragraphs.1
            && self.controls.values().all(|(a, b)| a == b)
            && self.tables.is_empty()
    }
}

/// Compares two documents. Pure function, no I/O: the caller owns loading and
/// rendering. Returns `Err` naming the paragraph-count ceiling when the LCS
/// table would exceed [`MAX_LCS_CELLS`] cells.
pub fn compare_documents(a: &Document, b: &Document) -> Result<DocumentDiff, String> {
    let a_paras = flatten_paragraph_chars(a);
    let b_paras = flatten_paragraph_chars(b);

    let ops = paragraph_lcs(&a_paras, &b_paras)?;
    let paragraphs = fold_paragraph_ops(ops, &a_paras, &b_paras);

    let structure = structure_diff(a, b, a_paras.len(), b_paras.len());

    let identical =
        paragraphs.iter().all(|e| e.op == ParagraphOp::Equal) && structure.is_identical();

    Ok(DocumentDiff {
        paragraphs,
        structure,
        identical,
    })
}

/// Flattens every section's paragraphs (top-level only — not nested table
/// cell or caption paragraphs) into one ordered `chars` sequence per
/// paragraph, in document order.
fn flatten_paragraph_chars(document: &Document) -> Vec<Vec<HwpChar>> {
    document
        .sections
        .iter()
        .flat_map(|section| section.paragraphs.iter())
        .map(|paragraph| paragraph.chars.clone())
        .collect()
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
/// `Replace` entry and attaches its character sub-diff.
fn fold_paragraph_ops(
    ops: Vec<AtomicOp>,
    a: &[Vec<HwpChar>],
    b: &[Vec<HwpChar>],
) -> Vec<ParagraphEntry> {
    let mut out = Vec::with_capacity(ops.len());
    let mut iter = ops.into_iter().peekable();
    while let Some(op) = iter.next() {
        match op {
            AtomicOp::Equal(ai, bi) => out.push(ParagraphEntry {
                op: ParagraphOp::Equal,
                a_index: Some(ai),
                b_index: Some(bi),
                chars: None,
            }),
            AtomicOp::Delete(ai) => {
                if let Some(AtomicOp::Insert(bi)) = iter.peek().copied() {
                    iter.next();
                    let chars = Some(char_lcs_runs(&a[ai], &b[bi]));
                    out.push(ParagraphEntry {
                        op: ParagraphOp::Replace,
                        a_index: Some(ai),
                        b_index: Some(bi),
                        chars,
                    });
                } else {
                    out.push(ParagraphEntry {
                        op: ParagraphOp::Delete,
                        a_index: Some(ai),
                        b_index: None,
                        chars: None,
                    });
                }
            }
            AtomicOp::Insert(bi) => {
                if let Some(AtomicOp::Delete(ai)) = iter.peek().copied() {
                    iter.next();
                    let chars = Some(char_lcs_runs(&a[ai], &b[bi]));
                    out.push(ParagraphEntry {
                        op: ParagraphOp::Replace,
                        a_index: Some(ai),
                        b_index: Some(bi),
                        chars,
                    });
                } else {
                    out.push(ParagraphEntry {
                        op: ParagraphOp::Insert,
                        a_index: None,
                        b_index: Some(bi),
                        chars: None,
                    });
                }
            }
        }
    }
    out
}

// ---- Character-level LCS (only inside a Replace pair) -----------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AtomicCharOp {
    Equal(usize, usize),
    Delete(usize),
    Insert(usize),
}

/// Same DP shape as [`paragraph_lcs`], one level down: over `HwpChar`
/// elements instead of paragraphs. No separate cell ceiling — a replaced
/// pair's element counts are individual-paragraph sized, already bounded by
/// the per-container reader limits each input passed through on load.
fn char_lcs(a: &[HwpChar], b: &[HwpChar]) -> Vec<AtomicCharOp> {
    let a_len = a.len();
    let b_len = b.len();
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
    ops
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
/// the atomic ops into WCHAR-offset [`CharRun`]s.
fn char_lcs_runs(a: &[HwpChar], b: &[HwpChar]) -> Vec<CharRun> {
    let ops = char_lcs(a, b);
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
    runs
}

// ---- Structural inventory (D-12) ---------------------------------------

fn structure_diff(
    a: &Document,
    b: &Document,
    a_paragraph_count: usize,
    b_paragraph_count: usize,
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

    StructureDiff {
        sections: (a.sections.len(), b.sections.len()),
        paragraphs: (a_paragraph_count, b_paragraph_count),
        controls,
        tables,
    }
}

/// Per-kind control counts, recursing into table cells/captions and picture
/// captions exactly like `hwp-cli`'s `commands/preservation.rs::collect_paragraph_controls`
/// (this crate cannot depend on that binary crate, so the counting shape is
/// mirrored rather than shared).
fn control_inventory(document: &Document) -> BTreeMap<[u8; 4], usize> {
    let mut counts = BTreeMap::new();
    for paragraph in document.sections.iter().flat_map(|s| s.paragraphs.iter()) {
        for control in &paragraph.controls {
            *counts.entry(control.ctrl_id()).or_default() += 1;
        }
    }
    counts
}

/// Flattened, ordered (rows, cols) for every table in the document.
fn table_geometries(document: &Document) -> Vec<(u16, u16)> {
    document
        .sections
        .iter()
        .flat_map(|s| s.paragraphs.iter())
        .flat_map(|p| &p.controls)
        .filter_map(|control| match control {
            Control::Table(table) => Some((table.rows, table.cols)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(md: &str) -> Document {
        crate::from_markdown::from_markdown(md)
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
