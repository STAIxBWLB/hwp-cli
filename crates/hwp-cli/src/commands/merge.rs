//! `hwp merge` — combine several HWP5/HWPX inputs into one output (GM-3, FLOW-01).
//!
//! One thin path: load every input with the existing password-aware loader, merge
//! them through `hwp_convert::document_merge::merge_documents` (D-02: one Section
//! per input, concatenated in argument order), pick the writer from `--output`'s
//! extension (D-03), and publish through the existing staged-write-then-verify
//! transaction (`crate::commands::output::write_validated`). `--strict` and
//! `--loss-report` follow the `convert` pattern exactly (D-15): the loss report is
//! written before any strict refusal, so the evidence for a failed run survives on
//! disk even when the run itself does not publish.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use hwp_cli::cli::PasswordArgs;

use crate::commands::cat::{
    LoadOptions, load_document, load_document_with_options, resolve_password_args,
};
use crate::format::FileFormat;

/// Output writer chosen from `--output`'s extension (D-03).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeFormat {
    Hwp,
    Hwpx,
}

impl MergeFormat {
    fn target_format(self) -> FileFormat {
        match self {
            Self::Hwp => FileFormat::Hwp5,
            Self::Hwpx => FileFormat::Hwpx,
        }
    }
}

fn infer_merge_format(output: &Path) -> anyhow::Result<MergeFormat> {
    match output
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("hwp") => Ok(MergeFormat::Hwp),
        Some("hwpx") => Ok(MergeFormat::Hwpx),
        other => anyhow::bail!(
            "병합 출력 확장자를 지원하지 않습니다 (확장자: {other:?}) — .hwp 또는 .hwpx만 지원합니다"
        ),
    }
}

/// Rejects a `--loss-report` path that aliases any input or the output, using
/// the same path normalization `convert.rs` applies to its own report path
/// (D-15's alias guard, extended from one input to N).
fn reject_loss_report_aliases(
    report_path: &Path,
    inputs: &[PathBuf],
    output: &Path,
) -> anyhow::Result<()> {
    let report_normalized = crate::commands::convert::normalize_for_alias_compare(report_path);
    for other in inputs.iter().map(PathBuf::as_path).chain([output]) {
        if report_normalized == crate::commands::convert::normalize_for_alias_compare(other) {
            anyhow::bail!(
                "--loss-report 경로가 입력/출력과 같을 수 없습니다: {}",
                report_path.display()
            );
        }
    }
    let mut alias_targets: Vec<&Path> = inputs.iter().map(PathBuf::as_path).collect();
    alias_targets.push(output);
    crate::commands::output::reject_output_aliases(report_path, &alias_targets)
}

/// D-17: the one-line Korean stderr summary for a lossy merge, naming the
/// total preservation event count and the distinct event codes. `None` when
/// the report is lossless — nothing to warn about. Split out as a pure
/// function so the summary contract is unit-testable without needing to
/// capture the process's real stderr.
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
        "병합 경고: 보존 불가 이벤트 {}건 ({})",
        report.events.iter().map(|event| event.count).sum::<usize>(),
        codes.into_iter().collect::<Vec<_>>().join(", ")
    ))
}

/// The aggregate preload ceiling for merge inputs — reuses `convert.rs`'s
/// existing `--out-dir` batch memory limit rather than inventing a new one.
const MAX_MERGE_INPUT_BYTES: u64 = crate::commands::convert::MAX_PRELOADED_BATCH_BYTES;

/// Overflow-checked aggregate accumulation against `MAX_MERGE_INPUT_BYTES`,
/// mirroring convert.rs's `add_preloaded_batch_reservation` arithmetic (same
/// ceiling, same checked-add shape; a merge-specific message since this path
/// runs unconditionally for every input, not only password-protected ones).
fn accumulate_input_reservation(total: u64, size: u64) -> anyhow::Result<u64> {
    let next = total
        .checked_add(size)
        .context("병합 입력 메모리 예약 오버플로")?;
    if next > MAX_MERGE_INPUT_BYTES {
        anyhow::bail!(
            "병합 입력의 총 크기가 사전 적재 메모리 한도를 초과했습니다: {next} > {MAX_MERGE_INPUT_BYTES} bytes"
        );
    }
    Ok(next)
}

/// FLOW-01 `empty` probe (open assumption A-01): a merge result carrying zero
/// Sections is refused rather than published — the writers would otherwise
/// silently pad it to one empty section.
fn reject_empty_sections(document: &hwp_model::Document) -> anyhow::Result<()> {
    if document.sections.is_empty() {
        anyhow::bail!("병합 결과 구역이 없어 게시하지 않습니다 — 입력에 유효한 구역이 없습니다");
    }
    Ok(())
}

/// `hwp merge` entry point. Resolves the password once and applies it uniformly
/// to every input (matching `commands::convert::run_multi_with_password`'s
/// single-password-per-batch precedent).
pub fn run(
    inputs: &[PathBuf],
    output: &Path,
    strict: bool,
    loss_report: Option<&Path>,
    password: PasswordArgs,
) -> anyhow::Result<()> {
    // v1 accepts no per-input stdin disambiguation (the loader stages stdin
    // to a single temp path); refuse "-" by name rather than merging it as a
    // literal file named "-".
    if let Some(stdin_input) = inputs.iter().find(|input| input.as_os_str() == "-") {
        anyhow::bail!(
            "hwp merge는 표준 입력(\"-\")을 지원하지 않습니다 — 파일 경로를 직접 지정하세요: {}",
            stdin_input.display()
        );
    }
    let first = inputs
        .first()
        .ok_or_else(|| anyhow::anyhow!("병합 입력이 비어 있습니다"))?;
    let resolved_password = resolve_password_args(password, first)?;
    let options = LoadOptions {
        password: resolved_password.as_ref(),
    };

    let format = infer_merge_format(output)?;
    let target_format = format.target_format();

    let input_refs: Vec<&Path> = inputs.iter().map(PathBuf::as_path).collect();
    crate::commands::output::reject_output_aliases(output, &input_refs)?;
    if let Some(report_path) = loss_report {
        reject_loss_report_aliases(report_path, inputs, output)?;
    }

    let mut documents = Vec::with_capacity(inputs.len());
    let mut source_formats = Vec::with_capacity(inputs.len());
    let mut reserved_bytes: u64 = 0;
    for input in inputs {
        let size = std::fs::metadata(input)
            .with_context(|| format!("입력 파일 크기를 확인할 수 없습니다: {}", input.display()))?
            .len();
        reserved_bytes = accumulate_input_reservation(reserved_bytes, size)?;
        source_formats.push(crate::format::detect(input)?);
        documents.push(load_document_with_options(input, &options).map_err(anyhow::Error::new)?);
    }

    let outcome = hwp_convert::document_merge::merge_documents(&documents)
        .map_err(|error| anyhow::anyhow!("병합 실패: {error}"))?;
    let merged = outcome.document;
    reject_empty_sections(&merged)?;

    let write_staged = |staged: &Path| -> anyhow::Result<hwp_model::WriteReport> {
        let mut report = match format {
            MergeFormat::Hwp => crate::commands::convert::write_hwp(&merged, staged, false)?,
            MergeFormat::Hwpx => hwpx::write::write_document_with_report_with(
                &merged,
                staged,
                &hwpx::write::HwpxWriteOptions {
                    preserve_linesegs: false,
                },
            )?,
        };
        for source_format in &source_formats {
            if *source_format != target_format {
                report.preservation.extend(
                    crate::commands::preservation::inspect_cross_format_container(
                        &merged,
                        *source_format,
                        target_format,
                    ),
                );
            }
        }
        let output_document = load_document(staged)
            .with_context(|| format!("병합 문서 재읽기 실패: {}", staged.display()))?;
        report
            .preservation
            .extend(crate::commands::preservation::inspect_conversion_semantics(
                &merged,
                &output_document,
            ));
        // D-14: every loss the general graft (or GSO renumbering) couldn't
        // carry losslessly also joins the ledger, so --strict and the D-17
        // stderr summary fire for these too, with no further wiring.
        report
            .preservation
            .extend(crate::commands::preservation::inspect_document_merge_losses(&outcome.losses));
        Ok(report)
    };
    let verify_staged = |staged: &Path, report: &hwp_model::WriteReport| -> anyhow::Result<()> {
        // strict 거부보다 먼저 기록해야 실패한 병합의 판정 근거가 파일로 남는다 (D-15).
        if let Some(report_path) = loss_report {
            crate::commands::convert::write_loss_report(report_path, &report.preservation)?;
        }
        // --strict/--loss-report와 무관하게, 보존 손실이 기록됐으면 항상 stderr에
        // 한 줄 요약을 남긴다 — 아무 플래그도 주지 않은 사용자가 손실 있는 문서를
        // 조용히 받지 않게 한다 (D-17).
        if let Some(line) = preservation_summary_line(&report.preservation) {
            eprintln!("{line}");
        }
        if strict {
            crate::commands::reject_preservation_loss("merge", &report.preservation)?;
        }
        load_document(staged)
            .with_context(|| format!("병합 문서 재읽기 실패: {}", staged.display()))?;
        Ok(())
    };

    crate::commands::output::write_validated(output, None, write_staged, verify_staged)?;

    eprintln!("병합 완료: {}개 입력 → {}", inputs.len(), output.display());
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
            "hwp-cli-merge-test-{label}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    fn write_generated_hwp(path: &Path, markdown: &str) {
        let doc = hwp_convert::from_markdown(markdown);
        crate::commands::convert::write_hwp(&doc, path, false).unwrap();
    }

    /// A genuine hwpx-origin input: `hwpx_settings_xml`/`hwpx_version_xml`
    /// always come back `Some(...)` from a real hwpx read (package entries
    /// every valid hwpx file carries), so merging this as a non-primary input
    /// naturally trips `MergeLoss::PackagePassthroughDropped` through the real
    /// path — no synthetic fixture construction needed.
    fn write_generated_hwpx(path: &Path, markdown: &str) {
        let doc = hwp_convert::from_markdown(markdown);
        hwpx::write::write_document_with_report_with(
            &doc,
            path,
            &hwpx::write::HwpxWriteOptions {
                preserve_linesegs: false,
            },
        )
        .unwrap();
    }

    #[test]
    fn 두_hwp_입력을_병합하면_본문이_순서대로_나온다() {
        let dir = temp_dir("two-hwp");
        let a = dir.join("a.hwp");
        let b = dir.join("b.hwp");
        let out = dir.join("out.hwp");
        write_generated_hwp(&a, "문서 A입니다\n");
        write_generated_hwp(&b, "문서 B입니다\n");

        run(&[a, b], &out, false, None, PasswordArgs::default()).unwrap();

        let merged = load_document(&out).unwrap();
        assert_eq!(merged.sections.len(), 2);
        let text = merged.plain_text();
        let a_pos = text.find("문서 A입니다").unwrap();
        let b_pos = text.find("문서 B입니다").unwrap();
        assert!(a_pos < b_pos);

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn 출력_확장자가_hwp_hwpx가_아니면_에러() {
        let dir = temp_dir("bad-ext");
        let a = dir.join("a.hwp");
        let b = dir.join("b.hwp");
        let out = dir.join("out.docx");
        write_generated_hwp(&a, "A\n");
        write_generated_hwp(&b, "B\n");

        let result = run(&[a, b], &out, false, None, PasswordArgs::default());
        assert!(result.is_err());
        assert!(!out.exists());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn preservation_summary_line_은_손실_없으면_none() {
        assert!(preservation_summary_line(&hwp_model::PreservationReport::new()).is_none());
    }

    #[test]
    fn preservation_summary_line_은_건수와_코드를_담는다() {
        let mut report = hwp_model::PreservationReport::new();
        report.record(hwp_model::PreservationEvent::new(
            hwp_model::PreservationCode::OpaqueControlUnrepresentable,
            hwp_model::PreservationResourceKind::Control,
            hwp_model::PreservationDisposition::Unrepresentable,
            3,
        ));
        let line = preservation_summary_line(&report).unwrap();
        assert!(line.contains("3건"));
        assert!(line.contains("opaque_control_unrepresentable"));
    }

    #[test]
    fn strict_실패시_기존_출력이_보존된다() {
        // Regression shape mirrors convert.rs's
        // strict_drop_failure_preserves_existing_destination: forces a loss
        // directly against write_validated, independent of what document_merge
        // itself can produce. `strict는_패키지_전용_필드_손실시_거부한다` below
        // exercises the same contract through a genuine merge-produced loss.
        let dir = temp_dir("strict-preserve");
        let destination = dir.join("result.hwp");
        std::fs::write(&destination, b"ORIGINAL").unwrap();

        let result = crate::commands::output::write_validated(
            &destination,
            None,
            |staged| {
                std::fs::write(staged, b"PARTIAL MERGE")?;
                let mut report = hwp_model::WriteReport::new();
                report.loss(
                    hwp_model::PreservationCode::OpaqueControlUnrepresentable,
                    hwp_model::PreservationResourceKind::Control,
                    hwp_model::PreservationDisposition::Unrepresentable,
                    1,
                );
                Ok(report)
            },
            |_, report| crate::commands::reject_preservation_loss("merge", &report.preservation),
        );
        assert!(result.is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"ORIGINAL");

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn strict는_패키지_전용_필드_손실시_거부한다() {
        // Closes .planning/WINDOWS.md #6: exercises a genuine (not
        // synthetic-formatter-only) preservation loss through the real
        // merge_documents → inspect_document_merge_losses → reject_preservation_loss
        // path end to end. A pre-existing --output must survive the refusal (D-15).
        let dir = temp_dir("strict-package-loss");
        let a = dir.join("a.hwp");
        let b = dir.join("b.hwpx");
        let out = dir.join("out.hwp");
        std::fs::write(&out, b"ORIGINAL").unwrap();
        write_generated_hwp(&a, "문서 A\n");
        write_generated_hwpx(&b, "문서 B\n");

        let result = run(&[a, b], &out, true, None, PasswordArgs::default());
        assert!(result.is_err());
        assert_eq!(std::fs::read(&out).unwrap(), b"ORIGINAL");

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn 실제_손실이_있어도_strict_없으면_게시된다() {
        // Closes .planning/WINDOWS.md #6: the same genuine loss as above, but
        // without --strict — D-17 says warn, not block, so the merge must
        // still publish (the eprintln! summary line itself fires on this
        // real path; its content contract was already unit-tested in
        // preservation_summary_line_은_건수와_코드를_담는다).
        let dir = temp_dir("real-loss-non-strict");
        let a = dir.join("a.hwp");
        let b = dir.join("b.hwpx");
        let out = dir.join("out.hwp");
        write_generated_hwp(&a, "문서 A\n");
        write_generated_hwpx(&b, "문서 B\n");

        run(&[a, b], &out, false, None, PasswordArgs::default()).unwrap();
        assert!(out.exists());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn loss_report_경로가_입력과_같으면_거부() {
        let dir = temp_dir("loss-report-alias-input");
        let a = dir.join("a.hwp");
        let b = dir.join("b.hwp");
        let out = dir.join("out.hwp");
        write_generated_hwp(&a, "A\n");
        write_generated_hwp(&b, "B\n");

        let result = run(
            &[a.clone(), b],
            &out,
            false,
            Some(a.as_path()),
            PasswordArgs::default(),
        );
        assert!(result.is_err());
        assert!(!out.exists());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn loss_report_경로가_출력과_같으면_거부() {
        let dir = temp_dir("loss-report-alias-output");
        let a = dir.join("a.hwp");
        let b = dir.join("b.hwp");
        let out = dir.join("out.hwp");
        write_generated_hwp(&a, "A\n");
        write_generated_hwp(&b, "B\n");

        let result = run(
            &[a, b],
            &out,
            false,
            Some(out.as_path()),
            PasswordArgs::default(),
        );
        assert!(result.is_err());
        assert!(!out.exists());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn loss_report는_보존_보고서_계약을_따른다() {
        let dir = temp_dir("loss-report-contract");
        let report_path = dir.join("report.json");
        crate::commands::convert::write_loss_report(
            &report_path,
            &hwp_model::PreservationReport::new(),
        )
        .unwrap();
        let parsed: hwp_model::PreservationReport =
            serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
        assert_eq!(parsed.contract, hwp_model::PRESERVATION_REPORT_CONTRACT);
        assert!(parsed.is_lossless());

        std::fs::remove_dir_all(dir).unwrap();
    }

    fn section_texts(doc: &hwp_model::Document) -> Vec<String> {
        doc.sections
            .iter()
            .map(|section| {
                section
                    .paragraphs
                    .iter()
                    .flat_map(|p| &p.chars)
                    .filter_map(|c| match c {
                        hwp_model::HwpChar::Text(ch) => Some(*ch),
                        _ => None,
                    })
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn 세_입력은_인자_순서대로_세_구역이_된다() {
        let dir = temp_dir("three-order");
        let a = dir.join("a.hwp");
        let b = dir.join("b.hwp");
        let c = dir.join("c.hwp");
        let out = dir.join("out.hwp");
        write_generated_hwp(
            &a,
            "문서 A
",
        );
        write_generated_hwp(
            &b,
            "문서 B
",
        );
        write_generated_hwp(
            &c,
            "문서 C
",
        );

        run(&[a, b, c], &out, false, None, PasswordArgs::default()).unwrap();

        let merged = load_document(&out).unwrap();
        let texts = section_texts(&merged);
        assert_eq!(texts.len(), 3);
        assert!(texts[0].contains("문서 A"));
        assert!(texts[1].contains("문서 B"));
        assert!(texts[2].contains("문서 C"));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn 인자_순서를_뒤집으면_구역_순서도_뒤집힌다() {
        let dir = temp_dir("three-reversed");
        let a = dir.join("a.hwp");
        let b = dir.join("b.hwp");
        let c = dir.join("c.hwp");
        let out = dir.join("out.hwp");
        write_generated_hwp(
            &a,
            "문서 A
",
        );
        write_generated_hwp(
            &b,
            "문서 B
",
        );
        write_generated_hwp(
            &c,
            "문서 C
",
        );

        run(&[c, b, a], &out, false, None, PasswordArgs::default()).unwrap();

        let merged = load_document(&out).unwrap();
        let texts = section_texts(&merged);
        assert_eq!(texts.len(), 3);
        assert!(texts[0].contains("문서 C"));
        assert!(texts[1].contains("문서 B"));
        assert!(texts[2].contains("문서 A"));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn 같은_경로를_두_번_넘기면_구역_두_개가_같은_본문을_담는다() {
        let dir = temp_dir("duplicate-path");
        let a = dir.join("a.hwp");
        let out = dir.join("out.hwp");
        write_generated_hwp(
            &a,
            "같은 문서
",
        );

        run(&[a.clone(), a], &out, false, None, PasswordArgs::default()).unwrap();

        let merged = load_document(&out).unwrap();
        let texts = section_texts(&merged);
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0], texts[1]);

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn 동일한_마크다운으로_만든_두_입력도_구역_두_개로_유지된다() {
        // Adjacency probe (FLOW-01, resolved by D-02): byte-equal inputs
        // still concatenate into two Sections — they never fuse.
        let dir = temp_dir("adjacency");
        let a = dir.join("a.hwp");
        let b = dir.join("b.hwp");
        let out = dir.join("out.hwp");
        write_generated_hwp(
            &a,
            "같은 내용
",
        );
        write_generated_hwp(
            &b,
            "같은 내용
",
        );

        run(&[a, b], &out, false, None, PasswordArgs::default()).unwrap();

        let merged = load_document(&out).unwrap();
        assert_eq!(merged.sections.len(), 2);
        let texts = section_texts(&merged);
        assert_eq!(texts[0], texts[1]);

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn 입력이_하나면_clap_파싱이_실패한다() {
        // FLOW-01 `empty` probe (open assumption A-01): num_args = 2.. makes
        // fewer than two inputs a clap usage error (exit 2), not an empty
        // output file. End-to-end exit-code/no-file-written behavior was
        // verified manually against the built binary (plan 03-01 checkpoint).
        use clap::Parser as _;
        use hwp_cli::cli::Cli;
        let result = Cli::try_parse_from(["hwp", "merge", "a.hwp", "-o", "out.hwp"]);
        assert!(result.is_err());
    }

    #[test]
    fn 입력이_둘이면_clap_파싱이_성공한다() {
        use clap::Parser as _;
        use hwp_cli::cli::Cli;
        let result = Cli::try_parse_from(["hwp", "merge", "a.hwp", "b.hwp", "-o", "out.hwp"]);
        assert!(result.is_ok());
    }

    #[test]
    fn 구역이_없는_병합_결과는_거부된다() {
        let empty = hwp_model::Document::default();
        assert!(reject_empty_sections(&empty).is_err());
    }

    #[test]
    fn 구역이_있으면_통과() {
        let doc = hwp_convert::from_markdown(
            "x
",
        );
        assert!(reject_empty_sections(&doc).is_ok());
    }

    #[test]
    fn 입력_누적_크기가_한도를_넘으면_거부() {
        let error = accumulate_input_reservation(MAX_MERGE_INPUT_BYTES, 1)
            .unwrap_err()
            .to_string();
        assert!(error.contains(&MAX_MERGE_INPUT_BYTES.to_string()));
    }

    #[test]
    fn 입력_누적_크기가_한도_이내면_허용() {
        assert_eq!(
            accumulate_input_reservation(MAX_MERGE_INPUT_BYTES - 1, 1).unwrap(),
            MAX_MERGE_INPUT_BYTES
        );
    }

    #[test]
    fn 표준입력_하이픈은_거부된다() {
        let dir = temp_dir("stdin-refused");
        let b = dir.join("b.hwp");
        let out = dir.join("out.hwp");
        write_generated_hwp(
            &b, "B
",
        );

        let result = run(
            &[PathBuf::from("-"), b],
            &out,
            false,
            None,
            PasswordArgs::default(),
        );
        assert!(result.is_err());
        assert!(!out.exists());

        std::fs::remove_dir_all(dir).unwrap();
    }
}
