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
    for input in inputs {
        source_formats.push(crate::format::detect(input)?);
        documents.push(load_document_with_options(input, &options).map_err(anyhow::Error::new)?);
    }

    let outcome = hwp_convert::document_merge::merge_documents(&documents)
        .map_err(|error| anyhow::anyhow!("병합 실패: {error}"))?;
    let merged = outcome.document;

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
        // strict_drop_failure_preserves_existing_destination: force a loss
        // directly against write_validated so the test does not depend on
        // document_merge actually producing a loss event yet (the general
        // graft path that can lose something lands in plan 03-02).
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
}
