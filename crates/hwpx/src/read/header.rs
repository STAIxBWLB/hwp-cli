//! `Contents/header.xml` → [`DocHeader`].
//!
//! M2 범위: 글꼴(fontfaces), 문자 모양(charPr), 문단 모양(paraPr — 정렬),
//! 스타일(style). 테두리/번호 등은 추후 마일스톤에서 채운다.

use std::collections::HashMap;

use hwp_model::{
    BorderFill, BorderLine, CharShape, CharShapeId, DocHeader, FaceName, LANG_COUNT, ParaShape,
    ParaShapeId, Style,
};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::error::{HwpxError, Result};
use crate::read::xml::{attr, attr_i32, attr_u16, attr_u32, parse_color};

/// OWPML 테두리선 종류 → hwp5 코드.
pub(crate) fn line_type_code(s: &str) -> u8 {
    match s {
        "NONE" => 0,
        "SOLID" => 1,
        "DASH" => 2,
        "DOT" => 3,
        "DASH_DOT" => 4,
        "DASH_DOT_DOT" => 5,
        "LONG_DASH" => 6,
        "CIRCLE" => 7,
        "DOUBLE_SLIM" => 8,
        "SLIM_THICK" => 9,
        "THICK_SLIM" => 10,
        "SLIM_THICK_SLIM" => 11,
        _ => 1,
    }
}

/// Converts an underline or strike shape to the zero-based decoration code used
/// by HWP5 attribute bits 4..=7 and 26..=29. WAVE and DOUBLE_WAVE are mapped
/// explicitly because the general border-line code table does not contain them.
pub(crate) fn decor_code(s: &str) -> u8 {
    match s {
        "WAVE" => 11,
        "DOUBLE_WAVE" => 12,
        _ => line_type_code(s).saturating_sub(1),
    }
}

/// Converts an OWPML `symMark` to HWP5 attribute bits 21..=24 using hwpxlib's
/// `SymMarkSort` ordering.
pub(crate) fn sym_mark_code(s: &str) -> u8 {
    match s {
        "DOT_ABOVE" => 1,
        "RING_ABOVE" => 2,
        "TILDE" => 3,
        "CARON" => 4,
        "SIDE" => 5,
        "COLON" => 6,
        "GRAVE_ACCENT" => 7,
        "ACUTE_ACCENT" => 8,
        "CIRCUMFLEX" => 9,
        "MACRON" => 10,
        "HOOK_ABOVE" => 11,
        "DOT_BELOW" => 12,
        _ => 0,
    }
}

/// "0.12 mm" → 굵기 인덱스 (가장 가까운 값).
pub(crate) fn width_index(s: &str) -> u8 {
    const TABLE: [f32; 16] = [
        0.1, 0.12, 0.15, 0.2, 0.25, 0.3, 0.4, 0.5, 0.6, 0.7, 1.0, 1.5, 2.0, 3.0, 4.0, 5.0,
    ];
    let mm: f32 = s.trim_end_matches("mm").trim().parse().unwrap_or(0.12);
    TABLE
        .iter()
        .enumerate()
        .min_by(|a, b| (a.1 - mm).abs().total_cmp(&(b.1 - mm).abs()))
        .map(|(i, _)| i as u8)
        .unwrap_or(1)
}

fn parse_border_line(e: &BytesStart<'_>) -> BorderLine {
    BorderLine {
        line_type: attr(e, "type").map_or(1, |t| line_type_code(&t)),
        width: attr(e, "width").map_or(1, |w| width_index(&w)),
        color: attr(e, "color").map_or(0, |c| parse_color(&c)),
    }
}

/// OWPML 언어 이름 → 7언어 슬롯 인덱스.
fn lang_slot(name: &str) -> Option<usize> {
    Some(match name {
        "HANGUL" => 0,
        "LATIN" => 1,
        "HANJA" => 2,
        "JAPANESE" => 3,
        "OTHER" => 4,
        "SYMBOL" => 5,
        "USER" => 6,
        _ => return None,
    })
}

/// hwpx numFormat 문자열 → NumFmt(모르면 십진).
fn num_fmt(s: &str) -> hwp_model::NumFmt {
    use hwp_model::NumFmt;
    match s {
        "HANGUL_SYLLABLE" => NumFmt::HangulSyllable,
        "HANGUL_JAMO" => NumFmt::HangulJamo,
        "CIRCLED_DIGIT" => NumFmt::CircledDigit,
        "LATIN_CAPITAL" | "LATIN_UPPER" => NumFmt::LatinUpper,
        "LATIN_SMALL" | "LATIN_LOWER" => NumFmt::LatinLower,
        "ROMAN_CAPITAL" | "ROMAN_UPPER" => NumFmt::RomanUpper,
        "ROMAN_SMALL" | "ROMAN_LOWER" => NumFmt::RomanLower,
        _ => NumFmt::Digit,
    }
}

/// OWPML tabItem type → hwp5 탭 종류 코드(§4.2.7): 왼0/오른1/가운데2/소수점3.
fn tab_kind_code(s: &str) -> u8 {
    match s {
        "RIGHT" => 1,
        "CENTER" => 2,
        "DECIMAL" => 3,
        _ => 0, // LEFT
    }
}

/// hwp5 ParaShape::alignment()과 같은 인코딩으로 정렬 매핑.
fn alignment_code(s: &str) -> u32 {
    match s {
        "JUSTIFY" => 0,
        "LEFT" => 1,
        "RIGHT" => 2,
        "CENTER" => 3,
        "DISTRIBUTE" => 4,
        "DISTRIBUTE_SPACE" => 5,
        _ => 0,
    }
}

pub fn parse_header(xml: &str) -> Result<(DocHeader, Vec<String>)> {
    let mut header = DocHeader::default();
    let mut warnings = Vec::new();
    let mut reader = Reader::from_str(xml);

    // 현재 컨텍스트
    let mut current_lang: Option<usize> = None;
    let mut current_char: Option<CharShape> = None;
    let mut current_para: Option<ParaShape> = None;
    let mut current_border: Option<BorderFill> = None;
    let mut current_numbering: Option<Vec<hwp_model::NumLevel>> = None;
    // HWPX 정의 id는 임의 값일 수 있다. IR에서는 HWP5와 동일하게 Vec의 0-based
    // 인덱스로 정규화하므로 외부 id→내부 인덱스 맵을 별도로 유지한다.
    let mut numbering_ids: HashMap<u16, u16> = HashMap::new();
    let mut bullet_ids: HashMap<u16, u16> = HashMap::new();
    let mut current_tab: Option<hwp_model::TabDef> = None;
    // Active hc:gradation state inside borderFill: radial flag, angle, colors.
    let mut current_gradation: Option<(bool, f32, Vec<u32>)> = None;
    // 정품 tabItem은 hp:switch로 감싸 case(HwpUnitChar, unit=HWPUNIT, pos=X)와
    // default(unit 없음, pos=2X)를 함께 낸다. case만 취하고 default는 버리기 위해
    // hp:default 안에 있는 동안 tabItem 수집을 막는다(중복 방지). switch 없는 naked
    // tabItem(구형 출력·타 구현체)은 이 플래그가 꺼진 채라 그대로 읽힌다.
    let mut in_tab_default = false;
    // paraHead 텍스트(형식 템플릿 "^1." 등)를 담을 현재 수준(1-기반).
    let mut current_para_head: Option<usize> = None;
    // hp:switch의 case/default 중복 — 첫 분기(신형 한글이 읽는 값)만 취한다
    let mut para_margin_done = false;
    let mut para_ls_done = false;

    loop {
        let event = reader.read_event().map_err(|e| HwpxError::Xml {
            entry: "Contents/header.xml".to_string(),
            message: e.to_string(),
        })?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(event, Event::Empty(_));
                match e.local_name().as_ref() {
                    b"beginNum" => {
                        header.properties.start_numbers = [
                            attr_u16(e, "page").unwrap_or(1),
                            attr_u16(e, "footnote").unwrap_or(1),
                            attr_u16(e, "endnote").unwrap_or(1),
                            attr_u16(e, "pic").unwrap_or(1),
                            attr_u16(e, "tbl").unwrap_or(1),
                            attr_u16(e, "equation").unwrap_or(1),
                        ];
                    }
                    b"fontface" => {
                        current_lang = attr(e, "lang").as_deref().and_then(lang_slot);
                        if current_lang.is_none() {
                            warnings
                                .push(format!("알 수 없는 fontface lang: {:?}", attr(e, "lang")));
                        }
                    }
                    b"font" => {
                        if let Some(slot) = current_lang {
                            header.fonts[slot].push(FaceName {
                                name: attr(e, "face").unwrap_or_default(),
                                ..FaceName::default()
                            });
                        }
                    }
                    b"substFont" => {
                        // OWPML stores a font's fallback face as a nested
                        // <hh:substFont face="..."/>.  The HWP5-compatible IR
                        // keeps that name on FaceName::alt_name; do not let the
                        // nested element disappear while parsing the flat
                        // reference list.
                        if let Some(slot) = current_lang
                            && let Some(font) = header.fonts[slot].last_mut()
                        {
                            font.alt_name = attr(e, "face").or_else(|| attr(e, "name"));
                        }
                    }
                    b"charPr" => {
                        let mut attr_bits = 0u32;
                        if attr(e, "useFontSpace").as_deref() == Some("1") {
                            attr_bits |= 1 << 25;
                        }
                        if attr(e, "useKerning").as_deref() == Some("1") {
                            attr_bits |= 1 << 30;
                        }
                        // Store symMark in attribute bits 21..=24.
                        if let Some(mark) = attr(e, "symMark") {
                            attr_bits |= u32::from(sym_mark_code(&mark)) << 21;
                        }
                        let cs = CharShape {
                            base_size: attr_i32(e, "height").unwrap_or(1000),
                            text_color: attr(e, "textColor").map_or(0, |c| parse_color(&c)),
                            shade_color: attr(e, "shadeColor")
                                .map_or(0xFFFF_FFFF, |c| parse_color(&c)),
                            ratios: [100; LANG_COUNT],
                            rel_sizes: [100; LANG_COUNT],
                            attr: attr_bits,
                            border_fill_id: attr_u16(e, "borderFillIDRef").unwrap_or(0),
                            ..CharShape::default()
                        };
                        if empty {
                            header.char_shapes.push(cs);
                        } else {
                            current_char = Some(cs);
                        }
                    }
                    // charPr 자식들
                    b"fontRef" => {
                        if let Some(cs) = &mut current_char {
                            for (i, name) in [
                                "hangul", "latin", "hanja", "japanese", "other", "symbol", "user",
                            ]
                            .iter()
                            .enumerate()
                            {
                                cs.face_ids[i] = attr_u16(e, name).unwrap_or(0);
                            }
                        }
                    }
                    b"ratio" | b"spacing" | b"relSz" | b"offset" => {
                        if let Some(cs) = &mut current_char {
                            let names = [
                                "hangul", "latin", "hanja", "japanese", "other", "symbol", "user",
                            ];
                            for (i, n) in names.iter().enumerate() {
                                let Some(v) = attr_i32(e, n) else { continue };
                                match e.local_name().as_ref() {
                                    b"ratio" => cs.ratios[i] = v.clamp(1, 255) as u8,
                                    b"spacing" => cs.spacings[i] = v.clamp(-128, 127) as i8,
                                    b"relSz" => cs.rel_sizes[i] = v.clamp(1, 255) as u8,
                                    _ => cs.offsets[i] = v.clamp(-128, 127) as i8,
                                }
                            }
                        }
                    }
                    b"bold" => {
                        if let Some(cs) = &mut current_char {
                            cs.attr |= 1 << 1;
                        }
                    }
                    b"italic" => {
                        if let Some(cs) = &mut current_char {
                            cs.attr |= 1;
                        }
                    }
                    b"underline" => {
                        if let Some(cs) = &mut current_char {
                            // BOTTOM→1, TOP→3 (hwp5 bits 2~3과 동일 인코딩)
                            let kind = match attr(e, "type").as_deref() {
                                Some("NONE") | None => 0u32,
                                Some("TOP") => 3,
                                _ => 1,
                            };
                            cs.attr |= kind << 2;
                            if let Some(c) = attr(e, "color") {
                                cs.underline_color = parse_color(&c);
                            }
                            // 밑줄 모양(SOLID/DASH/DOT…)을 테두리선 종류 코드로 보존.
                            // 미지정(0)이면 write가 기본 SOLID로 방출한다.
                            cs.underline_shape = attr(e, "shape").map_or(0, |s| line_type_code(&s));
                            // Store the normalized zero-based decoration code in bits
                            // 4..=7 only when the underline is active.
                            if kind != 0
                                && let Some(s) = attr(e, "shape")
                            {
                                cs.attr |= u32::from(decor_code(&s)) << 4;
                            }
                        }
                    }
                    b"strikeout" => {
                        // 취소선 비트(18)는 한글이 "보이는 취소선 선"으로 렌더하는 shape에만
                        // 켠다. shape="3D" 계열은 정품/한글에서 보이지 않게 렌더되는데(사용자
                        // 확인), 우리가 비트18(종류=1 실선)로 매핑하면 인라인 표 폭에 걸친
                        // 실선 취소선이 합성돼 목차에 가로선이 나타난다. NONE/3D는 비취소선
                        // 으로 둔다(시각 결과 동일, SOLID 등 실제 취소선은 보존).
                        if let Some(cs) = &mut current_char {
                            let shape = attr(e, "shape");
                            let visible = matches!(shape.as_deref(), Some(s) if s != "NONE" && !s.contains("3D"));
                            if visible {
                                cs.attr |= 1 << 18; // 바이트 왕복 보존
                                cs.strike = true; // 의미 플래그 (has_strike 근거)
                                // Store the zero-based strike shape in bits 26..=29.
                                if let Some(s) = shape.as_deref() {
                                    cs.attr |= u32::from(decor_code(s)) << 26;
                                }
                            }
                        }
                    }
                    // 위/아래 첨자(hwp5 attr bit15/16 — 스펙).
                    b"supscript" => {
                        if let Some(cs) = &mut current_char {
                            cs.attr |= 1 << 15;
                        }
                    }
                    b"subscript" => {
                        if let Some(cs) = &mut current_char {
                            cs.attr |= 1 << 16;
                        }
                    }
                    // 그림자(hwp5 attr bits11~12) + 색/간격.
                    b"shadow" => {
                        if let Some(cs) = &mut current_char
                            && attr(e, "type").as_deref() != Some("NONE")
                        {
                            cs.attr |= 1 << 11;
                            if let Some(c) = attr(e, "color") {
                                cs.shadow_color = parse_color(&c);
                            }
                            let gx = attr_i32(e, "offsetX").unwrap_or(0).clamp(-128, 127) as i8;
                            let gy = attr_i32(e, "offsetY").unwrap_or(0).clamp(-128, 127) as i8;
                            cs.shadow_gap = (gx, gy);
                        }
                    }
                    // 외곽선(bit8~10) / 양각(bit13) / 음각(bit14).
                    b"outline" => {
                        if let Some(cs) = &mut current_char
                            && attr(e, "type").as_deref() != Some("NONE")
                        {
                            cs.attr |= 1 << 8;
                        }
                    }
                    b"emboss" => {
                        if let Some(cs) = &mut current_char {
                            cs.attr |= 1 << 13;
                        }
                    }
                    b"engrave" => {
                        if let Some(cs) = &mut current_char {
                            cs.attr |= 1 << 14;
                        }
                    }
                    b"paraPr" => {
                        // paraPr 자체 속성을 attr1에 인코딩(정품 한글과 일치 — 강제
                        // 0x180 대신 실제 값). snapToGrid=bit8(줄 격자 사용),
                        // condense=bits9-15(공백 최소값), fontLineHeight=bit22.
                        // tabPrIDRef → tab_def_id.
                        let mut ps = ParaShape::default();
                        if attr(e, "snapToGrid").as_deref() == Some("1") {
                            ps.attr1 |= 1 << 8;
                        }
                        if let Some(c) = attr_u32(e, "condense") {
                            ps.attr1 |= (c & 0x7f) << 9;
                        }
                        if attr(e, "fontLineHeight").as_deref() == Some("1") {
                            ps.attr1 |= 1 << 22;
                        }
                        ps.tab_def_id = attr_u16(e, "tabPrIDRef").unwrap_or(0);
                        current_para = Some(ps);
                        para_margin_done = false;
                        para_ls_done = false;
                        if empty {
                            header
                                .para_shapes
                                .push(current_para.take().expect("방금 생성"));
                        }
                    }
                    b"align" => {
                        if let Some(ps) = &mut current_para
                            && let Some(h) = attr(e, "horizontal")
                        {
                            ps.attr1 |= alignment_code(&h) << 2;
                        }
                    }
                    // 문단 머리(목록): type→attr1 bit23~24, level→bit25~27, idRef→numbering_id.
                    b"heading" => {
                        if let Some(ps) = &mut current_para {
                            let ty = match attr(e, "type").as_deref() {
                                Some("OUTLINE") => 1u32,
                                Some("NUMBER") => 2,
                                Some("BULLET") => 3,
                                _ => 0,
                            };
                            ps.attr1 |= ty << 23;
                            if ty != 0 {
                                let level = attr_u32(e, "level").unwrap_or(1).clamp(1, 7);
                                ps.attr1 |= level << 25;
                                ps.numbering_id = attr_u16(e, "idRef").unwrap_or(0);
                            }
                        }
                    }
                    // 탭 정의(§4.2.7 / hh:tabPr): 자동탭 속성 + 자식 tabItem.
                    b"tabPr" => {
                        let mut td = hwp_model::TabDef::default();
                        if attr(e, "autoTabLeft").as_deref() == Some("1") {
                            td.attr |= 0x1;
                        }
                        if attr(e, "autoTabRight").as_deref() == Some("1") {
                            td.attr |= 0x2;
                        }
                        if empty {
                            header.tab_stops.push(td);
                        } else {
                            current_tab = Some(td);
                        }
                    }
                    // hp:default 안의 tabItem은 case의 중복(pos=2X)이므로 무시.
                    b"default" => {
                        if current_tab.is_some() {
                            in_tab_default = true;
                        }
                    }
                    b"tabItem" => {
                        if let Some(td) = &mut current_tab
                            && !in_tab_default
                        {
                            let pos = attr_i32(e, "pos").unwrap_or(0);
                            let kind = attr(e, "type").map_or(0, |t| tab_kind_code(&t));
                            // leader(리더/채움 선): 테두리선 종류 코드로 보존.
                            let fill = attr(e, "leader").map_or(0, |l| line_type_code(&l));
                            td.items.push(hwp_model::TabItem { pos, kind, fill });
                        }
                    }
                    // 번호 정의 시작 + 수준별 형식.
                    b"numbering" => {
                        let index = u16::try_from(header.numbering_levels.len()).map_err(|_| {
                            HwpxError::Xml {
                                entry: "Contents/header.xml".to_string(),
                                message: "번호 정의가 u16 범위를 초과합니다".to_string(),
                            }
                        })?;
                        let external_id = attr_u16(e, "id").unwrap_or(index.saturating_add(1));
                        // 중복 id는 첫 정의 우선(야생 파일 관용 — 하드 에러로 읽기를 막지 않는다).
                        if let std::collections::hash_map::Entry::Vacant(slot) =
                            numbering_ids.entry(external_id)
                        {
                            slot.insert(index);
                        } else {
                            warnings
                                .push(format!("중복 번호 정의 id: {external_id} (첫 정의 유지)"));
                        }
                        current_numbering = Some(Vec::new());
                        if empty {
                            header.numbering_levels.push(Vec::new());
                            current_numbering = None;
                        }
                    }
                    b"paraHead" => {
                        if let Some(levels) = &mut current_numbering {
                            let level = attr_u32(e, "level").unwrap_or(1).clamp(1, 10) as usize;
                            let nl = hwp_model::NumLevel {
                                start: attr_u32(e, "start").unwrap_or(1),
                                fmt: num_fmt(attr(e, "numFormat").as_deref().unwrap_or("DIGIT")),
                                template: String::new(),
                            };
                            if levels.len() < level {
                                levels.resize(level, hwp_model::NumLevel::default());
                            }
                            levels[level - 1] = nl;
                            // 비어 있지 않으면(텍스트 콘텐츠) 뒤따르는 Text를 템플릿으로 포착.
                            current_para_head = if empty { None } else { Some(level) };
                        }
                    }
                    // 글머리표 정의: char 속성(없으면 기본 •).
                    b"bullet" => {
                        let index = u16::try_from(header.bullet_chars.len()).map_err(|_| {
                            HwpxError::Xml {
                                entry: "Contents/header.xml".to_string(),
                                message: "글머리표 정의가 u16 범위를 초과합니다".to_string(),
                            }
                        })?;
                        let external_id = attr_u16(e, "id").unwrap_or(index.saturating_add(1));
                        // 중복 id는 첫 정의 우선(야생 파일 관용).
                        if let std::collections::hash_map::Entry::Vacant(slot) =
                            bullet_ids.entry(external_id)
                        {
                            slot.insert(index);
                        } else {
                            warnings.push(format!(
                                "중복 글머리표 정의 id: {external_id} (첫 정의 유지)"
                            ));
                        }
                        let ch = attr(e, "char")
                            .and_then(|s| s.chars().next())
                            .unwrap_or('•');
                        header.bullet_chars.push(ch);
                    }
                    b"intent" | b"left" | b"right" | b"prev" | b"next" => {
                        if let Some(ps) = &mut current_para
                            && !para_margin_done
                            && let Some(v) = attr_i32(e, "value")
                        {
                            // HWP5 PARA_SHAPE의 여백류 필드는 hwpx HWPUNIT의 2배 단위다
                            // (정품 한글 변환 실측: hwpx left=1500 → hwp5 ml=3000). ×2를
                            // 누락하면 간격이 절반이라 본문이 빽빽해지고 페이지네이션이
                            // 어긋난다. line_spacing(아래)은 2배 아님. IR은 hwp5 단위로 통일.
                            let v2 = v.saturating_mul(2);
                            match e.local_name().as_ref() {
                                b"intent" => ps.indent = v2,
                                b"left" => ps.margin_left = v2,
                                b"right" => ps.margin_right = v2,
                                b"prev" => ps.spacing_top = v2,
                                _ => ps.spacing_bottom = v2,
                            }
                        }
                    }
                    b"lineSpacing" => {
                        if let Some(ps) = &mut current_para
                            && !para_ls_done
                        {
                            para_ls_done = true;
                            let lstype: u8 = match attr(e, "type").as_deref() {
                                Some("FIXED") => 1,
                                Some("BETWEEN_LINES") => 2,
                                Some("AT_LEAST") => 3,
                                _ => 0, // PERCENT
                            };
                            ps.line_spacing_type = lstype;
                            let value = attr_i32(e, "value").unwrap_or(160);
                            // PERCENT(0)는 비율(%) 그대로. 길이 종류(고정/여백만/최소)는
                            // 여백류처럼 hwp5에서 HWPUNIT의 2배 단위(실측: 고정 1432→2864).
                            let stored = if lstype == 0 {
                                value
                            } else {
                                value.saturating_mul(2)
                            };
                            ps.line_spacing = stored;
                            // line_spacing_old(@24)에도 넣는다 — 한글은 이 필드로 문단
                            // 줄간격을 읽으며, 0이면 ensure_para_shape_defaults가 160으로
                            // 덮어써 문단별 줄간격(170/130 등)을 잃고 페이지가 밀린다.
                            ps.line_spacing_old = stored;
                            // 줄간격 종류를 attr1 bits0-1에 인코딩(PERCENT면 0).
                            ps.attr1 |= u32::from(lstype) & 0x3;
                        }
                    }
                    b"breakSetting" => {
                        if let Some(ps) = &mut current_para {
                            // 한글 줄나눔: KEEP_WORD(어절)=bit7=1, BREAK_WORD(글자)=0.
                            // 정품 실측: BREAK_WORD 문단은 글자 단위로 빽빽하게 줄바꿈.
                            if attr(e, "breakNonLatinWord").as_deref() != Some("BREAK_WORD") {
                                ps.attr1 |= 1 << 7;
                            }
                            // 영어 줄나눔: bits5-6 (KEEP_WORD=0, HYPHENATION=1, BREAK_WORD=2).
                            let blat = match attr(e, "breakLatinWord").as_deref() {
                                Some("BREAK_WORD") => 2u32,
                                Some("HYPHENATION") => 1,
                                _ => 0,
                            };
                            ps.attr1 |= blat << 5;
                            if attr(e, "widowOrphan").as_deref() == Some("1") {
                                ps.attr1 |= 1 << 16;
                            }
                            if attr(e, "keepWithNext").as_deref() == Some("1") {
                                ps.attr1 |= 1 << 17;
                            }
                            if attr(e, "keepLines").as_deref() == Some("1") {
                                ps.attr1 |= 1 << 18;
                            }
                            if attr(e, "pageBreakBefore").as_deref() == Some("1") {
                                ps.attr1 |= 1 << 19;
                            }
                        }
                    }
                    b"border" => {
                        // 문단 테두리/배경 참조(borderFillIDRef). 강제값(2) 대신 실제 ID.
                        if let Some(ps) = &mut current_para
                            && let Some(bf) = attr_u16(e, "borderFillIDRef")
                        {
                            ps.border_fill_id = bf;
                        }
                    }
                    b"typeInfo" => {
                        if let Some(slot) = current_lang
                            && let Some(font) = header.fonts[slot].last_mut()
                        {
                            let mut attrs = String::new();
                            for a in e.attributes().flatten() {
                                let k = String::from_utf8_lossy(a.key.local_name().as_ref())
                                    .into_owned();
                                let v = String::from_utf8_lossy(&a.value).into_owned();
                                attrs.push_str(&format!(r#" {k}="{v}""#));
                            }
                            font.type_info = Some(attrs);
                        }
                    }
                    b"borderFill" => {
                        current_border = Some(BorderFill::default());
                        if empty {
                            header
                                .border_fills
                                .push(current_border.take().expect("방금 생성"));
                        }
                    }
                    // 대각선 방향: <hh:slash>/<hh:backSlash> type≠NONE이면 hwp5 attr 비트로 합성
                    // (slash=bit2, backSlash=bit5 — 렌더러 diagonal_dirs와 동일 인코딩).
                    b"slash" | b"backSlash" => {
                        if let Some(bf) = &mut current_border
                            && attr(e, "type").is_some_and(|t| !t.eq_ignore_ascii_case("NONE"))
                        {
                            bf.attr |= if e.local_name().as_ref() == b"slash" {
                                0x4
                            } else {
                                0x20
                            };
                        }
                    }
                    b"leftBorder" | b"rightBorder" | b"topBorder" | b"bottomBorder" => {
                        if let Some(bf) = &mut current_border {
                            let idx = match e.local_name().as_ref() {
                                b"leftBorder" => 0,
                                b"rightBorder" => 1,
                                b"topBorder" => 2,
                                _ => 3,
                            };
                            bf.sides[idx] = parse_border_line(e);
                        }
                    }
                    b"diagonal" => {
                        if let Some(bf) = &mut current_border {
                            bf.diagonal = parse_border_line(e);
                        }
                    }
                    b"winBrush" => {
                        if let Some(bf) = &mut current_border
                            && let Some(c) = attr(e, "faceColor")
                        {
                            bf.fill_type |= 0x1;
                            bf.bg_color = Some(parse_color(&c));
                        }
                        // Genuine HWPX also uses constant hatchColor values for solid fills,
                        // so do not infer a hatch from this attribute. Only HWP5 preserves it.
                    }
                    // hc:gradation is a border-fill gradient (GG-7). The flat event loop
                    // retains explicit state while collecting its colors.
                    b"gradation" if current_border.is_some() && !empty => {
                        let gtype = attr(e, "type").unwrap_or_default();
                        current_gradation = Some((
                            !gtype.eq_ignore_ascii_case("LINEAR"),
                            attr_i32(e, "angle").unwrap_or(0) as f32,
                            Vec::new(),
                        ));
                    }
                    b"color" => {
                        if let Some((_, _, colors)) = &mut current_gradation
                            && let Some(v) = attr(e, "value")
                        {
                            colors.push(parse_color(&v));
                        }
                    }
                    b"style" => {
                        header.styles.push(Style {
                            name: attr(e, "name").unwrap_or_default(),
                            english_name: attr(e, "engName").unwrap_or_default(),
                            para_shape: ParaShapeId(attr_u16(e, "paraPrIDRef").unwrap_or(0)),
                            char_shape: CharShapeId(attr_u16(e, "charPrIDRef").unwrap_or(0)),
                            next_style: attr_u32(e, "nextStyleIDRef").unwrap_or(0) as u8,
                            lang_id: attr_i32(e, "langID").unwrap_or(0) as i16,
                            ..Style::default()
                        });
                    }
                    _ => {}
                }
            }
            Event::End(ref e) => match e.local_name().as_ref() {
                b"fontface" => current_lang = None,
                b"margin" => {
                    if current_para.is_some() {
                        para_margin_done = true;
                    }
                }
                b"charPr" => {
                    if let Some(cs) = current_char.take() {
                        header.char_shapes.push(cs);
                    }
                }
                b"borderFill" => {
                    if let Some(bf) = current_border.take() {
                        header.border_fills.push(bf);
                    }
                }
                b"gradation" => {
                    if let (Some(bf), Some((radial, angle_deg, colors))) =
                        (&mut current_border, current_gradation.take())
                        && colors.len() >= 2
                    {
                        let last = (colors.len() - 1) as f32;
                        let stops = colors
                            .into_iter()
                            .enumerate()
                            .map(|(i, c)| (i as f32 / last, c))
                            .collect();
                        bf.fill_type |= 0x4;
                        bf.gradient = Some(hwp_model::GradientSpec {
                            radial,
                            angle_deg,
                            stops,
                        });
                    }
                }
                b"paraPr" => {
                    if let Some(ps) = current_para.take() {
                        header.para_shapes.push(ps);
                    }
                }
                b"default" => in_tab_default = false,
                b"tabPr" => {
                    if let Some(td) = current_tab.take() {
                        header.tab_stops.push(td);
                    }
                    in_tab_default = false;
                }
                b"numbering" => {
                    if let Some(levels) = current_numbering.take() {
                        header.numbering_levels.push(levels);
                    }
                }
                b"paraHead" => current_para_head = None,
                _ => {}
            },
            Event::Text(ref t) => {
                // paraHead 안이면 텍스트를 형식 템플릿으로 포착("^1." "(^2)" "제^3조").
                if let Some(level) = current_para_head
                    && let Some(levels) = &mut current_numbering
                    && let Some(nl) = levels.get_mut(level - 1)
                    && let Ok(s) = t.xml10_content()
                {
                    nl.template.push_str(s.trim());
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    // paraPr이 정의보다 먼저 등장해도 처리되도록 전체 파싱 뒤 idRef를 정규화한다.
    for ps in &mut header.para_shapes {
        let raw = ps.numbering_id;
        let mapped = match ps.head_type() {
            2 => numbering_ids.get(&raw),
            3 => bullet_ids.get(&raw),
            _ => continue,
        };
        // 미정의 idRef는 야생 파일에서 관찰될 수 있다 — 읽기를 막지 않고 첫 정의(0)로
        // 폴백한다(렌더/마커는 기본 형식으로 근사, writer는 항상 정의 1개 이상 방출).
        ps.numbering_id = match mapped {
            Some(index) => *index,
            None => {
                warnings.push(format!(
                    "정의되지 않은 문단 머리 idRef: {raw} (기본 정의로 폴백)"
                ));
                0
            }
        };
    }

    Ok((header, warnings))
}
