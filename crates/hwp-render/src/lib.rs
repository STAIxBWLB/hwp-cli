//! IR → PNG/SVG/PDF 페이지 렌더러.
//!
//! 파이프라인: IR → Layout([`layout`] — LineSegLayouter) →
//! [`display::DisplayList`] → 백엔드([`png`] tiny-skia / [`svg`] / [`pdf`]).
//! 세 백엔드 모두 같은 DisplayList를 소비한다.
//!
//! v1 범위: lineseg 기반 텍스트 렌더링 (굵게/기울임/크기/색/자간/장평,
//! 가운데/오른쪽 정렬). 표·이미지·장식은 M5.

mod border;
pub mod diff;
pub mod display;
pub mod equation;
pub mod error;
pub mod fonts;
pub mod footnote;
pub mod gso;
pub mod issues;
pub mod layout;
pub mod lineseg;
pub mod list;
mod page_number;
pub mod pdf;
pub mod png;
pub mod shape;
pub mod shape_draw;
pub mod svg;
pub mod tab;

use hwp_model::{Document, Metadata};

pub use diff::{DiffReport, compare};
pub use error::RenderError;
pub use fonts::{FontResolution, FontResolutionOutcome, FontStore};
pub use issues::{
    FontCoverage, RenderIssueAccumulator, RenderIssueCode, RenderIssueReport, RenderIssueSeverity,
    RenderIssueStage, RenderIssueSummary,
};

/// 지원 렌더 해상도 범위. 너무 낮은 값은 무의미한 1px 출력을 만들고, 너무 높은
/// 값은 작은 문서도 과도한 래스터 메모리를 요구하므로 모든 호출 경로에서 공통 적용한다.
pub const MIN_DPI: f32 = 36.0;
pub const MAX_DPI: f32 = 600.0;

pub fn validate_dpi(dpi: f32) -> Result<f32, RenderError> {
    if !dpi.is_finite() || !(MIN_DPI..=MAX_DPI).contains(&dpi) {
        return Err(RenderError::Backend(format!(
            "DPI는 유한한 {MIN_DPI}..={MAX_DPI} 범위여야 합니다: {dpi}"
        )));
    }
    Ok(dpi)
}

pub struct RenderOptions {
    pub dpi: f32,
    /// 추가 폰트 디렉터리
    pub font_dirs: Vec<std::path::PathBuf>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            dpi: 96.0,
            font_dirs: Vec::new(),
        }
    }
}

pub struct RenderOutput {
    /// 페이지별 래스터 (PNG 인코딩 전)
    pub pages: Vec<tiny_skia::Pixmap>,
    /// 레이아웃된 전체 페이지 수. 선택 렌더에서는 `pages.len()`보다 클 수 있다.
    pub total_pages: usize,
    /// 원문을 보관하지 않는 source-bounded 렌더 진단.
    pub report: RenderIssueReport,
    /// 선택된 페이지에서 관측한 구조화 진단. `not_detected`는 해당 렌더러의 보수적
    /// 검사에서 발견하지 못했다는 뜻이며 다른 renderer와의 동일성을 보증하지 않는다.
    pub diagnostics: RenderDiagnostics,
}

#[derive(Debug, Clone)]
pub struct RenderDiagnostics {
    pub fonts: Vec<FontResolution>,
    pub font_resolution_complete: bool,
    pub pages: Vec<PageDiagnostics>,
}

#[derive(Debug, Clone)]
pub struct PageDiagnostics {
    pub page: usize,
    pub width_pt: f32,
    pub height_pt: f32,
    pub item_count: usize,
    pub visually_blank: bool,
    pub outside_page_bounds: Vec<ItemBounds>,
    pub outside_page_bounds_count: usize,
    pub outside_page_bounds_complete: bool,
    pub possible_collisions: Vec<Collision>,
    pub possible_collision_count: usize,
    pub possible_collision_complete: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ItemBounds {
    pub item: usize,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Collision {
    pub first_item: usize,
    pub second_item: usize,
}

pub struct SvgOutput {
    /// 페이지별 SVG 문서
    pub pages: Vec<String>,
    pub report: RenderIssueReport,
}

fn build_display_list(
    doc: &Document,
    opts: &RenderOptions,
) -> (
    display::DisplayList,
    RenderIssueAccumulator,
    Vec<FontResolution>,
    bool,
) {
    build_display_list_with_font_scope(doc, opts, true, &[])
}

fn build_display_list_with_font_scope(
    doc: &Document,
    opts: &RenderOptions,
    system_fonts: bool,
    font_files: &[std::path::PathBuf],
) -> (
    display::DisplayList,
    RenderIssueAccumulator,
    Vec<FontResolution>,
    bool,
) {
    let mut store = if system_fonts {
        FontStore::new()
    } else {
        FontStore::new_isolated()
    };
    for dir in &opts.font_dirs {
        store.load_dir(dir);
    }
    for file in font_files {
        if let Err(error) = store.load_file(file) {
            store
                .issues
                .push(RenderIssueCode::FontManifestLoadFailed, format!("{error}"));
            store.resolutions_complete = false;
        }
    }
    let mut issues = RenderIssueAccumulator::new();
    let list = layout::layout_document(doc, &mut store, &mut issues);
    issues.absorb(store.issues.finish());
    (list, issues, store.resolutions, store.resolutions_complete)
}

fn build_display_list_with_font_scope_bounded(
    doc: &Document,
    opts: &RenderOptions,
    font_files: &[std::path::PathBuf],
    budget: &layout::LayoutBudget,
) -> Result<
    (
        display::DisplayList,
        RenderIssueAccumulator,
        Vec<FontResolution>,
        bool,
    ),
    RenderError,
> {
    let mut store = FontStore::new_isolated();
    for dir in &opts.font_dirs {
        store.load_dir(dir);
    }
    for file in font_files {
        if let Err(error) = store.load_file(file) {
            store
                .issues
                .push(RenderIssueCode::FontManifestLoadFailed, format!("{error}"));
            store.resolutions_complete = false;
        }
    }
    let mut issues = RenderIssueAccumulator::new();
    let list = layout::layout_document_bounded(doc, &mut store, &mut issues, budget)?;
    issues.absorb(store.issues.finish());
    Ok((list, issues, store.resolutions, store.resolutions_complete))
}

/// 문서 전체를 PNG(래스터)로 렌더링한다.
pub fn render_document(doc: &Document, opts: &RenderOptions) -> Result<RenderOutput, RenderError> {
    render_document_pages(doc, opts, None)
}

/// 문서에서 선택한 1-기반 페이지만 PNG로 렌더링한다. `None`이면 전체 페이지.
pub fn render_document_pages(
    doc: &Document,
    opts: &RenderOptions,
    pages: Option<&[usize]>,
) -> Result<RenderOutput, RenderError> {
    validate_dpi(opts.dpi)?;
    let (list, mut report, fonts, font_resolution_complete) = build_display_list(doc, opts);
    let total_pages = list.pages.len();
    let selected = pages
        .map(|values| values.to_vec())
        .unwrap_or_else(|| (1..=total_pages).collect());
    let pages = png::render_png_pages_with_issues(&list, opts.dpi, Some(&selected), &mut report)?;
    let diagnostics = diagnose_pages(&list, &selected, &pages, fonts, font_resolution_complete);
    Ok(RenderOutput {
        pages,
        total_pages,
        report: report.finish(),
        diagnostics,
    })
}

/// 인증 전용 격리 렌더. 시스템 글꼴을 탐색하지 않고 `opts.font_dirs`만 사용한다.
/// 선택/픽셀/DPI 상한은 일반 렌더와 동일하다.
pub fn render_document_pages_isolated(
    doc: &Document,
    opts: &RenderOptions,
    pages: Option<&[usize]>,
    font_files: &[std::path::PathBuf],
) -> Result<RenderOutput, RenderError> {
    validate_dpi(opts.dpi)?;
    let (list, mut report, fonts, font_resolution_complete) =
        build_display_list_with_font_scope(doc, opts, false, font_files);
    let total_pages = list.pages.len();
    let selected = pages
        .map(|values| values.to_vec())
        .unwrap_or_else(|| (1..=total_pages).collect());
    let rendered =
        png::render_png_pages_with_issues(&list, opts.dpi, Some(&selected), &mut report)?;
    let diagnostics = diagnose_pages(&list, &selected, &rendered, fonts, font_resolution_complete);
    Ok(RenderOutput {
        pages: rendered,
        total_pages,
        report: report.finish(),
        diagnostics,
    })
}

/// 인증 전용 레이아웃 예산을 적용한 격리 렌더.
pub fn render_document_pages_isolated_bounded(
    doc: &Document,
    opts: &RenderOptions,
    pages: Option<&[usize]>,
    font_files: &[std::path::PathBuf],
    budget: &layout::LayoutBudget,
) -> Result<RenderOutput, RenderError> {
    validate_dpi(opts.dpi)?;
    let (list, mut report, fonts, font_resolution_complete) =
        build_display_list_with_font_scope_bounded(doc, opts, font_files, budget)?;
    let total_pages = list.pages.len();
    let selected = pages
        .map(|values| values.to_vec())
        .unwrap_or_else(|| (1..=total_pages).collect());
    let rendered =
        png::render_png_pages_with_issues(&list, opts.dpi, Some(&selected), &mut report)?;
    let diagnostics = diagnose_pages(&list, &selected, &rendered, fonts, font_resolution_complete);
    Ok(RenderOutput {
        pages: rendered,
        total_pages,
        report: report.finish(),
        diagnostics,
    })
}

/// 기준 PNG를 픽스맵으로 읽는다 (`hwp diff`의 기준 이미지 로드용).
pub fn load_png(path: &std::path::Path) -> Result<tiny_skia::Pixmap, RenderError> {
    tiny_skia::Pixmap::load_png(path)
        .map_err(|e| RenderError::Backend(format!("PNG 로드 실패 ({}): {e}", path.display())))
}

/// 문서 전체를 SVG로 렌더링한다.
pub fn render_document_svg(doc: &Document, opts: &RenderOptions) -> SvgOutput {
    let (list, report, _, _) = build_display_list(doc, opts);
    SvgOutput {
        pages: svg::render_svg(&list),
        report: report.finish(),
    }
}

pub struct PdfOutput {
    /// 단일 멀티페이지 PDF 바이트
    pub data: Vec<u8>,
    /// 경고 + 폰트 해석 리포트
    pub report: RenderIssueReport,
}

/// PDF output plus the pre-serialization text sequence used by corpus
/// validation. Existing PDF render APIs intentionally keep returning
/// [`PdfOutput`].
pub struct PdfValidationOutput {
    pub data: Vec<u8>,
    pub report: RenderIssueReport,
    pub expected_text: String,
}

/// 문서를 단일 멀티페이지 PDF로 렌더링한다 (폰트 임베드 + 검색 가능 텍스트).
/// `pages`는 1-기반 페이지 번호 목록; `None`이면 전체 페이지.
pub fn render_document_pdf(
    doc: &Document,
    opts: &RenderOptions,
    pages: Option<&[usize]>,
) -> Result<PdfOutput, RenderError> {
    let (list, report, _, _) = build_display_list(doc, opts);
    finish_pdf(list, report, pages, &doc.metadata)
}

/// Isolated PDF render for certification. It searches only `font_files`, not
/// ambient system fonts, so the PDF and PNG corpus paths use the same pinned
/// font set.
pub fn render_document_pdf_isolated(
    doc: &Document,
    opts: &RenderOptions,
    pages: Option<&[usize]>,
    font_files: &[std::path::PathBuf],
) -> Result<PdfOutput, RenderError> {
    let (list, report, _, _) = build_display_list_with_font_scope(doc, opts, false, font_files);
    finish_pdf(list, report, pages, &doc.metadata)
}

/// Isolated PDF render with a pre-serialization text trace for corpus
/// validation. The trace is independent of the emitted ToUnicode CMap and
/// therefore detects incorrect mappings, not only missing mappings.
pub fn render_document_pdf_isolated_with_text_trace(
    doc: &Document,
    opts: &RenderOptions,
    pages: Option<&[usize]>,
    font_files: &[std::path::PathBuf],
) -> Result<PdfValidationOutput, RenderError> {
    let (list, report, _, _) = build_display_list_with_font_scope(doc, opts, false, font_files);
    finish_pdf_with_text_trace(list, report, pages, &doc.metadata)
}

fn finish_pdf(
    mut list: display::DisplayList,
    mut report: RenderIssueAccumulator,
    pages: Option<&[usize]>,
    meta: &Metadata,
) -> Result<PdfOutput, RenderError> {
    select_pdf_pages(&mut list, pages);
    let data = pdf::render_pdf_with_metadata(&list, &mut report, meta)?;
    Ok(PdfOutput {
        data,
        report: report.finish(),
    })
}

fn select_pdf_pages(list: &mut display::DisplayList, pages: Option<&[usize]>) {
    let Some(selected) = pages else {
        return;
    };
    let mut available: Vec<Option<display::PageList>> = std::mem::take(&mut list.pages)
        .into_iter()
        .map(Some)
        .collect();
    let mut picked = Vec::with_capacity(selected.len());
    for &number in selected {
        if let Some(page) = available
            .get_mut(number.wrapping_sub(1))
            .and_then(Option::take)
        {
            picked.push(page);
        }
    }
    list.pages = picked;
}

fn finish_pdf_with_text_trace(
    mut list: display::DisplayList,
    mut report: RenderIssueAccumulator,
    pages: Option<&[usize]>,
    meta: &Metadata,
) -> Result<PdfValidationOutput, RenderError> {
    select_pdf_pages(&mut list, pages);
    let expected_text = pdf::expected_text_trace(&list)?;
    let data = pdf::render_pdf_with_metadata(&list, &mut report, meta)?;
    Ok(PdfValidationOutput {
        data,
        report: report.finish(),
        expected_text,
    })
}

/// 렌더 시 페이지 수 (PDF 페이지 선택 검증용).
pub fn count_pages(doc: &Document, opts: &RenderOptions) -> usize {
    build_display_list(doc, opts).0.pages.len()
}

/// 인증용 명시 글꼴 집합으로 레이아웃만 수행해 페이지 수를 preflight한다.
pub fn count_pages_isolated(
    doc: &Document,
    opts: &RenderOptions,
    font_files: &[std::path::PathBuf],
) -> usize {
    build_display_list_with_font_scope(doc, opts, false, font_files)
        .0
        .pages
        .len()
}

/// 인증 전용 예산 안에서 레이아웃하고 페이지 수를 계산한다.
pub fn count_pages_isolated_bounded(
    doc: &Document,
    opts: &RenderOptions,
    font_files: &[std::path::PathBuf],
    budget: &layout::LayoutBudget,
) -> Result<usize, RenderError> {
    Ok(
        build_display_list_with_font_scope_bounded(doc, opts, font_files, budget)?
            .0
            .pages
            .len(),
    )
}

fn diagnose_pages(
    list: &display::DisplayList,
    selected: &[usize],
    pixmaps: &[tiny_skia::Pixmap],
    fonts: Vec<FontResolution>,
    font_resolution_complete: bool,
) -> RenderDiagnostics {
    const MAX_GEOMETRY_FINDINGS_PER_PAGE: usize = 1_024;
    const MAX_COLLISION_COMPARISONS_PER_PAGE: usize = 1_000_000;
    let pages = selected
        .iter()
        .zip(pixmaps)
        .filter_map(|(&number, pixmap)| {
            let page = list.pages.get(number.checked_sub(1)?)?;
            let bounds: Vec<Option<(f32, f32, f32, f32)>> =
                page.items.iter().map(item_bounds).collect();
            let mut outside_page_bounds_count = 0usize;
            let mut outside_page_bounds = Vec::new();
            for (item, bounds) in bounds.iter().enumerate() {
                let Some((x0, y0, x1, y1)) = *bounds else {
                    continue;
                };
                let outside = ![x0, y0, x1, y1].iter().all(|value| value.is_finite())
                    || x0 < -0.01
                    || y0 < -0.01
                    || x1 > page.width_pt + 0.01
                    || y1 > page.height_pt + 0.01;
                if outside {
                    outside_page_bounds_count += 1;
                    if outside_page_bounds.len() < MAX_GEOMETRY_FINDINGS_PER_PAGE {
                        outside_page_bounds.push(ItemBounds {
                            item,
                            x0,
                            y0,
                            x1,
                            y1,
                        });
                    }
                }
            }
            let mut possible_collisions = Vec::new();
            let mut possible_collision_count = 0usize;
            let mut comparisons = 0usize;
            let mut possible_collision_complete = true;
            for left in 0..page.items.len() {
                let display::Item::Glyphs {
                    y: left_y,
                    run: left_run,
                    ..
                } = &page.items[left]
                else {
                    continue;
                };
                for right in left + 1..page.items.len() {
                    if comparisons >= MAX_COLLISION_COMPARISONS_PER_PAGE {
                        possible_collision_complete = false;
                        break;
                    }
                    comparisons += 1;
                    let display::Item::Glyphs {
                        y: right_y,
                        run: right_run,
                        ..
                    } = &page.items[right]
                    else {
                        continue;
                    };
                    // 같은 줄의 인접 run은 정상이다. 서로 다른 기준선의 글리프 상자가
                    // 상당히 겹치는 경우만 잠재 충돌로 보고한다.
                    if (left_y - right_y).abs() <= left_run.size_pt.min(right_run.size_pt) * 0.25 {
                        continue;
                    }
                    if let (Some(a), Some(b)) = (bounds[left], bounds[right])
                        && overlap_ratio(a, b) >= 0.25
                    {
                        possible_collision_count += 1;
                        if possible_collisions.len() < MAX_GEOMETRY_FINDINGS_PER_PAGE {
                            possible_collisions.push(Collision {
                                first_item: left,
                                second_item: right,
                            });
                        }
                    }
                }
                if comparisons >= MAX_COLLISION_COMPARISONS_PER_PAGE {
                    possible_collision_complete = false;
                    break;
                }
            }
            let visually_blank = pixmap
                .pixels()
                .iter()
                .all(|pixel| pixel.red() >= 250 && pixel.green() >= 250 && pixel.blue() >= 250);
            Some(PageDiagnostics {
                page: number,
                width_pt: page.width_pt,
                height_pt: page.height_pt,
                item_count: page.items.len(),
                visually_blank,
                outside_page_bounds,
                outside_page_bounds_count,
                outside_page_bounds_complete: true,
                possible_collisions,
                possible_collision_count,
                possible_collision_complete,
            })
        })
        .collect();
    RenderDiagnostics {
        fonts,
        font_resolution_complete,
        pages,
    }
}

fn item_bounds(item: &display::Item) -> Option<(f32, f32, f32, f32)> {
    match item {
        display::Item::Glyphs { x, y, run } => Some((
            *x,
            *y - run.size_pt,
            *x + run.width_pt,
            *y + run.size_pt * 0.25,
        )),
        display::Item::Rect { x, y, w, h, .. } | display::Item::Image { x, y, w, h, .. } => {
            Some((*x, *y, *x + *w, *y + *h))
        }
        display::Item::Path {
            commands, stroke, ..
        } => {
            let (x0, y0, x1, y1) = display::path_bbox(commands);
            let half = stroke.as_ref().map_or(0.0, |value| value.width / 2.0);
            Some((x0 - half, y0 - half, x1 + half, y1 + half))
        }
    }
}

fn overlap_ratio(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> f32 {
    let width = (a.2.min(b.2) - a.0.max(b.0)).max(0.0);
    let height = (a.3.min(b.3) - a.1.max(b.1)).max(0.0);
    let overlap = width * height;
    let area_a = (a.2 - a.0).max(0.0) * (a.3 - a.1).max(0.0);
    let area_b = (b.2 - b.0).max(0.0) * (b.3 - b.1).max(0.0);
    let smaller = area_a.min(area_b);
    if smaller <= f32::EPSILON {
        0.0
    } else {
        overlap / smaller
    }
}
