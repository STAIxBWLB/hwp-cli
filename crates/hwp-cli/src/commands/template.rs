//! `hwp template` - bounded TemplateSpec/Data v1 expansion and native output.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use hwp_cli::cli::SpecFormatArg;
use hwp_cli::document_spec::SpecInputFormat;
use hwp_cli::template_spec::{
    self, ExpandedOutput, ExpandedTemplate, TemplateError, TemplateMode, TemplateReport,
    TemplateVersion, ValidationStatus,
};
use sha2::{Digest as _, Sha256};

use crate::commands::output::SnapshotOutputMode;

struct InputHashes {
    template: String,
    data: String,
}

pub fn run(
    template_path: &Path,
    data_path: &Path,
    output: &Path,
    template_format: Option<SpecFormatArg>,
    data_format: Option<SpecFormatArg>,
    dry_run: bool,
    print_report: bool,
) -> anyhow::Result<()> {
    let template_input = read_bounded(
        template_path,
        template_spec::MAX_TEMPLATE_BYTES,
        "TemplateSpec",
    )?;
    let data_input = read_bounded(data_path, template_spec::MAX_DATA_BYTES, "TemplateData")?;
    let template_format = template_format
        .map(spec_format)
        .map(Ok)
        .unwrap_or_else(|| template_spec::infer_input_format(template_path))
        .map_err(template_error)?;
    let data_format = data_format
        .map(spec_format)
        .map(Ok)
        .unwrap_or_else(|| template_spec::infer_input_format(data_path))
        .map_err(template_error)?;
    let base_dir = template_path.parent().unwrap_or_else(|| Path::new("."));
    let sources = [template_path.to_path_buf(), data_path.to_path_buf()];
    let report = execute_text(
        &template_input,
        template_format,
        &data_input,
        data_format,
        base_dir,
        output,
        dry_run,
        &sources,
    )?;
    if print_report || dry_run {
        println!("{}", serialize_report(&report)?);
    } else {
        eprintln!(
            "템플릿 생성 완료: {} ({:?}, region {})",
            output.display(),
            report.mode,
            report.expansion.regions.len()
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn execute_text(
    template_input: &str,
    template_format: SpecInputFormat,
    data_input: &str,
    data_format: SpecInputFormat,
    base_dir: &Path,
    output: &Path,
    dry_run: bool,
    source_paths: &[PathBuf],
) -> anyhow::Result<TemplateReport> {
    execute_text_with_fonts(
        template_input,
        template_format,
        data_input,
        data_format,
        base_dir,
        output,
        dry_run,
        source_paths,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_text_with_fonts(
    template_input: &str,
    template_format: SpecInputFormat,
    data_input: &str,
    data_format: SpecInputFormat,
    base_dir: &Path,
    output: &Path,
    dry_run: bool,
    source_paths: &[PathBuf],
    font_files: Option<&[PathBuf]>,
) -> anyhow::Result<TemplateReport> {
    let template =
        template_spec::parse_template(template_input, template_format).map_err(template_error)?;
    let data = template_spec::parse_data(data_input, data_format).map_err(template_error)?;
    let expanded =
        template_spec::expand_template(&template, &data, base_dir).map_err(template_error)?;
    let mut immutable_inputs = source_paths
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    match &expanded.output {
        ExpandedOutput::Reference { path, .. }
        | ExpandedOutput::ReferenceRegenerate { path, .. } => immutable_inputs.push(path),
        ExpandedOutput::Compose(_) => {}
    }
    crate::commands::output::reject_output_aliases(output, &immutable_inputs)?;

    let input_hashes = InputHashes {
        template: template_spec::sha256_hex(template_input.as_bytes()),
        data: template_spec::sha256_hex(data_input.as_bytes()),
    };
    execute_expanded(
        expanded,
        &template,
        base_dir,
        output,
        dry_run,
        input_hashes,
        font_files,
    )
}

fn execute_expanded(
    expanded: ExpandedTemplate,
    template: &template_spec::TemplateSpec,
    base_dir: &Path,
    output: &Path,
    dry_run: bool,
    input_hashes: InputHashes,
    font_files: Option<&[PathBuf]>,
) -> anyhow::Result<TemplateReport> {
    let mode = expanded.mode.clone();
    let plan = expanded.plan.clone();
    ensure_report_budget(&plan)?;
    let mut reference_hash = None;
    let mut compose_report = None;
    let (semantic_validation, package_validation) = match &expanded.output {
        ExpandedOutput::Compose(document) => {
            let report = crate::commands::compose::execute_spec_with_source_and_fonts(
                document, base_dir, output, dry_run, false, None, font_files,
            )
            .map_err(|_| sanitized_downstream("compose_rejected", "/source/document"))?;
            let validation = if dry_run {
                ValidationStatus::NotRun
            } else {
                ValidationStatus::Passed
            };
            compose_report = Some(report);
            (validation, validation)
        }
        ExpandedOutput::Reference {
            path,
            placeholders,
            fields,
        } => {
            require_hwpx_output(output)?;
            recheck_reference(path, base_dir)?;
            let output_mode = if dry_run {
                SnapshotOutputMode::ValidateOnly
            } else {
                SnapshotOutputMode::Publish
            };
            let (hash, _) = crate::commands::output::write_with_private_input_snapshot(
                output,
                path,
                hwpx::NATIVE_PACKAGE_LIMITS.max_total_uncompressed_bytes,
                output_mode,
                |snapshot, staged, _| {
                    hwpx::patch::fill_template_values(snapshot, staged, placeholders, fields)
                        .map_err(|_| {
                            sanitized_downstream("reference_fill_rejected", "/source/bindings")
                        })
                },
                |staged, counts| {
                    validate_reference_counts(template, counts).map_err(template_error)?;
                    verify_reference_output(staged).map_err(|_| {
                        sanitized_downstream("reference_fill_rejected", "/source/bindings")
                    })
                },
            )?;
            reference_hash = Some(hash);
            (ValidationStatus::Passed, ValidationStatus::Passed)
        }
        ExpandedOutput::ReferenceRegenerate {
            path,
            strict_unsupported_objects,
            document,
        } => {
            if !strict_unsupported_objects {
                return Err(template_error(TemplateError::single(
                    "strict_gate_required",
                    "/source/strict_unsupported_objects",
                    "reference regeneration requires the strict gate",
                )));
            }
            recheck_reference(path, base_dir)?;
            let output_mode = if dry_run {
                SnapshotOutputMode::PlanOnly
            } else {
                SnapshotOutputMode::Publish
            };
            let (hash, mut report) = crate::commands::output::write_with_private_input_snapshot(
                output,
                path,
                hwpx::NATIVE_PACKAGE_LIMITS.max_total_uncompressed_bytes,
                output_mode,
                |snapshot, staged, _| {
                    strict_reference_gate(snapshot, staged).map_err(|_| {
                        sanitized_downstream(
                            "unsupported_reference_object",
                            "/source/strict_unsupported_objects",
                        )
                    })?;
                    crate::commands::compose::execute_spec_with_source_and_fonts(
                        document, base_dir, staged, dry_run, false, None, font_files,
                    )
                    .map_err(|_| sanitized_downstream("compose_rejected", "/source/document"))
                },
                |_, _| Ok(()),
            )?;
            reference_hash = Some(hash);
            report.output = output.display().to_string();
            let validation = if dry_run {
                ValidationStatus::NotRun
            } else {
                ValidationStatus::Passed
            };
            compose_report = Some(report);
            (validation, validation)
        }
    };

    let output_hash = if dry_run {
        None
    } else {
        Some(hash_file(output)?)
    };
    let (changed_regions, generated_regions) = match mode {
        TemplateMode::ReferencePackagePreserving => (plan.regions.clone(), Vec::new()),
        TemplateMode::Compose | TemplateMode::ReferenceRegenerate => {
            (Vec::new(), plan.regions.clone())
        }
    };
    Ok(TemplateReport {
        schema_version: TemplateVersion::V1,
        data_schema_version: TemplateVersion::V1,
        output: output.display().to_string(),
        dry_run,
        deterministic: true,
        mode,
        template_sha256: input_hashes.template,
        data_sha256: input_hashes.data,
        reference_sha256: reference_hash,
        output_sha256: output_hash,
        provided_variables: expanded.provided_variables,
        defaulted_variables: expanded.defaulted_variables,
        expansion: plan,
        changed_regions,
        generated_regions,
        unsupported: Vec::new(),
        fallback: Vec::new(),
        dropped: Vec::new(),
        template_validation: ValidationStatus::Passed,
        data_validation: ValidationStatus::Passed,
        semantic_validation,
        package_validation,
        compose: compose_report,
    })
}

pub(crate) fn serialize_report(report: &TemplateReport) -> anyhow::Result<String> {
    let serialized = serde_json::to_string_pretty(report)?;
    if serialized.len() > template_spec::MAX_REPORT_BYTES {
        return Err(template_error(TemplateError::single(
            "report_limit_exceeded",
            "/expansion",
            "template report exceeds the response byte budget",
        )));
    }
    Ok(serialized)
}

fn ensure_report_budget(plan: &template_spec::ExpansionPlan) -> anyhow::Result<()> {
    const FIXED_REPORT_RESERVE: usize = 1024 * 1024;
    let compact = serde_json::to_vec(plan)?;
    let estimated = compact
        .len()
        .checked_mul(4)
        .and_then(|bytes| bytes.checked_add(FIXED_REPORT_RESERVE))
        .unwrap_or(usize::MAX);
    if estimated > template_spec::MAX_REPORT_BYTES {
        return Err(template_error(TemplateError::single(
            "report_limit_exceeded",
            "/expansion",
            "template report exceeds the response byte budget",
        )));
    }
    Ok(())
}

fn verify_reference_output(path: &Path) -> anyhow::Result<()> {
    let read = hwpx::read_document(path).context("reference output semantic read failed")?;
    crate::commands::reject_drop_warnings("template", &read.warnings)?;
    let validation = crate::commands::validate::validate_json(path);
    if validation.get("valid").and_then(serde_json::Value::as_bool) != Some(true) {
        anyhow::bail!("reference output package validation failed");
    }
    Ok(())
}

fn validate_reference_counts(
    template: &template_spec::TemplateSpec,
    counts: &hwpx::patch::TemplateFillCounts,
) -> Result<(), TemplateError> {
    let template_spec::TemplateSource::ReferenceHwpx { bindings, .. } = &template.source else {
        return Ok(());
    };
    let mut issues = Vec::new();
    for (index, binding) in bindings.iter().enumerate() {
        let count = match binding.target {
            template_spec::ReferenceTarget::Placeholder => {
                counts.placeholders.get(&binding.name).copied().unwrap_or(0)
            }
            template_spec::ReferenceTarget::Field => {
                counts.fields.get(&binding.name).copied().unwrap_or(0)
            }
        };
        let valid = match binding.target {
            template_spec::ReferenceTarget::Placeholder => count > 0,
            template_spec::ReferenceTarget::Field => count == 1,
        };
        if !valid {
            issues.push(template_spec::TemplateIssue {
                code: "reference_target_missing".to_string(),
                pointer: format!("/source/bindings/{index}"),
                message: "requested reference target was not found exactly as required".to_string(),
            });
        }
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(TemplateError::new(issues))
    }
}

fn strict_reference_gate(input: &Path, output: &Path) -> anyhow::Result<()> {
    let read = hwpx::read_document(input).context("reference read failed")?;
    crate::commands::reject_drop_warnings("template reference read", &read.warnings)?;
    crate::commands::output::validate_without_publish(
        output,
        Some(input),
        |staged| Ok(hwpx::write_document(&read.document, staged)?),
        |staged, warnings| {
            crate::commands::reject_drop_warnings("template reference regeneration", warnings)?;
            crate::commands::edit::verify_document(staged, &read.document)
                .context("reference strict semantic gate failed")
        },
    )?;
    Ok(())
}

fn recheck_reference(path: &Path, base_dir: &Path) -> anyhow::Result<()> {
    let base = std::fs::canonicalize(base_dir).context("template base directory recheck failed")?;
    let metadata = std::fs::symlink_metadata(path).context("reference recheck failed")?;
    let canonical = std::fs::canonicalize(path).context("reference canonical recheck failed")?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || !canonical.starts_with(base)
    {
        return Err(sanitized_downstream("invalid_reference", "/source/path"));
    }
    Ok(())
}

fn require_hwpx_output(output: &Path) -> anyhow::Result<()> {
    if output.extension().and_then(|extension| extension.to_str()) == Some("hwpx") {
        Ok(())
    } else {
        Err(template_error(TemplateError::single(
            "invalid_output",
            "/output",
            "reference package-preserving mode requires a .hwpx output",
        )))
    }
}

pub(crate) fn read_bounded(path: &Path, limit: usize, label: &str) -> anyhow::Result<String> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("{label}를 열 수 없습니다: {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("{label}를 읽을 수 없습니다: {}", path.display()))?;
    if bytes.len() > limit {
        return Err(template_error(TemplateError::single(
            "limit_exceeded",
            "",
            format!("{label} exceeds {limit} bytes"),
        )));
    }
    String::from_utf8(bytes).map_err(|_| {
        template_error(TemplateError::single(
            "invalid_encoding",
            "",
            format!("{label} must be UTF-8"),
        ))
    })
}

fn hash_file(path: &Path) -> anyhow::Result<String> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("hash input open failed: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn spec_format(format: SpecFormatArg) -> SpecInputFormat {
    match format {
        SpecFormatArg::Json => SpecInputFormat::Json,
        SpecFormatArg::Yaml => SpecInputFormat::Yaml,
    }
}

fn sanitized_downstream(code: &str, pointer: &str) -> anyhow::Error {
    template_error(TemplateError::single(
        code,
        pointer,
        "native processing rejected the request; data-derived details are redacted",
    ))
}

fn template_error(error: TemplateError) -> anyhow::Error {
    let envelope = serde_json::json!({
        "error": "template_spec",
        "issues": error.issues(),
        "truncated": error.truncated(),
        "total_or_at_least": error.total_or_at_least(),
    });
    let serialized = serde_json::to_string_pretty(&envelope)
        .unwrap_or_else(|_| "template_spec validation failed".to_string());
    if serialized.len() <= template_spec::MAX_DIAGNOSTIC_BYTES {
        return anyhow::anyhow!(serialized);
    }
    anyhow::anyhow!(
        "{}",
        serde_json::json!({
            "error": "template_spec",
            "issues": [{
                "code": "diagnostics_truncated",
                "pointer": "",
                "message": "diagnostics exceeded the response byte budget"
            }],
            "truncated": true,
            "total_or_at_least": error.total_or_at_least()
        })
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitized_downstream_never_carries_source_error_or_secret() {
        let error = sanitized_downstream("compose_rejected", "/source/document");
        let display = format!("{error:#}");
        assert!(display.contains("compose_rejected"));
        assert!(!display.contains("TOPSECRET_CANARY"));
    }
}
