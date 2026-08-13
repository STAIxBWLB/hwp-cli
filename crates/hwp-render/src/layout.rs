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

use hwp_model::{BorderFill, Control, Document, HwpUnit, PageDef, Paragraph, Section, Table};

use crate::display::{DisplayList, Item, PageList, PathCmd};
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
const TAB_INTERVAL_PT: f32 = 40.0;

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

/// 쪽 테두리 한 변(페이지 좌표, pt).
struct PageBorderEdge {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    color: u32,
    width: f32,
}

/// 쪽 테두리 사각형의 4변 중 그릴 변(line_type≠0)만 계산한다.
/// 테두리ID 유효범위 밖(0 포함)이거나 참조 BorderFill이 전 변 무테두리면 빈 벡터.
#[allow(clippy::too_many_arguments)]
fn build_page_border_edges(
    pbf: &PageBorderFill,
    border_fills: &[BorderFill],
    paper_w: f32,
    paper_h: f32,
    body_left: f32,
    body_top: f32,
    body_right: f32,
    body_bottom: f32,
) -> Vec<PageBorderEdge> {
    // id는 1-기반. 0이거나 범위 밖이면 무출력(기본 문서 불변).
    let Some(bf) = (pbf.border_fill_id as usize)
        .checked_sub(1)
        .and_then(|i| border_fills.get(i))
    else {
        return Vec::new();
    };
    // gap HWPUNIT → pt (/100). 순서: 왼/오/위/아래.
    let (gl, gr, gt, gb) = (
        pbf.gap[0] as f32 / 100.0,
        pbf.gap[1] as f32 / 100.0,
        pbf.gap[2] as f32 / 100.0,
        pbf.gap[3] as f32 / 100.0,
    );
    let (x1, y1, x2, y2) = if pbf.paper_relative {
        // 종이 기준: 용지 가장자리에서 gap만큼 안쪽 사각형(정품 실측 경로).
        (gl, gt, paper_w - gr, paper_h - gb)
    } else {
        // 쪽(본문) 기준: 본문 영역 가장자리에서 gap만큼 바깥(여백 쪽). 정품 표본
        // 부재 — 근사 구현(EVEN/ODD와 함께 후속 실측으로 정밀화).
        (
            body_left - gl,
            body_top - gt,
            body_right + gr,
            body_bottom + gb,
        )
    };
    // sides: [왼, 오른, 위, 아래]. 각 변을 해당 line_type≠0일 때만 긋는다.
    let seg = [
        (x1, y1, x1, y2), // 왼
        (x2, y1, x2, y2), // 오른
        (x1, y1, x2, y1), // 위
        (x1, y2, x2, y2), // 아래
    ];
    let mut edges = Vec::new();
    for (side, (sx1, sy1, sx2, sy2)) in bf.sides.iter().zip(seg) {
        if side.is_visible() {
            edges.push(PageBorderEdge {
                x1: sx1,
                y1: sy1,
                x2: sx2,
                y2: sy2,
                color: side.color,
                width: side.width_mm() * 72.0 / 25.4, // mm → pt
            });
        }
    }
    edges
}

/// 계산된 쪽 테두리 변들을 페이지 아이템 맨 앞에 삽입한다(텍스트/개체 뒤 = 뒤에 그림).
/// 변 기하는 구역당 1회 계산하고, Item::Line은 페이지마다 새로 만든다(Item는 Clone 불가).
fn prepend_page_borders(
    page: &mut PageList,
    edges: &[PageBorderEdge],
    warnings: &mut RenderIssueAccumulator,
) {
    if edges.is_empty() {
        return;
    }
    if !warnings.charge_display_items(edges.len()) {
        return;
    }
    let mut items: Vec<Item> = edges
        .iter()
        .map(|e| Item::Line {
            x1: e.x1,
            y1: e.y1,
            x2: e.x2,
            y2: e.y2,
            color: e.color,
            width: e.width,
        })
        .collect();
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
        furniture.render(doc, store, page, self.visible_number(), warnings);
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
    push_run(page, x, y, run, warnings);
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

        // 쪽 테두리(PAGE_BORDER_FILL BOTH): 변 기하를 구역당 1회 계산해 두고,
        // 구역의 모든 페이지에 소급 삽입한다(테두리ID 무테두리·미존재면 빈 벡터).
        let page_border_edges = section_page_border_fill(section)
            .map(|pbf| {
                build_page_border_edges(
                    &pbf,
                    &doc.header.border_fills,
                    w,
                    h,
                    body_left,
                    body_top,
                    body_left + body_width,
                    body_bottom,
                )
            })
            .unwrap_or_default();

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

        // 머리말/꼬리말: 구역에서 처음 정의된 것을 모든 페이지에 반복
        let mut header_ctrl = None;
        let mut footer_ctrl = None;
        for para in &section.paragraphs {
            for c in &para.controls {
                if let Control::Generic(g) = c {
                    match &g.ctrl_id {
                        b"head" if header_ctrl.is_none() => header_ctrl = Some(g),
                        b"foot" if footer_ctrl.is_none() => footer_ctrl = Some(g),
                        _ => {}
                    }
                }
            }
        }
        let furniture = Furniture {
            header: header_ctrl,
            footer: footer_ctrl,
            page_def: &page_def,
            body_left,
            body_width,
        };

        // 각주/미주: 구역 전체에 번호를 매기고, 페이지마다 앵커가 든 노트를 모아
        // 하단에 그린다.
        let notes = footnote::collect_notes(&section.paragraphs);
        let mut page_notes: Vec<&Note> = Vec::new();
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
            let marks = footnote::para_marks(&notes, para);
            page_notes.extend(footnote::para_notes(&notes, para));
            let tabs = crate::tab::tab_stops(doc, para);
            let geom = para_geometry(doc, para);
            let links = crate::shape::hyperlink_ranges(para);
            // 목록 마커(불릿/번호) — 문서 순서로 카운터 진행(목록 아니면 None).
            let marker = list_state.marker(doc, para);

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
                    let natural = items_width(&items);
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
                        warnings,
                    );
                    content_bottom = last_y + max_size * 0.4 + geom.spacing_bottom;
                }
                content_bottom = layout_para_objects(
                    doc,
                    store,
                    &mut page,
                    para,
                    body_left,
                    para_top.unwrap_or(content_bottom),
                    content_bottom,
                    body_width,
                    warnings,
                );
                // 폴백 문단은 페이지를 걸치지 않는다(단일 조각 — 상·하 테두리 모두).
                if let Some(top) = para_top {
                    draw_para_bg_slice(
                        doc,
                        &mut page,
                        para,
                        body_left + geom.left,
                        (body_width - geom.left - geom.right).max(1.0),
                        bg_slice_insert,
                        top,
                        content_bottom,
                        true,
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
                let natural_width: f32 = items_width(&items);

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
                    warnings,
                );
                content_bottom = last_y + (line_height_pt - baseline_gap_pt).max(0.0);
            }

            content_bottom = layout_para_objects(
                doc,
                store,
                &mut page,
                para,
                body_left,
                para_top.unwrap_or(content_bottom),
                content_bottom,
                body_width,
                warnings,
            );
            // 마지막(또는 유일) 배경 조각: 하변 테두리 O, 상변은 첫 조각일 때만(=경계 안 걸침).
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
        if !push_page_checked(&mut pages, &mut page, None, warnings) {
            return DisplayList { pages };
        }

        // 쪽 테두리를 이 구역의 모든 페이지 맨 앞(뒤에 그림)에 소급 삽입한다.
        if !page_border_edges.is_empty() {
            for p in &mut pages[section_first_page..] {
                prepend_page_borders(p, &page_border_edges, warnings);
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

/// 기본 셀 안쪽 여백 (HWPUNIT — 한글 기본값).
const DEFAULT_CELL_MARGINS: [u16; 4] = [510, 510, 141, 141];

/// 페이지 가구 (머리말/꼬리말) — 페이지 마감 시마다 그린다.
struct Furniture<'a> {
    header: Option<&'a hwp_model::GenericControl>,
    footer: Option<&'a hwp_model::GenericControl>,
    page_def: &'a PageDef,
    body_left: f32,
    body_width: f32,
}

impl Furniture<'_> {
    fn render(
        &self,
        doc: &Document,
        store: &mut FontStore,
        page: &mut PageList,
        page_number: Option<u32>,
        warnings: &mut RenderIssueAccumulator,
    ) {
        if let Some(h) = self.header {
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
        if let Some(f) = self.footer {
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
    page.items.push(Item::Line {
        x1: body_left,
        y1: top - sep_gap,
        x2: body_left + body_width * 0.34,
        y2: top - sep_gap,
        color: 0x0000_0000,
        width: 0.5,
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
        push_run(page, x, baseline, run, warnings);
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
        push_run(page, x, baseline, run, warnings);
    }
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
        Item::Line {
            x1,
            y1,
            x2,
            y2,
            color,
            width,
        } => Item::Line {
            x1: x1 + dx,
            y1: y1 + dy,
            x2: x2 + dx,
            y2: y2 + dy,
            color,
            width,
        },
        Item::Image { x, y, w, h, data } => Item::Image {
            x: x + dx,
            y: y + dy,
            w,
            h,
            data,
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
    warnings: &mut RenderIssueAccumulator,
) -> f32 {
    let mut bottom = content_bottom;
    let mut object_y = anchor_top;

    for control in &para.controls {
        match control {
            Control::Table(table) => {
                let h = layout_table(doc, store, page, table, x, object_y, avail_width, warnings);
                bottom = bottom.max(object_y + h);
                object_y += h; // 한 문단에 개체가 여럿이면 세로로 이어 배치
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
                            return bottom;
                        }
                        page.items.push(Item::Image {
                            x,
                            y: object_y,
                            w,
                            h,
                            data: warnings.cached_binary(bytes),
                        });
                        bottom = bottom.max(object_y + h);
                        object_y += h;
                    }
                    None => warnings.push(
                        RenderIssueCode::ImageDataMissingOmitted,
                        format!("{:?}", pic.bin_ref),
                    ),
                }
            }
            // 글상자(text box): 텍스트 있는 gso 개체의 내부 문단을 박스 영역에 배치.
            Control::Generic(g) if g.ctrl_id == *b"gso " && !g.paragraph_lists.is_empty() => {
                let Some(b) = crate::gso::parse_gso_box(&g.data) else {
                    warnings.push(RenderIssueCode::TextBoxGeometryInvalidOmitted, b"gso");
                    continue;
                };
                let bw = (b.width as f32 / 100.0).max(8.0);
                let bh = b.height as f32 / 100.0;
                // 글자처럼취급=흐름 위치, 떠 있음=PAPER/PAGE 기준 페이지 절대 위치.
                let (bx, by, inline) = if b.treat_as_char() {
                    (x, object_y, true)
                } else {
                    (
                        b.horz_offset as f32 / 100.0,
                        b.vert_offset as f32 / 100.0,
                        false,
                    )
                };

                // 글상자 자체 테두리/배경(사각형 프레임)을 텍스트 뒤에 먼저 그린다.
                let frame_origin = if inline {
                    (bx as f64 * 100.0, by as f64 * 100.0)
                } else {
                    (b.horz_offset as f64, b.vert_offset as f64)
                };
                crate::shape_draw::draw_gso_shapes(g, frame_origin, doc, page, warnings);

                // 다단/연결 글상자: 내부 문단의 v_pos 리셋(단 나누기)으로 단을 분할한다.
                // 단 0은 이 박스, 단 1+는 연결 글상자(같은 크기·세로위치, 더 오른쪽
                // 떠 있는 gso 박스) 위치로 흐른다. 없으면 가로로 한 단 진행(근사).
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
                        flat[range.clone()].iter().copied(),
                        cx,
                        cy,
                        bw,
                        warnings,
                        None,
                        None,
                    );
                    max_bottom = max_bottom.max(inner);
                }

                if inline {
                    let used = (max_bottom - by).max(bh);
                    bottom = bottom.max(by + used);
                    object_y += used;
                }
            }
            // 순수 도형 (텍스트 없는 gso): 선/사각형/타원/호/다각형.
            Control::Generic(g)
                if g.ctrl_id == *b"gso "
                    && g.paragraph_lists.is_empty()
                    && crate::shape_draw::has_shape(&g.raw_children) =>
            {
                let Some(b) = crate::gso::parse_gso_box(&g.data) else {
                    continue;
                };
                let origin = if b.treat_as_char() {
                    (x as f64 * 100.0, object_y as f64 * 100.0)
                } else {
                    (b.horz_offset as f64, b.vert_offset as f64)
                };
                crate::shape_draw::draw_gso_shapes(g, origin, doc, page, warnings);
            }
            // hwpx 구조화 도형(rect/ellipse/line/polygon/curve) — 글상자 텍스트 포함.
            Control::Generic(g) if !g.gso_shapes.is_empty() => {
                // 글자처럼(anchored) 도형은 흐름 위치로 이동(clone-조정 — 원본 불변).
                let adjusted: Vec<hwp_model::ShapeGeom> = g
                    .gso_shapes
                    .iter()
                    .map(|s| {
                        let mut s2 = s.clone();
                        if s.anchored {
                            s2.x = (x * 100.0) as i32;
                            s2.y = (object_y * 100.0) as i32;
                        }
                        s2
                    })
                    .collect();
                crate::shape_draw::draw_ir_shapes(&adjusted, page, warnings);
                // 글상자 텍스트: 첫 도형 bbox 안에 배치(v1 단일 단 — hwp5 arm의 다단은 미지원).
                if !g.paragraph_lists.is_empty() {
                    let s0 = &adjusted[0];
                    let (bx, by) = (s0.x as f32 / 100.0, s0.y as f32 / 100.0);
                    let bw = (s0.w as f32 / 100.0).max(8.0);
                    let bh = s0.h as f32 / 100.0;
                    let flat = g.paragraph_lists.iter().flat_map(|l| l.paragraphs.iter());
                    let inner = layout_box_para_iter(
                        doc, store, page, flat, bx, by, bw, warnings, None, None,
                    );
                    if s0.anchored {
                        // 흐름 전진(hwp5 인라인 글상자와 동형).
                        let used = (inner - by).max(bh);
                        bottom = bottom.max(by + used);
                        object_y += used;
                    }
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
    bottom
}

/// 표 하나를 (x, y)에 배치하고 높이를 반환한다.
/// 셀 여백 (왼/오른/위/아래) pt — 셀 지정 → 표 기본 → 한글 기본.
fn cell_margins(table: &Table, cell: &hwp_model::Cell) -> (f32, f32, f32, f32) {
    let m = if cell.margins.iter().any(|&v| v > 0) {
        cell.margins
    } else if table.inner_margins.iter().any(|&v| v > 0) {
        table.inner_margins
    } else {
        DEFAULT_CELL_MARGINS
    };
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
    warnings: &mut RenderIssueAccumulator,
) -> f32 {
    let cols = table.cols.max(1) as usize;
    let rows = table.rows.max(1) as usize;

    // 그리드 기하: span=1 셀에서 열 폭/행 높이를 확정, 모르는 칸은 평균으로
    let mut col_w = vec![0.0f32; cols];
    let mut row_h = vec![0.0f32; rows];
    for cell in &table.cells {
        let (c, r) = (cell.col as usize, cell.row as usize);
        if cell.col_span == 1 && c < cols {
            col_w[c] = col_w[c].max(cell.width.to_pt() as f32);
        }
        if cell.row_span == 1 && r < rows {
            row_h[r] = row_h[r].max(cell.height.to_pt() as f32);
        }
    }
    derive_col_widths(&mut col_w, table, avail_width);
    fill_unknown(&mut row_h, 18.0);

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

    // 누적 오프셋
    let col_x: Vec<f32> = prefix_sums(&col_w, x);
    let row_y: Vec<f32> = prefix_sums(&row_h, y);

    // 표 셀 안 목록 카운터 — 이 표 안에서 셀 순서로 진행(표마다 리셋).
    let mut cell_ls = crate::list::ListState::default();
    for (ci, cell) in table.cells.iter().enumerate() {
        let (c, r) = (cell.col as usize, cell.row as usize);
        if c >= cols || r >= rows {
            warnings.push(RenderIssueCode::InvalidTableCellOmitted, format!("{r}:{c}"));
            continue;
        }
        let cx = col_x[c];
        let cy = row_y[r];
        let cw: f32 = col_w[c..(c + cell.col_span as usize).min(cols)]
            .iter()
            .sum();
        let ch: f32 = row_h[r..(r + cell.row_span as usize).min(rows)]
            .iter()
            .sum();

        let border_fill = doc
            .header
            .border_fills
            .get((cell.border_fill.0 as usize).saturating_sub(1));

        // 1) 배경
        if let Some(bg) = border_fill.and_then(|bf| bf.visible_bg()) {
            if !warnings.charge_display_items(1) {
                return 0.0;
            }
            page.items.push(Item::Rect {
                x: cx,
                y: cy,
                w: cw,
                h: ch,
                fill: bg,
            });
        }

        // 2) 내용 — 셀 여백 + 세로정렬(list_attr bits5~6: 0=위, 1=가운데, 2=아래).
        //    측정 패스의 실측 내용 높이로 남는 공간을 계산해 오프셋한다.
        let (ml, mr, mt, mb) = cell_margins(table, cell);
        let content_h = content_h_by_cell.get(ci).copied().unwrap_or(0.0);
        let avail = (ch - mt - mb - content_h).max(0.0);
        let voff = match (cell.list_attr >> 5) & 0x3 {
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
            Some(&mut cell_ls), // 렌더 패스: 셀 목록 마커 그림
            None,
        );

        // 3) 테두리 (왼/오른/위/아래)
        if let Some(bf) = border_fill {
            let edges = [
                (cx, cy, cx, cy + ch),           // 왼
                (cx + cw, cy, cx + cw, cy + ch), // 오른
                (cx, cy, cx + cw, cy),           // 위
                (cx, cy + ch, cx + cw, cy + ch), // 아래
            ];
            for (side, (x1, y1, x2, y2)) in bf.sides.iter().zip(edges) {
                if side.is_visible() {
                    if !warnings.charge_display_items(1) {
                        return 0.0;
                    }
                    page.items.push(Item::Line {
                        x1,
                        y1,
                        x2,
                        y2,
                        color: side.color,
                        width: side.width_mm() * 72.0 / 25.4, // mm → pt
                    });
                }
            }

            // 4) 대각선/역대각선 — slash/backSlash 비트가 켜졌을 때만(병합 셀은 전체 영역 가로지름).
            //    diagonal 선은 스타일/색만 제공하므로 방향 비트가 없으면 그리지 않는다.
            let (slash, backslash) = diagonal_dirs(bf.attr);
            if (slash || backslash) && bf.diagonal.is_visible() {
                let dw = bf.diagonal.width_mm() * 72.0 / 25.4;
                if backslash {
                    if !warnings.charge_display_items(1) {
                        return 0.0;
                    }
                    page.items.push(Item::Line {
                        x1: cx,
                        y1: cy,
                        x2: cx + cw,
                        y2: cy + ch,
                        color: bf.diagonal.color,
                        width: dw,
                    });
                }
                if slash {
                    if !warnings.charge_display_items(1) {
                        return 0.0;
                    }
                    page.items.push(Item::Line {
                        x1: cx,
                        y1: cy + ch,
                        x2: cx + cw,
                        y2: cy,
                        color: bf.diagonal.color,
                        width: dw,
                    });
                }
            }
        }
    }
    row_h.iter().sum()
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
        paras.iter(),
        origin_x,
        origin_y,
        width,
        warnings,
        list_state,
        page_number,
    )
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
    paras: impl Iterator<Item = &'a Paragraph>,
    origin_x: f32,
    origin_y: f32,
    width: f32,
    warnings: &mut RenderIssueAccumulator,
    mut list_state: Option<&mut crate::list::ListState>,
    page_number: Option<u32>,
) -> f32 {
    let mut content_bottom = origin_y;
    // 흐름 하한: 캐시 줄은 올리지 않고, 흐름 배치 콘텐츠만 올린다 (함수 doc 참고).
    let mut flow_floor = origin_y;
    for para in paras {
        let mut para_top: Option<f32> = None;
        let tabs = crate::tab::tab_stops(doc, para);
        // 목록 마커(셀 안 번호/불릿) — 렌더 패스에서만 counter 진행(측정 패스는 None).
        let marker = list_state
            .as_deref_mut()
            .and_then(|ls| ls.marker(doc, para));

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
                let natural = items_width(&items);
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
                    warnings,
                );
                content_bottom = last_y + max_size * 0.4 + geom.spacing_bottom;
            }
            // 폴백(캐시 없는) 문단은 흐름 배치 — 이후 캐시 문단이 넘지 않게 바닥을 올린다.
            flow_floor = flow_floor.max(content_bottom);
        } else {
            let last_content = last_content_seg(para);
            for (i, seg) in para.line_segs.iter().enumerate() {
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
                let natural_width = items_width(&items);

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
                let stored = origin_y + (seg.v_pos + seg.baseline_gap) as f32 / 100.0;
                // 캐시 v_pos를 존중: 흐름 하한 위로만 보정(흐름 커서로 끌어내리지 않음).
                let baseline_y = stored.max(flow_floor + gap_pt);
                if i == 0 {
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
        let before_objects = content_bottom;
        content_bottom = layout_para_objects(
            doc,
            store,
            page,
            para,
            origin_x,
            para_top.unwrap_or(content_bottom),
            content_bottom,
            width,
            warnings,
        );
        if content_bottom > before_objects {
            flow_floor = flow_floor.max(content_bottom);
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

/// 정렬에 따른 가로 shift(pt). 양쪽/배분/나눔(0/4/5)이고 마지막 줄이 아니면
/// items의 글리프 advance를 늘려 줄을 seg_width까지 채우고 shift 0을 반환한다.
fn align_line(
    items: &mut [InlineItem],
    align: u8,
    seg_width: f32,
    natural: f32,
    is_last: bool,
) -> f32 {
    match align {
        2 => (seg_width - natural).max(0.0),         // 오른쪽
        3 => ((seg_width - natural) / 2.0).max(0.0), // 가운데
        0 | 4 | 5 if !is_last => {
            // 잉여 폭 분배. 폰트 부재 등으로 natural이 비정상이면 캡(≤100% stretch)으로 폭주 방지.
            let slack = (seg_width - natural).max(0.0).min(natural.max(1.0));
            justify_line(items, slack);
            0.0
        }
        _ => 0.0,
    }
}

/// 양쪽 정렬: 잉여 폭 slack을 분배. 줄에 **공백이 있으면 공백에만**(단어 사이 벌림),
/// 없으면 전 글자 사이에 균등 분배한다. 후행 공백(마지막 보이는 글리프 뒤)에는 분배하지
/// 않아 보이는 텍스트가 오른쪽 끝에 닿도록 한다. 글리프↔글자는 CJK 1:1 가정.
fn justify_line(items: &mut [InlineItem], slack: f32) {
    if slack <= 0.0 {
        return;
    }
    // 줄 전체 글리프의 공백 여부(런 내 글자 순서 매핑).
    let mut is_space: Vec<bool> = Vec::new();
    for item in items.iter() {
        if let InlineItem::Run(run) = item {
            let mut chars = run.text.chars();
            for _ in 0..run.glyphs.len() {
                is_space.push(chars.next().is_some_and(|c| c.is_whitespace()));
            }
        }
    }
    let total = is_space.len();
    if total < 2 {
        return;
    }
    // 마지막 보이는(비공백) 글리프 — 그 이후엔 분배하지 않는다.
    let last_visible = is_space.iter().rposition(|&s| !s).unwrap_or(total - 1);
    let space_count = is_space[..last_visible].iter().filter(|&&s| s).count();

    // 공백 우선; 없으면 전 글자 사이(마지막 보이는 글리프 전까지의 gap).
    let use_spaces = space_count > 0;
    let denom = if use_spaces {
        space_count as f32
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
    if let Some(fill) = bf.visible_bg() {
        let ins = insert_idx.min(page.items.len());
        if !warnings.charge_display_items(1) {
            return;
        }
        page.items.insert(
            ins,
            Item::Rect {
                x: left,
                y: top,
                w: width,
                h: bottom - top,
                fill,
            },
        );
    }
    // 테두리: 좌/우변은 항상, 상/하변은 페이지 경계에 걸치지 않은 쪽만(가장자리라 위에 얹어도 무해).
    let edges = [
        (&bf.sides[0], (left, top, left, bottom), true),
        (
            &bf.sides[1],
            (left + width, top, left + width, bottom),
            true,
        ),
        (&bf.sides[2], (left, top, left + width, top), draw_top),
        (
            &bf.sides[3],
            (left, bottom, left + width, bottom),
            draw_bottom,
        ),
    ];
    for (side, (x1, y1, x2, y2), enabled) in edges {
        if enabled && side.is_visible() {
            if !warnings.charge_display_items(1) {
                return;
            }
            page.items.push(Item::Line {
                x1,
                y1,
                x2,
                y2,
                color: side.color,
                width: side.width_mm() * 72.0 / 25.4,
            });
        }
    }
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

fn items_width(items: &[InlineItem]) -> f32 {
    let mut x = 0.0f32;
    for item in items {
        match item {
            InlineItem::Run(run) => x += run.width_pt,
            InlineItem::Tab => {
                x = (x / TAB_INTERVAL_PT).floor() * TAB_INTERVAL_PT + TAB_INTERVAL_PT
            }
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

/// 글리프 런과 그 장식(밑줄/취소선)을 함께 배치한다.
/// 장식 상수(0.10em/0.25em/0.05em)는 U5 실측 전 초기값.
pub(crate) fn push_run(
    page: &mut PageList,
    x: f32,
    y: f32,
    run: crate::shape::ShapedRun,
    warnings: &mut RenderIssueAccumulator,
) {
    let w = run.width_pt;
    let em = run.size_pt;
    let underline = run.underline.then(|| {
        let color = if run.underline_color == 0xFFFF_FFFF {
            run.color
        } else {
            run.underline_color
        };
        (y + em * 0.10, color)
    });
    let strike = run.strike.then_some((y - em * 0.25, run.color));
    if !warnings.charge_display_items(1) {
        return;
    }
    page.items.push(Item::Glyphs { x, y, run });
    for (ly, color) in underline.into_iter().chain(strike) {
        if !warnings.charge_display_items(1) {
            return;
        }
        page.items.push(Item::Line {
            x1: x,
            y1: ly,
            x2: x + w,
            y2: ly,
            color,
            width: em * 0.05,
        });
    }
}

/// 인라인 항목들을 배치한다. `max_width`를 넘으면 글리프 단위 그리디
/// 줄바꿈(`f32::INFINITY`면 비활성). 마지막 베이스라인 y를 반환한다.
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
    warnings: &mut RenderIssueAccumulator,
) -> f32 {
    let limit = x0 + max_width;
    // 첫 줄만 들여쓰기/내어쓰기(first_indent). 이후 줄·줄바꿈은 x0(문단 좌여백)로 복귀.
    let mut x = x0 + first_indent;
    let mut y = first_baseline_y;

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
                if max_width.is_infinite() || x + run.width_pt <= limit {
                    let w = run.width_pt;
                    push_run(page, x, y, run, warnings);
                    x += w;
                    continue;
                }
                // 글리프 단위 분할 (CJK는 글자 사이 어디서나 분리 가능)
                let mut start = 0usize;
                let mut piece_x = x;
                let mut acc = 0.0f32;
                for (i, g) in run.glyphs.iter().enumerate() {
                    let over = piece_x + acc + g.x_advance > limit;
                    let line_has_content = i > start || piece_x > x0;
                    if over && line_has_content {
                        if i > start {
                            let piece = run.slice(start, i);
                            push_run(page, piece_x, y, piece, warnings);
                        }
                        y += line_advance;
                        piece_x = x0;
                        acc = 0.0;
                        start = i;
                    }
                    acc += g.x_advance;
                }
                if start < run.glyphs.len() {
                    let piece = run.slice(start, run.glyphs.len());
                    let w = piece.width_pt;
                    push_run(page, piece_x, y, piece, warnings);
                    x = piece_x + w;
                } else {
                    x = piece_x;
                }
            }
            InlineItem::Tab => {
                x = x0 + crate::tab::next_tab(tabs, x - x0, TAB_INTERVAL_PT);
            }
            InlineItem::LineBreak(_) => {
                y += line_advance;
                x = x0;
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
            underline: false,
            strike: false,
            underline_color: 0xFFFF_FFFF,
            shade_color: 0xFFFF_FFFF,
            shadow: None,
            outline: false,
            emboss: false,
            engrave: false,
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
            bin_ref: BinRef::ItemRef("shared-image".to_string()),
            extras: Vec::new(),
        });
        document.sections[0].paragraphs[0].controls = vec![picture; 300];
        assert_budget_failure(&document, LayoutBudget::certification());
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
            bin_ref: BinRef::ItemRef("one-byte-image".to_string()),
            extras: Vec::new(),
        });
        let mut paragraph = document.sections[0].paragraphs[0].clone();
        paragraph.controls = vec![picture];
        document.sections[0].paragraphs = vec![paragraph; 20_000];
        assert_budget_failure(&document, LayoutBudget::certification());
    }
}
