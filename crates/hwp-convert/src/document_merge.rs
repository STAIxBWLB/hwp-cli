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
//! lists, and table/picture captions. The input's `header.bin_data` table is
//! appended with colliding `storage_id`s reminted (and the payload streams
//! renamed to match) so `BinRef::Id` keeps resolving through
//! `Document::resolve_bin`'s `BIN{storage_id:04X}.{ext}` rule. A
//! `border_fill_id`/`border_fill` of 0 is the "unspecified" sentinel (not an
//! index) and is never shifted, so grafting never invents a border where none
//! existed. Two reference classes are deliberately not shifted: `Style`
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

use hwp_model::{BinRef, BinStream, Control, Document, LANG_COUNT, Paragraph, Section};

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
/// Returns `Err` only when `inputs` is empty or a GSO object-id/z-order
/// counter overflows.
pub fn merge_documents(inputs: &[Document]) -> Result<MergeOutcome, String> {
    let (first, rest) = inputs
        .split_first()
        .ok_or_else(|| "병합할 입력이 없습니다".to_string())?;

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
            graft_palette_compatible(&mut target, input)
        } else {
            general_path_used = true;
            let (offsets, dropped) = graft_header_general(&mut target, input);
            for section in &input.sections {
                let mut section = section.clone();
                for p in &mut section.paragraphs {
                    remap_paragraph_general(p, &offsets);
                }
                target.sections.push(section);
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

    target.header.properties.section_count = target.sections.len() as u16;

    Ok(MergeOutcome {
        document: target,
        general_path_used,
        losses,
    })
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
fn graft_bin_data(target: &mut Document, input: &Document) -> (u16, HashMap<u16, u16>, usize) {
    let offset = target.header.bin_data.len() as u16;
    let mut used: Vec<u16> = target
        .header
        .bin_data
        .iter()
        .filter_map(|b| b.storage_id)
        .collect();
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
            let ext = item.extension.as_deref().unwrap_or("");
            let new_id = if let Some(&mapped) = remap.get(&storage_id) {
                mapped
            } else if !used.contains(&storage_id) && !bin_stream_name_taken(target, storage_id, ext)
            {
                used.push(storage_id);
                storage_id
            } else {
                // Mint the first id that is neither grafted nor stream-named
                // already; exhausting the u16 space makes this entry (and
                // every later one) ungraftable.
                let mut candidate = used.iter().max().copied().unwrap_or(0);
                let fresh = loop {
                    match candidate.checked_add(1) {
                        Some(next)
                            if !used.contains(&next)
                                && !bin_stream_name_taken(target, next, ext) =>
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
    (offset, remap, dropped)
}

/// Whether a target bin stream already carries `BIN{storage_id:04X}.{ext}` as
/// its trailing name — the name `Document::resolve_bin` derives for a
/// bin_data entry with this storage id and extension.
fn bin_stream_name_taken(target: &Document, storage_id: u16, ext: &str) -> bool {
    let candidate = format!("BIN{storage_id:04X}.{ext}");
    target.bin_streams.iter().any(|b| {
        b.name
            .rsplit('/')
            .next()
            .unwrap_or(&b.name)
            .eq_ignore_ascii_case(&candidate)
    })
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

/// Grafts `input`'s bin streams onto `target`. A stream that is the payload
/// of an hwp5 BIN_DATA entry takes the entry's reminted canonical name so
/// `BinRef::Id` references keep resolving; every other stream keeps its name,
/// minting a `{prefix}{n}_` name on a collision. The returned map rewires
/// `BinRef::ItemRef` references. Shared by both grafting tiers.
fn graft_bin_streams(
    target: &mut Document,
    input: &Document,
    prefix: &str,
    storage_remap: &HashMap<u16, u16>,
) -> HashMap<String, String> {
    let mut rename: HashMap<String, String> = HashMap::new();
    for bin in &input.bin_streams {
        let mut name = bin.name.clone();
        if let Some(canonical) = reminted_canonical_name(&input.header.bin_data, bin, storage_remap)
        {
            name = canonical;
        } else if target.bin_streams.iter().any(|b| b.name == name) {
            let mut n = target.bin_streams.len() + 1;
            loop {
                let candidate = format!("{prefix}{n}_{}", bin.name);
                if !target.bin_streams.iter().any(|b| b.name == candidate) {
                    name = candidate;
                    break;
                }
                n += 1;
            }
        }
        rename.insert(bin.name.clone(), name.clone());
        target.bin_streams.push(BinStream {
            name,
            data: bin.data.clone(),
        });
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
/// as [`MergeLoss::BinDataDropped`].
fn graft_palette_compatible(target: &mut Document, input: &Document) -> usize {
    let cs_off = target.header.char_shapes.len() as u16 - PALETTE_LEN;
    let ps_off = target.header.para_shapes.len() as u16 - BASE_PARA_SHAPES;
    let num_off = target.header.numbering_levels.len() as u16;
    let bul_off = target.header.bullet_chars.len() as u16;

    let (bd_off, storage_remap, dropped) = graft_bin_data(target, input);
    let rename = graft_bin_streams(target, input, "merge", &storage_remap);

    // Extend target with the input's off-palette extras (numbering/bullet
    // references shifted by the offsets computed above).
    for ps in &input.header.para_shapes[BASE_PARA_SHAPES as usize..] {
        let mut ps = ps.clone();
        match ps.head_type() {
            2 => ps.numbering_id += num_off, // numbering definition reference
            3 => ps.numbering_id += bul_off, // bullet definition reference
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
            remap_paragraph(p, ps_off, cs_off, bd_off, &rename);
        }
        target.sections.push(section);
    }
    dropped
}

/// Shifts one paragraph's off-palette id references (para shape, char shape
/// runs), bin-data id references (`BinRef::Id`, shifted by the full bin-data
/// offset — bin data has no shared palette base) and rewires bin-stream
/// (Picture) references after a rename, including nested table cells,
/// Generic paragraph lists and table/picture captions. A `SectionDef`
/// control is untouched — it matches none of the arms below and stays as-is,
/// which is exactly D-02's "each input's Section carried over unchanged"
/// requirement. Mirrors `hwp_convert::merge::remap_paragraph`.
fn remap_paragraph(
    para: &mut Paragraph,
    ps_off: u16,
    cs_off: u16,
    bd_off: u16,
    rename: &HashMap<String, String>,
) {
    if para.para_shape.0 >= BASE_PARA_SHAPES {
        para.para_shape.0 += ps_off;
    }
    for (_, id) in &mut para.char_shape_runs {
        if id.0 >= PALETTE_LEN {
            id.0 += cs_off;
        }
    }
    for control in &mut para.controls {
        match control {
            Control::Table(t) => {
                if let Some(cap) = &mut t.caption {
                    for p in &mut cap.paragraphs {
                        remap_paragraph(p, ps_off, cs_off, bd_off, rename);
                    }
                }
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        remap_paragraph(p, ps_off, cs_off, bd_off, rename);
                    }
                }
            }
            Control::Generic(g) => {
                for list in &mut g.paragraph_lists {
                    for p in &mut list.paragraphs {
                        remap_paragraph(p, ps_off, cs_off, bd_off, rename);
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
                    BinRef::Id(id) => id.0 += bd_off,
                }
                if let Some(cap) = &mut pic.caption {
                    for p in &mut cap.paragraphs {
                        remap_paragraph(p, ps_off, cs_off, bd_off, rename);
                    }
                }
            }
            _ => {}
        }
    }
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
/// [`MergeLoss::BinDataDropped`].
fn graft_header_general(target: &mut Document, input: &Document) -> (HeaderOffsets, usize) {
    let char_shape = target.header.char_shapes.len() as u16;
    let para_shape = target.header.para_shapes.len() as u16;
    let style = target.header.styles.len() as u16;
    let border_fill = target.header.border_fills.len() as u16;
    let tab_def = target.header.tab_defs.len() as u16;
    let numbering = target.header.numbering_levels.len() as u16;
    let bullet = target.header.bullet_chars.len() as u16;
    let mut face = [0u16; LANG_COUNT];
    for (lang, offset) in face.iter_mut().enumerate() {
        *offset = target.header.fonts[lang].len() as u16;
    }

    let (bin_data, storage_remap, dropped) = graft_bin_data(target, input);
    let bin_rename = graft_bin_streams(target, input, "graft", &storage_remap);

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
    target
        .header
        .numberings
        .extend(input.header.numberings.iter().cloned());
    target
        .header
        .numbering_levels
        .extend(input.header.numbering_levels.iter().cloned());
    target
        .header
        .bullets
        .extend(input.header.bullets.iter().cloned());
    target
        .header
        .bullet_chars
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
            *face_id += offset;
        }
        if cs.border_fill_id != 0 {
            cs.border_fill_id += border_fill;
        }
        target.header.char_shapes.push(cs);
    }

    // ParaShapes: shift the tab-def id, the numbering/bullet reference
    // (head-type-dependent, same two-arm match `part_paragraphs` performs),
    // and the border-fill id with the zero-sentinel guard.
    for ps in &input.header.para_shapes {
        let mut ps = ps.clone();
        ps.tab_def_id += tab_def;
        match ps.head_type() {
            2 => ps.numbering_id += numbering,
            3 => ps.numbering_id += bullet,
            _ => {}
        }
        if ps.border_fill_id != 0 {
            ps.border_fill_id += border_fill;
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

    (
        HeaderOffsets {
            char_shape,
            para_shape,
            style,
            border_fill,
            bin_data,
            bin_rename,
        },
        dropped,
    )
}

/// General-graft counterpart of `remap_paragraph`: shifts every id
/// unconditionally (no shared-base guard — there is no assumed shared base on
/// this path), adds `para.style`, the `Table`/`Cell` border-fill shift (with
/// the zero-sentinel guard) and the hwp5 `BinRef::Id` bin-data shift.
/// Recurses into every nested paragraph collection — table cells, Generic
/// paragraph lists, and table/picture captions — so caption paragraphs
/// resolve to the same shifted header entries as body paragraphs and caption
/// pictures keep their rewired bin references.
fn remap_paragraph_general(para: &mut Paragraph, offsets: &HeaderOffsets) {
    para.para_shape.0 += offsets.para_shape;
    para.style.0 += offsets.style;
    for (_, id) in &mut para.char_shape_runs {
        id.0 += offsets.char_shape;
    }
    for control in &mut para.controls {
        match control {
            Control::Table(t) => {
                if t.border_fill.0 != 0 {
                    t.border_fill.0 += offsets.border_fill;
                }
                if let Some(cap) = &mut t.caption {
                    for p in &mut cap.paragraphs {
                        remap_paragraph_general(p, offsets);
                    }
                }
                for cell in &mut t.cells {
                    if cell.border_fill.0 != 0 {
                        cell.border_fill.0 += offsets.border_fill;
                    }
                    for p in &mut cell.paragraphs {
                        remap_paragraph_general(p, offsets);
                    }
                }
            }
            Control::Generic(g) => {
                for list in &mut g.paragraph_lists {
                    for p in &mut list.paragraphs {
                        remap_paragraph_general(p, offsets);
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
                    BinRef::Id(id) => id.0 += offsets.bin_data,
                }
                if let Some(cap) = &mut pic.caption {
                    for p in &mut cap.paragraphs {
                        remap_paragraph_general(p, offsets);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Gives every GSO-bearing (`Table`/`Picture`) control in `sections` a fresh
/// object identity so no object from a later-merged input can share a
/// Hancom-visible identity with one from an earlier input. Mirrors
/// `hwp_convert::edit::remap_clone_object_ids`/`remap_clone_object_ids_in_para`
/// (same two-branch shape: write the id into `common_data` bytes 32..36 when
/// present, otherwise bump `placement.z_order`), but threads `next_id`/`next_z`
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
    if !table.common_data.is_empty() {
        let id = *next_id;
        *next_id = next_id
            .checked_add(1)
            .ok_or_else(|| "개체 id 오버플로".to_string())?;
        if table.common_data.len() >= 36 {
            table.common_data[32..36].copy_from_slice(&id.to_le_bytes());
        }
        *count += 1;
    } else if let Some(pl) = &mut table.placement {
        pl.z_order = *next_z;
        *next_z = next_z
            .checked_add(1)
            .ok_or_else(|| "개체 z-order 오버플로".to_string())?;
        *count += 1;
    }
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
}
