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
    GsoHeaderUnrepresentable,
    GsoShapeUnrepresentable,
    HwpContainerStorageRemoved,
    HwpContainerStreamRemoved,
    HwpOpaqueStreamChanged,
    HwpxOpaqueEntryChanged,
    HwpxPackageEntryRemoved,
    MetadataValueRemoved,
    OpaqueControlUnrepresentable,
    PictureControlRemoved,
}

impl PreservationCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BinaryAssetRemoved => "binary_asset_removed",
            Self::BinaryRelationshipRemoved => "binary_relationship_removed",
            Self::ControlMetadataUnrepresentable => "control_metadata_unrepresentable",
            Self::ControlRemoved => "control_removed",
            Self::GsoHeaderUnrepresentable => "gso_header_unrepresentable",
            Self::GsoShapeUnrepresentable => "gso_shape_unrepresentable",
            Self::HwpContainerStorageRemoved => "hwp_container_storage_removed",
            Self::HwpContainerStreamRemoved => "hwp_container_stream_removed",
            Self::HwpOpaqueStreamChanged => "hwp_opaque_stream_changed",
            Self::HwpxOpaqueEntryChanged => "hwpx_opaque_entry_changed",
            Self::HwpxPackageEntryRemoved => "hwpx_package_entry_removed",
            Self::MetadataValueRemoved => "metadata_value_removed",
            Self::OpaqueControlUnrepresentable => "opaque_control_unrepresentable",
            Self::PictureControlRemoved => "picture_control_removed",
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
