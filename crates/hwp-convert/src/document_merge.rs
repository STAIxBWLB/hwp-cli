//! Document-level merge — the `hwp merge` IR transform (GM-3, D-01/D-02).
//!
//! **Tier 1 (this task): palette-compatible fast path only.** When a later input's
//! header is structurally compatible with the running target
//! (`hwp_convert::merge::palette_compatible`), its off-palette header extras are
//! grafted with the same offset arithmetic `hwp_convert::merge::part_paragraphs`
//! uses for part filling, and its whole `Section`s (each carrying its own
//! `SectionDef` — paper size, margins, headers/footers) are appended unchanged
//! (D-02: one Section per input, concatenated in argument order — sections never
//! fuse, even when two inputs are byte-equal). `part_paragraphs` itself is not
//! called: it rejects any paragraph carrying a `Control::SectionDef`, which every
//! section's first paragraph does; `merge.rs` stays untouched (D-01).
//!
//! **Tier 2** (the general graft path for headers outside the default-palette
//! family) is plan 03-02's scope. Until then, a palette mismatch between the
//! running target and a later input is a named refusal, not a silent drop.

use std::collections::HashMap;

use hwp_model::{BinRef, BinStream, Control, Document, Paragraph};

use crate::from_html::PALETTE_LEN;
use crate::from_markdown::BASE_PARA_SHAPES;
use crate::merge::palette_compatible;

/// A typed, content-free record of something [`merge_documents`] could not
/// carry losslessly. No variants yet — plan 03-02 fills this in alongside the
/// general graft path and the `--loss-report` ledger wiring (D-14: additive
/// only, once published a variant is never withdrawn).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeLoss {}

/// Result of [`merge_documents`].
#[derive(Debug)]
pub struct MergeOutcome {
    /// The merged document: one Section per input, concatenated in argument order.
    pub document: Document,
    /// Whether any input pair required the general (non-palette-fast-path)
    /// graft. Always `false` until plan 03-02 implements that path.
    pub general_path_used: bool,
    /// Typed preservation losses recorded while merging. Always empty until
    /// plan 03-02.
    pub losses: Vec<MergeLoss>,
}

/// Merges `inputs` into one document.
///
/// `inputs[0]`'s header, metadata and singular package-passthrough fields
/// (`hwpx_settings_xml`, `hwp5_xml_template`, `hwp5_doc_history`,
/// `hwpx_extra_entries`, `hwpx_bin_manifest`, `hwpx_opf_extra_items`,
/// `hwpx_preview_image`, ...) become the base unchanged — no later input's
/// singular fields are carried (03-02 records those as typed losses).
/// `hwpx_section_xmlns` is the one documented cross-section union field and is
/// unioned across every input. Each later input's Sections are appended
/// unchanged, in argument order, after its off-palette header extras
/// (additional char/para shapes, numbering/bullet definitions, bin streams)
/// are grafted onto the running target with offset shifts.
///
/// Returns `Err` when a later input's header is not palette-compatible with
/// the running target — the general graft path is not implemented yet
/// (plan 03-02) — or when `inputs` is empty.
pub fn merge_documents(inputs: &[Document]) -> Result<MergeOutcome, String> {
    let (first, rest) = inputs
        .split_first()
        .ok_or_else(|| "병합할 입력이 없습니다".to_string())?;

    let mut target = first.clone();
    for input in rest {
        if !palette_compatible(&target.header, &input.header) {
            return Err(
                "이 입력은 hwp-cli 기본 팔레트 계열이 아니어서 일반 병합 경로가 필요합니다 \
                 — 아직 구현되지 않았습니다"
                    .to_string(),
            );
        }
        graft_palette_compatible(&mut target, input);
    }

    target.header.properties.section_count = target.sections.len() as u16;

    Ok(MergeOutcome {
        document: target,
        general_path_used: false,
        losses: Vec::new(),
    })
}

/// Grafts `input`'s off-palette header extras onto `target` with offset
/// shifts, then appends `input`'s Sections (remapped, otherwise unchanged).
/// Mirrors `hwp_convert::merge::part_paragraphs`'s offset arithmetic.
fn graft_palette_compatible(target: &mut Document, input: &Document) {
    let cs_off = target.header.char_shapes.len() as u16 - PALETTE_LEN;
    let ps_off = target.header.para_shapes.len() as u16 - BASE_PARA_SHAPES;
    let num_off = target.header.numbering_levels.len() as u16;
    let bul_off = target.header.bullet_chars.len() as u16;

    // Bin streams — on a name collision, mint a new name and rewire Picture references.
    let mut rename: HashMap<String, String> = HashMap::new();
    for bin in &input.bin_streams {
        let mut name = bin.name.clone();
        if target.bin_streams.iter().any(|b| b.name == name) {
            let mut n = target.bin_streams.len() + 1;
            loop {
                let candidate = format!("merge{n}_{}", bin.name);
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

    // Extend target with the input's off-palette extras (numbering/bullet
    // references shifted by the offsets computed above).
    for ps in &input.header.para_shapes[BASE_PARA_SHAPES as usize..] {
        let mut ps = ps.clone();
        match (ps.attr1 >> 23) & 0x3 {
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
            remap_paragraph(p, ps_off, cs_off, &rename);
        }
        target.sections.push(section);
    }
}

/// Shifts one paragraph's off-palette id references (para shape, char shape
/// runs) and rewires bin-stream (Picture) references after a rename,
/// including nested table cells and Generic paragraph lists. A `SectionDef`
/// control is untouched — it matches none of the arms below and stays as-is,
/// which is exactly D-02's "each input's Section carried over unchanged"
/// requirement. Mirrors `hwp_convert::merge::remap_paragraph`.
fn remap_paragraph(
    para: &mut Paragraph,
    ps_off: u16,
    cs_off: u16,
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
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        remap_paragraph(p, ps_off, cs_off, rename);
                    }
                }
            }
            Control::Generic(g) => {
                for list in &mut g.paragraph_lists {
                    for p in &mut list.paragraphs {
                        remap_paragraph(p, ps_off, cs_off, rename);
                    }
                }
                // Reference ids changed above — the original XML is stale.
                g.hwpx_raw_xml = None;
            }
            Control::Picture(pic) => {
                if let BinRef::ItemRef(name) = &mut pic.bin_ref
                    && let Some(new_name) = rename.get(name)
                {
                    *name = new_name.clone();
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::from_markdown::from_markdown;
    use hwp_model::HwpChar;

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
    fn 팔레트_불일치는_에러() {
        let mut a = from_markdown("문서 A\n");
        let b = from_markdown("문서 B\n");
        a.header.styles.clear(); // mimics an arbitrary non-hwp-cli document
        assert!(merge_documents(&[a, b]).is_err());
    }

    #[test]
    fn 빈_입력은_에러() {
        assert!(merge_documents(&[]).is_err());
    }
}
