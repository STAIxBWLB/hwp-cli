//! LineSegLayouter restores saved line layout (PARA_LINE_SEG)
//! into a DisplayList.
//!
//! Coordinate interpretation established by measurement (U1):
//! - `v_pos`: relative to the page body top; resets to zero on each page.
//! - Baseline y = body_top + v_pos + baseline_gap.
//! - `col_start`/`seg_width`: relative to the body area's left edge.
//! - Page boundary: a decreasing v_pos starts a new page (v1 heuristic).
//!
//! Incomplete-file handling, measured in real-world HWPX files:
//! - Tool-generated files may save one lineseg per paragraph with v_pos
//!   assuming one line. Wrap greedily at the segment width and use a flow cursor.
//!   Baseline = max(saved-position baseline, previous content's bottom).
//!   Complete files retain their saved positions because those are larger;
//!   incomplete files only have overlapping content moved down.
//! - Paragraphs without linesegs use fallback layout at the body width.

use std::collections::HashSet;

use hwp_model::{
    BorderFill, Caption, CaptionSide, CharShapeId, Control, Document, HwpChar, HwpUnit, PageDef,
    Paragraph, Section, Table,
};

use crate::display::{DisplayList, Fill, FillRule, Gradient, Item, PageList, PathCmd, Stroke};
use crate::error::RenderError;
use crate::fonts::FontStore;
use crate::footnote::{self, Note};
use crate::issues::{RenderIssueAccumulator, RenderIssueCode};
use crate::segment_map::{SegmentMap, SegmentRecorder};
use crate::shape::{InlineItem, shape_range_page};
use crate::tab::TabStop;

/// Work and memory budgets applied before certification layout begins.
pub const CERTIFICATION_MAX_PAGES: usize = 4_096;
pub const CERTIFICATION_MAX_DISPLAY_ITEMS: usize = 1_000_000;

#[derive(Debug, Clone, Copy)]
pub struct LayoutBudget {
    pub max_pages: usize,
    pub max_display_items: usize,
    pub max_paragraphs: usize,
    pub max_cells: usize,
    pub max_objects: usize,
    pub max_glyphs: usize,
    pub max_nesting_depth: usize,
    pub max_line_segments: usize,
    pub max_raw_records: usize,
    pub max_raw_bytes: u64,
    pub max_shape_points: usize,
    pub max_gradient_stops: usize,
    pub max_unique_image_bytes: u64,
    pub max_referenced_image_bytes: u64,
    pub max_estimated_work: u64,
}

impl LayoutBudget {
    pub const fn certification() -> Self {
        Self {
            max_pages: CERTIFICATION_MAX_PAGES,
            max_display_items: CERTIFICATION_MAX_DISPLAY_ITEMS,
            max_paragraphs: 20_000,
            max_cells: 100_000,
            max_objects: 20_000,
            max_glyphs: 5_000_000,
            max_nesting_depth: 64,
            max_line_segments: 1_000_000,
            max_raw_records: 200_000,
            max_raw_bytes: 64 * 1024 * 1024,
            max_shape_points: 1_000_000,
            max_gradient_stops: 100_000,
            max_unique_image_bytes: 128 * 1024 * 1024,
            max_referenced_image_bytes: 256 * 1024 * 1024,
            max_estimated_work: 10_000_000,
        }
    }
}

#[derive(Default)]
struct LayoutUsage {
    pages: usize,
    display_items: usize,
    paragraphs: usize,
    cells: usize,
    objects: usize,
    glyphs: usize,
    line_segments: usize,
    raw_records: usize,
    raw_bytes: u64,
    shape_points: usize,
    gradient_stops: usize,
    unique_image_bytes: u64,
    referenced_image_bytes: u64,
    estimated_work: u64,
    unique_images: HashSet<(usize, usize)>,
}

impl LayoutUsage {
    fn charge_usize(
        value: &mut usize,
        amount: usize,
        limit: usize,
        label: &str,
    ) -> Result<(), RenderError> {
        *value = value
            .checked_add(amount)
            .ok_or_else(|| budget_error(label))?;
        if *value > limit {
            return Err(budget_error(label));
        }
        Ok(())
    }

    fn charge_u64(
        value: &mut u64,
        amount: u64,
        limit: u64,
        label: &str,
    ) -> Result<(), RenderError> {
        *value = value
            .checked_add(amount)
            .ok_or_else(|| budget_error(label))?;
        if *value > limit {
            return Err(budget_error(label));
        }
        Ok(())
    }

    fn visit_paragraph(
        &mut self,
        doc: &Document,
        paragraph: &Paragraph,
        budget: &LayoutBudget,
        depth: usize,
    ) -> Result<(), RenderError> {
        if depth > budget.max_nesting_depth {
            return Err(budget_error("nesting_depth"));
        }
        Self::charge_usize(&mut self.paragraphs, 1, budget.max_paragraphs, "paragraphs")?;
        let glyphs = usize::try_from(paragraph.wchar_len()).map_err(|_| budget_error("glyphs"))?;
        Self::charge_usize(&mut self.glyphs, glyphs, budget.max_glyphs, "glyphs")?;
        Self::charge_u64(
            &mut self.estimated_work,
            u64::try_from(glyphs).map_err(|_| budget_error("estimated_work"))?,
            budget.max_estimated_work,
            "estimated_work",
        )?;
        if paragraph.header.break_type & 0x04 != 0 {
            Self::charge_usize(&mut self.pages, 1, budget.max_pages, "pages")?;
        }
        Self::charge_usize(
            &mut self.line_segments,
            paragraph.line_segs.len(),
            budget.max_line_segments,
            "line_segments",
        )?;
        Self::charge_u64(
            &mut self.estimated_work,
            u64::try_from(paragraph.line_segs.len()).map_err(|_| budget_error("estimated_work"))?,
            budget.max_estimated_work,
            "estimated_work",
        )?;
        let mut previous_v_pos = None;
        for segment in &paragraph.line_segs {
            if previous_v_pos.is_some_and(|previous| segment.v_pos < previous) {
                Self::charge_usize(&mut self.pages, 1, budget.max_pages, "pages")?;
            }
            previous_v_pos = Some(segment.v_pos);
        }
        for control in &paragraph.controls {
            Self::charge_usize(&mut self.objects, 1, budget.max_objects, "objects")?;
            Self::charge_u64(
                &mut self.estimated_work,
                8,
                budget.max_estimated_work,
                "estimated_work",
            )?;
            match control {
                Control::Picture(picture) => {
                    if let Some(bytes) = doc.resolve_bin(&picture.bin_ref) {
                        let bytes_len = u64::try_from(bytes.len())
                            .map_err(|_| budget_error("referenced_image_bytes"))?;
                        Self::charge_u64(
                            &mut self.referenced_image_bytes,
                            bytes_len,
                            budget.max_referenced_image_bytes,
                            "referenced_image_bytes",
                        )?;
                        let identity = (bytes.as_ptr() as usize, bytes.len());
                        if self.unique_images.insert(identity) {
                            Self::charge_u64(
                                &mut self.unique_image_bytes,
                                bytes_len,
                                budget.max_unique_image_bytes,
                                "unique_image_bytes",
                            )?;
                        }
                    }
                }
                Control::Table(table) => {
                    Self::charge_usize(
                        &mut self.cells,
                        table.cells.len(),
                        budget.max_cells,
                        "cells",
                    )?;
                    for cell in &table.cells {
                        for nested in &cell.paragraphs {
                            self.visit_paragraph(doc, nested, budget, depth + 1)?;
                        }
                    }
                }
                Control::Generic(generic) => {
                    self.visit_raw_records(&generic.raw_children, budget)?;
                    // Every structured shape can allocate a path even when it has no explicit
                    // point list (rect/ellipse synthesize their geometry). Count the shapes,
                    // not just their points, before any path construction.
                    Self::charge_usize(
                        &mut self.objects,
                        generic.gso_shapes.len(),
                        budget.max_objects,
                        "objects",
                    )?;
                    Self::charge_u64(
                        &mut self.estimated_work,
                        u64::try_from(generic.gso_shapes.len())
                            .map_err(|_| budget_error("estimated_work"))?
                            .saturating_mul(3),
                        budget.max_estimated_work,
                        "estimated_work",
                    )?;
                    for shape in &generic.gso_shapes {
                        Self::charge_usize(
                            &mut self.shape_points,
                            shape.points.len(),
                            budget.max_shape_points,
                            "shape_points",
                        )?;
                        if let Some(gradient) = &shape.fill_gradient {
                            Self::charge_usize(
                                &mut self.gradient_stops,
                                gradient.stops.len(),
                                budget.max_gradient_stops,
                                "gradient_stops",
                            )?;
                        }
                    }
                    for list in &generic.paragraph_lists {
                        for nested in &list.paragraphs {
                            self.visit_paragraph(doc, nested, budget, depth + 1)?;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn visit_raw_records(
        &mut self,
        records: &[hwp_model::OpaqueRecord],
        budget: &LayoutBudget,
    ) -> Result<(), RenderError> {
        let mut stack: Vec<(&hwp_model::OpaqueRecord, usize)> =
            records.iter().map(|record| (record, 1)).collect();
        while let Some((record, depth)) = stack.pop() {
            if depth > budget.max_nesting_depth {
                return Err(budget_error("raw_nesting_depth"));
            }
            Self::charge_usize(
                &mut self.raw_records,
                1,
                budget.max_raw_records,
                "raw_records",
            )?;
            Self::charge_u64(
                &mut self.raw_bytes,
                u64::try_from(record.data.len()).map_err(|_| budget_error("raw_bytes"))?,
                budget.max_raw_bytes,
                "raw_bytes",
            )?;
            Self::charge_u64(
                &mut self.estimated_work,
                1 + u64::try_from(record.data.len() / 8)
                    .map_err(|_| budget_error("estimated_work"))?,
                budget.max_estimated_work,
                "estimated_work",
            )?;
            stack.extend(record.children.iter().map(|child| (child, depth + 1)));
        }
        Ok(())
    }

    fn finish_estimate(&mut self, budget: &LayoutBudget) -> Result<(), RenderError> {
        let item_estimate = self
            .paragraphs
            .checked_mul(4)
            .and_then(|value| value.checked_add(self.cells.saturating_mul(8)))
            .and_then(|value| value.checked_add(self.objects.saturating_mul(8)))
            .and_then(|value| value.checked_add(self.glyphs))
            .ok_or_else(|| budget_error("display_items"))?;
        Self::charge_usize(
            &mut self.display_items,
            item_estimate,
            budget.max_display_items,
            "display_items",
        )?;
        Self::charge_u64(
            &mut self.estimated_work,
            u64::try_from(item_estimate).map_err(|_| budget_error("estimated_work"))?,
            budget.max_estimated_work,
            "estimated_work",
        )
    }
}

fn budget_error(label: &str) -> RenderError {
    RenderError::LayoutBudgetExceeded {
        resource: label.to_string(),
    }
}

fn preflight_layout_budget(doc: &Document, budget: &LayoutBudget) -> Result<(), RenderError> {
    let mut usage = LayoutUsage {
        pages: doc.sections.len(),
        ..LayoutUsage::default()
    };
    if usage.pages > budget.max_pages {
        return Err(budget_error("pages"));
    }
    for section in &doc.sections {
        for paragraph in &section.paragraphs {
            usage.visit_paragraph(doc, paragraph, budget, 0)?;
        }
    }
    usage.finish_estimate(budget)
}

/// Default tab interval (40pt = 4000 HWPUNIT).
pub(crate) const TAB_INTERVAL_PT: f32 = 40.0;

/// Approximate inter-column horizontal gap when no linked text-box candidate exists (pt).
const COL_GAP_PT: f32 = 14.0;

/// Minimum measured overflow reported for a row with a stored height (pt).
/// Avoid warnings on every table from rounding and residual advance errors (~0.1pt per line).
const CELL_CONTENT_OVERFLOW_EPSILON_PT: f32 = 1.0;

/// Pattern scale for tab-leader dashes, in pt.
///
/// `dash_pattern` scales its on/off run lengths by the stroke width, which is right for
/// border lines (a thicker border wants proportionally longer dashes) but wrong for a tab
/// leader: leader pitch is a typographic convention independent of how thin the mark is.
/// Passing the 0.5pt stroke width produced a 1.5pt pitch - dense enough to read as a solid
/// rule.
///
/// Measured against a Hancom-exported PDF of a dotted-leader table of contents (rasterised at
/// 300dpi, long leader runs of 135-139 marks): mark 1.20pt, gap 1.92pt, pitch 3.12pt. The
/// mark-to-gap ratio there is ~1:1.6, which `dash_pattern`'s `[1u, 2u]` dot style already
/// matches; only the scale was wrong. `u = 1.04` reproduces that 3.12pt pitch.
///
/// Note this still differs from Hancom structurally: Hancom draws leaders as repeated glyph
/// characters (a `TJ` run of the leader char), not as a stroked dash pattern. This constant
/// matches the resulting rhythm, not the mechanism.
const LEADER_DASH_SCALE: f32 = 1.04;

/// Overflow ratio that identifies a single-lineseg paragraph as an incomplete cache.
///
/// Genuine lines fill their segment width; residual advance differences from Hangul can cause
/// slight overflow (private 13-page measurement: two paragraphs, 0.72% and 1.52% over 463.3pt).
/// Rewrapping these adds lines and shifts pages (13 pages became 15). Actual stub caches
/// contain a whole paragraph on one line and overflow by tens to hundreds of percent.
/// Use a ratio rather than font size so large fonts in narrow cells receive no excessive allowance.
const CACHE_OVERFLOW_TOLERANCE: f32 = 1.05;

/// Partition text-box paragraphs into column ranges. A decreasing line v_pos indicates
/// Hangul flowed into a new column. Paragraphs without line layout stay in the current column.
fn split_columns(paras: &[&Paragraph]) -> Vec<std::ops::Range<usize>> {
    let mut cols = Vec::new();
    let mut start = 0usize;
    let mut prev: Option<i32> = None;
    for (i, p) in paras.iter().enumerate() {
        if let Some(v) = p.line_segs.first().map(|s| s.v_pos) {
            if prev.is_some_and(|pv| v < pv) {
                cols.push(start..i);
                start = i;
            }
            prev = Some(v);
        }
    }
    cols.push(start..paras.len());
    cols
}

/// Continuation-column positions of linked text boxes (pt). Collect floating GSO boxes
/// with equal width, height and vertical offset farther right, in horizontal order.
fn continuation_columns(para: &Paragraph, base: &crate::gso::GsoBox) -> Vec<(f32, f32)> {
    const TOL: i32 = 200; // 2pt tolerance.
    let mut v: Vec<(f32, f32)> = para
        .controls
        .iter()
        .filter_map(|c| match c {
            Control::Generic(g) if g.ctrl_id == *b"gso " => {
                let b = crate::gso::parse_gso_box(&g.data)?;
                let same = (b.width - base.width).abs() < TOL
                    && (b.height - base.height).abs() < TOL
                    && (b.vert_offset - base.vert_offset).abs() < TOL;
                (same && !b.treat_as_char() && b.horz_offset > base.horz_offset)
                    .then_some((b.horz_offset as f32 / 100.0, b.vert_offset as f32 / 100.0))
            }
            _ => None,
        })
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.dedup();
    v
}

/// A4 fallback for malformed documents without PAGE_DEF.
fn default_page() -> PageDef {
    PageDef {
        width: HwpUnit(59528),
        height: HwpUnit(84186),
        margin_left: HwpUnit(8504),
        margin_right: HwpUnit(8504),
        margin_top: HwpUnit(5668),
        margin_bottom: HwpUnit(4252),
        margin_header: HwpUnit(4252),
        margin_footer: HwpUnit(4252),
        gutter: HwpUnit(0),
        attr: 0,
    }
}

/// Page border definition (14-byte PAGE_BORDER_FILL; layout established by gc23 measurements).
/// Attribute u32 (reference bit0, header bit1, footer bit2, fill area bits3-4), four u16 gaps
/// (left/right/top/bottom, HWPUNIT16), then border ID u16 (one-based BORDER_FILL reference).
struct PageBorderFill {
    /// Reference bit0: false = page/body, true = paper. Measured genuine default: paper.
    paper_relative: bool,
    /// Left/right/top/bottom gaps (HWPUNIT; genuine default 1417, approximately 5mm).
    gap: [u16; 4],
    /// One-based BORDER_FILL reference. By convention, 1 has no borders on any side.
    border_fill_id: u16,
}

/// Parse the original 14-byte PAGE_BORDER_FILL payload; return None if too short.
fn parse_page_border_fill(raw: &[u8]) -> Option<PageBorderFill> {
    if raw.len() < 14 {
        return None;
    }
    let attr = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    let g = |o: usize| u16::from_le_bytes([raw[o], raw[o + 1]]);
    Some(PageBorderFill {
        paper_relative: attr & 1 != 0,
        gap: [g(4), g(6), g(8), g(10)],
        border_fill_id: g(12),
    })
}

/// Find the section's BOTH PAGE_BORDER_FILL entry.
///
/// Consume `SectionDef.page_border_fills_raw` (task 1). Entries occur in BOTH/EVEN/ODD order,
/// so BOTH is the first. EVEN/ODD selection is out of scope without genuine samples.
fn section_page_border_fill(section: &Section) -> Option<PageBorderFill> {
    let sd = section.section_def()?;
    let raw = sd.page_border_fills_raw.first()?;
    parse_page_border_fill(raw)
}

/// Page-border rectangle in page coordinates (pt).
struct PageBorderRect {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    sides: [hwp_model::BorderLine; 4],
}

/// Resolves the page-border rectangle and its left, right, top, bottom styles.
///
/// Invalid IDs, including zero, and border fills with no visible sides produce
/// `None` so documents without page borders retain their previous output.
#[allow(clippy::too_many_arguments)]
fn build_page_border_rect(
    pbf: &PageBorderFill,
    border_fills: &[BorderFill],
    paper_w: f32,
    paper_h: f32,
    body_left: f32,
    body_top: f32,
    body_right: f32,
    body_bottom: f32,
) -> Option<PageBorderRect> {
    // IDs are one-based. Zero and out-of-range values suppress output.
    let bf = (pbf.border_fill_id as usize)
        .checked_sub(1)
        .and_then(|i| border_fills.get(i))?;
    if !bf.sides.iter().any(hwp_model::BorderLine::is_visible) {
        return None;
    }
    // Convert gaps from HWPUNIT to pt. Order: left, right, top, bottom.
    let (gl, gr, gt, gb) = (
        pbf.gap[0] as f32 / 100.0,
        pbf.gap[1] as f32 / 100.0,
        pbf.gap[2] as f32 / 100.0,
        pbf.gap[3] as f32 / 100.0,
    );
    let (x1, y1, x2, y2) = if pbf.paper_relative {
        // Paper-relative borders are inset from the paper edges.
        (gl, gt, paper_w - gr, paper_h - gb)
    } else {
        // Body-relative placement remains an approximation pending Hancom samples.
        (
            body_left - gl,
            body_top - gt,
            body_right + gr,
            body_bottom + gb,
        )
    };
    Some(PageBorderRect {
        x1,
        y1,
        x2,
        y2,
        sides: bf.sides,
    })
}

/// Prepends a resolved page border so it paints behind page content.
fn prepend_page_borders(
    page: &mut PageList,
    border: &PageBorderRect,
    warnings: &mut RenderIssueAccumulator,
) {
    let mut items = crate::border::border_rectangle_items(
        border.x1,
        border.y1,
        border.x2,
        border.y2,
        &border.sides,
        [true; 4],
    );
    if !warnings.charge_display_items(items.len()) {
        return;
    }
    items.append(&mut page.items);
    page.items = items;
}

/// Prepends vertical column dividers at the midpoint of each column gap.
#[allow(clippy::too_many_arguments)]
fn prepend_col_dividers(
    page: &mut PageList,
    divider: &hwp_model::BorderLine,
    body_left: f32,
    body_top: f32,
    body_bottom: f32,
    col_width: f32,
    col_gap: f32,
    col_count: usize,
    warnings: &mut RenderIssueAccumulator,
) {
    let mut items: Vec<Item> = Vec::new();
    for i in 0..col_count.saturating_sub(1) {
        // Right edge of column i plus half the gap gives the divider x position.
        let x = body_left + (col_width + col_gap) * (i + 1) as f32 - col_gap * 0.5;
        items.extend(crate::border::border_line_items(
            x,
            body_top,
            x,
            body_bottom,
            divider,
        ));
    }
    if !warnings.charge_display_items(items.len()) {
        return;
    }
    items.append(&mut page.items);
    page.items = items;
}

struct PageNumberState {
    logical: u32,
    placement: Option<crate::page_number::PageNumberPlacement>,
    char_shape: Option<CharShapeId>,
    hidden: bool,
}

impl PageNumberState {
    fn new(start: u16) -> Self {
        Self {
            logical: u32::from(start.max(1)),
            placement: None,
            char_shape: None,
            hidden: false,
        }
    }

    /// Page-level controls in a paragraph apply to the page containing that
    /// paragraph. `pgnp` remains active until another placement replaces it;
    /// `pghd` is reset after the current page is finalized.
    ///
    /// Hangul inserts these controls wherever the caret was, so they routinely sit
    /// in a nested paragraph list — a text box or a shape — rather than in the body
    /// flow; genuine corpus documents place `pgnp`/`nwno` inside a `subList`. Such a
    /// list is anchored to this paragraph and therefore lands on this page, so it is
    /// scanned here to a bounded depth.
    ///
    /// Table cells are deliberately **not** scanned. A table can split across pages,
    /// so a control in a later row belongs to a later page; applying it here would
    /// restart `nwno` or clear `pghd` on the wrong one. Honoring it correctly means
    /// scanning cell controls as their fragment is laid out, which no observed
    /// document needs yet.
    fn apply_controls(&mut self, para: &Paragraph, warnings: &mut RenderIssueAccumulator) {
        self.apply_controls_depth(para, 0, warnings);
    }

    fn apply_controls_depth(
        &mut self,
        para: &Paragraph,
        depth: u8,
        warnings: &mut RenderIssueAccumulator,
    ) {
        /// Nesting beyond this is a malformed document, not a page-control carrier.
        const MAX_NESTING: u8 = 8;
        for (control_index, control) in para.controls.iter().enumerate() {
            let Control::Generic(control) = control else {
                continue;
            };
            if depth < MAX_NESTING {
                for list in &control.paragraph_lists {
                    for nested in &list.paragraphs {
                        self.apply_controls_depth(nested, depth + 1, warnings);
                    }
                }
            }
            match &control.ctrl_id {
                b"pgnp" => match crate::page_number::parse_pgnp(&control.data) {
                    Some(placement) => {
                        self.placement = Some(placement);
                        self.char_shape = control_char_shape(para, control_index);
                    }
                    None => warnings.push_once(RenderIssueCode::PageControlPayloadOmitted, b"pgnp"),
                },
                b"pghd" => {
                    if control.data.len() < 4 {
                        warnings.push_once(RenderIssueCode::PageControlPayloadOmitted, b"pghd");
                    } else {
                        self.hidden |= crate::page_number::pghd_hides_page_number(&control.data);
                    }
                }
                b"nwno" => match crate::page_number::parse_nwno_page(&control.data) {
                    Some(number) => self.logical = number,
                    None if control.data.len() < 6 => {
                        warnings.push_once(RenderIssueCode::PageControlPayloadOmitted, b"nwno");
                    }
                    None => {} // Non-PAGE automatic-number restarts belong to their respective number renderers.
                },
                _ => {}
            }
        }
    }

    fn visible_number(&self) -> Option<u32> {
        (!self.hidden).then_some(self.logical)
    }

    fn finish(
        &mut self,
        doc: &Document,
        store: &mut FontStore,
        page: &mut PageList,
        furniture: &Furniture<'_>,
        warnings: &mut RenderIssueAccumulator,
    ) {
        furniture.render(
            doc,
            store,
            page,
            self.visible_number(),
            self.logical,
            warnings,
        );
        if self.visible_number().is_some() {
            render_positioned_page_number(doc, store, page, furniture, self, warnings);
        }
        self.logical = self.logical.saturating_add(1);
        self.hidden = false;
    }
}

/// Returns the character shape active at an extended control's WCHAR position.
/// HWPX groups controls inside a run, while HWP5 links the matching CTRL_HEADER
/// through `ctrl_index`; both representations therefore resolve through the
/// paragraph's character-shape run table.
fn control_char_shape(para: &Paragraph, control_index: usize) -> Option<CharShapeId> {
    let mut pos = 0u32;
    for ch in &para.chars {
        if matches!(
            ch,
            HwpChar::ExtCtrl {
                ctrl_index: Some(index),
                ..
            } if *index as usize == control_index
        ) {
            return para
                .char_shape_runs
                .iter()
                .rev()
                .find(|(start, _)| *start <= pos)
                .map(|(_, id)| *id)
                .or_else(|| para.char_shape_runs.first().map(|(_, id)| *id));
        }
        pos = pos.saturating_add(ch.wchar_width());
    }
    para.char_shape_runs.first().map(|(_, id)| *id)
}

fn page_control_is_rendered(control: &Control) -> bool {
    let Control::Generic(control) = control else {
        return false;
    };
    match &control.ctrl_id {
        b"pgnp" => crate::page_number::parse_pgnp(&control.data).is_some(),
        b"pghd" => control.data.len() >= 4,
        b"nwno" => crate::page_number::parse_nwno_page(&control.data).is_some(),
        b"atno" => crate::page_number::is_page_atno(&control.data),
        _ => false,
    }
}

fn page_number_alignment(position: u8, logical: u32) -> i8 {
    let outside = matches!(position, 7 | 8);
    let inside = matches!(position, 9 | 10);
    if matches!(position, 1 | 4)
        || outside && logical.is_multiple_of(2)
        || inside && !logical.is_multiple_of(2)
    {
        -1
    } else if matches!(position, 3 | 6)
        || outside && !logical.is_multiple_of(2)
        || inside && logical.is_multiple_of(2)
    {
        1
    } else {
        0
    }
}

fn render_positioned_page_number(
    doc: &Document,
    store: &mut FontStore,
    page: &mut PageList,
    furniture: &Furniture<'_>,
    number: &PageNumberState,
    warnings: &mut RenderIssueAccumulator,
) {
    let Some(placement) = number.placement else {
        return;
    };
    if placement.position == 0 {
        return;
    }
    if placement.position > 10 {
        warnings.push_once(
            RenderIssueCode::PageNumberPositionOmitted,
            [placement.position],
        );
        return;
    }
    let text = crate::page_number::format_placement(number.logical, placement, warnings);
    let run = number
        .char_shape
        .and_then(|id| crate::shape::shape_plain_with_char_shape(store, doc, &text, id))
        .or_else(|| crate::shape::shape_plain(store, doc, &text, 10.0, 0, false));
    let Some(run) = run else {
        warnings.push_once(RenderIssueCode::PageNumberShapingOmitted, b"positioned");
        return;
    };
    let top = matches!(placement.position, 1..=3 | 7 | 9);
    let x = if page_number_alignment(placement.position, number.logical) < 0 {
        furniture.body_left
    } else if page_number_alignment(placement.position, number.logical) > 0 {
        furniture.body_left + furniture.body_width - run.width_pt
    } else {
        furniture.body_left + (furniture.body_width - run.width_pt) * 0.5
    }
    .clamp(0.0, (page.width_pt - run.width_pt).max(0.0));
    // The number belongs to the header/footer band, not to the paper margin:
    // the header band starts at margin_top and the footer band ends at
    // page_height - margin_bottom, matching `Furniture::render`. Inside the band
    // it hugs the outer edge — measured against the oracle for the footer
    // (text bottom on the bottom-margin line, within 0.3pt); the header follows
    // the same rule mirrored, which no oracle covers yet. Ascent/descent are
    // approximated from the run size rather than the face metrics.
    let y = if top {
        let margin = furniture.page_def.margin_top.to_pt() as f32;
        (margin + run.size_pt * 0.8).min(page.height_pt)
    } else {
        let margin = furniture.page_def.margin_bottom.to_pt() as f32;
        (page.height_pt - margin - run.size_pt * 0.2).max(run.size_pt)
    };
    push_run(page, x, y, run, &doc.header.border_fills, warnings);
}

/// [`layout_document_bounded`] that also records where each source segment landed, for a
/// caller that must locate segments in the SAME layout it certifies (original-form
/// certification, #419). The budget is checked exactly as the bounded path does; the
/// segment map is itself bounded by `segment_map::MAX_SEGMENT_ROWS` and reports truncation.
pub fn layout_document_with_segments_bounded(
    doc: &Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
    budget: &LayoutBudget,
) -> Result<(DisplayList, SegmentMap), RenderError> {
    if let Err(error) = preflight_layout_budget(doc, budget) {
        warnings.push(RenderIssueCode::LayoutBudgetExceeded, error.to_string());
        return Err(error);
    }
    warnings.set_display_item_limit(budget.max_display_items);
    warnings.set_page_limit(budget.max_pages);
    let (list, map) = layout_document_with_segments(doc, store, warnings);
    if warnings.display_item_budget_exceeded()
        || warnings.page_budget_exceeded()
        || list.pages.len() > budget.max_pages
    {
        return Err(budget_error("pages"));
    }
    Ok((list, map))
}

pub fn layout_document_bounded(
    doc: &Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
    budget: &LayoutBudget,
) -> Result<DisplayList, RenderError> {
    if let Err(error) = preflight_layout_budget(doc, budget) {
        warnings.push(RenderIssueCode::LayoutBudgetExceeded, error.to_string());
        return Err(error);
    }
    warnings.set_display_item_limit(budget.max_display_items);
    warnings.set_page_limit(budget.max_pages);
    let list = layout_document(doc, store, warnings);
    if warnings.display_item_budget_exceeded() || warnings.page_budget_exceeded() {
        return Err(budget_error(if warnings.page_budget_exceeded() {
            "pages"
        } else {
            "display_items"
        }));
    }
    let item_count = list.pages.iter().try_fold(0usize, |count, page| {
        count
            .checked_add(page.items.len())
            .ok_or_else(|| budget_error("display_items"))
    })?;
    if list.pages.len() > budget.max_pages || item_count > budget.max_display_items {
        let error = budget_error(if list.pages.len() > budget.max_pages {
            "pages"
        } else {
            "display_items"
        });
        warnings.push(RenderIssueCode::LayoutBudgetExceeded, error.to_string());
        return Err(error);
    }
    Ok(list)
}

pub fn layout_document(
    doc: &Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
) -> DisplayList {
    layout_document_inner(doc, store, warnings, &mut SegmentRecorder::disabled(), None)
}

/// Lays the document out **and** records where each source segment landed, as the
/// per-(page, segment) rows of [`crate::segment_map`].
///
/// Separate from [`layout_document`] because recording is not free and an ordinary render has
/// no use for it: an untrusted document drives the segment count (T-05-04-01). The returned
/// boxes are read off the `DisplayList` before any backend transform, so PNG, SVG and PDF all
/// describe the same geometry (D-07).
pub fn layout_document_with_segments(
    doc: &Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
) -> (DisplayList, SegmentMap) {
    let mut rec = SegmentRecorder::new();
    let list = layout_document_inner(doc, store, warnings, &mut rec, None);
    (list, rec.finish())
}

/// Resolved first-line placement, relative to the page body.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BodyAnchor {
    pub v_pos: i32,
    /// Only the first line's own page move, never an earlier table continuation.
    pub starts_page: bool,
}

/// Body anchors collected by the prepass, with an explicit completion result.
///
/// A slot is `None` when the pass stopped before that paragraph was visited, so an
/// untouched entry can no longer be read as a legitimate page-top anchor (`v_pos == 0`,
/// `starts_page == false`). `completed` carries the other half of the answer: a budget stop
/// that refused a page *after* the last anchor was recorded leaves every slot filled, and a
/// floating-table continuation is exactly that case (#426).
#[derive(Clone, Debug, Default)]
pub(crate) struct BodyAnchors {
    slots: Vec<Vec<Option<BodyAnchor>>>,
    completed: bool,
}

impl BodyAnchors {
    pub(crate) fn completed(&self) -> bool {
        self.completed
    }

    pub(crate) fn slot(&self, section_index: usize, para_index: usize) -> Option<BodyAnchor> {
        self.slots
            .get(section_index)?
            .get(para_index)
            .copied()
            .flatten()
    }

    /// Every paragraph of this section was visited with no budget stop, so its anchors may
    /// drive the fragment flow.
    pub(crate) fn section_complete(&self, section_index: usize) -> bool {
        self.completed()
            && self
                .slots
                .get(section_index)
                .is_some_and(|slots| slots.iter().all(Option::is_some))
    }
}

/// Obtain body anchors from the same flow that plans floating-table fragments.
/// The caller supplies cell caches and clears body caches before this measurement.
///
/// The prepass gets its own accumulator, so it neither spends the caller's page/display-item
/// budget nor returns the document's warnings a second time - the real layout pass reports those
/// once. Only the diagnostics that can invalidate an anchor are propagated: a page budget that
/// refused a transition. The prepass sets no display-item limit itself, so that flag can only be
/// set by a caller that injects one later; it still counts as an incomplete pass because it is
/// the same refusal signal [`layout_document_bounded`] reads.
pub(crate) fn synthesized_body_anchors(
    doc: &Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
) -> BodyAnchors {
    synthesized_body_anchors_with_limit(doc, store, warnings, None)
}

/// [`synthesized_body_anchors`] with an explicit page budget, so a caller can prove how the
/// prepass behaves when it stops early. Production paths pass no limit.
pub(crate) fn synthesized_body_anchors_with_page_limit(
    doc: &Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
    page_limit: usize,
) -> BodyAnchors {
    synthesized_body_anchors_with_limit(doc, store, warnings, Some(page_limit))
}

fn synthesized_body_anchors_with_limit(
    doc: &Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
    page_limit: Option<usize>,
) -> BodyAnchors {
    let mut slots: Vec<Vec<Option<BodyAnchor>>> = doc
        .sections
        .iter()
        .map(|section| vec![None; section.paragraphs.len()])
        .collect();
    let mut prepass = RenderIssueAccumulator::new();
    if let Some(limit) = page_limit {
        prepass.set_page_limit(limit);
    }
    layout_document_inner(
        doc,
        store,
        &mut prepass,
        &mut SegmentRecorder::disabled(),
        Some(&mut slots),
    );
    // Every early return inside the layout is a refused page transition (a body overflow, an
    // explicit break, a paragraph's own line pagination, a table continuation or the section
    // close), so these two flags cover all of them. The unvisited check covers a stop that
    // returned before a later paragraph, and the flags cover a stop after the last one.
    let stopped = prepass.page_budget_exceeded() || prepass.display_item_budget_exceeded();
    if prepass.page_budget_exceeded() {
        warnings.push_once(RenderIssueCode::LayoutBudgetExceeded, b"pages");
    }
    if prepass.display_item_budget_exceeded() {
        warnings.push_once(RenderIssueCode::LayoutBudgetExceeded, b"display_items");
    }
    let visited = slots
        .iter()
        .all(|section| section.iter().all(Option::is_some));
    BodyAnchors {
        slots,
        completed: visited && !stopped,
    }
}

fn layout_document_inner(
    doc: &Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
    rec: &mut SegmentRecorder,
    anchors: Option<&mut [Vec<Option<BodyAnchor>>]>,
) -> DisplayList {
    // Memoized cell heights are keyed by addresses inside `doc`; never let one
    // document's entries answer for another's, and do not keep them past the
    // borrow (#321).
    store.measured_cells.clear();
    let list = layout_document_pages(doc, store, warnings, rec, anchors);
    store.measured_cells.clear();
    list
}

fn layout_document_pages(
    doc: &Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
    rec: &mut SegmentRecorder,
    mut anchors: Option<&mut [Vec<Option<BodyAnchor>>]>,
) -> DisplayList {
    let mut pages = Vec::new();
    let mut page_numbers = PageNumberState::new(doc.header.properties.start_numbers[0]);

    for (section_index, section) in doc.sections.iter().enumerate() {
        // Index of this section's first page; insert borders into all its pages when the section ends.
        let section_first_page = pages.len();
        let page_def = section
            .section_def()
            .and_then(|d| d.page)
            .unwrap_or_else(|| {
                warnings.push(RenderIssueCode::PageDefinitionFallback, b"a4");
                default_page()
            });
        // Landscape (PAGE_DEF attr bit0): rotate the paper 90 degrees by swapping width and height.
        // Previously, ignoring orientation rendered landscape files as portrait and clipped right columns.
        let landscape = page_def.attr & 1 != 0;
        let (paper_w_hu, paper_h_hu) = if landscape {
            (page_def.height.0, page_def.width.0)
        } else {
            (page_def.width.0, page_def.height.0)
        };
        let (w, h) = (paper_w_hu as f32 / 100.0, paper_h_hu as f32 / 100.0);
        let body_left = page_def.margin_left.to_pt() as f32;
        let body_top = (page_def.margin_top.0 + page_def.margin_header.0) as f32 / 100.0;
        let body_width =
            (paper_w_hu - page_def.margin_left.0 - page_def.margin_right.0) as f32 / 100.0;
        // Bottom of the body area, used as the overflow boundary.
        let body_bottom = h - (page_def.margin_bottom.0 + page_def.margin_footer.0) as f32 / 100.0;

        // Resolve PAGE_BORDER_FILL BOTH once, then prepend it to every section page.
        let page_border = section_page_border_fill(section).and_then(|pbf| {
            build_page_border_rect(
                &pbf,
                &doc.header.border_fills,
                w,
                h,
                body_left,
                body_top,
                body_left + body_width,
                body_bottom,
            )
        });

        let mut page = PageList {
            width_pt: w,
            height_pt: h,
            items: Vec::new(),
        };
        let mut prev_v_pos = -1i32;
        // Flow cursor: bottom y of content actually placed on this page, in page coordinates.
        let mut content_bottom = body_top;
        let mut skipped_controls = 0usize;
        let mut paras_on_page = 0usize;

        // Multi-column geometry comes from the section's first cold/ColumnDef control (one setup per section).
        // Hangul line_seg uses column-relative col_start=0 and seg_width=column width; derive x from the band.
        // Boundary detection prefers flags bit0/bit1. Without either flag,
        // each v_pos reset advances a band and every colCount-th band starts a page.
        let col_def = section
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Generic(g) => g.column_def.as_ref(),
                _ => None,
            });
        let col_count = col_def.map_or(1, |c| (c.count as usize).max(1));
        let col_gap = col_def.map_or(0.0, |c| c.gap as f32 / 100.0);
        let col_width = if col_count > 1 {
            (body_width - col_gap * (col_count - 1) as f32) / col_count as f32
        } else {
            body_width
        };
        let mut col_band = 0usize;
        // Flow state, not display items, proves that the current page/column has
        // been consumed. Empty lines and paragraphs must preserve explicit
        // blank pages and columns.
        let mut has_flow_in_current_band = false;

        // Collect every section header/footer with its odd/even/both target.
        // Page finalization selects the entry for the printed parity (GG-16).
        let mut header_ctrls = Vec::new();
        let mut footer_ctrls = Vec::new();
        for para in &section.paragraphs {
            for c in &para.controls {
                if let Control::Generic(g) = c {
                    match &g.ctrl_id {
                        b"head" => header_ctrls.push((g, head_foot_apply(g))),
                        b"foot" => footer_ctrls.push((g, head_foot_apply(g))),
                        _ => {}
                    }
                }
            }
        }
        let furniture = Furniture {
            header: header_ctrls,
            footer: footer_ctrls,
            page_def: &page_def,
            body_left,
            body_width,
        };

        // Number notes across the section. Footnotes stay on their anchor page;
        // endnotes accumulate for the section-closing block (GG-14).
        let notes = footnote::collect_notes(&section.paragraphs);
        let mut page_notes: Vec<&Note> = Vec::new();
        let mut pending_endnotes: Vec<&Note> = Vec::new();
        // List numbering/bullet counters advance in document order within each section.
        let mut list_state = crate::list::ListState::default();

        for (para_index, para) in section.paragraphs.iter().enumerate() {
            skipped_controls += para
                .controls
                .iter()
                .filter(|c| {
                    let rendered = matches!(
                        c,
                        Control::SectionDef(_) | Control::Table(_) | Control::Picture(_)
                    ) || page_control_is_rendered(c)
                        // `bokm` (bookmark) is an invisible marker by design (#254): omitting
                        // it from the page is correct, so it must not count as an unsupported
                        // omission. `%hlk` (hyperlink) is already drawn and styled by the
                        // hyperlink range pass (shape.rs::hyperlink_ranges), so its field
                        // control is not an omission either.
                        || [
                            *b"cold", *b"head", *b"foot", *b"fn  ", *b"en  ", *b"bokm", *b"%hlk",
                        ]
                        .contains(&c.ctrl_id())
                        // Render text boxes and shapes (line/rectangle/ellipse/arc/polygon).
                        || matches!(c, Control::Generic(g)
                            if g.ctrl_id == *b"gso "
                                && (!g.paragraph_lists.is_empty()
                                    || crate::shape_draw::has_shape(&g.raw_children)))
                        // Structured HWPX shapes (rect/ellipse/...).
                        || matches!(c, Control::Generic(g) if !g.gso_shapes.is_empty())
                        // HWPX containers (hp:container): render child shapes and text.
                        || matches!(c, Control::Generic(g) if g.container_box.is_some())
                        // Equations (hp:equation).
                        || matches!(c, Control::Generic(g) if g.equation.is_some());
                    !rendered
                })
                .count();

            // Body overflow: open a page when previous content passed the body bottom.
            // This is the default pagination for generated documents without linesegs.
            if content_bottom > body_bottom && paras_on_page > 0 {
                rec.content_end(&page);
                render_page_notes(
                    doc,
                    store,
                    &mut page,
                    &page_notes,
                    body_left,
                    body_width,
                    body_bottom,
                    warnings,
                );
                page_notes.clear();
                page_numbers.finish(doc, store, &mut page, &furniture, warnings);
                if !push_page_checked(&mut pages, &mut page, Some((w, h)), warnings, rec) {
                    // The returned pages carry content whose provenance was recorded; publishing
                    // them without resolving would silently drop it. Still-open spans are
                    // dropped and `SegmentMap::truncated` says the set is incomplete.
                    rec.resolve(&pages);
                    return DisplayList { pages };
                }
                content_bottom = body_top;
                prev_v_pos = -1;
                paras_on_page = 0;
                col_band = 0;
                has_flow_in_current_band = false;
            }

            // Explicit page break (PARA_HEADER break_type bit2 / hp:p pageBreak).
            // Break after prior paragraphs even when they contain only text boxes and no display items.
            if para.header.break_type & 0x04 != 0 && paras_on_page > 0 {
                rec.content_end(&page);
                render_page_notes(
                    doc,
                    store,
                    &mut page,
                    &page_notes,
                    body_left,
                    body_width,
                    body_bottom,
                    warnings,
                );
                page_notes.clear();
                page_numbers.finish(doc, store, &mut page, &furniture, warnings);
                if !push_page_checked(&mut pages, &mut page, Some((w, h)), warnings, rec) {
                    // The returned pages carry content whose provenance was recorded; publishing
                    // them without resolving would silently drop it. Still-open spans are
                    // dropped and `SegmentMap::truncated` says the set is incomplete.
                    rec.resolve(&pages);
                    return DisplayList { pages };
                }
                content_bottom = body_top;
                prev_v_pos = -1;
                paras_on_page = 0;
                col_band = 0;
                has_flow_in_current_band = false;
            }
            page_numbers.apply_controls(para, warnings);
            paras_on_page += 1;
            // Opened after the page-break blocks above: note blocks and page numbers are
            // emitted while finalizing the *previous* page and must stay outside this span.
            rec.begin_paragraph(section_index, para_index, para, &page);

            // Collect superscript footnote/endnote markers and notes belonging to this page.
            // Split footnotes into the page queue and endnotes into the section queue.
            let marks = footnote::para_marks(&notes, para);
            // Held with the WCHAR offset of each anchor rather than queued straight onto the
            // page: the fallback band can break a page in the middle of this paragraph (#282),
            // and a footnote belongs to the page its marker landed on. The cached band never
            // breaks mid-paragraph, so it queues the whole set at once.
            let mut para_foot_notes: Vec<(u32, &Note)> = Vec::new();
            for (at, note) in footnote::para_notes(&notes, para) {
                match note.kind {
                    footnote::NoteKind::Foot => para_foot_notes.push((at, note)),
                    footnote::NoteKind::End => pending_endnotes.push(note),
                }
            }
            if !para.line_segs.is_empty() {
                page_notes.extend(para_foot_notes.drain(..).map(|(_, note)| note));
            }
            let tabs = crate::tab::tab_stops(doc, para);
            let mut geom = para_geometry(doc, para);
            if anchors.is_some() && para_index == 0 {
                // B5: the section's first paragraph starts at zero. Apply this
                // to the anchor prepass flow; later explicit breaks and page
                // overflow retain their existing space-before placement.
                geom.spacing_top = 0.0;
            }
            let links = crate::shape::hyperlink_ranges(para);
            // Do not consume a list counter for a paragraph that has no visible line at all.
            let marker = if para.line_segs.is_empty() && para.chars.is_empty() {
                None
            } else {
                list_state.marker_for_render(doc, para)
            };

            // Top of this paragraph's first line, used as its table anchor.
            let mut para_top: Option<f32> = None;

            // Paragraph background/border pass (border_fill). Split the background into per-page
            // fragments when a paragraph crosses a page or column boundary (GC-9). Fragment state:
            // - bg_slice_top: current fragment's top y, set when its page's first line is placed.
            // - bg_slice_insert: insertion point in the current page's items, behind text.
            // - bg_slice_col_x: current fragment's horizontal column offset.
            // - bg_first_slice: whether this is the first fragment, which receives the top border.
            let mut bg_slice_top: Option<f32> = None;
            let mut bg_slice_insert = page.items.len();
            let mut bg_slice_col_x = 0.0f32;
            let mut bg_first_slice = true;

            if let Some(anchors) = anchors.as_deref_mut() {
                anchors[section_index][para_index] = Some(BodyAnchor {
                    v_pos: ((content_bottom + geom.spacing_top - body_top) * 100.0).round() as i32,
                    starts_page: false,
                });
            }
            if para.line_segs.is_empty() {
                // Cacheless body advance follows declared line spacing. Its fallback
                // baseline and paragraph-spacing placement remain distinct from cached layout.
                // Set when the greedy wrap itself crossed a page boundary (#282).
                let mut para_split = false;
                if para.chars.is_empty() {
                    content_bottom += 16.0; // Approximate empty-paragraph height.
                } else {
                    let end = para.wchar_len();
                    let mut items = shape_range_page(
                        store,
                        doc,
                        para,
                        (0, end),
                        &marks,
                        page_numbers.visible_number(),
                        warnings,
                    );
                    crate::shape::apply_link_style(&mut items, &links);
                    let max_size = items_max_size(&items).unwrap_or(10.0);
                    // Paragraph indent, margins and space-before (fallback only; caches encode these in col_start).
                    let left = body_left + geom.left;
                    let avail = (body_width - geom.left - geom.right).max(4.0);
                    let baseline_y = content_bottom + geom.spacing_top + max_size * 1.2;
                    para_top = Some(content_bottom + geom.spacing_top);
                    // A fallback paragraph carries no cached line geometry, so no line here can
                    // declare a page boundary the way the cached band's `LineSeg` flags do.
                    // Wrap into a scratch page first, then move whole lines onto real pages,
                    // breaking where the next line's descent would cross the body bottom
                    // (#282). Without this a paragraph taller than the page is emitted in full
                    // below the page bottom and only the *next* paragraph paginates.
                    let mut scratch = PageList {
                        width_pt: w,
                        height_pt: h,
                        items: Vec::new(),
                    };
                    // Place the marker left of the line box and shift the first line's text by its width.
                    // It goes into the scratch page too, so that if the first line moves to the
                    // next page the marker moves with it instead of being left behind.
                    let mut marker_advance = 0.0;
                    if let Some(m) = &marker {
                        marker_advance = render_list_marker(
                            &mut scratch,
                            store,
                            doc,
                            m,
                            (left, baseline_y, max_size),
                            warnings,
                        );
                    }
                    // First-line indent/hanging indent: cap excessively large positive values.
                    // Do not subtract from wrap width, avoiding runaway wrapping in narrow cells.
                    let (first_indent, rest_indent) = line_indents(&geom, marker_advance);
                    let first_indent = first_indent.min(avail * 0.8);
                    let rest_indent = rest_indent.min(avail * 0.8);
                    // Correct centered/right-aligned single lines in fallback layout too.
                    let natural = items_width(&items, &tabs);
                    let align = doc
                        .header
                        .para_shapes
                        .get(para.para_shape.0 as usize)
                        .map_or(1, |p| p.alignment());
                    // Centered/right-aligned single lines ignore indent; otherwise x0 = left margin + continuation offset.
                    let (x0, first_delta) = if natural <= avail && (align == 2 || align == 3) {
                        (
                            left + (avail - natural) * if align == 3 { 0.5 } else { 1.0 },
                            0.0,
                        )
                    } else {
                        (left + rest_indent, first_indent - rest_indent)
                    };
                    // place_wrapped's right edge is x0 + max_width. Reduce width by the indent
                    // to keep the paragraph's original right edge (left + avail).
                    let wrap_width = (left + avail - x0).max(1.0);
                    let line_advance = crate::lineseg::paragraph_advance(
                        doc,
                        para,
                        (max_size * 100.0).round() as i32,
                    ) as f32
                        / 100.0;
                    let descent = max_size * 0.4;
                    let mut lines: Vec<(usize, f32)> = Vec::new();
                    let last_y = place_wrapped(
                        &mut scratch,
                        items,
                        x0,
                        baseline_y,
                        wrap_width,
                        line_advance,
                        &tabs,
                        first_delta,
                        &doc.header.border_fills,
                        Some(&mut lines),
                        warnings,
                    );
                    // The marker was pushed before the wrap, so the first line's recorded start
                    // is past it. Pull it back to 0 so the marker travels with that line.
                    if let Some(first) = lines.first_mut() {
                        first.0 = 0;
                    }
                    let total_items = scratch.items.len();
                    // `place_wrapped` keeps advancing the baseline after `charge_display_items`
                    // stops accepting items, so a truncated wrap ends in a run of lines that
                    // carry nothing. Planning a page for each of those emits blank pages.
                    while lines.len() > 1
                        && lines.last().is_some_and(|&(start, _)| start >= total_items)
                    {
                        lines.pop();
                    }
                    // A line taller than the body box itself cannot be rescued by any break.
                    // It is still drawn, so this is a geometry deviation rather than lost
                    // content - the same contract `TableCellContentOverflow` carries.
                    if line_advance > body_bottom - body_top {
                        warnings.push_once(
                            RenderIssueCode::ParagraphLineContentOverflow,
                            format!("{:.0}", line_advance - (body_bottom - body_top)),
                        );
                    }
                    // Per line: where its items start in `scratch`, how far down it moves, and
                    // - when it opens a new page - the paragraph WCHAR offset the new page
                    // starts at, which decides how this paragraph's footnotes split. A line
                    // only breaks when the page it would leave behind carries something:
                    // otherwise the break pushes an empty page. That holds for the
                    // paragraph's own first line too, so a fallback paragraph starting within
                    // a descent of the body bottom moves down rather than drawing into the
                    // margin.
                    let mut plan: Vec<(usize, f32, Option<u32>)> = Vec::with_capacity(lines.len());
                    let mut dy = 0.0f32;
                    let mut lines_on_page = 0usize;
                    let mut page_has_content = has_flow_in_current_band;
                    for (i, &(start_item, baseline)) in lines.iter().enumerate() {
                        let mut opens_page = None;
                        if (lines_on_page > 0 || page_has_content)
                            && baseline + dy + descent > body_bottom
                        {
                            dy = body_top + max_size * 1.2 - baseline;
                            // The marker rides with line 0, so nothing of the paragraph
                            // precedes a break there.
                            opens_page = Some(if i == 0 {
                                0
                            } else {
                                first_wchar_from(&scratch.items, start_item)
                            });
                            lines_on_page = 0;
                            page_has_content = false;
                        }
                        lines_on_page += 1;
                        plan.push((start_item, dy, opens_page));
                    }
                    // Pagination may move the first line away from the entry cursor.
                    // Record its resolved top and only the break this line opened;
                    // a preceding table continuation already advanced the page.
                    if let (
                        Some(anchors),
                        Some(&(_, first_dy, first_opens_page)),
                        Some(&(_, first_baseline)),
                    ) = (anchors.as_deref_mut(), plan.first(), lines.first())
                    {
                        anchors[section_index][para_index] = Some(BodyAnchor {
                            v_pos: ((first_baseline + first_dy - max_size * 1.2 - body_top) * 100.0)
                                .round() as i32,
                            starts_page: first_opens_page.is_some(),
                        });
                    }
                    para_split = plan.iter().any(|&(_, _, opens)| opens.is_some());
                    let mut src = scratch.items.into_iter();
                    let mut page_budget_spent = false;
                    for (i, &(start_item, dy, opens_page)) in plan.iter().enumerate() {
                        let end = plan.get(i + 1).map_or(total_items, |next| next.0);
                        if let Some(cut) = opens_page {
                            // Only the notes whose marker stayed on this page. The rest wait
                            // for the page their marker moved to.
                            let split = para_foot_notes
                                .iter()
                                .position(|&(at, _)| at >= cut)
                                .unwrap_or(para_foot_notes.len());
                            page_notes.extend(para_foot_notes.drain(..split).map(|(_, note)| note));
                            rec.content_end(&page);
                            render_page_notes(
                                doc,
                                store,
                                &mut page,
                                &page_notes,
                                body_left,
                                body_width,
                                body_bottom,
                                warnings,
                            );
                            page_notes.clear();
                            page_numbers.finish(doc, store, &mut page, &furniture, warnings);
                            if !push_page_checked(
                                &mut pages,
                                &mut page,
                                Some((w, h)),
                                warnings,
                                rec,
                            ) {
                                page_budget_spent = true;
                                break;
                            }
                            prev_v_pos = -1;
                            col_band = 0;
                            // The continuation is real flow on the new page: a later explicit
                            // break must not be suppressed as "nothing on this page yet".
                            paras_on_page = 1;
                        }
                        for item in src.by_ref().take(end.saturating_sub(start_item)) {
                            page.items.push(if dy == 0.0 {
                                item
                            } else {
                                translate_item(item, 0.0, dy)
                            });
                        }
                    }
                    if page_budget_spent {
                        // The returned pages carry content whose provenance was recorded;
                        // publishing them without resolving would silently drop it.
                        rec.resolve(&pages);
                        return DisplayList { pages };
                    }
                    let last_dy = plan.last().map_or(0.0, |&(_, dy, _)| dy);
                    content_bottom = last_y + last_dy + descent + geom.spacing_bottom;
                    if para_split {
                        // The paragraph's remaining rows and any anchored object belong to the
                        // page it ended on, not to the one it started on.
                        para_top = Some(body_top);
                    }
                }
                // Whatever the band did not hand to an earlier page belongs to the page the
                // paragraph ended on.
                page_notes.extend(para_foot_notes.drain(..).map(|(_, note)| note));
                let (objects_bottom, objects_split) = layout_para_objects(
                    doc,
                    store,
                    &mut page,
                    para,
                    body_left,
                    para_top.unwrap_or(content_bottom),
                    content_bottom,
                    body_width,
                    (0.0, 0.0), // Top-level: a non-anchored shape's stored x/y is already page-absolute.
                    Some(&mut TableSplitCtx {
                        pages: &mut pages,
                        page_numbers: &mut page_numbers,
                        furniture: &furniture,
                        page_notes: &mut page_notes,
                        body_top,
                        body_bottom,
                        body_left,
                        body_width,
                        page_dims: (w, h),
                        prev_v_pos: &mut prev_v_pos,
                        paras_on_page: &mut paras_on_page,
                        col_band: &mut col_band,
                        has_flow: &mut has_flow_in_current_band,
                    }),
                    warnings,
                    rec,
                );
                content_bottom = objects_bottom;
                // A fallback paragraph is one slice with both borders. If an
                // attached table split, approximate only its final-page slice.
                if let Some(top) = para_top {
                    let (slice_insert, slice_top, slice_first) = if objects_split || para_split {
                        (0, body_top, false)
                    } else {
                        (bg_slice_insert, top, true)
                    };
                    let inserted = draw_para_bg_slice(
                        doc,
                        &mut page,
                        para,
                        body_left + geom.left,
                        (body_width - geom.left - geom.right).max(1.0),
                        slice_insert,
                        slice_top,
                        content_bottom,
                        slice_first,
                        true,
                        warnings,
                    );
                    rec.items_inserted(slice_insert, inserted);
                }
                rec.end_segment(&page);
                has_flow_in_current_band = true;
                continue;
            }

            let last_content = last_content_seg(para);
            for (i, seg) in para.line_segs.iter().enumerate() {
                // Bits0/1 declare authoritative page/column starts in saved
                // and synthesized caches. Without either boundary flag, use
                // the v_pos-reset heuristic.
                let (is_boundary, page_break) = if seg.flags & 0x3 != 0 {
                    (true, seg.flags & 0x1 != 0)
                } else if seg.v_pos < prev_v_pos {
                    let band = col_band + 1;
                    (true, col_count == 1 || band.is_multiple_of(col_count))
                } else {
                    (false, false)
                };
                if is_boundary && has_flow_in_current_band {
                    // Finish the current page/column background slice before
                    // crossing the boundary (GC-9), without drawing its lower edge.
                    if let Some(top) = bg_slice_top {
                        let inserted = draw_para_bg_slice(
                            doc,
                            &mut page,
                            para,
                            body_left + bg_slice_col_x + geom.left,
                            (col_width - geom.left - geom.right).max(1.0),
                            bg_slice_insert,
                            top,
                            content_bottom,
                            bg_first_slice,
                            false,
                            warnings,
                        );
                        rec.items_inserted(bg_slice_insert, inserted);
                        bg_first_slice = false;
                    }
                    if page_break {
                        // A page starts in band zero.
                        col_band = 0;
                        rec.content_end(&page);
                        render_page_notes(
                            doc,
                            store,
                            &mut page,
                            &page_notes,
                            body_left,
                            body_width,
                            body_bottom,
                            warnings,
                        );
                        page_notes.clear();
                        page_numbers.finish(doc, store, &mut page, &furniture, warnings);
                        if !push_page_checked(&mut pages, &mut page, Some((w, h)), warnings, rec) {
                            // The returned pages carry content whose provenance was recorded; publishing
                            // them without resolving would silently drop it. Still-open spans are
                            // dropped and `SegmentMap::truncated` says the set is incomplete.
                            rec.resolve(&pages);
                            return DisplayList { pages };
                        }
                        paras_on_page = 0;
                        // Re-anchor attached objects to the new page's flow
                        // position instead of the paragraph's stale first-page y.
                        para_top = None;
                    } else {
                        // A column boundary advances within the current page.
                        col_band += 1;
                    }
                    content_bottom = body_top;
                    // The next line establishes the new slice top and insertion
                    // point on the current page.
                    bg_slice_top = None;
                    bg_slice_insert = page.items.len();
                }
                prev_v_pos = seg.v_pos;
                has_flow_in_current_band = true;

                let line_start = seg.text_start;
                let line_end = para
                    .line_segs
                    .get(i + 1)
                    .map_or(para.wchar_len(), |next| next.text_start);
                if line_end <= line_start {
                    continue;
                }

                let mut items = shape_range_page(
                    store,
                    doc,
                    para,
                    (line_start, line_end),
                    &marks,
                    page_numbers.visible_number(),
                    warnings,
                );
                crate::shape::apply_link_style(&mut items, &links);
                let natural_width: f32 = items_width(&items, &tabs);

                // Alignment uses width after subtracting indent, so compute it below after resolving indent.
                let seg_width_pt = seg.seg_width as f32 / 100.0;
                let align = doc
                    .header
                    .para_shapes
                    .get(para.para_shape.0 as usize)
                    .map_or(0, |ps| ps.alignment());

                let baseline_gap_pt = seg.baseline_gap as f32 / 100.0;
                let line_height_pt = seg.line_height as f32 / 100.0;
                let stored_baseline = body_top + (seg.v_pos + seg.baseline_gap) as f32 / 100.0;
                // Flow correction: if previous content passed the saved position,
                // move the baseline down to content_bottom plus this line's ascent.
                let baseline_y = stored_baseline.max(content_bottom + baseline_gap_pt);

                let line_advance =
                    (seg.line_height + seg.line_spacing).max(seg.line_height) as f32 / 100.0;

                // Current column's x offset (col_start is column-relative and zero). Zero for a single column.
                let col_x = (col_band % col_count) as f32 * (col_width + col_gap);
                let box_x = body_left + col_x + seg.col_start as f32 / 100.0;
                // Start the background fragment at its page/column's first line top (GC-9).
                if bg_slice_top.is_none() {
                    bg_slice_top = Some(baseline_y - baseline_gap_pt);
                    bg_slice_col_x = col_x;
                }
                let mut marker_advance = 0.0;
                if i == 0 {
                    para_top = Some(baseline_y - baseline_gap_pt);
                    if let Some(m) = &marker {
                        let size = items_max_size(&items).unwrap_or(line_height_pt.max(8.0));
                        marker_advance = render_list_marker(
                            &mut page,
                            store,
                            doc,
                            m,
                            (box_x, baseline_y, size),
                            warnings,
                        );
                    }
                }
                // col_start contains only the left margin; apply first-line/hanging indent here.
                let (first_indent, rest_indent) = line_indents(&geom, marker_advance);
                let indent = if i == 0 { first_indent } else { rest_indent };
                // Align and wrap within the post-indent width so the right edge does not move.
                let text_width = (seg_width_pt - indent).max(1.0);
                let shift = align_line(
                    &mut items,
                    align,
                    text_width,
                    natural_width,
                    i == last_content,
                );
                let x = box_x + indent + shift;
                // A single-lineseg paragraph whose text substantially exceeds segment width has an
                // incomplete cache and wraps at segment width. Trust complete linesegs.
                // (`CACHE_OVERFLOW_TOLERANCE`).
                let wrap_width = if para.line_segs.len() == 1
                    && natural_width > text_width * CACHE_OVERFLOW_TOLERANCE
                {
                    text_width.max(10.0)
                } else {
                    f32::INFINITY
                };
                let last_y = place_wrapped(
                    &mut page,
                    items,
                    x,
                    baseline_y,
                    wrap_width,
                    line_advance,
                    &tabs,
                    0.0, // The line offset is already included in x.
                    &doc.header.border_fills,
                    None,
                    warnings,
                );
                content_bottom = last_y + (line_height_pt - baseline_gap_pt).max(0.0);
            }

            let (objects_bottom, objects_split) = layout_para_objects(
                doc,
                store,
                &mut page,
                para,
                body_left,
                para_top.unwrap_or(content_bottom),
                content_bottom,
                body_width,
                (0.0, 0.0), // Top-level: a non-anchored shape's stored x/y is already page-absolute.
                Some(&mut TableSplitCtx {
                    pages: &mut pages,
                    page_numbers: &mut page_numbers,
                    furniture: &furniture,
                    page_notes: &mut page_notes,
                    body_top,
                    body_bottom,
                    body_left,
                    body_width,
                    page_dims: (w, h),
                    prev_v_pos: &mut prev_v_pos,
                    paras_on_page: &mut paras_on_page,
                    col_band: &mut col_band,
                    has_flow: &mut has_flow_in_current_band,
                }),
                warnings,
                rec,
            );
            content_bottom = objects_bottom;
            // Final or only background fragment: bottom border on; top border only on the first fragment.
            if let Some(top) = bg_slice_top {
                let (slice_insert, slice_top, slice_first) = if objects_split {
                    (0, body_top, false)
                } else {
                    (bg_slice_insert, top, bg_first_slice)
                };
                let inserted = draw_para_bg_slice(
                    doc,
                    &mut page,
                    para,
                    body_left + bg_slice_col_x + geom.left,
                    (col_width - geom.left - geom.right).max(1.0),
                    slice_insert,
                    slice_top,
                    content_bottom,
                    slice_first,
                    true,
                    warnings,
                );
                rec.items_inserted(slice_insert, inserted);
            }
            rec.end_segment(&page);
        }
        if skipped_controls > 0 {
            warnings.push(
                RenderIssueCode::UnsupportedControlOmitted,
                skipped_controls.to_le_bytes(),
            );
        }
        let final_footnote_height =
            page_notes_reservation_height(doc, store, &page, &page_notes, body_left, body_width);
        render_page_notes(
            doc,
            store,
            &mut page,
            &page_notes,
            body_left,
            body_width,
            body_bottom,
            warnings,
        );
        page_notes.clear();
        // Endnotes form a section-closing flow. Partition them by measured
        // note height, preserving last-page footnote space and opening as many
        // continuation pages as required.
        if !pending_endnotes.is_empty() {
            let heights: Vec<f32> = pending_endnotes
                .iter()
                .map(|note| measure_note_height(doc, store, &page, note, body_left, body_width))
                .collect();
            let mut next = 0usize;
            let mut page_limit = (body_bottom - final_footnote_height).max(body_top);
            while next < pending_endnotes.len() {
                let available = (page_limit - content_bottom).max(0.0);
                let mut used = 5.0; // separator gap
                let mut end = next;
                while end < pending_endnotes.len() && used + heights[end] <= available + 0.01 {
                    used += heights[end];
                    end += 1;
                }

                if end == next
                    && (content_bottom > body_top + 0.01 || page_limit < body_bottom - 0.01)
                {
                    rec.content_end(&page);
                    page_numbers.finish(doc, store, &mut page, &furniture, warnings);
                    if !push_page_checked(&mut pages, &mut page, Some((w, h)), warnings, rec) {
                        // The returned pages carry content whose provenance was recorded; publishing
                        // them without resolving would silently drop it. Still-open spans are
                        // dropped and `SegmentMap::truncated` says the set is incomplete.
                        rec.resolve(&pages);
                        return DisplayList { pages };
                    }
                    content_bottom = body_top;
                    page_limit = body_bottom;
                    continue;
                }

                // A single note may itself exceed the body height. Consume it
                // to guarantee progress; the renderer will clip that legacy
                // unsupported case instead of looping indefinitely.
                if end == next {
                    used += heights[end];
                    end += 1;
                }
                let block_bottom = (content_bottom + used).min(page_limit);
                render_page_notes(
                    doc,
                    store,
                    &mut page,
                    &pending_endnotes[next..end],
                    body_left,
                    body_width,
                    block_bottom,
                    warnings,
                );
                content_bottom = (content_bottom + used).min(page_limit);
                next = end;
                if next < pending_endnotes.len() {
                    rec.content_end(&page);
                    page_numbers.finish(doc, store, &mut page, &furniture, warnings);
                    if !push_page_checked(&mut pages, &mut page, Some((w, h)), warnings, rec) {
                        // The returned pages carry content whose provenance was recorded; publishing
                        // them without resolving would silently drop it. Still-open spans are
                        // dropped and `SegmentMap::truncated` says the set is incomplete.
                        rec.resolve(&pages);
                        return DisplayList { pages };
                    }
                    content_bottom = body_top;
                    page_limit = body_bottom;
                }
            }
            pending_endnotes.clear();
        }
        rec.content_end(&page);
        page_numbers.finish(doc, store, &mut page, &furniture, warnings);
        if !push_page_checked(&mut pages, &mut page, None, warnings, rec) {
            // The returned pages carry content whose provenance was recorded; publishing
            // them without resolving would silently drop it. Still-open spans are
            // dropped and `SegmentMap::truncated` says the set is incomplete.
            rec.resolve(&pages);
            return DisplayList { pages };
        }

        // Turn this section's spans into rows **before** any furniture is prepended: the
        // prepends splice onto the front of `page.items` and would shift every recorded index.
        rec.resolve(&pages);

        // Prepend the page border to every page in this section.
        if let Some(border) = &page_border {
            for p in &mut pages[section_first_page..] {
                prepend_page_borders(p, border, warnings);
            }
        }

        // GG-17 prepends dividers to every page of a multi-column section.
        // Body height remains an approximation until Hancom samples establish
        // the exact per-page column extent.
        if col_count > 1
            && let Some(divider) = col_def.and_then(|c| c.divider)
            && divider.is_visible()
        {
            for p in &mut pages[section_first_page..] {
                prepend_col_dividers(
                    p,
                    &divider,
                    body_left,
                    body_top,
                    body_bottom,
                    col_width,
                    col_gap,
                    col_count,
                    warnings,
                );
            }
        }
    }

    DisplayList { pages }
}

fn push_page_checked(
    pages: &mut Vec<PageList>,
    page: &mut PageList,
    next_dimensions: Option<(f32, f32)>,
    warnings: &mut RenderIssueAccumulator,
    rec: &mut SegmentRecorder,
) -> bool {
    if !warnings.charge_page() {
        return false;
    }
    // The one place a page is pushed, and so the one place the recorder's page ordinal moves.
    rec.page_pushed();
    let next = next_dimensions.map_or(
        PageList {
            width_pt: 0.0,
            height_pt: 0.0,
            items: Vec::new(),
        },
        |(width_pt, height_pt)| PageList {
            width_pt,
            height_pt,
            items: Vec::new(),
        },
    );
    pages.push(std::mem::replace(page, next));
    true
}

/// Page-transition state used while splitting a body-flow table.
///
/// Nested tables inside cells or text boxes receive `None`. The current page
/// stays with the caller and is passed to `break_page` to avoid aliasing it.
struct TableSplitCtx<'a, 's: 'a> {
    pages: &'a mut Vec<PageList>,
    page_numbers: &'a mut PageNumberState,
    furniture: &'a Furniture<'s>,
    page_notes: &'a mut Vec<&'s Note<'s>>,
    body_top: f32,
    body_bottom: f32,
    body_left: f32,
    body_width: f32,
    page_dims: (f32, f32),
    prev_v_pos: &'a mut i32,
    paras_on_page: &'a mut usize,
    col_band: &'a mut usize,
    has_flow: &'a mut bool,
}

impl TableSplitCtx<'_, '_> {
    /// Runs the normal page-finalization sequence and opens a fresh page.
    /// Returns false if the page budget prevents the transition.
    fn break_page(
        &mut self,
        doc: &Document,
        store: &mut FontStore,
        page: &mut PageList,
        warnings: &mut RenderIssueAccumulator,
        rec: &mut SegmentRecorder,
    ) -> bool {
        rec.content_end(page);
        render_page_notes(
            doc,
            store,
            page,
            self.page_notes,
            self.body_left,
            self.body_width,
            self.body_bottom,
            warnings,
        );
        self.page_notes.clear();
        self.page_numbers
            .finish(doc, store, page, self.furniture, warnings);
        if !push_page_checked(self.pages, page, Some(self.page_dims), warnings, rec) {
            return false;
        }
        *self.prev_v_pos = -1;
        *self.paras_on_page = 0;
        *self.col_band = 0;
        *self.has_flow = false;
        true
    }

    /// Mark the continuation fragment as real flow on the new page. A later
    /// explicit or cached page break must not be suppressed merely because
    /// the page was opened by a table continuation.
    fn mark_flow(&mut self) {
        *self.paras_on_page = (*self.paras_on_page).max(1);
        *self.has_flow = true;
    }
}

/// One cached page-sized run of a cell.  The selections retain paragraph
/// boundaries, while `first_v_pos` translates Hancom's page-relative cache to
/// the fragment's page-space origin.
#[derive(Debug, Clone)]
struct CellFragmentGroup {
    selections: Vec<BoxParaSelection>,
    first_v_pos: i32,
    height: f32,
    has_content: bool,
}

#[derive(Debug, Clone)]
struct CellSplitPlan {
    groups: Vec<CellFragmentGroup>,
}

impl CellSplitPlan {
    fn is_usable(&self) -> bool {
        // Trailing blank groups carry declared row-height remainder only, so
        // they need no cached text.  Every fragment up to the last content
        // run must own cached geometry — a split required for text can never
        // be satisfied by a blank page.
        let Some(last_content) = self.groups.iter().rposition(|group| group.has_content) else {
            return false;
        };
        self.groups.len() > 1
            && self.groups[..=last_content]
                .iter()
                .all(|group| group.has_content)
    }
}

/// Build Regime-A cell fragments from the line layout saved by Hancom.
///
/// A CELL table is only safely splittable at a cached line boundary. Explicit
/// cached page starts (`flags & 1`) are preserved as hard page breaks.  A
/// vertical-position reset without that flag is a soft boundary: Hancom also
/// restarts `v_pos` for reasons other than a page break, so the run after the
/// reset is packed into the page capacity left by the previous fragment
/// whenever a complete cached line still fits there.  Otherwise a boundary is
/// selected immediately before the first cached line that would exceed the
/// remaining page capacity. A paragraph without line segments is not guessed
/// into a page: callers must surface an incomplete render when that paragraph
/// is part of a required split.
///
/// `fragment_margin_v` is the vertical cell margin each emitted fragment
/// spends on the page; it keeps the simulated capacity in step with the
/// emitter's greedy fragment packing.
fn cached_cell_split_plan(
    cell: &hwp_model::Cell,
    measured_content_h: f32,
    first_capacity_pt: f32,
    following_capacity_pt: f32,
    fragment_margin_v: f32,
) -> Option<CellSplitPlan> {
    let para_count = cell.paragraphs.len();
    let mut first_v = vec![0i32];
    let mut max_end = vec![0i32];
    let mut has_content = vec![false];
    let mut boundaries = HashSet::new();
    let mut current = 0usize;
    let mut previous_v = None;
    let mut saw_line = false;
    let mut current_capacity = first_capacity_pt.max(0.0);
    let following_capacity = following_capacity_pt.max(1.0);
    // Simulated content-space capacity left on the current page.  Closing a
    // fragment spends its height plus one vertical cell margin.
    let mut capacity_left = current_capacity;

    for (para_index, para) in cell.paragraphs.iter().enumerate() {
        if para.line_segs.is_empty() {
            // Empty paragraphs are harmless; text without cached geometry is
            // not, because it could cross the required split.
            if !para.chars.is_empty() {
                return None;
            }
            continue;
        }
        let wchar_len = para.wchar_len();
        if para
            .line_segs
            .iter()
            .any(|seg| seg.text_start > wchar_len || seg.line_height <= 0 || seg.v_pos < 0)
        {
            return None;
        }

        for (seg_index, seg) in para.line_segs.iter().enumerate() {
            let line_end = para
                .line_segs
                .get(seg_index + 1)
                .map_or(wchar_len, |next| next.text_start);
            if line_end <= seg.text_start {
                // A trailing empty cache record is legal.  It carries no
                // visible content and therefore cannot define a split.
                continue;
            }
            let end = seg
                .v_pos
                .saturating_add(seg.line_height.max(seg.text_height));
            if !saw_line && (end - seg.v_pos) as f32 / 100.0 > current_capacity + 0.5 {
                // No complete cached line fits in the current-page sliver.
                // Size the first group for the fresh-page capacity; the
                // emitter will move it before drawing.
                current_capacity = following_capacity;
                capacity_left = following_capacity;
            }
            let explicit_boundary = saw_line && seg.flags & 0x1 != 0;
            let reset_boundary = saw_line
                && !explicit_boundary
                && previous_v.is_some_and(|previous| seg.v_pos < previous);
            let capacity_boundary = saw_line
                && !explicit_boundary
                && !reset_boundary
                && has_content[current]
                && (end - first_v[current]) as f32 / 100.0 > current_capacity + 0.5;
            if explicit_boundary || reset_boundary || capacity_boundary {
                // `closed_height` is content-only and both capacities were
                // passed in already excluding one vertical margin per
                // fragment, so charging `fragment_margin_v` exactly once per
                // closed fragment is correct — not a double subtraction.
                // The emitter draws each fragment as content + mt + mb, and
                // every fragment after the first must reserve its own margin
                // out of what remains on the simulated page.
                let closed_height = (max_end[current] - first_v[current]).max(0) as f32 / 100.0;
                capacity_left = (capacity_left - closed_height - fragment_margin_v).max(0.0);
                if explicit_boundary || capacity_boundary {
                    // A hard page break or a filled page starts the next
                    // fragment on a fresh page.
                    capacity_left = following_capacity;
                    current_capacity = following_capacity;
                } else {
                    // Soft reset: keep the following run on the current page
                    // when its first complete cached line fits the remaining
                    // capacity.  A fragment that may be moved by the emitter
                    // is never sized beyond a fresh page.
                    let soft = capacity_left.min(following_capacity);
                    if (end - seg.v_pos) as f32 / 100.0 > soft + 0.5 {
                        capacity_left = following_capacity;
                        current_capacity = following_capacity;
                    } else {
                        current_capacity = soft;
                    }
                }
                boundaries.insert((para_index, seg_index));
                current += 1;
                first_v.push(seg.v_pos);
                max_end.push(seg.v_pos);
                has_content.push(false);
            }
            if !has_content[current] {
                first_v[current] = seg.v_pos;
            }
            max_end[current] = max_end[current].max(end);
            has_content[current] = true;
            previous_v = Some(seg.v_pos);
            saw_line = true;
        }
    }

    // Rebuild the per-group paragraph ranges in a second, deterministic pass.
    // Keeping the validation pass separate makes malformed cache handling
    // explicit and reuses the exact explicit/capacity decisions above.
    if !saw_line || !has_content.iter().all(|value| *value) {
        return None;
    }
    let group_count = has_content.len();
    let mut groups = (0..group_count)
        .map(|index| CellFragmentGroup {
            selections: vec![BoxParaSelection::Empty; para_count],
            first_v_pos: first_v[index],
            height: ((max_end[index] - first_v[index]).max(1) as f32) / 100.0,
            has_content: true,
        })
        .collect::<Vec<_>>();
    current = 0;
    for (para_index, para) in cell.paragraphs.iter().enumerate() {
        if para.line_segs.is_empty() {
            // A control-only paragraph has no line boundary of its own. It is
            // attached to the current cached group so nested tables/images
            // are emitted once without inventing a page split. A paragraph
            // containing text would need a cached range and was rejected in
            // the validation pass above.
            if !para.controls.is_empty() {
                groups[current].selections[para_index] = BoxParaSelection::All;
            }
            continue;
        }
        let wchar_len = para.wchar_len();
        for (seg_index, seg) in para.line_segs.iter().enumerate() {
            let line_end = para
                .line_segs
                .get(seg_index + 1)
                .map_or(wchar_len, |next| next.text_start);
            if line_end <= seg.text_start {
                continue;
            }
            if boundaries.contains(&(para_index, seg_index)) {
                current += 1;
            }
            match &mut groups[current].selections[para_index] {
                BoxParaSelection::Empty => {
                    groups[current].selections[para_index] =
                        BoxParaSelection::Segments(seg_index..seg_index + 1);
                }
                BoxParaSelection::Segments(range) => range.end = seg_index + 1,
                BoxParaSelection::All => return None,
            }
        }
    }

    // A measured nested object or a cache tail can add a small amount beyond
    // the line geometry. Keep object overflow on the fragment that owns the
    // object; otherwise keep ordinary cache drift on the final fragment rather
    // than manufacturing another page boundary that is not in the source.
    let cached_height: f32 = groups.iter().map(|group| group.height).sum();
    if measured_content_h > cached_height
        && let Some(target) = groups
            .iter()
            .position(|group| {
                group
                    .selections
                    .iter()
                    .enumerate()
                    .any(|(para_index, selection)| {
                        !cell.paragraphs[para_index].controls.is_empty()
                            && match selection {
                                BoxParaSelection::All => true,
                                BoxParaSelection::Segments(range) => range.start == 0,
                                BoxParaSelection::Empty => false,
                            }
                    })
            })
            .or_else(|| groups.len().checked_sub(1))
            .and_then(|index| groups.get_mut(index))
    {
        target.height += measured_content_h - cached_height;
    }
    Some(CellSplitPlan { groups })
}

/// Page furniture (headers and footers), drawn whenever a page is finalized.
/// Header/footer entries retain their apply target and are selected by printed
/// page parity (GG-16).
struct Furniture<'a> {
    header: Vec<(&'a hwp_model::GenericControl, u8)>,
    footer: Vec<(&'a hwp_model::GenericControl, u8)>,
    page_def: &'a PageDef,
    body_left: f32,
    body_width: f32,
}

/// Header/footer apply target in `data[0..4]` LE u32 bits 0-1:
/// 0=both, 1=even, 2=odd. Missing or short data defaults to both.
fn head_foot_apply(g: &hwp_model::GenericControl) -> u8 {
    g.data.get(0..4).map_or(0, |b| {
        (u32::from_le_bytes([b[0], b[1], b[2], b[3]]) & 0x3) as u8
    })
}

/// Selects an exact parity entry, then a BOTH fallback. A parity-specific
/// header must not leak onto the opposite page when no fallback exists.
fn select_furniture<'a>(
    entries: &[(&'a hwp_model::GenericControl, u8)],
    printed: u32,
) -> Option<&'a hwp_model::GenericControl> {
    let want = if printed % 2 == 1 { 2 } else { 1 };
    entries
        .iter()
        .find(|e| e.1 == want)
        .or_else(|| entries.iter().find(|e| e.1 == 0))
        .map(|e| e.0)
}

impl Furniture<'_> {
    fn render(
        &self,
        doc: &Document,
        store: &mut FontStore,
        page: &mut PageList,
        page_number: Option<u32>,
        printed: u32,
        warnings: &mut RenderIssueAccumulator,
    ) {
        if let Some(h) = select_furniture(&self.header, printed) {
            let top = self.page_def.margin_top.to_pt() as f32;
            for list in &h.paragraph_lists {
                layout_box_paragraphs(
                    doc,
                    store,
                    page,
                    &list.paragraphs,
                    self.body_left,
                    top,
                    self.body_width,
                    warnings,
                    None,
                    page_number,
                );
            }
        }
        if let Some(f) = select_furniture(&self.footer, printed) {
            let top = page.height_pt
                - self.page_def.margin_bottom.to_pt() as f32
                - self.page_def.margin_footer.to_pt() as f32;
            for list in &f.paragraph_lists {
                layout_box_paragraphs(
                    doc,
                    store,
                    page,
                    &list.paragraphs,
                    self.body_left,
                    top,
                    self.body_width,
                    warnings,
                    None,
                    page_number,
                );
            }
        }
    }
}

fn measure_note_height(
    doc: &Document,
    store: &mut FontStore,
    page: &PageList,
    note: &Note,
    body_left: f32,
    body_width: f32,
) -> f32 {
    let mut scratch = PageList {
        width_pt: page.width_pt,
        height_pt: page.height_pt,
        items: Vec::new(),
    };
    let mut scratch_warnings = RenderIssueAccumulator::new();
    render_one_note(
        doc,
        store,
        &mut scratch,
        note,
        body_left,
        body_width,
        0.0,
        &mut scratch_warnings,
    ) + 3.0
}

fn page_notes_reservation_height(
    doc: &Document,
    store: &mut FontStore,
    page: &PageList,
    notes: &[&Note],
    body_left: f32,
    body_width: f32,
) -> f32 {
    if notes.is_empty() {
        return 0.0;
    }
    let mut scratch = PageList {
        width_pt: page.width_pt,
        height_pt: page.height_pt,
        items: Vec::new(),
    };
    let mut scratch_warnings = RenderIssueAccumulator::new();
    let mut height = 0.0f32;
    for note in notes {
        height = render_one_note(
            doc,
            store,
            &mut scratch,
            note,
            body_left,
            body_width,
            height,
            &mut scratch_warnings,
        );
        height += 3.0;
    }
    height + 5.0
}

/// Draw the footnote/endnote area at the page bottom (separator, numbers and content).
/// Move the block up so its bottom meets body_bottom.
#[allow(clippy::too_many_arguments)]
fn render_page_notes(
    doc: &Document,
    store: &mut FontStore,
    page: &mut PageList,
    notes: &[&Note],
    body_left: f32,
    body_width: f32,
    body_bottom: f32,
    warnings: &mut RenderIssueAccumulator,
) {
    if notes.is_empty() {
        return;
    }
    // 1) Stack notes from y=0 on a scratch page to measure total height.
    let mut scratch = PageList {
        width_pt: page.width_pt,
        height_pt: page.height_pt,
        items: Vec::new(),
    };
    let mut y = 0.0f32;
    for note in notes {
        y = render_one_note(
            doc,
            store,
            &mut scratch,
            note,
            body_left,
            body_width,
            y,
            warnings,
        );
        y += 3.0; // Gap between notes.
    }
    // 2) Move up so the block bottom meets body_bottom; retain any overlap with body content.
    let top = (body_bottom - y).max(0.0);
    let sep_gap = 5.0;
    if !warnings.charge_display_items(1) {
        return;
    }
    page.items.push(Item::Path {
        commands: vec![
            PathCmd::MoveTo(body_left, top - sep_gap),
            PathCmd::LineTo(body_left + body_width * 0.34, top - sep_gap),
        ],
        fill: None,
        stroke: Some(Stroke::solid(0x0000_0000, 0.5)),
        rule: FillRule::NonZero,
    });
    // 3) Translate scratch items down by top and append them to the real page.
    for item in scratch.items.drain(..) {
        page.items.push(translate_item(item, 0.0, top));
    }
}

/// Draw one note (number marker and content paragraphs) at (x, y), returning its bottom y.
#[allow(clippy::too_many_arguments)]
fn render_one_note(
    doc: &Document,
    store: &mut FontStore,
    page: &mut PageList,
    note: &Note,
    x: f32,
    width: f32,
    y: f32,
    warnings: &mut RenderIssueAccumulator,
) -> f32 {
    let marker_size = 8.0;
    let indent = 16.0_f32.min(width * 0.25);
    let label = format!("{})", note.number);
    let baseline = y + marker_size;
    if let Some(run) = crate::shape::shape_plain(store, doc, &label, marker_size, 0, false) {
        push_run(page, x, baseline, run, &doc.header.border_fills, warnings);
    }
    // Content paragraphs use their own char_shape sizes and stack vertically.
    let mut bottom = y;
    for list in &note.content.paragraph_lists {
        bottom = layout_box_paragraphs(
            doc,
            store,
            page,
            &list.paragraphs,
            x + indent,
            bottom,
            width - indent,
            warnings,
            None,
            None,
        );
    }
    bottom.max(baseline + marker_size * 0.3)
}

/// Draw a list marker (bullet/number) hanging left of the text start.
fn render_list_marker(
    page: &mut PageList,
    store: &mut FontStore,
    doc: &Document,
    marker: &str,
    placement: (f32, f32, f32),
    warnings: &mut RenderIssueAccumulator,
) -> f32 {
    let (box_left, baseline, size) = placement;
    let Some(run) = crate::shape::shape_plain(store, doc, marker, size, 0, false) else {
        return 0.0;
    };
    let advance = run.width_pt + size * 0.3;
    push_run(
        page,
        box_left,
        baseline,
        run,
        &doc.header.border_fills,
        warnings,
    );
    advance
}

/// Text offsets (first line, following lines) from the left edge of the line box.
///
/// Ground truth: a genuine Hangul `line_seg` stores only the paragraph's left
/// margin in `horzpos` — never the first-line indent, not even for a hanging
/// indent. The hanging space belongs to the list marker and lives *inside* the
/// text area, so the marker sits at the box edge, the first line clears it, and
/// every following line is indented by the hanging width. A positive indent
/// (positive indent) applies to the first line only.
fn line_indents(geom: &ParaGeom, marker_advance: f32) -> (f32, f32) {
    let hang = (-geom.first_indent).max(0.0);
    let first = if marker_advance > 0.0 {
        marker_advance.max(hang)
    } else {
        geom.first_indent.max(0.0)
    };
    (first, hang)
}

/// Converts a `BorderFill` background into a display item (GG-7).
/// Gradients and hatches require `Item::Path`; solid fills retain `Item::Rect`.
fn bg_fill_item(bf: &BorderFill, x: f32, y: f32, w: f32, h: f32) -> Option<Item> {
    if let Some(g) = bf.visible_gradient() {
        return Some(Item::Path {
            commands: rect_path_cmds(x, y, w, h),
            fill: Some(Fill::Gradient(Gradient {
                radial: g.radial,
                angle_deg: g.angle_deg,
                stops: g.stops.clone(),
            })),
            stroke: None,
            rule: FillRule::NonZero,
        });
    }
    if let Some((fg, style, bg)) = bf.visible_hatch() {
        return Some(Item::Path {
            commands: rect_path_cmds(x, y, w, h),
            fill: Some(Fill::Hatch {
                fg,
                bg,
                style: style as u32,
            }),
            stroke: None,
            rule: FillRule::NonZero,
        });
    }
    bf.visible_bg().map(|fill| Item::Rect { x, y, w, h, fill })
}

/// Axis-aligned rectangle path for background fills.
fn rect_path_cmds(x: f32, y: f32, w: f32, h: f32) -> Vec<PathCmd> {
    vec![
        PathCmd::MoveTo(x, y),
        PathCmd::LineTo(x + w, y),
        PathCmd::LineTo(x + w, y + h),
        PathCmd::LineTo(x, y + h),
        PathCmd::Close,
    ]
}

/// Copy an Item translated by (dx, dy).
fn translate_item(item: Item, dx: f32, dy: f32) -> Item {
    match item {
        Item::Glyphs { x, y, run } => Item::Glyphs {
            x: x + dx,
            y: y + dy,
            run,
        },
        Item::Rect { x, y, w, h, fill } => Item::Rect {
            x: x + dx,
            y: y + dy,
            w,
            h,
            fill,
        },
        Item::Image {
            x,
            y,
            w,
            h,
            data,
            crop,
            flip,
            rotation_deg,
            brightness,
            contrast,
        } => Item::Image {
            x: x + dx,
            y: y + dy,
            w,
            h,
            data,
            crop,
            flip,
            rotation_deg,
            brightness,
            contrast,
        },
        Item::Path {
            commands,
            fill,
            stroke,
            rule,
        } => Item::Path {
            commands: commands
                .into_iter()
                .map(|c| translate_cmd(c, dx, dy))
                .collect(),
            fill,
            stroke,
            rule,
        },
    }
}

/// Translate a PathCmd by (dx, dy).
fn translate_cmd(c: PathCmd, dx: f32, dy: f32) -> PathCmd {
    match c {
        PathCmd::MoveTo(x, y) => PathCmd::MoveTo(x + dx, y + dy),
        PathCmd::LineTo(x, y) => PathCmd::LineTo(x + dx, y + dy),
        PathCmd::CubicTo(a, b, c, d, e, f) => {
            PathCmd::CubicTo(a + dx, b + dy, c + dx, d + dy, e + dx, f + dy)
        }
        PathCmd::Close => PathCmd::Close,
    }
}

/// Caption layout width in points: explicit width for vertical captions, or
/// the object width otherwise (table 72).
fn caption_wrap_pt(caption: &Caption, obj_width: f32) -> f32 {
    caption
        .width
        .map_or(obj_width, |w| (w as f32 / 100.0).max(4.0))
}

struct CaptionPrelude {
    items: Vec<Item>,
    height: f32,
    gap: f32,
}

struct CaptionBlock<'a> {
    caption: &'a Caption,
    items: Vec<Item>,
    height: f32,
    wrap: f32,
}

fn prepare_caption_block<'a>(
    doc: &Document,
    store: &mut FontStore,
    page: &PageList,
    caption: Option<&'a Caption>,
    object_width: f32,
    warnings: &mut RenderIssueAccumulator,
) -> Option<CaptionBlock<'a>> {
    caption.map(|caption| {
        let wrap = caption_wrap_pt(caption, object_width);
        let (items, height) = layout_caption_items(doc, store, page, caption, wrap, warnings);
        CaptionBlock {
            caption,
            items,
            height,
            wrap,
        }
    })
}

fn inline_top_caption_offset(block: Option<&CaptionBlock<'_>>) -> f32 {
    block
        .filter(|block| block.caption.side == CaptionSide::Top)
        .map_or(0.0, |block| block.height + block.caption.gap as f32 / 100.0)
}

fn place_caption_block(
    page: &mut PageList,
    block: CaptionBlock<'_>,
    object: (f32, f32, f32, f32),
) -> f32 {
    let object_bottom = object.1 + object.3;
    let side = block.caption.side;
    let height = block.height;
    let bottom = place_caption_items(page, block.items, height, block.caption, object, block.wrap);
    match side {
        CaptionSide::Bottom => bottom.unwrap_or(object_bottom),
        CaptionSide::Left | CaptionSide::Right => object_bottom.max(object.1 + height),
        CaptionSide::Top => object_bottom,
    }
}

/// Lays caption paragraphs onto a scratch page and returns items plus height.
/// The caller translates them according to the caption side (GB-13).
fn layout_caption_items(
    doc: &Document,
    store: &mut FontStore,
    page: &PageList,
    caption: &Caption,
    wrap_width: f32,
    warnings: &mut RenderIssueAccumulator,
) -> (Vec<Item>, f32) {
    if caption.paragraphs.is_empty() {
        return (Vec::new(), 0.0);
    }
    let mut scratch = PageList {
        width_pt: page.width_pt,
        height_pt: page.height_pt,
        items: Vec::new(),
    };
    let bottom = layout_box_paragraphs(
        doc,
        store,
        &mut scratch,
        &caption.paragraphs,
        0.0,
        0.0,
        wrap_width.max(4.0),
        warnings,
        None,
        None,
    );
    (scratch.items, bottom)
}

/// Places a caption beside the object rectangle with its configured gap.
/// Returns the caption bottom for bottom captions so body flow can advance.
fn place_caption_items(
    page: &mut PageList,
    items: Vec<Item>,
    height: f32,
    caption: &Caption,
    obj: (f32, f32, f32, f32),
    wrap: f32,
) -> Option<f32> {
    if items.is_empty() {
        return None;
    }
    let gap = caption.gap as f32 / 100.0;
    let (ox, oy, ow, oh) = obj;
    let (dx, dy) = match caption.side {
        CaptionSide::Bottom => (ox, oy + oh + gap),
        CaptionSide::Top => (ox, oy - gap - height),
        CaptionSide::Left => ((ox - gap - wrap).max(0.0), oy),
        CaptionSide::Right => (ox + ow + gap, oy),
    };
    for item in items {
        page.items.push(translate_item(item, dx, dy));
    }
    (caption.side == CaptionSide::Bottom).then_some(oy + oh + gap + height)
}

/// Lay out paragraph-attached block objects (tables/images), returning the updated content bottom.
#[allow(clippy::too_many_arguments)]
fn layout_para_objects(
    doc: &Document,
    store: &mut FontStore,
    page: &mut PageList,
    para: &Paragraph,
    x: f32,
    anchor_top: f32,
    content_bottom: f32,
    avail_width: f32,
    // The page-absolute origin of the *enclosing* shape, when this call lays out a
    // paragraph reached recursively (via `layout_box_para_iter`, e.g. a text box's own
    // nested paragraph_lists). A non-anchored `Generic` control's own stored `gso_shapes`
    // x/y is page-absolute at the top level but relative to this origin when nested one
    // level deeper (issue #255 / GG-26; `ContainerBox`'s own doc comment: "a container's
    // child shapes are relative to the container's own origin",
    // `crates/hwp-model/src/control.rs`). Top-level call sites pass `(0.0, 0.0)`, a no-op.
    nested_origin: (f32, f32),
    mut split: Option<&mut TableSplitCtx<'_, '_>>,
    warnings: &mut RenderIssueAccumulator,
    rec: &mut SegmentRecorder,
) -> (f32, bool) {
    let mut bottom = content_bottom;
    let mut object_y = anchor_top;
    // After a split, the returned cursor is relative to the final page.
    let mut page_split = false;

    for (control_index, control) in para.controls.iter().enumerate() {
        match control {
            // A bookmark is an invisible point marker by design (GG-25): it never reaches a
            // display item, so it is recorded directly instead of as a span (D-08a).
            Control::Generic(generic) if generic.ctrl_id == *b"bokm" => {
                rec.bookmark(control_index, generic, para);
            }
            Control::Table(table) => {
                rec.begin_table(control_index, table, page);
                // Measure a top caption before table planning so every fragment
                // reserves the correct first-fragment offset. The table emitter
                // places it after any page break selected by the planner.
                let mut top_caption: Option<CaptionPrelude> = None;
                if let Some(cap) = &table.caption
                    && cap.side == CaptionSide::Top
                {
                    let wrap = caption_wrap_pt(cap, avail_width);
                    let (items, h) = layout_caption_items(doc, store, page, cap, wrap, warnings);
                    let gap = cap.gap as f32 / 100.0;
                    top_caption = Some(CaptionPrelude {
                        items,
                        height: h,
                        gap,
                    });
                    object_y += h + gap;
                }
                let (end, table_split, final_fragment_top) = layout_table(
                    doc,
                    store,
                    page,
                    table,
                    x,
                    object_y,
                    avail_width,
                    top_caption,
                    split.as_deref_mut(),
                    warnings,
                    rec,
                );
                rec.end_segment(page);
                if table_split {
                    // After a split, the cursor uses the new page's coordinates, not the previous
                    // page's bottom; doing so would create a blank page.
                    bottom = end;
                    page_split = true;
                } else {
                    bottom = bottom.max(end);
                }
                object_y = end; // Stack multiple objects in one paragraph vertically.
                // Use available width as the table-width approximation; it only
                // affects the horizontal position of left/right captions.
                if let Some(cap) = &table.caption {
                    let wrap = caption_wrap_pt(cap, avail_width);
                    if cap.side != CaptionSide::Top {
                        let (items, h) =
                            layout_caption_items(doc, store, page, cap, wrap, warnings);
                        let obj = (x, final_fragment_top, avail_width, end - final_fragment_top);
                        if let Some(cap_bottom) =
                            place_caption_items(page, items, h, cap, obj, wrap)
                        {
                            bottom = bottom.max(cap_bottom);
                            object_y = cap_bottom;
                        }
                    }
                }
            }
            Control::Picture(pic) => {
                let (w, h) = (pic.width.to_pt() as f32, pic.height.to_pt() as f32);
                if w <= 0.0 || h <= 0.0 {
                    warnings.push(RenderIssueCode::ImageSizeMissingOmitted, b"picture");
                    continue;
                }
                match doc.resolve_bin(&pic.bin_ref) {
                    Some(bytes) => {
                        // Picture effects from tables 108-116 are reported but not yet rendered.
                        if pic.effect_flags != 0 {
                            warnings.push(
                                RenderIssueCode::PictureEffectsUnsupported,
                                format!("flags={:#010x}", pic.effect_flags),
                            );
                        }
                        // Reserve space above the picture for a top caption.
                        let mut img_y = object_y;
                        let mut top_caption: Option<(Vec<Item>, f32)> = None;
                        if let Some(cap) = &pic.caption
                            && cap.side == CaptionSide::Top
                        {
                            let wrap = caption_wrap_pt(cap, w);
                            let (items, ch) =
                                layout_caption_items(doc, store, page, cap, wrap, warnings);
                            top_caption = Some((items, ch));
                            img_y += ch + cap.gap as f32 / 100.0;
                        }
                        // Lay out WMF vectors here only without transforms (flip/rotation/brightness/contrast).
                        // Transformed images retain the existing raster path
                        // (Item::Image -> backend decode). WMF intersects cropping with its own window;
                        // the corpus uses full-extent crops throughout.
                        // File extensions are ignored; byte sniffing is authoritative.
                        // Sniffing examines about 20 bytes, so a separate fast path
                        // adds no benefit. Placeholder already emits a magenta rectangle and
                        // a typed issue from wmf; Fallback emits nothing,
                        // so it returns to the raster path.
                        let untransformed = pic.flip == 0
                            && pic.rotation.unwrap_or(0.0) == 0.0
                            && pic.brightness == 0
                            && pic.contrast == 0;
                        let wmf_handled = untransformed
                            && crate::wmf::is_wmf(bytes)
                            && crate::wmf::draw_wmf(
                                bytes, x, img_y, w, h, pic.crop, page, store, warnings,
                            ) != crate::wmf::WmfOutcome::Fallback;
                        if !wmf_handled {
                            if !warnings.charge_display_items(1) {
                                return (bottom, page_split);
                            }
                            page.items.push(Item::Image {
                                x,
                                y: img_y,
                                w,
                                h,
                                data: warnings.cached_binary(bytes),
                                crop: pic.crop,
                                flip: pic.flip,
                                rotation_deg: pic.rotation.unwrap_or(0.0),
                                brightness: pic.brightness,
                                contrast: pic.contrast,
                            });
                        }
                        bottom = bottom.max(img_y + h);
                        object_y = img_y + h;
                        // Place the caption; bottom is the default side.
                        if let Some(cap) = &pic.caption {
                            let wrap = caption_wrap_pt(cap, w);
                            if cap.side == CaptionSide::Top {
                                if let Some((items, ch)) = top_caption {
                                    place_caption_items(
                                        page,
                                        items,
                                        ch,
                                        cap,
                                        (x, img_y, w, 0.0),
                                        wrap,
                                    );
                                }
                            } else {
                                let (items, ch) =
                                    layout_caption_items(doc, store, page, cap, wrap, warnings);
                                let obj = (x, img_y, w, h);
                                if let Some(cap_bottom) =
                                    place_caption_items(page, items, ch, cap, obj, wrap)
                                {
                                    bottom = bottom.max(cap_bottom);
                                    object_y = cap_bottom;
                                }
                            }
                        }
                    }
                    None => warnings.push(
                        RenderIssueCode::ImageDataMissingOmitted,
                        format!("{:?}", pic.bin_ref),
                    ),
                }
            }
            // Text box: lay out the GSO's paragraph lists inside its frame.
            Control::Generic(g) if g.ctrl_id == *b"gso " && !g.paragraph_lists.is_empty() => {
                let Some(b) = crate::gso::parse_gso_box(&g.data) else {
                    warnings.push(RenderIssueCode::TextBoxGeometryInvalidOmitted, b"gso");
                    rec.unlaid(control_index, g, page);
                    continue;
                };
                let bw = (b.width as f32 / 100.0).max(8.0);
                let bh = b.height as f32 / 100.0;
                let caption =
                    prepare_caption_block(doc, store, page, g.caption.as_ref(), bw, warnings);
                let inline = b.treat_as_char();
                let top_offset = if inline {
                    inline_top_caption_offset(caption.as_ref())
                } else {
                    0.0
                };
                // Inline objects use the flow cursor; floating objects retain
                // their PAPER/PAGE-relative coordinates.
                let (bx, by, inline) = if b.treat_as_char() {
                    (x, object_y + top_offset, true)
                } else {
                    (
                        b.horz_offset as f32 / 100.0,
                        b.vert_offset as f32 / 100.0,
                        false,
                    )
                };

                // Draw the text-box frame/background before its text.
                let frame_origin = if inline {
                    (bx as f64 * 100.0, by as f64 * 100.0)
                } else {
                    (b.horz_offset as f64, b.vert_offset as f64)
                };
                crate::shape_draw::draw_gso_shapes(g, frame_origin, doc, page, warnings);

                // Split linked text boxes at internal v_pos resets. Column zero
                // uses this box; later columns use continuation boxes when known.
                let flat: Vec<&Paragraph> = g
                    .paragraph_lists
                    .iter()
                    .flat_map(|l| l.paragraphs.iter())
                    .collect();
                let columns = split_columns(&flat);
                let cont = if columns.len() > 1 && !inline {
                    continuation_columns(para, &b)
                } else {
                    Vec::new()
                };

                let mut max_bottom = by;
                let mut box_list_state = crate::list::ListState::default();
                for (k, range) in columns.iter().enumerate() {
                    let (cx, cy) = if k == 0 {
                        (bx, by)
                    } else if let Some(&o) = cont.get(k - 1) {
                        o
                    } else {
                        (bx + k as f32 * (bw + COL_GAP_PT), by)
                    };
                    let inner = layout_box_para_iter(
                        doc,
                        store,
                        page,
                        flat[range.clone()]
                            .iter()
                            .map(|para| (*para, BoxParaSelection::All)),
                        cx,
                        cy,
                        bw,
                        warnings,
                        Some(&mut box_list_state),
                        None,
                        0,
                        None,
                        // The envelope numbers a drawing object's text `[para, control, n]`
                        // across all of its lists, so a column continues where the one before
                        // it stopped.
                        Some(&mut *rec),
                        Some((control_index, range.start)),
                        false,
                    );
                    max_bottom = max_bottom.max(inner);
                }

                let used = (max_bottom - by).max(bh);
                let mut flow_end = by + used;
                if let Some(caption) = caption {
                    flow_end = flow_end.max(place_caption_block(page, caption, (bx, by, bw, used)));
                }
                if inline {
                    bottom = bottom.max(flow_end);
                    object_y = flow_end;
                }
            }
            // Text-free HWP5 GSO shape.
            Control::Generic(g)
                if g.ctrl_id == *b"gso "
                    && g.paragraph_lists.is_empty()
                    && crate::shape_draw::has_shape(&g.raw_children) =>
            {
                let Some(b) = crate::gso::parse_gso_box(&g.data) else {
                    continue;
                };
                let bw = (b.width as f32 / 100.0).max(8.0);
                let bh = (b.height as f32 / 100.0).max(0.0);
                let caption =
                    prepare_caption_block(doc, store, page, g.caption.as_ref(), bw, warnings);
                let inline = b.treat_as_char();
                let top_offset = if inline {
                    inline_top_caption_offset(caption.as_ref())
                } else {
                    0.0
                };
                let (bx, by) = if inline {
                    (x, object_y + top_offset)
                } else {
                    (b.horz_offset as f32 / 100.0, b.vert_offset as f32 / 100.0)
                };
                let origin = if inline {
                    (bx as f64 * 100.0, by as f64 * 100.0)
                } else {
                    (b.horz_offset as f64, b.vert_offset as f64)
                };
                crate::shape_draw::draw_gso_shapes(g, origin, doc, page, warnings);
                let mut flow_end = by + bh;
                if let Some(caption) = caption {
                    flow_end = flow_end.max(place_caption_block(page, caption, (bx, by, bw, bh)));
                }
                if inline {
                    bottom = bottom.max(flow_end);
                    object_y = flow_end;
                }
            }
            // HWPX container (hp:container): container_box plus child shapes relative to its origin.
            Control::Generic(g) if g.container_box.is_some() => {
                let cb = g.container_box.as_ref().expect("is_some 가드");
                // If the reader could not parse a visible child (pic/ole/chart/equation/unknown element),
                // report typed loss even when drawing the container; details are hashed.
                if cb.skipped_objects > 0 {
                    warnings.push(
                        RenderIssueCode::UnsupportedControlOmitted,
                        format!("container children skipped: {}", cb.skipped_objects),
                    );
                }
                let bw = (cb.w as f32 / 100.0).max(8.0);
                let bh = (cb.h as f32 / 100.0).max(0.0);
                let caption =
                    prepare_caption_block(doc, store, page, g.caption.as_ref(), bw, warnings);
                let top_offset = if cb.anchored {
                    inline_top_caption_offset(caption.as_ref())
                } else {
                    0.0
                };
                // Container origin: text-flow position when anchored, otherwise PAPER-relative offsets.
                let origin = if cb.anchored {
                    ((x * 100.0) as i32, ((object_y + top_offset) * 100.0) as i32)
                } else {
                    (cb.x, cb.y)
                };
                // Expand child shapes into absolute page coordinates on a copy, preserving IR immutability.
                let adjusted: Vec<hwp_model::ShapeGeom> = g
                    .gso_shapes
                    .iter()
                    .map(|s| {
                        let mut s2 = s.clone();
                        s2.x += origin.0;
                        s2.y += origin.1;
                        s2.anchored = false;
                        s2
                    })
                    .collect();
                crate::shape_draw::draw_ir_shapes(&adjusted, page, warnings);
                let (bx, by) = (origin.0 as f32 / 100.0, origin.1 as f32 / 100.0);
                let mut flow_end = by + bh;
                // Lay out container text (single column in v1). With text_boxes, each list uses
                // its owning child-shape box; otherwise lay it out flat in the container box.
                if !g.paragraph_lists.is_empty() {
                    if cb.text_boxes.is_empty() {
                        let flat = g.paragraph_lists.iter().flat_map(|l| l.paragraphs.iter());
                        let mut box_list_state = crate::list::ListState::default();
                        let inner = layout_box_para_iter(
                            doc,
                            store,
                            page,
                            flat.map(|para| (para, BoxParaSelection::All)),
                            bx,
                            by,
                            bw,
                            warnings,
                            Some(&mut box_list_state),
                            None,
                            0,
                            None,
                            Some(&mut *rec),
                            Some((control_index, 0)),
                            false,
                        );
                        flow_end = flow_end.max(inner);
                    } else {
                        // Laid out per list, but numbered across all of them, as for the
                        // text box above.
                        let mut first = 0;
                        for (i, list) in g.paragraph_lists.iter().enumerate() {
                            // The owning box is relative to the container origin; translate it to page coordinates.
                            // None means the container itself owns the text, so use the container box.
                            let (tx, ty, tw) = match cb.text_boxes.get(i).copied().flatten() {
                                Some([sx, sy, sw, _]) => (
                                    (origin.0 + sx) as f32 / 100.0,
                                    (origin.1 + sy) as f32 / 100.0,
                                    (sw as f32 / 100.0).max(8.0),
                                ),
                                None => (bx, by, bw),
                            };
                            let mut box_list_state = crate::list::ListState::default();
                            let inner = layout_box_para_iter(
                                doc,
                                store,
                                page,
                                list.paragraphs
                                    .iter()
                                    .map(|para| (para, BoxParaSelection::All)),
                                tx,
                                ty,
                                tw,
                                warnings,
                                Some(&mut box_list_state),
                                None,
                                0,
                                None,
                                Some(&mut *rec),
                                Some((control_index, first)),
                                false,
                            );
                            first += list.paragraphs.len();
                            flow_end = flow_end.max(inner);
                        }
                    }
                }
                if let Some(caption) = caption {
                    flow_end = flow_end.max(place_caption_block(
                        page,
                        caption,
                        (bx, by, bw, flow_end - by),
                    ));
                }
                if cb.anchored {
                    bottom = bottom.max(flow_end);
                    object_y = flow_end;
                }
            }
            // Structured HWPX shape, optionally with text and a caption.
            Control::Generic(g) if !g.gso_shapes.is_empty() => {
                let source = &g.gso_shapes[0];
                let bw = (source.w as f32 / 100.0).max(8.0);
                let bh = (source.h as f32 / 100.0).max(0.0);
                let caption =
                    prepare_caption_block(doc, store, page, g.caption.as_ref(), bw, warnings);
                let top_offset = if source.anchored {
                    inline_top_caption_offset(caption.as_ref())
                } else {
                    0.0
                };
                // Move anchored shapes to the flow position without mutating IR. A
                // non-anchored shape keeps its own stored x/y, but that value is only
                // page-absolute at the top level; reached recursively (nested inside a
                // parent shape's own paragraph_lists) it is relative to the parent's own
                // origin and must have that origin added (issue #255 / GG-26).
                let adjusted: Vec<hwp_model::ShapeGeom> = g
                    .gso_shapes
                    .iter()
                    .map(|s| {
                        let mut s2 = s.clone();
                        if s.anchored {
                            s2.x = (x * 100.0) as i32;
                            s2.y = ((object_y + top_offset) * 100.0) as i32;
                        } else {
                            s2.x += (nested_origin.0 * 100.0) as i32;
                            s2.y += (nested_origin.1 * 100.0) as i32;
                        }
                        s2
                    })
                    .collect();
                crate::shape_draw::draw_ir_shapes(&adjusted, page, warnings);
                let s0 = &adjusted[0];
                let (bx, by) = (s0.x as f32 / 100.0, s0.y as f32 / 100.0);
                let mut flow_end = by + bh;
                // Lay text inside the first shape's box (single-column v1).
                if !g.paragraph_lists.is_empty() {
                    let flat = g.paragraph_lists.iter().flat_map(|l| l.paragraphs.iter());
                    let mut box_list_state = crate::list::ListState::default();
                    let inner = layout_box_para_iter(
                        doc,
                        store,
                        page,
                        flat.map(|para| (para, BoxParaSelection::All)),
                        bx,
                        by,
                        bw,
                        warnings,
                        Some(&mut box_list_state),
                        None,
                        0,
                        None,
                        Some(&mut *rec),
                        Some((control_index, 0)),
                        false,
                    );
                    flow_end = flow_end.max(inner);
                }
                if let Some(caption) = caption {
                    flow_end = flow_end.max(place_caption_block(
                        page,
                        caption,
                        (bx, by, bw, flow_end - by),
                    ));
                }
                if s0.anchored {
                    bottom = bottom.max(flow_end);
                    object_y = flow_end;
                }
            }
            // Preserve a caption even when the underlying GSO subtype (for
            // example OLE) has no drawable shape implementation yet.
            Control::Generic(g) if g.ctrl_id == *b"gso " && g.caption.is_some() => {
                let Some(b) = crate::gso::parse_gso_box(&g.data) else {
                    continue;
                };
                let bw = (b.width as f32 / 100.0).max(8.0);
                let bh = (b.height as f32 / 100.0).max(0.0);
                let caption =
                    prepare_caption_block(doc, store, page, g.caption.as_ref(), bw, warnings)
                        .expect("guarded by caption.is_some()");
                let inline = b.treat_as_char();
                let top_offset = if inline {
                    inline_top_caption_offset(Some(&caption))
                } else {
                    0.0
                };
                let (bx, by) = if inline {
                    (x, object_y + top_offset)
                } else {
                    (b.horz_offset as f32 / 100.0, b.vert_offset as f32 / 100.0)
                };
                let flow_end = place_caption_block(page, caption, (bx, by, bw, bh));
                if inline {
                    bottom = bottom.max(flow_end);
                    object_y = flow_end;
                }
            }
            // Equations (hp:equation): lay out the script as mathematics (equation.rs).
            Control::Generic(g) if g.equation.is_some() => {
                let eq = g.equation.as_ref().expect("is_some");
                let h = (eq.height as f32 / 100.0).max(12.0);
                let (bx, by, inline) = if eq.inline {
                    (x, object_y, true)
                } else {
                    (eq.x as f32 / 100.0, eq.y as f32 / 100.0, false)
                };
                // Two-pass font size: multiline equations can be tall. Measure at 12pt first,
                // then scale to the equation box height (eq.height), removing the single-line assumption.
                let probe = crate::equation::typeset(store, doc, &eq.script, 12.0);
                let ph = (probe.ascent + probe.descent).max(1.0);
                let size = (12.0 * h / ph).clamp(6.0, 18.0);
                let ebox = crate::equation::typeset(store, doc, &eq.script, size);
                // Top alignment: place the equation's top (baseline - ascent) at the box top (by).
                let baseline_y = by + ebox.ascent;
                crate::equation::render_into(page, ebox, bx + 2.0, baseline_y, warnings);
                if inline {
                    object_y += h;
                    bottom = bottom.max(by + h);
                }
            }
            // A control with text that no arm above draws still has envelope segments.
            Control::Generic(g) if !g.paragraph_lists.is_empty() => {
                rec.unlaid(control_index, g, page);
            }
            _ => {}
        }
    }
    (bottom, page_split)
}

/// Cell margins in pt (left/right/top/bottom): cell override, table default, then Hangul
/// default. The resolution rule is `hwp_model::Table::cell_margins`, shared with
/// `insert_blocks`, so the two cannot disagree about how wide a cell's content is.
fn cell_margins(table: &Table, cell: &hwp_model::Cell) -> (f32, f32, f32, f32) {
    let m = table.cell_margins(cell);
    (
        m[0] as f32 / 100.0,
        m[1] as f32 / 100.0,
        m[2] as f32 / 100.0,
        m[3] as f32 / 100.0,
    )
}

/// Height, in points, that the line-layout cache of `cell` proves the cell's
/// content has: the text-bearing cached lines, summed per run. A run ends at a
/// restart — a page-start flag, a `v_pos` going backwards, or a paragraph whose
/// first line does not advance past the previous line (a cumulative cache
/// always advances between paragraphs; equal `v_pos` inside a paragraph is one
/// line split into several segments) — which is Hancom continuing the cell on a
/// later page (#233); each run's extent is its last line bottom minus its first
/// line top. A trailing empty cache record is legal and adds no height.
///
/// The stored row height is only a lower bound on this. Hancom's HWPX export
/// keeps the declared minimum height while the cache runs past it, and an edit
/// that re-synthesizes a cell keeps the pre-edit height under a taller cache
/// (#245); in both cases the row was taller than stored.
fn cell_cache_total(cell: &hwp_model::Cell) -> f32 {
    let mut previous_v: Option<i32> = None;
    let mut run_start = 0i32;
    let mut run_end = 0i32;
    let mut total = 0.0f32;
    for para in &cell.paragraphs {
        let wchar_len = para.wchar_len();
        let mut para_start = true;
        for (index, seg) in para.line_segs.iter().enumerate() {
            let line_end = para
                .line_segs
                .get(index + 1)
                .map_or(wchar_len, |next| next.text_start);
            if line_end <= seg.text_start {
                continue;
            }
            let restarts = previous_v.is_some_and(|previous| {
                seg.v_pos < previous || (para_start && seg.v_pos == previous)
            });
            para_start = false;
            if seg.flags & 0x1 != 0 || restarts {
                total += (run_end - run_start).max(0) as f32 / 100.0;
                run_start = seg.v_pos;
                run_end = seg.v_pos;
            }
            previous_v = Some(seg.v_pos);
            run_end = run_end.max(
                seg.v_pos
                    .saturating_add(seg.line_height.max(seg.text_height)),
            );
        }
    }
    total + (run_end - run_start).max(0) as f32 / 100.0
}

#[allow(clippy::too_many_arguments)]
fn layout_table(
    doc: &Document,
    store: &mut FontStore,
    page: &mut PageList,
    table: &Table,
    x: f32,
    y: f32,
    avail_width: f32,
    mut top_caption: Option<CaptionPrelude>,
    mut split: Option<&mut TableSplitCtx<'_, '_>>,
    warnings: &mut RenderIssueAccumulator,
    rec: &mut SegmentRecorder,
) -> (f32, bool, f32) {
    let cols = table.cols.max(1) as usize;
    let rows = table.rows.max(1) as usize;
    let top_caption_extent = top_caption
        .as_ref()
        .map_or(0.0, |caption| caption.height + caption.gap);

    // Grid geometry: span=1 cells establish column widths and row heights; average unknown values.
    let mut col_w = vec![0.0f32; cols];
    let mut row_h = vec![0.0f32; rows];
    let mut row_covered = vec![false; rows];
    // Rows with a stored height carry Hangul's final calculated height;
    // trust it. A measurement pass fills rows without a stored height.
    let mut row_stored = vec![false; rows];
    for cell in &table.cells {
        let (c, r) = (cell.col as usize, cell.row as usize);
        if cell.col_span == 1 && c < cols {
            col_w[c] = col_w[c].max(cell.width.to_pt() as f32);
        }
        if cell.row_span == 1 && r < rows {
            row_h[r] = row_h[r].max(cell.height.to_pt() as f32);
            row_stored[r] = true;
        }
        if c < cols && r < rows {
            let span_end = (r + (cell.row_span as usize).max(1)).min(rows);
            row_covered[r..span_end].fill(true);
        }
    }
    derive_col_widths(&mut col_w, table, avail_width);
    fill_unknown(&mut row_h, 18.0);
    if row_covered.iter().any(|covered| !covered) {
        warnings.push_once(
            RenderIssueCode::InvalidTableCellOmitted,
            b"uncovered-table-row",
        );
        for (height, covered) in row_h.iter_mut().zip(&row_covered) {
            if !covered {
                *height = 0.0;
            }
        }
    }

    // Measurement pass: grow rows to measured content height. Stored cell.height reflects Hangul's
    // line wrapping; extra shaped/synthesized lines could otherwise overlap the next row.
    // Measure on a scratch page and take the maximum. Reuse measured content heights
    // for vertical alignment rather than measuring them again.
    let mut spanned: Vec<(usize, usize, f32)> = Vec::new(); // (start row, span, required height).
    let mut content_h_by_cell: Vec<f32> = Vec::with_capacity(table.cells.len());
    for cell in &table.cells {
        let (c, r) = (cell.col as usize, cell.row as usize);
        if c >= cols || r >= rows {
            content_h_by_cell.push(0.0);
            continue;
        }
        let cw: f32 = col_w[c..(c + cell.col_span as usize).min(cols)]
            .iter()
            .sum();
        let (ml, mr, mt, mb) = cell_margins(table, cell);
        let content_w = (cw - ml - mr).max(4.0);
        // The measure pass is a pure function of the cell, its width and the page
        // size, so a repeat returns the memoized height (#321). Without this, a
        // nested table re-measured its cells inside the parent's draw pass as well,
        // and layout time doubled per nesting level.
        let measured_key = (
            std::ptr::from_ref(cell) as usize,
            content_w.to_bits(),
            page.width_pt.to_bits(),
            page.height_pt.to_bits(),
        );
        // Empty cells skip scratch layout and shaping: content height is zero; add mt+mb below.
        let content_h = if cell.paragraphs.is_empty() {
            0.0
        } else if let Some(&memoized) = store.measured_cells.get(&measured_key) {
            memoized
        } else {
            let mut scratch = PageList {
                width_pt: page.width_pt,
                height_pt: page.height_pt,
                items: Vec::new(),
            };
            let mut scratch_warn = RenderIssueAccumulator::new();
            let measured = layout_box_para_iter(
                doc,
                store,
                &mut scratch,
                cell.paragraphs
                    .iter()
                    .map(|para| (para, BoxParaSelection::All)),
                0.0,
                0.0,
                content_w,
                &mut scratch_warn,
                None,
                None,
                0,
                None,
                None,
                None,
                true,
            );
            store.measured_cells.insert(measured_key, measured);
            measured
        };
        content_h_by_cell.push(content_h);
        let needed = content_h + mt + mb;
        let span = (cell.row_span as usize).max(1);
        let end = (r + span).min(rows);
        // Cached cells' stored row heights are Hangul's final geometry. Do not grow them for larger
        // measurements: that shifts all following rows away from the genuine grid.
        // In one measured case an object enlarged a cell by 190pt and moved an entire next-page fragment.
        // Report typed overflow instead so the cause remains visible. Uncached cells, such as
        // authored documents, have no layout-derived stored height; their measurement
        // is the sole evidence, so continue growing those rows.
        //
        // When the cache itself exceeds the stored height, however, that height is only a lower bound.
        // Cache coordinates are the lines Hangul drew: lines ending outside the stored row prove
        // a taller row. Edited cells grow monotonic caches over their pre-edit heights (#245),
        // producing this mismatch; Hangul's HWPX export similarly records declared minima.
        // Fragments separated by v_pos decreases, page-start flags or nonadvancing paragraph starts
        // continue on later pages (#233), so sum their heights. Use the greater of stored height
        // and total cached height as the row floor. Measurements beyond that floor (objects or
        // our wrapping) are reported without growing the row, as before. Continuation fragments
        // follow prior lines in layout_box_para_iter and fit within the row. If our page breaks
        // differ from Hangul's, move the whole row to the next page without losing content.
        let cell_cached = cell
            .paragraphs
            .iter()
            .any(|para| !para.line_segs.is_empty());
        if cell_cached && (r..end).all(|i| row_stored.get(i).copied().unwrap_or(false)) {
            let stored: f32 = row_h[r..end].iter().sum();
            let floor = stored.max(cell_cache_total(cell) + mt + mb);
            if needed > floor + CELL_CONTENT_OVERFLOW_EPSILON_PT {
                warnings.push_once(
                    RenderIssueCode::TableCellContentOverflow,
                    format!("{:.0}", needed - floor),
                );
            }
            if floor <= stored + CELL_CONTENT_OVERFLOW_EPSILON_PT {
                continue;
            }
            if span == 1 {
                row_h[r] = row_h[r].max(floor);
            } else {
                spanned.push((r, span, floor));
            }
            continue;
        }
        if span == 1 {
            row_h[r] = row_h[r].max(needed);
        } else {
            spanned.push((r, span, needed));
        }
    }
    // For row_span>1, add any deficit in the spanned rows' total height to the final row.
    for (r, span, needed) in spanned {
        let end = (r + span).min(rows);
        let cur: f32 = row_h[r..end].iter().sum();
        if end > r && needed > cur {
            row_h[end - 1] += needed - cur;
        }
    }

    // Prefixes are table-relative so every page fragment can add its own y.
    let col_x: Vec<f32> = prefix_sums(&col_w, x);
    let row_prefix: Vec<f32> = prefix_sums(&row_h, 0.0);
    let total_h: f32 = row_h.iter().sum();

    // Report out-of-grid cells once rather than once per fragment.
    for cell in &table.cells {
        if cell.col as usize >= cols || cell.row as usize >= rows {
            warnings.push(
                RenderIssueCode::InvalidTableCellOmitted,
                format!("{}:{}", cell.row, cell.col),
            );
        }
    }

    // Count the leading all-header block and include rows covered by a header
    // cell's row span. A mixed row inside that span is unsafe to replay.
    let header_rows = if table.repeat_header() {
        let mut row = 0usize;
        let mut end = 0usize;
        let mut valid = true;
        while row < rows {
            let mut any = false;
            let mut all_header = true;
            for cell in &table.cells {
                if cell.row as usize == row && (cell.col as usize) < cols {
                    any = true;
                    if !cell.is_header() {
                        all_header = false;
                        break;
                    }
                }
            }
            if !any {
                if row < end {
                    row += 1;
                    continue;
                }
                break;
            }
            if !all_header {
                valid = row >= end;
                break;
            }
            for cell in &table.cells {
                if cell.row as usize == row && (cell.col as usize) < cols {
                    end = end.max((row + (cell.row_span as usize).max(1)).min(rows));
                }
            }
            row += 1;
        }
        if !valid || end >= rows { 0 } else { end }
    } else {
        0
    };
    let header_h: f32 = row_h[..header_rows].iter().sum();

    // A boundary is legal only when it does not cut through a row-spanning
    // cell. Keep the complete header block together as well, otherwise a
    // continuation could both replay and render the same header row.
    let mut legal_boundary = vec![true; rows + 1];
    for cell in &table.cells {
        let start = cell.row as usize;
        if start >= rows {
            continue;
        }
        let end = (start + (cell.row_span as usize).max(1)).min(rows);
        for allowed in &mut legal_boundary[start + 1..end] {
            *allowed = false;
        }
    }
    if header_rows > 1 {
        for allowed in &mut legal_boundary[1..header_rows] {
            *allowed = false;
        }
    }

    // CELL policy is the one table mode where a single row may legitimately
    // cross a page.  Use the source's cached line boundaries for that case;
    // the row-based planner below remains the conservative fallback for
    // TABLE/NONE and for CELL documents that do not carry enough cache data.
    if table.page_break_policy() == hwp_model::TablePageBreak::Cell
        && !table
            .placement
            .as_ref()
            .is_some_and(|placement| placement.treat_as_char)
        && split
            .as_deref()
            .is_some_and(|ctx| y + total_h > ctx.body_bottom)
        && let Some(result) = layout_table_cell_fragments(
            doc,
            store,
            page,
            table,
            col_x.as_slice(),
            col_w.as_slice(),
            row_h.as_slice(),
            row_prefix.as_slice(),
            content_h_by_cell.as_slice(),
            x,
            y,
            top_caption_extent,
            header_rows,
            &mut top_caption,
            &mut split,
            warnings,
            rec,
        )
    {
        return result;
    }

    // Fragment tuple: first row, exclusive end row, data top, replay header,
    // and whether a page must be finalized immediately before emission.
    let mut fragments: Vec<(usize, usize, f32, bool, bool)> = vec![(0, rows, y, false, false)];
    if let Some(ctx) = split.as_deref() {
        let body_top = ctx.body_top;
        let body_bottom = ctx.body_bottom;
        let first_body_bottom = (body_bottom
            - page_notes_reservation_height(
                doc,
                store,
                page,
                ctx.page_notes,
                ctx.body_left,
                ctx.body_width,
            ))
        .max(body_top);
        let page_h = body_bottom - body_top;
        let unsplit_h = total_h + top_caption_extent;
        let treat_as_char = table.placement.as_ref().is_some_and(|p| p.treat_as_char);
        let policy = table.page_break_policy();
        if y + total_h > first_body_bottom {
            if treat_as_char || (policy == hwp_model::TablePageBreak::None && unsplit_h <= page_h) {
                // An inline table is one indivisible character (GE-8).
                // NONE also keeps a page-sized table together.
                if y > body_top + top_caption_extent && unsplit_h <= page_h {
                    fragments = vec![(0, rows, body_top + top_caption_extent, false, true)];
                } else if unsplit_h > page_h {
                    // An indivisible table taller than a page must surface its overflow.
                    warnings.push(RenderIssueCode::TableRowTooTallClipped, b"treat-as-char");
                }
            } else {
                // TABLE splits at row boundaries; CELL falls back here only
                // when no row requires an internal split or its cached cell
                // boundaries were reported incomplete above. Oversized NONE
                // tables use the same conservative fallback.
                let boundaries: Vec<usize> = (0..=rows)
                    .filter(|&boundary| legal_boundary[boundary])
                    .collect();
                let bands: Vec<(usize, usize, f32)> = boundaries
                    .windows(2)
                    .map(|pair| {
                        let start = pair[0];
                        let end = pair[1];
                        (start, end, row_h[start..end].iter().sum())
                    })
                    .collect();

                let mut frags = Vec::new();
                let mut fragment_start = 0usize;
                let mut fragment_top = y;
                let mut fragment_bottom = first_body_bottom;
                let mut fragment_h = 0.0f32;
                let mut break_before = false;
                for &(band_start, band_end, band_h) in &bands {
                    let replay_header = header_rows > 0 && fragment_start > 0;
                    let fresh_top = body_top
                        + if replay_header { header_h } else { 0.0 }
                        + if fragment_start == 0 {
                            top_caption_extent
                        } else {
                            0.0
                        };

                    // If no band has been accepted and only a page-bottom
                    // sliver remains, move the same band to a fresh page.
                    if fragment_h == 0.0
                        && fragment_top + band_h > fragment_bottom
                        && fragment_top > fresh_top + 0.5
                    {
                        fragment_top = fresh_top;
                        fragment_bottom = body_bottom;
                        break_before = true;
                    }

                    // Flush at the last legal boundary before overflow. Each
                    // band is indivisible, so row-spanning cells stay intact.
                    if fragment_h > 0.0 && fragment_top + fragment_h + band_h > fragment_bottom {
                        frags.push((
                            fragment_start,
                            band_start,
                            fragment_top,
                            header_rows > 0 && fragment_start > 0,
                            break_before,
                        ));
                        fragment_start = band_start;
                        fragment_top = body_top + if header_rows > 0 { header_h } else { 0.0 };
                        fragment_bottom = body_bottom;
                        fragment_h = 0.0;
                        break_before = true;
                    }

                    let available = fragment_bottom - fragment_top;
                    if fragment_h == 0.0 && band_h > available + 0.5 {
                        warnings.push(
                            RenderIssueCode::TableRowTooTallClipped,
                            format!("rows {band_start}..{band_end}"),
                        );
                    }
                    fragment_h += band_h;
                }
                frags.push((
                    fragment_start,
                    rows,
                    fragment_top,
                    header_rows > 0 && fragment_start > 0,
                    break_before,
                ));
                fragments = frags;
            }
        }
    }

    // Emit each fragment before advancing the page. Planning must not replace
    // the current PageList, otherwise all fragments land on the final page.
    // Repeated headers always replay from the state before the original
    // header, while the main state advances through body rows exactly once.
    let mut cell_ls = crate::list::ListState::default();
    let header_ls_seed = cell_ls.clone();
    let mut end_y = y;
    let mut final_fragment_top = y;
    let mut emitted_fragments = 0usize;
    let mut page_advanced = false;
    for &(rs, re, data_top, with_header, break_before) in &fragments {
        if re <= rs {
            continue;
        }
        if break_before {
            let Some(ctx) = split.as_deref_mut() else {
                break;
            };
            if !ctx.break_page(doc, store, page, warnings, rec) {
                break;
            }
            page_advanced = true;
        }
        if emitted_fragments == 0
            && let Some(prelude) = top_caption.take()
        {
            let caption_y = data_top - prelude.gap - prelude.height;
            for item in prelude.items {
                page.items.push(translate_item(item, x, caption_y));
            }
        }
        if with_header {
            let mut header_ls = header_ls_seed.clone();
            draw_table_rows(
                doc,
                store,
                page,
                table,
                &col_x,
                &col_w,
                &row_h,
                &row_prefix,
                &content_h_by_cell,
                0..header_rows,
                data_top - header_h,
                &mut header_ls,
                warnings,
                rec,
            );
        }
        draw_table_rows(
            doc,
            store,
            page,
            table,
            &col_x,
            &col_w,
            &row_h,
            &row_prefix,
            &content_h_by_cell,
            rs..re,
            data_top - row_prefix[rs],
            &mut cell_ls,
            warnings,
            rec,
        );
        end_y = data_top + (row_prefix[re] - row_prefix[rs]);
        final_fragment_top = data_top - if with_header { header_h } else { 0.0 };
        emitted_fragments += 1;
        if let Some(ctx) = split.as_deref_mut() {
            ctx.mark_flow();
        }
    }
    if page_advanced && emitted_fragments > 1 {
        warnings.push(
            RenderIssueCode::TableSplitAcrossPages,
            format!("{rows} rows"),
        );
    }
    (end_y, page_advanced, final_fragment_top)
}

/// Maximum full vertical extent of cells starting at `row`.  A row-spanning
/// cell is drawn once with its whole span height, so fragment packing must
/// keep that extent on a single page.
fn row_span_extent(table: &Table, row: usize, rows: usize, row_h: &[f32]) -> f32 {
    table
        .cells
        .iter()
        .filter(|cell| cell.row as usize == row)
        .map(|cell| {
            let end = (row + usize::from(cell.row_span.max(1))).min(rows);
            row_h[row..end].iter().sum()
        })
        .fold(row_h[row], f32::max)
}

/// CELL-policy table emitter.  It is intentionally separate from the row
/// planner above: a row fragment has a different border contract and its
/// content must be selected from the cached line ranges rather than replaying
/// the whole cell on every page.
#[allow(clippy::too_many_arguments)]
fn layout_table_cell_fragments(
    doc: &Document,
    store: &mut FontStore,
    page: &mut PageList,
    table: &Table,
    col_x: &[f32],
    col_w: &[f32],
    row_h: &[f32],
    row_prefix: &[f32],
    content_h_by_cell: &[f32],
    x: f32,
    y: f32,
    top_caption_extent: f32,
    header_rows: usize,
    top_caption: &mut Option<CaptionPrelude>,
    split: &mut Option<&mut TableSplitCtx<'_, '_>>,
    warnings: &mut RenderIssueAccumulator,
    rec: &mut SegmentRecorder,
) -> Option<(f32, bool, f32)> {
    let (body_top, body_bottom, first_body_bottom) = {
        let ctx = split.as_deref()?;
        let first_body_bottom = (ctx.body_bottom
            - page_notes_reservation_height(
                doc,
                store,
                page,
                ctx.page_notes,
                ctx.body_left,
                ctx.body_width,
            ))
        .max(ctx.body_top);
        (ctx.body_top, ctx.body_bottom, first_body_bottom)
    };
    let header_height: f32 = row_h[..header_rows].iter().sum();

    let rows = row_h.len();
    let mut cell_plans: Vec<Option<CellSplitPlan>> = vec![None; table.cells.len()];
    let mut row_group_counts = vec![1usize; rows];
    let mut found_cached_split = false;
    // Page index each row starts on in the planned fragment sequence.  A row
    // span is unsafe only when it crosses one of these planned transitions.
    let mut row_start_page = vec![0usize; rows];
    let mut planned_page = 0usize;
    let mut planned_cursor = y;
    let mut planned_bottom = first_body_bottom;
    let mut planned_emitted_parts = 0usize;

    for row in 0..rows {
        let mut row_cells = Vec::new();
        for (cell_index, cell) in table.cells.iter().enumerate() {
            let start = cell.row as usize;
            let end = (start + usize::from(cell.row_span.max(1))).min(rows);
            if start <= row && row < end {
                row_cells.push((cell_index, cell));
            }
        }
        if row_cells.is_empty() {
            warnings.push(
                RenderIssueCode::TableCellFragmentationIncomplete,
                format!("row={row}:no-cells"),
            );
            return None;
        }

        let row_crosses_page = planned_cursor + row_h[row] > planned_bottom + 0.5;
        if !row_crosses_page {
            // A row-spanning cell starting here is drawn once with its full
            // span height, so the row may stay on this page only when the
            // whole span extent fits the remaining capacity.
            let span_extent = row_span_extent(table, row, rows, row_h);
            if planned_cursor + span_extent <= planned_bottom + 0.5 {
                row_start_page[row] = planned_page;
                planned_cursor += row_h[row];
                planned_emitted_parts += 1;
                continue;
            }
            let continuation_top = body_top
                + if planned_emitted_parts == 0 {
                    top_caption_extent
                } else if header_rows > 0 && row >= header_rows {
                    header_height
                } else {
                    0.0
                };
            // The emitter cannot break before the very first fragment when
            // the table still sits at the page top, so that case cannot move.
            let can_move =
                planned_emitted_parts > 0 || planned_cursor > body_top + top_caption_extent + 0.5;
            if !can_move || span_extent > body_bottom - continuation_top + 0.5 {
                // The span fits nowhere this row can be placed: keep the
                // fail-closed contract instead of clipping or splitting it.
                warnings.push(
                    RenderIssueCode::TableCellFragmentationIncomplete,
                    format!("row={row}:row-span-height"),
                );
                return None;
            }
            planned_page += 1;
            row_start_page[row] = planned_page;
            planned_cursor = continuation_top + row_h[row];
            planned_bottom = body_bottom;
            planned_emitted_parts += 1;
            continue;
        }

        let remaining_height = (planned_bottom - planned_cursor).max(0.0);
        let continuation_top = body_top
            + if planned_emitted_parts == 0 {
                top_caption_extent
            } else if header_rows > 0 && row >= header_rows {
                header_height
            } else {
                0.0
            };
        let continuation_height = (body_bottom - continuation_top).max(1.0);
        // The blank-remainder contract is the declared row height, decided
        // once per row after all cell plans are known.
        let declared_h: f32 = table
            .cells
            .iter()
            .filter(|cell| cell.row as usize == row && cell.row_span.max(1) == 1)
            .map(|cell| cell.height.to_pt() as f32)
            .fold(0.0, f32::max);
        let mut row_groups = 1usize;
        let mut cells_requiring_boundary = Vec::new();
        let mut row_plan_candidates: Vec<(usize, CellSplitPlan, f32)> = Vec::new();
        for &(cell_index, cell) in &row_cells {
            if cell.row as usize != row || cell.row_span.max(1) != 1 {
                continue;
            }
            let measured = content_h_by_cell.get(cell_index).copied().unwrap_or(0.0);
            let has_cell_payload = cell
                .paragraphs
                .iter()
                .any(|para| !para.chars.is_empty() || !para.controls.is_empty());
            if !has_cell_payload {
                continue;
            }
            let (_, _, mt, mb) = cell_margins(table, cell);
            let needs_boundary = measured + mt + mb > remaining_height + 0.5
                || measured + mt + mb > continuation_height + 0.5;
            if needs_boundary {
                cells_requiring_boundary.push(cell_index);
            }
            let Some(mut plan) = cached_cell_split_plan(
                cell,
                measured,
                remaining_height - mt - mb,
                continuation_height - mt - mb,
                mt + mb,
            ) else {
                continue;
            };
            for group in &mut plan.groups {
                group.height += mt + mb;
            }
            let cached_height: f32 = plan.groups.iter().map(|group| group.height).sum();
            row_plan_candidates.push((cell_index, plan, cached_height));
        }

        // Pad the declared row-height remainder into at most one plan: the
        // row-height owner with the largest cached fragment sum (first maximum
        // wins ties).  Cached content taller than declared is trusted as-is,
        // and our own re-measured cell height must not manufacture blank
        // continuation area.  Other cells keep their cached content groups and
        // emit Empty selections on continuation fragments, so they never
        // manufacture independent blank pages.
        let mut owner = None;
        for (index, (_, _, cached_height)) in row_plan_candidates.iter().enumerate() {
            if owner.is_none_or(|o: usize| *cached_height > row_plan_candidates[o].2) {
                owner = Some(index);
            }
        }
        if let Some(o) = owner {
            let (cell_index, plan, cached_height) = &mut row_plan_candidates[o];
            let target_h = if declared_h > 0.0 {
                declared_h
            } else {
                row_h[row]
            };
            if target_h > *cached_height {
                // Preserve the declared row height as blank continuation
                // area.  Grow each cached fragment only up to its page
                // capacity; the overflow becomes trailing blank-only
                // fragments that draw background and borders but replay no
                // text or nested objects.
                let mut remainder = target_h - *cached_height;
                for (index, group) in plan.groups.iter_mut().enumerate() {
                    if remainder <= 0.0 {
                        break;
                    }
                    let capacity = if index == 0 {
                        remaining_height
                    } else {
                        continuation_height
                    };
                    let grow = remainder.min((capacity - group.height).max(0.0));
                    group.height += grow;
                    remainder -= grow;
                }
                let para_count = table.cells[*cell_index].paragraphs.len();
                let last_v_pos = plan.groups.last().map_or(0, |group| group.first_v_pos);
                while remainder > 0.0 {
                    let height = remainder.min(continuation_height);
                    plan.groups.push(CellFragmentGroup {
                        selections: vec![BoxParaSelection::Empty; para_count],
                        first_v_pos: last_v_pos,
                        height,
                        has_content: false,
                    });
                    remainder -= height;
                }
            }
        }
        for (cell_index, plan, _) in row_plan_candidates {
            if !plan.is_usable() {
                continue;
            }
            row_groups = row_groups.max(plan.groups.len());
            cell_plans[cell_index] = Some(plan);
        }

        if row_groups > 1 {
            // A cell that needs a boundary but has no usable multi-fragment
            // plan is drawn whole into fragment 0 (All selections).  That is
            // safe only while its measured content is contained by the
            // planned first fragment; otherwise the required split has no
            // cached geometry and the render is incomplete.  `cell_plans`
            // persists across rows, so the containment height must come only
            // from plans owned by THIS row — an earlier row's taller first
            // fragment must not vouch for a later row.
            let first_part_height = table
                .cells
                .iter()
                .enumerate()
                .filter(|(_, cell)| cell.row as usize == row)
                .filter_map(|(cell_index, _)| {
                    cell_plans[cell_index]
                        .as_ref()
                        .and_then(|plan| plan.groups.first())
                })
                .map(|group| group.height)
                .fold(0.0f32, f32::max)
                .max(1.0);
            let uncontained = cells_requiring_boundary.iter().any(|&cell_index| {
                if cell_plans[cell_index]
                    .as_ref()
                    .is_some_and(|plan| plan.is_usable())
                {
                    return false;
                }
                let cell = &table.cells[cell_index];
                let (_, _, mt, mb) = cell_margins(table, cell);
                let measured = content_h_by_cell.get(cell_index).copied().unwrap_or(0.0);
                measured + mt + mb > first_part_height + 0.5
            });
            if uncontained {
                warnings.push(
                    RenderIssueCode::TableCellFragmentationIncomplete,
                    format!("row={row}:cache"),
                );
                return None;
            }
        }

        if row_groups == 1 {
            // No cached content needs to use the current-page remainder. Let
            // the existing row-boundary planner move this row as a unit.  A
            // span starting here must fit a fresh page as a whole.
            if row_span_extent(table, row, rows, row_h) > continuation_height + 0.5 {
                warnings.push(
                    RenderIssueCode::TableCellFragmentationIncomplete,
                    format!("row={row}:boundary"),
                );
                return None;
            }
            planned_page += 1;
            row_start_page[row] = planned_page;
            planned_cursor = continuation_top + row_h[row];
            planned_bottom = body_bottom;
            planned_emitted_parts += 1;
            continue;
        }

        found_cached_split = true;
        row_group_counts[row] = row_groups;
        row_start_page[row] = planned_page;
        for group_index in 0..row_groups {
            let part_height = table
                .cells
                .iter()
                .enumerate()
                .filter(|(_, cell)| cell.row as usize == row)
                .filter_map(|(cell_index, _)| {
                    cell_plans[cell_index]
                        .as_ref()
                        .and_then(|plan| plan.groups.get(group_index))
                        .map(|group| group.height)
                })
                .fold(0.0f32, f32::max)
                .max(1.0);
            if planned_cursor + part_height > planned_bottom + 0.5 {
                planned_page += 1;
                planned_cursor = body_top
                    + if planned_emitted_parts == 0 {
                        top_caption_extent
                    } else if header_rows > 0 && row >= header_rows {
                        header_height
                    } else {
                        0.0
                    };
                planned_bottom = body_bottom;
            }
            if planned_cursor + part_height > planned_bottom + 0.5 {
                warnings.push(
                    RenderIssueCode::TableCellFragmentationIncomplete,
                    format!("row={row}:fragment={group_index}:height"),
                );
                return None;
            }
            planned_cursor += part_height;
            planned_emitted_parts += 1;
        }
    }
    // A CELL row may fit on a fresh page yet still be split by Hancom to use
    // the space remaining on the current page. Cached line boundaries, not
    // `row_height > full_body_height`, define that Regime-A split.
    if !found_cached_split {
        return None;
    }
    // A row span is unsafe only where it intersects an internally fragmented
    // row or crosses a planned page transition — the fragment emitter draws a
    // spanning cell once, on the page where its first row starts.  Spans that
    // live entirely on one page keep the CELL fragmenting path.
    let unsafe_row_span = table.cells.iter().any(|cell| {
        let start = cell.row as usize;
        let span = usize::from(cell.row_span.max(1));
        if start >= rows || span <= 1 {
            return false;
        }
        let end = (start + span).min(rows);
        row_group_counts[start..end].iter().any(|&count| count > 1)
            || row_start_page[start] != row_start_page[end - 1]
    });
    if unsafe_row_span {
        warnings.push(
            RenderIssueCode::TableCellFragmentationIncomplete,
            b"row-span-with-cell-fragment",
        );
        return None;
    }

    // A local list state advances through body cells exactly once. Replayed
    // headers use a seed so their marker is stable on every continuation.
    let mut cell_ls = crate::list::ListState::default();
    let header_ls_seed = cell_ls.clone();
    let mut cursor = y;
    let mut current_bottom = first_body_bottom;
    let mut emitted_parts = 0usize;
    let mut page_advanced = false;
    let mut final_fragment_top = y;
    for row in 0..rows {
        let group_count = row_group_counts[row];
        // A soft cached reset opens another fragment on the *same* page, so the
        // closing edge must follow the real page crossings, not the group index.
        let mut opened_page = true;
        for group_index in 0..group_count {
            let group_height = |index: usize| {
                table
                    .cells
                    .iter()
                    .enumerate()
                    .filter(|(_, cell)| cell.row as usize == row)
                    .filter_map(|(cell_index, _)| {
                        cell_plans[cell_index]
                            .as_ref()
                            .and_then(|plan| plan.groups.get(index))
                            .map(|group| group.height)
                    })
                    .fold(0.0f32, f32::max)
                    .max(1.0)
            };
            let part_height = if group_count == 1 {
                row_h[row]
            } else {
                group_height(group_index)
            };
            let next_part_height = if group_index + 1 < group_count {
                group_height(group_index + 1)
            } else {
                0.0
            };
            let mut replay_header = false;
            // The page-break decision for an unfragmented row must cover the
            // full extent of any span starting here; the planner already
            // guaranteed that extent fits a fresh page.
            let fit_extent = if group_count == 1 {
                row_span_extent(table, row, rows, row_h)
            } else {
                part_height
            };
            if cursor + fit_extent > current_bottom + 0.5 {
                let ctx = split.as_deref_mut()?;
                if emitted_parts == 0 && cursor <= body_top + top_caption_extent + 0.5 {
                    warnings.push(
                        RenderIssueCode::TableRowTooTallClipped,
                        format!("cell row={row} fragment={group_index}"),
                    );
                } else if !ctx.break_page(doc, store, page, warnings, rec) {
                    return Some((cursor, page_advanced, final_fragment_top));
                } else {
                    page_advanced = true;
                    opened_page = true;
                    cursor = body_top
                        + if emitted_parts == 0 {
                            top_caption_extent
                        } else {
                            0.0
                        };
                    current_bottom = body_bottom;
                    replay_header = header_rows > 0 && row >= header_rows && emitted_parts > 0;
                }
            }
            if replay_header {
                let mut header_ls = header_ls_seed.clone();
                draw_table_rows(
                    doc,
                    store,
                    page,
                    table,
                    col_x,
                    col_w,
                    row_h,
                    row_prefix,
                    content_h_by_cell,
                    0..header_rows,
                    body_top,
                    &mut header_ls,
                    warnings,
                    rec,
                );
                cursor = body_top + row_h[..header_rows].iter().sum::<f32>();
                if cursor + part_height > body_bottom + 0.5 {
                    warnings.push(
                        RenderIssueCode::TableRowTooTallClipped,
                        format!("cell row={row} header"),
                    );
                }
            }
            if emitted_parts == 0
                && let Some(prelude) = top_caption.take()
            {
                let caption_y = cursor - prelude.gap - prelude.height;
                for item in prelude.items {
                    page.items.push(translate_item(item, x, caption_y));
                }
            }

            for (cell_index, cell) in table.cells.iter().enumerate() {
                if cell.row as usize != row {
                    continue;
                }
                let plan = cell_plans[cell_index].as_ref();
                let group = plan.and_then(|split_plan| split_plan.groups.get(group_index));
                let selections = if let Some(group) = group {
                    group.selections.clone()
                } else if group_index == 0 {
                    vec![BoxParaSelection::All; cell.paragraphs.len()]
                } else {
                    vec![BoxParaSelection::Empty; cell.paragraphs.len()]
                };
                let v_origin = group.map_or(0, |fragment| fragment.first_v_pos);
                // A spanning cell is drawn once with its full span height.
                // The planner's row-span safety check guarantees the whole
                // span stays on this page.
                let span_end = (row + usize::from(cell.row_span.max(1))).min(rows);
                let cell_height = if span_end > row + 1 {
                    row_h[row..span_end].iter().sum()
                } else {
                    part_height
                };
                // The next fragment breaks the page when it no longer fits, so
                // this one ends at a page boundary. Mirrors the break test above.
                let closes_page = group_index + 1 == group_count
                    || cursor + part_height + next_part_height > current_bottom + 0.5;
                rec.begin_cell(cell_index, cell, page);
                draw_table_cell_fragment(
                    doc,
                    store,
                    page,
                    table,
                    cell,
                    cell_index,
                    col_x,
                    col_w,
                    cursor,
                    cell_height,
                    content_h_by_cell.get(cell_index).copied().unwrap_or(0.0),
                    selections,
                    v_origin,
                    group_count == 1 || group_index == 0,
                    group_count == 1 || group_index + 1 == group_count,
                    opened_page,
                    closes_page,
                    &mut cell_ls,
                    warnings,
                    rec,
                );
                rec.end_segment(page);
            }
            opened_page = false;
            cursor += part_height;
            final_fragment_top = cursor - part_height;
            emitted_parts += 1;
            if let Some(ctx) = split.as_deref_mut() {
                ctx.mark_flow();
            }
        }
    }

    if page_advanced && emitted_parts > 1 {
        warnings.push(
            RenderIssueCode::TableSplitAcrossPages,
            format!("{} rows (cell fragments)", rows),
        );
    }
    Some((cursor, page_advanced, final_fragment_top))
}

#[allow(clippy::too_many_arguments)]
fn draw_table_cell_fragment(
    doc: &Document,
    store: &mut FontStore,
    page: &mut PageList,
    table: &Table,
    cell: &hwp_model::Cell,
    cell_index: usize,
    col_x: &[f32],
    col_w: &[f32],
    cy: f32,
    ch: f32,
    measured_content_h: f32,
    selections: Vec<BoxParaSelection>,
    v_origin: i32,
    first_fragment: bool,
    last_fragment: bool,
    opened_page: bool,
    closes_page: bool,
    cell_ls: &mut crate::list::ListState,
    warnings: &mut RenderIssueAccumulator,
    rec: &mut SegmentRecorder,
) {
    let c = cell.col as usize;
    if c >= col_w.len() {
        return;
    }
    let cx = col_x[c];
    let cw: f32 = col_w[c..(c + cell.col_span as usize).min(col_w.len())]
        .iter()
        .sum();
    if cw <= 0.0 || ch <= 0.0 {
        return;
    }
    let border_fill = doc
        .header
        .border_fills
        .get((cell.border_fill.0 as usize).saturating_sub(1));
    if let Some(item) = border_fill.and_then(|bf| bg_fill_item(bf, cx, cy, cw, ch))
        && warnings.charge_display_items(1)
    {
        page.items.push(item);
    }

    let (ml, mr, mt, mb) = cell_margins(table, cell);
    let available_h = (ch - mt - mb).max(0.0);
    // Vertical alignment applies to the complete cell, not independently to
    // each continuation fragment. Re-centering every fragment creates a large
    // blank band and can push selected cached lines under the next row.
    let voff = if first_fragment && last_fragment {
        let spare = (available_h - measured_content_h).max(0.0);
        match cell.vert_align() {
            1 => spare * 0.5,
            2 => spare,
            _ => 0.0,
        }
    } else {
        0.0
    };
    if !selections
        .iter()
        .all(|selection| matches!(selection, BoxParaSelection::Empty))
    {
        let _ = layout_box_para_iter(
            doc,
            store,
            page,
            cell.paragraphs.iter().zip(selections),
            cx + ml,
            cy + mt + voff,
            (cw - ml - mr).max(4.0),
            warnings,
            Some(cell_ls),
            None,
            v_origin,
            // The fragment's content bottom bounds `para_shift`: a paragraph
            // the flow floor pushes down may never be drawn outside the cell.
            Some(cy + ch - mb),
            Some(rec),
            None,
            true,
        );
    }

    if let Some(bf) = border_fill {
        // Hangul closes every page's fragment with its own horizontal edge, so a
        // split row reads as a complete box on each page (measured: the oracle
        // draws a full-width rule at both the page-bottom and the continuation
        // top of a split row, where we drew none). Only real page crossings get
        // that edge: a soft cached reset opens another fragment on the same
        // page, and closing it there would rule through the middle of the cell.
        let items = crate::border::border_rectangle_items(
            cx,
            cy,
            cx + cw,
            cy + ch,
            &bf.sides,
            [
                true,
                true,
                first_fragment || opened_page,
                last_fragment || closes_page,
            ],
        );
        if warnings.charge_display_items(items.len()) {
            page.items.extend(items);
        }
        // Diagonals describe the complete cell, so emit them only on the
        // first fragment; continuation fragments carry the four-sided frame.
        if first_fragment {
            let (slash, backslash) = diagonal_dirs(bf.attr);
            if (slash || backslash) && bf.diagonal.is_visible() {
                let mut dirs = Vec::with_capacity(2);
                if backslash {
                    dirs.push((cx, cy, cx + cw, cy + ch));
                }
                if slash {
                    dirs.push((cx, cy + ch, cx + cw, cy));
                }
                for (dx1, dy1, dx2, dy2) in dirs {
                    let items = crate::border::border_line_items(dx1, dy1, dx2, dy2, &bf.diagonal);
                    if warnings.charge_display_items(items.len()) {
                        page.items.extend(items);
                    }
                }
            }
        }
    }
    let _ = cell_index;
}

/// Draw one row-range fragment. `base_y + row_prefix[r]` is the row top.
/// The planner guarantees that the range never cuts through a row span.
#[allow(clippy::too_many_arguments)]
fn draw_table_rows(
    doc: &Document,
    store: &mut FontStore,
    page: &mut PageList,
    table: &Table,
    col_x: &[f32],
    col_w: &[f32],
    row_h: &[f32],
    row_prefix: &[f32],
    content_h_by_cell: &[f32],
    range: std::ops::Range<usize>,
    base_y: f32,
    cell_ls: &mut crate::list::ListState,
    warnings: &mut RenderIssueAccumulator,
    rec: &mut SegmentRecorder,
) {
    let cols = col_w.len();
    let rows = row_h.len();
    for (ci, cell) in table.cells.iter().enumerate() {
        let (c, r) = (cell.col as usize, cell.row as usize);
        if c >= cols || r >= rows || !range.contains(&r) {
            continue; // `layout_table` already reported this out-of-grid cell.
        }
        let cx = col_x[c];
        let cy = base_y + row_prefix[r];
        let cw: f32 = col_w[c..(c + cell.col_span as usize).min(cols)]
            .iter()
            .sum();
        let span_end = (r + (cell.row_span as usize).max(1)).min(rows);
        if span_end > range.end {
            debug_assert!(false, "fragment cut through row span at {r}:{c}");
            warnings.push(RenderIssueCode::InvalidTableCellOmitted, format!("{r}:{c}"));
            continue;
        }
        let ch: f32 = row_h[r..span_end].iter().sum();

        rec.begin_cell(ci, cell, page);

        let border_fill = doc
            .header
            .border_fills
            .get((cell.border_fill.0 as usize).saturating_sub(1));

        // 1) Background.
        if let Some(item) = border_fill.and_then(|bf| bg_fill_item(bf, cx, cy, cw, ch)) {
            if !warnings.charge_display_items(1) {
                rec.end_segment(page);
                return;
            }
            page.items.push(item);
        }

        // 2) Content: cell margins and vertical alignment (list_attr bits5-6: top/center/bottom = 0/1/2).
        // Use the measurement pass's content height to calculate the remaining-space offset.
        let (ml, mr, mt, mb) = cell_margins(table, cell);
        let content_h = content_h_by_cell.get(ci).copied().unwrap_or(0.0);
        let avail = (ch - mt - mb - content_h).max(0.0);
        let voff = match cell.vert_align() {
            1 => avail * 0.5,
            2 => avail,
            _ => 0.0,
        };
        layout_box_para_iter(
            doc,
            store,
            page,
            cell.paragraphs
                .iter()
                .map(|para| (para, BoxParaSelection::All)),
            cx + ml,
            cy + mt + voff,
            (cw - ml - mr).max(4.0),
            warnings,
            Some(cell_ls), // Render pass: draw cell list markers.
            None,
            0,
            // Same bound as the fragment emitter: a floor-pushed paragraph
            // must stay inside the cell instead of running into the row below.
            Some(cy + ch - mb),
            Some(rec),
            None,
            true,
        );

        // 3) Borders (left, right, top, bottom).
        if let Some(bf) = border_fill {
            let items = crate::border::border_rectangle_items(
                cx,
                cy,
                cx + cw,
                cy + ch,
                &bf.sides,
                [true; 4],
            );
            if !warnings.charge_display_items(items.len()) {
                rec.end_segment(page);
                return;
            }
            page.items.extend(items);

            // 4) Diagonals: draw only when slash/backSlash bits are set; merged cells span the full area.
            // The diagonal line provides style/color only; without direction bits, draw nothing.
            let (slash, backslash) = diagonal_dirs(bf.attr);
            if (slash || backslash) && bf.diagonal.is_visible() {
                // Emit backslash before slash and preserve compound/dashed styles (GG-24).
                let mut dirs = Vec::with_capacity(2);
                if backslash {
                    dirs.push((cx, cy, cx + cw, cy + ch));
                }
                if slash {
                    dirs.push((cx, cy + ch, cx + cw, cy));
                }
                for (dx1, dy1, dx2, dy2) in dirs {
                    let items = crate::border::border_line_items(dx1, dy1, dx2, dy2, &bf.diagonal);
                    if !warnings.charge_display_items(items.len()) {
                        rec.end_segment(page);
                        return;
                    }
                    page.items.extend(items);
                }
            }
        }
        rec.end_segment(page);
    }
}

/// BORDER_FILL attribute bits indicating whether to draw slash `/` or backslash `\\` diagonals.
/// slash uses bits2-4, backSlash bits5-7; both zero means no diagonals.
fn diagonal_dirs(attr: u16) -> (bool, bool) {
    let slash = (attr >> 2) & 0x7 != 0;
    let backslash = (attr >> 5) & 0x7 != 0;
    (slash, backslash)
}

/// Lay out paragraphs inside a box/cell; origin is the text area's top-left corner (pt).
/// Cell lineseg v_pos is relative to the cell text-area top, matching the body model.
#[allow(clippy::too_many_arguments)]
fn layout_box_paragraphs(
    doc: &Document,
    store: &mut FontStore,
    page: &mut PageList,
    paras: &[Paragraph],
    origin_x: f32,
    origin_y: f32,
    width: f32,
    warnings: &mut RenderIssueAccumulator,
    list_state: Option<&mut crate::list::ListState>,
    page_number: Option<u32>,
) -> f32 {
    layout_box_para_iter(
        doc,
        store,
        page,
        paras.iter().map(|para| (para, BoxParaSelection::All)),
        origin_x,
        origin_y,
        width,
        warnings,
        list_state,
        page_number,
        0,
        None,
        // This wrapper serves headers, footers, notes and captions.
        // These lists have no segment in the envelope `--segments v2` emits by default
        // (notes and captions never, headers and footers unless asked for), so a row would join
        // to nothing. Cell measurement calls layout_box_para_iter directly with no recorder.
        None,
        None,
        false,
    )
}

/// A cached cell fragment selects a contiguous range of line segments from
/// each paragraph.  `Empty` is deliberately distinct from `All`: a paragraph
/// can own a nested object, but must not be replayed on every continuation
/// page merely because its text was split there.
#[derive(Debug, Clone)]
enum BoxParaSelection {
    All,
    Segments(std::ops::Range<usize>),
    Empty,
}

// Render-only cell caches are not serialized by synthesize_linesegs.
fn compute_cell_render_linesegs(
    store: &mut FontStore,
    doc: &Document,
    para: &Paragraph,
    width: i32,
    v: &mut i32,
    warnings: &mut RenderIssueAccumulator,
) -> Vec<hwp_model::LineSeg> {
    // Keep paragraph margins within the rendered cell box. These temporary
    // widths never change the prior full-width caches written to native files.
    let shape = doc.header.para_shapes.get(para.para_shape.0 as usize);
    let left = shape.map_or(0, |shape| shape.margin_left.max(0) / 2);
    let right = shape.map_or(0, |shape| shape.margin_right.max(0) / 2);
    let width = width.saturating_sub(left).saturating_sub(right).max(1);
    crate::lineseg::compute_box_linesegs(store, doc, para, width, v, warnings)
}

/// Iterator form of `layout_box_paragraphs`; it also accepts a column fragment.
///
/// A cached lineseg `v_pos` is honoured exactly as Hancom stored it and is never
/// pulled down to the flow cursor: pulling it down accumulates per-line drift in
/// a tall text box until the content runs off the page. `flow_floor` is a lower
/// bound only, raised (never lowered) so that a following cached paragraph cannot
/// land on top of content already placed.
///
/// The floor advances in two ways. Flow-placed content raises it as it is
/// emitted: a fallback paragraph without a cache, a nested table or image, and a
/// line whose re-wrap pushed it below the position its cache claims. On top of
/// that it advances once at the end of every paragraph, to that paragraph's
/// content bottom. That per-paragraph raise is what keeps a paragraph whose
/// cached `v_pos` restarts at zero from landing on the previous paragraph
/// (#222); for the cell-cumulative caches genuine Hancom files carry it is a
/// no-op, because the next paragraph already starts below that bottom.
///
/// A paragraph the floor does push down moves as a whole, by the single offset
/// its first line needs (`para_shift`), so its own line spacing survives.
/// Clamping each of its lines to the floor separately would fold them onto one
/// baseline, which is the overlap this guard exists to remove.
///
/// `content_limit` is the bottom of the container the caller owns (a table
/// cell or cell fragment; `None` for a text box, a header/footer, and the
/// measurement pass, which have no such edge). It bounds the shift only: a
/// line the shift would push past that edge is clipped and reported as
/// `TableCellContentOverflow` instead of being painted over the row below or
/// over the footer. A cached line that already overflows where Hancom stored
/// it is left alone, because a stored row height is never grown here and the
/// measurement pass already reports that overflow.
#[allow(clippy::too_many_arguments)]
fn layout_box_para_iter<'a>(
    doc: &Document,
    store: &mut FontStore,
    page: &mut PageList,
    paras: impl Iterator<Item = (&'a Paragraph, BoxParaSelection)> + Clone,
    origin_x: f32,
    origin_y: f32,
    width: f32,
    warnings: &mut RenderIssueAccumulator,
    mut list_state: Option<&mut crate::list::ListState>,
    page_number: Option<u32>,
    v_origin: i32,
    content_limit: Option<f32>,
    // `Some` for a table cell's paragraphs and a drawing object's text, whose paths the recorder
    // derives from the open cell or paragraph. Every other box passes `None`, each call site
    // saying why.
    mut rec: Option<&mut SegmentRecorder>,
    // For a drawing object's text: the object's control index in the enclosing paragraph and
    // the position, across all of its lists, of the first paragraph laid out here. `None`
    // records the paragraphs as a cell's.
    drawing: Option<(usize, usize)>,
    cacheless_cells: bool,
) -> f32 {
    let mut content_bottom = origin_y;
    // Only flow-placed content raises this floor; saved cache positions stay authoritative.
    let mut flow_floor = origin_y;
    let mut uncached_next_v = v_origin;
    // A mixed list still uses its saved cache and existing flow-floor contract.
    // Only wholly cache-less lists receive ephemeral synthesized geometry.
    let synthesize_uncached =
        cacheless_cells && paras.clone().all(|(para, _)| para.line_segs.is_empty());
    // Enumerated before the `Empty` skip: the index is the paragraph's own position in the
    // cell, which is what the envelope's path component for it is.
    for (para_index, (para, selection)) in paras.enumerate() {
        if matches!(selection, BoxParaSelection::Empty) {
            continue;
        }
        if let Some(rec) = rec.as_deref_mut() {
            match drawing {
                Some((control_index, first)) => {
                    rec.begin_drawing_paragraph(control_index, first + para_index, para, page)
                }
                None => rec.begin_cell_paragraph(para_index, para, page),
            }
        }
        let synthesized;
        let was_uncached = para.line_segs.is_empty();
        let line_segs = if synthesize_uncached && !para.chars.is_empty() {
            let mut v = (((content_bottom - origin_y) * 100.0).round() as i32 + v_origin)
                .max(uncached_next_v);
            synthesized = compute_cell_render_linesegs(
                store,
                doc,
                para,
                (width * 100.0).round() as i32,
                &mut v,
                warnings,
            );
            synthesized.as_slice()
        } else {
            para.line_segs.as_slice()
        };
        let mut para_top: Option<f32> = None;
        let tabs = crate::tab::tab_stops(doc, para);
        // Do not advance a list counter for a completely empty box paragraph.
        let marker = if line_segs.is_empty() && para.chars.is_empty() {
            None
        } else {
            list_state
                .as_deref_mut()
                .and_then(|ls| ls.marker_for_render(doc, para))
        };

        if line_segs.is_empty() {
            if para.chars.is_empty() {
                content_bottom += 12.0;
            } else {
                let end = para.wchar_len();
                let items = shape_range_page(
                    store,
                    doc,
                    para,
                    (0, end),
                    &std::collections::HashMap::new(),
                    page_number,
                    warnings,
                );
                let max_size = items_max_size(&items).unwrap_or(10.0);
                let geom = para_geometry(doc, para);
                let left = origin_x + geom.left;
                let avail = (width - geom.left - geom.right).max(4.0);
                let baseline_y = content_bottom + geom.spacing_top + max_size * 1.2;
                para_top = Some(content_bottom + geom.spacing_top);
                let mut marker_advance = 0.0;
                if let Some(m) = &marker {
                    marker_advance = render_list_marker(
                        page,
                        store,
                        doc,
                        m,
                        (left, baseline_y, max_size),
                        warnings,
                    );
                    if let Some(rec) = rec.as_deref_mut() {
                        rec.marker_drawn(page);
                    }
                }
                // First-line/hanging indent follows the body fallback rules.
                // Do not reduce wrap width; cap extreme positive values to protect narrow cells.
                let (first_indent, rest_indent) = line_indents(&geom, marker_advance);
                let first_indent = first_indent.min(avail * 0.8);
                let rest_indent = rest_indent.min(avail * 0.8);
                let natural = items_width(&items, &tabs);
                let align = doc
                    .header
                    .para_shapes
                    .get(para.para_shape.0 as usize)
                    .map_or(1, |p| p.alignment());
                let (x0, first_delta) = if natural <= avail && (align == 2 || align == 3) {
                    (
                        left + (avail - natural) * if align == 3 { 0.5 } else { 1.0 },
                        0.0,
                    )
                } else {
                    (left + rest_indent, first_indent - rest_indent)
                };
                // Reduce width by the indent to preserve the cell's original right edge (left + avail).
                let wrap_width = (left + avail - x0).max(1.0);
                let last_y = place_wrapped(
                    page,
                    items,
                    x0,
                    baseline_y,
                    wrap_width,
                    max_size * 1.6,
                    &tabs,
                    first_delta,
                    &doc.header.border_fills,
                    None,
                    warnings,
                );
                content_bottom = last_y + max_size * 0.4 + geom.spacing_bottom;
            }
            // Uncached paragraphs use flow layout; raise the floor so later cached paragraphs cannot overlap.
            flow_floor = flow_floor.max(content_bottom);
        } else {
            let last_content = last_content_seg_in(para, line_segs);
            let selected = match &selection {
                BoxParaSelection::All => 0..line_segs.len(),
                BoxParaSelection::Segments(range) => {
                    range.start.min(line_segs.len())..range.end.min(line_segs.len())
                }
                BoxParaSelection::Empty => unreachable!("empty selections are skipped above"),
            };
            // A paragraph whose cached position starts above the flow floor is
            // moved down as a whole, by the one offset its first line needs.
            // Clamping each line to the floor on its own would fold every line
            // of the paragraph onto the floor and re-create the overlap this
            // guard exists to remove (#222). Zero for a well-formed cache.
            let para_shift = line_segs.get(selected.start).map_or(0.0, |first| {
                let v_pos = first.v_pos.saturating_sub(v_origin);
                let stored = origin_y + (v_pos + first.baseline_gap) as f32 / 100.0;
                (flow_floor + first.baseline_gap as f32 / 100.0 - stored).max(0.0)
            });
            // A cached run that restarts inside the paragraph is Hancom continuing
            // the cell on a later page (#233). It follows the previous line instead
            // of being painted over the paragraph's first run; the offset is the
            // one its first line needs, so the run keeps its own line spacing.
            let mut run_shift = 0.0f32;
            let mut previous_v: Option<i32> = None;
            for i in selected {
                let seg = &line_segs[i];
                let line_start = seg.text_start;
                let line_end = line_segs
                    .get(i + 1)
                    .map_or(para.wchar_len(), |next| next.text_start);
                if line_end <= line_start {
                    continue;
                }
                let gap_pt = seg.baseline_gap as f32 / 100.0;
                let v_pos = seg.v_pos.saturating_sub(v_origin);
                let stored = origin_y + (v_pos + seg.baseline_gap) as f32 / 100.0;
                if previous_v.is_some_and(|previous| seg.v_pos < previous) {
                    run_shift = run_shift.max(content_bottom + gap_pt - stored - para_shift);
                }
                previous_v = Some(seg.v_pos);
                let shift = para_shift + run_shift;
                // Respect cached v_pos, shifting only above the flow floor rather than forcing the flow cursor.
                // para_shift moves the entire paragraph equally, preserving its line spacing.
                let baseline_y = (stored + shift).max(flow_floor + gap_pt);
                // Fail closed on a shift that would leave the container: the
                // line is clipped rather than painted over the row below or
                // over the footer, and the loss is reported (#222 follow-up).
                // Only a shifted line is checked - a cached line that overflows
                // where Hancom stored it keeps the established contract that a
                // stored row height is never grown, and is already reported by
                // the measurement pass.
                if shift > 0.0
                    && let Some(limit) = content_limit
                    && baseline_y - gap_pt + seg.line_height.max(seg.text_height) as f32 / 100.0
                        > limit + CELL_CONTENT_OVERFLOW_EPSILON_PT
                {
                    warnings.push_once(
                        RenderIssueCode::TableCellContentOverflow,
                        format!("shift-clipped {:.0}", baseline_y - gap_pt - limit),
                    );
                    continue;
                }
                let mut items = shape_range_page(
                    store,
                    doc,
                    para,
                    (line_start, line_end),
                    &std::collections::HashMap::new(),
                    page_number,
                    warnings,
                );
                let natural_width = items_width(&items, &tabs);

                // Alignment uses width after subtracting indent, so compute it below after resolving indent.
                let seg_width_pt = (seg.seg_width as f32 / 100.0).min(width);
                let align = doc
                    .header
                    .para_shapes
                    .get(para.para_shape.0 as usize)
                    .map_or(0, |ps| ps.alignment());

                let first_selected = match &selection {
                    BoxParaSelection::All => i == 0,
                    BoxParaSelection::Segments(range) => i == range.start,
                    BoxParaSelection::Empty => false,
                };
                let box_x = origin_x + seg.col_start as f32 / 100.0;
                let mut marker_advance = 0.0;
                if first_selected {
                    para_top = Some(baseline_y - gap_pt);
                    if let Some(m) = &marker {
                        let size = items_max_size(&items).unwrap_or(8.0);
                        marker_advance = render_list_marker(
                            page,
                            store,
                            doc,
                            m,
                            (box_x, baseline_y, size),
                            warnings,
                        );
                        // Every fragment of a split cell paragraph redraws its marker, so
                        // without this each fragment's range would reach back to 0 and the
                        // paragraph's rows would overlap (D-09).
                        if let Some(rec) = rec.as_deref_mut() {
                            rec.marker_drawn(page);
                        }
                    }
                }
                // col_start contains only the left margin; apply first-line/hanging indent here.
                let geom = para_geometry(doc, para);
                let (first_indent, rest_indent) = line_indents(&geom, marker_advance);
                let indent = if i == 0 { first_indent } else { rest_indent };
                // Align and wrap within the post-indent width so the right edge does not move.
                let text_width = (seg_width_pt - indent).max(1.0);
                let shift = align_line(
                    &mut items,
                    align,
                    text_width,
                    natural_width,
                    i == last_content,
                );
                let x = box_x + indent + shift;
                // Match the body path's rule (CACHE_OVERFLOW_TOLERANCE).
                let wrap_width = if line_segs.len() == 1
                    && natural_width > text_width * CACHE_OVERFLOW_TOLERANCE
                {
                    text_width.max(4.0)
                } else {
                    f32::INFINITY
                };
                let line_advance =
                    (seg.line_height + seg.line_spacing).max(seg.line_height) as f32 / 100.0;

                let last_y = place_wrapped(
                    page,
                    items,
                    x,
                    baseline_y,
                    wrap_width,
                    line_advance,
                    &tabs,
                    0.0, // The line offset is already included in x.
                    &doc.header.border_fills,
                    None,
                    warnings,
                );
                content_bottom = last_y + (seg.line_height as f32 / 100.0 - gap_pt).max(0.0);
                // If our wrapping extends below the saved line (such as additional wrapping within
                // a single-segment paragraph), raise the flow floor to prevent later cache overlap.
                // Multiple segments use wrap_width=INFINITY, so last_y == baseline_y and the floor stays unchanged.
                if last_y > baseline_y {
                    flow_floor = flow_floor.max(content_bottom);
                }
            }
        }

        // Nested tables/images inside cells: raise the flow floor when objects extend the bottom.
        // A nested object belongs to the first fragment that contains this
        // paragraph.  Rendering it for every selected line range would
        // duplicate the object on continuation pages.
        let render_objects = match &selection {
            BoxParaSelection::All => true,
            BoxParaSelection::Segments(range) => range.start == 0,
            BoxParaSelection::Empty => false,
        };
        if render_objects {
            let before_objects = content_bottom;
            let mut disabled = SegmentRecorder::disabled();
            let (objects_bottom, _no_split) = layout_para_objects(
                doc,
                store,
                page,
                para,
                origin_x,
                para_top.unwrap_or(content_bottom),
                content_bottom,
                width,
                (origin_x, origin_y), // Nested shapes are relative to this box's own origin.
                None,                 // Nested objects inside a cell/text box do not cross pages.
                warnings,
                // A recorded paragraph's own tables, bookmarks and drawing objects nest under
                // its open span; any other box records nothing, so neither do its objects.
                match rec.as_deref_mut() {
                    Some(rec) => rec,
                    None => &mut disabled,
                },
            );
            content_bottom = objects_bottom;
            if content_bottom > before_objects {
                flow_floor = flow_floor.max(content_bottom);
            }
        }
        if let Some(rec) = rec.as_deref_mut() {
            rec.end_segment(page);
        }

        // End of a paragraph: the next one may not start above what this one
        // occupies. Documents whose cached v_pos restarts at 0 per paragraph
        // would otherwise stack every paragraph of a cell on the first one
        // (#222). No-op for cell-cumulative caches, which already sit below.
        flow_floor = flow_floor.max(content_bottom);
        if synthesize_uncached && was_uncached {
            // Retain the prior nested-list cursor: line advance only. Body
            // paragraph before/after gaps are not established for native cells.
            if let Some(last) = line_segs.last() {
                uncached_next_v = last
                    .v_pos
                    .saturating_add(crate::lineseg::paragraph_metrics(doc, para).advance);
            }
        }
    }
    content_bottom
}

/// Derive unknown columns from merged cells (col_span>1) when no col_span==1 cell defines them.
/// Scale widths to the table's actual width, the maximum sum of cell widths in any row.
/// If that exceeds avail_width (body/cell width), shrink to fit, matching Hangul.
/// This fixes merged tables overflowing pages under average fallback; normal tables retain s~1.
fn derive_col_widths(col_w: &mut [f32], table: &Table, avail_width: f32) {
    let cols = col_w.len();
    // 1) Derive unknown columns from merged cells, processing smaller spans before larger spans.
    let mut spanning: Vec<_> = table.cells.iter().filter(|c| c.col_span > 1).collect();
    spanning.sort_by_key(|c| c.col_span);
    for cell in spanning {
        let c = cell.col as usize;
        let end = (c + cell.col_span as usize).min(cols);
        if c >= end {
            continue;
        }
        let known: f32 = col_w[c..end].iter().filter(|w| **w > 0.0).sum();
        let unknown: Vec<usize> = (c..end).filter(|&i| col_w[i] <= 0.0).collect();
        let cw = cell.width.to_pt() as f32;
        if !unknown.is_empty() && cw > known {
            let each = (cw - known) / unknown.len() as f32;
            for i in unknown {
                col_w[i] = each;
            }
        }
    }
    // 2) Average any columns still unknown, retaining the existing fallback.
    fill_unknown(col_w, 60.0);
    // 3) Target the actual table width to correct derivation residuals, bounded by available width
    // to match Hangul's shrinking behavior. Normal tables have sum==table_width<=avail and s=1.
    let mut target = table_true_width(table);
    if avail_width > 0.0 && target > avail_width {
        target = avail_width;
    }
    let sum: f32 = col_w.iter().sum();
    if target > 0.0 && sum > 0.0 {
        let s = target / sum;
        for w in col_w.iter_mut() {
            *w *= s;
        }
    }
}

/// Actual table width in pt: maximum sum of cell widths per row, covering every column.
fn table_true_width(table: &Table) -> f32 {
    let mut by_row: std::collections::HashMap<u16, f32> = std::collections::HashMap::new();
    for cell in &table.cells {
        *by_row.entry(cell.row).or_default() += cell.width.to_pt() as f32;
    }
    by_row.values().copied().fold(0.0, f32::max)
}

fn fill_unknown(values: &mut [f32], fallback: f32) {
    let known: Vec<f32> = values.iter().copied().filter(|v| *v > 0.0).collect();
    let avg = if known.is_empty() {
        fallback
    } else {
        known.iter().sum::<f32>() / known.len() as f32
    };
    for v in values.iter_mut() {
        if *v <= 0.0 {
            *v = avg;
        }
    }
}

fn prefix_sums(values: &[f32], start: f32) -> Vec<f32> {
    let mut out = Vec::with_capacity(values.len() + 1);
    let mut acc = start;
    for v in values {
        out.push(acc);
        acc += v;
    }
    out.push(acc);
    out
}

/// Index of the last text-bearing segment (line_end>line_start), ignoring empty trailing segments.
fn last_content_seg(para: &Paragraph) -> usize {
    last_content_seg_in(para, &para.line_segs)
}

fn last_content_seg_in(para: &Paragraph, segs: &[hwp_model::LineSeg]) -> usize {
    let n = segs.len();
    (0..n)
        .rev()
        .find(|&j| {
            let ls = segs[j].text_start;
            let le = segs.get(j + 1).map_or(para.wchar_len(), |s| s.text_start);
            le > ls
        })
        .unwrap_or(n.saturating_sub(1))
}

/// Returns the horizontal shift for a paragraph alignment mode.
///
/// Justify (0) stretches non-final lines only. Distribute (4) and divide (5)
/// also stretch the final line to `seg_width`; that final-line behavior still
/// needs confirmation against Hancom output.
fn align_line(
    items: &mut [InlineItem],
    align: u8,
    seg_width: f32,
    natural: f32,
    is_last: bool,
) -> f32 {
    let available = (seg_width - natural).max(0.0);
    match align {
        2 => available,
        3 => available / 2.0,
        0 if !is_last => {
            // Retain the defensive 100% stretch cap for ordinary justification,
            // which may otherwise amplify a missing-font width estimate.
            let slack = available.min(natural.max(1.0));
            justify_line(items, slack, true, false);
            0.0
        }
        4 => {
            // Distribute includes the trailing gap and must fill the entire segment.
            justify_line(items, available, false, true);
            0.0
        }
        5 => {
            // Divide excludes the trailing gap and must fill the entire segment.
            justify_line(items, available, false, false);
            0.0
        }
        _ => 0.0,
    }
}

/// Distributes justification slack. `spaces_first` gives all slack to
/// whitespace before the last visible glyph, falling back to even gaps.
/// Otherwise the slack is spread evenly over inter-glyph gaps with no
/// space priority — `include_trailing` decides whether the gap after the last
/// glyph counts (distribute) or not (divide). Shaping-cluster source mappings identify
/// whitespace correctly even when ligatures change the glyph count.
fn justify_line(items: &mut [InlineItem], slack: f32, spaces_first: bool, include_trailing: bool) {
    if slack <= 0.0 {
        return;
    }
    // Determine whitespace from the source sequence represented by each glyph.
    let mut is_space: Vec<bool> = Vec::new();
    for item in items.iter() {
        if let InlineItem::Run(run) = item {
            is_space.extend(
                crate::shape::glyph_source_sequences(run)
                    .iter()
                    .map(|source| source.chars().all(char::is_whitespace)),
            );
        }
    }
    let total = is_space.len();
    if total == 0 || (total < 2 && !include_trailing) {
        return;
    }
    // Ordinary justification does not distribute after the last visible glyph.
    let last_visible = is_space.iter().rposition(|&s| !s).unwrap_or(total - 1);
    let space_count = is_space[..last_visible].iter().filter(|&&s| s).count();

    // Only ordinary justification prioritizes whitespace. Distribute and divide
    // treat every eligible glyph gap equally (05-rendering section 1.4).
    let use_spaces = spaces_first && space_count > 0;
    let denom = if use_spaces {
        space_count as f32
    } else if include_trailing {
        total as f32
    } else {
        last_visible.max(1) as f32
    };
    let extra = slack / denom;

    let mut gi = 0usize;
    for item in items.iter_mut() {
        if let InlineItem::Run(run) = item {
            let mut added = 0.0;
            for g in run.glyphs.iter_mut() {
                let apply = if use_spaces {
                    is_space[gi] && gi < last_visible
                } else if include_trailing {
                    true
                } else {
                    gi < last_visible
                };
                if apply {
                    g.x_advance += extra;
                    added += extra;
                }
                gi += 1;
            }
            run.width_pt += added;
        }
    }
}

/// Paragraph geometry in pt: indentation, margins and gaps for fallback layout.
/// Cached linesegs already encode these in col_start/v_pos, so that path does not use them.
#[derive(Default, Clone, Copy)]
struct ParaGeom {
    /// Left margin only (margin_left); indentation is separate in first_indent.
    left: f32,
    right: f32,
    /// Positive first-line indent or negative hanging indent, placing the first line farther left.
    /// Do not subtract this from wrap width: narrow cells would collapse avail and wrap every glyph,
    /// as measured in work_report. Clamp against the page's left edge where placement occurs.
    first_indent: f32,
    spacing_top: f32,
    spacing_bottom: f32,
}

/// Draw one page fragment of a paragraph background/border (ParaShape.border_fill_id -> BorderFill).
/// This is the paragraph equivalent of the cell background pass. Insert its background Rect at
/// insert_idx (the paragraph text start on this page), behind text, then overlay border lines.
/// left/width already account for paragraph geometry and column offset. For a paragraph crossing
/// pages, omit the boundary-facing borders via draw_top/draw_bottom=false (GC-9).
/// `#[must_use]`: the return value is the insert count every caller has to hand to
/// `SegmentRecorder::items_inserted`. Dropping it silently misattributes every item recorded
/// after the insert on that page, so the compiler is made to object to a call that ignores it.
#[must_use]
#[allow(clippy::too_many_arguments)]
fn draw_para_bg_slice(
    doc: &Document,
    page: &mut PageList,
    para: &Paragraph,
    left: f32,
    width: f32,
    insert_idx: usize,
    top: f32,
    bottom: f32,
    draw_top: bool,
    draw_bottom: bool,
    warnings: &mut RenderIssueAccumulator,
) -> usize {
    // How many items were *inserted* at `insert_idx` rather than appended. The caller feeds it
    // to `SegmentRecorder::items_inserted`, because an insert shifts every later recorded index
    // on this page; appends do not.
    let mut inserted = 0usize;
    if bottom <= top || width <= 0.0 {
        return inserted;
    }
    let Some(ps) = doc.header.para_shapes.get(para.para_shape.0 as usize) else {
        return inserted;
    };
    // border_fill_id is one-based; zero means no reference.
    let Some(idx) = (ps.border_fill_id as usize).checked_sub(1) else {
        return inserted;
    };
    let Some(bf) = doc.header.border_fills.get(idx) else {
        return inserted;
    };
    // Insert the background behind text.
    if let Some(item) = bg_fill_item(bf, left, top, width, bottom - top) {
        let ins = insert_idx.min(page.items.len());
        if !warnings.charge_display_items(1) {
            return inserted;
        }
        page.items.insert(ins, item);
        inserted += 1;
    }
    // Left and right are always visible; page-split slices suppress internal top/bottom edges.
    let items = crate::border::border_rectangle_items(
        left,
        top,
        left + width,
        bottom,
        &bf.sides,
        [true, true, draw_top, draw_bottom],
    );
    if !warnings.charge_display_items(items.len()) {
        return inserted;
    }
    page.items.extend(items);
    inserted
}

fn para_geometry(doc: &Document, para: &Paragraph) -> ParaGeom {
    // IR PARA_SHAPE margins/indent/spacing use twice HWPUNIT, matching HWP5 storage.
    // Measured HWPX reader rule: OWPML left=1500 becomes HWP5 ml=3000; see read/header.rs.
    // Convert to pt by dividing by 200.
    match doc.header.para_shapes.get(para.para_shape.0 as usize) {
        Some(p) => ParaGeom {
            left: (p.margin_left as f32 / 200.0).max(0.0),
            right: (p.margin_right as f32 / 200.0).max(0.0),
            // Preserve negative hanging indent; apply it only to the first line and clamp at the page left edge.
            first_indent: p.indent as f32 / 200.0,
            spacing_top: (p.spacing_top as f32 / 200.0).max(0.0),
            spacing_bottom: (p.spacing_bottom as f32 / 200.0).max(0.0),
        },
        None => ParaGeom::default(),
    }
}

fn items_width(items: &[InlineItem], tabs: &[TabStop]) -> f32 {
    let mut x = 0.0f32;
    let mut idx = 0usize;
    while idx < items.len() {
        match &items[idx] {
            InlineItem::Run(run) => x += run.width_pt,
            InlineItem::Tab => {
                let (position, stop) = crate::tab::next_tab_at(tabs, x, TAB_INTERVAL_PT);
                x = match stop {
                    // Same kind-aware offset rule as place_wrapped, so estimation and
                    // placement always agree (explicit_tab_stops_match_width_estimation_and_placement).
                    Some(stop) => {
                        let segment_width = tab_segment_width(items, idx + 1);
                        let decimal_offset = (stop.kind == 3)
                            .then(|| first_run_decimal_offset(items, idx + 1))
                            .flatten();
                        crate::tab::tab_segment_offset(
                            stop.kind,
                            stop.pos_pt,
                            segment_width,
                            decimal_offset,
                        )
                        .max(x)
                    }
                    None => position,
                };
            }
            InlineItem::LineBreak(_) => x = 0.0, // Start a new line.
        }
        idx += 1;
    }
    x
}

fn items_max_size(items: &[InlineItem]) -> Option<f32> {
    items
        .iter()
        .filter_map(|i| match i {
            InlineItem::Run(r) => Some(r.size_pt),
            InlineItem::Tab | InlineItem::LineBreak(_) => None,
        })
        .reduce(f32::max)
}

/// Places a glyph run together with underline, strike, emphasis, border, and background.
/// Decoration metrics are initial values pending U5 measurements.
pub(crate) fn push_run(
    page: &mut PageList,
    x: f32,
    y: f32,
    run: crate::shape::ShapedRun,
    border_fills: &[BorderFill],
    warnings: &mut RenderIssueAccumulator,
) {
    let w = run.width_pt;
    let em = run.size_pt;

    // GG-22 character border/background references are one-based. Emit the
    // background before Glyphs, using the same ordering as paragraph backgrounds.
    // The run box metrics are placeholders pending Hancom verification.
    let bf = if run.border_fill_id > 0 {
        border_fills.get(run.border_fill_id as usize - 1)
    } else {
        None
    };
    let box_top = y - em * 0.80;
    let box_bottom = y + em * 0.25;
    let bg_item = bf.and_then(|bf| bg_fill_item(bf, x, box_top, w, box_bottom - box_top));

    let decor_w = em * 0.05; // Initial underline/strike width pending U5 measurements.
    let ul_color = if run.underline_color == 0xFFFF_FFFF {
        run.color
    } else {
        run.underline_color
    };
    // Kind 1 is below the text; kind 3 uses the provisional ascent boundary.
    let underline_y = match run.underline_kind {
        1 => Some(y + em * 0.10),
        3 => Some(y - em * 0.80),
        _ => None,
    };
    let emphasis_centers = emphasis_centers(&run, x);
    let emphasis_count = emphasis_centers
        .len()
        .saturating_mul(emphasis_mark_count(run.emphasis));
    let underline_count = underline_y
        .map(|_| crate::border::decor_stroke_count(run.underline_shape))
        .unwrap_or(0);
    let strike_count =
        usize::from(run.strike) * crate::border::decor_stroke_count(run.strike_shape);
    let border_count = bf
        .map(|fill| {
            crate::border::border_rectangle_item_count(
                x,
                box_top,
                x + w,
                box_bottom,
                &fill.sides,
                [true; 4],
            )
        })
        .unwrap_or(0);
    let total_items = 1usize
        .saturating_add(usize::from(bg_item.is_some()))
        .saturating_add(underline_count)
        .saturating_add(strike_count)
        .saturating_add(emphasis_count)
        .saturating_add(border_count);
    if !warnings.charge_display_items(total_items) {
        return;
    }

    let mut decor = Vec::with_capacity(
        underline_count
            .saturating_add(strike_count)
            .saturating_add(emphasis_count),
    );
    if let Some(ly) = underline_y {
        decor.extend(decor_line_items(
            x,
            ly,
            w,
            run.underline_shape,
            decor_w,
            ul_color,
        ));
    }
    if run.strike {
        decor.extend(decor_line_items(
            x,
            y - em * 0.25,
            w,
            run.strike_shape,
            decor_w,
            run.color,
        ));
    }
    decor.extend(emphasis_items(
        run.emphasis,
        emphasis_centers,
        y,
        em,
        run.color,
    ));
    debug_assert_eq!(decor.len(), underline_count + strike_count + emphasis_count);

    if let Some(item) = bg_item {
        page.items.push(item);
    }
    page.items.push(Item::Glyphs { x, y, run });

    // Draw the character border above glyphs, matching paragraph border ordering.
    if let Some(bf) = bf {
        let items = crate::border::border_rectangle_items(
            x,
            box_top,
            x + w,
            box_bottom,
            &bf.sides,
            [true; 4],
        );
        debug_assert_eq!(items.len(), border_count);
        page.items.extend(items);
    }
    page.items.extend(decor);
}

/// Builds one underline or strike decoration from a zero-based decoration code.
/// Wave codes 11 and 12 emit cubic Bezier paths.
fn decor_line_items(x: f32, y: f32, w: f32, code: u8, width_pt: f32, color: u32) -> Vec<Item> {
    match crate::border::decor_is_wave(code) {
        0 => crate::border::decor_strokes(code, width_pt, color)
            .into_iter()
            .map(|(off, stroke)| Item::Path {
                commands: vec![PathCmd::MoveTo(x, y + off), PathCmd::LineTo(x + w, y + off)],
                fill: None,
                stroke: Some(stroke),
                rule: FillRule::NonZero,
            })
            .collect(),
        waves => {
            // Use the stroke width as amplitude and place a second DoubleWave
            // path 2.5 widths lower. These are provisional visual metrics.
            let amp = width_pt;
            let half_len = (width_pt * 3.5).max(0.9); // About 0.175em at a 0.05em width.
            (0..waves)
                .map(|i| {
                    let dy = f32::from(i) * width_pt * 2.5;
                    Item::Path {
                        commands: wave_commands(x, y + dy, x + w, half_len, amp),
                        fill: None,
                        stroke: Some(Stroke::solid(color, width_pt)),
                        rule: FillRule::NonZero,
                    }
                })
                .collect()
        }
    }
}

/// Builds a wave path from `(x, y)` to `(end, y)` in cubic half-wavelengths.
/// Control height `amp / 0.75` makes the midpoint displacement equal `amp`.
fn wave_commands(mut x: f32, y: f32, end: f32, half_len: f32, amp: f32) -> Vec<PathCmd> {
    let k = amp / 0.75;
    let mut cmds = vec![PathCmd::MoveTo(x, y)];
    let mut up = true;
    while x < end - 1e-3 {
        let seg = half_len.min(end - x);
        let cy = if up { y - k } else { y + k };
        cmds.push(PathCmd::CubicTo(
            x + seg * 0.5,
            cy,
            x + seg * 0.5,
            cy,
            x + seg,
            y,
        ));
        x += seg;
        up = !up;
    }
    cmds
}

/// Approximates a circle with four cubic Bezier segments.
fn circle_commands(cx: f32, cy: f32, r: f32) -> Vec<PathCmd> {
    const KAPPA: f32 = 0.5523; // Standard circle-to-Bezier approximation.
    let k = r * KAPPA;
    vec![
        PathCmd::MoveTo(cx + r, cy),
        PathCmd::CubicTo(cx + r, cy + k, cx + k, cy + r, cx, cy + r),
        PathCmd::CubicTo(cx - k, cy + r, cx - r, cy + k, cx - r, cy),
        PathCmd::CubicTo(cx - r, cy - k, cx - k, cy - r, cx, cy - r),
        PathCmd::CubicTo(cx + k, cy - r, cx + r, cy - k, cx + r, cy),
        PathCmd::Close,
    ]
}

// GG-8 emphasis metrics are U5-style placeholders pending Hancom verification.
const EMPHASIS_DOT_R: f32 = 0.05;
const EMPHASIS_SMALL_R: f32 = 0.035;
const EMPHASIS_ABOVE: f32 = 0.95;
const EMPHASIS_BELOW: f32 = 0.30;
const EMPHASIS_HALF: f32 = 0.08;
const EMPHASIS_LIFT: f32 = 0.04;
const EMPHASIS_STROKE: f32 = 0.025;

/// Returns the centers of emphasis marks, excluding whitespace glyphs.
/// Synthetic runs without glyphs distribute marks evenly across the run width.
fn emphasis_centers(run: &crate::shape::ShapedRun, x: f32) -> Vec<f32> {
    let mut centers = Vec::new();
    if run.emphasis == 0 {
        return centers;
    }
    if run.glyphs.is_empty() {
        let n = run.text.chars().filter(|c| !c.is_whitespace()).count();
        if n > 0 && run.width_pt > 0.0 {
            let step = run.width_pt / n as f32;
            centers.extend((0..n).map(|i| x + step * (i as f32 + 0.5)));
        }
    } else {
        let sources = crate::shape::glyph_source_sequences(run);
        let mut pen = x;
        for (i, g) in run.glyphs.iter().enumerate() {
            let cx = pen + g.x_offset + g.x_advance * 0.5;
            pen += g.x_advance;
            let blank = sources
                .get(i)
                .is_none_or(|s| s.chars().all(|c| c.is_whitespace()));
            if !blank {
                centers.push(cx);
            }
        }
    }
    centers
}

fn emphasis_mark_count(kind: u8) -> usize {
    match kind {
        6 => 2,
        1..=12 => 1,
        _ => 0,
    }
}

fn emphasis_items(kind: u8, centers: Vec<f32>, y: f32, em: f32, color: u32) -> Vec<Item> {
    let cy = if kind == 12 {
        y + em * EMPHASIS_BELOW
    } else {
        y - em * EMPHASIS_ABOVE
    };
    let mut items = Vec::with_capacity(centers.len().saturating_mul(emphasis_mark_count(kind)));
    for cx in centers {
        items.extend(emphasis_mark(kind, cx, cy, em, color));
    }
    items
}

/// Builds one emphasis mark using hwplib `EmphasisSort` ordering.
/// Accent forms 7 through 11 use short line or curve approximations.
fn emphasis_mark(kind: u8, cx: f32, cy: f32, em: f32, color: u32) -> Vec<Item> {
    let dot_r = em * EMPHASIS_DOT_R;
    let small_r = em * EMPHASIS_SMALL_R;
    let half = em * EMPHASIS_HALF;
    let lift = em * EMPHASIS_LIFT;
    let sw = em * EMPHASIS_STROKE;
    let filled = |cmds: Vec<PathCmd>| Item::Path {
        commands: cmds,
        fill: Some(Fill::Solid(color)),
        stroke: None,
        rule: FillRule::NonZero,
    };
    let stroked = |cmds: Vec<PathCmd>| Item::Path {
        commands: cmds,
        fill: None,
        stroke: Some(Stroke::solid(color, sw)),
        rule: FillRule::NonZero,
    };
    match kind {
        // 1 DOT_ABOVE: filled circle.
        1 => vec![filled(circle_commands(cx, cy, dot_r))],
        // 2 RING_ABOVE: outlined circle.
        2 => vec![stroked(circle_commands(cx, cy, dot_r))],
        // 3 TILDE: one short wave.
        3 => vec![stroked(wave_commands(
            cx - half * 1.2,
            cy,
            cx + half * 1.2,
            half * 1.2,
            lift,
        ))],
        // 4 CARON: two segments meeting below.
        4 => vec![stroked(vec![
            PathCmd::MoveTo(cx - half, cy - lift),
            PathCmd::LineTo(cx, cy + lift),
            PathCmd::LineTo(cx + half, cy - lift),
        ])],
        // 5 SIDE: smaller filled dot.
        5 => vec![filled(circle_commands(cx, cy, small_r))],
        // 6 COLON: two vertical dots.
        6 => vec![
            filled(circle_commands(cx, cy - small_r * 1.6, small_r)),
            filled(circle_commands(cx, cy + small_r * 1.6, small_r)),
        ],
        // 7 GRAVE_ACCENT: short backslash segment.
        7 => vec![stroked(vec![
            PathCmd::MoveTo(cx - half * 0.7, cy - lift),
            PathCmd::LineTo(cx + half * 0.7, cy + lift),
        ])],
        // 8 ACUTE_ACCENT: short slash segment.
        8 => vec![stroked(vec![
            PathCmd::MoveTo(cx - half * 0.7, cy + lift),
            PathCmd::LineTo(cx + half * 0.7, cy - lift),
        ])],
        // 9 CIRCUMFLEX: two segments meeting above.
        9 => vec![stroked(vec![
            PathCmd::MoveTo(cx - half, cy + lift),
            PathCmd::LineTo(cx, cy - lift),
            PathCmd::LineTo(cx + half, cy + lift),
        ])],
        // 10 MACRON: short horizontal segment.
        10 => vec![stroked(vec![
            PathCmd::MoveTo(cx - half, cy),
            PathCmd::LineTo(cx + half, cy),
        ])],
        // 11 HOOK_ABOVE: short curve approximation.
        11 => vec![stroked(vec![
            PathCmd::MoveTo(cx - half * 0.5, cy - lift),
            PathCmd::CubicTo(
                cx + half,
                cy - lift,
                cx + half * 0.8,
                cy + lift,
                cx,
                cy + lift,
            ),
        ])],
        // 12 DOT_BELOW: filled dot below the text.
        12 => vec![filled(circle_commands(cx, cy, dot_r))],
        _ => Vec::new(),
    }
}

/// The paragraph WCHAR offset the first glyph run at or after `from` starts at.
///
/// `u32::MAX` when there is none: a break there leaves no anchored note on the later side,
/// so every note the paragraph carries belongs to the page being finalized.
fn first_wchar_from(items: &[Item], from: usize) -> u32 {
    items
        .get(from..)
        .unwrap_or(&[])
        .iter()
        .find_map(|item| match item {
            Item::Glyphs { run, .. } => Some(run.start_wchar),
            _ => None,
        })
        .unwrap_or(u32::MAX)
}

/// Places inline items with greedy glyph-level wrapping at `max_width`.
///
/// An infinite width disables wrapping. The return value is the final baseline.
///
/// `lines`, when given, receives one `(item index, baseline)` entry per line placed, the first
/// one for the line that starts at `first_baseline_y`. The index is into `page.items` and is
/// where that line's items begin, so a caller that has to move whole lines elsewhere - the
/// fallback band's mid-paragraph page break (#282) - can slice them without re-deriving the
/// wrap. Every other caller passes `None` rather than a throwaway `Vec`: this runs once per
/// cell paragraph.
#[allow(clippy::too_many_arguments)]
fn place_wrapped(
    page: &mut PageList,
    mut items: Vec<InlineItem>,
    x0: f32,
    first_baseline_y: f32,
    max_width: f32,
    line_advance: f32,
    tabs: &[TabStop],
    first_indent: f32,
    border_fills: &[BorderFill],
    mut lines: Option<&mut Vec<(usize, f32)>>,
    warnings: &mut RenderIssueAccumulator,
) -> f32 {
    let limit = x0 + max_width;
    if let Some(lines) = lines.as_deref_mut() {
        lines.push((page.items.len(), first_baseline_y));
    }
    // Apply first_indent only to the first line; subsequent and broken lines return to x0 (left margin).
    let mut x = x0 + first_indent;
    let mut y = first_baseline_y;
    let mut previous_no_break = false;

    if std::env::var_os("HWP_RENDER_TRACE").is_some() {
        let preview: String = items
            .iter()
            .filter_map(|i| match i {
                InlineItem::Run(r) => Some(r.text.as_str()),
                InlineItem::Tab | InlineItem::LineBreak(_) => None,
            })
            .collect::<String>()
            .chars()
            .take(20)
            .collect();
        eprintln!("TRACE y={first_baseline_y:.1} x={x0:.1} wrap={max_width:.0} [{preview}]");
    }

    // Index-based walk (rather than `for item in items`) so the tab arm can look ahead at
    // the following segment via `tab_segment_width`/`first_run_decimal_offset`. Each slot is
    // taken by value via `mem::replace`; the placeholder left behind is never read again since
    // look-ahead only ever reads forward from `idx + 1`.
    let mut idx = 0usize;
    while idx < items.len() {
        let item = std::mem::replace(&mut items[idx], InlineItem::Tab);
        match item {
            InlineItem::Run(run) => {
                let sources = crate::shape::glyph_source_sequences(&run);
                if max_width.is_infinite() || x + run.width_pt <= limit {
                    let w = run.width_pt;
                    previous_no_break = sources
                        .last()
                        .is_some_and(|source| crate::shape::source_has_no_break_space(source));
                    push_run(page, x, y, run, border_fills, warnings);
                    x += w;
                    idx += 1;
                    continue;
                }
                // CJK can wrap between glyph clusters except on either side of NB_SPACE.
                let mut start = 0usize;
                let mut piece_x = x;
                let mut acc = 0.0f32;
                for (i, g) in run.glyphs.iter().enumerate() {
                    let over = piece_x + acc + g.x_advance > limit;
                    let line_has_content = i > start || piece_x > x0;
                    let current_no_break = crate::shape::source_has_no_break_space(&sources[i]);
                    let break_allowed = !previous_no_break && !current_no_break;
                    if over && line_has_content && break_allowed {
                        if i > start {
                            let piece = run.slice_with_sources(start, i, &sources);
                            push_run(page, piece_x, y, piece, border_fills, warnings);
                        }
                        y += line_advance;
                        if let Some(lines) = lines.as_deref_mut() {
                            lines.push((page.items.len(), y));
                        }
                        piece_x = x0;
                        acc = 0.0;
                        start = i;
                    }
                    acc += g.x_advance;
                    previous_no_break = current_no_break;
                }
                if start < run.glyphs.len() {
                    let piece = run.slice_with_sources(start, run.glyphs.len(), &sources);
                    let w = piece.width_pt;
                    push_run(page, piece_x, y, piece, border_fills, warnings);
                    x = piece_x + w;
                } else {
                    x = piece_x;
                }
                idx += 1;
            }
            InlineItem::Tab => {
                let (position, stop) = crate::tab::next_tab_at(tabs, x - x0, TAB_INTERVAL_PT);
                let next_x = match stop {
                    Some(stop) => {
                        let segment_width = tab_segment_width(&items, idx + 1);
                        let decimal_offset = (stop.kind == 3)
                            .then(|| first_run_decimal_offset(&items, idx + 1))
                            .flatten();
                        // Clamp so a crafted stop position or an oversized segment can never
                        // push text backwards — never before the current x (T-1-08).
                        (x0 + crate::tab::tab_segment_offset(
                            stop.kind,
                            stop.pos_pt,
                            segment_width,
                            decimal_offset,
                        ))
                        .max(x)
                    }
                    // Default grid: no kind-aware placement, unchanged from before this plan.
                    None => x0 + position,
                };
                // Leader line: only when the landed stop declares one (fill != 0). The fill
                // code is HWPX leader numbering, not dash_pattern's — hwp5_line_style bridges
                // the two families (see shape_draw.rs's own cross-reference comment). The
                // leader spans to `next_x`, the point where the following text actually
                // begins — not to the raw stop position.
                if let Some(stop) = stop
                    && stop.fill != 0
                    && warnings.charge_display_items(1)
                {
                    page.items.push(Item::Path {
                        commands: vec![PathCmd::MoveTo(x, y), PathCmd::LineTo(next_x, y)],
                        fill: None,
                        stroke: Some(Stroke {
                            color: 0x0000_0000,
                            width: 0.5,
                            dash: crate::shape_draw::dash_pattern(
                                crate::shape_draw::hwp5_line_style(stop.fill),
                                LEADER_DASH_SCALE,
                            ),
                        }),
                        rule: FillRule::NonZero,
                    });
                }
                x = next_x;
                previous_no_break = false;
                idx += 1;
            }
            InlineItem::LineBreak(_) => {
                y += line_advance;
                if let Some(lines) = lines.as_deref_mut() {
                    lines.push((page.items.len(), y));
                }
                x = x0;
                previous_no_break = false;
                idx += 1;
            }
        }
    }
    y
}

/// Natural width of the inline items following a tab, up to but excluding the next
/// `InlineItem::Tab` or `InlineItem::LineBreak` (or the end of `items`). Used so right,
/// center and decimal placement can look ahead at the segment before it is placed. Zero when
/// the segment is empty.
fn tab_segment_width(items: &[InlineItem], from: usize) -> f32 {
    items
        .get(from..)
        .unwrap_or(&[])
        .iter()
        .take_while(|item| !matches!(item, InlineItem::Tab | InlineItem::LineBreak(_)))
        .map(|item| match item {
            InlineItem::Run(run) => run.width_pt,
            InlineItem::Tab | InlineItem::LineBreak(_) => 0.0,
        })
        .sum()
}

/// The accumulated `x_advance` up to (not including) the first glyph whose source contains a
/// decimal separator (`.`), in the first run of the segment starting at `from`. `None` when
/// that run has no separator, or the segment doesn't start with a run at all — the tab arm
/// then falls back to right-tab placement, per a decimal tab with no decimal value.
fn first_run_decimal_offset(items: &[InlineItem], from: usize) -> Option<f32> {
    let InlineItem::Run(run) = items.get(from)? else {
        return None;
    };
    let sources = crate::shape::glyph_source_sequences(run);
    let dot_idx = sources.iter().position(|source| source.contains('.'))?;
    Some(run.glyphs[..dot_idx].iter().map(|g| g.x_advance).sum())
}

#[cfg(test)]
mod page_number_layout_tests {
    use super::{PageNumberState, control_char_shape, page_number_alignment};
    use crate::issues::RenderIssueAccumulator;
    use hwp_model::{CharShapeId, Control, GenericControl, HwpChar, Paragraph, ParagraphList};

    #[test]
    fn 고정_위치_정렬() {
        assert_eq!(page_number_alignment(1, 1), -1);
        assert_eq!(page_number_alignment(2, 1), 0);
        assert_eq!(page_number_alignment(3, 1), 1);
        assert_eq!(page_number_alignment(4, 2), -1);
        assert_eq!(page_number_alignment(5, 2), 0);
        assert_eq!(page_number_alignment(6, 2), 1);
    }

    #[test]
    fn 안쪽_바깥쪽은_홀짝에_따라_반전() {
        for outside in [7, 8] {
            assert_eq!(page_number_alignment(outside, 1), 1);
            assert_eq!(page_number_alignment(outside, 2), -1);
        }
        for inside in [9, 10] {
            assert_eq!(page_number_alignment(inside, 1), -1);
            assert_eq!(page_number_alignment(inside, 2), 1);
        }
    }

    #[test]
    fn 쪽번호_컨트롤은_자신의_문자모양을_사용() {
        let para = Paragraph {
            chars: vec![
                HwpChar::Text('앞'),
                HwpChar::ExtCtrl {
                    code: 3,
                    ctrl_id: *b"pgnp",
                    payload: Vec::new(),
                    ctrl_index: Some(1),
                },
            ],
            char_shape_runs: vec![(0, CharShapeId(2)), (1, CharShapeId(7))],
            ..Paragraph::default()
        };

        assert_eq!(control_char_shape(&para, 1), Some(CharShapeId(7)));
    }

    fn generic(ctrl_id: [u8; 4], data: Vec<u8>, paragraphs: Vec<Paragraph>) -> Control {
        Control::Generic(GenericControl {
            ctrl_id,
            data,
            paragraph_lists: if paragraphs.is_empty() {
                Vec::new()
            } else {
                vec![ParagraphList {
                    header_data: Vec::new(),
                    paragraphs,
                }]
            },
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        })
    }

    /// Hangul inserts page numbering wherever the caret was, so genuine documents
    /// carry `pgnp`/`nwno` inside a text box's paragraph list rather than in the
    /// body flow. The page-level state must still pick them up.
    #[test]
    fn 중첩_문단_리스트의_쪽번호_컨트롤도_적용된다() {
        let mut pgnp = vec![0u8; 12];
        pgnp[..4].copy_from_slice(&(5u32 << 8).to_le_bytes()); // Bottom center.
        pgnp[10..12].copy_from_slice(&(u16::from(b'-')).to_le_bytes());
        let mut nwno = vec![0u8; 8];
        nwno[4..6].copy_from_slice(&7u16.to_le_bytes());

        let inner = Paragraph {
            controls: vec![
                generic(*b"pgnp", pgnp, Vec::new()),
                generic(*b"nwno", nwno, Vec::new()),
            ],
            ..Paragraph::default()
        };
        let outer = Paragraph {
            controls: vec![generic(*b"gso ", Vec::new(), vec![inner])],
            ..Paragraph::default()
        };

        let mut warnings = RenderIssueAccumulator::new();
        let mut state = PageNumberState::new(1);
        state.apply_controls(&outer, &mut warnings);
        assert_eq!(
            state.placement.map(|p| p.position),
            Some(5),
            "중첩 문단의 pgnp도 쪽번호를 배치해야"
        );
        assert_eq!(
            state.visible_number(),
            Some(7),
            "중첩 nwno도 번호를 재시작해야"
        );
    }
}

#[cfg(test)]
mod para_geom_tests {
    use super::{ParaGeom, line_indents, para_geometry};
    use hwp_model::{Document, ParaShape, ParaShapeId, Paragraph};

    #[test]
    fn 문단_기하_단위변환() {
        let mut doc = Document::default();
        doc.header.para_shapes.push(ParaShape {
            margin_left: 4000,
            margin_right: 2000,
            indent: 3000,
            spacing_top: 1200,
            spacing_bottom: 600,
            ..ParaShape::default()
        });
        let para = Paragraph {
            para_shape: ParaShapeId(0),
            ..Paragraph::default()
        };
        let g = para_geometry(&doc, &para);
        // IR margins use twice HWPUNIT, so divide by 200; keep indentation separate from left.
        assert_eq!(g.left, 20.0); // margin_left 4000 / 200
        assert_eq!(g.first_indent, 15.0); // indent 3000 / 200
        assert_eq!(g.right, 10.0);
        assert_eq!(g.spacing_top, 6.0);
        assert_eq!(g.spacing_bottom, 3.0);
        // Preserve negative hanging indent: the first line is farther left; divide by 200.
        doc.header.para_shapes[0].indent = -1000;
        assert_eq!(para_geometry(&doc, &para).first_indent, -5.0);
        assert_eq!(para_geometry(&doc, &para).left, 20.0);
        // An out-of-range para_shape yields zero.
        let p2 = Paragraph {
            para_shape: ParaShapeId(99),
            ..Paragraph::default()
        };
        assert_eq!(para_geometry(&doc, &p2).left, 0.0);
    }

    #[test]
    fn 줄_들여쓰기는_내어쓰기_구간을_텍스트_영역_안에_둔다() {
        // Positive indent shifts only the first line; later lines start at the line box's left edge.
        let indent = ParaGeom {
            first_indent: 15.0,
            ..ParaGeom::default()
        };
        assert_eq!(line_indents(&indent, 0.0), (15.0, 0.0));
        // Negative indent without a marker: first line starts at the box left, later lines use the hanging offset.
        let hanging = ParaGeom {
            first_indent: -14.0,
            ..ParaGeom::default()
        };
        assert_eq!(line_indents(&hanging, 0.0), (0.0, 14.0));
        // With a marker, first-line text clears its width, at least the hanging-indent width.
        assert_eq!(line_indents(&hanging, 9.0), (14.0, 14.0));
        assert_eq!(line_indents(&hanging, 20.0), (20.0, 14.0));
        // Lists without hanging indent must still avoid overlapping their marker.
        assert_eq!(line_indents(&ParaGeom::default(), 9.0), (9.0, 0.0));
    }
}

#[cfg(test)]
mod diagonal_tests {
    use super::diagonal_dirs;

    #[test]
    fn 대각선_방향_비트() {
        // Both zero means no diagonals.
        assert_eq!(diagonal_dirs(0), (false, false));
        // Setting only 3D/shadow bits0-1 does not enable diagonals.
        assert_eq!(diagonal_dirs(0b11), (false, false));
        // slash(bit2~4) → `/`.
        assert_eq!(diagonal_dirs(0x4), (true, false));
        // backSlash(bit5~7) → `\`.
        assert_eq!(diagonal_dirs(0x20), (false, true));
        // Both diagonals, forming an X.
        assert_eq!(diagonal_dirs(0x4 | 0x20), (true, true));
    }
}

#[cfg(test)]
mod justify_tests {
    use super::*;
    use crate::fonts::LoadedFont;
    use crate::shape::Glyph;
    use std::sync::Arc;

    fn run(advs: &[f32]) -> InlineItem {
        run_t("", advs)
    }

    fn run_t(text: &str, advs: &[f32]) -> InlineItem {
        let glyphs: Vec<Glyph> = advs
            .iter()
            .map(|&a| Glyph {
                id: 0,
                x_advance: a,
                x_offset: 0.0,
                y_offset: 0.0,
            })
            .collect();
        InlineItem::Run(crate::shape::ShapedRun {
            font: Arc::new(LoadedFont {
                data: Arc::new(Vec::new()),
                index: 0,
                family: String::new(),
            }),
            size_pt: 10.0,
            x_scale: 1.0,
            color: 0,
            bold: false,
            italic: false,
            underline_kind: 0,
            underline_shape: 0,
            strike: false,
            strike_shape: 0,
            emphasis: 0,
            underline_color: 0xFFFF_FFFF,
            shade_color: 0xFFFF_FFFF,
            shadow: None,
            shadow_gap: (0, 0),
            outline: false,
            emboss: false,
            engrave: false,
            border_fill_id: 0,
            glyphs,
            width_pt: advs.iter().sum(),
            text: text.to_string(),
            start_wchar: 0,
        })
    }

    fn total_adv(items: &[InlineItem]) -> f32 {
        items
            .iter()
            .map(|i| match i {
                InlineItem::Run(r) => r.glyphs.iter().map(|g| g.x_advance).sum(),
                InlineItem::Tab | InlineItem::LineBreak(_) => 0.0,
            })
            .sum()
    }

    #[test]
    fn 양쪽정렬_잉여를_글자사이에_분배() {
        // Natural width 30, segment width 45: divide 15 slack equally over two non-final gaps (7.5 each).
        let mut items = vec![run(&[10.0, 10.0, 10.0])];
        let shift = align_line(&mut items, 0, 45.0, 30.0, false);
        assert_eq!(shift, 0.0);
        assert!(
            (total_adv(&items) - 45.0).abs() < 0.01,
            "줄이 seg_width를 채워야"
        );
        if let InlineItem::Run(r) = &items[0] {
            assert!((r.glyphs[0].x_advance - 17.5).abs() < 0.01);
            assert!(
                (r.glyphs[2].x_advance - 10.0).abs() < 0.01,
                "마지막 글리프는 불변"
            );
            assert!((r.width_pt - 45.0).abs() < 0.01, "width_pt 갱신");
        }
    }

    #[test]
    fn 공백이_있으면_공백에만_분배() {
        // Five glyphs for "ab cd"; assign all slack to the space at index 2.
        let mut items = vec![run_t("ab cd", &[10.0, 10.0, 5.0, 10.0, 10.0])];
        align_line(&mut items, 0, 60.0, 45.0, false); // slack 15
        if let InlineItem::Run(r) = &items[0] {
            assert!((r.glyphs[2].x_advance - 20.0).abs() < 0.01, "공백 5+15=20");
            assert!((r.glyphs[0].x_advance - 10.0).abs() < 0.01, "글자 불변");
            assert!((r.glyphs[4].x_advance - 10.0).abs() < 0.01, "글자 불변");
        }
        assert!((total_adv(&items) - 60.0).abs() < 0.01);
    }

    #[test]
    fn 후행_공백엔_분배안함() {
        // A trailing space in "ab ": distribute over visible glyphs as if no space, reaching the line end.
        let mut items = vec![run_t("ab ", &[10.0, 10.0, 5.0])];
        align_line(&mut items, 0, 40.0, 25.0, false); // slack 15
        if let InlineItem::Run(r) = &items[0] {
            // Exclude trailing space at index 2; last_visible=1 leaves one gap, giving index 0 all 15 slack.
            assert!(
                (r.glyphs[0].x_advance - 25.0).abs() < 0.01,
                "{}",
                r.glyphs[0].x_advance
            );
            assert!((r.glyphs[2].x_advance - 5.0).abs() < 0.01, "후행 공백 불변");
        }
    }

    #[test]
    fn 마지막_줄은_늘리지_않음() {
        let mut items = vec![run(&[10.0, 10.0, 10.0])];
        align_line(&mut items, 0, 45.0, 30.0, true);
        assert!(
            (total_adv(&items) - 30.0).abs() < 0.01,
            "마지막 줄은 ragged 유지"
        );
    }

    #[test]
    fn 가운데_오른쪽은_shift만() {
        let mut center = vec![run(&[10.0, 10.0])];
        assert!((align_line(&mut center, 3, 40.0, 20.0, false) - 10.0).abs() < 0.01);
        assert!(
            (total_adv(&center) - 20.0).abs() < 0.01,
            "가운데는 advance 불변"
        );
        let mut right = vec![run(&[10.0, 10.0])];
        assert!((align_line(&mut right, 2, 40.0, 20.0, false) - 20.0).abs() < 0.01);
    }

    #[test]
    fn distribute_includes_the_trailing_gap() {
        // Split 15pt of slack across three gaps, including the trailing gap.
        let mut items = vec![run(&[10.0, 10.0, 10.0])];
        let shift = align_line(&mut items, 4, 45.0, 30.0, false);
        assert_eq!(shift, 0.0);
        if let InlineItem::Run(r) = &items[0] {
            for g in &r.glyphs {
                assert!((g.x_advance - 15.0).abs() < 0.01);
            }
        }
        assert!((total_adv(&items) - 45.0).abs() < 0.01);
    }

    #[test]
    fn distribute_stretches_the_final_line() {
        // Unlike ordinary justification, distribute also stretches the final line.
        let mut items = vec![run(&[10.0, 10.0, 10.0])];
        align_line(&mut items, 4, 45.0, 30.0, true);
        assert!(
            (total_adv(&items) - 45.0).abs() < 0.01,
            "distribute should fill seg_width on the final line"
        );
    }

    #[test]
    fn divide_excludes_the_trailing_gap() {
        // Split 15pt of slack across the two gaps before the final glyph.
        let mut items = vec![run(&[10.0, 10.0, 10.0])];
        align_line(&mut items, 5, 45.0, 30.0, false);
        if let InlineItem::Run(r) = &items[0] {
            assert!((r.glyphs[0].x_advance - 17.5).abs() < 0.01);
            assert!((r.glyphs[1].x_advance - 17.5).abs() < 0.01);
            assert!(
                (r.glyphs[2].x_advance - 10.0).abs() < 0.01,
                "the trailing gap must remain unchanged"
            );
        }
        assert!((total_adv(&items) - 45.0).abs() < 0.01);
    }

    #[test]
    fn divide_stretches_the_final_line_without_space_priority() {
        // The final line is stretched and whitespace receives no priority.
        let mut items = vec![run_t("a b", &[10.0, 5.0, 10.0])];
        align_line(&mut items, 5, 40.0, 25.0, true);
        if let InlineItem::Run(r) = &items[0] {
            assert!((r.glyphs[0].x_advance - 17.5).abs() < 0.01);
            assert!(
                (r.glyphs[1].x_advance - 12.5).abs() < 0.01,
                "whitespace should receive one equal gap share"
            );
            assert!((r.glyphs[2].x_advance - 10.0).abs() < 0.01);
        }
        assert!(
            (total_adv(&items) - 40.0).abs() < 0.01,
            "divide should fill seg_width on the final line"
        );
    }

    #[test]
    fn distribute_uses_all_slack_on_a_short_line() {
        let mut items = vec![run(&[10.0, 10.0])];
        align_line(&mut items, 4, 100.0, 20.0, false);
        assert!((total_adv(&items) - 100.0).abs() < 0.01);
    }

    #[test]
    fn distribute_can_stretch_a_single_glyph_via_its_trailing_gap() {
        let mut items = vec![run(&[10.0])];
        align_line(&mut items, 4, 30.0, 10.0, false);
        assert!((total_adv(&items) - 30.0).abs() < 0.01);
    }
}

#[cfg(test)]
mod table_width_tests {
    use super::*;
    use hwp_model::{BorderFillId, Cell, HwpUnit};

    fn cell(col: u16, row: u16, col_span: u16, width: i32) -> Cell {
        Cell {
            list_attr: 0,
            col,
            row,
            col_span,
            row_span: 1,
            width: HwpUnit(width),
            height: HwpUnit(1800),
            margins: [0; 4],
            border_fill: BorderFillId(1),
            header_tail: Vec::new(),
            paragraphs: Vec::new(),
        }
    }

    fn table(rows: u16, cols: u16, cells: Vec<Cell>) -> Table {
        Table {
            common_data: Vec::new(),
            placement: None,
            attr: 0,
            rows,
            cols,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: Vec::new(),
            border_fill: BorderFillId(1),
            table_tail: Vec::new(),
            cells,
            caption: None,
            extras: Vec::new(),
        }
    }

    /// Columns covered only by merged cells must derive widths from those cells (no col_span==1 cell).
    #[test]
    fn 병합_열_폭_유도() {
        // row0: [col0 span2 w=200pt][col2 span1 w=100pt]
        // row1: [col0 span1 w=80pt][col1 span2 w=220pt]; only the merged cell defines col1.
        let cells = vec![
            cell(0, 0, 2, 20000),
            cell(2, 0, 1, 10000),
            cell(0, 1, 1, 8000),
            cell(1, 1, 2, 22000),
        ];
        let t = table(2, 3, cells);
        // Initialize from col_span==1 cells, matching layout_table.
        let mut col_w = vec![0.0f32; 3];
        for c in &t.cells {
            if c.col_span == 1 {
                col_w[c.col as usize] = col_w[c.col as usize].max(c.width.to_pt() as f32);
            }
        }
        assert_eq!(col_w, vec![80.0, 0.0, 100.0]); // col1 is unknown.

        derive_col_widths(&mut col_w, &t, f32::MAX); // Do not trigger the cap.
        assert!((col_w[1] - 120.0).abs() < 0.01, "col1 유도값: {col_w:?}");
        assert!(col_w.iter().all(|w| *w > 0.0), "미지 열 없어야: {col_w:?}");
        assert!(
            (col_w.iter().sum::<f32>() - 300.0).abs() < 0.5,
            "총 폭=표 실제 폭 300pt: {col_w:?}"
        );
    }

    /// Normal tables containing only col_span==1 cells must preserve their widths (scale s=1).
    #[test]
    fn 정상_표_열폭_불변() {
        let cells = vec![
            cell(0, 0, 1, 10000),
            cell(1, 0, 1, 15000),
            cell(2, 0, 1, 5000),
        ];
        let t = table(1, 3, cells);
        let mut col_w = vec![100.0f32, 150.0, 50.0];
        derive_col_widths(&mut col_w, &t, f32::MAX); // Do not trigger the cap.
        assert_eq!(col_w, vec![100.0, 150.0, 50.0]);
    }

    /// Shrink a 300pt actual table width to 150pt available width while preserving proportions.
    #[test]
    fn 본문폭_초과_표_축소() {
        let cells = vec![
            cell(0, 0, 1, 10000),
            cell(1, 0, 1, 15000),
            cell(2, 0, 1, 5000),
        ];
        let t = table(1, 3, cells); // Actual width is 300pt.
        let mut col_w = vec![100.0f32, 150.0, 50.0];
        derive_col_widths(&mut col_w, &t, 150.0);
        let sum: f32 = col_w.iter().sum();
        assert!((sum - 150.0).abs() < 0.5, "가용 폭 150pt로 축소: {col_w:?}");
        // Preserve relative proportions 2:3:1.
        assert!((col_w[0] - 50.0).abs() < 0.5, "{col_w:?}");
        assert!((col_w[1] - 75.0).abs() < 0.5, "{col_w:?}");
        assert!((col_w[2] - 25.0).abs() < 0.5, "{col_w:?}");
    }

    #[test]
    fn max_cell_margin_is_inherited_without_mutating_the_ir() {
        let mut c = cell(0, 0, 1, 10000);
        c.margins = [u16::MAX, 0, u16::MAX, u16::MAX];
        let t = table(1, 1, vec![c.clone()]);
        let margins = cell_margins(
            &Table {
                inner_margins: [700, 800, 900, 1000],
                ..t.clone()
            },
            &c,
        );
        assert_eq!(margins, (7.0, 0.0, 9.0, 10.0));
        assert_eq!(c.margins, [u16::MAX, 0, u16::MAX, u16::MAX]);

        let defaults = Table {
            inner_margins: [0; 4],
            ..t
        };
        assert_eq!(cell_margins(&defaults, &c), (5.1, 0.0, 1.41, 1.41));
    }
}

#[cfg(test)]
mod certification_budget_tests {
    use super::*;
    use hwp_model::{
        BinRef, BinStream, GenericControl, GradientSpec, HwpChar, LineSeg, OpaqueRecord, Picture,
        ShapeGeom, ShapeKind,
    };

    fn assert_budget_failure(document: &Document, budget: LayoutBudget) {
        let mut store = FontStore::new_isolated();
        let mut issues = RenderIssueAccumulator::new();
        let error = match layout_document_bounded(document, &mut store, &mut issues, &budget) {
            Ok(_) => panic!("budget must fail"),
            Err(error) => error,
        };
        assert!(matches!(error, RenderError::LayoutBudgetExceeded { .. }));
        assert!(issues.finish().issues.iter().any(|issue| {
            issue.code == RenderIssueCode::LayoutBudgetExceeded
                && issue.severity == crate::issues::RenderIssueSeverity::Fatal
        }));
    }

    fn generic(shapes: Vec<ShapeGeom>, raw_children: Vec<OpaqueRecord>) -> Control {
        Control::Generic(GenericControl {
            ctrl_id: *b"gso ",
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children,
            gso_shapes: shapes,
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        })
    }

    fn rect() -> ShapeGeom {
        ShapeGeom {
            kind: ShapeKind::Rect,
            x: 0,
            y: 0,
            w: 100,
            h: 100,
            points: Vec::new(),
            fill: 0,
            fill_gradient: None,
            border_color: 0,
            border_width: 1,
            round_ratio: 0,
            border_style: 0,
            arrow_start: 0,
            arrow_end: 0,
            anchored: false,
            description: None,
            image_fill_dropped: false,
        }
    }

    #[test]
    fn 백만_미만_대량_item도_사전_예산에서_중단() {
        let mut document = hwp_convert::from_markdown("x");
        document.sections[0].paragraphs[0].chars =
            (0..100_000).map(|_| HwpChar::Text('a')).collect();
        let mut budget = LayoutBudget::certification();
        budget.max_display_items = 50_000;
        budget.max_estimated_work = 300_000;
        assert_budget_failure(&document, budget);
    }

    #[test]
    fn 십만_page_reset은_페이지_할당_전에_중단() {
        let mut document = hwp_convert::from_markdown("x");
        document.sections[0].paragraphs[0].line_segs = (0..100_000)
            .map(|index| LineSeg {
                text_start: 0,
                v_pos: if index % 2 == 0 { 1 } else { 0 },
                line_height: 1_000,
                text_height: 1_000,
                baseline_gap: 850,
                line_spacing: 600,
                col_start: 0,
                seg_width: 42_000,
                flags: 0,
            })
            .collect();
        assert_budget_failure(&document, LayoutBudget::certification());
    }

    #[test]
    fn 같은_대형_bindata_반복_참조는_reference_quota로_중단() {
        let mut document = hwp_convert::from_markdown("x");
        document.bin_streams.push(BinStream {
            name: "shared-image".to_string(),
            data: vec![0; 1024 * 1024],
        });
        let picture = Control::Picture(Picture {
            common_data: Vec::new(),
            width: HwpUnit(100),
            height: HwpUnit(100),
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
            bin_ref: BinRef::ItemRef("shared-image".to_string()),
            extras: Vec::new(),
        });
        document.sections[0].paragraphs[0].controls = vec![picture; 300];
        assert_budget_failure(&document, LayoutBudget::certification());
    }

    /// GG-15: picture-effect flags produce a typed warning instead of a silent omission.
    #[test]
    fn 그림효과_플래그는_타입화된_보고() {
        let mut document = hwp_convert::from_markdown("x");
        document.bin_streams.push(BinStream {
            name: "fx-image".to_string(),
            data: vec![0; 16],
        });
        let picture = Control::Picture(Picture {
            common_data: Vec::new(),
            width: HwpUnit(100),
            height: HwpUnit(100),
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
            effect_flags: 0x1, // Shadow effect.
            effects_raw: vec![1, 0, 0, 0],
            caption: None,
            bin_ref: BinRef::ItemRef("fx-image".to_string()),
            extras: Vec::new(),
        });
        document.sections[0].paragraphs[0].controls = vec![picture];
        let mut store = FontStore::new();
        let mut warns = RenderIssueAccumulator::new();
        let _ = layout_document(&document, &mut store, &mut warns);
        let report = warns.finish();
        let issue = report
            .issues
            .iter()
            .find(|i| i.code == RenderIssueCode::PictureEffectsUnsupported)
            .expect("효과 미지원 보고");
        assert_eq!(issue.severity, crate::issues::RenderIssueSeverity::Warning);

        // Zero flags produce no warning.
        let document2 = hwp_convert::from_markdown("x");
        let mut warns2 = RenderIssueAccumulator::new();
        let _ = layout_document(&document2, &mut store, &mut warns2);
        assert!(
            !warns2
                .finish()
                .issues
                .iter()
                .any(|i| i.code == RenderIssueCode::PictureEffectsUnsupported)
        );
    }

    #[test]
    fn 점이_없는_도형_십만개도_path_할당_전에_중단() {
        let mut document = hwp_convert::from_markdown("x");
        document.sections[0].paragraphs[0].controls =
            vec![generic(vec![rect(); 100_000], Vec::new())];
        assert_budget_failure(&document, LayoutBudget::certification());
    }

    #[test]
    fn 깊은_raw_record는_재귀_렌더_전에_중단() {
        let mut record = OpaqueRecord {
            tag: 0x56,
            data: Vec::new(),
            children: Vec::new(),
        };
        for _ in 0..65 {
            record = OpaqueRecord {
                tag: 0x56,
                data: Vec::new(),
                children: vec![record],
            };
        }
        let mut document = hwp_convert::from_markdown("x");
        document.sections[0].paragraphs[0].controls = vec![generic(Vec::new(), vec![record])];
        assert_budget_failure(&document, LayoutBudget::certification());
    }

    #[test]
    fn shape_points와_gradient_stops는_각각_할당량으로_제한() {
        let mut points_document = hwp_convert::from_markdown("x");
        let mut point_shape = rect();
        point_shape.kind = ShapeKind::Polygon;
        point_shape.points = vec![(0, 0); 101];
        points_document.sections[0].paragraphs[0].controls =
            vec![generic(vec![point_shape], Vec::new())];
        let mut point_budget = LayoutBudget::certification();
        point_budget.max_shape_points = 100;
        assert_budget_failure(&points_document, point_budget);

        let mut stops_document = hwp_convert::from_markdown("x");
        let mut gradient_shape = rect();
        gradient_shape.fill_gradient = Some(GradientSpec {
            radial: false,
            angle_deg: 0.0,
            center_x: 0,
            center_y: 0,
            step: 255,
            stops: vec![(0.5, 0); 101],
        });
        stops_document.sections[0].paragraphs[0].controls =
            vec![generic(vec![gradient_shape], Vec::new())];
        let mut stop_budget = LayoutBudget::certification();
        stop_budget.max_gradient_stops = 100;
        assert_budget_failure(&stops_document, stop_budget);
    }

    #[test]
    fn 자동_넘침_이만_문단은_page_push_전에_중단() {
        let mut document = hwp_convert::from_markdown("");
        document.bin_streams.push(BinStream {
            name: "one-byte-image".to_string(),
            data: vec![0],
        });
        let picture = Control::Picture(Picture {
            common_data: Vec::new(),
            width: HwpUnit(100),
            height: HwpUnit(100_000),
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
            bin_ref: BinRef::ItemRef("one-byte-image".to_string()),
            extras: Vec::new(),
        });
        let mut paragraph = document.sections[0].paragraphs[0].clone();
        paragraph.controls = vec![picture];
        document.sections[0].paragraphs = vec![paragraph; 20_000];
        assert_budget_failure(&document, LayoutBudget::certification());
    }
}

/// #254: the paragraph-level rendered-control predicate must classify a
/// bookmark (`bokm`) or hyperlink (`%hlk`) control as rendered, because both
/// are legitimately not separate page items - a bookmark is an invisible
/// marker by design and a hyperlink's text is already styled by the
/// hyperlink range pass (`shape.rs::hyperlink_ranges`). A negative control
/// (a `ctrl_id` in none of the predicate's arms) proves the counter still
/// fires for a genuinely unsupported control, so this change narrows the
/// counter rather than disabling it.
#[cfg(test)]
mod control_classification_tests {
    use super::*;
    use hwp_model::GenericControl;

    fn generic(ctrl_id: [u8; 4]) -> Control {
        Control::Generic(GenericControl {
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
        })
    }

    fn omitted_control_issue_present_for(ctrl_ids: &[[u8; 4]]) -> bool {
        let mut document = hwp_convert::from_markdown("x");
        document.sections[0].paragraphs[0].controls =
            ctrl_ids.iter().map(|id| generic(*id)).collect();
        let mut store = FontStore::new();
        let mut warns = RenderIssueAccumulator::new();
        let _ = layout_document(&document, &mut store, &mut warns);
        warns
            .finish()
            .issues
            .iter()
            .any(|i| i.code == RenderIssueCode::UnsupportedControlOmitted)
    }

    fn omitted_control_issue_present(ctrl_id: [u8; 4]) -> bool {
        omitted_control_issue_present_for(&[ctrl_id])
    }

    #[test]
    fn 북마크_전용_문단은_생략_보고를_내지_않는다() {
        assert!(
            !omitted_control_issue_present(*b"bokm"),
            "bookmark is an invisible marker by design; it must not be counted as omitted"
        );
    }

    #[test]
    fn 하이퍼링크_전용_문단은_생략_보고를_내지_않는다() {
        assert!(
            !omitted_control_issue_present(*b"%hlk"),
            "a hyperlink's text is already drawn and styled by the hyperlink range pass; \
             it must not be counted as omitted"
        );
    }

    /// Guards against a widened fix: a control genuinely neither drawn nor
    /// deliberately invisible must still increment the skipped-control
    /// counter and still raise `UnsupportedControlOmitted`.
    #[test]
    fn 미지원_컨트롤은_여전히_생략_보고를_낸다() {
        assert!(
            omitted_control_issue_present(*b"zzzz"),
            "a control in none of the predicate's arms must still count as omitted"
        );
    }

    /// A paragraph mixing a now-classified-as-rendered control (bookmark) with
    /// a genuinely unsupported one must still raise the issue: the narrowed
    /// classification must not mask a real omission sitting alongside it.
    #[test]
    fn 북마크와_미지원_컨트롤이_섞인_문단은_여전히_생략_보고를_낸다() {
        assert!(
            omitted_control_issue_present_for(&[*b"bokm", *b"zzzz"]),
            "a genuinely unsupported control must still be flagged even when a \
             classified-as-rendered control shares the paragraph"
        );
    }
}

#[cfg(test)]
mod tab_width_tests {
    use std::sync::Arc;

    use super::{LEADER_DASH_SCALE, items_width, place_wrapped};
    use crate::display::{Item, PageList, PathCmd};
    use crate::fonts::LoadedFont;
    use crate::issues::RenderIssueAccumulator;
    use crate::shape::{Glyph, InlineItem, ShapedRun};
    use crate::tab::TabStop;

    /// Creates a font-independent dummy run whose only meaningful field is width.
    fn dummy_run(width_pt: f32) -> InlineItem {
        InlineItem::Run(ShapedRun {
            font: Arc::new(LoadedFont {
                data: Arc::new(Vec::new()),
                index: 0,
                family: String::new(),
            }),
            size_pt: 10.0,
            x_scale: 1.0,
            color: 0,
            bold: false,
            italic: false,
            underline_kind: 0,
            underline_shape: 0,
            strike: false,
            strike_shape: 0,
            emphasis: 0,
            underline_color: 0xFFFF_FFFF,
            shade_color: 0xFFFF_FFFF,
            shadow: None,
            shadow_gap: (0, 0),
            outline: false,
            emboss: false,
            engrave: false,
            border_fill_id: 0,
            glyphs: Vec::new(),
            width_pt,
            text: String::new(),
            start_wchar: 0,
        })
    }

    fn dummy_glyph_run(text: &str) -> InlineItem {
        let mut item = dummy_run(10.0);
        let InlineItem::Run(run) = &mut item else {
            unreachable!();
        };
        run.text = text.to_string();
        run.glyphs.push(Glyph {
            id: 1,
            x_advance: 10.0,
            x_offset: 0.0,
            y_offset: 0.0,
        });
        item
    }

    /// A glyph run with one glyph per `char` of `text`, each carrying the matching entry of
    /// `glyph_widths` as its `x_advance` — for tests that need to know exactly where a
    /// specific character (e.g. a decimal separator) starts. Font data stays empty so
    /// `glyph_source_sequences`'s rustybuzz path fails closed to its byte-count fallback,
    /// keeping the character-to-glyph mapping font-independent (see `distribute_source`).
    fn dummy_glyph_run_with_widths(text: &str, glyph_widths: &[f32]) -> InlineItem {
        let mut item = dummy_run(glyph_widths.iter().sum());
        let InlineItem::Run(run) = &mut item else {
            unreachable!();
        };
        run.text = text.to_string();
        run.glyphs = glyph_widths
            .iter()
            .map(|&x_advance| Glyph {
                id: 1,
                x_advance,
                x_offset: 0.0,
                y_offset: 0.0,
            })
            .collect();
        item
    }

    /// Places `items` at `x0 = 0.0`, `tabs`, and returns the x of each placed `Item::Glyphs`
    /// entry in order, plus the page for further (e.g. leader-path) inspection.
    fn place_at_origin(items: Vec<InlineItem>, tabs: &[TabStop]) -> (Vec<f32>, PageList) {
        let mut page = PageList {
            width_pt: 600.0,
            height_pt: 800.0,
            items: Vec::new(),
        };
        let mut warns = RenderIssueAccumulator::new();
        place_wrapped(
            &mut page,
            items,
            0.0,
            100.0,
            f32::INFINITY,
            16.0,
            tabs,
            0.0,
            &[],
            None,
            &mut warns,
        );
        let xs = page
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Glyphs { x, .. } => Some(*x),
                _ => None,
            })
            .collect();
        (xs, page)
    }

    /// Explicit tab stops must advance to the same x position in width
    /// estimation and final placement.
    #[test]
    fn explicit_tab_stops_match_width_estimation_and_placement() {
        let tabs = [TabStop {
            pos_pt: 150.0,
            kind: 0,
            fill: 0,
        }];
        let make_items = || vec![dummy_run(10.0), InlineItem::Tab, dummy_run(20.0)];

        // Width estimate: 10 + tab to 150 + 20 = 170.
        let natural = items_width(&make_items(), &tabs);
        assert!((natural - 170.0).abs() < 0.01, "natural={natural}");

        // Placement uses the same explicit stop at x=150.
        let mut page = PageList {
            width_pt: 600.0,
            height_pt: 800.0,
            items: Vec::new(),
        };
        let mut warns = RenderIssueAccumulator::new();
        place_wrapped(
            &mut page,
            make_items(),
            0.0,
            100.0,
            f32::INFINITY,
            16.0,
            &tabs,
            0.0,
            &[],
            None,
            &mut warns,
        );
        let xs: Vec<f32> = page
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Glyphs { x, .. } => Some(*x),
                _ => None,
            })
            .collect();
        assert_eq!(xs.len(), 2);
        assert!((xs[1] - 150.0).abs() < 0.01, "x after tab={}", xs[1]);
        // The estimated and placed positions after the tab match exactly.
        assert!((xs[1] - (natural - 20.0)).abs() < 0.01);

        // With no explicit stop, retain the existing 40 pt fallback interval.
        let natural_default = items_width(&make_items(), &[]);
        assert!((natural_default - (40.0 + 20.0)).abs() < 0.01);
    }

    #[test]
    fn grouped_space_suppresses_adjacent_wraps_across_runs() {
        let mut page = PageList {
            width_pt: 600.0,
            height_pt: 800.0,
            items: Vec::new(),
        };
        let mut warns = RenderIssueAccumulator::new();
        place_wrapped(
            &mut page,
            vec![
                dummy_glyph_run("a"),
                dummy_glyph_run("\u{00a0}"),
                dummy_glyph_run("b"),
            ],
            0.0,
            100.0,
            15.0,
            16.0,
            &[],
            0.0,
            &[],
            None,
            &mut warns,
        );

        let baselines: Vec<f32> = page
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Glyphs { y, .. } => Some(*y),
                _ => None,
            })
            .collect();
        assert_eq!(baselines, [100.0, 100.0, 100.0]);
    }

    /// A tab landing on a leadered stop emits exactly one dashed `Item::Path` between the
    /// pre-tab and post-tab x positions at the run baseline. The leader style code passes
    /// through `hwp5_line_style` before `dash_pattern` (fill=2 is HWPX DASH, which
    /// `hwp5_line_style` maps to dash_pattern style 1).
    #[test]
    fn leadered_tab_emits_a_dashed_path_between_the_two_x_positions() {
        let tabs = [TabStop {
            pos_pt: 150.0,
            kind: 0,
            fill: 2,
        }];
        let items = vec![dummy_run(10.0), InlineItem::Tab, dummy_run(20.0)];

        let mut page = PageList {
            width_pt: 600.0,
            height_pt: 800.0,
            items: Vec::new(),
        };
        let mut warns = RenderIssueAccumulator::new();
        place_wrapped(
            &mut page,
            items,
            0.0,
            100.0,
            f32::INFINITY,
            16.0,
            &tabs,
            0.0,
            &[],
            None,
            &mut warns,
        );

        let paths: Vec<(Vec<PathCmd>, f32, Vec<f32>)> = page
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Path {
                    commands,
                    stroke: Some(stroke),
                    ..
                } => Some((commands.clone(), stroke.width, stroke.dash.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            paths.len(),
            1,
            "expected exactly one leader path: {paths:?}"
        );
        let (commands, width, dash) = &paths[0];
        match commands.as_slice() {
            [PathCmd::MoveTo(x0, _), PathCmd::LineTo(x1, _)] => {
                assert!((x0 - 10.0).abs() < 0.01, "pre-tab x={x0}");
                assert!((x1 - 150.0).abs() < 0.01, "post-tab x={x1}");
            }
            other => panic!("unexpected leader path commands: {other:?}"),
        }
        assert!((width - 0.5).abs() < 0.01);
        // The mark stays a 0.5pt stroke, but the pattern is scaled by
        // LEADER_DASH_SCALE (leader pitch is typographic, not stroke-proportional):
        // hwp5_line_style(2) = 1 (dash) -> dash_pattern(1, 1.04) = [3.12, 2.08].
        assert_eq!(dash.len(), 2);
        assert!(
            (dash[0] - 3.0 * LEADER_DASH_SCALE).abs() < 0.001,
            "on={dash:?}"
        );
        assert!(
            (dash[1] - 2.0 * LEADER_DASH_SCALE).abs() < 0.001,
            "off={dash:?}"
        );
    }

    /// A tab landing on a stop with fill=0 (NONE) emits no `Item::Path` at all.
    #[test]
    fn tab_with_no_leader_emits_no_path() {
        let tabs = [TabStop {
            pos_pt: 150.0,
            kind: 0,
            fill: 0,
        }];
        let items = vec![dummy_run(10.0), InlineItem::Tab, dummy_run(20.0)];

        let mut page = PageList {
            width_pt: 600.0,
            height_pt: 800.0,
            items: Vec::new(),
        };
        let mut warns = RenderIssueAccumulator::new();
        place_wrapped(
            &mut page,
            items,
            0.0,
            100.0,
            f32::INFINITY,
            16.0,
            &tabs,
            0.0,
            &[],
            None,
            &mut warns,
        );

        let path_count = page
            .items
            .iter()
            .filter(|it| matches!(it, Item::Path { .. }))
            .count();
        assert_eq!(path_count, 0, "an unleadered tab must emit no path");
    }

    /// A right tab (kind 1) ends the following segment at the stop rather than starting
    /// there. The leader (fill != 0) spans from the pre-tab x to where the text actually
    /// begins — not to the raw stop position.
    #[test]
    fn right_tab_ends_the_segment_at_the_stop() {
        let tabs = [TabStop {
            pos_pt: 100.0,
            kind: 1,
            fill: 1,
        }];
        let items = vec![dummy_run(10.0), InlineItem::Tab, dummy_run(20.0)];
        let (xs, page) = place_at_origin(items, &tabs);

        assert_eq!(xs.len(), 2);
        assert!((xs[1] - 80.0).abs() < 0.01, "segment start={}", xs[1]);

        let leaders: Vec<(f32, f32)> = page
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Path { commands, .. } => match commands.as_slice() {
                    [PathCmd::MoveTo(x0, _), PathCmd::LineTo(x1, _)] => Some((*x0, *x1)),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(leaders.len(), 1, "expected one leader path: {leaders:?}");
        assert!(
            (leaders[0].0 - 10.0).abs() < 0.01,
            "leader start={:?}",
            leaders[0]
        );
        assert!(
            (leaders[0].1 - 80.0).abs() < 0.01,
            "leader must stop where the text begins (80.0), not at the stop (100.0): {:?}",
            leaders[0]
        );
    }

    /// A center tab (kind 2) centers the following segment on the stop, so it can extend
    /// past the stop on the right just as far as it starts before it on the left.
    #[test]
    fn center_tab_centers_the_segment_on_the_stop() {
        let tabs = [TabStop {
            pos_pt: 100.0,
            kind: 2,
            fill: 0,
        }];
        let items = vec![dummy_run(10.0), InlineItem::Tab, dummy_run(20.0)];
        let (xs, _page) = place_at_origin(items, &tabs);

        assert_eq!(xs.len(), 2);
        assert!((xs[1] - 90.0).abs() < 0.01, "segment start={}", xs[1]);
    }

    /// A decimal tab (kind 3) aligns the segment so its first decimal separator sits on the
    /// stop, not the segment's start or end.
    #[test]
    fn decimal_tab_aligns_the_separator_on_the_stop() {
        let tabs = [TabStop {
            pos_pt: 100.0,
            kind: 3,
            fill: 0,
        }];
        let items = vec![
            dummy_run(10.0),
            InlineItem::Tab,
            dummy_glyph_run_with_widths("12.5", &[5.0, 5.0, 5.0, 5.0]),
        ];
        let (xs, _page) = place_at_origin(items, &tabs);

        assert_eq!(xs.len(), 2);
        // The "." is the third character (glyph index 2); 5.0 + 5.0 = 10.0 of width precedes
        // it, so the segment must start at 100.0 - 10.0 = 90.0 for the "." to land at 100.0.
        assert!((xs[1] - 90.0).abs() < 0.01, "segment start={}", xs[1]);
        assert!(
            (xs[1] + 10.0 - 100.0).abs() < 0.01,
            "the '.' glyph must sit on the stop"
        );
    }

    /// A decimal tab whose segment carries no decimal separator behaves exactly like a right
    /// tab — the whole segment ends at the stop.
    #[test]
    fn decimal_tab_without_a_separator_behaves_as_right_tab() {
        let tabs = [TabStop {
            pos_pt: 100.0,
            kind: 3,
            fill: 0,
        }];
        let items = vec![
            dummy_run(10.0),
            InlineItem::Tab,
            dummy_glyph_run_with_widths("125", &[5.0, 5.0, 5.0]),
        ];
        let (xs, _page) = place_at_origin(items, &tabs);

        assert_eq!(xs.len(), 2);
        assert!((xs[1] - 85.0).abs() < 0.01, "segment start={}", xs[1]);
    }

    /// A left tab (kind 0) places the following segment starting at the stop, exactly as it
    /// did before kind-aware placement existed.
    #[test]
    fn left_tab_placement_is_unchanged() {
        let tabs = [TabStop {
            pos_pt: 100.0,
            kind: 0,
            fill: 0,
        }];
        let items = vec![dummy_run(10.0), InlineItem::Tab, dummy_run(20.0)];
        let (xs, _page) = place_at_origin(items, &tabs);

        assert_eq!(xs.len(), 2);
        assert!((xs[1] - 100.0).abs() < 0.01, "segment start={}", xs[1]);
    }

    /// A segment wider than the distance back to the stop cannot push text to a negative x
    /// or behind the paragraph's left edge (T-1-08) — it falls back to the current x.
    #[test]
    fn oversized_segment_does_not_move_text_left_of_the_current_x() {
        let tabs = [TabStop {
            pos_pt: 100.0,
            kind: 1,
            fill: 0,
        }];
        let items = vec![dummy_run(10.0), InlineItem::Tab, dummy_run(200.0)];
        let (xs, _page) = place_at_origin(items, &tabs);

        assert_eq!(xs.len(), 2);
        assert!(
            (xs[1] - 10.0).abs() < 0.01,
            "oversized segment must stay at the current x, got {}",
            xs[1]
        );
        assert!(xs[1] >= 0.0);
    }
}

#[cfg(test)]
mod decor_tests {
    use std::sync::Arc;

    use hwp_model::{BorderFill, BorderLine};

    use super::{push_run, wave_commands};
    use crate::display::{Fill, Item, PageList, PathCmd};
    use crate::fonts::LoadedFont;
    use crate::issues::RenderIssueAccumulator;
    use crate::shape::{Glyph, ShapedRun};

    /// Builds a synthetic 10pt run with `adv_count` glyphs of 10pt advance.
    fn dummy_run(text: &str, adv_count: usize) -> ShapedRun {
        let glyphs: Vec<Glyph> = (0..adv_count)
            .map(|_| Glyph {
                id: 0,
                x_advance: 10.0,
                x_offset: 0.0,
                y_offset: 0.0,
            })
            .collect();
        ShapedRun {
            font: Arc::new(LoadedFont {
                data: Arc::new(Vec::new()),
                index: 0,
                family: String::new(),
            }),
            size_pt: 10.0,
            x_scale: 1.0,
            color: 0,
            bold: false,
            italic: false,
            underline_kind: 0,
            underline_shape: 0,
            strike: false,
            strike_shape: 0,
            emphasis: 0,
            underline_color: 0xFFFF_FFFF,
            shade_color: 0xFFFF_FFFF,
            shadow: None,
            shadow_gap: (0, 0),
            outline: false,
            emboss: false,
            engrave: false,
            border_fill_id: 0,
            width_pt: glyphs.iter().map(|g| g.x_advance).sum(),
            glyphs,
            text: text.to_string(),
            start_wchar: 0,
        }
    }

    fn place(run: ShapedRun, border_fills: &[BorderFill]) -> PageList {
        let mut page = PageList {
            width_pt: 600.0,
            height_pt: 800.0,
            items: Vec::new(),
        };
        let mut warns = RenderIssueAccumulator::new();
        push_run(&mut page, 100.0, 200.0, run, border_fills, &mut warns);
        page
    }

    /// Summarizes path start y, dash presence, and stroke width.
    fn path_summary(page: &PageList) -> Vec<(f32, bool, f32)> {
        page.items
            .iter()
            .filter_map(|it| match it {
                Item::Path {
                    commands,
                    stroke: Some(st),
                    ..
                } => match commands[0] {
                    PathCmd::MoveTo(_, y) => Some((y, !st.dash.is_empty(), st.width)),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    #[test]
    fn emits_underlines_below_and_above() {
        // Kind 1: below at y + 0.10em = 201.
        let mut run = dummy_run("가", 1);
        run.underline_kind = 1;
        let page = place(run, &[]);
        let paths = path_summary(&page);
        assert_eq!(paths.len(), 1);
        assert!(
            (paths[0].0 - 201.0).abs() < 0.01,
            "lower underline y: {paths:?}"
        );
        assert!(!paths[0].1, "default is solid");
        assert!((paths[0].2 - 0.5).abs() < 0.01, "width is 0.05em");

        // Kind 3: above at the provisional ascent y - 0.80em = 192.
        let mut run = dummy_run("가", 1);
        run.underline_kind = 3;
        let page = place(run, &[]);
        let paths = path_summary(&page);
        assert_eq!(paths.len(), 1);
        assert!(
            (paths[0].0 - 192.0).abs() < 0.01,
            "upper underline y: {paths:?}"
        );

        // An explicit underline color overrides the text color.
        let mut run = dummy_run("가", 1);
        run.underline_kind = 1;
        run.color = 0x0000_00FF;
        run.underline_color = 0x00FF_0000;
        let page = place(run, &[]);
        let color = page.items.iter().find_map(|it| match it {
            Item::Path {
                stroke: Some(st), ..
            } => Some(st.color),
            _ => None,
        });
        assert_eq!(color, Some(0x00FF_0000));
    }

    #[test]
    fn emits_dashed_and_wave_underlines() {
        // Shape 1 (Dash): pattern [3u, 2u], with u=0.5.
        let mut run = dummy_run("가", 1);
        run.underline_kind = 1;
        run.underline_shape = 1;
        let page = place(run, &[]);
        let dash = page.items.iter().find_map(|it| match it {
            Item::Path {
                stroke: Some(st), ..
            } => Some(st.dash.clone()),
            _ => None,
        });
        assert_eq!(dash, Some(vec![1.5, 1.0]));

        // Shape 11 (Wave): one cubic path.
        let mut run = dummy_run("가", 1);
        run.underline_kind = 1;
        run.underline_shape = 11;
        let page = place(run, &[]);
        let wave_count = page
            .items
            .iter()
            .filter(|it| {
                matches!(it, Item::Path { commands, .. } if commands.iter().any(|c| matches!(c, PathCmd::CubicTo(..))))
            })
            .count();
        assert_eq!(wave_count, 1, "one wave path");

        // Shape 12 (DoubleWave): two paths separated by 2.5 widths (1.25pt).
        let mut run = dummy_run("가", 1);
        run.underline_kind = 1;
        run.underline_shape = 12;
        let page = place(run, &[]);
        let ys = path_summary(&page);
        assert_eq!(ys.len(), 2);
        assert!(
            (ys[1].0 - ys[0].0 - 1.25).abs() < 0.01,
            "double-wave spacing: {ys:?}"
        );
    }

    #[test]
    fn emits_strike_shape_at_expected_position() {
        // Double (7): two strokes at +/-0.15pt around y - 0.25em.
        let mut run = dummy_run("가", 1);
        run.strike = true;
        run.strike_shape = 7;
        let page = place(run, &[]);
        let ys = path_summary(&page);
        assert_eq!(ys.len(), 2);
        assert!((ys[0].0 - 197.35).abs() < 0.01, "first stroke: {ys:?}");
        assert!((ys[1].0 - 197.65).abs() < 0.01, "second stroke: {ys:?}");
    }

    #[test]
    fn emits_emphasis_marks_per_nonblank_glyph() {
        // Three glyphs for a syllable-space-syllable sequence produce two marks because whitespace is skipped.
        let mut run = dummy_run("가 나", 3);
        run.emphasis = 1; // DOT_ABOVE
        let page = place(run, &[]);
        let dots: Vec<(f32, f32)> = page
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Path {
                    commands,
                    fill: Some(Fill::Solid(_)),
                    ..
                } => match commands[0] {
                    PathCmd::MoveTo(x, y) => Some((x, y)),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(dots.len(), 2, "one mark per nonblank glyph: {dots:?}");
        // Radius is 0.05em and the center is 0.95em above the baseline.
        assert!((dots[0].0 - 105.5).abs() < 0.01, "first mark: {dots:?}");
        assert!(
            (dots[1].0 - 125.5).abs() < 0.01,
            "third-glyph mark: {dots:?}"
        );
        assert!(
            (dots[0].1 - 190.5).abs() < 0.01,
            "upper emphasis height: {dots:?}"
        );

        // DOT_BELOW (12): y + 0.30em = 203.
        let mut run = dummy_run("가", 1);
        run.emphasis = 12;
        let page = place(run, &[]);
        let below: Vec<f32> = page
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Path {
                    commands,
                    fill: Some(Fill::Solid(_)),
                    ..
                } => match commands[0] {
                    PathCmd::MoveTo(_, y) => Some(y),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(below.len(), 1);
        assert!(
            (below[0] - 203.0).abs() < 0.01,
            "DOT_BELOW height: {below:?}"
        );

        // COLON (6): two dots per glyph.
        let mut run = dummy_run("가나", 2);
        run.emphasis = 6;
        let page = place(run, &[]);
        let filled = page
            .items
            .iter()
            .filter(|it| matches!(it, Item::Path { fill: Some(_), .. }))
            .count();
        assert_eq!(filled, 4, "COLON has two dots per glyph");
    }

    #[test]
    fn character_background_precedes_glyphs() {
        let bf = BorderFill {
            bg_color: Some(0x0000_00FF),
            sides: [BorderLine {
                line_type: 1,
                width: 0,
                color: 0,
            }; 4],
            ..BorderFill::default()
        };
        let mut run = dummy_run("가", 1);
        run.border_fill_id = 1; // One-based reference.
        let page = place(run, std::slice::from_ref(&bf));

        // Order: background rectangle, glyphs, then border path.
        assert!(
            matches!(page.items[0], Item::Rect { fill, .. } if fill == 0x0000_00FF),
            "first item is the background rectangle: {:?}",
            std::mem::discriminant(&page.items[0])
        );
        assert!(
            matches!(page.items[1], Item::Glyphs { .. }),
            "second item contains glyphs"
        );
        // Four identical solid sides form one closed path after the glyphs.
        let border = page.items.iter().skip(2).find_map(|it| match it {
            Item::Path { commands, .. } => Some(commands),
            _ => None,
        });
        let cmds = border.expect("border path");
        assert!(matches!(cmds.last(), Some(PathCmd::Close)));
        // Provisional box metrics span y - 0.80em through y + 0.25em.
        let (top, bottom) = match page.items[0] {
            Item::Rect { y, h, .. } => (y, y + h),
            _ => unreachable!(),
        };
        assert!((top - 192.0).abs() < 0.01 && (bottom - 202.5).abs() < 0.01);

        // An out-of-range border fill reference does not emit a background.
        let mut run = dummy_run("가", 1);
        run.border_fill_id = 9;
        let page = place(run, std::slice::from_ref(&bf));
        assert!(
            !page.items.iter().any(|it| matches!(it, Item::Rect { .. })),
            "out-of-range reference has no background"
        );
    }

    #[test]
    fn builds_wave_path() {
        // half_len=2 and amp=1 use k=1/0.75 and return to the baseline.
        let cmds = wave_commands(0.0, 10.0, 8.0, 2.0, 1.0);
        assert!(matches!(cmds[0], PathCmd::MoveTo(0.0, 10.0)));
        assert_eq!(cmds.len(), 5, "four half-waves plus MoveTo");
        let k = 1.0 / 0.75;
        match cmds[1] {
            PathCmd::CubicTo(c1x, c1y, c2x, c2y, ex, ey) => {
                assert!((c1x - 1.0).abs() < 0.01 && (c1y - (10.0 - k)).abs() < 0.01);
                assert!((c2x - 1.0).abs() < 0.01 && (c2y - (10.0 - k)).abs() < 0.01);
                assert!((ex - 2.0).abs() < 0.01 && (ey - 10.0).abs() < 0.01);
            }
            _ => panic!("CubicTo"),
        }
        // The next half-wave bends downward.
        match cmds[2] {
            PathCmd::CubicTo(_, c1y, ..) => assert!((c1y - (10.0 + k)).abs() < 0.01),
            _ => panic!("CubicTo"),
        }
    }

    #[test]
    fn reserves_all_run_items_before_emission() {
        let mut run = dummy_run("가", 1);
        run.emphasis = 6; // Glyphs plus two COLON paths require three items.
        let mut page = PageList {
            width_pt: 600.0,
            height_pt: 800.0,
            items: Vec::new(),
        };
        let mut warnings = RenderIssueAccumulator::new();
        warnings.set_display_item_limit(2);

        push_run(&mut page, 100.0, 200.0, run, &[], &mut warnings);

        assert!(warnings.display_item_budget_exceeded());
        assert!(
            page.items.is_empty(),
            "a rejected run must be emitted atomically"
        );
    }
}

#[cfg(test)]
mod cell_split_plan_tests {
    use super::cached_cell_split_plan;
    use hwp_model::{BorderFillId, Cell, HwpChar, HwpUnit, LineSeg, Paragraph};

    /// Two cached 10 pt runs (50 lines each) separated by a soft v_pos reset
    /// (no page flag).  With first capacity 634.82 and a 2.82 fragment
    /// margin, the simulated remainder after run 1 is exactly 132 pt, so 13
    /// complete lines of run 2 pack into group 1.  Charging the margin twice
    /// would leave 129.18 pt and pack only 12 lines.
    #[test]
    fn fragment_margin_is_charged_once_per_fragment() {
        let mut chars = Vec::new();
        let mut line_segs = Vec::new();
        for _ in 0..2 {
            for index in 0..50u32 {
                chars.push(HwpChar::Text('a'));
                line_segs.push(LineSeg {
                    text_start: (chars.len() - 1) as u32,
                    v_pos: index as i32 * 1000,
                    line_height: 1000,
                    text_height: 900,
                    baseline_gap: 800,
                    line_spacing: 0,
                    col_start: 0,
                    seg_width: 50000,
                    flags: 0x0006_0000,
                });
            }
        }
        let cell = Cell {
            list_attr: 0,
            col: 0,
            row: 0,
            col_span: 1,
            row_span: 1,
            width: HwpUnit(5000),
            height: HwpUnit(100000),
            margins: [0; 4],
            border_fill: BorderFillId(0),
            header_tail: Vec::new(),
            paragraphs: vec![Paragraph {
                chars,
                line_segs,
                ..Paragraph::default()
            }],
        };

        let plan =
            cached_cell_split_plan(&cell, 0.0, 634.82, 654.80, 2.82).expect("two-run cached plan");
        let heights: Vec<f32> = plan.groups.iter().map(|group| group.height).collect();
        assert_eq!(heights, vec![500.0, 130.0, 370.0]);
    }
}

#[cfg(test)]
mod cell_render_cache_tests {
    #[test]
    fn ephemeral_cell_line_box_respects_both_paragraph_margins() {
        let mut doc = hwp_convert::from_markdown("Margin cell\n");
        let para = &doc.sections[0].paragraphs[0];
        let shape = &mut doc.header.para_shapes[para.para_shape.0 as usize];
        shape.margin_left = 2000;
        shape.margin_right = 4000;
        let para = &doc.sections[0].paragraphs[0];
        let mut store = crate::FontStore::new_isolated();
        let mut warnings = crate::RenderIssueAccumulator::new();
        let width = 10000;
        let lines = super::compute_cell_render_linesegs(
            &mut store,
            &doc,
            para,
            width,
            &mut 0,
            &mut warnings,
        );
        assert!(!lines.is_empty(), "exercise the render-only cell cache");
        for line in &lines {
            assert!(
                line.col_start >= 1000,
                "line stays after the left paragraph margin"
            );
            assert!(
                line.col_start + line.seg_width <= width - 2000,
                "render-only line box must stay before the right paragraph margin: {} + {} > {}",
                line.col_start,
                line.seg_width,
                width - 2000
            );
        }
        let native = crate::lineseg::compute_box_linesegs(
            &mut store,
            &doc,
            para,
            width,
            &mut 0,
            &mut warnings,
        );
        assert_eq!(
            native[0].seg_width, width,
            "native synthesis retains its prior full-width bytes"
        );
        assert_eq!(
            native[0].col_start, 1000,
            "native synthesis retains its prior col_start"
        );
    }
}
