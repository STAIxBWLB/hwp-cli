//! 구조 편집 — 문단 삽입/삭제.
//!
//! 새 문단/셀은 `set_cell`과 동일한 최소 IR(문단끝 0x0d는 writer가 idempotent하게
//! 보장, line_segs 비움)로 만들고, 앵커/템플릿의 글자·문단 모양을 상속한다.
//! 구조 편집본은 **합성 경로**(convert/new와 동일, 한글 수용 검증됨)로 써야 삽입
//! 문단/행에 모든 불변식(0x0d·마지막문단 비트·카운트)이 적용된다.
//!
//! 표 행/열 연산은 `crate::edit`(재귀 표 로케이터 계열 — set-cell과 인덱스 일치)으로
//! 단일화됐다. 여기서는 문단 수준 편집만 둔다.

use hwp_model::{
    CharShapeId, Control, Document, HwpChar, ParaShapeId, Paragraph, StyleId, ctrl_char,
};

use crate::edit::find_match;

/// 텍스트로 최소 문단을 만든다(글자/문단 모양 상속). 빈 텍스트면 빈 문단.
fn make_paragraph(
    text: &str,
    para_shape: ParaShapeId,
    style: StyleId,
    char_shape: CharShapeId,
) -> Paragraph {
    let mut chars: Vec<HwpChar> = text
        .chars()
        .map(|c| {
            if c == '\n' {
                HwpChar::CharCtrl(ctrl_char::LINE_BREAK)
            } else {
                HwpChar::Text(c)
            }
        })
        .collect();
    if !chars.is_empty() {
        chars.push(HwpChar::CharCtrl(ctrl_char::PARA_BREAK));
    }
    Paragraph {
        para_shape,
        style,
        chars,
        char_shape_runs: vec![(0, char_shape)],
        line_segs: Vec::new(),
        ..Paragraph::default()
    }
}

/// 문단의 (para_shape, style, 첫 char_shape) 템플릿.
fn para_template(p: &Paragraph) -> (ParaShapeId, StyleId, CharShapeId) {
    (
        p.para_shape,
        p.style,
        p.char_shape_runs.first().map_or(CharShapeId(0), |r| r.1),
    )
}

/// 문서의 모든 문단 리스트에 `f`를 적용한다 — 주어진 리스트를 **먼저** 부르고,
/// 그다음 그 리스트의 각 문단이 품은 중첩 리스트(표 캡션·표 셀·그림 캡션·개체
/// 문단 리스트·개체 캡션)로 재귀한다. 구역정의 컨트롤은 건너뛴다.
///
/// 리스트 순서가 곧 호환성 보장이다: 섹션 자기 문단이 어떤 중첩 리스트보다 먼저
/// 방문되므로, 이 변경 전에 최상위 문단을 맞히던 호출은 그대로 같은 문단을 맞힌다.
///
/// `f`가 참을 돌려주면 그 리스트가 바뀐 것으로 본다. 절대 조기 종료하지 않고
/// (결과는 OR로 합친다) 모든 레벨을 방문한다 — 조기 종료 여부는 `f`가 스스로
/// 상태를 들고 판단한다. 원문 XML(`hwpx_raw_xml`)을 품은 개체 안이 바뀌면 그
/// 원문은 낡으므로 지운다 — writer가 stale XML을 방출하는 것을 막는다.
///
/// **hwp5 원본 레코드를 품은 개체(`raw_children` 비어있지 않음)는 들어가지 않는다.**
/// hwp5 writer(`emit_control`)는 그런 개체를 `raw_children` 바이트 그대로 다시 쓰고
/// `paragraph_lists`는 보지 않으므로, 안쪽 IR을 고쳐도 저장 파일은 그대로다. IR에서
/// 재합성하는 길도 없다 — gso 재합성은 정품과 어긋나 손상을 부르는 것으로 판명돼
/// (규칙 E6) 글상자는 `degrade_hwpx_gso`가 본문으로 내리는 안전 강등만 지원한다.
/// 따라서 조용한 성공 대신 아예 방문하지 않는다. 그 안의 앵커는
/// [`text_in_unwritable_object`]가 따로 알려 준다.
pub(crate) fn walk_para_lists(
    paras: &mut Vec<Paragraph>,
    f: &mut dyn FnMut(&mut Vec<Paragraph>) -> bool,
) -> bool {
    let mut hit = f(paras);
    for para in paras.iter_mut() {
        for ctrl in &mut para.controls {
            match ctrl {
                Control::Table(t) => {
                    if let Some(cap) = &mut t.caption {
                        hit |= walk_para_lists(&mut cap.paragraphs, f);
                    }
                    for cell in &mut t.cells {
                        hit |= walk_para_lists(&mut cell.paragraphs, f);
                    }
                }
                Control::Picture(pic) => {
                    if let Some(cap) = &mut pic.caption {
                        hit |= walk_para_lists(&mut cap.paragraphs, f);
                    }
                }
                // 저장 시 원본 서브트리를 그대로 쓰는 개체 — 안쪽 편집은 버려진다.
                Control::Generic(g) if !g.raw_children.is_empty() => {}
                Control::Generic(g) => {
                    let mut inner = false;
                    for list in &mut g.paragraph_lists {
                        inner |= walk_para_lists(&mut list.paragraphs, f);
                    }
                    if let Some(cap) = &mut g.caption {
                        inner |= walk_para_lists(&mut cap.paragraphs, f);
                    }
                    if inner {
                        g.hwpx_raw_xml = None;
                    }
                    hit |= inner;
                }
                Control::SectionDef(_) => {}
            }
        }
    }
    hit
}

/// [`walk_para_lists`]가 건너뛰는 개체(hwp5 원본 레코드를 품은 글상자·머리말 등)
/// 안에 `text`가 있는지 본다. 문단 삽입·삭제가 아무 것도 못 했을 때 "앵커가 없다"와
/// "앵커가 손댈 수 없는 개체 안에 있다"를 구별해 알리기 위한 진단용이다.
pub fn text_in_unwritable_object(doc: &Document, text: &str) -> bool {
    fn in_list(paras: &[Paragraph], text: &str, inside_raw: bool) -> bool {
        if inside_raw
            && paras
                .iter()
                .any(|p| find_match(&p.chars, text, 0).is_some())
        {
            return true;
        }
        paras.iter().any(|p| {
            p.controls.iter().any(|ctrl| match ctrl {
                Control::Table(t) => {
                    t.caption
                        .as_ref()
                        .is_some_and(|c| in_list(&c.paragraphs, text, inside_raw))
                        || t.cells
                            .iter()
                            .any(|c| in_list(&c.paragraphs, text, inside_raw))
                }
                Control::Picture(pic) => pic
                    .caption
                    .as_ref()
                    .is_some_and(|c| in_list(&c.paragraphs, text, inside_raw)),
                Control::Generic(g) => {
                    let raw = inside_raw || !g.raw_children.is_empty();
                    g.paragraph_lists
                        .iter()
                        .any(|l| in_list(&l.paragraphs, text, raw))
                        || g.caption
                            .as_ref()
                            .is_some_and(|c| in_list(&c.paragraphs, text, raw))
                }
                Control::SectionDef(_) => false,
            })
        })
    }
    doc.sections
        .iter()
        .any(|s| in_list(&s.paragraphs, text, false))
}

/// `anchor`를 가진 첫 문단 뒤(또는 앞)에 `text` 문단을 삽입한다. 반환=삽입 여부.
/// 새 문단은 앵커 문단의 글자/문단 모양을 상속한다. 앵커는 본문뿐 아니라 표 셀·
/// 중첩 표·캡션 안에서도 찾는다([`walk_para_lists`]의 방문 순서대로 첫 리스트 하나만
/// 바뀐다).
pub fn insert_paragraph(doc: &mut Document, anchor: &str, text: &str, before: bool) -> bool {
    let mut done = false;
    for section in &mut doc.sections {
        let mut visit = |list: &mut Vec<Paragraph>| -> bool {
            if done {
                return false;
            }
            let Some(i) = list
                .iter()
                .position(|p| find_match(&p.chars, anchor, 0).is_some())
            else {
                return false;
            };
            let (ps, sty, cs) = para_template(&list[i]);
            let new = make_paragraph(text, ps, sty, cs);
            let at = if before { i } else { i + 1 };
            list.insert(at, new);
            crate::edit::fixup_last_para_flag(list);
            done = true;
            true
        };
        walk_para_lists(&mut section.paragraphs, &mut visit);
        if done {
            break;
        }
    }
    done
}

/// `matching`을 가진 문단을 삭제한다 — 본문뿐 아니라 표 셀·중첩 표·캡션 안에서도
/// 지운다. 리스트마다 최소 1문단은 남기고(섹션·셀·캡션이 비면 한글이 손상으로
/// 판정한다), 구역정의 문단은 보존한다. 반환=삭제 개수.
pub fn delete_paragraph(doc: &mut Document, matching: &str) -> usize {
    let mut count = 0usize;
    for section in &mut doc.sections {
        let mut visit = |list: &mut Vec<Paragraph>| -> bool {
            let mut removed = 0usize;
            let mut i = 0;
            while i < list.len() {
                let p = &list[i];
                let is_secd = p
                    .controls
                    .iter()
                    .any(|c| matches!(c, Control::SectionDef(_)));
                if !is_secd && list.len() > 1 && find_match(&p.chars, matching, 0).is_some() {
                    list.remove(i);
                    removed += 1;
                } else {
                    i += 1;
                }
            }
            if removed == 0 {
                return false;
            }
            crate::edit::fixup_last_para_flag(list);
            count += removed;
            true
        };
        walk_para_lists(&mut section.paragraphs, &mut visit);
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::from_markdown;

    #[test]
    fn 문단_삽입_삭제() {
        let mut doc = from_markdown("첫째 문단\n\n둘째 문단\n\n셋째 문단");
        let n0: usize = doc.sections.iter().map(|s| s.paragraphs.len()).sum();
        // 둘째 뒤에 삽입.
        assert!(insert_paragraph(&mut doc, "둘째", "삽입된 문단", false));
        let n1: usize = doc.sections.iter().map(|s| s.paragraphs.len()).sum();
        assert_eq!(n1, n0 + 1);
        let txt = doc.plain_text();
        assert!(txt.contains("삽입된 문단"));
        // 삽입 위치: "둘째"와 "셋째" 사이.
        let i2 = txt.find("둘째").unwrap();
        let ii = txt.find("삽입된").unwrap();
        let i3 = txt.find("셋째").unwrap();
        assert!(i2 < ii && ii < i3, "둘째 뒤·셋째 앞: {txt:?}");
        // 삭제.
        let d = delete_paragraph(&mut doc, "삽입된 문단");
        assert_eq!(d, 1);
        assert!(!doc.plain_text().contains("삽입된 문단"));
    }

    #[test]
    fn 마지막_문단은_안지움() {
        let mut doc = from_markdown("유일 문단");
        // 본문 문단이 secd 1개뿐이면 보존(섹션 빔 방지).
        let before: usize = doc.sections.iter().map(|s| s.paragraphs.len()).sum();
        delete_paragraph(&mut doc, "유일");
        let after: usize = doc.sections.iter().map(|s| s.paragraphs.len()).sum();
        assert_eq!(before, after, "최소 1문단 유지");
    }

    // ── #220: 앵커를 표 셀·중첩 표·캡션 안에서도 찾는다 ──────────────────────

    fn first_table_mut(doc: &mut Document) -> &mut hwp_model::Table {
        doc.sections
            .iter_mut()
            .flat_map(|s| &mut s.paragraphs)
            .flat_map(|p| &mut p.controls)
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("표 없음")
    }

    fn cell_texts(doc: &Document, row: u16, col: u16) -> Vec<String> {
        doc.sections
            .iter()
            .flat_map(|s| &s.paragraphs)
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("표 없음")
            .cells
            .iter()
            .find(|c| c.row == row && c.col == col)
            .expect("셀 없음")
            .paragraphs
            .iter()
            .map(|p| p.plain_text())
            .collect()
    }

    #[test]
    fn 셀_안_앵커에_삽입된다() {
        let mut doc = from_markdown("본문\n\n| 가 | 나 |\n|----|----|\n| 표안앵커 | 2 |\n");
        assert!(insert_paragraph(&mut doc, "표안앵커", "셀 새 문단", false));
        let texts = cell_texts(&doc, 1, 0);
        assert_eq!(texts.len(), 2, "셀 문단 2개: {texts:?}");
        assert!(texts[1].contains("셀 새 문단"), "앵커 뒤 삽입: {texts:?}");
        // 삽입 뒤에도 리스트 마지막 문단만 bit31.
        let t = first_table_mut(&mut doc);
        let cell = t.cells.iter().find(|c| c.row == 1 && c.col == 0).unwrap();
        for (i, p) in cell.paragraphs.iter().enumerate() {
            let last = i + 1 == cell.paragraphs.len();
            assert_eq!(
                p.header.chars_flags & 0x80 != 0,
                last,
                "문단 {i} 마지막 비트"
            );
        }
    }

    #[test]
    fn 중첩_표_셀_안_앵커에_삽입된다() {
        let mut doc = from_markdown("본문\n\n| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let inner = {
            let mut d = from_markdown("| 중첩앵커 |\n|---|\n| x |\n");
            first_table_mut(&mut d).clone()
        };
        {
            let outer = first_table_mut(&mut doc);
            let cell = outer
                .cells
                .iter_mut()
                .find(|c| c.row == 1 && c.col == 1)
                .unwrap();
            cell.paragraphs[0].controls.push(Control::Table(inner));
        }
        assert!(insert_paragraph(
            &mut doc,
            "중첩앵커",
            "중첩 새 문단",
            false
        ));
        // plain_text는 중첩 표까지 훑지 않으므로 구조로 확인한다.
        let inner_cell = {
            let outer = first_table_mut(&mut doc);
            let host = outer
                .cells
                .iter()
                .find(|c| c.row == 1 && c.col == 1)
                .unwrap();
            host.paragraphs
                .iter()
                .flat_map(|p| &p.controls)
                .find_map(|c| match c {
                    Control::Table(t) => Some(t),
                    _ => None,
                })
                .expect("중첩 표 없음")
                .cells
                .iter()
                .find(|c| c.row == 0 && c.col == 0)
                .expect("중첩 셀 없음")
                .clone()
        };
        let texts: Vec<String> = inner_cell
            .paragraphs
            .iter()
            .map(|p| p.plain_text())
            .collect();
        assert_eq!(texts.len(), 2, "중첩 셀 문단 2개: {texts:?}");
        assert!(texts[1].contains("중첩 새 문단"), "{texts:?}");
    }

    #[test]
    fn 캡션_안_앵커에_삽입된다() {
        let mut doc = from_markdown("본문\n\n| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        {
            let t = first_table_mut(&mut doc);
            let template = t.cells[0].paragraphs[0].clone();
            let mut cap_para = template.clone();
            cap_para.chars = "캡션앵커"
                .chars()
                .map(HwpChar::Text)
                .chain(std::iter::once(HwpChar::CharCtrl(ctrl_char::PARA_BREAK)))
                .collect();
            t.caption = Some(hwp_model::Caption {
                side: hwp_model::CaptionSide::Bottom,
                direction: hwp_model::CaptionDirection::Horizontal,
                gap: 0,
                width: None,
                last_width: 0,
                paragraphs: vec![cap_para],
            });
        }
        assert!(insert_paragraph(
            &mut doc,
            "캡션앵커",
            "캡션 새 문단",
            false
        ));
        let t = first_table_mut(&mut doc);
        let cap = t.caption.as_ref().unwrap();
        assert_eq!(cap.paragraphs.len(), 2, "캡션 문단 2개");
        assert!(cap.paragraphs[1].plain_text().contains("캡션 새 문단"));
    }

    #[test]
    fn 본문_문단이_셀보다_먼저_잡힌다() {
        // 같은 앵커가 본문과 셀 양쪽에 있으면 본문이 이긴다 — 이 변경 전에 최상위
        // 문단을 맞히던 호출이 계속 같은 문단을 맞힌다는 호환성 보장.
        let mut doc = from_markdown("공통앵커\n\n| 가 | 나 |\n|----|----|\n| 공통앵커 | 2 |\n");
        assert!(insert_paragraph(&mut doc, "공통앵커", "새 문단", false));
        // 셀은 그대로 1문단.
        assert_eq!(cell_texts(&doc, 1, 0).len(), 1, "셀은 안 바뀜");
        // 본문에 삽입.
        let body: Vec<String> = doc.sections[0]
            .paragraphs
            .iter()
            .map(|p| p.plain_text())
            .collect();
        assert!(
            body.iter().any(|t| t.contains("새 문단")),
            "본문에 삽입: {body:?}"
        );
    }

    #[test]
    fn 셀_안_문단도_지운다_단_마지막은_남는다() {
        // 셀 문단 2개 중 하나만 매치 → 지워진다.
        let mut doc = from_markdown("본문\n\n| 가 | 나 |\n|----|----|\n| 유지 | 2 |\n");
        insert_paragraph(&mut doc, "유지", "지울문단", false);
        assert_eq!(cell_texts(&doc, 1, 0).len(), 2);
        assert_eq!(delete_paragraph(&mut doc, "지울문단"), 1);
        assert_eq!(cell_texts(&doc, 1, 0).len(), 1, "하나만 지움");

        // 셀에 하나뿐인 문단이 매치되면 남긴다(셀이 비면 한글 손상 판정).
        assert_eq!(delete_paragraph(&mut doc, "유지"), 0, "마지막 문단 보존");
        assert_eq!(cell_texts(&doc, 1, 0).len(), 1);
    }

    /// hwp5 원본 서브트리(`raw_children`)를 품은 개체를 하나 단다 — writer가 그 바이트를
    /// 그대로 다시 쓰는, 안쪽 IR 편집이 저장되지 않는 개체.
    fn attach_raw_object(doc: &mut Document, text: &str) {
        let template = doc.sections[0].paragraphs[0].clone();
        let mut inner = template.clone();
        inner.chars = text
            .chars()
            .map(HwpChar::Text)
            .chain(std::iter::once(HwpChar::CharCtrl(ctrl_char::PARA_BREAK)))
            .collect();
        doc.sections[0].paragraphs[0]
            .controls
            .push(Control::Generic(hwp_model::GenericControl {
                ctrl_id: *b"head",
                data: vec![0u8; 8],
                paragraph_lists: vec![hwp_model::ParagraphList {
                    header_data: Vec::new(),
                    paragraphs: vec![inner],
                }],
                extras: Vec::new(),
                // 비어 있지 않다 = hwp5 출신, 저장 시 원본 그대로 방출.
                raw_children: vec![hwp_model::OpaqueRecord {
                    tag: 0x0048,
                    data: Vec::new(),
                    children: Vec::new(),
                }],
                gso_shapes: Vec::new(),
                equation: None,
                column_def: None,
                caption: None,
                hwpx_raw_xml: None,
                container_box: None,
            }));
    }

    #[test]
    fn 원본보존_개체_안은_건드리지_않는다() {
        let mut doc = from_markdown("본문");
        attach_raw_object(&mut doc, "개체앵커");
        // 삽입도 삭제도 그 안에서는 일어나지 않는다 — 조용한 성공 금지.
        assert!(!insert_paragraph(&mut doc, "개체앵커", "새 문단", false));
        assert_eq!(delete_paragraph(&mut doc, "개체앵커"), 0);
        // 진단 함수가 "없음"과 "손댈 수 없음"을 구별한다.
        assert!(text_in_unwritable_object(&doc, "개체앵커"));
        assert!(!text_in_unwritable_object(&doc, "본문"));
        assert!(!text_in_unwritable_object(&doc, "어디에도 없는 문자열"));
        let g = doc.sections[0].paragraphs[0]
            .controls
            .iter()
            .find_map(|c| match c {
                Control::Generic(g) if g.ctrl_id == *b"head" => Some(g),
                _ => None,
            })
            .expect("개체 없음");
        assert_eq!(g.paragraph_lists[0].paragraphs.len(), 1, "안쪽 IR 불변");
    }
}
