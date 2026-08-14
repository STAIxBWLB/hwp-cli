//! `Contents/sectionN.xml` → [`Section`].
//!
//! 재귀 하강 파서: `hp:p`는 표 셀(`hp:subList`) 안에 다시 나타나므로
//! 각 파서 함수가 자신의 닫는 태그까지 소비한다.
//!
//! IR 일치 규칙 (hwp5와 동일 의미):
//! - `hp:secPr` → ExtCtrl(2, "secd") + Control::SectionDef
//! - `hp:ctrl > hp:colPr` → ExtCtrl(2, "cold") + Control::Generic
//! - `hp:tbl` → ExtCtrl(11, "tbl ") + Control::Table
//! - 기타 개체(pic/rect/...) → ExtCtrl(11) + Control::Generic
//!   (`hp:subList` 문단은 텍스트 추출을 위해 재귀 수집)

use hwp_model::opaque::OpaqueRecord;
use hwp_model::{
    Caption, CaptionDirection, CaptionSide, Cell, CharShapeId, ColumnDef, Control, Equation,
    GenericControl, GradientSpec, HwpChar, HwpUnit, LineSeg, PageDef, ParaShapeId, Paragraph,
    ParagraphList, SECPR_PAGEPR_SLOT, Section, SectionDef, ShapeGeom, ShapeKind, StyleId, Table,
};
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};

use crate::error::{HwpxError, Result};
use crate::read::xml::{attr, attr_i32, attr_offset_i32, attr_u16, attr_u32, parse_color};

type XmlReader<'a> = Reader<&'a [u8]>;

fn next_event<'a>(reader: &mut XmlReader<'a>) -> Result<Event<'a>> {
    reader.read_event().map_err(|e| HwpxError::Xml {
        entry: "section".to_string(),
        message: e.to_string(),
    })
}

/// 자신의 닫는 태그까지 서브트리를 소비한다 (관심 없는 요소 건너뛰기).
fn skip_subtree(reader: &mut XmlReader<'_>, name: &[u8]) -> Result<()> {
    let mut depth = 1u32;
    loop {
        match next_event(reader)? {
            Event::Start(e) if e.local_name().as_ref() == name => depth += 1,
            Event::End(e) if e.local_name().as_ref() == name => {
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

pub fn parse_section(xml: &str) -> Result<(Section, Vec<String>)> {
    let mut reader = Reader::from_str(xml);
    let mut section = Section::default();
    let mut warnings = Vec::new();

    loop {
        match next_event(&mut reader)? {
            Event::Start(e) if e.local_name().as_ref() == b"p" => {
                section
                    .paragraphs
                    .push(parse_paragraph(&mut reader, &e, &mut warnings)?);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok((section, warnings))
}

/// `<hp:p>` 하나를 소비한다.
fn parse_paragraph(
    reader: &mut XmlReader<'_>,
    start: &BytesStart<'_>,
    warnings: &mut Vec<String>,
) -> Result<Paragraph> {
    let mut para = Paragraph {
        para_shape: ParaShapeId(attr_u16(start, "paraPrIDRef").unwrap_or(0)),
        style: StyleId(attr_u16(start, "styleIDRef").unwrap_or(0)),
        ..Paragraph::default()
    };
    // hwp5 break_type 비트와 동일 인코딩 (bit2 쪽, bit3 단)
    if attr(start, "pageBreak").as_deref() == Some("1") {
        para.header.break_type |= 0x04;
    }
    if attr(start, "columnBreak").as_deref() == Some("1") {
        para.header.break_type |= 0x08;
    }
    let mut wchar_pos = 0u32;
    let mut last_shape: Option<u16> = None;

    loop {
        let event = next_event(reader)?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                let empty = matches!(event, Event::Empty(_));
                let name = e.local_name().as_ref().to_vec();
                match name.as_slice() {
                    b"run" => {
                        let id = attr_u16(e, "charPrIDRef").unwrap_or(0);
                        if last_shape != Some(id) {
                            // 직전 run이 글자를 안 더했으면(빈 <hp:t/>) 같은 위치에
                            // 두 글자모양이 겹친다. HWP5 PARA_CHAR_SHAPE는 위치당 1개
                            // (마지막이 유효)이므로 같은 위치면 마지막 run으로 덮어쓴다.
                            // (빈 문단의 빈 run 2개 → 첫 run의 큰 글자로 줄이 높아져
                            // 페이지가 밀리는 문제 방지.)
                            if let Some(last) = para.char_shape_runs.last_mut()
                                && last.0 == wchar_pos
                            {
                                last.1 = CharShapeId(id);
                            } else {
                                para.char_shape_runs.push((wchar_pos, CharShapeId(id)));
                            }
                            last_shape = Some(id);
                        }
                    }
                    b"t" => {
                        if !empty {
                            parse_text(reader, &mut para, &mut wchar_pos, warnings)?;
                        }
                    }
                    b"tab" => {
                        para.chars.push(HwpChar::InlineCtrl {
                            code: 9,
                            payload: vec![0; 12],
                        });
                        wchar_pos += 8;
                        if !empty {
                            skip_subtree(reader, b"tab")?;
                        }
                    }
                    b"lineBreak" => {
                        para.chars.push(HwpChar::CharCtrl(10));
                        wchar_pos += 1;
                    }
                    b"secPr" => {
                        let def = if empty {
                            SectionDef {
                                data: Vec::new(),
                                page: None,
                                extras: Vec::new(),
                                secpr_raw_children: Vec::new(),
                                footnote_shape_raw: None,
                                endnote_shape_raw: None,
                                page_border_fills_raw: Vec::new(),
                            }
                        } else {
                            parse_sec_pr(reader)?
                        };
                        push_ext_ctrl(&mut para, &mut wchar_pos, 2, *b"secd");
                        para.controls.push(Control::SectionDef(def));
                    }
                    b"ctrl" => {
                        if !empty {
                            parse_ctrl(reader, &mut para, &mut wchar_pos, warnings)?;
                        }
                    }
                    b"tbl" => {
                        let table = parse_table(reader, e, warnings)?;
                        push_ext_ctrl(&mut para, &mut wchar_pos, 11, *b"tbl ");
                        para.controls.push(Control::Table(table));
                    }
                    b"equation" => {
                        let eq = parse_equation(reader, e, empty)?;
                        push_ext_ctrl(&mut para, &mut wchar_pos, 11, *b"eqed");
                        para.controls.push(Control::Generic(GenericControl {
                            ctrl_id: *b"eqed",
                            data: Vec::new(),
                            paragraph_lists: Vec::new(),
                            extras: Vec::new(),
                            raw_children: Vec::new(),
                            gso_shapes: Vec::new(),
                            equation: Some(eq),
                            column_def: None,
                            caption: None,
                            hwpx_raw_xml: None,
                        }));
                    }
                    b"linesegarray" => {
                        if !empty {
                            parse_linesegs(reader, &mut para)?;
                        }
                    }
                    b"pic" => {
                        let mut picture = if empty {
                            default_picture()
                        } else {
                            parse_picture(reader, warnings)?
                        };
                        // z-순서는 <hp:pic> 시작 태그 속성(자식 <hp:pos>가 아님).
                        // 누락하면 머리말/본문 로고 겹침 순서가 어긋난다.
                        picture.z_order = attr_i32(e, "zOrder").unwrap_or(0).max(0) as u32;
                        push_ext_ctrl(&mut para, &mut wchar_pos, 11, *b"gso ");
                        para.controls.push(Control::Picture(picture));
                    }
                    // 그 외 개체 (rect, ellipse, line, polygon, curve, equation, container...)
                    _ => {
                        let mut ctrl_id = [b' '; 4];
                        for (i, b) in name.iter().take(4).enumerate() {
                            ctrl_id[i] = *b;
                        }
                        let mut generic = GenericControl {
                            ctrl_id,
                            data: Vec::new(),
                            paragraph_lists: Vec::new(),
                            extras: Vec::new(),
                            raw_children: Vec::new(),
                            gso_shapes: Vec::new(),
                            equation: None,
                            column_def: None,
                            caption: None,
                            hwpx_raw_xml: None,
                        };
                        if let Some(kind) = shape_kind(&name) {
                            // 도형은 의미 파싱(gso_shapes) 경로를 유지한다(원문 캡처 없음).
                            if !empty {
                                // 둥근 사각형: <hp:rect ratio="N"> (모서리 곡률 %).
                                let round_ratio = if kind == ShapeKind::Rect {
                                    attr_i32(e, "ratio").unwrap_or(0).clamp(0, 100) as u8
                                } else {
                                    0
                                };
                                collect_shape(
                                    reader,
                                    &name,
                                    kind,
                                    round_ratio,
                                    &mut generic,
                                    warnings,
                                )?;
                            }
                        } else {
                            // 미해석 개체(hp:container 등)는 원문 XML을 통째로 캡처해
                            // 무손실 재직렬화에 쓰고, subList/caption은 텍스트 추출·
                            // 렌더용으로 캡처본 위에서 다시 수집한다(capture_element가
                            // 원본 reader의 서브트리를 소비하므로 순서가 바뀌면 안 된다).
                            let raw = capture_element(reader, e, empty)?;
                            if !empty {
                                let mut raw_reader = Reader::from_str(&raw);
                                // 캡처본의 첫 이벤트(개체 자신의 여는 태그)를 소비한다.
                                let _ = next_event(&mut raw_reader)?;
                                collect_sub_lists(
                                    &mut raw_reader,
                                    &name,
                                    &mut generic,
                                    warnings,
                                )?;
                            }
                            generic.hwpx_raw_xml = Some(raw);
                        }
                        push_ext_ctrl(&mut para, &mut wchar_pos, 11, ctrl_id);
                        para.controls.push(Control::Generic(generic));
                    }
                }
            }
            Event::End(e) if e.local_name().as_ref() == b"p" => break,
            Event::Eof => {
                warnings.push("hp:p가 닫히지 않은 채 EOF".to_string());
                break;
            }
            _ => {}
        }
    }
    Ok(para)
}

/// `<hp:t>` 텍스트 문자 하나를 IR로 적재한다.
///
/// raw 탭(0x09)은 과거 오염 파일(탭이 Text로 잘못 저장된 hwpx) 하위호환을 위해
/// `InlineCtrl(9)`로 정규화한다(§3.2.3 표 6: 탭 = 8 WCHAR 인라인 컨트롤 — IR 불변식).
/// 그 외 C0 제어문자는 제거한다(문서 손상 방지; 강제 줄바꿈은 `<hp:lineBreak/>` 요소로
/// 별도 도착한다). 일반 문자는 `Text`로 적재한다.
fn push_text_char(para: &mut Paragraph, wchar_pos: &mut u32, c: char) {
    match c {
        '\t' => {
            para.chars.push(HwpChar::InlineCtrl {
                code: 9,
                payload: vec![0; 12],
            });
            *wchar_pos += 8;
        }
        c if (c as u32) < 0x20 => {}
        c => {
            *wchar_pos += c.len_utf16() as u32;
            para.chars.push(HwpChar::Text(c));
        }
    }
}

/// `<hp:t>` 내부의 텍스트를 수집한다 (중첩 마크업은 무시).
fn parse_text(
    reader: &mut XmlReader<'_>,
    para: &mut Paragraph,
    wchar_pos: &mut u32,
    _warnings: &mut [String],
) -> Result<()> {
    loop {
        match next_event(reader)? {
            Event::Text(t) => {
                let s = t.xml10_content().map_err(|e| HwpxError::Xml {
                    entry: "section".to_string(),
                    message: e.to_string(),
                })?;
                for c in s.chars() {
                    push_text_char(para, wchar_pos, c);
                }
            }
            // 엔티티 참조(&amp; &#x...;)는 별도 이벤트로 온다
            Event::GeneralRef(r) => {
                if let Some(c) = resolve_entity(&r) {
                    push_text_char(para, wchar_pos, c);
                }
            }
            // <hp:t> 안의 강제 줄바꿈(정품 한글 구조: `앞<hp:lineBreak/>뒤`).
            Event::Empty(e) | Event::Start(e) if e.local_name().as_ref() == b"lineBreak" => {
                *wchar_pos += 1;
                para.chars.push(HwpChar::CharCtrl(10));
            }
            // <hp:t> 안의 중첩 탭(정품 mixed content: `앞<hp:tab width leader type/>뒤`)
            // → InlineCtrl(9). 속성값은 한글이 재계산·재유도하므로 보존하지 않고 payload는
            // 0으로 둔다(왕복 시 writer가 문단 탭 정의에서 종류/채움을 다시 뽑는다). 구형
            // bare 탭(hp:run 직속 <hp:tab/>)은 parse_paragraph의 b"tab" arm이 처리(하위호환).
            Event::Empty(e) | Event::Start(e) if e.local_name().as_ref() == b"tab" => {
                para.chars.push(HwpChar::InlineCtrl {
                    code: 9,
                    payload: vec![0; 12],
                });
                *wchar_pos += 8;
            }
            Event::End(e) if e.local_name().as_ref() == b"t" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

fn push_ext_ctrl(para: &mut Paragraph, wchar_pos: &mut u32, code: u16, ctrl_id: [u8; 4]) {
    // payload 선두 4바이트 = 역순 ctrl_id (hwp5 저장 형식과 동일하게 구성)
    let mut payload = vec![0u8; 12];
    payload[..4].copy_from_slice(&{
        let mut rev = ctrl_id;
        rev.reverse();
        rev
    });
    let ctrl_index = Some(para.controls.len() as u32);
    para.chars.push(HwpChar::ExtCtrl {
        code,
        ctrl_id,
        payload,
        ctrl_index,
    });
    *wchar_pos += 8;
}

/// `<hp:secPr>` — pagePr/margin은 의미 파싱하고, 그 외 자식(grid/startNum/visibility/
/// lineNumberShape/footNotePr/endNotePr/pageBorderFill 등)은 **원문 XML을 등장 순서대로**
/// 보존한다(저비용 pass-through). pagePr 자리는 [`SECPR_PAGEPR_SLOT`] 마커로 표시해
/// writer가 페이지 정의에서 재생성한 pagePr을 같은 위치에 방출하게 한다.
fn parse_sec_pr(reader: &mut XmlReader<'_>) -> Result<SectionDef> {
    let mut def = SectionDef {
        data: Vec::new(),
        page: None,
        extras: Vec::new(),
        secpr_raw_children: Vec::new(),
        footnote_shape_raw: None,
        endnote_shape_raw: None,
        page_border_fills_raw: Vec::new(),
    };
    let mut page = PageDef {
        width: HwpUnit(0),
        height: HwpUnit(0),
        margin_left: HwpUnit(0),
        margin_right: HwpUnit(0),
        margin_top: HwpUnit(0),
        margin_bottom: HwpUnit(0),
        margin_header: HwpUnit(0),
        margin_footer: HwpUnit(0),
        gutter: HwpUnit(0),
        attr: 0,
    };
    let mut has_page = false;

    loop {
        let event = next_event(reader)?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                let empty = matches!(event, Event::Empty(_));
                match e.local_name().as_ref() {
                    b"pagePr" => {
                        // pagePr은 의미 파싱(페이지 정의). 자식 margin은 pagePr 서브트리
                        // 안에서 읽고, 순서 표시용 마커를 원문 목록에 남긴다.
                        has_page = true;
                        page.width = HwpUnit(attr_i32(e, "width").unwrap_or(0));
                        page.height = HwpUnit(attr_i32(e, "height").unwrap_or(0));
                        // OWPML landscape="NARROWLY"(세로)/"WIDELY"(가로) — 가로면 bit0
                        if attr(e, "landscape").as_deref() == Some("NARROWLY") {
                            page.attr |= 1;
                        }
                        if !empty {
                            parse_page_pr_children(reader, &mut page)?;
                        }
                        def.secpr_raw_children.push(SECPR_PAGEPR_SLOT.to_string());
                    }
                    // 그 외 자식은 서브트리 원문을 그대로 캡처(의미 미해석 — 무손실 왕복).
                    _ => {
                        def.secpr_raw_children
                            .push(capture_element(reader, e, empty)?);
                    }
                }
            }
            Event::End(e) if e.local_name().as_ref() == b"secPr" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    if has_page {
        def.page = Some(page);
    }
    Ok(def)
}

/// `<hp:pagePr>` 내부의 `<hp:margin>`을 읽어 페이지 여백을 채운다(pagePr 닫힐 때까지).
fn parse_page_pr_children(reader: &mut XmlReader<'_>, page: &mut PageDef) -> Result<()> {
    loop {
        match next_event(reader)? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"margin" => {
                page.margin_left = HwpUnit(attr_i32(&e, "left").unwrap_or(0));
                page.margin_right = HwpUnit(attr_i32(&e, "right").unwrap_or(0));
                page.margin_top = HwpUnit(attr_i32(&e, "top").unwrap_or(0));
                page.margin_bottom = HwpUnit(attr_i32(&e, "bottom").unwrap_or(0));
                page.margin_header = HwpUnit(attr_i32(&e, "header").unwrap_or(0));
                page.margin_footer = HwpUnit(attr_i32(&e, "footer").unwrap_or(0));
                page.gutter = HwpUnit(attr_i32(&e, "gutter").unwrap_or(0));
            }
            Event::End(e) if e.local_name().as_ref() == b"pagePr" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

/// 시작(또는 빈) 요소 하나의 서브트리를 quick-xml 이벤트 재직렬화로 원문 XML 문자열로
/// 캡처한다. Empty면 단일 요소, Start면 짝 닫는 태그까지 포함한다(중첩 임의 깊이).
fn capture_element(
    reader: &mut XmlReader<'_>,
    start: &BytesStart<'_>,
    empty: bool,
) -> Result<String> {
    let mut writer = Writer::new(Vec::new());
    let to_xml_err = |e: std::io::Error| HwpxError::Xml {
        entry: "secPr".to_string(),
        message: e.to_string(),
    };
    if empty {
        writer
            .write_event(Event::Empty(start.borrow()))
            .map_err(to_xml_err)?;
    } else {
        writer
            .write_event(Event::Start(start.borrow()))
            .map_err(to_xml_err)?;
        let mut depth = 1u32;
        loop {
            let ev = next_event(reader)?;
            match &ev {
                Event::Start(_) => depth += 1,
                Event::End(_) => depth -= 1,
                Event::Eof => break,
                _ => {}
            }
            writer.write_event(ev).map_err(to_xml_err)?;
            if depth == 0 {
                break;
            }
        }
    }
    Ok(String::from_utf8_lossy(&writer.into_inner()).into_owned())
}

/// Parses the `<hp:colPr>` control and consumes its child subtree.
///
/// OWPML column types map to NEWSPAPER=0, BALANCED=1, PARALLEL=2;
/// layouts map to LEFT=0, RIGHT=1, MIRROR=2. GG-17 parses an optional
/// `<hp:colLine>` child into `ColumnDef.divider`. Per-column `<hp:colSz>`
/// widths are not collected yet. A start event consumes through the closing
/// `colPr`, so callers must not traverse that subtree again.
fn parse_col_pr(
    reader: &mut XmlReader<'_>,
    e: &BytesStart<'_>,
    is_start: bool,
) -> Result<ColumnDef> {
    let kind = match attr(e, "type").as_deref() {
        Some("BALANCED") => 1,
        Some("PARALLEL") => 2,
        _ => 0, // NEWSPAPER
    };
    let direction = match attr(e, "layout").as_deref() {
        Some("RIGHT") => 1,
        Some("MIRROR") => 2,
        _ => 0, // LEFT
    };
    let mut def = ColumnDef {
        count: attr_u16(e, "colCount").unwrap_or(1),
        kind,
        direction,
        same_width: attr_u16(e, "sameSz").unwrap_or(1) != 0,
        gap: attr_i32(e, "sameGap").unwrap_or(0),
        widths: Vec::new(),
        divider: None,
    };
    if is_start {
        // Parse colLine semantically; per-column colSz widths remain uncollected.
        loop {
            match next_event(reader)? {
                Event::Start(c) | Event::Empty(c) if c.local_name().as_ref() == b"colLine" => {
                    def.divider = Some(hwp_model::BorderLine {
                        line_type: attr(&c, "type")
                            .map_or(1, |t| crate::read::header::line_type_code(&t)),
                        width: attr(&c, "width")
                            .map_or(1, |w| crate::read::header::width_index(&w)),
                        color: attr(&c, "color").map_or(0, |v| parse_color(&v)),
                    });
                }
                Event::End(c) if c.local_name().as_ref() == b"colPr" => break,
                Event::Eof => break,
                _ => {}
            }
        }
    }
    Ok(def)
}

fn head_foot_data(e: &BytesStart<'_>) -> Vec<u8> {
    let apply: u32 = match attr(e, "applyPageType").as_deref() {
        Some("EVEN") => 1,
        Some("ODD") => 2,
        _ => 0, // BOTH
    };
    let id: u32 = attr(e, "id").and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut v = Vec::with_capacity(8);
    v.extend_from_slice(&apply.to_le_bytes());
    v.extend_from_slice(&id.to_le_bytes());
    v
}

/// hwpx `<hp:pageNum pos formatType sideChar>` → hwp5 pgnp(쪽 번호 위치) 12B.
/// `properties(u32: 서식 | 위치<<8)` + 예약(6B) + sideChar WCHAR. 정품 실측:
/// pos=BOTTOM_CENTER(5), sideChar='-' → `000500000000000000002d00`.
fn build_pgnp(e: &BytesStart<'_>) -> Vec<u8> {
    let position: u32 = match attr(e, "pos").as_deref() {
        Some("TOP_LEFT") => 1,
        Some("TOP_CENTER") => 2,
        Some("TOP_RIGHT") => 3,
        Some("BOTTOM_LEFT") => 4,
        Some("BOTTOM_CENTER") => 5,
        Some("BOTTOM_RIGHT") => 6,
        Some("OUTSIDE_TOP") => 7,
        Some("OUTSIDE_BOTTOM") => 8,
        Some("INSIDE_TOP") => 9,
        Some("INSIDE_BOTTOM") => 10,
        _ => 0, // NONE
    };
    // 서식은 아라비아 숫자(DIGIT=0)만 매핑, 그 외는 0으로 대체.
    let format: u32 = 0;
    let side_char: u16 = attr(e, "sideChar")
        .and_then(|s| s.chars().next())
        .map(|c| c as u16)
        .unwrap_or(0);
    let props = format | (position << 8);
    let mut v = Vec::with_capacity(12);
    v.extend_from_slice(&props.to_le_bytes());
    v.extend_from_slice(&[0u8; 6]);
    v.extend_from_slice(&side_char.to_le_bytes());
    v
}

/// hwpx `<hp:pageHiding hide.../>` → hwp5 pghd(쪽 감추기) 4B 비트맵.
/// bit0=머리말, 1=꼬리말, 2=바탕쪽, 3=테두리, 4=배경, 5=쪽번호.
/// 정품 실측: 표지=0x21(머리말+쪽번호), 목차=0x20(쪽번호).
fn build_pghd(e: &BytesStart<'_>) -> Vec<u8> {
    let bit = |name: &str, b: u32| {
        if attr(e, name).as_deref() == Some("1") {
            1u32 << b
        } else {
            0
        }
    };
    let mask = bit("hideHeader", 0)
        | bit("hideFooter", 1)
        | bit("hideMasterPage", 2)
        | bit("hideBorder", 3)
        | bit("hideFill", 4)
        | bit("hidePageNum", 5);
    mask.to_le_bytes().to_vec()
}

/// hwpx `<hp:autoNum/>` → hwp5 atno(자동 번호) 12B. 실측 표준값(사업계획서.hwp 전수
/// `0u32, 4u32, 0u32` — 쪽 번호 자동): 종류/모양 세부 해석은 정답지 확보 후 정밀화.
fn build_atno() -> Vec<u8> {
    let mut v = Vec::with_capacity(12);
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&4u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v
}

/// hwpx `<hp:newNum num/>` → hwp5 nwno(새 번호 지정) 6B. `종류(u32=0,PAGE)` + `번호(u16)`.
/// 정품 실측: num=1 → `000000000100`.
fn build_nwno(e: &BytesStart<'_>) -> Vec<u8> {
    let num: u16 = attr(e, "num").and_then(|s| s.parse().ok()).unwrap_or(1);
    let mut v = Vec::with_capacity(6);
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&num.to_le_bytes());
    v
}

/// para.chars에서 아직 닫히지 않은 가장 안쪽 FIELD_START(코드 3)의 ctrl_id를 찾는다(LIFO).
/// fieldEnd는 아직 push 전이라 depth=0에서 뒤→앞 스캔(중첩 필드 대응).
fn matching_field_start(chars: &[HwpChar]) -> Option<[u8; 4]> {
    let mut depth = 0u32;
    for ch in chars.iter().rev() {
        match ch {
            HwpChar::InlineCtrl { code: 4, .. } => depth += 1,
            HwpChar::ExtCtrl {
                code: 3, ctrl_id, ..
            } => {
                if depth == 0 {
                    return Some(*ctrl_id);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

fn parse_ctrl(
    reader: &mut XmlReader<'_>,
    para: &mut Paragraph,
    wchar_pos: &mut u32,
    warnings: &mut Vec<String>,
) -> Result<()> {
    loop {
        let event = next_event(reader)?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                let name = e.local_name().as_ref().to_vec();
                // 필드: fieldBegin → ExtCtrl(3) + Generic %xxx(이름 CTRL_DATA), fieldEnd → InlineCtrl(4).
                if name.as_slice() == b"fieldBegin" {
                    let ty = attr(e, "type").unwrap_or_default();
                    let ctrl_id = hwp_convert::field::field_ctrl_id_from_owpml(&ty);
                    let fname = attr(e, "name").unwrap_or_default();
                    // Start면 자식 <hp:parameters>의 Command를 읽는다(self-closing이면 없음).
                    let command = if matches!(event, Event::Start(_)) {
                        read_field_command(reader)?
                    } else {
                        None
                    };
                    // 필드 커맨드 data는 hwp-convert의 공용 생성기로(종류별 attr/etc +
                    // 비영 id — 한글이 %hlk를 하이퍼링크로 인식하는 데 필수).
                    let data = command
                        .as_deref()
                        .map(|c| hwp_convert::field::make_field_command_data(&ctrl_id, c))
                        .unwrap_or_default();
                    let generic = GenericControl {
                        ctrl_id,
                        data,
                        paragraph_lists: Vec::new(),
                        extras: Vec::new(),
                        raw_children: vec![OpaqueRecord {
                            tag: 0x0057, // HWPTAG_CTRL_DATA — 이름 Parameter Set
                            data: hwp_convert::field::make_field_ctrl_data(&fname),
                            children: Vec::new(),
                        }],
                        gso_shapes: Vec::new(),
                        equation: None,
                        column_def: None,
                        caption: None,
                        hwpx_raw_xml: None,
                    };
                    push_ext_ctrl(para, wchar_pos, 3, ctrl_id);
                    para.controls.push(Control::Generic(generic));
                    continue;
                }
                if name.as_slice() == b"fieldEnd" {
                    // FIELD_END payload = 매칭 FIELD_START의 역순 ctrl_id 3B(`%` 제외).
                    // 전부 0이면 한글이 필드 짝을 못 지어 하이퍼링크 클릭이 안 된다
                    // (정답지 대조 확정). LIFO로 짝을 찾는다.
                    let payload = matching_field_start(&para.chars)
                        .map(|cid| hwp_convert::field::field_end_payload(&cid))
                        .unwrap_or_else(|| vec![0u8; 12]);
                    para.chars.push(HwpChar::InlineCtrl { code: 4, payload });
                    *wchar_pos += 8;
                    continue;
                }
                // 책갈피(지점 표식): <hp:bookmark name="…"/> → ExtCtrl(22) + Generic bokm(이름 CTRL_DATA).
                if name.as_slice() == b"bookmark" {
                    let bname = attr(e, "name").unwrap_or_default();
                    let generic = GenericControl {
                        ctrl_id: *b"bokm",
                        data: Vec::new(),
                        paragraph_lists: Vec::new(),
                        extras: Vec::new(),
                        raw_children: vec![OpaqueRecord {
                            tag: 0x0057, // HWPTAG_CTRL_DATA — 이름 Parameter Set
                            data: hwp_convert::bookmark::make_bokm_ctrl_data(&bname),
                            children: Vec::new(),
                        }],
                        gso_shapes: Vec::new(),
                        equation: None,
                        column_def: None,
                        caption: None,
                        hwpx_raw_xml: None,
                    };
                    push_ext_ctrl(para, wchar_pos, 22, *b"bokm");
                    para.controls.push(Control::Generic(generic));
                    continue;
                }
                // hwp5와 동일한 ctrl_id/컨트롤 문자 코드 매핑. 쪽번호·감추기·새번호는
                // 코드 21(페이지 컨트롤)이며, hwp5 페이로드를 여기서 합성해 둔다(빈
                // GenericControl이면 writer가 드롭). head/foot는 적용쪽+id를 8B로.
                let (ctrl_id, code, data): ([u8; 4], u16, Vec<u8>) = match name.as_slice() {
                    b"colPr" => (*b"cold", 2, Vec::new()),
                    b"header" => (*b"head", 16, head_foot_data(e)),
                    b"footer" => (*b"foot", 16, head_foot_data(e)),
                    b"footNote" => (*b"fn  ", 17, Vec::new()),
                    b"endNote" => (*b"en  ", 17, Vec::new()),
                    // 자동 번호(쪽): 실측 표준 페이로드 12B(사업계획서.hwp 전수 동일) 복원.
                    // 빈 페이로드면 hwp5 writer의 strip이 드롭한다(왕복 갭 방지).
                    b"autoNum" => (*b"atno", 18, build_atno()),
                    b"pageNum" => (*b"pgnp", 21, build_pgnp(e)),
                    b"pageHiding" => (*b"pghd", 21, build_pghd(e)),
                    b"newNum" => (*b"nwno", 21, build_nwno(e)),
                    other => {
                        let mut id = [b' '; 4];
                        for (i, b) in other.iter().take(4).enumerate() {
                            id[i] = *b;
                        }
                        (id, 21, Vec::new())
                    }
                };
                // Capture colPr for renderer column layout and divider support.
                // A start event makes parse_col_pr consume colSz/colLine children.
                let column_def = if name.as_slice() == b"colPr" {
                    Some(parse_col_pr(reader, e, matches!(event, Event::Start(_)))?)
                } else {
                    None
                };
                let mut generic = GenericControl {
                    ctrl_id,
                    data,
                    paragraph_lists: Vec::new(),
                    extras: Vec::new(),
                    raw_children: Vec::new(),
                    gso_shapes: Vec::new(),
                    equation: None,
                    column_def,
                    caption: None,
                    hwpx_raw_xml: None,
                };
                // parse_col_pr has already consumed a colPr subtree.
                if matches!(event, Event::Start(_)) && name.as_slice() != b"colPr" {
                    collect_sub_lists(reader, &name, &mut generic, warnings)?;
                }
                push_ext_ctrl(para, wchar_pos, code, ctrl_id);
                para.controls.push(Control::Generic(generic));
            }
            Event::End(e) if e.local_name().as_ref() == b"ctrl" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

/// fieldBegin의 자식 `<hp:parameters>`에서 Command stringParam 텍스트를 읽는다
/// (`</hp:fieldBegin>`까지). 없으면 None.
fn read_field_command(reader: &mut XmlReader<'_>) -> Result<Option<String>> {
    let mut in_command = false;
    let mut command: Option<String> = None;
    loop {
        let event = next_event(reader)?;
        match &event {
            Event::Start(e) if e.local_name().as_ref() == b"stringParam" => {
                in_command = attr(e, "name").as_deref() == Some("Command");
            }
            Event::Text(t) if in_command => {
                let s = t.xml10_content().map_err(|e| HwpxError::Xml {
                    entry: "fieldBegin/parameters".to_string(),
                    message: e.to_string(),
                })?;
                command.get_or_insert_with(String::new).push_str(&s);
            }
            Event::End(e) if e.local_name().as_ref() == b"stringParam" => in_command = false,
            Event::End(e) if e.local_name().as_ref() == b"fieldBegin" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(command)
}

/// hwpx vertRelTo → hwp5 코드 (PAPER=0, PAGE=1, PARA=2).
fn vert_rel_to_code(s: Option<&str>) -> u8 {
    match s {
        Some("PAGE") => 1,
        Some("PARA") => 2,
        _ => 0, // PAPER
    }
}

/// hwpx horzRelTo → hwp5 코드 (PAPER=0, PAGE=1, COLUMN=2, PARA=3).
fn horz_rel_to_code(s: Option<&str>) -> u8 {
    match s {
        Some("PAGE") => 1,
        Some("COLUMN") => 2,
        Some("PARA") => 3,
        _ => 0, // PAPER
    }
}

/// hwpx vertAlign/horzAlign → hwp5 코드 (TOP/LEFT=0, CENTER=1, BOTTOM/RIGHT=2).
fn align_code(s: Option<&str>) -> u8 {
    match s {
        Some("CENTER") => 1,
        Some("BOTTOM") | Some("RIGHT") => 2,
        _ => 0, // TOP/LEFT
    }
}

/// `<hp:tbl>` — 표.
fn parse_table(
    reader: &mut XmlReader<'_>,
    start: &BytesStart<'_>,
    warnings: &mut Vec<String>,
) -> Result<Table> {
    // 표 속성(attr): bits0-1=쪽 나눔(NONE=0/TABLE=1/CELL=2), bit2=제목줄 반복,
    // bit3=자동 너비 조정 안 함. 정품 실측으로 검증. 0으로 두면(과거 버그) 표가
    // "나누지 않음"이 돼, 잔여 공간에 안 들어가는 표가 통째로 다음 쪽으로 밀린다
    // (목차 박스가 별도 쪽으로 분리되는 원인).
    let mut table_attr: u32 = match attr(start, "pageBreak").as_deref() {
        Some("TABLE") => 1,
        Some("CELL") => 2,
        _ => 0, // NONE
    };
    if attr(start, "repeatHeader").as_deref() == Some("1") {
        table_attr |= 1 << 2;
    }
    if attr(start, "noAdjust").as_deref() == Some("1") {
        table_attr |= 1 << 3;
    }
    let mut table = Table {
        common_data: Vec::new(),
        placement: None, // 루프 종료 후 GsoPlacement로 채운다
        attr: table_attr,
        rows: attr_u16(start, "rowCnt").unwrap_or(0),
        cols: attr_u16(start, "colCnt").unwrap_or(0),
        cell_spacing: attr_u16(start, "cellSpacing").unwrap_or(0),
        // 셀 안쪽 여백: hwpx <hp:inMargin>에서 읽는다(아래 루프). 기본 0.
        inner_margins: [0; 4],
        row_cell_counts: Vec::new(),
        border_fill: hwp_model::BorderFillId(attr_u16(start, "borderFillIDRef").unwrap_or(0)),
        table_tail: Vec::new(),
        cells: Vec::new(),
        caption: None,
        extras: Vec::new(),
    };

    // 개체 공통 속성(배치) — hwp5 CTRL_HEADER 40바이트로 합성한다. 읽지 않으면
    // writer가 떠 있는(floating) 상수로 덮어써, 인라인이어야 할 표가 본문 흐름에서
    // 빠지고 한글이 재배치해 겹침/빈 페이지가 생긴다. zOrder는 시작 태그에 있다.
    let mut placement = hwp_model::GsoPlacement {
        z_order: attr_i32(start, "zOrder").unwrap_or(0),
        ..Default::default()
    };

    loop {
        match next_event(reader)? {
            Event::Start(e) => match e.local_name().as_ref() {
                b"tc" => {
                    let cell = parse_cell(reader, &e, warnings)?;
                    table.cells.push(cell);
                }
                b"caption" => {
                    table.caption = Some(parse_caption(reader, &e, warnings)?);
                }
                b"tr" => {} // 행은 cellAddr로 복원되므로 컨테이너로만 취급
                _ => {
                    let name = e.local_name().as_ref().to_vec();
                    skip_subtree(reader, &name)?;
                }
            },
            // 표 자신의 셀 안쪽 여백(self-closing). 셀(tc) 안 중첩 표는 parse_cell이
            // 따로 소비하므로 여기서 보이는 건 이 표의 것뿐이다. 순서: left,right,top,bottom.
            Event::Empty(e) if e.local_name().as_ref() == b"inMargin" => {
                table.inner_margins = [
                    attr_margin_u16(&e, "left"),
                    attr_margin_u16(&e, "right"),
                    attr_margin_u16(&e, "top"),
                    attr_margin_u16(&e, "bottom"),
                ];
            }
            // 배치: <hp:pos>(글자처럼취급/위치기준/오프셋), <hp:sz>(경계 너비/높이 —
            // 병합 셀 합산보다 정확), <hp:outMargin>(바깥 여백).
            Event::Empty(e) if e.local_name().as_ref() == b"pos" => {
                placement.treat_as_char = attr(&e, "treatAsChar").as_deref() == Some("1");
                placement.affect_line_spacing = attr(&e, "affectLSpacing").as_deref() == Some("1");
                placement.flow_with_text = attr(&e, "flowWithText").as_deref() == Some("1");
                placement.hold_anchor = attr(&e, "holdAnchorAndSO").as_deref() == Some("1");
                placement.vert_rel_to = vert_rel_to_code(attr(&e, "vertRelTo").as_deref());
                placement.horz_rel_to = horz_rel_to_code(attr(&e, "horzRelTo").as_deref());
                placement.vert_align = align_code(attr(&e, "vertAlign").as_deref());
                placement.horz_align = align_code(attr(&e, "horzAlign").as_deref());
                placement.vert_offset = attr_offset_i32(&e, "vertOffset").unwrap_or(0);
                placement.horz_offset = attr_offset_i32(&e, "horzOffset").unwrap_or(0);
            }
            Event::Empty(e) if e.local_name().as_ref() == b"sz" => {
                placement.width = attr_i32(&e, "width").unwrap_or(0);
                placement.height = attr_i32(&e, "height").unwrap_or(0);
            }
            Event::Empty(e) if e.local_name().as_ref() == b"outMargin" => {
                placement.out_margins = [
                    attr_u16(&e, "left").unwrap_or(0),
                    attr_u16(&e, "right").unwrap_or(0),
                    attr_u16(&e, "top").unwrap_or(0),
                    attr_u16(&e, "bottom").unwrap_or(0),
                ];
            }
            Event::End(e) if e.local_name().as_ref() == b"tbl" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    table.placement = Some(placement);
    // 행별 셀 수 재구성 (hwp5와 동일 의미 유지)
    let mut counts = vec![0u16; table.rows as usize];
    for cell in &table.cells {
        if let Some(c) = counts.get_mut(cell.row as usize) {
            *c += 1;
        }
    }
    table.row_cell_counts = counts;
    Ok(table)
}

/// Parses `<hp:caption>` using hwpxlib's side/fullSz/width/gap/lastWidth
/// contract plus subList paragraphs. `fullSz="1"` maps to no explicit width.
fn parse_caption(
    reader: &mut XmlReader<'_>,
    start: &BytesStart<'_>,
    warnings: &mut Vec<String>,
) -> Result<Caption> {
    let side = match attr(start, "side").as_deref() {
        Some("LEFT") => CaptionSide::Left,
        Some("RIGHT") => CaptionSide::Right,
        Some("TOP") => CaptionSide::Top,
        _ => CaptionSide::Bottom,
    };
    let full_size = attr(start, "fullSz").as_deref() == Some("1");
    let mut caption = Caption {
        side,
        // subList@textDirection is authoritative. Without it, infer vertical
        // direction for LEFT/RIGHT, consistent with table 72's width rule.
        direction: match side {
            CaptionSide::Left | CaptionSide::Right => CaptionDirection::Vertical,
            _ => CaptionDirection::Horizontal,
        },
        gap: attr_i32(start, "gap").unwrap_or(0),
        width: if full_size {
            None
        } else {
            attr_u32(start, "width")
        },
        last_width: attr_u32(start, "lastWidth").unwrap_or(0),
        paragraphs: Vec::new(),
    };
    loop {
        match next_event(reader)? {
            Event::Start(e) => match e.local_name().as_ref() {
                b"subList" => match attr(&e, "textDirection").as_deref() {
                    Some("VERTICAL") => caption.direction = CaptionDirection::Vertical,
                    Some("HORIZONTAL") => caption.direction = CaptionDirection::Horizontal,
                    _ => {}
                },
                b"p" => caption
                    .paragraphs
                    .push(parse_paragraph(reader, &e, warnings)?),
                _ => {}
            },
            Event::End(e) if e.local_name().as_ref() == b"caption" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(caption)
}

/// Reads an HWPX cell/table margin while retaining the schema's unsigned
/// 32-bit default sentinel. `4294967295` means "use the enclosing table
/// default"; the IR uses `u16::MAX` for that sentinel so HWP5 identity data is
/// not normalized by the renderer. Ordinary margins remain the existing u16
/// values.
fn attr_margin_u16(e: &BytesStart<'_>, name: &str) -> u16 {
    match attr_u32(e, name) {
        Some(u32::MAX) => u16::MAX,
        Some(value) => u16::try_from(value).unwrap_or(0),
        None => 0,
    }
}

/// `<hp:tc>` — 셀 하나.
fn parse_cell(
    reader: &mut XmlReader<'_>,
    start: &BytesStart<'_>,
    warnings: &mut Vec<String>,
) -> Result<Cell> {
    let mut cell = Cell {
        // 제목(머리) 셀이면 bit18 — 표 헤더 행 반복 대상(정품 실측). vertAlign은
        // 아래 subList에서 bits5-6에 더한다.
        list_attr: if attr(start, "header").as_deref() == Some("1") {
            1 << 18
        } else {
            0
        },
        col: 0,
        row: 0,
        col_span: 1,
        row_span: 1,
        width: HwpUnit(0),
        height: HwpUnit(0),
        margins: [0; 4],
        border_fill: hwp_model::BorderFillId(attr_u16(start, "borderFillIDRef").unwrap_or(0)),
        header_tail: Vec::new(),
        paragraphs: Vec::new(),
    };
    loop {
        match next_event(reader)? {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                b"cellAddr" => {
                    cell.col = attr_u16(&e, "colAddr").unwrap_or(0);
                    cell.row = attr_u16(&e, "rowAddr").unwrap_or(0);
                }
                b"cellSpan" => {
                    cell.col_span = attr_u16(&e, "colSpan").unwrap_or(1);
                    cell.row_span = attr_u16(&e, "rowSpan").unwrap_or(1);
                }
                b"cellSz" => {
                    cell.width = HwpUnit(attr_i32(&e, "width").unwrap_or(0));
                    cell.height = HwpUnit(attr_i32(&e, "height").unwrap_or(0));
                }
                b"cellMargin" => {
                    cell.margins = [
                        attr_margin_u16(&e, "left"),
                        attr_margin_u16(&e, "right"),
                        attr_margin_u16(&e, "top"),
                        attr_margin_u16(&e, "bottom"),
                    ];
                }
                b"subList" => {
                    // 셀 세로 정렬(vertAlign)을 list_attr bits5-6에 인코딩:
                    // TOP=0, CENTER=1, BOTTOM=2. 정품 셀은 CENTER(0x20)인데 안 읽으면
                    // 0(TOP)이 돼 셀 내용이 위로 몰리고, 셀 높이가 내용보다 크면 빈
                    // 아래 영역이 다음 쪽으로 분리된다(빈 페이지 발생).
                    let va = match attr(&e, "vertAlign").as_deref() {
                        Some("CENTER") => 1u32,
                        Some("BOTTOM") => 2,
                        _ => 0, // TOP
                    };
                    cell.list_attr |= va << 5;
                }
                b"p" => {
                    cell.paragraphs.push(parse_paragraph(reader, &e, warnings)?);
                }
                _ => {}
            },
            Event::End(e) if e.local_name().as_ref() == b"tc" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(cell)
}

fn default_picture() -> hwp_model::Picture {
    hwp_model::Picture {
        common_data: Vec::new(),
        width: HwpUnit(0),
        height: HwpUnit(0),
        treat_as_char: false,
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
        bin_ref: hwp_model::BinRef::ItemRef(String::new()),
        extras: Vec::new(),
    }
}

/// Parses an `<hp:pic>` image semantically: size, placement, binary reference,
/// flip, rotation, crop, brightness, and contrast. `hp:imgRect` duplicates
/// `hp:sz` and is therefore not read separately.
fn parse_picture(
    reader: &mut XmlReader<'_>,
    warnings: &mut Vec<String>,
) -> Result<hwp_model::Picture> {
    let mut pic = default_picture();
    let mut depth = 1u32;
    loop {
        let event = next_event(reader)?;
        match &event {
            Event::Start(e) | Event::Empty(e) => {
                match e.local_name().as_ref() {
                    b"sz" => {
                        pic.width = HwpUnit(attr_i32(e, "width").unwrap_or(0));
                        pic.height = HwpUnit(attr_i32(e, "height").unwrap_or(0));
                    }
                    b"pos" => {
                        pic.treat_as_char = attr(e, "treatAsChar").as_deref() == Some("1");
                        // 떠 있는 개체 위치 오프셋(글자처럼 취급이면 무시됨). hwpx는
                        // 음수를 unsigned 2의보수 십진수로 저장(예: -77 = 4294967219)하므로
                        // u32로 파싱 후 i32로 재해석한다(i32 직접 파싱은 범위 초과로 실패).
                        pic.vert_offset = attr_offset_i32(e, "vertOffset").unwrap_or(0);
                        pic.horz_offset = attr_offset_i32(e, "horzOffset").unwrap_or(0);
                    }
                    b"img" => {
                        if let Some(item) = attr(e, "binaryItemIDRef") {
                            pic.bin_ref = hwp_model::BinRef::ItemRef(item);
                        }
                        pic.brightness = attr_i32(e, "bright").unwrap_or(0).clamp(-100, 100) as i8;
                        pic.contrast = attr_i32(e, "contrast").unwrap_or(0).clamp(-100, 100) as i8;
                    }
                    b"flip" => {
                        let h = attr(e, "horizontal").as_deref() == Some("1");
                        let v = attr(e, "vertical").as_deref() == Some("1");
                        pic.flip = u8::from(h) | (u8::from(v) << 1);
                    }
                    b"rotationInfo" => {
                        let angle = attr_i32(e, "angle").unwrap_or(0);
                        pic.rotation = (angle != 0).then_some(angle as f32);
                    }
                    // Crop in source-image HWPUNIT coordinates.
                    b"imgClip" => {
                        pic.crop = Some([
                            attr_i32(e, "left").unwrap_or(0) as f32,
                            attr_i32(e, "top").unwrap_or(0) as f32,
                            attr_i32(e, "right").unwrap_or(0) as f32,
                            attr_i32(e, "bottom").unwrap_or(0) as f32,
                        ]);
                    }
                    b"shapeComment" if matches!(event, Event::Start(_)) => {
                        let value = read_element_text(reader, b"shapeComment")?;
                        pic.description = (!value.is_empty()).then_some(value);
                    }
                    b"caption" if matches!(event, Event::Start(_)) => {
                        pic.caption = Some(parse_caption(reader, e, warnings)?);
                    }
                    // 중첩 pic은 여는 태그만 깊이 증가 (Empty는 닫는 태그가 없음)
                    b"pic" if matches!(event, Event::Start(_)) => depth += 1,
                    _ => {}
                }
            }
            Event::End(e) if e.local_name().as_ref() == b"pic" => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    // Normalize the writer's no-crop constant (0, 0, w, h) to None, preserving
    // the semantic behavior of readers that ignored imgClip.
    if pic.crop == Some([0.0, 0.0, pic.width.0 as f32, pic.height.0 as f32]) {
        pic.crop = None;
    }
    Ok(pic)
}

/// 일반 개체에서 `hp:subList`의 문단들을 재귀 수집 (글상자 텍스트).
fn collect_sub_lists(
    reader: &mut XmlReader<'_>,
    end_name: &[u8],
    generic: &mut GenericControl,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let mut depth = 1u32;
    loop {
        match next_event(reader)? {
            Event::Start(e) => {
                let name = e.local_name().as_ref().to_vec();
                if name == b"caption" {
                    // Unknown/generic shape containers (for example hp:container)
                    // do not go through collect_shape, but they can still carry
                    // the same hp:caption child as primitive shapes. Keep the
                    // caption separate from the object's text lists so renderers
                    // and text extraction can apply the configured side/order.
                    generic.caption = Some(parse_caption(reader, &e, warnings)?);
                } else if name == b"subList" {
                    let mut list = ParagraphList {
                        header_data: Vec::new(),
                        paragraphs: Vec::new(),
                    };
                    loop {
                        match next_event(reader)? {
                            Event::Start(inner) if inner.local_name().as_ref() == b"p" => {
                                list.paragraphs
                                    .push(parse_paragraph(reader, &inner, warnings)?);
                            }
                            Event::End(inner) if inner.local_name().as_ref() == b"subList" => {
                                break;
                            }
                            Event::Eof => break,
                            _ => {}
                        }
                    }
                    generic.paragraph_lists.push(list);
                } else if name == end_name {
                    depth += 1;
                }
            }
            Event::End(e) if e.local_name().as_ref() == end_name => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

/// 그리기 개체 요소 이름 → 도형 종류.
fn shape_kind(name: &[u8]) -> Option<ShapeKind> {
    match name {
        b"rect" => Some(ShapeKind::Rect),
        b"ellipse" => Some(ShapeKind::Ellipse),
        b"line" => Some(ShapeKind::Line),
        b"polygon" => Some(ShapeKind::Polygon),
        b"curve" => Some(ShapeKind::Curve),
        b"arc" => Some(ShapeKind::Arc),
        _ => None,
    }
}

/// 도형 요소를 파싱: hp:pos(오프셋)·hp:sz(크기)·hp:lineShape(테두리)·
/// hp:fillBrush>hp:winBrush(채움)·hp:pt*(점) + subList(텍스트). gso_shapes에 담는다.
fn collect_shape(
    reader: &mut XmlReader<'_>,
    end_name: &[u8],
    kind: ShapeKind,
    round_ratio: u8,
    generic: &mut GenericControl,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let (mut x, mut y, mut w, mut h) = (0i32, 0i32, 0i32, 0i32);
    let mut fill = 0xFFFF_FFFFu32;
    let mut border_color = 0xFFFF_FFFFu32;
    let mut border_width = 0i32;
    let mut points: Vec<(i32, i32)> = Vec::new();
    let mut fill_gradient: Option<GradientSpec> = None;
    let mut border_style = 0u8;
    let mut arrow_start = 0u8;
    let mut arrow_end = 0u8;
    let mut anchored = false;
    let mut description = None;
    let mut read_attrs = |e: &BytesStart<'_>| match e.local_name().as_ref() {
        b"pos" => {
            x = attr_offset_i32(e, "horzOffset").unwrap_or(x);
            y = attr_offset_i32(e, "vertOffset").unwrap_or(y);
            if attr(e, "treatAsChar").as_deref() == Some("1") {
                anchored = true;
            }
        }
        b"sz" => {
            w = attr_i32(e, "width").unwrap_or(w);
            h = attr_i32(e, "height").unwrap_or(h);
        }
        b"lineShape" => {
            if let Some(c) = attr(e, "color") {
                border_color = parse_color(&c);
            }
            border_width = attr_i32(e, "width").unwrap_or(border_width);
            if let Some(st) = attr(e, "style") {
                border_style = line_style_code(&st);
            }
            if let Some(hs) = attr(e, "headStyle") {
                arrow_start = arrow_code(&hs);
            }
            if let Some(ts) = attr(e, "tailStyle") {
                arrow_end = arrow_code(&ts);
            }
        }
        b"winBrush" => {
            if let Some(c) = attr(e, "faceColor") {
                fill = parse_color(&c);
            }
        }
        // pt0~N은 Polygon/Curve의 기하 점만 취한다. Rect/Ellipse/Arc의 pt0~3은 bbox
        // 모서리(정품 형식)라 sz로 왕복하므로 무시(안 그러면 도형에 헛점이 붙는다).
        n if n.starts_with(b"pt") && matches!(kind, ShapeKind::Polygon | ShapeKind::Curve) => {
            if let (Some(px), Some(py)) = (attr_i32(e, "x"), attr_i32(e, "y")) {
                points.push((px, py));
            }
        }
        // Arc는 center/ax1/ax2(bbox 기준, 등장 순서)로 곡선 기하를 운반한다 — writer의 역.
        // 렌더러가 이 3점(중심+켤레 두 축)으로 호를 그린다.
        b"center" | b"ax1" | b"ax2" if matches!(kind, ShapeKind::Arc) => {
            if let (Some(px), Some(py)) = (attr_i32(e, "x"), attr_i32(e, "y")) {
                points.push((px, py));
            }
        }
        _ => {}
    };

    let mut depth = 1u32;
    loop {
        match next_event(reader)? {
            Event::Empty(e) => read_attrs(&e),
            Event::Start(e) => {
                let n = e.local_name().as_ref().to_vec();
                if n == b"caption" {
                    generic.caption = Some(parse_caption(reader, &e, warnings)?);
                } else if n == b"subList" {
                    let mut list = ParagraphList {
                        header_data: Vec::new(),
                        paragraphs: Vec::new(),
                    };
                    loop {
                        match next_event(reader)? {
                            Event::Start(inner) if inner.local_name().as_ref() == b"p" => {
                                list.paragraphs
                                    .push(parse_paragraph(reader, &inner, warnings)?);
                            }
                            Event::End(inner) if inner.local_name().as_ref() == b"subList" => break,
                            Event::Eof => break,
                            _ => {}
                        }
                    }
                    generic.paragraph_lists.push(list);
                } else if n == b"shapeComment" {
                    let value = read_element_text(reader, b"shapeComment")?;
                    description = (!value.is_empty()).then_some(value);
                } else if n == b"gradation" {
                    fill_gradient = parse_gradation(reader, &e)?;
                } else {
                    read_attrs(&e);
                    if n == end_name {
                        depth += 1;
                    }
                }
            }
            Event::End(e) if e.local_name().as_ref() == end_name => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    // 가로/세로 선은 한 축이 0일 수 있으므로 w 또는 h만 있어도 받는다.
    if w != 0 || h != 0 || !points.is_empty() {
        generic.gso_shapes.push(ShapeGeom {
            kind,
            x,
            y,
            w,
            h,
            points,
            fill,
            fill_gradient,
            border_color,
            border_width,
            round_ratio,
            border_style,
            arrow_start,
            arrow_end,
            anchored,
            description,
        });
    }
    Ok(())
}

/// `<hp:equation>` — 수식. 스크립트(`script` 속성 또는 `<hp:script>` 자식)와
/// 크기(hp:sz)·위치(hp:pos)를 모은다. 렌더러는 상자+텍스트로 근사한다.
fn parse_equation(
    reader: &mut XmlReader<'_>,
    start: &BytesStart<'_>,
    empty: bool,
) -> Result<Equation> {
    let mut script = attr(start, "script").unwrap_or_default();
    let (mut width, mut height, mut x, mut y) = (0i32, 0i32, 0i32, 0i32);
    let mut inline = true;
    // 의미 모델링하지 않는 속성(글자색·baseUnit·수식 글꼴·zOrder·본문배치…)은 원문으로
    // 보존해 왕복에서 되쓴다. id 는 writer 가 문서 전역으로 다시 매기므로 제외한다.
    let raw_attrs = raw_attrs_except(start, b"id");
    let mut raw_props = Vec::new();
    if !empty {
        loop {
            let ev = next_event(reader)?;
            match &ev {
                Event::Start(e) | Event::Empty(e) => {
                    let is_start = matches!(ev, Event::Start(_));
                    let is_empty_el = matches!(ev, Event::Empty(_));
                    match e.local_name().as_ref() {
                        b"script" if is_start => script = read_element_text(reader, b"script")?,
                        b"sz" => {
                            width = attr_i32(e, "width").unwrap_or(width);
                            height = attr_i32(e, "height").unwrap_or(height);
                            raw_props.push(capture_element(reader, e, is_empty_el)?);
                        }
                        b"pos" => {
                            inline = attr(e, "treatAsChar").as_deref() == Some("1");
                            x = attr_offset_i32(e, "horzOffset").unwrap_or(0);
                            y = attr_offset_i32(e, "vertOffset").unwrap_or(0);
                            raw_props.push(capture_element(reader, e, is_empty_el)?);
                        }
                        // outMargin·caption·shapeComment 등 나머지 공통 자식도 원문 보존.
                        _ => raw_props.push(capture_element(reader, e, is_empty_el)?),
                    }
                }
                Event::End(e) if e.local_name().as_ref() == b"equation" => break,
                Event::Eof => break,
                _ => {}
            }
        }
    }
    Ok(Equation {
        script: script.trim().to_string(),
        width,
        height,
        inline,
        x,
        y,
        raw_attrs,
        raw_props,
    })
}

/// 시작 태그의 속성을 `name="value"` 원문으로 되돌린다(`skip` 이름 하나는 제외).
/// 값은 파일에 있던 이스케이프 형태 그대로 다시 쓰므로(재이스케이프 없음) 왕복이 닫힌다.
fn raw_attrs_except(start: &BytesStart<'_>, skip: &[u8]) -> Option<String> {
    let mut out = String::new();
    for a in start.attributes().flatten() {
        if a.key.local_name().as_ref() == skip {
            continue;
        }
        let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        let val = String::from_utf8_lossy(&a.value).into_owned();
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format!("{key}=\"{val}\""));
    }
    (!out.is_empty()).then_some(out)
}

/// 엔티티 참조(`&amp;` `&#x2264;`)를 문자로 되돌린다. quick-xml은 참조를 텍스트에
/// 합치지 않고 `Event::GeneralRef`로 따로 주므로, 텍스트를 모으는 쪽마다 이 해석이
/// 필요하다. DTD 없이는 외부 엔티티를 풀 수 없어 미리 정의된 5종 + 숫자 참조만 푼다.
fn resolve_entity(r: &quick_xml::events::BytesRef<'_>) -> Option<char> {
    r.resolve_char_ref()
        .ok()
        .flatten()
        .or_else(|| match &r[..] {
            b"amp" => Some('&'),
            b"lt" => Some('<'),
            b"gt" => Some('>'),
            b"quot" => Some('"'),
            b"apos" => Some('\''),
            _ => None,
        })
}

/// 주어진 요소가 닫힐 때까지 텍스트를 모은다.
fn read_element_text(reader: &mut XmlReader<'_>, end: &[u8]) -> Result<String> {
    let mut out = String::new();
    loop {
        match next_event(reader)? {
            Event::Text(t) => {
                let s = t.xml10_content().map_err(|e| HwpxError::Xml {
                    entry: "section".to_string(),
                    message: e.to_string(),
                })?;
                out.push_str(&s);
            }
            // 수식 스크립트는 `x &lt; y`처럼 XML 특수문자를 담는다 — 참조를 풀지 않으면
            // 그 글자가 통째로 사라진다(writer의 esc()와 짝을 이루는 역변환).
            Event::GeneralRef(r) => {
                if let Some(c) = resolve_entity(&r) {
                    out.push(c);
                }
            }
            // CDATA 구획(`<![CDATA[a<b]]>`)은 Text가 아닌 별도 이벤트다. 내용은 이미
            // 문자 그대로라 참조 해석 없이 붙인다.
            Event::CData(t) => {
                let s = t.xml10_content().map_err(|e| HwpxError::Xml {
                    entry: "section".to_string(),
                    message: e.to_string(),
                })?;
                out.push_str(&s);
            }
            Event::End(e) if e.local_name().as_ref() == end => break,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

/// hp:lineShape `style` → 선 종류 코드(0=실선,1=파선,2=점선,3=일점쇄선,4=이점쇄선,5=긴파선).
fn line_style_code(s: &str) -> u8 {
    match s.to_ascii_uppercase().as_str() {
        "DASH" => 1,
        "DOT" => 2,
        "DASH_DOT" | "DASHDOT" => 3,
        "DASH_DOT_DOT" | "DASHDOTDOT" => 4,
        "LONG_DASH" | "LONGDASH" => 5,
        _ => 0, // SOLID 등
    }
}

/// hp:lineShape `headStyle`/`tailStyle` → 화살촉 유무(0=없음/NORMAL, 1=화살촉).
fn arrow_code(s: &str) -> u8 {
    if s.is_empty() || s.eq_ignore_ascii_case("NORMAL") || s.eq_ignore_ascii_case("NONE") {
        0
    } else {
        1
    }
}

/// `<hp:gradation>` — 그러데이션 채움. type(LINEAR/RADIAL/...), angle, 자식
/// `hp:color value="#.."` 들을 균등 위치로 stop화한다.
fn parse_gradation(
    reader: &mut XmlReader<'_>,
    start: &BytesStart<'_>,
) -> Result<Option<GradientSpec>> {
    let gtype = attr(start, "type").unwrap_or_default();
    // LINEAR=선형, 그 외(RADIAL/CIRCLE/CONICAL/SQUARE)는 방사형 근사.
    let radial = !gtype.eq_ignore_ascii_case("LINEAR");
    let angle_deg = attr_i32(start, "angle").unwrap_or(0) as f32;
    let mut colors: Vec<u32> = Vec::new();
    loop {
        match next_event(reader)? {
            Event::Empty(e) | Event::Start(e) if e.local_name().as_ref() == b"color" => {
                if let Some(v) = attr(&e, "value") {
                    colors.push(parse_color(&v));
                }
            }
            Event::End(e) if e.local_name().as_ref() == b"gradation" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    if colors.len() < 2 {
        return Ok(None);
    }
    let last = (colors.len() - 1) as f32;
    let stops = colors
        .into_iter()
        .enumerate()
        .map(|(i, c)| (i as f32 / last, c))
        .collect();
    Ok(Some(GradientSpec {
        radial,
        angle_deg,
        stops,
    }))
}

/// `<hp:linesegarray>` — 줄 배치 정보.
fn parse_linesegs(reader: &mut XmlReader<'_>, para: &mut Paragraph) -> Result<()> {
    loop {
        match next_event(reader)? {
            Event::Empty(e) | Event::Start(e) if e.local_name().as_ref() == b"lineseg" => {
                para.line_segs.push(LineSeg {
                    text_start: attr_u32(&e, "textpos").unwrap_or(0),
                    v_pos: attr_i32(&e, "vertpos").unwrap_or(0),
                    line_height: attr_i32(&e, "vertsize").unwrap_or(0),
                    text_height: attr_i32(&e, "textheight").unwrap_or(0),
                    baseline_gap: attr_i32(&e, "baseline").unwrap_or(0),
                    line_spacing: attr_i32(&e, "spacing").unwrap_or(0),
                    col_start: attr_i32(&e, "horzpos").unwrap_or(0),
                    seg_width: attr_i32(&e, "horzsize").unwrap_or(0),
                    flags: attr_u32(&e, "flags").unwrap_or(0),
                });
            }
            Event::End(e) if e.local_name().as_ref() == b"linesegarray" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod page_ctrl_tests {
    use super::*;

    fn elem(name: &str, attrs: &[(&str, &str)]) -> BytesStart<'static> {
        let mut e = BytesStart::new(name.to_string());
        for (k, v) in attrs {
            e.push_attribute((*k, *v));
        }
        e
    }

    /// 쪽 번호 위치(pgnp): 정품 한라대 = pos BOTTOM_CENTER + sideChar '-'.
    #[test]
    fn pgnp_합성() {
        let e = elem(
            "hp:pageNum",
            &[
                ("pos", "BOTTOM_CENTER"),
                ("formatType", "DIGIT"),
                ("sideChar", "-"),
            ],
        );
        // props=위치5<<8=0x500, 예약6B, sideChar '-'(0x2d) WCHAR.
        assert_eq!(
            build_pgnp(&e),
            vec![0x00, 0x05, 0, 0, 0, 0, 0, 0, 0, 0, 0x2d, 0x00]
        );
    }

    /// 쪽 감추기(pghd): 표지=머리말+쪽번호(0x21), 목차=쪽번호(0x20).
    #[test]
    fn pghd_합성() {
        let cover = elem(
            "hp:pageHiding",
            &[("hideHeader", "1"), ("hidePageNum", "1")],
        );
        assert_eq!(build_pghd(&cover), vec![0x21, 0, 0, 0]);
        let toc = elem(
            "hp:pageHiding",
            &[("hideHeader", "0"), ("hidePageNum", "1")],
        );
        assert_eq!(build_pghd(&toc), vec![0x20, 0, 0, 0]);
    }

    /// 새 번호 지정(nwno): num=1 → 종류(0) + 번호(1).
    #[test]
    fn nwno_합성() {
        let e = elem("hp:newNum", &[("num", "1"), ("numType", "PAGE")]);
        assert_eq!(build_nwno(&e), vec![0, 0, 0, 0, 0x01, 0x00]);
    }

    /// 머리말/꼬리말: 적용쪽(u32) + id(u32). BOTH(0) + id=2.
    #[test]
    fn head_foot_8바이트() {
        let e = elem("hp:header", &[("id", "2"), ("applyPageType", "BOTH")]);
        assert_eq!(head_foot_data(&e), vec![0, 0, 0, 0, 0x02, 0, 0, 0]);
        let odd = elem("hp:footer", &[("id", "3"), ("applyPageType", "ODD")]);
        assert_eq!(head_foot_data(&odd), vec![0x02, 0, 0, 0, 0x03, 0, 0, 0]);
    }

    /// 완결된 `<hp:gradation>…` 문서를 열어 시작 태그를 소비하고 parse_gradation 호출.
    fn run_gradation(xml: &str) -> Option<GradientSpec> {
        let mut reader = Reader::from_str(xml);
        let start = match reader.read_event().unwrap() {
            Event::Start(e) => e.into_owned(),
            other => panic!("gradation 시작 태그 기대, 실제 {other:?}"),
        };
        parse_gradation(&mut reader, &start).unwrap()
    }

    /// 선형 그러데이션: type=LINEAR, 색 2개 → stop 0.0/1.0 균등.
    #[test]
    fn gradation_선형_2색() {
        let g = run_gradation(
            r##"<hp:gradation type="LINEAR" angle="90"><hp:color value="#FF0000"/><hp:color value="#0000FF"/></hp:gradation>"##,
        )
        .unwrap();
        assert!(!g.radial);
        assert_eq!(g.angle_deg, 90.0);
        assert_eq!(g.stops.len(), 2);
        assert_eq!(g.stops[0], (0.0, parse_color("#FF0000")));
        assert_eq!(g.stops[1], (1.0, parse_color("#0000FF")));
    }

    /// 방사형 그러데이션: type=RADIAL → radial=true, 색 3개 → 0/0.5/1.0.
    #[test]
    fn gradation_방사_3색() {
        let g = run_gradation(
            r##"<hp:gradation type="RADIAL"><hp:color value="#000000"/><hp:color value="#808080"/><hp:color value="#FFFFFF"/></hp:gradation>"##,
        )
        .unwrap();
        assert!(g.radial);
        assert_eq!(g.stops.len(), 3);
        assert!((g.stops[1].0 - 0.5).abs() < 0.001);
    }

    /// 과거 오염 파일 하위호환: `<hp:t>` 안 raw 0x09를 InlineCtrl(9)로 정규화하고
    /// 그 외 C0 제어문자는 제거한다(Text('\t')로 되읽으면 재저장 시 다시 깨진다).
    #[test]
    fn 오염_raw탭_읽기_인라인컨트롤_정규화() {
        let xml = concat!(
            r##"<?xml version="1.0"?><hs:sec><hp:p paraPrIDRef="0" styleIDRef="0">"##,
            "<hp:run charPrIDRef=\"0\"><hp:t xml:space=\"preserve\">앞\t뒤\u{0001}끝</hp:t></hp:run>",
            r##"</hp:p></hs:sec>"##,
        );
        let (section, _) = parse_section(xml).unwrap();
        let chars = &section.paragraphs[0].chars;
        // raw 탭 → InlineCtrl(9), 제어문자 0x01 제거, 나머지 텍스트 보존.
        assert!(
            chars
                .iter()
                .any(|c| matches!(c, HwpChar::InlineCtrl { code: 9, .. })),
            "raw 탭이 InlineCtrl(9)로 정규화돼야: {chars:?}"
        );
        assert!(
            !chars.iter().any(|c| matches!(c, HwpChar::Text('\t'))),
            "Text('\\t')로 남으면 안 됨"
        );
        assert!(
            !chars
                .iter()
                .any(|c| matches!(c, HwpChar::Text(x) if (*x as u32) < 0x20)),
            "C0 제어문자가 Text로 남으면 안 됨"
        );
        assert_eq!(
            section.paragraphs[0].wchar_len(),
            1 + 8 + 1 + 1,
            "앞(1)+탭(8)+뒤(1)+끝(1) WCHAR"
        );
    }

    /// 색이 1개 이하면 그러데이션 없음(None).
    #[test]
    fn gradation_단색_무시() {
        assert!(
            run_gradation(
                r##"<hp:gradation type="LINEAR"><hp:color value="#FF0000"/></hp:gradation>"##
            )
            .is_none()
        );
    }

    /// GG-15: reads all supported `hp:pic` transform and adjustment attributes into the IR.
    #[test]
    fn 그림_변환_보정_속성_읽기() {
        let xml = concat!(
            r##"<?xml version="1.0"?><hs:sec><hp:p paraPrIDRef="0" styleIDRef="0">"##,
            r##"<hp:run charPrIDRef="0"><hp:pic id="1" zOrder="0" numberingType="PICTURE" textWrap="SQUARE" textFlow="BOTH_SIDES" lock="0" dropcapstyle="None" href="" groupLevel="0" instid="1" reverse="0">"##,
            r##"<hp:sz width="4000" height="3000" widthRelTo="ABSOLUTE" heightRelTo="ABSOLUTE" protect="0"/>"##,
            r##"<hp:pos treatAsChar="1" affectLSpacing="0" flowWithText="1" allowOverlap="0" holdAnchorAndSO="0" vertRelTo="PARA" horzRelTo="PARA" vertAlign="TOP" horzAlign="LEFT" vertOffset="0" horzOffset="0"/>"##,
            r##"<hp:flip horizontal="1" vertical="0"/>"##,
            r##"<hp:rotationInfo angle="30" centerX="2000" centerY="1500" rotateimage="1"/>"##,
            r##"<hc:img binaryItemIDRef="image1" bright="30" contrast="-20" effect="REAL_PIC" alpha="0"/>"##,
            r##"<hp:imgClip left="100" right="3900" top="200" bottom="2800"/>"##,
            r##"</hp:pic></hp:run></hp:p></hs:sec>"##,
        );
        let (section, _) = parse_section(xml).unwrap();
        let pic = section.paragraphs[0]
            .controls
            .iter()
            .find_map(|c| match c {
                Control::Picture(p) => Some(p),
                _ => None,
            })
            .expect("그림 개체");
        assert_eq!(pic.flip, 1, "가로 뒤집기");
        assert_eq!(pic.rotation, Some(30.0));
        assert_eq!(pic.crop, Some([100.0, 200.0, 3900.0, 2800.0]));
        assert_eq!(pic.brightness, 30);
        assert_eq!(pic.contrast, -20);
        assert_eq!(
            pic.bin_ref,
            hwp_model::BinRef::ItemRef("image1".to_string())
        );
    }

    #[test]
    fn picture_adjustments_are_clamped_to_the_model_range() {
        let xml = concat!(
            r##"<?xml version="1.0"?><hs:sec><hp:p paraPrIDRef="0" styleIDRef="0">"##,
            r##"<hp:run charPrIDRef="0"><hp:pic id="1"><hp:sz width="1" height="1"/><hc:img binaryItemIDRef="image1" bright="130" contrast="-120"/></hp:pic></hp:run></hp:p></hs:sec>"##,
        );
        let (section, _) = parse_section(xml).unwrap();
        let picture = section.paragraphs[0]
            .controls
            .iter()
            .find_map(|control| match control {
                Control::Picture(picture) => Some(picture),
                _ => None,
            })
            .expect("picture");
        assert_eq!(picture.brightness, 100);
        assert_eq!(picture.contrast, -100);
    }
}
