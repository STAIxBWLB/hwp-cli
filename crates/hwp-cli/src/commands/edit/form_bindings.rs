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
            let mut report = write_output(prepared.document(), stage, request.plan.repeats.iter().any(|repeat| repeat.copies > 1), format, Some((snapshot, &original)))
                .map_err(|_| anyhow::anyhow!("바인딩 결과를 기존 native writer로 만들 수 없습니다. 지원하지 않는 개체/보존 항목을 validate로 확인하고 다른 대상을 선택하세요"))?;
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
            super::verify_output(stage, Some(prepared.document())).map_err(|_| anyhow::anyhow!(
                "바인딩 결과의 native 재읽기/의미 검증이 실패했습니다. 출력을 게시하지 않았습니다. 원본 validate 결과와 지원 대상/서식 보존 범위를 확인하세요"))?;
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
    let count = preview.changes.len();
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
        cloned_profiles: preview.copies.iter().filter(|copy| copy.copy != 0).count(),
        native_warning_count: preview.native_warnings.len(),
        semantic_budget: "selected_text_and_declared_clones_only",
        render_font_native: "unverified",
    }
}
