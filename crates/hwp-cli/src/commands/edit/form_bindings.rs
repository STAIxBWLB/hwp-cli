//! Exclusive source-bound orchestration through the standard edit writer and verifier.

use std::path::{Path, PathBuf};

use hwp_convert::form::binding::contract::{BindingPlan, Values};
use hwp_convert::form::binding::{BindingPreview, digest, prepare_binding_plan};
use serde::{Deserialize, Serialize};

use super::{EditPlan, EditReport, OpOutcome, OpStatus, OutputFormat, load_document, write_output};
use crate::commands::output;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub plan: BindingPlan,
    pub values: Values,
    pub receipt: PathBuf,
    #[serde(default)]
    pub preview: Option<PathBuf>,
}

/// Content-free receipt; logical profile names and semantic binding keys stay in the plan.
#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    source_sha256: String,
    plan_sha256: String,
    values_sha256: String,
    output_sha256: String,
    published: bool,
    occurrence_ids: Vec<String>,
    targets: usize,
    cloned_profiles: usize,
    native_warning_count: usize,
    semantic_budget: &'static str,
    render_font_native: &'static str,
}

pub(super) fn execute(
    input: &Path,
    output: &Path,
    edit: &EditPlan,
    request: &Request,
) -> anyhow::Result<EditReport> {
    if edit.allow_partial {
        anyhow::bail!(
            "apply_form_bindings는 부분 적용을 허용하지 않습니다. --allow-partial을 제거하고 계획 전체를 검토하세요"
        );
    }
    if edit.report.is_some() {
        anyhow::bail!(
            "apply_form_bindings는 별도 --report와 함께 쓸 수 없습니다. 연산의 receipt와 선택적 preview 경로를 사용해 문서와 같은 atomic 출력 집합으로 게시하세요"
        );
    }
    let format = OutputFormat::from_path(output)?;
    if !format.supports_verify() {
        anyhow::bail!(
            "apply_form_bindings는 native HWP/HWPX 출력이 필요합니다. 출력 확장자를 .hwp 또는 .hwpx로 지정하세요"
        );
    }
    let mut destinations = vec![
        (output.to_owned(), !edit.dry_run),
        (request.receipt.clone(), true),
    ];
    if let Some(preview) = &request.preview {
        destinations.push((preview.clone(), true));
    }
    // Capture every destination baseline before any prepare/write work. Publication uses
    // those same snapshots and the existing atomic-set identity/rollback machinery.
    let (_, (preview, preservation)) = output::write_snapshot_output_set(
        input,
        &edit.ops_input.as_deref().into_iter().collect::<Vec<_>>(),
        64 * 1024 * 1024,
        &destinations,
        |snapshot, stages, hash| {
            let stage = &stages[0];
            let original = load_document(snapshot).map_err(|_| anyhow::anyhow!(
                "바인딩 원본 문서를 읽을 수 없습니다. 원본을 validate로 확인한 뒤 discovery를 다시 실행하세요"))?;
            if !matches!(
                (original.meta.source_format.as_str(), format),
                ("hwp5", OutputFormat::Hwp) | ("hwpx", OutputFormat::Hwpx)
            ) {
                anyhow::bail!(
                    "양식 바인딩은 원본과 같은 native 형식에 적용해야 합니다. 먼저 convert한 문서에서 discovery와 계획을 새로 만드세요"
                );
            }
            let prepared = prepare_binding_plan(&original, hash, &request.plan, &request.values)
                .map_err(anyhow::Error::msg)?;
            let mut report = write_bound_output(
                prepared.document(),
                stage,
                request.plan.repeats.iter().any(|repeat| repeat.copies > 1),
                format,
                Some((snapshot, &original)),
            )?;
            report.preservation.extend(
                crate::commands::preservation::inspect_same_format_container_with_binary_allowance(
                    snapshot,
                    stage,
                    crate::commands::preservation::intentional_removed_binary_assets(
                        &original,
                        prepared.document(),
                    ),
                )?,
            );
            crate::commands::reject_preservation_loss("apply_form_bindings", &report.preservation)?;
            verify_bound_output(stage, prepared.document())?;
            let bytes = std::fs::read(stage)?;
            let mut preview = prepared.preview;
            // write_output includes preview-render/font diagnostics. Keep original messages
            // only in the explicitly requested local preview, never default logs or receipts.
            preview.native_warnings = report.warnings;
            let receipt = receipt(&preview, &bytes, !edit.dry_run);
            std::fs::write(&stages[1], serde_json::to_vec_pretty(&receipt)?)?;
            if request.preview.is_some() {
                std::fs::write(&stages[2], serde_json::to_vec_pretty(&preview)?)?;
            }
            Ok((preview, report.preservation))
        },
    )?;
    let count = preview.changes.len() + cloned_profiles(&preview);
    Ok(EditReport {
        output: output.display().to_string(),
        applied: count,
        warnings: if preview.native_warnings.is_empty() {
            Vec::new()
        } else {
            vec![format!(
                "native_preview_or_writer_warning: {}건. 원문은 공개 로그에서 생략했습니다. preview 경로를 지정해 native_warnings를 확인하고 hwp render/certify로 렌더·폰트를 별도 검증하세요",
                preview.native_warnings.len()
            )]
        },
        preservation,
        ops: vec![OpOutcome {
            index: 0,
            op: "apply_form_bindings".into(),
            status: OpStatus::Applied,
            pieces_touched: count,
            changed: Vec::new(),
            changed_total: None,
            reason: None,
            missing_style: None,
        }],
        dry_run: edit.dry_run,
    })
}

fn write_bound_output(
    doc: &hwp_model::Document,
    output: &Path,
    structural: bool,
    format: OutputFormat,
    source: Option<(&Path, &hwp_model::Document)>,
) -> anyhow::Result<hwp_model::WriteReport> {
    write_output(doc, output, structural, format, source)
        .map_err(|error| native_failure(NativePhase::Write, &error))
}

fn verify_bound_output(output: &Path, expected: &hwp_model::Document) -> anyhow::Result<()> {
    super::verify_output(output, Some(expected))
        .map_err(|error| native_failure(NativePhase::Verify, &error))
}

enum NativePhase {
    Write,
    Verify,
}

/// Inspect types only. Never retain the original chain: the CLI formats every source, and
/// native errors may contain cell text, XML attributes or private paths.
fn native_failure(phase: NativePhase, error: &anyhow::Error) -> anyhow::Error {
    let io = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>());
    let (cause, next_step) = match phase {
        NativePhase::Verify
            if error
                .downcast_ref::<super::SemanticVerificationMismatch>()
                .is_some() =>
        {
            (
                "verify_semantic_mismatch",
                "원본을 validate로 확인하고 지원 대상/서식 보존 범위를 줄인 뒤 다시 실행하세요",
            )
        }
        NativePhase::Verify if io.is_some() => (
            "verify_readback_io",
            "후보 파일의 읽기 권한·출력 경로·디스크 상태를 확인한 뒤 다시 실행하세요",
        ),
        NativePhase::Verify => (
            "verify_readback_package",
            "원본을 validate로 검사하고 손상된 native 패키지를 복구하거나 지원 대상을 선택하세요",
        ),
        NativePhase::Write => match io.map(std::io::Error::kind) {
            Some(std::io::ErrorKind::PermissionDenied) => (
                "native_write_io_permission",
                "출력 경로의 디렉터리 쓰기 권한을 확인하거나 쓰기 가능한 경로를 지정하세요",
            ),
            Some(std::io::ErrorKind::StorageFull) => (
                "native_write_io_space",
                "출력 디스크의 여유 공간을 확보하거나 다른 출력 경로를 지정하세요",
            ),
            Some(
                std::io::ErrorKind::IsADirectory
                | std::io::ErrorKind::NotADirectory
                | std::io::ErrorKind::NotFound,
            ) => (
                "native_write_io_path",
                "출력 경로가 파일이며 상위 디렉터리가 존재하는지 확인한 뒤 다시 실행하세요",
            ),
            Some(_) => (
                "native_write_io",
                "출력 경로·디렉터리 쓰기 권한·디스크 여유 공간을 확인한 뒤 다시 실행하세요",
            ),
            None => (
                "native_write_package",
                "원본을 validate로 검사하고 지원하지 않는 개체/보존 항목을 복구하거나 다른 대상을 선택하세요",
            ),
        },
    };
    anyhow::anyhow!(
        "바인딩 native 처리 실패 ({cause}). 출력을 게시하지 않았습니다. {next_step} (원문 생략)"
    )
}

fn cloned_profiles(preview: &BindingPreview) -> usize {
    preview.copies.iter().filter(|copy| copy.copy != 0).count()
}

fn receipt(preview: &BindingPreview, bytes: &[u8], published: bool) -> Receipt {
    Receipt {
        version: 1,
        source_sha256: preview.source_sha256.clone(),
        plan_sha256: preview.plan_sha256.clone(),
        values_sha256: preview.values_sha256.clone(),
        output_sha256: digest(bytes),
        published,
        occurrence_ids: preview
            .changes
            .iter()
            .map(|change| change.occurrence_id.clone())
            .collect(),
        targets: preview.changes.len(),
        cloned_profiles: cloned_profiles(preview),
        native_warning_count: preview.native_warnings.len(),
        semantic_budget: "selected_text_and_declared_clones_only",
        render_font_native: "unverified",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Workspace(PathBuf);
    impl Workspace {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "hwp-binding-failure-{}-{label}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn binding_native_write_failure_names_io_cause_without_content() {
        let workspace = Workspace::new("write");
        let output = workspace.0.join("PRIVATE_VALUE.hwpx");
        std::fs::create_dir(&output).unwrap();
        let doc = hwp_convert::from_markdown("PRIVATE_VALUE");
        let error = write_bound_output(&doc, &output, false, OutputFormat::Hwpx, None).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("native_write_io"),
            "write I/O must have its own cause: {message}"
        );
        assert!(
            message.contains("경로"),
            "write failure needs a path remedy: {message}"
        );
        assert!(!message.contains("PRIVATE_VALUE"));
    }

    #[test]
    fn binding_native_verify_failure_names_semantic_cause_without_content() {
        let workspace = Workspace::new("verify");
        let output = workspace.0.join("PRIVATE_VALUE.hwpx");
        let actual = hwp_convert::from_markdown("original");
        hwpx::write_document(&actual, &output).unwrap();
        let expected = hwp_convert::from_markdown("PRIVATE_VALUE");
        let error = verify_bound_output(&output, &expected).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("verify_semantic_mismatch"),
            "semantic mismatch needs its own cause: {message}"
        );
        assert!(
            message.contains("validate"),
            "semantic failure needs a repair step: {message}"
        );
        assert!(!message.contains("PRIVATE_VALUE"));
    }

    #[test]
    fn binding_native_verify_readback_failures_are_distinct_and_private() {
        let workspace = Workspace::new("readback");
        let output = workspace.0.join("PRIVATE_VALUE.hwpx");
        let expected = hwp_convert::from_markdown("PRIVATE_VALUE");
        let missing = verify_bound_output(&output, &expected).unwrap_err();
        assert!(missing.to_string().contains("verify_readback_io"));
        assert!(!format!("{missing:#}").contains("PRIVATE_VALUE"));
        std::fs::write(&output, b"PRIVATE_VALUE invalid package").unwrap();
        let corrupt = verify_bound_output(&output, &expected).unwrap_err();
        assert!(corrupt.to_string().contains("verify_readback_package"));
        assert!(!format!("{corrupt:#}").contains("PRIVATE_VALUE"));
    }

    #[test]
    fn binding_native_error_classification_discards_private_chains() {
        for (error, expected) in [
            (
                anyhow::Error::new(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "PRIVATE_VALUE",
                )),
                "native_write_io_permission",
            ),
            (
                anyhow::Error::new(std::io::Error::new(
                    std::io::ErrorKind::StorageFull,
                    "PRIVATE_VALUE",
                )),
                "native_write_io_space",
            ),
            (
                anyhow::Error::new(hwpx::HwpxError::PackageIntegrity("PRIVATE_VALUE".into())),
                "native_write_package",
            ),
        ] {
            let error = native_failure(NativePhase::Write, &error.context("PRIVATE_CONTEXT"));
            assert!(error.to_string().contains(expected));
            assert_eq!(
                error.chain().count(),
                1,
                "redacted errors must not retain private sources"
            );
            let rendered = format!("{error:#}");
            assert!(!rendered.contains("PRIVATE_VALUE"));
            assert!(!rendered.contains("PRIVATE_CONTEXT"));
        }
    }
}
