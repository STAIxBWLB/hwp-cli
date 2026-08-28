//! `hwp merge` — combine several HWP5/HWPX inputs into one output (GM-3, FLOW-01).
//!
//! One thin path: load every input with the existing password-aware loader, merge
//! them through `hwp_convert::document_merge::merge_documents` (D-02: one Section
//! per input, concatenated in argument order), pick the writer from `--output`'s
//! extension (D-03), and publish through the existing staged-write-then-verify
//! transaction (`crate::commands::output::write_validated`).

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use hwp_cli::cli::PasswordArgs;

use crate::commands::cat::{
    LoadOptions, load_document, load_document_with_options, resolve_password_args,
};

/// Output writer chosen from `--output`'s extension (D-03).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeFormat {
    Hwp,
    Hwpx,
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

    let input_refs: Vec<&Path> = inputs.iter().map(PathBuf::as_path).collect();
    crate::commands::output::reject_output_aliases(output, &input_refs)?;

    let mut documents = Vec::with_capacity(inputs.len());
    for input in inputs {
        documents.push(load_document_with_options(input, &options).map_err(anyhow::Error::new)?);
    }

    let outcome = hwp_convert::document_merge::merge_documents(&documents)
        .map_err(|error| anyhow::anyhow!("병합 실패: {error}"))?;
    let merged = outcome.document;

    let write_staged = |staged: &Path| -> anyhow::Result<hwp_model::WriteReport> {
        let report = match format {
            MergeFormat::Hwp => crate::commands::convert::write_hwp(&merged, staged, false)?,
            MergeFormat::Hwpx => hwpx::write::write_document_with_report_with(
                &merged,
                staged,
                &hwpx::write::HwpxWriteOptions {
                    preserve_linesegs: false,
                },
            )?,
        };
        Ok(report)
    };
    let verify_staged = |staged: &Path, _report: &hwp_model::WriteReport| -> anyhow::Result<()> {
        load_document(staged)
            .with_context(|| format!("병합 문서 재읽기 실패: {}", staged.display()))?;
        Ok(())
    };

    crate::commands::output::write_validated(output, None, write_staged, verify_staged)?;

    eprintln!("병합 완료: {}개 입력 → {}", inputs.len(), output.display());
    // strict/loss_report wiring lands in plan 03-01 Task 2.
    let _ = (strict, loss_report);
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
}
