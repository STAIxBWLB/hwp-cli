//! `hwp convert` — 포맷 변환.
//!
//! M2 범위: hwp/hwpx → markdown/JSON. hwpx 쓰기(M4)와 hwp 쓰기(M6)는
//! 이후 마일스톤.

use std::path::{Path, PathBuf};

use crate::commands::cat::load_document;
use anyhow::Context as _;
use hwp_cli::cli::ConvertFormat;

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
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    input: &Path,
    output: &Path,
    to: Option<ConvertFormat>,
    strict: bool,
    preserve_layout: bool,
    embed_bin: bool,
    md_opts: &MdOpts,
    font_dirs: Vec<PathBuf>,
) -> anyhow::Result<()> {
    let report = execute(
        input,
        output,
        to,
        strict,
        preserve_layout,
        embed_bin,
        md_opts,
        font_dirs,
    )?;
    print_warnings(&report.warnings);
    eprintln!("변환 완료: {} → {}", input.display(), output.display());
    Ok(())
}

/// `hwp convert`와 MCP가 함께 쓰는 변환 서비스.
///
/// 출력은 검증이 끝날 때까지 destination에 게시하지 않으며, Markdown 이미지 sidecar도
/// 본문과 같은 복구 journal에 참여한다. 사용자 메시지 출력은 호출자가 담당한다.
#[allow(clippy::too_many_arguments)]
pub fn execute(
    input: &Path,
    output: &Path,
    to: Option<ConvertFormat>,
    strict: bool,
    preserve_layout: bool,
    embed_bin: bool,
    md_opts: &MdOpts,
    font_dirs: Vec<PathBuf>,
) -> anyhow::Result<ConvertReport> {
    let target = match to {
        Some(t) => t,
        None => infer_format(output)?,
    };
    let doc = load_document(input)?;
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
        return Ok(ConvertReport { warnings });
    }

    let warnings = crate::commands::output::write_validated(
        output,
        Some(input),
        |staged| match target {
            ConvertFormat::Md => unreachable!("Markdown은 sidecar 트랜잭션 경로에서 처리"),
            ConvertFormat::Html => {
                std::fs::write(staged, hwp_convert::to_html(&doc))?;
                Ok(Vec::new())
            }
            ConvertFormat::Odt => {
                std::fs::write(staged, hwp_convert::to_odt(&doc)?)?;
                Ok(Vec::new())
            }
            ConvertFormat::Pdf => {
                let result =
                    hwp_render::render_document_pdf(&doc, &pdf_render_opts(font_dirs), None)?;
                std::fs::write(staged, &result.data)?;
                Ok(render_issue_messages(&result.report))
            }
            ConvertFormat::Json => {
                std::fs::write(staged, hwp_convert::to_json(&doc, true, embed_bin)?)?;
                Ok(Vec::new())
            }
            ConvertFormat::Hwpx => Ok(hwpx::write::write_document_with(
                &doc,
                staged,
                &hwpx::write::HwpxWriteOptions {
                    preserve_linesegs: preserve_layout,
                },
            )?),
            ConvertFormat::Hwp => write_hwp(&doc, staged, preserve_layout),
        },
        |staged, warnings| {
            // DROP 여부를 게시 전에 판정한다. strict 실패 시 기존 destination은 그대로다.
            if matches!(target, ConvertFormat::Hwp | ConvertFormat::Hwpx) {
                bail_on_strict(strict, warnings)?;
                load_document(staged)
                    .with_context(|| format!("변환 문서 재읽기 실패: {}", staged.display()))?;
            }
            Ok(())
        },
    )?;
    Ok(ConvertReport { warnings })
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

/// `--strict`이고 보존 불가(`DROP:`) 경고가 있으면 비정상 종료한다.
/// 구조 보존 대상(hwp/hwpx)에만 의미가 있다 (md/html/pdf는 본디 손실 변환).
fn bail_on_strict(strict: bool, warnings: &[String]) -> anyhow::Result<()> {
    if !strict {
        return Ok(());
    }
    let drops: Vec<&str> = warnings
        .iter()
        .filter(|w| w.starts_with("DROP: "))
        .map(|w| w.as_str())
        .collect();
    if drops.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "--strict: 보존 불가 데이터 {}건 드롭\n{}",
        drops.len(),
        drops
            .iter()
            .map(|w| format!("  - {}", w.trim_start_matches("DROP: ")))
            .collect::<Vec<_>>()
            .join("\n")
    );
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
) -> anyhow::Result<Vec<String>> {
    write_hwp_impl(doc, output, preserve_layout, false, None)
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
) -> anyhow::Result<Vec<String>> {
    if doc.meta.source_format == "hwp5" {
        // 원본 줄 배치 보존(preserve), 합성 정규화 없음 — 편집 문단만 count=0.
        write_hwp_impl(doc, output, true, false, None)
    } else {
        write_hwp_impl(doc, output, false, true, None)
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
) -> anyhow::Result<Vec<String>> {
    write_hwp_impl(doc, output, false, true, None)
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
) -> anyhow::Result<Vec<String>> {
    if font_files.is_empty() {
        anyhow::bail!("isolated HWP writer requires at least one explicit font file");
    }
    write_hwp_impl(doc, output, false, true, Some(font_files))
}

fn write_hwp_impl(
    doc: &hwp_model::Document,
    output: &std::path::Path,
    preserve_layout: bool,
    edited: bool,
    isolated_font_files: Option<&[std::path::PathBuf]>,
) -> anyhow::Result<Vec<String>> {
    let font_dir =
        std::path::PathBuf::from(std::env::var("HWP_FONT_DIR").unwrap_or_else(|_| "fonts".into()));
    let synthesize = doc.meta.source_format != "hwp5" || edited;
    let has_source_linesegs = doc
        .sections
        .iter()
        .flat_map(|s| &s.paragraphs)
        .any(|p| !p.line_segs.is_empty());

    let mut report: Vec<String> = Vec::new();
    let owned;
    let doc = if !synthesize || preserve_layout {
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
        report.append(&mut render_issue_messages(&warns.finish()));
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

    let warnings = hwp5::write_document(
        doc,
        output,
        &hwp5::WriteOptions {
            prv_image,
            preserve_linesegs: preserve_layout,
            edited,
        },
    )?;
    report.extend(warnings);
    Ok(report)
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
                Ok(vec!["DROP: 지원하지 않는 테스트 컨트롤".to_string()])
            },
            |_, warnings| bail_on_strict(true, warnings),
        );
        assert!(result.is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"ORIGINAL");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn informational_render_events_are_not_writer_warnings() {
        let mut issues = hwp_render::RenderIssueAccumulator::new();
        issues.push(hwp_render::RenderIssueCode::FontMatched, b"font");
        let report = issues.finish();
        assert!(render_issue_messages(&report).is_empty());
    }
}
