//! Table styling engine (GONG-03, STYL-01): the display-width primitive and `style_table`, the
//! pure function that turns a table's own content into header shading, alignment and
//! content-proportional column widths (D-07). Wired into markdown import from
//! `from_markdown.rs::table_paragraph`, and reused unchanged by the `hwp edit --style-tables`
//! walker (a later phase) so the two call sites cannot drift.
//!
//! D-08 (byte-stable idempotence) is a purity constraint on `style_table`: every value it
//! computes comes from the table's own cells, its header-row count and its total width — nothing
//! else. No marker, no probe of "already styled", no external state. Re-running it on unchanged
//! content must recompute the identical values and, through value-deduped shape allocation
//! (`find_or_insert`/`find_or_insert_para`), append nothing on the second call.

/// Whether `ch` counts as 2 half-width columns (a "wide" character) rather than 1.
///
/// Deliberately a simple per-codepoint range table, not a Unicode East-Asian-Width
/// implementation: it does not handle combining marks, zero-width characters or grapheme
/// clusters, and does not consult font metrics. The corpus evidence behind D-07
/// (`style-patterns.md` §Table conventions) is itself a per-glyph column count — "2타 = one
/// Hangul glyph = two half-width columns" (`korean-official-format.md` §2) — not a typographic
/// measurement, so this ceiling matches what the evidence actually measures.
fn is_wide(ch: char) -> bool {
    matches!(ch as u32,
        0x1100..=0x11FF   // Hangul Jamo
        | 0x3130..=0x318F // Hangul Compatibility Jamo
        | 0x3200..=0x32FF // Enclosed CJK letters and months
        | 0x3000..=0x303F // CJK symbols and punctuation
        | 0x3040..=0x30FF // Hiragana, Katakana
        | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
        | 0x4E00..=0x9FFF // CJK Unified Ideographs
        | 0xAC00..=0xD7A3 // Hangul Syllables
        | 0xFF00..=0xFF60 // Fullwidth forms
        | 0xFFE0..=0xFFE6 // Fullwidth signs
    )
}

/// Half-width column count for `text` — a Hangul syllable (or other wide character) counts 2,
/// everything else (ASCII, digits, plain punctuation, whitespace) counts 1. Total over
/// `text.chars()`: no allocation, no recursion, no `.unwrap()`, no panic path (T-02.4-07). Leading
/// and trailing whitespace are counted, not trimmed, so the measure is stable under the exact
/// cell text.
pub fn display_width(text: &str) -> usize {
    text.chars().map(|c| if is_wide(c) { 2 } else { 1 }).sum()
}

use hwp_model::{BorderFill, BorderFillId, BorderLine, ParaShape, ParaShapeId, Table};

/// Narrow-column threshold (D-07 rule 6): a column whose widest cell measures this many columns
/// or fewer gets its cells centered rather than left/justified.
const NARROW_COLUMN_MAX: usize = 8;

/// Minimum column width (D-07 rule 2), matching the corpus-observed sequence-number-column floor
/// (`style-patterns.md` §Table conventions, ≈12mm).
const MIN_COL_WIDTH: i32 = 3400;

/// The header-shade `BorderFill` (D-07 rule 5): solid black sides matching `TABLE_BORDER_FILL`'s
/// look (`from_markdown.rs`'s `solid_line`: `line_type: 1, width: 1, color: 0`), `#F2F2F2`
/// background — the most frequent header shade in the corpus (`style-patterns.md`).
fn header_shade_fill() -> BorderFill {
    let solid = BorderLine {
        line_type: 1,
        width: 1,
        color: 0,
    };
    BorderFill {
        sides: [solid; 4],
        fill_type: 1,
        bg_color: Some(0x00F2_F2F2),
        ..BorderFill::default()
    }
}

/// Value-dedup append-only allocator for `BorderFill`, mirroring `format::find_or_insert`'s
/// contract for `CharShape` (Pitfall 5: append-only, never insert in the middle). Returns the
/// 0-based position of `bf` within `fills`.
fn find_or_insert_border_fill(fills: &mut Vec<BorderFill>, bf: BorderFill) -> usize {
    if let Some(i) = fills.iter().position(|f| *f == bf) {
        return i;
    }
    fills.push(bf);
    fills.len() - 1
}

/// Rule 1/2/3/4 — per-column widths from per-column content weight, integer arithmetic only (no
/// float, no `HashMap` iteration order: both would break D-08's determinism).
fn column_widths(weights: &[usize], total_width: i32, cols: usize) -> Vec<i32> {
    let sum: usize = weights.iter().sum();
    let mut raw: Vec<i64> = if sum == 0 {
        // T-02.4-08: an all-empty table has no content signal to be proportional to — even split
        // rather than a division by zero.
        let each = i64::from(total_width) / cols as i64;
        vec![each; cols]
    } else if cols == 2 && weights[0] * 3 <= weights[1] {
        // Rule 3: label's weight is at most a third of value's — fixed 1:4 ratio (inside the
        // observed 1:3-1:5 band) rather than the raw proportional split.
        let label = i64::from(total_width) / 5;
        vec![label, i64::from(total_width) - label]
    } else {
        // Rule 1/4: proportional to weight. Equal weights divide identically for every column
        // (same numerator/denominator), which is rule 4's even split.
        weights
            .iter()
            .map(|&w| (i64::from(total_width) * w as i64) / sum as i64)
            .collect()
    };

    // Rule 2: clamp up to the minimum, then redistribute the remainder (from clamping and from
    // integer-division rounding) onto the last unclamped column so the total still sums exactly
    // to `total_width`.
    let mut clamped = vec![false; cols];
    for (w, is_clamped) in raw.iter_mut().zip(clamped.iter_mut()) {
        if *w < i64::from(MIN_COL_WIDTH) {
            *w = i64::from(MIN_COL_WIDTH);
            *is_clamped = true;
        }
    }
    let sum_now: i64 = raw.iter().sum();
    let remainder = i64::from(total_width) - sum_now;
    if remainder != 0 {
        // ponytail: a table with every column below the minimum (total_width < cols * 3400) has
        // no unclamped column to absorb the remainder — fall back to the last column regardless,
        // trading its floor guarantee for an exact total. Real official-document tables stay well
        // clear of this (BODY_WIDTH is tens of thousands of HWPUNIT).
        let target = (0..cols).rev().find(|&i| !clamped[i]).unwrap_or(cols - 1);
        raw[target] += remainder;
    }
    raw.into_iter().map(|w| w as i32).collect()
}

/// Styles `table` purely from its own content (D-08): header-row shading + centering, narrow-
/// column centering, and content-proportional column widths (D-07). Reads nothing but `table`'s
/// own cells, `header_rows` and `total_width` — no document state, no marker, no probe.
///
/// `para_shapes`/`border_fills` are the vectors new shapes are value-deduped and appended into;
/// `para_shape_base`/`border_fill_base` are the count of entries that will precede them once
/// merged into the document header (0 when the caller passes the vectors that already stand for
/// the final, complete header tables — the common case in tests and in `hwp edit --style-tables`;
/// nonzero when the caller is still assembling staging vectors, as `from_markdown.rs` does while
/// the header does not exist yet). `ParaShapeId` is 0-based; `BorderFillId` is the on-disk
/// 1-based reference id (Pitfall 1).
///
/// Returns `false` only for a degenerate 0-column table (nothing to style); `true` otherwise.
#[allow(clippy::too_many_arguments)]
pub fn style_table(
    table: &mut Table,
    header_rows: usize,
    total_width: i32,
    para_shapes: &mut Vec<ParaShape>,
    para_shape_base: u16,
    border_fills: &mut Vec<BorderFill>,
    border_fill_base: u16,
) -> bool {
    let cols = table.cols as usize;
    if cols == 0 {
        return false;
    }

    // Rule 1: per-column weight = max display_width over every cell's own text in that column,
    // header included.
    let mut weights = vec![0usize; cols];
    for cell in &table.cells {
        if let Some(w) = weights.get_mut(cell.col as usize) {
            *w = (*w).max(display_width(&crate::csv::cell_text(cell)));
        }
    }

    let widths = column_widths(&weights, total_width, cols);
    for cell in &mut table.cells {
        if let Some(&w) = widths.get(cell.col as usize) {
            cell.width = hwp_model::HwpUnit(w);
        }
    }

    // Rule 5/6: header shading (append-only value dedup) and centering. The header's centered
    // ParaShape is deliberately the SAME shape reused for narrow body columns (rule 6 says "the
    // same centred ParaShape"), so only one new ParaShape entry is ever needed.
    let base_para = crate::from_markdown::table_cell_para_shape();
    let centered = ParaShape {
        // Clear alignment bits 2-4, then set them to 3 (center) — Pitfall 2: this is the
        // paragraph-shape property; `Cell.list_attr`'s vertical-center bit is untouched below.
        attr1: (base_para.attr1 & !(0x7 << 2)) | (3 << 2),
        ..base_para
    };
    let centered_local = crate::format::find_or_insert_para(para_shapes, centered);
    let centered_id = ParaShapeId(para_shape_base + centered_local.0);

    let shade_local = find_or_insert_border_fill(border_fills, header_shade_fill());
    let shade_id = BorderFillId(border_fill_base + shade_local as u16 + 1);

    for cell in &mut table.cells {
        let is_header = (cell.row as usize) < header_rows;
        let narrow = weights
            .get(cell.col as usize)
            .is_some_and(|&w| w <= NARROW_COLUMN_MAX);
        if is_header {
            cell.border_fill = shade_id;
        }
        if is_header || narrow {
            for p in &mut cell.paragraphs {
                p.para_shape = centered_id;
            }
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── display_width ──

    #[test]
    fn display_width_empty() {
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn display_width_ascii() {
        assert_eq!(display_width("abcdef"), 6);
        assert_eq!(display_width("123"), 3);
    }

    #[test]
    fn display_width_hangul_syllables() {
        assert_eq!(display_width("가나다"), 6);
    }

    #[test]
    fn display_width_mixed() {
        assert_eq!(display_width("가a"), 3);
    }

    #[test]
    fn display_width_fullwidth_forms() {
        assert_eq!(display_width("\u{FF21}"), 2); // fullwidth A
        assert_eq!(display_width("\u{FFE5}"), 2); // fullwidth yen sign
    }

    #[test]
    fn display_width_cjk_ideographs() {
        assert_eq!(display_width("\u{4E2D}"), 2); // 中
    }

    #[test]
    fn display_width_hiragana_katakana() {
        assert_eq!(display_width("\u{3042}"), 2); // あ
        assert_eq!(display_width("\u{30A2}"), 2); // ア
    }

    #[test]
    fn display_width_hangul_jamo_and_compat_jamo() {
        assert_eq!(display_width("\u{1100}"), 2); // ᄀ (Hangul Jamo)
        assert_eq!(display_width("\u{3131}"), 2); // ㄱ (Hangul Compatibility Jamo)
    }

    #[test]
    fn display_width_cjk_ext_a_and_enclosed_letters() {
        assert_eq!(display_width("\u{3400}"), 2); // 㐀 (CJK Ext A)
        assert_eq!(display_width("\u{3220}"), 2); // ㈠ (enclosed CJK letter)
    }

    #[test]
    fn display_width_cjk_punctuation() {
        assert_eq!(display_width("\u{3001}"), 2); // 、
    }

    #[test]
    fn display_width_spaces_not_trimmed() {
        assert_eq!(display_width(" a "), 3);
        assert_eq!(display_width("  "), 2);
    }

    #[test]
    fn display_width_total_no_panic_on_any_char() {
        // Total function: every char in the BMP maps to 1 or 2, never panics.
        for c in ['\u{0}', '\u{FFFF}', '\u{10FFFF}'] {
            let _ = display_width(&c.to_string());
        }
    }
}
