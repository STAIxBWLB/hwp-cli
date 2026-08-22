//! [`DocHeader`] → `Contents/header.xml`.
//!
//! 기준 표본의 요소 구조를 따른다. 알 수 없는/미보존 속성은 한글 빈 문서
//! 기본값으로 채운다 — 의미 무손실이 아니라 "유효한 문서" 우선.

use std::fmt::Write as _;

use hwp_model::{BorderLine, CharShape, DocHeader, LANG_COUNT, ParaShape};

use crate::write::templates::{FULL_XMLNS, color_attr, esc};

const LANG_NAMES: [&str; LANG_COUNT] = [
    "HANGUL", "LATIN", "HANJA", "JAPANESE", "OTHER", "SYMBOL", "USER",
];

pub(crate) fn line_type_name(code: u8) -> &'static str {
    match code {
        0 => "NONE",
        1 => "SOLID",
        2 => "DASH",
        3 => "DOT",
        4 => "DASH_DOT",
        5 => "DASH_DOT_DOT",
        6 => "LONG_DASH",
        7 => "CIRCLE",
        8 => "DOUBLE_SLIM",
        9 => "SLIM_THICK",
        10 => "THICK_SLIM",
        11 => "SLIM_THICK_SLIM",
        _ => "SOLID",
    }
}

/// Converts a zero-based decoration code from HWP5 attribute bits 4..=7 or
/// 26..=29 to an OWPML line-shape name, including WAVE and DOUBLE_WAVE.
fn decor_name(code: u8) -> &'static str {
    match code {
        11 => "WAVE",
        12 => "DOUBLE_WAVE",
        c => line_type_name(c.saturating_add(1)),
    }
}

/// Converts an emphasis code from HWP5 attribute bits 21..=24 to an OWPML
/// `symMark` name using hwpxlib's `SymMarkSort` ordering.
fn sym_mark_name(code: u8) -> &'static str {
    match code {
        1 => "DOT_ABOVE",
        2 => "RING_ABOVE",
        3 => "TILDE",
        4 => "CARON",
        5 => "SIDE",
        6 => "COLON",
        7 => "GRAVE_ACCENT",
        8 => "ACUTE_ACCENT",
        9 => "CIRCUMFLEX",
        10 => "MACRON",
        11 => "HOOK_ABOVE",
        12 => "DOT_BELOW",
        _ => "NONE",
    }
}

pub(crate) fn width_mm_attr(line: &BorderLine) -> String {
    // 0.12 같은 값은 그대로, 정수는 "0.1"이 아닌 표기 유지
    let mm = line.width_mm();
    if (mm - mm.round()).abs() < f32::EPSILON {
        format!("{} mm", mm.round() as i32)
    } else {
        format!("{mm} mm")
    }
}

pub fn write_header(header: &DocHeader, section_count: usize) -> String {
    let mut out = String::with_capacity(32 * 1024);
    let _ = write!(
        out,
        r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><hh:head {FULL_XMLNS} version="1.5" secCnt="{section_count}">"##
    );
    let p = &header.properties;
    let _ = write!(
        out,
        r##"<hh:beginNum page="{}" footnote="{}" endnote="{}" pic="{}" tbl="{}" equation="{}"/>"##,
        p.start_numbers[0].max(1),
        p.start_numbers[1].max(1),
        p.start_numbers[2].max(1),
        p.start_numbers[3].max(1),
        p.start_numbers[4].max(1),
        p.start_numbers[5].max(1),
    );
    out.push_str("<hh:refList>");
    write_fontfaces(&mut out, header);
    write_border_fills(&mut out, header);
    write_char_properties(&mut out, header);
    write_tab_properties(&mut out, header);
    write_numberings(&mut out, header);
    write_bullets(&mut out, header);
    write_para_properties(&mut out, header);
    write_styles(&mut out, header);
    out.push_str("</hh:refList>");
    out.push_str(
        r##"<hh:compatibleDocument targetProgram="HWP201X"><hh:layoutCompatibility/></hh:compatibleDocument><hh:docOption><hh:linkinfo path="" pageInherit="0" footnoteInherit="0"/></hh:docOption></hh:head>"##,
    );
    out
}

fn write_fontfaces(out: &mut String, header: &DocHeader) {
    let _ = write!(out, r##"<hh:fontfaces itemCnt="{LANG_COUNT}">"##);
    for (slot, lang) in LANG_NAMES.iter().enumerate() {
        let fonts = &header.fonts[slot];
        if fonts.is_empty() {
            // 빈 슬롯 방어: 기본 글꼴 1종
            let _ = write!(
                out,
                r##"<hh:fontface lang="{lang}" fontCnt="1"><hh:font id="0" face="함초롬바탕" type="TTF" isEmbedded="0"/></hh:fontface>"##
            );
            continue;
        }
        let _ = write!(
            out,
            r##"<hh:fontface lang="{lang}" fontCnt="{}">"##,
            fonts.len()
        );
        for (i, f) in fonts.iter().enumerate() {
            if f.type_info.is_some() || f.alt_name.is_some() {
                let _ = write!(
                    out,
                    r##"<hh:font id="{i}" face="{}" type="TTF" isEmbedded="0">"##,
                    esc(&f.name)
                );
                if let Some(alt_name) = &f.alt_name {
                    let _ = write!(out, r##"<hh:substFont face="{}"/>"##, esc(alt_name));
                }
                if let Some(attrs) = &f.type_info {
                    let _ = write!(out, r##"<hh:typeInfo{attrs}/>"##);
                }
                out.push_str("</hh:font>");
            } else {
                let _ = write!(
                    out,
                    r##"<hh:font id="{i}" face="{}" type="TTF" isEmbedded="0"/>"##,
                    esc(&f.name)
                );
            }
        }
        out.push_str("</hh:fontface>");
    }
    out.push_str("</hh:fontfaces>");
}

fn write_border_line(out: &mut String, el: &str, line: &BorderLine) {
    let _ = write!(
        out,
        r##"<hh:{el} type="{}" width="{}" color="{}"/>"##,
        line_type_name(line.line_type),
        width_mm_attr(line),
        if line.color == 0xFFFF_FFFF {
            "#000000".to_string()
        } else {
            color_attr(line.color)
        },
    );
}

fn write_border_fills(out: &mut String, header: &DocHeader) {
    // 참조는 1-기반이므로 최소 2개(기본 무테두리)를 보장
    let count = header.border_fills.len().max(2);
    let _ = write!(out, r##"<hh:borderFills itemCnt="{count}">"##);
    let default_bf = hwp_model::BorderFill {
        diagonal: BorderLine {
            line_type: 1,
            width: 0,
            color: 0,
        },
        ..hwp_model::BorderFill::default()
    };
    for i in 0..count {
        let bf = header.border_fills.get(i).unwrap_or(&default_bf);
        let _ = write!(
            out,
            r##"<hh:borderFill id="{}" threeD="{}" shadow="{}" centerLine="NONE" breakCellSeparateLine="0">"##,
            i + 1,
            bf.attr & 1,
            (bf.attr >> 1) & 1,
        );
        out.push_str(r##"<hh:slash type="NONE" Crooked="0" isCounter="0"/><hh:backSlash type="NONE" Crooked="0" isCounter="0"/>"##);
        for (el, line) in ["leftBorder", "rightBorder", "topBorder", "bottomBorder"]
            .iter()
            .zip(&bf.sides)
        {
            write_border_line(out, el, line);
        }
        write_border_line(out, "diagonal", &bf.diagonal);
        if let Some(g) = &bf.gradient
            && g.stops.len() >= 2
        {
            // GG-7 gradient fill in the same form emitted for shapes.
            let _ = write!(
                out,
                r##"<hc:fillBrush><hc:gradation type="{}" angle="{}" centerX="{}" centerY="{}" step="{}" colorNum="{}" stepCenter="50" alpha="0">"##,
                if g.radial { "RADIAL" } else { "LINEAR" },
                g.angle_deg.round() as i32,
                g.center_x,
                g.center_y,
                g.step,
                g.stops.len(),
            );
            for (_, c) in &g.stops {
                let _ = write!(out, r##"<hc:color value="{}"/>"##, color_attr(*c));
            }
            out.push_str("</hc:gradation></hc:fillBrush>");
        } else if let Some(bg) = bf.visible_bg() {
            // Carry a hatch color when present. HWPX has no mapped field for the hatch
            // style, so that value is lost pending comparison with genuine Hangul output.
            // Otherwise retain the constant observed in genuine files.
            let hatch = bf
                .hatch
                .map(|(c, _)| color_attr(c))
                .unwrap_or_else(|| "#999999".to_string());
            let _ = write!(
                out,
                r##"<hc:fillBrush><hc:winBrush faceColor="{}" hatchColor="{hatch}" alpha="0"/></hc:fillBrush>"##,
                color_attr(bg)
            );
        }
        out.push_str("</hh:borderFill>");
    }
    out.push_str("</hh:borderFills>");
}

fn per_lang_attrs(values: impl IntoIterator<Item = i32>) -> String {
    let mut s = String::new();
    for (name, v) in [
        "hangul", "latin", "hanja", "japanese", "other", "symbol", "user",
    ]
    .iter()
    .zip(values)
    {
        let _ = write!(s, r##" {name}="{v}""##);
    }
    s
}

fn write_char_properties(out: &mut String, header: &DocHeader) {
    let shapes: Vec<&CharShape> = header.char_shapes.iter().collect();
    let count = shapes.len().max(1);
    let _ = write!(out, r##"<hh:charProperties itemCnt="{count}">"##);
    let default_cs = CharShape {
        base_size: 1000,
        ..CharShape::default()
    };
    for i in 0..count {
        let cs = shapes.get(i).copied().unwrap_or(&default_cs);
        let bf_ref = if cs.border_fill_id > 0 {
            cs.border_fill_id
        } else {
            2
        };
        let _ = write!(
            out,
            r##"<hh:charPr id="{i}" height="{}" textColor="{}" shadeColor="{}" useFontSpace="{}" useKerning="{}" symMark="{}" borderFillIDRef="{bf_ref}">"##,
            cs.base_size,
            color_attr(cs.text_color),
            color_attr(cs.shade_color),
            (cs.attr >> 25) & 1,
            (cs.attr >> 30) & 1,
            sym_mark_name(cs.emphasis_kind()),
        );
        let _ = write!(
            out,
            "<hh:fontRef{}/>",
            per_lang_attrs(cs.face_ids.iter().map(|&v| i32::from(v)))
        );
        let _ = write!(
            out,
            "<hh:ratio{}/>",
            per_lang_attrs(cs.ratios.iter().map(|&v| i32::from(v.max(1))))
        );
        let _ = write!(
            out,
            "<hh:spacing{}/>",
            per_lang_attrs(cs.spacings.iter().map(|&v| i32::from(v)))
        );
        let _ = write!(
            out,
            "<hh:relSz{}/>",
            per_lang_attrs(cs.rel_sizes.iter().map(|&v| i32::from(v.max(1))))
        );
        let _ = write!(
            out,
            "<hh:offset{}/>",
            per_lang_attrs(cs.offsets.iter().map(|&v| i32::from(v)))
        );
        if cs.is_italic() {
            out.push_str("<hh:italic/>");
        }
        if cs.is_bold() {
            out.push_str("<hh:bold/>");
        }
        let ul_type = match cs.underline_kind() {
            1 => "BOTTOM",
            3 => "TOP",
            _ => "NONE",
        };
        let ul_shape = decor_name(cs.underline_shape_code());
        let _ = write!(
            out,
            r##"<hh:underline type="{ul_type}" shape="{ul_shape}" color="{}"/>"##,
            if cs.underline_color == 0xFFFF_FFFF {
                "#000000".to_string()
            } else {
                color_attr(cs.underline_color)
            }
        );
        // Restore the strike shape from normalized attribute bits 26..=29.
        let strike_shape = if cs.has_strike() {
            decor_name(cs.strike_shape_code())
        } else {
            "NONE"
        };
        let _ = write!(
            out,
            r##"<hh:strikeout shape="{strike_shape}" color="#000000"/>"##
        );
        // 외곽선: 유무만 보존(read가 종류를 버림) — 있으면 SOLID, 없으면 NONE.
        let _ = write!(
            out,
            r##"<hh:outline type="{}"/>"##,
            if cs.has_outline() { "SOLID" } else { "NONE" }
        );
        // 그림자: 있으면 종류 DROP + 보존한 색/간격, 없으면 기존 상수(빈 문서 기본).
        if cs.has_shadow() {
            let _ = write!(
                out,
                r##"<hh:shadow type="DROP" color="{}" offsetX="{}" offsetY="{}"/>"##,
                color_attr(cs.shadow_color),
                cs.shadow_gap.0,
                cs.shadow_gap.1,
            );
        } else {
            out.push_str(r##"<hh:shadow type="NONE" color="#C0C0C0" offsetX="10" offsetY="10"/>"##);
        }
        // 양각/음각/위첨자/아래첨자: OWPML 스키마 순서상 shadow 뒤. 켜진 것만 방출.
        if cs.is_emboss() {
            out.push_str("<hh:emboss/>");
        }
        if cs.is_engrave() {
            out.push_str("<hh:engrave/>");
        }
        if cs.is_superscript() {
            out.push_str("<hh:supscript/>");
        }
        if cs.is_subscript() {
            out.push_str("<hh:subscript/>");
        }
        out.push_str("</hh:charPr>");
    }
    out.push_str("</hh:charProperties>");
}

/// 탭 종류 코드 → OWPML tabItem type(읽기 tab_kind_code의 역).
fn tab_kind_name(kind: u8) -> &'static str {
    match kind {
        1 => "RIGHT",
        2 => "CENTER",
        3 => "DECIMAL",
        _ => "LEFT",
    }
}

/// 탭 리더(채움 선) 코드 → OWPML leader 값.
///
/// 정품 한글이 저장한 hwpx 2종에서 실제 관찰된 leader 값은 NONE/DASH뿐이다.
/// 테두리선 종류(line_type_name)를 그대로 쓰면 한글이 tab leader로 허용하는지
/// 확인되지 않은 값(DASH_DOT·CIRCLE·DOUBLE_SLIM 등)까지 방출하게 된다. 따라서
/// tab 전용으로 분리해, 확신 가능한 근거값(NONE/SOLID/DASH/DOT)만 그대로 내고
/// 그 밖의 코드는 가장 가까운 관찰값(DASH)으로 강등한다.
fn tab_leader_name(fill: u8) -> &'static str {
    match fill {
        0 => "NONE",
        1 => "SOLID",
        3 => "DOT",
        // 2(DASH) 및 확신 없는 그 밖의 코드(4~11 등)는 관찰값 DASH로 방출/강등.
        _ => "DASH",
    }
}

fn write_tab_properties(out: &mut String, header: &DocHeader) {
    // 의미 탭 정의(tab_stops)가 있으면 그 개수를, 없으면 기존 raw 개수를 따른다.
    let count = header.tab_stops.len().max(header.tab_defs.len()).max(1);
    let _ = write!(out, r##"<hh:tabProperties itemCnt="{count}">"##);
    for i in 0..count {
        match header.tab_stops.get(i) {
            // 보존된 탭 정의: 자동탭 속성 + 항목을 그대로 방출.
            Some(td) => {
                let left = u8::from(td.auto_tab_left());
                let right = u8::from(td.auto_tab_right());
                if td.items.is_empty() {
                    let _ = write!(
                        out,
                        r##"<hh:tabPr id="{i}" autoTabLeft="{left}" autoTabRight="{right}"/>"##
                    );
                } else {
                    let _ = write!(
                        out,
                        r##"<hh:tabPr id="{i}" autoTabLeft="{left}" autoTabRight="{right}">"##
                    );
                    // 정품 한글 구조: 항목마다 hp:switch로 감싸고, case(HwpUnitChar
                    // 네임스페이스)는 unit="HWPUNIT" + pos=X, default는 unit 없이 pos=2X
                    // (정확히 2배)를 낸다. naked tabItem을 직접 방출하면 한글이 먹통이 된다.
                    for item in &td.items {
                        let ty = tab_kind_name(item.kind);
                        let leader = tab_leader_name(item.fill);
                        let pos = item.pos;
                        let pos2 = pos.saturating_mul(2);
                        let _ = write!(
                            out,
                            concat!(
                                r##"<hp:switch><hp:case hp:required-namespace="http://www.hancom.co.kr/hwpml/2016/HwpUnitChar">"##,
                                r##"<hh:tabItem pos="{pos}" type="{ty}" leader="{leader}" unit="HWPUNIT"/></hp:case>"##,
                                r##"<hp:default><hh:tabItem pos="{pos2}" type="{ty}" leader="{leader}"/></hp:default></hp:switch>"##,
                            ),
                            pos = pos,
                            ty = ty,
                            leader = leader,
                            pos2 = pos2,
                        );
                    }
                    out.push_str("</hh:tabPr>");
                }
            }
            // 값 없음: 기존 빈 상수(바이트 동일).
            None => {
                let _ = write!(
                    out,
                    r##"<hh:tabPr id="{i}" autoTabLeft="0" autoTabRight="0"/>"##
                );
            }
        }
    }
    out.push_str("</hh:tabProperties>");
}

/// NumFmt → OWPML numFormat 문자열(읽기 num_fmt의 역).
fn num_format_name(fmt: hwp_model::NumFmt) -> &'static str {
    use hwp_model::NumFmt;
    match fmt {
        NumFmt::Digit => "DIGIT",
        NumFmt::HangulSyllable => "HANGUL_SYLLABLE",
        NumFmt::HangulJamo => "HANGUL_JAMO",
        NumFmt::CircledDigit => "CIRCLED_DIGIT",
        NumFmt::CircledHangulSyllable => "CIRCLED_HANGUL_SYLLABLE",
        NumFmt::LatinUpper => "LATIN_CAPITAL",
        NumFmt::LatinLower => "LATIN_SMALL",
        NumFmt::RomanUpper => "ROMAN_CAPITAL",
        NumFmt::RomanLower => "ROMAN_SMALL",
    }
}

fn write_numberings(out: &mut String, header: &DocHeader) {
    // hwpx 읽기는 수준 형식을 numbering_levels에 담는다(numberings는 hwp5 raw 전용).
    // 둘 중 큰 개수를 방출해 hwpx→hwpx 왕복에서 번호 정의 수를 잃지 않는다. IR의
    // numbering_id는 0-기반이라(포맷 경계에서 정규화됨) idRef=numbering_id+1이 항상
    // 정의 범위 안에 든다 — 정의 수만큼 방출하면 dangling이 없다.
    let count = header
        .numbering_levels
        .len()
        .max(header.numberings.len())
        .max(1);
    let _ = write!(out, r##"<hh:numberings itemCnt="{count}">"##);
    for i in 0..count {
        let _ = write!(out, r##"<hh:numbering id="{}" start="0">"##, i + 1);
        let levels = header.numbering_levels.get(i);
        let level_count = levels.map_or(7, |levels| levels.len().min(10));
        for level in 1..=level_count {
            // 보존된 수준 형식이 있으면 그 시작/형식/템플릿을, 없으면 기존 상수 기본.
            let nl = levels.and_then(|v| v.get(level - 1));
            let start = nl.map_or(1, |n| n.start);
            let numfmt = nl.map_or("DIGIT", |n| num_format_name(n.fmt));
            let template = match nl {
                Some(n) if !n.template.is_empty() => esc(&n.template),
                _ => format!("^{level}."),
            };
            let _ = write!(
                out,
                r##"<hh:paraHead start="{start}" level="{level}" align="LEFT" useInstWidth="1" autoIndent="1" widthAdjust="0" textOffsetType="PERCENT" textOffset="50" numFormat="{numfmt}" charPrIDRef="4294967295" checkable="0">{template}</hh:paraHead>"##
            );
        }
        out.push_str("</hh:numbering>");
    }
    out.push_str("</hh:numberings>");
}

fn write_bullets(out: &mut String, header: &DocHeader) {
    // IR의 글머리 참조(numbering_id)는 0-기반이라 idRef=numbering_id+1이 정의 범위 안에
    // 든다(포맷 경계에서 정규화됨). 정의 수만큼 방출하면 dangling이 없다.
    let count = header.bullet_chars.len().max(header.bullets.len());
    if count == 0 {
        return;
    }
    let _ = write!(out, r##"<hh:bullets itemCnt="{count}">"##);
    for i in 0..count {
        let ch = header.bullet_chars.get(i).copied().unwrap_or('•');
        let _ = write!(
            out,
            r##"<hh:bullet id="{}" char="{}" useImage="0"/>"##,
            i + 1,
            esc(&ch.to_string()),
        );
    }
    out.push_str("</hh:bullets>");
}

fn write_para_properties(out: &mut String, header: &DocHeader) {
    let count = header.para_shapes.len().max(1);
    let _ = write!(out, r##"<hh:paraProperties itemCnt="{count}">"##);
    let default_ps = ParaShape::default();
    let tab_count = header.tab_stops.len().max(header.tab_defs.len()).max(1);
    for i in 0..count {
        let ps = header.para_shapes.get(i).unwrap_or(&default_ps);
        let align = match ps.alignment() {
            1 => "LEFT",
            2 => "RIGHT",
            3 => "CENTER",
            4 => "DISTRIBUTE",
            5 => "DISTRIBUTE_SPACE",
            _ => "JUSTIFY",
        };
        let tab_ref = (ps.tab_def_id as usize).min(tab_count - 1);
        let ls_type = match ps.line_spacing_type {
            1 => "FIXED",
            2 => "BETWEEN_LINES",
            3 => "AT_LEAST",
            _ => "PERCENT",
        };
        // IR 줄간격: PERCENT는 비율 그대로, 길이 종류(고정/여백만/최소)는 2배 단위라
        // ÷2 해서 HWPUNIT로 환원(hwpx 읽기의 ×2와 대칭).
        let ls_raw = if ps.line_spacing > 0 {
            ps.line_spacing
        } else {
            160
        };
        let ls_value = if ps.line_spacing_type == 0 {
            ls_raw
        } else {
            ls_raw / 2
        };
        // IR 여백류는 hwp5 PARA_SHAPE 단위(HWPUNIT의 2배)다. hwpx는 HWPUNIT이므로
        // ÷2 해서 내보낸다(hwpx 읽기의 ×2와 대칭 — hwpx 왕복 보존).
        // 문단 테두리/배경 참조: 실제 border_fill_id(0이면 기본 2=무테두리).
        let border_ref = if ps.border_fill_id > 0 {
            ps.border_fill_id
        } else {
            2
        };
        // IR의 번호/글머리표 정의 참조는 HWP5와 동일한 0-based 인덱스다. HWPX
        // idRef는 writer가 순서대로 내는 1-based 정의 id로 되돌린다(인덱스 0도 유효).
        // 개요(OUTLINE)는 read가 정규화하지 않는 원시 idRef(정품 표본은 0)라 그대로 낸다.
        let heading = if ps.head_type() != 0 {
            let (hty, id_ref) = match ps.head_type() {
                1 => ("OUTLINE", u32::from(ps.numbering_id)),
                2 => ("NUMBER", u32::from(ps.numbering_id) + 1),
                _ => ("BULLET", u32::from(ps.numbering_id) + 1),
            };
            format!(
                r##"<hh:heading type="{hty}" idRef="{id_ref}" level="{}"/>"##,
                ps.head_level().saturating_sub(1),
            )
        } else {
            r##"<hh:heading type="NONE" idRef="0" level="0"/>"##.to_string()
        };
        // 줄나눔 설정: read/header.rs의 비트 매핑의 역방향(§1013 breakSetting).
        // bit7 clear가 정상값(BREAK_WORD)이라 unwritten ParaShape도 왕복이 맞는다.
        let break_non_latin_word = if ps.break_non_latin_keep_word() {
            "KEEP_WORD"
        } else {
            "BREAK_WORD"
        };
        let break_latin_word = match ps.break_latin_word() {
            2 => "BREAK_WORD",
            1 => "HYPHENATION",
            _ => "KEEP_WORD",
        };
        let widow_orphan = u8::from(ps.widow_orphan());
        let keep_with_next = u8::from(ps.keep_with_next());
        let keep_lines = u8::from(ps.keep_lines());
        let page_break_before = u8::from(ps.page_break_before());
        let _ = write!(
            out,
            r##"<hh:paraPr id="{i}" tabPrIDRef="{tab_ref}" condense="0" fontLineHeight="0" snapToGrid="1" suppressLineNumbers="0" checked="0" textDir="LTR"><hh:align horizontal="{align}" vertical="BASELINE"/>{heading}<hh:breakSetting breakLatinWord="{break_latin_word}" breakNonLatinWord="{break_non_latin_word}" widowOrphan="{widow_orphan}" keepWithNext="{keep_with_next}" keepLines="{keep_lines}" pageBreakBefore="{page_break_before}" lineWrap="BREAK"/><hh:autoSpacing eAsianEng="0" eAsianNum="0"/><hh:margin><hc:intent value="{}" unit="HWPUNIT"/><hc:left value="{}" unit="HWPUNIT"/><hc:right value="{}" unit="HWPUNIT"/><hc:prev value="{}" unit="HWPUNIT"/><hc:next value="{}" unit="HWPUNIT"/></hh:margin><hh:lineSpacing type="{ls_type}" value="{ls_value}" unit="HWPUNIT"/><hh:border borderFillIDRef="{border_ref}" offsetLeft="0" offsetRight="0" offsetTop="0" offsetBottom="0" connect="0" ignoreMargin="0"/></hh:paraPr>"##,
            ps.indent / 2,
            ps.margin_left / 2,
            ps.margin_right / 2,
            ps.spacing_top / 2,
            ps.spacing_bottom / 2,
        );
    }
    out.push_str("</hh:paraProperties>");
}

fn write_styles(out: &mut String, header: &DocHeader) {
    let para_count = header.para_shapes.len().max(1);
    let char_count = header.char_shapes.len().max(1);
    if header.styles.is_empty() {
        out.push_str(
            r##"<hh:styles itemCnt="1"><hh:style id="0" type="PARA" name="바탕글" engName="Normal" paraPrIDRef="0" charPrIDRef="0" nextStyleIDRef="0" langID="1042" lockForm="0"/></hh:styles>"##,
        );
        return;
    }
    let _ = write!(out, r##"<hh:styles itemCnt="{}">"##, header.styles.len());
    for (i, s) in header.styles.iter().enumerate() {
        let _ = write!(
            out,
            r##"<hh:style id="{i}" type="PARA" name="{}" engName="{}" paraPrIDRef="{}" charPrIDRef="{}" nextStyleIDRef="{}" langID="{}" lockForm="0"/>"##,
            esc(&s.name),
            esc(&s.english_name),
            (s.para_shape.0 as usize).min(para_count - 1),
            (s.char_shape.0 as usize).min(char_count - 1),
            s.next_style,
            if s.lang_id > 0 { s.lang_id } else { 1042 },
        );
    }
    out.push_str("</hh:styles>");
}
