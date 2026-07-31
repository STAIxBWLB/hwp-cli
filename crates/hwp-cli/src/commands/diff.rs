//! `hwp diff` — 렌더 결과를 한글 기준 PNG와 비교해 오차를 측정한다.
//!
//! 한글에서 같은 페이지를 같은 DPI로 내보낸 기준 이미지와 우리 렌더를 픽셀·
//! 프로파일 비교해 위치 오차(dx/dy)·픽셀 차이율을 보고하고 차이 이미지를 저장한다.

use std::path::{Path, PathBuf};

use crate::commands::cat::load_document;

#[allow(clippy::too_many_arguments)]
pub fn run(
    input: &Path,
    reference: &Path,
    page: usize,
    dpi: f64,
    out: Option<&Path>,
    font_dirs: Vec<PathBuf>,
    tolerance: u8,
) -> anyhow::Result<()> {
    let dpi = crate::commands::render::validated_dpi(dpi)?;
    let doc = load_document(input)?;
    let result = hwp_render::render_document_pages(
        &doc,
        &hwp_render::RenderOptions {
            dpi,
            font_dirs: crate::commands::convert::resolve_font_dirs(font_dirs),
        },
        Some(&[page]),
    )?;
    for issue in result.report.info.iter().chain(&result.report.issues) {
        eprintln!("렌더: {issue}");
    }
    let ours = &result.pages[0];

    let reference_px = hwp_render::load_png(reference)?;

    let (report, diff_img) =
        hwp_render::compare(ours, &reference_px, tolerance).map_err(|e| anyhow::anyhow!(e))?;

    println!("페이지 {page} ({}×{}px)", ours.width(), ours.height());
    println!(
        "  잉크 적용률(완전성): {:.1}% (우리 잉크 / 한글 잉크 — 100%면 같은 양)",
        report.ink_ratio * 100.0
    );
    println!(
        "  위치 오프셋: dx={}px, dy={}px (작을수록 정합)",
        report.dx, report.dy
    );
    println!(
        "  픽셀 차이율: {:.2}% (대부분 글리프 모양·AA 차이 — 폰트/엔진 의존)",
        report.bad_pixel_pct * 100.0
    );
    println!("  평균 절대 오차(MAE): {:.2}/255", report.mae);

    let out_path = out
        .map(Path::to_path_buf)
        .unwrap_or_else(|| reference.with_extension("diff.png"));
    let png = diff_img
        .encode_png()
        .map_err(|error| anyhow::anyhow!("차이 이미지 인코딩 실패: {error}"))?;
    crate::commands::output::write_validated(
        &out_path,
        Some(input),
        |staged| {
            std::fs::write(staged, &png)?;
            Ok(())
        },
        |staged, _| {
            if std::fs::read(staged)? != png {
                anyhow::bail!(
                    "차이 이미지 출력 검증 중 바이트 불일치: {}",
                    staged.display()
                );
            }
            Ok(())
        },
    )?;
    eprintln!("차이 이미지: {}", out_path.display());
    Ok(())
}
