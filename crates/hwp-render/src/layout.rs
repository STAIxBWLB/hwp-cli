//! LineSegLayouter — 파일에 저장된 줄 배치(PARA_LINE_SEG)를 복원해
//! DisplayList를 만든다.
//!
//! 실측으로 확정한 좌표 해석 (U1):
//! - `v_pos`: 페이지 본문 영역 상단 기준, 페이지마다 0으로 리셋
//! - 베이스라인 y = body_top + v_pos + baseline_gap
//! - `col_start`/`seg_width`: 본문 영역 왼쪽 기준
//! - 페이지 경계: v_pos가 직전 줄보다 작아지면 새 페이지 (v1 휴리스틱)
//!
//! 불완전한 파일 대응 (실무 hwpx에서 실측):
//! - 도구 생성 파일은 문단당 lineseg 1개 + 문단당 1줄 가정의 v_pos를
//!   기록한다 → seg 폭에서 그리디 줄바꿈 + **흐름 커서**로 보정한다.
//!   베이스라인 = max(저장된 v_pos 기반, 직전 콘텐츠 하단 기반) —
//!   완전한 파일에서는 저장값이 항상 크므로 무손실, 불완전 파일에서는
//!   겹침만 아래로 밀어낸다.
//! - lineseg가 아예 없는 문단은 본문 폭 기준 폴백 배치.

use std::collections::HashSet;

use hwp_model::{
    BorderFill, Caption, CaptionSide, Control, Document, HwpUnit, PageDef, Paragraph, Section,
    Table,
};

use crate::display::{DisplayList, Fill, Gradient, Item, PageList, PathCmd, Stroke};
use crate::error::RenderError;
use crate::fonts::FontStore;
use crate::footnote::{self, Note};
use crate::issues::{RenderIssueAccumulator, RenderIssueCode};
use crate::shape::{InlineItem, shape_range_page};

/// 인증 렌더가 레이아웃 생성 전에 적용하는 작업/메모리 예산.
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

/// 기본 탭 간격 (40pt = 4000 HWPUNIT).
pub(crate) const TAB_INTERVAL_PT: f32 = 40.0;

/// 연결 글상자 후보가 없을 때의 단 사이 가로 간격 근사값(pt).
const COL_GAP_PT: f32 = 14.0;

/// 글상자 내부 문단을 단(컬럼)별 범위로 나눈다. 한 줄의 v_pos가 직전 줄보다 작아지면
/// (한컴이 단 나누기로 흘린 것) 새 단으로 본다. 줄 배치 없는 문단은 현재 단에 둔다.
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

/// 연결 글상자의 이음단 위치(pt). 같은 폭·높이·세로오프셋을 갖고 더 오른쪽에 있는
/// 떠 있는 gso 박스들을 가로 순으로 모은다(연결 글상자 = 다음 단).
fn continuation_columns(para: &Paragraph, base: &crate::gso::GsoBox) -> Vec<(f32, f32)> {
    const TOL: i32 = 200; // 2pt 허용
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

/// A4 기본값 (PAGE_DEF가 없는 비정상 문서 방어).
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

/// 쪽 테두리 정의(PAGE_BORDER_FILL 14바이트, gc23 실측으로 확정한 레이아웃).
/// 속성 u32(위치기준 bit0·머리말 bit1·꼬리말 bit2·채울영역 bit3-4) + gap u16×4
/// (왼/오/위/아래, HWPUNIT16) + 테두리ID u16(1-기반, BORDER_FILL 참조).
struct PageBorderFill {
    /// 위치기준 bit0: false=쪽(본문) 기준, true=종이 기준. 정품 실측 기본값=종이.
    paper_relative: bool,
    /// gap 왼/오/위/아래 (HWPUNIT — 정품 기본 1417 ≈ 5mm).
    gap: [u16; 4],
    /// BORDER_FILL 참조 id(1-기반). 1 = 전 변 무테두리 관례.
    border_fill_id: u16,
}

/// PAGE_BORDER_FILL 14바이트 원문을 해석한다. 길이 부족이면 None.
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

/// 구역의 PAGE_BORDER_FILL 중 BOTH를 찾는다.
///
/// 데이터 소스: `SectionDef.page_border_fills_raw`(과제 1)를 소비한다. 등장 순서가
/// 곧 BOTH/EVEN/ODD이므로 BOTH = 첫 원소다. EVEN/ODD 분기는 범위 밖(정품 표본 부재).
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
    hidden: bool,
}

impl PageNumberState {
    fn new(start: u16) -> Self {
        Self {
            logical: u32::from(start.max(1)),
            placement: None,
            hidden: false,
        }
    }

    /// Page-level controls in a paragraph apply to the page containing that
    /// paragraph. `pgnp` remains active until another placement replaces it;
    /// `pghd` is reset after the current page is finalized.
    fn apply_controls(&mut self, para: &Paragraph, warnings: &mut RenderIssueAccumulator) {
        for control in &para.controls {
            let Control::Generic(control) = control else {
                continue;
            };
            match &control.ctrl_id {
                b"pgnp" => match crate::page_number::parse_pgnp(&control.data) {
                    Some(placement) => self.placement = Some(placement),
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
                    None => {} // PAGE 외 자동번호 재시작은 해당 번호 렌더러가 담당.
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
        if self.visible_number().is_some()
            && let Some(placement) = self.placement
        {
            render_positioned_page_number(
                doc,
                store,
                page,
                furniture,
                self.logical,
                placement,
                warnings,
            );
        }
        self.logical = self.logical.saturating_add(1);
        self.hidden = false;
    }
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
    logical: u32,
    placement: crate::page_number::PageNumberPlacement,
    warnings: &mut RenderIssueAccumulator,
) {
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
    let text = crate::page_number::format_placement(logical, placement, warnings);
    let Some(run) = crate::shape::shape_plain(store, doc, &text, 10.0, 0, false) else {
        warnings.push_once(RenderIssueCode::PageNumberShapingOmitted, b"positioned");
        return;
    };
    let top = matches!(placement.position, 1..=3 | 7 | 9);
    let x = if page_number_alignment(placement.position, logical) < 0 {
        furniture.body_left
    } else if page_number_alignment(placement.position, logical) > 0 {
        furniture.body_left + furniture.body_width - run.width_pt
    } else {
        furniture.body_left + (furniture.body_width - run.width_pt) * 0.5
    }
    .clamp(0.0, (page.width_pt - run.width_pt).max(0.0));
    let y = if top {
        let margin = furniture.page_def.margin_top.to_pt() as f32;
        (margin * 0.5 + run.size_pt * 0.35).clamp(run.size_pt, margin.max(run.size_pt))
    } else {
        let margin = furniture.page_def.margin_bottom.to_pt() as f32;
        page.height_pt - (margin * 0.5 - run.size_pt * 0.35).max(run.size_pt * 0.2)
    };
    push_run(page, x, y, run, &doc.header.border_fills, warnings);
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
    let mut pages = Vec::new();
    let mut page_numbers = PageNumberState::new(doc.header.properties.start_numbers[0]);

    for section in &doc.sections {
        // 이 구역의 첫 페이지 인덱스 — 구역 끝에서 쪽 테두리를 전 페이지에 소급 삽입한다.
        let section_first_page = pages.len();
        let page_def = section
            .section_def()
            .and_then(|d| d.page)
            .unwrap_or_else(|| {
                warnings.push(RenderIssueCode::PageDefinitionFallback, b"a4");
                default_page()
            });
        // 가로(landscape, PAGE_DEF attr bit0): 용지를 90° 돌려 폭↔높이를 맞바꾼다.
        // (이전엔 방향 무시 → 가로 문서가 세로로 렌더돼 우측 열이 잘렸다.)
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
        // 본문 영역 하한 (넘침 분할 기준)
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
        // 흐름 커서: 이 페이지에 실제 배치된 콘텐츠의 하단 y (page 좌표)
        let mut content_bottom = body_top;
        let mut skipped_controls = 0usize;
        let mut paras_on_page = 0usize;

        // 다단(multi-column): 섹션의 첫 cold 컨트롤 ColumnDef로 단 기하 설정(v1: 섹션당 1구성).
        // 한글 line_seg는 col_start=0(단 상대)·seg_width=단폭이므로 단 x는 밴드 인덱스로 계산한다.
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
        // 목록(번호/불릿) 카운터 — 구역 단위, 문서 순서로 진행.
        let mut list_state = crate::list::ListState::default();

        for para in &section.paragraphs {
            skipped_controls += para
                .controls
                .iter()
                .filter(|c| {
                    let rendered = matches!(
                        c,
                        Control::SectionDef(_) | Control::Table(_) | Control::Picture(_)
                    ) || page_control_is_rendered(c)
                        || [*b"cold", *b"head", *b"foot", *b"fn  ", *b"en  "]
                        .contains(&c.ctrl_id())
                        // 글상자(텍스트) + 도형(선/사각형/타원/호/다각형)은 렌더한다.
                        || matches!(c, Control::Generic(g)
                            if g.ctrl_id == *b"gso "
                                && (!g.paragraph_lists.is_empty()
                                    || crate::shape_draw::has_shape(&g.raw_children)))
                        // hwpx 구조화 도형(rect/ellipse/...).
                        || matches!(c, Control::Generic(g) if !g.gso_shapes.is_empty())
                        // 수식(hp:equation).
                        || matches!(c, Control::Generic(g) if g.equation.is_some());
                    !rendered
                })
                .count();

            // 본문 넘침: 직전 콘텐츠가 본문 하한을 지났으면 새 페이지
            // (lineseg 없는 생성 문서의 기본 페이지네이션)
            if content_bottom > body_bottom && paras_on_page > 0 {
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
                if !push_page_checked(&mut pages, &mut page, Some((w, h)), warnings) {
                    return DisplayList { pages };
                }
                content_bottom = body_top;
                prev_v_pos = -1;
                paras_on_page = 0;
                col_band = 0;
                has_flow_in_current_band = false;
            }

            // 쪽 나누기 (PARA_HEADER break_type bit2 / hp:p pageBreak)
            // — 글상자만 있어 items가 비어도 문단을 거쳤으면 분할한다
            if para.header.break_type & 0x04 != 0 && paras_on_page > 0 {
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
                if !push_page_checked(&mut pages, &mut page, Some((w, h)), warnings) {
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

            // 본문 각주/미주 마커(윗첨자 번호)와 이 페이지에 속할 노트 수집.
            // Split footnotes into the page queue and endnotes into the section queue.
            let marks = footnote::para_marks(&notes, para);
            for note in footnote::para_notes(&notes, para) {
                match note.kind {
                    footnote::NoteKind::Foot => page_notes.push(note),
                    footnote::NoteKind::End => pending_endnotes.push(note),
                }
            }
            let tabs = crate::tab::tab_stops(doc, para);
            let geom = para_geometry(doc, para);
            let links = crate::shape::hyperlink_ranges(para);
            // Do not consume a list counter for a paragraph that has no visible line at all.
            let marker = if para.line_segs.is_empty() && para.chars.is_empty() {
                None
            } else {
                list_state.marker_for_render(doc, para)
            };

            // 이 문단의 첫 줄 상단 (표 앵커 위치)
            let mut para_top: Option<f32> = None;

            // 문단 배경/테두리(border_fill) 패스용. 문단이 페이지/단 경계를 걸치면 배경을
            // 페이지별 조각으로 나눠 각 페이지에 그린다(GC-9). 조각 상태:
            //  - bg_slice_top: 현재 페이지 조각의 상단 y(그 페이지 첫 줄 배치 시 설정)
            //  - bg_slice_insert: 현재 페이지 items 삽입 지점(배경 Rect를 텍스트 뒤로)
            //  - bg_slice_col_x: 현재 조각의 단 x-오프셋(다단)
            //  - bg_first_slice: 첫 조각인가(진짜 문단 상단 → 상단 테두리 O)
            let mut bg_slice_top: Option<f32> = None;
            let mut bg_slice_insert = page.items.len();
            let mut bg_slice_col_x = 0.0f32;
            let mut bg_first_slice = true;

            if para.line_segs.is_empty() {
                // 폴백: 본문 폭에서 그리디 줄바꿈
                if para.chars.is_empty() {
                    content_bottom += 16.0; // 빈 문단 높이 근사
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
                    // 문단 들여쓰기/여백/위 간격(폴백 전용 — 캐시는 col_start에 반영됨).
                    let left = body_left + geom.left;
                    let avail = (body_width - geom.left - geom.right).max(4.0);
                    // 첫 줄 들여쓰기/내어쓰기(음수 허용): 첫 줄 x = 좌여백 + indent를
                    // 페이지 좌변(body_left) 밖으로 안 나가게 클램프한 뒤 그 오프셋만
                    // 첫 줄에 준다(wrap 폭 미차감 — 좁은 셀 폭주 방지). 비정상 큰 양수는 캡.
                    let indent = geom.first_indent.min(avail * 0.8);
                    let first_x = (left + indent).max(body_left);
                    let baseline_y = content_bottom + geom.spacing_top + max_size * 1.2;
                    para_top = Some(content_bottom + geom.spacing_top);
                    // 한 줄에 들어가는 가운데/오른쪽 정렬은 폴백에서도 보정한다.
                    let natural = items_width(&items, &tabs);
                    let align = doc
                        .header
                        .para_shapes
                        .get(para.para_shape.0 as usize)
                        .map_or(1, |p| p.alignment());
                    // 정렬(가운데/오른쪽) 한 줄은 들여쓰기 무시; 그 외엔 x0=좌여백 + 첫 줄 오프셋.
                    let (x0, first_delta) = if natural <= avail && (align == 2 || align == 3) {
                        (
                            left + (avail - natural) * if align == 3 { 0.5 } else { 1.0 },
                            0.0,
                        )
                    } else if marker.is_some() && geom.first_indent < 0.0 {
                        // 목록 문단의 내어쓰기 구간은 마커 자리다(한글 실기와 동일).
                        // 첫 줄 텍스트를 left로 되돌리지 않으면 마커가 글자 밑에 깔린다.
                        (left, 0.0)
                    } else {
                        (left, first_x - left)
                    };
                    if let Some(m) = &marker {
                        render_list_marker(
                            &mut page,
                            store,
                            doc,
                            m,
                            (left, baseline_y, max_size),
                            warnings,
                        );
                    }
                    let last_y = place_wrapped(
                        &mut page,
                        items,
                        x0,
                        baseline_y,
                        avail,
                        max_size * 1.6,
                        &tabs,
                        first_delta,
                        &doc.header.border_fills,
                        warnings,
                    );
                    content_bottom = last_y + max_size * 0.4 + geom.spacing_bottom;
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
                );
                content_bottom = objects_bottom;
                // A fallback paragraph is one slice with both borders. If an
                // attached table split, approximate only its final-page slice.
                if let Some(top) = para_top {
                    let (slice_insert, slice_top, slice_first) = if objects_split {
                        (0, body_top, false)
                    } else {
                        (bg_slice_insert, top, true)
                    };
                    draw_para_bg_slice(
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
                }
                has_flow_in_current_band = true;
                continue;
            }

            let last_content = last_content_seg(para);
            for (i, seg) in para.line_segs.iter().enumerate() {
                // Hancom-saved linesegs use bit0 (first line of a page) and
                // bit1 (first line of a column) as authoritative boundaries.
                // Synthesized linesegs use 0x0006_0000 and therefore fall back
                // to the v_pos-reset heuristic.
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
                        draw_para_bg_slice(
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
                        bg_first_slice = false;
                    }
                    if page_break {
                        // A page starts in band zero.
                        col_band = 0;
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
                        if !push_page_checked(&mut pages, &mut page, Some((w, h)), warnings) {
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

                // 정렬 보정 (가운데/오른쪽 + 양쪽정렬은 마지막 줄 빼고 글자 사이로 잉여 분배).
                let seg_width_pt = seg.seg_width as f32 / 100.0;
                let align = doc
                    .header
                    .para_shapes
                    .get(para.para_shape.0 as usize)
                    .map_or(0, |ps| ps.alignment());
                let shift = align_line(
                    &mut items,
                    align,
                    seg_width_pt,
                    natural_width,
                    i == last_content,
                );

                let baseline_gap_pt = seg.baseline_gap as f32 / 100.0;
                let line_height_pt = seg.line_height as f32 / 100.0;
                let stored_baseline = body_top + (seg.v_pos + seg.baseline_gap) as f32 / 100.0;
                // 흐름 커서 보정: 앞 콘텐츠가 저장 위치를 이미 지났으면
                // 베이스라인을 (콘텐츠 하단 + 이 줄의 ascent) 아래로 밀어낸다
                let baseline_y = stored_baseline.max(content_bottom + baseline_gap_pt);

                // 문단에 lineseg가 1개뿐인데 텍스트가 폭을 넘으면 불완전한
                // lineseg로 보고 seg 폭에서 줄바꿈. 완전한 lineseg는 신뢰.
                let wrap_width = if para.line_segs.len() == 1 {
                    seg_width_pt.max(10.0)
                } else {
                    f32::INFINITY
                };
                let line_advance =
                    (seg.line_height + seg.line_spacing).max(seg.line_height) as f32 / 100.0;

                // 다단: 현재 밴드의 단 x-오프셋(col_start는 단 상대라 0). 단일 단이면 0.
                let col_x = (col_band % col_count) as f32 * (col_width + col_gap);
                let x = body_left + col_x + seg.col_start as f32 / 100.0 + shift;
                // 배경 조각 상단: 이 페이지/단에서 처음 놓이는 줄의 윗변에서 잡는다(GC-9).
                if bg_slice_top.is_none() {
                    bg_slice_top = Some(baseline_y - baseline_gap_pt);
                    bg_slice_col_x = col_x;
                }
                if i == 0 {
                    para_top = Some(baseline_y - baseline_gap_pt);
                    if let Some(m) = &marker {
                        let size = items_max_size(&items).unwrap_or(line_height_pt.max(8.0));
                        render_list_marker(
                            &mut page,
                            store,
                            doc,
                            m,
                            (x, baseline_y, size),
                            warnings,
                        );
                    }
                }
                let last_y = place_wrapped(
                    &mut page,
                    items,
                    x,
                    baseline_y,
                    wrap_width,
                    line_advance,
                    &tabs,
                    0.0, // 캐시 줄은 col_start에 들여쓰기가 이미 반영됨.
                    &doc.header.border_fills,
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
            );
            content_bottom = objects_bottom;
            // 마지막(또는 유일) 배경 조각: 하변 테두리 O, 상변은 첫 조각일 때만(=경계 안 걸침).
            if let Some(top) = bg_slice_top {
                let (slice_insert, slice_top, slice_first) = if objects_split {
                    (0, body_top, false)
                } else {
                    (bg_slice_insert, top, bg_first_slice)
                };
                draw_para_bg_slice(
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
            }
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
                    page_numbers.finish(doc, store, &mut page, &furniture, warnings);
                    if !push_page_checked(&mut pages, &mut page, Some((w, h)), warnings) {
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
                    page_numbers.finish(doc, store, &mut page, &furniture, warnings);
                    if !push_page_checked(&mut pages, &mut page, Some((w, h)), warnings) {
                        return DisplayList { pages };
                    }
                    content_bottom = body_top;
                    page_limit = body_bottom;
                }
            }
            pending_endnotes.clear();
        }
        page_numbers.finish(doc, store, &mut page, &furniture, warnings);
        if !push_page_checked(&mut pages, &mut page, None, warnings) {
            return DisplayList { pages };
        }

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
) -> bool {
    if !warnings.charge_page() {
        return false;
    }
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
    ) -> bool {
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
        if !push_page_checked(self.pages, page, Some(self.page_dims), warnings) {
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

/// 기본 셀 안쪽 여백 (HWPUNIT — 한글 기본값).
const DEFAULT_CELL_MARGINS: [u16; 4] = [510, 510, 141, 141];

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
        self.groups.len() > 1 && self.groups.iter().all(|group| group.has_content)
    }
}

/// Build Regime-A cell fragments from the line layout saved by Hancom.
///
/// A CELL table is only safely splittable at a cached line boundary. Explicit
/// cached page starts (`flags & 1` or a vertical-position reset) are preserved;
/// otherwise a boundary is selected immediately before the first cached line
/// that would exceed the remaining page capacity. A paragraph without line
/// segments is not guessed into a page: callers must surface an incomplete
/// render when that paragraph is part of a required split.
fn cached_cell_split_plan(
    cell: &hwp_model::Cell,
    measured_content_h: f32,
    first_capacity_pt: f32,
    following_capacity_pt: f32,
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
            }
            let explicit_boundary = saw_line
                && (seg.flags & 0x1 != 0
                    || previous_v.is_some_and(|previous| seg.v_pos < previous));
            let capacity_boundary = saw_line
                && !explicit_boundary
                && has_content[current]
                && (end - first_v[current]) as f32 / 100.0 > current_capacity + 0.5;
            if explicit_boundary || capacity_boundary {
                boundaries.insert((para_index, seg_index));
                current += 1;
                first_v.push(seg.v_pos);
                max_end.push(seg.v_pos);
                has_content.push(false);
                current_capacity = following_capacity;
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

/// 페이지 가구 (머리말/꼬리말) — 페이지 마감 시마다 그린다.
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

/// 페이지 하단에 각주/미주 영역을 그린다(구분선 + 번호 + 내용).
/// 블록 하단이 본문 하한(body_bottom)에 닿도록 위로 올려 배치한다.
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
    // 1) 스크래치 페이지에 y=0부터 노트를 쌓아 총 높이를 잰다.
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
        y += 3.0; // 노트 사이 간격
    }
    // 2) 블록 하단이 body_bottom에 닿도록 위로 올린다(본문과 겹치면 그대로 둠).
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
    });
    // 3) 스크래치 아이템을 top만큼 내려 본 페이지에 합친다.
    for item in scratch.items.drain(..) {
        page.items.push(translate_item(item, 0.0, top));
    }
}

/// 노트 하나(번호 마커 + 내용 문단)를 (x, y)에 그리고 다음 y(하단)를 반환.
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
    // 내용 문단들(자체 char_shape 크기 사용). 여러 문단은 세로로 누적.
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

/// 목록 마커(불릿/번호)를 텍스트 시작 왼쪽(매달린 위치)에 그린다.
fn render_list_marker(
    page: &mut PageList,
    store: &mut FontStore,
    doc: &Document,
    marker: &str,
    placement: (f32, f32, f32),
    warnings: &mut RenderIssueAccumulator,
) {
    let (text_left, baseline, size) = placement;
    if let Some(run) = crate::shape::shape_plain(store, doc, marker, size, 0, false) {
        let w = run.width_pt;
        let x = (text_left - w - size * 0.3).max(0.0);
        push_run(page, x, baseline, run, &doc.header.border_fills, warnings);
    }
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

/// Item을 (dx, dy)만큼 평행이동한 사본.
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
        } => Item::Path {
            commands: commands
                .into_iter()
                .map(|c| translate_cmd(c, dx, dy))
                .collect(),
            fill,
            stroke,
        },
    }
}

/// PathCmd를 (dx, dy)만큼 평행이동.
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

/// 문단에 달린 블록 개체(표/이미지)를 배치한다. 갱신된 콘텐츠 하단을 반환.
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
    mut split: Option<&mut TableSplitCtx<'_, '_>>,
    warnings: &mut RenderIssueAccumulator,
) -> (f32, bool) {
    let mut bottom = content_bottom;
    let mut object_y = anchor_top;
    // After a split, the returned cursor is relative to the final page.
    let mut page_split = false;

    for control in &para.controls {
        match control {
            Control::Table(table) => {
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
                );
                if table_split {
                    // 쪽이 나뉐 뒤의 커서는 새 페이지 좌표 — 이전 페이지의
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
                        if !warnings.charge_display_items(1) {
                            return (bottom, page_split);
                        }
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
                // Move anchored shapes to the flow position without mutating IR.
                let adjusted: Vec<hwp_model::ShapeGeom> = g
                    .gso_shapes
                    .iter()
                    .map(|s| {
                        let mut s2 = s.clone();
                        if s.anchored {
                            s2.x = (x * 100.0) as i32;
                            s2.y = ((object_y + top_offset) * 100.0) as i32;
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
            // 수식(hp:equation) — 스크립트를 실제 math로 조판(equation.rs).
            Control::Generic(g) if g.equation.is_some() => {
                let eq = g.equation.as_ref().expect("is_some");
                let h = (eq.height as f32 / 100.0).max(12.0);
                let (bx, by, inline) = if eq.inline {
                    (x, object_y, true)
                } else {
                    (eq.x as f32 / 100.0, eq.y as f32 / 100.0, false)
                };
                // 글자 크기 2-pass: 여러 행 수식은 총 높이가 크므로, 기준 12pt로 시험 조판해
                // 실제 높이를 잰 뒤 상자 높이(eq.height)에 맞춰 스케일한다(단일 행 가정 제거).
                let probe = crate::equation::typeset(store, doc, &eq.script, 12.0);
                let ph = (probe.ascent + probe.descent).max(1.0);
                let size = (12.0 * h / ph).clamp(6.0, 18.0);
                let ebox = crate::equation::typeset(store, doc, &eq.script, size);
                // 상단 정렬: 수식 상단(baseline-ascent)을 상자 상단(by)에 맞춘다.
                let baseline_y = by + ebox.ascent;
                crate::equation::render_into(page, ebox, bx + 2.0, baseline_y, warnings);
                if inline {
                    object_y += h;
                    bottom = bottom.max(by + h);
                }
            }
            _ => {}
        }
    }
    (bottom, page_split)
}

/// 셀 여백 (왼/오른/위/아래) pt — 셀 지정 → 표 기본 → 한글 기본.
fn cell_margins(table: &Table, cell: &hwp_model::Cell) -> (f32, f32, f32, f32) {
    // HWP5 uses 0xffff in a cell LIST_HEADER to mean "use the table
    // default".  It is important that the sentinel remains in the IR for a
    // lossless hwp5 -> hwp5 round trip, so resolve it only at render time.
    // An all-zero cell margin array is the older IR convention for an omitted
    // cell margin and keeps the existing table/default fallback behavior.
    let table_default = if table.inner_margins.iter().any(|&v| v > 0 && v != u16::MAX) {
        std::array::from_fn(|i| {
            if table.inner_margins[i] == u16::MAX {
                DEFAULT_CELL_MARGINS[i]
            } else {
                table.inner_margins[i]
            }
        })
    } else {
        DEFAULT_CELL_MARGINS
    };
    let raw = if cell.margins.iter().all(|&v| v == 0) {
        table_default
    } else {
        cell.margins
    };
    let m: [u16; 4] = std::array::from_fn(|i| {
        if raw[i] == u16::MAX {
            table_default[i]
        } else {
            raw[i]
        }
    });
    (
        m[0] as f32 / 100.0,
        m[1] as f32 / 100.0,
        m[2] as f32 / 100.0,
        m[3] as f32 / 100.0,
    )
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
) -> (f32, bool, f32) {
    let cols = table.cols.max(1) as usize;
    let rows = table.rows.max(1) as usize;
    let top_caption_extent = top_caption
        .as_ref()
        .map_or(0.0, |caption| caption.height + caption.gap);

    // 그리드 기하: span=1 셀에서 열 폭/행 높이를 확정, 모르는 칸은 평균으로
    let mut col_w = vec![0.0f32; cols];
    let mut row_h = vec![0.0f32; rows];
    let mut row_covered = vec![false; rows];
    for cell in &table.cells {
        let (c, r) = (cell.col as usize, cell.row as usize);
        if cell.col_span == 1 && c < cols {
            col_w[c] = col_w[c].max(cell.width.to_pt() as f32);
        }
        if cell.row_span == 1 && r < rows {
            row_h[r] = row_h[r].max(cell.height.to_pt() as f32);
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

    // 측정 패스: 실제 내용 높이로 행 높이를 확장한다(저장된 cell.height는 한글의 줄바꿈
    // 기준이라, 셰이핑/합성 줄바꿈이 더 많은 줄을 만들면 내용이 다음 행을 침범해 겹친다 —
    // 실측 높이와 max로 행을 늘려 방지). 스크래치 페이지에 그려 높이만 잰다. 실측 내용
    // 높이는 세로정렬에 재사용한다(재측정 회피).
    let mut spanned: Vec<(usize, usize, f32)> = Vec::new(); // (시작행, 스팬, 필요높이)
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
        // 빈 셀은 스크래치 레이아웃(할당+셰이핑)을 생략 — 내용 높이 0(여백 mt+mb는 아래서 반영).
        let content_h = if cell.paragraphs.is_empty() {
            0.0
        } else {
            let mut scratch = PageList {
                width_pt: page.width_pt,
                height_pt: page.height_pt,
                items: Vec::new(),
            };
            let mut scratch_warn = RenderIssueAccumulator::new();
            layout_box_paragraphs(
                doc,
                store,
                &mut scratch,
                &cell.paragraphs,
                0.0,
                0.0,
                (cw - ml - mr).max(4.0),
                &mut scratch_warn,
                None, // 측정 패스: 마커 미표시(counter 미증가)
                None,
            )
        };
        content_h_by_cell.push(content_h);
        let needed = content_h + mt + mb;
        let span = (cell.row_span as usize).max(1);
        if span == 1 {
            row_h[r] = row_h[r].max(needed);
        } else {
            spanned.push((r, span, needed));
        }
    }
    // row_span>1 셀: 스팬 행 합이 부족하면 마지막 스팬 행에 부족분을 더한다.
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
            if !ctx.break_page(doc, store, page, warnings) {
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
    let has_any_row_span = table.cells.iter().any(|cell| cell.row_span.max(1) > 1);
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
            planned_cursor += row_h[row];
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
        let mut row_groups = 1usize;
        let mut cells_requiring_boundary = Vec::new();
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
            ) else {
                continue;
            };
            if !plan.is_usable() {
                continue;
            }
            for group in &mut plan.groups {
                group.height += mt + mb;
            }
            let cached_height: f32 = plan.groups.iter().map(|group| group.height).sum();
            if row_h[row] > cached_height {
                // Preserve the declared row height as blank continuation
                // area.  It does not need a text boundary, but must not push
                // a cached line outside the cell's visual fragment.
                if let Some(group) = plan.groups.last_mut() {
                    group.height += row_h[row] - cached_height;
                }
            }
            row_groups = row_groups.max(plan.groups.len());
            cell_plans[cell_index] = Some(plan);
        }

        if row_groups > 1
            && cells_requiring_boundary.iter().any(|cell_index| {
                cell_plans[*cell_index]
                    .as_ref()
                    .is_none_or(|plan| !plan.is_usable())
            })
        {
            warnings.push(
                RenderIssueCode::TableCellFragmentationIncomplete,
                format!("row={row}:cache"),
            );
            return None;
        }

        if row_groups == 1 {
            // No cached content needs to use the current-page remainder. Let
            // the existing row-boundary planner move this row as a unit.
            if row_h[row] > continuation_height + 0.5 {
                warnings.push(
                    RenderIssueCode::TableCellFragmentationIncomplete,
                    format!("row={row}:boundary"),
                );
                return None;
            }
            planned_cursor = continuation_top + row_h[row];
            planned_bottom = body_bottom;
            planned_emitted_parts += 1;
            continue;
        }

        found_cached_split = true;
        row_group_counts[row] = row_groups;
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
    if has_any_row_span {
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
        for group_index in 0..group_count {
            let part_height = if group_count == 1 {
                row_h[row]
            } else {
                table
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
                    .max(1.0)
            };
            let mut replay_header = false;
            if cursor + part_height > current_bottom + 0.5 {
                let ctx = split.as_deref_mut()?;
                if emitted_parts == 0 && cursor <= body_top + top_caption_extent + 0.5 {
                    warnings.push(
                        RenderIssueCode::TableRowTooTallClipped,
                        format!("cell row={row} fragment={group_index}"),
                    );
                } else if !ctx.break_page(doc, store, page, warnings) {
                    return Some((cursor, page_advanced, final_fragment_top));
                } else {
                    page_advanced = true;
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
                    part_height,
                    content_h_by_cell.get(cell_index).copied().unwrap_or(0.0),
                    selections,
                    v_origin,
                    group_count == 1 || group_index == 0,
                    group_count == 1 || group_index + 1 == group_count,
                    &mut cell_ls,
                    warnings,
                );
            }
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
    draw_top: bool,
    draw_bottom: bool,
    cell_ls: &mut crate::list::ListState,
    warnings: &mut RenderIssueAccumulator,
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
    let voff = if draw_top && draw_bottom {
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
        );
    }

    if let Some(bf) = border_fill {
        let items = crate::border::border_rectangle_items(
            cx,
            cy,
            cx + cw,
            cy + ch,
            &bf.sides,
            [true, true, draw_top, draw_bottom],
        );
        if warnings.charge_display_items(items.len()) {
            page.items.extend(items);
        }
        // Diagonals describe the complete cell, so emit them only on the
        // first fragment; continuation fragments carry the four-sided frame.
        if draw_top {
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

        let border_fill = doc
            .header
            .border_fills
            .get((cell.border_fill.0 as usize).saturating_sub(1));

        // 1) 배경
        if let Some(item) = border_fill.and_then(|bf| bg_fill_item(bf, cx, cy, cw, ch)) {
            if !warnings.charge_display_items(1) {
                return;
            }
            page.items.push(item);
        }

        // 2) 내용 — 셀 여백 + 세로정렬(list_attr bits5~6: 0=위, 1=가운데, 2=아래).
        //    측정 패스의 실측 내용 높이로 남는 공간을 계산해 오프셋한다.
        let (ml, mr, mt, mb) = cell_margins(table, cell);
        let content_h = content_h_by_cell.get(ci).copied().unwrap_or(0.0);
        let avail = (ch - mt - mb - content_h).max(0.0);
        let voff = match cell.vert_align() {
            1 => avail * 0.5,
            2 => avail,
            _ => 0.0,
        };
        layout_box_paragraphs(
            doc,
            store,
            page,
            &cell.paragraphs,
            cx + ml,
            cy + mt + voff,
            (cw - ml - mr).max(4.0),
            warnings,
            Some(cell_ls), // 렌더 패스: 셀 목록 마커 그림
            None,
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
                return;
            }
            page.items.extend(items);

            // 4) 대각선/역대각선 — slash/backSlash 비트가 켜졌을 때만(병합 셀은 전체 영역 가로지름).
            //    diagonal 선은 스타일/색만 제공하므로 방향 비트가 없으면 그리지 않는다.
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
                        return;
                    }
                    page.items.extend(items);
                }
            }
        }
    }
}

/// BORDER_FILL 속성 비트 → (대각선 `/`, 역대각선 `\`) 그릴지.
/// slash=bit2~4, backSlash=bit5~7. 둘 다 0이면 대각선 없음.
fn diagonal_dirs(attr: u16) -> (bool, bool) {
    let slash = (attr >> 2) & 0x7 != 0;
    let backslash = (attr >> 5) & 0x7 != 0;
    (slash, backslash)
}

/// 상자(셀) 안 문단들을 배치한다. origin은 텍스트 영역 좌상단(pt).
/// 셀 내부 lineseg의 v_pos는 셀 텍스트 영역 상단 기준(본문과 동일 모델).
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

/// `layout_box_paragraphs`의 반복자 버전 — 단(컬럼)으로 분할된 조각도 받는다.
///
/// 캐시된 lineseg v_pos는 한컴 배치 그대로 존중한다(흐름 커서로 끌어내리지 않음 —
/// 끌어내리면 키 큰 글상자에서 줄마다 드리프트가 누적돼 페이지 밖으로 넘친다).
/// `flow_floor`는 "흐름으로 배치된 콘텐츠"(캐시 없는 폴백 문단, 표/이미지 블록 개체,
/// 우리 줄바꿈이 캐시와 어긋나 캐시 자리 아래로 넘친 줄)만 바닥을 올려, 뒤따르는
/// 캐시 문단이 그 위로 겹치지 않게 한다.
#[allow(clippy::too_many_arguments)]
fn layout_box_para_iter<'a>(
    doc: &Document,
    store: &mut FontStore,
    page: &mut PageList,
    paras: impl Iterator<Item = (&'a Paragraph, BoxParaSelection)>,
    origin_x: f32,
    origin_y: f32,
    width: f32,
    warnings: &mut RenderIssueAccumulator,
    mut list_state: Option<&mut crate::list::ListState>,
    page_number: Option<u32>,
    v_origin: i32,
) -> f32 {
    let mut content_bottom = origin_y;
    // 흐름 하한: 캐시 줄은 올리지 않고, 흐름 배치 콘텐츠만 올린다 (함수 doc 참고).
    let mut flow_floor = origin_y;
    for (para, selection) in paras {
        if matches!(selection, BoxParaSelection::Empty) {
            continue;
        }
        let mut para_top: Option<f32> = None;
        let tabs = crate::tab::tab_stops(doc, para);
        // Do not advance a list counter for a completely empty box paragraph.
        let marker = if para.line_segs.is_empty() && para.chars.is_empty() {
            None
        } else {
            list_state
                .as_deref_mut()
                .and_then(|ls| ls.marker_for_render(doc, para))
        };

        if para.line_segs.is_empty() {
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
                // 첫 줄 들여쓰기/내어쓰기(음수 허용): 셀 좌변(origin_x) 밖 클램프, 첫 줄에만
                // (wrap 폭 미차감 — 좁은 셀 폭주 방지). 비정상 큰 양수는 방어 캡.
                let indent = geom.first_indent.min(avail * 0.8);
                let first_x = (left + indent).max(origin_x);
                let baseline_y = content_bottom + geom.spacing_top + max_size * 1.2;
                para_top = Some(content_bottom + geom.spacing_top);
                if let Some(m) = &marker {
                    render_list_marker(page, store, doc, m, (left, baseline_y, max_size), warnings);
                }
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
                } else if marker.is_some() && geom.first_indent < 0.0 {
                    // 내어쓰기 구간은 마커 자리 — 본문 폴백 경로와 같은 규칙.
                    (left, 0.0)
                } else {
                    (left, first_x - left)
                };
                let last_y = place_wrapped(
                    page,
                    items,
                    x0,
                    baseline_y,
                    avail,
                    max_size * 1.6,
                    &tabs,
                    first_delta,
                    &doc.header.border_fills,
                    warnings,
                );
                content_bottom = last_y + max_size * 0.4 + geom.spacing_bottom;
            }
            // 폴백(캐시 없는) 문단은 흐름 배치 — 이후 캐시 문단이 넘지 않게 바닥을 올린다.
            flow_floor = flow_floor.max(content_bottom);
        } else {
            let last_content = last_content_seg(para);
            let selected = match &selection {
                BoxParaSelection::All => 0..para.line_segs.len(),
                BoxParaSelection::Segments(range) => {
                    range.start.min(para.line_segs.len())..range.end.min(para.line_segs.len())
                }
                BoxParaSelection::Empty => unreachable!("empty selections are skipped above"),
            };
            for i in selected {
                let seg = &para.line_segs[i];
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
                    &std::collections::HashMap::new(),
                    page_number,
                    warnings,
                );
                let natural_width = items_width(&items, &tabs);

                let seg_width_pt = (seg.seg_width as f32 / 100.0).min(width);
                let align = doc
                    .header
                    .para_shapes
                    .get(para.para_shape.0 as usize)
                    .map_or(0, |ps| ps.alignment());
                let shift = align_line(
                    &mut items,
                    align,
                    seg_width_pt,
                    natural_width,
                    i == last_content,
                );

                let gap_pt = seg.baseline_gap as f32 / 100.0;
                let v_pos = seg.v_pos.saturating_sub(v_origin);
                let stored = origin_y + (v_pos + seg.baseline_gap) as f32 / 100.0;
                // 캐시 v_pos를 존중: 흐름 하한 위로만 보정(흐름 커서로 끌어내리지 않음).
                let baseline_y = stored.max(flow_floor + gap_pt);
                let first_selected = match &selection {
                    BoxParaSelection::All => i == 0,
                    BoxParaSelection::Segments(range) => i == range.start,
                    BoxParaSelection::Empty => false,
                };
                if first_selected {
                    para_top = Some(baseline_y - gap_pt);
                    if let Some(m) = &marker {
                        let size = items_max_size(&items).unwrap_or(8.0);
                        render_list_marker(
                            page,
                            store,
                            doc,
                            m,
                            (
                                origin_x + seg.col_start as f32 / 100.0 + shift,
                                baseline_y,
                                size,
                            ),
                            warnings,
                        );
                    }
                }
                let wrap_width = if para.line_segs.len() == 1 {
                    seg_width_pt.max(4.0)
                } else {
                    f32::INFINITY
                };
                let line_advance =
                    (seg.line_height + seg.line_spacing).max(seg.line_height) as f32 / 100.0;

                let last_y = place_wrapped(
                    page,
                    items,
                    origin_x + seg.col_start as f32 / 100.0 + shift,
                    baseline_y,
                    wrap_width,
                    line_advance,
                    &tabs,
                    0.0, // 캐시 줄은 col_start에 들여쓰기가 이미 반영됨.
                    &doc.header.border_fills,
                    warnings,
                );
                content_bottom = last_y + (seg.line_height as f32 / 100.0 - gap_pt).max(0.0);
                // 우리 줄바꿈이 캐시와 어긋나 이 줄이 캐시 자리 아래로 넘쳤다면(단일 seg
                // 문단의 추가 줄바꿈 등) 흐름 하한을 올려 다음 캐시 문단 겹침을 막는다.
                // 다중 seg 줄은 wrap_width=INFINITY → last_y == baseline_y → 올리지 않음.
                if last_y > baseline_y {
                    flow_floor = flow_floor.max(content_bottom);
                }
            }
        }

        // 셀 안의 중첩 표/이미지 — 바닥을 늘렸으면 흐름 하한도 올려 후속 캐시 문단 겹침 방지.
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
            let (objects_bottom, _no_split) = layout_para_objects(
                doc,
                store,
                page,
                para,
                origin_x,
                para_top.unwrap_or(content_bottom),
                content_bottom,
                width,
                None, // 상자(셀/글상자) 안의 중첩 개체는 쪽을 걸치지 않는다
                warnings,
            );
            content_bottom = objects_bottom;
            if content_bottom > before_objects {
                flow_floor = flow_floor.max(content_bottom);
            }
        }
    }
    content_bottom
}

/// 열 폭 확정: `col_span==1`로 못 정한 열을 병합 셀(`col_span>1`)에서 유도하고, 표의 실제
/// 총 폭(행별 셀 폭 합의 최대)에 맞춰 스케일한다. 표 실제 폭이 가용 폭(`avail_width`,
/// 본문/셀 폭)을 넘으면 가용 폭에 맞춰 축소한다(한글 동작). 병합 위주 표가 평균 폴백으로
/// 페이지를 넘던 문제(잉크 초과)를 해소한다. 정상 표(실제 폭 ≤ 가용)는 s≈1이라 무영향.
fn derive_col_widths(col_w: &mut [f32], table: &Table, avail_width: f32) {
    let cols = col_w.len();
    // 1) 병합 셀에서 미지 열 유도 (작은 병합 먼저 확정해야 큰 병합이 남은 미지에 정확히 배분).
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
    // 2) 그래도 미지면 평균 폴백(기존 동작).
    fill_unknown(col_w, 60.0);
    // 3) 스케일 목표 = 표 실제 폭(유도 잔차 보정 + 안전망), 단 가용 폭 초과 시 가용 폭에
    //    맞춘다(한글 축소 동작). 정상 표는 sum==table_width≤avail라 s=1.
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

/// 표의 실제 총 폭(pt) = 행별 셀 폭 합의 최대(모든 열을 커버하는 행 = 표 폭).
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

/// 문단에서 텍스트가 있는(line_end>line_start) 마지막 seg 인덱스. 빈 trailing seg 방어.
fn last_content_seg(para: &Paragraph) -> usize {
    let n = para.line_segs.len();
    (0..n)
        .rev()
        .find(|&j| {
            let ls = para.line_segs[j].text_start;
            let le = para
                .line_segs
                .get(j + 1)
                .map_or(para.wchar_len(), |s| s.text_start);
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

/// 문단 기하(pt) — 폴백 경로에서 적용할 들여쓰기/여백/간격.
/// (캐시 lineseg 경로는 col_start/v_pos에 이미 반영돼 있어 쓰지 않는다.)
#[derive(Default, Clone, Copy)]
struct ParaGeom {
    /// 왼쪽 여백(margin_left만 — 들여쓰기는 first_indent로 분리).
    left: f32,
    right: f32,
    /// 첫 줄 들여쓰기(양수) / 내어쓰기(음수 허용 — 첫 줄이 나머지보다 왼쪽).
    /// wrap 폭에선 빼지 않는다 — 좁은 셀에서 avail이 붕괴해 글자마다 줄바꿈되는
    /// 폭주 방지(work_report 실측). 페이지 좌변 밖 클램프는 배치 지점에서 한다.
    first_indent: f32,
    spacing_top: f32,
    spacing_bottom: f32,
}

/// 문단 배경/테두리(ParaShape.border_fill_id → BorderFill)의 한 페이지 조각을 그린다 —
/// 셀 배경 패스의 문단판. 배경 Rect는 `insert_idx`(그 페이지의 문단 텍스트 시작 지점)에
/// 삽입해 글자 뒤로 보내고, 테두리 선은 위에 얹는다. `left`/`width`는 이미 들여쓰기(geom)와
/// 단 오프셋을 반영한 상자의 좌변/폭. 문단이 페이지를 걸치면 조각마다 이 함수가 호출되며,
/// 걸친 경계쪽 상/하변 테두리는 `draw_top`/`draw_bottom`을 false로 주어 긋지 않는다(GC-9).
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
) {
    if bottom <= top || width <= 0.0 {
        return;
    }
    let Some(ps) = doc.header.para_shapes.get(para.para_shape.0 as usize) else {
        return;
    };
    // border_fill_id는 1-based(0 = 참조 없음).
    let Some(idx) = (ps.border_fill_id as usize).checked_sub(1) else {
        return;
    };
    let Some(bf) = doc.header.border_fills.get(idx) else {
        return;
    };
    // 배경(텍스트보다 뒤에 오도록 삽입).
    if let Some(item) = bg_fill_item(bf, left, top, width, bottom - top) {
        let ins = insert_idx.min(page.items.len());
        if !warnings.charge_display_items(1) {
            return;
        }
        page.items.insert(ins, item);
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
        return;
    }
    page.items.extend(items);
}

fn para_geometry(doc: &Document, para: &Paragraph) -> ParaGeom {
    // IR의 PARA_SHAPE 여백류(margin/indent/spacing)는 2×HWPUNIT — hwp5 저장 단위
    // (hwpx reader 실측 규칙: OWPML left=1500 → hwp5 ml=3000, read/header.rs 참조).
    // pt 환산은 /200.
    match doc.header.para_shapes.get(para.para_shape.0 as usize) {
        Some(p) => ParaGeom {
            left: (p.margin_left as f32 / 200.0).max(0.0),
            right: (p.margin_right as f32 / 200.0).max(0.0),
            // 내어쓰기(음수)를 보존한다 — 첫 줄 배치에서만 쓰고 페이지 좌변으로 클램프.
            first_indent: p.indent as f32 / 200.0,
            spacing_top: (p.spacing_top as f32 / 200.0).max(0.0),
            spacing_bottom: (p.spacing_bottom as f32 / 200.0).max(0.0),
        },
        None => ParaGeom::default(),
    }
}

fn items_width(items: &[InlineItem], tabs: &[f32]) -> f32 {
    let mut x = 0.0f32;
    for item in items {
        match item {
            InlineItem::Run(run) => x += run.width_pt,
            InlineItem::Tab => x = crate::tab::next_tab(tabs, x, TAB_INTERVAL_PT),
            InlineItem::LineBreak(_) => x = 0.0, // 새 줄 시작
        }
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
    };
    let stroked = |cmds: Vec<PathCmd>| Item::Path {
        commands: cmds,
        fill: None,
        stroke: Some(Stroke::solid(color, sw)),
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

/// Places inline items with greedy glyph-level wrapping at `max_width`.
///
/// An infinite width disables wrapping. The return value is the final baseline.
#[allow(clippy::too_many_arguments)]
fn place_wrapped(
    page: &mut PageList,
    items: Vec<InlineItem>,
    x0: f32,
    first_baseline_y: f32,
    max_width: f32,
    line_advance: f32,
    tabs: &[f32],
    first_indent: f32,
    border_fills: &[BorderFill],
    warnings: &mut RenderIssueAccumulator,
) -> f32 {
    let limit = x0 + max_width;
    // 첫 줄만 들여쓰기/내어쓰기(first_indent). 이후 줄·줄바꿈은 x0(문단 좌여백)로 복귀.
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

    for item in items {
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
            }
            InlineItem::Tab => {
                x = x0 + crate::tab::next_tab(tabs, x - x0, TAB_INTERVAL_PT);
                previous_no_break = false;
            }
            InlineItem::LineBreak(_) => {
                y += line_advance;
                x = x0;
                previous_no_break = false;
            }
        }
    }
    y
}

#[cfg(test)]
mod page_number_layout_tests {
    use super::page_number_alignment;

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
}

#[cfg(test)]
mod para_geom_tests {
    use super::para_geometry;
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
        // IR 여백류는 2×HWPUNIT → /200. 들여쓰기는 left와 분리(first_indent).
        assert_eq!(g.left, 20.0); // margin_left 4000 / 200
        assert_eq!(g.first_indent, 15.0); // indent 3000 / 200
        assert_eq!(g.right, 10.0);
        assert_eq!(g.spacing_top, 6.0);
        assert_eq!(g.spacing_bottom, 3.0);
        // 음수 들여쓰기(내어쓰기)는 보존한다(첫 줄이 나머지보다 왼쪽). /200.
        doc.header.para_shapes[0].indent = -1000;
        assert_eq!(para_geometry(&doc, &para).first_indent, -5.0);
        assert_eq!(para_geometry(&doc, &para).left, 20.0);
        // para_shape 범위 밖이면 0.
        let p2 = Paragraph {
            para_shape: ParaShapeId(99),
            ..Paragraph::default()
        };
        assert_eq!(para_geometry(&doc, &p2).left, 0.0);
    }
}

#[cfg(test)]
mod diagonal_tests {
    use super::diagonal_dirs;

    #[test]
    fn 대각선_방향_비트() {
        // 둘 다 0 → 대각선 없음.
        assert_eq!(diagonal_dirs(0), (false, false));
        // 3D/그림자(bit0,1)만 켜져도 대각선 아님.
        assert_eq!(diagonal_dirs(0b11), (false, false));
        // slash(bit2~4) → `/`.
        assert_eq!(diagonal_dirs(0x4), (true, false));
        // backSlash(bit5~7) → `\`.
        assert_eq!(diagonal_dirs(0x20), (false, true));
        // 둘 다(X자).
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
        // natural 30, seg_width 45 → slack 15, 마지막 제외 2개에 7.5씩.
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
        // "ab cd" 5글자, glyph 5개. 공백(인덱스2)에만 slack 전부.
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
        // "ab " 끝 공백 → 보이는 텍스트가 끝까지 닿도록 공백 없는 줄처럼 전 글자 분배.
        let mut items = vec![run_t("ab ", &[10.0, 10.0, 5.0])];
        align_line(&mut items, 0, 40.0, 25.0, false); // slack 15
        if let InlineItem::Run(r) = &items[0] {
            // 후행 공백(idx2)은 분배 제외, last_visible=1 → gap 1개(idx0)에 15.
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

    /// 병합 셀만 커버하는 열(col_span==1 셀 없음)이 병합 셀 폭에서 유도돼야 한다.
    #[test]
    fn 병합_열_폭_유도() {
        // row0: [col0 span2 w=200pt][col2 span1 w=100pt]
        // row1: [col0 span1 w=80pt][col1 span2 w=220pt]  → col1은 병합 셀에서만 유도.
        let cells = vec![
            cell(0, 0, 2, 20000),
            cell(2, 0, 1, 10000),
            cell(0, 1, 1, 8000),
            cell(1, 1, 2, 22000),
        ];
        let t = table(2, 3, cells);
        // layout_table와 동일하게 col_span==1로 초기화.
        let mut col_w = vec![0.0f32; 3];
        for c in &t.cells {
            if c.col_span == 1 {
                col_w[c.col as usize] = col_w[c.col as usize].max(c.width.to_pt() as f32);
            }
        }
        assert_eq!(col_w, vec![80.0, 0.0, 100.0]); // col1 미지

        derive_col_widths(&mut col_w, &t, f32::MAX); // 캡 무발동
        assert!((col_w[1] - 120.0).abs() < 0.01, "col1 유도값: {col_w:?}");
        assert!(col_w.iter().all(|w| *w > 0.0), "미지 열 없어야: {col_w:?}");
        assert!(
            (col_w.iter().sum::<f32>() - 300.0).abs() < 0.5,
            "총 폭=표 실제 폭 300pt: {col_w:?}"
        );
    }

    /// 정상 표(전부 col_span==1)는 열 폭이 불변이어야 한다(스케일 s=1).
    #[test]
    fn 정상_표_열폭_불변() {
        let cells = vec![
            cell(0, 0, 1, 10000),
            cell(1, 0, 1, 15000),
            cell(2, 0, 1, 5000),
        ];
        let t = table(1, 3, cells);
        let mut col_w = vec![100.0f32, 150.0, 50.0];
        derive_col_widths(&mut col_w, &t, f32::MAX); // 캡 무발동
        assert_eq!(col_w, vec![100.0, 150.0, 50.0]);
    }

    /// 표 실제 폭(300pt)이 가용 폭(150pt)을 넘으면 가용 폭에 맞춰 축소하되 비율 유지.
    #[test]
    fn 본문폭_초과_표_축소() {
        let cells = vec![
            cell(0, 0, 1, 10000),
            cell(1, 0, 1, 15000),
            cell(2, 0, 1, 5000),
        ];
        let t = table(1, 3, cells); // 실제 폭 300pt
        let mut col_w = vec![100.0f32, 150.0, 50.0];
        derive_col_widths(&mut col_w, &t, 150.0);
        let sum: f32 = col_w.iter().sum();
        assert!((sum - 150.0).abs() < 0.5, "가용 폭 150pt로 축소: {col_w:?}");
        // 상대 비율 2:3:1 유지.
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
            effect_flags: 0x1, // 그림자 효과
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

#[cfg(test)]
mod tab_width_tests {
    use std::sync::Arc;

    use super::{items_width, place_wrapped};
    use crate::display::{Item, PageList};
    use crate::fonts::LoadedFont;
    use crate::issues::RenderIssueAccumulator;
    use crate::shape::{Glyph, InlineItem, ShapedRun};

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

    /// Explicit tab stops must advance to the same x position in width
    /// estimation and final placement.
    #[test]
    fn explicit_tab_stops_match_width_estimation_and_placement() {
        let tabs = [150.0f32];
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
        // Three glyphs for "가 나" produce two marks because whitespace is skipped.
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
