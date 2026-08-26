//! `hwp render` — 페이지 렌더링 (PNG/SVG/PDF).
//!
//! PNG/SVG는 페이지별 파일(out-1.png …)로, PDF는 단일 멀티페이지 파일로 쓴다.

use std::fs::File;
use std::io::{Read as _, Write as _};
use std::path::{Component, Path, PathBuf};

use crate::commands::cat::{LoadOptions, load_document_with_options, resolve_password_args};
use hwp_cli::certification::{
    RenderIssueReportEntry, canonical_render_issue_sha256, map_render_issue,
};
use hwp_cli::cli::{PasswordArgs, RenderFormat};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

const RENDER_REPORT_SCHEMA_VERSION: &str = "1.0";
const RENDER_REPORT_CONTRACT: &str = "hwp-render-report-v1";

#[derive(Debug, Serialize)]
struct RenderReportFile {
    schema_version: &'static str,
    contract: &'static str,
    input: RenderInputReport,
    format: &'static str,
    dpi: f32,
    total_pages: usize,
    selected_pages: Vec<usize>,
    font_coverage: RenderFontCoverage,
    font_resolution_complete: bool,
    fonts: Vec<RenderFontRecord>,
    issues: Vec<RenderIssueReportEntry>,
    info: Vec<RenderIssueReportEntry>,
    issue_count: u64,
    info_count: u64,
    issue_log_complete: bool,
    issue_sha256: String,
    complete: bool,
}

/// Deterministic privacy-safe per-font identity record: only hashed family
/// names and font bytes, never raw names or paths.
#[derive(Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
struct RenderFontRecord {
    requested_sha256: String,
    requested_bold: bool,
    resolved_family_sha256: Option<String>,
    resolved_sha256: Option<String>,
    resolved_face_index: Option<u32>,
    outcome: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RenderInputReport {
    format: &'static str,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct RenderFontCoverage {
    matched: u64,
    substituted: u64,
    missing: u64,
    subset_fallback: u64,
    substitution_free: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn run_with_password(
    input: &Path,
    output: &Path,
    pages_spec: &str,
    dpi: f64,
    format: Option<RenderFormat>,
    font_dirs: Vec<PathBuf>,
    report_path: Option<&Path>,
    password_args: PasswordArgs,
) -> anyhow::Result<()> {
    let password = resolve_password_args(password_args, input)?;
    run_with_report_with_options(
        input,
        output,
        pages_spec,
        dpi,
        format,
        font_dirs,
        report_path,
        &LoadOptions {
            password: password.as_ref(),
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn run_with_report_with_options(
    input: &Path,
    output: &Path,
    pages_spec: &str,
    dpi: f64,
    format: Option<RenderFormat>,
    font_dirs: Vec<PathBuf>,
    report_path: Option<&Path>,
    options: &LoadOptions<'_>,
) -> anyhow::Result<()> {
    let dpi = validated_dpi(dpi)?;
    let format = format.unwrap_or_else(|| infer_format(output));
    let input_report = report_path.map(|_| build_input_report(input)).transpose()?;
    let doc = load_document_with_options(input, options).map_err(anyhow::Error::new)?;
    if let Some(before) = &input_report {
        let after = build_input_report(input)?;
        if before != &after {
            anyhow::bail!("렌더 입력이 문서 로드 중 바뀌었습니다");
        }
    }
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
            if let Some(report_path) = report_path {
                ensure_report_destination(
                    report_path,
                    input,
                    outputs.iter().map(|(path, _)| path.as_path()),
                )?;
            }
            publish_render_set(&outputs, input)?;
            for (path, width, height) in dimensions {
                eprintln!("저장: {} ({}×{}px)", path.display(), width, height);
            }
            if let Some(report_path) = report_path {
                write_report(
                    report_path,
                    input_report
                        .clone()
                        .expect("report path guarantees an input report"),
                    "png",
                    dpi,
                    result.total_pages,
                    selected,
                    &result.diagnostics.fonts,
                    result.diagnostics.font_resolution_complete,
                    result.report,
                )?;
            }
        }
        RenderFormat::Svg => {
            let result = hwp_render::render_document_svg(&doc, &opts);
            report(&result.report);
            let total_pages = result.pages.len();
            let selected = parse_pages(pages_spec, total_pages)?;
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
            if let Some(report_path) = report_path {
                ensure_report_destination(
                    report_path,
                    input,
                    outputs.iter().map(|(path, _)| path.as_path()),
                )?;
            }
            publish_render_set(&outputs, input)?;
            for &page_no in &selected {
                let path = page_path(output, page_no, multi);
                eprintln!("저장: {}", path.display());
            }
            if let Some(report_path) = report_path {
                write_report(
                    report_path,
                    input_report
                        .clone()
                        .expect("report path guarantees an input report"),
                    "svg",
                    dpi,
                    total_pages,
                    selected,
                    &result.fonts,
                    result.font_resolution_complete,
                    result.report,
                )?;
            }
        }
        RenderFormat::Pdf => {
            // PNG/SVG와 달리 PDF는 단일 멀티페이지 파일이다 (페이지별 분리 없음).
            let total = hwp_render::count_pages(&doc, &opts);
            let selected = parse_pages(pages_spec, total)?;
            let result = hwp_render::render_document_pdf(&doc, &opts, Some(&selected))?;
            report(&result.report);
            if let Some(report_path) = report_path {
                ensure_report_destination(report_path, input, std::iter::once(output))?;
            }
            write_render_bytes(output, input, &result.data)?;
            eprintln!(
                "저장: {} ({}쪽, {} bytes)",
                output.display(),
                selected.len(),
                result.data.len()
            );
            if let Some(report_path) = report_path {
                write_report(
                    report_path,
                    input_report.expect("report path guarantees an input report"),
                    "pdf",
                    dpi,
                    total,
                    selected,
                    &result.fonts,
                    result.font_resolution_complete,
                    result.report,
                )?;
            }
        }
    }
    Ok(())
}

fn build_input_report(input: &Path) -> anyhow::Result<RenderInputReport> {
    let mut file = File::open(input).map_err(|error| {
        anyhow::anyhow!("렌더 입력을 열 수 없습니다 ({}): {error}", input.display())
    })?;
    let bytes = file
        .metadata()
        .map_err(|error| {
            anyhow::anyhow!(
                "렌더 입력 상태를 확인할 수 없습니다 ({}): {error}",
                input.display()
            )
        })?
        .len();
    let mut hasher = Sha256::new();
    let mut observed = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            anyhow::anyhow!(
                "렌더 입력을 읽을 수 없습니다 ({}): {error}",
                input.display()
            )
        })?;
        if read == 0 {
            break;
        }
        observed = observed
            .checked_add(read as u64)
            .ok_or_else(|| anyhow::anyhow!("렌더 입력 크기 overflow"))?;
        hasher.update(&buffer[..read]);
    }
    if observed != bytes {
        anyhow::bail!("렌더 입력이 해시 계산 중 바뀌었습니다: {observed} != {bytes} bytes");
    }
    Ok(RenderInputReport {
        format: input_format(input),
        bytes,
        sha256: hex_digest(hasher.finalize().as_slice()),
    })
}

fn input_format(input: &Path) -> &'static str {
    match input
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("hwp") => "hwp5",
        Some("hwpx") => "hwpx",
        _ => "unknown",
    }
}

fn format_issue_report(source: hwp_render::RenderIssueReport) -> RenderReportIssueChannels {
    let issues: Vec<_> = source.issues.into_iter().map(map_render_issue).collect();
    let info: Vec<_> = source.info.into_iter().map(map_render_issue).collect();
    let issue_count = issues.iter().map(|issue| issue.count).sum();
    let info_count = info.iter().map(|issue| issue.count).sum();
    let issue_sha256 = canonical_render_issue_sha256(&issues);
    assert_eq!(issue_count, source.issue_count);
    assert_eq!(info_count, source.info_count);
    assert_eq!(issue_sha256, source.sha256);
    RenderReportIssueChannels {
        issues,
        info,
        issue_count,
        info_count,
        issue_log_complete: source.complete,
        issue_sha256,
        complete: source.complete,
    }
}

struct RenderReportIssueChannels {
    issues: Vec<RenderIssueReportEntry>,
    info: Vec<RenderIssueReportEntry>,
    issue_count: u64,
    info_count: u64,
    issue_log_complete: bool,
    issue_sha256: String,
    complete: bool,
}

fn sha256_text(value: &str) -> String {
    hex_digest(Sha256::digest(value.as_bytes()).as_slice())
}

fn font_record(resolution: &hwp_render::FontResolution) -> RenderFontRecord {
    RenderFontRecord {
        requested_sha256: sha256_text(&resolution.requested),
        requested_bold: resolution.requested_bold,
        resolved_family_sha256: resolution.resolved.as_deref().map(sha256_text),
        resolved_sha256: resolution.resolved_sha256.clone(),
        resolved_face_index: resolution.resolved_face_index,
        outcome: match resolution.outcome {
            hwp_render::FontResolutionOutcome::Matched => "matched",
            hwp_render::FontResolutionOutcome::Substituted => "substituted",
            hwp_render::FontResolutionOutcome::Missing => "missing",
            hwp_render::FontResolutionOutcome::CoverageSubstituted => "coverage_substituted",
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn write_report(
    report_path: &Path,
    input: RenderInputReport,
    format: &'static str,
    dpi: f32,
    total_pages: usize,
    selected_pages: Vec<usize>,
    fonts: &[hwp_render::FontResolution],
    font_resolution_complete: bool,
    source: hwp_render::RenderIssueReport,
) -> anyhow::Result<()> {
    let coverage = source.font_coverage();
    let channels = format_issue_report(source);
    let mut font_records: Vec<RenderFontRecord> = fonts.iter().map(font_record).collect();
    font_records.sort();
    font_records.truncate(hwp_render::fonts::MAX_FONT_RESOLUTIONS);
    let report = RenderReportFile {
        schema_version: RENDER_REPORT_SCHEMA_VERSION,
        contract: RENDER_REPORT_CONTRACT,
        input,
        format,
        dpi,
        total_pages,
        selected_pages,
        font_coverage: RenderFontCoverage {
            matched: coverage.matched,
            substituted: coverage.substituted,
            missing: coverage.missing,
            subset_fallback: coverage.subset_fallback,
            substitution_free: coverage.substitution_free(),
        },
        font_resolution_complete,
        fonts: font_records,
        issues: channels.issues,
        info: channels.info,
        issue_count: channels.issue_count,
        info_count: channels.info_count,
        issue_log_complete: channels.issue_log_complete,
        issue_sha256: channels.issue_sha256,
        complete: channels.complete,
    };
    let bytes = serde_json::to_vec_pretty(&report)?;
    crate::commands::output::write_validated(
        report_path,
        None,
        |staged| {
            let mut file = File::create(staged)?;
            file.write_all(&bytes)?;
            file.flush()?;
            Ok(())
        },
        |staged, _| {
            let written = std::fs::read(staged)?;
            if written != bytes {
                anyhow::bail!("렌더 보고서 검증 중 바이트 불일치: {}", staged.display());
            }
            let parsed: serde_json::Value = serde_json::from_slice(&written)
                .map_err(|error| anyhow::anyhow!("렌더 보고서 JSON 검증 실패: {error}"))?;
            if !parsed.is_object() {
                anyhow::bail!("렌더 보고서가 JSON 객체가 아닙니다");
            }
            Ok(())
        },
    )?;
    eprintln!("렌더 보고서 저장: {}", report_path.display());
    Ok(())
}

fn ensure_report_destination<'a>(
    report_path: &Path,
    input: &Path,
    outputs: impl IntoIterator<Item = &'a Path>,
) -> anyhow::Result<()> {
    if paths_alias(report_path, input) {
        anyhow::bail!(
            "렌더 보고서 경로가 입력 문서를 덮어쓸 수 있어 거부합니다: {}",
            report_path.display()
        );
    }
    for output in outputs {
        if paths_alias(report_path, output) {
            anyhow::bail!(
                "렌더 보고서 경로가 렌더 출력과 같거나 별칭이라 거부합니다: {}",
                report_path.display()
            );
        }
    }
    Ok(())
}

fn paths_alias(left: &Path, right: &Path) -> bool {
    let left_absolute = lexical_absolute(left);
    let right_absolute = lexical_absolute(right);
    if left_absolute.is_some() && left_absolute == right_absolute {
        return true;
    }
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn lexical_absolute(path: &Path) -> Option<PathBuf> {
    let absolute = std::path::absolute(path).ok()?;
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Some(normalized)
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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
