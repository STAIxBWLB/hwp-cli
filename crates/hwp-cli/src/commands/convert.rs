//! `hwp convert`: format conversion.
//!
//! M2 scope: hwp/hwpx to Markdown/JSON. HWPX writing (M4) and HWP writing (M6)
//! belong to later milestones.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use crate::commands::cat::{
    LoadOptions, load_document, load_document_with_options, resolve_password_args,
};
use anyhow::Context as _;
use hwp_cli::cli::{ConvertFormat, PasswordArgs};
use sha2::{Digest as _, Sha256};

/// Additional options for Markdown output only; ignored for other formats.
#[derive(Default)]
pub struct MdOpts<'a> {
    /// Image extraction directory (default: <output-stem>.media).
    pub media_dir: Option<&'a Path>,
    /// Include header/footer text.
    pub with_header_footer: bool,
    /// Include hidden descriptive text.
    pub with_hidden: bool,
}

/// Programmable conversion result; CLI and MCP share the same warning contract.
#[derive(Debug, Default)]
pub struct ConvertReport {
    pub warnings: Vec<String>,
    pub preservation: hwp_model::PreservationReport,
}

pub(crate) const MAX_PRELOADED_BATCH_BYTES: u64 = 512 * 1024 * 1024;

struct PreloadedDocument {
    document: hwp_model::Document,
    source_sha256: String,
}

fn source_sha256(path: &Path) -> anyhow::Result<String> {
    let mut source = std::fs::File::open(path)
        .with_context(|| format!("입력 해시를 위해 파일을 열 수 없습니다: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn load_preloaded_document(
    input: &Path,
    options: &LoadOptions<'_>,
) -> anyhow::Result<PreloadedDocument> {
    let before = source_sha256(input)?;
    let document = load_document_with_options(input, options).map_err(anyhow::Error::new)?;
    let after = source_sha256(input)?;
    if before != after {
        anyhow::bail!(
            "batch 사전 적재 중 입력 내용이 변경되었습니다: {}",
            input.display()
        );
    }
    Ok(PreloadedDocument {
        document,
        source_sha256: after,
    })
}

fn protected_batch_preload_reservation(input: &Path) -> anyhow::Result<Option<u64>> {
    if input
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        return Ok(None);
    }
    match crate::format::detect(input)? {
        crate::format::FileFormat::Hwp5 => {
            let container = hwp5::Hwp5Container::open(input)?;
            Ok(container
                .file_header()
                .is_encrypted()
                .then_some(hwp5::PASSWORD_PROTECTED_DOCUMENT_LIVE_LIMIT))
        }
        crate::format::FileFormat::Hwpx => {
            let mut package = hwpx::HwpxPackage::open(input)?;
            Ok(package
                .has_encryption_marker()?
                .then_some(MAX_PRELOADED_BATCH_BYTES))
        }
    }
}

fn add_preloaded_batch_reservation(total: u64, document: u64) -> anyhow::Result<u64> {
    let next = total
        .checked_add(document)
        .context("batch document memory reservation overflow")?;
    if next > MAX_PRELOADED_BATCH_BYTES {
        anyhow::bail!(
            "--out-dir 입력의 사전 적재 메모리 한도를 초과했습니다: {next} > {MAX_PRELOADED_BATCH_BYTES} bytes"
        );
    }
    Ok(next)
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
            let mut documents: Vec<Option<PreloadedDocument>> =
                (0..inputs.len()).map(|_| None).collect();
            if options.password.is_none() {
                // Missing credentials must reach the shared loader before any
                // profile-specific reservation parser so batch and single-file
                // commands return the same stable refusal.
                for input in inputs {
                    load_document_with_options(input, options).map_err(anyhow::Error::new)?;
                }
            } else {
                // Classify and reserve every protected input before loading any
                // of them. Plain documents are validated first and dropped;
                // protected documents are retained within the aggregate
                // reservation so credential preflight does not repeat crypto.
                let reservations = inputs
                    .iter()
                    .map(|input| protected_batch_preload_reservation(input))
                    .collect::<anyhow::Result<Vec<_>>>()?;
                for (input, reservation) in inputs.iter().zip(&reservations) {
                    if reservation.is_none() {
                        load_document_with_options(input, options).map_err(anyhow::Error::new)?;
                    }
                }
                let mut reserved_batch_bytes = 0u64;
                for (index, (input, reservation)) in inputs.iter().zip(reservations).enumerate() {
                    if let Some(reservation) = reservation {
                        reserved_batch_bytes =
                            add_preloaded_batch_reservation(reserved_batch_bytes, reservation)?;
                        documents[index] = Some(load_preloaded_document(input, options)?);
                    }
                }
            }
            std::fs::create_dir_all(dir)?;
            let convert_member =
                |input: &Path, document: Option<PreloadedDocument>| -> anyhow::Result<()> {
                    let stem = input
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .with_context(|| {
                            format!("입력 파일 이름을 확인할 수 없습니다: {}", input.display())
                        })?;
                    let out = dir.join(format!("{stem}.{}", target_extension(target)));
                    let report = match document {
                        Some(document) => execute_with_preloaded_document(
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
                            Some(document),
                            PreservationContext::Convert,
                        )?,
                        None => execute_with_options(
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
                        )?,
                    };
                    print_warnings(&report.warnings);
                    crate::commands::preservation::print_report(&report.preservation);
                    eprintln!("변환 완료: {} → {}", input.display(), out.display());
                    Ok(())
                };
            // Consume every retained protected document first. Plain/JSON
            // reloads cannot then overlap an input-order-later HWPX semantic
            // tree whose size is deliberately treated as the full batch cap.
            for (input, document) in inputs.iter().zip(documents.iter_mut()) {
                if document.is_some() {
                    convert_member(input, document.take())?;
                }
            }
            for (input, document) in inputs.iter().zip(documents) {
                if document.is_none() {
                    convert_member(input, None)?;
                }
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

/// The operation whose preservation policy owns a nested conversion refusal.
#[derive(Clone, Copy)]
pub(crate) enum PreservationContext {
    Convert,
    Fill,
}

impl PreservationContext {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Convert => "convert",
            Self::Fill => "fill",
        }
    }
}

/// Conversion service shared by hwp convert and MCP.
///
/// Do not publish destination until validation finishes. Markdown image sidecars share
/// the body's recovery journal. The caller handles user-visible messages.
///
/// When loss_report is given, publish the typed hwp-preservation-report-v1 ledger as JSON
/// immediately after preservation checks, before strict refusal, even without loss.
/// Automation can then read the same evidence path regardless of success or failure.
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

/// Default-option conversion for a caller whose preservation refusal belongs to another
/// operation. It shares the same ledger, validation and publication path as `execute`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_for_context(
    context: PreservationContext,
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
    execute_with_preloaded_document(
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
        None,
        context,
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
    let preloaded =
        if options.password.is_some() && protected_batch_preload_reservation(input)?.is_some() {
            Some(load_preloaded_document(input, options)?)
        } else {
            None
        };
    execute_with_preloaded_document(
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
        preloaded,
        PreservationContext::Convert,
    )
}

#[allow(clippy::too_many_arguments)]
fn execute_with_preloaded_document(
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
    preloaded: Option<PreloadedDocument>,
    preservation_context: PreservationContext,
) -> anyhow::Result<ConvertReport> {
    let target = match to {
        Some(t) => t,
        None => infer_format(output)?,
    };
    if let Some(report_path) = loss_report {
        // Combine canonicalization, resolving dot segments and symlink spellings, with
        // output.rs identity-based alias detection. Raw Path comparison misses spellings
        // such as sub/../in.hwpx; path comparison alone misses hard-link aliases.
        // Reports must not overwrite the input; matching the output would let document
        // publication silently overwrite the report. Refuse both cases.
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
    let (doc, preloaded_source_sha256) = match preloaded {
        Some(preloaded) => (preloaded.document, Some(preloaded.source_sha256)),
        None => (
            load_document_with_options(input, options).map_err(anyhow::Error::new)?,
            None,
        ),
    };
    if let Some(expected) = &preloaded_source_sha256
        && source_sha256(input)? != *expected
    {
        anyhow::bail!(
            "batch 변환 전에 입력 내용이 변경되었습니다: {}",
            input.display()
        );
    }
    if matches!(target, ConvertFormat::Md) {
        let (media_destination, media_prefix) = markdown_media_paths(output, md_opts.media_dir)?;
        let write_markdown = |staged: &Path, staged_media: &Path| {
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
        };
        let warnings = if let Some(expected) = preloaded_source_sha256.as_deref() {
            let snapshot_limit = match crate::format::detect(input)? {
                crate::format::FileFormat::Hwp5 => hwp5::SUPPORTED_CONTAINER_MAX_BYTES,
                crate::format::FileFormat::Hwpx => {
                    hwpx::NATIVE_PACKAGE_LIMITS.max_total_uncompressed_bytes
                }
            };
            let (_, warnings) =
                crate::commands::output::write_with_private_input_snapshot_and_sidecar(
                    output,
                    input,
                    snapshot_limit,
                    &media_destination,
                    |_, staged, staged_media, snapshot_sha256| {
                        if snapshot_sha256 != expected {
                            anyhow::bail!(
                                "batch 사전 적재 후 Markdown 입력 내용이 변경되었습니다: {}",
                                input.display()
                            );
                        }
                        write_markdown(staged, staged_media)
                    },
                    |_, _, _| Ok(()),
                )?;
            warnings
        } else {
            crate::commands::output::write_validated_with_sidecar(
                output,
                Some(input),
                &media_destination,
                write_markdown,
                |_, _, _| Ok(()),
            )?
        };
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
    let password_unlocked_hwp5 = source_format == crate::format::FileFormat::Hwp5
        && matches!(target, ConvertFormat::Hwp)
        && options.password.is_some()
        && hwp5::Hwp5Container::open(input)?
            .file_header()
            .is_encrypted();
    let password_unlocked_hwpx = source_format == crate::format::FileFormat::Hwpx
        && matches!(target, ConvertFormat::Hwpx)
        && options.password.is_some()
        && {
            let mut package = hwpx::HwpxPackage::open(input)?;
            package.has_encryption_marker()?
        };
    // Password-unlocked native inputs cannot be compared or rewritten against
    // their ciphertext containers. HWP5 uses plaintext synthesis; HWPX writes
    // the authenticated in-memory entries and a plaintext manifest. Ordinary
    // native inputs keep the strict source-preserving container path.
    let preserve_same_native_container =
        same_native_format && !password_unlocked_hwp5 && !password_unlocked_hwpx;
    // Decrypted HWP5 still has a readable raw CFB directory, so compare its
    // removal-only opaque surface with the plaintext synthesis. HWPX cannot
    // use the same inspector because protected opaque entries are ciphertext
    // in the source and authenticated plaintext in the output.
    let inspect_same_native_container = same_native_format && !password_unlocked_hwpx;
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
            ConvertFormat::Hwp
                if source_format == crate::format::FileFormat::Hwp5
                    && preserve_same_native_container =>
            {
                write_hwp_preserving_source(source, &doc, &doc, staged, preserve_layout, false)?
            }
            ConvertFormat::Hwp => write_hwp(&doc, staged, preserve_layout)?,
        };

        if matches!(target, ConvertFormat::Hwp | ConvertFormat::Hwpx) {
            if inspect_same_native_container {
                report.preservation.extend(
                    crate::commands::preservation::inspect_same_format_container(source, staged)?,
                );
            } else {
                // Cross-format: record typed events for preserved package/container assets
                // that the target format cannot represent.
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
        // Record before strict refusal so failed conversions leave decision evidence.
        if let Some(report_path) = loss_report {
            write_loss_report(report_path, &report.preservation)?;
        }
        if matches!(target, ConvertFormat::Hwp | ConvertFormat::Hwpx) {
            if preserve_same_native_container || strict {
                crate::commands::reject_preservation_loss(
                    preservation_context.as_str(),
                    &report.preservation,
                )?;
            }
            load_document(staged)
                .with_context(|| format!("변환 문서 재읽기 실패: {}", staged.display()))?;
        }
        Ok(())
    };
    let snapshot_bound = preloaded_source_sha256.is_some()
        || (source_format == crate::format::FileFormat::Hwp5
            && matches!(target, ConvertFormat::Hwp));
    let write_report = if snapshot_bound {
        let snapshot_limit = match source_format {
            crate::format::FileFormat::Hwp5 => hwp5::SUPPORTED_CONTAINER_MAX_BYTES,
            crate::format::FileFormat::Hwpx => {
                hwpx::NATIVE_PACKAGE_LIMITS.max_total_uncompressed_bytes
            }
        };
        let (_, report) = crate::commands::output::write_with_private_input_snapshot(
            output,
            input,
            snapshot_limit,
            crate::commands::output::SnapshotOutputMode::Publish,
            |snapshot, staged, snapshot_sha256| {
                if preloaded_source_sha256
                    .as_deref()
                    .is_some_and(|expected| expected != snapshot_sha256)
                {
                    anyhow::bail!(
                        "batch 사전 적재 후 HWP 입력 내용이 변경되었습니다: {}",
                        input.display()
                    );
                }
                write_staged(snapshot, staged)
            },
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

/// Print warnings to stderr.
pub(crate) fn print_warnings(warnings: &[String]) {
    for w in warnings {
        eprintln!("경고: {w}");
    }
}

/// Whether the volume `path` lives on treats names that differ only in case as one name.
///
/// On unix the volume is asked, because the platform does not decide it: macOS volumes are
/// case-insensitive by default but can be case-sensitive, and Linux can mount case-insensitive
/// ones (casefold ext4, vfat, CIFS). The nearest existing path component is looked up again
/// with its case flipped, and the volume is case-insensitive when that finds the same file
/// (same device and inode). Nothing is written. When no component has a cased letter to flip,
/// or the probe cannot decide, it answers by the platform default (yes on macOS, no on other
/// unix), the safe side on macOS where folding too little lets a report replace a fragment
/// unnoticed. Windows has no inode to compare and defaults to case-insensitive.
pub(crate) fn path_is_case_insensitive(path: &Path) -> bool {
    probe_case_insensitive(path).unwrap_or(cfg!(any(windows, target_os = "macos")))
}

#[cfg(unix)]
fn probe_case_insensitive(path: &Path) -> Option<bool> {
    use std::os::unix::fs::MetadataExt;
    let absolute = std::path::absolute(path).ok()?;
    for ancestor in absolute.ancestors() {
        let Some(name) = ancestor.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let flipped: String = name
            .chars()
            .map(|ch| {
                if ch.is_lowercase() {
                    ch.to_uppercase().next().unwrap_or(ch)
                } else if ch.is_uppercase() {
                    ch.to_lowercase().next().unwrap_or(ch)
                } else {
                    ch
                }
            })
            .collect();
        if flipped == name {
            continue; // no cased letter to flip in this component
        }
        let (Ok(original), Some(parent)) = (std::fs::metadata(ancestor), ancestor.parent()) else {
            continue; // does not exist yet
        };
        return Some(match std::fs::metadata(parent.join(&flipped)) {
            Ok(other) => (other.dev(), other.ino()) == (original.dev(), original.ino()),
            Err(_) => false,
        });
    }
    None
}

#[cfg(not(unix))]
fn probe_case_insensitive(_path: &Path) -> Option<bool> {
    None
}

/// A key for the alias guards: two paths with equal keys are the same file, so a set of outputs
/// holding both would write one over the other. Canonicalize existing paths. For a path that
/// does not exist yet, canonicalize its longest existing ancestor and append the rest with `.`
/// and `..` resolved lexically: a component that does not exist cannot be a symlink, so `..`
/// after it simply cancels it. Dot segments and symlink spellings resolving to the same file
/// then compare equal even when the output directory is about to be created, which is the usual
/// `split --out-dir new-dir` case (`out/../out/x` and `out/x` are the same file). On a
/// case-insensitive volume ([`path_is_case_insensitive`]) the key is also case-folded, so
/// `out/SOURCE-001.HWPX` and `out/source-001.hwpx` are one file. The key is for COMPARING only:
/// it may be lowercased and is not a path to open or write. The key is the EARLY refusal and
/// models only what it can compute: dot segments, symlinks and case. It does not model Unicode
/// normalization (APFS treats the NFC and NFD spellings of a name as one file) or anything else
/// a volume folds. Those are caught where it cannot be wrong, at publish time, by comparing the
/// identity of the files themselves (`output::publish_output_set`).
pub(crate) fn normalize_for_alias_compare(path: &Path) -> PathBuf {
    let resolved = resolve_for_alias_compare(path);
    if path_is_case_insensitive(&resolved) {
        PathBuf::from(resolved.to_string_lossy().to_lowercase())
    } else {
        resolved
    }
}

fn resolve_for_alias_compare(path: &Path) -> PathBuf {
    use std::path::Component;
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    let Ok(absolute) = std::path::absolute(path) else {
        return path.to_path_buf();
    };
    // The longest prefix that exists. `ancestors()` yields the path itself first, then each
    // shorter prefix, so the first one that canonicalizes is the longest.
    let Some((existing, canonical_existing)) = absolute
        .ancestors()
        .find_map(|ancestor| Some((ancestor, std::fs::canonicalize(ancestor).ok()?)))
    else {
        return absolute;
    };
    let mut resolved = canonical_existing;
    let rest = absolute.strip_prefix(existing).unwrap_or(Path::new(""));
    for component in rest.components() {
        match component {
            Component::Normal(name) => resolved.push(name),
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    resolved
}

/// Publish --loss-report through the same staged/validated transaction as other output,
/// following render --report's write_report discipline. Reports are content-free
/// and contain neither input nor output paths.
pub(crate) fn write_loss_report(
    path: &Path,
    report: &hwp_model::PreservationReport,
) -> anyhow::Result<()> {
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

/// When --font-dir is empty, use HWP_FONT_DIR or fonts/ as the default font directory.
/// This lets render, diff and PDF conversion load the local HCR fonts without explicit arguments;
/// otherwise missing Korean defaults cause system-font substitution and reduce fidelity.
pub fn resolve_font_dirs(given: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
    if !given.is_empty() {
        return given;
    }
    vec![std::path::PathBuf::from(
        std::env::var("HWP_FONT_DIR").unwrap_or_else(|_| "fonts".into()),
    )]
}

/// PDF render options: when given (--font-dir) is empty, resolve fonts from HWP_FONT_DIR,
/// falling back to fonts/.
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

/// Write an HWP binary with a first-page render embedded as PrvImage.
///
/// Synthesized Markdown/HWPX documents need PARA_LINE_SEG for Hangul 5.1.x to render
/// the body correctly, avoiding black bars, empty content or corruption. Shape fonts
/// to calculate line breaks and fill the IR before writing; matching Hangul requires
/// HCR Batang from HWP_FONT_DIR or the project's fonts/ directory.
pub fn write_hwp(
    doc: &hwp_model::Document,
    output: &std::path::Path,
    preserve_layout: bool,
) -> anyhow::Result<hwp_model::WriteReport> {
    write_hwp_impl(doc, output, preserve_layout, false, false, None, None)
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
        false,
        None,
        Some((source, original)),
    )
}

/// Rewrite an edited document as HWP.
///
/// Treat HWP5 edits surgically: clear caches only in edited paragraphs, as the edit
/// primitives already do, for Hangul to reflow them; retain untouched paragraph caches.
/// Clearing every cache makes Hangul reflow every paragraph, including empty table-cell
/// paragraphs at large sizes, inflating row heights and producing observed blank gaps.
/// This matches Hangul's own behavior of relaying out only changed paragraphs.
///
/// HWPX/Markdown caches differ from HWP layout, so use the synthesis path with edited=true:
/// clear line caches, restore para_shape and let Hangul recalculate.
pub fn write_hwp_edited(
    doc: &hwp_model::Document,
    output: &std::path::Path,
) -> anyhow::Result<hwp_model::WriteReport> {
    if doc.meta.source_format == "hwp5" {
        // Retain original caches without synthesis normalization; only edited paragraphs have count=0.
        write_hwp_impl(doc, output, true, false, false, None, None)
    } else {
        write_hwp_impl(doc, output, false, true, false, None, None)
    }
}

/// Write structurally edited documents with inserted/deleted paragraphs or table rows as HWP.
///
/// Force edited=true synthesis for every origin so inserted paragraphs/rows receive
/// paragraph terminators, last-paragraph bits and PARA/cell counts through the same
/// Hangul-acceptance path as convert/new. Separate this from surgical write_hwp_edited.
pub fn write_hwp_structural(
    doc: &hwp_model::Document,
    output: &std::path::Path,
) -> anyhow::Result<hwp_model::WriteReport> {
    write_hwp_impl(doc, output, false, true, false, None, None)
}

/// Writes a merge whose converted inputs had their paragraph shapes prepared before grafting.
/// Mixed inputs need structural materialization; every HWP merge needs unique paragraph IDs.
pub(crate) fn write_hwp_merged(
    doc: &hwp_model::Document,
    output: &std::path::Path,
    structural: bool,
) -> anyhow::Result<hwp_model::WriteReport> {
    write_hwp_impl(doc, output, false, structural, true, None, None)
}

/// Write a structured document as HWP using only specified font files, without system fonts.
///
/// Used by environment-independent paths such as certification corpora. font_files must
/// follow manifest order, and an empty list is refused. Keep this separate from the
/// existing ambient-font behavior of ordinary user paths.
pub fn write_hwp_structural_isolated(
    doc: &hwp_model::Document,
    output: &std::path::Path,
    font_files: &[std::path::PathBuf],
) -> anyhow::Result<hwp_model::WriteReport> {
    if font_files.is_empty() {
        anyhow::bail!("isolated HWP writer requires at least one explicit font file");
    }
    write_hwp_impl(doc, output, false, true, false, Some(font_files), None)
}

fn write_hwp_impl(
    doc: &hwp_model::Document,
    output: &std::path::Path,
    preserve_layout: bool,
    edited: bool,
    merged: bool,
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
        // The paragraphs before the first edit keep their saved caches, so the re-synthesized
        // suffix has to continue from where that prefix ends: a bare section-top cursor gave the
        // first changed paragraph a v_pos *above* its kept predecessor, which the renderer reads
        // as a page start and draws the change mid-page (#462).
        let seeds = source.map_or_else(Vec::new, |(_, original)| body_flow_seeds(original, doc));
        // Put the caches the output will keep in place first, and drop the saved ones behind the
        // seam: a cache that is present is then one the writer keeps, so the walk measures it
        // instead of its own ephemeral synthesis of it. Otherwise a kept table whose saved cell
        // caches are taller grows again after the paragraphs below it were already placed, and a
        // retained sibling behind a moved paragraph is measured where it no longer sits (#509).
        if let Some((_, original)) = source {
            restore_unchanged_native_paragraphs(original, doc, &mut d, SuffixCache::Drop);
        }
        hwp_render::lineseg::synthesize_linesegs_seeded(
            &mut d,
            &mut store,
            &mut warns,
            &hwp_render::lineseg::BodyFlow {
                seeds: &seeds,
                preserve_nested: source.is_some(),
            },
        );
        // The walk re-synthesized every body cache; hand the kept prefix its saved records back and
        // keep what it synthesized behind the seam.
        if let Some((_, original)) = source {
            restore_unchanged_native_paragraphs(original, doc, &mut d, SuffixCache::Keep);
        }
        warns.absorb(store.issues.finish());
        report
            .warnings
            .append(&mut render_issue_messages(&warns.finish()));
        owned = d;
        &owned
    } else if source.is_some() || !synthesize || preserve_layout {
        // Unchanged HWP5/preserve-layout: keep original line caches.
        doc
    } else if has_source_linesegs {
        // HWPX or edited HWP5 caches may be stale after edits or reflect Hangul's HWPX export
        // layout, which can differ from its HWP save layout (for example, the same document
        // has six HWPX pages and five HWP pages). Removing caches lets Hangul recalculate
        // using paragraph/character shapes and flow like its HWP save. Matching genuine
        // paragraph shapes is essential. Edited documents follow this same established
        // conversion path: clear stale caches and let Hangul reflow them on opening.
        let mut d = doc.clone();
        clear_linesegs(&mut d);
        owned = d;
        &owned
    } else {
        // Sources without line caches, such as Markdown: synthesize through font shaping.
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
        para_shape_defaults_prepared: merged,
        deduplicate_instance_ids: merged,
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

/// What the restore leaves in a paragraph at or after the edit's seam (#509).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SuffixCache {
    /// Before the walk runs. A saved line cache behind the seam is measured against a flow the edit
    /// moved (a taller neighbour, a shifted index), so it is dropped and the walk re-synthesizes it.
    Drop,
    /// After the walk. The cache the renderer synthesized is the flow the output keeps.
    Keep,
}

/// Put the saved records the output keeps back into the document, before and after the renderer's
/// synthesis. This limits new PARA_LINE_SEG records to what the edit moved, including paragraphs
/// nested in table cells and generic controls.
fn restore_unchanged_native_paragraphs(
    original: &hwp_model::Document,
    edited: &hwp_model::Document,
    synthesized: &mut hwp_model::Document,
    suffix: SuffixCache,
) {
    for (section_index, synthesized_section) in synthesized.sections.iter_mut().enumerate() {
        let (Some(original_section), Some(edited_section)) = (
            original.sections.get(section_index),
            edited.sections.get(section_index),
        ) else {
            continue;
        };
        restore_unchanged_body_paragraphs(
            &original_section.paragraphs,
            &edited_section.paragraphs,
            &mut synthesized_section.paragraphs,
            suffix,
        );
    }
}

/// Restore one section's body list. Paragraphs before the edit keep their saved records; from the
/// first moved body paragraph on, the body cache belongs to the re-synthesized flow, because a
/// saved `v_pos` there measures the flow the edit invalidated: kept after a taller table it sits
/// above the table's drawn bottom and the renderer reads the step back as a new page (#462). The
/// nested lists this pass recurses into (table cells and generic paragraph lists, captions included
/// only where a whole unchanged paragraph brings them) apply the same rule at their own seam.
fn restore_unchanged_body_paragraphs(
    original: &[hwp_model::Paragraph],
    edited: &[hwp_model::Paragraph],
    synthesized: &mut [hwp_model::Paragraph],
    suffix: SuffixCache,
) {
    if edited.len() != synthesized.len() {
        return;
    }
    let matches = match_unchanged_paragraphs(original, edited);
    let first_moved = first_moved_paragraph(original, edited, &matches);
    for (edited_index, matched) in matches.iter().enumerate() {
        let Some(original_index) = *matched else {
            continue;
        };
        let paragraph = &original[original_index];
        let edited_paragraph = &edited[edited_index];
        if edited_index < first_moved {
            // Cache included: `first_moved` already said this paragraph's own lines are the ones the
            // saved record holds.
            synthesized[edited_index] = paragraph.clone();
        } else {
            restore_after_the_seam(
                paragraph,
                edited_paragraph,
                &mut synthesized[edited_index],
                suffix,
            );
        }
    }
}

/// First paragraph whose saved position the edit moved. A list is positional and cumulative: a
/// paragraph is still where it was only when it matches the original at its own index, because an
/// insertion or deletion before it shifts every later saved `v_pos` by a block the list no longer
/// has. `edited.len()` means the edit did not touch this list's flow.
fn first_moved_paragraph(
    original: &[hwp_model::Paragraph],
    edited: &[hwp_model::Paragraph],
    matches: &[Option<usize>],
) -> usize {
    matches
        .iter()
        .enumerate()
        .find(|(index, matched)| match matched {
            Some(original_index) => {
                *original_index != *index
                    || !unchanged_content(&original[*original_index], &edited[*index])
            }
            None => true,
        })
        .map_or(edited.len(), |(index, _)| index)
}

/// Whether the edit left a paragraph at its own place with its own content: every field that
/// decides its lines, compared through the two the editor rewrites rather than edits.
///
/// The line cache is the answer this pass computes, so it cannot be part of the question; and
/// PARA_TEXT's "last paragraph in the list" bit follows the list, not the paragraph, so inserting
/// or deleting a sibling flips it on a paragraph nobody touched (#509).
fn unchanged_content(original: &hwp_model::Paragraph, edited: &hwp_model::Paragraph) -> bool {
    fn strip(paragraph: &hwp_model::Paragraph) -> hwp_model::Paragraph {
        let mut stripped = paragraph.clone();
        stripped.line_segs.clear();
        stripped.header.chars_flags &= !0x80;
        stripped
    }
    strip(original) == strip(edited)
}

/// Original index for every edited paragraph that still names a paragraph of the immutable
/// snapshot: a unique `instance_id` first, then full equality, then the same index. Restoring and
/// seeding share this matching so both agree on where the edit starts.
fn match_unchanged_paragraphs(
    original: &[hwp_model::Paragraph],
    edited: &[hwp_model::Paragraph],
) -> Vec<Option<usize>> {
    let mut matches = vec![None; edited.len()];
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
            matches[edited_index] = Some(original_index);
        }
    }
    matches
}

/// Where a re-synthesized body flow has to start in an edited section: the edit's first body
/// paragraph, seeded with the bottom edge of the kept prefix. Without it the synthesizer restarts
/// the section at `v_pos` 0, which puts the first edited paragraph above the genuine cache the
/// paragraph before it still keeps, and the renderer reads that step back as a page start (#462).
fn body_flow_seeds(
    original: &hwp_model::Document,
    edited: &hwp_model::Document,
) -> Vec<hwp_render::lineseg::BodySeed> {
    let mut seeds = Vec::new();
    for (section_index, section) in original.sections.iter().enumerate() {
        let Some(edited_section) = edited.sections.get(section_index) else {
            continue;
        };
        let matches = match_unchanged_paragraphs(&section.paragraphs, &edited_section.paragraphs);
        let first_changed =
            first_moved_paragraph(&section.paragraphs, &edited_section.paragraphs, &matches);
        // An edit in the first paragraph restarts at the section top anyway, and a section the
        // edit did not reach keeps its saved flow.
        if first_changed == 0 || first_changed >= edited_section.paragraphs.len() {
            continue;
        }
        let Some(previous_index) = matches[first_changed - 1] else {
            continue;
        };
        let previous = &section.paragraphs[previous_index];
        let space_after = original
            .header
            .para_shapes
            .get(previous.para_shape.0 as usize)
            .map_or(0, |shape| shape.spacing_bottom);
        // The synthesizer's own vertical rule measures the prefix, and a genuinely cache-less
        // prefix has no bottom to continue from.
        let Some(v_pos_hu) = hwp_render::lineseg::saved_body_cursor(previous, space_after) else {
            continue;
        };
        seeds.push(hwp_render::lineseg::BodySeed {
            section: section_index,
            paragraph: first_changed,
            v_pos_hu,
        });
    }
    seeds
}

/// Restore one nested paragraph list. A list is cumulative: every paragraph's saved `v_pos` is
/// measured from the block before it, so only the part of the list before its first moved paragraph
/// is still valid. From that paragraph on the list follows the same rule as the body list at its
/// own seam, its unchanged neighbours included, because their saved positions were measured against
/// a block the edit moved (#509).
fn restore_unchanged_paragraph_list(
    original: &[hwp_model::Paragraph],
    edited: &[hwp_model::Paragraph],
    synthesized: &mut [hwp_model::Paragraph],
    suffix: SuffixCache,
    parent_layout_matches: bool,
) {
    if edited.len() != synthesized.len() {
        return;
    }
    let matches = match_unchanged_paragraphs(original, edited);
    // Equal paragraph content is insufficient when its box changed: editors intentionally
    // clear those caches, and normalizing line_segs must not undo that invalidation.
    let first_moved = if parent_layout_matches {
        first_moved_paragraph(original, edited, &matches)
    } else {
        0
    };
    for (edited_index, original_index) in matches.into_iter().enumerate() {
        let Some(original_index) = original_index else {
            // An unmatched paragraph belongs to the recomputed suffix too; an imported cache
            // must not make it look like a retained prefix to the nested synthesizer.
            if suffix == SuffixCache::Drop {
                synthesized[edited_index].line_segs.clear();
            }
            continue;
        };
        if edited_index < first_moved {
            // Cache included: `first_moved` already said this paragraph's own lines are the ones the
            // saved record holds.
            synthesized[edited_index] = original[original_index].clone();
        } else {
            restore_after_the_seam(
                &original[original_index],
                &edited[edited_index],
                &mut synthesized[edited_index],
                suffix,
            );
        }
    }
}

/// Require matching local geometry and raw list metadata before reusing native caches.
/// Position and border styling are not inputs to the existing cell-local line-box calculation.
/// Compare raw values rather than declaring different clamped boxes equivalent.
fn cell_layout_matches(original: &hwp_model::Cell, edited: &hwp_model::Cell) -> bool {
    original.width == edited.width
        && original.margins == edited.margins
        && original.list_attr == edited.list_attr
        && original.header_tail == edited.header_tail
}

/// Handle a paragraph at or after the edit's seam: its own line cache is not the saved one any
/// more. Before the walk the saved cache is dropped so it cannot be measured; after the walk the
/// synthesized cache is the answer and stays. Its unchanged controls are still restored below.
fn restore_after_the_seam(
    original: &hwp_model::Paragraph,
    edited: &hwp_model::Paragraph,
    synthesized: &mut hwp_model::Paragraph,
    suffix: SuffixCache,
) {
    match suffix {
        SuffixCache::Drop => {
            synthesized.line_segs.clear();
            restore_unchanged_controls(original, edited, synthesized, suffix);
        }
        SuffixCache::Keep => {}
    }
}

/// Restore everything except the paragraph's own body cache: unchanged controls keep their saved
/// records, and the paragraphs before their own seam inside table cells and generic paragraph lists
/// are restored recursively (applying `suffix` at each list's seam). A caption inside a control the
/// edit changed is not restored here.
fn restore_unchanged_controls(
    original: &hwp_model::Paragraph,
    edited: &hwp_model::Paragraph,
    synthesized: &mut hwp_model::Paragraph,
    suffix: SuffixCache,
) {
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
                        suffix,
                        cell_layout_matches(original_cell, edited_cell),
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
                        suffix,
                        original_generic.gso_shapes.first().map(|shape| shape.w)
                            == edited_generic.gso_shapes.first().map(|shape| shape.w)
                            && original_list.header_data == edited_list.header_data,
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

/// Clear caches in every paragraph, including nested cells and headers, so Hangul
/// recalculates from paragraph/character shapes instead of differing HWPX export caches.
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
    // ───── normalize_for_alias_compare: paths whose directories do not exist yet (#417 F13) ─────

    fn alias_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hwp-alias-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn 아직_없는_디렉터리_밑의_점점_경로도_같은_파일로_정규화된다() {
        let root = alias_dir("missing-dirs");
        let plain = root.join("out").join("a.hwpx");
        for spelling in [
            root.join("out").join("..").join("out").join("a.hwpx"),
            root.join("out").join(".").join("a.hwpx"),
            root.join("out").join("sub").join("..").join("a.hwpx"),
            root.join("x").join("..").join("out").join("a.hwpx"),
        ] {
            assert!(!root.join("out").exists());
            assert_eq!(
                normalize_for_alias_compare(&spelling),
                normalize_for_alias_compare(&plain),
                "{}",
                spelling.display()
            );
        }
        // The key is a comparison value, not a path to open: on a volume that folds case it is
        // lowercased, and elsewhere it is the resolved path itself.
        let key = normalize_for_alias_compare(&plain);
        if path_is_case_insensitive(&root) {
            assert_eq!(key, PathBuf::from(plain.to_string_lossy().to_lowercase()));
        } else {
            assert_eq!(key, plain);
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn 서로_다른_파일은_정규화_뒤에도_다르다() {
        let root = alias_dir("different");
        assert_ne!(
            normalize_for_alias_compare(&root.join("out").join("a.hwpx")),
            normalize_for_alias_compare(&root.join("out").join("b.hwpx"))
        );
        assert_ne!(
            normalize_for_alias_compare(&root.join("out").join("a.hwpx")),
            normalize_for_alias_compare(&root.join("out2").join("a.hwpx"))
        );
        // `..` past a real directory leaves it.
        assert_ne!(
            normalize_for_alias_compare(&root.join("out").join("..").join("a.hwpx")),
            normalize_for_alias_compare(&root.join("out").join("a.hwpx"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// Whether `dir` really folds case, decided by the filesystem itself and not by the code
    /// under test: a file is created and looked up under another case.
    fn dir_really_folds_case(dir: &Path) -> bool {
        std::fs::write(dir.join("CaseProbeFile"), b"x").unwrap();
        dir.join("caseprobefile").exists()
    }

    #[test]
    fn 대소문자만_다른_경로는_대소문자를_가리지_않는_볼륨에서만_같은_파일이다() {
        let root = alias_dir("case-fold");
        let really = dir_really_folds_case(&root);
        // The volume decides, not the platform: macOS can mount a case-sensitive volume and
        // Linux a case-insensitive one. A temp dir whose own name has no cased letter to flip
        // falls back to the platform default, which is also what this asserts.
        assert_eq!(
            path_is_case_insensitive(&root),
            really,
            "the probe agrees with the volume"
        );
        let lower = root.join("out").join("source-001.hwpx");
        let upper = root.join("OUT").join("SOURCE-001.HWPX");
        // Neither directory exists, which is the usual new --out-dir.
        let same = normalize_for_alias_compare(&lower) == normalize_for_alias_compare(&upper);
        assert_eq!(same, path_is_case_insensitive(&root));
        if really {
            assert!(
                same,
                "a folding volume must treat the two spellings as one file"
            );
        }
        // Different names never collapse, whatever the volume does with case.
        assert_ne!(
            normalize_for_alias_compare(&root.join("out").join("source-001.hwpx")),
            normalize_for_alias_compare(&root.join("out").join("source-002.hwpx"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn 이미_있는_파일도_대소문자만_다른_철자가_같은_키를_낸다() {
        let root = alias_dir("case-fold-existing");
        let really = dir_really_folds_case(&root);
        let spelled = |name: &str| normalize_for_alias_compare(&root.join(name));
        // The file exists, so a folding volume canonicalizes both spellings to its real name.
        // The key must say "same file" exactly when the filesystem does.
        assert_eq!(spelled("CaseProbeFile") == spelled("caseprobefile"), really);
        assert!(spelled("CaseProbeFile") != spelled("CaseProbeFile2"));
        let _ = std::fs::remove_dir_all(root);
    }

    /// Two DIFFERENT files whose names differ only in case can exist only on a case-sensitive
    /// volume. The probe must tell that apart from a folding volume, where the flipped name
    /// is the same file: finding "a file under the flipped name" is not enough. On a folding
    /// volume the second file cannot be created, so there is nothing to check there.
    #[cfg(unix)]
    #[test]
    fn 이름만_대소문자가_다른_서로_다른_두_파일이_있으면_접지_않는_볼륨이다() {
        let root = alias_dir("case-two-files");
        // The probe looks the name up with every letter's case inverted, so the second file
        // must be that exact twin ("Probe" and "pROBE"), or the probe never meets it.
        let upper = root.join("Probe");
        let lower = root.join("pROBE");
        std::fs::write(&upper, b"upper").unwrap();
        let second = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lower);
        if let Ok(mut file) = second {
            use std::io::Write as _;
            file.write_all(b"lower").unwrap();
            // Two files, so the volume is case-sensitive, and the probe must say so.
            assert!(!path_is_case_insensitive(&upper));
            assert_ne!(
                normalize_for_alias_compare(&upper),
                normalize_for_alias_compare(&lower),
                "two distinct files must keep distinct keys"
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn 존재하는_심볼릭_링크_조상은_실제_위치로_풀린다() {
        let root = alias_dir("symlink");
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        assert_eq!(
            normalize_for_alias_compare(&root.join("link").join("new").join("a.hwpx")),
            normalize_for_alias_compare(&root.join("real").join("new").join("a.hwpx"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    use super::*;
    use std::io::Write as _;
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
        // Previously, entries outside the writer's fixed list were silently dropped and
        // triggered fail-closed refusal. Same-format rewriting now preserves extra entry
        // bytes, so conversion must succeed and retain the entry (epic #90).
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
            writer.start_file(entry.name().unwrap(), options).unwrap();
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

    /// Cross-format HWPX-to-HWP conversion loses extra package entries such as DocOptions,
    /// which have no matching HWP container slot. Strict mode must fail closed, refuse
    /// publication and retain destination; --loss-report must still leave its typed
    /// ledger before refusal (epic #90 PR 3).
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

    /// Non-strict cross-format conversion publishes and records loss in the ledger.
    /// --loss-report emits the same events as the execution result in schema-valid JSON.
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

    /// Check --loss-report emits a valid report with empty events for lossless conversion.
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

    /// Refuse --loss-report aliases of the input, including ./in.hwpx and sub/../in.hwpx,
    /// and preserve the input. Raw Path comparison catches dot but misses parent segments
    /// (epic #90 PR 3 follow-up).
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

    /// Refuse --loss-report matching output (-o), preventing document publication from
    /// silently overwriting a report published earlier in the transaction.
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

    /// Validate generated loss reports against schemas/preservation-report-v1,
    /// using the same gate as render-report tests in tests/cli.rs.
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
    fn preloaded_batch_memory_reservation_enforces_its_exact_boundary() {
        assert_eq!(
            add_preloaded_batch_reservation(MAX_PRELOADED_BATCH_BYTES - 1, 1).unwrap(),
            MAX_PRELOADED_BATCH_BYTES
        );
        assert!(add_preloaded_batch_reservation(MAX_PRELOADED_BATCH_BYTES, 1).is_err());
    }

    fn cached_segment(v_pos: i32) -> hwp_model::LineSeg {
        hwp_model::LineSeg {
            text_start: 0,
            v_pos,
            line_height: 1000,
            text_height: 1000,
            baseline_gap: 800,
            line_spacing: 600,
            col_start: 0,
            seg_width: 5000,
            flags: 0x60000,
        }
    }

    fn native_suffix_line_records(file: &std::path::Path) -> Vec<Vec<Vec<u8>>> {
        use hwp5::record::{RecordNode, ScanMode, scan_stream, tag};
        fn collect(node: &RecordNode, lines: &mut Vec<Vec<u8>>) {
            if node.tag == tag::PARA_LINE_SEG {
                lines.push(node.data.clone());
            }
            for child in &node.children {
                collect(child, lines);
            }
        }
        let mut container = hwp5::container::Hwp5Container::open(file).unwrap();
        let stream = container.read_record_stream("/BodyText/Section0").unwrap();
        scan_stream(&stream, ScanMode::Strict)
            .unwrap()
            .roots
            .iter()
            .filter(|node| node.tag == tag::PARA_HEADER)
            .map(|node| {
                let mut lines = Vec::new();
                collect(node, &mut lines);
                lines
            })
            .collect()
    }

    /// #540 G3: a height-neutral body edit must not replace later stored line records.
    /// The suffix includes a floating table with a deliberately taller saved cell cache.
    #[test]
    fn native_height_neutral_body_suffix_records_stay_verbatim() {
        use hwp_model::{Control, GsoPlacement};
        let dir = std::env::temp_dir().join(format!("ahwp-g3-suffix-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.hwp");
        let output = dir.join("edited.hwp");
        let mut document = hwp_convert::from_markdown(
            "Prefix\n\nEdit here\n\n| A | B |\n|---|---|\n| C | D |\n\nTail one\n\nTail two\n",
        );
        assert_eq!(
            document.sections[0].paragraphs.len(),
            5,
            "five body paragraphs"
        );
        for shape in &mut document.header.char_shapes {
            shape.base_size = 1000;
        }
        for shape in &mut document.header.para_shapes {
            shape.spacing_top = 0;
            shape.spacing_bottom = 600;
            shape.line_spacing_type = 0;
            shape.line_spacing = 160;
            shape.line_spacing_old = 160;
        }
        for (index, (para, position)) in document.sections[0]
            .paragraphs
            .iter_mut()
            .zip([0, 1900, 3800, 14000, 15900])
            .enumerate()
        {
            para.header.instance_id = index as u32 + 1;
            para.line_segs = vec![cached_segment(position)];
        }
        let table = document.sections[0].paragraphs[2]
            .controls
            .iter_mut()
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .unwrap();
        table.placement = Some(GsoPlacement {
            treat_as_char: false,
            vert_rel_to: 2,
            horz_rel_to: 3,
            ..Default::default()
        });
        for cell in &mut table.cells {
            for para in &mut cell.paragraphs {
                para.line_segs = vec![cached_segment(0)];
            }
        }
        table.cells[0].paragraphs[0].line_segs[0].line_height = 5000;
        document.meta.source_format = "hwp5".to_owned();
        hwp5::write_document(
            &document,
            &source,
            &hwp5::WriteOptions {
                preserve_linesegs: true,
                ..Default::default()
            },
        )
        .unwrap();
        let original = hwp5::read_document(&source).unwrap().document;
        assert!(
            !original.sections[0].paragraphs[1].line_segs.is_empty(),
            "fixture must store the edited paragraph cache"
        );
        let mut edited = original.clone();
        edited.sections[0].paragraphs[1]
            .chars
            .insert(0, hwp_model::HwpChar::Text('X'));
        edited.sections[0].paragraphs[1].line_segs.clear();
        write_hwp_impl(
            &edited,
            &output,
            true,
            false,
            false,
            Some(&[]),
            Some((&source, &original)),
        )
        .unwrap();
        let result = hwp5::read_document(&output).unwrap().document;
        let before = &original.sections[0].paragraphs[1].line_segs;
        let changed = &result.sections[0].paragraphs[1].line_segs;
        let extent = |lines: &[hwp_model::LineSeg]| {
            let first = lines.first().unwrap();
            let last = lines.last().unwrap();
            i64::from(last.v_pos) + i64::from(last.line_height) + i64::from(last.line_spacing)
                - i64::from(first.v_pos)
        };
        assert_eq!(extent(before), 1600, "source extent premise");
        assert_eq!(extent(changed), 1600, "height-neutral edit premise");
        assert_eq!(before.len(), changed.len(), "line-count premise");
        assert_eq!(
            before.iter().map(|s| s.flags & 3).collect::<Vec<_>>(),
            changed.iter().map(|s| s.flags & 3).collect::<Vec<_>>(),
            "page/column flags premise"
        );
        let stored = native_suffix_line_records(&source);
        let written = native_suffix_line_records(&output);
        assert_eq!(stored.len(), 5);
        assert_eq!(written.len(), 5);
        assert_eq!(written[0], stored[0], "kept prefix line records");
        assert_ne!(
            written[1], stored[1],
            "changed paragraph has its own synthesized cache"
        );
        let source_table = original.sections[0].paragraphs[2]
            .controls
            .iter()
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .unwrap();
        let result_table = result.sections[0].paragraphs[2]
            .controls
            .iter()
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .unwrap();
        for (old, new) in source_table.cells.iter().zip(&result_table.cells) {
            assert_eq!(
                new.paragraphs, old.paragraphs,
                "unchanged saved cell caches must not regrow"
            );
        }
        for index in 2..stored.len() {
            assert_eq!(
                written[index], stored[index],
                "height-neutral edit must keep suffix body/cell PARA_LINE_SEG bytes at paragraph {index}"
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Snapshot with three saved caches, an edit in the middle paragraph, and a fully synthesized
    /// copy whose every cache is the placeholder 999.
    fn restore_fixture() -> (
        hwp_model::Document,
        hwp_model::Document,
        hwp_model::Document,
    ) {
        let mut original = hwp_convert::from_markdown("first\n\nsecond\n\nthird");
        assert_eq!(original.sections[0].paragraphs.len(), 3);
        for (index, paragraph) in original.sections[0].paragraphs.iter_mut().enumerate() {
            paragraph.header.instance_id = index as u32 + 1;
            paragraph.line_segs = vec![cached_segment(index as i32 * 1600)];
        }
        let mut edited = original.clone();
        edited.sections[0].paragraphs[1]
            .chars
            .insert(0, hwp_model::HwpChar::Text('X'));
        edited.sections[0].paragraphs[1].line_segs.clear();
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
        (original, edited, synthesized)
    }

    #[test]
    fn native_layout_restore_keeps_the_prefix_and_resynthesizes_the_suffix() {
        let (original, edited, mut synthesized) = restore_fixture();

        restore_unchanged_native_paragraphs(
            &original,
            &edited,
            &mut synthesized,
            SuffixCache::Keep,
        );

        assert_eq!(
            synthesized.sections[0].paragraphs[0], original.sections[0].paragraphs[0],
            "the paragraph before the edit comes back as its saved record"
        );
        assert_eq!(
            synthesized.sections[0].paragraphs[1].line_segs[0].v_pos, 999,
            "the edited paragraph keeps the cache the renderer synthesized"
        );
        assert_eq!(
            synthesized.sections[0].paragraphs[2].line_segs[0].v_pos, 999,
            "a paragraph after the edit moved with the flow, so its saved v_pos is stale (#462)"
        );
        assert_ne!(
            synthesized.sections[0].paragraphs[2], edited.sections[0].paragraphs[2],
            "the body cache stays synthesized while the rest of the paragraph is restored"
        );
    }

    #[test]
    fn body_flow_seed_continues_at_the_kept_prefix_bottom() {
        let (mut original, edited, _) = restore_fixture();
        let shape = original.sections[0].paragraphs[0].para_shape.0 as usize;
        original.header.para_shapes[shape].spacing_bottom = 400;

        let seeds = body_flow_seeds(&original, &edited);

        assert_eq!(seeds.len(), 1, "one section, one seam");
        assert_eq!(seeds[0].section, 0);
        assert_eq!(seeds[0].paragraph, 1, "the first changed body paragraph");
        // Saved prefix cache: v_pos 0, line_height 1000, line_spacing 600, then space-after 400.
        assert_eq!(seeds[0].v_pos_hu, 2000);
    }

    #[test]
    fn body_flow_seed_skips_a_cache_less_prefix_and_a_first_paragraph_edit() {
        let (original, _, _) = restore_fixture();

        let mut edited = original.clone();
        edited.sections[0].paragraphs[2]
            .chars
            .insert(0, hwp_model::HwpChar::Text('X'));
        assert_eq!(
            body_flow_seeds(&original, &edited)[0].paragraph,
            2,
            "a cached prefix seeds the third paragraph"
        );

        let mut cache_less = original.clone();
        cache_less.sections[0].paragraphs[1].line_segs.clear();
        let mut cache_less_edited = cache_less.clone();
        cache_less_edited.sections[0].paragraphs[2]
            .chars
            .insert(0, hwp_model::HwpChar::Text('X'));
        assert!(
            body_flow_seeds(&cache_less, &cache_less_edited).is_empty(),
            "without a saved bottom to measure, the edit gets no guessed position"
        );

        let mut at_top = original.clone();
        at_top.sections[0].paragraphs[0]
            .chars
            .insert(0, hwp_model::HwpChar::Text('X'));
        assert!(
            body_flow_seeds(&original, &at_top).is_empty(),
            "a section that starts with the edit needs no seed"
        );
    }

    fn context_table() -> hwp_model::Table {
        hwp_convert::from_markdown("| A | B |\n|---|---|\n| C | D |")
            .sections
            .into_iter()
            .flat_map(|section| section.paragraphs)
            .flat_map(|paragraph| paragraph.controls)
            .find_map(|control| match control {
                hwp_model::Control::Table(table) => Some(table),
                _ => None,
            })
            .expect("synthetic Markdown creates a table")
    }

    fn context_parent(control: hwp_model::Control) -> hwp_model::Paragraph {
        let mut parent = hwp_model::Paragraph::default();
        parent.controls.push(control);
        parent
    }

    fn context_cell(parent: &mut hwp_model::Paragraph) -> &mut hwp_model::Cell {
        let hwp_model::Control::Table(table) = &mut parent.controls[0] else {
            panic!("table fixture")
        };
        &mut table.cells[0]
    }

    fn cell_context_fixture() -> hwp_model::Paragraph {
        let mut parent = context_parent(hwp_model::Control::Table(context_table()));
        let paragraph = &mut context_cell(&mut parent).paragraphs[0];
        paragraph.header.instance_id = 1;
        paragraph.line_segs = vec![cached_segment(100)];
        parent
    }

    fn assert_changed_cell_context(case: usize) {
        // Change only parent metadata, with identical paragraph text and styles.
        let original = cell_context_fixture();
        let mut edited = original.clone();
        let cell = context_cell(&mut edited);
        match case {
            0..=3 => cell.margins[case] += 1,
            4 => cell.list_attr ^= 1,
            5 => cell.header_tail.push(1),
            _ => unreachable!(),
        }
        let mut synthesized = edited.clone();
        restore_unchanged_controls(&original, &edited, &mut synthesized, SuffixCache::Drop);
        assert!(
            context_cell(&mut synthesized).paragraphs[0]
                .line_segs
                .is_empty(),
            "changed cell context {case} must invalidate its own saved lines"
        );
        context_cell(&mut synthesized).paragraphs[0].line_segs = vec![cached_segment(999)];
        restore_unchanged_controls(&original, &edited, &mut synthesized, SuffixCache::Keep);
        assert_eq!(
            context_cell(&mut synthesized).paragraphs[0].line_segs[0].v_pos,
            999,
            "changed cell context {case} must retain newly synthesized lines"
        );
    }

    #[test]
    fn changed_cell_left_margin_invalidates_saved_lines() {
        assert_changed_cell_context(0);
    }

    #[test]
    fn changed_cell_right_margin_invalidates_saved_lines() {
        assert_changed_cell_context(1);
    }

    #[test]
    fn changed_cell_top_margin_invalidates_saved_lines() {
        assert_changed_cell_context(2);
    }

    #[test]
    fn changed_cell_bottom_margin_invalidates_saved_lines() {
        assert_changed_cell_context(3);
    }

    #[test]
    fn changed_cell_list_attr_invalidates_saved_lines() {
        assert_changed_cell_context(4);
    }

    #[test]
    fn changed_cell_header_tail_invalidates_saved_lines() {
        assert_changed_cell_context(5);
    }

    #[test]
    fn cell_position_and_border_changes_keep_cell_local_lines() {
        for case in 0..3 {
            let original = cell_context_fixture();
            let mut edited = original.clone();
            let cell = context_cell(&mut edited);
            match case {
                0 => cell.row += 1,
                1 => cell.col += 1,
                2 => cell.border_fill = hwp_model::BorderFillId(cell.border_fill.0 + 1),
                _ => unreachable!(),
            }
            let mut synthesized = edited.clone();
            context_cell(&mut synthesized).paragraphs[0]
                .line_segs
                .clear();
            restore_unchanged_controls(&original, &edited, &mut synthesized, SuffixCache::Drop);
            assert_eq!(
                context_cell(&mut synthesized).paragraphs[0].line_segs,
                context_cell(&mut original.clone()).paragraphs[0].line_segs,
                "cell-local cache does not depend on position/border case {case}"
            );
        }
    }

    #[test]
    fn changed_outer_cell_context_preserves_an_unchanged_nested_tall_cell() {
        let mut original = cell_context_fixture();
        let mut nested = context_table();
        nested.cells[0].paragraphs[0].line_segs = vec![cached_segment(12000)];
        context_cell(&mut original).paragraphs[0]
            .controls
            .push(hwp_model::Control::Table(nested));
        let mut edited = original.clone();
        context_cell(&mut edited).width.0 += 1;
        let mut synthesized = edited.clone();
        context_cell(&mut context_cell(&mut synthesized).paragraphs[0]).paragraphs[0].line_segs =
            vec![cached_segment(777)];
        restore_unchanged_controls(&original, &edited, &mut synthesized, SuffixCache::Drop);
        assert_eq!(
            context_cell(&mut context_cell(&mut synthesized).paragraphs[0]).paragraphs[0].line_segs
                [0]
            .v_pos,
            12000,
            "an outer width change must not recompute a valid nested cell"
        );
        assert!(
            context_cell(&mut synthesized).paragraphs[0]
                .line_segs
                .is_empty()
        );
    }

    #[test]
    fn changed_parent_invalidates_a_new_cached_leading_paragraph() {
        let original = cell_context_fixture();
        let mut edited = original.clone();
        let cell = context_cell(&mut edited);
        cell.width.0 += 1;
        let mut inserted = hwp_model::Paragraph::default();
        inserted.header.instance_id = 2;
        inserted.chars = vec![hwp_model::HwpChar::Text('Z')];
        inserted.line_segs = vec![cached_segment(500)];
        cell.paragraphs.insert(0, inserted);
        let mut synthesized = edited.clone();

        restore_unchanged_controls(&original, &edited, &mut synthesized, SuffixCache::Drop);

        assert!(
            context_cell(&mut synthesized)
                .paragraphs
                .iter()
                .all(|paragraph| paragraph.line_segs.is_empty()),
            "a changed parent invalidates every own cache, including an unmatched leading paragraph"
        );
    }

    fn context_generic(parent: &mut hwp_model::Paragraph) -> &mut hwp_model::GenericControl {
        let hwp_model::Control::Generic(generic) = &mut parent.controls[0] else {
            panic!("generic fixture")
        };
        generic
    }

    fn assert_changed_generic_context(change_width: bool) {
        let mut paragraph = hwp_model::Paragraph::default();
        paragraph.header.instance_id = 1;
        paragraph.line_segs = vec![cached_segment(100)];
        let shape = serde_json::from_value(serde_json::json!({
            "kind": "Rect", "x": 0, "y": 0, "w": 10000, "h": 10000,
            "fill": 0, "border_color": 0, "border_width": 0
        }))
        .unwrap();
        let original = context_parent(hwp_model::Control::Generic(hwp_model::GenericControl {
            ctrl_id: *b"gso ",
            data: Vec::new(),
            paragraph_lists: vec![hwp_model::ParagraphList {
                header_data: vec![0; 8],
                paragraphs: vec![paragraph],
            }],
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: vec![shape],
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        }));
        let mut edited = original.clone();
        let generic = context_generic(&mut edited);
        if change_width {
            generic.gso_shapes[0].w += 1;
        } else {
            generic.paragraph_lists[0].header_data[0] = 1;
        }
        let mut synthesized = edited.clone();
        restore_unchanged_controls(&original, &edited, &mut synthesized, SuffixCache::Drop);
        assert!(
            context_generic(&mut synthesized).paragraph_lists[0].paragraphs[0]
                .line_segs
                .is_empty(),
            "changed Generic context (width={change_width}) must invalidate saved lines"
        );
        context_generic(&mut synthesized).paragraph_lists[0].paragraphs[0].line_segs =
            vec![cached_segment(999)];
        restore_unchanged_controls(&original, &edited, &mut synthesized, SuffixCache::Keep);
        assert_eq!(
            context_generic(&mut synthesized).paragraph_lists[0].paragraphs[0].line_segs[0].v_pos,
            999,
            "changed Generic context (width={change_width}) keeps newly synthesized lines"
        );
    }
    #[test]
    fn changed_generic_shape_width_invalidates_saved_lines() {
        assert_changed_generic_context(true);
    }

    #[test]
    fn changed_generic_list_header_invalidates_saved_lines() {
        assert_changed_generic_context(false);
    }
}
