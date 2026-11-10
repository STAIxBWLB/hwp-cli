//! Field and click-here-field discovery and filling (Hancom GetFieldList/PutFieldText equivalents).
//!
//! HWP fields contain FIELD_START (character code 3, extended control), display text, then FIELD_END (code 4).
//! - Kind: control ID (%clk click-here, %fmu formula, %hlk hyperlink, etc.).
//! - Name: first BSTR in CTRL_DATA (HWPTAG Parameter Set), usually present only for click-here fields.
//! - Command: the field record command string (formula, click-here instructions, etc.).
//! - Value: text between FIELD_START and FIELD_END.
//!
//! Metadata is parsed on demand without changing the IR, readers, writers or identity round-trips.
//! Filling replaces the value and adjusts character-shape runs; writing uses write_hwp_edited.

use std::collections::BTreeMap;

use hwp_model::opaque::OpaqueRecord;
use hwp_model::{CharShapeId, Control, Document, GenericControl, HwpChar, Paragraph};

use crate::edit::{adjust_runs, find_match, utf16_len};

const CTRL_DATA_TAG: u16 = 0x0010 + 71; // HWPTAG_CTRL_DATA
const FIELD_START: u16 = 3;
const FIELD_END: u16 = 4;

/// Information about one field.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldInfo {
    /// Display name of the field kind.
    pub kind: String,
    /// Original control ID, such as "%clk".
    pub ctrl_id: String,
    /// Optional field name from CTRL_DATA.
    pub name: Option<String>,
    /// Optional command or instruction from the field record.
    pub command: Option<String>,
    /// Displayed value.
    pub value: String,
}

/// Recognize 33 of the 34 field kinds in specification table 128, excluding memo (`%%me`).
/// Unknown control IDs are otherwise dropped by the HWPX writer catch-all, so recognized
/// kinds preserve fieldBegin/End and their text even when their kind label is unknown.
/// The OWPML type then uses the UNKNOWN fallback in [`owpml_field_type`] (GF-1 loss level).
/// Memo fields must pair with memo lists (GB-7), whose bodies are not yet emitted.
/// A dangling memo field could corrupt the document in Hangul, so this kind remains
/// intentionally excluded until genuine-file evidence establishes safe emission.
pub fn is_field_ctrl_id(ctrl_id: &[u8; 4]) -> bool {
    matches!(
        ctrl_id,
        // Original 12 kinds (table 128).
        b"%clk"
            | b"%fmu"
            | b"%hlk"
            | b"%mmg"
            | b"%dte"
            | b"%ddt"
            | b"%xrf"
            | b"%bmk"
            | b"%pat"
            | b"%smr"
            | b"%usr"
            | b"%unk"
            // 19 revision-tracking kinds (FIELD_REVISION_*, table 128).
            | b"%sig"
            | b"%spl"
            | b"%%mr"
            | b"%%*d"
            | b"%%*a"
            | b"%%*C"
            | b"%%*S"
            | b"%%*T"
            | b"%%*P"
            | b"%%*L"
            | b"%%*c"
            | b"%%*h"
            | b"%%*A"
            | b"%%*i"
            | b"%%*t"
            | b"%%*r"
            | b"%%*l"
            | b"%%*n"
            | b"%%*e"
            // Privacy and table of contents (table 128). Memo needs GB-7 support.
            | b"%cpr"
            | b"%toc"
    )
}

/// Map a control ID to the OWPML fieldBegin type attribute.
pub fn owpml_field_type(ctrl_id: &[u8; 4]) -> &'static str {
    match ctrl_id {
        b"%clk" => "CLICK_HERE",
        b"%fmu" => "FORMULA",
        b"%hlk" => "HYPERLINK",
        b"%mmg" => "MAIL_MERGE",
        b"%dte" => "DATE",
        b"%ddt" => "DOCUMENT_DATE",
        b"%xrf" => "CROSS_REF",
        b"%bmk" => "BOOKMARK",
        b"%pat" => "PATH",
        b"%smr" => "SUMMARY",
        b"%usr" => "USER_INFO",
        // Genuine corpus evidence: the task-description HWPX uses type="TABLEOFCONTENTS".
        b"%toc" => "TABLEOFCONTENTS",
        // The OWPML names for revision kinds and privacy (%cpr) lack evidence, so use UNKNOWN.
        // Keep fieldBegin/End and content; only the kind label is lost (GF-1 level).
        _ => "UNKNOWN",
    }
}

/// Map an OWPML fieldBegin type back to a control ID, using `%unk` for unknown types.
pub fn field_ctrl_id_from_owpml(t: &str) -> [u8; 4] {
    match t {
        "CLICK_HERE" => *b"%clk",
        "FORMULA" => *b"%fmu",
        "HYPERLINK" => *b"%hlk",
        "MAIL_MERGE" => *b"%mmg",
        "DATE" => *b"%dte",
        "DOCUMENT_DATE" => *b"%ddt",
        "CROSS_REF" | "CROSS_REFERENCE" => *b"%xrf",
        "BOOKMARK" => *b"%bmk",
        "PATH" | "FILE_PATH" => *b"%pat",
        "SUMMARY" => *b"%smr",
        "USER_INFO" => *b"%usr",
        "TABLEOFCONTENTS" => *b"%toc",
        _ => *b"%unk",
    }
}

fn kind_of(ctrl_id: &[u8; 4]) -> &'static str {
    match ctrl_id {
        b"%clk" => "누름틀",
        b"%fmu" => "계산식",
        b"%hlk" => "하이퍼링크",
        b"%mmg" => "메일머지",
        b"%dte" => "날짜",
        b"%ddt" => "문서날짜",
        b"%xrf" => "상호참조",
        b"%bmk" => "책갈피",
        b"%pat" => "파일경로",
        b"%smr" => "문서요약",
        b"%usr" => "사용자정보",
        b"%cpr" => "개인정보보호",
        b"%toc" => "차례",
        b"%unk" => "알수없음",
        _ => "필드", // Revision-tracking kinds use the generic label.
    }
}

/// List every field in encounter order, recursing through body, table cells and text boxes.
pub fn list_fields(doc: &Document) -> Vec<FieldInfo> {
    let mut out = Vec::new();
    for section in &doc.sections {
        for para in &section.paragraphs {
            collect_fields(para, &mut out);
        }
    }
    out
}

fn collect_fields(para: &Paragraph, out: &mut Vec<FieldInfo>) {
    for (i, ch) in para.chars.iter().enumerate() {
        if let HwpChar::ExtCtrl {
            code,
            ctrl_id,
            ctrl_index,
            ..
        } = ch
            && *code == FIELD_START
        {
            let (name, command) = ctrl_index
                .and_then(|idx| para.controls.get(idx as usize))
                .map(field_meta)
                .unwrap_or((None, None));
            out.push(FieldInfo {
                kind: kind_of(ctrl_id).to_string(),
                ctrl_id: String::from_utf8_lossy(ctrl_id).into_owned(),
                name,
                command,
                value: field_value(&para.chars, i),
            });
        }
    }
    for ctrl in &para.controls {
        for_each_nested(ctrl, &mut |p| collect_fields(p, out));
    }
}

/// Display text after FIELD_START at `start` and before FIELD_END.
fn field_value(chars: &[HwpChar], start: usize) -> String {
    let mut s = String::new();
    for ch in &chars[start + 1..] {
        match ch {
            HwpChar::Text(c) => s.push(*c),
            HwpChar::CharCtrl(10) => s.push('\n'),
            HwpChar::InlineCtrl { code, .. } if *code == hwp_model::ctrl_char::TAB => s.push('\t'),
            HwpChar::InlineCtrl { code, .. } if *code == FIELD_END => break,
            _ => {}
        }
    }
    s
}

/// Restore the target URL of a %hlk control for Markdown/HTML link emission.
/// Display text is between FIELD_START and FIELD_END. The command stores the URL as
/// `{URL};1;0;0;`: take text before the first unescaped semicolon, then undo
/// the backslash escaping performed by [`hlk_command`].
pub fn hyperlink_url(ctrl: &Control) -> Option<String> {
    let Control::Generic(g) = ctrl else {
        return None;
    };
    if &g.ctrl_id != b"%hlk" {
        return None;
    }
    let cmd = parse_command(&g.data)?;
    Some(unescape_hlk_url(&cmd))
}

/// Invert [`hlk_command`]: take the URL before the first unescaped semicolon delimiter
/// and remove its backslash escapes.
fn unescape_hlk_url(cmd: &str) -> String {
    let mut out = String::with_capacity(cmd.len());
    let mut it = cmd.chars();
    while let Some(c) = it.next() {
        match c {
            '\\' => {
                if let Some(n) = it.next() {
                    out.push(n);
                }
            }
            ';' => break,
            _ => out.push(c),
        }
    }
    out
}

/// Read the name and command of a field control.
pub fn field_meta(ctrl: &Control) -> (Option<String>, Option<String>) {
    let Control::Generic(g) = ctrl else {
        return (None, None);
    };
    let name = g
        .raw_children
        .iter()
        .find(|r| r.tag == CTRL_DATA_TAG)
        .and_then(|r| first_bstr(&r.data));
    (name, parse_command(&g.data))
}

/// Field command data without ctrl_id: attributes(4), extra(1), length(2), WCHAR[length], id(4).
fn parse_command(data: &[u8]) -> Option<String> {
    if data.len() < 7 {
        return None;
    }
    let len = u16::from_le_bytes([data[5], data[6]]) as usize;
    let end = 7usize.checked_add(len * 2)?;
    if end > data.len() || len == 0 {
        return None;
    }
    Some(decode_utf16le(&data[7..end]))
}

/// Read the first BSTR (field name) in CTRL_DATA; stop at an unknown item type.
fn first_bstr(data: &[u8]) -> Option<String> {
    if data.len() < 4 {
        return None;
    }
    let count = i16::from_le_bytes([data[2], data[3]]).max(0) as usize;
    let mut o = 4usize;
    for _ in 0..count {
        if o + 4 > data.len() {
            break;
        }
        let item_type = u16::from_le_bytes([data[o + 2], data[o + 3]]);
        o += 4;
        match item_type {
            0 => {} // PIT_NULL
            1 => {
                // PIT_BSTR: UINT32 len + WCHAR[len]
                if o + 4 > data.len() {
                    break;
                }
                let len =
                    u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) as usize;
                o += 4;
                let end = o.checked_add(len * 2)?;
                if end > data.len() {
                    break;
                }
                return Some(decode_utf16le(&data[o..end]));
            }
            2 | 6 => o += 1,         // I1/UI1
            3 | 7 => o += 2,         // I2/UI2
            4 | 5 | 8 | 9 => o += 4, // I4/I/UI4/UI
            _ => break,              // Stop safely at SET/ARRAY/BINDATA or unknown types.
        }
    }
    None
}

fn decode_utf16le(b: &[u8]) -> String {
    let units: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

/// Fill matching named fields and return the count, refusing edits that shift opaque range tags.
pub fn set_field(doc: &mut Document, name: &str, value: &str) -> Result<usize, String> {
    if crate::edit::document_has_range_tags(doc) {
        for section in &mut doc.sections {
            for para in &mut section.paragraphs {
                set_field_para(para, name, value, false)?;
            }
        }
    }
    let mut count = 0;
    for section in &mut doc.sections {
        for para in &mut section.paragraphs {
            count += set_field_para(para, name, value, true)?;
        }
    }
    Ok(count)
}

fn set_field_para(
    para: &mut Paragraph,
    name: &str,
    value: &str,
    apply: bool,
) -> Result<usize, String> {
    // Scan without mutation: value start/end character indices, WCHAR position and old WCHAR width.
    let mut regions: Vec<(usize, usize, u32, u32)> = Vec::new();
    let mut wpos = 0u32;
    for (idx, ch) in para.chars.iter().enumerate() {
        if let HwpChar::ExtCtrl {
            code, ctrl_index, ..
        } = ch
            && *code == FIELD_START
        {
            let nm = ctrl_index
                .and_then(|i| para.controls.get(i as usize))
                .and_then(|c| field_meta(c).0);
            if nm.as_deref() == Some(name) {
                let value_wpos = wpos + ch.wchar_width();
                let mut e = idx + 1;
                let mut vw = 0u32;
                while e < para.chars.len() {
                    match &para.chars[e] {
                        HwpChar::InlineCtrl { code, .. } if *code == FIELD_END => break,
                        other => vw += other.wchar_width(),
                    }
                    e += 1;
                }
                regions.push((idx + 1, e, value_wpos, vw));
            }
        }
        wpos += ch.wchar_width();
    }

    let count = regions.len();
    let new_wlen = build_value_chars(value)
        .iter()
        .map(HwpChar::wchar_width)
        .sum();
    // Apply from the end so earlier character indices stay valid.
    for (s, e, vp, old_wlen) in regions.into_iter().rev() {
        crate::edit::ensure_range_tag_shift_allowed(para, old_wlen, new_wlen)?;
        if apply {
            para.chars.splice(s..e, build_value_chars(value));
            adjust_runs(&mut para.char_shape_runs, vp, old_wlen, new_wlen);
        }
    }
    if apply && count > 0 {
        para.line_segs.clear();
    }

    let mut nested = 0;
    for ctrl in &mut para.controls {
        match ctrl {
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        nested += set_field_para(p, name, value, apply)?;
                    }
                }
            }
            Control::Generic(g) => {
                let before = nested;
                for l in &mut g.paragraph_lists {
                    for p in &mut l.paragraphs {
                        nested += set_field_para(p, name, value, apply)?;
                    }
                }
                if apply && nested > before {
                    // Changed content makes the captured object XML stale.
                    g.hwpx_raw_xml = None;
                }
            }
            _ => {}
        }
    }
    Ok(count + nested)
}

/// Field tabs use the same eight-WCHAR inline representation emitted by the HWP5 writer.
/// Both preflight and insertion consume this builder, including field/hyperlink creation.
fn build_value_chars(value: &str) -> Vec<HwpChar> {
    value
        .chars()
        .map(|c| match c {
            '\t' => HwpChar::InlineCtrl {
                code: hwp_model::ctrl_char::TAB,
                payload: vec![0; 12],
            },
            '\n' => HwpChar::CharCtrl(hwp_model::ctrl_char::LINE_BREAK),
            c => HwpChar::Text(c),
        })
        .collect()
}

/// One `{{name}}` text placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceholderInfo {
    /// Placeholder name inside braces, with surrounding whitespace trimmed.
    pub name: String,
    /// Number of occurrences throughout the document.
    pub occurrences: usize,
}

/// Collect the `{{ name }}` text slots of the body, table cells and text boxes, in order of
/// appearance.
///
/// Separate from form fields ([`list_fields`]): these are plain-text template slots. The
/// grammar is [`hwp_model::slot_tokens`], the one `hwp fill` matches with. Only contiguous text
/// inside one paragraph is scanned, so a slot never spans a control character or a line break.
/// Fill slots with [`replace_slots`].
pub fn scan_placeholders(doc: &Document) -> Vec<PlaceholderInfo> {
    let mut out: Vec<PlaceholderInfo> = Vec::new();
    for section in &doc.sections {
        for para in &section.paragraphs {
            for_each_text_segment(para, &mut |seg| scan_segment(seg, &mut out));
        }
    }
    out
}

/// Replace every requested `{{ name }}` of the body, table cells and text boxes with its value
/// in one pass over the original text: a value is literal and never scanned again, and each
/// token is filled once whatever its spelling. Keys are trimmed like names
/// ([`hwp_model::slot_lookup`]); keys naming one slot with equal values each count its tokens.
/// Returns the count per key as the caller spelled it, every key included.
/// Validate every selected edit before mutation when the document carries opaque range tags.
pub fn replace_slots(
    doc: &mut Document,
    values: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, usize>, String> {
    let lookup = hwp_model::slot_lookup(values)?;
    if crate::edit::document_has_range_tags(doc) {
        let mut ignored = BTreeMap::new();
        for section in &mut doc.sections {
            for para in &mut section.paragraphs {
                replace_slots_in(para, &lookup, &mut ignored, false)?;
            }
        }
    }
    let mut counts: BTreeMap<String, usize> = values.keys().map(|k| (k.clone(), 0)).collect();
    for section in &mut doc.sections {
        for para in &mut section.paragraphs {
            replace_slots_in(para, &lookup, &mut counts, true)?;
        }
    }
    Ok(counts)
}

/// Validate or apply [`replace_slots`] on `para` and its nested paragraphs.
/// Only the apply pass invalidates captured raw XML and writes characters.
fn replace_slots_in(
    para: &mut Paragraph,
    lookup: &BTreeMap<&str, hwp_model::SlotRequest<'_, String>>,
    counts: &mut BTreeMap<String, usize>,
    apply: bool,
) -> Result<bool, String> {
    let mut edits = Vec::new();
    for (start, seg) in text_segments(para) {
        for token in hwp_model::slot_tokens(&seg) {
            if let Some(request) = lookup.get(token.name) {
                let from = start + seg[..token.range.start].chars().count();
                let to = from + seg[token.range.clone()].chars().count();
                edits.push((from..to, request.value.clone()));
                for key in &request.keys {
                    *counts.entry((*key).to_string()).or_default() += 1;
                }
            }
        }
    }
    let mut changed = !edits.is_empty();
    if apply {
        crate::edit::splice_edits(para, edits)?;
    } else {
        crate::edit::check_splice_edits(para, &edits)?;
    }
    for ctrl in &mut para.controls {
        match ctrl {
            Control::Table(table) => {
                for cell in &mut table.cells {
                    for p in &mut cell.paragraphs {
                        changed |= replace_slots_in(p, lookup, counts, apply)?;
                    }
                }
            }
            Control::Generic(generic) => {
                let mut inner = false;
                for list in &mut generic.paragraph_lists {
                    for p in &mut list.paragraphs {
                        inner |= replace_slots_in(p, lookup, counts, apply)?;
                    }
                }
                if apply && inner {
                    // The object's content changed, so its raw XML is stale.
                    generic.hwpx_raw_xml = None;
                }
                changed |= inner;
            }
            _ => {}
        }
    }
    Ok(changed)
}

/// Hand `f` each run of contiguous text characters of `para` and of its nested paragraphs.
fn for_each_text_segment(para: &Paragraph, f: &mut impl FnMut(&str)) {
    for (_, seg) in text_segments(para) {
        f(&seg);
    }
    for ctrl in &para.controls {
        for_each_nested(ctrl, &mut |p| for_each_text_segment(p, f));
    }
}

/// The runs of contiguous text characters in `para` itself (not its nested paragraphs), each
/// with the index of its first character in `para.chars`. Any other character ends a run.
pub(crate) fn text_segments(para: &Paragraph) -> Vec<(usize, String)> {
    let mut segments = Vec::new();
    let mut seg = String::new();
    let mut start = 0usize;
    for (i, ch) in para.chars.iter().enumerate() {
        match ch {
            HwpChar::Text(c) => {
                if seg.is_empty() {
                    start = i;
                }
                seg.push(*c);
            }
            _ if !seg.is_empty() => segments.push((start, std::mem::take(&mut seg))),
            _ => {}
        }
    }
    if !seg.is_empty() {
        segments.push((start, seg));
    }
    segments
}

fn scan_segment(seg: &str, out: &mut Vec<PlaceholderInfo>) {
    for token in hwp_model::slot_tokens(seg) {
        if let Some(p) = out.iter_mut().find(|p| p.name == token.name) {
            p.occurrences += 1;
        } else {
            out.push(PlaceholderInfo {
                name: token.name.to_string(),
                occurrences: 1,
            });
        }
    }
}

/// Apply `f` to nested table-cell and text-box paragraphs without mutation.
fn for_each_nested(ctrl: &Control, f: &mut impl FnMut(&Paragraph)) {
    match ctrl {
        Control::Table(t) => {
            for cell in &t.cells {
                for p in &cell.paragraphs {
                    f(p);
                }
            }
        }
        Control::Generic(g) => {
            for l in &g.paragraph_lists {
                for p in &l.paragraphs {
                    f(p);
                }
            }
        }
        _ => {}
    }
}

/// ExtCtrl payload (12 bytes): reversed ctrl_id in the first four bytes, then zeros.
pub fn rev_payload(ctrl_id: &[u8; 4]) -> Vec<u8> {
    let mut p = vec![0u8; 12];
    p[0] = ctrl_id[3];
    p[1] = ctrl_id[2];
    p[2] = ctrl_id[1];
    p[3] = ctrl_id[0];
    p
}

/// FIELD_END (code 4) payload: three reversed ctrl_id bytes (without `%`), then zero.
/// Genuine Hangul-authored test-file evidence: %hlk END starts `6b 6c 68 00` ("klh" then zero).
/// With an all-zero END payload Hangul cannot pair FIELD_START and END, so clicking fails
/// (confirmed in the fourth oracle comparison). START includes `%`; END omits it.
pub fn field_end_payload(ctrl_id: &[u8; 4]) -> Vec<u8> {
    let mut p = vec![0u8; 12];
    p[0] = ctrl_id[3];
    p[1] = ctrl_id[2];
    p[2] = ctrl_id[1];
    // p[3] stays zero; END omits the '%' marker.
    p
}

/// CTRL_DATA Parameter Set with one field-name BSTR (setid 0, count 1, item id 1).
pub fn make_field_ctrl_data(name: &str) -> Vec<u8> {
    let mut cd = vec![0u8, 0, 1, 0]; // setid=0, count=1
    cd.extend([1u8, 0, 1, 0]); // item id=1, type=BSTR(1)
    let units: Vec<u16> = name.encode_utf16().collect();
    cd.extend((units.len() as u32).to_le_bytes());
    for u in units {
        cd.extend(u.to_le_bytes());
    }
    cd
}

/// Build a field control record, with optional CTRL_DATA name and optional command
/// data such as a hyperlink URL.
fn make_field_control(ctrl_id: [u8; 4], name: Option<&str>, command: Option<&str>) -> Control {
    let data = match command {
        Some(cmd) => make_field_command_data(&ctrl_id, cmd),
        None => vec![0u8; 11], // attributes(4), extra(1), zero length(2), id(4).
    };
    let raw_children = match name {
        Some(nm) => vec![OpaqueRecord {
            tag: CTRL_DATA_TAG,
            data: make_field_ctrl_data(nm),
            children: Vec::new(),
        }],
        None => Vec::new(),
    };
    Control::Generic(GenericControl {
        ctrl_id,
        data,
        paragraph_lists: Vec::new(),
        extras: Vec::new(),
        raw_children,
        gso_shapes: Vec::new(),
        equation: None,
        column_def: None,
        caption: None,
        hwpx_raw_xml: None,
        container_box: None,
    })
}

/// Field command data: attributes(4), extra(1), length(2), WCHAR[length], nonzero id(4), trailing(4).
/// Genuine-file observations: %hlk attributes 0x00008800, extra 0; %fmu attributes 0, extra 0x08.
/// The ID must be nonzero: native testing shows Hangul treats an id=0 %hlk as plain text.
/// A genuine work_report %hlk uses id=0xd707bf6d. Hash the command for a deterministic
/// nonzero ID, so identical commands share an ID and different URLs get different IDs.
pub fn make_field_command_data(ctrl_id: &[u8; 4], command: &str) -> Vec<u8> {
    let (attr, etc): (u32, u8) = match ctrl_id {
        // Genuine Hangul %hlk uses 0x0000a800, including bit 0x2000. With only 0x00008800,
        // blue underline appears but clicking does not navigate (second native verification).
        b"%hlk" => (0x0000_a800, 0),
        b"%fmu" => (0x0000_0000, 0x08),
        _ => (0x0000_0000, 0),
    };
    let mut data = attr.to_le_bytes().to_vec();
    data.push(etc);
    let units: Vec<u16> = command.encode_utf16().collect();
    data.extend((units.len() as u16).to_le_bytes());
    for u in units {
        data.extend(u.to_le_bytes());
    }
    data.extend(field_instance_id(command).to_le_bytes()); // id(4) ≠ 0
    data.extend([0u8; 4]); // trailing(4)=0
    data
}

/// Derive a deterministic nonzero field instance ID with 32-bit FNV-1a, replacing zero with one.
fn field_instance_id(command: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in command.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    if h == 0 { 1 } else { h }
}

/// Inline hyperlink parts for streaming Markdown insertion: FIELD_START, FIELD_END and %hlk control.
/// The caller inserts display text between them, applies blue underline, then calls
/// `relink_ctrl_index`. Unlike anchored `create_hyperlink`, this inserts directly at
/// the current parsing position.
pub(crate) fn hyperlink_field_parts(url: &str) -> (HwpChar, HwpChar, Control) {
    let ctrl_id = *b"%hlk";
    (
        HwpChar::ExtCtrl {
            code: FIELD_START,
            ctrl_id,
            payload: rev_payload(&ctrl_id),
            ctrl_index: None,
        },
        HwpChar::InlineCtrl {
            code: FIELD_END,
            payload: field_end_payload(&ctrl_id),
        },
        make_field_control(ctrl_id, None, Some(&hlk_command(url))),
    )
}

/// Hyperlink command `{URL};1;0;0;`, with special URL characters escaped using genuine-file rules.
/// Version 1 escapes backslash, semicolon and colon only; complex URLs remain approximate.
fn hlk_command(url: &str) -> String {
    let mut esc = String::with_capacity(url.len() + 8);
    for c in url.chars() {
        match c {
            '\\' => esc.push_str("\\\\"),
            ';' => esc.push_str("\\;"),
            ':' => esc.push_str("\\:"),
            _ => esc.push(c),
        }
    }
    format!("{esc};1;0;0;")
}

/// Field characters: FIELD_START (ExtCtrl code 3), display value, FIELD_END (InlineCtrl code 4).
fn make_field_chars(ctrl_id: [u8; 4], value: &str) -> Vec<HwpChar> {
    let mut chars = vec![HwpChar::ExtCtrl {
        code: FIELD_START,
        ctrl_id,
        payload: rev_payload(&ctrl_id),
        ctrl_index: None,
    }];
    chars.extend(build_value_chars(value));
    chars.push(HwpChar::InlineCtrl {
        code: FIELD_END,
        payload: field_end_payload(&ctrl_id),
    });
    chars
}

/// Relink paragraph ExtCtrl characters to controls in encounter order.
pub fn relink_ctrl_index(para: &mut Paragraph) {
    let mut next = 0u32;
    for ch in &mut para.chars {
        if let HwpChar::ExtCtrl { ctrl_index, .. } = ch {
            *ctrl_index = Some(next);
            next += 1;
        }
    }
}

/// Find or add the blue-underlined character shape used by hyperlink display text.
/// Genuine work_report hyperlinks use a separate blue-underlined charPr; native testing shows
/// this shape is required for Hangul to recognize/display links. Reuse an existing shape.
fn hyperlink_char_shape(doc: &mut Document) -> CharShapeId {
    const BLUE: u32 = 0x00FF_0000; // COLORREF 0x00BBGGRR = RGB(0,0,255)
    if let Some(i) = doc
        .header
        .char_shapes
        .iter()
        .position(|c| c.text_color == BLUE && c.underline_kind() == 1)
    {
        return CharShapeId(i as u16);
    }
    let mut cs = doc.header.char_shapes.first().cloned().unwrap_or_default();
    cs.text_color = BLUE;
    cs.underline_color = BLUE;
    cs.attr = (cs.attr & !(0x3 << 2)) | (1 << 2); // Underline type 1, below the text.
    if cs.shade_color == 0 {
        cs.shade_color = 0xFFFF_FFFF; // Avoid black shading.
    }
    let id = doc.header.char_shapes.len() as u16;
    doc.header.char_shapes.push(cs);
    CharShapeId(id)
}

/// Apply `shape` to the [start, end) WCHAR interval and restore the previous shape at its end.
fn apply_run_style(runs: &mut Vec<(u32, CharShapeId)>, start: u32, end: u32, shape: CharShapeId) {
    if end <= start {
        return;
    }
    let after = runs
        .iter()
        .rfind(|(p, _)| *p <= end)
        .map(|(_, id)| *id)
        .unwrap_or_default();
    runs.retain(|(p, _)| *p < start || *p > end);
    runs.push((start, shape));
    runs.push((end, after));
    runs.sort_by_key(|(p, _)| *p);
    let mut out: Vec<(u32, CharShapeId)> = Vec::with_capacity(runs.len());
    for &(p, id) in runs.iter() {
        match out.last() {
            Some(&(_, lid)) if lid == id => {} // Drop redundant boundaries between identical shapes.
            _ => out.push((p, id)),
        }
    }
    if out.first().map(|(p, _)| *p) != Some(0) {
        out.insert(0, (0, CharShapeId::default()));
    }
    *runs = out;
}

/// Insert a field after an anchor in one paragraph, returning whether it was inserted.
/// When supplied, apply `value_shape` to the display text (blue underline for hyperlinks).
fn create_field_in_para(
    para: &mut Paragraph,
    anchor: &str,
    ctrl_id: [u8; 4],
    name: Option<&str>,
    command: Option<&str>,
    value: &str,
    value_shape: Option<CharShapeId>,
) -> bool {
    let Some((cidx, wpos)) = find_match(&para.chars, anchor, 0) else {
        return false;
    };
    let ins = (cidx + anchor.chars().count()).min(para.chars.len());
    let iw = wpos + utf16_len(anchor);
    // Insert the control after earlier ExtCtrl characters, keeping controls in character order.
    let ci = para.chars[..ins]
        .iter()
        .filter(|c| matches!(c, HwpChar::ExtCtrl { .. }))
        .count()
        .min(para.controls.len());
    para.controls
        .insert(ci, make_field_control(ctrl_id, name, command));
    let field_chars = make_field_chars(ctrl_id, value);
    let inserted_w: u32 = field_chars.iter().map(HwpChar::wchar_width).sum();
    para.chars.splice(ins..ins, field_chars);
    adjust_runs(&mut para.char_shape_runs, iw, 0, inserted_w);
    // Apply the hyperlink shape to the display interval after FIELD_START (eight WCHARs).
    if let Some(shape) = value_shape {
        let dstart = iw + 8;
        apply_run_style(
            &mut para.char_shape_runs,
            dstart,
            iw + inserted_w - 8, // Restore before the eight-WCHAR FIELD_END control.
            shape,
        );
    }
    relink_ctrl_index(para);
    para.header.ctrl_mask = 0; // The writer recomputes this from characters, including FIELD_START bit 3.
    para.line_segs.clear();
    true
}

/// Insert at the first matching body, table-cell or text-box paragraph recursively.
fn create_field_rec(
    para: &mut Paragraph,
    anchor: &str,
    ctrl_id: [u8; 4],
    name: Option<&str>,
    command: Option<&str>,
    value: &str,
    value_shape: Option<CharShapeId>,
) -> bool {
    if create_field_in_para(para, anchor, ctrl_id, name, command, value, value_shape) {
        return true;
    }
    for ctrl in &mut para.controls {
        match ctrl {
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        if create_field_rec(p, anchor, ctrl_id, name, command, value, value_shape) {
                            return true;
                        }
                    }
                }
            }
            Control::Generic(g) => {
                for l in &mut g.paragraph_lists {
                    for p in &mut l.paragraphs {
                        if create_field_rec(p, anchor, ctrl_id, name, command, value, value_shape) {
                            // Changed content makes the captured object XML stale.
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

/// Insert at the first anchor match in body, table cells or text boxes.
fn create_field_generic(
    doc: &mut Document,
    anchor: &str,
    ctrl_id: [u8; 4],
    name: Option<&str>,
    command: Option<&str>,
    value: &str,
    value_shape: Option<CharShapeId>,
) -> bool {
    for section in &mut doc.sections {
        for para in &mut section.paragraphs {
            if create_field_rec(para, anchor, ctrl_id, name, command, value, value_shape) {
                return true;
            }
        }
    }
    false
}

/// Insert a named %clk field after the first `anchor`, with `value` as its display text.
/// Return whether it was inserted, or refuse range-tag shifts. Fill it later with `set_field`.
pub fn create_field(
    doc: &mut Document,
    anchor: &str,
    name: &str,
    value: &str,
) -> Result<bool, String> {
    let inserted_w = make_field_chars(*b"%clk", value)
        .iter()
        .map(HwpChar::wchar_width)
        .sum();
    crate::edit::ensure_anchor_shift_allowed(doc, anchor, inserted_w)?;
    Ok(create_field_generic(
        doc,
        anchor,
        *b"%clk",
        Some(name),
        None,
        value,
        None,
    ))
}

/// Insert a %hlk hyperlink after `anchor`, with `display` as text and `url` as its target.
/// Return whether it was inserted, or refuse range-tag shifts. Both writers emit the same field.
/// Blue underline makes Hangul recognize/display the link; native tests show plain text otherwise.
pub fn create_hyperlink(
    doc: &mut Document,
    anchor: &str,
    url: &str,
    display: &str,
) -> Result<bool, String> {
    let inserted_w = make_field_chars(*b"%hlk", display)
        .iter()
        .map(HwpChar::wchar_width)
        .sum();
    crate::edit::ensure_anchor_shift_allowed(doc, anchor, inserted_w)?;
    let cmd = hlk_command(url);
    let shape = hyperlink_char_shape(doc);
    Ok(create_field_generic(
        doc,
        anchor,
        *b"%hlk",
        None,
        Some(&cmd),
        display,
        Some(shape),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwp_model::opaque::OpaqueRecord;
    use hwp_model::{CharShapeId, GenericControl};

    fn wbytes(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn parse_command_계산식() {
        // attributes(4), extra(1), length(2), WCHAR[length], id(4).
        let cmd = "SUM(A1:A3)";
        let mut data = vec![0u8; 5];
        data.extend((cmd.encode_utf16().count() as u16).to_le_bytes());
        data.extend(wbytes(cmd));
        data.extend([0u8; 4]);
        assert_eq!(parse_command(&data).as_deref(), Some("SUM(A1:A3)"));
    }

    #[test]
    fn first_bstr_파라미터셋() {
        // SetID(2), count(2)=2; item 1 is I2 with two payload bytes; item 2 is BSTR.
        let name = "수신";
        let mut data = vec![0, 0, 2, 0]; // setid=0, count=2
        data.extend([1, 0, 3, 0, 0xAA, 0xBB]); // item id=1 type=I2(3) value 2B
        data.extend([2, 0, 1, 0]); // item id=2 type=BSTR(1)
        data.extend((name.encode_utf16().count() as u32).to_le_bytes());
        data.extend(wbytes(name));
        assert_eq!(first_bstr(&data).as_deref(), Some("수신"));
    }

    /// Synthetic paragraph with one named click-here field.
    fn field_para(name: &str, value: &str) -> Paragraph {
        // CTRL_DATA: Parameter Set(setid=0, count=1) + item(id=1, type=BSTR) + len + WCHAR[]
        let mut cd = vec![0, 0, 1, 0];
        cd.extend([1, 0, 1, 0]); // item id=1, type=BSTR(1)
        cd.extend((name.encode_utf16().count() as u32).to_le_bytes());
        cd.extend(wbytes(name));
        let ctrl = Control::Generic(GenericControl {
            ctrl_id: *b"%clk",
            data: vec![0u8; 11], // attributes(4), extra(1), zero length(2), id(4).
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: vec![OpaqueRecord {
                tag: CTRL_DATA_TAG,
                data: cd,
                children: Vec::new(),
            }],
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        });
        let mut chars = vec![HwpChar::ExtCtrl {
            code: FIELD_START,
            ctrl_id: *b"%clk",
            payload: vec![0u8; 12],
            ctrl_index: Some(0),
        }];
        chars.extend(value.chars().map(HwpChar::Text));
        chars.push(HwpChar::InlineCtrl {
            code: FIELD_END,
            payload: vec![0u8; 12],
        });
        chars.push(HwpChar::CharCtrl(13));
        Paragraph {
            chars,
            char_shape_runs: vec![(0, CharShapeId(0))],
            controls: vec![ctrl],
            ..Default::default()
        }
    }

    #[test]
    fn list_fields_이름_종류_값() {
        let mut doc = Document::default();
        doc.sections.push(hwp_model::Section {
            paragraphs: vec![field_para("수신", "내부결재")],
            extras: Vec::new(),
        });
        let fields = list_fields(&doc);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].kind, "누름틀");
        assert_eq!(fields[0].name.as_deref(), Some("수신"));
        assert_eq!(fields[0].value, "내부결재");
    }

    #[test]
    fn scan_placeholders_이름_횟수() {
        let doc = crate::from_markdown::from_markdown("{{기관명}} 안녕 {{제목}} 끝 {{기관명}}\n");
        let slots = scan_placeholders(&doc);
        let map: std::collections::HashMap<_, _> = slots
            .iter()
            .map(|p| (p.name.as_str(), p.occurrences))
            .collect();
        assert_eq!(map.get("기관명"), Some(&2));
        assert_eq!(map.get("제목"), Some(&1));
        assert_eq!(slots.len(), 2);
    }

    /// One pass: values are literal (no chaining), each spelling of a name is filled once,
    /// keys are trimmed like names, and counts stay under the caller's key (#362).
    #[test]
    fn replace_slots_is_one_literal_pass() {
        let mut doc = crate::from_markdown::from_markdown(
            "{{a}} {{ a }} {{b}} / {{성 명}} {{사업명(국문)}}\n\n| {{ b }} |\n|---|\n",
        );
        let values = BTreeMap::from([
            ("a".to_string(), "{{b}}".to_string()),
            ("b".to_string(), "B".to_string()),
            (" 성 명 ".to_string(), "홍길동".to_string()),
            ("사업명(국문)".to_string(), "X".to_string()),
            ("없음".to_string(), "Y".to_string()),
        ]);
        let counts = replace_slots(&mut doc, &values).unwrap();
        assert_eq!(counts["a"], 2);
        assert_eq!(counts["b"], 2);
        assert_eq!(counts[" 성 명 "], 1);
        assert_eq!(counts["없음"], 0);
        let text = doc.plain_text();
        assert!(text.contains("{{b}} {{b}} B / 홍길동 X"), "{text}");
        assert!(text.contains("\nB"), "the cell slot is filled: {text}");
    }

    #[test]
    fn set_field_값_교체() {
        let mut doc = Document::default();
        doc.sections.push(hwp_model::Section {
            paragraphs: vec![field_para("수신", "")], // Empty click-here field.
            extras: Vec::new(),
        });
        let n = set_field(&mut doc, "수신", "기획팀").unwrap();
        assert_eq!(n, 1);
        let fields = list_fields(&doc);
        assert_eq!(fields[0].value, "기획팀");
        // A different name is unchanged.
        assert_eq!(set_field(&mut doc, "없는이름", "x").unwrap(), 0);
    }

    #[test]
    fn field_tab_creation_tracks_serialized_width_and_value() {
        let mut doc = crate::from_markdown("anchor tail");
        let before: u32 = doc.sections[0].paragraphs[0]
            .chars
            .iter()
            .map(HwpChar::wchar_width)
            .sum();
        assert!(create_field(&mut doc, "anchor", "entry", "\t").unwrap());
        assert_eq!(list_fields(&doc)[0].value, "\t");
        let after: u32 = doc.sections[0].paragraphs[0]
            .chars
            .iter()
            .map(HwpChar::wchar_width)
            .sum();
        assert_eq!(
            after - before,
            24,
            "two eight-WCHAR field controls surround an eight-WCHAR tab"
        );
    }

    #[test]
    fn field_tab_hyperlink_style_covers_the_complete_display_width() {
        let mut doc = crate::from_markdown("anchor tail");
        assert!(create_hyperlink(&mut doc, "anchor", "https://example.com", "A\tB").unwrap());
        assert_eq!(list_fields(&doc)[0].value, "A\tB");
        let shape = CharShapeId(
            doc.header
                .char_shapes
                .iter()
                .position(|shape| shape.text_color == 0x00FF_0000 && shape.underline_kind() == 1)
                .unwrap() as u16,
        );
        let para = &doc.sections[0].paragraphs[0];
        let start = para
            .chars
            .iter()
            .position(|ch| {
                matches!(ch,
            HwpChar::ExtCtrl { code, .. } if *code == FIELD_START)
            })
            .unwrap();
        let display_start: u32 = para.chars[..=start].iter().map(HwpChar::wchar_width).sum();
        let display_end = display_start + 10; // A + eight-WCHAR tab + B.
        let last_shape = para
            .char_shape_runs
            .iter()
            .rev()
            .find(|(pos, _)| *pos < display_end)
            .map(|(_, id)| *id);
        assert_eq!(
            last_shape,
            Some(shape),
            "the final display character must retain hyperlink style"
        );
        assert!(
            para.char_shape_runs
                .iter()
                .any(|(pos, id)| *pos == display_end && *id != shape),
            "restore the original shape at the serialized display end: {:?}",
            para.char_shape_runs
        );
    }

    #[test]
    fn create_field_삽입_후_읽기_채우기() {
        let mut doc = crate::from_markdown::from_markdown("수신: 부서");
        // Insert after the anchor.
        assert!(create_field(&mut doc, "수신:", "수신처", "").unwrap());
        let fields = list_fields(&doc);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].kind, "누름틀");
        assert_eq!(fields[0].ctrl_id, "%clk");
        assert_eq!(fields[0].name.as_deref(), Some("수신처"));
        assert_eq!(fields[0].value, "");
        // Fill the created field by name.
        assert_eq!(set_field(&mut doc, "수신처", "홍길동").unwrap(), 1);
        assert_eq!(list_fields(&doc)[0].value, "홍길동");
        // A missing anchor inserts nothing.
        assert!(!create_field(&mut doc, "없는앵커", "x", "").unwrap());
    }

    #[test]
    fn create_field_ctrl_index_정합() {
        // Inserted ExtCtrl and controls[ctrl_index] agree in encounter order.
        let mut doc = crate::from_markdown::from_markdown("가나다");
        assert!(create_field(&mut doc, "가", "필드", "값").unwrap());
        let para = &doc.sections[0].paragraphs[0];
        // The ExtCtrl index points to the %clk control.
        let ext = para
            .chars
            .iter()
            .find_map(|c| match c {
                HwpChar::ExtCtrl {
                    code, ctrl_index, ..
                } if *code == FIELD_START => *ctrl_index,
                _ => None,
            })
            .expect("ExtCtrl 존재");
        match &para.controls[ext as usize] {
            Control::Generic(g) => assert_eq!(&g.ctrl_id, b"%clk"),
            _ => panic!("필드 컨트롤이 %clk Generic이어야"),
        }
        // The first four ExtCtrl payload bytes contain the reversed ctrl_id ("%clk" to "klc%").
        // Filter by code to exclude the leading secd ExtCtrl.
        let payload = para.chars.iter().find_map(|c| match c {
            HwpChar::ExtCtrl { code, payload, .. } if *code == FIELD_START => Some(payload.clone()),
            _ => None,
        });
        assert_eq!(&payload.unwrap()[..4], b"klc%");
    }

    #[test]
    fn hlk_command_정품_포맷() {
        // Match genuine work_report.hwp: escape URL colons and append `;1;0;0;`.
        assert_eq!(
            hlk_command("http://hangeul.naver.com/font"),
            "http\\://hangeul.naver.com/font;1;0;0;"
        );
    }

    #[test]
    fn make_field_command_data_바이트() {
        // Genuine %hlk data has attributes, extra, length, WCHARs, nonzero id and trailing bytes.
        let cmd = "http\\://hangeul.naver.com/font;1;0;0;";
        let data = make_field_command_data(b"%hlk", cmd);
        assert_eq!(&data[0..5], &[0x00, 0xa8, 0x00, 0x00, 0x00]); // attr=0x0000a800+etc
        let len = u16::from_le_bytes([data[5], data[6]]) as usize;
        assert_eq!(len, cmd.encode_utf16().count());
        assert_eq!(len, 37); // Same length as the genuine work_report %hlk.
        // parse_command restores the command unchanged.
        assert_eq!(parse_command(&data).as_deref(), Some(cmd));
        // The ID must be nonzero for Hangul to recognize the hyperlink (native verification).
        let id = u32::from_le_bytes([
            data[7 + len * 2],
            data[8 + len * 2],
            data[9 + len * 2],
            data[10 + len * 2],
        ]);
        assert_ne!(id, 0, "필드 id는 비영");
        // trailing(4)=0.
        assert_eq!(&data[11 + len * 2..], &[0u8; 4]);
    }

    #[test]
    fn make_field_command_data_종류별_속성() {
        // Genuine %fmu uses attributes 0 and extra 0x08; check the observed %hlk attributes too.
        let fmu = make_field_command_data(b"%fmu", "=SUM(A1:A2)");
        assert_eq!(&fmu[0..5], &[0x00, 0x00, 0x00, 0x00, 0x08]);
        let hlk = make_field_command_data(b"%hlk", "x;1;0;0;");
        assert_eq!(&hlk[0..5], &[0x00, 0xa8, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn create_hyperlink_삽입_후_읽기() {
        let mut doc = crate::from_markdown::from_markdown("자세히: 여기");
        assert!(create_hyperlink(&mut doc, "자세히:", "https://example.com/path", "링크").unwrap());
        let fields = list_fields(&doc);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].kind, "하이퍼링크");
        assert_eq!(fields[0].ctrl_id, "%hlk");
        assert_eq!(fields[0].name, None);
        assert_eq!(fields[0].value, "링크");
        assert_eq!(
            fields[0].command.as_deref(),
            Some("https\\://example.com/path;1;0;0;")
        );
    }

    #[test]
    fn hyperlink_url_커맨드에서_복원() {
        // Restore and unescape the URL from the %hlk command.
        let ctrl = make_field_control(*b"%hlk", None, Some(&hlk_command("http://x.io/a")));
        assert_eq!(hyperlink_url(&ctrl).as_deref(), Some("http://x.io/a"));
        // Non-hyperlink controls, including %clk, return None.
        let clk = make_field_control(*b"%clk", Some("이름"), None);
        assert_eq!(hyperlink_url(&clk), None);
    }

    #[test]
    fn create_hyperlink_표시텍스트_파랑밑줄() {
        // Display text must have the blue-underlined character shape of a genuine hyperlink.
        // Without it, Hangul treats the field as plain text (native verification).
        let mut doc = crate::from_markdown::from_markdown("여기: 자리");
        assert!(create_hyperlink(&mut doc, "여기:", "https://x.io", "링크텍스트").unwrap());
        // The header now includes a blue-underlined character shape.
        let hlk = doc
            .header
            .char_shapes
            .iter()
            .position(|c| c.text_color == 0x00FF_0000 && c.underline_kind() == 1)
            .expect("하이퍼링크 글자모양");
        let para = &doc.sections[0].paragraphs[0];
        // The display interval has a hyperlink-shape run; check values because leading controls
        // change its position. This display text is five WCHARs wide.
        let hlk_run = para
            .char_shape_runs
            .iter()
            .find(|(_, id)| id.0 == hlk as u16)
            .map(|(p, _)| *p)
            .expect("하이퍼링크 글자모양 run");
        // A boundary after the link must restore the original shape.
        assert!(
            para.char_shape_runs
                .iter()
                .any(|(p, id)| *p > hlk_run && id.0 != hlk as u16),
            "링크 구간 뒤 원래 글자모양 복원: {:?}",
            para.char_shape_runs
        );
    }

    #[test]
    fn field_end_payload_ctrl_id_담김() {
        // Genuine Hangul-authored test-file END has three reversed ctrl_id bytes without `%`, then zero.
        // %hlk END starts 6b 6c 68 00; all-zero bytes prevent pairing and clicking in Hangul.
        assert_eq!(
            field_end_payload(b"%hlk"),
            vec![0x6b, 0x6c, 0x68, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        // The created hyperlink END payload (InlineCtrl code 4) contains the nonzero ctrl_id bytes.
        let mut doc = crate::from_markdown::from_markdown("여기: 자리");
        assert!(create_hyperlink(&mut doc, "여기:", "https://x.io", "링크").unwrap());
        let end = doc.sections[0].paragraphs[0]
            .chars
            .iter()
            .find_map(|c| match c {
                HwpChar::InlineCtrl { code, payload } if *code == FIELD_END => {
                    Some(payload.clone())
                }
                _ => None,
            })
            .expect("FIELD_END");
        assert_eq!(&end[..3], &[0x6b, 0x6c, 0x68], "END payload에 역순 ctrl_id");
    }

    #[test]
    fn create_field_누름틀은_글자모양_불변() {
        // A %clk field does not add the hyperlink shape.
        let mut doc = crate::from_markdown::from_markdown("수신: 부서");
        let before = doc.header.char_shapes.len();
        assert!(create_field(&mut doc, "수신:", "수신처", "").unwrap());
        assert_eq!(doc.header.char_shapes.len(), before, "글자모양 추가 없음");
    }
}
