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

/// Section-start spacing has no measured rule for a paragraph hosting a floating object.
/// Unknown placement retains that unresolved context rather than treating it as plain text.
pub(crate) fn hosts_floating_body_object(para: &Paragraph) -> bool {
    para.controls.iter().any(|control| match control {
        Control::Table(table) => table.placement.as_ref().is_none_or(|p| !p.treat_as_char),
        Control::Picture(picture) => !picture.treat_as_char,
        Control::Generic(generic) if generic.ctrl_id == *b"gso " => {
            crate::gso::parse_gso_box(&generic.data).is_none_or(|b| !b.treat_as_char())
        }
        _ => false,
    })
}

/// Fixed vertical table-block padding in HWPUNIT, added by genuine Hangul to the whole table.
///
/// Measurements of the genuine first-paragraph sample (5.1.1.0) and work_report.hwp (5.0.2.4)
/// both yield table advance minus summed row heights = 566 (2.0mm is 566.93 HWPUNIT).
/// It stays constant regardless of table top/bottom margins (141 each) or cell side margins,
/// so treat it as table-specific outer padding added once, separately from row heights.
const TABLE_BLOCK_PADDING: i32 = 566;

/// One page's body box in HWPUNIT: the paper less its top and bottom margins and its header and
/// footer bands. Both paginating paths read the sums from here, so the cache and the renderer
/// cannot drift on where the body ends (#423).
pub(crate) struct BodyBox {
    /// Paper top edge to body top edge: the top margin plus the header band.
    pub(crate) top_hu: i32,
    /// Bottom margin plus footer band: subtract this from the paper height to reach the body
    /// bottom.
    pub(crate) bottom_hu: i32,
    /// Body height, at least one HWPUNIT.
    pub(crate) height_hu: i32,
}

/// `paper_height_hu` is the paper extent the caller paginates on: the renderer passes the
/// landscape-resolved height, the body cache its page height.
pub(crate) fn body_box(page: &hwp_model::PageDef, paper_height_hu: i32) -> BodyBox {
    let top_hu = page.margin_top.0 + page.margin_header.0;
    let bottom_hu = page.margin_bottom.0 + page.margin_footer.0;
    BodyBox {
        top_hu,
        bottom_hu,
        height_hu: paper_height_hu
            .saturating_sub(top_hu)
            .saturating_sub(bottom_hu)
            .max(1),
    }
}

/// Synthesize line layout for the whole authored document, including body and table-cell paragraphs.
/// Native HWP5 sources keep the base cursor path, so native edits do not mix
/// cacheless fragment anchors/page-start flags with restored genuine caches. The base path
/// still restarts its cursor at an explicit page break, as genuine caches do.
/// To match Hangul's line breaks, store must have HCR Batang loaded.
///
/// A floating-table section reads its anchors from the prepass. If that pass did not finish
/// (a page or display-item budget stop, or a paragraph it never visited), the section is laid
/// out by the base cursor path instead and reports `anchor_prepass_incomplete`: default zero
/// anchors are never published as measured positions (#426).
pub fn synthesize_linesegs(
    doc: &mut Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
) {
    synthesize_linesegs_inner(doc, store, warnings, None, &BodyFlow::default());
}

/// One section's body flow continues at a caller-supplied cursor instead of restarting at the
/// section top: an edited HWP5 document keeps the saved caches of the paragraphs before the edit,
/// so the re-synthesized suffix has to start where that prefix ends (#462).
#[derive(Clone, Copy, Debug)]
pub struct BodySeed {
    pub section: usize,
    /// Body paragraph index the cursor applies to.
    pub paragraph: usize,
    /// Page-relative cursor the paragraph's flow starts from, in HWPUNIT: the kept prefix's bottom.
    /// The paragraph's own space-before is added by the walk, as for any other paragraph.
    pub v_pos_hu: i32,
}

/// The cursor the walk below carries past one body paragraph, in HWPUNIT: the paragraph's entry
/// position plus the tables it anchors when it has any (text cannot follow a table at the line
/// cursor, which is what Hangul flags as corruption), otherwise `line_cursor`, the position
/// `compute_linesegs` left the caller at; plus the paragraph's space-after.
///
/// `line_cursor` is passed in rather than re-derived from the committed segs on purpose: the
/// serialized `line_spacing` is `max(advance - base, 0)`, so `v_pos + line_height + line_spacing`
/// reads back `max(base, advance)`. With FIXED spacing below the base line, or FIXED 0, that is not
/// the advance the synthesizer committed, and re-deriving it would change unseeded synthesis
/// (HWPX/markdown -> HWP) that this rule is not allowed to touch (#509).
fn body_cursor(entry_v_pos: i32, line_cursor: i32, table_total: i32, space_after: i32) -> i32 {
    let bottom = if table_total > 0 {
        entry_v_pos.saturating_add(table_total)
    } else {
        line_cursor
    };
    bottom.saturating_add(space_after)
}

/// [`body_cursor`] measured from a paragraph's saved record, for a caller continuing a flow from
/// kept caches: the saved first line stands for the entry position, and the tables are measured
/// from their saved nested caches, which is what the renderer draws for that kept paragraph.
///
/// A saved record carries no separate advance field, so the line cursor is reconstructed as
/// `v_pos + line_height + line_spacing` of the last line. That is the only reading a saved cache
/// offers, and it is exact for every mode but FIXED spacing below the base line, which a genuine
/// record cannot express without also losing the committed position (#509).
pub fn saved_body_cursor(paragraph: &Paragraph, space_after: i32) -> Option<i32> {
    let entry = paragraph.line_segs.first()?;
    let table_total = paragraph
        .controls
        .iter()
        .filter_map(|control| match control {
            Control::Table(table) => Some(table_height(table)),
            _ => None,
        })
        .fold(0i32, |total, height| total.saturating_add(height));
    let last = paragraph.line_segs.last()?;
    let line_cursor = last
        .v_pos
        .saturating_add(last.line_height)
        .saturating_add(last.line_spacing);
    // A kept table anchor keeps the table inside its own line block: genuine records store the
    // extent in the anchor line's height, which can end below the summed cell heights (a 4412 HU
    // table under a 5100 HU line in the #462 fixture). Resuming at the smaller of the two would
    // start the next paragraph inside the line the output keeps (#509).
    let line_bottom = last.v_pos.saturating_add(last.line_height);
    Some(
        body_cursor(entry.v_pos, line_cursor, table_total, space_after)
            .max(line_bottom.saturating_add(space_after)),
    )
}

/// [`synthesize_linesegs`] with body-flow seeds, so an edited document can re-synthesize only the
/// paragraphs after an edit while keeping the genuine caches before it (#462). A seeded paragraph
/// starts at its seed; an explicit page break in that paragraph still restarts the page.
pub fn synthesize_linesegs_seeded(
    doc: &mut Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
    flow: &BodyFlow<'_>,
) {
    synthesize_linesegs_inner(doc, store, warnings, None, flow);
}

/// How an edited document's body flow is synthesized (#462, #509).
#[derive(Clone, Copy, Debug, Default)]
pub struct BodyFlow<'a> {
    /// One seed per section, each taken from [`saved_body_cursor`] over the paragraphs before the
    /// edit that the output keeps.
    pub seeds: &'a [BodySeed],
    /// The document already carries the caches the output will retain (their nested cells,
    /// captions and lists restored from the saved record), so a nested list that has one is
    /// measured as it is rather than re-synthesized. That is what keeps a kept table's height the
    /// one the paragraphs below it were placed against (#509).
    pub preserve_nested: bool,
}

/// [`synthesize_linesegs`] with an explicit prepass page budget, so a test can prove how an
/// interrupted prepass is reported and where layout falls back to. Production passes no limit.
/// Returns the prepass result the section policies were decided from.
#[cfg(test)]
fn synthesize_linesegs_with_prepass_page_limit(
    doc: &mut Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
    page_limit: usize,
) -> Option<crate::layout::BodyAnchors> {
    synthesize_linesegs_inner(doc, store, warnings, Some(page_limit), &BodyFlow::default())
}

fn synthesize_linesegs_inner(
    doc: &mut Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
    prepass_page_limit: Option<usize>,
    flow: &BodyFlow<'_>,
) -> Option<crate::layout::BodyAnchors> {
    let snap = doc.clone();
    let seeds = flow.seeds;
    let nested_caches = if flow.preserve_nested {
        ListCaches::KeepMeasured
    } else {
        ListCaches::Recompute
    };
    let use_fragment_flow = doc.meta.source_format != "hwp5";
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
    let flow_anchors = if use_fragment_flow && floating_sections.iter().any(|floating| *floating) {
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
                fill_nested(para, width, &snap, store, warnings, nested_caches);
            }
        }
        let mut flow = doc.clone();
        for section in &mut flow.sections {
            for para in &mut section.paragraphs {
                para.line_segs.clear();
            }
        }
        Some(match prepass_page_limit {
            Some(limit) => crate::layout::synthesized_body_anchors_with_page_limit(
                &flow, store, warnings, limit,
            ),
            None => crate::layout::synthesized_body_anchors(&flow, store, warnings),
        })
    } else {
        None
    };
    for (si, floating) in floating_sections.iter().copied().enumerate() {
        // An incomplete prepass must not publish its default entries as measured anchors:
        // that section keeps the base cursor path and reports the fallback once.
        let section_anchors = flow_anchors
            .as_ref()
            .filter(|prepass| floating && prepass.section_complete(si));
        if floating && flow_anchors.is_some() && section_anchors.is_none() {
            warnings.push(
                crate::issues::RenderIssueCode::AnchorPrepassIncomplete,
                format!("section={si}"),
            );
        }
        let page = snap.sections[si]
            .section_def()
            .and_then(|d| d.page.as_ref());
        let body_width = page.map_or(42520, |pg| {
            pg.width.0 - pg.margin_left.0 - pg.margin_right.0
        });
        // Body height excludes the top/bottom margins and, as of #423, the header/footer bands:
        // the same box `layout_document_pages` draws in. Genuine multipage caches are
        // page-relative: body vertpos resets on every page, and its maximum 59668 is below body
        // height. Without resetting, accumulated v_pos can greatly exceed page height (such as
        // 354408), which Hangul flags as corruption.
        let content_h = page
            .map_or(75686, |pg| body_box(pg, pg.height.0).height_hu)
            .max(1);
        let mut v_pos = 0i32;
        for pi in 0..doc.sections[si].paragraphs.len() {
            // #540 G1 / #570: Hangul 12.30.0 body caches use half the serialized
            // ParaShape spacing, keeping space-before at section and explicit-break starts.
            // Preserve the native edit/seed path's prior arithmetic; only authored-document
            // synthesis changes. Nested caches and per-line wrapping use their existing rules.
            let (sp_top, sp_bottom) = snap
                .header
                .para_shapes
                .get(snap.sections[si].paragraphs[pi].para_shape.0 as usize)
                .map_or((0, 0), |ps| {
                    if use_fragment_flow {
                        (ps.spacing_top / 2, ps.spacing_bottom / 2)
                    } else {
                        (ps.spacing_top, ps.spacing_bottom)
                    }
                });
            let explicit_break = snap.sections[si].paragraphs[pi].header.break_type & 0x04 != 0;
            // A seeded paragraph continues the flow a caller measured before it, so an edited
            // document's re-synthesized suffix begins where its kept prefix ends (#462) instead
            // of at a section-top cursor that contradicts the kept caches. An explicit page
            // break still wins: it restarts the page regardless of the seed. The seed comes from
            // a saved record, so the arithmetic saturates: an out-of-page cursor must degrade to
            // a page start, never to a panic or a release-mode wrap (#509).
            if let Some(seed) = seeds
                .iter()
                .find(|seed| seed.section == si && seed.paragraph == pi)
                && (0..=content_h).contains(&seed.v_pos_hu)
            {
                v_pos = seed.v_pos_hu;
            }
            if explicit_break {
                v_pos = 0;
            }
            if pi > 0
                || (use_fragment_flow
                    && !hosts_floating_body_object(&snap.sections[si].paragraphs[pi]))
            {
                v_pos = v_pos.saturating_add(sp_top);
            }
            // Floating-table measurement already populated nested caches.
            if section_anchors.is_none() {
                fill_nested(
                    &mut doc.sections[si].paragraphs[pi],
                    body_width,
                    &snap,
                    store,
                    warnings,
                    nested_caches,
                );
            }
            // Total table height in this paragraph.
            let mut table_total = 0i32;
            for ctrl in &doc.sections[si].paragraphs[pi].controls {
                if let Control::Table(t) = ctrl {
                    table_total = table_total.saturating_add(table_height(t));
                }
            }
            // Move the whole table to the next page when it does not fit in the current remainder.
            if section_anchors.is_none()
                && table_total > 0
                && v_pos > 0
                && v_pos.saturating_add(table_total) > content_h
            {
                v_pos = 0;
            }
            // A table anchor's cache starts at the incoming cursor after the preceding paragraph.
            // Genuine first-paragraph sample: body advance 1600, then table-anchor v_pos=1600.
            let anchor = section_anchors.and_then(|prepass| prepass.slot(si, pi));
            if let Some(anchor) = anchor {
                v_pos = anchor.v_pos;
            }
            let anchor_v = v_pos;
            let src = &snap.sections[si].paragraphs[pi];
            let mut segs = compute_linesegs(
                store, &snap, src, body_width, content_h, &mut v_pos, warnings,
            );
            // The exact position compute_linesegs advanced to. It is captured before anything
            // rewrites the cursor, because the committed segs cannot reproduce it: the serialized
            // line_spacing caps the reading at the base line, so FIXED spacing below it (or 0)
            // would otherwise move the following paragraph (#509).
            let line_cursor = v_pos;
            if anchor.is_some_and(|anchor| anchor.starts_page)
                && let Some(first) = segs.first_mut()
            {
                // HWP 5.0 §4.3.4 (PARA_LINE_SEG): bit0 marks the first line of a page.
                first.flags |= 0x1;
            }
            // A table paragraph advances by table height rather than one line_advance, keeping
            // following text from overlapping the table (which Hangul flags as corruption), and
            // space-after moves the next paragraph's first line down. `body_cursor` holds both
            // readings, so measuring a kept prefix from its saved record cannot drift from this.
            v_pos = body_cursor(anchor_v, line_cursor, table_total, sp_bottom);
            doc.sections[si].paragraphs[pi].line_segs = segs;
        }
    }
    flow_anchors
}

/// What a nested paragraph list does with the line caches it already carries (#509). The three
/// readings are separate switches: which paragraphs keep their own cache, what a descendant table
/// cell does, and whether a kept cache moves the list cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListCaches {
    /// Re-synthesize every paragraph. Table cells default to this because their line counts decide
    /// the table height, and a stale pre-edit cache would misalign the table.
    Recompute,
    /// Keep a paragraph's own cache and leave the list cursor where it was, which is the unedited
    /// convention for captions and generic paragraph lists. A kept cache does not move the cursor
    /// here, so unseeded synthesis publishes the same positions it always did (#509).
    KeepInPlace,
    /// Keep the cached prefix of the list, advancing the cursor by each kept paragraph's block, and
    /// re-synthesize from the first paragraph without a cache (#509). Only an edited document whose
    /// retained caches were restored first uses this: the prefix is what the output keeps, so it is
    /// what the table height must be measured from, and a cache after the seam is stale because an
    /// earlier paragraph in the same list moved.
    KeepMeasured,
}

impl ListCaches {
    /// What a caption or a generic control's paragraph list nested under this policy does: both are
    /// text containers the base synthesis keeps in place.
    fn text_list(self) -> Self {
        match self {
            Self::KeepMeasured => Self::KeepMeasured,
            Self::Recompute | Self::KeepInPlace => Self::KeepInPlace,
        }
    }

    /// What a table cell list nested under this policy does.
    fn cell(self) -> Self {
        match self {
            Self::KeepMeasured => Self::KeepMeasured,
            Self::Recompute | Self::KeepInPlace => Self::Recompute,
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
    caches: ListCaches,
) {
    for ctrl in &mut para.controls {
        match ctrl {
            Control::Table(t) => {
                if let Some(cap) = &mut t.caption {
                    let w = caption_width(cap.width, body_width);
                    fill_list(
                        &mut cap.paragraphs,
                        w,
                        caches.text_list(),
                        body_width,
                        snap,
                        store,
                        warnings,
                    );
                }
                for cell in &mut t.cells {
                    // Retain base native-cache arithmetic, including raw sentinels.
                    // Width/height sentinel interpretation requires genuine-byte
                    // evidence before changing written geometry (#425).
                    let w =
                        (cell.width.0 - i32::from(cell.margins[0]) - i32::from(cell.margins[1]))
                            .max(1);
                    fill_list(
                        &mut cell.paragraphs,
                        w,
                        caches.cell(),
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
                        caches.text_list(),
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
                        caches.text_list(),
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
                        caches.text_list(),
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
/// [`ListCaches::KeepInPlace`] keeps a paragraph's own cache without moving the cursor, and
/// [`ListCaches::KeepMeasured`] keeps the cached prefix while advancing by it and re-synthesizes
/// from the first paragraph without a cache.
fn fill_list(
    paras: &mut [Paragraph],
    width: i32,
    caches: ListCaches,
    body_width: i32,
    snap: &Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
) {
    let mut v = 0i32;
    // Set once KeepMeasured re-synthesized a paragraph: an earlier paragraph in this list moved, so
    // every later cache was measured against a position that no longer holds (#509).
    let mut past_seam = false;
    for para in paras.iter_mut() {
        fill_nested(para, body_width, snap, store, warnings, caches);
        let keep = match caches {
            ListCaches::Recompute => false,
            ListCaches::KeepInPlace => !para.line_segs.is_empty(),
            ListCaches::KeepMeasured => !past_seam && !para.line_segs.is_empty(),
        };
        if keep {
            if caches == ListCaches::KeepMeasured {
                // The retained prefix moves the cursor: the paragraph after it has to start below
                // it, and that is the height the table is measured from.
                if let Some(bottom) = saved_body_cursor(para, 0) {
                    v = bottom;
                }
            }
            continue;
        }
        past_seam = true;
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
        let cell_h = i32::from(cell.margins[2])
            .saturating_add(block)
            .saturating_add(i32::from(cell.margins[3]));
        if cell_h > row_heights[r] {
            row_heights[r] = cell_h;
        }
    }
    // Rows containing only row_span>1 cells have zero height; use one fallback line to prevent overlap.
    let fallback = 141 + 1000 + 141; // Top margin + one line (lh) + bottom margin, using genuine defaults.
    let sum = row_heights.iter().fold(0i32, |total, &h| {
        total.saturating_add(if h > 0 { h } else { fallback })
    });
    sum.saturating_add(TABLE_BLOCK_PADDING)
}

/// One cell paragraph's line-block height (HWPUNIT): last line.v_pos + last line.line_height.
/// Cell-local v_pos accumulates across paragraphs in fill_nested. The uniform-line
/// shortcut (line count - 1) * line_advance applies only when all line metrics match;
/// mixed sizes use the actual final v_pos plus line height.
fn para_line_block(para: &Paragraph) -> i32 {
    match para.line_segs.last() {
        // Saved caches are untrusted input: saturate so a corrupt position cannot wrap or panic.
        Some(seg) => seg.v_pos.saturating_add(seg.line_height),
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
    // next page when the line's own block (`line_height`, as in genuine saved
    // caches) no longer fits the body height; cell-local layout passes an
    // unbounded height and therefore never resets here. `layout`'s fallback
    // planner still reserves a 1.2 + 0.4 x size block in its own pass, so the two
    // can break the last line of a page differently. No local genuine sample
    // breaks a paragraph at the body bottom, so neither rule is established:
    // the cache keeps the block its own saved fields define (#423).
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

    #[test]
    fn saved_body_cursor_follows_the_walks_two_readings() {
        // No table: the saved line block (0 + 1000) is the bottom, plus space-after 300.
        let mut plain = Paragraph {
            line_segs: vec![seg(0, 1000)],
            ..Default::default()
        };
        assert_eq!(saved_body_cursor(&plain, 300), Some(1300));

        // A table paragraph advances past the table, not past its text line (#462): the saved
        // entry position 0 plus the table's height, plus the same space-after.
        let table = table_with(cell_with(&[0]));
        let height = table_height(&table);
        plain.controls.push(Control::Table(table));
        assert_eq!(saved_body_cursor(&plain, 300), Some(height + 300));
        assert!(height > 1300, "the table reaches below the text line");
    }
}

/// Regression coverage for #426: an anchor prepass that stops before it collected every
/// paragraph must not publish its unvisited slots as measured positions.
///
/// Every fixture paginates from geometry that exists without any font: a body paragraph
/// advances 1600 HWPUNIT plus 300 of space-after, an empty cell paragraph is one 1000-HWPUNIT
/// line, and a line's baseline sits 1200 below its top with a 400 descent. The store is
/// isolated, so no host face is consulted and no assertion counts pages or reads glyph metrics.
#[cfg(test)]
mod anchor_prepass_tests {
    use super::*;
    use crate::issues::{RenderIssueAccumulator, RenderIssueCode};
    use hwp_model::{Cell, HwpUnit, ids::BorderFillId};

    /// Body height that holds two paragraph lines (0 and 1900) and moves the third to page 2.
    const TWO_LINE_BODY: i32 = 4000;
    /// Body height that holds one paragraph line, so a three-row table cannot share the page.
    const ONE_LINE_BODY: i32 = 2400;

    /// Zero-margin page of `height_hu` HWPUNIT: the body box is the whole page, so pagination
    /// follows flow geometry alone.
    fn set_body_height(doc: &mut Document, height_hu: i32) {
        for control in &mut doc.sections[0].paragraphs[0].controls {
            let Control::SectionDef(definition) = control else {
                continue;
            };
            let page = definition
                .page
                .as_mut()
                .expect("from_markdown defines a page");
            page.height = HwpUnit(height_hu);
            page.margin_top = HwpUnit(0);
            page.margin_bottom = HwpUnit(0);
            page.margin_header = HwpUnit(0);
            page.margin_footer = HwpUnit(0);
            page.margin_left = HwpUnit(0);
            page.margin_right = HwpUnit(0);
        }
    }

    /// A body-flow table (`treat_as_char == false`), the shape that makes a section read
    /// prepass anchors. Its cells hold one empty paragraph each.
    fn floating_table(rows: u16, row_height: i32) -> Table {
        let cells = (0..rows)
            .map(|row| Cell {
                list_attr: 0,
                col: 0,
                row,
                col_span: 1,
                row_span: 1,
                width: HwpUnit(20000),
                height: HwpUnit(row_height),
                margins: [0; 4],
                border_fill: BorderFillId(1),
                header_tail: Vec::new(),
                paragraphs: vec![Paragraph::default()],
            })
            .collect();
        Table {
            common_data: Vec::new(),
            placement: Some(hwp_model::GsoPlacement {
                treat_as_char: false,
                vert_rel_to: 2,
                horz_rel_to: 3,
                ..Default::default()
            }),
            attr: 1,
            rows,
            cols: 1,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: vec![1; usize::from(rows)],
            border_fill: BorderFillId(1),
            table_tail: Vec::new(),
            cells,
            caption: None,
            extras: Vec::new(),
        }
    }

    fn issue_count(report: &crate::issues::RenderIssueReport, code: RenderIssueCode) -> u64 {
        report
            .issues
            .iter()
            .filter(|issue| issue.code == code)
            .map(|issue| issue.count)
            .sum()
    }

    fn synthesize_complete(
        doc: &mut Document,
    ) -> (crate::layout::BodyAnchors, crate::issues::RenderIssueReport) {
        let mut store = FontStore::new_isolated();
        let mut warnings = RenderIssueAccumulator::new();
        let anchors =
            synthesize_linesegs_inner(doc, &mut store, &mut warnings, None, &BodyFlow::default())
                .expect("a floating-table section runs the prepass");
        (anchors, warnings.finish())
    }

    fn synthesize_with_prepass_limit(
        doc: &mut Document,
        page_limit: usize,
    ) -> (crate::layout::BodyAnchors, crate::issues::RenderIssueReport) {
        let mut store = FontStore::new_isolated();
        let mut warnings = RenderIssueAccumulator::new();
        let anchors =
            synthesize_linesegs_with_prepass_page_limit(doc, &mut store, &mut warnings, page_limit)
                .expect("a floating-table section runs the prepass");
        (anchors, warnings.finish())
    }

    /// Ordinary complete pass: measured anchors drive the section, including the page-top anchor
    /// whose `starts_page` still has to reach the synthesized first line.
    #[test]
    fn complete_prepass_publishes_every_measured_anchor_and_its_page_start() {
        let mut doc = hwp_convert::from_markdown("A\n\nB\n\nC\n");
        set_body_height(&mut doc, TWO_LINE_BODY);
        doc.sections[0].paragraphs[0]
            .controls
            .push(Control::Table(floating_table(1, 1000)));

        let (anchors, warnings) = synthesize_complete(&mut doc);
        assert!(anchors.completed(), "an unconstrained prepass completes");
        assert!(anchors.section_complete(0));
        assert_eq!(
            issue_count(&warnings, RenderIssueCode::AnchorPrepassIncomplete),
            0,
            "a complete pass reports no fallback"
        );

        let page_top = anchors.slot(0, 2).expect("third paragraph visited");
        assert!(
            page_top.starts_page,
            "fixture: the third paragraph's first line opens page 2"
        );
        for pi in 0..3 {
            let anchor = anchors
                .slot(0, pi)
                .unwrap_or_else(|| panic!("paragraph {pi} visited"));
            let first = &doc.sections[0].paragraphs[pi].line_segs[0];
            assert_eq!(
                first.v_pos, anchor.v_pos,
                "paragraph {pi} publishes its measured anchor, not a base-cursor position"
            );
            assert_eq!(
                first.flags & 0x1 != 0,
                anchor.starts_page,
                "paragraph {pi} keeps the measured page-start bit"
            );
        }
    }

    /// Stop inside a paragraph's own line pagination: the third paragraph's first line opens
    /// page 2, which is the document's first page transition, so the fourth paragraph is never
    /// visited and its slot stays empty.
    #[test]
    fn prepass_stop_during_paragraph_pagination_never_publishes_a_default_zero_anchor() {
        let mut doc = hwp_convert::from_markdown("A\n\nB\n\nC\n\nD\n");
        set_body_height(&mut doc, TWO_LINE_BODY);
        doc.sections[0].paragraphs[3]
            .controls
            .push(Control::Table(floating_table(1, 500)));

        let (anchors, warnings) = synthesize_with_prepass_limit(&mut doc, 0);
        let opening = anchors.slot(0, 2).expect("third paragraph visited");
        assert!(
            opening.starts_page,
            "fixture: the stop happens where the third paragraph's first line opens page 2"
        );
        assert!(
            anchors.slot(0, 3).is_none(),
            "the fourth paragraph was never visited"
        );
        assert!(
            !anchors.completed(),
            "a page-budget stop is not a complete pass"
        );
        assert!(!anchors.section_complete(0));
        assert_eq!(
            issue_count(&warnings, RenderIssueCode::AnchorPrepassIncomplete),
            1,
            "the section reports its fallback once, not once per paragraph"
        );
        assert_eq!(
            issue_count(&warnings, RenderIssueCode::LayoutBudgetExceeded),
            1,
            "the prepass's own budget diagnostic reaches the caller"
        );

        let first = &doc.sections[0].paragraphs[3].line_segs[0];
        assert_ne!(
            first.v_pos, 0,
            "an unvisited anchor must not be published as a default-zero position"
        );
    }

    /// Stop inside the floating table's own page transition, after every paragraph's anchor was
    /// already recorded: the slots cannot reveal this stop, so the completion result has to.
    #[test]
    fn prepass_stop_during_floating_table_continuation_still_reports_incomplete() {
        let mut doc = hwp_convert::from_markdown("A\n\nB\n");
        set_body_height(&mut doc, ONE_LINE_BODY);
        doc.sections[0].paragraphs[0]
            .controls
            .push(Control::Table(floating_table(3, 1000)));

        let (anchors, warnings) = synthesize_with_prepass_limit(&mut doc, 0);
        assert!(
            anchors.slot(0, 0).is_some() && anchors.slot(0, 1).is_some(),
            "every paragraph's anchor was recorded before the table needed its next page"
        );
        assert!(
            !anchors.completed(),
            "a stop after the last anchor still leaves the prepass incomplete"
        );
        assert!(!anchors.section_complete(0));
        assert_eq!(
            issue_count(&warnings, RenderIssueCode::AnchorPrepassIncomplete),
            1
        );
        assert_eq!(
            issue_count(&warnings, RenderIssueCode::LayoutBudgetExceeded),
            1
        );

        let measured = anchors.slot(0, 1).expect("second paragraph visited");
        assert!(
            measured.starts_page,
            "fixture: the second paragraph opens a page once the table continues"
        );
        let first = &doc.sections[0].paragraphs[1].line_segs[0];
        assert_eq!(
            first.flags & 0x1,
            0,
            "the base cursor fallback must not apply a page-start flag it did not measure"
        );
    }

    /// Stop inside the first section leaves a later floating-table section unvisited; it must be
    /// reported and laid out by the base cursor, not from empty slots.
    #[test]
    fn prepass_stop_leaves_a_later_section_unvisited_and_reported() {
        let mut doc = hwp_convert::from_markdown("A\n\nB\n\nC\n\nD\n");
        set_body_height(&mut doc, TWO_LINE_BODY);
        let mut later = hwp_convert::from_markdown("E\n\nF\n");
        set_body_height(&mut later, TWO_LINE_BODY);
        later.sections[0].paragraphs[1]
            .controls
            .push(Control::Table(floating_table(1, 500)));
        doc.sections.push(later.sections.remove(0));

        let (anchors, warnings) = synthesize_with_prepass_limit(&mut doc, 0);
        assert!(
            anchors.slot(0, 2).is_some(),
            "fixture: the stop happens inside section 0's paragraph pagination"
        );
        assert!(
            anchors.slot(1, 0).is_none() && anchors.slot(1, 1).is_none(),
            "the later section is never visited"
        );
        assert!(!anchors.completed());
        assert!(!anchors.section_complete(1));
        assert_eq!(
            issue_count(&warnings, RenderIssueCode::AnchorPrepassIncomplete),
            1,
            "only the floating-table section consumes anchors and reports"
        );

        assert!(
            !doc.sections[1].paragraphs[0].line_segs.is_empty(),
            "the fallback still synthesizes the unvisited section"
        );
        let first = &doc.sections[1].paragraphs[1].line_segs[0];
        assert_ne!(
            first.v_pos, 0,
            "an unvisited later section is not published as default-zero anchors"
        );
    }
}
