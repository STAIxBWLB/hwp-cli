//! IR → PNG/SVG/PDF 페이지 렌더러.
//!
//! 파이프라인: IR → Layout([`layout`] — LineSegLayouter) →
//! [`display::DisplayList`] → 백엔드([`png`] tiny-skia / [`svg`] / [`pdf`]).
//! 세 백엔드 모두 같은 DisplayList를 소비한다.
//!
//! v1 범위: lineseg 기반 텍스트 렌더링 (굵게/기울임/크기/색/자간/장평,
//! 가운데/오른쪽 정렬). 표·이미지·장식은 M5.

pub mod diff;
pub mod display;
pub mod equation;
pub mod error;
pub mod fonts;
pub mod footnote;
pub mod gso;
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

use hwp_model::Document;

pub use diff::{DiffReport, compare};
pub use error::RenderError;
pub use fonts::FontStore;

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
    /// 경고 + 폰트 해석 리포트
    pub report: Vec<String>,
}

pub struct SvgOutput {
    /// 페이지별 SVG 문서
    pub pages: Vec<String>,
    pub report: Vec<String>,
}

fn build_display_list(doc: &Document, opts: &RenderOptions) -> (display::DisplayList, Vec<String>) {
    let mut store = FontStore::new();
    for dir in &opts.font_dirs {
        store.load_dir(dir);
    }
    let mut warnings = Vec::new();
    let list = layout::layout_document(doc, &mut store, &mut warnings);
    warnings.append(&mut store.report);
    (list, warnings)
}

/// 문서 전체를 PNG(래스터)로 렌더링한다.
pub fn render_document(doc: &Document, opts: &RenderOptions) -> Result<RenderOutput, RenderError> {
    let (list, report) = build_display_list(doc, opts);
    let pages = png::render_png(&list, opts.dpi)?;
    Ok(RenderOutput { pages, report })
}

/// 기준 PNG를 픽스맵으로 읽는다 (`hwp diff`의 기준 이미지 로드용).
pub fn load_png(path: &std::path::Path) -> Result<tiny_skia::Pixmap, RenderError> {
    tiny_skia::Pixmap::load_png(path)
        .map_err(|e| RenderError::Backend(format!("PNG 로드 실패 ({}): {e}", path.display())))
}

/// 문서 전체를 SVG로 렌더링한다.
pub fn render_document_svg(doc: &Document, opts: &RenderOptions) -> SvgOutput {
    let (list, report) = build_display_list(doc, opts);
    SvgOutput {
        pages: svg::render_svg(&list),
        report,
    }
}

pub struct PdfOutput {
    /// 단일 멀티페이지 PDF 바이트
    pub data: Vec<u8>,
    /// 경고 + 폰트 해석 리포트
    pub report: Vec<String>,
}

/// 문서를 단일 멀티페이지 PDF로 렌더링한다 (폰트 임베드 + 검색 가능 텍스트).
/// `pages`는 1-기반 페이지 번호 목록; `None`이면 전체 페이지.
pub fn render_document_pdf(
    doc: &Document,
    opts: &RenderOptions,
    pages: Option<&[usize]>,
) -> Result<PdfOutput, RenderError> {
    let (mut list, mut report) = build_display_list(doc, opts);
    if let Some(sel) = pages {
        let mut taken: Vec<Option<display::PageList>> = list.pages.into_iter().map(Some).collect();
        let mut picked = Vec::with_capacity(sel.len());
        for &n in sel {
            if let Some(page) = taken.get_mut(n.wrapping_sub(1)).and_then(Option::take) {
                picked.push(page);
            }
        }
        list = display::DisplayList { pages: picked };
    }
    let data = pdf::render_pdf(&list, &mut report)?;
    Ok(PdfOutput { data, report })
}

/// 렌더 시 페이지 수 (PDF 페이지 선택 검증용).
pub fn count_pages(doc: &Document, opts: &RenderOptions) -> usize {
    build_display_list(doc, opts).0.pages.len()
}
