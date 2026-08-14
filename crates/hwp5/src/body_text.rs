//! BodyText 섹션 스트림 → [`Section`] 파싱.
//!
//! 실측으로 확정한 구조 (fixtures 기준):
//! - 섹션 루트는 PARA_HEADER 트리들의 나열.
//! - CTRL_HEADER의 ctrl_id는 **역순으로 저장**된다 (b"dces" = secd).
//! - 표: CTRL_HEADER(tbl) 아래에 TABLE 레코드 + 셀마다
//!   [LIST_HEADER, PARA_HEADER...]가 **형제로** 나열된다 — LIST_HEADER가
//!   새 셀을 열고 다음 LIST_HEADER 전까지의 문단이 그 셀 소속.
//! - TABLE의 "행 크기" 배열은 행별 셀 개수다.

use hwp_model::{
    Caption, CaptionDirection, CaptionSide, Cell, CharKind, CharShapeId, ColumnDef, Control,
    Equation, GenericControl, Hwp5ParagraphChild, HwpChar, HwpUnit, LineSeg, PageDef,
    ParaHeaderInfo, ParaShapeId, Paragraph, ParagraphList, Section, SectionDef, StyleId, Table,
    char_kind,
};

use crate::codec::ByteReader;
use crate::doc_info::to_opaque;
use crate::error::Result;
use crate::record::{RecordNode, tag};

/// 섹션 레코드 트리를 Section으로 변환한다.
pub fn parse_section(roots: &[RecordNode]) -> (Section, Vec<String>) {
    let mut section = Section::default();
    let mut warnings = Vec::new();
    for node in roots {
        if node.tag == tag::PARA_HEADER {
            section
                .paragraphs
                .push(parse_paragraph(node, &mut warnings));
        } else {
            warnings.push(format!(
                "섹션 루트에 문단이 아닌 레코드 0x{:03X} — 보존",
                node.tag
            ));
            section.extras.push(to_opaque(node));
        }
    }
    (section, warnings)
}

fn parse_paragraph(node: &RecordNode, warnings: &mut Vec<String>) -> Paragraph {
    let mut para = Paragraph::default();

    // PARA_HEADER 페이로드 (22바이트 prefix + 버전별 tail)
    let mut nchars = None;
    match parse_para_header(&node.data) {
        Ok((shape, style, info, n)) => {
            para.para_shape = shape;
            para.style = style;
            para.header = info;
            nchars = Some(n);
        }
        Err(e) => warnings.push(format!("PARA_HEADER 파싱 실패: {e}")),
    }

    for child in &node.children {
        match child.tag {
            tag::PARA_TEXT => para.chars = decode_para_text(&child.data, warnings),
            tag::PARA_CHAR_SHAPE => {
                let mut r = ByteReader::new(&child.data);
                while r.remaining() >= 8 {
                    let pos = r.read_u32().expect("크기 확인됨");
                    let id = r.read_u32().expect("크기 확인됨");
                    para.char_shape_runs.push((pos, CharShapeId(id as u16)));
                }
            }
            tag::PARA_LINE_SEG => match parse_line_segs(&child.data) {
                Ok(segs) => para.line_segs = segs,
                Err(e) => warnings.push(format!("PARA_LINE_SEG 파싱 실패: {e}")),
            },
            tag::CTRL_HEADER => {
                para.header
                    .hwp5_child_order
                    .push(Hwp5ParagraphChild::Control);
                para.controls.push(parse_control(child, warnings));
            }
            _ => {
                para.header.hwp5_child_order.push(Hwp5ParagraphChild::Extra);
                para.extras.push(to_opaque(child));
            }
        }
    }

    // 위치 산수의 정합성 검증: 분류표가 틀리면 즉시 드러나는 강력한 불변식
    if let Some(n) = nchars
        && !para.chars.is_empty()
        && para.wchar_len() != n
    {
        warnings.push(format!(
            "문단 WCHAR 수 불일치: PARA_HEADER {n} vs PARA_TEXT 계산 {} — 컨트롤 분류 오류 가능성",
            para.wchar_len()
        ));
    }

    link_controls(&mut para, warnings);
    para
}

fn parse_para_header(data: &[u8]) -> Result<(ParaShapeId, StyleId, ParaHeaderInfo, u32)> {
    let mut r = ByteReader::new(data);
    let nchars_raw = r.read_u32()?;
    let ctrl_mask = r.read_u32()?;
    let para_shape = ParaShapeId(r.read_u16()?);
    let style = StyleId(u16::from(r.read_u8()?));
    let break_type = r.read_u8()?;
    let _char_shape_count = r.read_u16()?;
    let _range_tag_count = r.read_u16()?;
    let _line_seg_count = r.read_u16()?;
    let instance_id = r.read_u32()?;
    let info = ParaHeaderInfo {
        chars_flags: (nchars_raw >> 24 & 0x80) as u8,
        ctrl_mask,
        break_type,
        instance_id,
        tail: r.take_rest().to_vec(),
        hwp5_child_order: Vec::new(),
    };
    Ok((para_shape, style, info, nchars_raw & 0x7FFF_FFFF))
}

/// PARA_TEXT 디코딩 — 컨트롤 문자 분류표가 위치 산수의 기준.
fn decode_para_text(data: &[u8], warnings: &mut Vec<String>) -> Vec<HwpChar> {
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    if !data.len().is_multiple_of(2) {
        warnings.push("PARA_TEXT 길이가 홀수 — 마지막 바이트 무시".to_string());
    }

    let mut chars = Vec::new();
    let mut i = 0usize;
    while i < units.len() {
        let u = units[i];
        if u < 32 {
            match char_kind(u) {
                CharKind::Char => {
                    chars.push(HwpChar::CharCtrl(u));
                    i += 1;
                }
                CharKind::Inline | CharKind::Extended => {
                    if i + 8 > units.len() {
                        warnings.push(format!(
                            "컨트롤 문자 {u}의 8 WCHAR가 잘림 (남은 {}개) — 중단",
                            units.len() - i
                        ));
                        break;
                    }
                    // [코드, 정보 6 WCHAR, 코드] — 정보부를 바이트로 보존
                    let payload: Vec<u8> = units[i + 1..i + 7]
                        .iter()
                        .flat_map(|w| w.to_le_bytes())
                        .collect();
                    if units[i + 7] != u {
                        warnings.push(format!(
                            "컨트롤 문자 {u}의 닫는 코드 불일치 ({})",
                            units[i + 7]
                        ));
                    }
                    if char_kind(u) == CharKind::Inline {
                        chars.push(HwpChar::InlineCtrl { code: u, payload });
                    } else {
                        // 선두 4바이트 = 역순 ctrl_id
                        let mut ctrl_id = [payload[0], payload[1], payload[2], payload[3]];
                        ctrl_id.reverse();
                        chars.push(HwpChar::ExtCtrl {
                            code: u,
                            ctrl_id,
                            payload,
                            ctrl_index: None,
                        });
                    }
                    i += 8;
                }
            }
        } else if (0xD800..0xDC00).contains(&u) {
            // 서로게이트 쌍
            if i + 1 < units.len() && (0xDC00..0xE000).contains(&units[i + 1]) {
                let c = char::decode_utf16([u, units[i + 1]])
                    .next()
                    .and_then(|r| r.ok())
                    .unwrap_or(char::REPLACEMENT_CHARACTER);
                chars.push(HwpChar::Text(c));
                i += 2;
            } else {
                warnings.push(format!("짝 없는 서로게이트 0x{u:04X}"));
                chars.push(HwpChar::Text(char::REPLACEMENT_CHARACTER));
                i += 1;
            }
        } else if (0xDC00..0xE000).contains(&u) {
            warnings.push(format!("짝 없는 서로게이트 0x{u:04X}"));
            chars.push(HwpChar::Text(char::REPLACEMENT_CHARACTER));
            i += 1;
        } else {
            chars.push(HwpChar::Text(
                char::from_u32(u32::from(u)).unwrap_or(char::REPLACEMENT_CHARACTER),
            ));
            i += 1;
        }
    }
    chars
}

fn parse_line_segs(data: &[u8]) -> Result<Vec<LineSeg>> {
    let mut r = ByteReader::new(data);
    let mut segs = Vec::with_capacity(data.len() / 36);
    while r.remaining() >= 36 {
        segs.push(LineSeg {
            text_start: r.read_u32()?,
            v_pos: r.read_i32()?,
            line_height: r.read_i32()?,
            text_height: r.read_i32()?,
            baseline_gap: r.read_i32()?,
            line_spacing: r.read_i32()?,
            col_start: r.read_i32()?,
            seg_width: r.read_i32()?,
            flags: r.read_u32()?,
        });
    }
    Ok(segs)
}

/// ExtCtrl 문자 ↔ CTRL_HEADER 레코드를 등장 순서로 연결하고 ctrl_id를 교차 검증.
fn link_controls(para: &mut Paragraph, warnings: &mut Vec<String>) {
    let mut next = 0u32;
    let control_ids: Vec<[u8; 4]> = para.controls.iter().map(Control::ctrl_id).collect();
    for ch in &mut para.chars {
        if let HwpChar::ExtCtrl {
            ctrl_id,
            ctrl_index,
            ..
        } = ch
        {
            if (next as usize) < control_ids.len() {
                let expected = control_ids[next as usize];
                if *ctrl_id != expected {
                    warnings.push(format!(
                        "ExtCtrl ctrl_id 불일치: 텍스트 {:?} vs CTRL_HEADER {:?}",
                        String::from_utf8_lossy(ctrl_id),
                        String::from_utf8_lossy(&expected),
                    ));
                }
                *ctrl_index = Some(next);
                next += 1;
            } else {
                warnings.push(format!(
                    "ExtCtrl {:?}에 대응하는 CTRL_HEADER 없음",
                    String::from_utf8_lossy(ctrl_id)
                ));
            }
        }
    }
    if (next as usize) < para.controls.len() {
        warnings.push(format!(
            "CTRL_HEADER {}개가 텍스트의 ExtCtrl과 연결되지 않음",
            para.controls.len() - next as usize
        ));
    }
}

fn parse_control(node: &RecordNode, warnings: &mut Vec<String>) -> Control {
    if node.data.len() < 4 {
        warnings.push("CTRL_HEADER 페이로드가 4바이트 미만".to_string());
        return Control::Generic(GenericControl {
            ctrl_id: *b"????",
            data: node.data.clone(),
            paragraph_lists: Vec::new(),
            extras: node.children.iter().map(to_opaque).collect(),
            raw_children: node.children.iter().map(to_opaque).collect(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
        });
    }
    let mut ctrl_id = [node.data[0], node.data[1], node.data[2], node.data[3]];
    ctrl_id.reverse(); // 역순 저장 → 정방향
    let rest = node.data[4..].to_vec();

    match &ctrl_id {
        b"secd" => Control::SectionDef(parse_section_def(rest, &node.children, warnings)),
        b"tbl " => Control::Table(parse_table(rest, &node.children, warnings)),
        // Interpret a drawing object as an image when it has a picture record
        // and no text-box paragraphs. Direct caption records do not count as
        // text-box content (table 71).
        b"gso "
            if !node.children.iter().any(|c| match c.tag {
                tag::LIST_HEADER | tag::PARA_HEADER => false,
                _ => subtree_has_paragraphs(c),
            }) && find_picture_record(&node.children).is_some() =>
        {
            match parse_picture_gso(&rest, &node.children, warnings) {
                Ok(p) => Control::Picture(p),
                Err(e) => {
                    warnings.push(format!("그림 개체 파싱 실패: {e}"));
                    Control::Generic(parse_generic(ctrl_id, rest, &node.children, warnings))
                }
            }
        }
        _ => Control::Generic(parse_generic(ctrl_id, rest, &node.children, warnings)),
    }
}

/// 서브트리에서 SHAPE_COMPONENT_PICTURE 레코드를 찾는다.
fn find_picture_record(children: &[RecordNode]) -> Option<&RecordNode> {
    for child in children {
        if child.tag == tag::SHAPE_COMPONENT_PICTURE {
            return Some(child);
        }
        if let Some(found) = find_picture_record(&child.children) {
            return Some(found);
        }
    }
    None
}

/// Parses the 22-byte caption LIST_HEADER payload (tables 71-73), matching
/// pyhwp `tagid56_list_header.TableCaption`:
/// paragraph count i32 + listflags u32 (table 65 direction bits 0-2) +
/// caption flags u32 (table 73 side bits 0-1 and full-width bit 2) + caption
/// width HWPUNIT + gap HWPUNIT16 + maximum text extent HWPUNIT.
fn parse_caption_header(data: &[u8]) -> Result<Caption> {
    let mut r = ByteReader::new(data);
    let _para_count = r.read_i32()?;
    let list_attr = r.read_u32()?;
    let flags = r.read_u32()?;
    let width = r.read_i32()?;
    let gap = r.read_u16()?;
    let max_width = r.read_i32()?;
    Ok(Caption {
        side: match flags & 0x3 {
            0 => CaptionSide::Left,
            1 => CaptionSide::Right,
            2 => CaptionSide::Top,
            _ => CaptionSide::Bottom,
        },
        direction: if list_attr & 0x7 == 1 {
            CaptionDirection::Vertical
        } else {
            CaptionDirection::Horizontal
        },
        gap: i32::from(gap),
        // Bit 2 means full-size through margins, so explicit width is unused.
        width: if flags & 0x4 != 0 {
            None
        } else {
            Some(width.max(0) as u32)
        },
        last_width: max_width.max(0) as u32,
        paragraphs: Vec::new(),
    })
}

/// Separates a caption LIST_HEADER and following paragraphs from direct GSO
/// children. Direct GSO LIST_HEADER records are captions; text-box lists are
/// nested under SHAPE_COMPONENT (pyhwp `GShapeObjectCaption`). Consumed nodes
/// are omitted from extras.
fn take_gso_caption(
    children: &[RecordNode],
    warnings: &mut Vec<String>,
) -> (Option<Caption>, Vec<hwp_model::OpaqueRecord>) {
    let mut caption: Option<Caption> = None;
    // Only consecutive direct paragraphs after LIST_HEADER belong to the caption.
    let mut caption_open = false;
    let mut extras = Vec::new();
    for child in children {
        match child.tag {
            tag::LIST_HEADER if caption.is_none() => match parse_caption_header(&child.data) {
                Ok(c) => {
                    caption = Some(c);
                    caption_open = true;
                }
                Err(e) => {
                    warnings.push(format!("캡션 LIST_HEADER 파싱 실패: {e}"));
                    extras.push(to_opaque(child));
                }
            },
            tag::PARA_HEADER if caption_open => {
                let para = parse_paragraph(child, warnings);
                caption
                    .as_mut()
                    .expect("caption_open이면 Some")
                    .paragraphs
                    .push(para);
            }
            _ => {
                caption_open = false;
                extras.push(to_opaque(child));
            }
        }
    }
    (caption, extras)
}

/// gso 그림 개체: 개체 공통 속성(크기)과 그림 레코드의 BinItem ID를 추출한다.
///
/// 그림 개체 속성 레이아웃 (스펙 §표 91): 테두리 색(4)+굵기(4)+속성(4)
/// + 꼭지점 4점(32) + 자르기(16) + 안쪽 여백(8) + 밝기(1)+명암(1)+효과(1)
/// + **BinItem ID (2 bytes)** at offset 71, followed by border alpha (1 byte)
///   and instance ID (4 bytes)
/// + variable picture-effect data from tables 107-108, with flags at byte 78.
fn parse_picture_gso(
    common: &[u8],
    children: &[RecordNode],
    warnings: &mut Vec<String>,
) -> Result<hwp_model::Picture> {
    // 개체 공통 속성(표 69): 속성(4) 세로offset(4) 가로offset(4) 폭(4) 높이(4) z-order(4)
    let mut r = ByteReader::new(common);
    let attr = r.read_u32()?;
    let vert_offset = r.read_i32()?;
    let horz_offset = r.read_i32()?;
    let width = HwpUnit(r.read_i32()?);
    let height = HwpUnit(r.read_i32()?);
    // z-order는 폭/높이 뒤(@20) — GsoPlacement(표 69)와 동일 레이아웃. 손상·짧은
    // 데이터면 0 폴백.
    let z_order = r.read_i32().unwrap_or(0).max(0) as u32;

    let pic_node = find_picture_record(children)
        .ok_or_else(|| crate::error::Hwp5Error::MalformedRecord("그림 레코드 없음".into()))?;
    let pd = &pic_node.data;
    let mut pr = ByteReader::new(pd);
    pr.read_bytes(71)?;
    let bin_id = pr.read_u16()?;

    // The crop rectangle at byte 44 uses source-image HWPUNIT coordinates.
    // Normalize a full natural-size crop to None so synthesized records and
    // the semantic no-crop representation compare equally.
    let rd_i32 = |o: usize| -> Option<i32> {
        pd.get(o..o + 4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let crop = match (rd_i32(44), rd_i32(48), rd_i32(52), rd_i32(56)) {
        (Some(l), Some(t), Some(r_), Some(b)) => {
            let natural = (rd_i32(82), rd_i32(86));
            let full =
                matches!(natural, (Some(nw), Some(nh)) if l == 0 && t == 0 && r_ == nw && b == nh);
            if full {
                None
            } else {
                Some([l as f32, t as f32, r_ as f32, b as f32])
            }
        }
        _ => None,
    };
    let brightness = pd.get(68).map(|&v| (v as i8).clamp(-100, 100)).unwrap_or(0);
    let contrast = pd.get(69).map(|&v| (v as i8).clamp(-100, 100)).unwrap_or(0);
    let effect_flags = rd_i32(78).map(|v| v as u32).unwrap_or(0);
    // Discard the stream when flags are zero, including the writer's 13-byte constant block.
    let effects_raw = if effect_flags != 0 {
        pd.get(78..).unwrap_or(&[]).to_vec()
    } else {
        Vec::new()
    };
    // Rotation lives in the parent SHAPE_COMPONENT matrix, not the picture record.
    let rotation = children
        .iter()
        .find(|c| c.tag == tag::SHAPE_COMPONENT)
        .and_then(|sc| gso_rotation_deg(&sc.data));
    // Table 71 caption: direct GSO LIST_HEADER plus following paragraphs.
    let (caption, extras) = take_gso_caption(children, warnings);

    Ok(hwp_model::Picture {
        common_data: common.to_vec(),
        width,
        height,
        treat_as_char: attr & 1 != 0,
        // 부유(글 앞) 그림 배치 승계(GE-9): 세로/가로 오프셋·z-order를 실값으로
        // 올린다. GE-8이 TABLE에 쓴 GsoPlacement와 동일 표 69 레이아웃. hwpx write가
        // floating `<hp:pos>`/zOrder에 그대로 방출한다. common_data raw는 그대로
        // 두므로 hwp5→hwp5 identity 재직렬화는 무손실.
        z_order,
        vert_offset,
        horz_offset,
        description: parse_gso_description(common),
        crop,
        flip: 0, // The HWP5 flip bit is unverified; only HWPX populates it for now.
        rotation,
        brightness,
        contrast,
        effect_flags,
        effects_raw,
        caption,
        bin_ref: hwp_model::BinRef::Id(hwp_model::BinDataId(bin_id)),
        extras,
    })
}

/// Extracts rotation in degrees from a SHAPE_COMPONENT transform matrix.
/// The measured layout is `[CHID x2 or x1] + object properties + translation
/// (48 bytes) + (scale 48 + rotation 48) x count`, matching
/// `shape_draw::parse_style`. The result is clockwise-positive in y-down
/// coordinates; magnitudes below 0.01 degrees return `None`.
fn gso_rotation_deg(d: &[u8]) -> Option<f32> {
    let rd_u16 =
        |o: usize| -> Option<u16> { d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])) };
    let rd_f64 = |o: usize| -> Option<f64> {
        d.get(o..o + 8)
            .and_then(|b| b.try_into().ok())
            .map(f64::from_le_bytes)
    };
    if d.len() < 8 {
        return None;
    }
    // Top-level records repeat CHID twice; grouped members contain it once.
    let base = if d[0..4] == d[4..8] { 8 } else { 4 };
    let cnt = rd_u16(base + 42)? as usize;
    // Linear part [a b; d e]: x'=a*x+b*y+c, y'=d*x+e*y+f.
    let mat = |o: usize| -> Option<(f64, f64, f64, f64)> {
        Some((rd_f64(o)?, rd_f64(o + 8)?, rd_f64(o + 24)?, rd_f64(o + 32)?))
    };
    let (ta, tb, td, te) = mat(base + 44)?;
    let pair = base + 44 + 48 + cnt.saturating_sub(1) * 96;
    let (a, dd) = if d.len() >= pair + 96 {
        // Compose only the linear part of m = translation * (scale * rotation).
        let (sa, sb, sd, se) = mat(pair)?;
        let (ra, rb, rd, re) = mat(pair + 48)?;
        let (ma, _mb, md, _me) = (
            sa * ra + sb * rd,
            sa * rb + sb * re,
            sd * ra + se * rd,
            sd * rb + se * re,
        );
        (ta * ma + tb * md, td * ma + te * md)
    } else {
        (ta, td)
    };
    let deg = dd.atan2(a).to_degrees();
    (deg.abs() >= 0.01).then_some(deg as f32)
}

/// HWP5 common object property (table 69) description BSTR.
///
/// `common` excludes the four-byte reversed control id, so the UTF-16 length
/// is at byte 40 and the payload starts at byte 42. Older/short or malformed
/// records remain losslessly available in `common_data`; they simply do not
/// expose a semantic description.
fn parse_gso_description(common: &[u8]) -> Option<String> {
    let len = usize::from(u16::from_le_bytes([*common.get(40)?, *common.get(41)?]));
    if len == 0 {
        return None;
    }
    let bytes = common.get(42..42usize.checked_add(len.checked_mul(2)?)?)?;
    let units = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    String::from_utf16(&units)
        .ok()
        .filter(|value| !value.is_empty())
}

fn parse_section_def(
    data: Vec<u8>,
    children: &[RecordNode],
    warnings: &mut Vec<String>,
) -> SectionDef {
    let mut def = SectionDef {
        data,
        page: None,
        extras: Vec::new(),
        // hwp5 출신 구역은 hwpx 원문 자식이 없다(writer가 기존 상수 템플릿 방출).
        secpr_raw_children: Vec::new(),
        footnote_shape_raw: None,
        endnote_shape_raw: None,
        page_border_fills_raw: Vec::new(),
    };
    for child in children {
        if child.tag == tag::PAGE_DEF {
            match parse_page_def(&child.data) {
                Ok(p) => def.page = Some(p),
                Err(e) => {
                    warnings.push(format!("PAGE_DEF 파싱 실패: {e}"));
                    def.extras.push(to_opaque(child));
                }
            }
        } else {
            // FOOTNOTE_SHAPE·PAGE_BORDER_FILL은 extras에 그대로 보존하면서(identity
            // 정본) 교차 변환용으로 raw 필드에 **병행** 사본을 둔다. 정품 실측 순서상
            // 첫 FOOTNOTE_SHAPE가 각주, 둘째가 미주다.
            if child.tag == tag::FOOTNOTE_SHAPE {
                if def.footnote_shape_raw.is_none() {
                    def.footnote_shape_raw = Some(child.data.clone());
                } else if def.endnote_shape_raw.is_none() {
                    def.endnote_shape_raw = Some(child.data.clone());
                }
            } else if child.tag == tag::PAGE_BORDER_FILL {
                def.page_border_fills_raw.push(child.data.clone());
            }
            def.extras.push(to_opaque(child));
        }
    }
    def
}

fn parse_page_def(data: &[u8]) -> Result<PageDef> {
    let mut r = ByteReader::new(data);
    Ok(PageDef {
        width: HwpUnit(r.read_i32()?),
        height: HwpUnit(r.read_i32()?),
        margin_left: HwpUnit(r.read_i32()?),
        margin_right: HwpUnit(r.read_i32()?),
        margin_top: HwpUnit(r.read_i32()?),
        margin_bottom: HwpUnit(r.read_i32()?),
        margin_header: HwpUnit(r.read_i32()?),
        margin_footer: HwpUnit(r.read_i32()?),
        gutter: HwpUnit(r.read_i32()?),
        attr: r.read_u32()?,
    })
}

fn parse_table(common_data: Vec<u8>, children: &[RecordNode], warnings: &mut Vec<String>) -> Table {
    let mut table = Table {
        common_data,
        placement: None,
        attr: 0,
        rows: 0,
        cols: 0,
        cell_spacing: 0,
        inner_margins: [0; 4],
        row_cell_counts: Vec::new(),
        border_fill: hwp_model::BorderFillId(0),
        table_tail: Vec::new(),
        cells: Vec::new(),
        caption: None,
        extras: Vec::new(),
    };
    let mut current_cell: Option<Cell> = None;
    let mut current_caption: Option<Caption> = None;
    // Distinguish caption and cell LIST_HEADER records using pyhwp's measured
    // ordering: a header before TABLE is a caption; later headers are cells.
    // A header after the declared cell count is also treated as a caption.
    let mut seen_table_record = false;
    let mut declared_cells: Option<usize> = None;

    for child in children {
        match child.tag {
            tag::TABLE => {
                if let Some(cap) = current_caption.take() {
                    table.caption = Some(cap);
                }
                seen_table_record = true;
                match parse_table_record(&child.data, &mut table) {
                    Ok(()) => {
                        declared_cells =
                            Some(table.row_cell_counts.iter().map(|&c| c as usize).sum());
                    }
                    Err(e) => {
                        warnings.push(format!("TABLE 레코드 파싱 실패: {e}"));
                        table.extras.push(to_opaque(child));
                    }
                }
            }
            tag::LIST_HEADER => {
                let filled = declared_cells.is_some_and(|d| {
                    d > 0 && table.cells.len() + usize::from(current_cell.is_some()) >= d
                });
                let is_caption = (!seen_table_record || filled)
                    && table.caption.is_none()
                    && current_caption.is_none();
                if is_caption {
                    if let Some(done) = current_cell.take() {
                        table.cells.push(done);
                    }
                    match parse_caption_header(&child.data) {
                        Ok(cap) => current_caption = Some(cap),
                        Err(e) => {
                            warnings.push(format!("캡션 LIST_HEADER 파싱 실패: {e}"));
                            table.extras.push(to_opaque(child));
                        }
                    }
                    continue;
                }
                if let Some(cap) = current_caption.take() {
                    table.caption = Some(cap);
                }
                if let Some(done) = current_cell.take() {
                    table.cells.push(done);
                }
                match parse_cell_header(&child.data) {
                    Ok(cell) => current_cell = Some(cell),
                    Err(e) => {
                        warnings.push(format!("셀 LIST_HEADER 파싱 실패: {e}"));
                        table.extras.push(to_opaque(child));
                    }
                }
            }
            tag::PARA_HEADER => {
                let para = parse_paragraph(child, warnings);
                if let Some(cap) = &mut current_caption {
                    cap.paragraphs.push(para);
                    continue;
                }
                match &mut current_cell {
                    Some(cell) => cell.paragraphs.push(para),
                    None => {
                        warnings.push("셀 밖의 문단 — LIST_HEADER 누락".to_string());
                        table.extras.push(to_opaque(child));
                    }
                }
            }
            _ => table.extras.push(to_opaque(child)),
        }
    }
    if let Some(cap) = current_caption.take() {
        table.caption = Some(cap);
    }
    if let Some(done) = current_cell.take() {
        table.cells.push(done);
    }
    // 개체 공통 속성(표 69)을 배치(GsoPlacement)로 승계한다 — hwpx write가
    // treatAsChar·sz·pos를 이 값에서 방출하도록. 하드코딩 treatAsChar=1(글자처럼)은
    // 원본이 부유(0)인 긴 표까지 "한 글자"로 배치해 페이지 분할을 막고 하단을 관통시킨다
    // (정답지 직대조 확정 — GE-8). hwp5→hwp5 재직렬화는 common_data 원문을 그대로 쓰므로
    // 무손실 왕복(identity)에는 영향이 없다.
    table.placement = parse_gso_common_placement(&table.common_data);
    table
}

/// 개체 공통 속성(표 69, ctrl_id 제외 페이로드)을 배치로 파싱한다.
/// 레이아웃: 속성 u32(표 70), 세로/가로 오프셋 i32, 폭/높이 i32, z-order i32,
/// 바깥 여백 u16×4. 24바이트 미만이면 None(합성·손상 — writer가 기본값 폴백).
fn parse_gso_common_placement(common: &[u8]) -> Option<hwp_model::GsoPlacement> {
    if common.len() < 24 {
        return None;
    }
    let mut r = ByteReader::new(common);
    let attr = r.read_u32().ok()?;
    let vert_offset = r.read_i32().ok()?;
    let horz_offset = r.read_i32().ok()?;
    let width = r.read_i32().ok()?;
    let height = r.read_i32().ok()?;
    let z_order = r.read_i32().ok()?;
    let out_margins = r.read_u16_array::<4>().unwrap_or([0; 4]);
    Some(hwp_model::GsoPlacement {
        treat_as_char: attr & 1 != 0,
        affect_line_spacing: (attr >> 2) & 1 != 0,
        flow_with_text: (attr >> 13) & 1 != 0,
        hold_anchor: false,
        vert_rel_to: ((attr >> 3) & 0x3) as u8,
        horz_rel_to: ((attr >> 8) & 0x3) as u8,
        vert_align: ((attr >> 5) & 0x7) as u8,
        horz_align: ((attr >> 10) & 0x7) as u8,
        vert_offset,
        horz_offset,
        z_order,
        width,
        height,
        out_margins,
    })
}

fn parse_table_record(data: &[u8], table: &mut Table) -> Result<()> {
    let mut r = ByteReader::new(data);
    table.attr = r.read_u32()?;
    table.rows = r.read_u16()?;
    table.cols = r.read_u16()?;
    table.cell_spacing = r.read_u16()?;
    table.inner_margins = r.read_u16_array::<4>()?;
    table.row_cell_counts = (0..table.rows)
        .map(|_| r.read_u16())
        .collect::<Result<_>>()?;
    table.border_fill = hwp_model::BorderFillId(r.read_u16()?);
    table.table_tail = r.take_rest().to_vec();
    Ok(())
}

/// 셀 LIST_HEADER: 문단 수 i32 + 속성 u32 + 셀 속성 (실측 46바이트 레이아웃).
fn parse_cell_header(data: &[u8]) -> Result<Cell> {
    let mut r = ByteReader::new(data);
    let _para_count = r.read_i32()?;
    let list_attr = r.read_u32()?;
    let col = r.read_u16()?;
    let row = r.read_u16()?;
    let col_span = r.read_u16()?;
    let row_span = r.read_u16()?;
    let width = HwpUnit(r.read_i32()?);
    let height = HwpUnit(r.read_i32()?);
    let margins = r.read_u16_array::<4>()?;
    let border_fill = hwp_model::BorderFillId(r.read_u16()?);
    Ok(Cell {
        list_attr,
        col,
        row,
        col_span,
        row_span,
        width,
        height,
        margins,
        border_fill,
        header_tail: r.take_rest().to_vec(),
        paragraphs: Vec::new(),
    })
}

fn parse_generic(
    ctrl_id: [u8; 4],
    data: Vec<u8>,
    children: &[RecordNode],
    warnings: &mut Vec<String>,
) -> GenericControl {
    // 다단 정의(cold): CTRL_HEADER 페이로드에서 ColumnDef 파싱(렌더러 단 배치용).
    let column_def = if &ctrl_id == b"cold" {
        parse_coldef(&data)
    } else {
        None
    };
    // 수식(eqed): 자식 EQEDIT 레코드의 스크립트 + 공통 속성(크기)로 Equation(렌더 조판용).
    let equation = if &ctrl_id == b"eqed" {
        parse_eqed(&data, children)
    } else {
        None
    };
    // A direct GSO LIST_HEADER is a caption (table 71, pyhwp
    // GShapeObjectCaption). Keep its range out of ordinary text-box lists.
    let (caption, caption_range) = if &ctrl_id == b"gso " {
        parse_direct_gso_caption(children, warnings)
    } else {
        (None, None)
    };
    let mut g = GenericControl {
        ctrl_id,
        data,
        paragraph_lists: Vec::new(),
        extras: Vec::new(),
        // 원본 자식 서브트리를 중첩 그대로 보존 → 무손실 재직렬화.
        raw_children: children.iter().map(to_opaque).collect(),
        gso_shapes: Vec::new(),
        equation,
        column_def,
        caption,
        hwpx_raw_xml: None,
    };
    if let Some((start, end)) = caption_range {
        collect_paragraph_lists(&children[..start], &mut g, warnings);
        collect_paragraph_lists(&children[end..], &mut g, warnings);
    } else {
        collect_paragraph_lists(children, &mut g, warnings);
    }
    g
}

fn parse_direct_gso_caption(
    children: &[RecordNode],
    warnings: &mut Vec<String>,
) -> (Option<Caption>, Option<(usize, usize)>) {
    let Some(start) = children
        .iter()
        .position(|child| child.tag == tag::LIST_HEADER)
    else {
        return (None, None);
    };
    let mut caption = match parse_caption_header(&children[start].data) {
        Ok(caption) => caption,
        Err(error) => {
            warnings.push(format!("Failed to parse caption LIST_HEADER: {error}"));
            return (None, None);
        }
    };
    let mut end = start + 1;
    while let Some(child) = children
        .get(end)
        .filter(|child| child.tag == tag::PARA_HEADER)
    {
        caption.paragraphs.push(parse_paragraph(child, warnings));
        end += 1;
    }
    (Some(caption), Some((start, end)))
}

/// COLDEF(cold) CTRL_HEADER 페이로드 → ColumnDef. 실측(다단정답지.hwp): `08 10`=attr 0x1008
/// (bit0-1 종류·bit2-9 단수·bit10-11 방향·bit12 동일폭) + `dc 08`=gap 2268(HWPUNIT16) + 8B
/// 구분선(현재 표본 0). hwplib ControlColumnDefine과 bit단위 일치.
/// 수식(eqed) → Equation. 공통 속성(gso, data): attr@0·voff@4·hoff@8·width@12·height@16.
/// 스크립트는 자식 EQEDIT 레코드(태그 0x58): attr(4) + len(2) + WCHAR[len] script(실측 확인).
fn parse_eqed(data: &[u8], children: &[RecordNode]) -> Option<Equation> {
    let script = find_eqedit_script(children)?;
    let rd = |o: usize| -> i32 {
        data.get(o..o + 4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .unwrap_or(0)
    };
    let attr = rd(0) as u32;
    Some(Equation {
        script,
        width: rd(12).max(0),
        height: rd(16).max(0),
        inline: attr & 1 == 1, // bit0 = 글자처럼 취급
        x: rd(8),              // hoff
        y: rd(4),              // voff
        // hwpx 원문 pass-through는 hwpx 출신 전용 — hwp5 출신은 배치를 gso 공통 헤더
        // (`GenericControl::data`)에서 그대로 읽을 수 있어 hwpx writer가 재구성한다.
        ..Equation::default()
    })
}

/// 서브트리에서 EQEDIT(0x58) 레코드의 수식 스크립트를 찾는다.
fn find_eqedit_script(children: &[RecordNode]) -> Option<String> {
    const HWPTAG_EQEDIT: u16 = 0x58;
    for c in children {
        if c.tag == HWPTAG_EQEDIT && c.data.len() >= 6 {
            let len = u16::from_le_bytes([c.data[4], c.data[5]]) as usize;
            let end = (6 + len * 2).min(c.data.len());
            let units: Vec<u16> = c.data[6..end]
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            return Some(String::from_utf16_lossy(&units));
        }
        if let Some(s) = find_eqedit_script(&c.children) {
            return Some(s);
        }
    }
    None
}

fn parse_coldef(data: &[u8]) -> Option<ColumnDef> {
    if data.len() < 4 {
        return None;
    }
    let attr = u16::from_le_bytes([data[0], data[1]]);
    let gap = i16::from_le_bytes([data[2], data[3]]) as i32;
    Some(ColumnDef {
        count: ((attr >> 2) & 0xFF).max(1),
        kind: (attr & 0x3) as u8,
        direction: ((attr >> 10) & 0x3) as u8,
        same_width: (attr >> 12) & 1 != 0,
        gap,
        widths: Vec::new(),
        // TODO(GG-17): divider type/width/color offsets remain unknown. Tables
        // 138/139 specify 14 bytes, but observed E-5 payloads use 16 bytes and
        // all fixtures are single-column with zero divider bytes. Parse only
        // the HWPX representation until a ground-truth HWP5 sample exists.
        divider: None,
    })
}

/// 문단 리스트를 재귀 수집한다.
///
/// 글상자/도형은 CTRL_HEADER(gso) → SHAPE_COMPONENT → LIST_HEADER처럼
/// 컨테이너 레코드 한 단계 아래에 문단을 두므로(실측), 문단을 포함하는
/// 서브트리는 내려가며 수집한다. 이때 GenericControl의 IR은 원본 중첩
/// 구조를 평탄화한다 — 정확한 재직렬화는 L0 바이패스 경로의 몫이다.
fn collect_paragraph_lists(
    children: &[RecordNode],
    g: &mut GenericControl,
    warnings: &mut Vec<String>,
) {
    for child in children {
        match child.tag {
            tag::LIST_HEADER => {
                g.paragraph_lists.push(ParagraphList {
                    header_data: child.data.clone(),
                    paragraphs: Vec::new(),
                });
                // LIST_HEADER가 자식으로 문단을 갖는 변형도 방어
                collect_paragraph_lists(&child.children, g, warnings);
            }
            tag::PARA_HEADER => {
                let para = parse_paragraph(child, warnings);
                if g.paragraph_lists.is_empty() {
                    // LIST_HEADER 없이 문단이 오는 변형 방어
                    g.paragraph_lists.push(ParagraphList {
                        header_data: Vec::new(),
                        paragraphs: Vec::new(),
                    });
                }
                g.paragraph_lists
                    .last_mut()
                    .expect("위에서 보장")
                    .paragraphs
                    .push(para);
            }
            _ if subtree_has_paragraphs(child) => {
                // 컨테이너(SHAPE_COMPONENT 등): 페이로드는 보존하고 자식으로 재귀
                g.extras.push(hwp_model::OpaqueRecord {
                    tag: child.tag,
                    data: child.data.clone(),
                    children: Vec::new(),
                });
                collect_paragraph_lists(&child.children, g, warnings);
            }
            _ => g.extras.push(to_opaque(child)),
        }
    }
}

fn subtree_has_paragraphs(node: &RecordNode) -> bool {
    node.children.iter().any(|c| {
        c.tag == tag::PARA_HEADER || c.tag == tag::LIST_HEADER || subtree_has_paragraphs(c)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GE-9: 부유 그림의 세로/가로 오프셋·z-order를 개체 공통 속성(표 69)에서
    /// 실값으로 승계한다(이전엔 전부 0 하드코딩 → 좌상단 뭉침).
    #[test]
    fn 부유_그림_배치_승계() {
        let mut common = Vec::new();
        common.extend_from_slice(&0u32.to_le_bytes()); // attr: 글자처럼=0(부유)
        common.extend_from_slice(&5000i32.to_le_bytes()); // 세로 offset
        common.extend_from_slice(&3000i32.to_le_bytes()); // 가로 offset
        common.extend_from_slice(&10000i32.to_le_bytes()); // 폭
        common.extend_from_slice(&8000i32.to_le_bytes()); // 높이
        common.extend_from_slice(&7i32.to_le_bytes()); // z-order
        common.extend_from_slice(&[0u8; 8]); // 바깥 여백 4×u16

        // 그림 레코드: 71B 스킵 후 bin_id(u16).
        let mut pic_data = vec![0u8; 71];
        pic_data.extend_from_slice(&3u16.to_le_bytes());
        let pic_node = RecordNode {
            tag: tag::SHAPE_COMPONENT_PICTURE,
            data: pic_data,
            children: Vec::new(),
        };

        let p = parse_picture_gso(&common, &[pic_node], &mut Vec::new()).unwrap();
        assert!(!p.treat_as_char, "부유(글자처럼=false)");
        assert_eq!(p.vert_offset, 5000, "세로 offset 승계");
        assert_eq!(p.horz_offset, 3000, "가로 offset 승계");
        assert_eq!(p.z_order, 7, "z-order 승계");
        assert_eq!(p.width.0, 10000);
        assert_eq!(p.height.0, 8000);
        assert_eq!(p.bin_ref, hwp_model::BinRef::Id(hwp_model::BinDataId(3)));
    }

    #[test]
    fn gso_설명_utf16_코드유닛_파싱() {
        let expected = "제목😀\n\n설명";
        let encoded = expected.encode_utf16().collect::<Vec<_>>();
        let mut common = vec![0u8; 40];
        common.extend_from_slice(&(encoded.len() as u16).to_le_bytes());
        for unit in encoded {
            common.extend_from_slice(&unit.to_le_bytes());
        }

        assert_eq!(parse_gso_description(&common).as_deref(), Some(expected));
        assert_eq!(
            u16::from_le_bytes(common[40..42].try_into().unwrap()),
            8,
            "astral 문자는 UTF-16 코드 유닛 2개로 계산"
        );
        assert_eq!(parse_gso_description(&common[..common.len() - 1]), None);
        assert_eq!(parse_gso_description(&common[..40]), None);
    }

    /// GG-15: promotes crop, brightness, contrast, effect flags, and the
    /// SHAPE_COMPONENT rotation matrix into the IR.
    #[test]
    fn 그림_변환_보정_속성_파싱() {
        let mut common = Vec::new();
        common.extend_from_slice(&1u32.to_le_bytes()); // attr: 글자처럼
        common.extend_from_slice(&[0u8; 20]); // offsets/폭/높이/z-order

        // A 91-byte picture record containing crop, adjustments, bin ID, and effect flags.
        let mut pic_data = vec![0u8; 91];
        for (o, v) in [(44, 100i32), (48, 200), (52, 4100), (56, 3200)] {
            pic_data[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        pic_data[68] = 30u8; // 밝기 +30
        pic_data[69] = (-20i8) as u8; // 명암 -20
        pic_data[71..73].copy_from_slice(&3u16.to_le_bytes());
        pic_data[78..82].copy_from_slice(&0x3u32.to_le_bytes()); // 효과 플래그
        let pic_node = RecordNode {
            tag: tag::SHAPE_COMPONENT_PICTURE,
            data: pic_data,
            children: Vec::new(),
        };
        // A 196-byte SHAPE_COMPONENT with repeated CHID, properties, and a 30-degree rotation.
        let mut sc_data = vec![0u8; 196];
        sc_data[0..4].copy_from_slice(b"cip$");
        sc_data[4..8].copy_from_slice(b"cip$");
        sc_data[50..52].copy_from_slice(&1u16.to_le_bytes()); // cnt=1
        // Translation and scale are identity matrices.
        for base in [52usize, 100] {
            sc_data[base..base + 8].copy_from_slice(&1.0f64.to_le_bytes());
            sc_data[base + 32..base + 40].copy_from_slice(&1.0f64.to_le_bytes());
        }
        let t = 30.0f64.to_radians();
        for (o, v) in [
            (148, t.cos()),
            (156, -t.sin()),
            (172, t.sin()),
            (180, t.cos()),
        ] {
            sc_data[o..o + 8].copy_from_slice(&v.to_le_bytes());
        }
        let sc_node = RecordNode {
            tag: tag::SHAPE_COMPONENT,
            data: sc_data,
            children: Vec::new(),
        };

        let p = parse_picture_gso(&common, &[sc_node, pic_node], &mut Vec::new()).unwrap();
        assert_eq!(p.crop, Some([100.0, 200.0, 4100.0, 3200.0]));
        assert_eq!(p.brightness, 30);
        assert_eq!(p.contrast, -20);
        assert_eq!(p.effect_flags, 0x3);
        assert_eq!(p.effects_raw.len(), 91 - 78, "효과 스트림 @78~끝 보존");
        let rot = p.rotation.expect("회전 각도 승계");
        assert!((rot - 30.0).abs() < 0.01, "30° 분해, 실제 {rot}");
    }

    /// An identity rotation matrix produces `None`.
    #[test]
    fn 회전없음이면_none() {
        let mut sc_data = vec![0u8; 196];
        sc_data[0..4].copy_from_slice(b"cip$");
        sc_data[4..8].copy_from_slice(b"cip$");
        sc_data[50..52].copy_from_slice(&1u16.to_le_bytes());
        for base in [52usize, 100, 148] {
            sc_data[base..base + 8].copy_from_slice(&1.0f64.to_le_bytes());
            sc_data[base + 32..base + 40].copy_from_slice(&1.0f64.to_le_bytes());
        }
        assert_eq!(gso_rotation_deg(&sc_data), None);
        assert_eq!(gso_rotation_deg(&[]), None);
    }

    /// Synthesizes a 22-byte caption LIST_HEADER (tables 71-73).
    fn caption_header_bytes(side: u32, full_size: bool, gap: u16, max_width: i32) -> Vec<u8> {
        let flags = side | u32::from(full_size) << 2;
        let mut d = Vec::new();
        d.extend_from_slice(&1i32.to_le_bytes()); // 문단 수
        d.extend_from_slice(&0u32.to_le_bytes()); // listflags: 가로
        d.extend_from_slice(&flags.to_le_bytes()); // 캡션 속성 (표 73)
        d.extend_from_slice(&0i32.to_le_bytes()); // 캡션 폭 (가로 방향이라 미사용)
        d.extend_from_slice(&gap.to_le_bytes()); // 캡션과 틀 사이 간격
        d.extend_from_slice(&max_width.to_le_bytes()); // 텍스트 최대 길이(=개체 폭)
        d
    }

    fn para_header_node() -> RecordNode {
        RecordNode {
            tag: tag::PARA_HEADER,
            data: vec![0u8; 22],
            children: Vec::new(),
        }
    }

    /// GB-13: LIST_HEADER before TABLE is a caption; one after TABLE is a cell.
    #[test]
    fn 표_캡션_리스트헤더_판별() {
        // 1x1 TABLE: attr, rows, cols, spacing, margins, row cell count, border fill.
        let mut tbl = Vec::new();
        tbl.extend_from_slice(&0u32.to_le_bytes());
        tbl.extend_from_slice(&1u16.to_le_bytes()); // rows
        tbl.extend_from_slice(&1u16.to_le_bytes()); // cols
        tbl.extend_from_slice(&0u16.to_le_bytes()); // cellspacing
        tbl.extend_from_slice(&[0u8; 8]); // 안쪽 여백
        tbl.extend_from_slice(&1u16.to_le_bytes()); // 행별 셀 수
        tbl.extend_from_slice(&1u16.to_le_bytes()); // border fill id

        // 34-byte cell LIST_HEADER with position, spans, dimensions, and margins.
        let mut cell = Vec::new();
        cell.extend_from_slice(&1i32.to_le_bytes());
        cell.extend_from_slice(&0u32.to_le_bytes());
        cell.extend_from_slice(&[0u8; 8]); // col/row/colspan/rowspan
        cell.extend_from_slice(&1000i32.to_le_bytes());
        cell.extend_from_slice(&500i32.to_le_bytes());
        cell.extend_from_slice(&[0u8; 8]); // 여백
        cell.extend_from_slice(&1u16.to_le_bytes()); // border fill

        let children = vec![
            RecordNode {
                tag: tag::LIST_HEADER,
                data: caption_header_bytes(3, false, 850, 42520), // bottom
                children: Vec::new(),
            },
            para_header_node(), // 캡션 문단
            RecordNode {
                tag: tag::TABLE,
                data: tbl,
                children: Vec::new(),
            },
            RecordNode {
                tag: tag::LIST_HEADER,
                data: cell,
                children: Vec::new(),
            },
            para_header_node(), // 셀 문단
        ];

        let mut warnings = Vec::new();
        let table = parse_table(Vec::new(), &children, &mut warnings);
        let caption = table.caption.expect("캡션 파싱");
        assert_eq!(caption.side, CaptionSide::Bottom);
        assert_eq!(caption.direction, CaptionDirection::Horizontal);
        assert_eq!(caption.gap, 850);
        assert_eq!(caption.width, Some(0));
        assert_eq!(caption.last_width, 42520);
        assert_eq!(caption.paragraphs.len(), 1, "캡션 문단 수");
        assert_eq!(table.cells.len(), 1, "셀은 1개만");
        assert_eq!(table.cells[0].paragraphs.len(), 1, "셀 문단 수");
    }

    /// A full-size caption (bit 2) maps to width=None.
    #[test]
    fn 캡션_fullsize_폭_매핑() {
        let cap = parse_caption_header(&caption_header_bytes(0, true, 100, 0)).unwrap();
        assert_eq!(cap.side, CaptionSide::Left);
        assert_eq!(cap.width, None);
        let cap = parse_caption_header(&caption_header_bytes(2, false, 100, 0)).unwrap();
        assert_eq!(cap.side, CaptionSide::Top);
        assert_eq!(cap.width, Some(0));
    }

    /// GB-13: direct picture-GSO caption records are separated from extras.
    #[test]
    fn 그림_캡션_분리() {
        let mut common = Vec::new();
        common.extend_from_slice(&1u32.to_le_bytes()); // attr: 글자처럼
        common.extend_from_slice(&[0u8; 16]); // 오프셋/폭/높이
        common.extend_from_slice(&[0u8; 4]); // z-order
        common.extend_from_slice(&[0u8; 8]); // 바깥 여백

        let mut pic_data = vec![0u8; 71];
        pic_data.extend_from_slice(&2u16.to_le_bytes()); // bin id
        let pic_node = RecordNode {
            tag: tag::SHAPE_COMPONENT_PICTURE,
            data: pic_data,
            children: Vec::new(),
        };
        let children = vec![
            RecordNode {
                tag: tag::LIST_HEADER,
                data: caption_header_bytes(3, false, 600, 9000),
                children: Vec::new(),
            },
            para_header_node(),
            pic_node,
        ];

        let p = parse_picture_gso(&common, &children, &mut Vec::new()).unwrap();
        let caption = p.caption.expect("그림 캡션");
        assert_eq!(caption.side, CaptionSide::Bottom);
        assert_eq!(caption.gap, 600);
        assert_eq!(caption.paragraphs.len(), 1);
        assert_eq!(
            p.extras.len(),
            1,
            "캡션 레코드는 extras에서 빠지고 그림 레코드만 남는다"
        );
        assert_eq!(p.extras[0].tag, tag::SHAPE_COMPONENT_PICTURE);
    }

    #[test]
    fn direct_gso_caption_is_not_collected_as_text_box_content() {
        let nested_text_box = RecordNode {
            tag: tag::SHAPE_COMPONENT,
            data: Vec::new(),
            children: vec![
                RecordNode {
                    tag: tag::LIST_HEADER,
                    data: Vec::new(),
                    children: Vec::new(),
                },
                para_header_node(),
            ],
        };
        let children = vec![
            RecordNode {
                tag: tag::LIST_HEADER,
                data: caption_header_bytes(3, false, 600, 9000),
                children: Vec::new(),
            },
            para_header_node(),
            nested_text_box,
        ];

        let generic = parse_generic(*b"gso ", Vec::new(), &children, &mut Vec::new());
        assert_eq!(generic.caption.as_ref().unwrap().paragraphs.len(), 1);
        assert_eq!(generic.paragraph_lists.len(), 1);
        assert_eq!(generic.paragraph_lists[0].paragraphs.len(), 1);
    }
}
