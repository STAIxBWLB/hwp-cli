//! `Contents/header.xml` → [`DocHeader`].
//!
//! M2 scope: fonts (fontfaces), character shapes (charPr), paragraph shapes (paraPr alignment), and
//! styles. Borders/numbering are filled in later milestones.
use std::collections::HashMap;

use hwp_model::{
    BorderFill, BorderLine, CharShape, CharShapeId, DocHeader, FaceName, LANG_COUNT, ParaShape,
    ParaShapeId, Style,
};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::error::{HwpxError, Result};
use crate::read::xml::{attr, attr_i32, attr_u16, attr_u32, parse_color, resolve_entity};

/// OWPML border-line type -> hwp5 code.
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

/// "0.12 mm" -> nearest width index.
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

/// OWPML language name -> one of seven language-slot indices.
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

/// HWPX numFormat string -> NumFmt, defaulting to decimal for unknown names.
fn num_fmt(s: &str) -> hwp_model::NumFmt {
    use hwp_model::NumFmt;
    match s {
        "HANGUL_SYLLABLE" => NumFmt::HangulSyllable,
        "HANGUL_JAMO" => NumFmt::HangulJamo,
        "CIRCLED_DIGIT" => NumFmt::CircledDigit,
        "CIRCLED_HANGUL_SYLLABLE" => NumFmt::CircledHangulSyllable,
        "LATIN_CAPITAL" | "LATIN_UPPER" => NumFmt::LatinUpper,
        "LATIN_SMALL" | "LATIN_LOWER" => NumFmt::LatinLower,
        "ROMAN_CAPITAL" | "ROMAN_UPPER" => NumFmt::RomanUpper,
        "ROMAN_SMALL" | "ROMAN_LOWER" => NumFmt::RomanLower,
        _ => NumFmt::Digit,
    }
}

/// OWPML tabItem type -> hwp5 tab-kind code (section 4.2.7): left=0, right=1, center=2, decimal=3.
fn tab_kind_code(s: &str) -> u8 {
    match s {
        "RIGHT" => 1,
        "CENTER" => 2,
        "DECIMAL" => 3,
        _ => 0, // LEFT
    }
}

/// Maps alignment using the same encoding as hwp5 ParaShape::alignment().
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

    // Current context.
    let mut current_lang: Option<usize> = None;
    let mut current_char: Option<CharShape> = None;
    let mut current_para: Option<ParaShape> = None;
    let mut current_border: Option<BorderFill> = None;
    let mut current_numbering: Option<Vec<hwp_model::NumLevel>> = None;
    // HWPX definition IDs may be arbitrary. The IR normalizes them to zero-based Vec indices, like HWP5,
    // so retain separate external-ID -> internal-index maps.

    let mut numbering_ids: HashMap<u16, u16> = HashMap::new();
    let mut bullet_ids: HashMap<u16, u16> = HashMap::new();
    let mut current_tab: Option<hwp_model::TabDef> = None;
    // Active hc:gradation state inside borderFill: radial flag, angle,
    // center_x, center_y, step, colors.
    let mut current_gradation: Option<(bool, f32, i32, i32, i32, Vec<u32>)> = None;
    // Genuine tabItem entries are wrapped in hp:switch, with a case (HwpUnitChar, unit=HWPUNIT, pos=X)
    // and default (no unit, pos=2X). Suppress tabItem collection inside hp:default to retain only the
    // case and avoid duplicates. Unwrapped tabItem entries from older writers or other implementations
    // remain readable with this flag off.

    let mut in_tab_default = false;
    // Current one-based level capturing paraHead text (format templates such as "^1.").
    let mut current_para_head: Option<usize> = None;
    // hp:switch case/default duplication: retain only the first branch, which newer Hangul reads.
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
                match e.local_name().as_ref().as_bytes() {
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
                    // charPr children.
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
                                match e.local_name().as_ref().as_bytes() {
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
                            // BOTTOM=1, TOP=3, matching hwp5 bits 2-3.
                            let kind = match attr(e, "type").as_deref() {
                                Some("NONE") | None => 0u32,
                                Some("TOP") => 3,
                                _ => 1,
                            };
                            cs.attr |= kind << 2;
                            if let Some(c) = attr(e, "color") {
                                cs.underline_color = parse_color(&c);
                            }
                            // Preserve underline styles (SOLID/DASH/DOT/...) as border-line type codes.
                            // The writer emits default SOLID when unspecified (0).

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
                        // Set strikeout bit 18 only for shapes Hangul renders as visible strikeout
                        // lines. Genuine files and owner confirmation show that "3D" shapes are
                        // invisible; mapping them to bit 18 (kind=1, solid) synthesizes a strikeout
                        // across inline table widths, adding horizontal lines to the contents. Keep
                        // NONE/3D non-struck for the same visual result while preserving real strikeouts
                        // such as SOLID.

                        if let Some(cs) = &mut current_char {
                            let shape = attr(e, "shape");
                            let visible = matches!(shape.as_deref(), Some(s) if s != "NONE" && !s.contains("3D"));
                            if visible {
                                cs.attr |= 1 << 18; // Preserve byte round-trips.
                                cs.strike = true; // Semantic flag used by has_strike.
                                // Store the zero-based strike shape in bits 26..=29.
                                if let Some(s) = shape.as_deref() {
                                    cs.attr |= u32::from(decor_code(s)) << 26;
                                }
                            }
                        }
                    }
                    // Superscript/subscript (hwp5 attr bits 15/16, per specification).
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
                    // Shadow (hwp5 attr bits 11-12), color and spacing.
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
                    // Outline (bits 8-10), emboss (bit 13), engrave (bit 14).
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
                        // Encode paraPr attributes into attr1, matching genuine Hangul values instead of
                        // forcing 0x180: snapToGrid=bit8, condense=bits9-15 (minimum spaces),
                        // fontLineHeight=bit22.

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
                    // Paragraph headings use zero-based HWPX levels, while the IR is one-based.
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
                                let hwp_level = attr_u32(e, "level").unwrap_or(0).min(9) + 1;
                                ps.attr1 |= hwp_level.min(7) << 25;
                                ps.list_level = (hwp_level > 7).then_some(hwp_level as u8);
                                ps.numbering_id = attr_u16(e, "idRef").unwrap_or(0);
                            }
                        }
                    }
                    // Tab definitions (section 4.2.7 / hh:tabPr): automatic-tab attributes and tabItem
                    // children.
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
                    // Ignore tabItem in hp:default: it duplicates the case with pos=2X.
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
                            // Preserve leader/fill lines as border-line type codes.
                            let fill = attr(e, "leader").map_or(0, |l| line_type_code(&l));
                            td.items.push(hwp_model::TabItem { pos, kind, fill });
                        }
                    }
                    // Start a numbering definition and its per-level formats.
                    b"numbering" => {
                        let index = u16::try_from(header.numbering_levels.len()).map_err(|_| {
                            HwpxError::Xml {
                                entry: "Contents/header.xml".to_string(),
                                message: "번호 정의가 u16 범위를 초과합니다".to_string(),
                            }
                        })?;
                        let external_id = attr_u16(e, "id").unwrap_or(index.saturating_add(1));
                        // The first definition wins for duplicate IDs, tolerating in-the-wild files
                        // instead of failing the read.
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
                            // For nonempty elements, capture following Text events as the template.
                            current_para_head = if empty { None } else { Some(level) };
                        }
                    }
                    // Bullet definition: char attribute, defaulting to the standard bullet.
                    b"bullet" => {
                        let index = u16::try_from(header.bullet_chars.len()).map_err(|_| {
                            HwpxError::Xml {
                                entry: "Contents/header.xml".to_string(),
                                message: "글머리표 정의가 u16 범위를 초과합니다".to_string(),
                            }
                        })?;
                        let external_id = attr_u16(e, "id").unwrap_or(index.saturating_add(1));
                        // The first definition wins for duplicate IDs in in-the-wild files.
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
                            // HWP5 PARA_SHAPE margin fields use twice the HWPX HWPUNIT (measured in
                            // genuine conversions: HWPX left=1500 -> HWP5 ml=3000). Missing the factor
                            // of two halves spacing, compresses body text and shifts pagination.
                            // line_spacing below is not doubled; the IR uses HWP5 units consistently.

                            let v2 = v.saturating_mul(2);
                            match e.local_name().as_ref().as_bytes() {
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
                            // PERCENT(0) keeps its percentage. Length kinds (fixed/space-only/minimum)
                            // use twice the HWPUNIT in HWP5, like margins (measured: fixed 1432 ->
                            // 2864).

                            let stored = if lstype == 0 {
                                value
                            } else {
                                value.saturating_mul(2)
                            };
                            ps.line_spacing = stored;
                            // Also fill line_spacing_old (@24): Hangul reads paragraph line spacing from
                            // it. A zero lets ensure_para_shape_defaults overwrite it with 160, losing
                            // per-paragraph spacing such as 170/130 and shifting pages.

                            ps.line_spacing_old = stored;
                            // Encode the line-spacing kind in attr1 bits 0-1 (PERCENT=0). Clear then
                            // set, rather than OR, following align_para's mask rule: OR cannot clear
                            // existing bits, so PERCENT could not replace an earlier kind. Currently
                            // attr1 starts at zero per paraPr and para_ls_done prevents duplicate
                            // lineSpacing, but preserve the declared XML kind even if that premise
                            // changes.

                            ps.attr1 = (ps.attr1 & !0x3) | (u32::from(lstype) & 0x3);
                        }
                    }
                    b"breakSetting" => {
                        if let Some(ps) = &mut current_para {
                            // Korean line breaking: KEEP_WORD=bit7=1, BREAK_WORD=0. Genuine BREAK_WORD
                            // paragraphs wrap densely at character boundaries.

                            if attr(e, "breakNonLatinWord").as_deref() != Some("BREAK_WORD") {
                                ps.attr1 |= 1 << 7;
                            }
                            // English line breaking: bits 5-6 (KEEP_WORD=0, HYPHENATION=1,
                            // BREAK_WORD=2).
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
                        // Paragraph border/fill reference (borderFillIDRef): use the actual ID instead
                        // of forcing 2.
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
                                let k = a.key.local_name().as_ref().to_owned();
                                let v = a.value.into_owned();
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
                    // Synthesize hwp5 diagonal-direction attr bits for <hh:slash>/<hh:backSlash> when
                    // type is not NONE (slash=bit2, backSlash=bit5), matching the renderer's
                    // diagonal_dirs encoding.
                    b"slash" | b"backSlash" => {
                        if let Some(bf) = &mut current_border
                            && attr(e, "type").is_some_and(|t| !t.eq_ignore_ascii_case("NONE"))
                        {
                            bf.attr |= if e.local_name().as_ref().as_bytes() == b"slash" {
                                0x4
                            } else {
                                0x20
                            };
                        }
                    }
                    b"leftBorder" | b"rightBorder" | b"topBorder" | b"bottomBorder" => {
                        if let Some(bf) = &mut current_border {
                            let idx = match e.local_name().as_ref().as_bytes() {
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
                    // An image fill's payload (`hc:img binaryItemIDRef`) is not modeled: no
                    // genuine HWPX sample establishes the element's attributes or how the
                    // referenced BinData stream is encoded, so the fill is marked instead of
                    // guessed and the writer ledgers the drop as
                    // `image_fill_unrepresentable` (#452).
                    b"imgBrush" => {
                        if let Some(bf) = &mut current_border {
                            bf.image_fill_dropped = true;
                        }
                    }
                    // hc:gradation is a border-fill gradient (GG-7). The flat event loop
                    // retains explicit state while collecting its colors.
                    b"gradation" if current_border.is_some() && !empty => {
                        let gtype = attr(e, "type").unwrap_or_default();
                        current_gradation = Some((
                            !gtype.eq_ignore_ascii_case("LINEAR"),
                            attr_i32(e, "angle").unwrap_or(0) as f32,
                            attr_i32(e, "centerX").unwrap_or(0),
                            attr_i32(e, "centerY").unwrap_or(0),
                            attr_i32(e, "step").unwrap_or(255),
                            Vec::new(),
                        ));
                    }
                    b"color" => {
                        if let Some((_, _, _, _, _, colors)) = &mut current_gradation
                            && let Some(v) = attr(e, "value")
                        {
                            colors.push(parse_color(&v));
                        }
                    }
                    b"style" => {
                        header.styles.push(Style {
                            name: attr(e, "name").unwrap_or_default(),
                            english_name: attr(e, "engName").unwrap_or_default(),
                            attr: style_kind_attr(attr(e, "type").as_deref()).unwrap_or(0),
                            kind_unknown: style_kind_attr(attr(e, "type").as_deref()).is_none(),
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
            Event::End(ref e) => match e.local_name().as_ref().as_bytes() {
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
                    if let (Some(bf), Some((radial, angle_deg, center_x, center_y, step, colors))) =
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
                            center_x,
                            center_y,
                            step,
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
                b"paraHead" => {
                    // Trim once, here: the template is captured in Text/GeneralRef chunks, so
                    // trimming per chunk would eat whitespace next to an entity.
                    if let Some(level) = current_para_head.take()
                        && let Some(levels) = &mut current_numbering
                        && let Some(nl) = levels.get_mut(level - 1)
                    {
                        let trimmed = nl.template.trim();
                        if trimmed.len() != nl.template.len() {
                            nl.template = trimmed.to_string();
                        }
                    }
                }
                _ => {}
            },
            Event::Text(ref t) => {
                // Inside paraHead, capture text as the format template ("^1.", "(^2)", etc.).
                if let Some(level) = current_para_head
                    && let Some(levels) = &mut current_numbering
                    && let Some(nl) = levels.get_mut(level - 1)
                {
                    nl.template.push_str(&t.xml10_content());
                }
            }
            // A `<{n}>` marker reaches the file as the paraHead `&lt;^2&gt;`: quick-xml
            // delivers a reference as its own GeneralRef event rather than inside Text.
            // Undecoded, `<`, `>` and `&` vanish from the template and compose then fails its
            // semantic verification on the reread mismatch (#487).
            Event::GeneralRef(ref r) => {
                if let Some(level) = current_para_head
                    && let Some(levels) = &mut current_numbering
                    && let Some(nl) = levels.get_mut(level - 1)
                    && let Some(c) = resolve_entity(r)
                {
                    nl.template.push(c);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    // Normalize idRef after parsing so paraPr can precede the definitions it references.
    for ps in &mut header.para_shapes {
        let raw = ps.numbering_id;
        let mapped = match ps.head_type() {
            2 => numbering_ids.get(&raw),
            3 => bullet_ids.get(&raw),
            _ => continue,
        };
        // Undefined idRef values occur in in-the-wild files. Keep reading and fall back to the first
        // definition (0); rendering/markers approximate the default format, and the writer always emits
        // at least one definition.

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

/// The HWP5 `STYLE.attr` value for `hh:style@type`: 0 for `PARA`, 1 for `CHAR`, and `None` when
/// the file does not state a kind this reader knows.
///
/// The IR carries style kind like the HWP5 reader (low three bits of `attr`, HWP file format 5.0 section
/// 4.2.11 table 48), so one reader-independent rule serves both formats. Genuine
/// Hangul-saved HWPX states `type` on every style: `fixtures/hwpx/minimal.hwpx` (Hangul 13,
/// Windows) has `쪽 번호` and `줄 번호` as `CHAR`, and `fixtures/hwpx/seal-groundtruth.hwpx`
/// (Hangul 12.30, macOS) has `쪽 번호` as `CHAR`, which is the style the genuine HWP5 files carry at
/// attr 1.
///
/// The format's own value set is exactly `PARA` and `CHAR` (Hancom's OWPML model
/// `hancom-io/hwpx-owpml-model` @ 1453388, `OWPML/Class/enumdef.h` `g_StyleList`). Its reader
/// leaves the member at 0 when the attribute is absent or unmatched (`CStyleType`, `Util.cpp`
/// `GetAttribute` returns false without assigning), but that is an implementation default of one
/// reader, not a statement by the format that a missing `type` means a paragraph style, and an
/// unmatched value is not even that. So neither is answered: `None` makes the caller keep attr 0
/// (so no reserved value reaches an HWP5 record) and flag the kind as unknown instead.
fn style_kind_attr(value: Option<&str>) -> Option<u8> {
    match value {
        Some("PARA") => Some(0),
        Some("CHAR") => Some(1),
        _ => None,
    }
}

#[cfg(test)]
mod image_fill_tests {
    use super::*;

    /// #452: an image-only `hc:imgBrush` inside a border fill's `hc:fillBrush` is not modeled
    /// (no genuine HWPX sample establishes its attributes or payload encoding), so the reader
    /// marks the entry instead of inventing a solid or gradient fill for it.
    #[test]
    fn border_fill_image_brush_is_marked_without_inventing_a_solid_or_gradient() {
        let xml = r##"<hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core"><hh:refList><hh:borderFills itemCnt="1"><hh:borderFill id="1" threeD="0" shadow="0" centerLine="NONE" breakCellSeparateLine="0"><hh:leftBorder type="SOLID" width="0.1 mm" color="#000000"/><hh:rightBorder type="SOLID" width="0.1 mm" color="#000000"/><hh:topBorder type="SOLID" width="0.1 mm" color="#000000"/><hh:bottomBorder type="SOLID" width="0.1 mm" color="#000000"/><hh:diagonal type="SOLID" width="0.1 mm" color="#000000"/><hc:fillBrush><hc:imgBrush mode="TILE"><hc:img binaryItemIDRef="image1" bright="0" contrast="0" effect="REAL_PIC" alpha="0"/></hc:imgBrush></hc:fillBrush></hh:borderFill></hh:borderFills></hh:refList></hh:head>"##;
        let (header, _warnings) = parse_header(xml).unwrap();
        let fill = header
            .border_fills
            .first()
            .expect("the border fill table entry");
        assert!(
            fill.image_fill_dropped,
            "the image fill must be marked, not silently dropped"
        );
        assert_eq!(fill.fill_type & 0x1, 0, "no solid fill bit is invented");
        assert_eq!(fill.fill_type & 0x4, 0, "no gradient bit is invented");
        assert_eq!(fill.bg_color, None, "no face color is invented");
        assert_eq!(fill.gradient, None);
    }

    /// The control: a solid `hc:winBrush` border fill stays unmarked.
    #[test]
    fn solid_border_fill_is_not_marked() {
        let xml = r##"<hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core"><hh:refList><hh:borderFills itemCnt="1"><hh:borderFill id="1" threeD="0" shadow="0" centerLine="NONE" breakCellSeparateLine="0"><hh:leftBorder type="SOLID" width="0.1 mm" color="#000000"/><hh:rightBorder type="SOLID" width="0.1 mm" color="#000000"/><hh:topBorder type="SOLID" width="0.1 mm" color="#000000"/><hh:bottomBorder type="SOLID" width="0.1 mm" color="#000000"/><hh:diagonal type="SOLID" width="0.1 mm" color="#000000"/><hc:fillBrush><hc:winBrush faceColor="#FF0000" hatchColor="#000000" alpha="0"/></hc:fillBrush></hh:borderFill></hh:borderFills></hh:refList></hh:head>"##;
        let (header, _warnings) = parse_header(xml).unwrap();
        let fill = header
            .border_fills
            .first()
            .expect("the border fill table entry");
        assert!(!fill.image_fill_dropped);
        assert_eq!(fill.fill_type & 0x1, 0x1);
        assert_eq!(fill.bg_color, Some(0x0000_00FF));
    }
}
