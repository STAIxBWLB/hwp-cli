//! `Contents/sectionN.xml` → [`Section`].
//!
//! Recursive-descent parser: hp:p recurs inside table cells (hp:subList), so each parser consumes
//! through its own closing tag.
//!
//! IR consistency rules, sharing semantics with hwp5:
//! - `hp:secPr` → ExtCtrl(2, "secd") + Control::SectionDef
//! - `hp:ctrl > hp:colPr` → ExtCtrl(2, "cold") + Control::Generic
//! - `hp:tbl` → ExtCtrl(11, "tbl ") + Control::Table
//! - Other objects (pic/rect/...) -> ExtCtrl(11) + Control::Generic; hp:subList paragraphs are
//!   recursively collected for text extraction.
use hwp_model::opaque::OpaqueRecord;
use hwp_model::{
    Caption, CaptionDirection, CaptionSide, Cell, CharShapeId, ColumnDef, ContainerBox, Control,
    Equation, GenericControl, GradientSpec, HwpChar, HwpUnit, LineSeg, PageDef, ParaShapeId,
    Paragraph, ParagraphList, SECPR_PAGEPR_SLOT, Section, SectionDef, ShapeGeom, ShapeKind,
    StyleId, Table,
};
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};

use crate::error::{HwpxError, Result};
use crate::read::xml::{
    attr, attr_i32, attr_offset_i32, attr_u16, attr_u32, parse_color, resolve_entity,
};

type XmlReader<'a> = Reader<&'a [u8]>;

fn next_event<'a>(reader: &mut XmlReader<'a>) -> Result<Event<'a>> {
    reader.read_event().map_err(|e| HwpxError::Xml {
        entry: "section".to_string(),
        message: e.to_string(),
    })
}

/// Consumes a subtree through its closing tag to skip unneeded elements.
fn skip_subtree(reader: &mut XmlReader<'_>, name: &[u8]) -> Result<()> {
    let mut depth = 1u32;
    loop {
        match next_event(reader)? {
            Event::Start(e) if e.local_name().as_ref().as_bytes() == name => depth += 1,
            Event::End(e) if e.local_name().as_ref().as_bytes() == name => {
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

/// Collects section-root xmlns declarations outside the writer's standard hs/hp/hc set as raw attributes
/// (`xmlns:foo="uri"`), inspecting only the first start tag. Raw-captured objects may use root-only
/// extension prefixes (hp10:, vendor prefixes, etc.); preserve these declarations on the reserialized
/// root for namespace-well-formed output. Keep attribute values escaped exactly as in the source.
pub fn extract_section_root_xmlns(xml: &str) -> Vec<String> {
    // The writer's root constants already emit these standard namespace declarations.
    const STANDARD: &[&str] = &["hs", "hp", "hc"];
    let mut reader = Reader::from_str(xml);
    let mut out = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                for attr in e.attributes().flatten() {
                    let key = attr.key.as_ref().to_owned();
                    let keep = if key == "xmlns" {
                        true
                    } else if let Some(prefix) = key.strip_prefix("xmlns:") {
                        !STANDARD.contains(&prefix)
                    } else {
                        false
                    };
                    if !keep {
                        continue;
                    }
                    let value = attr.value.into_owned();
                    out.push(format!("{key}=\"{value}\""));
                }
                break; // Inspect only the root element.
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// Deepest element nesting a section may have (#317). One nested table adds six
/// levels (`p > run > tbl > tr > tc > subList`), so this admits about 42 nested
/// tables; the local corpus peaks at depth 17. The recursive parser needs about
/// 19 KiB of stack per table level in a debug build and overflows a default
/// 2 MiB thread near 100 levels. A stack overflow aborts instead of unwinding,
/// so the bound protects every caller, not only the CLI's 32 MiB thread.
const MAX_ELEMENT_DEPTH: usize = 256;

/// Refuses a section nested deeper than [`MAX_ELEMENT_DEPTH`] before the
/// recursive parse runs. Malformed XML is left to that parser, so its error
/// message is unchanged.
// ponytail: a second event pass per section; count inside the parser if section
// parsing ever shows up in a profile.
fn check_element_depth(xml: &str) -> Result<()> {
    let mut reader = Reader::from_str(xml);
    let mut depth = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(_)) => {
                depth += 1;
                if depth > MAX_ELEMENT_DEPTH {
                    return Err(HwpxError::PackageLimit(format!(
                        "섹션 XML 요소 중첩 깊이가 상한 {MAX_ELEMENT_DEPTH}을 넘습니다 (표·글상자·묶음 개체가 너무 깊게 중첩됨)"
                    )));
                }
            }
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::Eof) | Err(_) => return Ok(()),
            _ => {}
        }
    }
}

pub fn parse_section(xml: &str) -> Result<(Section, Vec<String>)> {
    check_element_depth(xml)?;
    let mut reader = Reader::from_str(xml);
    let mut section = Section::default();
    let mut warnings = Vec::new();

    loop {
        match next_event(&mut reader)? {
            Event::Start(e) if e.local_name().as_ref().as_bytes() == b"p" => {
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

/// Consumes one hp:p element.
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
    // Same encoding as hwp5 break_type bits (bit2=page, bit3=column).
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
                let name = e.local_name().as_ref().as_bytes().to_vec();
                match name.as_slice() {
                    b"run" => {
                        let id = attr_u16(e, "charPrIDRef").unwrap_or(0);
                        if last_shape != Some(id) {
                            // If the previous run added no characters (empty hp:t), two character shapes
                            // share a position. HWP5 PARA_CHAR_SHAPE allows one shape per position, with
                            // the last winning, so overwrite with the last run. This prevents two empty
                            // runs in an empty paragraph from using the first run's large font to raise
                            // the line and shift pages.

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
                            container_box: None,
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
                        // z-order belongs to the hp:pic start tag, not its hp:pos child. Omitting it
                        // changes header/body logo stacking.

                        picture.z_order = attr_i32(e, "zOrder").unwrap_or(0).max(0) as u32;
                        push_ext_ctrl(&mut para, &mut wchar_pos, 11, *b"gso ");
                        para.controls.push(Control::Picture(picture));
                    }
                    // Other objects (rect, ellipse, line, polygon, curve, equation, container, etc.).
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
                            container_box: None,
                        };
                        if let Some(kind) = shape_kind(&name) {
                            // Shapes keep the semantic gso_shapes parsing path without raw capture.
                            if !empty {
                                // Rounded rectangle: hp:rect ratio="N" (corner curvature percentage).
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
                            // Capture complete raw XML for unmodeled objects (e.g. hp:container) for
                            // lossless reserialization, then collect subList/caption from that capture
                            // for text extraction/rendering. Keep this order: capture_element consumes
                            // the original reader's subtree.

                            let raw = capture_element(reader, e, empty)?;
                            if !empty {
                                let mut raw_reader = Reader::from_str(&raw);
                                // Consume the capture's first event, the container start tag.
                                let _ = next_event(&mut raw_reader)?;
                                if name == b"container" {
                                    let mut skipped = 0u32;
                                    let mut text_boxes = Vec::new();
                                    collect_container(
                                        &mut raw_reader,
                                        &mut generic,
                                        warnings,
                                        0,
                                        (0, 0),
                                        &mut skipped,
                                        &mut text_boxes,
                                    )?;
                                } else {
                                    collect_sub_lists(
                                        &mut raw_reader,
                                        &name,
                                        &mut generic,
                                        warnings,
                                    )?;
                                }
                            }
                            generic.hwpx_raw_xml = Some(raw);
                        }
                        push_ext_ctrl(&mut para, &mut wchar_pos, 11, ctrl_id);
                        para.controls.push(Control::Generic(generic));
                    }
                }
            }
            Event::End(e) if e.local_name().as_ref().as_bytes() == b"p" => break,
            Event::Eof => {
                warnings.push("hp:p가 닫히지 않은 채 EOF".to_string());
                break;
            }
            _ => {}
        }
    }
    Ok(para)
}

/// Loads one hp:t character into the IR.
///
/// Normalize raw tabs (0x09) to InlineCtrl(9) for legacy malformed HWPX that stored tabs as Text
/// (section 3.2.3 table 6: a tab is an 8-WCHAR inline control, an IR invariant). Drop other C0 controls
/// to prevent document corruption; forced breaks arrive separately as hp:lineBreak. Load ordinary
/// characters as Text.
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

/// Collects text inside hp:t, ignoring nested markup.
fn parse_text(
    reader: &mut XmlReader<'_>,
    para: &mut Paragraph,
    wchar_pos: &mut u32,
    _warnings: &mut [String],
) -> Result<()> {
    loop {
        match next_event(reader)? {
            Event::Text(t) => {
                let s = t.xml10_content();
                for c in s.chars() {
                    push_text_char(para, wchar_pos, c);
                }
            }
            // Entity references (&amp;, &#x...;) arrive as separate events.
            Event::GeneralRef(r) => {
                if let Some(c) = resolve_entity(&r) {
                    push_text_char(para, wchar_pos, c);
                }
            }
            // Forced line break inside hp:t, as in genuine Hangul mixed content:
            // text<hp:lineBreak/>text.
            Event::Empty(e) | Event::Start(e)
                if e.local_name().as_ref().as_bytes() == b"lineBreak" =>
            {
                *wchar_pos += 1;
                para.chars.push(HwpChar::CharCtrl(10));
            }
            // Nested tabs inside hp:t (genuine mixed content: text<hp:tab width leader type/>text)
            // become InlineCtrl(9). Hangul recalculates these attributes, so leave the payload zero; on
            // round-trip the writer derives kind/leader from paragraph tab definitions. Legacy bare tabs
            // directly under hp:run are handled by parse_paragraph's tab arm.
            Event::Empty(e) | Event::Start(e) if e.local_name().as_ref().as_bytes() == b"tab" => {
                para.chars.push(HwpChar::InlineCtrl {
                    code: 9,
                    payload: vec![0; 12],
                });
                *wchar_pos += 8;
            }
            Event::End(e) if e.local_name().as_ref().as_bytes() == b"t" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

fn push_ext_ctrl(para: &mut Paragraph, wchar_pos: &mut u32, code: u16, ctrl_id: [u8; 4]) {
    // The payload's first four bytes contain the reversed ctrl_id, matching hwp5 storage.
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

/// Semantically parses pagePr/margin within hp:secPr and preserves other children
/// (grid/startNum/visibility/lineNumberShape/footNotePr/endNotePr/pageBorderFill, etc.) as raw XML in
/// source order for inexpensive passthrough. Mark pagePr's position with SECPR_PAGEPR_SLOT so the writer
/// regenerates it from the page definition at the same position.
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
                match e.local_name().as_ref().as_bytes() {
                    b"pagePr" => {
                        // Parse pagePr semantically as the page definition, read margin within its
                        // subtree, and leave a positional marker in the raw list.

                        has_page = true;
                        page.width = HwpUnit(attr_i32(e, "width").unwrap_or(0));
                        page.height = HwpUnit(attr_i32(e, "height").unwrap_or(0));
                        // OWPML landscape="NARROWLY" means portrait; "WIDELY" means landscape and sets
                        // bit0.
                        if attr(e, "landscape").as_deref() == Some("NARROWLY") {
                            page.attr |= 1;
                        }
                        if !empty {
                            parse_page_pr_children(reader, &mut page)?;
                        }
                        def.secpr_raw_children.push(SECPR_PAGEPR_SLOT.to_string());
                    }
                    // Capture other child subtrees as raw XML without semantic interpretation for
                    // lossless round-trips.
                    _ => {
                        def.secpr_raw_children
                            .push(capture_element(reader, e, empty)?);
                    }
                }
            }
            Event::End(e) if e.local_name().as_ref().as_bytes() == b"secPr" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    if has_page {
        def.page = Some(page);
    }
    Ok(def)
}

/// Reads hp:margin inside hp:pagePr through its closing tag to fill page margins.
fn parse_page_pr_children(reader: &mut XmlReader<'_>, page: &mut PageDef) -> Result<()> {
    loop {
        match next_event(reader)? {
            Event::Start(e) | Event::Empty(e)
                if e.local_name().as_ref().as_bytes() == b"margin" =>
            {
                page.margin_left = HwpUnit(attr_i32(&e, "left").unwrap_or(0));
                page.margin_right = HwpUnit(attr_i32(&e, "right").unwrap_or(0));
                page.margin_top = HwpUnit(attr_i32(&e, "top").unwrap_or(0));
                page.margin_bottom = HwpUnit(attr_i32(&e, "bottom").unwrap_or(0));
                page.margin_header = HwpUnit(attr_i32(&e, "header").unwrap_or(0));
                page.margin_footer = HwpUnit(attr_i32(&e, "footer").unwrap_or(0));
                page.gutter = HwpUnit(attr_i32(&e, "gutter").unwrap_or(0));
            }
            Event::End(e) if e.local_name().as_ref().as_bytes() == b"pagePr" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

/// Captures one start or empty element subtree as raw XML by reserializing quick-xml events. Empty
/// captures one element; Start includes its matching closing tag at arbitrary nesting depth.
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
                Event::Start(c) | Event::Empty(c)
                    if c.local_name().as_ref().as_bytes() == b"colLine" =>
                {
                    def.divider = Some(hwp_model::BorderLine {
                        line_type: attr(&c, "type")
                            .map_or(1, |t| crate::read::header::line_type_code(&t)),
                        width: attr(&c, "width")
                            .map_or(1, |w| crate::read::header::width_index(&w)),
                        color: attr(&c, "color").map_or(0, |v| parse_color(&v)),
                    });
                }
                Event::End(c) if c.local_name().as_ref().as_bytes() == b"colPr" => break,
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

/// HWPX hp:pageNum pos/formatType/sideChar -> hwp5 pgnp page-number positioning (12 bytes): properties
/// (u32: format | position<<8), six reserved bytes, and sideChar WCHAR. Genuine measurements:
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
    // Only Arabic-number formatting (DIGIT=0) is mapped; substitute zero for other formats.
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

/// HWPX hp:pageHiding hide... -> hwp5 pghd page-hiding bitmap (4 bytes): bit0=header, 1=footer, 2=master
/// page, 3=border, 4=background, 5=page number. Genuine measurements: cover=0x21 (header+page number),
/// contents=0x20 (page number).
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

/// HWPX hp:autoNum -> hwp5 atno automatic numbering (12 bytes). Measured standard payload throughout the
/// business-plan sample: 0u32, 4u32, 0u32 (automatic page number). Refine kind/shape details only after
/// ground truth is available.
fn build_atno() -> Vec<u8> {
    let mut v = Vec::with_capacity(12);
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&4u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v
}

/// HWPX hp:newNum num -> hwp5 nwno number reset (6 bytes): kind (u32=0, PAGE) and number (u16). Genuine
/// measurement: num=1 -> 000000000100.
fn build_nwno(e: &BytesStart<'_>) -> Vec<u8> {
    let num: u16 = attr(e, "num").and_then(|s| s.parse().ok()).unwrap_or(1);
    let mut v = Vec::with_capacity(6);
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&num.to_le_bytes());
    v
}

/// Finds the innermost unmatched FIELD_START (code 3) ctrl_id in para.chars, LIFO. fieldEnd is not
/// pushed yet, so scan backward from depth=0 to support nested fields.
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
                let name = e.local_name().as_ref().as_bytes().to_vec();
                // Fields: fieldBegin -> ExtCtrl(3) + Generic %xxx (name in CTRL_DATA); fieldEnd ->
                // InlineCtrl(4).
                if name.as_slice() == b"fieldBegin" {
                    let ty = attr(e, "type").unwrap_or_default();
                    let ctrl_id = hwp_convert::field::field_ctrl_id_from_owpml(&ty);
                    let fname = attr(e, "name").unwrap_or_default();
                    // For Start, read Command from the hp:parameters child; self-closing elements have
                    // none.
                    let command = if matches!(event, Event::Start(_)) {
                        read_field_command(reader)?
                    } else {
                        None
                    };
                    // Build field command data through hwp-convert's shared generator (per-kind
                    // attr/etc. and a nonzero ID, required for Hangul to recognize %hlk as a hyperlink).

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
                            tag: 0x0057, // HWPTAG_CTRL_DATA: name Parameter Set.
                            data: hwp_convert::field::make_field_ctrl_data(&fname),
                            children: Vec::new(),
                        }],
                        gso_shapes: Vec::new(),
                        equation: None,
                        column_def: None,
                        caption: None,
                        hwpx_raw_xml: None,
                        container_box: None,
                    };
                    push_ext_ctrl(para, wchar_pos, 3, ctrl_id);
                    para.controls.push(Control::Generic(generic));
                    continue;
                }
                if name.as_slice() == b"fieldEnd" {
                    // FIELD_END carries three reversed bytes of the matching FIELD_START ctrl_id,
                    // excluding %. An all-zero payload prevents Hangul pairing fields and disables
                    // hyperlink clicks (confirmed against ground truth). Match LIFO.

                    let payload = matching_field_start(&para.chars)
                        .map(|cid| hwp_convert::field::field_end_payload(&cid))
                        .unwrap_or_else(|| vec![0u8; 12]);
                    para.chars.push(HwpChar::InlineCtrl { code: 4, payload });
                    *wchar_pos += 8;
                    continue;
                }
                // Point bookmarks: hp:bookmark name="..." -> ExtCtrl(22) + Generic bokm (name in
                // CTRL_DATA).
                if name.as_slice() == b"bookmark" {
                    let bname = attr(e, "name").unwrap_or_default();
                    let generic = GenericControl {
                        ctrl_id: *b"bokm",
                        data: Vec::new(),
                        paragraph_lists: Vec::new(),
                        extras: Vec::new(),
                        raw_children: vec![OpaqueRecord {
                            tag: 0x0057, // HWPTAG_CTRL_DATA: name Parameter Set.
                            data: hwp_convert::bookmark::make_bokm_ctrl_data(&bname),
                            children: Vec::new(),
                        }],
                        gso_shapes: Vec::new(),
                        equation: None,
                        column_def: None,
                        caption: None,
                        hwpx_raw_xml: None,
                        container_box: None,
                    };
                    push_ext_ctrl(para, wchar_pos, 22, *b"bokm");
                    para.controls.push(Control::Generic(generic));
                    continue;
                }
                // Match hwp5 ctrl_id/control-character codes. Page numbering/hiding/reset use code 21
                // (page control); synthesize hwp5 payloads here because the writer drops empty
                // GenericControl values. Header/footer use 8 bytes for page scope plus ID.

                let (ctrl_id, code, data): ([u8; 4], u16, Vec<u8>) = match name.as_slice() {
                    b"colPr" => (*b"cold", 2, Vec::new()),
                    b"header" => (*b"head", 16, head_foot_data(e)),
                    b"footer" => (*b"foot", 16, head_foot_data(e)),
                    b"footNote" => (*b"fn  ", 17, Vec::new()),
                    b"endNote" => (*b"en  ", 17, Vec::new()),
                    // Restore the measured standard 12-byte automatic page-number payload, identical
                    // throughout the business-plan sample. hwp5 writer stripping drops empty payloads,
                    // so populate it to prevent a round-trip gap.
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
                    container_box: None,
                };
                // parse_col_pr has already consumed a colPr subtree.
                if matches!(event, Event::Start(_)) && name.as_slice() != b"colPr" {
                    collect_sub_lists(reader, &name, &mut generic, warnings)?;
                }
                push_ext_ctrl(para, wchar_pos, code, ctrl_id);
                para.controls.push(Control::Generic(generic));
            }
            Event::End(e) if e.local_name().as_ref().as_bytes() == b"ctrl" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

/// Reads Command stringParam text from fieldBegin's hp:parameters child through /hp:fieldBegin. Returns
/// None when absent.
fn read_field_command(reader: &mut XmlReader<'_>) -> Result<Option<String>> {
    let mut in_command = false;
    let mut command: Option<String> = None;
    loop {
        let event = next_event(reader)?;
        match &event {
            Event::Start(e) if e.local_name().as_ref().as_bytes() == b"stringParam" => {
                in_command = attr(e, "name").as_deref() == Some("Command");
            }
            Event::Text(t) if in_command => {
                let s = t.xml10_content();
                command.get_or_insert_with(String::new).push_str(&s);
            }
            Event::End(e) if e.local_name().as_ref().as_bytes() == b"stringParam" => {
                in_command = false
            }
            Event::End(e) if e.local_name().as_ref().as_bytes() == b"fieldBegin" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(command)
}

/// HWPX vertRelTo -> hwp5 code (PAPER=0, PAGE=1, PARA=2).
fn vert_rel_to_code(s: Option<&str>) -> u8 {
    match s {
        Some("PAGE") => 1,
        Some("PARA") => 2,
        _ => 0, // PAPER
    }
}

/// HWPX horzRelTo -> hwp5 code (PAPER=0, PAGE=1, COLUMN=2, PARA=3).
fn horz_rel_to_code(s: Option<&str>) -> u8 {
    match s {
        Some("PAGE") => 1,
        Some("COLUMN") => 2,
        Some("PARA") => 3,
        _ => 0, // PAPER
    }
}

/// HWPX vertAlign/horzAlign -> hwp5 code (TOP/LEFT=0, CENTER=1, BOTTOM/RIGHT=2).
fn align_code(s: Option<&str>) -> u8 {
    match s {
        Some("CENTER") => 1,
        Some("BOTTOM") | Some("RIGHT") => 2,
        _ => 0, // TOP/LEFT
    }
}

/// hp:tbl table.
fn parse_table(
    reader: &mut XmlReader<'_>,
    start: &BytesStart<'_>,
    warnings: &mut Vec<String>,
) -> Result<Table> {
    // Table attr: bits0-1=page breaking (NONE=0/TABLE=1/CELL=2), bit2=repeat heading rows, bit3=disable
    // automatic width adjustment. Verified against genuine files. Leaving zero (a former bug) disables
    // splitting and pushes the entire table onto the next page when it cannot fit, separating contents
    // boxes into extra pages.

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
        placement: None, // Filled as GsoPlacement after the loop.
        attr: table_attr,
        rows: attr_u16(start, "rowCnt").unwrap_or(0),
        cols: attr_u16(start, "colCnt").unwrap_or(0),
        cell_spacing: attr_u16(start, "cellSpacing").unwrap_or(0),
        // Cell inner margins come from hp:inMargin in the loop below; default zero.
        inner_margins: [0; 4],
        row_cell_counts: Vec::new(),
        border_fill: hwp_model::BorderFillId(attr_u16(start, "borderFillIDRef").unwrap_or(0)),
        table_tail: Vec::new(),
        cells: Vec::new(),
        caption: None,
        extras: Vec::new(),
    };

    // Object-common placement attributes become a 40-byte hwp5 CTRL_HEADER. Without them the writer
    // applies floating defaults, removing inline tables from body flow; Hangul relayout then causes
    // overlaps/blank pages. zOrder is on the start tag.

    let mut placement = hwp_model::GsoPlacement {
        z_order: attr_i32(start, "zOrder").unwrap_or(0),
        ..Default::default()
    };

    loop {
        match next_event(reader)? {
            Event::Start(e) => match e.local_name().as_ref().as_bytes() {
                b"tc" => {
                    let cell = parse_cell(reader, &e, warnings)?;
                    table.cells.push(cell);
                }
                b"caption" => {
                    table.caption = Some(parse_caption(reader, &e, warnings)?);
                }
                b"tr" => {} // Rows are restored from cellAddr; treat this as a container only.
                _ => {
                    let name = e.local_name().as_ref().as_bytes().to_vec();
                    skip_subtree(reader, &name)?;
                }
            },
            // This table's self-closing inner margins, ordered left/right/top/bottom. parse_cell
            // separately consumes nested tables inside tc, so only the current table reaches this loop.
            Event::Empty(e) if e.local_name().as_ref().as_bytes() == b"inMargin" => {
                table.inner_margins = [
                    attr_margin_u16(&e, "left"),
                    attr_margin_u16(&e, "right"),
                    attr_margin_u16(&e, "top"),
                    attr_margin_u16(&e, "bottom"),
                ];
            }
            // Placement: hp:pos (treat-as-character, positioning reference, offsets), hp:sz (bounding
            // width/height, more accurate than summing merged cells), and hp:outMargin (outer margins).
            Event::Empty(e) if e.local_name().as_ref().as_bytes() == b"pos" => {
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
            Event::Empty(e) if e.local_name().as_ref().as_bytes() == b"sz" => {
                placement.width = attr_i32(&e, "width").unwrap_or(0);
                placement.height = attr_i32(&e, "height").unwrap_or(0);
            }
            Event::Empty(e) if e.local_name().as_ref().as_bytes() == b"outMargin" => {
                placement.out_margins = [
                    attr_u16(&e, "left").unwrap_or(0),
                    attr_u16(&e, "right").unwrap_or(0),
                    attr_u16(&e, "top").unwrap_or(0),
                    attr_u16(&e, "bottom").unwrap_or(0),
                ];
            }
            Event::End(e) if e.local_name().as_ref().as_bytes() == b"tbl" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    table.placement = Some(placement);
    // Reconstruct cell counts per row with the same semantics as hwp5.
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
            Event::Start(e) => match e.local_name().as_ref().as_bytes() {
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
            Event::End(e) if e.local_name().as_ref().as_bytes() == b"caption" => break,
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

/// One hp:tc cell.
fn parse_cell(
    reader: &mut XmlReader<'_>,
    start: &BytesStart<'_>,
    warnings: &mut Vec<String>,
) -> Result<Cell> {
    let mut cell = Cell {
        // Heading cells set bit18 for repeated table header rows (genuine measurement). vertAlign adds
        // bits5-6 later in subList.
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
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref().as_bytes() {
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
                    // Encode cell vertical alignment in list_attr bits5-6: TOP=0, CENTER=1, BOTTOM=2.
                    // Genuine cells use CENTER (0x20); ignoring it yields TOP and pins text upward,
                    // letting empty space below content split onto another page when the cell is taller
                    // than its contents.

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
            Event::End(e) if e.local_name().as_ref().as_bytes() == b"tc" => break,
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
                match e.local_name().as_ref().as_bytes() {
                    b"sz" => {
                        pic.width = HwpUnit(attr_i32(e, "width").unwrap_or(0));
                        pic.height = HwpUnit(attr_i32(e, "height").unwrap_or(0));
                    }
                    b"pos" => {
                        pic.treat_as_char = attr(e, "treatAsChar").as_deref() == Some("1");
                        // Floating object offsets, ignored for treat-as-character. HWPX stores negatives
                        // as unsigned two's-complement decimal (-77 = 4294967219), so parse as u32 then
                        // reinterpret as i32; direct i32 parsing would overflow.

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
                    // Only nested pic start tags increase depth; Empty has no closing tag.
                    b"pic" if matches!(event, Event::Start(_)) => depth += 1,
                    _ => {}
                }
            }
            Event::End(e) if e.local_name().as_ref().as_bytes() == b"pic" => {
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

/// Recursively collects hp:subList paragraphs in generic objects (text-box text).
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
                let name = e.local_name().as_ref().as_bytes().to_vec();
                if name == b"caption" {
                    // Unknown/generic shape containers (for example hp:container)
                    // do not go through collect_shape, but they can still carry
                    // the same hp:caption child as primitive shapes. Keep the
                    // caption separate from the object's text lists so renderers
                    // and text extraction can apply the configured side/order.
                    generic.caption = Some(parse_caption(reader, &e, warnings)?);
                } else if name == b"subList" {
                    generic
                        .paragraph_lists
                        .push(read_sub_list(reader, warnings)?);
                } else if name == end_name {
                    depth += 1;
                }
            }
            Event::End(e) if e.local_name().as_ref().as_bytes() == end_name => {
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

/// Collects hp:subList paragraphs after its opening tag is consumed. Shared by collect_sub_lists,
/// collect_container and collect_shape.
fn read_sub_list(reader: &mut XmlReader<'_>, warnings: &mut Vec<String>) -> Result<ParagraphList> {
    let mut list = ParagraphList {
        header_data: Vec::new(),
        paragraphs: Vec::new(),
    };
    loop {
        match next_event(reader)? {
            Event::Start(inner) if inner.local_name().as_ref().as_bytes() == b"p" => {
                list.paragraphs
                    .push(parse_paragraph(reader, &inner, warnings)?);
            }
            Event::End(inner) if inner.local_name().as_ref().as_bytes() == b"subList" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(list)
}

/// For hp:container, put the outermost pos/sz/treatAsChar box in container_box, parse child shapes with
/// collect_shape, and store them in gso_shapes relative to the container origin. Nested containers
/// recursively add their own pos offsets with bounded depth. The caller separately preserves raw XML in
/// hwpx_raw_xml.
///
/// base is the accumulated parent-container offset. At outermost depth 0 the renderer adds
/// container_box's origin, so child offsets start at (0,0). Assumes pos/sz precede shape children, as
/// Hancom writes them.
fn collect_container(
    reader: &mut XmlReader<'_>,
    generic: &mut GenericControl,
    warnings: &mut Vec<String>,
    depth: u32,
    base: (i32, i32),
    skipped: &mut u32,
    text_boxes: &mut Vec<Option<[i32; 4]>>,
) -> Result<()> {
    /// Nested-container recursion limit, analogous to hwp5 shape_draw MAX_DEPTH.
    const MAX_CONTAINER_DEPTH: u32 = 8;
    if depth >= MAX_CONTAINER_DEPTH {
        warnings.push("container nesting depth limit; subtree skipped".to_string());
        return skip_subtree(reader, b"container");
    }
    // This container's pos is relative to its parent. At depth 0 the renderer adds container_box's
    // origin, so accumulated child offsets start at (0,0).

    let mut own = (0i32, 0i32);
    let mut sz = (0i32, 0i32);
    let mut anchored = false;
    // Nested containers without pos, observed in genuine files, still inherit accumulated parent
    // offsets.
    let mut child_off = base;
    loop {
        let ev = next_event(reader)?;
        let is_empty = matches!(ev, Event::Empty(_));
        match ev {
            Event::Start(e) | Event::Empty(e) => {
                let n = e.local_name().as_ref().as_bytes().to_vec();
                match n.as_slice() {
                    b"pos" => {
                        own.0 = attr_offset_i32(&e, "horzOffset").unwrap_or(0);
                        own.1 = attr_offset_i32(&e, "vertOffset").unwrap_or(0);
                        anchored = attr(&e, "treatAsChar").as_deref() == Some("1");
                        if depth > 0 {
                            child_off = (base.0 + own.0, base.1 + own.1);
                        }
                    }
                    b"sz" => {
                        sz.0 = attr_i32(&e, "width").unwrap_or(0);
                        sz.1 = attr_i32(&e, "height").unwrap_or(0);
                    }
                    b"caption" if !is_empty => {
                        generic.caption = Some(parse_caption(reader, &e, warnings)?);
                    }
                    b"subList" if !is_empty => {
                        generic
                            .paragraph_lists
                            .push(read_sub_list(reader, warnings)?);
                        // Text owned by the container itself has no owner box (None).
                        text_boxes.push(None);
                    }
                    b"container" if !is_empty => {
                        // Recurse into nested containers with the current accumulated offset as base;
                        // the nested container adds its own pos when read.

                        collect_container(
                            reader,
                            generic,
                            warnings,
                            depth + 1,
                            child_off,
                            skipped,
                            text_boxes,
                        )?;
                    }
                    n if !is_empty && shape_kind(n).is_some() => {
                        let kind = shape_kind(n).expect("guard");
                        // Rounded rectangle: hp:rect ratio="N" (corner curvature percentage).
                        let round_ratio = if kind == ShapeKind::Rect {
                            attr_i32(&e, "ratio").unwrap_or(0).clamp(0, 100) as u8
                        } else {
                            0
                        };
                        let shape_base = generic.gso_shapes.len();
                        let lists_base = generic.paragraph_lists.len();
                        collect_shape(reader, n, kind, round_ratio, generic, warnings)?;
                        // ShapeGeom.points are relative to the bounding box and must not be offset.
                        for s in &mut generic.gso_shapes[shape_base..] {
                            s.x += child_off.0;
                            s.y += child_off.1;
                        }
                        // Assign the offset-adjusted shape box to paragraphs contributed by that shape;
                        // fall back to the container box if the shape was discarded.

                        let owned = generic
                            .gso_shapes
                            .get(shape_base)
                            .map(|s| [s.x, s.y, s.w, s.h]);
                        for _ in lists_base..generic.paragraph_lists.len() {
                            text_boxes.push(owned);
                        }
                    }
                    _ => {
                        if !is_empty {
                            // Silently skip metadata such as
                            // pos/sz/outMargin/renderingInfo/shapeComment. Count only visible objects
                            // (pic/ole/chart/equation/unknown elements) as skipped_objects so the
                            // renderer reports typed loss.

                            if !matches!(
                                n.as_slice(),
                                b"outMargin"
                                    | b"renderingInfo"
                                    | b"shapeComment"
                                    | b"caption"
                                    | b"subList"
                            ) {
                                *skipped += 1;
                            }
                            skip_subtree(reader, &n)?;
                        }
                    }
                }
            }
            Event::End(e) if e.local_name().as_ref().as_bytes() == b"container" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    if depth == 0 {
        generic.container_box = Some(ContainerBox {
            x: own.0,
            y: own.1,
            w: sz.0,
            h: sz.1,
            anchored,
            skipped_objects: *skipped,
            text_boxes: std::mem::take(text_boxes),
        });
    }
    Ok(())
}

/// Drawing-object element name -> shape kind.
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

/// Parses a shape's hp:pos offsets, hp:sz size, hp:lineShape border, hp:fillBrush/hp:winBrush fill,
/// hp:pt* points, and subList text into gso_shapes.
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
    let mut image_fill_dropped = false;
    let mut read_attrs = |e: &BytesStart<'_>| match e.local_name().as_ref().as_bytes() {
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
        // An image fill's payload (`hc:img binaryItemIDRef`) is not modeled: no genuine HWPX
        // sample establishes the element's attributes or how the referenced BinData stream is
        // encoded, so the fill is marked instead of guessed and the writer ledgers the drop as
        // `image_fill_unrepresentable` (#452).
        b"imgBrush" => image_fill_dropped = true,
        // Read pt0..N only as Polygon/Curve geometry. Rect/Ellipse/Arc pt0..3 represent bounding-box
        // corners in genuine files and round-trip through sz; ignore them to avoid spurious geometry
        // points.
        n if n.starts_with(b"pt") && matches!(kind, ShapeKind::Polygon | ShapeKind::Curve) => {
            if let (Some(px), Some(py)) = (attr_i32(e, "x"), attr_i32(e, "y")) {
                points.push((px, py));
            }
        }
        // Arc carries geometry in center/ax1/ax2 points relative to the bounding box and in encounter
        // order, inverse to the writer. The renderer uses the center plus two conjugate axes to draw the
        // arc.
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
                let n = e.local_name().as_ref().as_bytes().to_vec();
                if n == b"caption" {
                    generic.caption = Some(parse_caption(reader, &e, warnings)?);
                } else if n == b"subList" {
                    let mut list = ParagraphList {
                        header_data: Vec::new(),
                        paragraphs: Vec::new(),
                    };
                    loop {
                        match next_event(reader)? {
                            Event::Start(inner)
                                if inner.local_name().as_ref().as_bytes() == b"p" =>
                            {
                                list.paragraphs
                                    .push(parse_paragraph(reader, &inner, warnings)?);
                            }
                            Event::End(inner)
                                if inner.local_name().as_ref().as_bytes() == b"subList" =>
                            {
                                break;
                            }
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
            Event::End(e) if e.local_name().as_ref().as_bytes() == end_name => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    // Horizontal/vertical lines may have a zero extent on one axis; accept either w or h.
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
            image_fill_dropped,
        });
    }
    Ok(())
}

/// hp:equation: collects the script attribute or hp:script child, hp:sz size and hp:pos position. The
/// renderer approximates it with a box and text.
fn parse_equation(
    reader: &mut XmlReader<'_>,
    start: &BytesStart<'_>,
    empty: bool,
) -> Result<Equation> {
    let mut script = attr(start, "script").unwrap_or_default();
    let (mut width, mut height, mut x, mut y) = (0i32, 0i32, 0i32, 0i32);
    let mut inline = true;
    // Preserve unmodeled attributes (character color, baseUnit, equation font, zOrder, body placement,
    // etc.) raw for round-trips. Exclude id because the writer renumbers it document-wide.

    let raw_attrs = raw_attrs_except(start, b"id");
    let mut raw_props = Vec::new();
    if !empty {
        loop {
            let ev = next_event(reader)?;
            match &ev {
                Event::Start(e) | Event::Empty(e) => {
                    let is_start = matches!(ev, Event::Start(_));
                    let is_empty_el = matches!(ev, Event::Empty(_));
                    match e.local_name().as_ref().as_bytes() {
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
                        // Also preserve other common children such as outMargin/caption/shapeComment
                        // raw.
                        _ => raw_props.push(capture_element(reader, e, is_empty_el)?),
                    }
                }
                Event::End(e) if e.local_name().as_ref().as_bytes() == b"equation" => break,
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

/// Reconstructs raw name="value" attributes from a start tag, excluding skip. Values retain their
/// original escaped form without re-escaping, completing the round-trip.
fn raw_attrs_except(start: &BytesStart<'_>, skip: &[u8]) -> Option<String> {
    let mut out = String::new();
    for a in start.attributes().flatten() {
        if a.key.local_name().as_ref().as_bytes() == skip {
            continue;
        }
        let key = a.key.as_ref().to_owned();
        let val = a.value.into_owned();
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format!("{key}=\"{val}\""));
    }
    (!out.is_empty()).then_some(out)
}

/// Collects text through the given element's closing tag.
fn read_element_text(reader: &mut XmlReader<'_>, end: &[u8]) -> Result<String> {
    let mut out = String::new();
    loop {
        match next_event(reader)? {
            Event::Text(t) => {
                let s = t.xml10_content();
                out.push_str(&s);
            }
            // Equation scripts contain XML special characters such as x &lt; y. Resolve references or
            // the characters vanish entirely; this is the inverse of the writer's esc().
            Event::GeneralRef(r) => {
                if let Some(c) = resolve_entity(&r) {
                    out.push(c);
                }
            }
            // CDATA (e.g. <![CDATA[a<b]]>) arrives separately from Text. Its content is already literal
            // and needs no reference resolution.
            Event::CData(t) => {
                let s = t.xml10_content();
                out.push_str(&s);
            }
            Event::End(e) if e.local_name().as_ref().as_bytes() == end => break,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

/// hp:lineShape style -> line-kind code (0=solid, 1=dash, 2=dot, 3=dash-dot, 4=dash-dot-dot, 5=long
/// dash).
fn line_style_code(s: &str) -> u8 {
    match s.to_ascii_uppercase().as_str() {
        "DASH" => 1,
        "DOT" => 2,
        "DASH_DOT" | "DASHDOT" => 3,
        "DASH_DOT_DOT" | "DASHDOTDOT" => 4,
        "LONG_DASH" | "LONGDASH" => 5,
        _ => 0, // SOLID, etc.
    }
}

/// hp:lineShape headStyle/tailStyle -> arrowhead presence (0=none/NORMAL, 1=arrowhead).
fn arrow_code(s: &str) -> u8 {
    if s.is_empty() || s.eq_ignore_ascii_case("NORMAL") || s.eq_ignore_ascii_case("NONE") {
        0
    } else {
        1
    }
}

/// hp:gradation fill: maps type (LINEAR/RADIAL/etc.), angle and hp:color value="#.." children to evenly
/// spaced color stops.
fn parse_gradation(
    reader: &mut XmlReader<'_>,
    start: &BytesStart<'_>,
) -> Result<Option<GradientSpec>> {
    let gtype = attr(start, "type").unwrap_or_default();
    // LINEAR is linear; approximate other kinds (RADIAL/CIRCLE/CONICAL/SQUARE) as radial.
    let radial = !gtype.eq_ignore_ascii_case("LINEAR");
    let angle_deg = attr_i32(start, "angle").unwrap_or(0) as f32;
    let center_x = attr_i32(start, "centerX").unwrap_or(0);
    let center_y = attr_i32(start, "centerY").unwrap_or(0);
    let step = attr_i32(start, "step").unwrap_or(255);
    let mut colors: Vec<u32> = Vec::new();
    loop {
        match next_event(reader)? {
            Event::Empty(e) | Event::Start(e) if e.local_name().as_ref().as_bytes() == b"color" => {
                if let Some(v) = attr(&e, "value") {
                    colors.push(parse_color(&v));
                }
            }
            Event::End(e) if e.local_name().as_ref().as_bytes() == b"gradation" => break,
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
        center_x,
        center_y,
        step,
        stops,
    }))
}

/// hp:linesegarray line-layout information.
fn parse_linesegs(reader: &mut XmlReader<'_>, para: &mut Paragraph) -> Result<()> {
    loop {
        match next_event(reader)? {
            Event::Empty(e) | Event::Start(e)
                if e.local_name().as_ref().as_bytes() == b"lineseg" =>
            {
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
            Event::End(e) if e.local_name().as_ref().as_bytes() == b"linesegarray" => break,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod depth_tests {
    use super::*;

    fn nested(depth: usize) -> String {
        format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth))
    }

    #[test]
    fn a_section_at_the_depth_bound_parses() {
        assert!(parse_section(&nested(MAX_ELEMENT_DEPTH)).is_ok());
    }

    #[test]
    fn a_section_past_the_depth_bound_is_refused() {
        let error = parse_section(&nested(MAX_ELEMENT_DEPTH + 1)).unwrap_err();
        assert!(
            matches!(error, HwpxError::PackageLimit(ref message) if message.contains("256")),
            "{error}"
        );
    }
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

    /// Page-number position (pgnp): the genuine university sample uses BOTTOM_CENTER plus sideChar '-'.
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
        // props=position5<<8=0x500, six reserved bytes, sideChar '-' (0x2d) WCHAR.
        assert_eq!(
            build_pgnp(&e),
            vec![0x00, 0x05, 0, 0, 0, 0, 0, 0, 0, 0, 0x2d, 0x00]
        );
    }

    /// Page hiding (pghd): cover=header+page number (0x21), contents=page number (0x20).
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

    /// Number reset (nwno): num=1 -> kind(0) + number(1).
    #[test]
    fn nwno_합성() {
        let e = elem("hp:newNum", &[("num", "1"), ("numType", "PAGE")]);
        assert_eq!(build_nwno(&e), vec![0, 0, 0, 0, 0x01, 0x00]);
    }

    /// Header/footer: page scope (u32) plus ID (u32); BOTH(0) and id=2.
    #[test]
    fn head_foot_8바이트() {
        let e = elem("hp:header", &[("id", "2"), ("applyPageType", "BOTH")]);
        assert_eq!(head_foot_data(&e), vec![0, 0, 0, 0, 0x02, 0, 0, 0]);
        let odd = elem("hp:footer", &[("id", "3"), ("applyPageType", "ODD")]);
        assert_eq!(head_foot_data(&odd), vec![0x02, 0, 0, 0, 0x03, 0, 0, 0]);
    }

    /// Opens a complete hp:gradation document, consumes its start tag, and calls parse_gradation.
    fn run_gradation(xml: &str) -> Option<GradientSpec> {
        let mut reader = Reader::from_str(xml);
        let start = match reader.read_event().unwrap() {
            Event::Start(e) => e.into_owned(),
            other => panic!("gradation 시작 태그 기대, 실제 {other:?}"),
        };
        parse_gradation(&mut reader, &start).unwrap()
    }

    /// Linear gradient: type=LINEAR with two colors -> evenly spaced stops 0.0/1.0.
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

    /// Radial gradient: type=RADIAL -> radial=true; three colors -> 0/0.5/1.0.
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

    /// Legacy malformed-file compatibility: normalize raw 0x09 inside hp:t to InlineCtrl(9), dropping
    /// other C0 controls. Rereading as Text(tab) would corrupt the next save again.
    #[test]
    fn 오염_raw탭_읽기_인라인컨트롤_정규화() {
        let xml = concat!(
            r##"<?xml version="1.0"?><hs:sec><hp:p paraPrIDRef="0" styleIDRef="0">"##,
            "<hp:run charPrIDRef=\"0\"><hp:t xml:space=\"preserve\">앞\t뒤\u{0001}끝</hp:t></hp:run>",
            r##"</hp:p></hs:sec>"##,
        );
        let (section, _) = parse_section(xml).unwrap();
        let chars = &section.paragraphs[0].chars;
        // Raw tab -> InlineCtrl(9), drop control 0x01, preserve remaining text.
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

    /// centerX/centerY/step round-trip: parse an `<hp:gradation>` carrying
    /// explicit values, then re-emit via the same `hc:gradation` attribute
    /// shape the section writer uses and confirm the values survive.
    #[test]
    fn gradation_center_and_step_round_trip() {
        let g = run_gradation(
            r##"<hp:gradation type="LINEAR" angle="90" centerX="30" centerY="70" step="120"><hp:color value="#FF0000"/><hp:color value="#0000FF"/></hp:gradation>"##,
        )
        .unwrap();
        assert_eq!(g.center_x, 30);
        assert_eq!(g.center_y, 70);
        assert_eq!(g.step, 120);

        // Re-emit with the writer's exact `hc:gradation` attribute shape and
        // re-parse it to confirm the values survive the round trip.
        let reemitted = format!(
            r##"<hp:gradation type="{}" angle="{}" centerX="{}" centerY="{}" step="{}"><hp:color value="#FF0000"/><hp:color value="#0000FF"/></hp:gradation>"##,
            if g.radial { "RADIAL" } else { "LINEAR" },
            g.angle_deg.round() as i32,
            g.center_x,
            g.center_y,
            g.step,
        );
        let g2 = run_gradation(&reemitted).unwrap();
        assert_eq!(g2.center_x, g.center_x);
        assert_eq!(g2.center_y, g.center_y);
        assert_eq!(g2.step, g.step);
    }

    /// One or fewer colors means no gradient (None).
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

#[cfg(test)]
mod image_fill_tests {
    use super::*;

    /// #452: a shape's `hc:imgBrush` is not modeled (no genuine HWPX sample establishes its
    /// attributes or payload encoding), so the semantic shape path marks the shape instead of
    /// publishing it as an unfilled shape without evidence.
    #[test]
    fn shape_image_brush_is_marked_and_keeps_its_geometric_path() {
        let xml = concat!(
            r##"<?xml version="1.0"?><hs:sec><hp:p paraPrIDRef="0" styleIDRef="0">"##,
            r##"<hp:run charPrIDRef="0"><hp:rect id="1" ratio="0">"##,
            r##"<hp:pos treatAsChar="0" horzOffset="1000" vertOffset="2000"/>"##,
            r##"<hp:sz width="3000" height="4000"/>"##,
            r##"<hc:fillBrush><hc:imgBrush binaryItemIDRef="image1" mode="TOTAL"/></hc:fillBrush>"##,
            r##"</hp:rect></hp:run></hp:p></hs:sec>"##,
        );
        let (section, _) = parse_section(xml).unwrap();
        let shapes = section.paragraphs[0]
            .controls
            .iter()
            .find_map(|control| match control {
                Control::Generic(generic) => Some(&generic.gso_shapes),
                _ => None,
            })
            .expect("shape control");
        assert_eq!(shapes.len(), 1, "{shapes:?}");
        let shape = &shapes[0];
        assert_eq!(shape.kind, ShapeKind::Rect);
        // The geometric path survives untouched: dropping the fill must not cost the geometry.
        assert_eq!(
            (shape.x, shape.y, shape.w, shape.h),
            (1000, 2000, 3000, 4000)
        );
        // An image fill is not a solid fill: marking it must not invent one.
        assert!(shape.image_fill_dropped);
        assert_eq!(shape.fill, 0xFFFF_FFFF);
        assert!(shape.fill_gradient.is_none());
    }

    /// Control for the test above: a solid `hc:winBrush` fill is fully modeled, so it is read as
    /// the shape's face colour and is not marked as a dropped image fill.
    #[test]
    fn shape_solid_brush_is_not_marked_as_an_image_fill() {
        let xml = concat!(
            r##"<?xml version="1.0"?><hs:sec><hp:p paraPrIDRef="0" styleIDRef="0">"##,
            r##"<hp:run charPrIDRef="0"><hp:ellipse id="2">"##,
            r##"<hp:sz width="500" height="600"/>"##,
            r##"<hc:fillBrush><hc:winBrush faceColor="#FF0000" hatchColor="#000000" alpha="0"/></hc:fillBrush>"##,
            r##"</hp:ellipse></hp:run></hp:p></hs:sec>"##,
        );
        let (section, _) = parse_section(xml).unwrap();
        let shapes = section.paragraphs[0]
            .controls
            .iter()
            .find_map(|control| match control {
                Control::Generic(generic) => Some(&generic.gso_shapes),
                _ => None,
            })
            .expect("shape control");
        let shape = shapes.first().expect("shape");
        assert_eq!(shape.kind, ShapeKind::Ellipse);
        assert!(!shape.image_fill_dropped);
        assert_eq!(shape.fill, 0x0000_00FF, "faceColor #FF0000 as COLORREF");
    }
}
