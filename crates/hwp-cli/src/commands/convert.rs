//! `hwp convert` — 포맷 변환.
//!
//! M2 범위: hwp/hwpx → markdown/JSON. hwpx 쓰기(M4)와 hwp 쓰기(M6)는
//! 이후 마일스톤.

use std::path::{Path, PathBuf};

use crate::commands::cat::{
    LoadOptions, load_document, load_document_with_options, resolve_password_args,
};
use anyhow::Context as _;
use hwp_cli::cli::{ConvertFormat, PasswordArgs};

/// markdown 출력 전용 추가 옵션 (다른 포맷에서는 무시).
#[derive(Default)]
pub struct MdOpts<'a> {
    /// 이미지 추출 디렉터리 (기본: "<출력스템>.media").
    pub media_dir: Option<&'a Path>,
    /// 머리말/꼬리말 텍스트 포함.
    pub with_header_footer: bool,
    /// 숨은 설명 텍스트 포함.
    pub with_hidden: bool,
}

/// 프로그래밍 가능한 변환 서비스의 결과. CLI와 MCP가 같은 경고 계약을 공유한다.
#[derive(Debug, Default)]
pub struct ConvertReport {
    pub warnings: Vec<String>,
    pub preservation: hwp_model::PreservationReport,
}

#[allow(clippy::too_many_arguments)]
fn run_with_options(
    input: &Path,
    output: &Path,
    to: Option<ConvertFormat>,
    strict: bool,
    loss_report: Option<&Path>,
    preserve_layout: bool,
    embed_bin: bool,
    md_opts: &MdOpts,
    font_dirs: Vec<PathBuf>,
    options: &LoadOptions<'_>,
) -> anyhow::Result<()> {
    let report = execute_with_options(
        input,
        output,
        to,
        strict,
        loss_report,
        preserve_layout,
        embed_bin,
        md_opts,
        font_dirs,
        options,
    )?;
    print_warnings(&report.warnings);
    crate::commands::preservation::print_report(&report.preservation);
    eprintln!("변환 완료: {} → {}", input.display(), output.display());
    Ok(())
}

/// Multi-input / stdin/stdout entry point (GM-1/GM-2).
///
/// - Input `-`: detects stdin bytes by signature and stages them to a temp file.
/// - Output `-`: emits only text formats (md/json/html/txt/csv) to stdout (`--to` required).
/// - `--out-dir`: batch-converts multiple inputs to `<stem>.<target extension>` (`--to` required).
#[allow(clippy::too_many_arguments)]
pub fn run_multi_with_password(
    inputs: &[PathBuf],
    output: Option<&Path>,
    out_dir: Option<&Path>,
    to: Option<ConvertFormat>,
    strict: bool,
    loss_report: Option<&Path>,
    preserve_layout: bool,
    embed_bin: bool,
    md_opts: &MdOpts,
    font_dirs: Vec<PathBuf>,
    password_args: PasswordArgs,
) -> anyhow::Result<()> {
    // Resolve a password only after every document input has claimed its I/O
    // channel. In particular, a later `-` must not let --password-stdin read
    // the document bytes as a password before the collision is reported.
    if password_args.password_stdin
        && inputs
            .iter()
            .any(|input| input.as_os_str() == std::ffi::OsStr::new("-"))
    {
        anyhow::bail!("문서 입력과 --password-stdin은 모두 표준 입력을 사용할 수 없습니다");
    }
    let input = inputs
        .first()
        .ok_or_else(|| anyhow::anyhow!("변환 입력이 비어 있습니다"))?;
    let password = resolve_password_args(password_args, input)?;
    run_multi_with_options(
        inputs,
        output,
        out_dir,
        to,
        strict,
        loss_report,
        preserve_layout,
        embed_bin,
        md_opts,
        font_dirs,
        &LoadOptions {
            password: password.as_ref(),
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn run_multi_with_options(
    inputs: &[PathBuf],
    output: Option<&Path>,
    out_dir: Option<&Path>,
    to: Option<ConvertFormat>,
    strict: bool,
    loss_report: Option<&Path>,
    preserve_layout: bool,
    embed_bin: bool,
    md_opts: &MdOpts,
    font_dirs: Vec<PathBuf>,
    options: &LoadOptions<'_>,
) -> anyhow::Result<()> {
    // Input staging (`-` → stdin).
    let mut staged: Option<PathBuf> = None;
    let inputs: Vec<PathBuf> = if inputs.len() == 1 && inputs[0].as_os_str() == "-" {
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut std::io::stdin(), &mut buf)?;
        if buf.is_empty() {
            anyhow::bail!("stdin이 비어 있습니다");
        }
        let ext = crate::format::detect_bytes(&buf)?;
        // Unpredictable name for exclusive creation (create_new) — prevents symlink
        // overwrite and concurrent-run collisions. On unix, make it owner-readable only (0600).
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path =
            std::env::temp_dir().join(format!("hwp-stdin-{}-{nanos}.{ext}", std::process::id()));
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        let mut file = opts.open(&path).with_context(|| {
            format!("stdin 스테이징 파일을 만들 수 없습니다: {}", path.display())
        })?;
        std::io::Write::write_all(&mut file, &buf)?;
        drop(file);
        staged = Some(path.clone());
        vec![path]
    } else {
        inputs.to_vec()
    };
    let result = run_multi_inner(
        &inputs,
        output,
        out_dir,
        to,
        strict,
        loss_report,
        preserve_layout,
        embed_bin,
        md_opts,
        font_dirs,
        options,
    );
    if let Some(path) = staged {
        let _ = std::fs::remove_file(path);
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn run_multi_inner(
    inputs: &[PathBuf],
    output: Option<&Path>,
    out_dir: Option<&Path>,
    to: Option<ConvertFormat>,
    strict: bool,
    loss_report: Option<&Path>,
    preserve_layout: bool,
    embed_bin: bool,
    md_opts: &MdOpts,
    font_dirs: Vec<PathBuf>,
    options: &LoadOptions<'_>,
) -> anyhow::Result<()> {
    match (output, out_dir) {
        (Some(out), None) => {
            if inputs.len() != 1 {
                anyhow::bail!("여러 입력에는 --out-dir이 필요합니다 (-o는 단일 입력 전용)");
            }
            if out.as_os_str() == "-" {
                if loss_report.is_some() {
                    anyhow::bail!("출력이 `-`(stdout)이면 --loss-report를 지원하지 않습니다");
                }
                let Some(target) = to else {
                    anyhow::bail!("출력이 `-`(stdout)이면 --to가 필요합니다");
                };
                let doc =
                    load_document_with_options(&inputs[0], options).map_err(anyhow::Error::new)?;
                print_text_output(&doc, target, embed_bin, md_opts)?;
                return Ok(());
            }
            run_with_options(
                &inputs[0],
                out,
                to,
                strict,
                loss_report,
                preserve_layout,
                embed_bin,
                md_opts,
                font_dirs,
                options,
            )
        }
        (None, Some(dir)) => {
            let Some(target) = to else {
                anyhow::bail!("여러 입력(--out-dir)에는 --to가 필요합니다");
            };
            if loss_report.is_some() {
                anyhow::bail!(
                    "--loss-report는 단일 입력 변환에서만 지원합니다 (--out-dir와 병용 불가)"
                );
            }
            // Pre-check: reject collisions where identical stems from different directories would overwrite one output.
            let mut seen = std::collections::BTreeMap::new();
            for input in inputs {
                let stem = input
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .with_context(|| {
                        format!("입력 파일 이름을 확인할 수 없습니다: {}", input.display())
                    })?;
                if let Some(prev) = seen.insert(stem.to_string(), input.clone()) {
                    anyhow::bail!(
                        "배치 출력 이름이 충돌합니다: {} 와 {} → {stem}.{}",
                        prev.display(),
                        input.display(),
                        target_extension(target)
                    );
                }
            }
            // Validate every source before creating the destination directory or
            // publishing any batch member. A later bad credential must not leave
            // earlier converted documents behind.
            for input in inputs {
                load_document_with_options(input, options).map_err(anyhow::Error::new)?;
            }
            std::fs::create_dir_all(dir)?;
            for input in inputs {
                let stem = input
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .with_context(|| {
                        format!("입력 파일 이름을 확인할 수 없습니다: {}", input.display())
                    })?;
                let out = dir.join(format!("{stem}.{}", target_extension(target)));
                run_with_options(
                    input,
                    &out,
                    Some(target),
                    strict,
                    None,
                    preserve_layout,
                    embed_bin,
                    md_opts,
                    font_dirs.clone(),
                    options,
                )?;
            }
            Ok(())
        }
        (None, None) => anyhow::bail!("출력을 지정하세요: -o <파일> 또는 --out-dir <디렉터리>"),
        (Some(_), Some(_)) => anyhow::bail!("-o와 --out-dir은 함께 쓸 수 없습니다"),
    }
}

/// stdout text output (GM-2) — text formats only.
fn print_text_output(
    doc: &hwp_model::Document,
    target: ConvertFormat,
    embed_bin: bool,
    md_opts: &MdOpts,
) -> anyhow::Result<()> {
    let text = match target {
        ConvertFormat::Md => hwp_convert::to_markdown_with(
            doc,
            &hwp_convert::MarkdownOptions {
                text: hwp_model::TextOptions {
                    include_header_footer: md_opts.with_header_footer,
                    include_hidden: md_opts.with_hidden,
                },
                ..Default::default()
            },
        )?,
        ConvertFormat::Json => hwp_convert::to_json(doc, true, embed_bin)?,
        ConvertFormat::Html => hwp_convert::to_html(doc),
        ConvertFormat::Txt => doc.plain_text(),
        ConvertFormat::Csv => hwp_convert::to_csv(doc),
        other => anyhow::bail!(
            "`-`(stdout) 출력은 텍스트 포맷(md/json/html/txt/csv)만 지원합니다: {other:?}"
        ),
    };
    print!("{text}");
    Ok(())
}

/// Standard extension of a format (for --out-dir output names).
fn target_extension(target: ConvertFormat) -> &'static str {
    match target {
        ConvertFormat::Hwp => "hwp",
        ConvertFormat::Hwpx => "hwpx",
        ConvertFormat::Md => "md",
        ConvertFormat::Json => "json",
        ConvertFormat::Html => "html",
        ConvertFormat::Pdf => "pdf",
        ConvertFormat::Odt => "odt",
        ConvertFormat::Txt => "txt",
        ConvertFormat::Csv => "csv",
        ConvertFormat::Docx => "docx",
    }
}

/// `hwp convert`와 MCP가 함께 쓰는 변환 서비스.
///
/// 출력은 검증이 끝날 때까지 destination에 게시하지 않으며, Markdown 이미지 sidecar도
/// 본문과 같은 복구 journal에 참여한다. 사용자 메시지 출력은 호출자가 담당한다.
///
/// `loss_report`가 주어지면 preservation 검사 직후(strict 거부 전)에 typed ledger
/// (`hwp-preservation-report-v1`)를 무손실이어도 항상 JSON으로 게시한다 — 자동화가
/// 성공/실패와 무관하게 같은 경로에서 판정 근거를 읽을 수 있게 하기 위함이다.
#[allow(clippy::too_many_arguments)]
pub fn execute(
    input: &Path,
    output: &Path,
    to: Option<ConvertFormat>,
    strict: bool,
    loss_report: Option<&Path>,
    preserve_layout: bool,
    embed_bin: bool,
    md_opts: &MdOpts,
    font_dirs: Vec<PathBuf>,
) -> anyhow::Result<ConvertReport> {
    execute_with_options(
        input,
        output,
        to,
        strict,
        loss_report,
        preserve_layout,
        embed_bin,
        md_opts,
        font_dirs,
        &LoadOptions::default(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn execute_with_options(
    input: &Path,
    output: &Path,
    to: Option<ConvertFormat>,
    strict: bool,
    loss_report: Option<&Path>,
    preserve_layout: bool,
    embed_bin: bool,
    md_opts: &MdOpts,
    font_dirs: Vec<PathBuf>,
    options: &LoadOptions<'_>,
) -> anyhow::Result<ConvertReport> {
    let target = match to {
        Some(t) => t,
        None => infer_format(output)?,
    };
    if let Some(report_path) = loss_report {
        // 경로 정규화(canonicalize 기반 — `.`/`..`/심볼릭 링크 철자 변형 해소) +
        // output.rs의 identity 기반 별칭 탐지를 함께 쓴다. raw Path 비교만으로는
        // `sub/../in.hwpx` 같은 철자 변형을 놓치고, 경로 비교만으로는 하드 링크
        // 별칭을 놓친다. 리포트는 입력을 덮어쓰면 안 되고, 출력과 같으면 문서
        // 게시가 리포트를 조용히 덮어쓰므로 둘 다 거부한다.
        let report_normalized = normalize_for_alias_compare(report_path);
        for other in [input, output] {
            if report_normalized == normalize_for_alias_compare(other) {
                anyhow::bail!(
                    "--loss-report 경로가 입력/출력과 같을 수 없습니다: {}",
                    report_path.display()
                );
            }
        }
        crate::commands::output::reject_output_aliases(report_path, &[input, output])?;
    }
    let doc = load_document_with_options(input, options).map_err(anyhow::Error::new)?;
    if matches!(target, ConvertFormat::Md) {
        let (media_destination, media_prefix) = markdown_media_paths(output, md_opts.media_dir)?;
        let warnings = crate::commands::output::write_validated_with_sidecar(
            output,
            Some(input),
            &media_destination,
            |staged, staged_media| {
                let md = hwp_convert::to_markdown_with(
                    &doc,
                    &hwp_convert::MarkdownOptions {
                        media_dir: Some(staged_media),
                        media_prefix: Some(&media_prefix),
                        text: hwp_model::TextOptions {
                            include_header_footer: md_opts.with_header_footer,
                            include_hidden: md_opts.with_hidden,
                        },
                    },
                )?;
                std::fs::write(staged, md)?;
                Ok(Vec::new())
            },
            |_, _, _| Ok(()),
        )?;
        let report = ConvertReport {
            warnings,
            preservation: hwp_model::PreservationReport::new(),
        };
        if let Some(report_path) = loss_report {
            write_loss_report(report_path, &report.preservation)?;
        }
        return Ok(report);
    }

    let source_format = crate::format::detect(input)?;
    let same_native_format = matches!(
        (source_format, target),
        (crate::format::FileFormat::Hwp5, ConvertFormat::Hwp)
            | (crate::format::FileFormat::Hwpx, ConvertFormat::Hwpx)
    );
    let write_staged = |source: &std::path::Path, staged: &std::path::Path| {
        let mut report = match target {
            ConvertFormat::Md => {
                unreachable!("Markdown은 sidecar 트랜잭션 경로에서 처리")
            }
            ConvertFormat::Html => {
                std::fs::write(staged, hwp_convert::to_html(&doc))?;
                hwp_model::WriteReport::new()
            }
            ConvertFormat::Txt => {
                std::fs::write(staged, doc.plain_text())?;
                hwp_model::WriteReport::new()
            }
            ConvertFormat::Csv => {
                std::fs::write(staged, hwp_convert::to_csv(&doc))?;
                hwp_model::WriteReport::new()
            }
            ConvertFormat::Docx => {
                std::fs::write(staged, hwp_convert::to_docx(&doc)?)?;
                hwp_model::WriteReport::new()
            }
            ConvertFormat::Odt => {
                std::fs::write(staged, hwp_convert::to_odt(&doc)?)?;
                hwp_model::WriteReport::new()
            }
            ConvertFormat::Pdf => {
                let result =
                    hwp_render::render_document_pdf(&doc, &pdf_render_opts(font_dirs), None)?;
                std::fs::write(staged, &result.data)?;
                hwp_model::WriteReport {
                    warnings: render_issue_messages(&result.report),
                    preservation: hwp_model::PreservationReport::new(),
                }
            }
            ConvertFormat::Json => {
                std::fs::write(staged, hwp_convert::to_json(&doc, true, embed_bin)?)?;
                hwp_model::WriteReport::new()
            }
            ConvertFormat::Hwpx => hwpx::write::write_document_with_report_with(
                &doc,
                staged,
                &hwpx::write::HwpxWriteOptions {
                    preserve_linesegs: preserve_layout,
                },
            )?,
            ConvertFormat::Hwp if source_format == crate::format::FileFormat::Hwp5 => {
                write_hwp_preserving_source(source, &doc, &doc, staged, preserve_layout, false)?
            }
            ConvertFormat::Hwp => write_hwp(&doc, staged, preserve_layout)?,
        };

        if matches!(target, ConvertFormat::Hwp | ConvertFormat::Hwpx) {
            if same_native_format {
                report.preservation.extend(
                    crate::commands::preservation::inspect_same_format_container(source, staged)?,
                );
            } else {
                // 크로스 포맷: IR이 원본 컨테이너에서 보존한 패키지/컨테이너 수준
                // 자산 중 대상 포맷이 표현 못 하는 것을 typed event로 계상한다.
                let target_format = match target {
                    ConvertFormat::Hwp => crate::format::FileFormat::Hwp5,
                    _ => crate::format::FileFormat::Hwpx,
                };
                report.preservation.extend(
                    crate::commands::preservation::inspect_cross_format_container(
                        &doc,
                        source_format,
                        target_format,
                    ),
                );
            }
            let output_document = load_document(staged)
                .with_context(|| format!("변환 문서 재읽기 실패: {}", staged.display()))?;
            report.preservation.extend(
                crate::commands::preservation::inspect_conversion_semantics(&doc, &output_document),
            );
        }
        Ok(report)
    };
    let verify_staged = |staged: &std::path::Path, report: &hwp_model::WriteReport| {
        // strict 거부 전에 기록해야 실패한 변환의 판정 근거가 파일로 남는다.
        if let Some(report_path) = loss_report {
            write_loss_report(report_path, &report.preservation)?;
        }
        if matches!(target, ConvertFormat::Hwp | ConvertFormat::Hwpx) {
            if same_native_format || strict {
                crate::commands::reject_preservation_loss("convert", &report.preservation)?;
            }
            load_document(staged)
                .with_context(|| format!("변환 문서 재읽기 실패: {}", staged.display()))?;
        }
        Ok(())
    };
    let write_report = if source_format == crate::format::FileFormat::Hwp5
        && matches!(target, ConvertFormat::Hwp)
    {
        let (_, report) = crate::commands::output::write_with_private_input_snapshot(
            output,
            input,
            hwp_cli::certification::MAX_INPUT_BYTES,
            crate::commands::output::SnapshotOutputMode::Publish,
            |snapshot, staged, _| write_staged(snapshot, staged),
            verify_staged,
        )?;
        report
    } else {
        crate::commands::output::write_validated(
            output,
            Some(input),
            |staged| write_staged(input, staged),
            verify_staged,
        )?
    };
    Ok(ConvertReport {
        warnings: write_report.warnings,
        preservation: write_report.preservation,
    })
}

fn markdown_media_paths(
    output: &Path,
    requested: Option<&Path>,
) -> anyhow::Result<(PathBuf, String)> {
    match requested {
        Some(directory) => {
            let resolved = if directory.is_absolute() {
                directory.to_path_buf()
            } else {
                output.parent().map_or_else(
                    || directory.to_path_buf(),
                    |parent| {
                        if parent.as_os_str().is_empty() {
                            directory.to_path_buf()
                        } else {
                            parent.join(directory)
                        }
                    },
                )
            };
            Ok((resolved, directory.to_string_lossy().into_owned()))
        }
        None => {
            let directory = output.with_extension("media");
            let prefix = directory
                .file_name()
                .with_context(|| {
                    format!(
                        "기본 미디어 디렉터리 이름을 확인할 수 없습니다: {}",
                        directory.display()
                    )
                })?
                .to_string_lossy()
                .into_owned();
            Ok((directory, prefix))
        }
    }
}

/// 경고 목록을 stderr로 출력.
pub(crate) fn print_warnings(warnings: &[String]) {
    for w in warnings {
        eprintln!("경고: {w}");
    }
}

/// `--loss-report` 별칭 가드용 경로 정규화. 존재하는 경로는 canonicalize하고,
/// 아직 없는 경로(새 리포트 파일)는 부모 디렉터리만 canonicalize해 파일명을
/// 붙인다 — `.`/`..`/심볼릭 링크 철자 변형이 같은 파일을 가리키면 같아진다.
/// 정규화가 불가능하면 절대경로로, 그마저 실패하면 원본 경로로 되돌아간다.
fn normalize_for_alias_compare(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
        && let Ok(canonical_parent) = std::fs::canonicalize(parent)
    {
        return canonical_parent.join(name);
    }
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// `--loss-report` 산출물 — 다른 출력과 같은 staged/검증 트랜잭션으로 게시한다
/// (렌더 `--report`의 write_report와 같은 규율). 보고서는 content-free 계약이라
/// 입력·출력 경로를 싣지 않는다.
fn write_loss_report(path: &Path, report: &hwp_model::PreservationReport) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(report)?;
    crate::commands::output::write_validated(
        path,
        None,
        |staged| {
            std::fs::write(staged, &bytes)?;
            Ok(())
        },
        |staged, _| {
            let written = std::fs::read(staged)?;
            if written != bytes {
                anyhow::bail!("보존 보고서 검증 중 바이트 불일치: {}", staged.display());
            }
            let parsed: serde_json::Value = serde_json::from_slice(&written)
                .map_err(|error| anyhow::anyhow!("보존 보고서 JSON 검증 실패: {error}"))?;
            if !parsed.is_object() {
                anyhow::bail!("보존 보고서가 JSON 객체가 아닙니다");
            }
            Ok(())
        },
    )?;
    eprintln!("보존 보고서 저장: {}", path.display());
    Ok(())
}

/// `--font-dir`가 비었으면 `HWP_FONT_DIR`(없으면 `fonts/`)로 기본 폰트 디렉터리를 정한다.
/// render·diff·convert(PDF)가 번들 함초롬 글꼴을 명시 인자 없이도 로드하도록 —
/// 안 그러면 한글 기본 글꼴을 못 찾아 시스템 폰트로 대체돼 렌더 충실도가 크게 떨어진다.
pub fn resolve_font_dirs(given: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
    if !given.is_empty() {
        return given;
    }
    vec![std::path::PathBuf::from(
        std::env::var("HWP_FONT_DIR").unwrap_or_else(|_| "fonts".into()),
    )]
}

/// PDF 렌더 옵션 — 폰트는 `given`(`--font-dir`)이 비었으면 `HWP_FONT_DIR`
/// (없으면 `fonts/`)에서 해석.
fn pdf_render_opts(given: Vec<PathBuf>) -> hwp_render::RenderOptions {
    hwp_render::RenderOptions {
        dpi: 96.0,
        font_dirs: resolve_font_dirs(given),
    }
}

fn infer_format(output: &Path) -> anyhow::Result<ConvertFormat> {
    match output
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("md") | Some("markdown") => Ok(ConvertFormat::Md),
        Some("html") | Some("htm") => Ok(ConvertFormat::Html),
        Some("odt") => Ok(ConvertFormat::Odt),
        Some("pdf") => Ok(ConvertFormat::Pdf),
        Some("json") => Ok(ConvertFormat::Json),
        Some("txt") => Ok(ConvertFormat::Txt),
        Some("csv") => Ok(ConvertFormat::Csv),
        Some("docx") => Ok(ConvertFormat::Docx),
        Some("hwpx") => Ok(ConvertFormat::Hwpx),
        Some("hwp") => Ok(ConvertFormat::Hwp),
        other => {
            anyhow::bail!("출력 포맷을 추론할 수 없습니다 (확장자: {other:?}) — --to로 지정하세요")
        }
    }
}

/// hwp 바이너리 저장 (1쪽 렌더를 PrvImage로 동봉).
///
/// 합성 문서(md/hwpx 출신)는 줄 배치(PARA_LINE_SEG)가 없으면 5.1.x 한글이
/// 본문을 못 그린다(검은 바/빈 내용/손상). 폰트 셰이핑으로 정확한 줄바꿈을
/// 계산해 IR에 채운 뒤 쓴다 — 한글과 동일한 함초롬바탕 폰트가 필요하다
/// (HWP_FONT_DIR 환경변수 또는 프로젝트 `fonts/`).
pub fn write_hwp(
    doc: &hwp_model::Document,
    output: &std::path::Path,
    preserve_layout: bool,
) -> anyhow::Result<hwp_model::WriteReport> {
    write_hwp_impl(doc, output, preserve_layout, false, None, None)
}

/// Writes an HWP edit against an immutable source snapshot. The low-level
/// writer derives its target stream set from `original` versus `doc` and keeps
/// every unrelated CFB entry owned by `source`.
pub fn write_hwp_preserving_source(
    source: &std::path::Path,
    original: &hwp_model::Document,
    doc: &hwp_model::Document,
    output: &std::path::Path,
    preserve_layout: bool,
    structural: bool,
) -> anyhow::Result<hwp_model::WriteReport> {
    write_hwp_impl(
        doc,
        output,
        preserve_layout,
        structural,
        None,
        Some((source, original)),
    )
}

/// 편집된 문서를 hwp로 다시 쓴다.
///
/// hwp5 원본 편집은 **외과적**으로 처리한다: 편집된 문단만 줄 배치를 비워 한글이
/// 그 문단만 재계산하게 하고(편집 프리미티브가 이미 비움), 미편집 문단은 원본 줄
/// 배치를 보존한다. 줄 배치를 전부 비우면 한글이 모든 문단을 재계산하면서 표 셀의
/// 빈 문단까지 큰 글자 크기로 한 줄을 잡아 행 높이가 부풀어 빈 칸이 생긴다(실측).
/// 한글 자신이 편집 시 바뀐 문단만 다시 배치하는 것과 같은 동작.
///
/// hwpx/md 출신은 보존할 원본 줄 배치가 한글 hwp 레이아웃과 다르므로 합성 경로
/// (`edited`=true: 줄 배치 비우고 para_shape 복원 후 한글 재계산)를 쓴다.
pub fn write_hwp_edited(
    doc: &hwp_model::Document,
    output: &std::path::Path,
) -> anyhow::Result<hwp_model::WriteReport> {
    if doc.meta.source_format == "hwp5" {
        // 원본 줄 배치 보존(preserve), 합성 정규화 없음 — 편집 문단만 count=0.
        write_hwp_impl(doc, output, true, false, None, None)
    } else {
        write_hwp_impl(doc, output, false, true, None, None)
    }
}

/// 구조 편집(문단/표 행 추가·삭제)본을 hwp로 쓴다.
///
/// 모든 출처에 합성 경로(edited=true)를 강제한다 — 삽입된 문단/행에 문단끝 0x0d·
/// 마지막문단 비트·PARA/셀 카운트 같은 불변식이 적용돼야 하기 때문(convert/new와
/// 동일한 한글 수용 검증 경로). hwp5 무수정용 surgical `write_hwp_edited`와 분리한다.
pub fn write_hwp_structural(
    doc: &hwp_model::Document,
    output: &std::path::Path,
) -> anyhow::Result<hwp_model::WriteReport> {
    write_hwp_impl(doc, output, false, true, None, None)
}

/// 구조 문서를 시스템 글꼴 없이 명시된 파일만으로 HWP로 쓴다.
///
/// 인증 코퍼스처럼 환경 독립성이 필요한 경로가 사용한다. `font_files`의 순서는
/// manifest 순서여야 하며 빈 목록은 거부한다. 일반 사용자 경로의 기존 ambient
/// 글꼴 동작과 의도적으로 분리한다.
pub fn write_hwp_structural_isolated(
    doc: &hwp_model::Document,
    output: &std::path::Path,
    font_files: &[std::path::PathBuf],
) -> anyhow::Result<hwp_model::WriteReport> {
    if font_files.is_empty() {
        anyhow::bail!("isolated HWP writer requires at least one explicit font file");
    }
    write_hwp_impl(doc, output, false, true, Some(font_files), None)
}

fn write_hwp_impl(
    doc: &hwp_model::Document,
    output: &std::path::Path,
    preserve_layout: bool,
    edited: bool,
    isolated_font_files: Option<&[std::path::PathBuf]>,
    source: Option<(&std::path::Path, &hwp_model::Document)>,
) -> anyhow::Result<hwp_model::WriteReport> {
    let font_dir =
        std::path::PathBuf::from(std::env::var("HWP_FONT_DIR").unwrap_or_else(|_| "fonts".into()));
    let synthesize = doc.meta.source_format != "hwp5" || edited;
    let has_source_linesegs = doc
        .sections
        .iter()
        .flat_map(|s| &s.paragraphs)
        .any(|p| !p.line_segs.is_empty());
    let source_has_edits = source.is_some_and(|(_, original)| original != doc);

    let mut report = hwp_model::WriteReport::new();
    let owned;
    let doc = if source_has_edits {
        // Native HWP edits keep every unchanged paragraph as an immutable source
        // subtree. Missing line layout therefore identifies changed/new paragraphs;
        // synthesize them before stream materialization. The source-aware writer
        // replaces the renderer's output for every unchanged paragraph with the
        // original record tree, so originally cache-less paragraphs stay untouched.
        let mut d = doc.clone();
        let mut store = if isolated_font_files.is_some() {
            hwp_render::FontStore::new_isolated()
        } else {
            hwp_render::FontStore::new()
        };
        if let Some(files) = isolated_font_files {
            for file in files {
                store.load_file(file).with_context(|| {
                    format!("isolated font file could not be loaded: {}", file.display())
                })?;
            }
        } else {
            store.load_dir(&font_dir);
        }
        let mut warns = hwp_render::RenderIssueAccumulator::new();
        hwp_render::lineseg::synthesize_linesegs(&mut d, &mut store, &mut warns);
        if let Some((_, original)) = source {
            restore_unchanged_native_paragraphs(original, doc, &mut d);
        }
        warns.absorb(store.issues.finish());
        report
            .warnings
            .append(&mut render_issue_messages(&warns.finish()));
        owned = d;
        &owned
    } else if source.is_some() || !synthesize || preserve_layout {
        // hwp5 무수정/preserve-layout: 원본 줄 배치 그대로.
        doc
    } else if has_source_linesegs {
        // hwpx 출신 또는 편집된 hwp5: 저장된 줄 배치는 (편집으로) 내용과 어긋나거나
        // 한글의 hwpx 내보내기 레이아웃이라 hwp 저장본과 다를 수 있다(예: 같은
        // 문서가 hwpx 6쪽, hwp 5쪽). 줄 배치를 제거하면 한글이 열 때 문단/글자
        // 모양 기준으로 재계산해 hwp 저장본과 같은 페이지로 흐른다(문단 모양을
        // 정품과 일치시킨 게 핵심). 편집본도 이 경로를 그대로 쓴다(편집으로 낡은
        // 줄 배치를 비우고 한글이 재계산 — convert hwpx→hwp와 동일한 검증된 동작).
        let mut d = doc.clone();
        clear_linesegs(&mut d);
        owned = d;
        &owned
    } else {
        // markdown 등 줄 배치 없는 출처: 폰트 셰이핑으로 합성.
        let mut d = doc.clone();
        let mut store = if isolated_font_files.is_some() {
            hwp_render::FontStore::new_isolated()
        } else {
            hwp_render::FontStore::new()
        };
        if let Some(files) = isolated_font_files {
            for file in files {
                store.load_file(file).with_context(|| {
                    format!("isolated font file could not be loaded: {}", file.display())
                })?;
            }
        } else {
            store.load_dir(&font_dir);
        }
        let mut warns = hwp_render::RenderIssueAccumulator::new();
        hwp_render::lineseg::synthesize_linesegs(&mut d, &mut store, &mut warns);
        warns.absorb(store.issues.finish());
        report
            .warnings
            .append(&mut render_issue_messages(&warns.finish()));
        owned = d;
        &owned
    };

    let render_options = hwp_render::RenderOptions {
        dpi: 48.0,
        font_dirs: if isolated_font_files.is_some() {
            Vec::new()
        } else {
            vec![font_dir]
        },
    };
    let prv_image = if let Some(files) = isolated_font_files {
        let output =
            hwp_render::render_document_pages_isolated(doc, &render_options, Some(&[1]), files)
                .context("isolated HWP preview render failed")?;
        Some(
            output
                .pages
                .first()
                .context("isolated HWP preview render returned no page")?
                .encode_png()
                .map_err(|error| anyhow::anyhow!("isolated HWP preview PNG failed: {error}"))?,
        )
    } else {
        hwp_render::render_document_pages(doc, &render_options, Some(&[1]))
            .ok()
            .and_then(|out| out.pages.first().and_then(|p| p.encode_png().ok()))
    };

    let options = hwp5::WriteOptions {
        prv_image,
        preserve_linesegs: preserve_layout,
        edited,
    };
    let writer_report = if let Some((source, original)) = source {
        hwp5::rewrite_document_with_report(source, original, doc, output, &options)?
    } else {
        hwp5::write_document_with_report(doc, output, &options)?
    };
    report.warnings.extend(writer_report.warnings);
    report.preservation.extend(writer_report.preservation);
    Ok(report)
}

/// After renderer layout synthesis, put every semantically unchanged native
/// paragraph back exactly as it appeared in the immutable input snapshot.
/// This limits new PARA_LINE_SEG records to edited and inserted paragraphs,
/// including paragraphs nested in table cells and generic controls.
fn restore_unchanged_native_paragraphs(
    original: &hwp_model::Document,
    edited: &hwp_model::Document,
    synthesized: &mut hwp_model::Document,
) {
    for (section_index, synthesized_section) in synthesized.sections.iter_mut().enumerate() {
        let (Some(original_section), Some(edited_section)) = (
            original.sections.get(section_index),
            edited.sections.get(section_index),
        ) else {
            continue;
        };
        restore_unchanged_paragraph_list(
            &original_section.paragraphs,
            &edited_section.paragraphs,
            &mut synthesized_section.paragraphs,
        );
    }
}

fn restore_unchanged_paragraph_list(
    original: &[hwp_model::Paragraph],
    edited: &[hwp_model::Paragraph],
    synthesized: &mut [hwp_model::Paragraph],
) {
    if edited.len() != synthesized.len() {
        return;
    }
    let mut used = vec![false; original.len()];
    for edited_index in 0..edited.len() {
        let instance_id = edited[edited_index].header.instance_id;
        let original_index = if instance_id != 0
            && edited
                .iter()
                .filter(|paragraph| paragraph.header.instance_id == instance_id)
                .count()
                == 1
        {
            let matches = original
                .iter()
                .enumerate()
                .filter(|(index, paragraph)| {
                    !used[*index] && paragraph.header.instance_id == instance_id
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if matches.len() == 1 {
                Some(matches[0])
            } else {
                None
            }
        } else {
            None
        }
        .or_else(|| {
            original
                .iter()
                .enumerate()
                .find(|(index, paragraph)| !used[*index] && *paragraph == &edited[edited_index])
                .map(|(index, _)| index)
        })
        .or_else(|| {
            (original.len() == edited.len() && !used[edited_index]).then_some(edited_index)
        });

        if let Some(original_index) = original_index {
            used[original_index] = true;
            restore_unchanged_paragraph(
                &original[original_index],
                &edited[edited_index],
                &mut synthesized[edited_index],
            );
        }
    }
}

fn restore_unchanged_paragraph(
    original: &hwp_model::Paragraph,
    edited: &hwp_model::Paragraph,
    synthesized: &mut hwp_model::Paragraph,
) {
    if original == edited {
        *synthesized = original.clone();
        return;
    }
    for ((original_control, edited_control), synthesized_control) in original
        .controls
        .iter()
        .zip(&edited.controls)
        .zip(&mut synthesized.controls)
    {
        if original_control == edited_control {
            *synthesized_control = original_control.clone();
            continue;
        }
        match (original_control, edited_control, synthesized_control) {
            (
                hwp_model::Control::Table(original_table),
                hwp_model::Control::Table(edited_table),
                hwp_model::Control::Table(synthesized_table),
            ) => {
                for ((original_cell, edited_cell), synthesized_cell) in original_table
                    .cells
                    .iter()
                    .zip(&edited_table.cells)
                    .zip(&mut synthesized_table.cells)
                {
                    restore_unchanged_paragraph_list(
                        &original_cell.paragraphs,
                        &edited_cell.paragraphs,
                        &mut synthesized_cell.paragraphs,
                    );
                }
            }
            (
                hwp_model::Control::Generic(original_generic),
                hwp_model::Control::Generic(edited_generic),
                hwp_model::Control::Generic(synthesized_generic),
            ) => {
                for ((original_list, edited_list), synthesized_list) in original_generic
                    .paragraph_lists
                    .iter()
                    .zip(&edited_generic.paragraph_lists)
                    .zip(&mut synthesized_generic.paragraph_lists)
                {
                    restore_unchanged_paragraph_list(
                        &original_list.paragraphs,
                        &edited_list.paragraphs,
                        &mut synthesized_list.paragraphs,
                    );
                }
            }
            _ => {}
        }
    }
}

fn render_issue_messages(report: &hwp_render::RenderIssueReport) -> Vec<String> {
    let mut messages = report
        .issues
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if !report.complete {
        messages.push("render_issue_accumulator_incomplete".to_string());
    }
    messages
}

/// 모든 문단(표 셀·머리말 등 중첩 포함)의 줄 배치를 제거한다 — 한글이 열 때
/// 문단/글자 모양 기준으로 재계산하도록(hwpx 내보내기 줄배치가 hwp와 다른 문제 회피).
fn clear_linesegs(doc: &mut hwp_model::Document) {
    fn clear_para(para: &mut hwp_model::Paragraph) {
        para.line_segs.clear();
        for control in &mut para.controls {
            match control {
                hwp_model::Control::Table(t) => {
                    for cell in &mut t.cells {
                        for p in &mut cell.paragraphs {
                            clear_para(p);
                        }
                    }
                }
                hwp_model::Control::Generic(g) => {
                    for list in &mut g.paragraph_lists {
                        for p in &mut list.paragraphs {
                            clear_para(p);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    for section in &mut doc.sections {
        for para in &mut section.paragraphs {
            clear_para(para);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn staged_markdown_media_keeps_final_link_prefixes() {
        let output = Path::new("/work/report.md");
        let (default_directory, default_prefix) = markdown_media_paths(output, None).unwrap();
        assert_eq!(default_directory, Path::new("/work/report.media"));
        assert_eq!(default_prefix, "report.media");

        let requested = Path::new("my figs");
        let (custom_directory, custom_prefix) =
            markdown_media_paths(output, Some(requested)).unwrap();
        assert_eq!(custom_directory, Path::new("/work/my figs"));
        assert_eq!(custom_prefix, "my figs");
    }

    #[test]
    fn strict_drop_failure_preserves_existing_destination() {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "hwp-convert-strict-test-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        let destination = dir.join("result.hwp");
        std::fs::write(&destination, b"ORIGINAL").unwrap();

        let result = crate::commands::output::write_validated(
            &destination,
            None,
            |staged| {
                std::fs::write(staged, b"PARTIAL CONVERSION")?;
                let mut report = hwp_model::WriteReport::new();
                report.loss(
                    hwp_model::PreservationCode::OpaqueControlUnrepresentable,
                    hwp_model::PreservationResourceKind::Control,
                    hwp_model::PreservationDisposition::Unrepresentable,
                    1,
                );
                Ok(report)
            },
            |_, report| crate::commands::reject_preservation_loss("convert", &report.preservation),
        );
        assert!(result.is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"ORIGINAL");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn same_format_opaque_package_entry_is_preserved() {
        // 예전에는 writer 고정 목록 밖 엔트리가 silently drop돼 fail-closed로
        // 거부됐다. 이제 같은 포맷 재작성은 잉여 엔트리를 바이트 그대로 보존하므로
        // 변환이 성공하고 엔트리가 살아 있어야 한다(epic #90).
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "hwp-convert-preservation-test-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        let generated = dir.join("generated.hwpx");
        let source = dir.join("source.hwpx");
        let destination = dir.join("result.hwpx");
        hwpx::write_document(
            &hwp_convert::from_markdown("owner-authored fixture"),
            &generated,
        )
        .unwrap();
        append_synthetic_package_entry(&generated, &source);
        std::fs::write(&destination, b"ORIGINAL").unwrap();

        let report = execute(
            &source,
            &destination,
            Some(ConvertFormat::Hwpx),
            false,
            None,
            false,
            false,
            &MdOpts::default(),
            Vec::new(),
        )
        .unwrap();

        assert!(
            report.preservation.is_lossless(),
            "preservation events: {:?}",
            report.preservation.events
        );
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&destination).unwrap()).unwrap();
        let mut bytes = Vec::new();
        archive
            .by_name("SyntheticOpaque/entry.bin")
            .expect("잉여 패키지 엔트리가 보존돼야 한다")
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"owner-authored opaque payload");
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn append_synthetic_package_entry(source: &Path, output: &Path) {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(source).unwrap()).unwrap();
        let mut writer = zip::ZipWriter::new(std::fs::File::create(output).unwrap());
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let options =
                zip::write::SimpleFileOptions::default().compression_method(entry.compression());
            writer.start_file(entry.name(), options).unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            writer.write_all(&bytes).unwrap();
        }
        writer
            .start_file(
                "SyntheticOpaque/entry.bin",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(b"owner-authored opaque payload").unwrap();
        writer.finish().unwrap();
    }

    /// 크로스 포맷(hwpx→hwp) 변환에서 패키지 수준 잉여 엔트리(DocOptions 등)는
    /// hwp 컨테이너에 대응 슬롯이 없어 사라진다. strict는 fail-closed로 게시를
    /// 거부하고 기존 destination을 보존해야 하며, --loss-report는 거부 전에도
    /// typed ledger를 남겨야 한다(epic #90 PR 3).
    #[test]
    fn strict_cross_format_hwpx_to_hwp_fails_closed_with_loss_report() {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "hwp-convert-cross-strict-test-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        let generated = dir.join("generated.hwpx");
        let source = dir.join("source.hwpx");
        let destination = dir.join("result.hwp");
        let report_path = dir.join("loss.json");
        hwpx::write_document(
            &hwp_convert::from_markdown("owner-authored fixture"),
            &generated,
        )
        .unwrap();
        append_synthetic_package_entry(&generated, &source);
        std::fs::write(&destination, b"ORIGINAL").unwrap();

        let result = execute(
            &source,
            &destination,
            Some(ConvertFormat::Hwp),
            true,
            Some(&report_path),
            false,
            false,
            &MdOpts::default(),
            Vec::new(),
        );
        let error = format!("{:#}", result.unwrap_err());
        assert!(
            error.contains("hwpx_package_entry_removed"),
            "strict 거부 사유에 typed code가 있어야 한다: {error}"
        );
        assert_eq!(std::fs::read(&destination).unwrap(), b"ORIGINAL");

        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
        assert_eq!(value["contract"], "hwp-preservation-report-v1");
        let events = value["events"].as_array().unwrap();
        assert!(
            events
                .iter()
                .any(|event| event["code"] == "hwpx_package_entry_removed"
                    && event["resource"] == "package_entry"
                    && event["disposition"] == "removed"
                    && event["count"].as_u64().unwrap() >= 1),
            "loss report events: {events:?}"
        );
        assert_loss_report_schema_valid(&value);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 비-strict 크로스 포맷 변환은 손실을 ledger에 남기고 게시한다.
    /// --loss-report는 실행 결과와 같은 이벤트를 스키마 적합 JSON으로 기록한다.
    #[test]
    fn non_strict_cross_format_hwpx_to_hwp_publishes_with_loss_report() {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "hwp-convert-cross-nonstrict-test-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        let generated = dir.join("generated.hwpx");
        let source = dir.join("source.hwpx");
        let destination = dir.join("result.hwp");
        let report_path = dir.join("loss.json");
        hwpx::write_document(
            &hwp_convert::from_markdown("owner-authored fixture"),
            &generated,
        )
        .unwrap();
        append_synthetic_package_entry(&generated, &source);

        let report = execute(
            &source,
            &destination,
            Some(ConvertFormat::Hwp),
            false,
            Some(&report_path),
            false,
            false,
            &MdOpts::default(),
            Vec::new(),
        )
        .unwrap();

        assert!(
            report
                .preservation
                .events
                .iter()
                .any(|event| event.code == hwp_model::PreservationCode::HwpxPackageEntryRemoved),
            "preservation events: {:?}",
            report.preservation.events
        );
        assert_ne!(std::fs::read(&destination).unwrap(), b"ORIGINAL");

        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
        assert_eq!(value["contract"], "hwp-preservation-report-v1");
        assert_eq!(
            value["events"].as_array().unwrap().len(),
            report.preservation.events.len(),
            "loss report는 실행 결과 ledger와 같은 이벤트 수를 가져야 한다"
        );
        assert_loss_report_schema_valid(&value);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// --loss-report가 무손실 변환에서도 유효한(빈 events) 보고서를 쓰는지 확인.
    #[test]
    fn loss_report_is_written_even_when_conversion_is_lossless() {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "hwp-convert-lossless-report-test-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        let generated = dir.join("generated.hwpx");
        let source = dir.join("source.hwpx");
        let destination = dir.join("result.hwpx");
        let report_path = dir.join("loss.json");
        hwpx::write_document(
            &hwp_convert::from_markdown("owner-authored fixture"),
            &generated,
        )
        .unwrap();
        append_synthetic_package_entry(&generated, &source);

        let report = execute(
            &source,
            &destination,
            Some(ConvertFormat::Hwpx),
            false,
            Some(&report_path),
            false,
            false,
            &MdOpts::default(),
            Vec::new(),
        )
        .unwrap();
        assert!(report.preservation.is_lossless());

        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
        assert_eq!(value["contract"], "hwp-preservation-report-v1");
        assert_eq!(value["events"], serde_json::json!([]));
        assert_loss_report_schema_valid(&value);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// --loss-report가 입력의 lexical 변형(`./in.hwpx`, `sub/../in.hwpx`)이어도
    /// 거부하고 입력을 보존한다. raw Path 비교는 `.`는 걸러도 `..` 철자는 놓친다
    /// (epic #90 PR 3 후속).
    #[test]
    fn loss_report_lexical_alias_of_input_is_rejected() {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "hwp-convert-loss-report-alias-test-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        let source = dir.join("source.hwpx");
        let destination = dir.join("result.hwpx");
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(&source, b"ORIGINAL").unwrap();

        for alias in [
            dir.join(".").join("source.hwpx"),
            dir.join("sub").join("..").join("source.hwpx"),
        ] {
            let result = execute(
                &source,
                &destination,
                Some(ConvertFormat::Hwpx),
                false,
                Some(&alias),
                false,
                false,
                &MdOpts::default(),
                Vec::new(),
            );
            let error = format!("{:#}", result.unwrap_err());
            assert!(
                error.contains("--loss-report"),
                "별칭 {} 거부 사유: {error}",
                alias.display()
            );
            assert_eq!(
                std::fs::read(&source).unwrap(),
                b"ORIGINAL",
                "입력 파일은 그대로여야 한다"
            );
            assert!(
                !destination.exists(),
                "거부된 변환은 출력을 게시하지 않는다"
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// --loss-report가 출력(-o)과 같으면 거부한다 — 리포트를 게시한 뒤 문서 게시가
    /// 조용히 덮어쓰는 순서 버그 방지.
    #[test]
    fn loss_report_same_as_output_is_rejected() {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "hwp-convert-loss-report-output-test-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        let source = dir.join("source.hwpx");
        let destination = dir.join("result.hwpx");
        hwpx::write_document(
            &hwp_convert::from_markdown("owner-authored fixture"),
            &source,
        )
        .unwrap();

        let result = execute(
            &source,
            &destination,
            Some(ConvertFormat::Hwpx),
            false,
            Some(&destination),
            false,
            false,
            &MdOpts::default(),
            Vec::new(),
        );
        let error = format!("{:#}", result.unwrap_err());
        assert!(
            error.contains("--loss-report"),
            "출력 동일 거부 사유: {error}"
        );
        assert!(
            !destination.exists(),
            "거부된 변환은 출력을 게시하지 않는다"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 생성된 loss report가 공개 스키마(`schemas/preservation-report-v1`)에
    /// 부합하는지 검증한다 — 렌더 보고서 테스트(tests/cli.rs)와 같은 게이트.
    fn assert_loss_report_schema_valid(value: &serde_json::Value) {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../schemas/preservation-report-v1.schema.json"
        ))
        .unwrap();
        let validator = jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .build(&schema)
            .unwrap();
        assert!(
            validator.is_valid(value),
            "schema rejected loss report: {value}"
        );
    }

    #[test]
    fn informational_render_events_are_not_writer_warnings() {
        let mut issues = hwp_render::RenderIssueAccumulator::new();
        issues.push(hwp_render::RenderIssueCode::FontMatched, b"font");
        let report = issues.finish();
        assert!(render_issue_messages(&report).is_empty());
    }

    #[test]
    fn native_layout_restore_limits_synthesis_to_changed_paragraphs() {
        let mut original = hwp_convert::from_markdown("first\n\nsecond");
        assert_eq!(original.sections[0].paragraphs.len(), 2);
        for (index, paragraph) in original.sections[0].paragraphs.iter_mut().enumerate() {
            paragraph.header.instance_id = index as u32 + 1;
            paragraph.line_segs = vec![hwp_model::LineSeg {
                text_start: 0,
                v_pos: index as i32 * 100,
                line_height: 1000,
                text_height: 1000,
                baseline_gap: 800,
                line_spacing: 600,
                col_start: 0,
                seg_width: 5000,
                flags: 0x60000,
            }];
        }
        let mut edited = original.clone();
        edited.sections[0].paragraphs[0]
            .chars
            .insert(0, hwp_model::HwpChar::Text('X'));
        edited.sections[0].paragraphs[0].line_segs.clear();
        let mut synthesized = edited.clone();
        for paragraph in &mut synthesized.sections[0].paragraphs {
            paragraph.line_segs = vec![hwp_model::LineSeg {
                text_start: 0,
                v_pos: 999,
                line_height: 999,
                text_height: 999,
                baseline_gap: 999,
                line_spacing: 999,
                col_start: 0,
                seg_width: 999,
                flags: 0,
            }];
        }

        restore_unchanged_native_paragraphs(&original, &edited, &mut synthesized);

        assert_eq!(
            synthesized.sections[0].paragraphs[0].line_segs[0].v_pos,
            999
        );
        assert_eq!(
            synthesized.sections[0].paragraphs[1].line_segs,
            original.sections[0].paragraphs[1].line_segs
        );
    }
}
