//! 각주/미주(footnote/endnote) 수집·번호 매기기.
//!
//! 본문에는 `HwpChar::ExtCtrl{ctrl_id:"fn  "/"en  "}` 앵커가 있고, 노트 내용은
//! 대응 `controls[ctrl_index]`(GenericControl)의 `paragraph_lists`에 들어 있다.
//! 렌더러는 앵커 위치에 윗첨자 번호를, 페이지 하단에 노트 영역을 그린다.

use std::collections::HashMap;

use hwp_model::{Control, GenericControl, HwpChar, Paragraph};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NoteKind {
    Foot,
    End,
}

/// 노트 하나: 종류·표시 번호·내용 컨트롤.
pub struct Note<'a> {
    pub kind: NoteKind,
    pub number: u32,
    pub content: &'a GenericControl,
}

/// 컨트롤이 각주/미주면 종류를 돌려준다.
fn note_kind(g: &GenericControl) -> Option<NoteKind> {
    match &g.ctrl_id {
        b"fn  " => Some(NoteKind::Foot),
        b"en  " => Some(NoteKind::End),
        _ => None,
    }
}

/// 구역 문단들을 본문 등장 순서로 훑어 각주·미주에 번호를 매긴다
/// (각주·미주 각각 1부터). 앵커는 `para.chars`의 ExtCtrl 순서를 따른다.
pub fn collect_notes(paras: &[Paragraph]) -> Vec<Note<'_>> {
    let mut notes = Vec::new();
    let (mut foot_n, mut end_n) = (0u32, 0u32);
    for para in paras {
        for ch in &para.chars {
            let HwpChar::ExtCtrl {
                ctrl_index: Some(ci),
                ..
            } = ch
            else {
                continue;
            };
            let Some(Control::Generic(g)) = para.controls.get(*ci as usize) else {
                continue;
            };
            match note_kind(g) {
                Some(NoteKind::Foot) => {
                    foot_n += 1;
                    notes.push(Note {
                        kind: NoteKind::Foot,
                        number: foot_n,
                        content: g,
                    });
                }
                Some(NoteKind::End) => {
                    end_n += 1;
                    notes.push(Note {
                        kind: NoteKind::End,
                        number: end_n,
                        content: g,
                    });
                }
                None => {}
            }
        }
    }
    notes
}

/// 이 문단의 (ctrl_index → 표시 번호) — 본문 윗첨자 마커용.
pub fn para_marks(notes: &[Note], para: &Paragraph) -> HashMap<u32, u32> {
    let mut marks = HashMap::new();
    for (ci, ctrl) in para.controls.iter().enumerate() {
        if let Control::Generic(g) = ctrl
            && let Some(n) = notes.iter().find(|n| std::ptr::eq(n.content, g))
        {
            marks.insert(ci as u32, n.number);
        }
    }
    marks
}

/// 이 문단이 앵커를 가진 노트들(하단 영역 렌더용, 본문 순서).
pub fn para_notes<'a, 'n>(notes: &'a [Note<'n>], para: &Paragraph) -> Vec<&'a Note<'n>> {
    let mut out = Vec::new();
    for ch in &para.chars {
        if let HwpChar::ExtCtrl {
            ctrl_index: Some(ci),
            ..
        } = ch
            && let Some(Control::Generic(g)) = para.controls.get(*ci as usize)
            && let Some(n) = notes.iter().find(|n| std::ptr::eq(n.content, g))
        {
            out.push(n);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwp_model::Paragraph;

    fn generic(id: &[u8; 4]) -> GenericControl {
        GenericControl {
            ctrl_id: *id,
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
        }
    }

    fn anchor(ci: u32) -> HwpChar {
        HwpChar::ExtCtrl {
            code: 17,
            ctrl_id: *b"fn  ",
            payload: vec![0; 12],
            ctrl_index: Some(ci),
        }
    }

    /// 각주 2개 + 미주 1개를 본문 순서로 모아 각각 1부터 번호를 매긴다.
    #[test]
    fn 각주_미주_번호_매기기() {
        let para = Paragraph {
            controls: vec![
                Control::Generic(generic(b"fn  ")),
                Control::Generic(generic(b"en  ")),
                Control::Generic(generic(b"fn  ")),
            ],
            chars: vec![
                HwpChar::Text('가'),
                anchor(0), // 각주 → 1
                HwpChar::Text('나'),
                anchor(1), // 미주 → 1
                anchor(2), // 각주 → 2
            ],
            ..Paragraph::default()
        };
        let notes = collect_notes(std::slice::from_ref(&para));
        assert_eq!(notes.len(), 3);
        assert_eq!((notes[0].kind, notes[0].number), (NoteKind::Foot, 1));
        assert_eq!((notes[1].kind, notes[1].number), (NoteKind::End, 1));
        assert_eq!((notes[2].kind, notes[2].number), (NoteKind::Foot, 2));

        // 본문 마커: ctrl_index → 표시 번호.
        let marks = para_marks(&notes, &para);
        assert_eq!(marks.get(&0), Some(&1)); // 각주1
        assert_eq!(marks.get(&1), Some(&1)); // 미주1
        assert_eq!(marks.get(&2), Some(&2)); // 각주2

        // 이 문단에 속한 노트 3개.
        assert_eq!(para_notes(&notes, &para).len(), 3);
    }

    /// 각주가 없는 문단은 빈 결과(회귀 안전).
    #[test]
    fn 각주_없으면_빈결과() {
        let para = Paragraph {
            chars: vec![HwpChar::Text('가'), HwpChar::Text('나')],
            ..Paragraph::default()
        };
        assert!(collect_notes(std::slice::from_ref(&para)).is_empty());
        assert!(para_marks(&[], &para).is_empty());
    }
}
