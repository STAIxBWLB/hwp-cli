//! Writes the IR to HWP 5.0 files.
//!
//! Every emitter mirrors its doc_info/body_text parser. The known-prefix-plus-tail rule restores
//! HWP5-origin documents byte-for-byte for simple controls, and semantically for flattened
//! structures such as gso. Derive ID_MAPPINGS counts from table lengths, never synchronize them
//! manually; retain only the tail when the source has version-specific extra counts.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use hwp_model::{
    BorderLine, Caption, CaptionDirection, CaptionSide, Cell, CharShape, Control, Document,
    FaceName, HwpChar, LANG_COUNT, Metadata, OpaqueRecord, ParaShape, Paragraph, Picture,
    PreservationCode, PreservationDisposition, PreservationResourceKind, RawEntry, Section,
    SectionDef, Style, Table, WriteReport,
};

use crate::codec::{ByteWriter, compress, decompress};
use crate::error::Result;
use crate::file_header::{FILE_HEADER_SIZE, FileHeader, HwpVersion};
use crate::record::{RecordNode, ScanMode, scan_stream, tag};

#[derive(Default)]
pub struct WriteOptions {
    /// PrvImage stream content (PNG recommended); omit the stream when absent.
    pub prv_image: Option<Vec<u8>>,
    /// Whether to preserve line layouts (PARA_LINE_SEG).
    ///
    /// Hangul raises a tampering security warning when cached line layouts disagree with content
    /// (Hancom officially recommends removing them after content edits). Default false lets Hangul
    /// recalculate on open. Use true only for unmodified byte round-trips.
    pub preserve_linesegs: bool,
    /// True when rewriting an edited HWP5 source.
    ///
    /// Even HWP5-origin content edits must restore synthesized-path paragraph invariants (ending
    /// 0x0d, nchars bit31 for the last paragraph, omitted PARA_TEXT for empty paragraphs, emitted
    /// line layouts) for Hangul acceptance. Do not resynthesize existing HWP5 pictures: they
    /// already carry shape records; synthesize_pictures is gated by origin.
    pub edited: bool,
    /// Conversion defaults were applied to each non-HWP5 input before merging.
    /// Skip document-wide paragraph-shape defaults so grafted HWP5 shapes keep
    /// their original values, including a zero border-fill sentinel.
    pub para_shape_defaults_prepared: bool,
    /// Assign unique, nonzero paragraph IDs across the emitted record forests.
    /// Merge enables this even without synthesis; ordinary HWP5 round trips
    /// leave it false to preserve source IDs and byte identity.
    pub deduplicate_instance_ids: bool,
}

#[derive(Debug)]
struct HwpStreamMaterialization {
    storages: BTreeSet<String>,
    streams: BTreeMap<String, Vec<u8>>,
    bin_stream_paths: BTreeMap<String, String>,
    report: WriteReport,
}

#[derive(Debug)]
struct HwpMutationPlan {
    metadata_changed: bool,
    header_changed: bool,
    changed_sections: BTreeSet<usize>,
    removed_sections: BTreeSet<usize>,
    changed_bins: BTreeSet<String>,
    removed_bins: BTreeSet<String>,
}

/// Saves a document as HWP 5.0 and returns flattening/removal warnings.
///
/// This is the legacy compatibility surface. New callers should use
/// [`write_document_with_report`] so preservation events remain typed rather
/// than being reconstructed from warning strings.
pub fn write_document(doc: &Document, path: &Path, opts: &WriteOptions) -> Result<Vec<String>> {
    write_document_with_report(doc, path, opts).map(WriteReport::into_legacy_warnings)
}

/// Writes an HWP 5.0 document and returns typed warnings and preservation events.
pub fn write_document_with_report(
    doc: &Document,
    path: &Path,
    opts: &WriteOptions,
) -> Result<WriteReport> {
    let materialization = materialize_document(doc, opts, true, false)?;
    write_materialized_document(path, &materialization)?;
    Ok(materialization.report)
}

/// Rewrites an HWP document by copying its immutable source container and
/// replacing only streams selected by a mutation plan derived from the
/// original and edited IR values.
///
/// A no-op is an exact file copy. Opaque streams, storages, directory metadata,
/// FileHeader, scripts, document options, history, and every unrelated binary
/// stream remain owned by the source container rather than being regenerated.
pub fn rewrite_document_with_report(
    source: &Path,
    original: &Document,
    edited: &Document,
    path: &Path,
    opts: &WriteOptions,
) -> Result<WriteReport> {
    if source_output_alias(source, path)? {
        return Err(crate::error::Hwp5Error::SourceRewrite(
            "source and output paths must be different".to_string(),
        ));
    }
    let source_document = crate::read_document(source)?.document;
    if source_document != *original {
        return Err(crate::error::Hwp5Error::SourceSnapshotMismatch);
    }
    validate_source_rewrite_contract(original, edited)?;

    if original == edited {
        std::fs::copy(source, path)?;
        return Ok(WriteReport::new());
    }

    let plan = HwpMutationPlan::derive(original, edited);
    let mut source_container = crate::Hwp5Container::open(source)?;
    let compressed = source_container.file_header().is_compressed();
    let source_bin_paths = source_container
        .list_streams()
        .into_iter()
        .filter_map(|entry| {
            let normalized = entry
                .path
                .strip_prefix("/BinData/")
                .map(normalized_bin_name);
            normalized.map(|name| (name, entry.path))
        })
        .collect::<BTreeMap<_, _>>();
    let source_section_roots = plan
        .changed_sections
        .iter()
        .filter(|index| **index < original.sections.len())
        .map(|index| {
            let stream_path = format!("/BodyText/Section{index}");
            let bytes = source_container.read_record_stream(&stream_path)?;
            let scan = scan_stream(&bytes, ScanMode::Strict)?;
            Ok((*index, scan.roots))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    drop(source_container);

    let mut materialization = materialize_document(edited, opts, compressed, true)?;
    preserve_unchanged_body_subtrees(
        original,
        edited,
        compressed,
        &source_section_roots,
        &mut materialization,
    )?;
    std::fs::copy(source, path)?;
    patch_source_container(path, &plan, &source_bin_paths, &materialization)?;
    Ok(materialization.report)
}

fn source_output_alias(source: &Path, output: &Path) -> Result<bool> {
    if source == output {
        return Ok(true);
    }
    let source_metadata = std::fs::metadata(source)?;
    let output_metadata = match std::fs::metadata(output) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        Ok(source_metadata.dev() == output_metadata.dev()
            && source_metadata.ino() == output_metadata.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = (source_metadata, output_metadata);
        Ok(std::fs::canonicalize(source)? == std::fs::canonicalize(output)?)
    }
}

/// Replaces regenerated, unchanged typed controls with their immutable source
/// subtrees. This keeps level-sensitive records such as a section definition's
/// CTRL_DATA/PAGE_DEF sequence byte-identical while still allowing the edited
/// paragraph records around them to change.
fn preserve_unchanged_body_subtrees(
    original: &Document,
    edited: &Document,
    compressed: bool,
    source_sections: &BTreeMap<usize, Vec<RecordNode>>,
    materialization: &mut HwpStreamMaterialization,
) -> Result<()> {
    for (index, source_roots) in source_sections {
        let stream_path = format!("/BodyText/Section{index}");
        let materialized = required_materialized_stream(materialization, &stream_path)?;
        let decoded = if compressed {
            decompress(materialized, &stream_path)?
        } else {
            materialized.to_vec()
        };
        let emitted_roots = scan_stream(&decoded, ScanMode::Strict)?.roots;
        let original_section = original.sections.get(*index).ok_or_else(|| {
            crate::error::Hwp5Error::SourceRewrite(format!(
                "source section {index} is missing from the original IR"
            ))
        })?;
        let edited_section = edited.sections.get(*index).ok_or_else(|| {
            crate::error::Hwp5Error::SourceRewrite(format!(
                "changed section {index} is missing from the edited IR"
            ))
        })?;
        let merged = merge_section_roots(
            source_roots,
            &emitted_roots,
            original_section,
            edited_section,
        )?;
        materialization.streams.insert(
            stream_path,
            encode_compressed_stream(RecordNode::serialize_forest(&merged), compressed),
        );
    }
    Ok(())
}

fn merge_section_roots(
    source_roots: &[RecordNode],
    emitted_roots: &[RecordNode],
    original: &Section,
    edited: &Section,
) -> Result<Vec<RecordNode>> {
    let source_paragraphs = source_roots
        .iter()
        .filter(|node| node.tag == tag::PARA_HEADER)
        .collect::<Vec<_>>();
    let emitted_paragraphs = emitted_roots
        .iter()
        .filter(|node| node.tag == tag::PARA_HEADER)
        .collect::<Vec<_>>();
    if source_paragraphs.len() != original.paragraphs.len()
        || emitted_paragraphs.len() != edited.paragraphs.len()
    {
        return Err(crate::error::Hwp5Error::SourceRewrite(
            "paragraph records cannot be matched to the source and edited IR".to_string(),
        ));
    }

    let matches = match_paragraphs(&original.paragraphs, &edited.paragraphs);
    if original.extras == edited.extras
        && source_paragraphs.len() == emitted_paragraphs.len()
        && matches
            .iter()
            .enumerate()
            .all(|(edited_index, matched)| *matched == Some(edited_index))
    {
        let mut paragraph_index = 0usize;
        return Ok(source_roots
            .iter()
            .map(|source_node| {
                if source_node.tag != tag::PARA_HEADER {
                    return source_node.clone();
                }
                let merged = merge_paragraph_node(
                    source_node,
                    emitted_paragraphs[paragraph_index],
                    &original.paragraphs[paragraph_index],
                    &edited.paragraphs[paragraph_index],
                );
                paragraph_index += 1;
                merged
            })
            .collect());
    }

    let mut roots = Vec::with_capacity(emitted_roots.len());
    for (edited_index, emitted_node) in emitted_paragraphs.iter().enumerate() {
        if let Some(original_index) = matches[edited_index] {
            roots.push(merge_paragraph_node(
                source_paragraphs[original_index],
                emitted_node,
                &original.paragraphs[original_index],
                &edited.paragraphs[edited_index],
            ));
        } else {
            roots.push((*emitted_node).clone());
        }
    }

    if original.extras == edited.extras {
        roots.extend(
            source_roots
                .iter()
                .filter(|node| node.tag != tag::PARA_HEADER)
                .cloned(),
        );
    } else {
        roots.extend(
            emitted_roots
                .iter()
                .filter(|node| node.tag != tag::PARA_HEADER)
                .cloned(),
        );
    }
    Ok(roots)
}

fn match_paragraphs(original: &[Paragraph], edited: &[Paragraph]) -> Vec<Option<usize>> {
    let mut used = vec![false; original.len()];
    let mut matches = vec![None; edited.len()];
    for (edited_index, paragraph) in edited.iter().enumerate() {
        let instance_id = paragraph.header.instance_id;
        if instance_id == 0
            || edited
                .iter()
                .filter(|candidate| candidate.header.instance_id == instance_id)
                .count()
                != 1
        {
            continue;
        }
        let candidates = original
            .iter()
            .enumerate()
            .filter(|(index, candidate)| {
                !used[*index] && candidate.header.instance_id == instance_id
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            used[candidates[0]] = true;
            matches[edited_index] = Some(candidates[0]);
        }
    }
    for (edited_index, paragraph) in edited.iter().enumerate() {
        if matches[edited_index].is_some() {
            continue;
        }
        if let Some((original_index, _)) = original
            .iter()
            .enumerate()
            .find(|(index, candidate)| !used[*index] && *candidate == paragraph)
        {
            used[original_index] = true;
            matches[edited_index] = Some(original_index);
        }
    }
    if original.len() == edited.len() {
        for edited_index in 0..edited.len() {
            if matches[edited_index].is_none() && !used[edited_index] {
                used[edited_index] = true;
                matches[edited_index] = Some(edited_index);
            }
        }
    }
    matches
}

fn merge_paragraph_node(
    source: &RecordNode,
    emitted: &RecordNode,
    original: &Paragraph,
    edited: &Paragraph,
) -> RecordNode {
    if original == edited {
        return source.clone();
    }
    let source_controls = source
        .children
        .iter()
        .filter(|node| node.tag == tag::CTRL_HEADER)
        .collect::<Vec<_>>();
    let control_matches = match_controls(&original.controls, &edited.controls);
    let mut merged = emitted.clone();
    let mut edited_control_index = 0usize;
    for child in &mut merged.children {
        if child.tag != tag::CTRL_HEADER {
            continue;
        }
        if let Some(Some(original_control_index)) = control_matches.get(edited_control_index)
            && let (Some(source_control), Some(original_control), Some(edited_control)) = (
                source_controls.get(*original_control_index),
                original.controls.get(*original_control_index),
                edited.controls.get(edited_control_index),
            )
        {
            *child = merge_control_node(source_control, child, original_control, edited_control);
        }
        edited_control_index += 1;
    }
    merged
}

fn match_controls(original: &[Control], edited: &[Control]) -> Vec<Option<usize>> {
    let mut used = vec![false; original.len()];
    let mut matches = vec![None; edited.len()];
    for (edited_index, control) in edited.iter().enumerate() {
        if edited
            .iter()
            .filter(|candidate| *candidate == control)
            .count()
            != 1
        {
            continue;
        }
        let candidates = original
            .iter()
            .enumerate()
            .filter(|(index, candidate)| !used[*index] && *candidate == control)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            used[candidates[0]] = true;
            matches[edited_index] = Some(candidates[0]);
        }
    }
    if original.len() == edited.len() {
        for edited_index in 0..edited.len() {
            if matches[edited_index].is_none() && !used[edited_index] {
                used[edited_index] = true;
                matches[edited_index] = Some(edited_index);
            }
        }
    }
    matches
}

fn merge_control_node(
    source: &RecordNode,
    emitted: &RecordNode,
    original: &Control,
    edited: &Control,
) -> RecordNode {
    if original == edited {
        return source.clone();
    }
    match (original, edited) {
        (Control::Table(original), Control::Table(edited)) => {
            merge_table_control(source, emitted, original, edited)
        }
        _ => emitted.clone(),
    }
}

/// Rewrites the edited paragraphs of a table into its **source** record tree, leaving every
/// other source child byte-identical and in place.
///
/// The table itself must be structurally unchanged (same cells, caption, spans, tails); a
/// structural edit still falls back to the fully regenerated `emitted` node. Within one
/// paragraph list the count may change: the list's paragraph records are then replaced as a
/// run and only its LIST_HEADER paragraph count is rewritten. Regenerating the whole table
/// instead would lose what the IR does not carry - `emit_caption_header` cannot reproduce a
/// caption's wrapping and vertical-alignment listflags, and `table.extras` are re-appended
/// after every cell rather than at the position they were read from.
fn merge_table_control(
    source: &RecordNode,
    emitted: &RecordNode,
    original: &Table,
    edited: &Table,
) -> RecordNode {
    let mut original_structure = original.clone();
    let mut edited_structure = edited.clone();
    clear_table_paragraphs(&mut original_structure);
    clear_table_paragraphs(&mut edited_structure);
    if original_structure != edited_structure {
        return emitted.clone();
    }

    let original_lists = table_paragraph_lists(original);
    let edited_lists = table_paragraph_lists(edited);
    let Some(source_lists) = record_paragraph_lists(source, original) else {
        return emitted.clone();
    };
    let Some(emitted_lists) = record_paragraph_lists(emitted, edited) else {
        return emitted.clone();
    };
    // The record tree and the IR must agree list for list, otherwise the addressing below
    // would splice into the wrong place - fall back to the regenerated table.
    if source_lists.len() != original_lists.len()
        || emitted_lists.len() != edited_lists.len()
        || original_lists.len() != edited_lists.len()
        || source_lists
            .iter()
            .zip(&original_lists)
            .any(|((_, records), paragraphs)| records.len() != paragraphs.len())
        || emitted_lists
            .iter()
            .zip(&edited_lists)
            .any(|((_, records), paragraphs)| records.len() != paragraphs.len())
    {
        return emitted.clone();
    }

    let mut merged = source.clone();
    // Splice from the last list backwards so a length change never shifts an index this loop
    // has yet to use.
    for list_index in (0..source_lists.len()).rev() {
        let (header_index, source_paragraph_indices) = &source_lists[list_index];
        let (_, emitted_paragraph_indices) = &emitted_lists[list_index];
        let original_paragraphs = original_lists[list_index];
        let edited_paragraphs = edited_lists[list_index];
        if original_paragraphs == edited_paragraphs {
            continue;
        }
        if original_paragraphs.len() == edited_paragraphs.len() {
            for (position, &child_index) in source_paragraph_indices.iter().enumerate() {
                if original_paragraphs[position] != edited_paragraphs[position] {
                    merged.children[child_index] = merge_paragraph_node(
                        &source.children[child_index],
                        &emitted.children[emitted_paragraph_indices[position]],
                        &original_paragraphs[position],
                        &edited_paragraphs[position],
                    );
                }
            }
            continue;
        }
        // The count changed. Replace this list's paragraph records as one run; that keeps
        // every other source child - opaque records, the TABLE record, other cells - exactly
        // where and as it was.
        let contiguous = source_paragraph_indices
            .windows(2)
            .all(|pair| pair[1] == pair[0] + 1);
        if !contiguous {
            return emitted.clone();
        }
        let start = source_paragraph_indices
            .first()
            .copied()
            .unwrap_or(header_index + 1);
        let end = source_paragraph_indices
            .last()
            .map_or(start, |last| last + 1);
        let replacement = emitted_paragraph_indices
            .iter()
            .map(|&index| emitted.children[index].clone());
        merged.children.splice(start..end, replacement);
        // LIST_HEADER's leading INT32 is the paragraph count (caption table 71, cell table 76).
        let count = edited_paragraphs.len() as i32;
        let header = &mut merged.children[*header_index];
        if header.data.len() < 4 {
            return emitted.clone();
        }
        header.data[..4].copy_from_slice(&count.to_le_bytes());
    }
    merged
}

fn clear_table_paragraphs(table: &mut Table) {
    if let Some(caption) = &mut table.caption {
        caption.paragraphs.clear();
    }
    for cell in &mut table.cells {
        cell.paragraphs.clear();
    }
}

/// The table's paragraph lists in canonical order: the caption first, then the cells - the
/// order `emit_table` writes them in.
fn table_paragraph_lists(table: &Table) -> Vec<&[Paragraph]> {
    let mut lists = table
        .caption
        .iter()
        .map(|caption| caption.paragraphs.as_slice())
        .collect::<Vec<_>>();
    lists.extend(table.cells.iter().map(|cell| cell.paragraphs.as_slice()));
    lists
}

/// Groups a table record node's children into `(LIST_HEADER index, paragraph record indices)`
/// per list, returned in the same canonical order as [`table_paragraph_lists`].
///
/// `parse_table` accepts a caption LIST_HEADER either before the TABLE record or after the
/// declared cells are full, so a source tree may carry it last; the emitted tree always writes
/// it first. Returns `None` when the children cannot be grouped that way - a paragraph record
/// with no list header before it, or a caption the parser found in neither position.
fn record_paragraph_lists(node: &RecordNode, table: &Table) -> Option<Vec<(usize, Vec<usize>)>> {
    let mut lists: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut table_record: Option<usize> = None;
    for (index, child) in node.children.iter().enumerate() {
        match child.tag {
            tag::TABLE if table_record.is_none() => table_record = Some(index),
            tag::LIST_HEADER => lists.push((index, Vec::new())),
            tag::PARA_HEADER => lists.last_mut()?.1.push(index),
            _ => {}
        }
    }
    if table.caption.is_some() {
        let first = lists.first()?.0;
        if first > table_record? {
            // Caption last in the source: rotate it to the front so list 0 is the caption.
            let caption = lists.pop()?;
            lists.insert(0, caption);
        }
    }
    Some(lists)
}

fn materialize_document(
    doc: &Document,
    opts: &WriteOptions,
    compressed: bool,
    source_preserving: bool,
) -> Result<HwpStreamMaterialization> {
    let mut report = WriteReport::new();
    let input_bin_names = doc
        .bin_streams
        .iter()
        .map(|stream| normalized_bin_name(&stream.name))
        .collect::<Vec<_>>();

    // Normalize HWPX-origin documents: pictures without HWP5 SHAPE_COMPONENT records cannot be
    // written, so remove controls and extended characters together.
    //
    // synthesize restores paragraph invariants and line layouts for HWPX/Markdown input or edited
    // HWP5. synth_pictures creates HWP5 shapes for HWPX/Markdown pictures based on origin,
    // excluding edited HWP5 pictures that already have shape records. Newly inserted pictures with
    // empty extras must be synthesized regardless of origin or strip_unwritable_pictures removes
    // them.
    let synth_pictures = doc.meta.source_format != "hwp5" || has_synthesizable_picture(doc);
    // HWPX-origin structured shapes/text boxes (gso_shapes) need HWP5 gso record synthesis to
    // survive stripping, using the same gate logic as picture synthesis.
    let synth_gso = doc.meta.source_format != "hwp5" || has_synthesizable_gso(doc);
    let synthesize = synth_pictures || synth_gso || opts.edited;
    let normalized;
    let doc = if needs_normalize(doc) || synthesize {
        let mut d = doc.clone();
        // First synthesize HWP5 shape records (SHAPE_COMPONENT plus picture) for HWPX/Markdown
        // images. Populated extras prevent stripping below.
        if synth_pictures {
            synthesize_pictures(&mut d, &mut report);
        }
        if synth_gso {
            degrade_hwpx_gso(&mut d, &mut report);
        }
        for section in &mut d.sections {
            for para in &mut section.paragraphs {
                strip_unwritable_pictures(para, &mut report);
            }
        }
        // Synthesize PARA_LINE_SEG for generated 5.1.x documents. Without this cache, Hangul draws
        // body text at zero height (black bars/empty content) or declares corruption. Hardware
        // round 13 confirmed that clearing bit31 corrupts all cases; work_rt 5.0.2.4 recalculates,
        // but genuine 5.1.x files have line segments for every body paragraph.
        if synthesize {
            // hwp-cli prepopulates para.line_segs using accurate hwp-render font shaping, since
            // hwp5 cannot depend back on hwp-render. This layer only emits those line layouts.
            // nchars bit31 marks the last paragraph of a list. Set it only on the last paragraph in
            // each section/table-cell/text-box list (measured in genuine multiparagraph 5.1.1.0
            // files).
            for section in &mut d.sections {
                set_last_para_flag(&mut section.paragraphs);
            }
            // HWPX round-trips lose ParaShape.attr1 Korean line-breaking/grid bits,
            // line_spacing_old, border_fill_id, and the first section paragraph's break_type=0x03
            // because its writer/reader does not serialize/restore them. Inject normal-sample
            // reference values only on the synthesized path. Unmodified HWP5 round-trips skip this
            // path (synthesize=false), preserving the byte-identity gate.
            if !source_preserving {
                // An HWP5 source's own para shapes are genuine: a border_fill_id of 0 there
                // means "no border", and 5.0.2.x files use it (#381 review). Only a converted
                // document lost these fields; a merge's grafted HWPX shapes carry them.
                if d.meta.source_format != "hwp5" && !opts.para_shape_defaults_prepared {
                    ensure_para_shape_defaults(&mut d.header);
                }
                encode_numbered_levels_zero_based(&mut d);
                materialize_evidenced_official_numbering(&mut d)?;
            }
            for section in &mut d.sections {
                if let Some(p) = section.paragraphs.first_mut() {
                    p.header.break_type |= 0x03;
                }
            }
        }
        normalized = d;
        &normalized
    } else {
        doc
    };

    // Construct record streams.
    let doc_info_nodes = emit_doc_info(doc, &mut report);
    let doc_info = RecordNode::serialize_forest(&doc_info_nodes);
    // PARA_HEADER tail gate: versions 5.0.3.2+ require a UINT16 merged-track-change-paragraph field
    // (specification table 58, total 24 bytes). Pad only synthesized paragraphs with empty tails to
    // 24 bytes. Writing 22 causes a version/layout mismatch and Hangul corruption/tampering
    // warnings, verified with sample_m6 5.1.0.1 and halla 5.1.1.0. Pre-5.0.3.2 (work_report
    // 5.0.2.4) correctly uses 22 bytes, so disable this gate there.
    let add_tracking_tail = parse_version(&doc.meta.source_version).to_u32() >= 0x05_00_03_02;
    // Paragraph instance id counter: a synthesized paragraph (instance_id 0) gets a non-zero id.
    // Hancom judges instance_id 0 abnormal ("corrupt/tampered", A8; every sample is non-zero).
    // An hwp5 round trip keeps the source ids (0 included) for byte identity, so it is skipped.
    // A new id also skips every id the document already carries (an HWP5 section merged with
    // HWPX paragraphs, a part spliced into an hwp-cli template), keeping new ids off existing
    // ones. Merge also repairs existing duplicates, including paragraphs in raw
    // Generic subtrees, without changing the source-preserving default behavior.
    let mut inst_counter = 0x1000_0000u32;
    let mut section_roots: Vec<Vec<RecordNode>> = doc
        .sections
        .iter()
        .map(|s| {
            emit_section(
                s,
                synthesize,
                opts.preserve_linesegs,
                add_tracking_tail,
                &mut report,
            )
        })
        .collect();
    if synthesize || opts.deduplicate_instance_ids {
        let mut used = BTreeSet::new();
        for roots in &section_roots {
            collect_instance_ids(roots, &mut used);
        }
        let mut seen = BTreeSet::new();
        for roots in &mut section_roots {
            assign_instance_ids(
                roots,
                &mut inst_counter,
                &mut used,
                &mut seen,
                opts.deduplicate_instance_ids,
            )?;
        }
    }
    let sections: Vec<Vec<u8>> = section_roots
        .iter()
        .map(|roots| RecordNode::serialize_forest(roots))
        .collect();

    // FileHeader
    let header = FileHeader {
        version: parse_version(&doc.meta.source_version),
        attributes: 0x1, // Compressed.
        license: 0,
        // Modern Hangul (2010+) always writes EncryptVersion=4 (Hangul 7.0+) even for unencrypted
        // documents. All six fixtures/hwp5 samples have attr1 encryption bit1 clear but encver=4.
        // Writing zero makes Hangul reject corruption/tampering (hardware gate).
        encrypt_version: 4,
        kogl_country: 0,
        reserved: [0u8; FILE_HEADER_SIZE - 49],
    };

    // Preview text (UTF-16LE, approximately 1000 characters).
    let mut preview = doc.plain_text();
    preview.truncate(
        preview
            .char_indices()
            .nth(1000)
            .map_or(preview.len(), |(i, _)| i),
    );
    let prv_text: Vec<u8> = preview.encode_utf16().flat_map(u16::to_le_bytes).collect();

    let mut storages = BTreeSet::from(["/BodyText".to_string()]);
    let mut streams = BTreeMap::new();
    streams.insert("/FileHeader".to_string(), header.serialize());
    streams.insert(
        "/DocInfo".to_string(),
        encode_compressed_stream(doc_info, compressed),
    );
    for (i, body) in sections.iter().enumerate() {
        streams.insert(
            format!("/BodyText/Section{i}"),
            encode_compressed_stream(body.clone(), compressed),
        );
    }
    // Include only streams referenced by the BIN_DATA table, following HWP5 naming rules.
    let referenced: Vec<String> = doc
        .header
        .bin_data
        .iter()
        .filter_map(|item| {
            let id = item.storage_id?;
            let ext = item.extension.as_deref().unwrap_or("");
            Some(format!("BIN{id:04X}.{ext}"))
        })
        .collect();
    let mut bin_written = 0usize;
    let mut bin_stream_paths = BTreeMap::new();
    if !referenced.is_empty() && !doc.bin_streams.is_empty() {
        storages.insert("/BinData".to_string());
        for (index, bin) in doc.bin_streams.iter().enumerate() {
            let base = bin.name.rsplit('/').next().unwrap_or(&bin.name);
            if referenced.iter().any(|r| r.eq_ignore_ascii_case(base)) {
                let stream_path = format!("/BinData/{base}");
                streams.insert(
                    stream_path.clone(),
                    encode_compressed_stream(bin.data.clone(), compressed),
                );
                bin_stream_paths.insert(normalized_bin_name(base), stream_path.clone());
                if let Some(input_name) = input_bin_names.get(index) {
                    bin_stream_paths.insert(input_name.clone(), stream_path);
                }
                bin_written += 1;
            }
        }
    }
    if bin_written < doc.bin_streams.len() {
        report.warning(format!(
            "BinData {}개 중 {}개만 동봉 (hwp5 BIN_DATA 테이블이 참조하는 항목만)",
            doc.bin_streams.len(),
            bin_written
        ));
    }
    storages.insert("/DocOptions".to_string());
    streams.insert("/DocOptions/_LinkDoc".to_string(), vec![0u8; 524]);
    storages.insert("/Scripts".to_string());
    streams.insert(
        "/Scripts/JScriptVersion".to_string(),
        vec![
            0x63, 0x64, 0x80, 0x00, 0x00, 0xF7, 0xDF, 0x88, 0xA9, 0x08, 0x00, 0x00, 0x00,
        ],
    );
    streams.insert(
        "/Scripts/DefaultJScript".to_string(),
        vec![
            0x63, 0x60, 0x40, 0x05, 0xFF, 0x81, 0x00, 0x00, 0x6E, 0xBB, 0x6E, 0xD1, 0x14, 0x00,
            0x00, 0x00,
        ],
    );
    streams.insert(
        "/\u{5}HwpSummaryInformation".to_string(),
        hwp_summary_information(&doc.metadata),
    );
    streams.insert("/PrvText".to_string(), prv_text);
    if let Some(img) = &opts.prv_image {
        streams.insert("/PrvImage".to_string(), img.clone());
    }
    for (path, bytes) in doc
        .hwp5_xml_template
        .iter()
        .chain(doc.hwp5_doc_history.iter())
    {
        if let Some((parent, _)) = path.rsplit_once('/')
            && !parent.is_empty()
        {
            storages.insert(parent.to_string());
        }
        streams.insert(path.clone(), bytes.clone());
    }

    Ok(HwpStreamMaterialization {
        storages,
        streams,
        bin_stream_paths,
        report,
    })
}

/// CFB entries (both streams AND storages) can pick up the wall clock from the underlying
/// `cfb` crate whenever a genuinely new directory entry is created (confirmed this session:
/// a freshly created `BinData` storage carries `SystemTime::now()` as its created/modified
/// time; a stream created via `create_stream`/`create_new_stream` does not, but nothing in the
/// `cfb` crate's public contract promises that will stay true across versions). The `cfb` crate
/// (0.14) never rewrites the times of an entry that already exists, so pinning every entry the
/// write itself created makes both write paths deterministic, while an in-place edit keeps the
/// source container's own entry times intact (#253 review).
const SECONDS_FROM_CFB_TO_UNIX_EPOCH: u64 = 11_644_473_600;

fn cfb_epoch() -> std::time::SystemTime {
    std::time::UNIX_EPOCH
        .checked_sub(std::time::Duration::from_secs(
            SECONDS_FROM_CFB_TO_UNIX_EPOCH,
        ))
        .expect("CFB epoch는 지원되는 SystemTime 범위")
}

/// Pins the created/modified time of every entry (stream and storage) not in `source_entries`
/// to the fixed CFB epoch, so a container's bytes do not depend on when it was written. The
/// compose path passes an empty set (pin everything); the in-place edit path passes the source
/// container's entry paths so untouched source metadata survives. Called right before
/// `cfb.flush()`.
fn pin_new_entry_times(
    cfb: &mut cfb::CompoundFile<std::fs::File>,
    source_entries: &BTreeSet<PathBuf>,
) -> Result<()> {
    let epoch = cfb_epoch();
    let entry_paths = cfb
        .walk()
        .map(|entry| entry.path().to_path_buf())
        .filter(|path| !source_entries.contains(path))
        .collect::<Vec<_>>();
    for entry_path in entry_paths {
        cfb.set_created_time(&entry_path, epoch)?;
        cfb.set_modified_time(&entry_path, epoch)?;
    }
    Ok(())
}

fn write_materialized_document(
    path: &Path,
    materialization: &HwpStreamMaterialization,
) -> Result<()> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    let mut cfb = cfb::CompoundFile::create_with_version(cfb::Version::V3, file)?;
    for storage in &materialization.storages {
        cfb.create_storage_all(storage)?;
    }
    for (stream_path, bytes) in &materialization.streams {
        if let Some((parent, _)) = stream_path.rsplit_once('/')
            && !parent.is_empty()
        {
            cfb.create_storage_all(parent)?;
        }
        cfb.create_new_stream(stream_path)?.write_all(bytes)?;
    }

    pin_new_entry_times(&mut cfb, &BTreeSet::new())?;
    cfb.flush()?;
    Ok(())
}

fn encode_compressed_stream(bytes: Vec<u8>, compressed: bool) -> Vec<u8> {
    if compressed { compress(&bytes) } else { bytes }
}

fn validate_source_rewrite_contract(original: &Document, edited: &Document) -> Result<()> {
    if original.meta.source_format != "hwp5" || edited.meta.source_format != "hwp5" {
        return Err(crate::error::Hwp5Error::SourceRewrite(
            "source-preserving rewrite requires HWP5 IR".to_string(),
        ));
    }
    if original.meta != edited.meta {
        return Err(crate::error::Hwp5Error::SourceRewrite(
            "source format and version cannot be changed by a native rewrite".to_string(),
        ));
    }
    if original.hwpx_settings_xml != edited.hwpx_settings_xml
        || original.hwpx_version_xml != edited.hwpx_version_xml
        || original.hwpx_preview_image != edited.hwpx_preview_image
        || original.hwp5_xml_template != edited.hwp5_xml_template
        || original.hwp5_doc_history != edited.hwp5_doc_history
    {
        return Err(crate::error::Hwp5Error::SourceRewrite(
            "opaque ancillary payloads are not editable through the HWP IR writer".to_string(),
        ));
    }
    ensure_unique_bin_names(original)?;
    ensure_unique_bin_names(edited)?;
    Ok(())
}

fn ensure_unique_bin_names(document: &Document) -> Result<()> {
    let mut names = BTreeSet::new();
    for stream in &document.bin_streams {
        let name = normalized_bin_name(&stream.name);
        if !names.insert(name) {
            return Err(crate::error::Hwp5Error::SourceRewrite(
                "duplicate case-insensitive BinData names cannot be rewritten safely".to_string(),
            ));
        }
    }
    Ok(())
}

impl HwpMutationPlan {
    fn derive(original: &Document, edited: &Document) -> Self {
        let changed_sections = edited
            .sections
            .iter()
            .enumerate()
            .filter(|(index, section)| original.sections.get(*index) != Some(*section))
            .map(|(index, _)| index)
            .collect();
        let removed_sections = (edited.sections.len()..original.sections.len()).collect();

        let original_bins = document_bins(original);
        let edited_bins = document_bins(edited);
        let changed_bins: BTreeSet<String> = edited_bins
            .iter()
            .filter(|(name, stream)| original_bins.get(*name) != Some(stream))
            .map(|(name, _)| name.clone())
            .collect();
        let removed_bins: BTreeSet<String> = original_bins
            .keys()
            .filter(|name| !edited_bins.contains_key(*name))
            .cloned()
            .collect();
        let bin_relationships_changed = original_bins.keys().ne(edited_bins.keys());
        let header_changed = original.header != edited.header
            || original.sections.len() != edited.sections.len()
            || bin_relationships_changed;

        Self {
            metadata_changed: original.metadata != edited.metadata,
            header_changed,
            changed_sections,
            removed_sections,
            changed_bins,
            removed_bins,
        }
    }
}

fn document_bins(document: &Document) -> BTreeMap<String, &hwp_model::BinStream> {
    document
        .bin_streams
        .iter()
        .map(|stream| (normalized_bin_name(&stream.name), stream))
        .collect()
}

fn normalized_bin_name(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_ascii_lowercase()
}

fn patch_source_container(
    path: &Path,
    plan: &HwpMutationPlan,
    source_bin_paths: &BTreeMap<String, String>,
    materialization: &HwpStreamMaterialization,
) -> Result<()> {
    let mut cfb = cfb::open_rw(path)?;
    let source_entries = cfb
        .walk()
        .map(|entry| entry.path().to_path_buf())
        .collect::<BTreeSet<_>>();

    if plan.metadata_changed {
        replace_cfb_stream(
            &mut cfb,
            "/\u{5}HwpSummaryInformation",
            required_materialized_stream(materialization, "/\u{5}HwpSummaryInformation")?,
        )?;
    }
    if plan.header_changed {
        replace_cfb_stream(
            &mut cfb,
            "/DocInfo",
            required_materialized_stream(materialization, "/DocInfo")?,
        )?;
    }
    for index in &plan.changed_sections {
        let stream_path = format!("/BodyText/Section{index}");
        replace_cfb_stream(
            &mut cfb,
            &stream_path,
            required_materialized_stream(materialization, &stream_path)?,
        )?;
    }
    for index in &plan.removed_sections {
        let stream_path = format!("/BodyText/Section{index}");
        if cfb.is_stream(&stream_path) {
            cfb.remove_stream(&stream_path)?;
        }
    }

    for normalized_name in &plan.changed_bins {
        let stream_path = materialization
            .bin_stream_paths
            .get(normalized_name)
            .ok_or_else(|| {
                crate::error::Hwp5Error::SourceRewrite(
                    "a changed BinData item is not representable in the materialized document"
                        .to_string(),
                )
            })?;
        let bytes = required_materialized_stream(materialization, stream_path)?;
        replace_cfb_stream(&mut cfb, stream_path, bytes)?;
    }
    for normalized_name in &plan.removed_bins {
        if let Some(stream_path) = source_bin_paths.get(normalized_name)
            && cfb.is_stream(stream_path)
        {
            cfb.remove_stream(stream_path)?;
        }
    }

    if !plan.changed_sections.is_empty() || !plan.removed_sections.is_empty() {
        replace_cfb_stream(
            &mut cfb,
            "/PrvText",
            required_materialized_stream(materialization, "/PrvText")?,
        )?;
        if let Some(preview) = materialization.streams.get("/PrvImage") {
            replace_cfb_stream(&mut cfb, "/PrvImage", preview)?;
        }
    }
    pin_new_entry_times(&mut cfb, &source_entries)?;
    cfb.flush()?;
    Ok(())
}

fn required_materialized_stream<'a>(
    materialization: &'a HwpStreamMaterialization,
    path: &str,
) -> Result<&'a [u8]> {
    materialization
        .streams
        .get(path)
        .map(Vec::as_slice)
        .ok_or_else(|| crate::error::Hwp5Error::StreamNotFound(path.to_string()))
}

fn replace_cfb_stream(
    cfb: &mut cfb::CompoundFile<std::fs::File>,
    path: &str,
    bytes: &[u8],
) -> Result<()> {
    if let Some((parent, _)) = path.rsplit_once('/')
        && !parent.is_empty()
    {
        cfb.create_storage_all(parent)?;
    }
    cfb.create_stream(path)?.write_all(bytes)?;
    Ok(())
}

/// The instance id (offset 18..22) of a PARA_HEADER record.
fn para_header_instance_id(node: &RecordNode) -> Option<u32> {
    (node.tag == tag::PARA_HEADER && node.data.len() >= 22)
        .then(|| u32::from_le_bytes([node.data[18], node.data[19], node.data[20], node.data[21]]))
}

fn collect_instance_ids(roots: &[RecordNode], used: &mut BTreeSet<u32>) {
    for node in roots {
        if let Some(inst) = para_header_instance_id(node).filter(|&inst| inst != 0) {
            used.insert(inst);
        }
        collect_instance_ids(&node.children, used);
    }
}

/// Allocates past `counter`, skipping zero and all original or newly allocated IDs.
/// Wrapping searches the remaining nonzero space instead of overflowing or reusing an ID.
fn next_instance_id(counter: &mut u32, used: &mut BTreeSet<u32>) -> Result<u32> {
    if used.len() >= u32::MAX as usize {
        return Err(crate::error::Hwp5Error::StructureLimitExceeded {
            resource: "문단 인스턴스 id".to_string(),
            limit: u32::MAX as usize,
        });
    }
    loop {
        *counter = counter.wrapping_add(1).max(1);
        if used.insert(*counter) {
            return Ok(*counter);
        }
    }
}

/// Fills zero IDs and, for merges, keeps only the first occurrence of each nonzero ID.
/// The record forest is authoritative for raw-backed controls as well as modeled paragraphs.
fn assign_instance_ids(
    roots: &mut [RecordNode],
    counter: &mut u32,
    used: &mut BTreeSet<u32>,
    seen: &mut BTreeSet<u32>,
    deduplicate: bool,
) -> Result<()> {
    for node in roots {
        if let Some(id) = para_header_instance_id(node) {
            let duplicate = !seen.insert(id);
            if id == 0 || (deduplicate && duplicate) {
                let replacement = next_instance_id(counter, used)?;
                seen.insert(replacement);
                node.data[18..22].copy_from_slice(&replacement.to_le_bytes());
            }
        }
        assign_instance_ids(&mut node.children, counter, used, seen, deduplicate)?;
    }
    Ok(())
}

/// Sets nchars bit31 (chars_flags 0x80) only on the last paragraph of each list. Genuine
/// multiparagraph 5.1.1.0 files set only the last paragraph in sections, table cells and text
/// boxes; others are zero. Recurse into table-cell/text-box paragraph lists.
fn set_last_para_flag(paras: &mut [Paragraph]) {
    let n = paras.len();
    for (i, p) in paras.iter_mut().enumerate() {
        if i + 1 == n {
            p.header.chars_flags |= 0x80;
        } else {
            p.header.chars_flags &= !0x80;
        }
        for ctrl in &mut p.controls {
            match ctrl {
                Control::Table(t) => {
                    for cell in &mut t.cells {
                        set_last_para_flag(&mut cell.paragraphs);
                    }
                }
                Control::Generic(g) => {
                    for list in &mut g.paragraph_lists {
                        set_last_para_flag(&mut list.paragraphs);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Restores missing HWP5 defaults on paragraph shapes from a non-HWP5 source.
/// Merge callers apply this per converted input before grafting, so border-fill
/// references receive that input's offset, then set `para_shape_defaults_prepared`.
/// The input palette must contain fill 2; merge supplies missing canonical no-border fills.
/// Do not apply it to genuine HWP5 shapes: zero can be their original no-border value.
/// The reader already restores `attr1` line-break/grid bits; forcing those would
/// change BREAK_WORD to KEEP_WORD and alter wrapping and pagination.
pub fn ensure_para_shape_defaults(header: &mut hwp_model::DocHeader) {
    for ps in &mut header.para_shapes {
        if ps.line_spacing_old == 0 {
            ps.line_spacing_old = 160;
        }
        if ps.border_fill_id == 0 {
            ps.border_fill_id = 2;
        }
    }
}

/// Rejects direct HWP5 official-numbering layouts that were not established by
/// the Hancom evidence bundle. In particular, one direct definition is proven
/// for each semantic level, not for independently restarted ordered lists.
///
/// This is public so command entry points can reject the input before they
/// allocate a staged output path. The writer calls it again as a defence in
/// depth for library users.
pub fn validate_evidenced_official_numbering(doc: &Document) -> Result<()> {
    collect_evidenced_official_numbering_counts(doc).map(|_| ())
}

/// Validates an official-profile HWP request before output staging. Unlike the
/// generic writer path, the profile contract cannot silently fall back to the
/// default seven-level HWP5 NUMBERING record: all authored ordered-list
/// definitions must match the proven direct record, including `start = 1`.
pub fn validate_official_hwp_numbering(doc: &Document) -> Result<()> {
    if doc.meta.source_format == "hwp5" || !doc.header.numberings.is_empty() {
        return Ok(());
    }

    if doc.header.numbering_levels.is_empty() {
        return Ok(());
    }

    if !crate::numbering::is_official_eight_level_contract(&doc.header.numbering_levels) {
        return Err(
            crate::error::Hwp5Error::UnsupportedOfficialNumberingTopology(
                "official HWP supports only the evidenced ordered-list definition with start = 1"
                    .to_string(),
            ),
        );
    }

    validate_evidenced_official_numbering(doc)
}

/// Counts all numbered paragraphs that the synthetic writer will emit, while
/// also enforcing the direct-record topology. Keeping the traversal beside the
/// writer avoids silently treating text-only control mirrors as output.
fn collect_evidenced_official_numbering_counts(doc: &Document) -> Result<[usize; 8]> {
    if doc.meta.source_format == "hwp5" {
        return Ok([0; 8]);
    }

    let is_direct_contract = doc.header.numberings.is_empty()
        && crate::numbering::is_official_eight_level_contract(&doc.header.numbering_levels);
    if !is_direct_contract {
        // HWPX carries semantic numbering but no raw HWP5 NUMBERING record.
        // Non-official definitions must keep the prior default serializer;
        // reinterpreting them as the private direct contract would be a guess.
        return Ok([0; 8]);
    }

    fn visit_paragraphs(
        paragraphs: &[Paragraph],
        header: &hwp_model::DocHeader,
        counters: &mut [usize; 8],
    ) -> Result<()> {
        for paragraph in paragraphs {
            if let Some(para_shape) = header.para_shapes.get(paragraph.para_shape.0 as usize)
                && para_shape.head_type() == 2
            {
                let semantic_level = para_shape.head_level();
                if semantic_level == 0 || semantic_level > 8 {
                    return Err(crate::error::Hwp5Error::UnsupportedOfficialNumberingRange(
                        "levels outside one through eight are not part of the direct HWP5 contract"
                            .to_string(),
                    ));
                }
                if para_shape.numbering_id as usize != semantic_level as usize - 1 {
                    return Err(crate::error::Hwp5Error::UnsupportedOfficialNumberingTopology(
                        "independent ordered-list definitions at the same semantic level are not proven"
                            .to_string(),
                    ));
                }
                counters[semantic_level as usize - 1] += 1;
            }
            visit_controls(&paragraph.controls, header, counters)?;
        }
        Ok(())
    }

    fn visit_controls(
        controls: &[Control],
        header: &hwp_model::DocHeader,
        counters: &mut [usize; 8],
    ) -> Result<()> {
        for control in controls {
            match control {
                Control::Table(table) => {
                    if let Some(caption) = &table.caption {
                        visit_paragraphs(&caption.paragraphs, header, counters)?;
                    }
                    for cell in &table.cells {
                        visit_paragraphs(&cell.paragraphs, header, counters)?;
                    }
                }
                Control::Picture(picture) => {
                    if let Some(caption) = &picture.caption {
                        visit_paragraphs(&caption.paragraphs, header, counters)?;
                    }
                }
                Control::Generic(generic) if generic.raw_children.is_empty() => {
                    if let Some(caption) = &generic.caption {
                        visit_paragraphs(&caption.paragraphs, header, counters)?;
                    }
                    for list in &generic.paragraph_lists {
                        visit_paragraphs(&list.paragraphs, header, counters)?;
                    }
                }
                Control::SectionDef(_) | Control::Generic(_) => {}
            }
        }
        Ok(())
    }

    let mut counters = [0; 8];
    for section in &doc.sections {
        visit_paragraphs(&section.paragraphs, &doc.header, &mut counters)?;
    }
    for level in [2usize, 6, 8] {
        if counters[level - 1] > 15 {
            return Err(crate::error::Hwp5Error::UnsupportedOfficialNumberingRange(
                format!("level {level} exceeds the evidenced continuation range of 15 items"),
            ));
        }
    }
    Ok(counters)
}

/// Re-encodes a numbered paragraph shape whose semantic level no longer matches its stored
/// attr1 bits 25..=27 (07-hangul-compat-rules K1, #472). The HWP5 reader sets `list_level`
/// from the zero-based bits, so a shape read and left alone already agrees and nothing is
/// touched (the identity gate stays byte-identical, including the tails the probe could not
/// explain). A shape with no `list_level` was not read from HWP5 (a merge or fill grafts
/// markdown/HWPX shapes into an HWP5 target): its bits hold the IR's one-based level. An edit
/// such as `ParaShape::set_head_level` writes a one-based level into the bits together with
/// `list_level`. Both are put back to level-1 (at most 6) in the bits and the same level (at
/// most 7, the deepest measured form) in the tail's final u32, materializing the 16-byte tail
/// `emit_para_shape` would otherwise synthesize with a zero level.
pub fn normalize_numbered_level_encoding(para_shape: &mut ParaShape) {
    if para_shape.head_type() != 2 {
        return;
    }
    let level = para_shape.head_level();
    let zero_based = u32::from(level.saturating_sub(1));
    // Preserve the prior canonical-bit guard for the reader's official-numbering override,
    // and the local decoder's agreeing encodings (raw bits 7 also decode to level 8).
    // Neither case may rewrite an untouched record's unexplained tail.
    if para_shape.list_level.is_some()
        && ((para_shape.attr1 >> 25) & 0x7 == zero_based.min(6)
            || crate::doc_info::semantic_numbered_level(para_shape) == level)
    {
        return;
    }
    para_shape.list_level = Some(level);
    para_shape.attr1 = (para_shape.attr1 & !(0x7 << 25)) | (zero_based.min(6) << 25);
    if para_shape.tail.is_empty() {
        if para_shape.line_spacing <= 0 {
            para_shape.line_spacing = 160;
        }
        para_shape.tail = vec![0; 16];
    }
    if para_shape.tail.len() >= 16 {
        para_shape.tail[12..16].copy_from_slice(&zero_based.min(7).to_le_bytes());
    }
}

/// Encodes numbered-paragraph levels the way genuine Hangul-saved HWP5 files store them, for a
/// document that did not come from HWP5 (07-hangul-compat-rules K1, #472). Such an IR holds
/// the one-based semantic level in attr1 bits 25..=27 (the HWPX reader adds one), while
/// Hancom reads those bits zero-based and draws NUMBERING slot bits+1, so level 0 drew the
/// second slot. The bits become `level - 1` (at most 6, the three-bit ceiling) and the final
/// u32 of the 16-byte PARA_SHAPE tail repeats the level. The deepest measured form is bits 6
/// with tail 7 (the eighth level) and no deeper value is measured, so the tail is bounded to
/// 7: HWPX's levels 9 and 10 have no measured HWP5 encoding and land on the deepest one.
///
/// Skipped for HWP5 sources, whose bits are already on-disk values, and for the official
/// eight-level contract, which [`materialize_evidenced_official_numbering`] encodes itself.
/// Only head type 2 (numbered) is touched: bullets were not measured. Public so the
/// semantic verifier can project an expected document the same way.
pub fn encode_numbered_levels_zero_based(doc: &mut Document) {
    if doc.meta.source_format == "hwp5"
        || (doc.header.numberings.is_empty()
            && crate::numbering::is_official_eight_level_contract(&doc.header.numbering_levels))
    {
        return;
    }
    for para_shape in &mut doc.header.para_shapes {
        if para_shape.head_type() != 2 {
            continue;
        }
        let zero_based = u32::from(para_shape.head_level().saturating_sub(1));
        para_shape.list_level = Some(para_shape.head_level());
        para_shape.attr1 = (para_shape.attr1 & !(0x7 << 25)) | (zero_based.min(6) << 25);
        if para_shape.tail.is_empty() {
            // Same 16-byte layout emit_para_shape synthesizes for an empty tail: attr2, attr3,
            // line spacing (the emitter rewrites tail[8..12] from `line_spacing`), list level.
            if para_shape.line_spacing <= 0 {
                para_shape.line_spacing = 160;
            }
            para_shape.tail = vec![0; 16];
        }
        if para_shape.tail.len() >= 16 {
            para_shape.tail[12..16].copy_from_slice(&zero_based.min(7).to_le_bytes());
        }
    }
}

/// Materializes only the complete direct NUMBERING contract observed in the
/// private Hancom-saved source. A source-free document that merely contains a
/// circled-Hangul level remains on the established default serializer unless
/// every definition carries the exact eight-level official signature.
fn materialize_evidenced_official_numbering(doc: &mut Document) -> Result<()> {
    if doc.meta.source_format == "hwp5" {
        return Ok(());
    }

    let is_direct_contract = doc.header.numberings.is_empty()
        && crate::numbering::is_official_eight_level_contract(&doc.header.numbering_levels);
    if !is_direct_contract {
        // HWPX carries semantic numbering but no raw HWP5 NUMBERING record.
        // Non-official definitions must keep the prior default serializer;
        // reinterpreting them as the private direct contract would be a guess.
        return Ok(());
    }

    collect_evidenced_official_numbering_counts(doc)?;

    for para_shape in &mut doc.header.para_shapes {
        if para_shape.head_type() != 2 {
            continue;
        }
        let semantic_level = para_shape.head_level();
        let raw_level = u32::from(semantic_level.saturating_sub(1).min(6));
        para_shape.attr1 = (para_shape.attr1 & !(0x7 << 25)) | (raw_level << 25);
        // The bits are now on-disk values: record the semantic level so the emit-time
        // normalization (#472) reads them as encoded instead of re-encoding them.
        para_shape.list_level = Some(semantic_level);
        para_shape.border_fill_id = 1;
        // The native HWP5 contract repeats the list level in the final u32 of
        // PARA_SHAPE.  The attr1 bits are enough for our semantic reader, but
        // Hancom uses this persisted binding to select the matching
        // NUMBERING slot.  A zero here makes every generated paragraph use
        // level one while all definitions still look valid to an internal
        // reread.  Keep the observed 16-byte tail (attr2, attr3, line spacing,
        // zero-based list level) so the HWP5 writer emits the direct evidence
        // rather than a guessed alternate encoding.
        let mut tail = vec![0; 16];
        let line_spacing = if para_shape.line_spacing > 0 {
            para_shape.line_spacing
        } else {
            160
        };
        // emit_para_shape now patches tail[8..12] from this field, so the fallback has to land
        // in the field too. Leaving line_spacing at 0 here would make the emitter write that 0
        // back over the 160 this tail carries.
        para_shape.line_spacing = line_spacing;
        tail[8..12].copy_from_slice(&u32::try_from(line_spacing).unwrap_or(160).to_le_bytes());
        tail[12..16].copy_from_slice(&u32::from(semantic_level - 1).to_le_bytes());
        para_shape.tail = tail;
    }
    doc.header.numberings = (0..8)
        .map(|_| RawEntry {
            data: crate::numbering::official_eight_level_data(),
            children: Vec::new(),
        })
        .collect();
    Ok(())
}

/// Whether any pictures lack writable HWP5 SHAPE_COMPONENT records.
fn needs_normalize(doc: &Document) -> bool {
    fn para_has(para: &Paragraph) -> bool {
        para.controls.iter().any(|c| match c {
            Control::Picture(p) => p.extras.is_empty(),
            Control::Table(t) => t.cells.iter().flat_map(|c| &c.paragraphs).any(para_has),
            // HWP5-origin controls with raw_children emit the original tree without normalization.
            // Only controls missing both raw_children and data (unsynthesizable HWPX/Markdown
            // input) are removed.
            Control::Generic(g) => {
                (g.data.is_empty() && g.ctrl_id != *b"cold" && g.raw_children.is_empty())
                    || g.paragraph_lists
                        .iter()
                        .flat_map(|l| &l.paragraphs)
                        .any(para_has)
            }
            _ => false,
        })
    }
    doc.sections
        .iter()
        .flat_map(|s| &s.paragraphs)
        .any(para_has)
}

// HWP5 shape-record synthesis for HWPX/Markdown images.
//
// HWPX hp:pic becomes an IR Picture with empty extras. HWP5 stores it as a gso CTRL_HEADER ->
// SHAPE_COMPONENT -> SHAPE_COMPONENT_PICTURE tree, a BIN_DATA item, and a BinData stream. Use the
// genuine work_report.hwp picture records as templates, patching only dimensions, corners and
// BinItem ID (the identity transform is size-independent). Populated extras keep synthesized
// pictures from being stripped.

/// 34-byte header/footer (head/foot) LIST_HEADER template from the genuine work_report.hwp footer.
/// Patch only paraCount at offset 0 (u16) to the actual count. HWPX-origin headers have no
/// LIST_HEADER; this template lets Hangul draw the header and its logo.
const HEADER_LIST_HEADER_TEMPLATE: &str =
    "01000000400000003ebc0000130b0000000000000000000000000000000000000000";
/// SHAPE_COMPONENT 196B. width@20/28, height@24/32.
const SHAPE_COMPONENT_TEMPLATE: &str = "636970246369702400000000000000000000010038220000ec04000038220000ec0400000000082400001c110000760200000100000000000000f03f000000000000000000000000000000000000000000000000000000000000f03f0000000000000000000000000000f03f000000000000000000000000000000000000000000000000000000000000f03f0000000000000000000000000000f03f000000000000000000000000000000000000000000000000000000000000f03f0000000000000000";
fn hex_to_bytes(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("템플릿 hex"))
        .collect()
}

/// Reads embedded PNG/JPEG/GIF/BMP pixel dimensions (width, height) for the original natural size
/// in picture clipping and picture_effect.
fn image_pixel_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() >= 24 && data.starts_with(b"\x89PNG\r\n\x1a\n") {
        // IHDR: width@16, height@20 (big-endian)
        let w = u32::from_be_bytes(data[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(data[20..24].try_into().ok()?);
        return Some((w, h));
    }
    if data.len() >= 10 && data.starts_with(b"GIF") {
        // Logical screen width at 6 and height at 8 (little-endian u16).
        let w = u16::from_le_bytes([data[6], data[7]]) as u32;
        let h = u16::from_le_bytes([data[8], data[9]]) as u32;
        return Some((w, h));
    }
    if data.len() >= 26 && data.starts_with(b"BM") {
        // BITMAPINFOHEADER: width@18, height@22 (little-endian i32)
        let w = i32::from_le_bytes(data[18..22].try_into().ok()?);
        let h = i32::from_le_bytes(data[22..26].try_into().ok()?);
        return Some((w.unsigned_abs(), h.unsigned_abs()));
    }
    if data.len() >= 4 && data[0] == 0xFF && data[1] == 0xD8 {
        // JPEG: height/width in SOFn markers (C0..CF except C4/C8/CC), big-endian.
        let mut i = 2;
        while i + 9 < data.len() {
            if data[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = data[i + 1];
            if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC
            {
                let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                return Some((w, h));
            }
            let seg_len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
            if seg_len < 2 {
                break;
            }
            i += 2 + seg_len;
        }
        return None;
    }
    None
}

/// Pixel dimensions -> natural HWPUNIT dimensions at 96 DPI: px * 7200/96 = px * 75.
fn natural_size_hwpunit(px: (u32, u32)) -> (i32, i32) {
    let conv = |v: u32| (v as i64 * 7200 / 96) as i32;
    (conv(px.0), conv(px.1))
}

/// Patches the genuine template's dimensions and BinItem ID into picture extras: SHAPE_COMPONENT
/// plus its SHAPE_COMPONENT_PICTURE child.
///
/// - width/height: display dimensions in HWPUNIT, the final gso box/shape size.
/// - natural: original-image natural size (pixels * 7200/96). The clipping rectangle (0,0,natural)
///   shows the whole image. Using display size for clipping shows only a small upper-left crop
///   (e.g. 8196/150000, about 5%), nearly hiding the picture in Hangul. Genuine Hangul always uses
///   natural size.
/// - bin_id: BIN_DATA item ID. pic_inst: the picture object's unique instance_id.
fn build_picture_extras(
    width: i32,
    height: i32,
    natural: (i32, i32),
    bin_id: u16,
    pic_inst: u32,
) -> Vec<OpaqueRecord> {
    let w = width.to_le_bytes();
    let h = height.to_le_bytes();
    let (nw, nh) = natural;
    let mut sc = hex_to_bytes(SHAPE_COMPONENT_TEMPLATE);
    sc[20..24].copy_from_slice(&w); // Final width.
    sc[24..28].copy_from_slice(&h); // Final height.
    sc[28..32].copy_from_slice(&w); // Initial width, equal to final width under the identity matrix.
    sc[32..36].copy_from_slice(&h); // Initial height.

    // SHAPE_COMPONENT_PICTURE, genuine 5.1.x layout (91 bytes): border (12), four display-rectangle
    // corners (32), clip (16: 0,0,natural), inner margins (8), picture (5: brightness, contrast,
    // effect, BinItem ID), border transparency (1),
    // instance_id(4) + picture_effect(13: flags4=0 + natural_w4 + natural_h4 + 1)
    let mut pic = Vec::with_capacity(91);
    pic.extend_from_slice(&[0u8; 12]); // Border.
    for &(x, y) in &[(0, 0), (width, 0), (width, height), (0, height)] {
        pic.extend_from_slice(&i32::to_le_bytes(x));
        pic.extend_from_slice(&i32::to_le_bytes(y));
    }
    pic.extend_from_slice(&0i32.to_le_bytes()); // Clip left.
    pic.extend_from_slice(&0i32.to_le_bytes()); // Clip top.
    pic.extend_from_slice(&nw.to_le_bytes()); // Clip right = natural width.
    pic.extend_from_slice(&nh.to_le_bytes()); // Clip bottom = natural height.
    pic.extend_from_slice(&[0u8; 8]); // Inner margins.
    pic.push(0); // Brightness.
    pic.push(0); // Contrast.
    pic.push(0); // Effect.
    pic.extend_from_slice(&bin_id.to_le_bytes()); // BinItem ID
    pic.push(0); // Border transparency.
    pic.extend_from_slice(&pic_inst.to_le_bytes()); // Unique instance_id.
    pic.extend_from_slice(&0u32.to_le_bytes()); // picture_effect flags=0 (no effects).
    pic.extend_from_slice(&nw.to_le_bytes()); // picture_effect natural width.
    pic.extend_from_slice(&nh.to_le_bytes()); // picture_effect natural height.
    pic.push(0); // picture_effect reserved

    vec![OpaqueRecord {
        tag: tag::SHAPE_COMPONENT,
        data: sc,
        children: vec![OpaqueRecord {
            tag: tag::SHAPE_COMPONENT_PICTURE,
            data: pic,
            children: Vec::new(),
        }],
    }]
}

/// Applies IR crop, brightness, contrast, and rotation to synthesized picture
/// extras. This runs only for newly materialized extras, such as HWPX-origin
/// pictures; original HWP5 extras remain on the lossless raw round-trip path.
/// [`is_materialized_generated_picture`] applies the same patch so post-write
/// verification compares the exact materialized representation.
fn patch_picture_extras_fx(
    extras: &mut [OpaqueRecord],
    crop: Option<[f32; 4]>,
    brightness: i8,
    contrast: i8,
    rotation: Option<f32>,
) {
    let default = crop.is_none() && brightness == 0 && contrast == 0 && rotation.is_none();
    if default {
        return;
    }
    let Some(sc) = extras.first_mut() else { return };
    if sc.tag != tag::SHAPE_COMPONENT {
        return;
    }
    // Replace the synthesized 196-byte template's rotation matrix at byte 148.
    if let Some(deg) = rotation
        && sc.data.len() >= 196
    {
        let t = (deg as f64).to_radians();
        let (sn, cs) = (t.sin(), t.cos());
        // [a b c; d e f], clockwise-positive in y-down coordinates. This is
        // the inverse convention used by gso_rotation_deg.
        for (o, v) in [(148, cs), (156, -sn), (172, sn), (180, cs)] {
            sc.data[o..o + 8].copy_from_slice(&v.to_le_bytes());
        }
    }
    let Some(p) = sc.children.first_mut() else {
        return;
    };
    if p.tag != tag::SHAPE_COMPONENT_PICTURE || p.data.len() < 73 {
        return;
    }
    if let Some([l, t, r, b]) = crop {
        for (o, v) in [(44, l), (48, t), (52, r), (56, b)] {
            p.data[o..o + 4].copy_from_slice(&(v as i32).to_le_bytes());
        }
    }
    p.data[68] = brightness.clamp(-100, 100) as u8;
    p.data[69] = contrast.clamp(-100, 100) as u8;
}

/// Returns true only for the exact Picture scaffolding synthesized by this
/// writer from an IR picture whose `common_data`/`extras` were empty.
///
/// The predicate is used by post-write semantic verification. It recomputes
/// natural dimensions from the referenced media bytes and checks every common
/// header and shape byte except the writer-assigned instance/storage IDs. A
/// foreign or edited payload therefore remains part of the semantic digest.
pub fn is_materialized_generated_picture(picture: &hwp_model::Picture, media: &[u8]) -> bool {
    let common = &picture.common_data;
    if common.len() < 42 {
        return false;
    }
    let description_units = picture
        .description
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(str::encode_utf16)
        .map(Iterator::collect::<Vec<_>>)
        .unwrap_or_default();
    let Ok(description_len) = u16::try_from(description_units.len()) else {
        return false;
    };
    if common.len() != 42 + description_units.len() * 2 {
        return false;
    }
    let attr: u32 = if picture.treat_as_char {
        0x042a_6001
    } else {
        (0x040a_6000 & !(1 << 13)) | (2 << 3) | (3 << 8) | (5 << 21)
    };
    let instance_id = u32::from_le_bytes(common[32..36].try_into().expect("checked length"));
    if !(0x3000_0000..0x3010_0000).contains(&instance_id) {
        return false;
    }
    let mut expected_common = Vec::with_capacity(common.len());
    expected_common.extend_from_slice(&attr.to_le_bytes());
    expected_common.extend_from_slice(&picture.vert_offset.to_le_bytes());
    expected_common.extend_from_slice(&picture.horz_offset.to_le_bytes());
    expected_common.extend_from_slice(&picture.width.0.to_le_bytes());
    expected_common.extend_from_slice(&picture.height.0.to_le_bytes());
    expected_common.extend_from_slice(&picture.z_order.to_le_bytes());
    expected_common.extend_from_slice(&[0u8; 8]);
    expected_common.extend_from_slice(&instance_id.to_le_bytes());
    expected_common.extend_from_slice(&0u32.to_le_bytes());
    expected_common.extend_from_slice(&description_len.to_le_bytes());
    for unit in description_units {
        expected_common.extend_from_slice(&unit.to_le_bytes());
    }
    if common != &expected_common {
        return false;
    }
    let hwp_model::BinRef::Id(bin_id) = picture.bin_ref else {
        return false;
    };
    let natural = image_pixel_size(media)
        .map(natural_size_hwpunit)
        .unwrap_or((picture.width.0, picture.height.0));
    let mut expected = build_picture_extras(
        picture.width.0,
        picture.height.0,
        natural,
        bin_id.0,
        instance_id ^ 0x0010_0000,
    );
    patch_picture_extras_fx(
        &mut expected,
        picture.crop,
        picture.brightness,
        picture.contrast,
        picture.rotation,
    );
    picture.extras == expected
}

// Safe degradation of HWPX-origin shapes/text boxes.
//
// Structured shapes produced by the HWPX reader (GenericControl.gso_shapes) have no HWP5
// SHAPE_COMPONENT records. Attempt 22 patched genuine templates to synthesize them, but Hangul
// hardware checks reported corruption: using a 252-byte line template for a rectangle shifted
// border/fill sections by 13 bytes relative to the genuine 239-byte layout, with attr/desc
// mismatches. Without self-verifiable Hangul acceptance, resynthesis risks corruption; degrade
// safely instead. Hoist text-box paragraphs into the body to preserve text, and leave textless
// decorations for strip_unwritable_pictures to remove. Correct per-kind synthesis from genuine
// templates remains follow-up work requiring repeated owner hardware checks.

/// Hoists HWPX-origin text-box paragraphs into the body, preserving text without the shape wrapper.
/// Leave textless decorations for stripping. Recurse through body, table-cell and nested paragraph
/// lists.
fn degrade_hwpx_gso(doc: &mut Document, warnings: &mut WriteReport) {
    for section in &mut doc.sections {
        degrade_hwpx_gso_list(&mut section.paragraphs, warnings);
    }
}

fn degrade_hwpx_gso_list(paras: &mut Vec<Paragraph>, warnings: &mut WriteReport) {
    // Recurse first through nested containers: table cells and every Generic paragraph list.
    for p in paras.iter_mut() {
        for c in &mut p.controls {
            match c {
                Control::Table(t) => {
                    for cell in &mut t.cells {
                        degrade_hwpx_gso_list(&mut cell.paragraphs, warnings);
                    }
                }
                Control::Generic(g) => {
                    for l in &mut g.paragraph_lists {
                        degrade_hwpx_gso_list(&mut l.paragraphs, warnings);
                    }
                }
                _ => {}
            }
        }
    }
    // Expand HWPX-origin gso text-box paragraphs in this list into the body after the host
    // paragraph.
    let mut out: Vec<Paragraph> = Vec::with_capacity(paras.len());
    for mut p in std::mem::take(paras) {
        let mut hoisted: Vec<Paragraph> = Vec::new();
        let mut removed: Vec<u32> = Vec::new();
        for (i, c) in p.controls.iter().enumerate() {
            if let Control::Generic(g) = c
                && g.ctrl_id != *b"gso "
                && !g.gso_shapes.is_empty()
                && g.raw_children.is_empty()
                && g.data.is_empty()
                && !g.paragraph_lists.is_empty()
            {
                for l in &g.paragraph_lists {
                    hoisted.extend(l.paragraphs.iter().cloned());
                }
                removed.push(i as u32);
                warnings.warning(
                    "hwpx 출신 글상자를 본문 텍스트로 저하(도형 래퍼 생략 — 한글 수용)".to_string(),
                );
            }
        }
        if !removed.is_empty() {
            let mut kept = Vec::with_capacity(p.controls.len());
            for (i, c) in std::mem::take(&mut p.controls).into_iter().enumerate() {
                if !removed.contains(&(i as u32)) {
                    kept.push(c);
                }
            }
            p.controls = kept;
            // Remove the deleted controls' ExtCtrl characters and remap remaining ctrl_index
            // values, as stripping does.
            p.chars.retain(|ch| match ch {
                HwpChar::ExtCtrl {
                    ctrl_index: Some(i),
                    ..
                } => !removed.contains(i),
                _ => true,
            });
            for ch in &mut p.chars {
                if let HwpChar::ExtCtrl {
                    ctrl_index: Some(i),
                    ..
                } = ch
                {
                    let shift = removed.iter().filter(|r| **r < *i).count() as u32;
                    *i -= shift;
                }
            }
        }
        out.push(p);
        out.append(&mut hoisted);
    }
    *paras = out;
}

/// Recursively checks for HWPX-origin structured shapes: non-gso ctrl_id plus gso_shapes.
fn has_synthesizable_gso(doc: &Document) -> bool {
    fn in_para(p: &Paragraph) -> bool {
        p.controls.iter().any(|c| match c {
            Control::Generic(g) => {
                (g.ctrl_id != *b"gso " && !g.gso_shapes.is_empty())
                    || g.paragraph_lists
                        .iter()
                        .any(|l| l.paragraphs.iter().any(in_para))
            }
            Control::Table(t) => t
                .cells
                .iter()
                .any(|cell| cell.paragraphs.iter().any(in_para)),
            _ => false,
        })
    }
    doc.sections
        .iter()
        .any(|s| s.paragraphs.iter().any(in_para))
}

/// Recursively checks for inserted pictures with empty extras and no shape records. Enable picture
/// synthesis even for HWP5 origin, or strip_unwritable_pictures removes them.
fn has_synthesizable_picture(doc: &Document) -> bool {
    fn in_para(p: &Paragraph) -> bool {
        p.controls.iter().any(|c| match c {
            Control::Picture(pic) => pic.extras.is_empty(),
            Control::Table(t) => t
                .cells
                .iter()
                .any(|cell| cell.paragraphs.iter().any(in_para)),
            Control::Generic(g) => g
                .paragraph_lists
                .iter()
                .any(|l| l.paragraphs.iter().any(in_para)),
            _ => false,
        })
    }
    doc.sections.iter().flat_map(|s| &s.paragraphs).any(in_para)
}

/// Resolves a `BinRef` to the index into `bin_names` (and the parallel `bin_streams`) it
/// actually names. `BinRef::Id` mirrors `Document::resolve_bin`'s naming rule
/// (`BIN{storage_id:04X}.{ext}` from `bin_data[id-1]`) and matches it against each name's
/// trailing path component case-insensitively, since a stream name may or may not carry a
/// `BinData/` prefix. `BinRef::ItemRef` is resolved through the package manifest first, the same
/// rule `Document::bin_stream_index` applies: an id the manifest lists names its href's stream or
/// nothing (`id="logo" href="BinData/p1.png"` need not look like its file name). An unlisted id
/// uses the model's exact name, path-suffix and filename-stem rules. There is no substring or
/// index-0 fallback: an unresolvable reference returns `None`, which the caller reports as a
/// typed loss instead of silently substituting an unrelated stream.
fn bin_stream_index(
    bin_names: &[String],
    bin_data: &[hwp_model::BinDataItem],
    manifest: &[(String, String)],
    bin_ref: &hwp_model::BinRef,
) -> Option<usize> {
    fn trailing(name: &str) -> &str {
        name.rsplit('/').next().unwrap_or(name)
    }
    match bin_ref {
        hwp_model::BinRef::Id(id) => {
            let item = bin_data.get((id.0 as usize).checked_sub(1)?)?;
            let storage_id = item.storage_id?;
            let ext = item.extension.as_deref().unwrap_or("");
            let name = format!("BIN{storage_id:04X}.{ext}");
            bin_names
                .iter()
                .position(|n| trailing(n).eq_ignore_ascii_case(&name))
        }
        hwp_model::BinRef::ItemRef(item) => {
            if let Some((_, href)) = manifest.iter().find(|(id, _)| id == item) {
                return bin_names.iter().position(|name| name == href);
            }
            bin_names.iter().position(|name| {
                name == item
                    || name.ends_with(&format!("/{item}"))
                    || name
                        .rsplit('/')
                        .next()
                        .and_then(|file| file.split('.').next())
                        .is_some_and(|stem| stem == item)
            })
        }
    }
}

/// Populates shape records and BIN_DATA items for HWPX-origin images in synthesized documents.
fn synthesize_pictures(doc: &mut Document, warnings: &mut WriteReport) {
    let bin_names: Vec<String> = doc.bin_streams.iter().map(|b| b.name.clone()).collect();
    if bin_names.is_empty() {
        return;
    }
    // Snapshot header.bin_data before the mutable section walk below, so each
    // picture's BinRef can be resolved against the BIN_DATA table it actually
    // named without borrowing `doc` immutably and mutably at once.
    let bin_data_snapshot = doc.header.bin_data.clone();
    let manifest_snapshot = doc.hwpx_bin_manifest.clone();
    let bin_index = |r: &hwp_model::BinRef| {
        bin_stream_index(&bin_names, &bin_data_snapshot, &manifest_snapshot, r)
    };
    // Original pixel dimensions per image, used for clipping and picture_effect natural sizes.
    let bin_dims: Vec<Option<(u32, u32)>> = doc
        .bin_streams
        .iter()
        .map(|b| image_pixel_size(&b.data))
        .collect();
    let mut next_id: u16 = doc
        .header
        .bin_data
        .iter()
        .filter_map(|b| b.storage_id)
        .max()
        .unwrap_or(0)
        + 1;
    // Assigned storage_id per bin_stream, reused when several pictures share one image.
    let mut assigned: Vec<Option<u16>> = vec![None; bin_names.len()];
    let mut new_items: Vec<hwp_model::BinDataItem> = Vec::new();
    // Pairs of bin_stream index and new name.
    let mut renames: Vec<(usize, String)> = Vec::new();
    // Unique instance_id per gso object; Hangul may treat equal IDs as one object and ignore
    // duplicates.
    // A document that already carries synthesized pictures (a re-read output of this writer)
    // holds ids from this same range, so start past the highest one instead of reusing it.
    let mut inst = InstanceIds::new(doc);

    // Fill empty HWPX-origin header/footer LIST_HEADER values from section PageDef dimensions:
    // textWidth=body width (paper minus side margins), textHeight=header/footer margin. The
    // previous work_report-derived template (textHeight=2835, list_attr=0x40) gave the wrong header
    // height. Leave HWP5 header_data untouched when already populated.
    for section in &mut doc.sections {
        if let Some(pg) = section_page_def(section) {
            let text_width = pg.width.0 - pg.margin_left.0 - pg.margin_right.0;
            for para in &mut section.paragraphs {
                fill_head_foot_list_header(
                    para,
                    text_width,
                    pg.margin_header.0,
                    pg.margin_footer.0,
                );
            }
        }
    }

    for section in &mut doc.sections {
        for para in &mut section.paragraphs {
            synth_pictures_para(
                para,
                &bin_names,
                &bin_dims,
                &bin_index,
                &mut assigned,
                &mut next_id,
                &mut inst,
                &mut new_items,
                &mut renames,
                warnings,
            );
        }
    }
    for item in new_items {
        doc.header.bin_data.push(item);
    }
    for (idx, new_name) in renames {
        doc.bin_streams[idx].name = new_name;
    }
}

/// Allocator for the gso instance ids this writer synthesizes for pictures.
///
/// Ids come from `0x3000_0000..0x3010_0000`. An id is handed out only when neither it nor the
/// id derived from it for the picture record (`id ^ 0x0010_0000`, which lands in
/// `0x3010_0000..0x3020_0000`) is already held by a picture, table or gso container of the
/// document, wherever that id sits. A re-read output of this writer holds ids from this same
/// range, so a plain counter would repeat them. The counter only moves forward and every
/// candidate has bit 20 clear, so an id handed out is never a later candidate and never the
/// derived value of one: only ids the document already holds need avoiding. When the range is
/// used up the allocator returns `None` instead of stepping past the range edge into another
/// object's id; the caller leaves that picture unsynthesized and the existing strip step
/// reports it as removed.
struct InstanceIds {
    used: BTreeSet<u32>,
    next: u32,
    end: u32,
}

impl InstanceIds {
    const BASE: u32 = 0x3000_0000;
    const END: u32 = 0x3010_0000;
    const PICTURE_RECORD_BIT: u32 = 0x0010_0000;

    fn new(doc: &Document) -> Self {
        Self::with_range(doc, Self::BASE, Self::END)
    }

    fn with_range(doc: &Document, base: u32, end: u32) -> Self {
        fn common_id(common: &[u8]) -> Option<u32> {
            common
                .get(32..36)
                .map(|b| u32::from_le_bytes(b.try_into().expect("slice len checked")))
        }
        fn walk(para: &Paragraph, used: &mut BTreeSet<u32>) {
            for control in &para.controls {
                match control {
                    Control::Picture(p) => {
                        used.extend(common_id(&p.common_data));
                        if let Some(caption) = &p.caption {
                            caption.paragraphs.iter().for_each(|q| walk(q, used));
                        }
                    }
                    Control::Table(t) => {
                        used.extend(common_id(&t.common_data));
                        if let Some(caption) = &t.caption {
                            caption.paragraphs.iter().for_each(|q| walk(q, used));
                        }
                        for cell in &t.cells {
                            cell.paragraphs.iter().for_each(|q| walk(q, used));
                        }
                    }
                    Control::Generic(g) => {
                        if g.ctrl_id == *b"gso " {
                            used.extend(common_id(&g.data));
                        }
                        for list in &g.paragraph_lists {
                            list.paragraphs.iter().for_each(|q| walk(q, used));
                        }
                        // The reader keeps a shape's caption apart from its paragraph lists.
                        if let Some(caption) = &g.caption {
                            caption.paragraphs.iter().for_each(|q| walk(q, used));
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut used = BTreeSet::new();
        for section in &doc.sections {
            section.paragraphs.iter().for_each(|p| walk(p, &mut used));
        }
        Self {
            used,
            next: base,
            end,
        }
    }

    /// The next unused id, or `None` once the range holds no more.
    fn next(&mut self) -> Option<u32> {
        while self.next < self.end {
            let candidate = self.next;
            self.next += 1;
            let derived = candidate ^ Self::PICTURE_RECORD_BIT;
            if self.used.contains(&candidate) || self.used.contains(&derived) {
                continue;
            }
            return Some(candidate);
        }
        None
    }
}

/// Finds the section PageDef in its SectionDef control.
fn section_page_def(section: &Section) -> Option<hwp_model::PageDef> {
    section
        .paragraphs
        .iter()
        .flat_map(|p| &p.controls)
        .find_map(|c| match c {
            Control::SectionDef(sd) => sd.page,
            _ => None,
        })
}

/// Fills empty head/foot GenericControl LIST_HEADER values from section dimensions: textWidth=body
/// width, textHeight=header/footer margin. Leave already populated HWP5-origin header_data
/// untouched.
fn fill_head_foot_list_header(para: &mut Paragraph, text_width: i32, head_h: i32, foot_h: i32) {
    for control in &mut para.controls {
        let Control::Generic(g) = control else {
            continue;
        };
        let is_head = g.ctrl_id == *b"head";
        let is_foot = g.ctrl_id == *b"foot";
        if !(is_head || is_foot) || !g.raw_children.is_empty() {
            continue;
        }
        let text_height = if is_head { head_h } else { foot_h };
        for list in &mut g.paragraph_lists {
            if !list.header_data.is_empty() {
                continue;
            }
            let mut lh = hex_to_bytes(HEADER_LIST_HEADER_TEMPLATE);
            let npara = list.paragraphs.len().max(1) as u16;
            lh[0..2].copy_from_slice(&npara.to_le_bytes());
            lh[4..8].copy_from_slice(&0u32.to_le_bytes()); // list_attr=0, as in genuine headers.
            lh[8..12].copy_from_slice(&text_width.to_le_bytes());
            lh[12..16].copy_from_slice(&text_height.to_le_bytes());
            list.header_data = lh;
        }
    }
}

/// Synthesizes shape records for empty-extras pictures in a paragraph, recursing through table
/// cells/text boxes.
#[allow(clippy::too_many_arguments)]
fn synth_pictures_para(
    para: &mut Paragraph,
    bin_names: &[String],
    bin_dims: &[Option<(u32, u32)>],
    bin_index: &dyn Fn(&hwp_model::BinRef) -> Option<usize>,
    assigned: &mut [Option<u16>],
    next_id: &mut u16,
    inst: &mut InstanceIds,
    new_items: &mut Vec<hwp_model::BinDataItem>,
    renames: &mut Vec<(usize, String)>,
    warnings: &mut WriteReport,
) {
    for control in &mut para.controls {
        match control {
            Control::Picture(p) if p.extras.is_empty() => {
                // Resolve the stream this picture actually references: BinRef::Id
                // through header.bin_data's storage_id naming, BinRef::ItemRef through the
                // manifest or exact model name rules. No index-0 fallback - an unresolvable
                // reference is a reported loss below, never a silently substituted
                // image (a different logo, seal, signature or photograph).
                let pick = bin_index(&p.bin_ref);
                let Some(idx) = pick else {
                    warnings.loss(
                        PreservationCode::BinaryAssetRemoved,
                        PreservationResourceKind::BinaryAsset,
                        PreservationDisposition::Removed,
                        1,
                    );
                    continue;
                };
                // Take the instance id before minting a storage id: when the synthesized range is
                // used up the picture is left as is (the strip step below reports it removed), and
                // no BinData item or renamed stream may be left behind for it.
                let Some(inst_id) = inst.next() else {
                    continue;
                };
                // Reuse storage_id for a shared stream already assigned one.
                let sid = match assigned[idx] {
                    Some(s) => s,
                    None => {
                        let s = *next_id;
                        *next_id += 1;
                        assigned[idx] = Some(s);
                        let ext = bin_names[idx]
                            .rsplit('.')
                            .next()
                            .filter(|e| !e.contains('/'))
                            .unwrap_or("png")
                            .to_ascii_lowercase();
                        new_items.push(hwp_model::BinDataItem {
                            attr: 1, // Embedded.
                            storage_id: Some(s),
                            extension: Some(ext.clone()),
                            ..Default::default()
                        });
                        renames.push((idx, format!("BinData/BIN{s:04X}.{ext}")));
                        s
                    }
                };
                // Synthesize the 40-byte gso common attributes from placement. Genuine university
                // HWP samples measured inline (treat-as-character)=0x042a6001, floating=0x040a6000.
                // Assign unique instance_id values per object. A fixed template incorrectly placed
                // floating logos inline, hiding or misplacing them in Hangul.
                //
                // Floating objects use paragraph-relative placement (vertRelTo/horzRelTo=PARA).
                // 0x040a6000 uses relTo=PAPER, the paper's upper-left origin; applying insert_seal
                // offsets computed within an anchor paragraph jumps the seal to that corner (D2
                // non-overlap defect). Set PARA bits (vertRelTo=2<<3, horzRelTo=3<<8) to match HWPX
                // D1 PARA placement and offset semantics.
                //
                // Set wrapping (bits21-23) to IN_FRONT_OF_TEXT=5. The sample's 0x040a6000 wrap=0
                // (SQUARE) pushes body text away and prevents overlap (section 4.3.9.1, D2 hardware
                // defect: seal shifted right). Cross-checked the value scheme with genuine annual
                // attr 0x046a4000 -> wrap=3 (TOP_AND_BOTTOM). Also clear bit13 (restrict to body
                // area), since the specification says it overrides overlap-allowed bit14. This
                // mirrors HWPX floating-picture output fixed to IN_FRONT_OF_TEXT.
                let attr: u32 = if p.treat_as_char {
                    0x042a_6001
                } else {
                    (0x040a_6000 & !(1 << 13)) | (2 << 3) | (3 << 8) | (5 << 21) // = 0x04aa4310
                };
                // Preserve offsets even for treat_as_char. Genuine body logos retain
                // vertOffset=68401 with treatAsChar=1; zeroing it moves the logo to the upper-left
                // and overlaps body text.
                let voff: i32 = p.vert_offset;
                let hoff: i32 = p.horz_offset;
                let mut common = Vec::with_capacity(40);
                common.extend_from_slice(&attr.to_le_bytes());
                common.extend_from_slice(&voff.to_le_bytes());
                common.extend_from_slice(&hoff.to_le_bytes());
                common.extend_from_slice(&p.width.0.to_le_bytes());
                common.extend_from_slice(&p.height.0.to_le_bytes());
                common.extend_from_slice(&p.z_order.to_le_bytes());
                common.extend_from_slice(&0u32.to_le_bytes()); // Outer margins, left/top.
                common.extend_from_slice(&0u32.to_le_bytes()); // Outer margins, right/bottom.
                common.extend_from_slice(&inst_id.to_le_bytes());
                common.extend_from_slice(&0u32.to_le_bytes()); // Prevent page splitting (split).
                // Object description BSTR, required for CommonControl >=5.0.0.5. Even an empty
                // description must include its u16 length=0; genuine Hangul always writes it.
                append_gso_description(&mut common, p.description.as_deref(), warnings);
                p.common_data = common;
                // Natural image dimensions (pixels * 7200/96). If unreadable, fall back to display
                // size (older behavior); clipping may show only part of the image, but this is the
                // next-best choice.
                let natural = bin_dims
                    .get(idx)
                    .copied()
                    .flatten()
                    .map(natural_size_hwpunit)
                    .unwrap_or((p.width.0, p.height.0));
                // Picture-object instance_id (SCPIC) must differ from the gso CTRL instance_id.
                // Derive it by toggling a unique bit, retaining uniqueness.
                let pic_inst = inst_id ^ 0x0010_0000;
                p.extras = build_picture_extras(p.width.0, p.height.0, natural, sid, pic_inst);
                patch_picture_extras_fx(
                    &mut p.extras,
                    p.crop,
                    p.brightness,
                    p.contrast,
                    p.rotation,
                );
                p.bin_ref = hwp_model::BinRef::Id(hwp_model::BinDataId(sid));
            }
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for cp in &mut cell.paragraphs {
                        synth_pictures_para(
                            cp, bin_names, bin_dims, bin_index, assigned, next_id, inst, new_items,
                            renames, warnings,
                        );
                    }
                }
            }
            Control::Generic(g) => {
                // HWPX-origin head/foot controls lack LIST_HEADER and would be stripped with their
                // pictures. Populate the genuine LIST_HEADER template, patching paraCount to the
                // actual count. The HWPX reader fills 8-byte data (page scope plus ID); if absent,
                // use at least 4 bytes to prevent stripping.
                if (g.ctrl_id == *b"head" || g.ctrl_id == *b"foot") && g.raw_children.is_empty() {
                    if g.data.is_empty() {
                        g.data = vec![0, 0, 0, 0];
                    }
                    for list in &mut g.paragraph_lists {
                        if list.header_data.is_empty() {
                            let mut lh = hex_to_bytes(HEADER_LIST_HEADER_TEMPLATE);
                            let npara = list.paragraphs.len().max(1) as u16;
                            lh[0..2].copy_from_slice(&npara.to_le_bytes());
                            list.header_data = lh;
                        }
                    }
                }
                for list in &mut g.paragraph_lists {
                    for lp in &mut list.paragraphs {
                        synth_pictures_para(
                            lp, bin_names, bin_dims, bin_index, assigned, next_id, inst, new_items,
                            renames, warnings,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

fn append_gso_description(
    common: &mut Vec<u8>,
    description: Option<&str>,
    warnings: &mut WriteReport,
) {
    let Some(description) = description.filter(|value| !value.is_empty()) else {
        common.extend_from_slice(&0u16.to_le_bytes());
        return;
    };
    let encoded = description.encode_utf16().collect::<Vec<_>>();
    let Ok(len) = u16::try_from(encoded.len()) else {
        warnings.loss(
            PreservationCode::ControlMetadataUnrepresentable,
            PreservationResourceKind::Metadata,
            PreservationDisposition::Unrepresentable,
            1,
        );
        common.extend_from_slice(&0u16.to_le_bytes());
        return;
    };
    common.extend_from_slice(&len.to_le_bytes());
    for unit in encoded {
        common.extend_from_slice(&unit.to_le_bytes());
    }
}

/// Removes picture controls lacking HWP5 records together with their extended characters and
/// readjusts remaining ExtCtrl ctrl_index values recursively.
fn strip_unwritable_pictures(para: &mut Paragraph, warnings: &mut WriteReport) {
    let mut removed: Vec<u32> = Vec::new();
    let mut kept = Vec::with_capacity(para.controls.len());
    for (i, mut control) in std::mem::take(&mut para.controls).into_iter().enumerate() {
        match &mut control {
            Control::Picture(p) if p.extras.is_empty() => {
                warnings.loss(
                    PreservationCode::PictureControlRemoved,
                    PreservationResourceKind::Control,
                    PreservationDisposition::Removed,
                    1,
                );
                removed.push(i as u32);
                continue;
            }
            // Omit only controls whose HWP5 payload cannot be synthesized (HWPX/Markdown headers,
            // automatic numbering, etc.). Preserve HWP5 originals with raw_children. Exception:
            // footnotes/endnotes (fn/en) normally have only ctrl_id and empty CTRL_HEADER data;
            // their paragraph_lists can synthesize LIST_HEADER plus paragraphs, so preserve them.
            Control::Generic(g)
                if g.data.is_empty()
                    && g.ctrl_id != *b"cold"
                    && g.raw_children.is_empty()
                    && !((g.ctrl_id == *b"fn  " || g.ctrl_id == *b"en  ")
                        && !g.paragraph_lists.is_empty()) =>
            {
                warnings.loss(
                    PreservationCode::OpaqueControlUnrepresentable,
                    PreservationResourceKind::Control,
                    PreservationDisposition::Unrepresentable,
                    1,
                );
                removed.push(i as u32);
                continue;
            }
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for cp in &mut cell.paragraphs {
                        strip_unwritable_pictures(cp, warnings);
                    }
                }
            }
            Control::Generic(g) => {
                for list in &mut g.paragraph_lists {
                    for lp in &mut list.paragraphs {
                        strip_unwritable_pictures(lp, warnings);
                    }
                }
            }
            _ => {}
        }
        kept.push(control);
    }
    para.controls = kept;
    if removed.is_empty() {
        return;
    }
    para.chars.retain(|ch| match ch {
        HwpChar::ExtCtrl {
            ctrl_index: Some(i),
            ..
        } => !removed.contains(i),
        _ => true,
    });
    for ch in &mut para.chars {
        if let HwpChar::ExtCtrl {
            ctrl_index: Some(i),
            ..
        } = ch
        {
            let shift = removed.iter().filter(|r| **r < *i).count() as u32;
            *i -= shift;
        }
    }
}

/// Section-definition payload from a genuine Hangul blank document (43 bytes), reproducing values
/// rather than embedding the source.
const DEFAULT_SECD_DATA: [u8; 43] = [
    0x00, 0x00, 0x00, 0x00, 0x6E, 0x04, 0x00, 0x00, 0x00, 0x00, 0x40, 0x1F, 0x00, 0x00, 0x01, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// Column-definition payload from a genuine Hangul blank document (12 bytes).
const DEFAULT_COLD_DATA: [u8; 12] = [
    0x04, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// secd child: FOOTNOTE_SHAPE (28 bytes), measured in hello_world.
const DEFAULT_FOOTNOTE_SHAPE: [u8; 28] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x29, 0x00, 0x01, 0x00, 0xff, 0xff, 0xff, 0xff,
    0x52, 0x03, 0x37, 0x02, 0x1b, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00,
];

/// secd child: endnote shape (same FOOTNOTE_SHAPE tag, 28 bytes), measured in hello_world.
const DEFAULT_ENDNOTE_SHAPE: [u8; 28] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x29, 0x00, 0x01, 0x00, 0xf8, 0x2f, 0xe0, 0x00,
    0x52, 0x03, 0x37, 0x02, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00,
];

/// secd children: PAGE_BORDER_FILL (14 bytes) for BOTH/EVEN/ODD. Layout: properties (u32), four
/// HWPUNIT16 margins (left/right/top/bottom=1417), borderFillID (u16)=1, always present in
/// synthesized DocInfo. The first u32 properties value is 1 (flags=1): genuine Hangul-saved 가나다.hwp
/// 5.1.1.0 and work_report.hwp 5.0.2.4 use 0x00000001 for all three. Do not use hello_world's
/// BOTH/EVEN values (0x0978f9c1, 0x271b0b81), which appear uninitialized; pyhwp parses them as
/// those invalid flag values.
const DEFAULT_PAGE_BORDER_FILLS: [[u8; 14]; 3] = [
    [
        0x01, 0x00, 0x00, 0x00, 0x89, 0x05, 0x89, 0x05, 0x89, 0x05, 0x89, 0x05, 0x01, 0x00,
    ],
    [
        0x01, 0x00, 0x00, 0x00, 0x89, 0x05, 0x89, 0x05, 0x89, 0x05, 0x89, 0x05, 0x01, 0x00,
    ],
    [
        0x01, 0x00, 0x00, 0x00, 0x89, 0x05, 0x89, 0x05, 0x89, 0x05, 0x89, 0x05, 0x01, 0x00,
    ],
];

/// Whether a synthesized SectionDef has the exact default payload/children materialized by the HWP5
/// writer.
///
/// Semantic reopen verification uses this predicate to equate only writer-added defaults with the
/// original empty synthesized form. Any differing byte in user payload or opaque children yields
/// false, so active document semantics cannot be hidden.
pub fn is_materialized_default_section_def(def: &SectionDef) -> bool {
    if def.data != DEFAULT_SECD_DATA
        || def.extras.len() != 5
        || def.footnote_shape_raw.as_deref() != Some(DEFAULT_FOOTNOTE_SHAPE.as_slice())
        || def.endnote_shape_raw.as_deref() != Some(DEFAULT_ENDNOTE_SHAPE.as_slice())
        || def.page_border_fills_raw
            != DEFAULT_PAGE_BORDER_FILLS
                .iter()
                .map(|bytes| bytes.to_vec())
                .collect::<Vec<_>>()
    {
        return false;
    }
    let expected = [
        (tag::FOOTNOTE_SHAPE, DEFAULT_FOOTNOTE_SHAPE.as_slice()),
        (tag::FOOTNOTE_SHAPE, DEFAULT_ENDNOTE_SHAPE.as_slice()),
        (
            tag::PAGE_BORDER_FILL,
            DEFAULT_PAGE_BORDER_FILLS[0].as_slice(),
        ),
        (
            tag::PAGE_BORDER_FILL,
            DEFAULT_PAGE_BORDER_FILLS[1].as_slice(),
        ),
        (
            tag::PAGE_BORDER_FILL,
            DEFAULT_PAGE_BORDER_FILLS[2].as_slice(),
        ),
    ];
    def.extras
        .iter()
        .zip(expected)
        .all(|(record, (tag, data))| {
            record.tag == tag && record.data == data && record.children.is_empty()
        })
}

/// Whether a synthesized column definition has the exact default payload materialized by the HWP5
/// writer.
pub fn is_materialized_default_column_def(generic: &hwp_model::GenericControl) -> bool {
    generic.ctrl_id == *b"cold"
        && generic.data == DEFAULT_COLD_DATA
        && generic.column_def.as_ref().is_some_and(|column| {
            column.count == 1
                && column.kind == 0
                && column.direction == 0
                && column.same_width
                && column.gap == 0
                && column.widths.is_empty()
                && column.divider.is_none()
        })
}

/// Default NUMBERING payload (226 bytes), byte-identical to the 5.1.0.1 hello_world sample: seven
/// paragraph-heading levels (^1..^7), seven start numbers, and three 5.1.x extension levels.
/// PARA_SHAPE references numbering_id=0, so an empty table dangles and Hangul rejects
/// corruption/tampering. This default is a safety net for synthesized/HWPX-origin documents.
pub(crate) const DEFAULT_NUMBERING_DATA: [u8; 226] = [
    0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x32, 0x00, 0xff, 0xff, 0xff, 0xff, 0x03, 0x00, 0x5e, 0x00,
    0x31, 0x00, 0x2e, 0x00, 0x0c, 0x01, 0x00, 0x00, 0x00, 0x00, 0x32, 0x00, 0xff, 0xff, 0xff, 0xff,
    0x03, 0x00, 0x5e, 0x00, 0x32, 0x00, 0x2e, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x32, 0x00,
    0xff, 0xff, 0xff, 0xff, 0x03, 0x00, 0x5e, 0x00, 0x33, 0x00, 0x29, 0x00, 0x0c, 0x01, 0x00, 0x00,
    0x00, 0x00, 0x32, 0x00, 0xff, 0xff, 0xff, 0xff, 0x03, 0x00, 0x5e, 0x00, 0x34, 0x00, 0x29, 0x00,
    0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x32, 0x00, 0xff, 0xff, 0xff, 0xff, 0x04, 0x00, 0x28, 0x00,
    0x5e, 0x00, 0x35, 0x00, 0x29, 0x00, 0x0c, 0x01, 0x00, 0x00, 0x00, 0x00, 0x32, 0x00, 0xff, 0xff,
    0xff, 0xff, 0x04, 0x00, 0x28, 0x00, 0x5e, 0x00, 0x36, 0x00, 0x29, 0x00, 0x2c, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x32, 0x00, 0xff, 0xff, 0xff, 0xff, 0x02, 0x00, 0x5e, 0x00, 0x37, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x32, 0x00, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x32, 0x00, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x32, 0x00,
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00,
    0x00, 0x00,
];

/// Whether these are the three default TAB_DEF records injected by the writer into synthesized
/// DocInfo.
pub fn is_materialized_default_tab_defs(raw: &[RawEntry], modeled: &[hwp_model::TabDef]) -> bool {
    let expected_raw = [
        [0, 0, 0, 0, 0, 0, 0, 0],
        [1, 0, 0, 0, 0, 0, 0, 0],
        [2, 0, 0, 0, 0, 0, 0, 0],
    ];
    let expected_attrs = [0, 1, 2];
    raw.len() == expected_raw.len()
        && modeled.len() == expected_attrs.len()
        && raw
            .iter()
            .zip(expected_raw)
            .all(|(entry, data)| entry.data == data && entry.children.is_empty())
        && modeled
            .iter()
            .zip(expected_attrs)
            .all(|(tab, attr)| tab.attr == attr && tab.items.is_empty())
}

/// The NUMBERING records the writer emits: one per `numbering_levels` entry. An entry with a raw
/// record keeps it; an entry past the raw records (a definition an edit grafted onto an
/// HWP5-sourced document, #377) gets the default record a synthetic document gets. A synthetic
/// document (Markdown-origin) has no raw records, so every entry gets the default; at least one,
/// because
/// every PARA_SHAPE references numbering_id=0 and an empty table is a dangling reference.
pub fn numbering_records(h: &hwp_model::DocHeader) -> Vec<RawEntry> {
    let count = h.numbering_levels.len().max(h.numberings.len()).max(1);
    let mut records = h.numberings.clone();
    records.resize_with(count, || RawEntry {
        data: DEFAULT_NUMBERING_DATA.to_vec(),
        children: Vec::new(),
    });
    records
}

/// The number format of each level of the default NUMBERING record, from its level selectors
/// (0x0c digit, 0x010c Hangul syllable, 0x2c circled digit; the pairing `numbering::OFFICIAL_SLOTS`
/// records): Hancom draws `1. 가. 1) 가) (1) (가) ①`. The reader drops the selectors and reports
/// every level as a digit, so this is what the record draws, not what it re-reads as.
pub const DEFAULT_NUMBERING_FORMATS: [hwp_model::NumFmt; 7] = [
    hwp_model::NumFmt::Digit,
    hwp_model::NumFmt::HangulSyllable,
    hwp_model::NumFmt::Digit,
    hwp_model::NumFmt::HangulSyllable,
    hwp_model::NumFmt::Digit,
    hwp_model::NumFmt::HangulSyllable,
    hwp_model::NumFmt::CircledDigit,
];

/// The levels the HWP5 reader parses from a NUMBERING record [`numbering_records`] synthesized:
/// what a modeled definition without a raw record becomes in HWP5 output (#377).
pub fn synthesized_numbering_levels() -> Vec<hwp_model::NumLevel> {
    crate::doc_info::parse_numbering_levels(&DEFAULT_NUMBERING_DATA)
}

/// The BULLET records the writer emits: one per `bullet_chars` entry, the raw record where there
/// is one and a synthesized one past them (#377). An empty table would leave a bulleted
/// paragraph's numbering_id dangling, which Hangul judges corrupt.
pub fn bullet_records(h: &hwp_model::DocHeader) -> Vec<RawEntry> {
    let mut records = h.bullets.clone();
    records.extend(
        h.bullet_chars
            .iter()
            .skip(h.bullets.len())
            .map(|&ch| RawEntry {
                data: make_bullet_data(ch),
                children: Vec::new(),
            }),
    );
    records
}

/// Whether these are the default NUMBERING injected into synthesized DocInfo and its parsed
/// semantic form.
pub fn is_materialized_default_numberings(
    raw: &[RawEntry],
    modeled: &[Vec<hwp_model::NumLevel>],
) -> bool {
    const TEMPLATES: [&str; 7] = ["^1.", "^2.", "^3)", "^4)", "(^5)", "(^6)", "^7"];
    !raw.is_empty()
        && raw.len() == modeled.len()
        && raw
            .iter()
            .all(|entry| entry.data == DEFAULT_NUMBERING_DATA && entry.children.is_empty())
        && modeled.iter().all(|levels| {
            levels.len() == TEMPLATES.len()
                && levels.iter().zip(TEMPLATES).all(|(level, template)| {
                    level.start == 1
                        && level.fmt == hwp_model::NumFmt::Digit
                        && level.template == template
                })
        })
}

/// Whether this is a raw BULLET record created by the writer from bullet_chars in synthesized
/// DocInfo.
pub fn is_materialized_generated_bullets(raw: &[RawEntry], chars: &[char]) -> bool {
    !raw.is_empty()
        && raw.len() == chars.len()
        && raw
            .iter()
            .zip(chars)
            .all(|(entry, ch)| entry.data == make_bullet_data(*ch) && entry.children.is_empty())
}

/// Whether this is the exact COMPATIBLE_DOCUMENT subtree the writer adds to synthesized documents.
pub fn is_materialized_compatible_document(record: &OpaqueRecord) -> bool {
    record.tag == tag::COMPATIBLE_DOCUMENT
        && record.data == [0; 4]
        && record.children.len() == 2
        && record.children[0].tag == tag::LAYOUT_COMPATIBILITY
        && record.children[0].data == [0; 20]
        && record.children[0].children.is_empty()
        && record.children[1].tag == tag::TRACKCHANGE
        && record.children[1].data.len() == 1032
        && record.children[1].data.first() == Some(&0x38)
        && record.children[1].data[1..].iter().all(|byte| *byte == 0)
        && record.children[1].children.is_empty()
}

/// Whether the writer filled an empty BorderFill::tail with the exact version-specific payload.
pub fn is_materialized_generated_border_fill_tail(fill: &hwp_model::BorderFill) -> bool {
    let expected: &[u8] = if fill.fill_type & 0x1 != 0 {
        &[0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0]
    } else {
        &[0, 0, 0, 0]
    };
    fill.tail == expected
}

/// Whether the writer filled an empty CharShape::tail with the exact six-byte payload.
pub fn is_materialized_generated_char_shape_tail(shape: &CharShape) -> bool {
    shape.border_fill_id >= 2
        && shape.tail.len() == 6
        && shape.tail[..2] == shape.border_fill_id.to_le_bytes()
        && shape.tail[2..].iter().all(|byte| *byte == 0)
}

/// Whether the writer filled an empty ParaShape::tail with the exact 5.1.x 16-byte payload.
pub fn is_materialized_generated_para_shape_tail(shape: &ParaShape) -> bool {
    shape.tail.len() == 16
        && shape.tail[..8].iter().all(|byte| *byte == 0)
        && shape.tail[8..12] == shape.line_spacing.to_le_bytes()
        && shape.tail[12..].iter().all(|byte| *byte == 0)
}

/// Whether the writer filled an empty Style::tail with the exact default lock value.
pub fn is_materialized_generated_style_tail(style: &Style) -> bool {
    style.tail == [0, 0]
}

/// HWPTAG_BULLET data: 25 bytes measured against every BULLET in the genuine Jeju Halla University
/// business-plan sample. Layout: paragraph-heading information (8 bytes, attr 0x08=automatic
/// hanging indent and 50% text distance), numbering character-shape ID (4 bytes, 0xFFFFFFFF=none),
/// bullet WCHAR (offset 12), and 11 trailing zero bytes.
///
/// The specification markdown's table 42 incorrectly says 20 bytes with the bullet at offset 8.
/// Genuine files put a four-byte character-shape ID between heading information and the character,
/// placing it at offset 12, analogous to NUMBERING's ff ff ff ff field. Omitting it makes Hangul
/// read zero at offset 12 and hide the bullet marker (first H2 hardware defect). Correct the
/// doc_info.rs BULLET parser to offset 12 as well.
fn make_bullet_data(ch: char) -> Vec<u8> {
    let mut v = Vec::with_capacity(25);
    v.extend_from_slice(&[0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x32, 0x00]); // Eight bytes of paragraph-heading information.
    v.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]); // Numbering character-shape ID: none.
    let code = u16::try_from(u32::from(ch)).unwrap_or(0x2022); // Bullet character at offset 12.
    v.extend_from_slice(&code.to_le_bytes());
    v.extend_from_slice(&[0u8; 11]); // Image presence/information, check character and tail; zero in every genuine sample.
    v
}

/// Writes the HwpSummaryInformation OLE property set (stream name starts with 0x05). Fill metadata
/// (title, subject, author, keywords, description, last saver, creation/modification times),
/// retaining prior defaults of empty strings and FILETIME zero when absent. Preserve the sample
/// structure, without redesigning stream byte layout. Header constants are measured from
/// Hangul-saved samples: FMTID 9FA2B660-1061-11D4-B4C6-006097C09D8C.
/// Property IDs (MS-OLEPS PIDSI): 0x02=title, 0x03=subject, 0x04=author, 0x14=date string (creation
/// time in KST, VT_LPWSTR), 0x05=keywords, 0x06=description, 0x08=last saver, 0x0C=creation time
/// and 0x0D=modification time (VT_FILETIME). Mirror the summary.rs parser, except 0x14 is derived
/// from create_time, ignored while reading and regenerated while writing.
pub(crate) fn hwp_summary_information(meta: &Metadata) -> Vec<u8> {
    const HWP_FMTID: [u8; 16] = [
        0x60, 0xB6, 0xA2, 0x9F, 0x61, 0x10, 0xD4, 0x11, 0xB4, 0xC6, 0x00, 0x60, 0x97, 0xC0, 0x9D,
        0x8C,
    ];
    let title = meta.title.as_deref().unwrap_or("");
    let subject = meta.subject.as_deref().unwrap_or("");
    let author = meta.author.as_deref().unwrap_or("");
    let keywords = meta.keywords.as_deref().unwrap_or("");
    let description = meta.description.as_deref().unwrap_or("");
    let last_saved_by = meta.last_saved_by.as_deref().unwrap_or("");
    // Write FILETIME as raw u64; None becomes zero, byte-identical to prior defaults and preserving
    // the gate.
    let create_time = meta.create_time.unwrap_or(0);
    let modify_time = meta.modify_time.unwrap_or(0);
    // PID 0x14 is a Korean KST date string. Across all 40 genuine samples it represents creation
    // time (0x0C, create_time), not modification time: its year matches create_time rather than
    // modify_time in every sample. Hangul's document-information date comes from this string. Fall
    // back to modify_time when create_time is absent, and to an empty string when both are absent,
    // retaining the existing structure/bytes and gate.
    let date_str = meta
        .create_time
        .or(meta.modify_time)
        .and_then(hwp_model::filetime_to_korean_kst)
        .unwrap_or_default();
    // Use the same property ID order/types as the genuine blank-document sample; fill metadata
    // only.
    enum Val<'a> {
        Str(&'a str),
        FileTime(u64),
        I4,
        Dictionary,
    }
    let props: [(u32, Val); 14] = [
        (0x02, Val::Str(title)),         // Title.
        (0x03, Val::Str(subject)),       // Subject.
        (0x04, Val::Str(author)),        // Author.
        (0x14, Val::Str(&date_str)),     // Date string: creation time in KST.
        (0x05, Val::Str(keywords)),      // Keywords.
        (0x06, Val::Str(description)),   // Description.
        (0x08, Val::Str(last_saved_by)), // Last saver.
        (0x09, Val::Str("hwp-cli")),     // Application.
        (0x0C, Val::FileTime(create_time)),
        (0x0D, Val::FileTime(modify_time)),
        (0x0B, Val::FileTime(0)),
        (0x0E, Val::I4),
        (0x15, Val::I4),
        // PID 0 is PID_DICTIONARY (MS-OLEPS 2.17), requiring a dictionary rather than VT_NULL.
        // Hangul's hello_world sample has one entry (id=0, empty name). With VT_NULL, pyhwp and
        // similar parsers read count=1, expect an entry, and reject EOF as truncation.
        (0x00, Val::Dictionary),
    ];

    // Section body: property ID/offset table followed by values.
    let mut values = ByteWriter::new();
    let table_size = 8 + props.len() * 8;
    let mut offsets = Vec::with_capacity(props.len());
    for (_, val) in &props {
        offsets.push(table_size + values.len());
        match val {
            Val::Str(text) => {
                values.write_u32(31); // VT_LPWSTR
                let units: Vec<u16> = text.encode_utf16().chain([0]).collect();
                values.write_u32(units.len() as u32);
                for u in &units {
                    values.write_u16(*u);
                }
                while !values.len().is_multiple_of(4) {
                    values.write_u8(0);
                }
            }
            Val::FileTime(ft) => {
                values.write_u32(64); // VT_FILETIME
                // dwLowDateTime then dwHighDateTime, little-endian. Zero matches prior defaults.
                values.write_u32((*ft & 0xFFFF_FFFF) as u32);
                values.write_u32((*ft >> 32) as u32);
            }
            Val::I4 => {
                values.write_u32(3); // VT_I4
                values.write_u32(0);
            }
            Val::Dictionary => {
                // MS-OLEPS Dictionary: entry count followed by DictionaryEntry (ID, name length,
                // name). Byte-identical to hello_world: 13 bytes, no padding because this is the
                // last property; one entry, id=0, one-byte name containing the NUL terminator.
                values.write_u32(1); // Dictionary entry count.
                values.write_u32(0); // DictionaryEntry.id
                values.write_u32(1); // Name length in bytes.
                values.write_u8(0); // Name: empty string, NUL-terminated.
            }
        }
    }
    let values = values.into_bytes();
    let section_size = table_size + values.len();

    let mut w = ByteWriter::new();
    w.write_u16(0xFFFE); // byte order
    w.write_u16(0); // format
    w.write_u32(0x0002_0400); // OS version, sample value.
    w.write_bytes(&HWP_FMTID); // CLSID
    w.write_u32(1); // Section count.
    w.write_bytes(&HWP_FMTID); // FMTID
    w.write_u32(48); // Section offset.
    w.write_u32(section_size as u32);
    w.write_u32(props.len() as u32);
    for ((pid, _), off) in props.iter().zip(&offsets) {
        w.write_u32(*pid);
        w.write_u32(*off as u32);
    }
    w.write_bytes(&values);
    w.into_bytes()
}

fn parse_version(s: &str) -> HwpVersion {
    let mut parts = s.split('.').filter_map(|p| p.parse::<u8>().ok());
    let (a, b, c, d) = (parts.next(), parts.next(), parts.next(), parts.next());
    match (a, b, c, d) {
        (Some(major), Some(minor), Some(build), Some(revision)) => HwpVersion {
            major,
            minor,
            build,
            revision,
        },
        _ => HwpVersion {
            major: 5,
            minor: 1,
            build: 0,
            revision: 1,
        },
    }
}

fn opaque_to_node(o: &OpaqueRecord) -> RecordNode {
    RecordNode {
        tag: o.tag,
        data: o.data.clone(),
        children: o.children.iter().map(opaque_to_node).collect(),
    }
}

fn hwp_string(w: &mut ByteWriter, s: &str) {
    let units: Vec<u16> = s.encode_utf16().collect();
    w.write_u16(units.len() as u16);
    for u in units {
        w.write_u16(u);
    }
}

// ─────────────────────────── DocInfo ───────────────────────────

/// Numbering references in paragraphs emitted from the model, excluding raw-only mirrors.
fn referenced_numbering_levels(doc: &Document) -> BTreeSet<(usize, usize)> {
    let mut used = BTreeSet::new();
    let mut pending: Vec<_> = doc.sections.iter().flat_map(|s| &s.paragraphs).collect();
    while let Some(paragraph) = pending.pop() {
        if let Some(shape) = doc.header.para_shapes.get(paragraph.para_shape.0 as usize)
            && shape.head_type() == 2
        {
            used.insert((shape.numbering_id as usize, shape.head_level() as usize - 1));
        }
        for control in &paragraph.controls {
            let caption = match control {
                Control::Table(table) => {
                    for cell in &table.cells {
                        pending.extend(&cell.paragraphs);
                    }
                    &table.caption
                }
                Control::Picture(picture) => &picture.caption,
                Control::Generic(generic) if generic.raw_children.is_empty() => {
                    for list in &generic.paragraph_lists {
                        pending.extend(&list.paragraphs);
                    }
                    &generic.caption
                }
                Control::Generic(_) | Control::SectionDef(_) => continue,
            };
            if let Some(caption) = caption {
                pending.extend(&caption.paragraphs);
            }
        }
    }
    used
}

fn emit_doc_info(doc: &Document, warnings: &mut WriteReport) -> Vec<RecordNode> {
    let h = &doc.header;
    let mut roots = Vec::new();

    // Safety net: synthesized Markdown/HWPX documents arrive with empty tab_defs/numberings, but
    // every PARA_SHAPE references tab_def_id=0/numbering_id=0. Empty tables leave dangling ID 0 and
    // Hangul rejects corruption/tampering. Confirmed with halla.hwp: HWPX input bypasses
    // from_markdown, so changing default_header alone is insufficient. Every normal hello_world
    // 5.1.0.1 sample has TAB_DEF>=1 and NUMBERING=1; inject copies of those bytes as defaults.
    // Emission and ID_MAPPINGS counts use the same Vec and always stay consistent.
    let tab_defs_owned: Vec<RawEntry> = if h.tab_defs.is_empty() {
        vec![
            RawEntry {
                data: vec![0, 0, 0, 0, 0, 0, 0, 0],
                children: Vec::new(),
            },
            RawEntry {
                data: vec![1, 0, 0, 0, 0, 0, 0, 0],
                children: Vec::new(),
            },
            RawEntry {
                data: vec![2, 0, 0, 0, 0, 0, 0, 0],
                children: Vec::new(),
            },
        ]
    } else {
        h.tab_defs.clone()
    };
    let numberings_owned = numbering_records(h);
    // Raw-backed definitions are emitted verbatim, including the official records already
    // materialized upstream. Only unbacked modeled levels can be replaced by the default.
    let default_levels = synthesized_numbering_levels();
    let used_levels = referenced_numbering_levels(doc);
    for (definition, levels) in h
        .numbering_levels
        .iter()
        .enumerate()
        .skip(h.numberings.len())
    {
        let mut changed_levels = Vec::new();
        let mut changed_template = false;
        let mut changed_format = false;
        let mut changed_start = false;
        for (index, level) in levels.iter().enumerate() {
            // HWPX and Markdown carry extra unused levels beyond the native seven-slot
            // fallback. Their absence changes visible numbering only if a paragraph uses one.
            if index >= default_levels.len() && !used_levels.contains(&(definition, index)) {
                continue;
            }
            let default = default_levels.get(index);
            let template_changed = default.is_none_or(|value| value.template != level.template);
            // The reader drops format selectors, so compare what Hancom actually draws.
            let format_changed = DEFAULT_NUMBERING_FORMATS
                .get(index)
                .is_none_or(|format| *format != level.fmt);
            let start_changed = default.is_none_or(|value| value.start != level.start);
            if template_changed || format_changed || start_changed {
                changed_levels.push(index.to_string());
                changed_template |= template_changed;
                changed_format |= format_changed;
                changed_start |= start_changed;
            }
        }
        if !changed_levels.is_empty() {
            let mut categories = Vec::new();
            if changed_template {
                categories.push("template");
            }
            if changed_format {
                categories.push("format");
            }
            if changed_start {
                categories.push("start");
            }
            warnings.warning(format!(
                "numbering_default_substitution: definition={definition} levels=[{}] changed={} - 원본 NUMBERING 레코드가 없어 모델의 번호 정의를 기본 번호로 대체합니다. 번호 모양을 보존하려면 HWPX로 저장하세요.",
                changed_levels.join(","),
                categories.join(",")
            ));
        }
    }
    let bullets_owned = bullet_records(h);

    // DOCUMENT_PROPERTIES: derive the section count from actual sections.
    let mut w = ByteWriter::new();
    w.write_u16(doc.sections.len().max(1) as u16);
    // Starting page/footnote/endnote/picture/table/equation numbers must be at least 1. Synthesized
    // documents use zero throughout, but Hangul treats page zero as corruption/tampering; every
    // sample uses 1.
    for n in h.properties.start_numbers {
        w.write_u16(n.max(1));
    }
    w.write_u32(h.properties.caret.0);
    w.write_u32(h.properties.caret.1);
    w.write_u32(h.properties.caret.2);
    roots.push(RecordNode {
        tag: tag::DOCUMENT_PROPERTIES,
        data: w.into_bytes(),
        children: Vec::new(),
    });

    // ID_MAPPINGS: derive counts from table lengths and preserve source extension tails.
    let mut counts: Vec<u32> = Vec::with_capacity(18);
    counts.push(h.bin_data.len() as u32);
    for slot in 0..LANG_COUNT {
        counts.push(h.fonts[slot].len() as u32);
    }
    counts.push(h.border_fills.len() as u32);
    counts.push(h.char_shapes.len() as u32);
    counts.push(tab_defs_owned.len() as u32);
    counts.push(numberings_owned.len() as u32);
    counts.push(bullets_owned.len() as u32);
    counts.push(h.para_shapes.len() as u32);
    counts.push(h.styles.len() as u32);
    if h.id_mappings_counts.len() > counts.len() {
        counts.extend_from_slice(&h.id_mappings_counts[counts.len()..]);
    }
    // The ID_MAPPINGS count length must match the declared version (specification tables 16 and 15:
    // variable by document version). Index 15=memo shape (5.0.2.1+); indices 16/17=track changes
    // (5.0.3.2+). The derived default has 15 counts (indices 0..14, through styles).
    // - For source round-trips, preserve h.id_mappings_counts length; genuine Hancom-saved files
    //   already use their version's length.
    // - For synthesized documents without original counts, derive the target length from the
    //   declared version.
    // Unconditionally padding to 18 inflates 5.0.2.x documents from 16 counts, creating a
    // version/layout mismatch Hangul rejects as corrupt, verified with work_report 5.0.2.4.
    let ver = parse_version(&doc.meta.source_version);
    let version_target = if ver.to_u32() >= 0x05_00_03_02 {
        18 // 5.0.3.2+: memo plus two track-change counts.
    } else if ver.to_u32() >= 0x05_00_02_01 {
        16 // 5.0.2.1+: memo shape.
    } else {
        15 // Earlier versions: through styles.
    };
    let target = h
        .id_mappings_counts
        .len()
        .max(version_target)
        .max(counts.len());
    while counts.len() < target {
        counts.push(0);
    }
    let mut w = ByteWriter::new();
    for c in &counts {
        w.write_u32(*c);
    }

    let mut children = Vec::new();
    for item in &h.bin_data {
        children.push(emit_bin_data(item));
    }
    for slot in 0..LANG_COUNT {
        for f in &h.fonts[slot] {
            children.push(emit_face_name(f));
        }
    }
    for bf in &h.border_fills {
        children.push(emit_border_fill(bf));
    }
    for cs in &h.char_shapes {
        children.push(emit_char_shape(cs));
    }
    for t in &tab_defs_owned {
        children.push(RecordNode {
            tag: tag::TAB_DEF,
            data: t.data.clone(),
            children: t.children.iter().map(opaque_to_node).collect(),
        });
    }
    for n in &numberings_owned {
        children.push(RecordNode {
            tag: tag::NUMBERING,
            data: n.data.clone(),
            children: n.children.iter().map(opaque_to_node).collect(),
        });
    }
    for b in &bullets_owned {
        children.push(RecordNode {
            tag: tag::BULLET,
            data: b.data.clone(),
            children: b.children.iter().map(opaque_to_node).collect(),
        });
    }
    for ps in &h.para_shapes {
        children.push(emit_para_shape(ps));
    }
    for st in &h.styles {
        children.push(emit_style(st));
    }
    children.extend(h.id_extras.iter().map(opaque_to_node));

    roots.push(RecordNode {
        tag: tag::ID_MAPPINGS,
        data: w.into_bytes(),
        children,
    });

    // Synthesized 5.1.x documents require a COMPATIBLE_DOCUMENT subtree. Genuine 가나다.hwp 5.1.1.0
    // and hello_world 5.1.0.1 have it; older work_report 5.0.2.4 is exempt. Missing it triggers
    // Hangul corruption/tampering rejection. HWP5 round-trips preserve it through h.extras
    // (extended below), so add it only on synthesized paths where absent from the source.
    let synth = doc.meta.source_format != "hwp5";
    let has_compat = h.extras.iter().any(|r| r.tag == tag::COMPATIBLE_DOCUMENT);
    if synth && !has_compat {
        let mut trackchange = vec![0u8; 1032];
        trackchange[0] = 0x38; // Measured in the 가나다/hello_world samples: first byte 0x38, all others zero.
        roots.push(RecordNode {
            tag: tag::COMPATIBLE_DOCUMENT,
            data: vec![0u8; 4], // Target application 0.
            children: vec![
                RecordNode {
                    tag: tag::LAYOUT_COMPATIBILITY,
                    data: vec![0u8; 20],
                    children: Vec::new(),
                },
                RecordNode {
                    tag: tag::TRACKCHANGE,
                    data: trackchange,
                    children: Vec::new(),
                },
            ],
        });
    }

    roots.extend(h.extras.iter().map(opaque_to_node));
    roots
}

fn emit_bin_data(item: &hwp_model::BinDataItem) -> RecordNode {
    let mut w = ByteWriter::new();
    w.write_u16(item.attr);
    if item.kind() == 0 {
        hwp_string(&mut w, item.link_abs.as_deref().unwrap_or(""));
        hwp_string(&mut w, item.link_rel.as_deref().unwrap_or(""));
    } else {
        w.write_u16(item.storage_id.unwrap_or(0));
        if item.kind() == 1 {
            hwp_string(&mut w, item.extension.as_deref().unwrap_or(""));
        }
    }
    w.write_bytes(&item.tail);
    RecordNode {
        tag: tag::BIN_DATA,
        data: w.into_bytes(),
        children: Vec::new(),
    }
}

fn emit_face_name(f: &FaceName) -> RecordNode {
    let mut attr = f.attr;
    if f.alt_name.is_some() {
        attr |= 0x80;
    }
    if f.panose.is_some() {
        attr |= 0x40;
    }
    if f.default_name.is_some() {
        attr |= 0x20;
    }
    let mut w = ByteWriter::new();
    w.write_u8(attr);
    hwp_string(&mut w, &f.name);
    if let Some(alt) = &f.alt_name {
        w.write_u8(f.alt_kind.unwrap_or(0));
        hwp_string(&mut w, alt);
    }
    if let Some(p) = &f.panose {
        w.write_bytes(p);
    }
    if let Some(d) = &f.default_name {
        hwp_string(&mut w, d);
    }
    w.write_bytes(&f.tail);
    RecordNode {
        tag: tag::FACE_NAME,
        data: w.into_bytes(),
        children: Vec::new(),
    }
}

fn write_border_line(w: &mut ByteWriter, line: &BorderLine) {
    w.write_u8(line.line_type);
    w.write_u8(line.width);
    w.write_u32(line.color);
}

fn emit_border_fill(bf: &hwp_model::BorderFill) -> RecordNode {
    let mut w = ByteWriter::new();
    w.write_u16(bf.attr);
    for side in &bf.sides {
        write_border_line(&mut w, side);
    }
    write_border_line(&mut w, &bf.diagonal);
    w.write_u32(bf.fill_type);
    if bf.fill_type & 0x1 != 0 {
        w.write_u32(bf.bg_color.unwrap_or(0xFFFF_FFFF));
    }
    if bf.tail.is_empty() {
        // HWPX/Markdown input: complete the HWP5 fill block, derived from genuine 40/53-byte
        // layouts.
        if bf.fill_type & 0x1 != 0 {
            w.write_u32(0); // Pattern color.
            w.write_u32(0xFFFF_FFFF); // Pattern kind (-1=none).
        }
        w.write_u32(0); // Additional fill-property size.
        if bf.fill_type & 0x1 != 0 {
            w.write_u8(0); // Transparency.
        }
    } else {
        // HWP5 round-trip: tail holds everything after the pattern color.
        w.write_bytes(&bf.tail);
    }
    RecordNode {
        tag: tag::BORDER_FILL,
        data: w.into_bytes(),
        children: Vec::new(),
    }
}

fn emit_char_shape(cs: &CharShape) -> RecordNode {
    let mut w = ByteWriter::new();
    for id in cs.face_ids {
        w.write_u16(id);
    }
    for v in cs.ratios {
        w.write_u8(v);
    }
    for v in cs.spacings {
        w.write_u8(v as u8);
    }
    for v in cs.rel_sizes {
        w.write_u8(v);
    }
    for v in cs.offsets {
        w.write_u8(v as u8);
    }
    w.write_i32(cs.base_size);
    // If the IR strike flag is set, set CHAR_SHAPE attr bit18 (strikeout=1). Based on specification
    // section 4.2.7 table 35 (bits18..20) and genuine corpus paragraphs with strikeout value 1.
    // Apply only while writing Markdown-origin content known to be struck. The reader does not
    // trust bit18: genuine track-change deletion markers can set it and cause false strikeout on
    // plain text (DIFFSPEC; doc_info.rs fixes strike=false). Final Hangul acceptance of bit
    // combinations still requires H2 hardware verification.
    let attr = if cs.strike {
        cs.attr | (1 << 18)
    } else {
        cs.attr
    };
    w.write_u32(attr);
    w.write_u8(cs.shadow_gap.0 as u8);
    w.write_u8(cs.shadow_gap.1 as u8);
    w.write_u32(cs.text_color);
    w.write_u32(cs.underline_color);
    w.write_u32(cs.shade_color);
    w.write_u32(cs.shadow_color);
    if cs.tail.is_empty() {
        // HWPX/Markdown input: fill the 5.1.x layout, with border-fill ID (5.0.2.1+) and strikeout
        // color (5.0.3.0+).
        w.write_u16(cs.border_fill_id.max(2));
        w.write_u32(0); // Strikeout color.
    } else {
        // HWP5 round-trip: border_fill_id was read from the tail's beginning and remains there.
        w.write_bytes(&cs.tail);
    }
    RecordNode {
        tag: tag::CHAR_SHAPE,
        data: w.into_bytes(),
        children: Vec::new(),
    }
}

fn emit_para_shape(ps: &ParaShape) -> RecordNode {
    // A level edited after reading (set_head_level) is put back into the zero-based
    // on-disk form here, the one place every write path passes; untouched shapes are
    // already consistent and pass through unchanged (#472, K1).
    let mut normalized = ps.clone();
    normalize_numbered_level_encoding(&mut normalized);
    let ps = &normalized;
    let mut w = ByteWriter::new();
    // Line-spacing kind lives in attr1 bits0..1, matching parse_para_shape. Fold IR changes back
    // into those bits or Hangul 2010+ reads the old kind. Clear then set; unchanged paragraph
    // shapes receive their original bits, retaining the byte-identity round-trip gate.
    w.write_u32((ps.attr1 & !0x3) | (u32::from(ps.line_spacing_type) & 0x3));
    w.write_i32(ps.margin_left);
    w.write_i32(ps.margin_right);
    w.write_i32(ps.indent);
    w.write_i32(ps.spacing_top);
    w.write_i32(ps.spacing_bottom);
    w.write_i32(ps.line_spacing_old);
    w.write_u16(ps.tab_def_id);
    // IR numbering/bullet references are zero-based and disk references are one-based. Restore +1
    // only for head_type 2/3, reversing parse_para_shape's -1 normalization for byte-identical
    // round-trips. Leave outline/no-heading values unchanged.
    let numbering_id_raw = if matches!(ps.head_type(), 2 | 3) {
        ps.numbering_id.saturating_add(1)
    } else {
        ps.numbering_id
    };
    w.write_u16(numbering_id_raw);
    w.write_u16(ps.border_fill_id);
    for v in ps.border_offsets {
        w.write_u16(v as u16);
    }
    if ps.tail.is_empty() {
        // HWPX/Markdown input: fill the 5.1.0.1 layout (attr2, attr3, line spacing, trailing four
        // bytes). Normal hello_world 5.1.0.1 PARA_SHAPE records are 58 bytes with a 16-byte tail:
        // attr2(4)+attr3(4)+line spacing(4)+tail(4). Omitting the last four bytes yields 54 and a
        // Hangul integrity corruption/tampering warning. Synthesized documents always declare
        // 5.1.0.1 (parse_version default), requiring 58 bytes.
        w.write_u32(0);
        w.write_u32(0);
        w.write_u32(if ps.line_spacing > 0 {
            ps.line_spacing as u32
        } else {
            160
        });
        w.write_u32(0); // Trailing four bytes, matching hello_world 5.1.0.1: 00 00 00 00.
    } else if ps.tail.len() >= 12 {
        // For 5.0.2.5+, the reader takes line spacing from tail[8..12]. Emitting the preserved tail
        // unchanged would lose IR edits and show the old spacing in Hangul. Unedited shapes receive
        // the same value and stay unchanged. Leave tails shorter than 12 bytes at their original
        // length.
        let mut tail = ps.tail.clone();
        tail[8..12].copy_from_slice(&ps.line_spacing.to_le_bytes());
        w.write_bytes(&tail);
    } else {
        w.write_bytes(&ps.tail);
    }
    RecordNode {
        tag: tag::PARA_SHAPE,
        data: w.into_bytes(),
        children: Vec::new(),
    }
}

fn emit_style(st: &Style) -> RecordNode {
    let mut w = ByteWriter::new();
    hwp_string(&mut w, &st.name);
    hwp_string(&mut w, &st.english_name);
    w.write_u8(st.attr);
    w.write_u8(st.next_style);
    w.write_u16(st.lang_id as u16);
    w.write_u16(st.para_shape.0);
    w.write_u16(st.char_shape.0);
    if st.tail.is_empty() {
        w.write_u16(0); // Trailing two bytes for locking, etc., measured in genuine samples.
    } else {
        w.write_bytes(&st.tail);
    }
    RecordNode {
        tag: tag::STYLE,
        data: w.into_bytes(),
        children: Vec::new(),
    }
}

// ─────────────────────────── BodyText ───────────────────────────

fn emit_section(
    section: &Section,
    synthesize: bool,
    preserve_linesegs: bool,
    add_tracking_tail: bool,
    warnings: &mut WriteReport,
) -> Vec<RecordNode> {
    let mut roots: Vec<RecordNode> = section
        .paragraphs
        .iter()
        .map(|p| {
            emit_paragraph(
                p,
                synthesize,
                preserve_linesegs,
                add_tracking_tail,
                warnings,
            )
        })
        .collect();
    roots.extend(section.extras.iter().map(opaque_to_node));
    roots
}

fn emit_paragraph(
    para: &Paragraph,
    synthesize: bool,
    preserve_linesegs: bool,
    add_tracking_tail: bool,
    warnings: &mut WriteReport,
) -> RecordNode {
    // Normalize Hangul paragraph invariants during serialization on synthesized Markdown/HWPX
    // paths. Every synthesized paragraph (body, table cells, text boxes) passes through this point.
    // Centralize in the writer rather than the IR/converter to cover both from_markdown and
    // HWPX-to-HWP conversion. Unmodified HWP5 round-trips use synthesize=false, preserving
    // originals and the byte-identity gate.
    let patched;
    let para = if synthesize {
        let mut p = para.clone();
        // 1. Every paragraph must end with 0x0d=13, as in all 188 genuine paragraphs examined.
        //    Omitting it causes Hangul corruption/tampering rejection.
        if p.chars.last() != Some(&HwpChar::CharCtrl(13)) {
            p.chars.push(HwpChar::CharCtrl(13));
        }
        // 2. nchars high bit 0x80000000 marks the last paragraph of a list. Genuine multiparagraph
        //    5.1.1.0 files set only the section's last paragraph; all others use zero. Setting it
        //    everywhere makes Hangul treat the first paragraph as final and ignore the rest, hiding
        //    paragraphs after the first. The IR's set_last_para_flag sets chars_flags per list, so
        //    leave it untouched here.
        // 3. Merge consecutive PARA_CHAR_SHAPE runs with identical IDs; duplicate runs are treated
        //    as corruption.
        p.char_shape_runs.dedup_by(|(_, b), (_, a)| a == b);
        patched = p;
        &patched
    } else {
        para
    };
    // PARA_HEADER
    let mut w = ByteWriter::new();
    // Hangul records one character even for empty paragraphs, the implicit paragraph end, omitting
    // PARA_TEXT. All 60 measured empty-cell paragraphs have low nchars=1. Zero causes a corruption
    // verdict.
    let char_count = if para.chars.is_empty() {
        1
    } else {
        para.wchar_len()
    };
    let nchars = char_count | (u32::from(para.header.chars_flags) << 24);
    w.write_u32(nchars);
    // ctrl_mask marks only extended/inline controls, excluding character controls such as paragraph
    // end 13, line break 10 and hyphens. Genuine text-only paragraphs ending in 13 have
    // ctrl_mask=0. Setting bits for CharCtrl makes Hangul report a control declared in ctrl_mask
    // but absent from the content, treating it as corrupt.
    let ctrl_mask = if para.header.ctrl_mask != 0 {
        para.header.ctrl_mask
    } else {
        para.chars
            .iter()
            .filter_map(|c| match c {
                HwpChar::InlineCtrl { code, .. } | HwpChar::ExtCtrl { code, .. } => {
                    Some(1u32 << code)
                }
                _ => None,
            })
            .fold(0, |a, b| a | b)
    };
    w.write_u32(ctrl_mask);
    w.write_u16(para.para_shape.0);
    w.write_u8(para.style.0 as u8);
    w.write_u8(para.header.break_type);
    w.write_u16(para.char_shape_runs.len() as u16);
    let range_tags = para
        .extras
        .iter()
        .filter(|e| e.tag == tag::PARA_RANGE_TAG)
        .count() as u16;
    w.write_u16(range_tags);
    // PARA_LINE_SEG count.
    // - preserve_linesegs (unmodified HWP5 round-trip): preserve the original for the byte-identity
    //   gate.
    // - synthesize (Markdown/HWPX synthesis): emit layouts populated by synthesize_linesegs.
    //   Without this cache, 5.1.x body text does not draw (hardware round 13).
    let emit_lineseg = synthesize || preserve_linesegs;
    let seg_count = if emit_lineseg {
        para.line_segs.len()
    } else {
        0
    };
    w.write_u16(seg_count as u16);
    w.write_u32(para.header.instance_id);
    if para.header.tail.is_empty() {
        // Synthesized paragraphs declaring 5.0.3.2+ append the UINT16 merged-track-change-paragraph
        // field (=0) for 24 bytes (specification table 58), matching the 00 00 tail in normal
        // hello_world 5.1.0.1 PARA_HEADER records. Omitting it yields 22 bytes and Hangul
        // version/layout corruption/tampering warnings.
        if add_tracking_tail {
            w.write_u16(0);
        }
    } else {
        // HWP5 round-trip: preserve the tail's version-specific extra fields unchanged.
        w.write_bytes(&para.header.tail);
    }

    let mut children = Vec::new();
    // Do not emit PARA_TEXT for empty paragraphs containing only the implicit end (nchars=1).
    // Genuine work_report and Halla University empty paragraphs/cells all have nchars=1 without
    // PARA_TEXT. Synthesis appends CharCtrl(13) to every paragraph, yielding chars=[0x0d] with
    // char_count=1 for empties; emitting PARA_TEXT=[0x0d] makes Hangul reject them as corrupt,
    // formerly corrupting every table with empty cells. Emit PARA_TEXT only for char_count>1.
    // Nonempty paragraphs retain the ending 0x0d and PARA_TEXT length=nchars, matching genuine
    // files.
    if char_count > 1 {
        children.push(RecordNode {
            tag: tag::PARA_TEXT,
            data: emit_para_text(&para.chars),
            children: Vec::new(),
        });
    }
    if !para.char_shape_runs.is_empty() {
        let mut cw = ByteWriter::new();
        for (pos, id) in &para.char_shape_runs {
            cw.write_u32(*pos);
            cw.write_u32(u32::from(id.0));
        }
        children.push(RecordNode {
            tag: tag::PARA_CHAR_SHAPE,
            data: cw.into_bytes(),
            children: Vec::new(),
        });
    }
    if emit_lineseg && !para.line_segs.is_empty() {
        let mut lw = ByteWriter::new();
        for seg in &para.line_segs {
            lw.write_u32(seg.text_start);
            lw.write_i32(seg.v_pos);
            lw.write_i32(seg.line_height);
            lw.write_i32(seg.text_height);
            lw.write_i32(seg.baseline_gap);
            lw.write_i32(seg.line_spacing);
            lw.write_i32(seg.col_start);
            lw.write_i32(seg.seg_width);
            lw.write_u32(seg.flags);
        }
        children.push(RecordNode {
            tag: tag::PARA_LINE_SEG,
            data: lw.into_bytes(),
            children: Vec::new(),
        });
    }
    let mut next_extra = 0;
    let mut next_control = 0;
    for kind in &para.header.hwp5_child_order {
        match kind {
            hwp_model::Hwp5ParagraphChild::Extra => {
                if let Some(extra) = para.extras.get(next_extra) {
                    children.push(opaque_to_node(extra));
                    next_extra += 1;
                }
            }
            hwp_model::Hwp5ParagraphChild::Control => {
                if let Some(control) = para.controls.get(next_control) {
                    children.push(emit_control(
                        control,
                        synthesize,
                        preserve_linesegs,
                        add_tracking_tail,
                        warnings,
                    ));
                    next_control += 1;
                }
            }
        }
    }
    children.extend(para.extras[next_extra..].iter().map(opaque_to_node));
    for control in &para.controls[next_control..] {
        children.push(emit_control(
            control,
            synthesize,
            preserve_linesegs,
            add_tracking_tail,
            warnings,
        ));
    }

    RecordNode {
        tag: tag::PARA_HEADER,
        data: w.into_bytes(),
        children,
    }
}

fn emit_para_text(chars: &[HwpChar]) -> Vec<u8> {
    let mut w = ByteWriter::new();
    for ch in chars {
        match ch {
            // Defensively normalize tabs incorrectly loaded as Text in legacy malformed IR into
            // 8-WCHAR InlineCtrl(9). Emitting code9 as one WCHAR makes Hangul consume the next
            // seven WCHARs as part of the inline control (section 3.2.3 table 6). Normal paths
            // already store tabs as InlineCtrl(9), so this does not affect the byte-identity gate;
            // genuine HWP5 does not store tabs as Text.
            HwpChar::Text('\t') => {
                w.write_u16(9);
                w.write_bytes(&[0u8; 12]);
                w.write_u16(9);
            }
            HwpChar::Text(c) => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    w.write_u16(*u);
                }
            }
            HwpChar::CharCtrl(code) => w.write_u16(*code),
            HwpChar::InlineCtrl { code, payload } | HwpChar::ExtCtrl { code, payload, .. } => {
                w.write_u16(*code);
                let mut p = payload.clone();
                p.resize(12, 0);
                w.write_bytes(&p);
                w.write_u16(*code);
            }
        }
    }
    w.into_bytes()
}

fn reversed(id: [u8; 4]) -> [u8; 4] {
    let mut r = id;
    r.reverse();
    r
}

fn emit_control(
    control: &Control,
    synthesize: bool,
    preserve_linesegs: bool,
    add_tracking_tail: bool,
    warnings: &mut WriteReport,
) -> RecordNode {
    match control {
        Control::SectionDef(def) => emit_section_def(def),
        Control::Table(table) => emit_table(
            table,
            synthesize,
            preserve_linesegs,
            add_tracking_tail,
            warnings,
        ),
        Control::Picture(pic) => emit_picture(
            pic,
            synthesize,
            preserve_linesegs,
            add_tracking_tail,
            warnings,
        ),
        Control::Generic(g) => {
            let mut w = ByteWriter::new();
            w.write_bytes(&reversed(g.ctrl_id));
            if g.data.is_empty() && g.ctrl_id == *b"cold" {
                w.write_bytes(&DEFAULT_COLD_DATA);
            } else {
                w.write_bytes(&g.data);
            }
            // Emit original HWP5 child subtrees with their nesting intact for lossless
            // preservation. paragraph_lists/extras serve only text extraction; do not flatten them.
            if !g.raw_children.is_empty() {
                return RecordNode {
                    tag: tag::CTRL_HEADER,
                    data: w.into_bytes(),
                    children: g.raw_children.iter().map(opaque_to_node).collect(),
                };
            }
            let mut children = match &g.caption {
                Some(caption) => emit_caption_records(
                    caption,
                    synthesize,
                    preserve_linesegs,
                    add_tracking_tail,
                    warnings,
                ),
                None => Vec::new(),
            };
            for list in &g.paragraph_lists {
                children.push(RecordNode {
                    tag: tag::LIST_HEADER,
                    data: generic_list_header(g.ctrl_id, list),
                    children: Vec::new(),
                });
                for p in &list.paragraphs {
                    children.push(emit_paragraph(
                        p,
                        synthesize,
                        preserve_linesegs,
                        add_tracking_tail,
                        warnings,
                    ));
                }
            }
            if !g.extras.is_empty() && !g.paragraph_lists.is_empty() {
                warnings.warning(format!(
                    "{:?} 컨트롤 내부 구조가 평탄화되어 저장됨 — 한글에서 확인 필요",
                    String::from_utf8_lossy(&g.ctrl_id)
                ));
            }
            children.extend(g.extras.iter().map(opaque_to_node));
            RecordNode {
                tag: tag::CTRL_HEADER,
                data: w.into_bytes(),
                children,
            }
        }
    }
}

fn generic_list_header(ctrl_id: [u8; 4], list: &hwp_model::ParagraphList) -> Vec<u8> {
    // New notes need a list header even in an HWP5 template without pictures. Native raw
    // subtrees bypass this materialization in emit_control.
    if matches!(&ctrl_id, b"fn  " | b"en  ") && list.header_data.is_empty() {
        synthesized_note_list_header(list.paragraphs.len())
    } else {
        list.header_data.clone()
    }
}

/// The existing generated note LIST_HEADER, independent of picture synthesis (#391).
/// Native or custom nonempty headers remain owned by their source document.
pub fn synthesized_note_list_header(paragraph_count: usize) -> Vec<u8> {
    let mut header = hex_to_bytes(HEADER_LIST_HEADER_TEMPLATE);
    let count = paragraph_count.max(1) as u16;
    header[0..2].copy_from_slice(&count.to_le_bytes());
    header
}

/// Whether a note's raw subtree is exactly the serialized modeled view. The comparison keeps
/// source paragraph caches and tails; raw-only records or different nesting prevent equivalence.
/// Represented opaque extras remain in the modeled view. Call before canonicalizing caches.
pub fn note_has_redundant_raw_children(note: &hwp_model::GenericControl) -> bool {
    if !matches!(&note.ctrl_id, b"fn  " | b"en  ") || note.raw_children.is_empty() {
        return false;
    }
    let mut modeled = note.clone();
    modeled.raw_children.clear();
    let mut report = WriteReport::new();
    let emitted = emit_control(&Control::Generic(modeled), false, true, false, &mut report);
    emitted.children
        == note
            .raw_children
            .iter()
            .map(opaque_to_node)
            .collect::<Vec<_>>()
}

fn emit_section_def(def: &SectionDef) -> RecordNode {
    let mut w = ByteWriter::new();
    w.write_bytes(b"dces");
    if def.data.is_empty() {
        w.write_bytes(&DEFAULT_SECD_DATA);
    } else {
        w.write_bytes(&def.data);
    }
    let mut children = Vec::new();
    if let Some(p) = &def.page {
        let mut pw = ByteWriter::new();
        pw.write_i32(p.width.0);
        pw.write_i32(p.height.0);
        pw.write_i32(p.margin_left.0);
        pw.write_i32(p.margin_right.0);
        pw.write_i32(p.margin_top.0);
        pw.write_i32(p.margin_bottom.0);
        pw.write_i32(p.margin_header.0);
        pw.write_i32(p.margin_footer.0);
        pw.write_i32(p.gutter.0);
        pw.write_u32(p.attr);
        children.push(RecordNode {
            tag: tag::PAGE_DEF,
            data: pw.into_bytes(),
            children: Vec::new(),
        });
    }
    if def.extras.is_empty() {
        // For synthesized Markdown/HWPX documents, supply required secd children from hello_world
        // 5.1.0.1 values. Hangul expects footnote/endnote shapes and three page borders
        // (BOTH/EVEN/ODD) under a section definition; PAGE_DEF alone causes corruption/tampering
        // rejection. PAGE_BORDER_FILL ends with borderFillID=1 (two bytes), always present.
        for shape in [&DEFAULT_FOOTNOTE_SHAPE, &DEFAULT_ENDNOTE_SHAPE] {
            children.push(RecordNode {
                tag: tag::FOOTNOTE_SHAPE,
                data: shape.to_vec(),
                children: Vec::new(),
            });
        }
        for bf in &DEFAULT_PAGE_BORDER_FILLS {
            children.push(RecordNode {
                tag: tag::PAGE_BORDER_FILL,
                data: bf.to_vec(),
                children: Vec::new(),
            });
        }
    } else {
        children.extend(def.extras.iter().map(opaque_to_node));
    }
    RecordNode {
        tag: tag::CTRL_HEADER,
        data: w.into_bytes(),
        children,
    }
}

/// Sums total table-cell grid width/height, skipping merged cells and adding column/row maxima.
/// Matches HWPX grid estimation in write/section.rs write_table.
fn table_grid_totals(table: &Table) -> (i32, i32) {
    let mut col_w = vec![0i64; table.cols.max(1) as usize];
    let mut row_h = vec![0i64; table.rows.max(1) as usize];
    for cell in &table.cells {
        let (c, r) = (cell.col as usize, cell.row as usize);
        if cell.col_span == 1 && c < col_w.len() {
            col_w[c] = col_w[c].max(i64::from(cell.width.0));
        }
        if cell.row_span == 1 && r < row_h.len() {
            row_h[r] = row_h[r].max(i64::from(cell.height.0));
        }
    }
    (
        col_w.iter().sum::<i64>() as i32,
        row_h.iter().sum::<i64>() as i32,
    )
}

fn emit_table(
    table: &Table,
    synthesize: bool,
    preserve_linesegs: bool,
    add_tracking_tail: bool,
    report: &mut WriteReport,
) -> RecordNode {
    let mut w = ByteWriter::new();
    w.write_bytes(b" lbt");
    if !table.common_data.is_empty() {
        // HWP5 origin: preserve original object-common attributes unchanged.
        w.write_bytes(&table.common_data);
    } else if let Some(pl) = &table.placement {
        // HWPX origin: synthesize from hp:pos/hp:sz/hp:outMargin and zOrder placement. Previously
        // forcing 0x082A2210 (treat-as-character off) made inline tables float outside body flow,
        // causing Hangul relayout overlaps/blank pages.
        w.write_u32(pl.synth_attr());
        w.write_i32(pl.vert_offset);
        w.write_i32(pl.horz_offset);
        // #296: converter-generated placement has unmeasured width/height=0. Writing zero collapses
        // the object in Hangul; use the HWPX writer's positive-dimension rule and fall back to
        // cell-grid sums, matching the placement-free branch below.
        let (width, height) = if pl.width > 0 && pl.height > 0 {
            (pl.width, pl.height)
        } else {
            table_grid_totals(table)
        };
        w.write_i32(width); // hp:sz width, more accurate than merged-cell sums.
        w.write_i32(height);
        w.write_i32(pl.z_order);
        for m in pl.out_margins {
            w.write_u16(m);
        }
        // Unique nonzero instance ID based on z-order; used for object linking with little
        // rendering effect.
        w.write_u32(0x5000_0000 | (pl.z_order as u32 & 0xffff));
        // INT32 at offset 36 controls prevention of page splitting: on=1/off=0 (specification
        // table 69). Measured zero in the genuine color_fill.hwp table. Previously this incorrectly
        // stored HWPX holdAnchorAndSO, a different semantic field with no HWP5 common-attribute
        // counterpart. Drop it during HWP5 synthesis; HWPX-to-HWPX still preserves pl.hold_anchor
        // in hp:pos anchor attributes.
        w.write_i32(0); // Prevent page splitting: off.
        // Object-description length WORD (len=0), the final required field in specification table 69
        // (>=5.0.0.5). Omitting it changes 45 to 44 bytes, below the 46-byte minimum
        // (4-byte control ID plus 42), so Hangul may read a short record and reject it. Emit the same field
        // as the synthesized emit_picture path.
        w.write_u16(0); // desc_len = 0
    } else {
        // Markdown origin: floating sample values calculated from cell sizes, preserving prior
        // behavior.
        let (total_w, total_h) = table_grid_totals(table);
        w.write_u32(0x082A_2210); // Attributes, sample value.
        w.write_u32(0); // Vertical offset.
        w.write_u32(0); // Horizontal offset.
        w.write_i32(total_w);
        w.write_i32(total_h);
        w.write_u32(0); // z-order
        for _ in 0..4 {
            w.write_u16(283); // Outer margin, sample value.
        }
        w.write_u32(0); // instance id
        w.write_i32(0); // Prevent page splitting.
        w.write_u16(0); // Object-description length WORD (len=0), required by specification table 69 for 46-byte consistency.
    }

    let mut tw = ByteWriter::new();
    tw.write_u32(table.attr);
    tw.write_u16(table.rows);
    tw.write_u16(table.cols);
    tw.write_u16(table.cell_spacing);
    for m in table.inner_margins {
        tw.write_u16(m);
    }
    for c in &table.row_cell_counts {
        tw.write_u16(*c);
    }
    tw.write_u16(table.border_fill.0);
    if table.table_tail.is_empty() {
        tw.write_u16(0); // Region-attribute size (5.0.1.0+).
    } else {
        tw.write_bytes(&table.table_tail);
    }

    // Table 71 caption records precede the TABLE record.
    let mut children = match &table.caption {
        Some(caption) => emit_caption_records(
            caption,
            synthesize,
            preserve_linesegs,
            add_tracking_tail,
            report,
        ),
        None => Vec::new(),
    };
    children.push(RecordNode {
        tag: tag::TABLE,
        data: tw.into_bytes(),
        children: Vec::new(),
    });
    for cell in &table.cells {
        children.push(emit_cell_header(cell));
        for p in &cell.paragraphs {
            children.push(emit_paragraph(
                p,
                synthesize,
                preserve_linesegs,
                add_tracking_tail,
                report,
            ));
        }
    }
    children.extend(table.extras.iter().map(opaque_to_node));
    RecordNode {
        tag: tag::CTRL_HEADER,
        data: w.into_bytes(),
        children,
    }
}

/// Caption LIST_HEADER (tables 71-73, 22 bytes), inverse of
/// `body_text::parse_caption_header`.
fn emit_caption_header(caption: &Caption) -> RecordNode {
    let mut w = ByteWriter::new();
    w.write_i32(caption.paragraphs.len() as i32);
    // Reconstruct only text direction (table 65 bits 0-2); the IR does not
    // preserve line wrapping (bits 3-4) or vertical alignment (bits 5-6).
    // TODO(hancom-verify): measure upper listflag bits in a genuine caption.
    w.write_u32(match caption.direction {
        CaptionDirection::Horizontal => 0,
        CaptionDirection::Vertical => 1,
    });
    let side = match caption.side {
        CaptionSide::Left => 0,
        CaptionSide::Right => 1,
        CaptionSide::Top => 2,
        CaptionSide::Bottom => 3,
    };
    // Bit 2 extends caption width through margins (full-size), mapped to None.
    w.write_u32(side | (u32::from(caption.width.is_none()) << 2));
    w.write_i32(caption.width.unwrap_or(0).min(i32::MAX as u32) as i32);
    w.write_u16(caption.gap.clamp(0, i32::from(u16::MAX)) as u16);
    w.write_i32(caption.last_width.min(i32::MAX as u32) as i32);
    RecordNode {
        tag: tag::LIST_HEADER,
        data: w.into_bytes(),
        children: Vec::new(),
    }
}

/// Caption record sequence (LIST_HEADER plus paragraphs). It precedes the
/// object record such as TABLE, matching pyhwp `seen_table_body` behavior.
fn emit_caption_records(
    caption: &Caption,
    synthesize: bool,
    preserve_linesegs: bool,
    add_tracking_tail: bool,
    warnings: &mut WriteReport,
) -> Vec<RecordNode> {
    let mut out = vec![emit_caption_header(caption)];
    for p in &caption.paragraphs {
        out.push(emit_paragraph(
            p,
            synthesize,
            preserve_linesegs,
            add_tracking_tail,
            warnings,
        ));
    }
    out
}

fn emit_cell_header(cell: &Cell) -> RecordNode {
    let mut w = ByteWriter::new();
    w.write_i32(cell.paragraphs.len() as i32);
    w.write_u32(cell.list_attr);
    w.write_u16(cell.col);
    w.write_u16(cell.row);
    w.write_u16(cell.col_span);
    w.write_u16(cell.row_span);
    w.write_i32(cell.width.0);
    w.write_i32(cell.height.0);
    for m in cell.margins {
        w.write_u16(m);
    }
    w.write_u16(cell.border_fill.0);
    if cell.header_tail.is_empty() {
        w.write_bytes(&synthesized_cell_header_tail(cell.width.0));
    } else {
        w.write_bytes(&cell.header_tail);
    }
    RecordNode {
        tag: tag::LIST_HEADER,
        data: w.into_bytes(),
        children: Vec::new(),
    }
}

/// Measured 46-byte genuine-file layout padding: text width (repeats the cell
/// width) + 8 reserved bytes. These are the bytes the writer synthesizes when a
/// cell LIST_HEADER's `header_tail` is empty — the canonicalizer uses the same
/// projection, so this function is the single source of truth.
pub fn synthesized_cell_header_tail(width: i32) -> Vec<u8> {
    let mut v = Vec::with_capacity(12);
    v.extend_from_slice(&width.to_le_bytes());
    v.extend_from_slice(&[0u8; 8]);
    v
}

fn emit_picture(
    pic: &Picture,
    synthesize: bool,
    preserve_linesegs: bool,
    add_tracking_tail: bool,
    warnings: &mut WriteReport,
) -> RecordNode {
    let mut w = ByteWriter::new();
    w.write_bytes(b" osg");
    if pic.common_data.is_empty() {
        // HWPX/Markdown origin: minimal object-common attributes, treat-as-character.
        warnings.warning("그림 개체 공통 속성을 기본값으로 생성 — 한글에서 확인 필요");
        w.write_u32(u32::from(pic.treat_as_char)); // Attributes.
        w.write_u32(0); // Vertical offset.
        w.write_u32(0); // Horizontal offset.
        w.write_i32(pic.width.0);
        w.write_i32(pic.height.0);
        w.write_u32(0); // z-order
        for _ in 0..4 {
            w.write_u16(0); // Outer margins.
        }
        w.write_u32(0); // instance id
        w.write_i32(0); // Prevent page splitting.
        let mut description = Vec::new();
        append_gso_description(&mut description, pic.description.as_deref(), warnings);
        w.write_bytes(&description);
    } else {
        w.write_bytes(&pic.common_data);
    }
    // Table 71 caption follows common object properties and leads child records.
    let mut children = match &pic.caption {
        Some(caption) => emit_caption_records(
            caption,
            synthesize,
            preserve_linesegs,
            add_tracking_tail,
            warnings,
        ),
        None => Vec::new(),
    };
    children.extend(pic.extras.iter().map(opaque_to_node));
    RecordNode {
        tag: tag::CTRL_HEADER,
        data: w.into_bytes(),
        children,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Raw bits 7 are representable and decode as semantic level 8, though K1 observed no
    /// genuine sample. Preserve their original record bytes rather than invent an encoding.
    #[test]
    fn read_raw_7_numbered_shape_emits_byte_identical() {
        for tail_level in [0_u32, 5, 7] {
            let mut data = vec![0; 58];
            data[..4].copy_from_slice(&((2_u32 << 23) | (7 << 25)).to_le_bytes());
            data[30..32].copy_from_slice(&1_u16.to_le_bytes());
            data[50..54].copy_from_slice(&160_i32.to_le_bytes());
            data[54..58].copy_from_slice(&tail_level.to_le_bytes());
            let roots = [RecordNode {
                tag: tag::ID_MAPPINGS,
                data: Vec::new(),
                children: vec![RecordNode {
                    tag: tag::PARA_SHAPE,
                    data: data.clone(),
                    children: Vec::new(),
                }],
            }];
            let (header, warnings) = crate::doc_info::parse_doc_info(&roots);
            assert!(warnings.is_empty(), "{warnings:?}");
            assert_eq!(header.para_shapes.len(), 1);
            let shape = &header.para_shapes[0];
            assert_eq!(shape.list_level, Some(8));
            assert_eq!(
                emit_para_shape(shape).data,
                data,
                "a read raw-7 record must retain both its level bits and tail (tail={tail_level})"
            );
        }
    }

    #[test]
    fn read_official_level_override_emits_byte_identical() {
        let mut data = vec![0; 58];
        data[..4].copy_from_slice(&((2_u32 << 23) | (6 << 25)).to_le_bytes());
        data[30..32].copy_from_slice(&8_u16.to_le_bytes());
        data[50..54].copy_from_slice(&160_i32.to_le_bytes());
        data[54..58].copy_from_slice(&5_u32.to_le_bytes());
        let mut children: Vec<_> = (0..8)
            .map(|_| RecordNode {
                tag: tag::NUMBERING,
                data: crate::numbering::official_eight_level_data(),
                children: Vec::new(),
            })
            .collect();
        children.push(RecordNode {
            tag: tag::PARA_SHAPE,
            data: data.clone(),
            children: Vec::new(),
        });
        let (header, warnings) = crate::doc_info::parse_doc_info(&[RecordNode {
            tag: tag::ID_MAPPINGS,
            data: Vec::new(),
            children,
        }]);
        assert!(warnings.is_empty(), "{warnings:?}");
        let shape = &header.para_shapes[0];
        assert_eq!(shape.numbering_id, 7);
        assert_eq!(shape.list_level, Some(8), "the official override applies");
        assert_eq!(crate::doc_info::semantic_numbered_level(shape), 7);
        assert_eq!(
            emit_para_shape(shape).data,
            data,
            "the official reader override must not cause an untouched tail to be rewritten"
        );
    }

    #[test]
    fn edited_and_grafted_numbered_shapes_still_emit_zero_based_levels() {
        let mut edited = ParaShape {
            attr1: (2 << 23) | (6 << 25),
            list_level: Some(7),
            ..ParaShape::default()
        };
        edited.set_head_level(3);
        let grafted = ParaShape {
            attr1: (2 << 23) | (3 << 25),
            list_level: None,
            ..ParaShape::default()
        };
        for shape in [edited, grafted] {
            let emitted = emit_para_shape(&shape).data;
            let attr = u32::from_le_bytes(emitted[..4].try_into().unwrap());
            assert_eq!((attr >> 25) & 7, 2);
            assert_eq!(u32::from_le_bytes(emitted[54..58].try_into().unwrap()), 2);
        }
    }

    fn paragraph_id_record(id: u32) -> RecordNode {
        let mut data = vec![0x5a; 24];
        data[18..22].copy_from_slice(&id.to_le_bytes());
        RecordNode {
            tag: tag::PARA_HEADER,
            data,
            children: Vec::new(),
        }
    }

    #[test]
    fn instance_id_allocation_reserves_later_ids_and_preserves_first_occurrences() {
        let original = vec![
            paragraph_id_record(7),
            paragraph_id_record(7),
            paragraph_id_record(0),
            paragraph_id_record(0x1000_0001),
            paragraph_id_record(0x1000_0002),
        ];
        for (deduplicate, expected) in [
            (false, vec![7, 7, 0x1000_0003, 0x1000_0001, 0x1000_0002]),
            (
                true,
                vec![7, 0x1000_0003, 0x1000_0004, 0x1000_0001, 0x1000_0002],
            ),
        ] {
            let mut roots = original.clone();
            let mut used = BTreeSet::new();
            collect_instance_ids(&roots, &mut used);
            assign_instance_ids(
                &mut roots,
                &mut 0x1000_0000,
                &mut used,
                &mut BTreeSet::new(),
                deduplicate,
            )
            .unwrap();
            assert_eq!(
                roots
                    .iter()
                    .filter_map(para_header_instance_id)
                    .collect::<Vec<_>>(),
                expected,
            );
        }
    }

    #[test]
    fn instance_id_allocation_wraps_without_zero_or_reuse() {
        let mut used = BTreeSet::from([1, 2, u32::MAX]);
        let mut counter = u32::MAX - 1;
        assert_eq!(next_instance_id(&mut counter, &mut used).unwrap(), 3);
        assert_eq!(next_instance_id(&mut counter, &mut used).unwrap(), 4);
        assert!(!used.contains(&0));
    }

    #[test]
    fn merge_instance_ids_cover_raw_control_trees_without_rewriting_other_bytes() {
        let mut document = hwp_convert::from_markdown("body\n");
        document.meta.source_format = "hwp5".to_string();
        let paragraph = &mut document.sections[0].paragraphs[0];
        paragraph.header.instance_id = 7;
        let raw = OpaqueRecord {
            tag: tag::SHAPE_COMPONENT,
            data: vec![0x31; 17],
            children: vec![OpaqueRecord {
                tag: tag::LIST_HEADER,
                data: vec![0x42; 10],
                children: [7, 0]
                    .into_iter()
                    .map(|id| OpaqueRecord {
                        tag: tag::PARA_HEADER,
                        data: paragraph_id_record(id).data,
                        children: Vec::new(),
                    })
                    .collect(),
            }],
        };
        let control_index = paragraph.controls.len() as u32;
        paragraph.chars.push(HwpChar::ExtCtrl {
            code: hwp_model::ctrl_char::OBJECT,
            ctrl_id: *b"gso ",
            payload: vec![0; 12],
            ctrl_index: Some(control_index),
        });
        paragraph
            .controls
            .push(Control::Generic(hwp_model::GenericControl {
                ctrl_id: *b"gso ",
                data: vec![0; 36],
                paragraph_lists: Vec::new(),
                extras: Vec::new(),
                raw_children: vec![raw],
                gso_shapes: Vec::new(),
                equation: None,
                column_def: None,
                caption: None,
                hwpx_raw_xml: None,
                container_box: None,
            }));
        document.sections.push(document.sections[0].clone());

        let original =
            materialize_document(&document, &WriteOptions::default(), false, false).unwrap();
        let options = WriteOptions {
            deduplicate_instance_ids: true,
            ..WriteOptions::default()
        };
        let merged = materialize_document(&document, &options, false, false).unwrap();
        let repeated = materialize_document(&document, &options, false, false).unwrap();
        assert_eq!(
            merged.streams, repeated.streams,
            "allocation is deterministic"
        );

        fn collect_and_clear(roots: &mut [RecordNode], ids: &mut Vec<u32>) {
            for node in roots {
                if let Some(id) = para_header_instance_id(node) {
                    ids.push(id);
                    node.data[18..22].fill(0);
                }
                collect_and_clear(&mut node.children, ids);
            }
        }
        let mut original_ids = Vec::new();
        let mut merged_ids = Vec::new();
        for (path, bytes) in &original.streams {
            let written = &merged.streams[path];
            if path.starts_with("/BodyText/") {
                let mut before = scan_stream(bytes, ScanMode::Strict).unwrap().roots;
                let mut after = scan_stream(written, ScanMode::Strict).unwrap().roots;
                collect_and_clear(&mut before, &mut original_ids);
                collect_and_clear(&mut after, &mut merged_ids);
                assert_eq!(before, after, "only paragraph ID bytes change: {path}");
            } else {
                assert_eq!(bytes, written, "unrelated stream: {path}");
            }
        }
        assert_eq!(original_ids, vec![7, 7, 0, 7, 7, 0]);
        assert_eq!(
            merged_ids,
            vec![
                7,
                0x1000_0001,
                0x1000_0002,
                0x1000_0003,
                0x1000_0004,
                0x1000_0005
            ],
        );
    }

    /// #377: a definition past the raw records (one an edit grafted onto an HWP5 source) gets a
    /// synthesized record; the raw ones stay as they are, and an empty header keeps its single
    /// default NUMBERING.
    #[test]
    fn records_cover_definitions_grafted_past_the_raw_ones() {
        let raw = |byte: u8| RawEntry {
            data: vec![byte; 4],
            children: Vec::new(),
        };
        let mut h = hwp_model::DocHeader::default();
        assert_eq!(numbering_records(&h).len(), 1);
        assert!(bullet_records(&h).is_empty());

        h.numberings = vec![raw(1)];
        h.numbering_levels = vec![Vec::new(), Vec::new()];
        h.bullets = vec![raw(2)];
        h.bullet_chars = vec!['x', 'y'];
        let numberings = numbering_records(&h);
        assert_eq!(numberings.len(), 2);
        assert_eq!(numberings[0], raw(1));
        assert_eq!(numberings[1].data, DEFAULT_NUMBERING_DATA);
        let bullets = bullet_records(&h);
        assert_eq!(
            bullets,
            vec![
                raw(2),
                RawEntry {
                    data: make_bullet_data('y'),
                    children: Vec::new()
                }
            ]
        );
        assert_eq!(synthesized_numbering_levels().len(), 7);

        // An HWP5 source reads one modeled entry per raw record: nothing is synthesized.
        h.numbering_levels.truncate(1);
        h.bullet_chars.truncate(1);
        assert_eq!(numbering_records(&h), vec![raw(1)]);
        assert_eq!(bullet_records(&h), vec![raw(2)]);
    }

    fn drawn_default_numbering_levels() -> Vec<hwp_model::NumLevel> {
        let mut levels = synthesized_numbering_levels();
        for (level, format) in levels.iter_mut().zip(DEFAULT_NUMBERING_FORMATS) {
            level.fmt = format;
        }
        levels
    }

    fn assert_numbering_default_substitution_warning(
        category: &str,
        level_index: usize,
        change: impl FnOnce(&mut hwp_model::NumLevel),
    ) {
        let mut levels = drawn_default_numbering_levels();
        change(&mut levels[level_index]);
        let private_marker = "PRIVATE_NUMBERING_MARKER^1";
        let mut raw_backed_levels = levels.clone();
        raw_backed_levels[0].template = private_marker.to_string();
        let raw = RawEntry {
            data: DEFAULT_NUMBERING_DATA.to_vec(),
            children: Vec::new(),
        };
        let mut doc = Document::default();
        doc.header.numberings = vec![raw.clone()];
        doc.header.numbering_levels = vec![raw_backed_levels, levels];
        let mut report = WriteReport::new();
        let roots = emit_doc_info(&doc, &mut report);
        let records: Vec<_> = roots
            .iter()
            .flat_map(|root| &root.children)
            .filter(|node| node.tag == tag::NUMBERING)
            .collect();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].data, raw.data);
        assert_eq!(records[1].data, DEFAULT_NUMBERING_DATA);

        let substitutions: Vec<_> = report
            .warnings
            .iter()
            .filter(|warning| warning.contains("numbering_default_substitution"))
            .collect();
        assert_eq!(
            substitutions.len(),
            1,
            "default substitution must warn exactly once for the unbacked definition: {:?}",
            report.warnings
        );
        let warning = substitutions[0];
        assert!(warning.contains("definition=1"), "{warning}");
        assert!(
            warning.contains(&format!("levels=[{level_index}]")),
            "{warning}"
        );
        assert!(
            warning.contains(&format!("changed={category}")),
            "{warning}"
        );
        assert!(warning.contains("기본"), "{warning}");
        assert!(warning.contains("보존"), "{warning}");
        assert!(warning.contains("HWPX"), "{warning}");
        assert!(!warning.contains(private_marker), "{warning}");
        assert!(report.preservation.events.is_empty());
    }

    #[test]
    fn emit_doc_info_warns_for_used_eighth_level_in_each_emitted_paragraph_scope() {
        let numbered = Paragraph {
            para_shape: hwp_model::ParaShapeId(1),
            chars: vec![HwpChar::Text('x')],
            ..Paragraph::default()
        };
        let caption = Caption {
            side: CaptionSide::Bottom,
            direction: CaptionDirection::Horizontal,
            gap: 0,
            width: None,
            last_width: 0,
            paragraphs: vec![numbered.clone()],
        };
        for scope in [
            "body",
            "cell",
            "table-caption",
            "picture-caption",
            "header",
            "footer",
            "shape-caption",
        ] {
            let mut root = Paragraph::default();
            match scope {
                "body" => root = numbered.clone(),
                "cell" | "table-caption" => {
                    let mut table = synth_table(None);
                    if scope == "cell" {
                        table.cells.push(Cell {
                            list_attr: 0,
                            col: 0,
                            row: 0,
                            col_span: 1,
                            row_span: 1,
                            width: hwp_model::HwpUnit(1000),
                            height: hwp_model::HwpUnit(1000),
                            margins: [0; 4],
                            border_fill: hwp_model::BorderFillId(0),
                            header_tail: Vec::new(),
                            paragraphs: vec![numbered.clone()],
                        });
                    } else {
                        table.caption = Some(caption.clone());
                    }
                    root.controls.push(Control::Table(table));
                }
                "picture-caption" => {
                    let Control::Picture(mut picture) = picture_with_instance_id(1) else {
                        unreachable!()
                    };
                    picture.caption = Some(caption.clone());
                    root.controls.push(Control::Picture(picture));
                }
                _ => {
                    let Control::Generic(mut generic) = shape_caption_holding(Vec::new()) else {
                        unreachable!()
                    };
                    if scope == "shape-caption" {
                        generic.caption = Some(caption.clone());
                    } else {
                        generic.ctrl_id = if scope == "header" {
                            *b"head"
                        } else {
                            *b"foot"
                        };
                        generic.caption = None;
                        generic.paragraph_lists.push(hwp_model::ParagraphList {
                            header_data: Vec::new(),
                            paragraphs: vec![numbered.clone()],
                        });
                    }
                    root.controls.push(Control::Generic(generic));
                }
            }
            let mut doc = Document::default();
            let mut levels = drawn_default_numbering_levels();
            let mut eighth = levels[0].clone();
            eighth.template = "^8".to_string();
            levels.push(eighth);
            // Only definition 1 is referenced; an unused shape is not a paragraph use.
            doc.header.numbering_levels = vec![levels.clone(), levels];
            doc.header.para_shapes = vec![
                ParaShape::default(),
                ParaShape {
                    attr1: 2 << 23,
                    list_level: Some(8),
                    numbering_id: 1,
                    ..ParaShape::default()
                },
            ];
            let mut unused_report = WriteReport::new();
            emit_doc_info(&doc, &mut unused_report);
            assert!(
                unused_report.warnings.is_empty(),
                "{scope}: {:?}",
                unused_report.warnings
            );
            doc.sections.push(Section {
                paragraphs: vec![root],
                extras: Vec::new(),
            });
            let mut report = WriteReport::new();
            let roots = emit_doc_info(&doc, &mut report);
            assert!(
                roots
                    .iter()
                    .flat_map(|r| &r.children)
                    .filter(|r| r.tag == tag::NUMBERING)
                    .all(|r| r.data == DEFAULT_NUMBERING_DATA)
            );
            assert_eq!(report.warnings.len(), 1, "{scope}: {:?}", report.warnings);
            assert!(
                report.warnings[0].contains("definition=1 levels=[7]"),
                "{scope}: {:?}",
                report.warnings
            );
        }
    }

    #[test]
    fn emit_doc_info_warns_when_default_numbering_substitutes_template() {
        assert_numbering_default_substitution_warning("template", 0, |level| {
            level.template = "PRIVATE_NUMBERING_MARKER^1".to_string();
        });
    }

    #[test]
    fn emit_doc_info_warns_when_default_numbering_substitutes_start() {
        assert_numbering_default_substitution_warning("start", 2, |level| {
            level.start = 7;
        });
    }

    #[test]
    fn emit_doc_info_warns_when_default_numbering_substitutes_drawn_format() {
        // The reader reports Digit here, but the emitted record draws Hangul syllables.
        assert_numbering_default_substitution_warning("format", 1, |level| {
            level.fmt = hwp_model::NumFmt::Digit;
        });
    }

    #[test]
    fn emit_doc_info_does_not_warn_for_exact_drawn_numbering_defaults() {
        let mut doc = Document::default();
        doc.header.numbering_levels = vec![drawn_default_numbering_levels()];
        let mut report = WriteReport::new();
        let roots = emit_doc_info(&doc, &mut report);
        let numbering = roots
            .iter()
            .flat_map(|root| &root.children)
            .find(|node| node.tag == tag::NUMBERING)
            .expect("the existing writer emits its default record");
        assert_eq!(numbering.data, DEFAULT_NUMBERING_DATA);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert!(report.preservation.events.is_empty());
    }

    #[test]
    fn emit_doc_info_does_not_warn_for_raw_backed_numbering() {
        let mut levels = drawn_default_numbering_levels();
        levels[0].template = "PRIVATE_RAW_BACKED_MARKER^1".to_string();
        levels[1].fmt = hwp_model::NumFmt::LatinUpper;
        levels[2].start = 7;
        let raw = RawEntry {
            data: DEFAULT_NUMBERING_DATA.to_vec(),
            children: Vec::new(),
        };
        let mut doc = Document::default();
        doc.header.numberings = vec![raw.clone()];
        doc.header.numbering_levels = vec![levels];
        let mut report = WriteReport::new();
        let roots = emit_doc_info(&doc, &mut report);
        let numbering = roots
            .iter()
            .flat_map(|root| &root.children)
            .find(|node| node.tag == tag::NUMBERING)
            .expect("the existing raw record is emitted");
        assert_eq!(numbering.data, raw.data);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert!(report.preservation.events.is_empty());
    }

    #[test]
    fn emit_doc_info_does_not_warn_after_official_numbering_materialization() {
        let data = crate::numbering::official_eight_level_data();
        let levels = crate::numbering::parse_official_eight_level_data(&data)
            .expect("the evidenced official record has its modeled levels");
        let mut doc = Document::default();
        doc.header.numbering_levels = vec![levels; 8];
        materialize_evidenced_official_numbering(&mut doc)
            .expect("the existing official path materializes its preserved raw records");
        assert_eq!(doc.header.numberings.len(), 8);
        let mut report = WriteReport::new();
        let roots = emit_doc_info(&doc, &mut report);
        let records: Vec<_> = roots
            .iter()
            .flat_map(|root| &root.children)
            .filter(|node| node.tag == tag::NUMBERING)
            .collect();
        assert_eq!(records.len(), 8);
        assert!(records.iter().all(|record| record.data == data));
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert!(report.preservation.events.is_empty());
    }

    /// #225: the attr1 fold and the tail patch must be byte-level no-ops for a shape whose
    /// stored bits already agree with the IR spacing fields - that is exactly what keeps the
    /// hwp5 identity round trip byte-identical. Also pins the two short-tail branches: a tail
    /// under 12 bytes is emitted verbatim and never grown, and the empty tail keeps today's
    /// four trailing u32 values.
    #[test]
    fn emit_para_shape_preserves_agreeing_tail_and_never_grows_a_short_one() {
        // attr1: bit7 (KEEP_WORD) plus spacing type 1 in bits 0..1, agreeing with the fields.
        const ATTR1: u32 = 0x0000_0081;
        const SPACING: i32 = 320;

        let shape = |tail: Vec<u8>| ParaShape {
            attr1: ATTR1,
            indent: 0,
            margin_left: 100,
            margin_right: 200,
            spacing_top: 300,
            spacing_bottom: 400,
            line_spacing_old: SPACING,
            tab_def_id: 1,
            numbering_id: 0,
            list_level: None,
            border_fill_id: 2,
            border_offsets: [1, 2, 3, 4],
            line_spacing_type: 1,
            line_spacing: SPACING,
            tail,
        };

        let mut head = Vec::new();
        head.extend_from_slice(&ATTR1.to_le_bytes());
        head.extend_from_slice(&100i32.to_le_bytes()); // margin_left
        head.extend_from_slice(&200i32.to_le_bytes()); // margin_right
        head.extend_from_slice(&0i32.to_le_bytes()); // indent
        head.extend_from_slice(&300i32.to_le_bytes()); // spacing_top
        head.extend_from_slice(&400i32.to_le_bytes()); // spacing_bottom
        head.extend_from_slice(&SPACING.to_le_bytes()); // line_spacing_old
        head.extend_from_slice(&1u16.to_le_bytes()); // tab_def_id
        head.extend_from_slice(&0u16.to_le_bytes()); // numbering_id (head_type 0)
        head.extend_from_slice(&2u16.to_le_bytes()); // border_fill_id
        for offset in [1u16, 2, 3, 4] {
            head.extend_from_slice(&offset.to_le_bytes());
        }

        // 16-byte tail already carrying line_spacing at [8..12]: emitted unchanged.
        let mut tail = vec![0u8; 16];
        tail[8..12].copy_from_slice(&SPACING.to_le_bytes());
        let mut expected = head.clone();
        expected.extend_from_slice(&tail);
        assert_eq!(
            emit_para_shape(&shape(tail.clone())).data,
            expected,
            "일치하는 tail은 바이트 동일하게 재방출되어야 한다"
        );

        // 8-byte tail: too short for the 5.0.2.5+ window, emitted verbatim and not padded.
        let short = vec![0xAAu8; 8];
        let mut expected = head.clone();
        expected.extend_from_slice(&short);
        let emitted = emit_para_shape(&shape(short.clone())).data;
        assert_eq!(emitted, expected, "12바이트 미만 tail은 그대로 방출한다");
        assert_eq!(emitted.len(), head.len() + 8, "짧은 tail은 늘어나지 않는다");

        // Empty tail: unchanged 5.1.0.1 filler (attr2, attr3, line spacing, trailing 4B).
        let emitted = emit_para_shape(&shape(Vec::new())).data;
        assert_eq!(emitted.len(), head.len() + 16);
        let filler: Vec<u32> = emitted[head.len()..]
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(filler, vec![0, 0, SPACING as u32, 0]);
    }

    fn source_rewrite_path(label: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "hwp5-source-rewrite-{label}-{}-{unique}.hwp",
            std::process::id(),
        ))
    }

    fn add_opaque_source_entries(path: &Path) {
        let mut cfb = cfb::open_rw(path).unwrap();
        cfb.create_stream("/MemoExtended")
            .unwrap()
            .write_all(b"opaque-memo-payload")
            .unwrap();
        cfb.create_storage_all("/OpaqueStorage").unwrap();
        cfb.create_stream("/OpaqueStorage/Hidden")
            .unwrap()
            .write_all(b"opaque-hidden-payload")
            .unwrap();
        cfb.flush().unwrap();
    }

    fn raw_streams(path: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut container = crate::Hwp5Container::open(path).unwrap();
        container
            .list_streams()
            .into_iter()
            .map(|stream| {
                let bytes = container.read_stream_raw(&stream.path).unwrap();
                (stream.path, bytes)
            })
            .collect()
    }

    fn find_control_node(nodes: &[RecordNode], reversed_id: &[u8; 4]) -> Option<RecordNode> {
        for node in nodes {
            if node.tag == tag::CTRL_HEADER && node.data.get(..4) == Some(reversed_id) {
                return Some(node.clone());
            }
            if let Some(found) = find_control_node(&node.children, reversed_id) {
                return Some(found);
            }
        }
        None
    }

    fn section_definition_node(path: &Path) -> RecordNode {
        let mut container = crate::Hwp5Container::open(path).unwrap();
        let bytes = container.read_record_stream("/BodyText/Section0").unwrap();
        let roots = scan_stream(&bytes, ScanMode::Strict).unwrap().roots;
        find_control_node(&roots, b"dces").expect("section definition")
    }

    fn prepend_section_ctrl_data(path: &Path) {
        let mut container = crate::Hwp5Container::open(path).unwrap();
        let compressed = container.file_header().is_compressed();
        let bytes = container.read_record_stream("/BodyText/Section0").unwrap();
        drop(container);
        let mut roots = scan_stream(&bytes, ScanMode::Strict).unwrap().roots;

        fn prepend(nodes: &mut [RecordNode]) -> bool {
            for node in nodes {
                if node.tag == tag::CTRL_HEADER && node.data.get(..4) == Some(b"dces") {
                    node.children.insert(
                        0,
                        RecordNode {
                            tag: tag::CTRL_DATA,
                            data: b"source-order-sentinel".to_vec(),
                            children: Vec::new(),
                        },
                    );
                    return true;
                }
                if prepend(&mut node.children) {
                    return true;
                }
            }
            false
        }
        assert!(prepend(&mut roots));
        let encoded = encode_compressed_stream(RecordNode::serialize_forest(&roots), compressed);
        let mut cfb = cfb::open_rw(path).unwrap();
        cfb.create_stream("/BodyText/Section0")
            .unwrap()
            .write_all(&encoded)
            .unwrap();
        cfb.flush().unwrap();
    }

    #[test]
    fn source_preserving_noop_is_an_exact_file_copy() {
        let source = source_rewrite_path("noop-source");
        let output = source_rewrite_path("noop-output");
        let document = hwp_convert::from_markdown("source preserving no-op");
        write_document_with_report(&document, &source, &WriteOptions::default()).unwrap();
        add_opaque_source_entries(&source);
        let original = crate::read_document(&source).unwrap().document;

        rewrite_document_with_report(
            &source,
            &original,
            &original,
            &output,
            &WriteOptions::default(),
        )
        .unwrap();

        assert_eq!(
            std::fs::read(&source).unwrap(),
            std::fs::read(&output).unwrap()
        );
        std::fs::remove_file(source).unwrap();
        std::fs::remove_file(output).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn source_preserving_rewrite_rejects_hardlink_output_alias() {
        let source = source_rewrite_path("alias-source");
        let output = source_rewrite_path("alias-output");
        let document = hwp_convert::from_markdown("source alias guard");
        write_document_with_report(&document, &source, &WriteOptions::default()).unwrap();
        std::fs::hard_link(&source, &output).unwrap();
        let original_bytes = std::fs::read(&source).unwrap();
        let original = crate::read_document(&source).unwrap().document;

        let error = rewrite_document_with_report(
            &source,
            &original,
            &original,
            &output,
            &WriteOptions::default(),
        )
        .unwrap_err();

        assert!(matches!(error, crate::error::Hwp5Error::SourceRewrite(_)));
        assert_eq!(std::fs::read(&source).unwrap(), original_bytes);
        std::fs::remove_file(output).unwrap();
        std::fs::remove_file(source).unwrap();
    }

    #[test]
    fn source_mutation_plan_updates_docinfo_when_section_count_changes() {
        let original = hwp_convert::from_markdown("first section");
        let mut added = original.clone();
        added.sections.push(hwp_model::Section::default());
        let add_plan = HwpMutationPlan::derive(&original, &added);
        assert!(add_plan.header_changed);
        assert_eq!(add_plan.changed_sections, BTreeSet::from([1]));
        assert!(add_plan.removed_sections.is_empty());

        let remove_plan = HwpMutationPlan::derive(&added, &original);
        assert!(remove_plan.header_changed);
        assert_eq!(remove_plan.removed_sections, BTreeSet::from([1]));
    }

    #[test]
    fn source_mutation_plan_does_not_rewrite_docinfo_for_bin_payload_only() {
        let original = hwp_convert::from_markdown("binary payload");
        let mut edited = original.clone();
        edited.bin_streams.push(hwp_model::BinStream {
            name: "BIN0001.png".to_string(),
            data: vec![1, 2, 3],
        });
        let with_bin = edited.clone();
        edited.bin_streams[0].data = vec![4, 5, 6];

        let plan = HwpMutationPlan::derive(&with_bin, &edited);
        assert!(!plan.header_changed);
        assert_eq!(
            plan.changed_bins,
            BTreeSet::from(["bin0001.png".to_string()])
        );
        assert!(plan.removed_bins.is_empty());
    }

    #[test]
    fn control_matching_preserves_shifted_unique_controls_without_guessing_duplicates() {
        fn generic(id: [u8; 4]) -> Control {
            Control::Generic(hwp_model::GenericControl {
                ctrl_id: id,
                data: Vec::new(),
                paragraph_lists: Vec::new(),
                extras: Vec::new(),
                raw_children: Vec::new(),
                gso_shapes: Vec::new(),
                equation: None,
                column_def: None,
                caption: None,
                hwpx_raw_xml: None,
                container_box: None,
            })
        }

        let first = generic(*b"one ");
        let second = generic(*b"two ");
        assert_eq!(
            match_controls(&[first, second.clone()], std::slice::from_ref(&second)),
            vec![Some(1)]
        );
        assert_eq!(
            match_controls(&[second.clone(), second.clone()], &[second]),
            vec![None]
        );
    }

    #[test]
    fn source_preserving_metadata_edit_changes_only_summary_stream() {
        let source = source_rewrite_path("metadata-source");
        let output = source_rewrite_path("metadata-output");
        let document = hwp_convert::from_markdown("source preserving metadata");
        write_document_with_report(&document, &source, &WriteOptions::default()).unwrap();
        add_opaque_source_entries(&source);
        let original = crate::read_document(&source).unwrap().document;
        let mut edited = original.clone();
        edited.metadata.title = Some("edited title".to_string());

        rewrite_document_with_report(
            &source,
            &original,
            &edited,
            &output,
            &WriteOptions {
                preserve_linesegs: true,
                ..WriteOptions::default()
            },
        )
        .unwrap();

        let source_streams = raw_streams(&source);
        let output_streams = raw_streams(&output);
        assert_eq!(
            source_streams.keys().collect::<Vec<_>>(),
            output_streams.keys().collect::<Vec<_>>()
        );
        for (path, source_bytes) in source_streams {
            if path == "/\u{5}HwpSummaryInformation" {
                assert_ne!(source_bytes, output_streams[&path]);
            } else {
                assert_eq!(
                    source_bytes, output_streams[&path],
                    "unexpected stream change: {path}"
                );
            }
        }
        assert_eq!(
            crate::read_document(&output)
                .unwrap()
                .document
                .metadata
                .title
                .as_deref(),
            Some("edited title")
        );
        std::fs::remove_file(source).unwrap();
        std::fs::remove_file(output).unwrap();
    }

    #[test]
    fn source_preserving_text_edit_keeps_opaque_streams_raw() {
        let source = source_rewrite_path("text-source");
        let output = source_rewrite_path("text-output");
        let document = hwp_convert::from_markdown("replace this text");
        write_document_with_report(&document, &source, &WriteOptions::default()).unwrap();
        add_opaque_source_entries(&source);
        let original = crate::read_document(&source).unwrap().document;
        let mut edited = original.clone();
        assert_eq!(
            hwp_convert::replace_text(&mut edited, "replace", "updated", true),
            1
        );

        rewrite_document_with_report(
            &source,
            &original,
            &edited,
            &output,
            &WriteOptions {
                preserve_linesegs: true,
                ..WriteOptions::default()
            },
        )
        .unwrap();

        let source_streams = raw_streams(&source);
        let output_streams = raw_streams(&output);
        for path in [
            "/FileHeader",
            "/DocInfo",
            "/MemoExtended",
            "/OpaqueStorage/Hidden",
        ] {
            assert_eq!(
                source_streams[path], output_streams[path],
                "unexpected stream change: {path}"
            );
        }
        assert_ne!(
            source_streams["/BodyText/Section0"],
            output_streams["/BodyText/Section0"]
        );
        assert_ne!(source_streams["/PrvText"], output_streams["/PrvText"]);
        assert!(
            crate::read_document(&output)
                .unwrap()
                .document
                .plain_text()
                .contains("updated")
        );
        std::fs::remove_file(source).unwrap();
        std::fs::remove_file(output).unwrap();
    }

    #[test]
    fn source_preserving_text_edit_keeps_unchanged_typed_control_subtrees_raw() {
        let source = source_rewrite_path("typed-control-source");
        let output = source_rewrite_path("typed-control-output");
        let document = hwp_convert::from_markdown("replace this text");
        write_document_with_report(&document, &source, &WriteOptions::default()).unwrap();
        prepend_section_ctrl_data(&source);
        let original = crate::read_document(&source).unwrap().document;
        let mut edited = original.clone();
        assert_eq!(
            hwp_convert::replace_text(&mut edited, "replace", "updated", true),
            1
        );

        rewrite_document_with_report(
            &source,
            &original,
            &edited,
            &output,
            &WriteOptions {
                preserve_linesegs: true,
                ..WriteOptions::default()
            },
        )
        .unwrap();

        let source_section = section_definition_node(&source);
        assert_eq!(source_section.children[0].tag, tag::CTRL_DATA);
        assert_eq!(source_section, section_definition_node(&output));
        assert!(
            crate::read_document(&output)
                .unwrap()
                .document
                .plain_text()
                .contains("updated")
        );
        std::fs::remove_file(source).unwrap();
        std::fs::remove_file(output).unwrap();
    }

    /// Caption LIST_HEADER carrying listflag bits the IR does not model (wrapping and vertical
    /// alignment live above the direction bits `emit_caption_header` reconstructs).
    const CAPTION_LIST_ATTR: u32 = 0x0000_0068;

    fn table_control_node(path: &Path) -> RecordNode {
        let mut container = crate::Hwp5Container::open(path).unwrap();
        let bytes = container.read_record_stream("/BodyText/Section0").unwrap();
        let roots = scan_stream(&bytes, ScanMode::Strict).unwrap().roots;
        find_control_node(&roots, b" lbt").expect("table control")
    }

    /// Gives the source table a caption whose listflags the IR cannot round-trip and an
    /// unknown record wedged between the TABLE record and the first cell - the two things a
    /// full table regeneration would rewrite or move.
    fn inject_table_caption_and_opaque(path: &Path) {
        let mut container = crate::Hwp5Container::open(path).unwrap();
        let compressed = container.file_header().is_compressed();
        let bytes = container.read_record_stream("/BodyText/Section0").unwrap();
        drop(container);
        let mut roots = scan_stream(&bytes, ScanMode::Strict).unwrap().roots;

        fn inject(nodes: &mut [RecordNode]) -> bool {
            for node in nodes {
                if node.tag == tag::CTRL_HEADER && node.data.get(..4) == Some(b" lbt") {
                    let table_index = node
                        .children
                        .iter()
                        .position(|child| child.tag == tag::TABLE)
                        .expect("TABLE record");
                    let paragraph = node
                        .children
                        .iter()
                        .find(|child| child.tag == tag::PARA_HEADER)
                        .expect("cell paragraph")
                        .clone();
                    let mut data = Vec::with_capacity(22);
                    data.extend_from_slice(&1_i32.to_le_bytes()); // paragraph count
                    data.extend_from_slice(&CAPTION_LIST_ATTR.to_le_bytes());
                    data.extend_from_slice(&3_u32.to_le_bytes()); // side = bottom, explicit width
                    data.extend_from_slice(&12_345_i32.to_le_bytes()); // width
                    data.extend_from_slice(&283_u16.to_le_bytes()); // gap
                    data.extend_from_slice(&54_321_i32.to_le_bytes()); // maximum text extent
                    node.children.insert(
                        table_index,
                        RecordNode {
                            tag: tag::LIST_HEADER,
                            data,
                            children: Vec::new(),
                        },
                    );
                    node.children.insert(table_index + 1, paragraph);
                    node.children.insert(
                        table_index + 3,
                        RecordNode {
                            tag: tag::CTRL_DATA,
                            data: b"table-order-sentinel".to_vec(),
                            children: Vec::new(),
                        },
                    );
                    return true;
                }
                if inject(&mut node.children) {
                    return true;
                }
            }
            false
        }
        assert!(inject(&mut roots));
        let encoded = encode_compressed_stream(RecordNode::serialize_forest(&roots), compressed);
        let mut cfb = cfb::open_rw(path).unwrap();
        cfb.create_stream("/BodyText/Section0")
            .unwrap()
            .write_all(&encoded)
            .unwrap();
        cfb.flush().unwrap();
    }

    /// #220 review: a `--set-cell` value that becomes several paragraphs changes one cell's
    /// paragraph count. That used to drop the whole table on the regeneration path, which
    /// cannot reproduce a caption's upper listflags and re-appends unknown records after every
    /// cell. The paragraph run is spliced into the source tree instead, so only the target
    /// cell's records and its LIST_HEADER paragraph count change.
    #[test]
    fn source_preserving_multi_paragraph_cell_splices_into_the_source_table() {
        let source = source_rewrite_path("cell-splice-source");
        let output = source_rewrite_path("cell-splice-output");
        let document = hwp_convert::from_markdown("본문\n\n| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        write_document_with_report(&document, &source, &WriteOptions::default()).unwrap();
        inject_table_caption_and_opaque(&source);

        let original = crate::read_document(&source).unwrap().document;
        let mut edited = original.clone();
        hwp_convert::set_cell(&mut edited, 0, 1, 0, "첫째 블록\n\n둘째 블록").unwrap();

        rewrite_document_with_report(
            &source,
            &original,
            &edited,
            &output,
            &WriteOptions {
                preserve_linesegs: true,
                ..WriteOptions::default()
            },
        )
        .unwrap();

        let source_table = table_control_node(&source);
        let output_table = table_control_node(&output);

        // The caption LIST_HEADER, its paragraph, the TABLE record and the unknown record keep
        // their bytes and their positions.
        assert_eq!(source_table.children[0].tag, tag::LIST_HEADER);
        assert_eq!(
            u32::from_le_bytes(source_table.children[0].data[4..8].try_into().unwrap()),
            CAPTION_LIST_ATTR,
            "픽스처 캡션 listflags"
        );
        for index in 0..4 {
            assert_eq!(
                source_table.children[index], output_table.children[index],
                "표 자식 {index} 보존"
            );
        }
        assert_eq!(output_table.children[2].tag, tag::TABLE);
        assert_eq!(output_table.children[3].data, b"table-order-sentinel");

        // Every source child that is not part of the edited cell survives unchanged.
        let sentinels = |node: &RecordNode| {
            node.children
                .iter()
                .filter(|child| child.tag == tag::CTRL_DATA)
                .count()
        };
        assert_eq!(sentinels(&source_table), sentinels(&output_table));
        assert_eq!(
            source_table.children.len() + 1,
            output_table.children.len(),
            "문단 레코드 하나만 늘어난다"
        );

        // The edited cell now declares and holds two paragraphs.
        let cell_headers = |node: &RecordNode| -> Vec<(usize, i32)> {
            node.children
                .iter()
                .enumerate()
                .filter(|(_, child)| child.tag == tag::LIST_HEADER)
                .map(|(index, child)| {
                    (
                        index,
                        i32::from_le_bytes(child.data[..4].try_into().unwrap()),
                    )
                })
                .collect()
        };
        let headers = cell_headers(&output_table);
        // Index 0 is the caption; the first cell of the table follows the TABLE record.
        assert_eq!(headers[0].1, 1, "캡션 문단 수 그대로");
        let edited_cell = headers
            .iter()
            .find(|(_, count)| *count == 2)
            .expect("문단 2개 셀");
        for offset in 1..=2 {
            assert_eq!(
                output_table.children[edited_cell.0 + offset].tag,
                tag::PARA_HEADER,
                "셀 LIST_HEADER 뒤 문단 레코드"
            );
        }

        let written = crate::read_document(&output).unwrap().document;
        let table = written.sections[0]
            .paragraphs
            .iter()
            .flat_map(|paragraph| &paragraph.controls)
            .find_map(|control| match control {
                Control::Table(table) => Some(table),
                _ => None,
            })
            .expect("표 없음");
        let cell = table
            .cells
            .iter()
            .find(|cell| cell.row == 1 && cell.col == 0)
            .expect("셀 (1,0)");
        assert_eq!(cell.paragraphs.len(), 2);
        assert!(cell.paragraphs[0].plain_text().contains("첫째 블록"));
        assert!(cell.paragraphs[1].plain_text().contains("둘째 블록"));
        let caption = table.caption.as_ref().expect("캡션 없음");
        assert_eq!(caption.paragraphs.len(), 1);

        std::fs::remove_file(source).unwrap();
        std::fs::remove_file(output).unwrap();
    }

    #[test]
    fn source_preserving_image_insert_materializes_only_new_relationships() {
        let source = source_rewrite_path("image-source");
        let output = source_rewrite_path("image-output");
        let asset = source_rewrite_path("image-asset").with_extension("gif");
        let gif = vec![
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x02, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xff, 0xff, 0xff, 0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c,
            0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00,
            0x3b,
        ];
        std::fs::write(&asset, gif).unwrap();
        let document = hwp_convert::from_markdown("image anchor");
        write_document_with_report(&document, &source, &WriteOptions::default()).unwrap();
        add_opaque_source_entries(&source);
        let original = crate::read_document(&source).unwrap().document;
        let mut edited = original.clone();
        hwp_convert::insert_image(
            &mut edited,
            "anchor",
            &asset,
            hwp_convert::ImageSize::Mm(20.0, 10.0),
        )
        .unwrap();

        let report = rewrite_document_with_report(
            &source,
            &original,
            &edited,
            &output,
            &WriteOptions {
                edited: true,
                ..WriteOptions::default()
            },
        )
        .unwrap();
        assert!(report.preservation.is_lossless());

        let source_streams = raw_streams(&source);
        let output_streams = raw_streams(&output);
        for path in ["/FileHeader", "/MemoExtended", "/OpaqueStorage/Hidden"] {
            assert_eq!(
                source_streams[path], output_streams[path],
                "unexpected stream change: {path}"
            );
        }
        let reopened = crate::read_document(&output).unwrap().document;
        assert_eq!(reopened.bin_streams.len(), original.bin_streams.len() + 1);
        let picture = reopened.sections[0]
            .paragraphs
            .iter()
            .flat_map(|paragraph| &paragraph.controls)
            .find_map(|control| match control {
                Control::Picture(picture) => Some(picture),
                _ => None,
            })
            .expect("inserted picture");
        assert!(reopened.resolve_bin(&picture.bin_ref).is_some());

        std::fs::remove_file(source).unwrap();
        std::fs::remove_file(output).unwrap();
        std::fs::remove_file(asset).unwrap();
    }

    #[test]
    fn caption_hwpunit_values_saturate_instead_of_wrapping() {
        let caption = Caption {
            side: CaptionSide::Bottom,
            direction: CaptionDirection::Horizontal,
            gap: i32::MAX,
            width: Some(u32::MAX),
            last_width: u32::MAX,
            paragraphs: Vec::new(),
        };
        let record = emit_caption_header(&caption);
        assert_eq!(
            i32::from_le_bytes(record.data[12..16].try_into().unwrap()),
            i32::MAX
        );
        assert_eq!(
            u16::from_le_bytes(record.data[16..18].try_into().unwrap()),
            u16::MAX
        );
        assert_eq!(
            i32::from_le_bytes(record.data[18..22].try_into().unwrap()),
            i32::MAX
        );
    }

    #[test]
    fn synthesized_generic_gso_emits_its_caption_first() {
        let generic = hwp_model::GenericControl {
            ctrl_id: *b"gso ",
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: Some(Caption {
                side: CaptionSide::Bottom,
                direction: CaptionDirection::Horizontal,
                gap: 100,
                width: None,
                last_width: 200,
                paragraphs: vec![hwp_model::Paragraph::default()],
            }),
            hwpx_raw_xml: None,
            container_box: None,
        };
        let node = emit_control(
            &Control::Generic(generic),
            true,
            false,
            false,
            &mut WriteReport::new(),
        );
        assert_eq!(node.children[0].tag, tag::LIST_HEADER);
        assert_eq!(node.children[1].tag, tag::PARA_HEADER);
    }

    /// Synthesized HWPX-origin picture records must match the genuine 5.1.x structure:
    /// SHAPE_COMPONENT (196 bytes, "$pic") -> SHAPE_COMPONENT_PICTURE (91 bytes). Clipping must use
    /// the original natural size rather than display size, avoiding a partial crop that hides the
    /// image.
    #[test]
    fn 그림_extras_합성_구조() {
        // Display 8760x1260, natural 150000x61875.
        let extras = build_picture_extras(8760, 1260, (150000, 61875), 1, 0x3010_0000);
        assert_eq!(extras.len(), 1, "SHAPE_COMPONENT 1개");
        let sc = &extras[0];
        assert_eq!(sc.tag, tag::SHAPE_COMPONENT);
        assert_eq!(sc.data.len(), 196, "SHAPE_COMPONENT 196B");
        assert_eq!(&sc.data[0..4], b"cip$", "chid = $pic(역순)");
        assert_eq!(
            i32::from_le_bytes(sc.data[20..24].try_into().unwrap()),
            8760
        );
        assert_eq!(
            i32::from_le_bytes(sc.data[24..28].try_into().unwrap()),
            1260
        );

        assert_eq!(sc.children.len(), 1, "SHAPE_COMPONENT_PICTURE 자식");
        let pic = &sc.children[0];
        assert_eq!(pic.tag, tag::SHAPE_COMPONENT_PICTURE);
        assert_eq!(
            pic.data.len(),
            91,
            "PICTURE 91B (정품 5.1.x picture_effect)"
        );
        assert_eq!(
            u16::from_le_bytes(pic.data[71..73].try_into().unwrap()),
            1,
            "BinItem ID @71"
        );
        // Corner 2 = display dimensions (width, height).
        assert_eq!(
            i32::from_le_bytes(pic.data[28..32].try_into().unwrap()),
            8760
        );
        assert_eq!(
            i32::from_le_bytes(pic.data[32..36].try_into().unwrap()),
            1260
        );
        // Right/bottom clipping bounds = natural dimensions, not display dimensions.
        assert_eq!(
            i32::from_le_bytes(pic.data[52..56].try_into().unwrap()),
            150000,
            "자르기 우 = 자연 폭"
        );
        assert_eq!(
            i32::from_le_bytes(pic.data[56..60].try_into().unwrap()),
            61875,
            "자르기 하 = 자연 높이"
        );
        // Unique instance_id at offset 74.
        assert_eq!(
            u32::from_le_bytes(pic.data[74..78].try_into().unwrap()),
            0x3010_0000,
            "instance_id @74"
        );
        // picture_effect at 78: flags=0, natural width at 82, natural height at 86.
        assert_eq!(
            u32::from_le_bytes(pic.data[78..82].try_into().unwrap()),
            0,
            "picture_effect flags=0"
        );
        assert_eq!(
            i32::from_le_bytes(pic.data[82..86].try_into().unwrap()),
            150000
        );
        assert_eq!(
            i32::from_le_bytes(pic.data[86..90].try_into().unwrap()),
            61875
        );
    }

    #[test]
    fn gso_설명_utf16_u16_경계() {
        let mut report = WriteReport::new();
        let mut common = Vec::new();
        append_gso_description(&mut common, Some("😀"), &mut report);
        assert_eq!(u16::from_le_bytes(common[0..2].try_into().unwrap()), 2);
        assert_eq!(common.len(), 6);
        assert!(report.warnings.is_empty());

        let mut max_common = Vec::new();
        append_gso_description(&mut max_common, Some(&"가".repeat(65_535)), &mut report);
        assert_eq!(
            u16::from_le_bytes(max_common[0..2].try_into().unwrap()),
            u16::MAX
        );
        assert_eq!(max_common.len(), 2 + 65_535 * 2);

        let mut overflow = Vec::new();
        append_gso_description(&mut overflow, Some(&"가".repeat(65_536)), &mut report);
        assert_eq!(overflow, 0u16.to_le_bytes());
        assert_eq!(
            report.preservation.events.as_slice(),
            &[hwp_model::PreservationEvent::new(
                PreservationCode::ControlMetadataUnrepresentable,
                PreservationResourceKind::Metadata,
                PreservationDisposition::Unrepresentable,
                1,
            )]
        );
    }

    #[test]
    fn typed_write_report_keeps_loss_events_out_of_ordinary_warnings() {
        let mut doc = hwp_convert::from_markdown("typed report");
        doc.sections[0].paragraphs[0]
            .controls
            .push(Control::Generic(hwp_model::GenericControl {
                ctrl_id: *b"head",
                data: Vec::new(),
                paragraph_lists: Vec::new(),
                extras: Vec::new(),
                raw_children: Vec::new(),
                gso_shapes: Vec::new(),
                equation: None,
                column_def: None,
                caption: None,
                hwpx_raw_xml: None,
                container_box: None,
            }));
        let output =
            std::env::temp_dir().join(format!("hwp5-typed-report-{}.hwp", std::process::id()));

        let report = write_document_with_report(&doc, &output, &WriteOptions::default()).unwrap();
        assert!(
            report
                .warnings
                .iter()
                .all(|warning| !warning.starts_with("DROP:"))
        );
        assert_eq!(report.preservation.events.len(), 1);
        let event = &report.preservation.events[0];
        assert_eq!(event.code, PreservationCode::OpaqueControlUnrepresentable);
        assert_eq!(event.resource, PreservationResourceKind::Control);
        assert_eq!(event.disposition, PreservationDisposition::Unrepresentable);
        assert_eq!(event.count, 1);

        let legacy = write_document(&doc, &output, &WriteOptions::default()).unwrap();
        assert!(
            legacy
                .iter()
                .any(|warning| { warning.starts_with("DROP: opaque_control_unrepresentable") })
        );
        std::fs::remove_file(output).unwrap();
    }

    #[test]
    fn typed_write_report_accounts_for_missing_picture_payload() {
        let mut doc = hwp_convert::from_markdown("picture report");
        doc.sections[0].paragraphs[0]
            .controls
            .push(Control::Picture(Picture {
                common_data: Vec::new(),
                width: hwp_model::HwpUnit(1000),
                height: hwp_model::HwpUnit(500),
                treat_as_char: true,
                z_order: 0,
                vert_offset: 0,
                horz_offset: 0,
                description: None,
                crop: None,
                flip: 0,
                rotation: None,
                brightness: 0,
                contrast: 0,
                effect_flags: 0,
                effects_raw: Vec::new(),
                caption: None,
                bin_ref: hwp_model::BinRef::ItemRef("missing-image".to_string()),
                extras: Vec::new(),
            }));
        let output = std::env::temp_dir().join(format!(
            "hwp5-typed-picture-report-{}.hwp",
            std::process::id()
        ));

        let report = write_document_with_report(&doc, &output, &WriteOptions::default()).unwrap();
        assert!(
            report
                .warnings
                .iter()
                .all(|warning| !warning.starts_with("DROP:"))
        );
        assert!(report.preservation.events.iter().any(|event| {
            event.code == PreservationCode::PictureControlRemoved
                && event.resource == PreservationResourceKind::Control
                && event.disposition == PreservationDisposition::Removed
                && event.count == 1
        }));
        std::fs::remove_file(output).unwrap();
    }

    // --- FIDL-02 reproduction: bin_stream_index resolution (01-02-PLAN.md Task 1) ---
    //
    // All three scenarios below share a non-empty `bin_streams`/`header.bin_data` (two
    // distinguishable streams, storage_id 1 and 2) - this is what distinguishes them from
    // `typed_write_report_accounts_for_missing_picture_payload` above, which covers the
    // genuinely-empty-payload case and must keep passing unchanged.

    fn picture_report_path(label: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("hwp5-{label}-{}-{unique}.hwp", std::process::id()))
    }

    /// Two bin streams with distinguishable payloads and two matching `BinDataItem`
    /// entries (storage_id 1 and 2), so which stream the writer actually consumes for a
    /// given `BinRef` is directly observable in the produced container.
    fn picture_binref_repro_doc(bin_ref: hwp_model::BinRef) -> Document {
        let mut doc = hwp_convert::from_markdown("picture report");
        doc.bin_streams = vec![
            hwp_model::BinStream {
                name: "BIN0001.png".to_string(),
                data: vec![1, 2, 3],
            },
            hwp_model::BinStream {
                name: "BIN0002.png".to_string(),
                data: vec![4, 5, 6],
            },
        ];
        doc.header.bin_data = vec![
            hwp_model::BinDataItem {
                attr: 1,
                storage_id: Some(1),
                extension: Some("png".to_string()),
                ..Default::default()
            },
            hwp_model::BinDataItem {
                attr: 1,
                storage_id: Some(2),
                extension: Some("png".to_string()),
                ..Default::default()
            },
        ];
        doc.sections[0].paragraphs[0]
            .controls
            .push(Control::Picture(Picture {
                common_data: Vec::new(),
                width: hwp_model::HwpUnit(1000),
                height: hwp_model::HwpUnit(500),
                treat_as_char: true,
                z_order: 0,
                vert_offset: 0,
                horz_offset: 0,
                description: None,
                crop: None,
                flip: 0,
                rotation: None,
                brightness: 0,
                contrast: 0,
                effect_flags: 0,
                effects_raw: Vec::new(),
                caption: None,
                bin_ref,
                extras: Vec::new(),
            }));
        doc
    }

    /// The writer mints a fresh `storage_id` for a synthesized picture (`next_id` starts
    /// one past the highest existing `storage_id`, here 2), so a resolved picture's bytes
    /// always land at this fixed path regardless of which input stream fed it. `None`
    /// means no synthesized stream was embedded at all.
    fn synthesized_bin_stream_bytes(path: &Path) -> Option<Vec<u8>> {
        let mut container = crate::Hwp5Container::open(path).unwrap();
        let raw = container.read_stream_raw("/BinData/BIN0003.png").ok()?;
        Some(decompress(&raw, "/BinData/BIN0003.png").unwrap())
    }

    fn picture_with_instance_id(id: u32) -> Control {
        let mut common_data = vec![0u8; 44];
        common_data[32..36].copy_from_slice(&id.to_le_bytes());
        Control::Picture(hwp_model::Picture {
            common_data,
            width: hwp_model::HwpUnit(100),
            height: hwp_model::HwpUnit(100),
            treat_as_char: true,
            z_order: 0,
            vert_offset: 0,
            horz_offset: 0,
            description: None,
            crop: None,
            flip: 0,
            rotation: None,
            brightness: 0,
            contrast: 0,
            effect_flags: 0,
            effects_raw: Vec::new(),
            caption: None,
            bin_ref: hwp_model::BinRef::ItemRef("x".to_string()),
            extras: Vec::new(),
        })
    }

    fn shape_caption_holding(controls: Vec<Control>) -> Control {
        Control::Generic(hwp_model::GenericControl {
            ctrl_id: *b"gso ",
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: Some(hwp_model::Caption {
                side: hwp_model::CaptionSide::Bottom,
                direction: hwp_model::CaptionDirection::Horizontal,
                gap: 0,
                width: None,
                last_width: 0,
                paragraphs: vec![Paragraph {
                    controls,
                    ..Paragraph::default()
                }],
            }),
            hwpx_raw_xml: None,
            container_box: None,
        })
    }

    /// The HWP5 reader keeps a shape's caption apart from its paragraph lists, so a picture
    /// whose only home is a shape caption still holds an instance id from the synthesized
    /// range. The allocator must start past it, or the next synthesized picture repeats it and
    /// the raw-preserved caption leaves two objects with one id.
    #[test]
    fn synthesized_picture_ids_start_past_ids_held_in_shape_captions() {
        let mut doc = Document::default();
        doc.sections.push(Section {
            paragraphs: vec![Paragraph {
                controls: vec![shape_caption_holding(vec![picture_with_instance_id(
                    0x3000_0004,
                )])],
                ..Paragraph::default()
            }],
            extras: Vec::new(),
        });
        let mut ids = InstanceIds::new(&doc);
        for _ in 0..16 {
            let id = ids.next().expect("the range has room");
            assert!(
                id != 0x3000_0004 && id != 0x3000_0004 ^ 0x0010_0000,
                "{id:#x} repeats (or derives) the id held in the shape caption"
            );
        }
    }

    fn doc_with_picture_ids(ids: &[u32]) -> Document {
        let mut doc = Document::default();
        doc.sections.push(Section {
            paragraphs: vec![Paragraph {
                controls: ids.iter().map(|id| picture_with_instance_id(*id)).collect(),
                ..Paragraph::default()
            }],
            extras: Vec::new(),
        });
        doc
    }

    /// An id handed out must be inside the synthesized range and not already used by any
    /// picture, table or container. `max + 1` of the in-range ids steps over the range edge:
    /// with `0x300fffff` present it returns `0x30100000`, which another object already holds.
    #[test]
    fn a_synthesized_picture_id_stays_in_range_and_avoids_every_used_id() {
        let doc = doc_with_picture_ids(&[0x300f_ffff, 0x3010_0000]);
        let mut ids = InstanceIds::new(&doc);
        let first = ids.next().expect("the range has room");
        assert!(
            (0x3000_0000..0x3010_0000).contains(&first),
            "{first:#x} is outside the synthesized range"
        );
        assert!(
            first != 0x300f_ffff && first != 0x3010_0000,
            "{first:#x} is already used"
        );
        // Successive ids never repeat.
        let second = ids.next().unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn a_synthesized_picture_id_does_not_derive_a_used_effect_id() {
        let doc = doc_with_picture_ids(&[0x3010_0000]);
        let mut ids = InstanceIds::new(&doc);
        let first = ids.next().unwrap();
        assert_ne!(
            first ^ 0x0010_0000,
            0x3010_0000,
            "{first:#x} derives a used id"
        );
    }

    /// With the synthesized range used up a picture must be left exactly as it was: no
    /// BinData item, no renamed stream and no records, so the strip step that follows removes
    /// it and reports the loss instead of the writer inventing an id another object holds.
    #[test]
    fn an_exhausted_range_leaves_the_picture_and_the_bin_tables_untouched() {
        let mut doc = Document::default();
        doc.bin_streams.push(hwp_model::BinStream {
            name: "image.png".to_string(),
            data: vec![1, 2, 3],
        });
        let mut picture = match picture_with_instance_id(0) {
            Control::Picture(p) => p,
            _ => unreachable!(),
        };
        picture.common_data.clear();
        picture.bin_ref = hwp_model::BinRef::ItemRef("image.png".to_string());
        let mut paragraph = Paragraph {
            controls: vec![Control::Picture(picture)],
            ..Paragraph::default()
        };
        let bin_names = vec!["image.png".to_string()];
        let bin_dims = vec![None];
        let bin_index = |_: &hwp_model::BinRef| Some(0usize);
        let mut assigned: Vec<Option<u16>> = vec![None];
        let mut next_id = 1u16;
        let mut new_items = Vec::new();
        let mut renames = Vec::new();
        let mut report = WriteReport::new();
        let mut inst = InstanceIds::with_range(&doc, 0x3000_0000, 0x3000_0000);
        synth_pictures_para(
            &mut paragraph,
            &bin_names,
            &bin_dims,
            &bin_index,
            &mut assigned,
            &mut next_id,
            &mut inst,
            &mut new_items,
            &mut renames,
            &mut report,
        );
        let Control::Picture(picture) = &paragraph.controls[0] else {
            unreachable!()
        };
        assert!(picture.extras.is_empty() && picture.common_data.is_empty());
        assert!(
            new_items.is_empty(),
            "a BinData item was minted for a picture that stays unwritten"
        );
        assert!(renames.is_empty() && assigned == vec![None] && next_id == 1);
    }

    #[test]
    fn an_exhausted_synthesized_range_is_reported_not_wrapped() {
        let doc = doc_with_picture_ids(&[0x3000_0002]);
        // A range of three ids with one already used leaves two, then none.
        let mut ids = InstanceIds::with_range(&doc, 0x3000_0000, 0x3000_0003);
        assert!(ids.next().is_some());
        assert!(ids.next().is_some());
        assert!(ids.next().is_none(), "the third request has nothing left");
        assert!(ids.next().is_none(), "and it stays exhausted");
    }

    #[test]
    fn picture_binref_id_resolves_to_its_own_stream() {
        // S1: BinRef::Id(2) names bin_streams[1] (storage_id 2) through header.bin_data,
        // not bin_streams[0].
        let doc = picture_binref_repro_doc(hwp_model::BinRef::Id(hwp_model::BinDataId(2)));
        let output = picture_report_path("picture-binref-id");

        let report = write_document_with_report(&doc, &output, &WriteOptions::default()).unwrap();

        let bytes = synthesized_bin_stream_bytes(&output)
            .expect("a resolvable BinRef::Id must still synthesize a picture stream");
        assert_eq!(
            bytes,
            vec![4, 5, 6],
            "BinRef::Id(2) names bin_streams[1] (storage_id 2); the writer must not fall \
             back to bin_streams[0]. Preservation events: {:?}",
            report.preservation.events
        );
        std::fs::remove_file(output).unwrap();
    }

    #[test]
    fn picture_itemref_resolves_to_the_named_stream() {
        // An exact unlisted filename continues to resolve to its own stream.
        let doc = picture_binref_repro_doc(hwp_model::BinRef::ItemRef("BIN0002.png".to_string()));
        let output = picture_report_path("picture-itemref");

        let report = write_document_with_report(&doc, &output, &WriteOptions::default()).unwrap();

        let bytes = synthesized_bin_stream_bytes(&output)
            .expect("a resolvable ItemRef must synthesize a picture stream");
        assert_eq!(
            bytes,
            vec![4, 5, 6],
            "ItemRef(\"BIN0002.png\") must consume bin_streams[1]. Preservation events: {:?}",
            report.preservation.events
        );
        std::fs::remove_file(output).unwrap();
    }

    /// `picture_binref_repro_doc` with HWPX-style streams: the two payloads are named
    /// `BinData/p1.png` and `BinData/p2.png` and a manifest maps `logo` to the second one, an id
    /// that does not look like its file name.
    fn manifest_repro_doc(item: &str, manifest: Vec<(String, String)>) -> Document {
        let mut doc = picture_binref_repro_doc(hwp_model::BinRef::ItemRef(item.to_string()));
        doc.bin_streams[0].name = "BinData/p1.png".to_string();
        doc.bin_streams[1].name = "BinData/p2.png".to_string();
        doc.hwpx_bin_manifest = manifest;
        doc
    }

    #[test]
    fn picture_itemref_resolves_through_the_manifest_when_the_id_is_not_the_file_name() {
        // `id="logo" href="BinData/p2.png"`: neither the trailing name nor a substring of any
        // stream says "logo", so only the manifest can find the second stream.
        let doc = manifest_repro_doc(
            "logo",
            vec![("logo".to_string(), "BinData/p2.png".to_string())],
        );
        let output = picture_report_path("picture-manifest-id");
        let report = write_document_with_report(&doc, &output, &WriteOptions::default()).unwrap();
        assert_eq!(
            synthesized_bin_stream_bytes(&output),
            Some(vec![4, 5, 6]),
            "the manifest names bin_streams[1]. Events: {:?}",
            report.preservation.events
        );
        assert!(
            !report
                .preservation
                .events
                .iter()
                .any(|event| event.code == PreservationCode::BinaryAssetRemoved),
            "a manifest id must not be ledgered as a removed asset: {:?}",
            report.preservation.events
        );
        std::fs::remove_file(output).unwrap();
    }

    #[test]
    fn picture_itemref_listed_in_the_manifest_never_falls_back_to_a_same_named_stream() {
        // `logo` is listed, but its href is a stream the document does not carry. A stream
        // whose name merely contains "logo" must not stand in for it.
        let mut doc = manifest_repro_doc(
            "logo",
            vec![("logo".to_string(), "BinData/missing.png".to_string())],
        );
        doc.bin_streams[1].name = "BinData/logo-of-someone-else.png".to_string();
        let output = picture_report_path("picture-manifest-missing");
        let report = write_document_with_report(&doc, &output, &WriteOptions::default()).unwrap();
        assert_eq!(
            synthesized_bin_stream_bytes(&output),
            None,
            "no stream may be substituted for a listed id whose href is missing"
        );
        assert!(
            report
                .preservation
                .events
                .iter()
                .any(|event| event.code == PreservationCode::BinaryAssetRemoved),
            "the loss is reported: {:?}",
            report.preservation.events
        );
        std::fs::remove_file(output).unwrap();
    }

    #[test]
    fn picture_itemref_without_a_manifest_entry_keeps_the_name_rules() {
        // An id the manifest does not list still resolves by name (the existing behavior).
        let doc = manifest_repro_doc(
            "p2.png",
            vec![("other".to_string(), "BinData/p1.png".to_string())],
        );
        let output = picture_report_path("picture-manifest-unlisted");
        write_document_with_report(&doc, &output, &WriteOptions::default()).unwrap();
        assert_eq!(synthesized_bin_stream_bytes(&output), Some(vec![4, 5, 6]));
        std::fs::remove_file(output).unwrap();
    }

    #[derive(Clone, Copy)]
    enum PartialItemRefSite {
        Body,
        Cell,
        GenericList,
    }

    fn partial_itemref_doc(site: PartialItemRefSite) -> Document {
        let mut doc = picture_binref_repro_doc(hwp_model::BinRef::ItemRef("image1".into()));
        doc.meta.source_format = "hwpx".into();
        doc.bin_streams[0].name = "BinData/image10.png".into();
        doc.bin_streams[1].name = "BinData/unrelated.png".into();
        if matches!(site, PartialItemRefSite::Body) {
            return doc;
        }
        let picture = doc.sections[0].paragraphs[0].controls.pop().unwrap();
        let control = match site {
            PartialItemRefSite::Body => unreachable!(),
            PartialItemRefSite::Cell => {
                let table_doc = hwp_convert::from_markdown("| value |\n| --- |\n| nested |\n");
                let mut table = table_doc.sections[0]
                    .paragraphs
                    .iter()
                    .flat_map(|paragraph| &paragraph.controls)
                    .find_map(|control| match control {
                        Control::Table(table) => Some(table.clone()),
                        _ => None,
                    })
                    .unwrap();
                table.cells[0].paragraphs[0].controls.push(picture);
                Control::Table(table)
            }
            PartialItemRefSite::GenericList => Control::Generic(hwp_model::GenericControl {
                ctrl_id: *b"head",
                data: vec![0; 8],
                paragraph_lists: vec![hwp_model::ParagraphList {
                    header_data: Vec::new(),
                    paragraphs: vec![Paragraph {
                        controls: vec![picture],
                        ..Paragraph::default()
                    }],
                }],
                extras: Vec::new(),
                raw_children: Vec::new(),
                gso_shapes: Vec::new(),
                equation: None,
                column_def: None,
                caption: None,
                hwpx_raw_xml: None,
                container_box: None,
            }),
        };
        doc.sections[0].paragraphs[0].controls.push(control);
        doc
    }

    fn assert_partial_itemref_is_a_loss(site: PartialItemRefSite, label: &str) {
        let doc = partial_itemref_doc(site);
        let output = picture_report_path(label);
        let report = write_document_with_report(&doc, &output, &WriteOptions::default()).unwrap();
        let synthesized = synthesized_bin_stream_bytes(&output);
        std::fs::remove_file(output).unwrap();
        assert!(
            report.preservation.events.iter().any(|event| {
                event.code == PreservationCode::BinaryAssetRemoved
                    && event.resource == PreservationResourceKind::BinaryAsset
                    && event.disposition == PreservationDisposition::Removed
                    && event.count == 1
            }),
            "an unlisted partial stem must report BinaryAssetRemoved instead of another picture: {:?}",
            report.preservation.events
        );
        assert_eq!(synthesized, None, "image1 must not select image10.png");
    }

    #[test]
    fn partial_itemref_in_body_is_reported_as_removed() {
        assert_partial_itemref_is_a_loss(PartialItemRefSite::Body, "partial-itemref-body");
    }

    #[test]
    fn partial_itemref_in_cell_is_reported_as_removed() {
        assert_partial_itemref_is_a_loss(PartialItemRefSite::Cell, "partial-itemref-cell");
    }

    #[test]
    fn partial_itemref_in_generic_list_is_reported_as_removed() {
        assert_partial_itemref_is_a_loss(PartialItemRefSite::GenericList, "partial-itemref-list");
    }

    #[test]
    fn exact_itemref_stem_wins_over_an_earlier_partial_stem() {
        let mut doc = partial_itemref_doc(PartialItemRefSite::Body);
        doc.bin_streams[1].name = "BinData/image1.png".into();
        let output = picture_report_path("itemref-exact-stem");
        let report = write_document_with_report(&doc, &output, &WriteOptions::default()).unwrap();
        let synthesized = synthesized_bin_stream_bytes(&output);
        std::fs::remove_file(output).unwrap();
        assert_eq!(
            synthesized,
            Some(vec![4, 5, 6]),
            "the model's exact filename stem must select the second stream, not image10.png"
        );
        assert!(
            report
                .preservation
                .events
                .iter()
                .all(|event| { event.code != PreservationCode::BinaryAssetRemoved })
        );
    }

    #[test]
    fn itemref_name_resolution_agrees_with_the_model() {
        let names = ["BinData/image10.png", "BinData/image1.png", "plain.png"];
        let mut doc = Document {
            bin_streams: names
                .iter()
                .map(|name| hwp_model::BinStream {
                    name: (*name).into(),
                    data: Vec::new(),
                })
                .collect(),
            ..Document::default()
        };
        let bin_names: Vec<String> = names.iter().map(|name| (*name).into()).collect();
        for item in [
            "image10.png",
            "BinData/image1.png",
            "plain.png",
            "image10",
            "image1",
            "plain",
            "image",
            "IMAGE1.PNG",
            "",
            "missing",
        ] {
            let reference = hwp_model::BinRef::ItemRef(item.into());
            assert_eq!(
                bin_stream_index(&bin_names, &[], &doc.hwpx_bin_manifest, &reference),
                doc.bin_stream_index(&reference),
                "unlisted reference {item:?} must use the same stream as the model"
            );
        }
        for (item, href) in [("image1", "plain.png"), ("image1", "BinData/missing.png")] {
            doc.hwpx_bin_manifest = vec![(item.into(), href.into())];
            let reference = hwp_model::BinRef::ItemRef(item.into());
            assert_eq!(
                bin_stream_index(&bin_names, &[], &doc.hwpx_bin_manifest, &reference),
                doc.bin_stream_index(&reference),
                "a listed id must follow its manifest href"
            );
        }
    }

    #[test]
    fn picture_with_unresolvable_reference_reports_binary_asset_removed() {
        // A reference that names no available stream stays unresolved even when other
        // payloads are available, and is reported as a typed loss.
        let doc = picture_binref_repro_doc(hwp_model::BinRef::ItemRef(
            "not-in-this-document.png".to_string(),
        ));
        let output = picture_report_path("picture-unresolvable");

        let report = write_document_with_report(&doc, &output, &WriteOptions::default()).unwrap();

        assert!(
            report.preservation.events.iter().any(|event| {
                event.code == PreservationCode::BinaryAssetRemoved
                    && event.resource == PreservationResourceKind::BinaryAsset
                    && event.disposition == PreservationDisposition::Removed
                    && event.count == 1
            }),
            "an unresolvable reference must be reported, not silently backfilled with \
             bin_streams[0]: {:?}",
            report.preservation.events
        );
        assert!(
            report
                .preservation
                .events
                .iter()
                .any(|event| event.code == PreservationCode::PictureControlRemoved),
            "the picture control itself still has no payload to write once its bin \
             reference does not resolve: {:?}",
            report.preservation.events
        );
        assert!(
            synthesized_bin_stream_bytes(&output).is_none(),
            "no picture stream should be synthesized for an unresolvable reference"
        );
        std::fs::remove_file(output).unwrap();
    }

    fn synth_table(placement: Option<hwp_model::GsoPlacement>) -> Table {
        Table {
            common_data: Vec::new(),
            placement,
            attr: 0,
            rows: 1,
            cols: 1,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: vec![1],
            border_fill: hwp_model::BorderFillId(1),
            table_tail: Vec::new(),
            cells: Vec::new(),
            caption: None,
            extras: Vec::new(),
        }
    }

    /// Synthesized HWPX/Markdown tables must emit the minimum 46-byte object-common attributes
    /// required by specification table 69 (4-byte " lbt" plus 40-byte fields plus 2-byte desc_len),
    /// matching the 46-byte picture path. INT32 at 40 (control-common 4 + offset 36) prevents page
    /// splitting and is 0 in genuine samples; WORD at 44 is desc_len=0.
    #[test]
    fn 합성_표_공통속성_46b_쪽나눔0() {
        // 1) HWPX-origin placement path.
        let pl = hwp_model::GsoPlacement {
            hold_anchor: true, // Must be dropped in HWP5 synthesis, never leaked into the page-splitting field.
            width: 40000,
            height: 20000,
            z_order: 3,
            ..Default::default()
        };
        let node = emit_table(
            &synth_table(Some(pl)),
            true,
            false,
            false,
            &mut WriteReport::new(),
        );
        assert_eq!(&node.data[0..4], b" lbt", "표 ctrl ID");
        assert_eq!(
            node.data.len(),
            46,
            "placement 경로: ctrl ID 4 + 공통 42 = 46B (스펙 표 69 최소)"
        );
        assert_eq!(
            i32::from_le_bytes(node.data[40..44].try_into().unwrap()),
            0,
            "@40 쪽나눔 방지 = 0 (holdAnchorAndSO가 새면 안 됨)"
        );
        assert_eq!(
            u16::from_le_bytes(node.data[44..46].try_into().unwrap()),
            0,
            "@44 desc_len = 0"
        );

        // 2) Markdown-origin path: placement=None, empty common_data.
        let node = emit_table(
            &synth_table(None),
            true,
            false,
            false,
            &mut WriteReport::new(),
        );
        assert_eq!(node.data.len(), 46, "md 경로: 46B (스펙 표 69 최소)");
        assert_eq!(
            i32::from_le_bytes(node.data[40..44].try_into().unwrap()),
            0,
            "md 경로 @40 쪽나눔 방지 = 0"
        );
        assert_eq!(
            u16::from_le_bytes(node.data[44..46].try_into().unwrap()),
            0,
            "md 경로 @44 desc_len = 0"
        );
    }
}
