//! Compare one rendered page with a Hancom-produced reference PNG.
//!
//! The command measures pixel and projection-profile differences at the same DPI,
//! reports positional offsets and pixel error, and can save a difference image.
//! `--format json` emits the machine-readable `hwp-diff-report-v1` contract for
//! `scripts/pdf-parity.sh`. `--ours-png` compares an already-rasterized PDF page
//! instead of rendering the document, so both PDFs use the same Poppler path.

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
    // Reuse a pre-rasterized page when the parity runner supplies --ours-png.
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
            let ours_input = ours_png.unwrap_or(input);
            write_diff_image(&[ours_input, reference], &out_path, &encode_png(&diff_img)?)?;
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
            // JSON mode writes an image only when the caller explicitly requests one.
            if let Some(out_path) = out {
                let ours_input = ours_png.unwrap_or(input);
                write_diff_image(&[ours_input, reference], out_path, &encode_png(&diff_img)?)?;
            }
        }
    }
    Ok(())
}

/// Encode the difference image as PNG.
fn encode_png(diff_img: &tiny_skia::Pixmap) -> anyhow::Result<Vec<u8>> {
    diff_img
        .encode_png()
        .map_err(|error| anyhow::anyhow!("차이 이미지 인코딩 실패: {error}"))
}

/// Publish a validated difference image without overwriting either immutable input.
fn write_diff_image(inputs: &[&Path], out_path: &Path, png: &[u8]) -> anyhow::Result<()> {
    crate::commands::output::reject_output_aliases(out_path, inputs)?;
    crate::commands::output::write_validated(
        out_path,
        None,
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

/// Build the machine-readable `hwp-diff-report-v1` payload.
/// `ours_png` and `font_coverage` appear only on their respective code paths.
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
    fn json_report_contract() {
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
        // Neither optional field applies to this synthetic report.
        assert!(v.get("ours_png").is_none());
        assert!(v.get("font_coverage").is_none());
    }

    #[test]
    fn json_report_raster_path_has_no_font_coverage() {
        let v = diff_report_json(
            Path::new("doc.hwp"),
            Some(Path::new("ours-1.png")),
            Path::new("ref.png"),
            1,
            150.0,
            1240,
            1754,
            &report(),
            None,
        );
        assert_eq!(v["ours_png"], "ours-1.png");
        assert!(v.get("font_coverage").is_none());
    }

    #[test]
    fn difference_output_cannot_replace_a_raster_input() {
        let directory =
            std::env::temp_dir().join(format!("hwp-cli-diff-alias-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let ours = directory.join("ours.png");
        let reference = directory.join("reference.png");
        std::fs::write(&ours, b"ours").unwrap();
        std::fs::write(&reference, b"reference").unwrap();

        assert!(write_diff_image(&[&ours, &reference], &ours, b"replacement").is_err());
        assert_eq!(std::fs::read(&ours).unwrap(), b"ours");
        assert!(write_diff_image(&[&ours, &reference], &reference, b"replacement").is_err());
        assert_eq!(std::fs::read(&reference).unwrap(), b"reference");

        std::fs::remove_dir_all(directory).unwrap();
    }
}
