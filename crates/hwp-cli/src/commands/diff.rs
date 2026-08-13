//! `hwp diff` — 렌더 결과를 한글 기준 PNG와 비교해 오차를 측정한다.
//!
//! 한글에서 같은 페이지를 같은 DPI로 낸 기준 이미지와 우리 렌더를 픽셀·프로파일
//! 비교해 위치 오차(dx/dy)·픽셀 차이율을 보고하고 차이 이미지를 저장한다.
//! `--format json`은 배치 러너(scripts/pdf-parity.sh)용 기계 판독 리포트(contract
//! hwp-diff-report-v1)를 stdout에 출력한다. `--ours-png`는 문서 렌더 대신 주어진
//! 래스터(우리 PDF의 pdftoppm 결과)를 기준과 비교한다 — docs/design/21 §3 규약:
//! 지표 4~5는 양쪽 PDF를 같은 Poppler로 래스터화해 비교한다.

use std::path::{Path, PathBuf};

use crate::commands::cat::load_document;
use hwp_cli::cli::DiffFormat;

#[allow(clippy::too_many_arguments)]
pub fn run(
    input: &Path,
    reference: &Path,
    page: usize,
    dpi: f64,
    out: Option<&Path>,
    font_dirs: Vec<PathBuf>,
    tolerance: u8,
    format: DiffFormat,
    ours_png: Option<&Path>,
) -> anyhow::Result<()> {
    let dpi = crate::commands::render::validated_dpi(dpi)?;
    // ours 래스터: --ours-png가 있으면 문서 렌더를 건어너고 그 파일을 쓴다.
    let (ours, coverage) = if let Some(png) = ours_png {
        (hwp_render::load_png(png)?, None)
    } else {
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
        let coverage = result.report.font_coverage();
        let page_px = result
            .pages
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("페이지 {page} 렌더 결과 없음"))?;
        (page_px, Some(coverage))
    };

    let reference_px = hwp_render::load_png(reference)?;

    let (report, diff_img) =
        hwp_render::compare(&ours, &reference_px, tolerance).map_err(|e| anyhow::anyhow!(e))?;

    match format {
        DiffFormat::Text => {
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
            write_diff_image(input, &out_path, &encode_png(&diff_img)?)?;
        }
        DiffFormat::Json => {
            let json = diff_report_json(
                input,
                ours_png,
                reference,
                page,
                dpi,
                ours.width(),
                ours.height(),
                &report,
                coverage,
            );
            println!("{}", serde_json::to_string_pretty(&json)?);
            // 배치 모드는 차이 이미지를 -o 지정 시에만 저장한다(기본 산출물 남발 방지).
            if let Some(out_path) = out {
                write_diff_image(input, out_path, &encode_png(&diff_img)?)?;
            }
        }
    }
    Ok(())
}

/// 차이 이미지 PNG 인코딩.
fn encode_png(diff_img: &tiny_skia::Pixmap) -> anyhow::Result<Vec<u8>> {
    diff_img
        .encode_png()
        .map_err(|error| anyhow::anyhow!("차이 이미지 인코딩 실패: {error}"))
}

/// 차이 이미지를 검증 쓰기한다.
fn write_diff_image(input: &Path, out_path: &Path, png: &[u8]) -> anyhow::Result<()> {
    crate::commands::output::write_validated(
        out_path,
        Some(input),
        |staged| {
            std::fs::write(staged, png)?;
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

/// 기계 판독 리포트 JSON (contract `hwp-diff-report-v1`).
/// `ours_png`·`font_coverage`는 해당 경로에서만 들어간다.
#[allow(clippy::too_many_arguments)]
fn diff_report_json(
    input: &Path,
    ours_png: Option<&Path>,
    reference: &Path,
    page: usize,
    dpi: f32,
    width: u32,
    height: u32,
    report: &hwp_render::DiffReport,
    coverage: Option<hwp_render::FontCoverage>,
) -> serde_json::Value {
    let mut value = serde_json::json!({
        "contract": "hwp-diff-report-v1",
        "input": input.to_string_lossy(),
        "reference": reference.to_string_lossy(),
        "page": page,
        "dpi": dpi,
        "width": width,
        "height": height,
        "dx": report.dx,
        "dy": report.dy,
        "ink_ratio": report.ink_ratio,
        "bad_pixel_pct": report.bad_pixel_pct,
        "mae": report.mae,
    });
    if let Some(png) = ours_png {
        value["ours_png"] = serde_json::json!(png.to_string_lossy());
    }
    if let Some(c) = coverage {
        value["font_coverage"] = serde_json::json!({
            "matched": c.matched,
            "substituted": c.substituted,
            "missing": c.missing,
            "subset_fallback": c.subset_fallback,
            "substitution_free": c.substitution_free(),
        });
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> hwp_render::DiffReport {
        hwp_render::DiffReport {
            mae: 1.5,
            bad_pixel_pct: 0.02,
            dx: 1,
            dy: -1,
            ink_ratio: 0.99,
        }
    }

    #[test]
    fn json_리포트_계약() {
        let v = diff_report_json(
            Path::new("doc.hwp"),
            None,
            Path::new("ref.png"),
            2,
            150.0,
            1240,
            1754,
            &report(),
            None,
        );
        assert_eq!(v["contract"], "hwp-diff-report-v1");
        assert_eq!(v["page"], 2);
        assert_eq!(v["dx"], 1);
        assert_eq!(v["dy"], -1);
        // 문서 렌더 경로가 아니면 두 필드 모두 생략.
        assert!(v.get("ours_png").is_none());
        assert!(v.get("font_coverage").is_none());
    }

    #[test]
    fn json_리포트_래스터_경로() {
        let coverage = hwp_render::FontCoverage {
            matched: 2,
            ..Default::default()
        };
        let v = diff_report_json(
            Path::new("doc.hwp"),
            Some(Path::new("ours-1.png")),
            Path::new("ref.png"),
            1,
            150.0,
            1240,
            1754,
            &report(),
            Some(coverage),
        );
        assert_eq!(v["ours_png"], "ours-1.png");
        assert_eq!(v["font_coverage"]["matched"], 2);
        assert_eq!(v["font_coverage"]["substitution_free"], true);
    }
}
