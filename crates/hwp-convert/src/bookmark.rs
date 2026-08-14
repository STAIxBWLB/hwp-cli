//! 책갈피(bookmark) 읽기·생성 — hwp5 `bokm` 확장 컨트롤.
//!
//! 책갈피는 필드(%bmk)가 아니라 `bokm` 컨트롤로 저장된다(그래서 `list_fields`가 못 잡는다):
//! - 문자: `ExtCtrl { code: 22(BOOKMARK), ctrl_id: b"bokm" }` — START/END 쌍 없는 **단일 지점 표식**.
//! - 컨트롤: `Generic { ctrl_id: b"bokm", data: []빈, raw_children: [CTRL_DATA(이름)] }`.
//! - 이름: CTRL_DATA Parameter Set의 BSTR — %clk 필드와 레이아웃이 달라(별도 코덱) 정품 바이트를
//!   그대로 복제해야 한글이 수용한다(가나다·다문단 정답지 방식).
//!
//! 리더/IR/writer는 `bokm`을 무손실 왕복하므로(identity 게이트, `fixtures/hwp5/bookmark.hwp`),
//! 이 모듈은 IR을 바꾸지 않는 온디맨드 조회와 편집 경로 삽입만 담당한다.

use hwp_model::ctrl_char::BOOKMARK;
use hwp_model::opaque::OpaqueRecord;
use hwp_model::{Control, Document, GenericControl, HwpChar, Paragraph};

use crate::edit::{adjust_runs, find_match, utf16_len};
use crate::field::{relink_ctrl_index, rev_payload};

const CTRL_DATA_TAG: u16 = 0x0010 + 71; // HWPTAG_CTRL_DATA (0x57)
const BOKM: [u8; 4] = *b"bokm";

/// 책갈피 하나의 정보.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmarkInfo {
    /// 책갈피 이름.
    pub name: String,
}

/// 문서의 모든 책갈피를 등장 순서로 나열한다(본문·표 셀·글상자 재귀).
pub fn list_bookmarks(doc: &Document) -> Vec<BookmarkInfo> {
    let mut out = Vec::new();
    for section in &doc.sections {
        for para in &section.paragraphs {
            collect_bookmarks(para, &mut out);
        }
    }
    out
}

fn collect_bookmarks(para: &Paragraph, out: &mut Vec<BookmarkInfo>) {
    for ch in &para.chars {
        if let HwpChar::ExtCtrl {
            code,
            ctrl_id,
            ctrl_index,
            ..
        } = ch
            && *code == BOOKMARK
            && *ctrl_id == BOKM
        {
            let name = ctrl_index
                .and_then(|idx| para.controls.get(idx as usize))
                .and_then(bookmark_name)
                .unwrap_or_default();
            out.push(BookmarkInfo { name });
        }
    }
    for ctrl in &para.controls {
        match ctrl {
            Control::Table(t) => {
                for cell in &t.cells {
                    for p in &cell.paragraphs {
                        collect_bookmarks(p, out);
                    }
                }
            }
            Control::Generic(g) => {
                for l in &g.paragraph_lists {
                    for p in &l.paragraphs {
                        collect_bookmarks(p, out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// bokm 컨트롤(Generic)에서 책갈피 이름을 읽는다. hwpx writer가 `<hp:bookmark name>`
/// 방출 시 재사용한다.
pub fn bookmark_name(ctrl: &Control) -> Option<String> {
    let Control::Generic(g) = ctrl else {
        return None;
    };
    g.raw_children
        .iter()
        .find(|r| r.tag == CTRL_DATA_TAG)
        .and_then(|r| decode_bokm_name(&r.data))
}

/// bokm CTRL_DATA Parameter Set에서 이름 BSTR을 디코드한다.
/// 레이아웃: setid(2) count(2) id(4) type(2) len(2=WCHAR 수) 이름(utf16-le).
fn decode_bokm_name(cd: &[u8]) -> Option<String> {
    if cd.len() < 12 {
        return None;
    }
    let len = u16::from_le_bytes([cd[10], cd[11]]) as usize;
    let end = 12usize.checked_add(len * 2)?;
    if end > cd.len() {
        return None;
    }
    Some(decode_utf16le(&cd[12..end]))
}

fn decode_utf16le(b: &[u8]) -> String {
    let units: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

/// bokm CTRL_DATA Parameter Set 바이트(정품 한글과 동일 레이아웃).
/// setid=0x021b · count=1 · id=0x40000000 · type=1(BSTR) · len(2B) · 이름.
/// hwpx reader가 `<hp:bookmark name>` → bokm Generic 합성 시 재사용한다.
pub fn make_bokm_ctrl_data(name: &str) -> Vec<u8> {
    let mut cd = vec![0x1b, 0x02, 0x01, 0x00, 0x00, 0x00, 0x00, 0x40, 0x01, 0x00];
    let units: Vec<u16> = name.encode_utf16().collect();
    cd.extend((units.len() as u16).to_le_bytes());
    for u in units {
        cd.extend(u.to_le_bytes());
    }
    cd
}

/// bokm 컨트롤 레코드(CTRL_HEADER data 빈, CTRL_DATA에 이름).
fn make_bokm_control(name: &str) -> Control {
    Control::Generic(GenericControl {
        ctrl_id: BOKM,
        data: Vec::new(),
        paragraph_lists: Vec::new(),
        extras: Vec::new(),
        raw_children: vec![OpaqueRecord {
            tag: CTRL_DATA_TAG,
            data: make_bokm_ctrl_data(name),
            children: Vec::new(),
        }],
        gso_shapes: Vec::new(),
        equation: None,
        column_def: None,
        caption: None,
        hwpx_raw_xml: None,
    })
}

/// 책갈피 문자: 단일 code-22 ExtCtrl(bokm). payload 선두 4B = 역순 ctrl_id.
fn make_bokm_char() -> HwpChar {
    HwpChar::ExtCtrl {
        code: BOOKMARK,
        ctrl_id: BOKM,
        payload: rev_payload(&BOKM),
        ctrl_index: None,
    }
}

/// 한 문단에서 앵커 텍스트 뒤에 책갈피를 삽입한다. 반환=삽입 여부.
fn create_bookmark_in_para(para: &mut Paragraph, anchor: &str, name: &str) -> bool {
    let Some((cidx, wpos)) = find_match(&para.chars, anchor, 0) else {
        return false;
    };
    let ins = (cidx + anchor.chars().count()).min(para.chars.len());
    let iw = wpos + utf16_len(anchor);
    // control 삽입 위치 = ins 이전 ExtCtrl 개수(등장순서가 chars와 정합해야 함).
    let ci = para.chars[..ins]
        .iter()
        .filter(|c| matches!(c, HwpChar::ExtCtrl { .. }))
        .count()
        .min(para.controls.len());
    para.controls.insert(ci, make_bokm_control(name));
    let ch = make_bokm_char();
    let inserted_w = ch.wchar_width();
    para.chars.insert(ins, ch);
    adjust_runs(&mut para.char_shape_runs, iw, 0, inserted_w);
    relink_ctrl_index(para);
    para.header.ctrl_mask = 0; // writer가 chars에서 재계산(BOOKMARK bit22 포함)
    para.line_segs.clear();
    true
}

/// 본문/표 셀/글상자 문단을 재귀로 훑어 첫 매칭에 책갈피를 삽입한다.
fn create_bookmark_rec(para: &mut Paragraph, anchor: &str, name: &str) -> bool {
    if create_bookmark_in_para(para, anchor, name) {
        return true;
    }
    for ctrl in &mut para.controls {
        match ctrl {
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        if create_bookmark_rec(p, anchor, name) {
                            return true;
                        }
                    }
                }
            }
            Control::Generic(g) => {
                for l in &mut g.paragraph_lists {
                    for p in &mut l.paragraphs {
                        if create_bookmark_rec(p, anchor, name) {
                            // 내용이 바뀐 개체의 원문 XML은 낡았다 — stale 방출 금지.
                            g.hwpx_raw_xml = None;
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// `anchor` 텍스트를 가진 첫 문단의 그 뒤에 `name` 이름의 책갈피(지점 표식)를 삽입한다.
/// 반환=삽입 여부. 삽입 후 [`list_bookmarks`]로 확인할 수 있다.
pub fn create_bookmark(doc: &mut Document, anchor: &str, name: &str) -> bool {
    for section in &mut doc.sections {
        for para in &mut section.paragraphs {
            if create_bookmark_rec(para, anchor, name) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwp_model::{CharShapeId, Section};

    /// 정품 bookmark.hwp에서 추출한 "책갈피테스트" CTRL_DATA 24바이트.
    const FIXTURE_CTRL_DATA: [u8; 24] = [
        0x1b, 0x02, 0x01, 0x00, 0x00, 0x00, 0x00, 0x40, 0x01, 0x00, 0x06, 0x00, 0x45, 0xcc, 0x08,
        0xac, 0x3c, 0xd5, 0x4c, 0xd1, 0xa4, 0xc2, 0xb8, 0xd2,
    ];

    #[test]
    fn ctrl_data_정품_바이트_동일() {
        // 생성 바이트가 정품 한글과 완전히 같아야 한글이 수용한다.
        assert_eq!(make_bokm_ctrl_data("책갈피테스트"), FIXTURE_CTRL_DATA);
    }

    #[test]
    fn 이름_디코드_왕복() {
        assert_eq!(
            decode_bokm_name(&FIXTURE_CTRL_DATA).as_deref(),
            Some("책갈피테스트")
        );
        assert_eq!(
            decode_bokm_name(&make_bokm_ctrl_data("표1")).as_deref(),
            Some("표1")
        );
        // 잘린 데이터는 None(패닉 없음).
        assert_eq!(decode_bokm_name(&FIXTURE_CTRL_DATA[..8]), None);
    }

    #[test]
    fn 생성_후_조회() {
        let mut doc = Document::default();
        doc.sections.push(Section {
            paragraphs: vec![Paragraph {
                chars: "제목 여기".chars().map(HwpChar::Text).collect(),
                char_shape_runs: vec![(0, CharShapeId(0))],
                ..Default::default()
            }],
            extras: Vec::new(),
        });
        assert!(create_bookmark(&mut doc, "제목", "책갈피1"));
        let bms = list_bookmarks(&doc);
        assert_eq!(bms.len(), 1);
        assert_eq!(bms[0].name, "책갈피1");
        // 없는 앵커는 삽입 실패.
        assert!(!create_bookmark(&mut doc, "없는앵커", "x"));
    }
}
