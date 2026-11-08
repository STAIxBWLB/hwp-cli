//! Document-level merge — the `hwp merge` IR transform (GM-3, D-01/D-02).
//!
//! **Tier 1: palette-compatible fast path.** When a later input's header is
//! structurally compatible with the running target
//! (`hwp_convert::merge::palette_compatible`), its off-palette header extras are
//! grafted with the same offset arithmetic `hwp_convert::merge::part_paragraphs`
//! uses for part filling, and its whole `Section`s (each carrying its own
//! `SectionDef` — paper size, margins, headers/footers) are appended unchanged
//! (D-02: one Section per input, concatenated in argument order — sections never
//! fuse, even when two inputs are byte-equal). `part_paragraphs` itself is not
//! called: it rejects any paragraph carrying a `Control::SectionDef`, which every
//! section's first paragraph does; `merge.rs` stays untouched (D-01).
//!
//! **Tier 2: general graft.** When a later input's header is not
//! palette-compatible (an arbitrary, non-hwp-cli-generated document — a genuine
//! Hancom-saved file), the DocHeader-referencing ids it carries (char shape,
//! para shape, style, border fill, per-language face, tab def, numbering and
//! bullet ids, and hwp5 bin-data ids — `BinRef::Id`) are shifted by that
//! collection's offset with no shared-base assumption, in body paragraphs and
//! in every nested paragraph collection: table cells, Generic paragraph
//! lists, and captions on tables, pictures and Generic controls. The input's
//! `header.bin_data` table is
//! appended with colliding `storage_id`s reminted (and the payload streams
//! renamed to match) so `BinRef::Id` keeps resolving through
//! `Document::resolve_bin`'s `BIN{storage_id:04X}.{ext}` rule. A
//! `border_fill_id`/`border_fill` of 0 is the "unspecified" sentinel (not an
//! index) and is never shifted, so grafting never invents a border where none
//! existed. Section-level page border/fill references ride the same
//! border-fill offset (#171): hwp5-origin sections carry them as raw
//! PAGE_BORDER_FILL records (`SectionDef::page_border_fills_raw` plus the
//! parallel `SectionDef::extras` copies the hwp5 writer re-emits — both are
//! shifted, bytes 12..14 holding the 1-based id), hwpx-origin sections as
//! `<hp:pageBorderFill>` raw-XML passthrough children
//! (`SectionDef::secpr_raw_children`) whose numeric `borderFillIDRef`
//! attribute is rewritten in place. A passthrough child whose
//! `borderFillIDRef` is present but not numeric cannot be shifted onto the
//! merged border-fill table, so it is kept verbatim and recorded as
//! [`MergeLoss::SectionBorderFillRefUnresolvable`] (#180) rather than
//! silently passing through to resolve against the wrong table. Two
//! reference classes are deliberately not shifted: `Style`
//! entries are appended verbatim because no writer or render path reads their
//! internal shape ids, and the input header's unparsed passthrough records
//! (`extras`, `id_extras`, `id_mappings_counts`) are not grafted — the
//! primary's stand, matching the singular package-passthrough policy (D-14).
//!
//! Every non-primary input's GSO (table/picture) object identities are
//! renumbered from a running maximum across both tiers, so no two objects
//! originating in different inputs can share a Hancom-visible identity. Every
//! non-primary input's dropped singular package-passthrough fields and
//! superseded metadata are recorded as typed [`MergeLoss`] events (D-14).

use std::collections::HashMap;

use hwp_model::{BinRef, BinStream, Control, Document, LANG_COUNT, Paragraph, Section, SectionDef};

use crate::from_html::PALETTE_LEN;
use crate::from_markdown::BASE_PARA_SHAPES;
use crate::merge::palette_compatible;

/// A typed, content-free record of something [`merge_documents`] could not
/// carry losslessly, or a non-target change it had to make to avoid a
/// collision (D-14: additive only — once published a variant is never
/// withdrawn or repurposed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeLoss {
    /// A non-primary input carried one or more non-empty singular
    /// package-passthrough fields (`hwpx_settings_xml`, `hwpx_version_xml`,
    /// `hwpx_preview_image`, `hwp5_xml_template`, `hwp5_doc_history`,
    /// `hwpx_extra_entries`, `hwpx_bin_manifest`, `hwpx_opf_extra_items`) that
    /// were dropped — only the primary input's singular fields are carried.
    /// `fields` names the dropped field identifiers only, never their content.
    PackagePassthroughDropped {
        input_index: usize,
        fields: Vec<&'static str>,
    },
    /// A non-primary input's document metadata differed from the primary's
    /// and was superseded — the primary input's metadata wins.
    MetadataSuperseded { input_index: usize },
    /// One or more GSO (table/picture) object identities from a non-primary
    /// input were renumbered to avoid colliding with an earlier input's.
    /// This is a recorded, non-target change, not data loss.
    GsoObjectIdRenumbered { count: usize },
    /// A non-primary input carried hwp5 BIN_DATA entries the merged header
    /// could not graft because the u16 storage-id space was exhausted — the
    /// remaining entries were dropped, so `BinRef::Id` references to them no
    /// longer resolve (their payload streams are still carried).
    BinDataDropped { input_index: usize, count: usize },
    /// A non-primary hwpx-origin input's `<hp:pageBorderFill>` passthrough
    /// carried a `borderFillIDRef` that is present but not numeric, so the
    /// tier-2 graft could not shift it onto the merged border-fill table —
    /// the attribute is passed through verbatim and now resolves against the
    /// wrong table (#180). A recorded, non-target change, not data loss.
    SectionBorderFillRefUnresolvable { input_index: usize, count: usize },
}

/// Result of [`merge_documents`].
#[derive(Debug)]
pub struct MergeOutcome {
    /// The merged document: one Section per input, concatenated in argument order.
    pub document: Document,
    /// Whether any non-primary input required the general (non-palette-fast-path) graft.
    pub general_path_used: bool,
    /// Typed preservation losses and non-target changes recorded while merging.
    pub losses: Vec<MergeLoss>,
}

/// The eight singular package-passthrough fields on [`Document`] that only the
/// primary input's values survive a merge (D-14). `hwpx_section_xmlns` is
/// deliberately excluded — it is the one documented cross-section union field
/// and is unioned across every input instead of reported as a loss.
fn dropped_package_passthrough_fields(input: &Document) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if input.hwpx_settings_xml.is_some() {
        fields.push("hwpx_settings_xml");
    }
    if input.hwpx_version_xml.is_some() {
        fields.push("hwpx_version_xml");
    }
    if input.hwpx_preview_image.is_some() {
        fields.push("hwpx_preview_image");
    }
    if !input.hwp5_xml_template.is_empty() {
        fields.push("hwp5_xml_template");
    }
    if !input.hwp5_doc_history.is_empty() {
        fields.push("hwp5_doc_history");
    }
    if !input.hwpx_extra_entries.is_empty() {
        fields.push("hwpx_extra_entries");
    }
    if !input.hwpx_bin_manifest.is_empty() {
        fields.push("hwpx_bin_manifest");
    }
    if !input.hwpx_opf_extra_items.is_empty() {
        fields.push("hwpx_opf_extra_items");
    }
    fields
}

/// Merges `inputs` into one document.
///
/// `inputs[0]`'s header, metadata and singular package-passthrough fields
/// (`hwpx_settings_xml`, `hwp5_xml_template`, `hwp5_doc_history`,
/// `hwpx_extra_entries`, `hwpx_bin_manifest`, `hwpx_opf_extra_items`,
/// `hwpx_preview_image`, ...) become the base unchanged — no later input's
/// singular fields are carried (recorded as [`MergeLoss::PackagePassthroughDropped`]).
/// `hwpx_section_xmlns` is the one documented cross-section union field and is
/// unioned across every input. Each later input's Sections are appended in
/// argument order, after its header is grafted onto the running target: the
/// palette-compatible fast path (tier 1) when the input's header is a
/// hwp-cli-default-palette-family member, otherwise the general graft (tier 2)
/// that shifts each DocHeader-referencing id unconditionally (the module doc
/// lists the two deliberately unshifted classes). Both tiers graft the input's
/// hwp5 `header.bin_data` table and shift `BinRef::Id` picture references.
/// Every non-primary input's GSO object identities are renumbered from a
/// running maximum to avoid colliding with an earlier input's.
///
/// Returns `Err` only when `inputs` is empty, a GSO object-id/z-order counter
/// overflows, or a DocHeader collection length / shifted id reference
/// overflows the u16 id space (#173).
pub fn merge_documents(inputs: &[Document]) -> Result<MergeOutcome, String> {
    let (first, rest) = inputs
        .split_first()
        .ok_or_else(|| "병합할 입력이 없습니다".to_string())?;

    let missing_bin_name = unresolved_bin_name(inputs);
    let mut target = first.clone();
    let primary_metadata = target.metadata.clone();
    let mut general_path_used = false;
    let mut losses: Vec<MergeLoss> = Vec::new();
    let mut next_object_id = crate::edit::doc_max_object_id(&target)
        .checked_add(1)
        .ok_or_else(|| "개체 id 오버플로".to_string())?;
    let mut next_z_order = crate::edit::doc_max_table_z_order(&target)
        .checked_add(1)
        .ok_or_else(|| "개체 z-order 오버플로".to_string())?;

    for (i, input) in rest.iter().enumerate() {
        let input_index = i + 1;
        let sections_before = target.sections.len();

        let dropped_bin_data = if palette_compatible(&target.header, &input.header) {
            graft_palette_compatible(&mut target, input, &missing_bin_name)?
        } else {
            general_path_used = true;
            let (offsets, dropped) = graft_header_general(&mut target, input, &missing_bin_name)?;
            let mut unresolvable_border_fill_refs = 0usize;
            for section in &input.sections {
                let mut section = section.clone();
                for p in &mut section.paragraphs {
                    unresolvable_border_fill_refs += remap_paragraph_general(p, &offsets)?;
                }
                target.sections.push(section);
            }
            if unresolvable_border_fill_refs > 0 {
                losses.push(MergeLoss::SectionBorderFillRefUnresolvable {
                    input_index,
                    count: unresolvable_border_fill_refs,
                });
            }
            dropped
        };
        if dropped_bin_data > 0 {
            losses.push(MergeLoss::BinDataDropped {
                input_index,
                count: dropped_bin_data,
            });
        }

        let renumbered = renumber_gso_objects(
            &mut target.sections[sections_before..],
            &mut next_object_id,
            &mut next_z_order,
        )?;
        if renumbered > 0 {
            losses.push(MergeLoss::GsoObjectIdRenumbered { count: renumbered });
        }

        let dropped_fields = dropped_package_passthrough_fields(input);
        if !dropped_fields.is_empty() {
            losses.push(MergeLoss::PackagePassthroughDropped {
                input_index,
                fields: dropped_fields,
            });
        }
        if input.metadata != primary_metadata {
            losses.push(MergeLoss::MetadataSuperseded { input_index });
        }
    }

    target.header.properties.section_count =
        u16::try_from(target.sections.len()).map_err(|_| "구역 수 오버플로".to_string())?;

    Ok(MergeOutcome {
        document: target,
        general_path_used,
        losses,
    })
}

/// Refusal message for a DocHeader collection length or a shifted id
/// reference overflowing the u16 id space (#173) — parallels the GSO
/// object-id/z-order counter refusals.
fn header_id_overflow() -> String {
    "헤더 id 오버플로".to_string()
}

/// Checked `*id += offset` for a DocHeader-referencing u16 id (#173): the
/// merged header's cumulative collections can exceed the u16 id space, in
/// which case the merge refuses rather than silently wrapping the reference.
fn shift_id(id: &mut u16, offset: u16) -> Result<(), String> {
    *id = id.checked_add(offset).ok_or_else(header_id_overflow)?;
    Ok(())
}

/// u16 length of a header collection, refusing when it no longer fits the id
/// space (a shifted reference could not resolve past it).
fn u16_len(len: usize) -> Result<u16, String> {
    u16::try_from(len).map_err(|_| header_id_overflow())
}

/// Grafts `input`'s hwp5 BIN_DATA table onto `target` (appending, so the
/// caller shifts `BinRef::Id` references by the returned offset), reminting a
/// `storage_id` the running target already resolves — through its own
/// bin_data table or through a stream already carrying the canonical name —
/// so every grafted entry's payload keeps resolving under
/// `Document::resolve_bin`'s `BIN{storage_id:04X}.{ext}` rule. Returns the id
/// offset, the storage-id remap [`graft_bin_streams`] renames payload streams
/// with, and the number of entries dropped because the u16 storage-id space
/// was exhausted (once an entry is dropped every later entry is dropped too,
/// keeping the surviving prefix 1:1 aligned with the shifted references).
/// `Err` when the running target's table no longer fits the u16 id space.
fn graft_bin_data(
    target: &mut Document,
    input: &Document,
) -> Result<(u16, HashMap<u16, u16>, usize), String> {
    let offset = u16_len(target.header.bin_data.len())?;
    let mut used: Vec<u16> = target
        .header
        .bin_data
        .iter()
        .filter_map(|b| b.storage_id)
        .collect();
    // One source storage id can be referenced by multiple typed rows. Choose one id that
    // is safe for all their extensions, then reuse it for every row in that source group.
    let mut extensions: HashMap<u16, Vec<&str>> = HashMap::new();
    for item in &input.header.bin_data {
        if let Some(id) = item.storage_id {
            extensions
                .entry(id)
                .or_default()
                .push(item.extension.as_deref().unwrap_or(""));
        }
    }
    let mut remap: HashMap<u16, u16> = HashMap::new();
    let mut dropped = 0usize;
    let mut exhausted = false;

    for item in &input.header.bin_data {
        if exhausted {
            dropped += 1;
            continue;
        }
        let mut item = item.clone();
        if let Some(storage_id) = item.storage_id {
            let reserved = |document: &Document, candidate| {
                extensions[&storage_id]
                    .iter()
                    .any(|ext| bin_stream_name_taken(document, candidate, ext))
            };
            let new_id = if let Some(&mapped) = remap.get(&storage_id) {
                mapped
            } else if !used.contains(&storage_id) && !reserved(target, storage_id) {
                used.push(storage_id);
                remap.insert(storage_id, storage_id);
                storage_id
            } else {
                // Mint an id outside both the target namespace and source carrier names;
                // a preceding orphan must not occupy the new canonical name. Exhausting
                // the u16 space makes this entry and every later one ungraftable.
                let mut candidate = used.iter().max().copied().unwrap_or(0);
                let fresh = loop {
                    match candidate.checked_add(1) {
                        Some(next)
                            if !used.contains(&next)
                                && !reserved(target, next)
                                && !reserved(input, next) =>
                        {
                            break Some(next);
                        }
                        Some(next) => candidate = next,
                        None => break None,
                    }
                };
                match fresh {
                    Some(fresh) => {
                        used.push(fresh);
                        remap.insert(storage_id, fresh);
                        fresh
                    }
                    None => {
                        exhausted = true;
                        dropped = 1;
                        continue;
                    }
                }
            };
            item.storage_id = Some(new_id);
        }
        target.header.bin_data.push(item);
    }
    Ok((offset, remap, dropped))
}

/// Whether a canonical BIN carrier name is reserved by existing streams or the primary
/// manifest namespace. Native numeric refs require a storage-id remint, not a filename prefix.
fn bin_stream_name_taken(target: &Document, storage_id: u16, ext: &str) -> bool {
    bin_namespace_name_taken(target, &format!("BIN{storage_id:04X}.{ext}"))
}

/// If `bin` is the payload stream of one of `bin_data`'s entries (its
/// trailing name matches `BIN{storage_id:04X}.{ext}`), returns the canonical
/// name under the entry's post-graft storage id — the name
/// `Document::resolve_bin` derives for the shifted `BinRef::Id`.
fn reminted_canonical_name(
    bin_data: &[hwp_model::BinDataItem],
    bin: &BinStream,
    storage_remap: &HashMap<u16, u16>,
) -> Option<String> {
    let trailing = bin.name.rsplit('/').next().unwrap_or(&bin.name);
    bin_data.iter().find_map(|item| {
        let storage_id = item.storage_id?;
        let ext = item.extension.as_deref().unwrap_or("");
        if trailing.eq_ignore_ascii_case(&format!("BIN{storage_id:04X}.{ext}")) {
            let new_id = storage_remap
                .get(&storage_id)
                .copied()
                .unwrap_or(storage_id);
            Some(format!("BIN{new_id:04X}.{ext}"))
        } else {
            None
        }
    })
}

/// A carried filename must not be captured by an earlier stream's suffix/stem/substring
/// lookup or by the primary manifest's id/href namespace. Checking containment in this
/// direction lets a longer graft prefix escape a reserved spelling without restoring it.
fn bin_namespace_name_taken(target: &Document, name: &str) -> bool {
    let candidate = name.to_ascii_lowercase();
    target
        .bin_streams
        .iter()
        .map(|bin| bin.name.as_str())
        .chain(
            target
                .hwpx_bin_manifest
                .iter()
                .flat_map(|(id, href)| [id.as_str(), href.as_str()]),
        )
        .any(|reserved| reserved.to_ascii_lowercase().contains(&candidate))
}

/// Reserve a non-content name that no input or eventual graft can resolve. The HWP5
/// writer also has an unlisted-id substring fallback (#453), so absence must hold for
/// substrings, not just full names, stems and suffixes. All inputs are checked up front.
fn unresolved_bin_name(inputs: &[Document]) -> String {
    let reserved: Vec<String> = inputs
        .iter()
        .flat_map(|input| {
            input.bin_streams.iter().map(|bin| bin.name.as_str()).chain(
                input
                    .hwpx_bin_manifest
                    .iter()
                    .flat_map(|(id, href)| [id.as_str(), href.as_str()]),
            )
        })
        .map(|name| format!("_{name}").to_ascii_lowercase())
        .collect();
    let mut counter = 0usize;
    loop {
        let candidate = format!("__hwp_cli_missing_bin_{counter}__");
        if reserved.iter().all(|name| !name.contains(&candidate)) {
            return candidate;
        }
        counter = counter
            .checked_add(1)
            .expect("finite input namespace cannot exhaust missing-name candidates");
    }
}

/// Read every modeled picture reference on the same paragraph paths the two remappers visit.
/// Actual spellings matter: valid suffixes can contain multiple path components, and an
/// unlisted but unresolved id must not gain a meaning through another input's manifest.
fn collect_item_spellings(paragraphs: &[Paragraph], spellings: &mut Vec<String>) {
    for paragraph in paragraphs {
        for control in &paragraph.controls {
            match control {
                Control::Picture(picture) => {
                    if let BinRef::ItemRef(name) = &picture.bin_ref {
                        spellings.push(name.clone());
                    }
                    if let Some(caption) = &picture.caption {
                        collect_item_spellings(&caption.paragraphs, spellings);
                    }
                }
                Control::Table(table) => {
                    if let Some(caption) = &table.caption {
                        collect_item_spellings(&caption.paragraphs, spellings);
                    }
                    for cell in &table.cells {
                        collect_item_spellings(&cell.paragraphs, spellings);
                    }
                }
                Control::Generic(generic) => {
                    if let Some(caption) = &generic.caption {
                        collect_item_spellings(&caption.paragraphs, spellings);
                    }
                    for list in &generic.paragraph_lists {
                        collect_item_spellings(&list.paragraphs, spellings);
                    }
                }
                Control::SectionDef(_) => {}
            }
        }
    }
}

/// Grafts `input`'s bin streams onto `target`. A stream that is the payload
/// of an hwp5 BIN_DATA entry takes the entry's reminted canonical name so
/// `BinRef::Id` references keep resolving; every other stream keeps its name,
/// minting a `{prefix}{n}_` name on a stream/manifest namespace collision. The returned map
/// rewires
/// `BinRef::ItemRef` references. Shared by both grafting tiers.
///
/// The map is keyed by what a picture's `ItemRef` can say, resolved through the INPUT document:
/// every stream name, and every id the input's own manifest lists (to the final name of the
/// stream that id points at in the input). A later input's picture that says `image1` therefore
/// follows its own document's `image1`, not whatever `image1` means in the merged result, whose
/// manifest is the first input's. Unresolvable spellings use the globally reserved missing
/// name, so neither the primary manifest nor a later input can give them another meaning.
fn graft_bin_streams(
    target: &mut Document,
    input: &Document,
    prefix: &str,
    storage_remap: &HashMap<u16, u16>,
    missing_name: &str,
) -> HashMap<String, String> {
    // A double-underscore token cannot begin inside this prefix. Prepending one underscore
    // to every source name above covers the only possible boundary match after `{prefix}{n}_`.
    // Reminted canonical BIN names remove a path and keep their source's extension.
    assert!(
        !prefix.contains("__"),
        "graft prefix must preserve missing-token separation"
    );
    let mut rename: HashMap<String, String> = HashMap::new();
    let mut final_name: Vec<String> = Vec::with_capacity(input.bin_streams.len());
    for bin in &input.bin_streams {
        let mut name = bin.name.clone();
        if let Some(canonical) = reminted_canonical_name(&input.header.bin_data, bin, storage_remap)
        {
            name = canonical;
        } else if bin_namespace_name_taken(target, &name) {
            let mut n = target.bin_streams.len() + 1;
            loop {
                let candidate = format!("{prefix}{n}_{}", bin.name);
                if !bin_namespace_name_taken(target, &candidate) {
                    name = candidate;
                    break;
                }
                n += 1;
            }
        }
        rename.insert(bin.name.clone(), name.clone());
        final_name.push(name.clone());
        target.bin_streams.push(BinStream {
            name,
            data: bin.data.clone(),
        });
    }
    // Known aliases plus every actual modeled `ItemRef`, resolved the way the input resolves
    // it (manifest first), mapped to the final name. Never leave unresolved spellings exposed
    // to the first input's manifest or to another carried stream's fallback names.
    let mut spellings: Vec<String> = input
        .hwpx_bin_manifest
        .iter()
        .map(|(id, _)| id.clone())
        .collect();
    for bin in &input.bin_streams {
        spellings.push(bin.name.clone());
        if let Some(file) = bin.name.rsplit('/').next() {
            spellings.push(file.to_string());
            if let Some(stem) = file.split('.').next() {
                spellings.push(stem.to_string());
            }
        }
    }
    for section in &input.sections {
        collect_item_spellings(&section.paragraphs, &mut spellings);
    }
    for spelling in spellings {
        let name = input
            .bin_stream_index(&BinRef::ItemRef(spelling.clone()))
            .map_or_else(
                || missing_name.to_string(),
                |index| final_name[index].clone(),
            );
        rename.insert(spelling, name);
    }
    rename
}

/// Grafts `input`'s off-palette header extras onto `target` with offset
/// shifts, then appends `input`'s Sections (remapped, otherwise unchanged).
/// Mirrors `hwp_convert::merge::part_paragraphs`'s offset arithmetic, plus
/// the bin-data graft `graft_bin_data` performs for hwp5-sourced inputs (a
/// default-palette-family document saved as .hwp and read back carries
/// `BinRef::Id` pictures). Returns the number of bin_data entries that could
/// not be grafted (u16 storage-id space exhausted) for the caller to record
/// as [`MergeLoss::BinDataDropped`]. `Err` when a header collection length or
/// a shifted id overflows the u16 id space (#173).
fn graft_palette_compatible(
    target: &mut Document,
    input: &Document,
    missing_name: &str,
) -> Result<usize, String> {
    let cs_off = u16_len(target.header.char_shapes.len())? - PALETTE_LEN;
    let ps_off = u16_len(target.header.para_shapes.len())? - BASE_PARA_SHAPES;
    let num_off = u16_len(target.header.numbering_levels.len())?;
    let bul_off = u16_len(target.header.bullet_chars.len())?;

    let (bd_off, storage_remap, dropped) = graft_bin_data(target, input)?;
    let rename = graft_bin_streams(target, input, "merge", &storage_remap, missing_name);

    // Extend target with the input's off-palette extras (numbering/bullet
    // references shifted by the offsets computed above).
    for ps in &input.header.para_shapes[BASE_PARA_SHAPES as usize..] {
        let mut ps = ps.clone();
        match ps.head_type() {
            2 => shift_id(&mut ps.numbering_id, num_off)?, // numbering definition reference
            3 => shift_id(&mut ps.numbering_id, bul_off)?, // bullet definition reference
            _ => {}
        }
        target.header.para_shapes.push(ps);
    }
    target
        .header
        .numbering_levels
        .extend(input.header.numbering_levels.iter().cloned());
    target
        .header
        .bullet_chars
        .extend(input.header.bullet_chars.iter().copied());
    target.header.char_shapes.extend(
        input.header.char_shapes[PALETTE_LEN as usize..]
            .iter()
            .cloned(),
    );

    // hwpx_section_xmlns is the one documented cross-section union field
    // (Document doc comment) — extend, deduplicated, never dropped.
    for xmlns in &input.hwpx_section_xmlns {
        if !target.hwpx_section_xmlns.contains(xmlns) {
            target.hwpx_section_xmlns.push(xmlns.clone());
        }
    }

    for section in &input.sections {
        let mut section = section.clone();
        for p in &mut section.paragraphs {
            remap_paragraph(p, ps_off, cs_off, bd_off, &rename)?;
        }
        target.sections.push(section);
    }
    Ok(dropped)
}

/// Shifts one paragraph's off-palette id references (para shape, char shape
/// runs), bin-data id references (`BinRef::Id`, shifted by the full bin-data
/// offset — bin data has no shared palette base) and rewires bin-stream
/// (Picture) references after a rename, including nested table cells,
/// Generic paragraph lists and captions on every modeled control. A `SectionDef`
/// control is untouched — it matches none of the arms below and stays as-is,
/// which is exactly D-02's "each input's Section carried over unchanged"
/// requirement. Mirrors `hwp_convert::merge::remap_paragraph`. `Err` when a
/// shifted id overflows the u16 id space (#173).
fn remap_paragraph(
    para: &mut Paragraph,
    ps_off: u16,
    cs_off: u16,
    bd_off: u16,
    rename: &HashMap<String, String>,
) -> Result<(), String> {
    if para.para_shape.0 >= BASE_PARA_SHAPES {
        shift_id(&mut para.para_shape.0, ps_off)?;
    }
    for (_, id) in &mut para.char_shape_runs {
        if id.0 >= PALETTE_LEN {
            shift_id(&mut id.0, cs_off)?;
        }
    }
    for control in &mut para.controls {
        match control {
            Control::Table(t) => {
                if let Some(cap) = &mut t.caption {
                    for p in &mut cap.paragraphs {
                        remap_paragraph(p, ps_off, cs_off, bd_off, rename)?;
                    }
                }
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        remap_paragraph(p, ps_off, cs_off, bd_off, rename)?;
                    }
                }
            }
            Control::Generic(g) => {
                for list in &mut g.paragraph_lists {
                    for p in &mut list.paragraphs {
                        remap_paragraph(p, ps_off, cs_off, bd_off, rename)?;
                    }
                }
                // A shape's caption holds paragraphs of its own, with shape ids and pictures
                // like any other, and is not one of its paragraph lists.
                if let Some(cap) = &mut g.caption {
                    for p in &mut cap.paragraphs {
                        remap_paragraph(p, ps_off, cs_off, bd_off, rename)?;
                    }
                }
                // Reference ids changed above — the original XML is stale.
                g.hwpx_raw_xml = None;
            }
            Control::Picture(pic) => {
                match &mut pic.bin_ref {
                    BinRef::ItemRef(name) => {
                        if let Some(new_name) = rename.get(name) {
                            *name = new_name.clone();
                        }
                    }
                    BinRef::Id(id) => shift_id(&mut id.0, bd_off)?,
                }
                if let Some(cap) = &mut pic.caption {
                    for p in &mut cap.paragraphs {
                        remap_paragraph(p, ps_off, cs_off, bd_off, rename)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Per-non-primary-input collection-length offsets [`graft_header_general`]
/// computes before it appends `input`'s header collections onto `target`'s,
/// and [`remap_paragraph_general`] then applies to that input's paragraph
/// tree (`bin_data` shifts hwp5 `BinRef::Id` picture references). The
/// tab-def/numbering/bullet/face offsets are consumed entirely
/// inside `graft_header_general` itself (they shift ids embedded *inside* the
/// appended `ParaShape`/`CharShape` entries, not anything a `Paragraph`
/// references directly) and so stay local to that function rather than living
/// here.
#[derive(Debug)]
struct HeaderOffsets {
    char_shape: u16,
    para_shape: u16,
    style: u16,
    border_fill: u16,
    bin_data: u16,
    bin_rename: HashMap<String, String>,
}

/// Grafts `input`'s header onto `target` with the general (non-palette-fast-path)
/// offset shift: every DocHeader-referencing id `input` carries is shifted by
/// that collection's offset, with no shared-base assumption (unlike tier 1's
/// `graft_palette_compatible`, which only shifts ids past the shared default
/// palette). Border-fill ids are the one exception: 0 is the "unspecified"
/// sentinel (`hwp_model::header::CharShape`/`ParaShape` doc comments; the same
/// 1-based convention applies to `hwp_model::BorderFillId` on `Table`/`Cell`)
/// and is never shifted, so grafting never invents a border where none existed.
/// hwp5 bin-data entries are appended by [`graft_bin_data`]; returns the
/// offsets alongside the number of bin_data entries that could not be grafted
/// (u16 storage-id space exhausted) for the caller to record as
/// [`MergeLoss::BinDataDropped`]. `Err` when a header collection length or a
/// shifted id overflows the u16 id space (#173).
fn graft_header_general(
    target: &mut Document,
    input: &Document,
    missing_name: &str,
) -> Result<(HeaderOffsets, usize), String> {
    let char_shape = u16_len(target.header.char_shapes.len())?;
    let para_shape = u16_len(target.header.para_shapes.len())?;
    let style = u16_len(target.header.styles.len())?;
    let border_fill = u16_len(target.header.border_fills.len())?;
    let tab_def = u16_len(target.header.tab_defs.len())?;
    let numbering = u16_len(target.header.numbering_levels.len())?;
    let bullet = u16_len(target.header.bullet_chars.len())?;
    let mut face = [0u16; LANG_COUNT];
    for (lang, offset) in face.iter_mut().enumerate() {
        *offset = u16_len(target.header.fonts[lang].len())?;
    }

    let (bin_data, storage_remap, dropped) = graft_bin_data(target, input)?;
    let bin_rename = graft_bin_streams(target, input, "graft", &storage_remap, missing_name);

    // Fonts, border fills, tab defs/stops, numberings/numbering_levels and
    // bullets/bullet_chars carry no id references of their own — append verbatim.
    for (target_fonts, input_fonts) in target.header.fonts.iter_mut().zip(&input.header.fonts) {
        target_fonts.extend(input_fonts.iter().cloned());
    }
    target
        .header
        .border_fills
        .extend(input.header.border_fills.iter().cloned());
    target
        .header
        .tab_defs
        .extend(input.header.tab_defs.iter().cloned());
    target
        .header
        .tab_stops
        .extend(input.header.tab_stops.iter().cloned());
    // A raw NUMBERING/BULLET record stands for the modeled entry at its index, and the HWP5
    // writer synthesizes records past the raw ones (#377). Append the input's raw records only
    // while both sides are aligned; otherwise they would land on another input's definitions,
    // and the input's definitions get synthesized records like an HWPX input's do.
    let h = &mut target.header;
    if h.numberings.len() == h.numbering_levels.len()
        && input.header.numberings.len() == input.header.numbering_levels.len()
    {
        h.numberings.extend(input.header.numberings.iter().cloned());
    }
    h.numbering_levels
        .extend(input.header.numbering_levels.iter().cloned());
    if h.bullets.len() == h.bullet_chars.len()
        && input.header.bullets.len() == input.header.bullet_chars.len()
    {
        h.bullets.extend(input.header.bullets.iter().cloned());
    }
    h.bullet_chars
        .extend(input.header.bullet_chars.iter().copied());

    // Styles carry `para_shape`/`char_shape` fields too, but neither is read
    // by any writer or render path (confirmed: no `.para_shape`/`.char_shape`
    // access on a Style anywhere in hwp5/hwpx write or hwp-render) — append
    // verbatim, matching the scope `part_paragraphs`'s reference precedent set.
    target
        .header
        .styles
        .extend(input.header.styles.iter().cloned());

    // CharShapes: shift the per-language face ids, and the border-fill id
    // with the zero-sentinel guard.
    for cs in &input.header.char_shapes {
        let mut cs = cs.clone();
        for (face_id, offset) in cs.face_ids.iter_mut().zip(face) {
            shift_id(face_id, offset)?;
        }
        if cs.border_fill_id != 0 {
            shift_id(&mut cs.border_fill_id, border_fill)?;
        }
        target.header.char_shapes.push(cs);
    }

    // ParaShapes: shift the tab-def id, the numbering/bullet reference
    // (head-type-dependent, same two-arm match `part_paragraphs` performs),
    // and the border-fill id with the zero-sentinel guard.
    for ps in &input.header.para_shapes {
        let mut ps = ps.clone();
        shift_id(&mut ps.tab_def_id, tab_def)?;
        match ps.head_type() {
            2 => shift_id(&mut ps.numbering_id, numbering)?,
            3 => shift_id(&mut ps.numbering_id, bullet)?,
            _ => {}
        }
        if ps.border_fill_id != 0 {
            shift_id(&mut ps.border_fill_id, border_fill)?;
        }
        target.header.para_shapes.push(ps);
    }

    // hwpx_section_xmlns is the one documented cross-section union field —
    // extend, deduplicated, never dropped (same as tier 1).
    for xmlns in &input.hwpx_section_xmlns {
        if !target.hwpx_section_xmlns.contains(xmlns) {
            target.hwpx_section_xmlns.push(xmlns.clone());
        }
    }

    Ok((
        HeaderOffsets {
            char_shape,
            para_shape,
            style,
            border_fill,
            bin_data,
            bin_rename,
        },
        dropped,
    ))
}

/// General-graft counterpart of `remap_paragraph`: shifts every id
/// unconditionally (no shared-base guard — there is no assumed shared base on
/// this path), adds `para.style`, the `Table`/`Cell` border-fill shift (with
/// the zero-sentinel guard), the section-level page border/fill shift on a
/// `SectionDef` control (#171) and the hwp5 `BinRef::Id` bin-data shift.
/// Recurses into every nested paragraph collection — table cells, Generic
/// paragraph lists, and table/picture captions — so caption paragraphs
/// resolve to the same shifted header entries as body paragraphs and caption
/// pictures keep their rewired bin references. Returns the number of
/// non-numeric `borderFillIDRef` passthrough attributes that could not be
/// shifted and are left resolving against the wrong table (#180) — the
/// caller records them as `MergeLoss::SectionBorderFillRefUnresolvable`.
/// `Err` when a shifted id overflows the u16 id space (#173).
fn remap_paragraph_general(para: &mut Paragraph, offsets: &HeaderOffsets) -> Result<usize, String> {
    let mut unresolvable = 0usize;
    shift_id(&mut para.para_shape.0, offsets.para_shape)?;
    shift_id(&mut para.style.0, offsets.style)?;
    for (_, id) in &mut para.char_shape_runs {
        shift_id(&mut id.0, offsets.char_shape)?;
    }
    for control in &mut para.controls {
        match control {
            Control::Table(t) => {
                if t.border_fill.0 != 0 {
                    shift_id(&mut t.border_fill.0, offsets.border_fill)?;
                }
                if let Some(cap) = &mut t.caption {
                    for p in &mut cap.paragraphs {
                        unresolvable += remap_paragraph_general(p, offsets)?;
                    }
                }
                for cell in &mut t.cells {
                    if cell.border_fill.0 != 0 {
                        shift_id(&mut cell.border_fill.0, offsets.border_fill)?;
                    }
                    for p in &mut cell.paragraphs {
                        unresolvable += remap_paragraph_general(p, offsets)?;
                    }
                }
            }
            Control::Generic(g) => {
                for list in &mut g.paragraph_lists {
                    for p in &mut list.paragraphs {
                        unresolvable += remap_paragraph_general(p, offsets)?;
                    }
                }
                // A shape's caption holds paragraphs of its own (see the fast tier).
                if let Some(cap) = &mut g.caption {
                    for p in &mut cap.paragraphs {
                        unresolvable += remap_paragraph_general(p, offsets)?;
                    }
                }
                // Reference ids changed above — the original XML is stale.
                g.hwpx_raw_xml = None;
            }
            Control::Picture(pic) => {
                match &mut pic.bin_ref {
                    BinRef::ItemRef(name) => {
                        if let Some(new_name) = offsets.bin_rename.get(name) {
                            *name = new_name.clone();
                        }
                    }
                    BinRef::Id(id) => shift_id(&mut id.0, offsets.bin_data)?,
                }
                if let Some(cap) = &mut pic.caption {
                    for p in &mut cap.paragraphs {
                        unresolvable += remap_paragraph_general(p, offsets)?;
                    }
                }
            }
            Control::SectionDef(def) => {
                unresolvable += shift_section_def_border_fills(def, offsets.border_fill)?;
            }
        }
    }
    Ok(unresolvable)
}

/// hwp5 PAGE_BORDER_FILL record tag (한글문서파일형식 5.0 §3 record-tag table:
/// HWPTAG_BEGIN 0x10 + 59), spelled out here so hwp-convert stays
/// format-agnostic (no hwp5 dependency — hub-and-spoke, invariant 1), the
/// same precedent `bookmark.rs`/`field.rs`'s CTRL_DATA_TAG set.
const PAGE_BORDER_FILL_TAG: u16 = 0x0010 + 59;

/// Shifts a grafted-in section's own page border/fill references by the
/// border-fill offset so they resolve against the merged header instead of
/// the primary's border-fill table (#171). Covers both representations: the
/// hwp5 raw records (`page_border_fills_raw`, plus their parallel `extras`
/// copies — the hwp5 re-serialization 정본) and the hwpx
/// `<hp:pageBorderFill>` raw-XML passthrough in `secpr_raw_children`.
/// Returns the number of non-numeric `borderFillIDRef` attributes that
/// could not be shifted (#180).
fn shift_section_def_border_fills(def: &mut SectionDef, offset: u16) -> Result<usize, String> {
    let mut unresolvable = 0usize;
    for raw in &mut def.page_border_fills_raw {
        shift_page_border_fill_raw(raw, offset)?;
    }
    for extra in &mut def.extras {
        if extra.tag == PAGE_BORDER_FILL_TAG {
            shift_page_border_fill_raw(&mut extra.data, offset)?;
        }
    }
    for child in &mut def.secpr_raw_children {
        match shift_secpr_page_border_fill(child, offset)? {
            SecprBorderFillShift::Rewritten(rewritten) => *child = rewritten,
            SecprBorderFillShift::Unresolvable => unresolvable += 1,
            SecprBorderFillShift::Untouched => {}
        }
    }
    Ok(unresolvable)
}

/// Outcome of attempting to shift one `<hp:pageBorderFill>` raw-XML
/// passthrough child (#180).
enum SecprBorderFillShift {
    /// Not a pageBorderFill element, no `borderFillIDRef` at all (nothing
    /// references the table), or the "unspecified" sentinel id 0 — the child
    /// is kept verbatim and records nothing.
    Untouched,
    /// The numeric `borderFillIDRef` was rewritten by the border-fill offset.
    Rewritten(String),
    /// `borderFillIDRef` is present but not a parseable u16 — it cannot be
    /// shifted onto the merged border-fill table, so the child is kept
    /// verbatim and the caller records
    /// `MergeLoss::SectionBorderFillRefUnresolvable` (#180).
    Unresolvable,
}

/// Shifts the 1-based border-fill id (bytes 12..14 — the layout the hwpx
/// writer's `write_page_border_fills` documents) of one hwp5
/// PAGE_BORDER_FILL raw record. A record shorter than 14 B carries no
/// readable id and is left untouched; id 0 is the "unspecified" sentinel and
/// is never shifted.
fn shift_page_border_fill_raw(raw: &mut [u8], offset: u16) -> Result<(), String> {
    if raw.len() < 14 {
        return Ok(());
    }
    let id = u16::from_le_bytes([raw[12], raw[13]]);
    if id == 0 {
        return Ok(());
    }
    let shifted = id.checked_add(offset).ok_or_else(header_id_overflow)?;
    raw[12..14].copy_from_slice(&shifted.to_le_bytes());
    Ok(())
}

/// Rewrites the numeric `borderFillIDRef` attribute of a preserved
/// `<hp:pageBorderFill>` raw-XML child by `offset`. See
/// [`SecprBorderFillShift`] for the three outcomes; in particular a present
/// but non-numeric (or unterminated) reference is reported `Unresolvable`
/// rather than silently passed through (#180). `Err` when the shift
/// overflows the u16 id space (#173).
fn shift_secpr_page_border_fill(child: &str, offset: u16) -> Result<SecprBorderFillShift, String> {
    let is_page_border_fill = child
        .trim_start()
        .strip_prefix('<')
        .and_then(|rest| {
            rest.split(|c: char| c.is_whitespace() || c == '>' || c == '/')
                .next()
        })
        .map(|name| name.rsplit(':').next().unwrap_or(name) == "pageBorderFill")
        .unwrap_or(false);
    if !is_page_border_fill {
        return Ok(SecprBorderFillShift::Untouched);
    }
    const ATTR: &str = "borderFillIDRef=\"";
    let Some(attr_pos) = child.find(ATTR) else {
        return Ok(SecprBorderFillShift::Untouched);
    };
    let value_start = attr_pos + ATTR.len();
    let Some(value_end) = child[value_start..].find('"').map(|i| value_start + i) else {
        return Ok(SecprBorderFillShift::Unresolvable);
    };
    let Ok(id) = child[value_start..value_end].parse::<u16>() else {
        return Ok(SecprBorderFillShift::Unresolvable);
    };
    if id == 0 {
        return Ok(SecprBorderFillShift::Untouched); // "unspecified" sentinel — never shifted
    }
    let shifted = id.checked_add(offset).ok_or_else(header_id_overflow)?;
    Ok(SecprBorderFillShift::Rewritten(format!(
        "{}{shifted}{}",
        &child[..value_start],
        &child[value_end..]
    )))
}

/// Gives every GSO-bearing (`Table`/`Picture`) control in `sections` a fresh
/// object identity so no object from a later-merged input can share a
/// Hancom-visible identity with one from an earlier input. Mirrors
/// `hwp_convert::edit::remap_clone_object_ids`/`remap_clone_object_ids_in_para`
/// (same two-branch shape: write the id into `common_data` bytes 32..36 when
/// the buffer is full-length, otherwise bump `placement.z_order` when there
/// is no `common_data` at all; a truncated non-empty `common_data` has no
/// writable id field, so the object keeps its identity untouched — no id
/// consumed, not counted, #172), but threads `next_id`/`next_z`
/// across every non-primary input the caller processes rather than resetting
/// them per input. Returns the number of objects renumbered.
fn renumber_gso_objects(
    sections: &mut [Section],
    next_id: &mut u32,
    next_z: &mut i32,
) -> Result<usize, String> {
    let mut count = 0usize;
    for section in sections {
        for para in &mut section.paragraphs {
            renumber_gso_objects_in_para(para, next_id, next_z, &mut count)?;
        }
    }
    Ok(count)
}

fn renumber_gso_objects_in_para(
    para: &mut Paragraph,
    next_id: &mut u32,
    next_z: &mut i32,
    count: &mut usize,
) -> Result<(), String> {
    for control in &mut para.controls {
        match control {
            Control::Table(t) => renumber_table_object_id(t, next_id, next_z, count)?,
            Control::Picture(pic) => {
                if pic.common_data.len() >= 36 {
                    let id = *next_id;
                    *next_id = next_id
                        .checked_add(1)
                        .ok_or_else(|| "개체 id 오버플로".to_string())?;
                    pic.common_data[32..36].copy_from_slice(&id.to_le_bytes());
                    *count += 1;
                }
                if let Some(cap) = &mut pic.caption {
                    for p in &mut cap.paragraphs {
                        renumber_gso_objects_in_para(p, next_id, next_z, count)?;
                    }
                }
            }
            Control::Generic(g) => {
                for list in &mut g.paragraph_lists {
                    for p in &mut list.paragraphs {
                        renumber_gso_objects_in_para(p, next_id, next_z, count)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn renumber_table_object_id(
    table: &mut hwp_model::Table,
    next_id: &mut u32,
    next_z: &mut i32,
    count: &mut usize,
) -> Result<(), String> {
    if table.common_data.len() >= 36 {
        let id = *next_id;
        *next_id = next_id
            .checked_add(1)
            .ok_or_else(|| "개체 id 오버플로".to_string())?;
        table.common_data[32..36].copy_from_slice(&id.to_le_bytes());
        *count += 1;
    } else if table.common_data.is_empty()
        && let Some(pl) = &mut table.placement
    {
        pl.z_order = *next_z;
        *next_z = next_z
            .checked_add(1)
            .ok_or_else(|| "개체 z-order 오버플로".to_string())?;
        *count += 1;
    }
    // A truncated non-empty `common_data` (<36 B) has no writable id field:
    // the object keeps its identity — no id consumed, not counted (#172).
    if let Some(cap) = &mut table.caption {
        for p in &mut cap.paragraphs {
            renumber_gso_objects_in_para(p, next_id, next_z, count)?;
        }
    }
    for cell in &mut table.cells {
        for p in &mut cell.paragraphs {
            renumber_gso_objects_in_para(p, next_id, next_z, count)?;
        }
    }
    Ok(())
}

/// Gives freshly compiled paragraphs the object identities they need before they are spliced
/// into `doc`, the remapping `merge` applies to a grafted input, narrowed to what a block
/// compiler produces:
///
/// - every placement-only table gets a z-order whose synthesized HWP instance id
///   (`0x5000_0000 | z_order & 0xffff`) no table, picture or gso container of `doc` and no
///   earlier inserted table uses;
/// - every field gets a non-zero instance id (spec table 152, "unique in the document") that no
///   field of `doc` or of an earlier inserted paragraph carries. The compiler writes id 0 for
///   all of them.
///
/// Both walks reach every place `merge` does: cells, `Generic` paragraph lists and the captions
/// of tables, pictures and `Generic` shapes.
///
/// Pictures keep their identity: the HWP writer draws each synthesized picture id from a
/// counter seeded past the ids the document already holds, and the HWPX writer numbers objects
/// per write. Returns the number of tables and fields renumbered.
pub fn renumber_inserted_objects(
    doc: &Document,
    paragraphs: &mut [Paragraph],
) -> Result<usize, String> {
    let mut used = crate::edit::doc_object_ids(doc);
    let mut next_z = crate::edit::doc_max_table_z_order(doc)
        .checked_add(1)
        .ok_or_else(|| "개체 z-order 오버플로".to_string())?;
    let mut next_field = 0u32;
    for section in &doc.sections {
        for para in &section.paragraphs {
            max_field_id_in_para(para, &mut next_field);
        }
    }
    let mut next_field = next_field
        .checked_add(1)
        .ok_or_else(|| "필드 id 오버플로".to_string())?;
    let mut count = 0usize;
    for para in paragraphs {
        renumber_inserted_in_para(para, &mut used, &mut next_z, &mut next_field, &mut count)?;
    }
    Ok(count)
}

/// `attr(4) etc(1) len(2) WCHAR[len] id(4)` (spec table 152): the byte range of the id.
fn field_id_range(data: &[u8]) -> Option<std::ops::Range<usize>> {
    let len = usize::from(u16::from_le_bytes([*data.get(5)?, *data.get(6)?]));
    let start = 7usize.checked_add(len.checked_mul(2)?)?;
    let end = start.checked_add(4)?;
    (data.len() >= end).then_some(start..end)
}

fn max_field_id_in_para(para: &Paragraph, max: &mut u32) {
    for control in &para.controls {
        match control {
            Control::Generic(g) => {
                if crate::field::is_field_ctrl_id(&g.ctrl_id)
                    && let Some(range) = field_id_range(&g.data)
                {
                    *max = (*max).max(u32::from_le_bytes(
                        g.data[range].try_into().expect("range is four bytes"),
                    ));
                }
                for list in &g.paragraph_lists {
                    list.paragraphs
                        .iter()
                        .for_each(|p| max_field_id_in_para(p, max));
                }
                g.caption
                    .iter()
                    .flat_map(|c| &c.paragraphs)
                    .for_each(|p| max_field_id_in_para(p, max));
            }
            Control::Table(t) => {
                t.caption
                    .iter()
                    .flat_map(|c| &c.paragraphs)
                    .for_each(|p| max_field_id_in_para(p, max));
                t.cells
                    .iter()
                    .flat_map(|c| &c.paragraphs)
                    .for_each(|p| max_field_id_in_para(p, max));
            }
            Control::Picture(pic) => pic
                .caption
                .iter()
                .flat_map(|c| &c.paragraphs)
                .for_each(|p| max_field_id_in_para(p, max)),
            Control::SectionDef(_) => {}
        }
    }
}

fn renumber_inserted_in_para(
    para: &mut Paragraph,
    used: &mut std::collections::BTreeSet<u32>,
    next_z: &mut i32,
    next_field: &mut u32,
    count: &mut usize,
) -> Result<(), String> {
    for control in &mut para.controls {
        match control {
            Control::Table(t) => {
                if t.common_data.is_empty()
                    && let Some(placement) = &mut t.placement
                {
                    loop {
                        let z = *next_z;
                        *next_z = z
                            .checked_add(1)
                            .ok_or_else(|| "개체 z-order 오버플로".to_string())?;
                        if used.insert(crate::edit::synthesized_table_object_id(z)) {
                            placement.z_order = z;
                            *count += 1;
                            break;
                        }
                    }
                }
                if let Some(caption) = &mut t.caption {
                    for p in &mut caption.paragraphs {
                        renumber_inserted_in_para(p, used, next_z, next_field, count)?;
                    }
                }
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        renumber_inserted_in_para(p, used, next_z, next_field, count)?;
                    }
                }
            }
            Control::Picture(pic) => {
                if let Some(caption) = &mut pic.caption {
                    for p in &mut caption.paragraphs {
                        renumber_inserted_in_para(p, used, next_z, next_field, count)?;
                    }
                }
            }
            Control::Generic(g) => {
                if crate::field::is_field_ctrl_id(&g.ctrl_id)
                    && let Some(range) = field_id_range(&g.data)
                {
                    let id = *next_field;
                    *next_field = id
                        .checked_add(1)
                        .ok_or_else(|| "필드 id 오버플로".to_string())?;
                    g.data[range].copy_from_slice(&id.to_le_bytes());
                    *count += 1;
                }
                for list in &mut g.paragraph_lists {
                    for p in &mut list.paragraphs {
                        renumber_inserted_in_para(p, used, next_z, next_field, count)?;
                    }
                }
                if let Some(caption) = &mut g.caption {
                    for p in &mut caption.paragraphs {
                        renumber_inserted_in_para(p, used, next_z, next_field, count)?;
                    }
                }
            }
            Control::SectionDef(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::from_markdown::from_markdown;
    use hwp_model::{BorderFillId, CharShapeId, HwpChar};

    fn section_texts(doc: &Document) -> Vec<String> {
        doc.sections
            .iter()
            .map(|s| {
                s.paragraphs
                    .iter()
                    .flat_map(|p| &p.chars)
                    .filter_map(|c| match c {
                        HwpChar::Text(ch) => Some(*ch),
                        _ => None,
                    })
                    .collect::<String>()
            })
            .collect()
    }

    /// #377: raw NUMBERING/BULLET records stay index-aligned with the modeled entries. A target
    /// whose definitions have no raw records (read from HWPX) takes no raw records from an HWP5
    /// input, which would otherwise stand for the target's own definitions.
    #[test]
    fn general_graft_keeps_raw_records_aligned_with_the_modeled_entries() {
        let raw = |byte: u8| hwp_model::RawEntry {
            data: vec![byte; 4],
            children: Vec::new(),
        };
        let mut hwpx_like = non_palette_compatible(from_markdown("가\n"));
        hwpx_like.header.numberings.clear();
        hwpx_like.header.bullets.clear();
        hwpx_like.header.bullet_chars = vec!['x'];
        let mut hwp5_like = non_palette_compatible(from_markdown("나\n"));
        hwp5_like.header.numberings = vec![raw(1); hwp5_like.header.numbering_levels.len()];
        hwp5_like.header.bullet_chars = vec!['y'];
        hwp5_like.header.bullets = vec![raw(2)];

        let merged = merge_documents(&[hwpx_like.clone(), hwp5_like.clone()])
            .unwrap()
            .document;
        assert!(
            merged.header.numberings.is_empty(),
            "no raw record for the HWPX definitions"
        );
        assert!(merged.header.bullets.is_empty());
        assert_eq!(merged.header.bullet_chars, ['x', 'y']);

        // Both aligned: the HWP5 input's records follow the target's, each at its own index.
        let merged = merge_documents(&[hwp5_like.clone(), hwp5_like.clone()])
            .unwrap()
            .document;
        assert_eq!(
            merged.header.numberings.len(),
            merged.header.numbering_levels.len()
        );
        assert_eq!(merged.header.bullets, [raw(2), raw(2)]);

        // Once an HWPX input breaks the alignment, a later HWP5 input's records stay out too.
        let merged = merge_documents(&[hwp5_like.clone(), hwpx_like, hwp5_like.clone()])
            .unwrap()
            .document;
        assert_eq!(
            merged.header.numberings.len(),
            hwp5_like.header.numberings.len()
        );
        assert_eq!(merged.header.bullets, [raw(2)]);
    }

    /// Mutates a `from_markdown` document's header so
    /// `hwp_convert::merge::palette_compatible` returns `false` against every
    /// default-palette-family member, forcing the general (tier 2) graft path —
    /// fixture-free and CI-safe (Task 1's stated test-construction approach).
    fn non_palette_compatible(mut doc: Document) -> Document {
        doc.header.styles.push(hwp_model::Style {
            name: "여분스타일".to_string(),
            english_name: "ExtraStyle".to_string(),
            ..Default::default()
        });
        doc.header
            .border_fills
            .push(hwp_model::BorderFill::default());
        doc
    }

    #[test]
    fn 두_입력_구역_수는_합이다() {
        let a = from_markdown("문서 A\n");
        let b = from_markdown("문서 B\n");
        let outcome = merge_documents(&[a.clone(), b.clone()]).unwrap();
        assert_eq!(
            outcome.document.sections.len(),
            a.sections.len() + b.sections.len()
        );
        assert_eq!(
            outcome.document.header.properties.section_count as usize,
            outcome.document.sections.len()
        );
    }

    #[test]
    fn 구역_본문은_인자_순서를_따른다() {
        let a = from_markdown("문서 A\n");
        let b = from_markdown("문서 B\n");
        let outcome = merge_documents(&[a, b]).unwrap();
        let texts = section_texts(&outcome.document);
        assert!(texts[0].contains("문서 A"));
        assert!(texts[1].contains("문서 B"));
    }

    #[test]
    fn 동일한_입력_두_개는_구역_두_개다() {
        // Adjacency probe (FLOW-01, resolved by D-02): byte-equal inputs still
        // concatenate into two Sections — they never fuse into one.
        let a = from_markdown("같은 문서\n");
        let outcome = merge_documents(&[a.clone(), a]).unwrap();
        assert_eq!(outcome.document.sections.len(), 2);
        let texts = section_texts(&outcome.document);
        assert_eq!(texts[0], texts[1]);
    }

    #[test]
    fn 팔레트_불일치도_일반_경로로_성공한다() {
        // Supersedes the old tier-1-only refusal: a non-palette-compatible
        // input now succeeds through the general graft (task 1), not `Err`.
        let a = from_markdown("문서 A\n");
        let b = non_palette_compatible(from_markdown("문서 B\n"));
        let outcome = merge_documents(&[a, b]).unwrap();
        assert!(outcome.general_path_used);
        assert_eq!(outcome.document.sections.len(), 2);
        let texts = section_texts(&outcome.document);
        assert!(texts[1].contains("문서 B"));
    }

    #[test]
    fn 빈_입력은_에러() {
        assert!(merge_documents(&[]).is_err());
    }

    // ── Task 1: general graft id-reference offset map ──────────────────────

    #[test]
    fn 일반_경로_border_fill_0은_0으로_유지된다() {
        let a = from_markdown("문서 A\n");
        let mut b = non_palette_compatible(from_markdown("문서 B\n"));
        // Force the body paragraph's char shape to a fresh, explicitly
        // unspecified (0) border-fill id.
        b.header.char_shapes.push(hwp_model::CharShape::default()); // border_fill_id: 0
        let cs_id = CharShapeId((b.header.char_shapes.len() - 1) as u16);
        b.sections[0].paragraphs[0].char_shape_runs = vec![(0, cs_id)];

        let outcome = merge_documents(&[a.clone(), b]).unwrap();
        assert!(outcome.general_path_used);

        // The paragraph's char-shape run must resolve to the appended entry,
        // which must still carry border_fill_id == 0 (the zero-sentinel guard).
        let merged_para = &outcome.document.sections[a.sections.len()].paragraphs[0];
        let (_, run_cs_id) = merged_para.char_shape_runs[0];
        let target_char_shapes = &outcome.document.header.char_shapes;
        assert_eq!(run_cs_id.0 as usize, target_char_shapes.len() - 1);
        assert_eq!(target_char_shapes[run_cs_id.0 as usize].border_fill_id, 0);
    }

    #[test]
    fn 일반_경로_border_fill_비영_값은_오프셋만큼_이동한다() {
        let a = from_markdown("문서 A\n");
        let mut b = non_palette_compatible(from_markdown("문서 B\n"));

        let cs = hwp_model::CharShape {
            border_fill_id: 1, // real 1-based reference into b's own border_fills
            ..Default::default()
        };
        b.header.char_shapes.push(cs);
        let cs_id = CharShapeId((b.header.char_shapes.len() - 1) as u16);
        b.sections[0].paragraphs[0].char_shape_runs = vec![(0, cs_id)];

        let expected_border_fill_offset = a.header.border_fills.len() as u16;
        let outcome = merge_documents(&[a.clone(), b]).unwrap();
        let target_char_shapes = &outcome.document.header.char_shapes;
        let merged_para = &outcome.document.sections[a.sections.len()].paragraphs[0];
        let (_, run_cs_id) = merged_para.char_shape_runs[0];
        assert_eq!(
            target_char_shapes[run_cs_id.0 as usize].border_fill_id,
            1 + expected_border_fill_offset
        );
    }

    #[test]
    fn 일반_경로_표_셀_border_fill_0은_유지되고_비영은_이동한다() {
        let a = from_markdown("문서 A\n");
        let b_markdown = "<table><tr><td>가</td><td>나</td></tr></table>\n";
        let mut b = non_palette_compatible(from_markdown(b_markdown));
        // Locate the generated Table control and force one cell's border_fill
        // to 0 (unspecified) and leave the other at its natural non-zero value.
        let table = b.sections[0].paragraphs[0]
            .controls
            .iter_mut()
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("표 컨트롤");
        table.cells[0].border_fill = BorderFillId(0);
        let original_second_border_fill = table.cells[1].border_fill.0;
        assert_ne!(
            original_second_border_fill, 0,
            "표본이 비영이어야 이동을 검증할 수 있다"
        );

        let expected_border_fill_offset = a.header.border_fills.len() as u16;
        let outcome = merge_documents(&[a.clone(), b]).unwrap();
        let merged_table = outcome.document.sections[a.sections.len()].paragraphs[0]
            .controls
            .iter()
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("병합된 표 컨트롤");
        assert_eq!(merged_table.cells[0].border_fill.0, 0, "0은 그대로 유지");
        assert_eq!(
            merged_table.cells[1].border_fill.0,
            original_second_border_fill + expected_border_fill_offset,
            "비영 값은 오프셋만큼 이동"
        );
    }

    #[test]
    fn 일반_경로_스타일_id는_이름이_같은_스타일로_해석된다() {
        let a = from_markdown("문서 A\n");
        let mut b = non_palette_compatible(from_markdown("문서 B\n"));
        let extra_style = hwp_model::Style {
            name: "본문2".to_string(),
            english_name: "Body2".to_string(),
            ..Default::default()
        };
        b.header.styles.push(extra_style.clone());
        let style_id = hwp_model::StyleId((b.header.styles.len() - 1) as u16);
        b.sections[0].paragraphs[0].style = style_id;

        let outcome = merge_documents(&[a.clone(), b]).unwrap();
        let merged_para = &outcome.document.sections[a.sections.len()].paragraphs[0];
        let resolved_style = &outcome.document.header.styles[merged_para.style.0 as usize];
        assert_eq!(resolved_style.name, "본문2");
    }

    #[test]
    fn 일반_경로_빈_스트림_이름_충돌시_새_이름으로_그림이_해석된다() {
        let a_markdown = "![로고](logo.png)\n";
        let mut a = from_markdown(a_markdown);
        a.bin_streams.push(hwp_model::BinStream {
            name: "logo.png".to_string(),
            data: b"AAAA".to_vec(),
        });
        set_first_picture_bin_ref(&mut a, "logo.png");

        let mut b = non_palette_compatible(from_markdown("문서 B\n"));
        b.bin_streams.push(hwp_model::BinStream {
            name: "logo.png".to_string(),
            data: b"BBBB".to_vec(),
        });
        b.sections[0].paragraphs.insert(
            0,
            hwp_model::Paragraph {
                controls: vec![Control::Picture(sample_picture("logo.png"))],
                ..Default::default()
            },
        );

        let outcome = merge_documents(&[a, b]).unwrap();
        let merged_second_section = &outcome.document.sections[1];
        let picture = merged_second_section.paragraphs[0]
            .controls
            .iter()
            .find_map(|c| match c {
                Control::Picture(p) => Some(p),
                _ => None,
            })
            .expect("그림 컨트롤");
        let BinRef::ItemRef(resolved_name) = &picture.bin_ref else {
            panic!("ItemRef expected");
        };
        assert_ne!(
            resolved_name, "logo.png",
            "충돌 시 새 이름으로 교체돼야 한다"
        );
        let resolved_bytes = outcome
            .document
            .bin_streams
            .iter()
            .find(|s| &s.name == resolved_name)
            .map(|s| s.data.as_slice());
        assert_eq!(resolved_bytes, Some(b"BBBB".as_slice()));
    }

    /// The picture of the paragraph at `index` of section `section`, resolved to its bytes.
    fn picture_bytes(doc: &Document, section: usize, index: usize) -> Option<Vec<u8>> {
        let picture = doc.sections[section].paragraphs[index]
            .controls
            .iter()
            .find_map(|c| match c {
                Control::Picture(p) => Some(p),
                _ => None,
            })?;
        doc.resolve_bin(&picture.bin_ref).map(<[u8]>::to_vec)
    }

    /// A document with one picture paragraph that says `item`, carrying one stream `name` with
    /// `data` and a manifest `item -> name`.
    fn document_with_item(item: &str, name: &str, data: &[u8]) -> Document {
        let mut doc = from_markdown("본문\n");
        doc.sections[0].paragraphs.insert(
            0,
            hwp_model::Paragraph {
                controls: vec![Control::Picture(sample_picture(item))],
                ..Default::default()
            },
        );
        doc.bin_streams.push(hwp_model::BinStream {
            name: name.into(),
            data: data.to_vec(),
        });
        doc.hwpx_bin_manifest = vec![(item.into(), name.into())];
        doc
    }

    /// A document whose first paragraph holds a shape (`gso `) with a caption of `caption`.
    fn document_with_shape_caption(caption_paragraph: Paragraph) -> Document {
        let mut doc = from_markdown("본문\n");
        doc.sections[0].paragraphs.insert(
            0,
            hwp_model::Paragraph {
                controls: vec![Control::Generic(hwp_model::GenericControl {
                    ctrl_id: *b"gso ",
                    data: Vec::new(),
                    paragraph_lists: Vec::new(),
                    extras: Vec::new(),
                    raw_children: Vec::new(),
                    gso_shapes: Vec::new(),
                    equation: None,
                    column_def: None,
                    caption: Some(caption_with_para(caption_paragraph)),
                    hwpx_raw_xml: None,
                    container_box: None,
                })],
                ..Default::default()
            },
        );
        doc
    }

    fn shape_caption_paragraph(doc: &Document, section: usize) -> &Paragraph {
        doc.sections[section].paragraphs[0]
            .controls
            .iter()
            .find_map(|c| match c {
                Control::Generic(g) => g.caption.as_ref(),
                _ => None,
            })
            .expect("a shape with a caption")
            .paragraphs
            .first()
            .expect("the caption paragraph")
    }

    #[test]
    fn 병합_뒤_도형_캡션_안_그림도_자기_문서의_이미지를_가리킨다() {
        for general in [false, true] {
            let a = document_with_item("image1", "BinData/p1.png", b"A-IMAGE");
            let picture_paragraph = hwp_model::Paragraph {
                controls: vec![Control::Picture(sample_picture("image1"))],
                ..Default::default()
            };
            let mut b = document_with_shape_caption(picture_paragraph);
            b.bin_streams.push(hwp_model::BinStream {
                name: "BinData/image1.png".into(),
                data: b"B-IMAGE".to_vec(),
            });
            b.hwpx_bin_manifest = vec![("image1".into(), "BinData/image1.png".into())];
            if general {
                b = non_palette_compatible(b);
                b.hwpx_bin_manifest = vec![("image1".into(), "BinData/image1.png".into())];
            }
            let a_sections = a.sections.len();
            let outcome = merge_documents(&[a, b]).unwrap();
            assert_eq!(outcome.general_path_used, general);
            let caption = shape_caption_paragraph(&outcome.document, a_sections);
            let Some(Control::Picture(picture)) = caption.controls.first() else {
                panic!("general={general}: the caption keeps its picture");
            };
            assert_eq!(
                outcome.document.resolve_bin(&picture.bin_ref),
                Some(b"B-IMAGE".as_slice()),
                "general={general}: a caption picture must follow its own input's manifest"
            );
        }
    }

    #[test]
    fn 병합_뒤_도형_캡션_문단도_이동된_모양으로_해석된다() {
        for general in [false, true] {
            let mut a = from_markdown("문서 A\n");
            // The first input carries extra shapes of its own, so the offset a later input's
            // ids must move by is not zero: with an offset of zero, "shifted" and "left alone"
            // look the same and the test cannot tell a missed caption from a handled one.
            a.header.para_shapes.push(hwp_model::ParaShape {
                line_spacing: 111,
                ..Default::default()
            });
            a.header.char_shapes.push(hwp_model::CharShape {
                base_size: 1111,
                ..Default::default()
            });
            // The fast tier only accepts extra char and para shapes (its signature pins the
            // style and border-fill counts), so the marker shapes are added to the header
            // first and the style marker only on the general tier.
            let mut scratch = from_markdown("문서 B\n");
            scratch.header.para_shapes.push(hwp_model::ParaShape {
                line_spacing: 250,
                ..Default::default()
            });
            let para_shape = hwp_model::ParaShapeId((scratch.header.para_shapes.len() - 1) as u16);
            scratch.header.char_shapes.push(hwp_model::CharShape {
                base_size: 4321,
                ..Default::default()
            });
            let char_shape = CharShapeId((scratch.header.char_shapes.len() - 1) as u16);
            let mut b = document_with_shape_caption(Paragraph {
                para_shape,
                char_shape_runs: vec![(0, char_shape)],
                ..Default::default()
            });
            b.header = scratch.header;
            if general {
                b = non_palette_compatible(b);
            }
            let outcome = merge_documents(&[a.clone(), b]).unwrap();
            assert_eq!(outcome.general_path_used, general);
            let merged = &outcome.document;
            let caption = shape_caption_paragraph(merged, a.sections.len());
            // The fast tier shifts only ids past the shared palette and the general tier shifts
            // them all, so the offsets differ; what must hold on both is that the id resolves
            // to the entry the caption paragraph named.
            assert_eq!(
                merged.header.para_shapes[caption.para_shape.0 as usize].line_spacing, 250,
                "general={general}"
            );
            let (_, run) = caption.char_shape_runs[0];
            assert_eq!(
                merged.header.char_shapes[run.0 as usize].base_size, 4321,
                "general={general}: the caption run's char shape resolves to its own entry"
            );
        }
    }

    /// Two HWPX-style inputs that both say `image1`, with different manifests: A's `image1`
    /// is `BinData/p1.png`, B's is `BinData/image1.png`. `general` makes B take the general
    /// graft tier.
    fn two_inputs_reusing_image1(general: bool) -> (Document, Document) {
        let a = document_with_item("image1", "BinData/p1.png", b"A-IMAGE");
        let mut b = document_with_item("image1", "BinData/image1.png", b"B-IMAGE");
        if general {
            b = non_palette_compatible(b);
            b.hwpx_bin_manifest = vec![("image1".into(), "BinData/image1.png".into())];
        }
        (a, b)
    }

    #[test]
    fn 병합_뒤_입력의_그림은_자기_문서의_매니페스트_id로_자기_바이트를_가리킨다() {
        for general in [false, true] {
            let (a, b) = two_inputs_reusing_image1(general);
            let a_sections = a.sections.len();
            let outcome = merge_documents(&[a, b]).unwrap();
            let doc = &outcome.document;
            assert_eq!(
                picture_bytes(doc, 0, 0).as_deref(),
                Some(b"A-IMAGE".as_slice()),
                "general={general}: the first input's picture"
            );
            assert_eq!(
                picture_bytes(doc, a_sections, 0).as_deref(),
                Some(b"B-IMAGE".as_slice()),
                "general={general}: the second input's picture must follow ITS manifest, \
                 not the first input's `image1`"
            );
        }
    }

    #[test]
    fn 병합_뒤_충돌로_이름이_바뀐_스트림도_id로_해석된다() {
        // Both inputs name their stream BinData/image1.png and both manifests say image1; the
        // second stream is renamed on collision, and its id must follow it.
        let a = document_with_item("image1", "BinData/image1.png", b"A-IMAGE");
        let b = document_with_item("image1", "BinData/image1.png", b"B-IMAGE");
        let a_sections = a.sections.len();
        let doc = merge_documents(&[a, b]).unwrap().document;
        assert_eq!(
            picture_bytes(&doc, 0, 0).as_deref(),
            Some(b"A-IMAGE".as_slice())
        );
        assert_eq!(
            picture_bytes(&doc, a_sections, 0).as_deref(),
            Some(b"B-IMAGE".as_slice())
        );
    }

    fn sample_picture(bin_name: &str) -> hwp_model::Picture {
        hwp_model::Picture {
            common_data: Vec::new(),
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
            bin_ref: BinRef::ItemRef(bin_name.to_string()),
            extras: Vec::new(),
        }
    }

    fn set_first_picture_bin_ref(doc: &mut Document, name: &str) {
        for section in &mut doc.sections {
            for para in &mut section.paragraphs {
                for control in &mut para.controls {
                    if let Control::Picture(pic) = control {
                        pic.bin_ref = BinRef::ItemRef(name.to_string());
                        return;
                    }
                }
            }
        }
    }

    // ── Review follow-up (#163): caption paragraphs on the general path ────

    /// Builds a caption whose single paragraph is `para`.
    fn caption_with_para(para: Paragraph) -> hwp_model::Caption {
        hwp_model::Caption {
            side: hwp_model::CaptionSide::Bottom,
            direction: hwp_model::CaptionDirection::Horizontal,
            gap: 0,
            width: None,
            last_width: 0,
            paragraphs: vec![para],
        }
    }

    /// Pushes fresh marker entries onto `doc`'s header and returns the ids a
    /// caption paragraph then references.
    fn push_caption_marker_shapes(
        doc: &mut Document,
    ) -> (hwp_model::ParaShapeId, hwp_model::StyleId, CharShapeId) {
        doc.header.para_shapes.push(hwp_model::ParaShape {
            line_spacing: 250,
            ..Default::default()
        });
        let para_shape = hwp_model::ParaShapeId((doc.header.para_shapes.len() - 1) as u16);
        doc.header.styles.push(hwp_model::Style {
            name: "캡션스타일".to_string(),
            english_name: "CaptionStyle".to_string(),
            ..Default::default()
        });
        let style = hwp_model::StyleId((doc.header.styles.len() - 1) as u16);
        doc.header.char_shapes.push(hwp_model::CharShape {
            base_size: 4321,
            ..Default::default()
        });
        let char_shape = CharShapeId((doc.header.char_shapes.len() - 1) as u16);
        (para_shape, style, char_shape)
    }

    /// Asserts a merged caption paragraph's ids moved by the primary input's
    /// collection lengths and resolve to the appended marker entries.
    fn assert_caption_shapes_shifted(
        merged: &Document,
        primary: &Document,
        caption_para: &Paragraph,
        markers: (hwp_model::ParaShapeId, hwp_model::StyleId, CharShapeId),
    ) {
        let (para_shape, style, char_shape) = markers;
        assert_eq!(
            caption_para.para_shape.0,
            para_shape.0 + primary.header.para_shapes.len() as u16
        );
        assert_eq!(
            merged.header.para_shapes[caption_para.para_shape.0 as usize].line_spacing,
            250
        );
        assert_eq!(
            caption_para.style.0,
            style.0 + primary.header.styles.len() as u16
        );
        assert_eq!(
            merged.header.styles[caption_para.style.0 as usize].name,
            "캡션스타일"
        );
        let (_, run_cs) = caption_para.char_shape_runs[0];
        assert_eq!(
            run_cs.0,
            char_shape.0 + primary.header.char_shapes.len() as u16
        );
        assert_eq!(merged.header.char_shapes[run_cs.0 as usize].base_size, 4321);
    }

    #[test]
    fn 일반_경로_표_캡션_문단도_이동된_모양으로_해석된다() {
        let a = from_markdown("문서 A\n");
        let mut b = non_palette_compatible(from_markdown("<table><tr><td>가</td></tr></table>\n"));
        let markers = push_caption_marker_shapes(&mut b);
        let (para_shape, style, char_shape) = markers;

        let table = b.sections[0].paragraphs[0]
            .controls
            .iter_mut()
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("표 컨트롤");
        table.caption = Some(caption_with_para(Paragraph {
            para_shape,
            style,
            char_shape_runs: vec![(0, char_shape)],
            ..Default::default()
        }));

        let outcome = merge_documents(&[a.clone(), b]).unwrap();
        assert!(outcome.general_path_used);
        let merged_table = outcome.document.sections[a.sections.len()]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("병합된 표 컨트롤");
        let caption_para = &merged_table
            .caption
            .as_ref()
            .expect("캡션이 유지돼야 한다")
            .paragraphs[0];
        assert_caption_shapes_shifted(&outcome.document, &a, caption_para, markers);
    }

    #[test]
    fn 일반_경로_그림_캡션_문단도_이동된_모양으로_해석된다() {
        let a = from_markdown("문서 A\n");
        let mut b = non_palette_compatible(from_markdown("문서 B\n"));
        let markers = push_caption_marker_shapes(&mut b);
        let (para_shape, style, char_shape) = markers;

        let mut picture = sample_picture("cap-pic.png");
        picture.caption = Some(caption_with_para(Paragraph {
            para_shape,
            style,
            char_shape_runs: vec![(0, char_shape)],
            ..Default::default()
        }));
        b.sections[0].paragraphs.insert(
            0,
            Paragraph {
                controls: vec![Control::Picture(picture)],
                ..Default::default()
            },
        );

        let outcome = merge_documents(&[a.clone(), b]).unwrap();
        assert!(outcome.general_path_used);
        let merged_picture = outcome.document.sections[a.sections.len()]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Picture(p) => Some(p),
                _ => None,
            })
            .expect("병합된 그림 컨트롤");
        let caption_para = &merged_picture
            .caption
            .as_ref()
            .expect("캡션이 유지돼야 한다")
            .paragraphs[0];
        assert_caption_shapes_shifted(&outcome.document, &a, caption_para, markers);
    }

    // ── Review follow-up (#164): BinRef::Id / header.bin_data grafting ─────

    /// Pushes an hwp5-style embedded bin-data entry plus its payload stream
    /// (canonical `BIN{storage_id:04X}.png` name, as the hwp5 reader emits).
    fn push_hwp5_bin(doc: &mut Document, storage_id: u16, data: &[u8]) {
        doc.header.bin_data.push(hwp_model::BinDataItem {
            attr: 1, // kind 1: embedded
            storage_id: Some(storage_id),
            extension: Some("png".to_string()),
            ..Default::default()
        });
        doc.bin_streams.push(BinStream {
            name: format!("BIN{storage_id:04X}.png"),
            data: data.to_vec(),
        });
    }

    /// Inserts a paragraph whose picture references `header.bin_data` entry
    /// `id` (1-based, as the hwp5 reader emits).
    fn insert_id_picture(doc: &mut Document, id: u16) {
        let mut picture = sample_picture("");
        picture.bin_ref = BinRef::Id(hwp_model::BinDataId(id));
        doc.sections[0].paragraphs.insert(
            0,
            Paragraph {
                controls: vec![Control::Picture(picture)],
                ..Default::default()
            },
        );
    }

    fn only_picture(section: &Section) -> &hwp_model::Picture {
        section
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Picture(p) => Some(p),
                _ => None,
            })
            .expect("그림 컨트롤")
    }

    #[test]
    fn 일반_경로_bin_ref_id_그림은_그라프트된_bin_data로_해석된다() {
        let mut a = from_markdown("문서 A\n");
        push_hwp5_bin(&mut a, 1, b"AAAA");
        insert_id_picture(&mut a, 1);
        let mut b = non_palette_compatible(from_markdown("문서 B\n"));
        push_hwp5_bin(&mut b, 1, b"BBBB"); // storage id collides with a's
        insert_id_picture(&mut b, 1);

        let outcome = merge_documents(&[a, b]).unwrap();
        assert!(outcome.general_path_used);
        assert!(
            !outcome
                .losses
                .iter()
                .any(|loss| matches!(loss, MergeLoss::BinDataDropped { .. })),
            "그라프트가 성공하면 bin-data 손실은 기록되지 않는다"
        );

        // The picture's reference is shifted by the primary's table length and
        // resolution lands on the merged-in input's own payload bytes.
        let merged_picture = only_picture(&outcome.document.sections[1]);
        let BinRef::Id(id) = &merged_picture.bin_ref else {
            panic!("BinRef::Id expected");
        };
        assert_eq!(id.0, 2, "기본 입력의 bin_data 하나만큼 이동해야 한다");
        assert_eq!(
            outcome.document.resolve_bin(&merged_picture.bin_ref),
            Some(b"BBBB".as_slice())
        );
        // The grafted entry's storage id was reminted so its canonical stream
        // name no longer collides with the primary's payload stream.
        let grafted = &outcome.document.header.bin_data[(id.0 - 1) as usize];
        assert_eq!(grafted.storage_id, Some(2));

        // The primary input's own picture still resolves to its own payload.
        let primary_picture = only_picture(&outcome.document.sections[0]);
        assert_eq!(
            outcome.document.resolve_bin(&primary_picture.bin_ref),
            Some(b"AAAA".as_slice())
        );
    }

    #[test]
    fn 일반_경로_bin_ref_id는_충돌_없으면_원래_storage_id를_유지한다() {
        let a = from_markdown("문서 A\n"); // no bin_data of its own
        let mut b = non_palette_compatible(from_markdown("문서 B\n"));
        push_hwp5_bin(&mut b, 7, b"CCCC");
        insert_id_picture(&mut b, 1);

        let outcome = merge_documents(&[a, b]).unwrap();
        let merged_picture = only_picture(&outcome.document.sections[1]);
        let BinRef::Id(id) = &merged_picture.bin_ref else {
            panic!("BinRef::Id expected");
        };
        assert_eq!(id.0, 1, "기본 입력의 bin_data가 비어 있으면 이동 없음");
        assert_eq!(
            outcome.document.header.bin_data[0].storage_id,
            Some(7),
            "충돌이 없으면 storage id를 유지한다"
        );
        assert_eq!(
            outcome.document.resolve_bin(&merged_picture.bin_ref),
            Some(b"CCCC".as_slice())
        );
    }

    #[test]
    fn 일반_경로_캡션_안_그림의_bin_ref_id도_이동한다() {
        let mut a = from_markdown("문서 A\n");
        push_hwp5_bin(&mut a, 1, b"AAAA");
        let mut b = non_palette_compatible(from_markdown("<table><tr><td>가</td></tr></table>\n"));
        push_hwp5_bin(&mut b, 1, b"BBBB");

        // The table's caption carries a picture referencing b's bin_data.
        let mut caption_picture = sample_picture("");
        caption_picture.bin_ref = BinRef::Id(hwp_model::BinDataId(1));
        let table = b.sections[0].paragraphs[0]
            .controls
            .iter_mut()
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("표 컨트롤");
        table.caption = Some(caption_with_para(Paragraph {
            controls: vec![Control::Picture(caption_picture)],
            ..Default::default()
        }));

        let outcome = merge_documents(&[a, b]).unwrap();
        let merged_table = outcome.document.sections[1]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("병합된 표 컨트롤");
        let caption_picture = merged_table.caption.as_ref().expect("캡션").paragraphs[0]
            .controls
            .iter()
            .find_map(|c| match c {
                Control::Picture(p) => Some(p),
                _ => None,
            })
            .expect("캡션 안 그림");
        let BinRef::Id(id) = &caption_picture.bin_ref else {
            panic!("BinRef::Id expected");
        };
        assert_eq!(
            id.0, 2,
            "캡션 안 그림의 bin 참조도 오프셋만큼 이동해야 한다"
        );
        assert_eq!(
            outcome.document.resolve_bin(&caption_picture.bin_ref),
            Some(b"BBBB".as_slice())
        );
    }

    #[test]
    fn 실제_hwpx_표본과_병합해도_구역과_표가_유지된다() {
        // The one committed fixture exception (fixtures/README.md) — exercises
        // the general path against a genuine document, not only synthesized
        // headers. No HWP_CORPUS_DIR needed.
        let fixture_path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/samples/report-tables.hwpx"
        ));
        let sample = hwpx::read::read_document(fixture_path)
            .expect("report-tables.hwpx must be committed")
            .document;
        let generated = from_markdown("생성 문서\n");

        let sample_section_count = sample.sections.len();
        let sample_had_table = sample.sections.iter().any(|s| {
            s.paragraphs
                .iter()
                .any(|p| p.controls.iter().any(|c| matches!(c, Control::Table(_))))
        });
        assert!(sample_had_table, "표본에 표가 있어야 이 테스트가 의미 있다");

        let outcome = merge_documents(&[sample, generated.clone()]).unwrap();
        assert!(
            outcome.general_path_used,
            "정품 hwpx 표본은 팔레트 비호환이어야 일반 경로가 실행된다"
        );
        assert_eq!(
            outcome.document.sections.len(),
            sample_section_count + generated.sections.len()
        );
        let merged_has_table = outcome.document.sections[..sample_section_count]
            .iter()
            .any(|s| {
                s.paragraphs
                    .iter()
                    .any(|p| p.controls.iter().any(|c| matches!(c, Control::Table(_))))
            });
        assert!(merged_has_table, "표본의 표가 병합 후에도 남아 있어야 한다");
    }

    // ── Task 2: GSO object identity renumbering ─────────────────────────────

    fn table_with_common_id(id: u32) -> Control {
        let mut common_data = vec![0u8; 44];
        common_data[32..36].copy_from_slice(&id.to_le_bytes());
        Control::Table(hwp_model::Table {
            common_data,
            placement: None,
            attr: 0,
            rows: 1,
            cols: 1,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: vec![1],
            border_fill: BorderFillId(0),
            table_tail: Vec::new(),
            cells: vec![hwp_model::Cell {
                list_attr: 0,
                col: 0,
                row: 0,
                col_span: 1,
                row_span: 1,
                width: hwp_model::HwpUnit(100),
                height: hwp_model::HwpUnit(100),
                margins: [0; 4],
                border_fill: BorderFillId(0),
                header_tail: Vec::new(),
                paragraphs: vec![Paragraph::default()],
            }],
            caption: None,
            extras: Vec::new(),
        })
    }

    fn table_object_id(control: &Control) -> u32 {
        match control {
            Control::Table(t) => u32::from_le_bytes(t.common_data[32..36].try_into().unwrap()),
            _ => panic!("Table expected"),
        }
    }

    fn insert_table(doc: &mut Document, id: u32) {
        doc.sections[0].paragraphs.insert(
            0,
            Paragraph {
                controls: vec![table_with_common_id(id)],
                ..Default::default()
            },
        );
    }

    #[test]
    fn 두_입력의_표_id는_서로_다르다() {
        let mut a = from_markdown("문서 A\n");
        insert_table(&mut a, 7);
        let mut b = from_markdown("문서 B\n");
        insert_table(&mut b, 7); // same id as `a` — must be renumbered

        let outcome = merge_documents(&[a, b]).unwrap();
        let ids: Vec<u32> = outcome
            .document
            .sections
            .iter()
            .flat_map(|s| &s.paragraphs)
            .flat_map(|p| &p.controls)
            .filter(|c| matches!(c, Control::Table(_)))
            .map(table_object_id)
            .collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
    }

    #[test]
    fn 세_입력의_표_id는_모두_다르다() {
        let mut a = from_markdown("문서 A\n");
        insert_table(&mut a, 1);
        let mut b = from_markdown("문서 B\n");
        insert_table(&mut b, 1);
        let mut c = from_markdown("문서 C\n");
        insert_table(&mut c, 1);

        let outcome = merge_documents(&[a, b, c]).unwrap();
        let mut ids: Vec<u32> = outcome
            .document
            .sections
            .iter()
            .flat_map(|s| &s.paragraphs)
            .flat_map(|p| &p.controls)
            .filter(|c| matches!(c, Control::Table(_)))
            .map(table_object_id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 3, "세 표 id가 모두 달라야 한다");
    }

    #[test]
    fn 기본_입력의_표_id는_변경되지_않는다() {
        let mut a = from_markdown("문서 A\n");
        insert_table(&mut a, 42);
        let mut b = from_markdown("문서 B\n");
        insert_table(&mut b, 1);

        let outcome = merge_documents(&[a, b]).unwrap();
        let primary_id = table_object_id(&outcome.document.sections[0].paragraphs[0].controls[0]);
        assert_eq!(primary_id, 42, "기본 입력의 id는 그대로 유지돼야 한다");
    }

    #[test]
    fn 표_id_재부여는_손실_목록에_기록된다() {
        let mut a = from_markdown("문서 A\n");
        insert_table(&mut a, 7);
        let mut b = from_markdown("문서 B\n");
        insert_table(&mut b, 7);

        let outcome = merge_documents(&[a, b]).unwrap();
        assert!(
            outcome.losses.iter().any(
                |loss| matches!(loss, MergeLoss::GsoObjectIdRenumbered { count } if *count == 1)
            )
        );
    }

    #[test]
    fn hwpx_출신_배치_z_순서도_서로_다르게_재부여된다() {
        let mut a = from_markdown("문서 A\n");
        with_placement_table(&mut a, 3);
        let mut b = from_markdown("문서 B\n");
        with_placement_table(&mut b, 3);

        let outcome = merge_documents(&[a, b]).unwrap();
        let z_orders: Vec<i32> = outcome
            .document
            .sections
            .iter()
            .flat_map(|s| &s.paragraphs)
            .flat_map(|p| &p.controls)
            .filter_map(|c| match c {
                Control::Table(t) => t.placement.as_ref().map(|pl| pl.z_order),
                _ => None,
            })
            .collect();
        assert_eq!(z_orders.len(), 2);
        assert_ne!(z_orders[0], z_orders[1]);
    }

    fn with_placement_table(doc: &mut Document, z_order: i32) {
        let table = Control::Table(hwp_model::Table {
            common_data: Vec::new(),
            placement: Some(hwp_model::GsoPlacement {
                z_order,
                ..Default::default()
            }),
            attr: 0,
            rows: 1,
            cols: 1,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: vec![1],
            border_fill: BorderFillId(0),
            table_tail: Vec::new(),
            cells: vec![hwp_model::Cell {
                list_attr: 0,
                col: 0,
                row: 0,
                col_span: 1,
                row_span: 1,
                width: hwp_model::HwpUnit(100),
                height: hwp_model::HwpUnit(100),
                margins: [0; 4],
                border_fill: BorderFillId(0),
                header_tail: Vec::new(),
                paragraphs: vec![Paragraph::default()],
            }],
            caption: None,
            extras: Vec::new(),
        });
        doc.sections[0].paragraphs.insert(
            0,
            Paragraph {
                controls: vec![table],
                ..Default::default()
            },
        );
    }

    // ── Task 3: additive preservation codes for the document-level loss surface ──

    #[test]
    fn 두번째_입력의_hwpx_설정이_있으면_손실로_기록된다() {
        let a = from_markdown("문서 A\n");
        let mut b = from_markdown("문서 B\n");
        b.hwpx_settings_xml = Some("<settings/>".to_string());

        let outcome = merge_documents(&[a, b]).unwrap();
        assert_eq!(
            outcome.losses,
            vec![MergeLoss::PackagePassthroughDropped {
                input_index: 1,
                fields: vec!["hwpx_settings_xml"],
            }]
        );
    }

    #[test]
    fn 메타데이터가_다르면_손실로_기록된다() {
        let mut a = from_markdown("문서 A\n");
        a.metadata.title = Some("제목A".to_string());
        let mut b = from_markdown("문서 B\n");
        b.metadata.title = Some("제목B".to_string());

        let outcome = merge_documents(&[a, b]).unwrap();
        assert!(
            outcome
                .losses
                .iter()
                .any(|loss| matches!(loss, MergeLoss::MetadataSuperseded { input_index: 1 }))
        );
    }

    #[test]
    fn 손실이_없으면_비어_있다() {
        let a = from_markdown("문서 A\n");
        let b = from_markdown("문서 B\n");
        let outcome = merge_documents(&[a, b]).unwrap();
        assert!(outcome.losses.is_empty());
    }

    // ── Review follow-up (#171): section-level page border/fill shifting ────

    /// 14-byte hwp5 PAGE_BORDER_FILL raw record with the 1-based border-fill
    /// id at bytes 12..14 (the layout the hwpx writer's
    /// `write_page_border_fills` documents).
    fn page_border_fill_raw(border_fill_id: u16) -> Vec<u8> {
        let mut raw = vec![0u8; 14];
        raw[12..14].copy_from_slice(&border_fill_id.to_le_bytes());
        raw
    }

    fn raw_border_fill_id(raw: &[u8]) -> u16 {
        u16::from_le_bytes([raw[12], raw[13]])
    }

    /// Mutates the SectionDef control of `doc`'s first section (carried by
    /// the first paragraph, as `from_markdown` emits).
    fn with_first_section_def(doc: &mut Document, f: impl FnOnce(&mut SectionDef)) {
        let def = doc.sections[0].paragraphs[0]
            .controls
            .iter_mut()
            .find_map(|c| match c {
                Control::SectionDef(sd) => Some(sd),
                _ => None,
            })
            .expect("구역 정의 컨트롤");
        f(def);
    }

    fn section_def(doc: &Document, section: usize) -> &SectionDef {
        doc.sections[section].paragraphs[0]
            .controls
            .iter()
            .find_map(|c| match c {
                Control::SectionDef(sd) => Some(sd),
                _ => None,
            })
            .expect("구역 정의 컨트롤")
    }

    #[test]
    fn 일반_경로_hwp5_쪽_테두리_raw도_오프셋만큼_이동한다() {
        let a = from_markdown("문서 A\n");
        let mut b = non_palette_compatible(from_markdown("문서 B\n"));
        // id 1 (real reference) and id 0 (unspecified sentinel), mirrored in
        // the parallel `extras` copies the hwp5 writer re-emits.
        with_first_section_def(&mut b, |def| {
            def.page_border_fills_raw = vec![page_border_fill_raw(1), page_border_fill_raw(0)];
            for raw in [page_border_fill_raw(1), page_border_fill_raw(0)] {
                def.extras.push(hwp_model::OpaqueRecord {
                    tag: 0x0010 + 59, // PAGE_BORDER_FILL
                    data: raw,
                    children: Vec::new(),
                });
            }
        });
        let offset = a.header.border_fills.len() as u16;

        let outcome = merge_documents(&[a.clone(), b]).unwrap();
        assert!(outcome.general_path_used);
        let def = section_def(&outcome.document, a.sections.len());
        assert_eq!(
            raw_border_fill_id(&def.page_border_fills_raw[0]),
            1 + offset
        );
        assert_eq!(
            raw_border_fill_id(&def.page_border_fills_raw[1]),
            0,
            "0은 미지정 센티넬이라 이동하지 않는다"
        );
        // The `extras` parallel copies are shifted identically, so hwp5
        // re-serialization resolves against the merged header too.
        assert_eq!(def.extras[0].data, def.page_border_fills_raw[0]);
        assert_eq!(raw_border_fill_id(&def.extras[1].data), 0);
    }

    #[test]
    fn 일반_경로_hwpx_쪽_테두리_passthrough의_id도_이동한다() {
        let a = from_markdown("문서 A\n");
        let mut b = non_palette_compatible(from_markdown("문서 B\n"));
        with_first_section_def(&mut b, |def| {
            def.secpr_raw_children = vec![
                "<hp:grid lineGrid=\"0\" charGrid=\"0\"/>".to_string(),
                "<hp:pageBorderFill type=\"BOTH\" borderFillIDRef=\"1\" textBorder=\"PAPER\" headerInside=\"0\" footerInside=\"0\" fillArea=\"PAPER\"><hp:offset left=\"1417\" right=\"1417\" top=\"1417\" bottom=\"1417\"/></hp:pageBorderFill>".to_string(),
            ];
        });
        let offset = a.header.border_fills.len() as u16;

        let outcome = merge_documents(&[a.clone(), b]).unwrap();
        let def = section_def(&outcome.document, a.sections.len());
        assert_eq!(
            def.secpr_raw_children[0], "<hp:grid lineGrid=\"0\" charGrid=\"0\"/>",
            "pageBorderFill이 아닌 자식은 원문 그대로다"
        );
        assert!(
            def.secpr_raw_children[1].contains(&format!("borderFillIDRef=\"{}\"", 1 + offset)),
            "borderFillIDRef가 오프셋만큼 이동해야 한다: {}",
            def.secpr_raw_children[1]
        );
        // Numeric references shift cleanly and record no loss (#180).
        assert!(
            !outcome
                .losses
                .iter()
                .any(|loss| matches!(loss, MergeLoss::SectionBorderFillRefUnresolvable { .. })),
            "숫자 참조는 손실 없이 이동해야 한다: {:?}",
            outcome.losses
        );
    }

    #[test]
    fn 일반_경로_숫자가_아닌_border_fill_idref는_원문을_유지하고_손실로_기록한다() {
        let a = from_markdown("문서 A\n");
        let mut b = non_palette_compatible(from_markdown("문서 B\n"));
        let malformed =
            "<hp:pageBorderFill type=\"BOTH\" borderFillIDRef=\"abc\"></hp:pageBorderFill>"
                .to_string();
        with_first_section_def(&mut b, |def| {
            def.secpr_raw_children = vec![malformed.clone()];
        });

        let outcome = merge_documents(&[a.clone(), b]).unwrap();
        let def = section_def(&outcome.document, a.sections.len());
        assert_eq!(
            def.secpr_raw_children[0], malformed,
            "안전하게 재작성할 수 없는 참조는 원문 그대로 둔다"
        );
        // #180: the verbatim passthrough is no longer silent — exactly one
        // typed loss event is recorded for the unshiftable reference.
        let recorded: Vec<&MergeLoss> = outcome
            .losses
            .iter()
            .filter(|loss| matches!(loss, MergeLoss::SectionBorderFillRefUnresolvable { .. }))
            .collect();
        assert_eq!(
            recorded,
            [&MergeLoss::SectionBorderFillRefUnresolvable {
                input_index: 1,
                count: 1
            }],
            "이동하지 못한 참조 하나가 정확히 한 건의 손실로 기록돼야 한다: {:?}",
            outcome.losses
        );
    }

    // ── Review follow-up (#172): truncated common_data consumes nothing ─────

    #[test]
    fn 잘린_common_data_표는_id를_소비하지도_세지도_않는다() {
        let mut a = from_markdown("문서 A\n");
        insert_table(&mut a, 7); // primary max id 7 → next fresh id is 8
        let mut b = from_markdown("문서 B\n");
        let truncated = match table_with_common_id(9) {
            Control::Table(mut t) => {
                t.common_data.truncate(10); // <36 B: no writable id field
                Control::Table(t)
            }
            _ => unreachable!(),
        };
        b.sections[0].paragraphs.insert(
            0,
            Paragraph {
                controls: vec![truncated],
                ..Default::default()
            },
        );
        insert_table(&mut b, 9); // full-length table — the only one renumbered

        let outcome = merge_documents(&[a, b]).unwrap();
        assert!(
            outcome.losses.iter().any(
                |loss| matches!(loss, MergeLoss::GsoObjectIdRenumbered { count } if *count == 1)
            ),
            "전체 길이 표 하나만 재부여돼야 한다"
        );
        let tables: Vec<&hwp_model::Table> = outcome.document.sections[1]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .filter_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .collect();
        let full = tables
            .iter()
            .find(|t| t.common_data.len() >= 36)
            .expect("전체 길이 표");
        assert_eq!(
            u32::from_le_bytes(full.common_data[32..36].try_into().unwrap()),
            8,
            "잘린 표가 id를 소비했다면 8이 아니라 9가 된다"
        );
        let truncated = tables
            .iter()
            .find(|t| t.common_data.len() == 10)
            .expect("잘린 표");
        assert_eq!(
            truncated.common_data,
            vec![0u8; 10],
            "잘린 common_data는 그대로 유지돼야 한다"
        );
    }

    // ── Review follow-up (#173): checked u16 id arithmetic ──────────────────

    #[test]
    fn 일반_경로_u16_id_오버플로는_에러다() {
        let a = from_markdown("문서 A\n");
        let mut b = non_palette_compatible(from_markdown("문서 B\n"));
        b.sections[0].paragraphs[0].para_shape = hwp_model::ParaShapeId(u16::MAX);
        let err = merge_documents(&[a, b]).unwrap_err();
        assert!(err.contains("오버플로"), "오버플로 거부 메시지: {err}");
    }

    #[test]
    fn 팔레트_경로_u16_id_오버플로도_에러다() {
        let a = from_markdown("문서 A\n");
        let mut b = from_markdown("문서 B\n");
        // One off-palette extra, so the running target's para-shape offset for
        // the next input is non-zero.
        b.header.para_shapes.push(hwp_model::ParaShape::default());
        let mut c = from_markdown("문서 C\n");
        c.sections[0].paragraphs[0].para_shape = hwp_model::ParaShapeId(u16::MAX);
        assert!(merge_documents(&[a, b, c]).is_err());
    }

    // ── #428 T3: renumber_inserted_objects ──────────────────────────────────

    fn placement_table(z_order: i32) -> Control {
        let mut table = match table_with_common_id(0) {
            Control::Table(t) => t,
            _ => unreachable!(),
        };
        table.common_data.clear();
        table.placement = Some(hwp_model::GsoPlacement {
            z_order,
            ..Default::default()
        });
        Control::Table(table)
    }

    fn field_control(id: u32, name: &str) -> Control {
        let mut data = crate::field::make_field_command_data(b"%clk", name);
        let range = field_id_range(&data).expect("field id range");
        data[range].copy_from_slice(&id.to_le_bytes());
        Control::Generic(hwp_model::GenericControl {
            ctrl_id: *b"%clk",
            data,
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

    fn stored_field_id(control: &Control) -> u32 {
        let Control::Generic(g) = control else {
            panic!("field control expected")
        };
        let range = field_id_range(&g.data).expect("field id range");
        u32::from_le_bytes(g.data[range].try_into().unwrap())
    }

    fn placement_z(control: &Control) -> i32 {
        let Control::Table(t) = control else {
            panic!("table expected")
        };
        t.placement.as_ref().expect("placement").z_order
    }

    #[test]
    fn inserted_placement_tables_get_synthesized_ids_unused_by_target_or_each_other() {
        let mut target = from_markdown("target\n");
        // A placement table at z-order 2 makes the allocator start at 3. Raw-id tables sit on
        // 0x5000_0003 and 0x5000_0004, exactly the synthesized ids z-orders 3 and 4 would take.
        target.sections[0].paragraphs.insert(
            0,
            Paragraph {
                controls: vec![
                    placement_table(2),
                    table_with_common_id(0x5000_0003),
                    table_with_common_id(0x5000_0004),
                ],
                ..Default::default()
            },
        );
        // Compose gives every compiled table z-order 0; three of them must not share an id.
        let mut inserted: Vec<Paragraph> = (0..3)
            .map(|_| Paragraph {
                controls: vec![placement_table(0)],
                ..Default::default()
            })
            .collect();
        let renumbered = renumber_inserted_objects(&target, &mut inserted).unwrap();
        assert_eq!(renumbered, 3);
        let ids: Vec<u32> = inserted
            .iter()
            .map(|p| crate::edit::synthesized_table_object_id(placement_z(&p.controls[0])))
            .collect();
        let existing = crate::edit::doc_object_ids(&target);
        for id in &ids {
            assert!(!existing.contains(id), "{id:x} collides with the target");
        }
        let unique: std::collections::BTreeSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), 3, "inserted tables share an id: {ids:x?}");
    }

    #[test]
    fn inserted_fields_get_nonzero_ids_distinct_from_target_and_each_other() {
        let mut target = from_markdown("target\n");
        target.sections[0].paragraphs.insert(
            0,
            Paragraph {
                controls: vec![field_control(41, "existing")],
                ..Default::default()
            },
        );
        let mut inserted = vec![
            Paragraph {
                controls: vec![field_control(0, "a")],
                ..Default::default()
            },
            Paragraph {
                controls: vec![placement_table(0)],
                ..Default::default()
            },
        ];
        // A field nested in a cell of an inserted table is renumbered too.
        let Control::Table(table) = &mut inserted[1].controls[0] else {
            unreachable!()
        };
        table.cells[0].paragraphs[0]
            .controls
            .push(field_control(0, "b"));
        renumber_inserted_objects(&target, &mut inserted).unwrap();
        let first = stored_field_id(&inserted[0].controls[0]);
        let Control::Table(table) = &inserted[1].controls[0] else {
            unreachable!()
        };
        let nested = stored_field_id(&table.cells[0].paragraphs[0].controls[0]);
        assert!(first > 41 && nested > 41, "ids must clear the target's 41");
        assert_ne!(first, nested);
    }

    #[test]
    fn renumbering_nothing_changes_nothing() {
        let target = from_markdown("target\n");
        let mut inserted = vec![Paragraph::default()];
        assert_eq!(
            renumber_inserted_objects(&target, &mut inserted).unwrap(),
            0
        );
        assert_eq!(inserted, vec![Paragraph::default()]);
    }

    fn caption_with(controls: Vec<Control>) -> hwp_model::Caption {
        hwp_model::Caption {
            side: hwp_model::CaptionSide::Bottom,
            direction: hwp_model::CaptionDirection::Horizontal,
            gap: 0,
            width: None,
            last_width: 0,
            paragraphs: vec![Paragraph {
                controls,
                ..Default::default()
            }],
        }
    }

    /// Objects hosted in a table or picture caption are inserted content too: their tables and
    /// fields need the same fresh identities as the ones in cells.
    #[test]
    fn inserted_objects_in_table_and_picture_captions_are_renumbered() {
        let mut target = from_markdown("target\n");
        target.sections[0].paragraphs.insert(
            0,
            Paragraph {
                controls: vec![placement_table(0), field_control(9, "existing")],
                ..Default::default()
            },
        );
        let Control::Table(mut outer) = placement_table(0) else {
            unreachable!()
        };
        outer.caption = Some(caption_with(vec![
            placement_table(0),
            field_control(0, "in-table-caption"),
        ]));
        let picture = hwp_model::Picture {
            common_data: Vec::new(),
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
            caption: Some(caption_with(vec![field_control(0, "in-picture-caption")])),
            bin_ref: BinRef::ItemRef("x".into()),
            extras: Vec::new(),
        };
        let mut inserted = vec![Paragraph {
            controls: vec![Control::Table(outer), Control::Picture(picture)],
            ..Default::default()
        }];
        let renumbered = renumber_inserted_objects(&target, &mut inserted).unwrap();
        assert_eq!(
            renumbered, 4,
            "outer table, caption table, two caption fields"
        );
        let Control::Table(outer) = &inserted[0].controls[0] else {
            unreachable!()
        };
        let caption = outer.caption.as_ref().unwrap();
        let nested_table_z = placement_z(&caption.paragraphs[0].controls[0]);
        let outer_z = outer.placement.as_ref().unwrap().z_order;
        assert_ne!(
            nested_table_z, outer_z,
            "caption table shares the outer table's id"
        );
        assert_ne!(
            nested_table_z, 0,
            "caption table kept the compiler's z-order 0"
        );
        let table_field = stored_field_id(&caption.paragraphs[0].controls[1]);
        let Control::Picture(picture) = &inserted[0].controls[1] else {
            unreachable!()
        };
        let picture_field =
            stored_field_id(&picture.caption.as_ref().unwrap().paragraphs[0].controls[0]);
        assert!(table_field > 9 && picture_field > 9);
        assert_ne!(table_field, picture_field);
    }

    fn shape_with_caption(controls: Vec<Control>) -> Control {
        Control::Generic(hwp_model::GenericControl {
            ctrl_id: *b"gso ",
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: Some(caption_with(controls)),
            hwpx_raw_xml: None,
            container_box: None,
        })
    }

    /// #446 made merge walk `GenericControl::caption`. Identities held there by the target are
    /// taken ids, and objects inserted there need fresh ones, exactly like those in cells.
    #[test]
    fn identities_in_shape_captions_are_counted_in_the_target_and_renumbered_when_inserted() {
        let mut target = from_markdown("target\n");
        target.sections[0].paragraphs.insert(
            0,
            Paragraph {
                controls: vec![shape_with_caption(vec![
                    table_with_common_id(0x5000_0001),
                    field_control(77, "in-shape-caption"),
                    placement_table(5),
                ])],
                ..Default::default()
            },
        );
        let mut inserted = vec![
            Paragraph {
                controls: vec![placement_table(0)],
                ..Default::default()
            },
            Paragraph {
                controls: vec![shape_with_caption(vec![
                    placement_table(0),
                    field_control(0, "inserted-in-shape-caption"),
                ])],
                ..Default::default()
            },
        ];
        let renumbered = renumber_inserted_objects(&target, &mut inserted).unwrap();
        assert_eq!(
            renumbered, 3,
            "two tables and a field, one table inside a shape caption"
        );
        let existing = crate::edit::doc_object_ids(&target);
        assert!(existing.contains(&0x5000_0001) && existing.contains(&0x5000_0005));
        let top = crate::edit::synthesized_table_object_id(placement_z(&inserted[0].controls[0]));
        let Control::Generic(shape) = &inserted[1].controls[0] else {
            unreachable!()
        };
        let caption = shape.caption.as_ref().unwrap();
        let nested = crate::edit::synthesized_table_object_id(placement_z(
            &caption.paragraphs[0].controls[0],
        ));
        for id in [top, nested] {
            assert!(
                !existing.contains(&id),
                "{id:x} collides with a target caption table"
            );
        }
        assert_ne!(top, nested);
        assert!(
            stored_field_id(&caption.paragraphs[0].controls[1]) > 77,
            "the inserted caption field must clear the target caption's field id 77"
        );
    }

    /// Placement tables draw their synthesized HWP id from 65,536 values
    /// (`0x5000_0000 | z & 0xffff`). When the target holds every one of them there is nothing
    /// to hand out, and the search must stop with a named error rather than walk z-orders up to
    /// the i32 limit (about 2^31 attempts).
    #[test]
    fn a_target_holding_every_synthesized_table_id_fails_fast_with_a_named_error() {
        let mut target = from_markdown("target\n");
        target.sections[0].paragraphs.insert(
            0,
            Paragraph {
                controls: (0..=0xffff_u32)
                    .map(|low| table_with_common_id(0x5000_0000 | low))
                    .collect(),
                ..Default::default()
            },
        );
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut inserted = vec![Paragraph {
                controls: vec![placement_table(0)],
                ..Default::default()
            }];
            let _ = sender.send(renumber_inserted_objects(&target, &mut inserted));
        });
        let outcome = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("renumbering did not finish within 10 s: the id search is unbounded");
        let error = outcome.expect_err("no table id is left to hand out");
        assert!(
            error.contains("65536") && error.contains("표"),
            "the error must name the exhausted id space and the object: {error}"
        );
    }
}
