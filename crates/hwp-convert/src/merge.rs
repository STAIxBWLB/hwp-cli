//! 부분(part) 문서 병합 — 템플릿+부분 채우기(`hwp fill --set name=@part.md`)의 IR 이식 계층.
//!
//! Maru 부분(part) 작성 워크플로: 본문 산문은 markdown, 표·그림은 HTML fragment
//! (계약 docs/design/18)로 부분을 작성하고, 템플릿 문서의 `{{name}}` 앵커 문단을 부분의
//! 블록으로 교체해 대규모 문서(사업보고서 등)를 조합한다.
//!
//! **팔레트 호환 제약(v1)**: 템플릿과 부분 모두 hwp-cli 생성 문서(default_header 계열 —
//! 프리셋 포함)여야 한다. 부분의 팔레트 id(문자모양 0~15·문단모양 0~4·스타일·테두리)는
//! 템플릿의 같은 번호를 그대로 가리키므로, 부분은 템플릿의 타이포그래피를 상속한다.
//! 팔레트 초과분(추가 문자/문단 모양·번호/글머리 정의·bin)만 오프셋 시프트로 이식한다.
//! 한글 등 외부 도구가 만든 임의 문서와의 일반 병합(GM-3)은 범위 밖이다.

use std::collections::HashMap;

use hwp_model::{BinStream, Control, DocHeader, Document, Paragraph};

use crate::from_html::PALETTE_LEN;
use crate::from_markdown::{self, BASE_PARA_SHAPES};

/// 두 헤더가 hwp-cli 기본 팔레트 계열로 호환되는지 — 부분의 팔레트 id를 템플릿에
/// 그대로 쓸 수 있는 조건. hwpx 저장/읽기 왕복이 부수 필드(shadow_gap·attr1·lang_id·
/// tab_defs·글꼴 attr/default_name)를 정규화하므로 **값 동등이 아니라 구조 시그니처**
/// (스타일/글꼴 이름 + 컬렉션 크기)만 본다. 프리셋(크기·글꼴 변형)도 같은 계열로 인정.
pub fn palette_compatible(a: &DocHeader, b: &DocHeader) -> bool {
    default_family(a) && default_family(b)
}

/// default_header 계열 구조 시그니처 — 팔레트 id의 의미가 default_header와 정렬되는 조건.
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

/// part 문서의 본문 문단들을 target에 이식 가능한 형태로 리맵해 반환한다.
/// target의 헤더 컬렉션(추가 문자/문단 모양·번호/글머리 정의·bin 스트림)을 part의
/// 초과분으로 연장하고, part 문단의 참조 id를 그 오프셋으로 시프트한다.
///
/// part 문단에 구역 정의(SectionDef)가 있으면 에러 — 부분은 반드시
/// `from_markdown_blocks`(구역 주입 없음)로 만든다.
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

    // bin 스트림 — 이름 충돌 시 새 이름을 만들고 Picture 참조를 갈아끼운다.
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

    // part의 팔레트 초과분을 target에 연장 (번호/글머리 참조는 오프셋 적용 후).
    for ps in &part.header.para_shapes[BASE_PARA_SHAPES as usize..] {
        let mut ps = ps.clone();
        match (ps.attr1 >> 23) & 0x3 {
            2 => ps.numbering_id += num_off, // 번호 정의 참조
            3 => ps.numbering_id += bul_off, // 글머리 정의 참조
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

/// 문단 하나의 id 참조를 시프트한다 (중첩 표 셀·Generic 문단 리스트·그림 bin 참조 포함).
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
        // 템플릿에 이미 번호 목록이 있으면 부분의 번호 정의는 뒤에 붙는다.
        let mut target = from_markdown("1. 템플릿 항목\n\n{{본문}}\n");
        let before = target.header.numbering_levels.len();
        let part = from_markdown_blocks("1. 부분 항목\n", &MarkdownImportOptions::default());
        let blocks = part_paragraphs(&mut target, &part).unwrap();
        assert_eq!(target.header.numbering_levels.len(), before + 1);
        // 부분 항목 문단의 문단모양이 시프트된 인덱스를 가리켜야 한다.
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
        let part = from_markdown("부분\n"); // from_markdown은 구역 주입
        assert!(part_paragraphs(&mut target, &part).is_err());
    }

    #[test]
    fn 팔레트_불일치는_에러() {
        let mut target = from_markdown("{{본문}}\n");
        target.header.styles.clear(); // 임의 문서 흉내 — 스타일 시그니처 불일치
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
