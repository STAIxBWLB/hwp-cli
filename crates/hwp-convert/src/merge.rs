//! Part document merge — the IR grafting layer of template+part filling (`hwp fill --set name=@part.md`).
//!
//! Maru part authoring workflow: prose is written in markdown, tables/images as HTML fragments
//! (contract docs/design/18), and the `{{name}}` anchor paragraphs of a template document are
//! replaced with the part's blocks to compose large documents (project reports, etc.).
//!
//! **Palette compatibility constraint (v1)**: both the template and the part must be hwp-cli
//! generated documents (default_header family — presets included). The part's palette ids
//! (char shapes 0~15, para shapes 0~4, styles, borders) point at the same template slots as-is,
//! so the part inherits the template's typography. Only off-palette extras (additional char/para
//! shapes, numbering/bullet definitions, bins) are grafted with offset shifts. General merging
//! with arbitrary documents made by external tools like Hancom (GM-3) is out of scope.

use std::collections::HashMap;

use hwp_model::{BinStream, Control, DocHeader, Document, Paragraph};

use crate::from_html::PALETTE_LEN;
use crate::from_markdown::{self, BASE_PARA_SHAPES};

/// Whether two headers are compatible as hwp-cli default-palette family members — the condition
/// for using the part's palette ids in the template as-is. The hwpx save/read round-trip
/// normalizes incidental fields (shadow_gap, attr1, lang_id, tab_defs, font attr/default_name),
/// so this checks the **structural signature** (style/font names + collection sizes), not value
/// equality. Presets (size/font variants) are also accepted as the same family.
pub fn palette_compatible(a: &DocHeader, b: &DocHeader) -> bool {
    default_family(a) && default_family(b)
}

/// default_header family structural signature — the condition for palette ids to mean the same as in default_header.
fn default_family(h: &DocHeader) -> bool {
    let d = from_markdown::default_header();
    h.char_shapes.len() >= d.char_shapes.len()
        && h.para_shapes.len() >= d.para_shapes.len()
        && h.border_fills.len() == d.border_fills.len()
        && h.tab_stops.len() == d.tab_stops.len()
        && h.styles.len() == d.styles.len()
        && h.styles
            .iter()
            .zip(&d.styles)
            .all(|(x, y)| x.name == y.name)
        && h.fonts.len() == d.fonts.len()
        && h.fonts.iter().zip(&d.fonts).all(|(fa, fd)| {
            fa.len() == fd.len() && fa.iter().zip(fd).all(|(x, y)| x.name == y.name)
        })
}

/// Remaps the part document's body paragraphs into a form graftable into target and returns them.
/// Extends target's header collections (additional char/para shapes, numbering/bullet
/// definitions, bin streams) with the part's extras, and shifts the part paragraphs' reference
/// ids by those offsets.
///
/// Returns an error if a part paragraph has a section definition (SectionDef) — parts must be
/// made with `from_markdown_blocks` (no section injection).
pub fn part_paragraphs(target: &mut Document, part: &Document) -> Result<Vec<Paragraph>, String> {
    if !palette_compatible(&target.header, &part.header) {
        return Err(
            "부분 채우기는 hwp-cli가 생성한 문서(기본 팔레트 계열)끼리만 지원됩니다 \
             — 템플릿/부분의 헤더 구성이 다릅니다"
                .into(),
        );
    }
    for section in &part.sections {
        for p in &section.paragraphs {
            if p.controls
                .iter()
                .any(|c| matches!(c, Control::SectionDef(_)))
            {
                return Err(
                    "part 문단에 구역 정의가 있습니다 — 부분은 from_markdown_blocks로 만듭니다"
                        .into(),
                );
            }
        }
    }

    let cs_off = target.header.char_shapes.len() as u16 - PALETTE_LEN;
    let ps_off = target.header.para_shapes.len() as u16 - BASE_PARA_SHAPES;
    let num_off = target.header.numbering_levels.len() as u16;
    let bul_off = target.header.bullet_chars.len() as u16;

    // Bin streams — on a name collision, mint a new name and rewire the Picture references.
    let mut rename: HashMap<String, String> = HashMap::new();
    for bin in &part.bin_streams {
        let mut name = bin.name.clone();
        if target.bin_streams.iter().any(|b| b.name == name) {
            let mut n = target.bin_streams.len() + 1;
            loop {
                let candidate = format!("part{n}_{}", bin.name);
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

    // Extend target with the part's off-palette extras (numbering/bullet references after applying offsets).
    for ps in &part.header.para_shapes[BASE_PARA_SHAPES as usize..] {
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
        .extend(part.header.numbering_levels.iter().cloned());
    target
        .header
        .bullet_chars
        .extend(part.header.bullet_chars.iter().copied());
    target.header.char_shapes.extend(
        part.header.char_shapes[PALETTE_LEN as usize..]
            .iter()
            .cloned(),
    );

    let mut out: Vec<Paragraph> = part
        .sections
        .iter()
        .flat_map(|s| s.paragraphs.iter().cloned())
        .collect();
    for p in &mut out {
        remap_paragraph(p, ps_off, cs_off, &rename);
    }
    Ok(out)
}

/// Shifts one paragraph's id references (including nested table cells, Generic paragraph lists, and picture bin references).
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
                // ID 재매핑으로 개체 안 문단의 모양 참조가 바뀐다 — 원문 XML은 stale.
                g.hwpx_raw_xml = None;
            }
            Control::Picture(pic) => {
                if let hwp_model::BinRef::ItemRef(name) = &mut pic.bin_ref
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
    use crate::from_markdown::{MarkdownImportOptions, from_markdown, from_markdown_blocks};

    fn para_text(p: &Paragraph) -> String {
        p.chars
            .iter()
            .filter_map(|c| match c {
                hwp_model::HwpChar::Text(c) => Some(*c),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn 부분_문단_이식_기본() {
        let mut target = from_markdown("# 보고서\n\n{{본문}}\n");
        let part = from_markdown_blocks("부분 본문입니다.\n", &MarkdownImportOptions::default());
        let blocks = part_paragraphs(&mut target, &part).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(para_text(&blocks[0]), "부분 본문입니다.");
    }

    #[test]
    fn 목록_번호정의_오프셋() {
        // If the template already has a numbered list, the part's numbering definition is appended after it.
        let mut target = from_markdown("1. 템플릿 항목\n\n{{본문}}\n");
        let before = target.header.numbering_levels.len();
        let part = from_markdown_blocks("1. 부분 항목\n", &MarkdownImportOptions::default());
        let blocks = part_paragraphs(&mut target, &part).unwrap();
        assert_eq!(target.header.numbering_levels.len(), before + 1);
        // The part item paragraph's para shape must point at the shifted index.
        let item = blocks
            .iter()
            .find(|p| para_text(p).contains("부분 항목"))
            .expect("부분 항목 문단");
        assert!(item.para_shape.0 >= BASE_PARA_SHAPES);
        let shape = &target.header.para_shapes[item.para_shape.0 as usize];
        assert_eq!(shape.numbering_id as usize, before);
    }

    #[test]
    fn 구역정의_포함_part는_에러() {
        let mut target = from_markdown("{{본문}}\n");
        let part = from_markdown("부분\n"); // from_markdown injects the section
        assert!(part_paragraphs(&mut target, &part).is_err());
    }

    #[test]
    fn 팔레트_불일치는_에러() {
        let mut target = from_markdown("{{본문}}\n");
        target.header.styles.clear(); // mimics an arbitrary document — style signature mismatch
        let part = from_markdown_blocks("부분\n", &MarkdownImportOptions::default());
        assert!(part_paragraphs(&mut target, &part).is_err());
    }

    #[test]
    fn html_표_포함_부분() {
        let mut target = from_markdown("{{표부분}}\n");
        let part = from_markdown_blocks(
            "<table><tr><td colspan=\"2\">가로</td></tr><tr><td>a</td><td>b</td></tr></table>\n",
            &MarkdownImportOptions::default(),
        );
        let blocks = part_paragraphs(&mut target, &part).unwrap();
        let table = blocks
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("표 컨트롤");
        assert_eq!(table.cells[0].col_span, 2);
    }
}
