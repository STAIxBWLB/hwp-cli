//! Synthesize line layout (PARA_LINE_SEG) for authored documents originating from Markdown/HWPX.
//!
//! Hangul 5.1.x renders body text at zero height without line-layout caches,
//! producing black bars, blank content or corruption. One-line-per-paragraph synthesis
//! causes overlap and collapsed heights in multiline paragraphs and long table cells.
//! Measure character widths with font shaping (shape_range), wrap greedily at body width,
//! and generate one PARA_LINE_SEG per line. Body v_pos is page-relative; cell v_pos is cumulative.
//!
//! Accurate wrapping requires the same font as Hangul (HCR Batang).

use hwp_model::{Control, Document, LineSeg, Paragraph, Table};

use crate::fonts::FontStore;
use crate::issues::RenderIssueAccumulator;
use crate::layout::TAB_INTERVAL_PT;
use crate::shape::{InlineItem, glyph_source_sequences, shape_range, source_has_no_break_space};

/// Fixed vertical table-block padding in HWPUNIT, added by genuine Hangul to the whole table.
///
/// Measurements of the genuine first-paragraph sample (5.1.1.0) and work_report.hwp (5.0.2.4)
/// both yield table advance minus summed row heights = 566 (2.0mm is 566.93 HWPUNIT).
/// It stays constant regardless of table top/bottom margins (141 each) or cell side margins,
/// so treat it as table-specific outer padding added once, separately from row heights.
const TABLE_BLOCK_PADDING: i32 = 566;

/// Synthesize line layout for the whole authored document, including body and table-cell paragraphs.
/// To match Hangul's line breaks, store must have HCR Batang loaded.
pub fn synthesize_linesegs(
    doc: &mut Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
) {
    let snap = doc.clone();
    let floating_sections: Vec<bool> = doc
        .sections
        .iter()
        .map(|section| {
            section
                .paragraphs
                .iter()
                .flat_map(|p| &p.controls)
                .any(|c| {
                    matches!(c,
            Control::Table(t) if t.placement.as_ref().is_some_and(|p| !p.treat_as_char))
                })
        })
        .collect();
    // Floating-table sections reuse the renderer's legal row/cell fragment flow.
    // Other sections keep their measured Hancom inline-table advance.
    let flow_anchors = if floating_sections.iter().any(|floating| *floating) {
        for (si, section) in doc.sections.iter_mut().enumerate() {
            if !floating_sections[si] {
                continue;
            }
            let width = section
                .section_def()
                .and_then(|d| d.page.as_ref())
                .map_or(42520, |pg| {
                    pg.width.0 - pg.margin_left.0 - pg.margin_right.0
                });
            for para in &mut section.paragraphs {
                fill_nested(para, width, &snap, store, warnings);
            }
        }
        let mut flow = doc.clone();
        for section in &mut flow.sections {
            for para in &mut section.paragraphs {
                para.line_segs.clear();
            }
        }
        Some(crate::layout::synthesized_body_anchors(&flow, store))
    } else {
        None
    };
    for si in 0..doc.sections.len() {
        let section_anchors = flow_anchors.as_ref().filter(|_| floating_sections[si]);
        let page = snap.sections[si]
            .section_def()
            .and_then(|d| d.page.as_ref());
        let body_width = page.map_or(42520, |pg| {
            pg.width.0 - pg.margin_left.0 - pg.margin_right.0
        });
        // Body height excludes top/bottom margins. Lines/tables crossing this height move
        // to a new page and restart v_pos at zero: genuine multipage caches are page-relative.
        // Measured in a genuine university HWPX: body vertpos resets on every page,
        // and its maximum 59668 is below body height. Without resetting, accumulated v_pos
        // can greatly exceed page height (such as 354408), which Hangul flags as corruption.
        let content_h = page
            .map_or(75686, |pg| {
                pg.height.0 - pg.margin_top.0 - pg.margin_bottom.0
            })
            .max(1);
        let mut v_pos = 0i32;
        for pi in 0..doc.sections[si].paragraphs.len() {
            // Include paragraph space-before/after (spacing_top/bottom) in v_pos. Hangul uses
            // cached v_pos for vertical placement; omitting spacing compresses paragraphs
            // together and loses heading space-before, causing vertical displacement.
            // Paragraph gaps add the preceding space-after and current space-before. Omit
            // space-before at a section's first paragraph (genuine v_pos=0). Explicit
            // breaks also omit it for consistency; that case is unverified against Hangul.
            let (sp_top, sp_bottom) = snap
                .header
                .para_shapes
                .get(snap.sections[si].paragraphs[pi].para_shape.0 as usize)
                .map_or((0, 0), |ps| (ps.spacing_top, ps.spacing_bottom));
            let explicit_break = snap.sections[si].paragraphs[pi].header.break_type & 0x04 != 0;
            if explicit_break {
                v_pos = 0;
            }
            if pi > 0 && !explicit_break {
                v_pos += sp_top;
            }
            // Floating-table measurement already populated nested caches.
            if section_anchors.is_none() {
                fill_nested(
                    &mut doc.sections[si].paragraphs[pi],
                    body_width,
                    &snap,
                    store,
                    warnings,
                );
            }
            // Total table height in this paragraph.
            let mut table_total = 0i32;
            for ctrl in &doc.sections[si].paragraphs[pi].controls {
                if let Control::Table(t) = ctrl {
                    table_total += table_height(t);
                }
            }
            // Move the whole table to the next page when it does not fit in the current remainder.
            if section_anchors.is_none()
                && table_total > 0
                && v_pos > 0
                && v_pos + table_total > content_h
            {
                v_pos = 0;
            }
            // A table anchor's cache starts at the incoming cursor after the preceding paragraph.
            // Genuine first-paragraph sample: body advance 1600, then table-anchor v_pos=1600.
            if let Some(anchors) = section_anchors {
                v_pos = anchors[si][pi].v_pos;
            }
            let anchor_v = v_pos;
            let src = &snap.sections[si].paragraphs[pi];
            let mut segs = compute_linesegs(
                store, &snap, src, body_width, content_h, &mut v_pos, warnings,
            );
            if section_anchors.is_some_and(|anchors| anchors[si][pi].starts_page)
                && let Some(first) = segs.first_mut()
            {
                // HWP 5.0 §4.3.4 (PARA_LINE_SEG): bit0 marks the first line of a page.
                first.flags |= 0x1;
            }
            doc.sections[si].paragraphs[pi].line_segs = segs;
            // A table paragraph advances by table height rather than one line_advance, keeping
            // following text from overlapping the table (which Hangul flags as corruption).
            // compute_linesegs has already added one line_advance for the anchor paragraph,
            // so replace the cursor with entry position plus table heights, summing multiple tables.
            if table_total > 0 {
                v_pos = anchor_v + table_total;
            }
            // Space-after moves the next paragraph's first line down accordingly.
            v_pos += sp_bottom;
        }
    }
}

/// Synthesize every nested paragraph list: table captions/cells, controls within cells
/// (including nested tables), picture captions, object paragraph lists and object captions.
/// This matches the lists where hwp-convert walk_para_lists can insert paragraphs.
/// Leaving an inserted paragraph cacheless makes Hangul draw it at zero height (B2);
/// a final list paragraph also contradicts bit31 with an empty PARA_LINE_SEG (B3).
///
/// snap is a separate clone of doc, so doc can be borrowed freely here without conflict
/// while looking up paragraph and character properties.
fn fill_nested(
    para: &mut Paragraph,
    body_width: i32,
    snap: &Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
) {
    for ctrl in &mut para.controls {
        match ctrl {
            Control::Table(t) => {
                if let Some(cap) = &mut t.caption {
                    let w = caption_width(cap.width, body_width);
                    fill_list(
                        &mut cap.paragraphs,
                        w,
                        false,
                        body_width,
                        snap,
                        store,
                        warnings,
                    );
                }
                let margins: Vec<_> = t
                    .cells
                    .iter()
                    .map(|cell| crate::layout::cell_margins(t, cell))
                    .collect();
                for (cell, (ml, mr, _, _)) in t.cells.iter_mut().zip(margins) {
                    let w = (cell.width.0 - ((ml + mr) * 100.0).round() as i32).max(1);
                    // Always recompute cells, as before: their line counts determine table height,
                    // and stale pre-edit caches would misalign the table.
                    fill_list(
                        &mut cell.paragraphs,
                        w,
                        true,
                        body_width,
                        snap,
                        store,
                        warnings,
                    );
                }
            }
            Control::Picture(pic) => {
                if let Some(cap) = &mut pic.caption {
                    let w = caption_width(cap.width, body_width);
                    fill_list(
                        &mut cap.paragraphs,
                        w,
                        false,
                        body_width,
                        snap,
                        store,
                        warnings,
                    );
                }
            }
            Control::Generic(g) => {
                // Text box with gso_shapes: subtract LIST_HEADER inner margins (283 twice) from box width.
                let bw = g
                    .gso_shapes
                    .first()
                    .map_or(body_width, |s| (s.w - 566).max(1));
                for list in &mut g.paragraph_lists {
                    fill_list(
                        &mut list.paragraphs,
                        bw,
                        false,
                        body_width,
                        snap,
                        store,
                        warnings,
                    );
                }
                if let Some(cap) = &mut g.caption {
                    let w = caption_width(cap.width, body_width);
                    fill_list(
                        &mut cap.paragraphs,
                        w,
                        false,
                        body_width,
                        snap,
                        store,
                        warnings,
                    );
                }
            }
            Control::SectionDef(_) => {}
        }
    }
}

/// Caption text width. None means extending into margins, so use body width.
fn caption_width(width: Option<u32>, body_width: i32) -> i32 {
    width
        .and_then(|w| i32::try_from(w).ok())
        .filter(|w| *w > 0)
        .unwrap_or(body_width)
}

/// Fill one paragraph list. Accumulate v_pos per list rather than resetting each paragraph;
/// table_height reads the last paragraph's v_pos as the whole cell's height on that basis.
/// With force=false, preserve paragraphs that already carry line layout.
fn fill_list(
    paras: &mut [Paragraph],
    width: i32,
    force: bool,
    body_width: i32,
    snap: &Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
) {
    let mut v = 0i32;
    for para in paras.iter_mut() {
        fill_nested(para, body_width, snap, store, warnings);
        if !force && !para.line_segs.is_empty() {
            continue;
        }
        // Do not paginate inside lists (content_h=MAX); synthesize_linesegs handles overflow
        // at the table/object level.
        let segs = compute_box_linesegs(store, snap, para, width, &mut v, warnings);
        para.line_segs = segs;
    }
}

/// Compute one table's vertical height in HWPUNIT using measured genuine Hangul rules.
///
/// fill_nested must populate cell paragraph line_segs before this function runs.
/// Formula derived from genuine first-paragraph and work_report.hwp measurements:
///
/// ```text
/// Row height rowH = cell.top_margin + cell.bottom_margin + line block.
/// Line block of N lines = last line.v_pos + last line.line_height.
/// With uniform line metrics: (N-1)*line_advance + line_height.
/// Table height = sum over rows of max(rowH among that row's cells) + TABLE_BLOCK_PADDING(566).
/// ```
///
/// Evidence: first-paragraph sample (3 rows, one line/cell, margins top141/bottom141, lh=1000, la=1600)
/// gives 3*(141+1000+141)+566 = 4412, the genuine advance (6012-1600). work_report.hwp's
/// first table (one row, two columns, two lines in one cell) gives (141+(3200+2000)+141)+566
/// = 5482+566 = 6048, also matching genuine advance. Both yield the same 566 (2.0mm) constant.
///
/// Skip row_span>1 cells rather than assigning their full height to their starting row.
/// Derive row heights from row_span==1 cells, matching genuine single-row height measurements.
/// If a row has no single-row cells because all are merged, use a safe fallback height.
fn table_height(table: &Table) -> i32 {
    // Maximum cell height per row (rowH), indexed by Cell.row.
    let mut row_heights = vec![0i32; usize::from(table.rows)];
    for cell in &table.cells {
        // Do not force a merged cell's height onto its starting row; use the fallback below.
        if cell.row_span != 1 {
            continue;
        }
        let r = usize::from(cell.row);
        if r >= row_heights.len() {
            continue;
        }
        // The cell line block is that of its last paragraph alone. fill_nested accumulates
        // v_pos across the cell rather than resetting each paragraph, so the final line's
        // v_pos + line_height is already the whole cell height. Summing paragraph heights
        // would count earlier paragraphs twice because their heights are included in later v_pos.
        let block = cell.paragraphs.last().map_or(0, para_line_block);
        let cell_h = i32::from(cell.margins[2]) + block + i32::from(cell.margins[3]);
        if cell_h > row_heights[r] {
            row_heights[r] = cell_h;
        }
    }
    // Rows containing only row_span>1 cells have zero height; use one fallback line to prevent overlap.
    let fallback = 141 + 1000 + 141; // Top margin + one line (lh) + bottom margin, using genuine defaults.
    let sum: i32 = row_heights
        .iter()
        .map(|&h| if h > 0 { h } else { fallback })
        .sum();
    sum + TABLE_BLOCK_PADDING
}

/// One cell paragraph's line-block height (HWPUNIT): last line.v_pos + last line.line_height.
/// Cell-local v_pos accumulates across paragraphs in fill_nested. The uniform-line
/// shortcut (line count - 1) * line_advance applies only when all line metrics match;
/// mixed sizes use the actual final v_pos plus line height.
fn para_line_block(para: &Paragraph) -> i32 {
    match para.line_segs.last() {
        Some(seg) => seg.v_pos + seg.line_height,
        // Without line layout (theoretically impossible), fall back to one default line.
        None => 1000,
    }
}

fn line_advance_hu(base: i32, kind: u8, value: i32) -> i32 {
    let base = i64::from(base.max(0));
    let value = i64::from(value.max(0));
    let advance = match kind {
        // FIXED is exact and may intentionally overlap adjacent lines.
        1 => value / 2,
        // BETWEEN_LINES adds the serialized margin to the natural line height.
        2 => base + value / 2,
        // AT_LEAST cannot be smaller than the natural line height.
        3 => (value / 2).max(base),
        // PERCENT defaults to Hancom's observed 160% when unspecified.
        _ if value > 0 => base * value / 100,
        _ => base * 160 / 100,
    };
    advance.clamp(0, i64::from(i32::MAX)) as i32
}

fn source_wchar_len(source: &str) -> u32 {
    source.chars().fold(0u32, |length, ch| {
        length.saturating_add(ch.len_utf16() as u32)
    })
}

/// Shared fallback and cache metrics, in HWPUNIT.
pub(crate) struct ParagraphMetrics {
    pub height: i32,
    pub advance: i32,
    pub baseline_gap: i32,
}

pub(crate) fn paragraph_metrics(doc: &Document, para: &Paragraph) -> ParagraphMetrics {
    let base = para
        .char_shape_runs
        .first()
        .and_then(|(_, id)| doc.header.char_shapes.get(id.0 as usize))
        .map_or(
            1000,
            |cs| if cs.base_size > 0 { cs.base_size } else { 1000 },
        );
    let advance = paragraph_advance(doc, para, base);
    let baseline_gap = (i64::from(base) * 85 / 100).clamp(0, i64::from(i32::MAX)) as i32;
    ParagraphMetrics {
        height: base,
        advance,
        baseline_gap,
    }
}

// Derive synthesized line spacing from the version-aware ParaShape fields
// (GG-18, specification table 46). Length-based modes serialize at twice
// the HWPUNIT value. An unspecified percentage retains the observed 160% default.
pub(crate) fn paragraph_advance(doc: &Document, para: &Paragraph, base: i32) -> i32 {
    let ps = doc.header.para_shapes.get(para.para_shape.0 as usize);
    let ls_type = ps.map_or(0, |p| p.line_spacing_type);
    let ls_val = ps.map_or(0, |p| {
        if p.line_spacing_type != 0 || p.line_spacing != 0 {
            p.line_spacing
        } else {
            p.line_spacing_old
        }
    });
    line_advance_hu(base, ls_type, ls_val)
}

/// Retain the prior nested-list synthesis convention: full supplied box width,
/// list-cumulative line advances, and no body paragraph before/after gaps.
/// This compatibility fallback is not evidence for additional native cell geometry.
pub(crate) fn compute_box_linesegs(
    store: &mut FontStore,
    doc: &Document,
    para: &Paragraph,
    width: i32,
    v: &mut i32,
    warnings: &mut RenderIssueAccumulator,
) -> Vec<LineSeg> {
    compute_linesegs(store, doc, para, width, i32::MAX, v, warnings)
}

/// Compute one paragraph's cache using a page-relative or cell-local cursor.
/// Empty paragraphs still receive one line, as required by the HWP5 writer.
pub(crate) fn compute_linesegs(
    store: &mut FontStore,
    doc: &Document,
    para: &Paragraph,
    body_width: i32,
    content_h: i32,
    v_pos: &mut i32,
    warnings: &mut RenderIssueAccumulator,
) -> Vec<LineSeg> {
    // Preserve the measured height/baseline formula and the paragraph's line spacing.
    let metrics = paragraph_metrics(doc, para);
    let base = metrics.height;
    let line_advance = metrics.advance;
    let line_spacing = line_advance.saturating_sub(base).max(0);
    let baseline_gap = metrics.baseline_gap;
    let seg_width = body_width.max(1);
    // Reduce line width by the renderer's indent (layout::line_indents): positive indent
    // affects the first line, hanging indent subsequent lines. Without subtracting it,
    // synthesized lines exceed the paragraph's right edge. List-marker width requires shaping
    // and is not included here; a marker wider than the hanging indent can lengthen the first line.
    let ps = doc.header.para_shapes.get(para.para_shape.0 as usize);
    let first_indent_hu = ps.map_or(0, |p| p.indent.max(0) / 2);
    let rest_indent_hu = ps.map_or(0, |p| (-p.indent).max(0) / 2);
    let line_limit_pt = |committed: usize| {
        let indent = if committed == 0 {
            first_indent_hu
        } else {
            rest_indent_hu
        };
        ((seg_width - indent).max(1)) as f32 / 100.0
    };
    let total = para.wchar_len();

    let make = |start: u32, v: i32| LineSeg {
        text_start: start,
        v_pos: v,
        line_height: base,
        text_height: base,
        baseline_gap,
        line_spacing,
        col_start: 0,
        seg_width,
        flags: 0x0006_0000,
    };

    // Place one line and advance the vertical cursor. Body flow restarts at the
    // next page when the line exceeds the content height; cell-local layout passes
    // an unbounded height and therefore never resets here.
    let place = |segs: &mut Vec<LineSeg>, v_pos: &mut i32, start: u32| {
        if *v_pos > 0 && (*v_pos).saturating_add(base) > content_h {
            *v_pos = 0;
        }
        segs.push(make(start, *v_pos));
        *v_pos = (*v_pos).saturating_add(line_advance);
    };

    // Shape fonts to measure character widths and wrap greedily within body width.
    // Use the same glyph x_advance accumulation as place_wrapped in layout.rs.
    let items = shape_range(store, doc, para, (0, total), warnings);
    // Match place_wrapped: prefer explicit tab stops, then use the default interval.
    let tabs = crate::tab::tab_stops(doc, para);
    let mut segs = Vec::new();
    let mut line_start = 0u32;
    let mut acc = 0.0f32;
    let mut content = false;
    let mut previous_no_break = false;
    for item in &items {
        match item {
            InlineItem::Run(run) => {
                let sources = glyph_source_sequences(run);
                let mut wchar_offset = 0u32;
                for (g, source) in run.glyphs.iter().zip(&sources) {
                    let current_no_break = source_has_no_break_space(source);
                    let break_allowed = !previous_no_break && !current_no_break;
                    if content && acc + g.x_advance > line_limit_pt(segs.len()) && break_allowed {
                        place(&mut segs, v_pos, line_start);
                        line_start = run.start_wchar.saturating_add(wchar_offset);
                        acc = 0.0;
                    }
                    acc += g.x_advance;
                    content = true;
                    previous_no_break = current_no_break;
                    wchar_offset = wchar_offset.saturating_add(source_wchar_len(source));
                }
            }
            InlineItem::Tab => {
                acc = crate::tab::next_tab(&tabs, acc, TAB_INTERVAL_PT);
                content = true;
                previous_no_break = false;
            }
            // LINE_BREAK (CharCtrl 10) commits the current line and resumes after the control.
            InlineItem::LineBreak(next_start) => {
                place(&mut segs, v_pos, line_start);
                line_start = *next_start;
                acc = 0.0;
                content = false;
                previous_no_break = false;
            }
        }
    }
    // Commit the final line, which is also the only line for an empty paragraph.
    place(&mut segs, v_pos, line_start);

    // Encode the paragraph's left margin in col_start (HWPUNIT). Genuine line_seg horzpos
    // stores only this margin, not first-line/hanging indent; measured hanging-indent paragraphs
    // also have horzpos = margin_left. The renderer applies indent within the line box via
    // layout::line_indents. IR margins use twice HWPUNIT, so divide by two. With zero left margin,
    // col_start stays zero and retains the previous bytes.
    let ps = doc.header.para_shapes.get(para.para_shape.0 as usize);
    let ml_hu = ps.map_or(0, |p| p.margin_left / 2).max(0);
    for seg in segs.iter_mut() {
        seg.col_start = ml_hu;
    }
    segs
}

#[cfg(test)]
mod advance_tests {
    use super::{line_advance_hu, source_wchar_len};

    #[test]
    fn line_spacing_modes_preserve_zero_and_saturate_damaged_values() {
        assert_eq!(line_advance_hu(1000, 0, 160), 1600);
        assert_eq!(line_advance_hu(1000, 1, 0), 0);
        assert_eq!(line_advance_hu(1000, 1, 600), 300);
        assert_eq!(line_advance_hu(1000, 2, 0), 1000);
        assert_eq!(line_advance_hu(1000, 2, 600), 1300);
        assert_eq!(line_advance_hu(1000, 3, 600), 1000);
        assert_eq!(line_advance_hu(i32::MAX, 2, i32::MAX), i32::MAX);
    }

    #[test]
    fn source_offsets_count_utf16_code_units() {
        assert_eq!(source_wchar_len("a"), 1);
        assert_eq!(source_wchar_len("\u{1f600}"), 2);
        assert_eq!(source_wchar_len("x\u{301}"), 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwp_model::{Cell, HwpUnit, ids::BorderFillId};

    /// Because fill_nested accumulates v_pos within each cell, a cell's line block with N paragraphs
    /// is the last paragraph's line block alone.
    fn seg(v_pos: i32, line_height: i32) -> LineSeg {
        LineSeg {
            text_start: 0,
            v_pos,
            line_height,
            text_height: line_height,
            baseline_gap: 0,
            line_spacing: 0,
            col_start: 0,
            seg_width: 1000,
            flags: 0,
        }
    }

    fn cell_with(blocks: &[i32]) -> Cell {
        // blocks[i] is paragraph i's final line v_pos; line_height is fixed at 1000.
        Cell {
            list_attr: 0,
            col: 0,
            row: 0,
            col_span: 1,
            row_span: 1,
            width: HwpUnit(5000),
            height: HwpUnit(1000),
            margins: [141, 141, 141, 141],
            border_fill: BorderFillId::default(),
            header_tail: Vec::new(),
            paragraphs: blocks
                .iter()
                .map(|&v| Paragraph {
                    line_segs: vec![seg(v, 1000)],
                    ..Default::default()
                })
                .collect(),
        }
    }

    fn table_with(cell: Cell) -> Table {
        Table {
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
        }
    }

    #[test]
    fn table_height_multi_paragraph_cell_is_not_double_counted() {
        // Four paragraphs at v_pos 0/1600/3200/4800 give cell line block 4800 + 1000.
        let h = table_height(&table_with(cell_with(&[0, 1600, 3200, 4800])));
        assert_eq!(h, 141 + (4800 + 1000) + 141 + TABLE_BLOCK_PADDING);
    }

    #[test]
    fn table_height_single_paragraph_cell_unchanged() {
        let h = table_height(&table_with(cell_with(&[0])));
        assert_eq!(h, 141 + 1000 + 141 + TABLE_BLOCK_PADDING);
    }

    #[test]
    fn table_height_empty_cell_falls_back() {
        // With no paragraphs, the cell line block is zero; only margins remain and fallback does not panic.
        let h = table_height(&table_with(cell_with(&[])));
        assert_eq!(h, 141 + 141 + TABLE_BLOCK_PADDING);
    }
}
