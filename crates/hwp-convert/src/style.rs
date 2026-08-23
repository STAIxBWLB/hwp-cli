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

    // ── style_table ──

    use hwp_model::{Cell, HwpChar, HwpUnit, Paragraph};

    /// A cell holding `text` as its only paragraph's own text. `border_fill`/`para_shape` start at
    /// sentinel values (99) distinguishable from anything `style_table` assigns (`BorderFillId(1)`/
    /// `ParaShapeId(0)` for `border_fill_base`/`para_shape_base` of 0), so tests can tell "touched"
    /// from "untouched" cells apart from an accidental match with a default value.
    fn cell_with_text(text: &str, col: u16, row: u16) -> Cell {
        Cell {
            list_attr: 0,
            col,
            row,
            col_span: 1,
            row_span: 1,
            width: HwpUnit(0),
            height: HwpUnit(1700),
            margins: [0; 4],
            border_fill: BorderFillId(99),
            header_tail: Vec::new(),
            paragraphs: vec![Paragraph {
                chars: text.chars().map(HwpChar::Text).collect(),
                para_shape: ParaShapeId(99),
                ..Paragraph::default()
            }],
        }
    }

    fn make_table(rows: &[&[&str]]) -> Table {
        let row_count = rows.len();
        let col_count = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut cells = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            for (c, text) in row.iter().enumerate() {
                cells.push(cell_with_text(text, c as u16, r as u16));
            }
        }
        Table {
            common_data: Vec::new(),
            placement: None,
            attr: 0,
            rows: row_count as u16,
            cols: col_count as u16,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: vec![col_count as u16; row_count],
            border_fill: BorderFillId(99),
            table_tail: Vec::new(),
            caption: None,
            cells,
            extras: Vec::new(),
        }
    }

    #[test]
    fn style_table_header_row_shaded_and_centered_body_row_untouched() {
        // Both columns are wide (weight > 8) in every row, so rule 6 (narrow-column centering)
        // cannot contaminate this test — only the header rule (row 0) should touch anything.
        let mut table = make_table(&[
            &["헤더가나다라마바사아자차카", "헤더의두번째칸입니다길게씀"],
            &["본문가나다라마바사아자차카", "본문의두번째칸입니다길게씀"],
        ]);
        let mut para_shapes = Vec::new();
        let mut border_fills = Vec::new();
        assert!(style_table(
            &mut table,
            1,
            42520,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
        ));

        for cell in &table.cells {
            if cell.row == 0 {
                assert_eq!(
                    cell.border_fill,
                    BorderFillId(1),
                    "헤더 셀은 셰이딩을 받는다"
                );
                assert_eq!(
                    cell.paragraphs[0].para_shape,
                    ParaShapeId(0),
                    "헤더 셀은 가운데 정렬을 받는다"
                );
            } else {
                assert_eq!(
                    cell.border_fill,
                    BorderFillId(99),
                    "본문 셀은 헤더 셰이딩을 받지 않는다"
                );
                assert_eq!(
                    cell.paragraphs[0].para_shape,
                    ParaShapeId(99),
                    "넓은 칸의 본문 셀은 가운데 정렬 대상이 아니다"
                );
            }
        }
    }

    #[test]
    fn style_table_widths_proportional_to_column_weight() {
        // weight[0]=10, weight[1]=15 (10*3=30 > 15, so the 2-col label:value special case does
        // not trigger — the plain proportional branch is exercised).
        let mut table = make_table(&[&["AAAAAAAAAA", "AAAAAAAAAAAAAAA"]]);
        let mut para_shapes = Vec::new();
        let mut border_fills = Vec::new();
        style_table(
            &mut table,
            0,
            50000,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
        );

        let width_of = |c: u16| {
            table
                .cells
                .iter()
                .find(|cell| cell.col == c)
                .unwrap()
                .width
                .0
        };
        assert_eq!(width_of(0), 20000);
        assert_eq!(width_of(1), 30000);
    }

    #[test]
    fn style_table_min_column_width_floor() {
        // Column 0 is a single character — its raw proportional share is well under the 3400
        // HWPUNIT floor and must be clamped up to it.
        let mut table = make_table(&[&["A", "AAAAAAAAAAAAAAAAAAAA", "AAAAAAAAAAAAAAAAAAAA"]]);
        let mut para_shapes = Vec::new();
        let mut border_fills = Vec::new();
        style_table(
            &mut table,
            0,
            42520,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
        );

        let width_of = |c: u16| {
            table
                .cells
                .iter()
                .find(|cell| cell.col == c)
                .unwrap()
                .width
                .0
        };
        assert_eq!(
            width_of(0),
            MIN_COL_WIDTH,
            "1글자 칸도 최소 3400 hwpu 미만으로 내려가지 않는다"
        );
        let total: i32 = (0..3).map(width_of).sum();
        assert_eq!(total, 42520, "클램프 후에도 전체 폭 합은 그대로다");
    }

    #[test]
    fn style_table_widths_sum_exactly_with_rounding_remainder() {
        // Equal weights, but total_width is not evenly divisible by the column count — the
        // rounding remainder must land on the last column so the sum stays exact.
        let mut table = make_table(&[&["AAAAA", "AAAAA", "AAAAA"]]);
        let mut para_shapes = Vec::new();
        let mut border_fills = Vec::new();
        style_table(
            &mut table,
            0,
            42521,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
        );

        let width_of = |c: u16| {
            table
                .cells
                .iter()
                .find(|cell| cell.col == c)
                .unwrap()
                .width
                .0
        };
        let (w0, w1, w2) = (width_of(0), width_of(1), width_of(2));
        assert_eq!(w0, w1, "나머지를 받지 않는 두 칸은 동일하다");
        assert_eq!(w2, w0 + 2, "반올림 나머지는 마지막 칸으로 간다");
        assert_eq!(w0 + w1 + w2, 42521, "합은 정확히 total_width와 같다");
    }

    #[test]
    fn style_table_two_column_label_value_ratio() {
        // weight[0]=1, weight[1]=20 (1*3=3 <= 20 triggers the special case): fixed 1:4 ratio.
        let mut table = make_table(&[&["A", "AAAAAAAAAAAAAAAAAAAA"]]);
        let mut para_shapes = Vec::new();
        let mut border_fills = Vec::new();
        style_table(
            &mut table,
            0,
            42520,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
        );

        let width_of = |c: u16| {
            table
                .cells
                .iter()
                .find(|cell| cell.col == c)
                .unwrap()
                .width
                .0
        };
        let (label, value) = (width_of(0), width_of(1));
        assert_eq!(label, 8504);
        assert_eq!(value, 34016);
        assert_eq!(label + value, 42520);
        // Inside the observed 1:3-1:5 band.
        assert!((3.0..=5.0).contains(&(value as f64 / label as f64)));
    }

    #[test]
    fn style_table_narrow_column_centered_wide_column_not() {
        // No header row (header_rows=0) isolates rule 6 from rule 5. Column 0's widest cell
        // measures exactly 8 (the centering threshold); column 1's measures exactly 9 (just over).
        let mut table = make_table(&[&["12345678", "123456789"]]);
        let mut para_shapes = Vec::new();
        let mut border_fills = Vec::new();
        style_table(
            &mut table,
            0,
            42520,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
        );

        let para_shape_of = |c: u16| {
            table
                .cells
                .iter()
                .find(|cell| cell.col == c)
                .unwrap()
                .paragraphs[0]
                .para_shape
        };
        assert_eq!(
            para_shape_of(0),
            ParaShapeId(0),
            "폭 8 이하 칸은 가운데 정렬된다"
        );
        assert_eq!(
            para_shape_of(1),
            ParaShapeId(99),
            "폭 9 이상 칸은 가운데 정렬되지 않는다"
        );
    }

    #[test]
    fn style_table_equal_weights_stay_even_split() {
        let mut table = make_table(&[&["AAAA", "AAAA", "AAAA", "AAAA"]]);
        let mut para_shapes = Vec::new();
        let mut border_fills = Vec::new();
        style_table(
            &mut table,
            0,
            42520,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
        );

        let widths: Vec<i32> = (0..4)
            .map(|c| {
                table
                    .cells
                    .iter()
                    .find(|cell| cell.col == c)
                    .unwrap()
                    .width
                    .0
            })
            .collect();
        assert_eq!(widths, vec![10630; 4]);
    }

    #[test]
    fn style_table_idempotent_on_second_call() {
        let mut table = make_table(&[
            &["헤더1", "헤더2매우길게작성합니다예시본문"],
            &["본문1", "본문2매우길게작성합니다예시본문"],
        ]);
        let mut para_shapes = Vec::new();
        let mut border_fills = Vec::new();

        style_table(
            &mut table,
            1,
            42520,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
        );
        let widths_1: Vec<i32> = table.cells.iter().map(|c| c.width.0).collect();
        let borders_1: Vec<BorderFillId> = table.cells.iter().map(|c| c.border_fill).collect();
        let paras_1: Vec<ParaShapeId> = table
            .cells
            .iter()
            .map(|c| c.paragraphs[0].para_shape)
            .collect();
        let (para_len_1, border_len_1) = (para_shapes.len(), border_fills.len());

        style_table(
            &mut table,
            1,
            42520,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
        );
        let widths_2: Vec<i32> = table.cells.iter().map(|c| c.width.0).collect();
        let borders_2: Vec<BorderFillId> = table.cells.iter().map(|c| c.border_fill).collect();
        let paras_2: Vec<ParaShapeId> = table
            .cells
            .iter()
            .map(|c| c.paragraphs[0].para_shape)
            .collect();

        assert_eq!(widths_1, widths_2, "재적용해도 열 폭은 그대로다");
        assert_eq!(borders_1, borders_2, "재적용해도 테두리 참조는 그대로다");
        assert_eq!(paras_1, paras_2, "재적용해도 문단모양 참조는 그대로다");
        assert_eq!(
            para_shapes.len(),
            para_len_1,
            "재적용해도 문단모양 테이블이 자라지 않는다 (D-08)"
        );
        assert_eq!(
            border_fills.len(),
            border_len_1,
            "재적용해도 테두리 테이블이 자라지 않는다 (D-08)"
        );
    }

    #[test]
    fn style_table_every_preset_styles_header_row() {
        use crate::from_markdown::{
            MarkdownImportOptions, TABLE_BORDER_FILL, from_markdown_report,
        };
        use crate::official::OfficialPreset;
        use hwp_model::Control;

        let md = "| 가나다라마바사아자차카타파하 | 값 |\n|---|---|\n| 1 | 2 |\n";
        for preset in [
            OfficialPreset::Official,
            OfficialPreset::Report,
            OfficialPreset::Plan,
            OfficialPreset::Notice,
            OfficialPreset::Minutes,
            OfficialPreset::Press,
        ] {
            let opts = MarkdownImportOptions {
                preset: Some(preset),
                ..Default::default()
            };
            let (doc, warnings) = from_markdown_report(md, &opts).expect("import ok");
            assert!(warnings.is_empty(), "{preset:?}: {warnings:?}");
            let table = doc.sections[0]
                .paragraphs
                .iter()
                .flat_map(|p| &p.controls)
                .find_map(|c| match c {
                    Control::Table(t) => Some(t),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("표 없음: {preset:?}"));
            let header_cell = table
                .cells
                .iter()
                .find(|c| c.row == 0)
                .unwrap_or_else(|| panic!("헤더 행 없음: {preset:?}"));
            assert_ne!(
                header_cell.border_fill,
                BorderFillId(TABLE_BORDER_FILL),
                "헤더 셰이딩 미적용: {preset:?}"
            );
        }
    }
}
