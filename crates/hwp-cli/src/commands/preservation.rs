//! Content-free preservation accounting for native document publication.
//!
//! Paths, package entry names, document text, and payload hashes are used only
//! inside the comparison. Public reports contain stable codes and aggregate
//! counts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use hwp_model::{
    Control, Document, Paragraph, PreservationCode, PreservationDisposition, PreservationEvent,
    PreservationReport, PreservationResourceKind,
};
use sha2::{Digest as _, Sha256};

use crate::format::FileFormat;

/// Compare native containers after a same-format write.
pub(crate) fn inspect_same_format_container(
    source: &Path,
    output: &Path,
) -> anyhow::Result<PreservationReport> {
    inspect_same_format_container_with_binary_allowance(source, output, 0)
}

/// Compare native containers while allowing only the exact number of binary
/// entries deliberately removed by the requested edit.
pub(crate) fn inspect_same_format_container_with_binary_allowance(
    source: &Path,
    output: &Path,
    removed_binary_allowance: usize,
) -> anyhow::Result<PreservationReport> {
    let source_format = crate::format::detect(source)?;
    let output_format = crate::format::detect(output)?;
    if source_format != output_format {
        return Ok(PreservationReport::new());
    }
    match source_format {
        FileFormat::Hwp5 => inspect_hwp_container(source, output, removed_binary_allowance),
        FileFormat::Hwpx => inspect_hwpx_package(source, output, removed_binary_allowance),
    }
}

pub(crate) fn intentional_removed_binary_assets(source: &Document, edited: &Document) -> usize {
    removed_multiset_count(
        resolved_picture_asset_multiset(source),
        resolved_picture_asset_multiset(edited),
    )
}

/// Compare semantic resources for conversion, irrespective of container format.
pub(crate) fn inspect_conversion_semantics(
    source: &Document,
    output: &Document,
) -> PreservationReport {
    let mut report = PreservationReport::new();

    let removed_assets = removed_multiset_count(binary_multiset(source), binary_multiset(output));
    record_removed(
        &mut report,
        PreservationCode::BinaryAssetRemoved,
        PreservationResourceKind::BinaryAsset,
        removed_assets,
    );

    let source_controls = control_multiset(source);
    let output_controls = control_multiset(output);
    let removed_controls = removed_multiset_count(source_controls, output_controls);
    record_removed(
        &mut report,
        PreservationCode::ControlRemoved,
        PreservationResourceKind::Control,
        removed_controls,
    );

    let removed_relationships = resolved_picture_relationships(source)
        .saturating_sub(resolved_picture_relationships(output));
    record_removed(
        &mut report,
        PreservationCode::BinaryRelationshipRemoved,
        PreservationResourceKind::Relationship,
        removed_relationships,
    );

    let removed_metadata = metadata_values(source)
        .into_iter()
        .zip(metadata_values(output))
        .filter(|(source, output)| source.is_some() && source != output)
        .count()
        + usize::from(
            source.metadata.create_time.is_some()
                && source.metadata.create_time != output.metadata.create_time,
        )
        + usize::from(
            source.metadata.modify_time.is_some()
                && source.metadata.modify_time != output.metadata.modify_time,
        );
    record_removed(
        &mut report,
        PreservationCode::MetadataValueRemoved,
        PreservationResourceKind::Metadata,
        removed_metadata,
    );

    report
}

pub(crate) fn reject_loss(context: &str, report: &PreservationReport) -> anyhow::Result<()> {
    if report.is_lossless() {
        return Ok(());
    }
    anyhow::bail!(
        "{context}: 보존 불가 데이터 {}건을 제거하거나 변경할 수 없어 출력을 게시하지 않습니다\n{}",
        report.events.iter().map(|event| event.count).sum::<usize>(),
        report
            .events
            .iter()
            .map(|event| format!("  - {}: {} item(s)", event.code, event.count))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

pub(crate) fn print_report(report: &PreservationReport) {
    for event in &report.events {
        eprintln!(
            "보존 경고: {} ({:?}, {:?}, {} item(s))",
            event.code, event.resource, event.disposition, event.count
        );
    }
}

fn inspect_hwp_container(
    source: &Path,
    output: &Path,
    removed_binary_allowance: usize,
) -> anyhow::Result<PreservationReport> {
    let mut source_container = hwp5::Hwp5Container::open(source)?;
    let mut output_container = hwp5::Hwp5Container::open(output)?;
    let source_streams = source_container
        .list_streams()
        .into_iter()
        .map(|entry| entry.path)
        .collect::<BTreeSet<_>>();
    let output_streams = output_container
        .list_streams()
        .into_iter()
        .map(|entry| entry.path)
        .collect::<BTreeSet<_>>();
    let source_storages = source_container
        .list_storages()
        .into_iter()
        .collect::<BTreeSet<_>>();
    let output_storages = output_container
        .list_storages()
        .into_iter()
        .collect::<BTreeSet<_>>();

    let mut report = PreservationReport::new();
    let removed_streams = source_streams.difference(&output_streams);
    let (removed_binary_streams, removed_other_streams): (Vec<_>, Vec<_>) =
        removed_streams.partition(|path| path.starts_with("/BinData/"));
    record_removed(
        &mut report,
        PreservationCode::HwpContainerStreamRemoved,
        PreservationResourceKind::ContainerStream,
        removed_other_streams.len()
            + removed_binary_streams
                .len()
                .saturating_sub(removed_binary_allowance),
    );
    record_removed(
        &mut report,
        PreservationCode::HwpContainerStorageRemoved,
        PreservationResourceKind::ContainerStorage,
        source_storages.difference(&output_storages).count(),
    );

    let changed = source_streams
        .intersection(&output_streams)
        .filter(|path| is_hwp_opaque_stream(path))
        .try_fold(0usize, |count, path| {
            let source_bytes = source_container.read_stream_raw(path)?;
            let output_bytes = output_container.read_stream_raw(path)?;
            Ok::<_, hwp5::Hwp5Error>(count + usize::from(source_bytes != output_bytes))
        })?;
    record_changed(
        &mut report,
        PreservationCode::HwpOpaqueStreamChanged,
        PreservationResourceKind::ContainerStream,
        changed,
    );
    Ok(report)
}

fn inspect_hwpx_package(
    source: &Path,
    output: &Path,
    removed_binary_allowance: usize,
) -> anyhow::Result<PreservationReport> {
    let mut source_package = hwpx::HwpxPackage::open(source)?;
    let mut output_package = hwpx::HwpxPackage::open(output)?;
    let source_entries = source_package
        .entries()?
        .into_iter()
        .map(|entry| entry.name)
        .filter(|name| !name.ends_with('/'))
        .collect::<BTreeSet<_>>();
    let output_entries = output_package
        .entries()?
        .into_iter()
        .map(|entry| entry.name)
        .filter(|name| !name.ends_with('/'))
        .collect::<BTreeSet<_>>();

    let mut report = PreservationReport::new();
    let removed_entries = source_entries.difference(&output_entries);
    let (removed_binary_entries, removed_other_entries): (Vec<_>, Vec<_>) =
        removed_entries.partition(|name| name.starts_with("BinData/"));
    record_removed(
        &mut report,
        PreservationCode::HwpxPackageEntryRemoved,
        PreservationResourceKind::PackageEntry,
        removed_other_entries.len()
            + removed_binary_entries
                .len()
                .saturating_sub(removed_binary_allowance),
    );

    let changed = source_entries
        .intersection(&output_entries)
        .filter(|name| is_hwpx_opaque_entry(name))
        .try_fold(0usize, |count, name| {
            let source_bytes = source_package.read_entry(name)?;
            let output_bytes = output_package.read_entry(name)?;
            Ok::<_, hwpx::HwpxError>(count + usize::from(source_bytes != output_bytes))
        })?;
    record_changed(
        &mut report,
        PreservationCode::HwpxOpaqueEntryChanged,
        PreservationResourceKind::PackageEntry,
        changed,
    );
    Ok(report)
}

fn is_hwp_opaque_stream(path: &str) -> bool {
    path != "/FileHeader"
        && path != "/DocInfo"
        && !path.starts_with("/BodyText/")
        && !path.starts_with("/BinData/")
        && path != "/\u{5}HwpSummaryInformation"
        && path != "/PrvText"
        && path != "/PrvImage"
}

fn is_hwpx_opaque_entry(name: &str) -> bool {
    name != "mimetype"
        && name != "version.xml"
        && name != "settings.xml"
        && name != "META-INF/container.xml"
        && name != "META-INF/manifest.xml"
        && name != "Contents/header.xml"
        && name != "Contents/content.hpf"
        && !name.starts_with("Contents/section")
        && !name.starts_with("BinData/")
        && !name.starts_with("Preview/")
}

fn record_removed(
    report: &mut PreservationReport,
    code: PreservationCode,
    resource: PreservationResourceKind,
    count: usize,
) {
    if count > 0 {
        report.record(PreservationEvent::new(
            code,
            resource,
            PreservationDisposition::Removed,
            count,
        ));
    }
}

fn record_changed(
    report: &mut PreservationReport,
    code: PreservationCode,
    resource: PreservationResourceKind,
    count: usize,
) {
    if count > 0 {
        report.record(PreservationEvent::new(
            code,
            resource,
            PreservationDisposition::ChangedNonTarget,
            count,
        ));
    }
}

fn binary_multiset(document: &Document) -> BTreeMap<[u8; 32], usize> {
    let mut values = BTreeMap::new();
    for stream in &document.bin_streams {
        let digest: [u8; 32] = Sha256::digest(&stream.data).into();
        *values.entry(digest).or_default() += 1;
    }
    values
}

fn control_multiset(document: &Document) -> BTreeMap<[u8; 4], usize> {
    let mut values = BTreeMap::new();
    for paragraph in document
        .sections
        .iter()
        .flat_map(|section| section.paragraphs.iter())
    {
        collect_paragraph_controls(paragraph, &mut values, None);
    }
    values
}

fn resolved_picture_relationships(document: &Document) -> usize {
    let mut count = 0usize;
    for paragraph in document
        .sections
        .iter()
        .flat_map(|section| section.paragraphs.iter())
    {
        let mut ignored = BTreeMap::new();
        collect_paragraph_controls(paragraph, &mut ignored, Some((document, &mut count)));
    }
    count
}

fn resolved_picture_asset_multiset(document: &Document) -> BTreeMap<[u8; 32], usize> {
    fn visit(document: &Document, paragraph: &Paragraph, values: &mut BTreeMap<[u8; 32], usize>) {
        for control in &paragraph.controls {
            if let Control::Picture(picture) = control
                && let Some(bytes) = document.resolve_bin(&picture.bin_ref)
            {
                let digest: [u8; 32] = Sha256::digest(bytes).into();
                *values.entry(digest).or_default() += 1;
            }
            let mut visit_nested = |paragraph: &Paragraph| visit(document, paragraph, values);
            match control {
                Control::Table(table) => {
                    for cell in &table.cells {
                        for paragraph in &cell.paragraphs {
                            visit_nested(paragraph);
                        }
                    }
                    if let Some(caption) = &table.caption {
                        for paragraph in &caption.paragraphs {
                            visit_nested(paragraph);
                        }
                    }
                }
                Control::Picture(picture) => {
                    if let Some(caption) = &picture.caption {
                        for paragraph in &caption.paragraphs {
                            visit_nested(paragraph);
                        }
                    }
                }
                Control::Generic(generic) => {
                    for list in &generic.paragraph_lists {
                        for paragraph in &list.paragraphs {
                            visit_nested(paragraph);
                        }
                    }
                    if let Some(caption) = &generic.caption {
                        for paragraph in &caption.paragraphs {
                            visit_nested(paragraph);
                        }
                    }
                }
                Control::SectionDef(_) => {}
            }
        }
    }

    let mut values = BTreeMap::new();
    for paragraph in document
        .sections
        .iter()
        .flat_map(|section| section.paragraphs.iter())
    {
        visit(document, paragraph, &mut values);
    }
    values
}

fn collect_paragraph_controls(
    paragraph: &Paragraph,
    values: &mut BTreeMap<[u8; 4], usize>,
    mut relationships: Option<(&Document, &mut usize)>,
) {
    for control in &paragraph.controls {
        *values.entry(control.ctrl_id()).or_default() += 1;
        if let Control::Picture(picture) = control
            && let Some((document, count)) = relationships.as_mut()
            && document.resolve_bin(&picture.bin_ref).is_some()
        {
            **count += 1;
        }
        let mut visit = |nested: &Paragraph| {
            collect_paragraph_controls(
                nested,
                values,
                relationships
                    .as_mut()
                    .map(|(document, count)| (*document, &mut **count)),
            );
        };
        match control {
            Control::Table(table) => {
                for cell in &table.cells {
                    for nested in &cell.paragraphs {
                        visit(nested);
                    }
                }
                if let Some(caption) = &table.caption {
                    for nested in &caption.paragraphs {
                        visit(nested);
                    }
                }
            }
            Control::Picture(picture) => {
                if let Some(caption) = &picture.caption {
                    for nested in &caption.paragraphs {
                        visit(nested);
                    }
                }
            }
            Control::Generic(generic) => {
                for list in &generic.paragraph_lists {
                    for nested in &list.paragraphs {
                        visit(nested);
                    }
                }
                if let Some(caption) = &generic.caption {
                    for nested in &caption.paragraphs {
                        visit(nested);
                    }
                }
            }
            Control::SectionDef(_) => {}
        }
    }
}

fn metadata_values(document: &Document) -> [Option<&str>; 6] {
    [
        document.metadata.title.as_deref(),
        document.metadata.author.as_deref(),
        document.metadata.subject.as_deref(),
        document.metadata.keywords.as_deref(),
        document.metadata.description.as_deref(),
        document.metadata.last_saved_by.as_deref(),
    ]
}

fn removed_multiset_count<K: Ord>(source: BTreeMap<K, usize>, output: BTreeMap<K, usize>) -> usize {
    source
        .into_iter()
        .map(|(key, count)| count.saturating_sub(output.get(&key).copied().unwrap_or(0)))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwp_model::{BinStream, Metadata};

    #[test]
    fn semantic_report_is_content_free_and_counts_removed_resources() {
        let source = Document {
            metadata: Metadata {
                title: Some("private title".to_string()),
                create_time: Some(123),
                ..Metadata::default()
            },
            bin_streams: vec![BinStream {
                name: "private/name.png".to_string(),
                data: b"private bytes".to_vec(),
            }],
            ..Document::default()
        };
        let output = Document::default();
        let report = inspect_conversion_semantics(&source, &output);

        assert_eq!(report.events.len(), 2);
        assert!(report.events.iter().any(|event| {
            event.code == PreservationCode::BinaryAssetRemoved
                && event.resource == PreservationResourceKind::BinaryAsset
                && event.count == 1
        }));
        assert!(report.events.iter().any(|event| {
            event.code == PreservationCode::MetadataValueRemoved
                && event.resource == PreservationResourceKind::Metadata
                && event.count == 2
        }));
        assert!(report.events.iter().all(|event| {
            !event.code.as_str().contains("private") && !event.code.as_str().contains('/')
        }));
    }

    #[test]
    fn reject_loss_uses_only_bounded_event_codes() {
        let mut report = PreservationReport::new();
        record_removed(
            &mut report,
            PreservationCode::ControlRemoved,
            PreservationResourceKind::Control,
            3,
        );
        let error = reject_loss("convert", &report).unwrap_err().to_string();
        assert!(error.contains("control_removed: 3 item(s)"));
    }
}
