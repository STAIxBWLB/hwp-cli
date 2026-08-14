//! `hwp compose` - DocumentSpec v1을 검증하고 네이티브 HWP/HWPX로 합성한다.

use std::io::Read as _;
use std::path::Path;

use anyhow::Context as _;
use hwp_cli::cli::SpecFormatArg;
use hwp_cli::document_spec::{self, ComposeReport, DocumentSpec, MAX_SPEC_BYTES, SpecInputFormat};
use hwp_cli::document_spec_v2::{self, ComposeReportV2};

#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum ComposeReportOutput {
    V1(ComposeReport),
    V2(ComposeReportV2),
}

impl ComposeReportOutput {
    fn counts(&self) -> (usize, usize, usize, usize) {
        match self {
            Self::V1(report) => (
                report.sections,
                report.paragraphs,
                report.tables,
                report.images,
            ),
            Self::V2(report) => (
                report.sections,
                report.paragraphs,
                report.tables,
                report.images,
            ),
        }
    }

    #[cfg(test)]
    fn dry_run(&self) -> bool {
        match self {
            Self::V1(report) => report.dry_run,
            Self::V2(report) => report.dry_run,
        }
    }
}

pub fn run(
    spec_path: &Path,
    output: &Path,
    format: Option<SpecFormatArg>,
    dry_run: bool,
    print_report: bool,
    allow_visual_fallback: bool,
) -> anyhow::Result<()> {
    let input = read_bounded(spec_path)?;
    let format = format
        .map(spec_format)
        .map(Ok)
        .unwrap_or_else(|| document_spec::infer_input_format(spec_path))
        .map_err(compose_error)?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    let report = execute_text_with_source(
        &input,
        format,
        base_dir,
        output,
        dry_run,
        allow_visual_fallback,
        Some(spec_path),
        &[],
    )?;

    if print_report || dry_run {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let (sections, paragraphs, tables, images) = report.counts();
        eprintln!(
            "합성 완료: {} (구역 {}, 문단 {}, 표 {}, 이미지 {})",
            output.display(),
            sections,
            paragraphs,
            tables,
            images
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn execute_text_with_source(
    input: &str,
    format: SpecInputFormat,
    base_dir: &Path,
    output: &Path,
    dry_run: bool,
    allow_visual_fallback: bool,
    source_path: Option<&Path>,
    roots: &[std::path::PathBuf],
) -> anyhow::Result<ComposeReportOutput> {
    execute_text_with_source_and_fonts(
        input,
        format,
        base_dir,
        output,
        dry_run,
        allow_visual_fallback,
        source_path,
        None,
        roots,
    )
}

/// `execute_text_with_source`의 hermetic font variant. `font_files`가 있으면 HWP
/// writer와 preview는 시스템 글꼴을 전혀 로드하지 않는다.
/// When `roots` is non-empty, spec assets are restricted below those roots (MCP sandbox).
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_text_with_source_and_fonts(
    input: &str,
    format: SpecInputFormat,
    base_dir: &Path,
    output: &Path,
    dry_run: bool,
    allow_visual_fallback: bool,
    source_path: Option<&Path>,
    font_files: Option<&[std::path::PathBuf]>,
    roots: &[std::path::PathBuf],
) -> anyhow::Result<ComposeReportOutput> {
    if spec_version(input, format).as_deref() == Some("2.0") {
        if allow_visual_fallback {
            return Err(compose_error(document_spec::ComposeError::Validation {
                issues: vec![document_spec::SpecIssue {
                    code: "policy_conflict".to_string(),
                    path: "$.policy".to_string(),
                    message: "--allow-visual-fallback is deprecated and cannot override DocumentSpec v2 target policies".to_string(),
                }],
            }));
        }
        let spec = document_spec_v2::parse_spec_v2(input, format).map_err(compose_error)?;
        execute_spec_v2_with_source(
            &spec,
            base_dir,
            output,
            dry_run,
            source_path,
            font_files,
            roots,
        )
        .map(ComposeReportOutput::V2)
    } else {
        let spec = document_spec::parse_spec(input, format).map_err(compose_error)?;
        execute_spec_with_source_and_fonts(
            &spec,
            base_dir,
            output,
            dry_run,
            allow_visual_fallback,
            source_path,
            font_files,
            roots,
        )
        .map(ComposeReportOutput::V1)
    }
}

fn spec_version(input: &str, format: SpecInputFormat) -> Option<String> {
    match format {
        SpecInputFormat::Json => serde_json::from_str::<serde_json::Value>(input).ok(),
        SpecInputFormat::Yaml => serde_yaml::from_str::<serde_json::Value>(input).ok(),
    }
    .and_then(|value| value.get("version")?.as_str().map(str::to_string))
}

fn execute_spec_v2_with_source(
    spec: &document_spec_v2::DocumentSpecV2,
    base_dir: &Path,
    output: &Path,
    dry_run: bool,
    source_path: Option<&Path>,
    font_files: Option<&[std::path::PathBuf]>,
    roots: &[std::path::PathBuf],
) -> anyhow::Result<ComposeReportV2> {
    let write_hwp = output_kind(output)?;
    let compiled = document_spec_v2::compile_spec_v2(spec, base_dir, output, dry_run, roots)
        .map_err(compose_error)?;
    let mut report = compiled.report;
    if dry_run {
        return Ok(report);
    }
    let document = compiled.document;
    let writer_report = crate::commands::output::write_validated(
        output,
        source_path,
        |staged| {
            if write_hwp {
                if let Some(font_files) = font_files {
                    crate::commands::convert::write_hwp_structural_isolated(
                        &document, staged, font_files,
                    )
                } else {
                    crate::commands::convert::write_hwp_structural(&document, staged)
                }
            } else {
                Ok(hwpx::write_document_with_report(&document, staged)?)
            }
        },
        |staged, writer_report| {
            crate::commands::reject_preservation_loss("compose", &writer_report.preservation)?;
            if font_files.is_some() {
                crate::commands::edit::verify_document_quiet(staged, &document)
            } else {
                crate::commands::edit::verify_document(staged, &document)
            }
            .context("합성 문서 의미 검증 실패")
        },
    )?;
    report.warnings = writer_report.warnings;
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_spec_with_source_and_fonts(
    spec: &DocumentSpec,
    base_dir: &Path,
    output: &Path,
    dry_run: bool,
    allow_visual_fallback: bool,
    source_path: Option<&Path>,
    font_files: Option<&[std::path::PathBuf]>,
    roots: &[std::path::PathBuf],
) -> anyhow::Result<ComposeReport> {
    let write_hwp = output_kind(output)?;
    let compiled = document_spec::compile_spec(
        spec,
        base_dir,
        output,
        dry_run,
        allow_visual_fallback,
        roots,
    )
    .map_err(compose_error)?;
    let mut report = compiled.report;
    if dry_run {
        return Ok(report);
    }

    let document = compiled.document;
    let writer_report = crate::commands::output::write_validated(
        output,
        source_path,
        |staged| {
            if write_hwp {
                if let Some(font_files) = font_files {
                    crate::commands::convert::write_hwp_structural_isolated(
                        &document, staged, font_files,
                    )
                } else {
                    crate::commands::convert::write_hwp_structural(&document, staged)
                }
            } else {
                Ok(hwpx::write_document_with_report(&document, staged)?)
            }
        },
        |staged, writer_report| {
            crate::commands::reject_preservation_loss("compose", &writer_report.preservation)?;
            if font_files.is_some() {
                crate::commands::edit::verify_document_quiet(staged, &document)
            } else {
                crate::commands::edit::verify_document(staged, &document)
            }
            .context("합성 문서 의미 검증 실패")
        },
    )?;
    report.warnings = writer_report.warnings;
    Ok(report)
}

pub fn read_bounded(path: &Path) -> anyhow::Result<String> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("DocumentSpec를 열 수 없습니다: {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take((MAX_SPEC_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("DocumentSpec를 읽을 수 없습니다: {}", path.display()))?;
    if bytes.len() > MAX_SPEC_BYTES {
        anyhow::bail!(
            "DocumentSpec가 최대 {} bytes를 초과합니다: {}",
            MAX_SPEC_BYTES,
            path.display()
        );
    }
    String::from_utf8(bytes)
        .with_context(|| format!("DocumentSpec는 UTF-8이어야 합니다: {}", path.display()))
}

fn output_kind(output: &Path) -> anyhow::Result<bool> {
    match output
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("hwp") => Ok(true),
        Some("hwpx") => Ok(false),
        Some(other) => anyhow::bail!(
            "지원하지 않는 출력 확장자입니다: .{other} (`hwp compose`는 .hwp 또는 .hwpx만 지원합니다)"
        ),
        None => anyhow::bail!(
            "출력 파일에 확장자가 없습니다: {} (`hwp compose`는 .hwp 또는 .hwpx만 지원합니다)",
            output.display()
        ),
    }
}

fn spec_format(format: SpecFormatArg) -> SpecInputFormat {
    match format {
        SpecFormatArg::Json => SpecInputFormat::Json,
        SpecFormatArg::Yaml => SpecInputFormat::Yaml,
    }
}

fn compose_error(error: document_spec::ComposeError) -> anyhow::Error {
    let envelope = serde_json::json!({
        "error": "document_spec",
        "issues": error.issues(),
    });
    anyhow::anyhow!(
        "{}",
        serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| error.to_string())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_SPEC: &str = r#"{
      "version": "1.0",
      "sections": [{
        "blocks": [{
          "type": "paragraph",
          "runs": [{"type": "text", "text": "본문"}]
        }]
      }]
    }"#;

    #[test]
    fn dry_run_still_rejects_invalid_output_extension() {
        let error = execute_text_with_source(
            MINIMAL_SPEC,
            SpecInputFormat::Json,
            Path::new("."),
            Path::new("out.txt"),
            true,
            false,
            None,
            &[],
        )
        .expect_err("invalid output extension");
        assert!(error.to_string().contains(".txt"));
    }

    #[test]
    fn dry_run_does_not_publish_output() {
        let output = std::env::temp_dir().join(format!(
            "hwp-compose-dry-run-{}-{}.hwpx",
            std::process::id(),
            std::thread::current()
                .name()
                .unwrap_or("test")
                .replace(':', "-")
        ));
        let report = execute_text_with_source(
            MINIMAL_SPEC,
            SpecInputFormat::Json,
            Path::new("."),
            &output,
            true,
            false,
            None,
            &[],
        )
        .expect("dry-run");
        assert!(report.dry_run());
        assert!(!output.exists());
    }
}
