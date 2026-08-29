//! Typed preservation diagnostics shared by the HWP and HWPX writers.
//!
//! These types deliberately carry only bounded, content-free identifiers. Format
//! readers and writers may keep private hashes and entry names while comparing
//! containers, but public reports must expose only stable codes, resource classes,
//! dispositions, and aggregate counts.

use serde::{Deserialize, Serialize};

pub const PRESERVATION_REPORT_CONTRACT: &str = "hwp-preservation-report-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreservationCode {
    BinaryAssetRemoved,
    BinaryRelationshipRemoved,
    ControlMetadataUnrepresentable,
    ControlRemoved,
    /// A non-primary `hwp merge` input's document metadata differed from the
    /// primary's and was superseded (D-14).
    DocumentMetadataSuperseded,
    /// A non-primary `hwp merge` input's non-empty singular package field
    /// (`hwpx_settings_xml`, `hwp5_xml_template`, ...) was dropped — only the
    /// primary input's fields are carried (D-14).
    DocumentPackagePassthroughDropped,
    GsoHeaderUnrepresentable,
    /// One or more GSO (table/picture) object identities were renumbered to
    /// avoid a collision across `hwp merge` inputs — a non-target change, not
    /// data loss (D-14).
    GsoObjectIdRenumbered,
    GsoShapeUnrepresentable,
    HwpContainerStorageRemoved,
    HwpContainerStreamRemoved,
    HwpOpaqueStreamChanged,
    HwpxOpaqueEntryChanged,
    HwpxPackageEntryRemoved,
    MetadataValueRemoved,
    OpaqueControlUnrepresentable,
    /// A page-range split boundary that crossed a paragraph was rounded to
    /// the paragraph boundary (D-08) — an adjustment, not a loss: `hwp split`
    /// ledgers it for audit but excludes it from `--strict` refusal (#174).
    PageRangeParagraphRounded,
    PictureControlRemoved,
    /// An hwpx-origin `hwp merge` input's `<hp:pageBorderFill>` passthrough
    /// carried a `borderFillIDRef` that is present but not numeric, so the
    /// tier-2 graft could not shift it onto the merged border-fill table and
    /// passed it through verbatim — the reference now resolves against the
    /// wrong table (#180).
    SectionBorderFillRefUnresolvable,
}

impl PreservationCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BinaryAssetRemoved => "binary_asset_removed",
            Self::BinaryRelationshipRemoved => "binary_relationship_removed",
            Self::ControlMetadataUnrepresentable => "control_metadata_unrepresentable",
            Self::ControlRemoved => "control_removed",
            Self::DocumentMetadataSuperseded => "document_metadata_superseded",
            Self::DocumentPackagePassthroughDropped => "document_package_passthrough_dropped",
            Self::GsoHeaderUnrepresentable => "gso_header_unrepresentable",
            Self::GsoObjectIdRenumbered => "gso_object_id_renumbered",
            Self::GsoShapeUnrepresentable => "gso_shape_unrepresentable",
            Self::HwpContainerStorageRemoved => "hwp_container_storage_removed",
            Self::HwpContainerStreamRemoved => "hwp_container_stream_removed",
            Self::HwpOpaqueStreamChanged => "hwp_opaque_stream_changed",
            Self::HwpxOpaqueEntryChanged => "hwpx_opaque_entry_changed",
            Self::HwpxPackageEntryRemoved => "hwpx_package_entry_removed",
            Self::MetadataValueRemoved => "metadata_value_removed",
            Self::OpaqueControlUnrepresentable => "opaque_control_unrepresentable",
            Self::PageRangeParagraphRounded => "page_range_paragraph_rounded",
            Self::PictureControlRemoved => "picture_control_removed",
            Self::SectionBorderFillRefUnresolvable => "section_border_fill_ref_unresolvable",
        }
    }
}

impl std::fmt::Display for PreservationCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreservationResourceKind {
    ContainerStream,
    ContainerStorage,
    PackageEntry,
    Control,
    BinaryAsset,
    Relationship,
    Metadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreservationDisposition {
    Removed,
    ChangedNonTarget,
    Unrepresentable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreservationEvent {
    pub code: PreservationCode,
    pub resource: PreservationResourceKind,
    pub disposition: PreservationDisposition,
    pub count: usize,
}

impl PreservationEvent {
    pub fn new(
        code: PreservationCode,
        resource: PreservationResourceKind,
        disposition: PreservationDisposition,
        count: usize,
    ) -> Self {
        Self {
            code,
            resource,
            disposition,
            count: count.max(1),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreservationReport {
    #[serde(default = "contract")]
    pub contract: String,
    #[serde(default)]
    pub events: Vec<PreservationEvent>,
}

fn contract() -> String {
    PRESERVATION_REPORT_CONTRACT.to_string()
}

impl PreservationReport {
    pub fn new() -> Self {
        Self {
            contract: contract(),
            events: Vec::new(),
        }
    }

    pub fn record(&mut self, event: PreservationEvent) {
        if let Some(existing) = self.events.iter_mut().find(|existing| {
            existing.code == event.code
                && existing.resource == event.resource
                && existing.disposition == event.disposition
        }) {
            existing.count = existing.count.saturating_add(event.count);
        } else {
            self.events.push(event);
        }
        self.events.sort_by(|left, right| {
            (&left.code, left.resource, left.disposition).cmp(&(
                &right.code,
                right.resource,
                right.disposition,
            ))
        });
    }

    pub fn extend(&mut self, other: Self) {
        for event in other.events {
            self.record(event);
        }
    }

    pub fn is_lossless(&self) -> bool {
        self.events.is_empty()
    }

    pub fn status(&self) -> &'static str {
        if self.is_lossless() {
            "clean"
        } else {
            "loss_detected"
        }
    }
}

impl Default for PreservationReport {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteReport {
    pub warnings: Vec<String>,
    pub preservation: PreservationReport,
}

impl WriteReport {
    pub fn new() -> Self {
        Self {
            warnings: Vec::new(),
            preservation: PreservationReport::new(),
        }
    }

    pub fn warning(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }

    pub fn loss(
        &mut self,
        code: PreservationCode,
        resource: PreservationResourceKind,
        disposition: PreservationDisposition,
        count: usize,
    ) {
        self.preservation
            .record(PreservationEvent::new(code, resource, disposition, count));
    }

    /// Compatibility rendering for callers that still consume the legacy list.
    pub fn into_legacy_warnings(mut self) -> Vec<String> {
        self.warnings
            .extend(self.preservation.events.iter().map(|event| {
                format!(
                    "DROP: {} ({:?}, {} item(s))",
                    event.code, event.resource, event.count
                )
            }));
        self.warnings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Every `PreservationCode` variant, exhaustively. The trailing `match`
    /// has no wildcard arm on purpose: adding a variant without extending
    /// this list is a compile error, so the schema-locking test below cannot
    /// silently miss a new code (#179).
    fn every_preservation_code() -> Vec<PreservationCode> {
        use PreservationCode::*;
        let all = [
            BinaryAssetRemoved,
            BinaryRelationshipRemoved,
            ControlMetadataUnrepresentable,
            ControlRemoved,
            DocumentMetadataSuperseded,
            DocumentPackagePassthroughDropped,
            GsoHeaderUnrepresentable,
            GsoObjectIdRenumbered,
            GsoShapeUnrepresentable,
            HwpContainerStorageRemoved,
            HwpContainerStreamRemoved,
            HwpOpaqueStreamChanged,
            HwpxOpaqueEntryChanged,
            HwpxPackageEntryRemoved,
            MetadataValueRemoved,
            OpaqueControlUnrepresentable,
            PageRangeParagraphRounded,
            PictureControlRemoved,
            SectionBorderFillRefUnresolvable,
        ];
        for code in all {
            match code {
                BinaryAssetRemoved
                | BinaryRelationshipRemoved
                | ControlMetadataUnrepresentable
                | ControlRemoved
                | DocumentMetadataSuperseded
                | DocumentPackagePassthroughDropped
                | GsoHeaderUnrepresentable
                | GsoObjectIdRenumbered
                | GsoShapeUnrepresentable
                | HwpContainerStorageRemoved
                | HwpContainerStreamRemoved
                | HwpOpaqueStreamChanged
                | HwpxOpaqueEntryChanged
                | HwpxPackageEntryRemoved
                | MetadataValueRemoved
                | OpaqueControlUnrepresentable
                | PageRangeParagraphRounded
                | PictureControlRemoved
                | SectionBorderFillRefUnresolvable => {}
            }
        }
        all.to_vec()
    }

    /// #179: the published schema's `code` enum must list exactly the wire
    /// strings of every `PreservationCode`. Adding a variant without updating
    /// `schemas/preservation-report-v1.schema.json` fails this test, and the
    /// exhaustive list above fails to compile until the variant is named.
    #[test]
    fn schema_code_enum_matches_every_preservation_code_wire_string() {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/preservation-report-v1.schema.json"
        ))
        .unwrap();
        let schema_codes: Vec<&str> = schema["$defs"]["event"]["properties"]["code"]["enum"]
            .as_array()
            .expect("schema $defs/event code enum")
            .iter()
            .map(|value| value.as_str().expect("string enum entry"))
            .collect();
        assert!(
            schema_codes.windows(2).all(|pair| pair[0] < pair[1]),
            "schema code enum must stay sorted: {schema_codes:?}"
        );
        let schema_set: BTreeSet<&str> = schema_codes.iter().copied().collect();
        let wire_strings: BTreeSet<&str> = every_preservation_code()
            .iter()
            .map(|code| code.as_str())
            .collect();
        assert_eq!(
            schema_set, wire_strings,
            "schemas/preservation-report-v1.schema.json code enum drifted from PreservationCode"
        );
    }

    /// Every variant's serde wire string equals its `as_str()` — the two
    /// spellings can never drift apart.
    #[test]
    fn serde_wire_string_matches_as_str_for_every_code() {
        for code in every_preservation_code() {
            assert_eq!(
                serde_json::to_string(&code).unwrap(),
                format!("\"{}\"", code.as_str()),
            );
        }
    }

    /// D-14: every pre-existing code's wire string is a published contract —
    /// this plan's four new variants must be additive only. Locks each
    /// pre-existing `as_str` value so a future edit cannot silently rename one.
    #[test]
    fn pre_existing_codes_keep_their_wire_string() {
        assert_eq!(
            PreservationCode::BinaryAssetRemoved.as_str(),
            "binary_asset_removed"
        );
        assert_eq!(
            PreservationCode::BinaryRelationshipRemoved.as_str(),
            "binary_relationship_removed"
        );
        assert_eq!(
            PreservationCode::ControlMetadataUnrepresentable.as_str(),
            "control_metadata_unrepresentable"
        );
        assert_eq!(PreservationCode::ControlRemoved.as_str(), "control_removed");
        assert_eq!(
            PreservationCode::GsoHeaderUnrepresentable.as_str(),
            "gso_header_unrepresentable"
        );
        assert_eq!(
            PreservationCode::GsoShapeUnrepresentable.as_str(),
            "gso_shape_unrepresentable"
        );
        assert_eq!(
            PreservationCode::HwpContainerStorageRemoved.as_str(),
            "hwp_container_storage_removed"
        );
        assert_eq!(
            PreservationCode::HwpContainerStreamRemoved.as_str(),
            "hwp_container_stream_removed"
        );
        assert_eq!(
            PreservationCode::HwpOpaqueStreamChanged.as_str(),
            "hwp_opaque_stream_changed"
        );
        assert_eq!(
            PreservationCode::HwpxOpaqueEntryChanged.as_str(),
            "hwpx_opaque_entry_changed"
        );
        assert_eq!(
            PreservationCode::HwpxPackageEntryRemoved.as_str(),
            "hwpx_package_entry_removed"
        );
        assert_eq!(
            PreservationCode::MetadataValueRemoved.as_str(),
            "metadata_value_removed"
        );
        assert_eq!(
            PreservationCode::OpaqueControlUnrepresentable.as_str(),
            "opaque_control_unrepresentable"
        );
        assert_eq!(
            PreservationCode::PictureControlRemoved.as_str(),
            "picture_control_removed"
        );
    }

    /// The four new D-14 variants and their snake_case wire strings.
    #[test]
    fn new_document_level_codes_have_the_expected_wire_strings() {
        assert_eq!(
            PreservationCode::DocumentMetadataSuperseded.as_str(),
            "document_metadata_superseded"
        );
        assert_eq!(
            PreservationCode::DocumentPackagePassthroughDropped.as_str(),
            "document_package_passthrough_dropped"
        );
        assert_eq!(
            PreservationCode::GsoObjectIdRenumbered.as_str(),
            "gso_object_id_renumbered"
        );
        assert_eq!(
            PreservationCode::PageRangeParagraphRounded.as_str(),
            "page_range_paragraph_rounded"
        );
    }

    /// The #180 variant and its snake_case wire string.
    #[test]
    fn section_border_fill_ref_unresolvable_has_the_expected_wire_string() {
        assert_eq!(
            PreservationCode::SectionBorderFillRefUnresolvable.as_str(),
            "section_border_fill_ref_unresolvable"
        );
        assert_eq!(
            serde_json::to_string(&PreservationCode::SectionBorderFillRefUnresolvable).unwrap(),
            "\"section_border_fill_ref_unresolvable\""
        );
    }

    #[test]
    fn events_are_aggregated_and_sorted_without_payloads() {
        let mut report = PreservationReport::new();
        report.record(PreservationEvent::new(
            PreservationCode::OpaqueControlUnrepresentable,
            PreservationResourceKind::Control,
            PreservationDisposition::Unrepresentable,
            1,
        ));
        report.record(PreservationEvent::new(
            PreservationCode::BinaryAssetRemoved,
            PreservationResourceKind::BinaryAsset,
            PreservationDisposition::Removed,
            2,
        ));
        report.record(PreservationEvent::new(
            PreservationCode::BinaryAssetRemoved,
            PreservationResourceKind::BinaryAsset,
            PreservationDisposition::Removed,
            3,
        ));

        assert_eq!(report.status(), "loss_detected");
        assert_eq!(report.events.len(), 2);
        assert_eq!(report.events[0].code, PreservationCode::BinaryAssetRemoved);
        assert_eq!(report.events[0].count, 5);
        assert!(
            report
                .events
                .iter()
                .all(|event| !event.code.as_str().contains('/'))
        );
        assert!(
            report
                .events
                .iter()
                .all(|event| !event.code.as_str().contains("sha256"))
        );
        assert_eq!(
            PreservationReport::default().contract,
            PRESERVATION_REPORT_CONTRACT
        );
    }
}
