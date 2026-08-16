//! 렌더 진단의 고정 코드, 심각도, 단계 및 source-bounded 집계기.
//!
//! 문서 텍스트나 경로를 보관하지 않는다. 호출자가 제공한 detail은 즉시 SHA-256으로
//! 바뀌며, 코드별로 제한된 수의 sample hash만 남는다.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use sha2::{Digest as _, Sha256};

pub const MAX_RECORDED_ISSUES: u64 = 1_000_000;
pub const MAX_RECORDED_INFO: u64 = 1_000_000;
pub const MAX_SAMPLES_PER_CODE: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RenderIssueSeverity {
    Info,
    Warning,
    Incomplete,
    Fatal,
}

impl RenderIssueSeverity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Incomplete => "incomplete",
            Self::Fatal => "fatal",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RenderIssueStage {
    InputParse,
    FontResolution,
    Layout,
    Shaping,
    Rasterization,
    PdfExport,
}

impl RenderIssueStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InputParse => "input_parse",
            Self::FontResolution => "font_resolution",
            Self::Layout => "layout",
            Self::Shaping => "shaping",
            Self::Rasterization => "rasterization",
            Self::PdfExport => "pdf_export",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RenderIssueCode {
    ParseBudgetExceeded,
    RenderExecutionFailed,
    PaginationDriftDetected,
    FontMatched,
    FontSubstituted,
    FontMissing,
    FontManifestLoadFailed,
    FontResolutionBudgetExceeded,
    ShapingFailed,
    PageDefinitionFallback,
    PageControlPayloadOmitted,
    PageNumberFormatFallback,
    PageNumberPositionOmitted,
    PageNumberShapingOmitted,
    UnsupportedControlOmitted,
    ImageSizeMissingOmitted,
    ImageDataMissingOmitted,
    ImageDecodePlaceholder,
    ImageDecodeBudgetExceeded,
    /// Picture effects such as shadow, glow, and reflection (tables 108-116)
    /// are parsed and reported but not rendered.
    PictureEffectsUnsupported,
    InvalidTableCellOmitted,
    TableSplitAcrossPages,
    TableRowTooTallClipped,
    /// A CELL-policy row needed an internal page boundary, but the source did
    /// not provide a usable cached line layout to locate one.
    TableCellFragmentationIncomplete,
    /// Measured cell content is taller than the row height the document stores.
    /// The row keeps Hancom's stored height so the grid below it does not move,
    /// and the content is still drawn, so this is a geometry deviation rather
    /// than lost content.
    TableCellContentOverflow,
    TextBoxGeometryInvalidOmitted,
    ShapeDepthLimitOmitted,
    ShapeStyleInvalidOmitted,
    ShapeGeometryInvalidOmitted,
    FontSubsetFallback,
    LayoutBudgetExceeded,
    /// WMF 스트림이 손상/절단되어 해석을 중단하고 자홍색 placeholder로 대체.
    WmfParseInvalidPlaceholder,
    /// WMF 부분집합 밖 레코드 계열을 bounded-skip (detail은 함수 코드뿐).
    WmfUnsupportedRecordOmitted,
    /// WMF 해석의 레코드/객체/픽셀 상한 초과 → placeholder.
    WmfBudgetExceeded,
}

impl RenderIssueCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ParseBudgetExceeded => "parse_budget_exceeded",
            Self::RenderExecutionFailed => "render_execution_failed",
            Self::PaginationDriftDetected => "pagination_drift_detected",
            Self::FontMatched => "font_matched",
            Self::FontSubstituted => "font_substituted",
            Self::FontMissing => "font_missing",
            Self::FontManifestLoadFailed => "font_manifest_load_failed",
            Self::FontResolutionBudgetExceeded => "font_resolution_budget_exceeded",
            Self::ShapingFailed => "shaping_failed",
            Self::PageDefinitionFallback => "page_definition_fallback",
            Self::PageControlPayloadOmitted => "page_control_payload_omitted",
            Self::PageNumberFormatFallback => "page_number_format_fallback",
            Self::PageNumberPositionOmitted => "page_number_position_omitted",
            Self::PageNumberShapingOmitted => "page_number_shaping_omitted",
            Self::UnsupportedControlOmitted => "unsupported_control_omitted",
            Self::ImageSizeMissingOmitted => "image_size_missing_omitted",
            Self::ImageDataMissingOmitted => "image_data_missing_omitted",
            Self::ImageDecodePlaceholder => "image_decode_placeholder",
            Self::ImageDecodeBudgetExceeded => "image_decode_budget_exceeded",
            Self::PictureEffectsUnsupported => "picture_effects_unsupported",
            Self::InvalidTableCellOmitted => "invalid_table_cell_omitted",
            Self::TableSplitAcrossPages => "table_split_across_pages",
            Self::TableRowTooTallClipped => "table_row_too_tall_clipped",
            Self::TableCellFragmentationIncomplete => "table_cell_fragmentation_incomplete",
            Self::TableCellContentOverflow => "table_cell_content_overflow",
            Self::TextBoxGeometryInvalidOmitted => "text_box_geometry_invalid_omitted",
            Self::ShapeDepthLimitOmitted => "shape_depth_limit_omitted",
            Self::ShapeStyleInvalidOmitted => "shape_style_invalid_omitted",
            Self::ShapeGeometryInvalidOmitted => "shape_geometry_invalid_omitted",
            Self::FontSubsetFallback => "font_subset_fallback",
            Self::LayoutBudgetExceeded => "layout_budget_exceeded",
            Self::WmfParseInvalidPlaceholder => "wmf_parse_invalid_placeholder",
            Self::WmfUnsupportedRecordOmitted => "wmf_unsupported_record_omitted",
            Self::WmfBudgetExceeded => "wmf_budget_exceeded",
        }
    }

    pub const fn severity(self) -> RenderIssueSeverity {
        match self {
            Self::FontMatched | Self::TableSplitAcrossPages => RenderIssueSeverity::Info,
            Self::FontSubstituted
            | Self::FontSubsetFallback
            | Self::PictureEffectsUnsupported
            | Self::TableCellContentOverflow => RenderIssueSeverity::Warning,
            Self::ParseBudgetExceeded
            | Self::RenderExecutionFailed
            | Self::PaginationDriftDetected
            | Self::FontManifestLoadFailed
            | Self::FontResolutionBudgetExceeded
            | Self::LayoutBudgetExceeded
            | Self::ImageDecodeBudgetExceeded
            | Self::WmfBudgetExceeded => RenderIssueSeverity::Fatal,
            _ => RenderIssueSeverity::Incomplete,
        }
    }

    pub const fn stage(self) -> RenderIssueStage {
        match self {
            Self::ParseBudgetExceeded => RenderIssueStage::InputParse,
            Self::RenderExecutionFailed | Self::PaginationDriftDetected => RenderIssueStage::Layout,
            Self::FontMatched
            | Self::FontSubstituted
            | Self::FontMissing
            | Self::FontManifestLoadFailed
            | Self::FontResolutionBudgetExceeded => RenderIssueStage::FontResolution,
            Self::ShapingFailed | Self::PageNumberShapingOmitted => RenderIssueStage::Shaping,
            Self::ImageDecodePlaceholder | Self::ImageDecodeBudgetExceeded => {
                RenderIssueStage::Rasterization
            }
            Self::FontSubsetFallback => RenderIssueStage::PdfExport,
            _ => RenderIssueStage::Layout,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderIssueSummary {
    pub code: RenderIssueCode,
    pub severity: RenderIssueSeverity,
    pub stage: RenderIssueStage,
    pub count: u64,
    pub sample_sha256: Vec<String>,
    pub samples_complete: bool,
}

impl fmt::Display for RenderIssueSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}/{}/{} count={} samples_complete={}",
            self.stage.as_str(),
            self.severity.as_str(),
            self.code.as_str(),
            self.count,
            self.samples_complete
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderIssueReport {
    /// warning/incomplete/fatal channel.
    pub issues: Vec<RenderIssueSummary>,
    /// Informational events such as successful font matches and page splits.
    pub info: Vec<RenderIssueSummary>,
    pub issue_count: u64,
    pub info_count: u64,
    pub complete: bool,
    pub sha256: String,
}

/// Aggregated font-resolution outcomes for one render. Successful matches come
/// from the info channel; all other outcomes come from the issue channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FontCoverage {
    /// Requests resolved to the requested font.
    pub matched: u64,
    /// Requests resolved through substitution.
    pub substituted: u64,
    /// Requests with no resolvable font.
    pub missing: u64,
    /// Fonts embedded in full because subsetting failed.
    pub subset_fallback: u64,
}

impl FontCoverage {
    /// Gate parity publication on zero substitutions, misses, and subset
    /// fallbacks because each changes metrics relevant to layout or glyphs.
    pub const fn substitution_free(&self) -> bool {
        self.substituted == 0 && self.missing == 0 && self.subset_fallback == 0
    }
}

impl RenderIssueReport {
    pub fn has_required_failure(&self) -> bool {
        !self.complete
            || self.issues.iter().any(|issue| {
                matches!(
                    issue.severity,
                    RenderIssueSeverity::Incomplete | RenderIssueSeverity::Fatal
                )
            })
    }

    /// Aggregate font-resolution entries into parity coverage counters.
    pub fn font_coverage(&self) -> FontCoverage {
        let mut coverage = FontCoverage::default();
        for summary in self.info.iter().chain(&self.issues) {
            match summary.code {
                RenderIssueCode::FontMatched => coverage.matched += summary.count,
                RenderIssueCode::FontSubstituted => coverage.substituted += summary.count,
                RenderIssueCode::FontMissing => coverage.missing += summary.count,
                RenderIssueCode::FontSubsetFallback => coverage.subset_fallback += summary.count,
                _ => {}
            }
        }
        coverage
    }
}

struct Bucket {
    count: u64,
    sample_sha256: Vec<String>,
    samples_complete: bool,
}

pub struct RenderIssueAccumulator {
    issues: BTreeMap<RenderIssueCode, Bucket>,
    info: BTreeMap<RenderIssueCode, Bucket>,
    issue_count: u64,
    info_count: u64,
    complete: bool,
    binary_cache: BTreeMap<(usize, usize), Arc<Vec<u8>>>,
    display_item_limit: Option<usize>,
    display_item_count: usize,
    display_item_budget_exceeded: bool,
    page_limit: Option<usize>,
    page_count: usize,
    page_budget_exceeded: bool,
}

impl Default for RenderIssueAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderIssueAccumulator {
    pub fn new() -> Self {
        Self {
            issues: BTreeMap::new(),
            info: BTreeMap::new(),
            issue_count: 0,
            info_count: 0,
            complete: true,
            binary_cache: BTreeMap::new(),
            display_item_limit: None,
            display_item_count: 0,
            display_item_budget_exceeded: false,
            page_limit: None,
            page_count: 0,
            page_budget_exceeded: false,
        }
    }

    pub fn push(&mut self, code: RenderIssueCode, detail: impl AsRef<[u8]>) {
        let detail = detail.as_ref();
        let detail_hash = hex_digest(Sha256::digest(detail).as_slice());
        let info = code.severity() == RenderIssueSeverity::Info;
        let (total, limit, buckets) = if info {
            (&mut self.info_count, MAX_RECORDED_INFO, &mut self.info)
        } else {
            (&mut self.issue_count, MAX_RECORDED_ISSUES, &mut self.issues)
        };
        if *total >= limit {
            self.complete = false;
            return;
        }
        *total += 1;
        let bucket = buckets.entry(code).or_insert_with(|| Bucket {
            count: 0,
            sample_sha256: Vec::new(),
            samples_complete: true,
        });
        bucket.count += 1;
        if !bucket.sample_sha256.contains(&detail_hash) {
            if bucket.sample_sha256.len() < MAX_SAMPLES_PER_CODE {
                bucket.sample_sha256.push(detail_hash);
                bucket.sample_sha256.sort();
            } else {
                bucket.samples_complete = false;
            }
        }
    }

    /// 같은 문서 바이너리 slice는 렌더 전체에서 한 번만 복사하고 모든 이미지 Item이
    /// 같은 Arc를 공유한다. 인증 경로의 byte quota는 layout preflight가 적용한다.
    pub fn cached_binary(&mut self, bytes: &[u8]) -> Arc<Vec<u8>> {
        let identity = (bytes.as_ptr() as usize, bytes.len());
        self.binary_cache
            .entry(identity)
            .or_insert_with(|| Arc::new(bytes.to_vec()))
            .clone()
    }

    pub fn set_display_item_limit(&mut self, limit: usize) {
        self.display_item_limit = Some(limit);
    }

    /// Display Item을 Vec에 넣기 전에 호출한다. false면 caller는 push/insert를 하지
    /// 않아야 하며 bounded layout은 즉시 실패 상태로 전환된다.
    pub fn charge_display_items(&mut self, count: usize) -> bool {
        let Some(next) = self.display_item_count.checked_add(count) else {
            self.display_item_budget_exceeded = true;
            self.push_once(RenderIssueCode::LayoutBudgetExceeded, b"display_items");
            return false;
        };
        if self.display_item_limit.is_some_and(|limit| next > limit) {
            self.display_item_budget_exceeded = true;
            self.push_once(RenderIssueCode::LayoutBudgetExceeded, b"display_items");
            return false;
        }
        self.display_item_count = next;
        true
    }

    pub fn display_item_budget_exceeded(&self) -> bool {
        self.display_item_budget_exceeded
    }

    pub fn set_page_limit(&mut self, limit: usize) {
        self.page_limit = Some(limit);
    }

    /// Reserve a page slot before replacing/allocating the next PageList.
    pub fn charge_page(&mut self) -> bool {
        let Some(next) = self.page_count.checked_add(1) else {
            self.page_budget_exceeded = true;
            self.push_once(RenderIssueCode::LayoutBudgetExceeded, b"pages");
            return false;
        };
        if self.page_limit.is_some_and(|limit| next > limit) {
            self.page_budget_exceeded = true;
            self.push_once(RenderIssueCode::LayoutBudgetExceeded, b"pages");
            return false;
        }
        self.page_count = next;
        true
    }

    pub fn page_budget_exceeded(&self) -> bool {
        self.page_budget_exceeded
    }

    pub fn push_once(&mut self, code: RenderIssueCode, detail: impl AsRef<[u8]>) {
        let detail = detail.as_ref();
        let detail_hash = hex_digest(Sha256::digest(detail).as_slice());
        let buckets = if code.severity() == RenderIssueSeverity::Info {
            &self.info
        } else {
            &self.issues
        };
        if buckets
            .get(&code)
            .is_some_and(|bucket| bucket.sample_sha256.contains(&detail_hash))
        {
            return;
        }
        self.push(code, detail);
    }

    pub fn absorb(&mut self, report: RenderIssueReport) {
        self.complete &= report.complete;
        for summary in report.info.into_iter().chain(report.issues) {
            let target = if summary.severity == RenderIssueSeverity::Info {
                &mut self.info
            } else {
                &mut self.issues
            };
            let bucket = target.entry(summary.code).or_insert_with(|| Bucket {
                count: 0,
                sample_sha256: Vec::new(),
                samples_complete: true,
            });
            let channel_count = if summary.severity == RenderIssueSeverity::Info {
                &mut self.info_count
            } else {
                &mut self.issue_count
            };
            let limit = if summary.severity == RenderIssueSeverity::Info {
                MAX_RECORDED_INFO
            } else {
                MAX_RECORDED_ISSUES
            };
            let available = limit.saturating_sub(*channel_count);
            let accepted = summary.count.min(available);
            if accepted != summary.count {
                self.complete = false;
            }
            bucket.count = bucket.count.saturating_add(accepted);
            *channel_count = channel_count.saturating_add(accepted);
            bucket.samples_complete &= summary.samples_complete;
            for sample in summary.sample_sha256 {
                if !bucket.sample_sha256.contains(&sample) {
                    if bucket.sample_sha256.len() < MAX_SAMPLES_PER_CODE {
                        bucket.sample_sha256.push(sample);
                        bucket.sample_sha256.sort();
                    } else {
                        bucket.samples_complete = false;
                    }
                }
            }
        }
    }

    pub fn finish(self) -> RenderIssueReport {
        let convert = |(code, bucket): (RenderIssueCode, Bucket)| RenderIssueSummary {
            code,
            severity: code.severity(),
            stage: code.stage(),
            count: bucket.count,
            sample_sha256: bucket.sample_sha256,
            samples_complete: bucket.samples_complete,
        };
        let issues: Vec<_> = self.issues.into_iter().map(convert).collect();
        let info: Vec<_> = self.info.into_iter().map(convert).collect();
        let issue_count = issues.iter().map(|issue| issue.count).sum();
        let info_count = info.iter().map(|issue| issue.count).sum();
        debug_assert_eq!(issue_count, self.issue_count);
        debug_assert_eq!(info_count, self.info_count);
        let sha256 = canonical_issue_sha256(&issues);
        RenderIssueReport {
            issues,
            info,
            issue_count,
            info_count,
            complete: self.complete,
            sha256,
        }
    }
}

/// Hash the exact canonical typed issue list represented in a report. Informational
/// entries are intentionally excluded from `issue_sha256` and have their own count.
pub fn canonical_issue_sha256(issues: &[RenderIssueSummary]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"hwp-render-typed-issues-v1\0");
    for issue in issues {
        hash_field(&mut hasher, issue.code.as_str().as_bytes());
        hash_field(&mut hasher, issue.severity.as_str().as_bytes());
        hash_field(&mut hasher, issue.stage.as_str().as_bytes());
        hasher.update(issue.count.to_le_bytes());
        hasher.update([u8::from(issue.samples_complete)]);
        hasher.update((issue.sample_sha256.len() as u64).to_le_bytes());
        for sample in &issue.sample_sha256 {
            hash_field(&mut hasher, sample.as_bytes());
        }
    }
    hex_digest(hasher.finalize().as_slice())
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value);
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_are_hashed_and_info_is_separate() {
        let mut issues = RenderIssueAccumulator::new();
        issues.push(RenderIssueCode::FontMatched, "secret-font");
        issues.push(RenderIssueCode::ShapingFailed, "secret-text");
        let report = issues.finish();
        assert_eq!(report.info_count, 1);
        assert_eq!(report.issue_count, 1);
        let debug = format!("{report:?}");
        assert!(!debug.contains("secret"));
        assert!(report.has_required_failure());
    }

    #[test]
    fn empty_typed_issue_hash_is_a_frozen_domain_separated_value() {
        assert_eq!(
            canonical_issue_sha256(&[]),
            "7ae3724fbab92218a9d2bf86fca465264e88ca44311bb2d07e4f48083fadaceb"
        );
    }

    #[test]
    fn font_coverage_aggregates_resolution_codes_and_gates_parity() {
        let mut issues = RenderIssueAccumulator::new();
        issues.push(RenderIssueCode::FontMatched, "font-a");
        issues.push(RenderIssueCode::FontMatched, "font-a");
        issues.push(RenderIssueCode::FontMatched, "font-b");
        issues.push(RenderIssueCode::FontSubstituted, "font-c");
        issues.push(RenderIssueCode::FontMissing, "font-d");
        issues.push(RenderIssueCode::FontSubsetFallback, "font-e");
        issues.push(RenderIssueCode::ShapingFailed, "not-a-font-code");
        let coverage = issues.finish().font_coverage();
        assert_eq!(coverage.matched, 3);
        assert_eq!(coverage.substituted, 1);
        assert_eq!(coverage.missing, 1);
        assert_eq!(coverage.subset_fallback, 1);
        assert!(!coverage.substitution_free());

        let mut clean = RenderIssueAccumulator::new();
        clean.push(RenderIssueCode::FontMatched, "font-a");
        let coverage = clean.finish().font_coverage();
        assert_eq!(coverage.matched, 1);
        assert!(coverage.substitution_free());
    }
}
