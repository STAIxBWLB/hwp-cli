//! Checked-in structured-document corpus runner.
//!
//! This runner deliberately avoids the legacy diagnostic Python script. Inputs are closed,
//! versioned, hash-pinned and bounded. Every output is generated twice on the same platform,
//! reopened, structurally validated and certified with the native-only profile. The report tree
//! is private while it is built and is published atomically without replacement.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result};
use hwp_cli::certification::{CertificationPolicy, MAX_INPUT_BYTES, OracleMode, OverallStatus};
use hwp_cli::document_spec::{MAX_SPEC_BYTES, SpecInputFormat};
use hwp_cli::template_spec::{MAX_DATA_BYTES, MAX_TEMPLATE_BYTES};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

mod pdf_roundtrip;

const MANIFEST_VERSION: &str = "1.0";
const MANIFEST_CONTRACT: &str = "hwp-structured-corpus-v1";
const SUMMARY_CONTRACT: &str = "hwp-structured-corpus-run-v1";
const ARTIFACT_CONTRACT: &str = "hwp-structured-corpus-artifacts-v1";
const FROZEN_MANIFEST_SHA256: &str =
    "03ef22e59a45a03d49de5e611f95edebb268a76665feaa63e8fc5d2e92f30dc5";
const FROZEN_POLICY_SHA256: &str =
    "2da9ef212ac3c5e10c85229d62e307e0c29a8e06a848e47feb039db1fd09fdb8";
const FROZEN_FONT_SHA256: &str = "194018e6b2b293a7964f037b25c0249ce1418bc9ab3c971060a03aa57861e252";
const FROZEN_FONT_LICENSE_SHA256: &str =
    "1c05c68c34f9708415aada51f17e1b0092d2cea709bf4a94cd38114f9e73d7d9";
const FROZEN_FONT_METADATA_SHA256: &str =
    "9b8b27c0dd1adbb0057d1fbe9534207ab3606114ace6755453d14668912471a1";
const FROZEN_FONT_REVISION: &str = "2796410152d4f9524b68ed46e69c1b60f8e0f7c3";
const FROZEN_FONT_SOURCE_URL: &str = "https://raw.githubusercontent.com/google/fonts/2796410152d4f9524b68ed46e69c1b60f8e0f7c3/ofl/notosanskr/NotoSansKR%5Bwght%5D.ttf";
#[cfg(test)]
const FROZEN_MANIFEST_SCHEMA_SHA256: &str =
    "b8057de94b15deebceb58f014071d57d96fd9bb61603d9cbc2fd94a4398b3b3a";
#[cfg(test)]
const FROZEN_RUN_SCHEMA_SHA256: &str =
    "f6b4ada36bb9151fadb5233770a6a235c106a815c7eb8567237c871deb5f10b5";
#[cfg(test)]
const FROZEN_ARTIFACT_SCHEMA_SHA256: &str =
    "8735bbe43e21a40bbcf4d20f61c0b886414ffcd5c4d0c690635e191334800ef7";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_CASES: usize = 32;
const MAX_REQUIRED_TEXT: usize = 64;
const MAX_REQUIRED_TEXT_CHARS: usize = 4096;
const MAX_REPORT_BYTES: u64 = 1024 * 1024;
const MAX_TREE_FILES: usize = 256;
const MAX_TREE_DIRECTORIES: usize = 128;
const MAX_TREE_DEPTH: usize = 8;
const MAX_ARTIFACT_PATH_BYTES: usize = 512;
const MAX_ARTIFACT_FILE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_TREE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SEMANTIC_NODES: usize = 100_000;
const MAX_SEMANTIC_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusManifest {
    schema_version: String,
    contract: String,
    profile: String,
    policy: FilePin,
    fonts: Vec<FontPin>,
    cases: Vec<CorpusCase>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FilePin {
    path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FontPin {
    path: String,
    sha256: String,
    license_path: String,
    license_sha256: String,
    metadata_path: String,
    metadata_sha256: String,
    license: String,
    source_repository: String,
    source_revision: String,
    source_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusCase {
    id: String,
    category: CorpusCategory,
    generator: Generator,
    formats: Vec<CorpusFormat>,
    expected: ExpectedAssertions,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum CorpusCategory {
    KoreanOfficialLetter,
    ApprovalDraftMemo,
    Report,
    BusinessPlan,
    MeetingMinutes,
    AcademicEducation,
    PrintForm,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Generator {
    DocumentSpec {
        source: FilePin,
        format: InputFormat,
    },
    TemplateSpecData {
        template: FilePin,
        template_format: InputFormat,
        data: FilePin,
        data_format: InputFormat,
    },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum InputFormat {
    Json,
    Yaml,
}

impl From<InputFormat> for SpecInputFormat {
    fn from(value: InputFormat) -> Self {
        match value {
            InputFormat::Json => Self::Json,
            InputFormat::Yaml => Self::Yaml,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum CorpusFormat {
    Hwpx,
    Hwp,
}

impl CorpusFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Hwpx => "hwpx",
            Self::Hwp => "hwp",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedAssertions {
    semantic: SemanticAssertions,
    certification: CertificationAssertions,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SemanticAssertions {
    required_text: Vec<String>,
    min_sections: usize,
    min_paragraphs: usize,
    min_tables: usize,
    min_text_chars: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CertificationAssertions {
    selected_pages: Vec<usize>,
    page_count_min: usize,
    page_count_max: usize,
    require_nonblank: bool,
    require_complete_diagnostics: bool,
    max_render_issue_count: u64,
}

#[derive(Debug, Serialize)]
struct CorpusSummary {
    schema_version: &'static str,
    contract: &'static str,
    profile: String,
    platform: Platform,
    status: RunStatus,
    reason_codes: Vec<&'static str>,
    manifest_sha256: String,
    policy_sha256: String,
    fonts: Vec<FontIdentity>,
    cases: Vec<CaseSummary>,
    limits: SummaryLimits,
    claims: Claims,
}

#[derive(Debug, Serialize)]
struct Platform {
    os: &'static str,
    arch: &'static str,
    family: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RunStatus {
    Passed,
    Failed,
}

#[derive(Debug, Serialize)]
struct FontIdentity {
    sha256: String,
    license: String,
    source_revision: String,
    source_url_sha256: String,
}

#[derive(Debug, Serialize)]
struct CaseSummary {
    id: String,
    category: CorpusCategory,
    status: RunStatus,
    reason_codes: Vec<&'static str>,
    formats: Vec<FormatSummary>,
}

#[derive(Debug, Serialize)]
struct FormatSummary {
    format: CorpusFormat,
    status: RunStatus,
    reason_codes: Vec<&'static str>,
    two_run_byte_identical: bool,
    output_sha256: Option<String>,
    output_bytes: Option<u64>,
    two_run_render_identical: bool,
    two_run_pdf_identical: bool,
    pdf: Option<PdfSummary>,
    semantic: Option<SemanticSummary>,
    certification: Option<CertificationSummary>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct PdfSummary {
    sha256: String,
    bytes: u64,
    page_count: usize,
    tounicode_roundtrip_ok: bool,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct SemanticSummary {
    plain_text_sha256: String,
    structural_semantic_sha256: String,
    text_chars: usize,
    sections: usize,
    paragraphs: usize,
    tables: usize,
    required_text_count: usize,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct CertificationSummary {
    overall: String,
    total_pages: usize,
    selected_pages: Vec<usize>,
    render_issue_count: u64,
    render_issue_sha256: String,
    fonts: Vec<CertificationFontIdentity>,
    pages: Vec<PageHash>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct CertificationFontIdentity {
    font_file_sha256: String,
    outcome: String,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct PageHash {
    page: usize,
    png_sha256: String,
    visual_blank: bool,
    outside_page_bounds: DetectionEvidence,
    possible_collision: DetectionEvidence,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct DetectionEvidence {
    result: String,
    count: u64,
    complete: bool,
}

#[derive(Debug, Serialize)]
struct SummaryLimits {
    max_manifest_bytes: u64,
    max_report_bytes: u64,
    max_cases: usize,
    max_tree_files: usize,
    max_tree_directories: usize,
    max_tree_depth: usize,
    max_artifact_path_bytes: usize,
    max_artifact_file_bytes: u64,
    max_tree_bytes: u64,
    max_semantic_nodes: usize,
    max_semantic_bytes: u64,
}

#[derive(Debug, Serialize)]
struct Claims {
    coverage_scope: &'static str,
    semantic_digest_profile: &'static str,
    byte_determinism_scope: &'static str,
    render_hash_scope: &'static str,
    oracle_scope: &'static str,
    manual_checks: bool,
    limitations: [&'static str; 10],
}

#[derive(Debug, Serialize)]
struct ArtifactManifest {
    schema_version: &'static str,
    contract: &'static str,
    file_count: usize,
    total_bytes: u64,
    files: Vec<Artifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Artifact {
    path: String,
    bytes: u64,
    sha256: String,
}

pub fn run(manifest_path: &Path, report_path: &Path) -> Result<()> {
    let manifest_bytes = read_regular_bounded(manifest_path, MAX_MANIFEST_BYTES)
        .context("corpus manifest rejected")?;
    let manifest_sha256 = sha256_hex(&manifest_bytes);
    if manifest_sha256 != FROZEN_MANIFEST_SHA256 {
        anyhow::bail!("corpus manifest does not match the frozen checked-in profile")
    }
    let manifest: CorpusManifest =
        serde_json::from_slice(&manifest_bytes).context("corpus manifest JSON rejected")?;
    validate_manifest(&manifest)?;
    let base = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let (_, policy_bytes) = snapshot_pin(base, &manifest.policy, MAX_MANIFEST_BYTES)?;
    let policy: CertificationPolicy =
        serde_json::from_slice(&policy_bytes).context("corpus policy JSON rejected")?;
    verify_policy_contract(&policy, &manifest.fonts)?;
    let mut font_snapshots = Vec::with_capacity(manifest.fonts.len());
    let mut font_identities = Vec::with_capacity(manifest.fonts.len());
    for font in &manifest.fonts {
        let (_, font_bytes) = snapshot_pin(
            base,
            &FilePin {
                path: font.path.clone(),
                sha256: font.sha256.clone(),
            },
            hwp_cli::certification::MAX_FONT_FILE_BYTES,
        )?;
        snapshot_pin(
            base,
            &FilePin {
                path: font.license_path.clone(),
                sha256: font.license_sha256.clone(),
            },
            MAX_MANIFEST_BYTES,
        )?;
        snapshot_pin(
            base,
            &FilePin {
                path: font.metadata_path.clone(),
                sha256: font.metadata_sha256.clone(),
            },
            MAX_MANIFEST_BYTES,
        )?;
        font_snapshots.push(font_bytes);
        font_identities.push(FontIdentity {
            sha256: font.sha256.clone(),
            license: font.license.clone(),
            source_revision: font.source_revision.clone(),
            source_url_sha256: sha256_hex(font.source_url.as_bytes()),
        });
    }

    let mut stage = AtomicCorpusDir::new(report_path)?;
    let inputs_dir = stage.root().join(".inputs");
    create_private_workspace(&inputs_dir)?;
    let policy_path = inputs_dir.join("policy.json");
    write_new(&policy_path, &policy_bytes)?;
    let mut font_files = Vec::with_capacity(font_snapshots.len());
    for (pin, bytes) in policy.document.fonts.manifest.iter().zip(&font_snapshots) {
        let font_path = inputs_dir.join(Path::new(&pin.path));
        if let Some(parent) = font_path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_new(&font_path, bytes)?;
        font_files.push(font_path);
    }
    let isolated_source_base = inputs_dir.join("sources");
    fs::create_dir(&isolated_source_base)?;
    fs::create_dir(stage.root().join("documents"))?;
    fs::create_dir(stage.root().join("certification"))?;

    let mut case_summaries = Vec::with_capacity(manifest.cases.len());
    for case in &manifest.cases {
        case_summaries.push(run_case(
            case,
            base,
            &policy_path,
            policy.render.dpi,
            &font_files,
            &isolated_source_base,
            stage.root(),
        ));
    }
    fs::remove_dir_all(&inputs_dir).context("private corpus input cleanup failed")?;
    let failed = case_summaries
        .iter()
        .any(|case| case.status == RunStatus::Failed);
    let summary = CorpusSummary {
        schema_version: MANIFEST_VERSION,
        contract: SUMMARY_CONTRACT,
        profile: manifest.profile,
        platform: Platform {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            family: std::env::consts::FAMILY,
        },
        status: if failed {
            RunStatus::Failed
        } else {
            RunStatus::Passed
        },
        reason_codes: if failed {
            vec!["case_failed"]
        } else {
            Vec::new()
        },
        manifest_sha256,
        policy_sha256: manifest.policy.sha256,
        fonts: font_identities,
        cases: case_summaries,
        limits: SummaryLimits {
            max_manifest_bytes: MAX_MANIFEST_BYTES,
            max_report_bytes: MAX_REPORT_BYTES,
            max_cases: MAX_CASES,
            max_tree_files: MAX_TREE_FILES,
            max_tree_directories: MAX_TREE_DIRECTORIES,
            max_tree_depth: MAX_TREE_DEPTH,
            max_artifact_path_bytes: MAX_ARTIFACT_PATH_BYTES,
            max_artifact_file_bytes: MAX_ARTIFACT_FILE_BYTES,
            max_tree_bytes: MAX_TREE_BYTES,
            max_semantic_nodes: MAX_SEMANTIC_NODES,
            max_semantic_bytes: MAX_SEMANTIC_BYTES,
        },
        claims: Claims {
            coverage_scope: "bounded_representative_structured_smoke",
            semantic_digest_profile: "hwp-corpus-common-semantic-v1",
            byte_determinism_scope: "same_process_same_platform_two_run",
            render_hash_scope: "recorded_for_platform_profile_not_cross_platform_equivalence",
            oracle_scope: "native_only_oracle_disabled",
            manual_checks: false,
            limitations: [
                "no_hancom_parity_claim",
                "no_independent_office_oracle",
                "single_page_fixture_profile",
                "no_advanced_drawing_or_chart_coverage",
                "no_comments_revisions_or_security_controls",
                "no_unparsed_target_specific_control_payloads_in_semantic_digest",
                "no_line_layout_cache_or_opaque_record_bytes_in_semantic_digest",
                "no_advanced_shape_geometry_or_style_in_semantic_digest",
                "no_hwp5_ambiguous_strike_or_underline_shape_in_semantic_digest",
                "category_labels_do_not_imply_complete_feature_coverage",
            ],
        },
    };
    let summary_bytes = pretty_json_bounded(&summary, MAX_REPORT_BYTES)?;
    write_new(&stage.root().join("summary.json"), &summary_bytes)?;

    let artifacts = collect_artifacts(stage.root())?;
    let total_bytes = artifacts.iter().try_fold(0u64, |total, artifact| {
        total
            .checked_add(artifact.bytes)
            .context("corpus artifact byte count overflow")
    })?;
    let artifact_manifest = ArtifactManifest {
        schema_version: MANIFEST_VERSION,
        contract: ARTIFACT_CONTRACT,
        file_count: artifacts.len(),
        total_bytes,
        files: artifacts,
    };
    let artifact_bytes = pretty_json_bounded(&artifact_manifest, MAX_REPORT_BYTES)?;
    write_new(&stage.root().join("artifacts.json"), &artifact_bytes)?;
    let case_ids = manifest
        .cases
        .iter()
        .map(|case| case.id.as_str())
        .collect::<Vec<_>>();
    audit_tree(stage.root(), &artifact_manifest, &case_ids)?;
    stage.publish()?;

    println!(
        "{}",
        serde_json::json!({
            "contract": SUMMARY_CONTRACT,
            "status": if failed { "failed" } else { "passed" }
        })
    );
    if failed {
        anyhow::bail!("structured corpus failed; inspect the atomic bounded report")
    }
    Ok(())
}

fn validate_manifest(manifest: &CorpusManifest) -> Result<()> {
    if manifest.schema_version != MANIFEST_VERSION || manifest.contract != MANIFEST_CONTRACT {
        anyhow::bail!("unsupported corpus manifest contract")
    }
    if manifest.policy.sha256 != FROZEN_POLICY_SHA256 {
        anyhow::bail!("corpus policy does not match the frozen native profile")
    }
    validate_short_ascii(&manifest.profile, 64, "profile")?;
    if manifest.fonts.is_empty() || manifest.fonts.len() > hwp_cli::certification::MAX_FONT_FILES {
        anyhow::bail!("corpus font manifest cardinality rejected")
    }
    let font_paths = manifest
        .fonts
        .iter()
        .map(|font| font.path.as_str())
        .collect::<Vec<_>>();
    if font_paths.windows(2).any(|pair| pair[0] >= pair[1]) {
        anyhow::bail!("corpus font manifest must be strictly path-sorted")
    }
    for font in &manifest.fonts {
        validate_sha256(&font.sha256)?;
        validate_sha256(&font.license_sha256)?;
        validate_sha256(&font.metadata_sha256)?;
        validate_short_ascii(&font.license, 64, "font license")?;
        validate_short_ascii(&font.source_revision, 128, "font source revision")?;
        if font.path != "fonts/NotoSansKR[wght].ttf"
            || font.sha256 != FROZEN_FONT_SHA256
            || font.license_path != "fonts/OFL.txt"
            || font.license_sha256 != FROZEN_FONT_LICENSE_SHA256
            || font.metadata_path != "fonts/METADATA.pb"
            || font.metadata_sha256 != FROZEN_FONT_METADATA_SHA256
            || font.license != "OFL-1.1"
            || font.source_repository != "https://github.com/google/fonts"
            || font.source_revision != FROZEN_FONT_REVISION
            || font.source_url != FROZEN_FONT_SOURCE_URL
        {
            anyhow::bail!("font provenance is not an allowed official immutable source")
        }
    }
    if manifest.cases.len() != 7 || manifest.cases.len() > MAX_CASES {
        anyhow::bail!("corpus must contain exactly seven bounded cases")
    }
    let mut input_paths = BTreeSet::new();
    for path in std::iter::once(manifest.policy.path.as_str()).chain(
        manifest.fonts.iter().flat_map(|font| {
            [
                font.path.as_str(),
                font.license_path.as_str(),
                font.metadata_path.as_str(),
            ]
        }),
    ) {
        validate_portable_relative(path)?;
        if !input_paths.insert(path.to_ascii_lowercase()) {
            anyhow::bail!("corpus input paths collide under Windows case folding")
        }
    }
    let mut ids = BTreeSet::new();
    let mut categories = BTreeSet::new();
    for case in &manifest.cases {
        validate_id(&case.id)?;
        if !ids.insert(case.id.as_str()) {
            anyhow::bail!("duplicate corpus case id")
        }
        let category = serde_json::to_string(&case.category)?;
        if !categories.insert(category) {
            anyhow::bail!("duplicate corpus category")
        }
        if case.formats.is_empty() || case.formats.len() > 2 {
            anyhow::bail!("corpus format cardinality rejected")
        }
        let unique: BTreeSet<_> = case
            .formats
            .iter()
            .map(|format| format.extension())
            .collect();
        if unique.len() != case.formats.len() {
            anyhow::bail!("duplicate corpus output format")
        }
        if !unique.contains("hwpx") || !unique.contains("hwp") {
            anyhow::bail!("every structured corpus case requires HWPX and HWP outputs")
        }
        let generator_paths = match &case.generator {
            Generator::DocumentSpec { source, .. } => vec![&source.path],
            Generator::TemplateSpecData { template, data, .. } => {
                vec![&template.path, &data.path]
            }
        };
        for path in generator_paths {
            validate_portable_relative(path)?;
            if !input_paths.insert(path.to_ascii_lowercase()) {
                anyhow::bail!("corpus input paths collide under Windows case folding")
            }
        }
        validate_assertions(&case.expected)?;
    }
    validate_sha256(&manifest.policy.sha256)?;
    Ok(())
}

fn verify_policy_contract(policy: &CertificationPolicy, fonts: &[FontPin]) -> Result<()> {
    if policy.oracle.mode != OracleMode::Disabled || policy.oracle.configuration.is_some() {
        anyhow::bail!("corpus certification policy must be native-only with oracle disabled")
    }
    let policy_fonts = &policy.document.fonts;
    if !policy_fonts.forbid_substitution
        || policy_fonts.manifest.len() != fonts.len()
        || policy_fonts.allowed_requested != ["Noto Sans KR"]
        || policy_fonts.required_requested != ["Noto Sans KR"]
    {
        anyhow::bail!("corpus generation and certification font contracts differ")
    }
    for (policy_pin, corpus_pin) in policy_fonts.manifest.iter().zip(fonts) {
        if policy_pin.path != corpus_pin.path || policy_pin.sha256 != corpus_pin.sha256 {
            anyhow::bail!("corpus generation and certification font identities differ")
        }
    }
    Ok(())
}

fn validate_assertions(expected: &ExpectedAssertions) -> Result<()> {
    let semantic = &expected.semantic;
    if semantic.required_text.is_empty() || semantic.required_text.len() > MAX_REQUIRED_TEXT {
        anyhow::bail!("required semantic text cardinality rejected")
    }
    if semantic
        .required_text
        .iter()
        .any(|text| text.is_empty() || text.chars().count() > MAX_REQUIRED_TEXT_CHARS)
    {
        anyhow::bail!("required semantic text length rejected")
    }
    if semantic.required_text.len() < 3
        || semantic.min_sections < 1
        || semantic.min_paragraphs < 5
        || semantic.min_tables < 1
        || semantic.min_text_chars < 80
    {
        anyhow::bail!("semantic gate may not be weakened below the frozen profile")
    }
    let certification = &expected.certification;
    if certification.selected_pages.is_empty()
        || certification.selected_pages.len() > hwp_cli::certification::MAX_SELECTED_PAGES
        || certification
            .selected_pages
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        || certification.page_count_min == 0
        || certification.page_count_min > certification.page_count_max
        || certification.page_count_max > hwp_cli::certification::MAX_PAGE_NUMBER
        || !certification.require_nonblank
        || !certification.require_complete_diagnostics
        || certification.max_render_issue_count != 0
    {
        anyhow::bail!("certification expectation bounds rejected")
    }
    Ok(())
}

fn run_case(
    case: &CorpusCase,
    base: &Path,
    policy: &Path,
    render_dpi: f32,
    font_files: &[PathBuf],
    isolated_source_base: &Path,
    stage: &Path,
) -> CaseSummary {
    let prepared = match prepare_generator(&case.generator, base) {
        Ok(prepared) => prepared,
        Err(_) => {
            return CaseSummary {
                id: case.id.clone(),
                category: case.category,
                status: RunStatus::Failed,
                reason_codes: vec!["format_failed"],
                formats: case
                    .formats
                    .iter()
                    .map(|format| failed_format(*format, "input_snapshot_failed"))
                    .collect(),
            };
        }
    };
    let mut formats = Vec::with_capacity(case.formats.len());
    for format in &case.formats {
        formats.push(run_format(
            case,
            &prepared,
            *format,
            policy,
            render_dpi,
            font_files,
            isolated_source_base,
            stage,
        ));
    }
    let failed = formats
        .iter()
        .any(|format| format.status == RunStatus::Failed);
    let cross_format_semantics_match = formats.len() == 2
        && formats[0].semantic.is_some()
        && formats[0].semantic == formats[1].semantic;
    let failed = failed || !cross_format_semantics_match;
    let mut reason_codes = Vec::new();
    if formats
        .iter()
        .any(|format| format.status == RunStatus::Failed)
    {
        reason_codes.push("format_failed");
    }
    if !cross_format_semantics_match {
        reason_codes.push("cross_format_semantic_mismatch");
    }
    CaseSummary {
        id: case.id.clone(),
        category: case.category,
        status: if failed {
            RunStatus::Failed
        } else {
            RunStatus::Passed
        },
        reason_codes,
        formats,
    }
}

fn failed_format(format: CorpusFormat, reason: &'static str) -> FormatSummary {
    FormatSummary {
        format,
        status: RunStatus::Failed,
        reason_codes: vec![reason],
        two_run_byte_identical: false,
        output_sha256: None,
        output_bytes: None,
        two_run_render_identical: false,
        two_run_pdf_identical: false,
        pdf: None,
        semantic: None,
        certification: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn run_format(
    case: &CorpusCase,
    generator: &PreparedGenerator,
    format: CorpusFormat,
    policy: &Path,
    render_dpi: f32,
    font_files: &[PathBuf],
    isolated_source_base: &Path,
    stage: &Path,
) -> FormatSummary {
    let mut summary = FormatSummary {
        format,
        status: RunStatus::Failed,
        reason_codes: Vec::new(),
        two_run_byte_identical: false,
        output_sha256: None,
        output_bytes: None,
        two_run_render_identical: false,
        two_run_pdf_identical: false,
        pdf: None,
        semantic: None,
        certification: None,
    };
    let documents_dir = stage.join("documents").join(&case.id);
    if fs::create_dir(&documents_dir).is_err() && !documents_dir.is_dir() {
        summary.reason_codes.push("workspace_failed");
        return summary;
    }
    let extension = format.extension();
    let first = documents_dir.join(format!("run-a.{extension}"));
    let second = documents_dir.join(format!("run-b.{extension}"));
    if generate(generator, isolated_source_base, &first, font_files).is_err()
        || generate(generator, isolated_source_base, &second, font_files).is_err()
    {
        summary.reason_codes.push("generation_failed");
        return summary;
    }
    let first_bytes = match read_regular_bounded(&first, MAX_INPUT_BYTES) {
        Ok(bytes) => bytes,
        Err(_) => {
            summary.reason_codes.push("output_read_failed");
            return summary;
        }
    };
    let second_bytes = match read_regular_bounded(&second, MAX_INPUT_BYTES) {
        Ok(bytes) => bytes,
        Err(_) => {
            summary.reason_codes.push("output_read_failed");
            return summary;
        }
    };
    summary.output_sha256 = Some(sha256_hex(&first_bytes));
    summary.output_bytes = u64::try_from(first_bytes.len()).ok();
    summary.two_run_byte_identical = first_bytes == second_bytes;
    if !summary.two_run_byte_identical {
        summary.reason_codes.push("two_run_byte_mismatch");
    }

    for document in [&first, &second] {
        let validation = crate::commands::validate::validate_json(document);
        if validation.get("valid").and_then(serde_json::Value::as_bool) != Some(true)
            || validation
                .get("warnings")
                .and_then(serde_json::Value::as_array)
                .is_none_or(|warnings| !warnings.is_empty())
        {
            summary.reason_codes.push("package_validation_failed");
            break;
        }
    }
    match (
        read_semantic(&first, format, &case.expected.semantic),
        read_semantic(&second, format, &case.expected.semantic),
    ) {
        (Ok(first_semantic), Ok(second_semantic)) if first_semantic == second_semantic => {
            summary.semantic = Some(first_semantic);
        }
        _ => summary.reason_codes.push("semantic_assertion_failed"),
    }

    let cert_parent = stage.join("certification").join(&case.id).join(extension);
    if fs::create_dir_all(&cert_parent).is_err() && !cert_parent.is_dir() {
        summary.reason_codes.push("workspace_failed");
        return summary;
    }
    let allowed_font_hashes = font_files
        .iter()
        .filter_map(|path| read_regular_bounded(path, MAX_TREE_BYTES).ok())
        .map(|bytes| sha256_hex(&bytes))
        .collect::<BTreeSet<_>>();
    let mut certification_runs = Vec::with_capacity(2);
    for (label, document) in [("run-a", &first), ("run-b", &second)] {
        let cert_dir = cert_parent.join(label);
        match hwp_cli::certification::execute(document, policy, &cert_dir) {
            Ok(outcome) => {
                if outcome.overall != OverallStatus::Passed {
                    summary.reason_codes.push("certification_failed");
                }
                match read_certification_summary(
                    &cert_dir.join("report.json"),
                    &case.expected.certification,
                    &allowed_font_hashes,
                ) {
                    Ok(certification) => certification_runs.push(certification),
                    Err(()) => summary.reason_codes.push("certification_assertion_failed"),
                }
            }
            Err(_) => summary.reason_codes.push("certification_execution_failed"),
        }
    }
    if certification_runs.len() == 2 && certification_runs[0] == certification_runs[1] {
        summary.two_run_render_identical = true;
        summary.certification = certification_runs.into_iter().next();
    } else {
        summary.reason_codes.push("two_run_render_mismatch");
    }
    // Run the PDF backend check only after PNG certification establishes the
    // expected page count. Earlier reason codes already diagnose certification
    // failures.
    let expected_pdf_pages = summary
        .certification
        .as_ref()
        .map(|certification| certification.total_pages);
    if let Some(expected_pages) = expected_pdf_pages {
        check_pdf_backend(
            &mut summary,
            &first,
            &second,
            format,
            render_dpi,
            font_files,
            expected_pages,
            &documents_dir,
            extension,
        );
    }
    if summary.reason_codes.is_empty() {
        summary.status = RunStatus::Passed;
    }
    summary
}

/// Validate PDF page-count agreement, the complete pre-serialization text
/// trace through ToUnicode, and same-platform two-run byte determinism.
#[allow(clippy::too_many_arguments)]
fn check_pdf_backend(
    summary: &mut FormatSummary,
    first: &Path,
    second: &Path,
    format: CorpusFormat,
    render_dpi: f32,
    font_files: &[PathBuf],
    expected_pages: usize,
    documents_dir: &Path,
    extension: &str,
) {
    let options = hwp_render::RenderOptions {
        dpi: render_dpi,
        font_dirs: Vec::new(),
    };
    let mut pdfs = Vec::with_capacity(2);
    let mut expected_texts = Vec::with_capacity(2);
    for (label, document_path) in [("run-a", first), ("run-b", second)] {
        let Some(document) = read_generated_document(document_path, format) else {
            summary.reason_codes.push("pdf_render_failed");
            return;
        };
        let output = match hwp_render::render_document_pdf_isolated_with_text_trace(
            &document, &options, None, font_files,
        ) {
            Ok(output) if output.report.issue_count == 0 => output,
            _ => {
                summary.reason_codes.push("pdf_render_failed");
                return;
            }
        };
        let path = documents_dir.join(format!("{label}-{extension}.pdf"));
        if write_new(&path, &output.data).is_err() {
            summary.reason_codes.push("workspace_failed");
            return;
        }
        expected_texts.push(output.expected_text);
        pdfs.push(output.data);
    }
    summary.two_run_pdf_identical = pdfs[0] == pdfs[1];
    if !summary.two_run_pdf_identical {
        summary.reason_codes.push("two_run_pdf_mismatch");
    }
    let inspection = match pdf_roundtrip::inspect_pdf(&pdfs[0]) {
        Ok(inspection) => {
            if inspection.page_count != expected_pages {
                summary.reason_codes.push("pdf_page_count_mismatch");
            }
            let roundtrip_ok =
                tounicode_trace_matches(&inspection.decoded_text, &expected_texts[0]);
            if !roundtrip_ok {
                summary.reason_codes.push("pdf_tounicode_roundtrip_failed");
            }
            Some((inspection.page_count, roundtrip_ok))
        }
        Err(_) => {
            summary.reason_codes.push("pdf_tounicode_roundtrip_failed");
            None
        }
    };
    if let Some((page_count, roundtrip_ok)) = inspection {
        summary.pdf = Some(PdfSummary {
            sha256: sha256_hex(&pdfs[0]),
            bytes: u64::try_from(pdfs[0].len()).unwrap_or(u64::MAX),
            page_count,
            tounicode_roundtrip_ok: roundtrip_ok,
        });
    }
}

/// Compare the complete emitted text trace while normalizing platform-neutral
/// whitespace spellings. Whitespace cardinality is retained so a missing or
/// incorrect space mapping cannot pass.
fn tounicode_trace_matches(decoded: &str, expected: &str) -> bool {
    normalize_text_trace(decoded) == normalize_text_trace(expected)
}

fn normalize_text_trace(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_whitespace() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

enum PreparedGenerator {
    DocumentSpec {
        input: String,
        format: InputFormat,
    },
    TemplateSpecData {
        template_input: String,
        template_format: InputFormat,
        data_input: String,
        data_format: InputFormat,
    },
}

fn prepare_generator(generator: &Generator, base: &Path) -> Result<PreparedGenerator> {
    match generator {
        Generator::DocumentSpec { source, format } => {
            let (_, bytes) = snapshot_pin(base, source, MAX_SPEC_BYTES as u64)?;
            Ok(PreparedGenerator::DocumentSpec {
                input: String::from_utf8(bytes).context("DocumentSpec is not UTF-8")?,
                format: *format,
            })
        }
        Generator::TemplateSpecData {
            template,
            template_format,
            data,
            data_format,
        } => {
            let (_, template_bytes) = snapshot_pin(base, template, MAX_TEMPLATE_BYTES as u64)?;
            let (_, data_bytes) = snapshot_pin(base, data, MAX_DATA_BYTES as u64)?;
            Ok(PreparedGenerator::TemplateSpecData {
                template_input: String::from_utf8(template_bytes)
                    .context("TemplateSpec is not UTF-8")?,
                template_format: *template_format,
                data_input: String::from_utf8(data_bytes).context("TemplateData is not UTF-8")?,
                data_format: *data_format,
            })
        }
    }
}

fn generate(
    generator: &PreparedGenerator,
    isolated_source_base: &Path,
    output: &Path,
    font_files: &[PathBuf],
) -> Result<()> {
    if output
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("hwp"))
    {
        let intermediate = output.with_extension("source.hwpx");
        let result = (|| {
            generate_native_source(generator, isolated_source_base, &intermediate, font_files)?;
            let read = hwpx::read_document(&intermediate)?;
            if !read.warnings.is_empty() {
                anyhow::bail!("intermediate HWPX emitted warnings")
            }
            let writer_report = crate::commands::convert::write_hwp_structural_isolated(
                &read.document,
                output,
                font_files,
            )?;
            crate::commands::reject_preservation_loss("corpus", &writer_report.preservation)
        })();
        let cleanup = if intermediate.is_file() {
            fs::remove_file(&intermediate).context("corpus intermediate cleanup failed")
        } else {
            Ok(())
        };
        result?;
        cleanup?;
        return Ok(());
    }
    generate_native_source(generator, isolated_source_base, output, font_files)
}

fn generate_native_source(
    generator: &PreparedGenerator,
    isolated_source_base: &Path,
    output: &Path,
    font_files: &[PathBuf],
) -> Result<()> {
    match generator {
        PreparedGenerator::DocumentSpec { input, format } => {
            crate::commands::compose::execute_text_with_source_and_fonts(
                input,
                (*format).into(),
                isolated_source_base,
                output,
                false,
                false,
                None,
                Some(font_files),
                &[],
            )?;
        }
        PreparedGenerator::TemplateSpecData {
            template_input,
            template_format,
            data_input,
            data_format,
        } => {
            crate::commands::template::execute_text_with_fonts(
                template_input,
                (*template_format).into(),
                data_input,
                (*data_format).into(),
                isolated_source_base,
                output,
                false,
                &[],
                Some(font_files),
                &[],
            )?;
        }
    }
    Ok(())
}

fn read_semantic(
    path: &Path,
    format: CorpusFormat,
    expected: &SemanticAssertions,
) -> std::result::Result<SemanticSummary, ()> {
    let document = read_generated_document(path, format).ok_or(())?;
    let text = document.plain_text();
    let stats = semantic_stats(&document);
    if expected
        .required_text
        .iter()
        .any(|required| !text.contains(required))
        || stats.sections < expected.min_sections
        || stats.paragraphs < expected.min_paragraphs
        || stats.tables < expected.min_tables
        || text.chars().count() < expected.min_text_chars
    {
        return Err(());
    }
    Ok(SemanticSummary {
        plain_text_sha256: sha256_hex(text.as_bytes()),
        structural_semantic_sha256: structural_semantic_sha256(&document)?,
        text_chars: text.chars().count(),
        sections: stats.sections,
        paragraphs: stats.paragraphs,
        tables: stats.tables,
        required_text_count: expected.required_text.len(),
    })
}

/// Read a generated HWPX/HWP document without warnings. Semantic and PDF
/// validation share this entry point.
fn read_generated_document(path: &Path, format: CorpusFormat) -> Option<hwp_model::Document> {
    match format {
        CorpusFormat::Hwpx => hwpx::read_document(path)
            .ok()
            .filter(|read| read.warnings.is_empty())
            .map(|read| read.document),
        CorpusFormat::Hwp => hwp5::read_document(path)
            .ok()
            .filter(|read| read.warnings.is_empty())
            .map(|read| read.document),
    }
}

fn structural_semantic_sha256(document: &hwp_model::Document) -> std::result::Result<String, ()> {
    let mut projection = SemanticProjection::new();
    projection.project_document(document)?;
    Ok(projection.finish())
}

/// Target-neutral, bounded streaming projection shared by HWPX and HWP.
///
/// Only modeled, author-visible semantics enter this digest. Format-specific raw records,
/// cached line layout and opaque pass-through XML/bytes are deliberately excluded and disclosed
/// in the machine-readable limitations list. Every field is length framed, so concatenation is
/// unambiguous without materializing a second document-sized JSON value.
struct SemanticProjection {
    hasher: Sha256,
    nodes: usize,
    bytes: u64,
}

impl SemanticProjection {
    fn new() -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"hwp-corpus-common-semantic-v1\0");
        Self {
            hasher,
            nodes: 0,
            bytes: 0,
        }
    }

    fn finish(self) -> String {
        self.hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn node(&mut self, name: &str) -> std::result::Result<(), ()> {
        self.nodes = self.nodes.checked_add(1).ok_or(())?;
        if self.nodes > MAX_SEMANTIC_NODES {
            return Err(());
        }
        self.field("node", name.as_bytes())
    }

    fn field(&mut self, name: &str, value: &[u8]) -> std::result::Result<(), ()> {
        let added = u64::try_from(name.len())
            .ok()
            .and_then(|name_len| name_len.checked_add(u64::try_from(value.len()).ok()?))
            .and_then(|length| length.checked_add(16))
            .ok_or(())?;
        self.bytes = self.bytes.checked_add(added).ok_or(())?;
        if self.bytes > MAX_SEMANTIC_BYTES {
            return Err(());
        }
        self.hasher.update((name.len() as u64).to_le_bytes());
        self.hasher.update(name.as_bytes());
        self.hasher.update((value.len() as u64).to_le_bytes());
        self.hasher.update(value);
        Ok(())
    }

    fn bool(&mut self, name: &str, value: bool) -> std::result::Result<(), ()> {
        self.field(name, &[u8::from(value)])
    }

    fn u64(&mut self, name: &str, value: u64) -> std::result::Result<(), ()> {
        self.field(name, &value.to_le_bytes())
    }

    fn i64(&mut self, name: &str, value: i64) -> std::result::Result<(), ()> {
        self.field(name, &value.to_le_bytes())
    }

    fn usize(&mut self, name: &str, value: usize) -> std::result::Result<(), ()> {
        self.u64(name, u64::try_from(value).map_err(|_| ())?)
    }

    fn text(&mut self, name: &str, value: &str) -> std::result::Result<(), ()> {
        self.field(name, value.as_bytes())
    }

    fn optional_text(&mut self, name: &str, value: Option<&str>) -> std::result::Result<(), ()> {
        self.bool(&format!("{name}.present"), value.is_some())?;
        if let Some(value) = value {
            self.text(name, value)?;
        }
        Ok(())
    }

    fn optional_u64(&mut self, name: &str, value: Option<u64>) -> std::result::Result<(), ()> {
        self.bool(&format!("{name}.present"), value.is_some())?;
        if let Some(value) = value {
            self.u64(name, value)?;
        }
        Ok(())
    }

    fn project_document(&mut self, document: &hwp_model::Document) -> std::result::Result<(), ()> {
        self.node("document")?;
        self.optional_text("metadata.title", document.metadata.title.as_deref())?;
        self.optional_text("metadata.author", document.metadata.author.as_deref())?;
        self.optional_text("metadata.subject", document.metadata.subject.as_deref())?;
        self.optional_text("metadata.keywords", document.metadata.keywords.as_deref())?;
        self.optional_text(
            "metadata.description",
            document.metadata.description.as_deref(),
        )?;
        self.optional_text(
            "metadata.last_saved_by",
            document.metadata.last_saved_by.as_deref(),
        )?;
        self.optional_u64("metadata.create_time", document.metadata.create_time)?;
        self.optional_u64("metadata.modify_time", document.metadata.modify_time)?;
        self.usize("sections.count", document.sections.len())?;
        for section in &document.sections {
            self.node("section")?;
            self.usize("section.paragraphs.count", section.paragraphs.len())?;
            for paragraph in &section.paragraphs {
                self.project_paragraph(document, paragraph)?;
            }
        }
        Ok(())
    }

    fn project_paragraph(
        &mut self,
        document: &hwp_model::Document,
        paragraph: &hwp_model::Paragraph,
    ) -> std::result::Result<(), ()> {
        self.node("paragraph")?;
        // HWP5 requires 0x03 on the first paragraph of a section; HWPX represents the section
        // structurally. Only the common explicit page/column break bits are author semantics.
        self.u64(
            "paragraph.break_type",
            u64::from(paragraph.header.break_type & 0x0c),
        )?;
        self.project_para_shape(document, paragraph.para_shape.0)?;
        self.project_style(document, paragraph.style.0)?;

        // HWP5 stores the paragraph terminator as a trailing CharCtrl(13), while HWPX models the
        // paragraph boundary structurally. It is not author-visible content, so normalize it away.
        let semantic_chars = paragraph
            .chars
            .strip_suffix(&[hwp_model::HwpChar::CharCtrl(
                hwp_model::ctrl_char::PARA_BREAK,
            )])
            .unwrap_or(&paragraph.chars);
        self.usize("paragraph.chars.count", semantic_chars.len())?;
        for character in semantic_chars {
            self.node("character")?;
            match character {
                hwp_model::HwpChar::Text(character) => {
                    self.text("character.kind", "text")?;
                    let mut encoded = [0u8; 4];
                    self.text("character.value", character.encode_utf8(&mut encoded))?;
                }
                hwp_model::HwpChar::CharCtrl(code) => {
                    self.text("character.kind", "char_control")?;
                    self.u64("character.code", u64::from(*code))?;
                }
                hwp_model::HwpChar::InlineCtrl { code, .. } => {
                    self.text("character.kind", "inline_control")?;
                    self.u64("character.code", u64::from(*code))?;
                }
                hwp_model::HwpChar::ExtCtrl {
                    code,
                    ctrl_id,
                    ctrl_index,
                    ..
                } => {
                    self.text("character.kind", "extended_control")?;
                    self.u64("character.code", u64::from(*code))?;
                    self.field("character.control_id", ctrl_id)?;
                    if hwp_convert::field::is_field_ctrl_id(ctrl_id) {
                        let metadata = ctrl_index
                            .and_then(|index| paragraph.controls.get(index as usize))
                            .map(hwp_convert::field::field_meta)
                            .unwrap_or((None, None));
                        self.optional_text("field.name", metadata.0.as_deref())?;
                        self.optional_text("field.command", metadata.1.as_deref())?;
                    } else if ctrl_id == b"bokm" {
                        let name = ctrl_index
                            .and_then(|index| paragraph.controls.get(index as usize))
                            .and_then(hwp_convert::bookmark::bookmark_name);
                        self.optional_text("bookmark.name", name.as_deref())?;
                    }
                }
            }
        }

        self.usize(
            "paragraph.char_shape_runs.count",
            paragraph.char_shape_runs.len(),
        )?;
        for (start, shape) in &paragraph.char_shape_runs {
            self.node("char_shape_run")?;
            self.u64("char_shape_run.start", u64::from(*start))?;
            self.project_char_shape(document, shape.0)?;
        }
        self.usize("paragraph.controls.count", paragraph.controls.len())?;
        for control in &paragraph.controls {
            self.project_control(document, control)?;
        }
        Ok(())
    }

    fn project_para_shape(
        &mut self,
        document: &hwp_model::Document,
        id: u16,
    ) -> std::result::Result<(), ()> {
        self.node("para_shape")?;
        let Some(shape) = document.header.para_shapes.get(id as usize) else {
            self.bool("para_shape.resolved", false)?;
            return Ok(());
        };
        self.bool("para_shape.resolved", true)?;
        self.u64("para_shape.alignment", u64::from(shape.alignment()))?;
        self.u64("para_shape.head_type", u64::from(shape.head_type()))?;
        self.u64("para_shape.head_level", u64::from(shape.head_level()))?;
        self.i64("para_shape.indent", i64::from(shape.indent))?;
        self.i64("para_shape.margin_left", i64::from(shape.margin_left))?;
        self.i64("para_shape.margin_right", i64::from(shape.margin_right))?;
        self.i64("para_shape.spacing_top", i64::from(shape.spacing_top))?;
        self.i64("para_shape.spacing_bottom", i64::from(shape.spacing_bottom))?;
        self.i64(
            "para_shape.line_spacing_old",
            i64::from(shape.line_spacing_old),
        )?;
        self.u64(
            "para_shape.line_spacing_type",
            u64::from(shape.line_spacing_type),
        )?;
        self.i64("para_shape.line_spacing", i64::from(shape.line_spacing))?;
        for offset in shape.border_offsets {
            self.i64("para_shape.border_offset", i64::from(offset))?;
        }
        self.project_tab_def(document, shape.tab_def_id)?;
        self.project_list_definition(document, shape)?;
        self.project_border_fill(document, shape.border_fill_id)?;
        Ok(())
    }

    fn project_style(
        &mut self,
        document: &hwp_model::Document,
        id: u16,
    ) -> std::result::Result<(), ()> {
        self.node("style")?;
        let Some(style) = document.header.styles.get(id as usize) else {
            self.bool("style.resolved", false)?;
            return Ok(());
        };
        self.bool("style.resolved", true)?;
        self.text("style.name", &style.name)?;
        self.text("style.english_name", &style.english_name)?;
        self.project_para_shape(document, style.para_shape.0)?;
        self.project_char_shape(document, style.char_shape.0)
    }

    fn project_char_shape(
        &mut self,
        document: &hwp_model::Document,
        id: u16,
    ) -> std::result::Result<(), ()> {
        self.node("char_shape")?;
        let Some(shape) = document.header.char_shapes.get(id as usize) else {
            self.bool("char_shape.resolved", false)?;
            return Ok(());
        };
        self.bool("char_shape.resolved", true)?;
        for (language, face_id) in shape.face_ids.iter().enumerate() {
            let name = document.header.fonts[language]
                .get(*face_id as usize)
                .map(|face| face.name.as_str());
            self.optional_text("char_shape.font_name", name)?;
        }
        for value in shape.ratios {
            self.u64("char_shape.ratio", u64::from(value))?;
        }
        for value in shape.spacings {
            self.i64("char_shape.spacing", i64::from(value))?;
        }
        for value in shape.rel_sizes {
            self.u64("char_shape.relative_size", u64::from(value))?;
        }
        for value in shape.offsets {
            self.i64("char_shape.offset", i64::from(value))?;
        }
        self.i64("char_shape.base_size", i64::from(shape.base_size))?;
        for (name, value) in [
            ("bold", shape.is_bold()),
            ("italic", shape.is_italic()),
            ("underline", shape.has_underline()),
            ("outline", shape.has_outline()),
            ("shadow", shape.has_shadow()),
            ("emboss", shape.is_emboss()),
            ("engrave", shape.is_engrave()),
            ("superscript", shape.is_superscript()),
            ("subscript", shape.is_subscript()),
        ] {
            self.bool(&format!("char_shape.{name}"), value)?;
        }
        self.u64(
            "char_shape.underline_kind",
            u64::from(shape.underline_kind()),
        )?;
        self.i64("char_shape.shadow_gap_x", i64::from(shape.shadow_gap.0))?;
        self.i64("char_shape.shadow_gap_y", i64::from(shape.shadow_gap.1))?;
        self.u64("char_shape.text_color", u64::from(shape.text_color))?;
        self.u64(
            "char_shape.underline_color",
            u64::from(shape.underline_color),
        )?;
        self.u64("char_shape.shade_color", u64::from(shape.shade_color))?;
        self.u64("char_shape.shadow_color", u64::from(shape.shadow_color))?;
        self.project_border_fill(document, shape.border_fill_id)
    }

    fn project_tab_def(
        &mut self,
        document: &hwp_model::Document,
        id: u16,
    ) -> std::result::Result<(), ()> {
        self.node("tab_definition")?;
        let Some(tab) = document.header.tab_stops.get(id as usize) else {
            self.bool("tab_definition.resolved", false)?;
            return Ok(());
        };
        self.bool("tab_definition.resolved", true)?;
        self.bool("tab_definition.auto_left", tab.auto_tab_left())?;
        self.bool("tab_definition.auto_right", tab.auto_tab_right())?;
        self.usize("tab_definition.items.count", tab.items.len())?;
        for item in &tab.items {
            self.node("tab_item")?;
            self.i64("tab_item.position", i64::from(item.pos))?;
            self.u64("tab_item.kind", u64::from(item.kind))?;
            self.u64("tab_item.fill", u64::from(item.fill))?;
        }
        Ok(())
    }

    fn project_list_definition(
        &mut self,
        document: &hwp_model::Document,
        shape: &hwp_model::ParaShape,
    ) -> std::result::Result<(), ()> {
        self.node("list_definition")?;
        match shape.head_type() {
            2 => {
                self.text("list_definition.kind", "numbering")?;
                let levels = document
                    .header
                    .numbering_levels
                    .get(shape.numbering_id as usize);
                self.bool("list_definition.resolved", levels.is_some())?;
                if let Some(levels) = levels {
                    self.usize("list_definition.levels.count", levels.len())?;
                    for level in levels {
                        self.node("numbering_level")?;
                        self.u64("numbering_level.start", u64::from(level.start))?;
                        self.text("numbering_level.format", &format!("{:?}", level.fmt))?;
                        self.text("numbering_level.template", &level.template)?;
                    }
                }
            }
            3 => {
                self.text("list_definition.kind", "bullet")?;
                let bullet = document
                    .header
                    .bullet_chars
                    .get(shape.numbering_id as usize);
                self.bool("list_definition.resolved", bullet.is_some())?;
                if let Some(bullet) = bullet {
                    let mut encoded = [0u8; 4];
                    self.text("list_definition.bullet", bullet.encode_utf8(&mut encoded))?;
                }
            }
            _ => self.text("list_definition.kind", "none_or_outline")?,
        }
        Ok(())
    }

    fn project_border_fill(
        &mut self,
        document: &hwp_model::Document,
        id: u16,
    ) -> std::result::Result<(), ()> {
        self.node("border_fill")?;
        let fill = id
            .checked_sub(1)
            .and_then(|index| document.header.border_fills.get(index as usize));
        self.bool("border_fill.resolved", fill.is_some())?;
        if let Some(fill) = fill {
            for side in fill.sides.iter().chain(std::iter::once(&fill.diagonal)) {
                self.node("border_line")?;
                self.u64("border_line.type", u64::from(side.line_type))?;
                self.u64("border_line.width", u64::from(side.width))?;
                self.u64("border_line.color", u64::from(side.color))?;
            }
            self.bool("border_fill.solid", fill.fill_type & 1 != 0)?;
            self.bool("border_fill.background.present", fill.bg_color.is_some())?;
            if let Some(color) = fill.bg_color {
                self.u64("border_fill.background", u64::from(color))?;
            }
        }
        Ok(())
    }

    fn project_control(
        &mut self,
        document: &hwp_model::Document,
        control: &hwp_model::Control,
    ) -> std::result::Result<(), ()> {
        self.node("control")?;
        self.field("control.id", &control.ctrl_id())?;
        match control {
            hwp_model::Control::SectionDef(section) => {
                self.text("control.kind", "section_definition")?;
                self.bool("section.page.present", section.page.is_some())?;
                if let Some(page) = section.page {
                    self.node("page_definition")?;
                    for (name, value) in [
                        ("width", page.width.0),
                        ("height", page.height.0),
                        ("margin_left", page.margin_left.0),
                        ("margin_right", page.margin_right.0),
                        ("margin_top", page.margin_top.0),
                        ("margin_bottom", page.margin_bottom.0),
                        ("margin_header", page.margin_header.0),
                        ("margin_footer", page.margin_footer.0),
                        ("gutter", page.gutter.0),
                    ] {
                        self.i64(&format!("page.{name}"), i64::from(value))?;
                    }
                    self.bool("page.landscape", page.attr & 1 != 0)?;
                    self.u64("page.binding", u64::from((page.attr >> 1) & 0x3))?;
                }
            }
            hwp_model::Control::Table(table) => self.project_table(document, table)?,
            hwp_model::Control::Picture(picture) => {
                self.text("control.kind", "picture")?;
                self.i64("picture.width", i64::from(picture.width.0))?;
                self.i64("picture.height", i64::from(picture.height.0))?;
                self.bool("picture.treat_as_char", picture.treat_as_char)?;
                self.u64("picture.z_order", u64::from(picture.z_order))?;
                self.i64("picture.vertical_offset", i64::from(picture.vert_offset))?;
                self.i64("picture.horizontal_offset", i64::from(picture.horz_offset))?;
                self.optional_text("picture.description", picture.description.as_deref())?;
                let bytes = document.resolve_bin(&picture.bin_ref);
                self.bool("picture.content.resolved", bytes.is_some())?;
                if let Some(bytes) = bytes {
                    self.usize("picture.content.bytes", bytes.len())?;
                    self.text("picture.content.sha256", &sha256_hex(bytes))?;
                }
            }
            hwp_model::Control::Generic(generic) => {
                self.text("control.kind", "generic")?;
                self.project_generic_control(document, generic)?;
            }
        }
        Ok(())
    }

    fn project_table(
        &mut self,
        document: &hwp_model::Document,
        table: &hwp_model::Table,
    ) -> std::result::Result<(), ()> {
        self.text("control.kind", "table")?;
        self.u64("table.page_break", u64::from(table.attr & 0x3))?;
        self.bool("table.repeat_header", table.attr & (1 << 2) != 0)?;
        self.bool("table.no_adjust", table.attr & (1 << 3) != 0)?;
        self.u64("table.rows", u64::from(table.rows))?;
        self.u64("table.columns", u64::from(table.cols))?;
        self.u64("table.cell_spacing", u64::from(table.cell_spacing))?;
        for margin in table.inner_margins {
            self.u64("table.inner_margin", u64::from(margin))?;
        }
        self.usize("table.row_cell_counts.count", table.row_cell_counts.len())?;
        for count in &table.row_cell_counts {
            self.u64("table.row_cell_count", u64::from(*count))?;
        }
        self.project_border_fill(document, table.border_fill.0)?;
        self.bool("table.placement.present", table.placement.is_some())?;
        if let Some(placement) = &table.placement {
            self.node("table_placement")?;
            for (name, value) in [
                ("treat_as_char", placement.treat_as_char),
                ("affect_line_spacing", placement.affect_line_spacing),
                ("flow_with_text", placement.flow_with_text),
                ("hold_anchor", placement.hold_anchor),
            ] {
                self.bool(&format!("table.placement.{name}"), value)?;
            }
            self.u64(
                "table.placement.vertical_relative_to",
                u64::from(placement.vert_rel_to),
            )?;
            self.u64(
                "table.placement.horizontal_relative_to",
                u64::from(placement.horz_rel_to),
            )?;
            self.u64(
                "table.placement.vertical_alignment",
                u64::from(placement.vert_align),
            )?;
            self.u64(
                "table.placement.horizontal_alignment",
                u64::from(placement.horz_align),
            )?;
            for (name, value) in [
                ("vertical_offset", placement.vert_offset),
                ("horizontal_offset", placement.horz_offset),
                ("z_order", placement.z_order),
                ("width", placement.width),
                ("height", placement.height),
            ] {
                self.i64(&format!("table.placement.{name}"), i64::from(value))?;
            }
            for margin in placement.out_margins {
                self.u64("table.placement.outer_margin", u64::from(margin))?;
            }
        }
        self.usize("table.cells.count", table.cells.len())?;
        for cell in &table.cells {
            self.node("table_cell")?;
            self.u64("cell.column", u64::from(cell.col))?;
            self.u64("cell.row", u64::from(cell.row))?;
            self.u64("cell.column_span", u64::from(cell.col_span))?;
            self.u64("cell.row_span", u64::from(cell.row_span))?;
            self.i64("cell.width", i64::from(cell.width.0))?;
            self.i64("cell.height", i64::from(cell.height.0))?;
            for margin in cell.margins {
                self.u64("cell.margin", u64::from(margin))?;
            }
            self.project_border_fill(document, cell.border_fill.0)?;
            self.usize("cell.paragraphs.count", cell.paragraphs.len())?;
            for paragraph in &cell.paragraphs {
                self.project_paragraph(document, paragraph)?;
            }
        }
        Ok(())
    }

    fn project_generic_control(
        &mut self,
        document: &hwp_model::Document,
        generic: &hwp_model::GenericControl,
    ) -> std::result::Result<(), ()> {
        match &generic.ctrl_id {
            b"head" | b"foot" => {
                let application = generic
                    .data
                    .get(..4)
                    .and_then(|bytes| bytes.try_into().ok())
                    .map(u32::from_le_bytes);
                self.bool("header_footer.application.present", application.is_some())?;
                if let Some(application) = application {
                    self.u64("header_footer.application", u64::from(application))?;
                }
            }
            b"pgnp" => {
                let props = generic
                    .data
                    .get(..4)
                    .and_then(|bytes| bytes.try_into().ok())
                    .map(u32::from_le_bytes);
                let side = generic
                    .data
                    .get(10..12)
                    .and_then(|bytes| bytes.try_into().ok())
                    .map(u16::from_le_bytes);
                self.bool("page_number.position.present", props.is_some())?;
                if let Some(props) = props {
                    self.u64("page_number.format", u64::from(props & 0xff))?;
                    self.u64("page_number.position", u64::from((props >> 8) & 0xff))?;
                }
                self.bool("page_number.side_character.present", side.is_some())?;
                if let Some(side) = side {
                    self.u64("page_number.side_character", u64::from(side))?;
                }
            }
            b"nwno" => {
                let start = generic
                    .data
                    .get(4..6)
                    .and_then(|bytes| bytes.try_into().ok())
                    .map(u16::from_le_bytes);
                self.bool("page_number.start.present", start.is_some())?;
                if let Some(start) = start {
                    self.u64("page_number.start", u64::from(start))?;
                }
            }
            b"pghd" => {
                let mask = generic
                    .data
                    .get(..4)
                    .and_then(|bytes| bytes.try_into().ok())
                    .map(u32::from_le_bytes);
                self.bool("page_hide.mask.present", mask.is_some())?;
                if let Some(mask) = mask {
                    self.u64("page_hide.mask", u64::from(mask))?;
                }
            }
            _ => {}
        }

        self.bool("equation.present", generic.equation.is_some())?;
        if let Some(equation) = &generic.equation {
            self.node("equation")?;
            self.text("equation.script", &equation.script)?;
            self.i64("equation.width", i64::from(equation.width))?;
            self.i64("equation.height", i64::from(equation.height))?;
            self.bool("equation.inline", equation.inline)?;
            self.i64("equation.x", i64::from(equation.x))?;
            self.i64("equation.y", i64::from(equation.y))?;
        }
        self.bool("column_definition.present", generic.column_def.is_some())?;
        if let Some(column) = &generic.column_def {
            self.node("column_definition")?;
            self.u64("column.count", u64::from(column.count))?;
            self.u64("column.kind", u64::from(column.kind))?;
            self.u64("column.direction", u64::from(column.direction))?;
            self.bool("column.same_width", column.same_width)?;
            self.i64("column.gap", i64::from(column.gap))?;
            self.usize("column.widths.count", column.widths.len())?;
            for width in &column.widths {
                self.i64("column.width", i64::from(*width))?;
            }
            self.bool("column.divider.present", column.divider.is_some())?;
            if let Some(divider) = column.divider {
                self.u64("column.divider.type", u64::from(divider.line_type))?;
                self.u64("column.divider.width", u64::from(divider.width))?;
                self.u64("column.divider.color", u64::from(divider.color))?;
            }
        }
        self.usize(
            "generic.paragraph_lists.count",
            generic.paragraph_lists.len(),
        )?;
        for list in &generic.paragraph_lists {
            self.node("paragraph_list")?;
            self.usize("paragraph_list.paragraphs.count", list.paragraphs.len())?;
            for paragraph in &list.paragraphs {
                self.project_paragraph(document, paragraph)?;
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct SemanticStats {
    sections: usize,
    paragraphs: usize,
    tables: usize,
}

fn semantic_stats(document: &hwp_model::Document) -> SemanticStats {
    let mut stats = SemanticStats {
        sections: document.sections.len(),
        ..Default::default()
    };
    for section in &document.sections {
        count_paragraphs(&section.paragraphs, &mut stats);
    }
    stats
}

fn count_paragraphs(paragraphs: &[hwp_model::Paragraph], stats: &mut SemanticStats) {
    stats.paragraphs += paragraphs.len();
    for paragraph in paragraphs {
        for control in &paragraph.controls {
            match control {
                hwp_model::Control::Table(table) => {
                    stats.tables += 1;
                    for cell in &table.cells {
                        count_paragraphs(&cell.paragraphs, stats);
                    }
                }
                hwp_model::Control::Generic(generic) => {
                    for list in &generic.paragraph_lists {
                        count_paragraphs(&list.paragraphs, stats);
                    }
                }
                _ => {}
            }
        }
    }
}

fn read_certification_summary(
    report_path: &Path,
    expected: &CertificationAssertions,
    allowed_font_hashes: &BTreeSet<String>,
) -> std::result::Result<CertificationSummary, ()> {
    let bytes = read_regular_bounded(report_path, MAX_REPORT_BYTES).map_err(|_| ())?;
    let report: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
    let overall = report
        .get("overall")
        .and_then(|value| value.as_str())
        .ok_or(())?;
    let render = report
        .get("render")
        .and_then(|value| value.as_object())
        .ok_or(())?;
    let total_pages = usize::try_from(
        render
            .get("total_pages")
            .and_then(|value| value.as_u64())
            .ok_or(())?,
    )
    .map_err(|_| ())?;
    let selected_pages = render
        .get("selected_pages")
        .and_then(|value| value.as_array())
        .ok_or(())?
        .iter()
        .map(|value| value.as_u64().and_then(|page| usize::try_from(page).ok()))
        .collect::<Option<Vec<_>>>()
        .ok_or(())?;
    let issue_count = render
        .get("issue_count")
        .and_then(|value| value.as_u64())
        .ok_or(())?;
    let issue_sha256 = render
        .get("issue_sha256")
        .and_then(|value| value.as_str())
        .filter(|value| validate_sha256(value).is_ok())
        .ok_or(())?
        .to_string();
    let issue_complete = render
        .get("issue_log_complete")
        .and_then(|value| value.as_bool())
        .ok_or(())?;
    let fonts = render
        .get("fonts")
        .and_then(|value| value.as_array())
        .ok_or(())?
        .iter()
        .map(|font| {
            let hash = font.get("font_file_sha256")?.as_str()?;
            let outcome = font.get("outcome")?.as_str()?;
            if validate_sha256(hash).is_err()
                || !allowed_font_hashes.contains(hash)
                || outcome != "matched"
            {
                return None;
            }
            Some(CertificationFontIdentity {
                font_file_sha256: hash.to_string(),
                outcome: outcome.to_string(),
            })
        })
        .collect::<Option<Vec<_>>>()
        .filter(|fonts| !fonts.is_empty())
        .ok_or(())?;
    let pages = render
        .get("pages")
        .and_then(|value| value.as_array())
        .ok_or(())?
        .iter()
        .map(|page| {
            let page_number = usize::try_from(page.get("page")?.as_u64()?).ok()?;
            let png_sha256 = page.get("png_sha256")?.as_str()?;
            validate_sha256(png_sha256).ok()?;
            let visual_blank = page.get("visual_blank")?.as_bool()?;
            let outside = detection_evidence(page, "outside_page_bounds")?;
            let collision = detection_evidence(page, "possible_collision")?;
            let geometry_clear = outside.is_clear() && collision.is_clear();
            Some((
                PageHash {
                    page: page_number,
                    png_sha256: png_sha256.to_string(),
                    visual_blank,
                    outside_page_bounds: outside,
                    possible_collision: collision,
                },
                geometry_clear,
            ))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or(())?;
    let diagnostics_complete = issue_complete && pages.iter().all(|(_, complete)| *complete);
    if overall != "passed"
        || total_pages < expected.page_count_min
        || total_pages > expected.page_count_max
        || selected_pages != expected.selected_pages
        || issue_count > expected.max_render_issue_count
        || (expected.require_nonblank && pages.iter().any(|(page, _)| page.visual_blank))
        || (expected.require_complete_diagnostics && !diagnostics_complete)
    {
        return Err(());
    }
    Ok(CertificationSummary {
        overall: overall.to_string(),
        total_pages,
        selected_pages,
        render_issue_count: issue_count,
        render_issue_sha256: issue_sha256,
        fonts,
        pages: pages.into_iter().map(|(page, _)| page).collect(),
    })
}

impl DetectionEvidence {
    fn is_clear(&self) -> bool {
        self.result == "not_detected" && self.count == 0 && self.complete
    }
}

fn detection_evidence(page: &serde_json::Value, name: &str) -> Option<DetectionEvidence> {
    let detection = page.get(name)?;
    Some(DetectionEvidence {
        result: detection.get("result")?.as_str()?.to_string(),
        count: detection.get("count")?.as_u64()?,
        complete: detection.get("complete")?.as_bool()?,
    })
}

fn snapshot_pin(base: &Path, pin: &FilePin, max_bytes: u64) -> Result<(PathBuf, Vec<u8>)> {
    validate_sha256(&pin.sha256)?;
    let relative = validate_portable_relative(&pin.path)?;
    let snapshot =
        hwp_cli::asset_snapshot::read_contained(base, relative, max_bytes).map_err(|error| {
            anyhow::anyhow!("corpus input snapshot rejected: {}", error.code.as_str())
        })?;
    let observed = snapshot
        .sha256
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if observed != pin.sha256 {
        anyhow::bail!("corpus input hash mismatch")
    }
    Ok((base.join(relative), snapshot.data))
}

fn validate_portable_relative(relative: &str) -> Result<&Path> {
    if relative.is_empty()
        || relative.len() > MAX_ARTIFACT_PATH_BYTES
        || !relative.is_ascii()
        || relative.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
        || relative.contains(['\\', ':'])
    {
        anyhow::bail!("corpus relative path rejected")
    }
    let path = Path::new(relative);
    if relative
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        anyhow::bail!("corpus path contains a non-normal component")
    }
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        anyhow::bail!("corpus path must be normalized and relative")
    }
    for component in relative.split('/') {
        if component.ends_with(['.', ' ']) {
            anyhow::bail!("corpus path has a Windows-ambiguous component")
        }
        let folded = component.trim_end_matches(['.', ' ']).to_ascii_uppercase();
        let stem = folded.split('.').next().unwrap_or("");
        if matches!(
            stem,
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        ) {
            anyhow::bail!("corpus path uses a Windows reserved component")
        }
    }
    Ok(path)
}

fn read_regular_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        anyhow::bail!("bounded read requires a regular non-symlink file")
    }
    if metadata.len() > max_bytes {
        anyhow::bail!("bounded read limit exceeded")
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    File::open(path)?
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_bytes {
        anyhow::bail!("bounded read limit exceeded")
    }
    Ok(bytes)
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || id.starts_with('-')
        || id.ends_with('-')
    {
        anyhow::bail!("corpus id rejected")
    }
    Ok(())
}

fn validate_short_ascii(value: &str, max: usize, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > max
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/+-".contains(&byte))
    {
        anyhow::bail!("{label} rejected")
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        anyhow::bail!("invalid lowercase sha256")
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn pretty_json_bounded(value: &impl Serialize, max_bytes: u64) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_bytes {
        anyhow::bail!("corpus machine report limit exceeded")
    }
    Ok(bytes)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn collect_artifacts(root: &Path) -> Result<Vec<Artifact>> {
    collect_tree(root).map(|(files, _)| files)
}

fn collect_tree(root: &Path) -> Result<(Vec<Artifact>, BTreeSet<String>)> {
    fn walk(
        root: &Path,
        current: &Path,
        depth: usize,
        files: &mut Vec<Artifact>,
        directories: &mut BTreeSet<String>,
        total: &mut u64,
    ) -> Result<()> {
        if depth > MAX_TREE_DEPTH {
            anyhow::bail!("corpus artifact tree depth exceeded")
        }
        for entry in fs::read_dir(current)? {
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                anyhow::bail!("corpus artifact tree contains a symlink")
            }
            if metadata.file_type().is_dir() {
                let relative = normalized_relative(root, &path)?;
                if relative.len() > MAX_ARTIFACT_PATH_BYTES
                    || !directories.insert(relative)
                    || directories.len() > MAX_TREE_DIRECTORIES
                {
                    anyhow::bail!("corpus artifact directory budget exceeded")
                }
                walk(root, &path, depth + 1, files, directories, total)?;
            } else if metadata.file_type().is_file() {
                if hwp_cli::certification::has_multiple_links(&path, &metadata) {
                    anyhow::bail!("corpus artifact tree contains a multiply-linked file")
                }
                *total = total
                    .checked_add(metadata.len())
                    .context("corpus artifact byte count overflow")?;
                if metadata.len() > MAX_ARTIFACT_FILE_BYTES {
                    anyhow::bail!("corpus artifact file byte budget exceeded")
                }
                if *total > MAX_TREE_BYTES || files.len() >= MAX_TREE_FILES - 1 {
                    anyhow::bail!("corpus artifact tree budget exceeded")
                }
                let relative = normalized_relative(root, &path)?;
                if relative.len() > MAX_ARTIFACT_PATH_BYTES {
                    anyhow::bail!("corpus artifact path budget exceeded")
                }
                files.push(Artifact {
                    path: relative,
                    bytes: metadata.len(),
                    sha256: sha256_hex(&read_regular_bounded(&path, MAX_TREE_BYTES)?),
                });
            } else {
                anyhow::bail!("corpus artifact tree contains a non-file entry")
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    let mut directories = BTreeSet::new();
    let mut total = 0;
    walk(root, root, 0, &mut files, &mut directories, &mut total)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok((files, directories))
}

fn normalized_relative(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root)?;
    let components = relative
        .components()
        .map(|component| match component {
            Component::Normal(value) => value.to_str().map(str::to_string),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .context("corpus artifact path is not normalized UTF-8")?;
    Ok(components.join("/"))
}

/// 실패한 case는 자기 디렉터리를 비운 채 남긴다. 그 디렉터리를 "예상 밖"으로 처리하면
/// 실패 사유가 담긴 report 자체가 게시되지 못하고 트리 불일치 오류로 덮인다. 그래서 runner가
/// 만드는 case 디렉터리는 파일이 없어도 허용한다.
fn audit_tree(root: &Path, manifest: &ArtifactManifest, case_ids: &[&str]) -> Result<()> {
    let expected_paths: BTreeSet<_> = manifest
        .files
        .iter()
        .map(|artifact| artifact.path.as_str())
        .chain(std::iter::once("artifacts.json"))
        .collect();
    let (observed, observed_directories) = collect_tree(root)?;
    let observed_paths: BTreeSet<_> = observed
        .iter()
        .map(|artifact| artifact.path.as_str())
        .collect();
    if observed_paths != expected_paths {
        anyhow::bail!("corpus artifact tree does not match the closed manifest")
    }
    for expected in &manifest.files {
        if observed
            .iter()
            .find(|observed| observed.path == expected.path)
            != Some(expected)
        {
            anyhow::bail!("corpus artifact size/hash does not match the closed manifest")
        }
    }
    let expected_directories = expected_paths
        .iter()
        .flat_map(|path| {
            let components = path.split('/').collect::<Vec<_>>();
            (1..components.len()).map(move |end| components[..end].join("/"))
        })
        .chain(["documents".to_string(), "certification".to_string()])
        .chain(
            case_ids
                .iter()
                .flat_map(|id| [format!("documents/{id}"), format!("certification/{id}")]),
        )
        .collect::<BTreeSet<_>>();
    if !observed_directories.is_subset(&expected_directories) {
        // 경로는 닫힌 manifest에서 온 산출물 이름이라 진단에 실어도 된다.
        let unexpected = observed_directories
            .difference(&expected_directories)
            .cloned()
            .collect::<Vec<_>>();
        anyhow::bail!(
            "corpus artifact tree contains directories outside the closed manifest: {unexpected:?}"
        )
    }
    Ok(())
}

struct AtomicCorpusDir {
    root: PathBuf,
    destination: PathBuf,
    parent: PathBuf,
    published: bool,
}

impl AtomicCorpusDir {
    fn new(destination: &Path) -> Result<Self> {
        if fs::symlink_metadata(destination).is_ok() {
            anyhow::bail!("corpus report directory must not already exist")
        }
        let name = destination
            .file_name()
            .filter(|name| !name.is_empty())
            .context("corpus report directory needs a final component")?;
        let parent = destination
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .canonicalize()
            .context("corpus report parent directory is unavailable")?;
        if !fs::metadata(&parent)?.is_dir() {
            anyhow::bail!("corpus report parent is not a directory")
        }
        let destination = parent.join(name);
        if fs::symlink_metadata(&destination).is_ok() {
            anyhow::bail!("corpus report directory must not already exist")
        }
        let mut root = None;
        for _ in 0..128 {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random)
                .map_err(|error| anyhow::anyhow!("corpus random token failed: {error}"))?;
            let token = sha256_hex(&random);
            let candidate = parent.join(format!(
                ".{}.hwp-corpus-{}-{token}.tmp",
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
            root: root.context("could not create unique corpus workspace")?,
            destination,
            parent,
            published: false,
        })
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn publish(&mut self) -> Result<()> {
        sync_tree(&self.root)?;
        if fs::symlink_metadata(&self.destination).is_ok() {
            anyhow::bail!("corpus report destination appeared during execution")
        }
        rename_directory_noreplace(&self.root, &self.destination)?;
        sync_parent_directory(&self.parent)?;
        self.published = true;
        Ok(())
    }
}

impl Drop for AtomicCorpusDir {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
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
                anyhow::bail!("unsupported corpus artifact entry during fsync")
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
    anyhow::bail!("atomic no-replace corpus publish is unsupported on this platform")
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
        "private corpus workspace ACL unsupported",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn compiled_official_letter() -> hwp_model::Document {
        let root = workspace_root();
        let source =
            fs::read_to_string(root.join("corpus/structured-v1/cases/official-letter/spec.json"))
                .unwrap();
        let spec = hwp_cli::document_spec::parse_spec(&source, SpecInputFormat::Json).unwrap();
        hwp_cli::document_spec::compile_spec(
            &spec,
            &root,
            Path::new("projection-test.hwpx"),
            true,
            false,
            &[],
        )
        .unwrap()
        .document
    }

    fn generic_control(
        ctrl_id: [u8; 4],
        data: Vec<u8>,
        equation: Option<hwp_model::Equation>,
    ) -> hwp_model::Control {
        hwp_model::Control::Generic(hwp_model::GenericControl {
            ctrl_id,
            data,
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
        })
    }

    #[test]
    fn rejects_windows_special_paths_on_every_platform() {
        for path in [
            "../x",
            "a//b",
            "a/./b",
            "a/..",
            "//server/share",
            "C:/x",
            "C:relative",
            "file:ads",
            "x\\y",
            "CON",
            "con.txt",
            "a/NUL.txt",
            "a/com1.log",
            "x.",
            "x ",
            "dir./file",
            "dir /file",
            "line\nbreak",
            "control\u{001f}byte",
            "delete\u{007f}byte",
            "제목.json",
        ] {
            assert!(validate_portable_relative(path).is_err(), "{path}");
        }
        assert!(validate_portable_relative(&format!("{}.json", "a".repeat(513))).is_err());
        assert_eq!(
            validate_portable_relative("cases/official-letter/spec.json").unwrap(),
            Path::new("cases/official-letter/spec.json")
        );
    }

    #[test]
    fn sha256_validation_is_lowercase_and_exact() {
        assert!(validate_sha256(&"a".repeat(64)).is_ok());
        assert!(validate_sha256(&"A".repeat(64)).is_err());
        assert!(validate_sha256(&"a".repeat(63)).is_err());
    }

    #[test]
    fn checked_in_manifest_and_every_pinned_input_are_loadable() {
        let root = workspace_root();
        let corpus = root.join("corpus/structured-v1");
        let manifest_bytes =
            read_regular_bounded(&corpus.join("manifest.json"), MAX_MANIFEST_BYTES).unwrap();
        assert_eq!(sha256_hex(&manifest_bytes), FROZEN_MANIFEST_SHA256);
        let manifest: CorpusManifest = serde_json::from_slice(&manifest_bytes).unwrap();
        validate_manifest(&manifest).unwrap();
        let (_, policy_bytes) =
            snapshot_pin(&corpus, &manifest.policy, MAX_MANIFEST_BYTES).unwrap();
        let policy: CertificationPolicy = serde_json::from_slice(&policy_bytes).unwrap();
        verify_policy_contract(&policy, &manifest.fonts).unwrap();
        for font in &manifest.fonts {
            // 폰트 바이트는 커밋하지 않는다. scripts/fetch-corpus-fonts.sh 가 채우기 전에는 건너뛰고,
            // 있으면 커밋된 입력과 똑같이 hash를 강제한다(코퍼스 게이트는 fetch 후 항상 검사).
            if !corpus.join(&font.path).exists() {
                eprintln!("skip: corpus fonts absent — run scripts/fetch-corpus-fonts.sh");
                continue;
            }
            for pin in [
                FilePin {
                    path: font.path.clone(),
                    sha256: font.sha256.clone(),
                },
                FilePin {
                    path: font.license_path.clone(),
                    sha256: font.license_sha256.clone(),
                },
                FilePin {
                    path: font.metadata_path.clone(),
                    sha256: font.metadata_sha256.clone(),
                },
            ] {
                snapshot_pin(&corpus, &pin, MAX_ARTIFACT_FILE_BYTES).unwrap();
            }
        }
        for case in &manifest.cases {
            prepare_generator(&case.generator, &corpus).unwrap();
        }
    }

    #[test]
    fn corpus_schemas_are_closed_json_objects() {
        let root = workspace_root();
        for (name, expected_sha256) in [
            (
                "structured-corpus-v1.schema.json",
                FROZEN_MANIFEST_SCHEMA_SHA256,
            ),
            (
                "structured-corpus-run-v1.schema.json",
                FROZEN_RUN_SCHEMA_SHA256,
            ),
            (
                "structured-corpus-artifacts-v1.schema.json",
                FROZEN_ARTIFACT_SCHEMA_SHA256,
            ),
        ] {
            let bytes =
                read_regular_bounded(&root.join("schemas").join(name), MAX_REPORT_BYTES).unwrap();
            assert_eq!(sha256_hex(&bytes), expected_sha256);
            let schema: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                schema.get("type").and_then(|value| value.as_str()),
                Some("object")
            );
            assert_eq!(
                schema
                    .get("additionalProperties")
                    .and_then(|value| value.as_bool()),
                Some(false)
            );
            let validator = jsonschema::options()
                .with_draft(jsonschema::Draft::Draft202012)
                .build(&schema)
                .unwrap();
            for malformed in [
                serde_json::Value::Null,
                serde_json::json!([]),
                serde_json::json!("object"),
                serde_json::json!({"unexpected": []}),
            ] {
                assert!(!validator.is_valid(&malformed), "{name}: {malformed}");
            }
        }
    }

    #[test]
    fn tounicode_trace_requires_every_character_and_whitespace_slot() {
        assert!(tounicode_trace_matches("all text\r", "all text\n"));
        assert!(!tounicode_trace_matches(
            "required text X",
            "required text Y"
        ));
        assert!(!tounicode_trace_matches("missing space", "missing  space"));
    }

    #[test]
    fn failed_pdf_diagnostics_validate_while_passed_pdf_is_strict() {
        let root = workspace_root();
        let bytes = read_regular_bounded(
            &root.join("schemas/structured-corpus-run-v1.schema.json"),
            MAX_REPORT_BYTES,
        )
        .unwrap();
        let corpus_schema: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let mut format_schema = corpus_schema.pointer("/$defs/format").unwrap().clone();
        format_schema.as_object_mut().unwrap().insert(
            "$schema".to_string(),
            serde_json::json!("https://json-schema.org/draft/2020-12/schema"),
        );
        format_schema
            .as_object_mut()
            .unwrap()
            .insert("$defs".to_string(), corpus_schema["$defs"].clone());
        let validator = jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .build(&format_schema)
            .unwrap();

        let hash = "a".repeat(64);
        let failed_with_diagnostics = serde_json::json!({
            "format": "hwp",
            "status": "failed",
            "reason_codes": ["pdf_tounicode_roundtrip_failed"],
            "two_run_byte_identical": true,
            "output_sha256": hash,
            "output_bytes": 1,
            "two_run_render_identical": true,
            "two_run_pdf_identical": true,
            "pdf": {
                "sha256": "b".repeat(64),
                "bytes": 1,
                "page_count": 2,
                "tounicode_roundtrip_ok": false
            },
            "semantic": null,
            "certification": null
        });
        assert!(validator.is_valid(&failed_with_diagnostics));

        let mut failed_without_inspection = failed_with_diagnostics.clone();
        failed_without_inspection["pdf"] = serde_json::Value::Null;
        assert!(validator.is_valid(&failed_without_inspection));

        let clear_detection = serde_json::json!({
            "result": "not_detected",
            "count": 0,
            "complete": true
        });
        let mut passed = serde_json::json!({
            "format": "hwp",
            "status": "passed",
            "reason_codes": [],
            "two_run_byte_identical": true,
            "output_sha256": "a".repeat(64),
            "output_bytes": 1,
            "two_run_render_identical": true,
            "two_run_pdf_identical": true,
            "pdf": {
                "sha256": "b".repeat(64),
                "bytes": 1,
                "page_count": 1,
                "tounicode_roundtrip_ok": true
            },
            "semantic": {
                "plain_text_sha256": "c".repeat(64),
                "structural_semantic_sha256": "d".repeat(64),
                "text_chars": 80,
                "sections": 1,
                "paragraphs": 5,
                "tables": 1,
                "required_text_count": 3
            },
            "certification": {
                "overall": "passed",
                "total_pages": 1,
                "selected_pages": [1],
                "render_issue_count": 0,
                "render_issue_sha256": "e".repeat(64),
                "fonts": [{
                    "font_file_sha256": FROZEN_FONT_SHA256,
                    "outcome": "matched"
                }],
                "pages": [{
                    "page": 1,
                    "png_sha256": "f".repeat(64),
                    "visual_blank": false,
                    "outside_page_bounds": clear_detection,
                    "possible_collision": {
                        "result": "not_detected",
                        "count": 0,
                        "complete": true
                    }
                }]
            }
        });
        assert!(validator.is_valid(&passed));
        passed["pdf"]["page_count"] = serde_json::json!(2);
        assert!(!validator.is_valid(&passed));
        passed["pdf"]["page_count"] = serde_json::json!(1);
        passed["pdf"]["tounicode_roundtrip_ok"] = serde_json::json!(false);
        assert!(!validator.is_valid(&passed));
    }

    #[test]
    fn detected_or_incomplete_geometry_never_counts_as_clear() {
        let clear = serde_json::json!({
            "possible_collision": {
                "result": "not_detected",
                "count": 0,
                "complete": true
            }
        });
        assert!(
            detection_evidence(&clear, "possible_collision")
                .unwrap()
                .is_clear()
        );
        for detected in [
            serde_json::json!({"result":"detected","count":1,"complete":true}),
            serde_json::json!({"result":"not_detected","count":1,"complete":true}),
            serde_json::json!({"result":"not_detected","count":0,"complete":false}),
        ] {
            let page = serde_json::json!({"possible_collision": detected});
            assert!(
                !detection_evidence(&page, "possible_collision")
                    .unwrap()
                    .is_clear()
            );
        }
    }

    #[test]
    fn common_semantic_digest_detects_metadata_paragraph_and_table_cell_loss() {
        let original = compiled_official_letter();
        let digest = structural_semantic_sha256(&original).unwrap();

        let mut metadata = original.clone();
        metadata.metadata.subject = Some("changed".to_string());
        assert_ne!(structural_semantic_sha256(&metadata).unwrap(), digest);

        let mut metadata_time = original.clone();
        metadata_time.metadata.create_time = Some(133_713_837_705_000_000);
        assert_ne!(structural_semantic_sha256(&metadata_time).unwrap(), digest);

        let mut paragraph = original.clone();
        paragraph.sections[0].paragraphs[1]
            .chars
            .push(hwp_model::HwpChar::Text('X'));
        assert_ne!(structural_semantic_sha256(&paragraph).unwrap(), digest);

        let mut table_cell = original.clone();
        let table = table_cell.sections[0]
            .paragraphs
            .iter_mut()
            .flat_map(|paragraph| paragraph.controls.iter_mut())
            .find_map(|control| match control {
                hwp_model::Control::Table(table) => Some(table),
                _ => None,
            })
            .unwrap();
        table.cells[0].paragraphs[0].chars.clear();
        assert_ne!(structural_semantic_sha256(&table_cell).unwrap(), digest);
    }

    #[test]
    fn common_semantic_digest_detects_page_style_run_list_and_break_mutations() {
        let original = compiled_official_letter();
        let digest = structural_semantic_sha256(&original).unwrap();

        let mut page = original.clone();
        let page_def = page.sections[0]
            .paragraphs
            .iter_mut()
            .flat_map(|paragraph| paragraph.controls.iter_mut())
            .find_map(|control| match control {
                hwp_model::Control::SectionDef(section) => section.page.as_mut(),
                _ => None,
            })
            .unwrap();
        page_def.margin_left.0 += 1;
        assert_ne!(structural_semantic_sha256(&page).unwrap(), digest);

        let paragraph = &original.sections[0].paragraphs[1];
        let mut paragraph_shape = original.clone();
        paragraph_shape.header.para_shapes[paragraph.para_shape.0 as usize].margin_left += 1;
        assert_ne!(
            structural_semantic_sha256(&paragraph_shape).unwrap(),
            digest
        );

        let mut style = original.clone();
        style.header.styles[paragraph.style.0 as usize]
            .name
            .push_str(" changed");
        assert_ne!(structural_semantic_sha256(&style).unwrap(), digest);

        let run_shape_id = paragraph.char_shape_runs[0].1.0 as usize;
        let mut run = original.clone();
        run.header.char_shapes[run_shape_id].base_size += 1;
        assert_ne!(structural_semantic_sha256(&run).unwrap(), digest);

        let mut list = original.clone();
        let para_shape = &mut list.header.para_shapes[paragraph.para_shape.0 as usize];
        para_shape.attr1 = (para_shape.attr1 & !(0x3 << 23)) | (2 << 23);
        para_shape.numbering_id = 0;
        list.header
            .numbering_levels
            .push(vec![hwp_model::NumLevel::default()]);
        let list_digest = structural_semantic_sha256(&list).unwrap();
        assert_ne!(list_digest, digest);
        list.header.numbering_levels[0][0]
            .template
            .push_str(" changed");
        assert_ne!(structural_semantic_sha256(&list).unwrap(), list_digest);

        let mut explicit_break = original.clone();
        explicit_break.sections[0].paragraphs[1].header.break_type ^= 0x04;
        assert_ne!(structural_semantic_sha256(&explicit_break).unwrap(), digest);

        let mut implicit_hwp_section_marker = original.clone();
        implicit_hwp_section_marker.sections[0].paragraphs[0]
            .header
            .break_type |= 0x03;
        assert_eq!(
            structural_semantic_sha256(&implicit_hwp_section_marker).unwrap(),
            digest
        );
    }

    #[test]
    fn common_semantic_digest_detects_table_header_footer_and_page_number_mutations() {
        let original = compiled_official_letter();
        let digest = structural_semantic_sha256(&original).unwrap();

        let mut table_geometry = original.clone();
        let table = table_geometry.sections[0]
            .paragraphs
            .iter_mut()
            .flat_map(|paragraph| paragraph.controls.iter_mut())
            .find_map(|control| match control {
                hwp_model::Control::Table(table) => Some(table),
                _ => None,
            })
            .unwrap();
        table.cells[0].width.0 += 1;
        assert_ne!(structural_semantic_sha256(&table_geometry).unwrap(), digest);

        let mut placement = original.clone();
        let table = placement.sections[0]
            .paragraphs
            .iter_mut()
            .flat_map(|paragraph| paragraph.controls.iter_mut())
            .find_map(|control| match control {
                hwp_model::Control::Table(table) => Some(table),
                _ => None,
            })
            .unwrap();
        table.placement.as_mut().unwrap().width += 1;
        assert_ne!(structural_semantic_sha256(&placement).unwrap(), digest);

        let mut footer = original.clone();
        let footer_control = footer.sections[0]
            .paragraphs
            .iter_mut()
            .flat_map(|paragraph| paragraph.controls.iter_mut())
            .find_map(|control| match control {
                hwp_model::Control::Generic(generic) if generic.ctrl_id == *b"foot" => {
                    Some(generic)
                }
                _ => None,
            })
            .unwrap();
        footer_control.paragraph_lists[0].paragraphs[0]
            .chars
            .push(hwp_model::HwpChar::Text('!'));
        assert_ne!(structural_semantic_sha256(&footer).unwrap(), digest);

        let mut page_number = original.clone();
        let mut page_number_data = vec![0; 12];
        page_number_data[1] = 3;
        page_number.sections[0].paragraphs[0]
            .controls
            .push(generic_control(*b"pgnp", page_number_data, None));
        let page_number_digest = structural_semantic_sha256(&page_number).unwrap();
        assert_ne!(page_number_digest, digest);
        let hwp_model::Control::Generic(page_number_control) = page_number.sections[0].paragraphs
            [0]
        .controls
        .last_mut()
        .unwrap() else {
            unreachable!()
        };
        page_number_control.data[1] ^= 1;
        assert_ne!(
            structural_semantic_sha256(&page_number).unwrap(),
            page_number_digest
        );
    }

    #[test]
    fn common_semantic_digest_detects_field_equation_and_picture_mutations() {
        let original = compiled_official_letter();

        let mut first_link = original.clone();
        assert!(hwp_convert::create_hyperlink(
            &mut first_link,
            "제주한라대학교",
            "https://example.invalid/first",
            "링크"
        ));
        let mut second_link = original.clone();
        assert!(hwp_convert::create_hyperlink(
            &mut second_link,
            "제주한라대학교",
            "https://example.invalid/second",
            "링크"
        ));
        assert_ne!(
            structural_semantic_sha256(&first_link).unwrap(),
            structural_semantic_sha256(&second_link).unwrap()
        );

        let mut equation = original.clone();
        equation.sections[0].paragraphs[0]
            .controls
            .push(generic_control(
                *b"eqed",
                Vec::new(),
                Some(hwp_model::Equation {
                    script: "x+y".to_string(),
                    width: 1000,
                    height: 500,
                    inline: true,
                    x: 0,
                    y: 0,
                    ..Default::default()
                }),
            ));
        let equation_digest = structural_semantic_sha256(&equation).unwrap();
        let hwp_model::Control::Generic(equation_control) = equation.sections[0].paragraphs[0]
            .controls
            .last_mut()
            .unwrap()
        else {
            unreachable!()
        };
        equation_control.equation.as_mut().unwrap().script = "x-y".to_string();
        assert_ne!(
            structural_semantic_sha256(&equation).unwrap(),
            equation_digest
        );

        let mut picture = original.clone();
        picture.bin_streams.push(hwp_model::BinStream {
            name: "BinData/projection-image.png".to_string(),
            data: vec![1, 2, 3, 4],
        });
        picture.sections[0].paragraphs[0]
            .controls
            .push(hwp_model::Control::Picture(hwp_model::Picture {
                common_data: Vec::new(),
                width: hwp_model::HwpUnit(1000),
                height: hwp_model::HwpUnit(500),
                treat_as_char: true,
                z_order: 0,
                vert_offset: 0,
                horz_offset: 0,
                description: Some("projection image".to_string()),
                crop: None,
                flip: 0,
                rotation: None,
                brightness: 0,
                contrast: 0,
                effect_flags: 0,
                effects_raw: Vec::new(),
                caption: None,
                bin_ref: hwp_model::BinRef::ItemRef("projection-image".to_string()),
                extras: Vec::new(),
            }));
        let picture_digest = structural_semantic_sha256(&picture).unwrap();

        let mut picture_dimensions = picture.clone();
        let hwp_model::Control::Picture(picture_control) = picture_dimensions.sections[0]
            .paragraphs[0]
            .controls
            .last_mut()
            .unwrap()
        else {
            unreachable!()
        };
        picture_control.width.0 += 1;
        assert_ne!(
            structural_semantic_sha256(&picture_dimensions).unwrap(),
            picture_digest
        );

        let mut picture_metadata = picture.clone();
        let hwp_model::Control::Picture(picture_control) = picture_metadata.sections[0].paragraphs
            [0]
        .controls
        .last_mut()
        .unwrap() else {
            unreachable!()
        };
        picture_control.description = Some("changed".to_string());
        assert_ne!(
            structural_semantic_sha256(&picture_metadata).unwrap(),
            picture_digest
        );

        picture.bin_streams.last_mut().unwrap().data[0] ^= 0xff;
        assert_ne!(
            structural_semantic_sha256(&picture).unwrap(),
            picture_digest
        );
    }

    #[test]
    fn common_semantic_projection_enforces_node_and_byte_bounds() {
        let mut nodes = SemanticProjection::new();
        for _ in 0..MAX_SEMANTIC_NODES {
            nodes.node("bounded").unwrap();
        }
        assert!(nodes.node("overflow").is_err());

        let mut bytes = SemanticProjection::new();
        bytes.bytes = MAX_SEMANTIC_BYTES;
        assert!(bytes.field("overflow", b"x").is_err());
    }
}
