//! Bookmark reading/creation through the HWP5 bokm extended control.
//!
//! Bookmarks use bokm controls rather than %bmk fields, so list_fields does not collect them:
//! - Character: ExtCtrl with code 22 (BOOKMARK) and ctrl_id=b"bokm", a single point marker without
//!   a START/END pair.
//! - Control: Generic with ctrl_id=b"bokm", empty data, and raw_children holding CTRL_DATA for the
//!   name.
//! - Name: a BSTR in a CTRL_DATA Parameter Set. Its layout differs from %clk fields and requires a
//!   separate codec copying genuine bytes for Hangul acceptance, following the `가나다`/`다문단`
//!   ground-truth method.
//!
//! The reader/IR/writer round-trip bokm losslessly (identity gate, fixtures/hwp5/bookmark.hwp).
//! This module provides on-demand inspection without IR mutation, plus insertion on edit paths.

use hwp_model::ctrl_char::BOOKMARK;
use hwp_model::opaque::OpaqueRecord;
use hwp_model::{Control, Document, GenericControl, HwpChar, Paragraph};

use crate::edit::{adjust_runs, ensure_anchor_shift_allowed, find_match, utf16_len};
use crate::field::{relink_ctrl_index, rev_payload};

pub(crate) const CTRL_DATA_TAG: u16 = 0x0010 + 71; // HWPTAG_CTRL_DATA (0x57)
const BOKM: [u8; 4] = *b"bokm";

/// Information about one bookmark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmarkInfo {
    /// Bookmark name.
    pub name: String,
}

/// Lists all bookmarks in encounter order, recursively through body, table cells and text boxes.
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

/// Reads a bookmark name from a Generic bokm control, reused by the HWPX writer when emitting
/// hp:bookmark name.
pub fn bookmark_name(ctrl: &Control) -> Option<String> {
    let Control::Generic(g) = ctrl else {
        return None;
    };
    g.raw_children
        .iter()
        .find(|r| r.tag == CTRL_DATA_TAG)
        .and_then(|r| decode_bokm_name(&r.data))
}

/// Decodes the name BSTR from a bokm CTRL_DATA Parameter Set: setid(2), count(2), id(4), type(2),
/// length(2, in WCHAR), and UTF-16LE name.
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

/// bokm CTRL_DATA Parameter Set bytes matching the genuine Hangul layout: setid=0x021b, count=1,
/// id=0x40000000, type=1 (BSTR), length(2 bytes), name. Reused by the HWPX reader when synthesizing
/// hp:bookmark name into Generic bokm.
pub fn make_bokm_ctrl_data(name: &str) -> Vec<u8> {
    let mut cd = vec![0x1b, 0x02, 0x01, 0x00, 0x00, 0x00, 0x00, 0x40, 0x01, 0x00];
    let units: Vec<u16> = name.encode_utf16().collect();
    cd.extend((units.len() as u16).to_le_bytes());
    for u in units {
        cd.extend(u.to_le_bytes());
    }
    cd
}

/// bokm control record: empty CTRL_HEADER data, with the name in CTRL_DATA.
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
        container_box: None,
    })
}

/// Bookmark character: one code-22 bokm ExtCtrl; the first four payload bytes are reversed ctrl_id.
fn make_bokm_char() -> HwpChar {
    HwpChar::ExtCtrl {
        code: BOOKMARK,
        ctrl_id: BOKM,
        payload: rev_payload(&BOKM),
        ctrl_index: None,
    }
}

/// Inserts a bookmark after anchor text in one paragraph, returning whether it was inserted.
fn create_bookmark_in_para(para: &mut Paragraph, anchor: &str, name: &str) -> bool {
    let Some((cidx, wpos)) = find_match(&para.chars, anchor, 0) else {
        return false;
    };
    let ins = (cidx + anchor.chars().count()).min(para.chars.len());
    let iw = wpos + utf16_len(anchor);
    // The insertion control index is the preceding ExtCtrl count, matching character/control order.
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
    para.header.ctrl_mask = 0; // The writer recalculates ctrl_mask from chars, including BOOKMARK bit22.
    para.line_segs.clear();
    true
}

/// Recursively inserts at the first anchor match in body, table-cell or text-box paragraphs.
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
                            // The object's content changed; clear captured XML to prevent stale
                            // output.
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

/// Inserts a named point bookmark after anchor text in the first matching paragraph. The result
/// indicates insertion; inspect it with list_bookmarks afterward.
/// Refuses before mutation when insertion would shift opaque range tags in the target.
pub fn create_bookmark(doc: &mut Document, anchor: &str, name: &str) -> Result<bool, String> {
    ensure_anchor_shift_allowed(doc, anchor, 8)?;
    for section in &mut doc.sections {
        for para in &mut section.paragraphs {
            if create_bookmark_rec(para, anchor, name) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwp_model::{CharShapeId, Section};

    /// 24 CTRL_DATA bytes for "책갈피테스트", extracted from the genuine bookmark.hwp fixture.
    const FIXTURE_CTRL_DATA: [u8; 24] = [
        0x1b, 0x02, 0x01, 0x00, 0x00, 0x00, 0x00, 0x40, 0x01, 0x00, 0x06, 0x00, 0x45, 0xcc, 0x08,
        0xac, 0x3c, 0xd5, 0x4c, 0xd1, 0xa4, 0xc2, 0xb8, 0xd2,
    ];

    #[test]
    fn ctrl_data_정품_바이트_동일() {
        // Generated bytes must match genuine Hangul exactly for native acceptance.
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
        // Truncated data returns None without panicking.
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
        assert!(create_bookmark(&mut doc, "제목", "책갈피1").unwrap());
        let bms = list_bookmarks(&doc);
        assert_eq!(bms.len(), 1);
        assert_eq!(bms[0].name, "책갈피1");
        // A missing anchor inserts nothing.
        assert!(!create_bookmark(&mut doc, "없는앵커", "x").unwrap());
    }
}
