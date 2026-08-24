//! Table styling engine (GONG-03, STYL-01): the display-width primitive, `style_table` and
//! `style_tables`. `style_table` is the pure function that turns a table's own content into
//! header shading, alignment and content-proportional column widths (D-07). Wired into markdown
//! import from `from_markdown.rs::table_paragraph`, and reused unchanged by `style_tables`, the
//! `hwp edit --style-tables` document walker, so the two call sites cannot drift (one styling
//! implementation only).
//!
//! D-08 (byte-stable idempotence) is a purity constraint on `style_table`: every value it
//! computes comes from the table's own cells, its header-row count and its total width — nothing
//! else. No marker, no probe of "already styled", no external state. Re-running it on unchanged
//! content must recompute the identical values and, through value-deduped shape allocation
//! (`find_or_insert`/`find_or_insert_para`), append nothing on the second call.
//!
//! D-11: `style_tables` skips single-column tables (every frame block plan 01/02 emits is one
//! column) so a frame's row 0 is never mistaken for a header row. Column count is part of the
//! table's own content, so the skip costs no marker and D-08's purity survives.

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

use hwp_model::{
    BorderFill, BorderFillId, BorderLine, CharShape, CharShapeId, Control, Document, HwpUnit,
    ParaShape, ParaShapeId, Paragraph, Table,
};

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
///
/// Matches ignoring `tail`: a hwp5 round trip fills an originally-empty `tail` with a
/// deterministic writer-generated payload (`hwp5::write::is_materialized_generated_*_tail`,
/// invisible from here — `hwp-convert` depends on no format crate, hub-and-spoke). `bf` is always
/// freshly built with an EMPTY tail, so matching on every OTHER field means a fresh candidate
/// still recognizes an already-styled entry loaded from a previous write (whose tail was filled
/// in by the writer), instead of seeing the filled tail as a difference and appending a
/// duplicate on every re-application (D-08). The matched, PRE-EXISTING entry (tail and all) is
/// returned untouched — nothing is normalized or overwritten as a side effect.
fn find_or_insert_border_fill(fills: &mut Vec<BorderFill>, bf: BorderFill) -> usize {
    if let Some(i) = fills
        .iter()
        .position(|f| border_fill_eq_ignoring_tail(f, &bf))
    {
        return i;
    }
    fills.push(bf);
    fills.len() - 1
}

fn border_fill_eq_ignoring_tail(a: &BorderFill, b: &BorderFill) -> bool {
    a.attr == b.attr
        && a.sides == b.sides
        && a.diagonal == b.diagonal
        && a.fill_type == b.fill_type
        && a.bg_color == b.bg_color
        && a.hatch == b.hatch
        && a.gradient == b.gradient
}

/// Same D-08 tail-tolerance as [`find_or_insert_border_fill`], for `ParaShape`. Deliberately a
/// local helper rather than reusing `format::find_or_insert_para` (strict equality): that shared
/// helper's other callers (`set_para_align`) always clone an EXISTING run's shape before
/// modifying it, so their candidate's tail is never a fresh empty one and strict equality never
/// mismatches for them — `style_table`'s centered-shape candidate is the one case built from a
/// hardcoded base ([`crate::from_markdown::table_cell_para_shape`]), so it is the one that needs
/// tolerance.
fn find_or_insert_para_ignoring_tail(shapes: &mut Vec<ParaShape>, ps: ParaShape) -> ParaShapeId {
    if let Some(i) = shapes
        .iter()
        .position(|s| para_shape_eq_ignoring_tail(s, &ps))
    {
        return ParaShapeId(i as u16);
    }
    shapes.push(ps);
    ParaShapeId((shapes.len() - 1) as u16)
}

fn para_shape_eq_ignoring_tail(a: &ParaShape, b: &ParaShape) -> bool {
    a.attr1 == b.attr1
        && a.indent == b.indent
        && a.margin_left == b.margin_left
        && a.margin_right == b.margin_right
        && a.spacing_top == b.spacing_top
        && a.spacing_bottom == b.spacing_bottom
        && a.line_spacing_old == b.line_spacing_old
        && a.tab_def_id == b.tab_def_id
        && a.numbering_id == b.numbering_id
        && a.list_level == b.list_level
        && a.border_fill_id == b.border_fill_id
        && a.border_offsets == b.border_offsets
        && a.line_spacing_type == b.line_spacing_type
        && a.line_spacing == b.line_spacing
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
/// 1-based reference id (Pitfall 1). `char_shapes` is the FULL char shape table addressed directly
/// by every run's `CharShapeId` (no base offset — callers that stage char shapes separately from
/// the fixed base palette, as `from_markdown.rs` does, must materialize a full vector first).
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
    char_shapes: &mut Vec<CharShape>,
) -> bool {
    let cols = table.cols as usize;
    if cols == 0 {
        return false;
    }
    // The return value drives the walker's cache invalidation, including clearing a raw-backed
    // generic's `hwpx_raw_xml` — which the HWPX writer cannot reconstruct. Reporting "I ran"
    // instead of "I changed something" made every re-application destroy that XML, so a document
    // carrying an `hp:container` grew on each pass instead of converging. Snapshot first, compare
    // at the end, and report the truth.
    let before: Vec<CellStyleSnapshot> = table.cells.iter().map(CellStyleSnapshot::of).collect();

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
        // A horizontally merged cell owns every column it spans, so its width is the SUM over
        // `col..col + col_span`. Assigning only `widths[col]` shrank a spanning header to its
        // first column's width while the unmerged body row still occupied all of them, leaving
        // the table geometry inconsistent after `--style-tables`.
        let start = cell.col as usize;
        let span = (cell.col_span as usize).max(1);
        let total: i32 = widths.iter().skip(start).take(span).copied().sum();
        if total > 0 {
            cell.width = hwp_model::HwpUnit(total);
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
    let centered_local = find_or_insert_para_ignoring_tail(para_shapes, centered);
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
        if is_header {
            // Rule 5 (bold): each header run becomes the bold variant of its OWN current
            // CharShape (color/size/etc. preserved), not a hardcoded default — an existing
            // document's header runs may carry anything. On the import path the run is already
            // bold (`tb.in_head`), so the "bold variant" is itself; `find_or_insert` finds the
            // same entry and appends nothing (D-08).
            for p in &mut cell.paragraphs {
                for (_, run_id) in &mut p.char_shape_runs {
                    let current = char_shapes
                        .get(run_id.0 as usize)
                        .cloned()
                        .unwrap_or_default();
                    let bold = CharShape {
                        attr: current.attr | (1 << 1),
                        ..current
                    };
                    *run_id = crate::format::find_or_insert(char_shapes, bold);
                }
            }
        }
    }

    table.cells.iter().map(CellStyleSnapshot::of).ne(before)
}

/// `hwp edit --style-tables <preset>`: styles every eligible table already in `doc` through the
/// SAME `style_table` markdown import calls (one styling implementation only, D-08). Returns
/// `(eligible, changed)`: how many multi-column tables the walk visited, and how many of them
/// actually moved. A caller needs both, because "this document has no styleable table" is an
/// unapplied edit while "every table is already styled" is a successful no-op (D-08). Collapsing
/// them into one count made a correct second application fail the fail-closed publish gate.
///
/// Copies `format.rs::restyle_para`'s recursion shape: walks every section paragraph, recursing
/// into `Control::Table` cells and `Control::Generic` paragraph lists, clearing stale cached
/// layout (`line_segs`) on every styled cell paragraph and stale cached XML
/// (`Generic.hwpx_raw_xml`) on any generic control whose nested content changed.
///
/// D-11: a table fixed at exactly one column (every frame block plans 01/02 emit) is skipped —
/// treating its row 0 as a header row would shade 발신명의/기관명 on top of the shape they already
/// carry. Column count is part of the table's own content, so this costs no marker and D-08's
/// purity survives.
pub fn style_tables(doc: &mut Document) -> (usize, usize) {
    let Document {
        header, sections, ..
    } = doc;
    let mut eligible = 0;
    let mut styled = 0;
    for section in sections.iter_mut() {
        for para in &mut section.paragraphs {
            let (e, c) = style_tables_in_para(
                para,
                &mut header.para_shapes,
                &mut header.border_fills,
                &mut header.char_shapes,
            );
            eligible += e;
            styled += c;
        }
    }
    (eligible, styled)
}

/// The subset of a `Cell`'s state `style_table` can change — used only to detect whether a table
/// cell actually changed, so `style_tables` clears cached layout (`line_segs`) exclusively on
/// cells whose styled value moved. Deliberately excludes `line_segs` itself (the thing being
/// invalidated) and unrelated fields (`chars`, `controls`, ...) that `style_table` never touches.
#[derive(PartialEq)]
struct CellStyleSnapshot {
    width: HwpUnit,
    border_fill: BorderFillId,
    paragraph_shapes: Vec<(ParaShapeId, Vec<(u32, CharShapeId)>)>,
}

impl CellStyleSnapshot {
    fn of(cell: &hwp_model::Cell) -> Self {
        Self {
            width: cell.width,
            border_fill: cell.border_fill,
            paragraph_shapes: cell
                .paragraphs
                .iter()
                .map(|p| (p.para_shape, p.char_shape_runs.clone()))
                .collect(),
        }
    }
}

fn style_tables_in_para(
    para: &mut Paragraph,
    para_shapes: &mut Vec<ParaShape>,
    border_fills: &mut Vec<BorderFill>,
    char_shapes: &mut Vec<CharShape>,
) -> (usize, usize) {
    let mut eligible = 0;
    let mut styled = 0;
    for ctrl in &mut para.controls {
        match ctrl {
            Control::Table(t) => {
                // D-11: single-column tables (frame blocks) are never treated as styleable.
                if t.cols > 1 {
                    eligible += 1;
                    let total_width: i32 = t
                        .cells
                        .iter()
                        .filter(|c| c.row == 0)
                        .map(|c| c.width.0)
                        .sum();
                    // Snapshot the fields `style_table` can touch, per cell, BEFORE the call.
                    // Re-styling an already-correctly-styled table (D-08) must leave every one
                    // of these values unchanged; only clear cached layout where a value actually
                    // moved. An unconditional clear would make BodyText/Section look "changed"
                    // to the hwp5 writer on EVERY reapplication (populated line_segs -> empty),
                    // forcing a stream rewrite that churns the CFB container's sector layout
                    // even when nothing visible moved — breaking byte-stability on the second
                    // application even though the styled VALUES are already stable.
                    let before: Vec<CellStyleSnapshot> =
                        t.cells.iter().map(CellStyleSnapshot::of).collect();
                    let processed = style_table(
                        t,
                        1,
                        total_width,
                        para_shapes,
                        0,
                        border_fills,
                        0,
                        char_shapes,
                    );
                    if processed {
                        styled += 1;
                        for (cell, before_cell) in t.cells.iter_mut().zip(before.iter()) {
                            if CellStyleSnapshot::of(cell) != *before_cell {
                                for p in &mut cell.paragraphs {
                                    p.line_segs.clear();
                                }
                            }
                        }
                    }
                }
                // Recurse into nested tables (a table inside a table cell) regardless of
                // whether this table itself was eligible.
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        let (e, c) =
                            style_tables_in_para(p, para_shapes, border_fills, char_shapes);
                        eligible += e;
                        styled += c;
                    }
                }
            }
            Control::Generic(g) => {
                let before = styled;
                for list in &mut g.paragraph_lists {
                    for p in &mut list.paragraphs {
                        let (e, c) =
                            style_tables_in_para(p, para_shapes, border_fills, char_shapes);
                        eligible += e;
                        styled += c;
                    }
                }
                if styled > before {
                    // A nested table ACTUALLY changed, so the captured XML is stale. Gate on the
                    // changed count, never on the eligible count: `hwpx_raw_xml` is the only
                    // lossless serialization source for a raw-backed generic (the HWPX writer
                    // reports `OpaqueControlUnrepresentable` without it), so clearing it on a
                    // no-op re-application destroyed the container and made the file grow on
                    // every pass instead of converging.
                    g.hwpx_raw_xml = None;
                }
            }
            _ => {}
        }
    }
    (eligible, styled)
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
        let mut char_shapes: Vec<CharShape> = Vec::new();
        assert!(style_table(
            &mut table,
            1,
            42520,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
            &mut char_shapes,
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
    fn style_table_header_bold_from_current_shape_preserves_color_and_is_idempotent() {
        // Header run starts on a NON-bold custom CharShape (a red 12pt shape at id 0) — an
        // arbitrary existing document's header run is "whatever it is", not a hardcoded default.
        let red_12pt = CharShape {
            base_size: 1200,
            text_color: 0x0000_00FF,
            ..CharShape::default()
        };
        let mut table = make_table(&[&["헤더가나다라마바사아자차카"]]);
        for cell in &mut table.cells {
            for p in &mut cell.paragraphs {
                p.char_shape_runs = vec![(0, hwp_model::CharShapeId(0))];
            }
        }
        let mut para_shapes = Vec::new();
        let mut border_fills = Vec::new();
        let mut char_shapes = vec![red_12pt.clone()];

        style_table(
            &mut table,
            1,
            42520,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
            &mut char_shapes,
        );
        let run_id_1 = table.cells[0].paragraphs[0].char_shape_runs[0].1;
        let shape_1 = char_shapes[run_id_1.0 as usize].clone();
        assert!(shape_1.is_bold(), "헤더 글자는 굵게 처리된다");
        assert_eq!(
            shape_1.text_color, 0x0000_00FF,
            "색상 등 기존 속성은 보존된다"
        );
        assert_eq!(shape_1.base_size, 1200, "크기 등 기존 속성은 보존된다");
        let len_after_first = char_shapes.len();

        // Second application: the run already points at the bold shape, so "the bold variant of
        // a bold shape is itself" — find_or_insert finds it and appends nothing (D-08).
        style_table(
            &mut table,
            1,
            42520,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
            &mut char_shapes,
        );
        let run_id_2 = table.cells[0].paragraphs[0].char_shape_runs[0].1;
        assert_eq!(run_id_2, run_id_1, "재적용해도 같은 글자모양을 가리킨다");
        assert_eq!(
            char_shapes.len(),
            len_after_first,
            "재적용해도 글자모양 테이블이 자라지 않는다 (D-08)"
        );
    }

    #[test]
    fn style_table_widths_proportional_to_column_weight() {
        // weight[0]=10, weight[1]=15 (10*3=30 > 15, so the 2-col label:value special case does
        // not trigger — the plain proportional branch is exercised).
        let mut table = make_table(&[&["AAAAAAAAAA", "AAAAAAAAAAAAAAA"]]);
        let mut para_shapes = Vec::new();
        let mut border_fills = Vec::new();
        let mut char_shapes: Vec<CharShape> = Vec::new();
        style_table(
            &mut table,
            0,
            50000,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
            &mut char_shapes,
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
        let mut char_shapes: Vec<CharShape> = Vec::new();
        style_table(
            &mut table,
            0,
            42520,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
            &mut char_shapes,
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
        let mut char_shapes: Vec<CharShape> = Vec::new();
        style_table(
            &mut table,
            0,
            42521,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
            &mut char_shapes,
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
        let mut char_shapes: Vec<CharShape> = Vec::new();
        style_table(
            &mut table,
            0,
            42520,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
            &mut char_shapes,
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
        let mut char_shapes: Vec<CharShape> = Vec::new();
        style_table(
            &mut table,
            0,
            42520,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
            &mut char_shapes,
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
        let mut char_shapes: Vec<CharShape> = Vec::new();
        style_table(
            &mut table,
            0,
            42520,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
            &mut char_shapes,
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
        let mut char_shapes: Vec<CharShape> = Vec::new();

        style_table(
            &mut table,
            1,
            42520,
            &mut para_shapes,
            0,
            &mut border_fills,
            0,
            &mut char_shapes,
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
            &mut char_shapes,
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

    // ── #133 review regressions ──

    /// A horizontally merged cell owns every column it spans, so its width is the SUM over
    /// `col..col + col_span`. Assigning only `widths[col]` shrank a spanning header to its first
    /// column's width while the unmerged body row still occupied all of them.
    #[test]
    fn merged_cell_width_is_the_sum_of_the_columns_it_spans() {
        let mut table = make_table(&[&["머리", ""], &["짧", "아주 긴 본문 내용입니다"]]);
        // Turn row 0 into one cell spanning both columns.
        table.cells.retain(|c| !(c.row == 0 && c.col == 1));
        table.row_cell_counts[0] = 1;
        for cell in &mut table.cells {
            if cell.row == 0 {
                cell.col_span = 2;
            }
        }

        let (mut ps, mut bf, mut cs) = (Vec::new(), Vec::new(), Vec::new());
        style_table(&mut table, 1, 40000, &mut ps, 0, &mut bf, 0, &mut cs);

        let spanning = table.cells.iter().find(|c| c.row == 0).unwrap().width.0;
        let body: i32 = table
            .cells
            .iter()
            .filter(|c| c.row == 1)
            .map(|c| c.width.0)
            .sum();
        assert_eq!(
            spanning, body,
            "a cell spanning both columns must be as wide as both body cells together"
        );
    }

    /// `style_table`'s return value drives the walker's cache invalidation, including clearing a
    /// raw-backed generic's `hwpx_raw_xml` — which the HWPX writer cannot reconstruct. It must
    /// report "I changed something", never merely "I ran".
    #[test]
    fn style_table_reports_change_not_mere_processing() {
        let mut table = make_table(&[&["이름", "소속"], &["홍길동", "총무처"]]);
        let (mut ps, mut bf, mut cs) = (Vec::new(), Vec::new(), Vec::new());

        assert!(
            style_table(&mut table, 1, 40000, &mut ps, 0, &mut bf, 0, &mut cs),
            "first application changes the table"
        );
        assert!(
            !style_table(&mut table, 1, 40000, &mut ps, 0, &mut bf, 0, &mut cs),
            "re-styling an already-styled table changes nothing and must say so"
        );
    }
}
