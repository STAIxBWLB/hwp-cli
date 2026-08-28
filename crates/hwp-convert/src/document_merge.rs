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
//! Hancom-saved file), every DocHeader-referencing id it carries (char shape,
//! para shape, style, border fill, per-language face, tab def, numbering and
//! bullet ids) is shifted by that collection's offset with no shared-base
//! assumption. A `border_fill_id`/`border_fill` of 0 is the "unspecified"
//! sentinel (not an index) and is never shifted, so grafting never invents a
//! border where none existed.
//!
//! GSO object identity renumbering and the D-14 typed loss surface are plan
//! 03-02's later tasks.

use std::collections::HashMap;

use hwp_model::{BinRef, BinStream, Control, Document, LANG_COUNT, Paragraph};

use crate::from_html::PALETTE_LEN;
use crate::from_markdown::BASE_PARA_SHAPES;
use crate::merge::palette_compatible;

/// A typed, content-free record of something [`merge_documents`] could not
/// carry losslessly. No variants yet — later tasks in this plan fill this in
/// alongside the `--loss-report` ledger wiring (D-14: additive only, once
/// published a variant is never withdrawn).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeLoss {}

/// Result of [`merge_documents`].
#[derive(Debug)]
pub struct MergeOutcome {
    /// The merged document: one Section per input, concatenated in argument order.
    pub document: Document,
    /// Whether any non-primary input required the general (non-palette-fast-path) graft.
    pub general_path_used: bool,
    /// Typed preservation losses recorded while merging. Always empty until a
    /// later task in this plan.
    pub losses: Vec<MergeLoss>,
}

/// Merges `inputs` into one document.
///
/// `inputs[0]`'s header, metadata and singular package-passthrough fields
/// (`hwpx_settings_xml`, `hwp5_xml_template`, `hwp5_doc_history`,
/// `hwpx_extra_entries`, `hwpx_bin_manifest`, `hwpx_opf_extra_items`,
/// `hwpx_preview_image`, ...) become the base unchanged — no later input's
/// singular fields are carried (a later task in this plan records those as
/// typed losses). `hwpx_section_xmlns` is the one documented cross-section
/// union field and is unioned across every input. Each later input's Sections
/// are appended in argument order, after its header is grafted onto the
/// running target: the palette-compatible fast path (tier 1) when the input's
/// header is a hwp-cli-default-palette-family member, otherwise the general
/// graft (tier 2) that shifts every DocHeader-referencing id unconditionally.
///
/// Returns `Err` only when `inputs` is empty.
pub fn merge_documents(inputs: &[Document]) -> Result<MergeOutcome, String> {
    let (first, rest) = inputs
        .split_first()
        .ok_or_else(|| "병합할 입력이 없습니다".to_string())?;

    let mut target = first.clone();
    let mut general_path_used = false;

    for input in rest {
        if palette_compatible(&target.header, &input.header) {
            graft_palette_compatible(&mut target, input);
        } else {
            general_path_used = true;
            let offsets = graft_header_general(&mut target, input);
            for section in &input.sections {
                let mut section = section.clone();
                for p in &mut section.paragraphs {
                    remap_paragraph_general(p, &offsets);
                }
                target.sections.push(section);
            }
        }
    }

    target.header.properties.section_count = target.sections.len() as u16;

    Ok(MergeOutcome {
        document: target,
        general_path_used,
        losses: Vec::new(),
    })
}

/// Grafts `input`'s bin streams onto `target`, minting a `{prefix}{n}_` name on
/// a collision and rewiring the returned map so callers can rewrite `Picture`
/// references. Shared by both grafting tiers.
fn graft_bin_streams(
    target: &mut Document,
    input: &Document,
    prefix: &str,
) -> HashMap<String, String> {
    let mut rename: HashMap<String, String> = HashMap::new();
    for bin in &input.bin_streams {
        let mut name = bin.name.clone();
        if target.bin_streams.iter().any(|b| b.name == name) {
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
/// Mirrors `hwp_convert::merge::part_paragraphs`'s offset arithmetic.
fn graft_palette_compatible(target: &mut Document, input: &Document) {
    let cs_off = target.header.char_shapes.len() as u16 - PALETTE_LEN;
    let ps_off = target.header.para_shapes.len() as u16 - BASE_PARA_SHAPES;
    let num_off = target.header.numbering_levels.len() as u16;
    let bul_off = target.header.bullet_chars.len() as u16;

    let rename = graft_bin_streams(target, input, "merge");

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

/// Per-non-primary-input collection-length offsets [`graft_header_general`]
/// computes before it appends `input`'s header collections onto `target`'s,
/// and [`remap_paragraph_general`] then applies to that input's paragraph
/// tree. The tab-def/numbering/bullet/face offsets are consumed entirely
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
fn graft_header_general(target: &mut Document, input: &Document) -> HeaderOffsets {
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

    let bin_rename = graft_bin_streams(target, input, "graft");

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

    HeaderOffsets {
        char_shape,
        para_shape,
        style,
        border_fill,
        bin_rename,
    }
}

/// General-graft counterpart of `remap_paragraph`: shifts every id
/// unconditionally (no shared-base guard — there is no assumed shared base on
/// this path), adds `para.style` and the `Table`/`Cell` border-fill shift (with
/// the zero-sentinel guard). Recurses into table cells and Generic paragraph
/// lists exactly as `remap_paragraph` does — not into captions, matching that
/// function's own scope.
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
                if let BinRef::ItemRef(name) = &mut pic.bin_ref
                    && let Some(new_name) = offsets.bin_rename.get(name)
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
}
