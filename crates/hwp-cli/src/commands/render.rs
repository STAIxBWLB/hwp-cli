//! `hwp render` — 페이지 렌더링 (PNG/SVG/PDF).
//!
//! PNG/SVG는 페이지별 파일(out-1.png …)로, PDF는 단일 멀티페이지 파일로 쓴다.

use std::path::{Path, PathBuf};

use crate::commands::cat::load_document;
use hwp_cli::cli::RenderFormat;

pub fn run(
    input: &Path,
    output: &Path,
    pages_spec: &str,
    dpi: f64,
    format: Option<RenderFormat>,
    font_dirs: Vec<PathBuf>,
) -> anyhow::Result<()> {
    let dpi = validated_dpi(dpi)?;
    let format = format.unwrap_or_else(|| infer_format(output));
    let doc = load_document(input)?;
    // --font-dir 미지정 시 번들 함초롬 글꼴(HWP_FONT_DIR/fonts)을 기본 로드.
    let opts = hwp_render::RenderOptions {
        dpi,
        font_dirs: crate::commands::convert::resolve_font_dirs(font_dirs),
    };

    match format {
        RenderFormat::Png => {
            let total = hwp_render::count_pages(&doc, &opts);
            let selected = parse_pages(pages_spec, total)?;
            let result = hwp_render::render_document_pages(&doc, &opts, Some(&selected))?;
            report(&result.report);
            let multi = selected.len() > 1;
            let mut outputs = Vec::with_capacity(selected.len());
            let mut dimensions = Vec::with_capacity(selected.len());
            for (&page_no, pixmap) in selected.iter().zip(&result.pages) {
                let path = page_path(output, page_no, multi);
                let png = pixmap.encode_png().map_err(|error| {
                    anyhow::anyhow!("PNG 인코딩 실패 ({}): {error}", path.display())
                })?;
                dimensions.push((path.clone(), pixmap.width(), pixmap.height()));
                outputs.push((path, png));
            }
            publish_render_set(&outputs, input)?;
            for (path, width, height) in dimensions {
                eprintln!("저장: {} ({}×{}px)", path.display(), width, height);
            }
        }
        RenderFormat::Svg => {
            let result = hwp_render::render_document_svg(&doc, &opts);
            report(&result.report);
            let selected = parse_pages(pages_spec, result.pages.len())?;
            let multi = selected.len() > 1;
            let outputs = selected
                .iter()
                .map(|&page_no| {
                    (
                        page_path(output, page_no, multi),
                        result.pages[page_no - 1].as_bytes().to_vec(),
                    )
                })
                .collect::<Vec<_>>();
            publish_render_set(&outputs, input)?;
            for &page_no in &selected {
                let path = page_path(output, page_no, multi);
                eprintln!("저장: {}", path.display());
            }
        }
        RenderFormat::Pdf => {
            // PNG/SVG와 달리 PDF는 단일 멀티페이지 파일이다 (페이지별 분리 없음).
            let total = hwp_render::count_pages(&doc, &opts);
            let selected = parse_pages(pages_spec, total)?;
            let result = hwp_render::render_document_pdf(&doc, &opts, Some(&selected))?;
            report(&result.report);
            write_render_bytes(output, input, &result.data)?;
            eprintln!(
                "저장: {} ({}쪽, {} bytes)",
                output.display(),
                selected.len(),
                result.data.len()
            );
        }
    }
    Ok(())
}

pub(crate) fn validated_dpi(dpi: f64) -> anyhow::Result<f32> {
    let min = f64::from(hwp_render::MIN_DPI);
    let max = f64::from(hwp_render::MAX_DPI);
    if !dpi.is_finite() || !(min..=max).contains(&dpi) {
        anyhow::bail!("DPI는 유한한 {min}..={max} 범위여야 합니다: {dpi}");
    }
    hwp_render::validate_dpi(dpi as f32).map_err(Into::into)
}

pub(crate) fn publish_render_set(
    outputs: &[(PathBuf, Vec<u8>)],
    input: &Path,
) -> anyhow::Result<()> {
    if let Some(warning) = crate::commands::output::write_validated_files(outputs, Some(input))? {
        eprintln!("경고: {warning}");
    }
    Ok(())
}

pub(crate) fn write_render_bytes(
    destination: &Path,
    input: &Path,
    bytes: &[u8],
) -> anyhow::Result<()> {
    crate::commands::output::write_validated(
        destination,
        Some(input),
        |staged| {
            std::fs::write(staged, bytes)?;
            Ok(())
        },
        |staged, _| {
            let written = std::fs::read(staged)?;
            if written != bytes {
                anyhow::bail!("렌더 출력 검증 중 바이트 불일치: {}", staged.display());
            }
            Ok(())
        },
    )
}

fn report(report: &hwp_render::RenderIssueReport) {
    for issue in report.info.iter().chain(&report.issues) {
        eprintln!("렌더: {issue}");
    }
    let coverage = report.font_coverage();
    if coverage.matched > 0 || !coverage.substitution_free() {
        eprintln!(
            "렌더: 글꼴 커버리지 matched={} substituted={} missing={} subset_fallback={}",
            coverage.matched, coverage.substituted, coverage.missing, coverage.subset_fallback
        );
    }
    if !coverage.substitution_free() {
        eprintln!(
            "렌더: 글꼴 대체/실패 발생 — 이 출력으로 잰 parity 수치는 발행할 수 없습니다(F1 게이트)"
        );
    }
    if !report.complete {
        eprintln!("렌더: issue accumulator incomplete");
    }
}

fn infer_format(output: &Path) -> RenderFormat {
    match output
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("svg") => RenderFormat::Svg,
        Some("pdf") => RenderFormat::Pdf,
        _ => RenderFormat::Png,
    }
}

/// Per-page output path: `<stem>-<N>.<ext>` for multiple pages, the path as-is for a single page.
/// MCP render output_path filenames follow the same rule.
pub(crate) fn page_path(base: &Path, page: usize, multi: bool) -> PathBuf {
    if multi {
        numbered_path(base, page)
    } else {
        base.to_path_buf()
    }
}

/// "all" | "3" | "1-5" → 1-기반 페이지 번호 목록.
pub(crate) fn parse_pages(spec: &str, total: usize) -> anyhow::Result<Vec<usize>> {
    if total == 0 {
        anyhow::bail!("렌더링된 페이지가 없습니다");
    }
    let pages: Vec<usize> = if spec.eq_ignore_ascii_case("all") {
        (1..=total).collect()
    } else if let Some((a, b)) = spec.split_once('-') {
        let (a, b): (usize, usize) = (a.trim().parse()?, b.trim().parse()?);
        (a..=b.min(total)).collect()
    } else {
        vec![spec.trim().parse()?]
    };
    if pages.is_empty() || pages.iter().any(|&p| p == 0 || p > total) {
        anyhow::bail!("페이지 범위가 잘못되었습니다 (문서: {total}쪽, 요청: {spec})");
    }
    Ok(pages)
}

/// out.png → out-3.png 형태의 페이지별 경로.
fn numbered_path(base: &Path, page: usize) -> PathBuf {
    let stem = base.file_stem().and_then(|s| s.to_str()).unwrap_or("page");
    let ext = base.extension().and_then(|s| s.to_str()).unwrap_or("png");
    base.with_file_name(format!("{stem}-{page}.{ext}"))
}
