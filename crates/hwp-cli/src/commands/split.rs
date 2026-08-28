//! `hwp split` — divide one document into several outputs (GM-4, FLOW-02).
//!
//! One document in, a set of documents out, published as a single all-or-nothing
//! unit (D-16): every fragment is built and verified at a private temp path first,
//! and only the whole set is handed to the atomic multi-file publish transaction
//! in `crate::commands::output` once. The default split unit is the section
//! boundary (D-05); `--pages` opts into
//! page-range splitting, where a boundary that falls inside a paragraph rounds
//! forward to the next paragraph's start (D-08) and the adjustment is reported.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Context as _;
use hwp_cli::cli::PasswordArgs;
use hwp_convert::document_split::{PageRange, SplitOutcome};
use hwp_model::{
    Document, PreservationCode, PreservationDisposition, PreservationEvent,
    PreservationResourceKind, WriteReport,
};

use crate::commands::cat::{
    LoadOptions, load_document, load_document_with_options, resolve_password_args,
};
use crate::format::FileFormat;

/// Parses every `--pages` value ("N" or "N-M", 1-based inclusive) into a
/// [`PageRange`]. A malformed value is refused by name, naming the accepted
/// forms — never silently coerced.
fn parse_page_ranges(values: &[String]) -> anyhow::Result<Vec<PageRange>> {
    values.iter().map(|value| parse_page_range(value)).collect()
}

fn parse_page_range(value: &str) -> anyhow::Result<PageRange> {
    let (first_str, last_str) = value.split_once('-').unwrap_or((value, value));
    let first = first_str.trim().parse::<usize>().ok();
    let last = last_str.trim().parse::<usize>().ok();
    match (first, last) {
        (Some(first), Some(last)) if first >= 1 => Ok(PageRange { first, last }),
        _ => anyhow::bail!(
            "잘못된 --pages 값: '{value}' (허용 형식: N 또는 N-M, N/M은 1 이상의 정수) — 예: --pages 2-3"
        ),
    }
}

static FRAGMENT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A throwaway temp path this process owns exclusively — neither writer format
/// accepts an in-memory sink, so every fragment is built to disk, verified, and
/// removed before this function returns (success or failure).
fn fragment_temp_path(extension: &str) -> PathBuf {
    let sequence = FRAGMENT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "hwp-cli-split-fragment-{}-{sequence}.{extension}",
        std::process::id()
    ))
}

/// Writes one fragment to a private temp file, reloads it to verify it round-trips
/// through the reader, and reads its bytes back — always cleaning up the temp file,
/// on every return path.
fn build_fragment_file(
    fragment: &Document,
    source_format: FileFormat,
    extension: &str,
) -> anyhow::Result<(WriteReport, Document, Vec<u8>)> {
    let temp_path = fragment_temp_path(extension);
    let result = (|| -> anyhow::Result<(WriteReport, Document, Vec<u8>)> {
        let write_report = match source_format {
            FileFormat::Hwp5 => crate::commands::convert::write_hwp(fragment, &temp_path, false)?,
            FileFormat::Hwpx => hwpx::write::write_document_with_report_with(
                fragment,
                &temp_path,
                &hwpx::write::HwpxWriteOptions {
                    preserve_linesegs: false,
                },
            )?,
        };
        let output_document = load_document(&temp_path)
            .with_context(|| format!("분할 조각 재읽기 실패: {}", temp_path.display()))?;
        let bytes = std::fs::read(&temp_path)
            .with_context(|| format!("분할 조각을 읽을 수 없습니다: {}", temp_path.display()))?;
        Ok((write_report, output_document, bytes))
    })();
    let _ = std::fs::remove_file(&temp_path);
    result
}

/// D-17: the one-line Korean stderr summary for a lossy split, mirroring
/// `commands::merge`'s `preservation_summary_line` exactly (kept as a small,
/// module-local copy rather than a cross-module call — the contract is a
/// handful of lines and each command owns its own context string).
fn preservation_summary_line(report: &hwp_model::PreservationReport) -> Option<String> {
    if report.is_lossless() {
        return None;
    }
    let codes: std::collections::BTreeSet<&str> = report
        .events
        .iter()
        .map(|event| event.code.as_str())
        .collect();
    Some(format!(
        "분할 경고: 보존 불가 이벤트 {}건 ({})",
        report.events.iter().map(|event| event.count).sum::<usize>(),
        codes.into_iter().collect::<Vec<_>>().join(", ")
    ))
}

fn print_rounding_lines(roundings: &[hwp_convert::document_split::PageBoundaryRounding]) {
    for rounding in roundings {
        eprintln!(
            "페이지 범위 경계 보정: 범위 {} — 구역 {} 문단 {}(오프셋 {})에서 구역 {} 문단 {}(문단 시작)로 이동",
            rounding.range_index + 1,
            rounding.from.section,
            rounding.from.paragraph,
            rounding.from.wchar_offset,
            rounding.to.section,
            rounding.to.paragraph,
        );
    }
}

/// The strict/summary view of a split's preservation ledger: every event
/// except D-08 boundary roundings. A rounding is an adjustment, not a loss —
/// the straddling paragraph stays whole in the earlier fragment — so it stays
/// in the ledger for `--loss-report` audit but must not trip `--strict`
/// (whose contract is "data that cannot be preserved (opaque)") nor be
/// counted again in the "보존 불가" summary, which would double-report what
/// `print_rounding_lines` already printed (#174).
fn strict_loss_view(report: &hwp_model::PreservationReport) -> hwp_model::PreservationReport {
    let mut view = hwp_model::PreservationReport::new();
    for event in &report.events {
        if event.code != PreservationCode::PageRangeParagraphRounded {
            view.record(event.clone());
        }
    }
    view
}

/// Removes `stem-NNN.ext` fragments a previous, larger run left in `out_dir`
/// but the set just published does not include (#177). Runs only after the
/// all-or-nothing publish succeeded, so a failed split never deletes a prior
/// run's output; a removal failure is a warning, never an error, because the
/// new fragment set is already in place.
fn remove_stale_fragments(out_dir: &Path, stem: &str, extension: &str, keep: &[PathBuf]) {
    let entries = match std::fs::read_dir(out_dir) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!(
                "출력 디렉터리를 읽지 못해 이전 조각을 정리하지 못했습니다: {} ({error})",
                out_dir.display()
            );
            return;
        }
    };
    let prefix = format!("{stem}-");
    let suffix = format!(".{extension}");
    for entry in entries.flatten() {
        let path = entry.path();
        let is_fragment_name =
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.strip_prefix(&prefix)
                        .and_then(|rest| rest.strip_suffix(&suffix))
                        .is_some_and(|index| {
                            !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
                        })
                });
        if is_fragment_name
            && path.is_file()
            && !keep.contains(&path)
            && let Err(error) = std::fs::remove_file(&path)
        {
            eprintln!(
                "이전 실행의 분할 조각을 지우지 못했습니다: {} ({error})",
                path.display()
            );
        }
    }
}

/// `hwp split` entry point.
pub fn run(
    input: &Path,
    out_dir: &Path,
    pages: &[String],
    strict: bool,
    loss_report: Option<&Path>,
    password: PasswordArgs,
) -> anyhow::Result<()> {
    let resolved_password = resolve_password_args(password, input)?;
    let options = LoadOptions {
        password: resolved_password.as_ref(),
    };
    let source_format = crate::format::detect(input)?;
    // Fragment names normalize the extension to lowercase, the same casing
    // rule the rest of the CLI applies to format-bearing names (#177).
    let extension = input
        .extension()
        .and_then(|extension| extension.to_str())
        .with_context(|| format!("입력 파일 확장자를 확인할 수 없습니다: {}", input.display()))?
        .to_ascii_lowercase();
    let stem = input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .with_context(|| format!("입력 파일 이름을 확인할 수 없습니다: {}", input.display()))?
        .to_string();

    // D-15 alias guard: the loss report must never alias the input (or the
    // output directory) — an unguarded report write would silently overwrite
    // the input with JSON. Same rejection merge applies, before any work.
    if let Some(report_path) = loss_report {
        crate::commands::merge::reject_loss_report_aliases(
            report_path,
            &[input.to_path_buf()],
            out_dir,
        )?;
    }

    let document = load_document_with_options(input, &options).map_err(anyhow::Error::new)?;

    let outcome: SplitOutcome = if pages.is_empty() {
        hwp_convert::document_split::split_sections(&document)
    } else {
        let ranges = parse_page_ranges(pages)?;
        hwp_convert::document_split::split_page_ranges(&document, &ranges)
    }
    .map_err(|error| anyhow::anyhow!("분할 실패: {error}"))?;
    print_rounding_lines(&outcome.roundings);

    let destinations: Vec<PathBuf> = (1..=outcome.fragments.len())
        .map(|index| out_dir.join(format!("{stem}-{index:03}.{extension}")))
        .collect();

    // Stem-collision precheck before any staging (T-03-08): every fragment
    // destination must not alias the input or the loss-report path.
    let mut alias_targets: Vec<&Path> = vec![input];
    if let Some(report_path) = loss_report {
        alias_targets.push(report_path);
    }
    for destination in &destinations {
        crate::commands::output::reject_output_aliases(destination, &alias_targets)?;
    }

    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("출력 디렉터리를 만들 수 없습니다: {}", out_dir.display()))?;

    let mut report = WriteReport::new();
    let mut fragment_bytes: Vec<(PathBuf, Vec<u8>)> = Vec::with_capacity(outcome.fragments.len());
    for (fragment, destination) in outcome.fragments.iter().zip(&destinations) {
        let (write_report, output_document, bytes) =
            build_fragment_file(fragment, source_format, &extension)?;
        report.warnings.extend(write_report.warnings);
        report.preservation.extend(write_report.preservation);
        report
            .preservation
            .extend(crate::commands::preservation::inspect_conversion_semantics(
                fragment,
                &output_document,
            ));
        fragment_bytes.push((destination.clone(), bytes));
    }

    // D-08 roundings are ledgered for audit (`--loss-report`); they are
    // adjustments, not losses, so `strict_loss_view` keeps them out of the
    // --strict refusal and the loss summary (#174).
    for _rounding in &outcome.roundings {
        report.preservation.record(PreservationEvent::new(
            PreservationCode::PageRangeParagraphRounded,
            PreservationResourceKind::Control,
            PreservationDisposition::ChangedNonTarget,
            1,
        ));
    }

    // D-15: write the loss report before any strict refusal, so the evidence
    // for a failed run survives on disk even when the run itself does not
    // publish.
    if let Some(report_path) = loss_report {
        crate::commands::convert::write_loss_report(report_path, &report.preservation)?;
    }
    // #174: the ledger keeps D-08 roundings for audit, but --strict and the
    // loss summary judge only genuine preservation losses.
    let strict_view = strict_loss_view(&report.preservation);
    if strict {
        crate::commands::reject_preservation_loss("split", &strict_view)?;
    }
    // D-17: unconditional stderr summary, independent of --strict/--loss-report.
    if let Some(line) = preservation_summary_line(&strict_view) {
        eprintln!("{line}");
    }

    // D-16: publish the whole fragment set in one transaction — never per
    // member inside the build loop above, which has no outer transaction.
    crate::commands::output::write_validated_files(&fragment_bytes, Some(input))?;
    // #177: only after the publish succeeded, drop fragments a previous,
    // larger run left behind.
    remove_stale_fragments(out_dir, &stem, &extension, &destinations);

    eprintln!(
        "분할 완료: {} → {}개 조각 ({})",
        input.display(),
        destinations.len(),
        out_dir.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "hwp-cli-split-test-{label}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    fn write_three_section_hwp(path: &Path) {
        let mut doc = hwp_convert::from_markdown("첫 구역\n");
        doc.sections
            .push(hwp_convert::from_markdown("둘째 구역\n").sections[0].clone());
        doc.sections
            .push(hwp_convert::from_markdown("셋째 구역\n").sections[0].clone());
        crate::commands::convert::write_hwp(&doc, path, false).unwrap();
    }

    #[test]
    fn 세_구역_문서를_분할하면_조각_세_개가_순서대로_생긴다() {
        let dir = temp_dir("three-sections");
        let input = dir.join("in.hwp");
        let out_dir = dir.join("frag");
        write_three_section_hwp(&input);

        run(&input, &out_dir, &[], false, None, PasswordArgs::default()).unwrap();

        assert!(out_dir.join("in-001.hwp").exists());
        assert!(out_dir.join("in-002.hwp").exists());
        assert!(out_dir.join("in-003.hwp").exists());
        assert!(!out_dir.join("in-004.hwp").exists());

        let frag1 = load_document(&out_dir.join("in-001.hwp")).unwrap();
        assert_eq!(frag1.sections.len(), 1);
        assert!(frag1.plain_text().contains("첫 구역"));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn 단일_구역_문서는_조각_하나만_낸다() {
        let dir = temp_dir("single-section");
        let input = dir.join("in.hwp");
        let out_dir = dir.join("frag");
        let doc = hwp_convert::from_markdown("단일 구역\n");
        crate::commands::convert::write_hwp(&doc, &input, false).unwrap();

        run(&input, &out_dir, &[], false, None, PasswordArgs::default()).unwrap();

        assert!(out_dir.join("in-001.hwp").exists());
        assert!(!out_dir.join("in-002.hwp").exists());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn 잘못된_pages_값은_허용_형식을_알려주며_거부된다() {
        let error = parse_page_ranges(&["abc".to_string()])
            .unwrap_err()
            .to_string();
        assert!(error.contains("N 또는 N-M"));
    }

    #[test]
    fn pages_범위_구문은_단일_페이지와_범위를_모두_받아들인다() {
        let ranges = parse_page_ranges(&["2".to_string(), "3-5".to_string()]).unwrap();
        assert_eq!(
            ranges,
            vec![
                PageRange { first: 2, last: 2 },
                PageRange { first: 3, last: 5 }
            ]
        );
    }

    #[test]
    fn 출력_경로가_입력과_같으면_거부된다() {
        let dir = temp_dir("alias-input");
        let input = dir.join("in.hwp");
        write_three_section_hwp(&input);

        let result = crate::commands::output::reject_output_aliases(&input, &[input.as_path()]);
        assert!(result.is_err());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn loss_report_경로가_입력과_같으면_거부() {
        // Issue #167: the fragment-destination precheck never guarded the
        // report path itself, so `--loss-report in.hwp` used to overwrite the
        // input with JSON.
        let dir = temp_dir("loss-report-alias-input");
        let input = dir.join("in.hwp");
        let out_dir = dir.join("frag");
        write_three_section_hwp(&input);
        let original = std::fs::read(&input).unwrap();

        let result = run(
            &input,
            &out_dir,
            &[],
            false,
            Some(input.as_path()),
            PasswordArgs::default(),
        );
        assert!(result.is_err());
        assert!(!out_dir.join("in-001.hwp").exists());
        assert_eq!(std::fs::read(&input).unwrap(), original);

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn 손실_없으면_요약_없음() {
        assert!(preservation_summary_line(&hwp_model::PreservationReport::new()).is_none());
    }

    #[test]
    fn 라운딩_이벤트는_장부에는_남지만_strict와_요약에서는_빠진다() {
        // Issue #174: a rounded --pages boundary is an adjustment, not a loss.
        // It must stay in the ledger for --loss-report audit, yet neither trip
        // --strict nor appear in the "보존 불가" summary (double report).
        let mut report = hwp_model::PreservationReport::new();
        report.record(PreservationEvent::new(
            PreservationCode::PageRangeParagraphRounded,
            PreservationResourceKind::Control,
            PreservationDisposition::ChangedNonTarget,
            1,
        ));

        let view = strict_loss_view(&report);
        assert!(view.is_lossless());
        assert!(preservation_summary_line(&view).is_none());
        assert!(crate::commands::reject_preservation_loss("split", &view).is_ok());
        // The ledger itself still carries the rounding event.
        assert_eq!(report.events.len(), 1);
        assert_eq!(
            report.events[0].code,
            PreservationCode::PageRangeParagraphRounded
        );
    }

    #[test]
    fn 진짜_손실은_라운딩과_섞여_있어도_strict가_거부하고_요약한다() {
        let mut report = hwp_model::PreservationReport::new();
        report.record(PreservationEvent::new(
            PreservationCode::PageRangeParagraphRounded,
            PreservationResourceKind::Control,
            PreservationDisposition::ChangedNonTarget,
            2,
        ));
        report.record(PreservationEvent::new(
            PreservationCode::ControlRemoved,
            PreservationResourceKind::Control,
            PreservationDisposition::Removed,
            1,
        ));

        let view = strict_loss_view(&report);
        assert!(!view.is_lossless());
        assert!(crate::commands::reject_preservation_loss("split", &view).is_err());
        let line = preservation_summary_line(&view).unwrap();
        assert!(line.contains("1건"));
        assert!(line.contains("control_removed"));
        assert!(!line.contains("page_range_paragraph_rounded"));
    }

    #[test]
    fn 이전_실행이_남긴_초과_조각만_지우고_나머지_파일은_남긴다() {
        // Issue #177: re-running with fewer fragments must clean up the stale
        // stem-NNN files, without touching anything else in --out-dir.
        let dir = temp_dir("stale-fragments");
        let out_dir = dir.join("frag");
        std::fs::create_dir(&out_dir).unwrap();
        for name in [
            "in-001.hwp",
            "in-002.hwp",
            "in-003.hwp",
            "in-notes.hwp",
            "other-003.hwp",
        ] {
            std::fs::write(out_dir.join(name), b"stale").unwrap();
        }
        // A directory that merely matches the fragment naming is not a file a
        // previous run published — leave it alone.
        std::fs::create_dir(out_dir.join("in-004.hwp")).unwrap();

        let keep = vec![out_dir.join("in-001.hwp"), out_dir.join("in-002.hwp")];
        remove_stale_fragments(&out_dir, "in", "hwp", &keep);

        assert!(out_dir.join("in-001.hwp").exists());
        assert!(out_dir.join("in-002.hwp").exists());
        assert!(!out_dir.join("in-003.hwp").exists());
        assert!(out_dir.join("in-004.hwp").is_dir());
        assert!(out_dir.join("in-notes.hwp").exists());
        assert!(out_dir.join("other-003.hwp").exists());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn 대문자_확장자_입력의_조각_이름은_소문자_확장자를_쓴다() {
        // Issue #177: fragment names normalize the input extension's casing.
        let dir = temp_dir("uppercase-extension");
        let input = dir.join("in.HWP");
        let out_dir = dir.join("frag");
        write_three_section_hwp(&input);

        run(&input, &out_dir, &[], false, None, PasswordArgs::default()).unwrap();

        for name in ["in-001.hwp", "in-002.hwp", "in-003.hwp"] {
            assert!(
                out_dir.join(name).exists(),
                "소문자 확장자 조각이 있어야 합니다: {name}"
            );
        }

        std::fs::remove_dir_all(dir).unwrap();
    }
}
