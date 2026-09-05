//! Versioned native HWP/HWPX certification contract.
//!
//! This is deliberately a conservative, local renderer/import contract. A `not_detected`
//! diagnostic means only that this implementation's bounded algorithm did not observe a problem;
//! it is not evidence of Hancom rendering parity.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub const POLICY_SCHEMA_VERSION: &str = "1.0";
pub const REPORT_SCHEMA_VERSION: &str = "1.0";
pub const MAX_INPUT_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_POLICY_BYTES: u64 = 1024 * 1024;
pub const MAX_SELECTED_PAGES: usize = 256;
pub const MAX_POLICY_NAMES: usize = 512;
pub const MAX_DEFINITION_INDEXES: usize = 4096;
pub const MAX_RULE_COUNT: usize = 1_000_000;
pub const MAX_PAGE_NUMBER: usize = hwp_render::layout::CERTIFICATION_MAX_PAGES;
pub const MAX_FONT_FILES: usize = 128;
pub const MAX_FONT_FILE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_FONT_TOTAL_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_REPORT_ARTIFACTS: usize = MAX_SELECTED_PAGES + 1;
pub const MAX_MANIFEST_FILES: usize = MAX_REPORT_ARTIFACTS + 1;
pub const MAX_PUBLISHED_TREE_ENTRIES: usize = MAX_MANIFEST_FILES + 1;
/// Internal publisher guard with one entry of implementation slack. Public
/// reports still use the exact 257/258/259 limits above.
pub const MAX_ARTIFACTS: usize = 260;
pub const MAX_ARTIFACT_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
/// Maximum UTF-16 path expansion beyond the requested report destination for
/// the private certification workspace and its deepest fixed artifact path.
pub const WINDOWS_CERTIFICATION_TREE_OVERHEAD_UTF16: usize = 101;
pub const ORACLE_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_ORACLE_RESULT_BYTES: u64 = 64 * 1024;
/// Bounded read limit for the optional preservation/hancom_open evidence artifacts.
const MAX_EVIDENCE_ARTIFACT_BYTES: u64 = 64 * 1024;
/// Mirrors `preservation-report-v1` `events.maxItems`.
const MAX_PRESERVATION_EVENTS: usize = 1000;
/// Per-event and per-code loss bound shared with the report schema's `lossCount`.
const MAX_PRESERVATION_LOSS_COUNT: usize = 1_000_000;
const MAX_LOG_BYTES_RECORDED: u64 = 64 * 1024;
const MAX_PARSE_XML_DEPTH: usize = 128;
const MAX_PARSE_XML_NODES: usize = 1_000_000;
const MAX_PARSE_RECORD_DEPTH: usize = 128;
const MAX_PARSE_RECORDS: usize = 200_000;
const MAX_PARSE_RECORD_STREAM_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PARSE_RECORD_TOTAL_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PARSE_HWP5_STREAMS: usize = 4_096;
const MAX_PARSE_HWP5_STREAM_NAME_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertificationPolicy {
    pub schema_version: String,
    #[serde(default)]
    pub document: DocumentPolicy,
    pub render: RenderPolicy,
    #[serde(default)]
    pub oracle: OraclePolicy,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentPolicy {
    #[serde(default)]
    pub fonts: FontPolicy,
    #[serde(default)]
    pub defined_styles: NamedSetPolicy,
    #[serde(default)]
    pub used_styles: NamedSetPolicy,
    #[serde(default)]
    pub numbering: NumberingPolicy,
    #[serde(default)]
    pub tables: CountPolicy,
    #[serde(default)]
    pub links: LinkPolicy,
    #[serde(default)]
    pub metadata: MetadataPolicy,
    #[serde(default)]
    pub macros: PresencePolicy,
    #[serde(default)]
    pub external_references: PresencePolicy,
    #[serde(default)]
    pub accessibility: AccessibilityPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preservation: Option<PreservationPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hancom_open: Option<HancomOpenPolicy>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FontPolicy {
    /// The only font files available to the certification renderer. Paths are relative to the
    /// policy file and each file is copied into a private snapshot after SHA-256 verification.
    #[serde(default)]
    pub manifest: Vec<FilePin>,
    #[serde(default)]
    pub allowed_requested: Vec<String>,
    #[serde(default)]
    pub required_requested: Vec<String>,
    #[serde(default = "default_true")]
    pub forbid_substitution: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilePin {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedSetPolicy {
    #[serde(default)]
    pub allowed: Vec<String>,
    #[serde(default)]
    pub required: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumberingPolicy {
    #[serde(default)]
    pub allowed_definition_indexes: Vec<usize>,
    #[serde(default)]
    pub required_definition_indexes: Vec<usize>,
    #[serde(default)]
    pub definitions: CountPolicy,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountPolicy {
    #[serde(default)]
    pub min: Option<usize>,
    #[serde(default)]
    pub max: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkPolicy {
    #[serde(default)]
    pub allowed_schemes: Vec<String>,
    #[serde(default)]
    pub count: CountPolicy,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataPolicy {
    #[serde(default)]
    pub allowed_fields: Vec<MetadataField>,
    #[serde(default)]
    pub required_fields: Vec<MetadataField>,
    #[serde(default)]
    pub forbidden_fields: Vec<MetadataField>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataField {
    Title,
    Author,
    Subject,
    Keywords,
    Description,
    LastSavedBy,
    CreateTime,
    ModifyTime,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresencePolicy {
    #[default]
    Allow,
    Deny,
    Require,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessibilityPolicy {
    #[serde(default)]
    pub require_picture_descriptions: bool,
    #[serde(default)]
    pub require_shape_descriptions: bool,
}

/// Optional evidence check against a `preservation-report-v1` artifact produced by an earlier
/// conversion run. The artifact carries only stable loss codes and counts, never content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreservationPolicy {
    /// Path to the preservation report JSON, relative to the policy file.
    pub report: String,
    /// Maximum tolerated loss total (sum of event counts). Defaults to zero.
    #[serde(default)]
    pub max_loss_codes: usize,
}

/// Optional evidence check against a `hancom-verification-receipt-v1` artifact attesting that
/// Hancom Office opened the document without repair or damage warnings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HancomOpenPolicy {
    /// Path to the verification receipt JSON, relative to the policy file.
    pub receipt: String,
    /// When true (default) the receipt result must be `pass`.
    #[serde(default = "default_true")]
    pub require_pass: bool,
    /// When true, the receipt must carry `artifact_sha256` and it must equal the certified
    /// input's hash. The field stays optional at the schema level; a policy that binds one
    /// receipt to one artifact turns this on so an unbound receipt cannot stand in for it.
    /// Defaults to false, so policies written before this option keep their behaviour.
    #[serde(default)]
    pub require_artifact_sha256: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenderPolicy {
    #[serde(default = "default_dpi")]
    pub dpi: f32,
    #[serde(default)]
    pub pages: PageSelection,
    #[serde(default)]
    pub page_count: PageCountPolicy,
    #[serde(default)]
    pub allowed_blank_pages: Vec<usize>,
    #[serde(default = "default_true")]
    pub fail_on_outside_page_bounds: bool,
    #[serde(default = "default_true")]
    pub fail_on_potential_collision: bool,
    #[serde(default = "default_true")]
    pub fail_on_unresolved_fields: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PageSelection {
    All(String),
    Selected(Vec<usize>),
}

impl Default for PageSelection {
    fn default() -> Self {
        Self::All("all".to_string())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageCountPolicy {
    #[serde(default)]
    pub exact: Option<usize>,
    #[serde(default)]
    pub min: Option<usize>,
    #[serde(default)]
    pub max: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OraclePolicy {
    #[serde(default)]
    pub mode: OracleMode,
    #[serde(default)]
    pub configuration: Option<OracleConfiguration>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OracleMode {
    #[default]
    Disabled,
    Optional,
    Required,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OracleConfiguration {
    pub runtime: RuntimePin,
    pub libreoffice: SoftwarePin,
    pub extension: ExtensionPin,
    pub image: ImagePin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimePin {
    pub version: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoftwarePin {
    pub version: String,
    pub executable_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionPin {
    pub version: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImagePin {
    pub digest: String,
}

fn default_true() -> bool {
    true
}

fn default_dpi() -> f32 {
    96.0
}

#[derive(Debug, Clone, Serialize)]
pub struct CertificationReport {
    pub schema_version: &'static str,
    pub contract: &'static str,
    pub overall: OverallStatus,
    pub scope: &'static str,
    pub input: InputReport,
    pub policy_sha256: String,
    pub checks: CheckSet,
    pub render: RenderReport,
    pub oracle: OracleReport,
    pub artifacts: Vec<ArtifactReport>,
    pub limitations: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverallStatus {
    Passed,
    Failed,
    Partial,
}

#[derive(Debug, Clone, Serialize)]
pub struct InputReport {
    pub format: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckSet {
    pub package: CheckResult,
    pub repeat_import_consistency: CheckResult,
    pub rules: Vec<RuleResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preservation: Option<PreservationCheckReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hancom_open: Option<HancomOpenCheckReport>,
}

/// Outcome of the optional preservation evidence check. Content-free: only aggregated loss
/// codes and counts, echoing the referenced `preservation-report-v1` artifact.
#[derive(Debug, Clone, Serialize)]
pub struct PreservationCheckReport {
    pub status: CheckStatus,
    pub reason_codes: Vec<String>,
    pub loss_code_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loss_codes: Option<BTreeMap<String, usize>>,
}

/// Outcome of the optional Hancom open evidence check. Echoes only the receipt's content-free
/// attestation fields; a missing or invalid receipt fails closed with no echo fields.
#[derive(Debug, Clone, Serialize)]
pub struct HancomOpenCheckReport {
    pub status: CheckStatus,
    pub reason_codes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verifier: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub status: CheckStatus,
    pub reason_codes: Vec<String>,
    pub issue_count: usize,
    pub issue_sha256: String,
}

impl CheckResult {
    fn with_issue_digest(
        status: CheckStatus,
        reason_codes: Vec<String>,
        issue_count: usize,
        issue_sha256: String,
    ) -> Self {
        let passed = status == CheckStatus::Passed;
        assert_eq!(passed, reason_codes.is_empty());
        assert_eq!(passed, issue_count == 0);
        assert!(validate_sha256(&issue_sha256, "check issue sha256").is_ok());
        Self {
            status,
            reason_codes,
            issue_count,
            issue_sha256,
        }
    }
}

fn passed_check() -> CheckResult {
    CheckResult::with_issue_digest(CheckStatus::Passed, Vec::new(), 0, sha256_hex(&[]))
}

fn fixed_check(status: CheckStatus, reason: &'static str) -> CheckResult {
    CheckResult::with_issue_digest(
        status,
        vec![reason.to_string()],
        1,
        sha256_hex(reason.as_bytes()),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleResult {
    pub id: &'static str,
    pub status: CheckStatus,
    pub observed_count: usize,
    pub reason_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderReport {
    pub profile: &'static str,
    pub dpi: f32,
    pub total_pages: usize,
    pub selected_pages: Vec<usize>,
    pub status: CheckStatus,
    pub reason_codes: Vec<String>,
    pub fonts: Vec<FontReport>,
    pub pages: Vec<PageReport>,
    pub issues: Vec<RenderIssueReportEntry>,
    pub info: Vec<RenderIssueReportEntry>,
    pub issue_count: u64,
    pub info_count: u64,
    pub issue_log_complete: bool,
    pub issue_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderIssueReportEntry {
    pub code: &'static str,
    pub severity: &'static str,
    pub stage: &'static str,
    pub count: u64,
    pub sample_sha256: Vec<String>,
    pub samples_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FontReport {
    pub requested_name_sha256: String,
    pub resolved_family_sha256: Option<String>,
    pub font_file_sha256: Option<String>,
    pub face_index: Option<u32>,
    pub outcome: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageReport {
    pub page: usize,
    pub width_pt: f32,
    pub height_pt: f32,
    pub item_count: usize,
    pub visual_blank: bool,
    pub outside_page_bounds: DetectionReport,
    pub possible_collision: DetectionReport,
    pub png_sha256: String,
    pub png_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DetectionReport {
    pub result: &'static str,
    pub count: usize,
    pub algorithm: &'static str,
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct OracleReport {
    pub mode: OracleMode,
    pub status: OracleStatus,
    pub reason_code: Option<String>,
    pub expected: Option<OracleAttestation>,
    pub observed: Option<OracleAttestation>,
    pub stdout: Option<LogReport>,
    pub stderr: Option<LogReport>,
    pub artifact_determinism: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OracleStatus {
    Disabled,
    NotRun,
    Passed,
    Failed,
    OracleUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OracleAttestation {
    pub runtime_kind: String,
    pub runtime_version: String,
    pub runtime_sha256: String,
    pub libreoffice_version: String,
    pub libreoffice_executable_sha256: String,
    pub extension_version: String,
    pub extension_sha256: String,
    pub image_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker_client_version_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker_server_version_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_reference_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogReport {
    pub bytes_observed: u64,
    pub bytes_hashed: u64,
    pub truncated: bool,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactReport {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub deterministic: bool,
}

#[derive(Debug)]
pub struct CertificationOutcome {
    pub overall: OverallStatus,
    pub report_dir: PathBuf,
}

/// CLI and MCP shared entry point. Both immutable inputs are copied exactly once into a private
/// sibling workspace. The final report directory must not exist and is published with an atomic
/// no-replace directory rename.
pub fn execute(
    input: &Path,
    policy_path: &Path,
    report_dir: &Path,
) -> Result<CertificationOutcome> {
    let mut stage = AtomicCertificationDir::new(report_dir)?;
    let input_ext = input
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| matches!(value.to_ascii_lowercase().as_str(), "hwp" | "hwpx"))
        .unwrap_or("bin");
    let input_snapshot = stage.root.join(format!(".input.snapshot.{input_ext}"));
    let input_snapshot_report = snapshot_file(input, &input_snapshot, MAX_INPUT_BYTES, None)?;
    let policy_snapshot = stage.root.join(".policy.snapshot");
    let policy_snapshot_report =
        snapshot_file(policy_path, &policy_snapshot, MAX_POLICY_BYTES, None)?;
    let policy_bytes = read_bounded(&policy_snapshot, MAX_POLICY_BYTES)?;
    let policy: CertificationPolicy = if first_non_whitespace(&policy_bytes) == Some(b'{') {
        serde_json::from_slice(&policy_bytes).context("certification policy JSON 파싱 실패")?
    } else {
        serde_yaml::from_slice(&policy_bytes).context("certification policy YAML 파싱 실패")?
    };
    validate_policy(&policy)?;
    let normalized_policy = serde_json::to_vec(&policy)?;
    let normalized_policy_sha256 = sha256_hex(&normalized_policy);
    let policy_base = policy_path.parent().unwrap_or_else(|| Path::new("."));

    let font_dir = stage.root.join(".fonts");
    fs::create_dir(&font_dir)?;
    set_private_permissions(&font_dir)?;
    let font_files =
        snapshot_font_manifest(&policy.document.fonts.manifest, policy_base, &font_dir)?;

    let (preflight, parse_budget_failed) = match preflight_parse_budget(&input_snapshot) {
        Ok(preflight) => (Some(preflight), false),
        Err(error) => {
            let budget_failed = format!("{error:#}").contains("parse_budget_exceeded:");
            (None, budget_failed)
        }
    };
    let package = preflight.as_ref().map_or_else(
        || Err(anyhow::anyhow!("package preflight failed")),
        |preflight| inspect_package(&input_snapshot, preflight),
    );
    let mut package_check = fixed_check(CheckStatus::Failed, "package_validation_failed");
    let mut semantic_check = fixed_check(CheckStatus::Skipped, "package_validation_failed");
    let mut format = "unknown".to_string();
    let mut document = None;
    if let Ok(info) = package {
        format = info.format;
        let package_warnings = info.warnings;
        package_check = if package_warnings.is_empty() {
            passed_check()
        } else {
            CheckResult::with_issue_digest(
                CheckStatus::Failed,
                vec!["package_or_import_warnings".to_string()],
                package_warnings.len(),
                hash_string_list(&package_warnings),
            )
        };
        let preflight = preflight
            .as_ref()
            .expect("successful package inspection requires preflight");
        match load_document(&input_snapshot, preflight) {
            Ok(first) => {
                let first_hash = semantic_sha256(&first)?;
                match load_document(&input_snapshot, preflight) {
                    Ok(second) if semantic_sha256(&second)? == first_hash => {
                        semantic_check = passed_check();
                        document = Some(first);
                    }
                    Ok(_) => {
                        semantic_check = fixed_check(CheckStatus::Failed, "repeat_import_mismatch");
                    }
                    Err(_) => {
                        semantic_check = fixed_check(CheckStatus::Failed, "repeat_import_failed");
                    }
                }
            }
            Err(_) => {
                package_check = fixed_check(CheckStatus::Failed, "document_parse_failed");
            }
        }
    }

    let raw = if let (Some(document), Some(preflight)) = (document.as_ref(), preflight.as_ref()) {
        inspect_raw_features(&input_snapshot, &format, document, preflight).unwrap_or({
            RawFeatures {
                macros: FeaturePresence::Unknown,
                external_references: FeaturePresence::Unknown,
            }
        })
    } else {
        RawFeatures {
            macros: FeaturePresence::Unknown,
            external_references: FeaturePresence::Unknown,
        }
    };
    let mut rules = Vec::new();
    let mut render_report = empty_render_report(policy.render.dpi);
    if parse_budget_failed {
        render_report.status = CheckStatus::Failed;
        render_report.reason_codes = vec!["parse_budget_exceeded".to_string()];
        let mut issues = hwp_render::RenderIssueAccumulator::new();
        issues.push_once(
            hwp_render::RenderIssueCode::ParseBudgetExceeded,
            b"parse_budget_exceeded",
        );
        apply_render_issue_report(&mut render_report, issues.finish());
    }
    let mut local_artifacts = Vec::new();
    if let Some(doc) = document.as_ref() {
        let stats = collect_document_stats(doc);
        rules = evaluate_document_rules(&policy.document, doc, &stats, raw);
        let unresolved_reasons =
            if policy.render.fail_on_unresolved_fields && stats.unresolved_fields > 0 {
                vec!["unresolved_field_binding".to_string()]
            } else {
                Vec::new()
            };
        rules.push(rule_from_reasons(
            "unresolved_fields",
            stats.unresolved_fields,
            unresolved_reasons,
        ));
        match run_render(
            doc,
            &policy,
            &font_files,
            &stage.root,
            &stats,
            &mut local_artifacts,
        ) {
            Ok(value) => {
                render_report = value.report;
                rules.push(value.font_rule);
            }
            Err(error) => {
                let reason = render_failure_reason(&error);
                render_report.status = CheckStatus::Failed;
                render_report.reason_codes = vec![reason.to_string()];
                let code = match reason {
                    "layout_budget_exceeded" => hwp_render::RenderIssueCode::LayoutBudgetExceeded,
                    "image_decode_budget_exceeded" => {
                        hwp_render::RenderIssueCode::ImageDecodeBudgetExceeded
                    }
                    "pagination_drift_detected" => {
                        hwp_render::RenderIssueCode::PaginationDriftDetected
                    }
                    _ => hwp_render::RenderIssueCode::RenderExecutionFailed,
                };
                let mut issues = hwp_render::RenderIssueAccumulator::new();
                issues.push_once(code, reason.as_bytes());
                apply_render_issue_report(&mut render_report, issues.finish());
                rules.push(rule_from_reasons(
                    "fonts",
                    0,
                    vec!["font_resolution_not_run".to_string()],
                ));
            }
        }
    }

    // Optional evidence checks evaluate external artifacts pinned by the policy. They do not
    // depend on native parsing and fail closed on missing or invalid artifacts.
    let preservation_check = policy
        .document
        .preservation
        .as_ref()
        .map(|section| evaluate_preservation_evidence(section, policy_base));
    let hancom_open_check = policy.document.hancom_open.as_ref().map(|section| {
        evaluate_hancom_open_evidence(section, policy_base, &input_snapshot_report.sha256)
    });
    let evidence_failed = preservation_check
        .as_ref()
        .is_some_and(|check| check.status != CheckStatus::Passed)
        || hancom_open_check
            .as_ref()
            .is_some_and(|check| check.status != CheckStatus::Passed);

    let local_failed = package_check.status != CheckStatus::Passed
        || semantic_check.status != CheckStatus::Passed
        || rules.iter().any(|rule| rule.status == CheckStatus::Failed)
        || render_report.status != CheckStatus::Passed
        || evidence_failed;

    let (oracle, mut oracle_artifacts) = if local_failed {
        (
            OracleReport {
                mode: policy.oracle.mode,
                status: if policy.oracle.mode == OracleMode::Disabled {
                    OracleStatus::Disabled
                } else {
                    OracleStatus::NotRun
                },
                reason_code: (policy.oracle.mode != OracleMode::Disabled)
                    .then(|| "local_certification_failed".to_string()),
                expected: expected_attestation(&policy.oracle),
                observed: None,
                stdout: None,
                stderr: None,
                artifact_determinism: "not_claimed",
            },
            Vec::new(),
        )
    } else {
        run_oracle(
            &policy.oracle,
            &input_snapshot,
            if format == "hwp5" { "hwp" } else { "hwpx" },
            &stage.root,
        )
    };
    local_artifacts.append(&mut oracle_artifacts);

    let overall = overall_status(local_failed, policy.oracle.mode, oracle.status);
    let scope = if oracle.status == OracleStatus::Passed {
        "native_plus_independent_import"
    } else {
        "native_only"
    };
    let report = CertificationReport {
        schema_version: REPORT_SCHEMA_VERSION,
        contract: "hwp-certification-report-v1",
        overall,
        scope,
        input: InputReport {
            format,
            bytes: input_snapshot_report.bytes,
            sha256: input_snapshot_report.sha256,
        },
        policy_sha256: normalized_policy_sha256,
        checks: CheckSet {
            package: package_check,
            repeat_import_consistency: semantic_check,
            rules,
            preservation: preservation_check,
            hancom_open: hancom_open_check,
        },
        render: render_report,
        oracle,
        artifacts: local_artifacts.clone(),
        limitations: vec![
            "native_not_detected_is_algorithm_scoped",
            "hancom_rendering_parity_not_claimed",
            "oracle_artifact_determinism_not_claimed",
            "oracle_page_count_not_host_verified",
            "selected_pages_only_when_policy_selects_pages",
        ],
    };
    validate_render_report_invariants(&report.render)?;
    validate_rule_composition(&report.checks.rules)?;
    validate_evidence_check_invariants(&report)?;
    validate_report_artifact_invariants(&report)?;

    let report_bytes = with_final_newline(serde_json::to_vec_pretty(&report)?);
    let report_artifact = write_artifact(&stage.root, "report.json", &report_bytes, true)?;
    let mut manifest_files = local_artifacts;
    manifest_files.push(report_artifact);
    manifest_files.sort_by(|left, right| left.path.cmp(&right.path));
    if manifest_files.len() > MAX_MANIFEST_FILES {
        anyhow::bail!("manifest file count exceeds {MAX_MANIFEST_FILES}");
    }
    validate_artifact_budget(&manifest_files)?;
    let total_bytes = manifest_files.iter().map(|item| item.bytes).sum::<u64>();
    let manifest_bytes = build_artifact_manifest(&manifest_files, total_bytes)?;
    write_artifact(&stage.root, "manifest.json", &manifest_bytes, true)?;

    // Private inputs, policy and font snapshots are never published as report artifacts.
    fs::remove_file(&input_snapshot)?;
    fs::remove_file(&policy_snapshot)?;
    fs::remove_dir_all(&font_dir)?;
    // Keep the compiler from accidentally treating raw policy bytes as a second source of truth.
    let _ = policy_snapshot_report;
    let mut expected_paths: BTreeSet<String> = manifest_files
        .iter()
        .map(|artifact| artifact.path.clone())
        .collect();
    expected_paths.insert("manifest.json".to_string());
    if expected_paths.len() > MAX_PUBLISHED_TREE_ENTRIES {
        anyhow::bail!("published artifact tree exceeds {MAX_PUBLISHED_TREE_ENTRIES}");
    }
    stage.publish(&expected_paths)?;
    Ok(CertificationOutcome {
        overall,
        report_dir: report_dir.to_path_buf(),
    })
}

fn overall_status(local_failed: bool, mode: OracleMode, oracle: OracleStatus) -> OverallStatus {
    if local_failed {
        return OverallStatus::Failed;
    }
    match (mode, oracle) {
        (OracleMode::Required, OracleStatus::Failed) => OverallStatus::Failed,
        (OracleMode::Required, OracleStatus::OracleUnavailable)
        | (OracleMode::Optional, OracleStatus::Failed) => OverallStatus::Partial,
        (OracleMode::Optional, OracleStatus::OracleUnavailable)
        | (OracleMode::Disabled, OracleStatus::Disabled)
        | (_, OracleStatus::Passed) => OverallStatus::Passed,
        (_, OracleStatus::NotRun) => OverallStatus::Failed,
        _ => OverallStatus::Failed,
    }
}

fn validate_policy(policy: &CertificationPolicy) -> Result<()> {
    if policy.schema_version != POLICY_SCHEMA_VERSION {
        anyhow::bail!(
            "지원하지 않는 certification policy schema_version: {}",
            policy.schema_version
        );
    }
    hwp_render::validate_dpi(policy.render.dpi).map_err(anyhow::Error::msg)?;
    match &policy.render.pages {
        PageSelection::All(value) if value == "all" => {}
        PageSelection::All(_) => anyhow::bail!("render.pages 문자열은 'all'만 허용합니다"),
        PageSelection::Selected(pages) => {
            validate_page_list(pages, "render.pages")?;
            if pages.is_empty() {
                anyhow::bail!("render.pages selected 목록은 비어 있을 수 없습니다");
            }
            if policy
                .render
                .allowed_blank_pages
                .iter()
                .any(|page| !pages.contains(page))
            {
                anyhow::bail!("allowed_blank_pages는 selected render.pages 범위 안이어야 합니다");
            }
        }
    }
    validate_page_list(&policy.render.allowed_blank_pages, "allowed_blank_pages")?;
    if policy.render.page_count.exact.is_some()
        && (policy.render.page_count.min.is_some() || policy.render.page_count.max.is_some())
    {
        anyhow::bail!("page_count.exact와 min/max는 함께 사용할 수 없습니다");
    }
    if policy
        .render
        .page_count
        .exact
        .is_some_and(|value| value > MAX_PAGE_NUMBER)
        || policy
            .render
            .page_count
            .min
            .into_iter()
            .chain(policy.render.page_count.max)
            .any(|value| value > MAX_PAGE_NUMBER)
    {
        anyhow::bail!("page_count.exact가 {MAX_PAGE_NUMBER} 상한을 초과합니다");
    }
    validate_count_policy(
        &CountPolicy {
            min: policy.render.page_count.min,
            max: policy.render.page_count.max,
        },
        "page_count",
    )?;
    validate_count_policy(&policy.document.tables, "tables")?;
    validate_count_policy(&policy.document.links.count, "links.count")?;
    validate_count_policy(
        &policy.document.numbering.definitions,
        "numbering.definitions",
    )?;
    validate_named_set(
        &policy.document.defined_styles.allowed,
        &policy.document.defined_styles.required,
        "defined_styles",
    )?;
    validate_named_set(
        &policy.document.used_styles.allowed,
        &policy.document.used_styles.required,
        "used_styles",
    )?;
    validate_named_set(
        &policy.document.fonts.allowed_requested,
        &policy.document.fonts.required_requested,
        "fonts",
    )?;
    validate_unique_usizes(
        &policy.document.numbering.allowed_definition_indexes,
        "numbering.allowed_definition_indexes",
    )?;
    validate_unique_usizes(
        &policy.document.numbering.required_definition_indexes,
        "numbering.required_definition_indexes",
    )?;
    if !policy
        .document
        .numbering
        .allowed_definition_indexes
        .is_empty()
        && policy
            .document
            .numbering
            .required_definition_indexes
            .iter()
            .any(|index| {
                !policy
                    .document
                    .numbering
                    .allowed_definition_indexes
                    .contains(index)
            })
    {
        anyhow::bail!("numbering required indexes는 allowed indexes의 부분집합이어야 합니다");
    }
    validate_metadata_policy(&policy.document.metadata)?;
    if policy.document.fonts.manifest.len() > MAX_FONT_FILES {
        anyhow::bail!("font manifest가 {MAX_FONT_FILES}개 상한을 초과합니다");
    }
    for pin in &policy.document.fonts.manifest {
        validate_relative_asset_path(&pin.path)?;
        validate_sha256(&pin.sha256, "font sha256")?;
    }
    if policy
        .document
        .fonts
        .manifest
        .windows(2)
        .any(|window| window[0].path >= window[1].path)
    {
        anyhow::bail!("font manifest paths must be strictly sorted");
    }
    validate_unique_strings(
        policy
            .document
            .fonts
            .manifest
            .iter()
            .map(|pin| pin.path.as_str()),
        "font manifest path",
    )?;
    validate_unique_strings(
        policy
            .document
            .fonts
            .manifest
            .iter()
            .map(|pin| pin.sha256.as_str()),
        "font manifest sha256",
    )?;
    if policy.document.links.allowed_schemes.len() > 64 {
        anyhow::bail!("allowed_schemes가 64개 상한을 초과합니다");
    }
    let mut normalized_schemes = BTreeSet::new();
    for scheme in &policy.document.links.allowed_schemes {
        let bytes = scheme.as_bytes();
        if bytes.is_empty()
            || scheme.len() > 32
            || !bytes[0].is_ascii_alphabetic()
            || !bytes[1..]
                .iter()
                .copied()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
            || !normalized_schemes.insert(scheme.to_ascii_lowercase())
        {
            anyhow::bail!("link scheme가 유효하지 않습니다");
        }
    }
    match (policy.oracle.mode, &policy.oracle.configuration) {
        (OracleMode::Disabled, None) => {}
        (OracleMode::Disabled, Some(_)) => {
            anyhow::bail!("oracle.mode=disabled이면 configuration을 둘 수 없습니다")
        }
        (_, None) => anyhow::bail!("활성 oracle에는 configuration이 필요합니다"),
        (_, Some(config)) => validate_oracle_configuration(config)?,
    }
    if let Some(preservation) = &policy.document.preservation {
        validate_relative_asset_path(&preservation.report)?;
        if preservation.max_loss_codes > MAX_RULE_COUNT {
            anyhow::bail!("preservation.max_loss_codes가 {MAX_RULE_COUNT} 상한을 초과합니다");
        }
    }
    if let Some(hancom_open) = &policy.document.hancom_open {
        validate_relative_asset_path(&hancom_open.receipt)?;
    }
    Ok(())
}

fn validate_page_list(pages: &[usize], label: &str) -> Result<()> {
    if pages.len() > MAX_SELECTED_PAGES {
        anyhow::bail!("{label}가 {MAX_SELECTED_PAGES}개 상한을 초과합니다");
    }
    let mut previous = 0;
    for &page in pages {
        if page == 0 || page > MAX_PAGE_NUMBER || page <= previous {
            anyhow::bail!("{label}는 중복 없는 1-based 오름차순이어야 합니다");
        }
        previous = page;
    }
    Ok(())
}

fn validate_unique_usizes(values: &[usize], label: &str) -> Result<()> {
    if values.len() > MAX_DEFINITION_INDEXES
        || values.iter().any(|value| *value > u16::MAX as usize)
    {
        anyhow::bail!("{label}가 정의 index 범위를 초과합니다");
    }
    let mut seen = BTreeSet::new();
    if values.iter().any(|value| !seen.insert(*value)) {
        anyhow::bail!("{label}에 중복 값이 있습니다");
    }
    Ok(())
}

fn validate_metadata_policy(policy: &MetadataPolicy) -> Result<()> {
    for (label, values) in [
        ("allowed_fields", &policy.allowed_fields),
        ("required_fields", &policy.required_fields),
        ("forbidden_fields", &policy.forbidden_fields),
    ] {
        let mut seen = BTreeSet::new();
        if values.iter().any(|field| !seen.insert(*field)) {
            anyhow::bail!("metadata.{label}에 중복 값이 있습니다");
        }
    }
    if !policy.allowed_fields.is_empty()
        && policy
            .required_fields
            .iter()
            .any(|field| !policy.allowed_fields.contains(field))
    {
        anyhow::bail!("metadata.required_fields는 allowed_fields의 부분집합이어야 합니다");
    }
    if policy
        .required_fields
        .iter()
        .any(|field| policy.forbidden_fields.contains(field))
    {
        anyhow::bail!("metadata required_fields와 forbidden_fields는 겹칠 수 없습니다");
    }
    Ok(())
}

fn validate_count_policy(policy: &CountPolicy, label: &str) -> Result<()> {
    if policy
        .min
        .into_iter()
        .chain(policy.max)
        .any(|value| value > MAX_RULE_COUNT)
    {
        anyhow::bail!("{label} 값이 {MAX_RULE_COUNT} 상한을 초과합니다");
    }
    if policy
        .min
        .zip(policy.max)
        .is_some_and(|(min, max)| min > max)
    {
        anyhow::bail!("{label}.min은 max보다 클 수 없습니다");
    }
    Ok(())
}

fn validate_named_set(allowed: &[String], required: &[String], label: &str) -> Result<()> {
    if allowed.len() > MAX_POLICY_NAMES || required.len() > MAX_POLICY_NAMES {
        anyhow::bail!("{label} 이름 목록이 {MAX_POLICY_NAMES}개 상한을 초과합니다");
    }
    validate_unique_strings(allowed.iter().map(String::as_str), label)?;
    validate_unique_strings(required.iter().map(String::as_str), label)?;
    for value in allowed.iter().chain(required) {
        if value.is_empty() || value.chars().count() > 256 || value.chars().any(char::is_control) {
            anyhow::bail!("{label} 이름은 제어문자 없는 1..=256 characters여야 합니다");
        }
    }
    if !allowed.is_empty() && required.iter().any(|value| !allowed.contains(value)) {
        anyhow::bail!("{label}.required는 allowed의 부분집합이어야 합니다");
    }
    Ok(())
}

fn validate_unique_strings<'a>(values: impl Iterator<Item = &'a str>, label: &str) -> Result<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            anyhow::bail!("{label}에 중복 값이 있습니다");
        }
    }
    Ok(())
}

fn validate_oracle_configuration(config: &OracleConfiguration) -> Result<()> {
    validate_sha256(&config.runtime.sha256, "oracle runtime sha256")?;
    validate_sha256(
        &config.libreoffice.executable_sha256,
        "LibreOffice executable sha256",
    )?;
    validate_sha256(&config.extension.sha256, "extension sha256")?;
    if !config.image.digest.starts_with("sha256:")
        || config.image.digest.len() != "sha256:".len() + 64
        || !config.image.digest["sha256:".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        anyhow::bail!("oracle image digest는 sha256:<64 hex>여야 합니다");
    }
    for value in [
        &config.runtime.version,
        &config.libreoffice.version,
        &config.extension.version,
    ] {
        if value.is_empty()
            || value.chars().count() > 256
            || value.contains('\0')
            || value.chars().any(char::is_control)
        {
            anyhow::bail!("oracle pin 문자열이 유효 범위를 벗어납니다");
        }
    }
    Ok(())
}

struct PackageInfo {
    format: String,
    warnings: Vec<String>,
}

fn parse_budget_error(label: &str) -> anyhow::Error {
    anyhow::anyhow!("parse_budget_exceeded:{label}")
}

/// Bound recursive parser inputs before either HWPX XML or HWP5 record trees are
/// materialized. This runs against the immutable input snapshot used by both imports.
enum ParsePreflight {
    Hwp5(Box<hwp5::BoundedReadSnapshot>),
    Hwpx,
}

fn preflight_parse_budget(path: &Path) -> Result<ParsePreflight> {
    if hwp5::Hwp5Container::open(path).is_ok() {
        let snapshot = hwp5::BoundedReadSnapshot::open(
            path,
            hwp5::BoundedReadLimits {
                max_streams: MAX_PARSE_HWP5_STREAMS,
                max_total_stream_name_bytes: MAX_PARSE_HWP5_STREAM_NAME_BYTES,
                max_stream_bytes: MAX_PARSE_RECORD_STREAM_BYTES,
                max_total_materialized_bytes: MAX_PARSE_RECORD_TOTAL_BYTES,
                max_records: MAX_PARSE_RECORDS,
                max_record_depth: MAX_PARSE_RECORD_DEPTH,
            },
        )
        .map_err(|error| match error {
            hwp5::Hwp5Error::ResourceLimitExceeded { .. }
            | hwp5::Hwp5Error::StructureLimitExceeded { .. } => {
                parse_budget_error("hwp5_stream_budget")
            }
            other => anyhow::Error::new(other),
        })?;
        return Ok(ParsePreflight::Hwp5(Box::new(snapshot)));
    }

    let limits = hwpx::PackageLimits {
        max_entries: 4_096,
        max_entry_name_bytes: 64 * 1024,
        max_total_name_bytes: 16 * 1024 * 1024,
        reject_duplicate_names: true,
        max_entry_uncompressed_bytes: MAX_INPUT_BYTES,
        max_total_uncompressed_bytes: MAX_INPUT_BYTES,
        max_xml_uncompressed_bytes: 32 * 1024 * 1024,
        max_compression_ratio: 1_000,
    };
    let mut package = hwpx::HwpxPackage::open_with_limits(path, &limits).map_err(|error| {
        if error.to_string().contains("제한") || error.to_string().contains("초과") {
            parse_budget_error("hwpx_package")
        } else {
            anyhow::Error::new(error)
        }
    })?;
    let entries = package.entries()?;
    let mut nodes = 0usize;
    for entry in entries {
        if !(entry.name.ends_with(".xml") || entry.name.ends_with(".hpf")) {
            continue;
        }
        let xml = package.read_entry(&entry.name)?;
        preflight_hwpx_xml(&entry.name, &xml, &mut nodes)?;
    }
    Ok(ParsePreflight::Hwpx)
}

#[cfg(test)]
fn preflight_hwp5_record_stream(data: &[u8], total_records: &mut usize) -> Result<()> {
    let mut reader = hwp5::codec::ByteReader::new(data);
    while !reader.is_empty() {
        let header = hwp5::record::RecordHeader::decode(&mut reader)?;
        *total_records = total_records
            .checked_add(1)
            .ok_or_else(|| parse_budget_error("hwp5_record_count"))?;
        if *total_records > MAX_PARSE_RECORDS {
            return Err(parse_budget_error("hwp5_record_count"));
        }
        if usize::from(header.level) > MAX_PARSE_RECORD_DEPTH {
            return Err(parse_budget_error("hwp5_record_depth"));
        }
        reader.read_bytes(header.size as usize)?;
    }
    Ok(())
}

fn preflight_hwpx_xml(entry: &str, xml: &[u8], nodes: &mut usize) -> Result<()> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut depth = 0usize;
    loop {
        use quick_xml::events::Event;
        match reader
            .read_event()
            .map_err(|error| anyhow::anyhow!("{entry} XML preflight failed: {error}"))?
        {
            Event::Start(_) => {
                *nodes = nodes
                    .checked_add(1)
                    .ok_or_else(|| parse_budget_error("hwpx_xml_nodes"))?;
                depth = depth
                    .checked_add(1)
                    .ok_or_else(|| parse_budget_error("hwpx_xml_depth"))?;
                if *nodes > MAX_PARSE_XML_NODES {
                    return Err(parse_budget_error("hwpx_xml_nodes"));
                }
                if depth > MAX_PARSE_XML_DEPTH {
                    return Err(parse_budget_error("hwpx_xml_depth"));
                }
            }
            Event::Empty(_) => {
                *nodes = nodes
                    .checked_add(1)
                    .ok_or_else(|| parse_budget_error("hwpx_xml_nodes"))?;
                if *nodes > MAX_PARSE_XML_NODES {
                    return Err(parse_budget_error("hwpx_xml_nodes"));
                }
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| anyhow::anyhow!("{entry} XML depth underflow"))?;
            }
            Event::DocType(_) => anyhow::bail!("{entry} XML DTD is not accepted"),
            Event::Eof => break,
            _ => {}
        }
    }
    if depth != 0 {
        anyhow::bail!("{entry} XML depth is unbalanced");
    }
    Ok(())
}

fn inspect_package(path: &Path, preflight: &ParsePreflight) -> Result<PackageInfo> {
    if let ParsePreflight::Hwp5(snapshot) = preflight {
        let read = snapshot.read_document()?;
        return Ok(PackageInfo {
            format: "hwp5".to_string(),
            warnings: read.warnings,
        });
    }
    let mut package = hwpx::HwpxPackage::open(path)?;
    let names: Vec<String> = package
        .entries()?
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    for required in [
        "mimetype",
        "version.xml",
        "Contents/header.xml",
        "META-INF/container.xml",
    ] {
        if !names.iter().any(|name| name == required) {
            anyhow::bail!("required_hwpx_entry_missing");
        }
    }
    if !names
        .iter()
        .any(|name| name.starts_with("Contents/section"))
    {
        anyhow::bail!("required_hwpx_section_missing");
    }
    package.verify_integrity()?;
    let read = hwpx::read_structure(path)?;
    Ok(PackageInfo {
        format: "hwpx".to_string(),
        warnings: read.warnings,
    })
}

fn load_document(path: &Path, preflight: &ParsePreflight) -> Result<hwp_model::Document> {
    match preflight {
        ParsePreflight::Hwp5(snapshot) => Ok(snapshot.read_document()?.document),
        ParsePreflight::Hwpx => Ok(hwpx::read_document(path)?.document),
    }
}

fn semantic_sha256(document: &hwp_model::Document) -> Result<String> {
    let mut writer = DigestWriter(Sha256::new());
    serde_json::to_writer(&mut writer, document)?;
    for stream in &document.bin_streams {
        writer.0.update((stream.name.len() as u64).to_le_bytes());
        writer.0.update(stream.name.as_bytes());
        writer.0.update((stream.data.len() as u64).to_le_bytes());
        writer.0.update(Sha256::digest(&stream.data));
    }
    Ok(hex_digest(writer.0.finalize().as_slice()))
}

struct DigestWriter(Sha256);

impl Write for DigestWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum FeaturePresence {
    Absent,
    Present,
    Unknown,
}

#[derive(Clone, Copy)]
struct RawFeatures {
    macros: FeaturePresence,
    external_references: FeaturePresence,
}

fn inspect_raw_features(
    path: &Path,
    format: &str,
    document: &hwp_model::Document,
    preflight: &ParsePreflight,
) -> Result<RawFeatures> {
    match format {
        "hwp5" => {
            let ParsePreflight::Hwp5(snapshot) = preflight else {
                anyhow::bail!("HWP5 preflight snapshot missing");
            };
            let macros = match snapshot.script_presence() {
                hwp5::ScriptPresence::Absent => FeaturePresence::Absent,
                hwp5::ScriptPresence::Present => FeaturePresence::Present,
                hwp5::ScriptPresence::Indeterminate => FeaturePresence::Unknown,
            };
            let external = document.header.bin_data.iter().any(|item| {
                item.kind() == 0 && (item.link_abs.is_some() || item.link_rel.is_some())
            });
            Ok(RawFeatures {
                macros,
                external_references: if external {
                    FeaturePresence::Present
                } else {
                    FeaturePresence::Absent
                },
            })
        }
        "hwpx" => {
            let mut package = hwpx::HwpxPackage::open(path)?;
            let entries = package.entries()?;
            let mut macros = if entries
                .iter()
                .any(|entry| entry.name.starts_with("Scripts/"))
            {
                FeaturePresence::Present
            } else {
                FeaturePresence::Absent
            };
            let mut external = FeaturePresence::Absent;
            for entry in entries {
                let relevant = entry.name.ends_with(".rels")
                    || entry.name == "Contents/content.hpf"
                    || entry.name == "META-INF/container.xml";
                if relevant {
                    let bytes = package.read_entry(&entry.name)?;
                    let scan = scan_relationship_xml(&bytes)?;
                    if scan.external {
                        external = FeaturePresence::Present;
                    }
                    if scan.macros {
                        macros = FeaturePresence::Present;
                    }
                }
            }
            Ok(RawFeatures {
                macros,
                external_references: external,
            })
        }
        _ => Ok(RawFeatures {
            macros: FeaturePresence::Unknown,
            external_references: FeaturePresence::Unknown,
        }),
    }
}

#[derive(Default)]
struct RelationshipScan {
    external: bool,
    macros: bool,
}

fn scan_relationship_xml(bytes: &[u8]) -> Result<RelationshipScan> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut scan = RelationshipScan::default();
    loop {
        match reader.read_event()? {
            Event::Start(element) | Event::Empty(element) => {
                for attribute in element.attributes() {
                    let attribute = attribute?;
                    let key = attribute.key.local_name();
                    let raw = String::from_utf8_lossy(&attribute.value);
                    let value = quick_xml::escape::unescape(&raw)
                        .map(|value| value.into_owned())
                        .unwrap_or_else(|_| raw.into_owned());
                    if key.as_ref().eq_ignore_ascii_case(b"TargetMode")
                        && value.eq_ignore_ascii_case("external")
                    {
                        scan.external = true;
                    }
                    if matches!(key.as_ref(), b"href" | b"target" | b"src")
                        && is_external_uri(&value)
                    {
                        scan.external = true;
                    }
                    if matches!(key.as_ref(), b"media-type" | b"mediaType")
                        && matches!(
                            value.to_ascii_lowercase().as_str(),
                            "application/javascript"
                                | "text/javascript"
                                | "application/ecmascript"
                                | "text/ecmascript"
                                | "application/x-hwp-script"
                        )
                    {
                        scan.macros = true;
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(scan)
}

fn is_external_uri(value: &str) -> bool {
    let Some((scheme, _)) = value.split_once(':') else {
        return value.starts_with("//");
    };
    !matches!(scheme.to_ascii_lowercase().as_str(), "urn" | "data")
}

#[derive(Default)]
struct DocumentStats {
    tables: usize,
    pictures: usize,
    shapes: usize,
    links: usize,
    link_schemes: BTreeSet<String>,
    unresolved_fields: usize,
    picture_descriptions_missing: usize,
    shape_descriptions_missing: usize,
    numbering_used: BTreeSet<usize>,
    styles_used: BTreeSet<usize>,
    linked_bin_references: usize,
}

fn collect_document_stats(document: &hwp_model::Document) -> DocumentStats {
    let mut stats = DocumentStats::default();
    for section in &document.sections {
        for paragraph in &section.paragraphs {
            collect_paragraph_stats(document, paragraph, &mut stats);
        }
    }
    stats.linked_bin_references = document
        .header
        .bin_data
        .iter()
        .filter(|item| item.kind() == 0 && (item.link_abs.is_some() || item.link_rel.is_some()))
        .count();
    stats
}

fn collect_paragraph_stats(
    document: &hwp_model::Document,
    paragraph: &hwp_model::Paragraph,
    stats: &mut DocumentStats,
) {
    stats.styles_used.insert(paragraph.style.0 as usize);
    if let Some(shape) = document
        .header
        .para_shapes
        .get(paragraph.para_shape.0 as usize)
        && shape.head_type() == 2
    {
        stats.numbering_used.insert(shape.numbering_id as usize);
    }
    for character in &paragraph.chars {
        if let hwp_model::HwpChar::ExtCtrl {
            code, ctrl_index, ..
        } = character
            && *code == hwp_model::ctrl_char::FIELD_START
            && ctrl_index
                .and_then(|index| paragraph.controls.get(index as usize))
                .is_none()
        {
            stats.unresolved_fields += 1;
        }
    }
    for control in &paragraph.controls {
        if let Some(url) = hwp_convert::hyperlink_url(control) {
            stats.links += 1;
            let scheme = url
                .split_once(':')
                .map(|(scheme, _)| scheme.to_ascii_lowercase())
                .unwrap_or_default();
            stats.link_schemes.insert(scheme);
        }
        match control {
            hwp_model::Control::Table(table) => {
                stats.tables += 1;
                for cell in &table.cells {
                    for nested in &cell.paragraphs {
                        collect_paragraph_stats(document, nested, stats);
                    }
                }
            }
            hwp_model::Control::Picture(picture) => {
                stats.pictures += 1;
                if picture
                    .description
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty())
                {
                    stats.picture_descriptions_missing += 1;
                }
            }
            hwp_model::Control::Generic(generic) => {
                stats.shapes += generic.gso_shapes.len();
                stats.shape_descriptions_missing += generic
                    .gso_shapes
                    .iter()
                    .filter(|shape| {
                        shape
                            .description
                            .as_deref()
                            .is_none_or(|value| value.trim().is_empty())
                    })
                    .count();
                for list in &generic.paragraph_lists {
                    for nested in &list.paragraphs {
                        collect_paragraph_stats(document, nested, stats);
                    }
                }
            }
            hwp_model::Control::SectionDef(_) => {}
        }
    }
}

fn evaluate_document_rules(
    policy: &DocumentPolicy,
    document: &hwp_model::Document,
    stats: &DocumentStats,
    raw: RawFeatures,
) -> Vec<RuleResult> {
    let mut rules = Vec::new();
    let styles: BTreeSet<&str> = document
        .header
        .styles
        .iter()
        .map(|style| style.name.as_str())
        .collect();
    rules.push(named_set_rule(
        "defined_styles",
        &policy.defined_styles,
        &styles,
    ));
    let used_styles: BTreeSet<&str> = stats
        .styles_used
        .iter()
        .filter_map(|index| document.header.styles.get(*index))
        .map(|style| style.name.as_str())
        .collect();
    let mut used_rule = named_set_rule("used_styles", &policy.used_styles, &used_styles);
    if stats.styles_used.len() != used_styles.len() {
        used_rule.status = CheckStatus::Failed;
        used_rule
            .reason_codes
            .push("style_reference_out_of_range".to_string());
    }
    rules.push(used_rule);
    rules.push(count_rule(
        "numbering.definitions",
        &policy.numbering.definitions,
        document.header.numberings.len(),
    ));
    let numbering_reasons = set_index_reasons(
        &stats.numbering_used,
        &policy.numbering.allowed_definition_indexes,
        &policy.numbering.required_definition_indexes,
    );
    rules.push(rule_from_reasons(
        "numbering.used",
        stats.numbering_used.len(),
        numbering_reasons,
    ));
    rules.push(count_rule("tables", &policy.tables, stats.tables));
    let mut link_reasons = count_reasons(&policy.links.count, stats.links);
    if !policy.links.allowed_schemes.is_empty()
        && stats.link_schemes.iter().any(|scheme| {
            !policy
                .links
                .allowed_schemes
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(scheme))
        })
    {
        link_reasons.push("disallowed_scheme".to_string());
    }
    rules.push(rule_from_reasons("links", stats.links, link_reasons));
    rules.push(metadata_rule(&policy.metadata, &document.metadata));
    rules.push(presence_rule("macros", policy.macros, raw.macros));
    let external = if stats.linked_bin_references > 0 {
        FeaturePresence::Present
    } else {
        raw.external_references
    };
    rules.push(presence_rule(
        "external_references",
        policy.external_references,
        external,
    ));
    let accessibility_missing = if policy.accessibility.require_picture_descriptions {
        stats.picture_descriptions_missing
    } else {
        0
    } + if policy.accessibility.require_shape_descriptions {
        stats.shape_descriptions_missing
    } else {
        0
    };
    let mut reasons = Vec::new();
    if policy.accessibility.require_picture_descriptions && stats.picture_descriptions_missing > 0 {
        reasons.push("picture_description_missing".to_string());
    }
    if policy.accessibility.require_shape_descriptions && stats.shape_descriptions_missing > 0 {
        reasons.push("shape_description_missing".to_string());
    }
    rules.push(rule_from_reasons(
        "accessibility",
        accessibility_missing,
        reasons,
    ));
    rules
}

fn named_set_rule(
    id: &'static str,
    policy: &NamedSetPolicy,
    observed: &BTreeSet<&str>,
) -> RuleResult {
    let mut reasons = Vec::new();
    if !policy.allowed.is_empty()
        && observed
            .iter()
            .any(|name| !policy.allowed.iter().any(|allowed| allowed == name))
    {
        reasons.push("disallowed_value".to_string());
    }
    if policy
        .required
        .iter()
        .any(|name| !observed.contains(name.as_str()))
    {
        reasons.push("required_value_missing".to_string());
    }
    rule_from_reasons(id, observed.len(), reasons)
}

fn set_index_reasons(
    observed: &BTreeSet<usize>,
    allowed: &[usize],
    required: &[usize],
) -> Vec<String> {
    let mut reasons = Vec::new();
    if !allowed.is_empty() && observed.iter().any(|index| !allowed.contains(index)) {
        reasons.push("disallowed_definition_index".to_string());
    }
    if required.iter().any(|index| !observed.contains(index)) {
        reasons.push("required_definition_index_missing".to_string());
    }
    reasons
}

fn count_rule(id: &'static str, policy: &CountPolicy, observed: usize) -> RuleResult {
    rule_from_reasons(id, observed, count_reasons(policy, observed))
}

fn count_reasons(policy: &CountPolicy, observed: usize) -> Vec<String> {
    let mut reasons = Vec::new();
    if policy.min.is_some_and(|minimum| observed < minimum) {
        reasons.push("below_minimum".to_string());
    }
    if policy.max.is_some_and(|maximum| observed > maximum) {
        reasons.push("above_maximum".to_string());
    }
    reasons
}

fn presence_rule(
    id: &'static str,
    policy: PresencePolicy,
    observed: FeaturePresence,
) -> RuleResult {
    let reasons = match (policy, observed) {
        (PresencePolicy::Deny, FeaturePresence::Present) => vec!["forbidden_present".to_string()],
        (PresencePolicy::Require, FeaturePresence::Absent) => vec!["required_missing".to_string()],
        (PresencePolicy::Deny | PresencePolicy::Require, FeaturePresence::Unknown) => {
            vec!["inspection_incomplete".to_string()]
        }
        _ => Vec::new(),
    };
    rule_from_reasons(
        id,
        usize::from(matches!(observed, FeaturePresence::Present)),
        reasons,
    )
}

fn metadata_rule(policy: &MetadataPolicy, metadata: &hwp_model::Metadata) -> RuleResult {
    let present: BTreeSet<MetadataField> = MetadataField::all()
        .into_iter()
        .filter(|field| metadata_present(metadata, *field))
        .collect();
    let mut reasons = Vec::new();
    if !policy.allowed_fields.is_empty()
        && present
            .iter()
            .any(|field| !policy.allowed_fields.contains(field))
    {
        reasons.push("disallowed_field_present".to_string());
    }
    if policy
        .required_fields
        .iter()
        .any(|field| !present.contains(field))
    {
        reasons.push("required_field_missing".to_string());
    }
    if policy
        .forbidden_fields
        .iter()
        .any(|field| present.contains(field))
    {
        reasons.push("forbidden_field_present".to_string());
    }
    rule_from_reasons("metadata", present.len(), reasons)
}

impl MetadataField {
    const fn all() -> [Self; 8] {
        [
            Self::Title,
            Self::Author,
            Self::Subject,
            Self::Keywords,
            Self::Description,
            Self::LastSavedBy,
            Self::CreateTime,
            Self::ModifyTime,
        ]
    }
}

fn metadata_present(metadata: &hwp_model::Metadata, field: MetadataField) -> bool {
    match field {
        MetadataField::Title => metadata
            .title
            .as_deref()
            .is_some_and(|value| !value.is_empty()),
        MetadataField::Author => metadata
            .author
            .as_deref()
            .is_some_and(|value| !value.is_empty()),
        MetadataField::Subject => metadata
            .subject
            .as_deref()
            .is_some_and(|value| !value.is_empty()),
        MetadataField::Keywords => metadata
            .keywords
            .as_deref()
            .is_some_and(|value| !value.is_empty()),
        MetadataField::Description => metadata
            .description
            .as_deref()
            .is_some_and(|value| !value.is_empty()),
        MetadataField::LastSavedBy => metadata
            .last_saved_by
            .as_deref()
            .is_some_and(|value| !value.is_empty()),
        MetadataField::CreateTime => metadata.create_time.is_some(),
        MetadataField::ModifyTime => metadata.modify_time.is_some(),
    }
}

fn rule_from_reasons(
    id: &'static str,
    observed_count: usize,
    reason_codes: Vec<String>,
) -> RuleResult {
    RuleResult {
        id,
        status: if reason_codes.is_empty() {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        },
        observed_count,
        reason_codes,
    }
}

struct RenderExecution {
    report: RenderReport,
    font_rule: RuleResult,
}

fn empty_render_report(dpi: f32) -> RenderReport {
    let empty_issues = hwp_render::RenderIssueAccumulator::new().finish();
    RenderReport {
        profile: "hwp-cli-native-certification-render-v1",
        dpi,
        total_pages: 0,
        selected_pages: Vec::new(),
        status: CheckStatus::Skipped,
        reason_codes: vec!["not_run".to_string()],
        fonts: Vec::new(),
        pages: Vec::new(),
        issues: Vec::new(),
        info: Vec::new(),
        issue_count: 0,
        info_count: 0,
        issue_log_complete: true,
        issue_sha256: empty_issues.sha256,
    }
}

/// Map the renderer's typed issue summary to the certification/report wire shape.
///
/// The renderer intentionally does not depend on serde or the CLI crate. Keep this
/// conversion in the CLI layer so every machine-readable report uses the same
/// code/severity/stage/sample semantics.
pub fn map_render_issue(issue: hwp_render::RenderIssueSummary) -> RenderIssueReportEntry {
    RenderIssueReportEntry {
        code: issue.code.as_str(),
        severity: issue.severity.as_str(),
        stage: issue.stage.as_str(),
        count: issue.count,
        sample_sha256: issue.sample_sha256,
        samples_complete: issue.samples_complete,
    }
}

/// Compute the canonical digest for the typed non-info issue channel.
pub fn canonical_render_issue_sha256(issues: &[RenderIssueReportEntry]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"hwp-render-typed-issues-v1\0");
    for issue in issues {
        for value in [issue.code, issue.severity, issue.stage] {
            hasher.update((value.len() as u64).to_le_bytes());
            hasher.update(value.as_bytes());
        }
        hasher.update(issue.count.to_le_bytes());
        hasher.update([u8::from(issue.samples_complete)]);
        hasher.update((issue.sample_sha256.len() as u64).to_le_bytes());
        for sample in &issue.sample_sha256 {
            hasher.update((sample.len() as u64).to_le_bytes());
            hasher.update(sample.as_bytes());
        }
    }
    hex_digest(hasher.finalize().as_slice())
}

fn apply_render_issue_report(target: &mut RenderReport, source: hwp_render::RenderIssueReport) {
    let issues: Vec<_> = source.issues.into_iter().map(map_render_issue).collect();
    let info: Vec<_> = source.info.into_iter().map(map_render_issue).collect();
    let issue_count = issues.iter().map(|issue| issue.count).sum();
    let info_count = info.iter().map(|issue| issue.count).sum();
    let issue_sha256 = canonical_render_issue_sha256(&issues);
    assert_eq!(issue_count, source.issue_count);
    assert_eq!(info_count, source.info_count);
    assert_eq!(issue_sha256, source.sha256);
    target.issues = issues;
    target.info = info;
    target.issue_count = issue_count;
    target.info_count = info_count;
    target.issue_log_complete = source.complete;
    target.issue_sha256 = issue_sha256;
}

fn validate_render_report_invariants(report: &RenderReport) -> Result<()> {
    let issue_count: u64 = report.issues.iter().map(|issue| issue.count).sum();
    let info_count: u64 = report.info.iter().map(|issue| issue.count).sum();
    if issue_count != report.issue_count || info_count != report.info_count {
        anyhow::bail!("render typed issue count invariant violated");
    }
    if canonical_render_issue_sha256(&report.issues) != report.issue_sha256 {
        anyhow::bail!("render typed issue hash invariant violated");
    }
    if (report.status == CheckStatus::Passed) != report.reason_codes.is_empty() {
        anyhow::bail!("render status/reason invariant violated");
    }
    Ok(())
}

fn validate_rule_composition(rules: &[RuleResult]) -> Result<()> {
    const IDS: [&str; 12] = [
        "defined_styles",
        "used_styles",
        "numbering.definitions",
        "numbering.used",
        "tables",
        "links",
        "metadata",
        "macros",
        "external_references",
        "accessibility",
        "unresolved_fields",
        "fonts",
    ];
    if !rules.is_empty()
        && (rules.len() != IDS.len()
            || rules
                .iter()
                .zip(IDS)
                .any(|(rule, expected)| rule.id != expected))
    {
        anyhow::bail!("certification rule composition invariant violated");
    }
    Ok(())
}

/// Content-free `hancom-verification-receipt-v1` artifact. Mirrors the closed receipt schema;
/// validated field-by-field after parsing because the CLI never ships the schema at runtime.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HancomVerificationReceipt {
    schema_version: String,
    application: String,
    result: String,
    verified_at: String,
    verifier: String,
    #[serde(default)]
    artifact_sha256: Option<String>,
}

fn valid_receipt_text(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= 256 && !value.chars().any(char::is_control)
}

fn valid_receipt_timestamp(value: &str) -> bool {
    fn digits(bytes: &[u8]) -> bool {
        bytes.iter().all(u8::is_ascii_digit)
    }
    let bytes = value.as_bytes();
    if !(20..=64).contains(&bytes.len())
        || !digits(&bytes[0..4])
        || bytes[4] != b'-'
        || !digits(&bytes[5..7])
        || bytes[7] != b'-'
        || !digits(&bytes[8..10])
        || bytes[10] != b'T'
        || !digits(&bytes[11..13])
        || bytes[13] != b':'
        || !digits(&bytes[14..16])
        || bytes[16] != b':'
        || !digits(&bytes[17..19])
    {
        return false;
    }
    // Optional fractional seconds, then `Z` or a `±HH:MM` offset.
    let mut tail = &value[19..];
    if let Some(rest) = tail.strip_prefix('.') {
        let Some(index) = rest.find(['Z', '+', '-']) else {
            return false;
        };
        if index == 0 || !digits(&rest.as_bytes()[..index]) {
            return false;
        }
        tail = &rest[index..];
    }
    if tail == "Z" {
        return true;
    }
    let offset = tail.as_bytes();
    offset.len() == 6
        && matches!(offset[0], b'+' | b'-')
        && digits(&offset[1..3])
        && offset[3] == b':'
        && digits(&offset[4..6])
}

fn valid_receipt(
    receipt: &HancomVerificationReceipt,
    input_sha256: &str,
    require_artifact_sha256: bool,
) -> bool {
    if require_artifact_sha256 && receipt.artifact_sha256.is_none() {
        // The policy binds this receipt to one artifact. A receipt with no hash names no
        // artifact, so it cannot be the evidence this policy asks for.
        return false;
    }
    receipt.schema_version == "1.0"
        && matches!(receipt.result.as_str(), "pass" | "fail")
        && valid_receipt_text(&receipt.application)
        && valid_receipt_text(&receipt.verifier)
        && valid_receipt_timestamp(&receipt.verified_at)
        && receipt.artifact_sha256.as_ref().is_none_or(|value| {
            // A supplied hash must be well-formed and bound to the certified input, so a
            // receipt produced for a different document cannot be replayed.
            validate_sha256(value, "receipt artifact sha256").is_ok() && value == input_sha256
        })
}

/// `preservation-report-v1` artifact. Unlike the writer-side model type, both fields are
/// required here so a malformed or stub artifact fails closed instead of reading as lossless.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreservationReportArtifact {
    contract: String,
    events: Vec<hwp_model::preservation::PreservationEvent>,
}

/// The preservation evidence check loads the policy-pinned `preservation-report-v1` artifact
/// and compares the aggregated loss total against the budget. Missing, oversized or malformed
/// artifacts fail closed.
fn evaluate_preservation_evidence(
    policy: &PreservationPolicy,
    policy_base: &Path,
) -> PreservationCheckReport {
    let invalid = || PreservationCheckReport {
        status: CheckStatus::Failed,
        reason_codes: vec!["preservation_report_invalid".to_string()],
        loss_code_count: 0,
        loss_codes: None,
    };
    let Ok(bytes) = read_bounded(
        &policy_base.join(&policy.report),
        MAX_EVIDENCE_ARTIFACT_BYTES,
    ) else {
        return invalid();
    };
    let Ok(report) = serde_json::from_slice::<PreservationReportArtifact>(&bytes) else {
        return invalid();
    };
    // Per-event counts stay inside the report schema's lossCount range so a huge or wrapping
    // artifact can never mint a schema-invalid section.
    if report.contract != hwp_model::preservation::PRESERVATION_REPORT_CONTRACT
        || report.events.len() > MAX_PRESERVATION_EVENTS
        || report
            .events
            .iter()
            .any(|event| !(1..=MAX_PRESERVATION_LOSS_COUNT).contains(&event.count))
    {
        return invalid();
    }
    let mut loss_codes: BTreeMap<String, usize> = BTreeMap::new();
    for event in &report.events {
        let entry = loss_codes
            .entry(event.code.as_str().to_string())
            .or_insert(0);
        let Some(total) = entry.checked_add(event.count) else {
            return invalid();
        };
        *entry = total;
    }
    let mut loss_code_count: usize = 0;
    for count in loss_codes.values() {
        let Some(total) = loss_code_count.checked_add(*count) else {
            return invalid();
        };
        loss_code_count = total;
    }
    let passed = loss_code_count <= policy.max_loss_codes;
    PreservationCheckReport {
        status: if passed {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        },
        reason_codes: if passed {
            Vec::new()
        } else {
            vec!["preservation_loss_detected".to_string()]
        },
        loss_code_count,
        loss_codes: (!loss_codes.is_empty()).then_some(loss_codes),
    }
}

/// The Hancom open evidence check loads the policy-pinned receipt artifact and requires a
/// `pass` result unless the policy relaxes it. Missing, oversized or malformed receipts fail
/// closed and echo nothing.
fn evaluate_hancom_open_evidence(
    policy: &HancomOpenPolicy,
    policy_base: &Path,
    input_sha256: &str,
) -> HancomOpenCheckReport {
    let invalid = || HancomOpenCheckReport {
        status: CheckStatus::Failed,
        reason_codes: vec!["hancom_open_receipt_invalid".to_string()],
        application: None,
        verified_at: None,
        verifier: None,
    };
    let Ok(bytes) = read_bounded(
        &policy_base.join(&policy.receipt),
        MAX_EVIDENCE_ARTIFACT_BYTES,
    ) else {
        return invalid();
    };
    let Ok(receipt) = serde_json::from_slice::<HancomVerificationReceipt>(&bytes) else {
        return invalid();
    };
    if !valid_receipt(&receipt, input_sha256, policy.require_artifact_sha256) {
        return invalid();
    }
    let passed = !policy.require_pass || receipt.result == "pass";
    HancomOpenCheckReport {
        status: if passed {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        },
        reason_codes: if passed {
            Vec::new()
        } else {
            vec!["hancom_open_not_attested".to_string()]
        },
        application: Some(receipt.application),
        verified_at: Some(receipt.verified_at),
        verifier: Some(receipt.verifier),
    }
}

/// Post-build invariants for the optional evidence sections: status/reason consistency,
/// aggregated preservation counts, and the overall-failure fold.
fn validate_evidence_check_invariants(report: &CertificationReport) -> Result<()> {
    let mut evidence_failed = false;
    if let Some(check) = &report.checks.preservation {
        if (check.status == CheckStatus::Passed) != check.reason_codes.is_empty() {
            anyhow::bail!("preservation status/reason invariant violated");
        }
        if check
            .loss_codes
            .as_ref()
            .is_some_and(|codes| codes.values().sum::<usize>() != check.loss_code_count)
        {
            anyhow::bail!("preservation loss count invariant violated");
        }
        evidence_failed |= check.status != CheckStatus::Passed;
    }
    if let Some(check) = &report.checks.hancom_open {
        if (check.status == CheckStatus::Passed) != check.reason_codes.is_empty() {
            anyhow::bail!("hancom_open status/reason invariant violated");
        }
        if check.status != CheckStatus::Passed {
            evidence_failed = true;
        }
    }
    if evidence_failed && report.overall != OverallStatus::Failed {
        anyhow::bail!("evidence failure must fail the overall certification");
    }
    Ok(())
}

fn validate_report_artifact_invariants(report: &CertificationReport) -> Result<()> {
    if report.artifacts.len() > MAX_REPORT_ARTIFACTS {
        anyhow::bail!("report artifact count exceeds {MAX_REPORT_ARTIFACTS}");
    }
    let mut paths = BTreeSet::new();
    for artifact in &report.artifacts {
        if !paths.insert(artifact.path.as_str()) {
            anyhow::bail!("duplicate certification artifact path");
        }
        let valid_page = artifact
            .path
            .strip_prefix("pages/page-")
            .and_then(|value| value.strip_suffix(".png"))
            .is_some_and(|digits| digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_digit()));
        if !valid_page && artifact.path != "oracle/import.pdf" {
            anyhow::bail!("certification artifact path is outside the fixed set");
        }
    }
    let oracle_pdf = paths.contains("oracle/import.pdf");
    let reported_pages: Vec<usize> = report.render.pages.iter().map(|page| page.page).collect();
    if reported_pages != report.render.selected_pages {
        anyhow::bail!("render page diagnostics do not match selected_pages");
    }
    let expected_page_artifacts: BTreeSet<String> = report
        .render
        .selected_pages
        .iter()
        .map(|page| format!("pages/page-{page:06}.png"))
        .collect();
    let actual_page_artifacts: BTreeSet<String> = paths
        .iter()
        .filter(|path| path.starts_with("pages/"))
        .map(|path| (*path).to_string())
        .collect();
    if actual_page_artifacts != expected_page_artifacts {
        anyhow::bail!("render page artifacts do not match selected_pages");
    }
    if report.scope == "native_plus_independent_import" {
        if report.oracle.status != OracleStatus::Passed || !oracle_pdf {
            anyhow::bail!("independent-import scope requires passed oracle/import.pdf");
        }
    } else if oracle_pdf {
        anyhow::bail!("native-only scope cannot contain oracle/import.pdf");
    }
    Ok(())
}

struct RenderArtifactTransaction {
    temporary_root: PathBuf,
    final_pages: PathBuf,
    committed: bool,
}

impl RenderArtifactTransaction {
    fn new(stage_root: &Path) -> Result<Self> {
        let temporary_root = stage_root.join(".render-artifacts");
        let final_pages = stage_root.join("pages");
        if fs::symlink_metadata(&temporary_root).is_ok()
            || fs::symlink_metadata(&final_pages).is_ok()
        {
            anyhow::bail!("render artifact transaction paths already exist");
        }
        fs::create_dir(&temporary_root)?;
        set_private_permissions(&temporary_root)?;
        fs::create_dir(temporary_root.join("pages"))?;
        set_private_permissions(&temporary_root.join("pages"))?;
        Ok(Self {
            temporary_root,
            final_pages,
            committed: false,
        })
    }

    fn write(&self, relative: &str, bytes: &[u8]) -> Result<ArtifactReport> {
        write_artifact(&self.temporary_root, relative, bytes, true)
    }

    fn commit(mut self) -> Result<()> {
        fs::rename(self.temporary_root.join("pages"), &self.final_pages)?;
        fs::remove_dir(&self.temporary_root)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for RenderArtifactTransaction {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_dir_all(&self.temporary_root);
            let _ = fs::remove_dir_all(&self.final_pages);
        }
    }
}

fn ensure_pagination_consistency(counted: usize, rendered: usize) -> Result<()> {
    if counted != rendered {
        return Err(hwp_render::RenderError::PaginationDriftDetected { counted, rendered }.into());
    }
    Ok(())
}

fn run_render(
    document: &hwp_model::Document,
    policy: &CertificationPolicy,
    font_files: &[PathBuf],
    stage_root: &Path,
    _stats: &DocumentStats,
    artifacts: &mut Vec<ArtifactReport>,
) -> Result<RenderExecution> {
    let options = hwp_render::RenderOptions {
        dpi: policy.render.dpi,
        font_dirs: Vec::new(),
    };
    let layout_budget = hwp_render::layout::LayoutBudget::certification();
    let total_pages =
        hwp_render::count_pages_isolated_bounded(document, &options, font_files, &layout_budget)?;
    if total_pages > MAX_PAGE_NUMBER {
        anyhow::bail!("document page count가 {MAX_PAGE_NUMBER}쪽 상한을 초과합니다");
    }
    let selected_pages = match &policy.render.pages {
        PageSelection::All(_) => {
            if total_pages > MAX_SELECTED_PAGES {
                anyhow::bail!("all page selection이 {MAX_SELECTED_PAGES}쪽 상한을 초과합니다");
            }
            (1..=total_pages).collect::<Vec<_>>()
        }
        PageSelection::Selected(pages) => pages.clone(),
    };
    if selected_pages.iter().any(|page| *page > total_pages) {
        anyhow::bail!("selected page가 전체 페이지 수를 초과합니다");
    }
    if policy
        .render
        .allowed_blank_pages
        .iter()
        .any(|page| *page > total_pages)
    {
        anyhow::bail!("allowed blank page가 전체 페이지 수를 초과합니다");
    }
    let output = hwp_render::render_document_pages_isolated_bounded(
        document,
        &options,
        Some(&selected_pages),
        font_files,
        &layout_budget,
    )?;
    ensure_pagination_consistency(total_pages, output.total_pages)?;
    if output.pages.len() != selected_pages.len()
        || output.diagnostics.pages.len() != selected_pages.len()
    {
        anyhow::bail!("selected page render cardinality mismatch");
    }
    let transaction = RenderArtifactTransaction::new(stage_root)?;

    let mut status = CheckStatus::Passed;
    let mut page_reports = Vec::with_capacity(output.pages.len());
    let mut pending_artifacts = Vec::with_capacity(output.pages.len());
    for (pixmap, diagnostic) in output.pages.iter().zip(&output.diagnostics.pages) {
        let png = pixmap
            .encode_png()
            .map_err(|error| anyhow::anyhow!("PNG encode failed: {error}"))?;
        let relative = format!("pages/page-{:06}.png", diagnostic.page);
        let artifact = transaction.write(&relative, &png)?;
        let blank_disallowed = diagnostic.visually_blank
            && !policy.render.allowed_blank_pages.contains(&diagnostic.page);
        let outside_failed = policy.render.fail_on_outside_page_bounds
            && (!diagnostic.outside_page_bounds_complete
                || diagnostic.outside_page_bounds_count > 0);
        let collision_failed = policy.render.fail_on_potential_collision
            && (!diagnostic.possible_collision_complete || diagnostic.possible_collision_count > 0);
        if blank_disallowed || outside_failed || collision_failed {
            status = CheckStatus::Failed;
        }
        page_reports.push(PageReport {
            page: diagnostic.page,
            width_pt: diagnostic.width_pt,
            height_pt: diagnostic.height_pt,
            item_count: diagnostic.item_count,
            visual_blank: diagnostic.visually_blank,
            outside_page_bounds: detection_report(
                diagnostic.outside_page_bounds_count,
                diagnostic.outside_page_bounds_complete,
                "display_item_finite_bbox_vs_page_rect_v1",
            ),
            possible_collision: detection_report(
                diagnostic.possible_collision_count,
                diagnostic.possible_collision_complete,
                "cross_baseline_glyph_bbox_overlap_ge_0_25_v1",
            ),
            png_sha256: artifact.sha256.clone(),
            png_bytes: artifact.bytes,
        });
        pending_artifacts.push(artifact);
        let mut candidate_artifacts = artifacts.clone();
        candidate_artifacts.extend(pending_artifacts.iter().cloned());
        validate_artifact_budget(&candidate_artifacts)?;
    }
    if !page_count_matches(&policy.render.page_count, total_pages) {
        status = CheckStatus::Failed;
    }
    if !output.diagnostics.font_resolution_complete {
        status = CheckStatus::Failed;
    }
    if output.report.has_required_failure() {
        status = CheckStatus::Failed;
    }

    let font_rule = evaluate_font_rule(&policy.document.fonts, &output.diagnostics);
    if font_rule.status == CheckStatus::Failed {
        status = CheckStatus::Failed;
    }
    let mut fonts: Vec<FontReport> = output
        .diagnostics
        .fonts
        .iter()
        .map(|font| FontReport {
            requested_name_sha256: sha256_hex(font.requested.as_bytes()),
            resolved_family_sha256: font
                .resolved
                .as_deref()
                .map(|value| sha256_hex(value.as_bytes())),
            font_file_sha256: font.resolved_sha256.clone(),
            face_index: font.resolved_face_index,
            outcome: match font.outcome {
                hwp_render::FontResolutionOutcome::Matched => "matched",
                hwp_render::FontResolutionOutcome::Substituted => "substituted",
                hwp_render::FontResolutionOutcome::Missing => "missing",
                hwp_render::FontResolutionOutcome::CoverageSubstituted => "coverage_substituted",
            },
        })
        .collect();
    fonts.sort_by(|left, right| {
        (
            &left.requested_name_sha256,
            &left.font_file_sha256,
            left.face_index,
            left.outcome,
        )
            .cmp(&(
                &right.requested_name_sha256,
                &right.font_file_sha256,
                right.face_index,
                right.outcome,
            ))
    });
    // Certification v1 has no requested-weight field.  Keep render resolutions
    // weight-aware internally, but collapse report rows that differ only by
    // that omitted dimension.
    fonts.dedup();
    let mut report = RenderReport {
        profile: "hwp-cli-native-certification-render-v1",
        dpi: policy.render.dpi,
        total_pages,
        selected_pages,
        status,
        reason_codes: if status == CheckStatus::Passed {
            Vec::new()
        } else {
            vec!["render_policy_failed".to_string()]
        },
        fonts,
        pages: page_reports,
        issues: Vec::new(),
        info: Vec::new(),
        issue_count: 0,
        info_count: 0,
        issue_log_complete: true,
        issue_sha256: String::new(),
    };
    apply_render_issue_report(&mut report, output.report);
    transaction.commit()?;
    artifacts.extend(pending_artifacts);
    Ok(RenderExecution { report, font_rule })
}

fn render_failure_reason(error: &anyhow::Error) -> &'static str {
    if let Some(render_error) = error.downcast_ref::<hwp_render::RenderError>() {
        match render_error {
            hwp_render::RenderError::LayoutBudgetExceeded { .. } => {
                return "layout_budget_exceeded";
            }
            hwp_render::RenderError::ImageDecodeBudgetExceeded { .. } => {
                return "image_decode_budget_exceeded";
            }
            hwp_render::RenderError::PaginationDriftDetected { .. } => {
                return "pagination_drift_detected";
            }
            _ => {}
        }
    }
    let message = format!("{error:#}");
    if message.contains("상한") || message.contains("limit") || message.contains("budget") {
        "layout_budget_exceeded"
    } else if message.contains("font") || message.contains("글꼴") {
        "font_manifest_or_resolution_failed"
    } else if message.contains("page") || message.contains("페이지") {
        "render_page_scope_invalid"
    } else {
        "render_execution_failed"
    }
}

fn detection_report(count: usize, complete: bool, algorithm: &'static str) -> DetectionReport {
    DetectionReport {
        result: if !complete {
            "incomplete"
        } else if count == 0 {
            "not_detected"
        } else {
            "detected"
        },
        count,
        algorithm,
        complete,
    }
}

fn page_count_matches(policy: &PageCountPolicy, observed: usize) -> bool {
    policy.exact.is_none_or(|expected| observed == expected)
        && policy.min.is_none_or(|minimum| observed >= minimum)
        && policy.max.is_none_or(|maximum| observed <= maximum)
}

fn evaluate_font_rule(
    policy: &FontPolicy,
    diagnostics: &hwp_render::RenderDiagnostics,
) -> RuleResult {
    let mut reasons = Vec::new();
    let requested: BTreeSet<&str> = diagnostics
        .fonts
        .iter()
        .filter(|font| font.requested != "coverage_fallback")
        .map(|font| font.requested.as_str())
        .collect();
    if requested
        .iter()
        .any(|name| name.len() > 256 || name.chars().any(char::is_control))
    {
        reasons.push("requested_font_name_invalid".to_string());
    }
    if !policy.allowed_requested.is_empty()
        && requested.iter().any(|name| {
            !policy
                .allowed_requested
                .iter()
                .any(|allowed| allowed == *name)
        })
    {
        reasons.push("disallowed_requested_font".to_string());
    }
    if policy
        .required_requested
        .iter()
        .any(|name| !requested.contains(name.as_str()))
    {
        reasons.push("required_requested_font_missing".to_string());
    }
    if diagnostics
        .fonts
        .iter()
        .any(|font| matches!(font.outcome, hwp_render::FontResolutionOutcome::Missing))
    {
        reasons.push("font_or_glyph_missing".to_string());
    }
    if policy.forbid_substitution
        && diagnostics.fonts.iter().any(|font| {
            matches!(
                font.outcome,
                hwp_render::FontResolutionOutcome::Substituted
                    | hwp_render::FontResolutionOutcome::CoverageSubstituted
            )
        })
    {
        reasons.push("font_substitution_forbidden".to_string());
    }
    if !diagnostics.font_resolution_complete {
        reasons.push("font_resolution_report_incomplete".to_string());
    }
    let manifest_hashes: BTreeSet<&str> = policy
        .manifest
        .iter()
        .map(|pin| pin.sha256.as_str())
        .collect();
    if diagnostics.fonts.iter().any(|font| {
        font.resolved_sha256
            .as_deref()
            .is_some_and(|hash| !manifest_hashes.contains(hash))
    }) {
        reasons.push("resolved_font_outside_manifest".to_string());
    }
    reasons.sort();
    reasons.dedup();
    rule_from_reasons("fonts", requested.len(), reasons)
}

#[derive(Debug)]
struct SnapshotReport {
    bytes: u64,
    sha256: String,
}

fn snapshot_file(
    source: &Path,
    destination: &Path,
    max_bytes: u64,
    expected_sha256: Option<&str>,
) -> Result<SnapshotReport> {
    let mut input = File::open(source)
        .with_context(|| format!("snapshot input open failed: {}", source.display()))?;
    let opened_before = input.metadata()?;
    let path_before = fs::symlink_metadata(source)?;
    if path_before.file_type().is_symlink() || !path_before.file_type().is_file() {
        anyhow::bail!("snapshot input must be a non-symlink regular file");
    }
    if has_multiple_links(source, &path_before) {
        anyhow::bail!("snapshot input must not have hardlink aliases");
    }
    if !open_file_still_matches_path(&input, source) {
        anyhow::bail!("snapshot input path changed before copy");
    }
    if opened_before.len() > max_bytes {
        anyhow::bail!("snapshot input exceeds {max_bytes} byte limit");
    }
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    let mut destination_guard = PartialFileGuard {
        path: destination.to_path_buf(),
        keep: false,
    };
    let mut hasher = Sha256::new();
    let mut copied = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        copied = copied
            .checked_add(read as u64)
            .context("snapshot size overflow")?;
        if copied > max_bytes || copied > opened_before.len() {
            anyhow::bail!("snapshot input changed or exceeds limit");
        }
        output.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
    }
    if copied != opened_before.len() {
        anyhow::bail!("snapshot input length changed during copy");
    }
    output.sync_all()?;
    let opened_after = input.metadata()?;
    let path_after = fs::symlink_metadata(source)?;
    if path_after.file_type().is_symlink()
        || !open_file_still_matches_path(&input, source)
        || opened_before.len() != opened_after.len()
        || opened_before.len() != path_after.len()
        || opened_before.modified()? != opened_after.modified()?
        || opened_before.modified()? != path_after.modified()?
    {
        anyhow::bail!("snapshot input path or content changed during copy");
    }
    let sha256 = hex_digest(hasher.finalize().as_slice());
    if expected_sha256.is_some_and(|expected| expected != sha256) {
        anyhow::bail!("snapshot sha256 does not match pin");
    }
    destination_guard.keep = true;
    Ok(SnapshotReport {
        bytes: copied,
        sha256,
    })
}

struct PartialFileGuard {
    path: PathBuf,
    keep: bool,
}

impl Drop for PartialFileGuard {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// 열어 둔 핸들이 지금도 `path`가 가리키는 바로 그 파일인지. 신원을 못 읽으면
/// "그대로다"를 증명할 수 없으므로 false(fail-closed)를 돌려준다.
#[cfg(unix)]
fn open_file_still_matches_path(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let (Ok(opened), Ok(current)) = (file.metadata(), fs::symlink_metadata(path)) else {
        return false;
    };
    opened.dev() == current.dev() && opened.ino() == current.ino()
}

#[cfg(windows)]
fn open_file_still_matches_path(file: &File, path: &Path) -> bool {
    match (windows_handle_info(file), windows_path_info(path)) {
        (Some(opened), Some(current)) => {
            opened.volume == current.volume && opened.index == current.index
        }
        _ => false,
    }
}

#[cfg(not(any(unix, windows)))]
fn open_file_still_matches_path(file: &File, path: &Path) -> bool {
    let (Ok(opened), Ok(current)) = (file.metadata(), fs::symlink_metadata(path)) else {
        return false;
    };
    opened.len() == current.len() && opened.modified().ok() == current.modified().ok()
}

/// Windows 파일 신원과 링크 수. std의 `Metadata` 경로(`volume_serial_number`·
/// `file_index`·`number_of_links`)는 nightly 전용 `windows_by_handle`이라 핸들에서 읽는다.
#[cfg(windows)]
struct WindowsFileInfo {
    volume: u32,
    index: u64,
    links: u32,
}

#[cfg(windows)]
fn windows_handle_info(file: &File) -> Option<WindowsFileInfo> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: 핸들은 호출 동안 살아 있는 File이 소유하고, 출력 구조체는 Win32 정의다.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return None;
    }
    Some(WindowsFileInfo {
        volume: information.dwVolumeSerialNumber,
        index: (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
        links: information.nNumberOfLinks,
    })
}

/// 링크를 따라가지 않고 연다 — 검사와 열기 사이에 reparse point로 바뀌었으면 링크 자신의
/// 신원이 나와 비교가 실패한다.
#[cfg(windows)]
fn windows_path_info(path: &Path) -> Option<WindowsFileInfo> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let file = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .ok()?;
    windows_handle_info(&file)
}

fn snapshot_font_manifest(
    manifest: &[FilePin],
    policy_base: &Path,
    destination: &Path,
) -> Result<Vec<PathBuf>> {
    if manifest
        .windows(2)
        .any(|window| window[0].path >= window[1].path)
    {
        anyhow::bail!("font manifest paths must be strictly sorted");
    }
    let mut total = 0u64;
    let mut hashes = BTreeSet::new();
    let mut files = Vec::with_capacity(manifest.len());
    for (index, pin) in manifest.iter().enumerate() {
        if !hashes.insert(pin.sha256.as_str()) {
            anyhow::bail!("font manifest contains duplicate file identities");
        }
        let target = destination.join(format!("font-{index:04}.bin"));
        let snapshot = crate::asset_snapshot::read_contained(
            policy_base,
            Path::new(&pin.path),
            MAX_FONT_FILE_BYTES,
        )
        .map_err(|error| anyhow::anyhow!("font asset snapshot failed: {}", error.code.as_str()))?;
        let actual_hash = hex_digest(&snapshot.sha256);
        if actual_hash != pin.sha256 {
            anyhow::bail!("font asset checksum mismatch");
        }
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&target)?;
        output.write_all(&snapshot.data)?;
        output.sync_all()?;
        total = total
            .checked_add(snapshot.data.len() as u64)
            .context("font bytes overflow")?;
        if total > MAX_FONT_TOTAL_BYTES {
            anyhow::bail!("font manifest exceeds aggregate byte limit");
        }
        files.push(target);
    }
    Ok(files)
}

fn read_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > max_bytes {
        anyhow::bail!("file exceeds bounded read limit");
    }
    let mut file = File::open(path)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    std::io::Read::by_ref(&mut file)
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        anyhow::bail!("file exceeds bounded read limit");
    }
    Ok(bytes)
}

fn first_non_whitespace(bytes: &[u8]) -> Option<u8> {
    bytes
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
}

fn validate_relative_asset_path(value: &str) -> Result<()> {
    if value.is_empty()
        || value.chars().count() > 4096
        || value.contains('\0')
        || value.chars().any(char::is_control)
    {
        anyhow::bail!("asset path length/content invalid");
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        anyhow::bail!("asset path must contain only relative normal components");
    }
    Ok(())
}

fn validate_sha256(value: &str, label: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        anyhow::bail!("{label} must be exactly 64 lowercase hex characters");
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_digest(Sha256::digest(bytes).as_slice())
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hash_string_list(values: &[String]) -> String {
    let mut hasher = Sha256::new();
    for value in values {
        hasher.update((value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    hex_digest(hasher.finalize().as_slice())
}

fn with_final_newline(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.push(b'\n');
    bytes
}

fn write_artifact(
    root: &Path,
    relative: &str,
    bytes: &[u8],
    deterministic: bool,
) -> Result<ArtifactReport> {
    validate_relative_asset_path(relative)?;
    let path = root.join(relative);
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(ArtifactReport {
        path: relative.to_string(),
        bytes: bytes.len() as u64,
        sha256: sha256_hex(bytes),
        deterministic,
    })
}

fn validate_artifact_budget(artifacts: &[ArtifactReport]) -> Result<()> {
    if artifacts.len() > MAX_ARTIFACTS {
        anyhow::bail!("artifact count exceeds {MAX_ARTIFACTS}");
    }
    let total = artifacts.iter().try_fold(0u64, |total, artifact| {
        total
            .checked_add(artifact.bytes)
            .context("artifact byte overflow")
    })?;
    if total > MAX_ARTIFACT_TOTAL_BYTES {
        anyhow::bail!("artifact bytes exceed {MAX_ARTIFACT_TOTAL_BYTES}");
    }
    Ok(())
}

fn set_private_permissions(path: &Path) -> Result<()> {
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn build_artifact_manifest(files: &[ArtifactReport], files_total: u64) -> Result<Vec<u8>> {
    let count = files
        .len()
        .checked_add(1)
        .context("manifest count overflow")?;
    if files.len() > MAX_MANIFEST_FILES || count > MAX_PUBLISHED_TREE_ENTRIES {
        anyhow::bail!("artifact count including manifest exceeds {MAX_PUBLISHED_TREE_ENTRIES}");
    }
    let mut manifest_size = 0u64;
    for _ in 0..16 {
        let total = files_total
            .checked_add(manifest_size)
            .context("manifest total overflow")?;
        let manifest = serde_json::json!({
            "schema_version": "1.0",
            "contract": "hwp-certification-artifact-manifest-v1",
            "artifact_count": count,
            "total_bytes": total,
            "files": files,
            "self": {
                "path": "manifest.json",
                "bytes": manifest_size,
                "sha256": null,
                "deterministic": true,
                "reason": "self_hash_not_representable"
            }
        });
        let bytes = with_final_newline(serde_json::to_vec_pretty(&manifest)?);
        let next = bytes.len() as u64;
        if next == manifest_size {
            if total > MAX_ARTIFACT_TOTAL_BYTES {
                anyhow::bail!("artifact bytes including manifest exceed limit");
            }
            return Ok(bytes);
        }
        manifest_size = next;
    }
    anyhow::bail!("manifest self-size did not converge")
}

#[cfg(windows)]
fn exact_ordinary_spelling(canonical: &Path) -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};

    const SLASH: u16 = b'\\' as u16;
    const VERBATIM: [u16; 4] = [SLASH, SLASH, b'?' as u16, SLASH];
    const VERBATIM_UNC: [u16; 8] = [
        SLASH,
        SLASH,
        b'?' as u16,
        SLASH,
        b'U' as u16,
        b'N' as u16,
        b'C' as u16,
        SLASH,
    ];

    let wide: Vec<u16> = canonical.as_os_str().encode_wide().collect();
    if wide.starts_with(&VERBATIM_UNC) {
        let mut ordinary = Vec::with_capacity(wide.len() - VERBATIM_UNC.len() + 2);
        ordinary.extend_from_slice(&[SLASH, SLASH]);
        ordinary.extend_from_slice(&wide[VERBATIM_UNC.len()..]);
        return Some(PathBuf::from(OsString::from_wide(&ordinary)));
    }
    if wide.starts_with(&VERBATIM)
        && wide.get(5) == Some(&(b':' as u16))
        && wide
            .get(4)
            .is_some_and(|letter| matches!(*letter, 65..=90 | 97..=122))
    {
        return Some(PathBuf::from(OsString::from_wide(&wide[VERBATIM.len()..])));
    }
    None
}

#[cfg(windows)]
fn can_preserve_ordinary_certification_parent(
    requested_parent: &Path,
    canonical_parent: &Path,
    report_name: &std::ffi::OsStr,
) -> bool {
    use std::os::windows::ffi::OsStrExt as _;

    const LEGACY_MAX_PATH_UTF16: usize = 248;
    if !requested_parent.is_absolute()
        || exact_ordinary_spelling(canonical_parent).as_deref() != Some(requested_parent)
    {
        return false;
    }
    let same_identity = matches!(
        (
            windows_path_info(requested_parent),
            windows_path_info(canonical_parent)
        ),
        (Some(requested), Some(canonical))
            if requested.volume == canonical.volume && requested.index == canonical.index
    );
    if !same_identity {
        return false;
    }

    requested_parent
        .as_os_str()
        .encode_wide()
        .count()
        .checked_add(1)
        .and_then(|units| units.checked_add(report_name.encode_wide().count()))
        .and_then(|units| units.checked_add(WINDOWS_CERTIFICATION_TREE_OVERHEAD_UTF16))
        .and_then(|units| units.checked_add(1))
        .is_some_and(|units_with_nul| units_with_nul < LEGACY_MAX_PATH_UTF16)
}

struct AtomicCertificationDir {
    root: PathBuf,
    destination: PathBuf,
    parent: PathBuf,
    published: bool,
}

impl AtomicCertificationDir {
    fn new(destination: &Path) -> Result<Self> {
        if fs::symlink_metadata(destination).is_ok() {
            anyhow::bail!("certification report directory must not already exist");
        }
        let name = destination
            .file_name()
            .filter(|name| !name.is_empty())
            .context("report directory needs a final component")?;
        let requested_parent = destination.parent().unwrap_or_else(|| Path::new("."));
        let canonical_parent = requested_parent
            .canonicalize()
            .context("report parent directory is unavailable")?;
        if !fs::metadata(&canonical_parent)?.is_dir() {
            anyhow::bail!("report parent is not a directory");
        }
        #[cfg(windows)]
        let parent = if can_preserve_ordinary_certification_parent(
            requested_parent,
            &canonical_parent,
            name,
        ) {
            // MCP may already have selected an ordinary Win32 spelling that stays
            // below the legacy path threshold. Preserve it only when it is exactly
            // the canonical path without its verbatim prefix and handle identity
            // proves the final directory did not change.
            requested_parent.to_path_buf()
        } else {
            canonical_parent
        };
        #[cfg(not(windows))]
        let parent = canonical_parent;
        let destination = parent.join(name);
        if fs::symlink_metadata(&destination).is_ok() {
            anyhow::bail!("certification report directory must not already exist");
        }
        let mut root = None;
        for _ in 0..128 {
            let token = random_token()?;
            let candidate = parent.join(format!(
                ".{}.hwp-certify-{}-{token}.tmp",
                name.to_string_lossy(),
                std::process::id()
            ));
            match create_private_workspace(&candidate) {
                Ok(()) => {
                    root = Some(candidate);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(Self {
            root: root.context("could not create unique certification workspace")?,
            destination,
            parent,
            published: false,
        })
    }

    fn publish(&mut self, expected_paths: &BTreeSet<String>) -> Result<()> {
        audit_published_tree(&self.root, expected_paths)?;
        sync_tree(&self.root)?;
        if fs::symlink_metadata(&self.destination).is_ok() {
            anyhow::bail!("report destination appeared during certification");
        }
        rename_directory_noreplace(&self.root, &self.destination)?;
        sync_parent_directory(&self.parent)?;
        self.published = true;
        Ok(())
    }
}

impl Drop for AtomicCertificationDir {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn random_token() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| anyhow::anyhow!("random token failed: {error}"))?;
    Ok(hex_digest(&bytes))
}

fn audit_published_tree(root: &Path, expected_paths: &BTreeSet<String>) -> Result<()> {
    fn walk(
        root: &Path,
        current: &Path,
        found: &mut BTreeSet<String>,
        directories: &mut BTreeSet<String>,
    ) -> Result<()> {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                anyhow::bail!("artifact tree contains a symlink");
            }
            if metadata.file_type().is_dir() {
                let relative = path
                    .strip_prefix(root)?
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                directories.insert(relative);
                walk(root, &path, found, directories)?;
            } else if metadata.file_type().is_file() {
                if has_multiple_links(&path, &metadata) {
                    anyhow::bail!("artifact tree contains a multiply-linked file");
                }
                let relative = path
                    .strip_prefix(root)?
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                found.insert(relative);
            } else {
                anyhow::bail!("artifact tree contains a non-file entry");
            }
        }
        Ok(())
    }
    let mut found = BTreeSet::new();
    let mut directories = BTreeSet::new();
    walk(root, root, &mut found, &mut directories)?;
    if &found != expected_paths {
        anyhow::bail!("artifact tree does not match fixed manifest allowlist");
    }
    let expected_directories: BTreeSet<String> = expected_paths
        .iter()
        .flat_map(|path| {
            let components: Vec<&str> = path.split('/').collect();
            (1..components.len()).map(move |end| components[..end].join("/"))
        })
        .collect();
    if directories != expected_directories {
        anyhow::bail!("artifact tree contains an unexpected directory");
    }
    Ok(())
}

/// 하드링크 별칭이 있는지. Windows는 링크 수를 핸들에서만 읽을 수 있으므로 열지 못하면
/// 판정 불가이며, 호출부가 모두 거부 조건으로 쓰므로 true(fail-closed)를 돌려준다.
#[cfg(unix)]
pub fn has_multiple_links(_path: &Path, metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    metadata.nlink() > 1
}

#[cfg(windows)]
pub fn has_multiple_links(path: &Path, _metadata: &fs::Metadata) -> bool {
    windows_path_info(path).is_none_or(|info| info.links > 1)
}

#[cfg(not(any(unix, windows)))]
pub fn has_multiple_links(_path: &Path, _metadata: &fs::Metadata) -> bool {
    false
}

fn sync_tree(root: &Path) -> Result<()> {
    fn sync(directory: &Path) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_dir() {
                sync(&path)?;
            } else if metadata.file_type().is_file() {
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&path)?
                    .sync_all()?;
            } else {
                anyhow::bail!("unsupported artifact entry during fsync");
            }
        }
        #[cfg(unix)]
        File::open(directory)?.sync_all()?;
        Ok(())
    }
    sync(root)
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> Result<()> {
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> Result<()> {
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn rename_directory_noreplace(source: &Path, destination: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;
    let source = CString::new(source.as_os_str().as_bytes())?;
    let destination = CString::new(destination.as_os_str().as_bytes())?;
    let result =
        unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
    if result == -1 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn rename_directory_noreplace(source: &Path, destination: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;
    let source = CString::new(source.as_os_str().as_bytes())?;
    let destination = CString::new(destination.as_os_str().as_bytes())?;
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == -1 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(windows)]
fn rename_directory_noreplace(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "linux", windows)))]
fn rename_directory_noreplace(_source: &Path, _destination: &Path) -> Result<()> {
    anyhow::bail!("atomic no-replace directory publish is unsupported on this platform")
}

#[cfg(unix)]
fn create_private_workspace(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    fs::DirBuilder::new().mode(0o700).create(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(windows)]
fn create_private_workspace(path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
    let sddl: Vec<u16> = "D:P(A;OICI;FA;;;OW)\0".encode_utf16().collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let created = unsafe { CreateDirectoryW(wide.as_ptr(), &attributes) };
    let error = (created == 0).then(std::io::Error::last_os_error);
    unsafe { LocalFree(descriptor) };
    error.map_or(Ok(()), Err)
}

#[cfg(not(any(unix, windows)))]
fn create_private_workspace(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "private workspace ACL unsupported",
    ))
}

fn expected_attestation(policy: &OraclePolicy) -> Option<OracleAttestation> {
    let config = policy.configuration.as_ref()?;
    Some(OracleAttestation {
        runtime_kind: "docker".to_string(),
        runtime_version: config.runtime.version.clone(),
        runtime_sha256: config.runtime.sha256.clone(),
        libreoffice_version: config.libreoffice.version.clone(),
        libreoffice_executable_sha256: config.libreoffice.executable_sha256.clone(),
        extension_version: config.extension.version.clone(),
        extension_sha256: config.extension.sha256.clone(),
        image_digest: config.image.digest.clone(),
        docker_client_version_sha256: None,
        docker_server_version_sha256: None,
        image_id: None,
        image_reference_sha256: None,
    })
}

struct TrustedOracleConfig {
    runtime_path: PathBuf,
    extension_path: PathBuf,
    image_reference: String,
    docker_client_version: String,
    docker_server_version: String,
    image_id: String,
}

impl TrustedOracleConfig {
    fn from_environment() -> Option<Self> {
        Some(Self {
            runtime_path: PathBuf::from(std::env::var_os("HWP_CERTIFY_ORACLE_RUNTIME")?),
            extension_path: PathBuf::from(std::env::var_os("HWP_CERTIFY_ORACLE_EXTENSION")?),
            image_reference: std::env::var("HWP_CERTIFY_ORACLE_IMAGE").ok()?,
            docker_client_version: std::env::var("HWP_CERTIFY_ORACLE_DOCKER_CLIENT_VERSION")
                .ok()?,
            docker_server_version: std::env::var("HWP_CERTIFY_ORACLE_DOCKER_SERVER_VERSION")
                .ok()?,
            image_id: std::env::var("HWP_CERTIFY_ORACLE_IMAGE_ID").ok()?,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OracleRunnerResult {
    schema_version: String,
    status: RunnerResultStatus,
    attestation: OracleAttestation,
    #[serde(default)]
    artifact: Option<OracleRunnerArtifact>,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RunnerResultStatus {
    Passed,
    Failed,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OracleRunnerArtifact {
    path: String,
    bytes: u64,
    sha256: String,
}

fn run_oracle(
    policy: &OraclePolicy,
    input_snapshot: &Path,
    input_extension: &str,
    stage_root: &Path,
) -> (OracleReport, Vec<ArtifactReport>) {
    if policy.mode == OracleMode::Disabled {
        return (
            OracleReport {
                mode: policy.mode,
                status: OracleStatus::Disabled,
                reason_code: None,
                expected: None,
                observed: None,
                stdout: None,
                stderr: None,
                artifact_determinism: "not_applicable",
            },
            Vec::new(),
        );
    }
    let expected = expected_attestation(policy);
    let unavailable = |reason: &str| {
        (
            OracleReport {
                mode: policy.mode,
                status: OracleStatus::OracleUnavailable,
                reason_code: Some(reason.to_string()),
                expected: expected.clone(),
                observed: None,
                stdout: None,
                stderr: None,
                artifact_determinism: "not_claimed",
            },
            Vec::new(),
        )
    };
    let Some(config) = policy.configuration.as_ref() else {
        return unavailable("policy_configuration_missing");
    };
    let Some(trusted) = TrustedOracleConfig::from_environment() else {
        return unavailable("trusted_runner_not_configured");
    };
    // 오라클은 process group kill에 의존한다(unix 전용). cfg! 로 두어 양쪽 플랫폼에서
    // 같은 코드가 컴파일되게 한다 — #[cfg] 블록이면 Windows에서 이후 문장이 죽어 경고가 난다.
    if cfg!(not(unix)) {
        let _ = (&trusted, input_snapshot, input_extension, stage_root);
        return unavailable("oracle_process_group_unavailable_on_platform");
    }
    if !trusted
        .image_reference
        .ends_with(&format!("@{}", config.image.digest))
    {
        return unavailable("trusted_image_digest_mismatch");
    }
    let runtime_snapshot = stage_root.join(".oracle-runtime");
    if snapshot_file(
        &trusted.runtime_path,
        &runtime_snapshot,
        64 * 1024 * 1024,
        Some(&config.runtime.sha256),
    )
    .is_err()
    {
        return unavailable("runtime_unavailable_or_mismatched");
    }
    let _runtime_cleanup = FileCleanup(runtime_snapshot.clone());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if fs::set_permissions(&runtime_snapshot, fs::Permissions::from_mode(0o700)).is_err() {
            let _ = fs::remove_file(&runtime_snapshot);
            return unavailable("runtime_snapshot_not_executable");
        }
    }
    let version_execution = match run_bounded_command(
        &runtime_snapshot,
        &["--version".to_string()],
        Duration::from_secs(10),
    ) {
        Ok(value) => value,
        Err(_) => {
            let _ = fs::remove_file(&runtime_snapshot);
            return unavailable("runtime_version_probe_failed");
        }
    };
    if !version_execution.success
        || String::from_utf8_lossy(&version_execution.stdout.captured).trim()
            != config.runtime.version
    {
        let _ = fs::remove_file(&runtime_snapshot);
        return unavailable("runtime_version_mismatch");
    }
    let daemon_execution = match run_bounded_command(
        &runtime_snapshot,
        &[
            "version".to_string(),
            "--format={{.Client.Version}}|{{.Server.Version}}".to_string(),
        ],
        Duration::from_secs(10),
    ) {
        Ok(value) => value,
        Err(_) => return unavailable("host_daemon_unattested"),
    };
    let expected_daemon = format!(
        "{}|{}",
        trusted.docker_client_version, trusted.docker_server_version
    );
    if !daemon_execution.success
        || String::from_utf8_lossy(&daemon_execution.stdout.captured).trim() != expected_daemon
    {
        return unavailable("host_daemon_unattested");
    }
    if !trusted.image_id.starts_with("sha256:")
        || validate_sha256(&trusted.image_id["sha256:".len()..], "trusted image id").is_err()
    {
        return unavailable("trusted_image_attestation_invalid");
    }
    let image_execution = match run_bounded_command(
        &runtime_snapshot,
        &[
            "image".to_string(),
            "inspect".to_string(),
            "--format={{.Id}}|{{join .RepoDigests \",\"}}".to_string(),
            trusted.image_reference.clone(),
        ],
        Duration::from_secs(10),
    ) {
        Ok(value) => value,
        Err(_) => return unavailable("image_attestation_probe_failed"),
    };
    let image_observation = String::from_utf8_lossy(&image_execution.stdout.captured);
    let Some((observed_image_id, repo_digests)) = image_observation.trim().split_once('|') else {
        return unavailable("image_attestation_probe_failed");
    };
    if !image_execution.success
        || observed_image_id != trusted.image_id
        || !repo_digests
            .split(',')
            .any(|digest| digest == trusted.image_reference)
    {
        return unavailable("image_attestation_mismatch");
    }

    let extension_snapshot = stage_root.join(".oracle-extension.oxt");
    let extension_source = trusted.extension_path;
    // The trusted extension location is administrator configuration, not policy-controlled.
    if snapshot_file(
        &extension_source,
        &extension_snapshot,
        64 * 1024 * 1024,
        Some(&config.extension.sha256),
    )
    .is_err()
    {
        let _ = fs::remove_file(&runtime_snapshot);
        return unavailable("extension_unavailable_or_mismatched");
    }
    let _extension_cleanup = FileCleanup(extension_snapshot.clone());
    let output_dir = stage_root.join(".oracle-output");
    if fs::create_dir(&output_dir).is_err() || set_private_permissions(&output_dir).is_err() {
        let _ = fs::remove_file(&extension_snapshot);
        return unavailable("oracle_workspace_unavailable");
    }
    let _output_cleanup = DirectoryCleanup(output_dir.clone());
    let input_mount = match input_snapshot.canonicalize() {
        Ok(path) => path,
        Err(_) => return unavailable("oracle_input_snapshot_unavailable"),
    };
    let extension_mount = match extension_snapshot.canonicalize() {
        Ok(path) => path,
        Err(_) => return unavailable("oracle_extension_snapshot_unavailable"),
    };
    if !valid_bind_mount_source(&input_mount) || !valid_bind_mount_source(&extension_mount) {
        return unavailable("oracle_mount_source_invalid");
    }
    let container_token = match random_token() {
        Ok(value) => value,
        Err(_) => return unavailable("container_identity_unavailable"),
    };
    let container_name = format!(
        "hwp-certify-{}-{}",
        std::process::id(),
        &container_token[..16]
    );
    let cidfile = stage_root.join(format!(".oracle-{container_token}.cid"));
    let oracle_input = format!("/input/document.{input_extension}");
    let mut args = vec![
        "run".to_string(),
        format!("--name={container_name}"),
        format!("--cidfile={}", cidfile.display()),
        "--pull=never".to_string(),
        "--network=none".to_string(),
        "--read-only".to_string(),
        "--cap-drop=ALL".to_string(),
        "--security-opt=no-new-privileges".to_string(),
        "--pids-limit=128".to_string(),
        "--memory=2g".to_string(),
        "--cpus=2".to_string(),
        "--tmpfs=/tmp:rw,noexec,nosuid,nodev,size=256m".to_string(),
        "--tmpfs=/home/oracle:rw,noexec,nosuid,nodev,size=64m".to_string(),
        "--tmpfs=/output:rw,noexec,nosuid,nodev,size=512m,mode=1777".to_string(),
        format!(
            "--mount=type=bind,src={},dst={oracle_input},readonly",
            input_mount.display()
        ),
        format!(
            "--mount=type=bind,src={},dst=/extension/H2Orestart.oxt,readonly",
            extension_mount.display()
        ),
        "--entrypoint=/usr/local/bin/hwp-certify-oracle".to_string(),
        trusted.image_reference.clone(),
        "--input".to_string(),
        oracle_input,
        "--extension".to_string(),
        "/extension/H2Orestart.oxt".to_string(),
        "--output".to_string(),
        "/output".to_string(),
        "--profile".to_string(),
        "/tmp/libreoffice-profile".to_string(),
        "--offline".to_string(),
        "--disable-macros".to_string(),
        "--disable-updates".to_string(),
        "--disable-external-links".to_string(),
        "--timeout-seconds".to_string(),
        "110".to_string(),
    ];
    #[cfg(unix)]
    {
        args.insert(
            13,
            format!("--user={}:{}", unsafe { libc::geteuid() }, unsafe {
                libc::getegid()
            }),
        );
    }
    #[cfg(not(unix))]
    {
        args.insert(12, "--user=65532:65532".to_string());
    }
    let execution = run_bounded_command(&runtime_snapshot, &args, ORACLE_TIMEOUT);
    let _ = fs::remove_file(&extension_snapshot);
    if execution.is_ok() {
        let _ = copy_container_artifact(
            &runtime_snapshot,
            &container_name,
            "/output/oracle-result.json",
            &output_dir.join("oracle-result.json"),
        );
        let _ = copy_container_artifact(
            &runtime_snapshot,
            &container_name,
            "/output/import.pdf",
            &output_dir.join("import.pdf"),
        );
    }
    let cleanup_verified = cleanup_container_verified(&runtime_snapshot, &container_name);
    let _ = fs::remove_file(&cidfile);
    if !cleanup_verified {
        return (
            OracleReport {
                mode: policy.mode,
                status: OracleStatus::Failed,
                reason_code: Some("container_cleanup_unverified".to_string()),
                expected,
                observed: None,
                stdout: None,
                stderr: None,
                artifact_determinism: "not_claimed",
            },
            Vec::new(),
        );
    }
    let execution = match execution {
        Ok(value) => value,
        Err(_) => {
            return (
                OracleReport {
                    mode: policy.mode,
                    status: OracleStatus::Failed,
                    reason_code: Some("runner_execution_failed".to_string()),
                    expected,
                    observed: None,
                    stdout: None,
                    stderr: None,
                    artifact_determinism: "not_claimed",
                },
                Vec::new(),
            );
        }
    };
    let stdout = Some(execution.stdout.report());
    let stderr = Some(execution.stderr.report());
    if execution.timed_out {
        let _ = fs::remove_dir_all(&output_dir);
        return (
            OracleReport {
                mode: policy.mode,
                status: OracleStatus::Failed,
                reason_code: Some("oracle_timeout".to_string()),
                expected,
                observed: None,
                stdout,
                stderr,
                artifact_determinism: "not_claimed",
            },
            Vec::new(),
        );
    }
    let output_files = match audit_oracle_output(&output_dir) {
        Ok(files) => files,
        Err(_) => {
            return (
                OracleReport {
                    mode: policy.mode,
                    status: OracleStatus::Failed,
                    reason_code: Some("oracle_output_allowlist_violation".to_string()),
                    expected,
                    observed: None,
                    stdout,
                    stderr,
                    artifact_determinism: "not_claimed",
                },
                Vec::new(),
            );
        }
    };
    let result_path = output_dir.join("oracle-result.json");
    let result_bytes = match read_bounded(&result_path, MAX_ORACLE_RESULT_BYTES) {
        Ok(value) => value,
        Err(_) => {
            let _ = fs::remove_dir_all(&output_dir);
            return (
                OracleReport {
                    mode: policy.mode,
                    status: OracleStatus::Failed,
                    reason_code: Some(
                        if execution.success {
                            "runner_contract_missing"
                        } else {
                            "runner_execution_failed"
                        }
                        .to_string(),
                    ),
                    expected,
                    observed: None,
                    stdout,
                    stderr,
                    artifact_determinism: "not_claimed",
                },
                Vec::new(),
            );
        }
    };
    let mut result: OracleRunnerResult = match serde_json::from_slice(&result_bytes) {
        Ok(value) => value,
        Err(_) => {
            let _ = fs::remove_dir_all(&output_dir);
            return (
                OracleReport {
                    mode: policy.mode,
                    status: OracleStatus::Failed,
                    reason_code: Some("runner_contract_invalid".to_string()),
                    expected,
                    observed: None,
                    stdout,
                    stderr,
                    artifact_determinism: "not_claimed",
                },
                Vec::new(),
            );
        }
    };
    let runner_claimed_host_attestation = result.attestation.docker_client_version_sha256.is_some()
        || result.attestation.docker_server_version_sha256.is_some()
        || result.attestation.image_id.is_some()
        || result.attestation.image_reference_sha256.is_some();
    result.attestation.docker_client_version_sha256 =
        Some(sha256_hex(trusted.docker_client_version.as_bytes()));
    result.attestation.docker_server_version_sha256 =
        Some(sha256_hex(trusted.docker_server_version.as_bytes()));
    result.attestation.image_id = Some(trusted.image_id.clone());
    result.attestation.image_reference_sha256 =
        Some(sha256_hex(trusted.image_reference.as_bytes()));
    let expected_output_files: BTreeSet<String> = if result.status == RunnerResultStatus::Passed {
        ["import.pdf".to_string(), "oracle-result.json".to_string()]
            .into_iter()
            .collect()
    } else {
        ["oracle-result.json".to_string()].into_iter().collect()
    };
    if output_files != expected_output_files {
        return (
            OracleReport {
                mode: policy.mode,
                status: OracleStatus::Failed,
                reason_code: Some("oracle_output_contract_mismatch".to_string()),
                expected,
                observed: None,
                stdout,
                stderr,
                artifact_determinism: "not_claimed",
            },
            Vec::new(),
        );
    }
    let _ = fs::remove_file(&result_path);
    if result.schema_version != "1.0"
        || runner_claimed_host_attestation
        || !expected
            .as_ref()
            .is_some_and(|value| base_attestation_matches(value, &result.attestation))
        || !valid_attestation(&result.attestation)
        || !execution.success
    {
        let _ = fs::remove_dir_all(&output_dir);
        return (
            OracleReport {
                mode: policy.mode,
                status: OracleStatus::Failed,
                reason_code: Some("runner_attestation_mismatch".to_string()),
                expected,
                observed: None,
                stdout,
                stderr,
                artifact_determinism: "not_claimed",
            },
            Vec::new(),
        );
    }
    if result.status == RunnerResultStatus::Failed {
        let _ = fs::remove_dir_all(&output_dir);
        return (
            OracleReport {
                mode: policy.mode,
                status: OracleStatus::Failed,
                reason_code: Some("conversion_failed".to_string()),
                expected,
                observed: Some(result.attestation),
                stdout,
                stderr,
                artifact_determinism: "not_claimed",
            },
            Vec::new(),
        );
    }
    let Some(artifact) = result.artifact else {
        let _ = fs::remove_dir_all(&output_dir);
        return (
            OracleReport {
                mode: policy.mode,
                status: OracleStatus::Failed,
                reason_code: Some("oracle_artifact_missing".to_string()),
                expected,
                observed: Some(result.attestation),
                stdout,
                stderr,
                artifact_determinism: "not_claimed",
            },
            Vec::new(),
        );
    };
    if artifact.path != "import.pdf"
        || validate_sha256(&artifact.sha256, "oracle artifact").is_err()
    {
        let _ = fs::remove_dir_all(&output_dir);
        return (
            OracleReport {
                mode: policy.mode,
                status: OracleStatus::Failed,
                reason_code: Some("oracle_artifact_contract_invalid".to_string()),
                expected,
                observed: Some(result.attestation),
                stdout,
                stderr,
                artifact_determinism: "not_claimed",
            },
            Vec::new(),
        );
    }
    let imported = output_dir.join("import.pdf");
    if validate_pdf_structure(&imported).is_err() {
        return (
            OracleReport {
                mode: policy.mode,
                status: OracleStatus::Failed,
                reason_code: Some("oracle_pdf_invalid".to_string()),
                expected,
                observed: Some(result.attestation),
                stdout,
                stderr,
                artifact_determinism: "not_claimed",
            },
            Vec::new(),
        );
    }
    let final_oracle_dir = stage_root.join("oracle");
    if fs::create_dir(&final_oracle_dir).is_err()
        || set_private_permissions(&final_oracle_dir).is_err()
        || snapshot_file(
            &imported,
            &final_oracle_dir.join("import.pdf"),
            MAX_ARTIFACT_TOTAL_BYTES,
            Some(&artifact.sha256),
        )
        .is_err()
    {
        let _ = fs::remove_dir_all(&final_oracle_dir);
        return (
            OracleReport {
                mode: policy.mode,
                status: OracleStatus::Failed,
                reason_code: Some("oracle_artifact_copy_failed".to_string()),
                expected,
                observed: Some(result.attestation),
                stdout,
                stderr,
                artifact_determinism: "not_claimed",
            },
            Vec::new(),
        );
    }
    let final_pdf = final_oracle_dir.join("import.pdf");
    let actual = match artifact_from_existing(&final_pdf, "oracle/import.pdf", false) {
        Ok(value) if value.bytes == artifact.bytes && value.sha256 == artifact.sha256 => value,
        _ => {
            let _ = fs::remove_dir_all(&final_oracle_dir);
            return (
                OracleReport {
                    mode: policy.mode,
                    status: OracleStatus::Failed,
                    reason_code: Some("oracle_artifact_mismatch".to_string()),
                    expected,
                    observed: Some(result.attestation),
                    stdout,
                    stderr,
                    artifact_determinism: "not_claimed",
                },
                Vec::new(),
            );
        }
    };
    let _ = fs::remove_dir_all(&output_dir);
    (
        OracleReport {
            mode: policy.mode,
            status: OracleStatus::Passed,
            reason_code: None,
            expected,
            observed: Some(result.attestation),
            stdout,
            stderr,
            artifact_determinism: "not_claimed",
        },
        vec![actual],
    )
}

fn audit_oracle_output(directory: &Path) -> Result<BTreeSet<String>> {
    let allowed = ["oracle-result.json", "import.pdf"];
    let mut files = BTreeSet::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let metadata = fs::symlink_metadata(entry.path())?;
        if !allowed.contains(&name.as_str())
            || metadata.file_type().is_symlink()
            || !metadata.file_type().is_file()
            || has_multiple_links(&entry.path(), &metadata)
        {
            anyhow::bail!("oracle output violates fixed allowlist");
        }
        files.insert(name);
    }
    if !files.contains("oracle-result.json") {
        anyhow::bail!("oracle result missing");
    }
    Ok(files)
}

fn valid_attestation(attestation: &OracleAttestation) -> bool {
    if attestation.runtime_kind != "docker" {
        return false;
    }
    for value in [
        &attestation.runtime_version,
        &attestation.libreoffice_version,
        &attestation.extension_version,
    ] {
        if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            return false;
        }
    }
    validate_sha256(&attestation.runtime_sha256, "runtime").is_ok()
        && validate_sha256(&attestation.libreoffice_executable_sha256, "libreoffice").is_ok()
        && validate_sha256(&attestation.extension_sha256, "extension").is_ok()
        && attestation.image_digest.starts_with("sha256:")
        && validate_sha256(&attestation.image_digest["sha256:".len()..], "image").is_ok()
        && attestation
            .docker_client_version_sha256
            .as_deref()
            .is_some_and(|value| validate_sha256(value, "docker client version").is_ok())
        && attestation
            .docker_server_version_sha256
            .as_deref()
            .is_some_and(|value| validate_sha256(value, "docker server version").is_ok())
        && attestation.image_id.as_deref().is_some_and(|value| {
            value.starts_with("sha256:")
                && validate_sha256(&value["sha256:".len()..], "image id").is_ok()
        })
        && attestation
            .image_reference_sha256
            .as_deref()
            .is_some_and(|value| validate_sha256(value, "image reference").is_ok())
}

fn base_attestation_matches(expected: &OracleAttestation, observed: &OracleAttestation) -> bool {
    expected.runtime_kind == observed.runtime_kind
        && expected.runtime_version == observed.runtime_version
        && expected.runtime_sha256 == observed.runtime_sha256
        && expected.libreoffice_version == observed.libreoffice_version
        && expected.libreoffice_executable_sha256 == observed.libreoffice_executable_sha256
        && expected.extension_version == observed.extension_version
        && expected.extension_sha256 == observed.extension_sha256
        && expected.image_digest == observed.image_digest
}

fn validate_pdf_structure(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || has_multiple_links(path, &metadata)
        || metadata.len() < 16
        || metadata.len() > MAX_ARTIFACT_TOTAL_BYTES
    {
        anyhow::bail!("oracle PDF file type/size invalid");
    }
    let mut file = File::open(path)?;
    let mut header = [0u8; 5];
    file.read_exact(&mut header)?;
    if &header != b"%PDF-" {
        anyhow::bail!("oracle PDF header invalid");
    }
    let tail_len = metadata.len().min(4096) as usize;
    file.seek(SeekFrom::End(-(tail_len as i64)))?;
    let mut tail = vec![0u8; tail_len];
    file.read_exact(&mut tail)?;
    if !tail.windows(5).any(|window| window == b"%%EOF")
        || !tail.windows(9).any(|window| window == b"startxref")
    {
        anyhow::bail!("oracle PDF trailer invalid");
    }
    Ok(())
}

fn valid_bind_mount_source(path: &Path) -> bool {
    path.to_str().is_some_and(|value| {
        !value.is_empty()
            && !value.contains(',')
            && !value.contains('\0')
            && !value.chars().any(char::is_control)
    })
}

fn copy_container_artifact(
    runtime: &Path,
    name: &str,
    source: &'static str,
    destination: &Path,
) -> bool {
    let Some(destination) = destination.to_str() else {
        return false;
    };
    run_bounded_command(
        runtime,
        &[
            "cp".to_string(),
            format!("{name}:{source}"),
            destination.to_string(),
        ],
        Duration::from_secs(30),
    )
    .is_ok_and(|execution| execution.success && !execution.timed_out)
}

fn cleanup_container_verified(runtime: &Path, name: &str) -> bool {
    for _ in 0..3 {
        let _ = run_bounded_command(
            runtime,
            &["rm".to_string(), "-f".to_string(), name.to_string()],
            Duration::from_secs(10),
        );
        let Ok(inspect) = run_bounded_command(
            runtime,
            &["inspect".to_string(), name.to_string()],
            Duration::from_secs(10),
        ) else {
            continue;
        };
        let stderr = String::from_utf8_lossy(&inspect.stderr.captured);
        if !inspect.success
            && (stderr.contains("No such object") || stderr.contains("No such container"))
        {
            return true;
        }
    }
    false
}

struct FileCleanup(PathBuf);

impl Drop for FileCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct DirectoryCleanup(PathBuf);

impl Drop for DirectoryCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn artifact_from_existing(
    path: &Path,
    relative: &str,
    deterministic: bool,
) -> Result<ArtifactReport> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        anyhow::bail!("artifact must be a non-symlink regular file");
    }
    let sha256 = hash_regular_file(path, MAX_ARTIFACT_TOTAL_BYTES)?;
    Ok(ArtifactReport {
        path: relative.to_string(),
        bytes: metadata.len(),
        sha256,
        deterministic,
    })
}

fn hash_regular_file(path: &Path, max_bytes: u64) -> Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() > max_bytes
    {
        anyhow::bail!("hash target is not an allowed regular file");
    }
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > max_bytes {
            anyhow::bail!("hash target exceeds limit");
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_digest(hasher.finalize().as_slice()))
}

struct BoundedExecution {
    success: bool,
    timed_out: bool,
    stdout: PipeDigest,
    stderr: PipeDigest,
}

struct PipeDigest {
    observed: u64,
    hashed: u64,
    truncated: bool,
    sha256: String,
    captured: Vec<u8>,
}

impl PipeDigest {
    fn report(&self) -> LogReport {
        LogReport {
            bytes_observed: self.observed,
            bytes_hashed: self.hashed,
            truncated: self.truncated,
            sha256: self.sha256.clone(),
        }
    }
}

fn run_bounded_command(
    program: &Path,
    args: &[String],
    timeout: Duration,
) -> Result<BoundedExecution> {
    let mut command = Command::new(program);
    command
        .args(args)
        .env_clear()
        .env("LANG", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // SAFETY: pre_exec only calls async-signal-safe setpgid and does not allocate.
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command.spawn()?;
    // process group kill(unix)에서만 쓴다. 다른 플랫폼은 child.kill() 경로.
    #[cfg_attr(not(unix), allow(unused_variables))]
    let pid = child.id();
    let stdout = child.stdout.take().context("missing child stdout")?;
    let stderr = child.stderr.take().context("missing child stderr")?;
    let stdout_thread = std::thread::spawn(move || digest_pipe(stdout));
    let stderr_thread = std::thread::spawn(move || digest_pipe(stderr));
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            #[cfg(unix)]
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            #[cfg(not(unix))]
            {
                let _ = child.kill();
            }
            break child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let stdout = stdout_thread
        .join()
        .map_err(|_| anyhow::anyhow!("stdout reader panicked"))??;
    let stderr = stderr_thread
        .join()
        .map_err(|_| anyhow::anyhow!("stderr reader panicked"))??;
    Ok(BoundedExecution {
        success: status.success(),
        timed_out,
        stdout,
        stderr,
    })
}

fn digest_pipe(mut pipe: impl Read) -> Result<PipeDigest> {
    let mut hasher = Sha256::new();
    let mut observed = 0u64;
    let mut hashed = 0u64;
    let mut captured = Vec::new();
    let mut buffer = [0u8; 8 * 1024];
    loop {
        let read = pipe.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        observed = observed.saturating_add(read as u64);
        let remaining = MAX_LOG_BYTES_RECORDED.saturating_sub(hashed) as usize;
        let accepted = remaining.min(read);
        if accepted > 0 {
            hasher.update(&buffer[..accepted]);
            hashed += accepted as u64;
            let capture_remaining = 4096usize.saturating_sub(captured.len());
            captured.extend_from_slice(&buffer[..accepted.min(capture_remaining)]);
        }
    }
    Ok(PipeDigest {
        observed,
        hashed,
        truncated: observed > hashed,
        sha256: hex_digest(hasher.finalize().as_slice()),
        captured,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "hwp-certify-{label}-{}-{}",
            std::process::id(),
            random_token().unwrap()
        ));
        let _ = fs::remove_file(&path);
        path
    }

    #[cfg(windows)]
    #[test]
    fn canonical_parent_ordinary_spelling_does_not_accept_ancestor_aliases() {
        let canonical = Path::new(r"\\?\C:\target\child");
        assert_eq!(
            exact_ordinary_spelling(canonical).unwrap(),
            PathBuf::from(r"C:\target\child")
        );
        assert_ne!(
            exact_ordinary_spelling(canonical).unwrap(),
            PathBuf::from(r"C:\base\junction\child")
        );
    }

    #[cfg(windows)]
    #[test]
    fn atomic_certification_preserves_verified_ordinary_parent_spelling() {
        let requested_parent = scratch("ordinary-parent");
        fs::create_dir_all(&requested_parent).unwrap();
        let canonical_parent = requested_parent.canonicalize().unwrap();
        let parent = exact_ordinary_spelling(&canonical_parent).unwrap();
        let destination = parent.join("report");

        // Hosted Windows runners may expose the temp directory through an 8.3
        // alias (for example, RUNNER~1). Build the requested ordinary spelling
        // from the canonical path so this test exercises the exact-equivalence
        // branch rather than correctly falling back to verbatim spelling.
        let stage = AtomicCertificationDir::new(&destination).unwrap();
        assert_eq!(stage.parent, parent);
        assert!(stage.root.starts_with(&stage.parent));
        assert_eq!(stage.destination, destination);
        drop(stage);
        fs::remove_dir_all(parent).unwrap();
    }

    /// 하드링크 별칭 판정. Windows에서는 링크 수를 핸들에서만 읽을 수 있어 구현이 갈리므로
    /// 두 플랫폼 모두에서 돌려 회귀를 잡는다.
    #[test]
    fn hardlink_alias_is_detected_and_plain_file_is_not() {
        let plain = scratch("plain");
        fs::write(&plain, b"payload").unwrap();
        let metadata = fs::symlink_metadata(&plain).unwrap();
        assert!(!has_multiple_links(&plain, &metadata));

        let alias = scratch("alias");
        fs::hard_link(&plain, &alias).unwrap();
        let metadata = fs::symlink_metadata(&plain).unwrap();
        assert!(has_multiple_links(&plain, &metadata));

        fs::remove_file(&alias).unwrap();
        fs::remove_file(&plain).unwrap();
    }

    /// 열어 둔 핸들과 경로의 동일성. 경로가 다른 파일로 교체되면 false여야 한다(TOCTOU 게이트).
    #[test]
    fn open_handle_stops_matching_a_replaced_path() {
        let path = scratch("swap");
        fs::write(&path, b"original").unwrap();
        let opened = File::open(&path).unwrap();
        assert!(open_file_still_matches_path(&opened, &path));

        let replacement = scratch("swap-replacement");
        fs::write(&replacement, b"replacement").unwrap();
        fs::rename(&replacement, &path).unwrap();
        assert!(!open_file_still_matches_path(&opened, &path));

        drop(opened);
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn open_handle_does_not_match_a_missing_path() {
        let path = scratch("removed");
        fs::write(&path, b"payload").unwrap();
        let opened = File::open(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(!open_file_still_matches_path(&opened, &path));
    }

    fn policy_json(mode: &str) -> serde_json::Value {
        serde_json::json!({
            "schema_version": "1.0",
            "render": {"pages": "all"},
            "oracle": if mode == "disabled" {
                serde_json::json!({"mode": "disabled"})
            } else {
                serde_json::json!({
                    "mode": mode,
                    "configuration": {
                        "runtime": {"version": "Docker version 1", "sha256": "0".repeat(64)},
                        "libreoffice": {"version": "26.2.5", "executable_sha256": "1".repeat(64)},
                        "extension": {"version": "0.7.12", "sha256": "2".repeat(64)},
                        "image": {"digest": format!("sha256:{}", "3".repeat(64))}
                    }
                })
            }
        })
    }

    #[test]
    fn page_selection_all_has_stable_string_wire_form() {
        let policy: CertificationPolicy = serde_json::from_value(policy_json("disabled")).unwrap();
        validate_policy(&policy).unwrap();
        assert_eq!(
            serde_json::to_value(&policy).unwrap()["render"]["pages"],
            "all"
        );
    }

    #[test]
    fn policy_cannot_select_runtime_extension_or_image_paths() {
        let mut value = policy_json("required");
        value["oracle"]["configuration"]["runtime"]["path"] = serde_json::json!("/tmp/docker");
        value["oracle"]["configuration"]["extension"]["path"] = serde_json::json!("evil.oxt");
        value["oracle"]["configuration"]["image"]["reference"] = serde_json::json!("evil/image");
        assert!(serde_json::from_value::<CertificationPolicy>(value).is_err());
    }

    #[test]
    fn oracle_state_matrix_is_explicit() {
        assert_eq!(
            overall_status(false, OracleMode::Required, OracleStatus::OracleUnavailable),
            OverallStatus::Partial
        );
        assert_eq!(
            overall_status(false, OracleMode::Required, OracleStatus::Failed),
            OverallStatus::Failed
        );
        assert_eq!(
            overall_status(false, OracleMode::Optional, OracleStatus::OracleUnavailable),
            OverallStatus::Passed
        );
        assert_eq!(
            overall_status(false, OracleMode::Optional, OracleStatus::Failed),
            OverallStatus::Partial
        );
        assert_eq!(
            overall_status(false, OracleMode::Disabled, OracleStatus::Disabled),
            OverallStatus::Passed
        );
        assert_eq!(
            overall_status(true, OracleMode::Optional, OracleStatus::Passed),
            OverallStatus::Failed
        );
    }

    #[test]
    fn policy_runtime_enforces_schema_bounds() {
        let mut value = policy_json("disabled");
        value["render"]["pages"] = serde_json::json!([0]);
        let policy: CertificationPolicy = serde_json::from_value(value).unwrap();
        assert!(validate_policy(&policy).is_err());

        let mut value = policy_json("disabled");
        value["document"] = serde_json::json!({
            "tables": {"max": MAX_RULE_COUNT + 1}
        });
        let policy: CertificationPolicy = serde_json::from_value(value).unwrap();
        assert!(validate_policy(&policy).is_err());
    }

    #[test]
    fn manifest_counts_its_own_bytes() {
        let files = vec![ArtifactReport {
            path: "report.json".to_string(),
            bytes: 10,
            sha256: "0".repeat(64),
            deterministic: true,
        }];
        let bytes = build_artifact_manifest(&files, 10).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["artifact_count"], 2);
        assert_eq!(value["total_bytes"], 10 + bytes.len() as u64);
        assert_eq!(value["self"]["bytes"], bytes.len() as u64);
        assert_eq!(MAX_REPORT_ARTIFACTS, 257);
        assert_eq!(MAX_MANIFEST_FILES, 258);
        assert_eq!(MAX_PUBLISHED_TREE_ENTRIES, 259);
        assert_eq!(MAX_ARTIFACTS, 260);
    }

    #[test]
    fn report_schema_limits_match_the_shared_layout_budget() {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/certification-report-v1.schema.json"
        ))
        .unwrap();
        let page = &schema["properties"]["render"]["properties"]["pages"]["items"]["properties"];
        assert_eq!(
            page["page"]["maximum"],
            hwp_render::layout::CERTIFICATION_MAX_PAGES
        );
        assert_eq!(
            page["item_count"]["maximum"],
            hwp_render::layout::CERTIFICATION_MAX_DISPLAY_ITEMS
        );
        assert_eq!(
            schema["properties"]["artifacts"]["maxItems"],
            MAX_REPORT_ARTIFACTS
        );
        assert_eq!(
            schema["properties"]["render"]["properties"]["fonts"]["maxItems"],
            hwp_render::fonts::MAX_FONT_RESOLUTIONS
        );
    }

    #[test]
    fn table_pagination_issues_match_the_certification_schema() {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/certification-report-v1.schema.json"
        ))
        .unwrap();
        let issue_schema = serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$defs": {
                "sha256": schema["$defs"]["sha256"].clone(),
                "renderIssue": schema["$defs"]["renderIssue"].clone()
            },
            "$ref": "#/$defs/renderIssue"
        });
        let validator = jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .build(&issue_schema)
            .unwrap();

        let mut accumulator = hwp_render::RenderIssueAccumulator::new();
        accumulator.push_once(hwp_render::RenderIssueCode::TableSplitAcrossPages, b"split");
        accumulator.push_once(
            hwp_render::RenderIssueCode::TableRowTooTallClipped,
            b"oversized",
        );
        accumulator.push_once(
            hwp_render::RenderIssueCode::TableCellContentOverflow,
            b"overflow",
        );
        let mut report = empty_render_report(96.0);
        apply_render_issue_report(&mut report, accumulator.finish());

        for issue in report.issues.iter().chain(&report.info) {
            let value = serde_json::to_value(issue).unwrap();
            assert!(validator.is_valid(&value), "schema rejected {value}");
        }
    }

    #[test]
    fn structured_image_budget_error_is_not_classified_as_layout() {
        let error = anyhow::Error::new(hwp_render::RenderError::ImageDecodeBudgetExceeded {
            resource: "dimensions".to_string(),
        });
        assert_eq!(
            render_failure_reason(&error),
            "image_decode_budget_exceeded"
        );
    }

    #[test]
    fn pagination_drift_is_typed_and_contains_only_counts() {
        let error = ensure_pagination_consistency(7, 8).unwrap_err();
        assert_eq!(render_failure_reason(&error), "pagination_drift_detected");
        assert_eq!(
            error.to_string(),
            "pagination drift detected: counted=7, rendered=8"
        );
    }

    #[test]
    fn late_page_write_failure_rolls_back_all_render_artifacts() {
        let stage = std::env::temp_dir().join(format!(
            "hwp-cert-render-transaction-{}-{}",
            std::process::id(),
            random_token().unwrap()
        ));
        fs::create_dir(&stage).unwrap();
        {
            let transaction = RenderArtifactTransaction::new(&stage).unwrap();
            transaction
                .write("pages/page-000001.png", b"first")
                .unwrap();
            fs::create_dir(stage.join(".render-artifacts/pages/page-000002.png")).unwrap();
            assert!(
                transaction
                    .write("pages/page-000002.png", b"late-failure")
                    .is_err()
            );
        }
        assert!(!stage.join(".render-artifacts").exists());
        assert!(!stage.join("pages").exists());
        fs::remove_dir(stage).unwrap();
    }

    #[test]
    fn typed_render_issue_count_and_hash_invariants_are_enforced() {
        let mut render = empty_render_report(96.0);
        let mut issues = hwp_render::RenderIssueAccumulator::new();
        issues.push_once(hwp_render::RenderIssueCode::RenderExecutionFailed, b"fixed");
        apply_render_issue_report(&mut render, issues.finish());
        render.status = CheckStatus::Failed;
        render.reason_codes = vec!["render_execution_failed".to_string()];
        validate_render_report_invariants(&render).unwrap();

        render.issue_count += 1;
        assert!(validate_render_report_invariants(&render).is_err());
    }

    #[test]
    fn hwpx_xml_depth_129_is_rejected_before_semantic_parse() {
        let xml = format!("{}{}", "<n>".repeat(129), "</n>".repeat(129));
        let mut nodes = 0;
        let error = preflight_hwpx_xml("deep.xml", xml.as_bytes(), &mut nodes).unwrap_err();
        assert!(format!("{error:#}").contains("parse_budget_exceeded:hwpx_xml_depth"));
    }

    #[test]
    fn hwp5_level_1023_is_rejected_before_record_tree_build() {
        let mut writer = hwp5::codec::ByteWriter::new();
        hwp5::record::RecordHeader {
            tag: 0x10,
            level: 1023,
            size: 0,
        }
        .encode(&mut writer);
        let mut records = 0;
        let error = preflight_hwp5_record_stream(&writer.into_bytes(), &mut records).unwrap_err();
        assert!(format!("{error:#}").contains("parse_budget_exceeded:hwp5_record_depth"));
    }

    #[test]
    fn docker_mount_source_rejects_option_delimiters_and_controls() {
        assert!(valid_bind_mount_source(Path::new("/tmp/safe path")));
        assert!(!valid_bind_mount_source(Path::new("/tmp/source,readonly")));
        assert!(!valid_bind_mount_source(Path::new("/tmp/source\nnext")));
    }

    #[cfg(unix)]
    fn fake_runtime(script_body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let path = std::env::temp_dir().join(format!(
            "hwp-certify-fake-runtime-{}-{}",
            std::process::id(),
            random_token().unwrap()
        ));
        fs::write(&path, format!("#!/bin/sh\n{script_body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn container_cleanup_requires_verified_not_found() {
        let removed = fake_runtime(
            r#"if [ "$1" = "rm" ]; then exit 0; fi
if [ "$1" = "inspect" ]; then echo "Error: No such object" >&2; exit 1; fi
exit 2"#,
        );
        assert!(cleanup_container_verified(&removed, "fixed-container"));
        fs::remove_file(removed).unwrap();

        let leftover = fake_runtime(
            r#"if [ "$1" = "rm" ]; then exit 0; fi
if [ "$1" = "inspect" ]; then echo "still-present"; exit 0; fi
exit 2"#,
        );
        assert!(!cleanup_container_verified(&leftover, "fixed-container"));
        fs::remove_file(leftover).unwrap();
    }

    #[test]
    fn observed_attestation_never_serializes_private_registry_reference() {
        let policy: CertificationPolicy = serde_json::from_value(policy_json("required")).unwrap();
        let mut attestation = expected_attestation(&policy.oracle).unwrap();
        let private =
            "private.registry.example/secret/team/oracle@sha256:".to_string() + &"3".repeat(64);
        let client = "private-build/client-27";
        let server = "private-daemon/server-91";
        attestation.docker_client_version_sha256 = Some(sha256_hex(client.as_bytes()));
        attestation.docker_server_version_sha256 = Some(sha256_hex(server.as_bytes()));
        attestation.image_id = Some(format!("sha256:{}", "4".repeat(64)));
        attestation.image_reference_sha256 = Some(sha256_hex(private.as_bytes()));
        let serialized = serde_json::to_string(&attestation).unwrap();
        assert!(!serialized.contains("private.registry.example"));
        assert!(!serialized.contains("secret/team"));
        assert!(!serialized.contains(client));
        assert!(!serialized.contains(server));
    }

    fn evidence_scratch_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "hwp-certify-evidence-{label}-{}-{}",
            std::process::id(),
            random_token().unwrap()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        path
    }

    fn evidence_policy_value() -> serde_json::Value {
        let mut value = policy_json("disabled");
        value["document"] = serde_json::json!({
            "preservation": {"report": "preservation.json", "max_loss_codes": 2},
            "hancom_open": {"receipt": "hancom-receipt.json", "require_pass": true}
        });
        value
    }

    fn policy_with_evidence() -> CertificationPolicy {
        serde_json::from_value(evidence_policy_value()).unwrap()
    }

    #[test]
    fn evidence_policy_sections_parse_and_validate() {
        let policy = policy_with_evidence();
        validate_policy(&policy).unwrap();
        let preservation = policy.document.preservation.unwrap();
        assert_eq!(preservation.report, "preservation.json");
        assert_eq!(preservation.max_loss_codes, 2);
        let hancom_open = policy.document.hancom_open.unwrap();
        assert_eq!(hancom_open.receipt, "hancom-receipt.json");
        assert!(hancom_open.require_pass);

        // Defaults: max_loss_codes = 0, require_pass = true.
        let mut value = policy_json("disabled");
        value["document"] = serde_json::json!({
            "preservation": {"report": "preservation.json"},
            "hancom_open": {"receipt": "hancom-receipt.json"}
        });
        let policy: CertificationPolicy = serde_json::from_value(value).unwrap();
        validate_policy(&policy).unwrap();
        assert_eq!(policy.document.preservation.unwrap().max_loss_codes, 0);
        assert!(policy.document.hancom_open.unwrap().require_pass);

        // The sections stay closed and path-validated like the rest of the policy.
        let mut value = policy_json("disabled");
        value["document"] = serde_json::json!({
            "preservation": {"report": "preservation.json", "bogus": true}
        });
        assert!(serde_json::from_value::<CertificationPolicy>(value).is_err());
        let mut value = policy_json("disabled");
        value["document"] = serde_json::json!({
            "hancom_open": {"receipt": "../escape.json"}
        });
        let policy: CertificationPolicy = serde_json::from_value(value).unwrap();
        assert!(validate_policy(&policy).is_err());
    }

    #[test]
    fn evidence_policy_sections_match_the_policy_schema() {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/certification-policy-v1.schema.json"
        ))
        .unwrap();
        let validator = jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .build(&schema)
            .unwrap();
        let value = evidence_policy_value();
        assert!(validator.is_valid(&value), "schema rejected {value}");

        // A policy without the optional sections keeps its previous shape and stays valid.
        let minimal = policy_json("disabled");
        assert!(minimal.get("document").is_none());
        assert!(validator.is_valid(&minimal), "schema rejected {minimal}");
    }

    #[test]
    fn preservation_evidence_passes_within_the_loss_budget() {
        let dir = evidence_scratch_dir("preservation-pass");
        fs::write(
            dir.join("preservation.json"),
            serde_json::json!({
                "contract": "hwp-preservation-report-v1",
                "events": [
                    {"code": "control_removed", "resource": "control", "disposition": "removed", "count": 2},
                    {"code": "control_removed", "resource": "control", "disposition": "unrepresentable", "count": 1},
                    {"code": "metadata_value_removed", "resource": "metadata", "disposition": "removed", "count": 3}
                ]
            })
            .to_string(),
        )
        .unwrap();
        let policy = PreservationPolicy {
            report: "preservation.json".to_string(),
            max_loss_codes: 6,
        };
        let check = evaluate_preservation_evidence(&policy, &dir);
        assert_eq!(check.status, CheckStatus::Passed);
        assert!(check.reason_codes.is_empty());
        assert_eq!(check.loss_code_count, 6);
        let codes = check.loss_codes.unwrap();
        assert_eq!(codes["control_removed"], 3);
        assert_eq!(codes["metadata_value_removed"], 3);
        fs::remove_dir_all(&dir).unwrap();

        // A lossless report passes the default zero budget and omits loss_codes.
        let dir = evidence_scratch_dir("preservation-lossless");
        fs::write(
            dir.join("preservation.json"),
            serde_json::json!({"contract": "hwp-preservation-report-v1", "events": []}).to_string(),
        )
        .unwrap();
        let policy = PreservationPolicy {
            report: "preservation.json".to_string(),
            max_loss_codes: 0,
        };
        let check = evaluate_preservation_evidence(&policy, &dir);
        assert_eq!(check.status, CheckStatus::Passed);
        assert_eq!(check.loss_code_count, 0);
        assert!(check.loss_codes.is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn preservation_evidence_fails_above_the_loss_budget() {
        let dir = evidence_scratch_dir("preservation-loss");
        fs::write(
            dir.join("preservation.json"),
            serde_json::json!({
                "contract": "hwp-preservation-report-v1",
                "events": [
                    {"code": "picture_control_removed", "resource": "control", "disposition": "removed", "count": 2}
                ]
            })
            .to_string(),
        )
        .unwrap();
        let policy = PreservationPolicy {
            report: "preservation.json".to_string(),
            max_loss_codes: 1,
        };
        let check = evaluate_preservation_evidence(&policy, &dir);
        assert_eq!(check.status, CheckStatus::Failed);
        assert_eq!(check.reason_codes, ["preservation_loss_detected"]);
        assert_eq!(check.loss_code_count, 2);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn preservation_evidence_fails_closed_on_missing_or_invalid_artifacts() {
        let dir = evidence_scratch_dir("preservation-invalid");
        let policy = PreservationPolicy {
            report: "preservation.json".to_string(),
            max_loss_codes: 0,
        };
        let check = evaluate_preservation_evidence(&policy, &dir);
        assert_eq!(check.status, CheckStatus::Failed);
        assert_eq!(check.reason_codes, ["preservation_report_invalid"]);
        assert!(check.loss_codes.is_none());

        for (label, bytes) in [
            ("malformed", b"{not json".to_vec()),
            // Required fields must be present; a stub must not read as a lossless report.
            ("empty-object", serde_json::json!({}).to_string().into_bytes()),
            (
                "missing-events",
                serde_json::json!({"contract": "hwp-preservation-report-v1"})
                    .to_string()
                    .into_bytes(),
            ),
            (
                "wrong-contract",
                serde_json::json!({"contract": "other", "events": []})
                    .to_string()
                    .into_bytes(),
            ),
            (
                "zero-count",
                serde_json::json!({
                    "contract": "hwp-preservation-report-v1",
                    "events": [
                        {"code": "control_removed", "resource": "control", "disposition": "removed", "count": 0}
                    ]
                })
                .to_string()
                .into_bytes(),
            ),
            // Counts above the report schema's lossCount bound fail closed.
            (
                "oversized-count",
                serde_json::json!({
                    "contract": "hwp-preservation-report-v1",
                    "events": [
                        {"code": "control_removed", "resource": "control", "disposition": "removed", "count": 1_000_001}
                    ]
                })
                .to_string()
                .into_bytes(),
            ),
            (
                "huge-count",
                serde_json::json!({
                    "contract": "hwp-preservation-report-v1",
                    "events": [
                        {"code": "control_removed", "resource": "control", "disposition": "removed", "count": 18_446_744_073_709_551_615_u64}
                    ]
                })
                .to_string()
                .into_bytes(),
            ),
        ] {
            fs::write(dir.join("preservation.json"), bytes).unwrap();
            let check = evaluate_preservation_evidence(&policy, &dir);
            assert_eq!(check.status, CheckStatus::Failed, "{label}");
            assert_eq!(check.reason_codes, ["preservation_report_invalid"], "{label}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    fn write_receipt(dir: &Path, receipt: serde_json::Value) {
        fs::write(dir.join("hancom-receipt.json"), receipt.to_string()).unwrap();
    }

    fn valid_receipt_json() -> serde_json::Value {
        serde_json::json!({
            "schema_version": "1.0",
            "application": "Hancom Office HWP",
            "result": "pass",
            "verified_at": "2026-08-15T12:00:00Z",
            "verifier": "qa-operator-1",
            "artifact_sha256": "0".repeat(64)
        })
    }

    #[test]
    fn hancom_open_evidence_passes_and_echoes_the_receipt() {
        let dir = evidence_scratch_dir("hancom-pass");
        write_receipt(&dir, valid_receipt_json());
        let policy = HancomOpenPolicy {
            receipt: "hancom-receipt.json".to_string(),
            require_pass: true,
            require_artifact_sha256: false,
        };
        let check = evaluate_hancom_open_evidence(&policy, &dir, &"0".repeat(64));
        assert_eq!(check.status, CheckStatus::Passed);
        assert!(check.reason_codes.is_empty());
        assert_eq!(check.application.as_deref(), Some("Hancom Office HWP"));
        assert_eq!(check.verified_at.as_deref(), Some("2026-08-15T12:00:00Z"));
        assert_eq!(check.verifier.as_deref(), Some("qa-operator-1"));

        // require_pass=false accepts an explicit fail receipt as attested evidence.
        let mut receipt = valid_receipt_json();
        receipt["result"] = serde_json::json!("fail");
        write_receipt(&dir, receipt);
        let policy = HancomOpenPolicy {
            require_pass: false,
            ..policy
        };
        let check = evaluate_hancom_open_evidence(&policy, &dir, &"0".repeat(64));
        assert_eq!(check.status, CheckStatus::Passed);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hancom_open_evidence_fails_when_not_attested() {
        let dir = evidence_scratch_dir("hancom-fail");
        let mut receipt = valid_receipt_json();
        receipt["result"] = serde_json::json!("fail");
        write_receipt(&dir, receipt);
        let policy = HancomOpenPolicy {
            receipt: "hancom-receipt.json".to_string(),
            require_pass: true,
            require_artifact_sha256: false,
        };
        let check = evaluate_hancom_open_evidence(&policy, &dir, &"0".repeat(64));
        assert_eq!(check.status, CheckStatus::Failed);
        assert_eq!(check.reason_codes, ["hancom_open_not_attested"]);
        assert_eq!(check.application.as_deref(), Some("Hancom Office HWP"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hancom_open_evidence_fails_closed_on_missing_or_invalid_receipts() {
        let dir = evidence_scratch_dir("hancom-invalid");
        let policy = HancomOpenPolicy {
            receipt: "hancom-receipt.json".to_string(),
            require_pass: true,
            require_artifact_sha256: false,
        };
        let check = evaluate_hancom_open_evidence(&policy, &dir, &"0".repeat(64));
        assert_eq!(check.status, CheckStatus::Failed);
        assert_eq!(check.reason_codes, ["hancom_open_receipt_invalid"]);
        assert!(check.application.is_none());
        assert!(check.verified_at.is_none());
        assert!(check.verifier.is_none());

        for (label, mutate) in [
            ("schema-version", "schema_version"),
            ("result", "result"),
            ("timestamp", "verified_at"),
            ("hash", "artifact_sha256"),
        ] {
            let mut receipt = valid_receipt_json();
            receipt[mutate] = serde_json::json!("bogus");
            write_receipt(&dir, receipt);
            let check = evaluate_hancom_open_evidence(&policy, &dir, &"0".repeat(64));
            assert_eq!(check.status, CheckStatus::Failed, "{label}");
            assert_eq!(
                check.reason_codes,
                ["hancom_open_receipt_invalid"],
                "{label}"
            );
            assert!(check.application.is_none(), "{label}");
        }
        // Unknown fields are rejected by the closed contract.
        let mut receipt = valid_receipt_json();
        receipt["extra"] = serde_json::json!(true);
        write_receipt(&dir, receipt);
        let check = evaluate_hancom_open_evidence(&policy, &dir, &"0".repeat(64));
        assert_eq!(check.status, CheckStatus::Failed);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hancom_open_receipt_hash_binds_to_the_certified_input() {
        let dir = evidence_scratch_dir("hancom-hash");
        let policy = HancomOpenPolicy {
            receipt: "hancom-receipt.json".to_string(),
            require_pass: true,
            require_artifact_sha256: false,
        };
        // The receipt hash matches the certified input.
        write_receipt(&dir, valid_receipt_json());
        let check = evaluate_hancom_open_evidence(&policy, &dir, &"0".repeat(64));
        assert_eq!(check.status, CheckStatus::Passed);

        // A receipt produced for a different document fails closed.
        let check = evaluate_hancom_open_evidence(&policy, &dir, &"1".repeat(64));
        assert_eq!(check.status, CheckStatus::Failed);
        assert_eq!(check.reason_codes, ["hancom_open_receipt_invalid"]);
        assert!(check.application.is_none());

        // An omitted hash remains allowed; the field is optional in the schema.
        let mut receipt = valid_receipt_json();
        receipt.as_object_mut().unwrap().remove("artifact_sha256");
        write_receipt(&dir, receipt);
        let check = evaluate_hancom_open_evidence(&policy, &dir, &"1".repeat(64));
        assert_eq!(check.status, CheckStatus::Passed);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn require_artifact_sha256_makes_the_receipt_binding_mandatory() {
        let dir = evidence_scratch_dir("hancom-require-hash");
        let policy = HancomOpenPolicy {
            receipt: "hancom-receipt.json".to_string(),
            require_pass: true,
            require_artifact_sha256: true,
        };

        // A receipt naming the certified artifact passes.
        write_receipt(&dir, valid_receipt_json());
        let check = evaluate_hancom_open_evidence(&policy, &dir, &"0".repeat(64));
        assert_eq!(check.status, CheckStatus::Passed);
        assert!(check.reason_codes.is_empty());

        // A receipt naming a different artifact fails closed.
        let check = evaluate_hancom_open_evidence(&policy, &dir, &"1".repeat(64));
        assert_eq!(check.status, CheckStatus::Failed);
        assert_eq!(check.reason_codes, ["hancom_open_receipt_invalid"]);

        // A receipt with no hash names no artifact, so under this option it is not
        // evidence for any of them. The default policy still accepts it.
        let mut unbound = valid_receipt_json();
        unbound.as_object_mut().unwrap().remove("artifact_sha256");
        write_receipt(&dir, unbound);
        let check = evaluate_hancom_open_evidence(&policy, &dir, &"0".repeat(64));
        assert_eq!(check.status, CheckStatus::Failed);
        assert_eq!(check.reason_codes, ["hancom_open_receipt_invalid"]);
        let default_policy = HancomOpenPolicy {
            require_artifact_sha256: false,
            ..policy
        };
        let check = evaluate_hancom_open_evidence(&default_policy, &dir, &"0".repeat(64));
        assert_eq!(check.status, CheckStatus::Passed);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn require_artifact_sha256_defaults_to_false_in_a_parsed_policy() {
        let policy: CertificationPolicy = serde_json::from_value(serde_json::json!({
            "schema_version": "1.0",
            "document": {"hancom_open": {"receipt": "hancom-receipt.json"}},
            "render": {}
        }))
        .unwrap();
        let hancom_open = policy.document.hancom_open.unwrap();
        assert!(hancom_open.require_pass);
        assert!(!hancom_open.require_artifact_sha256);

        let policy: CertificationPolicy = serde_json::from_value(serde_json::json!({
            "schema_version": "1.0",
            "document": {
                "hancom_open": {
                    "receipt": "hancom-receipt.json",
                    "require_artifact_sha256": true
                }
            },
            "render": {}
        }))
        .unwrap();
        assert!(policy.document.hancom_open.unwrap().require_artifact_sha256);
    }

    #[test]
    fn receipt_timestamp_validation_matches_the_schema_shape() {
        for valid in [
            "2026-08-15T12:00:00Z",
            "2026-08-15T12:00:00.123Z",
            "2026-08-15T12:00:00+09:00",
            "2026-08-15T12:00:00.5-05:30",
        ] {
            assert!(valid_receipt_timestamp(valid), "{valid}");
        }
        for invalid in [
            "2026-08-15 12:00:00Z",
            "2026-08-15T12:00:00",
            "2026-08-15T12:00:00.Z",
            "2026-08-15T12:00:00+0900",
            "not-a-timestamp0000",
        ] {
            assert!(!valid_receipt_timestamp(invalid), "{invalid}");
        }
    }

    #[test]
    fn hancom_receipt_schema_pins_the_closed_contract() {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/hancom-verification-receipt-v1.schema.json"
        ))
        .unwrap();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(
            schema["required"],
            serde_json::json!([
                "schema_version",
                "application",
                "result",
                "verified_at",
                "verifier"
            ])
        );
        assert_eq!(schema["properties"]["schema_version"]["const"], "1.0");
        assert_eq!(
            schema["properties"]["result"]["enum"],
            serde_json::json!(["pass", "fail"])
        );

        let validator = jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .build(&schema)
            .unwrap();
        assert!(validator.is_valid(&valid_receipt_json()));
        let mut receipt = valid_receipt_json();
        receipt["verifier"] = serde_json::json!("has\ncontrol");
        assert!(!validator.is_valid(&receipt));
    }

    #[test]
    fn report_schema_evidence_sections_mirror_the_preservation_codes() {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/certification-report-v1.schema.json"
        ))
        .unwrap();
        let checks = &schema["properties"]["checks"]["properties"];
        assert!(checks.get("preservation").is_some());
        assert!(checks.get("hancom_open").is_some());
        let mut schema_codes: Vec<String> = schema["$defs"]["preservationLossCodes"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        schema_codes.sort();
        let model_codes: Vec<&str> = [
            hwp_model::preservation::PreservationCode::BinaryAssetRemoved,
            hwp_model::preservation::PreservationCode::BinaryRelationshipRemoved,
            hwp_model::preservation::PreservationCode::ControlMetadataUnrepresentable,
            hwp_model::preservation::PreservationCode::ControlRemoved,
            hwp_model::preservation::PreservationCode::GsoHeaderUnrepresentable,
            hwp_model::preservation::PreservationCode::GsoShapeUnrepresentable,
            hwp_model::preservation::PreservationCode::HwpContainerStorageRemoved,
            hwp_model::preservation::PreservationCode::HwpContainerStreamRemoved,
            hwp_model::preservation::PreservationCode::HwpOpaqueStreamChanged,
            hwp_model::preservation::PreservationCode::HwpxOpaqueEntryChanged,
            hwp_model::preservation::PreservationCode::HwpxPackageEntryRemoved,
            hwp_model::preservation::PreservationCode::MetadataValueRemoved,
            hwp_model::preservation::PreservationCode::OpaqueControlUnrepresentable,
            hwp_model::preservation::PreservationCode::PictureControlRemoved,
        ]
        .into_iter()
        .map(|code| code.as_str())
        .collect();
        assert_eq!(schema_codes, model_codes);
        let reason_codes = schema["$defs"]["reasonCode"]["enum"].to_string();
        for reason in [
            "preservation_loss_detected",
            "preservation_report_invalid",
            "hancom_open_not_attested",
            "hancom_open_receipt_invalid",
        ] {
            assert!(reason_codes.contains(reason), "{reason}");
        }
    }

    #[test]
    fn example_native_policy_and_evidence_artifacts_validate_against_their_schemas() {
        let validator = |schema_path: &str| {
            let schema: serde_json::Value = serde_json::from_str(schema_path).unwrap();
            jsonschema::options()
                .with_draft(jsonschema::Draft::Draft202012)
                .build(&schema)
                .unwrap()
        };
        let document = |path: &str| -> serde_json::Value { serde_json::from_str(path).unwrap() };
        let policy = document(include_str!(
            "../../../examples/certification-v1/native-policy.json"
        ));
        let schema = validator(include_str!(
            "../../../schemas/certification-policy-v1.schema.json"
        ));
        assert!(schema.is_valid(&policy), "schema rejected {policy}");
        let preservation = document(include_str!(
            "../../../examples/certification-v1/preservation-report.json"
        ));
        let schema = validator(include_str!(
            "../../../schemas/preservation-report-v1.schema.json"
        ));
        assert!(
            schema.is_valid(&preservation),
            "schema rejected {preservation}"
        );
        let receipt = document(include_str!(
            "../../../examples/certification-v1/hancom-receipt.json"
        ));
        let schema = validator(include_str!(
            "../../../schemas/hancom-verification-receipt-v1.schema.json"
        ));
        assert!(schema.is_valid(&receipt), "schema rejected {receipt}");

        // The example policy also parses and validates through the runtime path.
        let policy: CertificationPolicy = serde_json::from_value(policy).unwrap();
        validate_policy(&policy).unwrap();
    }

    /// Build a fully passing native-only report value. Evidence sections are added by the
    /// callers that need them; the baseline matches the pre-PR-8a wire shape.
    fn passing_report_value() -> serde_json::Value {
        let rules: Vec<RuleResult> = [
            "defined_styles",
            "used_styles",
            "numbering.definitions",
            "numbering.used",
            "tables",
            "links",
            "metadata",
            "macros",
            "external_references",
            "accessibility",
            "unresolved_fields",
            "fonts",
        ]
        .into_iter()
        .map(|id| RuleResult {
            id,
            status: CheckStatus::Passed,
            observed_count: 0,
            reason_codes: Vec::new(),
        })
        .collect();
        let mut render = empty_render_report(96.0);
        render.status = CheckStatus::Passed;
        render.reason_codes = Vec::new();
        let report = CertificationReport {
            schema_version: REPORT_SCHEMA_VERSION,
            contract: "hwp-certification-report-v1",
            overall: OverallStatus::Passed,
            scope: "native_only",
            input: InputReport {
                format: "hwpx".to_string(),
                bytes: 42,
                sha256: "0".repeat(64),
            },
            policy_sha256: "1".repeat(64),
            checks: CheckSet {
                package: passed_check(),
                repeat_import_consistency: passed_check(),
                rules,
                preservation: None,
                hancom_open: None,
            },
            render,
            oracle: OracleReport {
                mode: OracleMode::Disabled,
                status: OracleStatus::Disabled,
                reason_code: None,
                expected: None,
                observed: None,
                stdout: None,
                stderr: None,
                artifact_determinism: "not_applicable",
            },
            artifacts: Vec::new(),
            limitations: vec![
                "native_not_detected_is_algorithm_scoped",
                "hancom_rendering_parity_not_claimed",
                "oracle_artifact_determinism_not_claimed",
                "oracle_page_count_not_host_verified",
                "selected_pages_only_when_policy_selects_pages",
            ],
        };
        validate_rule_composition(&report.checks.rules).unwrap();
        validate_evidence_check_invariants(&report).unwrap();
        serde_json::to_value(&report).unwrap()
    }

    /// The report schema cross-references the oracle-result schema by relative file ref;
    /// inline that def so the test validator resolves every reference locally.
    fn certification_report_validator() -> jsonschema::Validator {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/certification-report-v1.schema.json"
        ))
        .unwrap();
        let oracle: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/certification-oracle-result-v1.schema.json"
        ))
        .unwrap();
        let mut rewritten = schema.to_string();
        rewritten = rewritten.replace("certification-oracle-result-v1.schema.json#/", "#/");
        let mut schema: serde_json::Value = serde_json::from_str(&rewritten).unwrap();
        schema["$defs"]["attestation"] = oracle["$defs"]["attestation"].clone();
        jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .build(&schema)
            .unwrap()
    }

    #[test]
    fn report_without_evidence_sections_still_validates_against_the_updated_schema() {
        let value = passing_report_value();
        assert!(value["checks"].get("preservation").is_none());
        assert!(value["checks"].get("hancom_open").is_none());
        let validator = certification_report_validator();
        assert!(validator.is_valid(&value), "schema rejected {value}");
    }

    #[test]
    fn report_with_passed_evidence_sections_validates_against_the_updated_schema() {
        let mut value = passing_report_value();
        value["checks"]["preservation"] = serde_json::json!({
            "status": "passed",
            "reason_codes": [],
            "loss_code_count": 2,
            "loss_codes": {"control_removed": 2}
        });
        value["checks"]["hancom_open"] = serde_json::json!({
            "status": "passed",
            "reason_codes": [],
            "application": "Hancom Office HWP",
            "verified_at": "2026-08-15T12:00:00Z",
            "verifier": "qa-operator-1"
        });
        let validator = certification_report_validator();
        assert!(validator.is_valid(&value), "schema rejected {value}");
    }

    #[test]
    fn report_with_failed_evidence_requires_a_failed_overall() {
        let validator = certification_report_validator();
        let failed_preservation = serde_json::json!({
            "status": "failed",
            "reason_codes": ["preservation_loss_detected"],
            "loss_code_count": 3,
            "loss_codes": {"control_removed": 3}
        });

        // Failed evidence with an otherwise passing local composition: overall failed and the
        // disabled oracle reports not_claimed (new oneOf branch).
        let mut value = passing_report_value();
        value["overall"] = serde_json::json!("failed");
        value["oracle"]["artifact_determinism"] = serde_json::json!("not_claimed");
        value["checks"]["preservation"] = failed_preservation.clone();
        assert!(validator.is_valid(&value), "schema rejected {value}");

        // Failed evidence can never compose with a passed or partial overall.
        for overall in ["passed", "partial"] {
            let mut value = passing_report_value();
            value["overall"] = serde_json::json!(overall);
            value["checks"]["preservation"] = failed_preservation.clone();
            assert!(!validator.is_valid(&value), "schema accepted {value}");
        }

        // The echo fields and loss code names stay closed and content-free.
        let mut value = passing_report_value();
        value["overall"] = serde_json::json!("failed");
        value["oracle"]["artifact_determinism"] = serde_json::json!("not_claimed");
        let mut section = failed_preservation.clone();
        section["loss_codes"] = serde_json::json!({"not_a_loss_code": 1});
        value["checks"]["preservation"] = section;
        assert!(!validator.is_valid(&value), "schema accepted {value}");
    }
}
