//! Format editing: character shapes (weight/slant/underline/strike/size/color) and alignment.
//!
//! Point matched text runs at a new CharShape for character changes, or replace
//! a paragraph's ParaShape with an alignment-adjusted copy. Appending shapes to
//! header tables is safe because the writer derives ID_MAPPINGS counts from
//! their lengths. Clear edited paragraph caches so the writer resynthesizes them.

use hwp_model::{
    CharShape, CharShapeId, Control, Document, FaceName, HwpChar, LANG_COUNT, ParaShape,
    ParaShapeId, Paragraph,
};

use crate::edit::{find_match, paragraph_has_range_tags, utf16_len};
use crate::segment_id::SegmentPath;

/// Character-format changes; None retains the existing value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CharFormat {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    /// Font size in points (base_size = points times 100).
    pub size_pt: Option<f32>,
    /// Text color as COLORREF (0x00BBGGRR).
    pub color: Option<u32>,
    /// Font name (A2): apply one name uniformly across all seven language slots,
    /// finding or inserting it in header.fonts through find_or_insert_face.
    pub font: Option<String>,
}

impl CharFormat {
    pub fn is_empty(&self) -> bool {
        self.bold.is_none()
            && self.italic.is_none()
            && self.underline.is_none()
            && self.strike.is_none()
            && self.size_pt.is_none()
            && self.color.is_none()
            && self.font.is_none()
    }
}

/// Change every text range matching pattern, recursing through body paragraphs,
/// table cells and text boxes. Return the number of ranges changed.
pub fn set_char_format(doc: &mut Document, pattern: &str, fmt: &CharFormat) -> usize {
    if pattern.is_empty() || fmt.is_empty() {
        return 0;
    }
    let Document {
        header, sections, ..
    } = doc;
    // Resolved ONCE per call, not per matched run — find_or_insert_face is idempotent on a
    // repeat name, so re-resolving per run would only waste cycles, never change the result.
    let resolved_font = fmt
        .font
        .as_deref()
        .map(|name| resolve_font_faces(&mut header.fonts, name));
    let shapes = &mut header.char_shapes;
    let mut n = 0;
    for section in sections.iter_mut() {
        for para in &mut section.paragraphs {
            n += restyle_para(para, pattern, fmt, resolved_font, shapes);
        }
    }
    n
}

fn restyle_para(
    para: &mut Paragraph,
    pattern: &str,
    fmt: &CharFormat,
    resolved_font: Option<[u16; LANG_COUNT]>,
    shapes: &mut Vec<CharShape>,
) -> usize {
    let pat_w = utf16_len(pattern);
    let pat_chars = pattern.chars().count();
    let para_len = para.wchar_len();
    let mut n = 0;
    let mut start = 0usize;
    while let Some((char_idx, wpos)) = find_match(&para.chars, pattern, start) {
        restyle_range(
            &mut para.char_shape_runs,
            wpos,
            wpos + pat_w,
            shapes,
            fmt,
            resolved_font,
            para_len,
        );
        para.line_segs.clear();
        n += 1;
        start = char_idx + pat_chars;
    }
    // Recurse through table-cell and text-box paragraphs, like replacement editing.
    for ctrl in &mut para.controls {
        match ctrl {
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        n += restyle_para(p, pattern, fmt, resolved_font, shapes);
                    }
                }
            }
            Control::Generic(g) => {
                let before = n;
                for list in &mut g.paragraph_lists {
                    for p in &mut list.paragraphs {
                        n += restyle_para(p, pattern, fmt, resolved_font, shapes);
                    }
                }
                if n > before {
                    // Changed object content invalidates captured XML; never emit stale XML.
                    g.hwpx_raw_xml = None;
                }
            }
            _ => {}
        }
    }
    n
}

/// Apply character formatting to runs in [w_start, w_end).
/// Base each run on its existing shape and change only requested properties.
fn restyle_range(
    runs: &mut Vec<(u32, CharShapeId)>,
    w_start: u32,
    w_end: u32,
    shapes: &mut Vec<CharShape>,
    fmt: &CharFormat,
    resolved_font: Option<[u16; LANG_COUNT]>,
    para_len: u32,
) {
    if w_end <= w_start {
        return;
    }
    // Invariant: a caller must never push a starting boundary beyond the paragraph's own
    // length. `restyle_range_at` re-checks this against the CURRENT paragraph before calling in
    // (a batch's earlier op may have shortened the paragraph since the range was resolved — see
    // its own doc comment); this is the release-build backstop for any future caller that
    // bypasses it. Symmetric with the existing `w_end < para_len` end-boundary guard below.
    if w_start >= para_len {
        return;
    }
    debug_assert!(
        w_start < para_len,
        "restyle_range: w_start must be inside the paragraph"
    );
    let start_id = id_at(runs, w_start);
    let end_id = id_at(runs, w_end);
    // Add missing range boundaries.
    if !runs.iter().any(|(p, _)| *p == w_start) {
        runs.push((w_start, start_id));
    }
    if w_end < para_len && !runs.iter().any(|(p, _)| *p == w_end) {
        runs.push((w_end, end_id));
    }
    runs.sort_by_key(|(p, _)| *p);
    // Replace shape IDs for runs inside the range.
    for (p, id) in runs.iter_mut() {
        if *p >= w_start && *p < w_end {
            let base = shapes.get(id.0 as usize).cloned().unwrap_or_default();
            *id = find_or_insert(shapes, apply_format(base, fmt, resolved_font));
        }
    }
    normalize_runs(runs);
}

/// Address-driven entry point for a run-range char-format edit (EDT-05): resolves the target
/// paragraph via [`crate::address::paragraph_at_mut`], re-checks the requested `[w_start,
/// w_end)` against that paragraph's CURRENT WCHAR length before mutating anything — a batch's
/// earlier op may have shortened the paragraph since the resolver saw it (planner decision 5;
/// `replace_in_chars` splices `para.chars` and calls `adjust_runs`, so the premise that a
/// paragraph's length never changes this phase is false) — then calls the existing private
/// [`restyle_range`] with the resolved pair (no pattern search) and performs the same
/// invalidation tail [`restyle_para`] already does on a touched paragraph.
///
/// Checks both ends explicitly: `w_start >= current_len` OR `w_end > current_len` both fail. A
/// stale `w_end` that still fits would otherwise silently over-apply the format past what the
/// caller addressed, since `restyle_range`'s own selection loop is `*p >= w_start && *p < w_end`.
pub fn restyle_range_at(
    doc: &mut Document,
    path: &SegmentPath,
    w_start: u32,
    w_end: u32,
    fmt: &CharFormat,
) -> Result<(), String> {
    // Resolved ONCE, before char_shapes is taken out (a different DocHeader field — no borrow
    // conflict), mirroring set_char_format's own resolve-once discipline.
    let resolved_font = fmt
        .font
        .as_deref()
        .map(|name| resolve_font_faces(&mut doc.header.fonts, name));
    let mut shapes = std::mem::take(&mut doc.header.char_shapes);
    let result = restyle_range_at_inner(doc, path, w_start, w_end, fmt, resolved_font, &mut shapes);
    doc.header.char_shapes = shapes;
    if result.is_ok() {
        crate::address::invalidate_ancestors(doc, path);
    }
    result
}

fn restyle_range_at_inner(
    doc: &mut Document,
    path: &SegmentPath,
    w_start: u32,
    w_end: u32,
    fmt: &CharFormat,
    resolved_font: Option<[u16; LANG_COUNT]>,
    shapes: &mut Vec<CharShape>,
) -> Result<(), String> {
    let para = crate::address::paragraph_at_mut(doc, path)
        .ok_or_else(|| "restyle_range_at: 주소가 가리키는 문단을 찾을 수 없습니다".to_string())?;
    let current_len = para.wchar_len();
    if w_start >= current_len || w_end > current_len {
        return Err(format!(
            "restyle_range_at: 요청 범위 [{w_start}, {w_end})가 현재 문단 길이 {current_len}을 벗어났습니다"
        ));
    }
    restyle_range(
        &mut para.char_shape_runs,
        w_start,
        w_end,
        shapes,
        fmt,
        resolved_font,
        current_len,
    );
    para.line_segs.clear();
    Ok(())
}

/// Address-driven entry point for a paragraph-style edit (EDT-05, 07-02): resolves the target
/// paragraph via [`crate::address::paragraph_at_mut`], converts `props` to IR units via
/// [`to_ir_units`] (the same HWPUNIT scaling [`set_para_props`]'s pattern path applies), then
/// calls the existing [`apply_para_props`] directly — no pattern search, no recursion into child
/// controls (an address names exactly one paragraph). Returns `false` when the resolved path no
/// longer names a paragraph or `props` is empty; this plan's ops never resize paragraph lists, so
/// the former should not happen against a preflight-resolved target.
pub fn apply_para_props_at(doc: &mut Document, path: &SegmentPath, props: &ParaProps) -> bool {
    if props.is_empty() {
        return false;
    }
    let props = to_ir_units(props);
    let mut pshapes = std::mem::take(&mut doc.header.para_shapes);
    let applied = match crate::address::paragraph_at_mut(doc, path) {
        Some(para) => {
            apply_para_props(para, &props, &mut pshapes);
            true
        }
        None => false,
    };
    doc.header.para_shapes = pshapes;
    if applied {
        crate::address::invalidate_ancestors(doc, path);
    }
    applied
}

/// Address-driven entry point for a paragraph-alignment edit (EDT-05, 07-02): resolves the target
/// paragraph via [`crate::address::paragraph_at_mut`] and applies the same attr1-bit mutation
/// [`align_para`]'s pattern path performs, with no pattern search or recursion. Mirrors
/// `align_para`'s 4-line mutation body directly rather than factoring it out, so the align-bit
/// encoding stays in exactly one place per caller; a comment on both sides notes they must move
/// together if that encoding ever changes.
pub fn set_para_align_at(doc: &mut Document, path: &SegmentPath, align: u8) -> bool {
    let mut pshapes = std::mem::take(&mut doc.header.para_shapes);
    let applied = match crate::address::paragraph_at_mut(doc, path) {
        Some(para) => {
            // Mirrors align_para's 4-line mutation body (format.rs, `align_para`) — the two must
            // move together if the align-bit encoding (attr1 bits 2..4) ever changes.
            let mut ps = pshapes
                .get(para.para_shape.0 as usize)
                .cloned()
                .unwrap_or_default();
            ps.attr1 = (ps.attr1 & !(0x7 << 2)) | ((u32::from(align) & 0x7) << 2);
            para.para_shape = find_or_insert_para(&mut pshapes, ps);
            para.line_segs.clear();
            true
        }
        None => false,
    };
    doc.header.para_shapes = pshapes;
    if applied {
        crate::address::invalidate_ancestors(doc, path);
    }
    applied
}

/// Why `keep_together` is not implemented. The paragraph-shape bits it would write (attr1 bit 17
/// keep-with-next, bit 18 keep-lines, bit 19 break-before) are **unverified**: none of the
/// genuine Hangul-saved files available (902 parsed, about 13,000 paragraph shapes) sets any of
/// them, so there is no saved byte to copy the encoding from (`docs/design/07-hangul-compat-rules.md`,
/// rule J2). The op refuses before writing and names the supported alternative.
pub const KEEP_TOGETHER_REFUSAL: &str = "keep_together는 아직 지원하지 않습니다: 문단 모양의 '다음 문단과 함께'·'문단 보호'·'문단 앞 쪽 나눔' 비트는 한글이 저장한 정품 파일에서 한 번도 관찰되지 않아 저장 형식을 추측할 수 없습니다(Hangul-saved sample required). 한글에서 해당 옵션을 켜 저장한 파일을 제공하면 근거를 맞추어 구현합니다. 새 쪽에서 시작하려면 set_break_before를 사용하세요";

/// Address-driven page-break-before for ONE body paragraph: sets or clears the page bit (0x04)
/// of the paragraph header's `break_type`, the encoding both genuine formats show (HWP5
/// `break_type`; HWPX `pageBreak="1"` on `hp:p`, which the reader maps to the same bit).
///
/// Refused, each with a cause and a remedy, because no genuine saved file shows them:
/// - a paragraph inside a table cell or any nested list (`path.indices.len() != 1`);
/// - the first paragraph of a section, which owns the section setup (`SectionDef` /
///   `break_type` 0x03 on HWP5 output, set by the writer).
///
/// Returns whether the document changed: `Ok(false)` when the bit already has the requested
/// value (nothing is touched then, caches included). A change clears the line caches of the
/// paragraph and of the body paragraphs that follow it up to the next explicit break, because
/// their page-relative positions were measured against the old boundary.
pub fn set_break_before_at(
    doc: &mut Document,
    path: &SegmentPath,
    on: bool,
) -> Result<bool, String> {
    if path.indices.len() != 1 {
        return Err("set_break_before는 표 셀 등 중첩 문단에 쓸 수 없습니다: 한글이 저장한 정품 파일에서 쪽 나눔이 표시된 문단은 모두 본문 문단이었습니다. 본문 문단 주소를 선택하세요".to_string());
    }
    let section = doc.sections.get_mut(path.section).ok_or_else(|| {
        "set_break_before: 주소가 가리키는 구역을 찾을 수 없습니다. 최신 문서에서 문단 주소를 다시 선택하세요".to_string()
    })?;
    let index = path.indices[0];
    let para = section.paragraphs.get_mut(index).ok_or_else(|| {
        "set_break_before: 주소가 가리키는 문단을 찾을 수 없습니다. 최신 문서에서 문단 주소를 다시 선택하세요".to_string()
    })?;
    if index == 0
        || para
            .controls
            .iter()
            .any(|control| matches!(control, Control::SectionDef(_)))
    {
        return Err("set_break_before는 구역의 첫 문단에 쓸 수 없습니다: 그 문단이 구역 설정을 가지고 있고 정품 파일에서도 이에 쪽 나눔이 표시된 적이 없습니다. 다음 문단에 설정하거나 새 구역이 필요하면 새 문서를 만드세요".to_string());
    }
    let current = para.header.break_type & 0x04 != 0;
    if current == on {
        return Ok(false);
    }
    if on {
        para.header.break_type |= 0x04;
    } else {
        para.header.break_type &= !0x04;
    }
    // The line caches are page-relative: this paragraph and every body paragraph after it up
    // to the next explicit break were measured with the old page boundary, so their `v_pos`
    // values (and, for a new break, the target's own accumulated offset) are stale. Dropping
    // them lets the writer re-measure from the new boundary: genuine Hangul-saved files start
    // a break paragraph at `v_pos` 0 and continue the next paragraph from it (J1). Paragraphs
    // before the target, the next break paragraph and everything after it keep their caches.
    for (offset, later) in section.paragraphs.iter_mut().enumerate().skip(index) {
        if offset > index && later.header.break_type & 0x04 != 0 {
            break;
        }
        later.line_segs.clear();
    }
    crate::address::invalidate_ancestors(doc, path);
    Ok(true)
}

/// Whether `ch` is a blank that `set_indent_level` may remove: ASCII space, ideographic space
/// (U+3000) or no-break space (U+00A0).
fn is_indent_blank(ch: char) -> bool {
    matches!(ch, ' ' | '\u{3000}' | '\u{00a0}')
}

/// Whether a control may sit between a paragraph's start and its first visible character
/// without making the blanks around it "not leading": it prints nothing itself. That is the
/// section/column definition pair a section's first paragraph opens with, a field start, a
/// bookmark and a page control. Auto numbers, tabs, objects and notes are visible, so blanks
/// after them are separators, not indentation, and stay.
fn is_invisible_leading_control(ch: &HwpChar) -> bool {
    use hwp_model::ctrl_char::{BOOKMARK, FIELD_START, PAGE_CONTROL, SECTION_COLUMN_DEF};
    matches!(
        ch,
        HwpChar::ExtCtrl { code, .. }
            if matches!(*code, SECTION_COLUMN_DEF | FIELD_START | BOOKMARK | PAGE_CONTROL)
    )
}

/// Which characters (by index) and WCHAR offsets a leading-blank strip would remove.
struct BlankPlan {
    doomed: Vec<usize>,
    /// WCHAR offsets of the removed blanks. Every accepted blank is in the BMP: one WCHAR each.
    removed: Vec<u32>,
}

/// Finds the blanks that lead `para`'s text without changing it, keeping every control where
/// it is; `None` when there is nothing to strip.
///
/// "Leading" means before the first visible character, so blanks that follow invisible
/// leading controls count (a section's first paragraph is `secd`, `cold`, then the text). A
/// paragraph with nothing after its blanks yields `None`: a spacer line is not an indent.
fn leading_blank_plan(para: &Paragraph) -> Option<BlankPlan> {
    let mut doomed: Vec<usize> = Vec::new();
    let mut removed: Vec<u32> = Vec::new();
    let mut wpos = 0u32;
    let mut reached_text = false;
    for (index, ch) in para.chars.iter().enumerate() {
        match ch {
            HwpChar::Text(c) if is_indent_blank(*c) => {
                doomed.push(index);
                removed.push(wpos);
            }
            ch if is_invisible_leading_control(ch) => {}
            HwpChar::CharCtrl(code) if *code == hwp_model::ctrl_char::PARA_BREAK => break,
            _ => {
                reached_text = true;
                break;
            }
        }
        wpos += ch.wchar_width();
    }
    if !reached_text || doomed.is_empty() {
        return None;
    }
    Some(BlankPlan { doomed, removed })
}

/// Removes the planned blanks from `para`.
///
/// Character-shape runs are rebased here rather than through `adjust_runs`: when the removed
/// blanks end exactly where a run starts, that run must win, so the first surviving character
/// keeps the shape it had. Runs whose whole span was removed collapse onto the next run. The
/// stale line layout is dropped.
fn apply_blank_plan(para: &mut Paragraph, plan: &BlankPlan) {
    let BlankPlan { doomed, removed } = plan;
    let mut next = 0usize;
    let mut position = 0usize;
    para.chars.retain(|_| {
        let keep = doomed.get(next) != Some(&position);
        if !keep {
            next += 1;
        }
        position += 1;
        keep
    });
    // A run keeps its start minus the width removed before it; two runs landing on one offset
    // leave the LATER one, which is the run that covers the first surviving character.
    let mut rebased: Vec<(u32, hwp_model::CharShapeId)> =
        Vec::with_capacity(para.char_shape_runs.len());
    for &(start, id) in &para.char_shape_runs {
        let before = removed.iter().filter(|at| **at < start).count() as u32;
        let start = start - before;
        match rebased.last_mut() {
            Some(last) if last.0 == start => *last = (start, id),
            _ => rebased.push((start, id)),
        }
    }
    para.char_shape_runs = rebased;
    para.line_segs.clear();
}

/// Address-driven, idempotent nested-indentation recipe for ONE paragraph: replaces its
/// leading blank characters with shape indentation. The paragraph's `margin_left` becomes
/// `level * step` and its `indent` becomes `-hanging` (a hanging first line), both given in
/// HWPUNIT and stored in IR units like every [`ParaProps`] field; the leading blanks, and ONLY
/// those, are removed from the text. Words, inner spacing and labels are never touched.
/// Blanks that follow invisible leading controls are removed too and the controls stay (see
/// [`leading_blank_plan`]); the first surviving character keeps its character shape.
///
/// Idempotent: a second application finds no leading blanks and an identical shape, so it
/// returns `Ok(false)`, appends no shape and leaves the paragraph, line layout included,
/// untouched. Refuses a `level` outside 0..=7, and a paragraph that carries a
/// [`crate::edit::PARA_RANGE_TAG`] record when blanks would be removed (before anything is changed).
pub fn set_indent_level_at(
    doc: &mut Document,
    path: &SegmentPath,
    level: u8,
    step: i32,
    hanging: i32,
) -> Result<bool, String> {
    if level > 7 {
        return Err(format!(
            "set_indent_level의 level은 0..=7이어야 합니다: {level}. 수준을 줄여 다시 시도하세요"
        ));
    }
    let left = i32::from(level).checked_mul(step).ok_or_else(|| {
        "set_indent_level: level과 step_mm의 곱이 범위를 벗어났습니다. 더 작은 step_mm을 쓰세요"
            .to_string()
    })?;
    let Some(para) = crate::address::paragraph_at_mut(doc, path) else {
        return Err("set_indent_level: 주소가 가리키는 문단을 찾을 수 없습니다. 최신 문서에서 문단 주소를 다시 선택하세요".to_string());
    };
    // Plan first, mutate later: every refusal happens before the document is touched.
    let plan = leading_blank_plan(para);
    if plan.is_some() && paragraph_has_range_tags(para) {
        return Err("set_indent_level은 구간 표시(형광펜·교정 부호)가 있는 문단의 앞쪽 공백을 지울 수 없습니다: 표시의 위치가 글자 위치를 가리켜 공백을 지우면 엉뚱한 글자에 표시가 걸립니다. 표시를 먼저 지우거나, 공백을 지우지 않는 set_para로 left_mm·indent_mm만 지정하세요".to_string());
    }
    let stripped = plan.is_some();
    if let Some(plan) = &plan {
        apply_blank_plan(para, plan);
    }
    let props = to_ir_units(&ParaProps {
        margin_left: Some(left),
        indent: Some(-hanging),
        ..ParaProps::default()
    });
    // Apply the shape only when it differs: `apply_para_props` drops the paragraph's native
    // line layout, which a request that is already in effect must leave alone.
    let mut pshapes = std::mem::take(&mut doc.header.para_shapes);
    let mut shape_changed = false;
    if let Some(para) = crate::address::paragraph_at_mut(doc, path) {
        let current = pshapes
            .get(para.para_shape.0 as usize)
            .cloned()
            .unwrap_or_default();
        if apply_para_shape_props(current.clone(), &props) != current {
            apply_para_props(para, &props, &mut pshapes);
            shape_changed = true;
        }
    }
    doc.header.para_shapes = pshapes;
    if stripped || shape_changed {
        crate::address::invalidate_ancestors(doc, path);
    }
    Ok(stripped || shape_changed)
}

/// Address-driven entry point for list indent/outdent (EDT-05, 07-04): resolves the target
/// paragraph via [`crate::address::paragraph_at_mut`] and moves its `head_level` by `delta`
/// (+1 for `indent_para`, -1 for `outdent_para`) WITHIN the paragraph's existing numbering or
/// bullet definition — never reading or writing the definition's `RawEntry` itself, which stays
/// opaque. Refuses when the paragraph is not a list item (`head_type` neither numbered(2) nor
/// bullet(3)) and when the resulting level would leave 1..=7 (A3: HWP5's attr1-bit range;
/// 8..=10 is an HWPX-only `list_level` extension this op does not reach) — both boundaries fail
/// loudly rather than clamp. Returns the new level on success.
pub fn shift_head_level_at(
    doc: &mut Document,
    path: &SegmentPath,
    delta: i8,
) -> Result<u8, String> {
    let mut pshapes = std::mem::take(&mut doc.header.para_shapes);
    let result = shift_head_level_at_inner(doc, path, delta, &mut pshapes);
    doc.header.para_shapes = pshapes;
    if result.is_ok() {
        crate::address::invalidate_ancestors(doc, path);
    }
    result
}

fn shift_head_level_at_inner(
    doc: &mut Document,
    path: &SegmentPath,
    delta: i8,
    pshapes: &mut Vec<ParaShape>,
) -> Result<u8, String> {
    let para = crate::address::paragraph_at_mut(doc, path).ok_or_else(|| {
        "shift_head_level_at: 주소가 가리키는 문단을 찾을 수 없습니다".to_string()
    })?;
    let ps = pshapes
        .get(para.para_shape.0 as usize)
        .cloned()
        .unwrap_or_default();
    let head_type = ps.head_type();
    if head_type != 2 && head_type != 3 {
        return Err(
            "shift_head_level_at: 목록 항목(번호 매기기 또는 글머리표)이 아닌 문단입니다"
                .to_string(),
        );
    }
    let current = ps.head_level();
    let new_level = i16::from(current) + i16::from(delta);
    if !(1..=7).contains(&new_level) {
        return Err(format!(
            "shift_head_level_at: 목록 수준이 범위(1..=7)를 벗어났습니다: 현재={current}, 요청={new_level}"
        ));
    }
    let new_level = new_level as u8;
    let mut new_ps = ps;
    new_ps.set_head_level(new_level);
    para.para_shape = find_or_insert_para(pshapes, new_ps);
    para.line_segs.clear();
    Ok(new_level)
}

/// Active character-shape ID at pos: the last run starting at or before it.
fn id_at(runs: &[(u32, CharShapeId)], pos: u32) -> CharShapeId {
    runs.iter()
        .rev()
        .find(|(p, _)| *p <= pos)
        .map(|(_, id)| *id)
        .unwrap_or(CharShapeId(0))
}

/// Restore run invariants: sort, deduplicate positions, merge adjacent IDs and start at zero.
fn normalize_runs(runs: &mut Vec<(u32, CharShapeId)>) {
    runs.sort_by_key(|(p, _)| *p);
    let mut out: Vec<(u32, CharShapeId)> = Vec::with_capacity(runs.len());
    for &(pos, id) in runs.iter() {
        match out.last_mut() {
            Some(last) if last.0 == pos => last.1 = id, // Same position: retain the later entry.
            Some(last) if last.1 == id => {}            // Remove adjacent equal IDs.
            _ => out.push((pos, id)),
        }
    }
    match out.first() {
        Some(&(0, _)) => {}
        Some(&(_, id)) => out.insert(0, (0, id)),
        None => out.push((0, CharShapeId::default())),
    }
    *runs = out;
}

/// Apply requested formatting to a base shape, retaining unspecified fields.
pub(crate) fn apply_format(
    mut cs: CharShape,
    fmt: &CharFormat,
    resolved_font: Option<[u16; LANG_COUNT]>,
) -> CharShape {
    if let Some(b) = fmt.bold {
        toggle(&mut cs.attr, 1 << 1, b);
    }
    if let Some(i) = fmt.italic {
        toggle(&mut cs.attr, 1, i);
    }
    if let Some(u) = fmt.underline {
        cs.attr &= !(0x3 << 2); // Underline kind occupies bits 2-3.
        if u {
            cs.attr |= 1 << 2; // 1 = below the text.
        }
    }
    if let Some(s) = fmt.strike {
        cs.attr &= !(0x7 << 18); // Strike bits 18-20, retained for byte round trips.
        if s {
            cs.attr |= 1 << 18;
        }
        cs.strike = s; // Semantic flag used by has_strike, rendering and HWPX SOLID output.
    }
    if let Some(sz) = fmt.size_pt {
        cs.base_size = (sz * 100.0).round().max(100.0) as i32;
    }
    if let Some(c) = fmt.color {
        cs.text_color = c;
    }
    if let Some(ids) = resolved_font {
        cs.face_ids = ids;
    }
    cs
}

fn toggle(attr: &mut u32, bit: u32, on: bool) {
    if on {
        *attr |= bit;
    } else {
        *attr &= !bit;
    }
}

pub(crate) fn find_or_insert(shapes: &mut Vec<CharShape>, cs: CharShape) -> CharShapeId {
    if let Some(i) = shapes.iter().position(|s| *s == cs) {
        return CharShapeId(i as u16);
    }
    shapes.push(cs);
    CharShapeId((shapes.len() - 1) as u16)
}

/// `find_or_insert` one level up, against a single language slot of `header.fonts`: reuse an
/// existing `FaceName` by exact name match, or append one with default metadata otherwise.
fn find_or_insert_face(fonts: &mut Vec<FaceName>, name: &str) -> u16 {
    if let Some(i) = fonts.iter().position(|f| f.name == name) {
        return i as u16;
    }
    fonts.push(FaceName {
        name: name.to_string(),
        ..FaceName::default()
    });
    (fonts.len() - 1) as u16
}

/// One font name -> a face id per language slot (A2: uniform across all seven), each resolved
/// through [`find_or_insert_face`] against that slot's own table. Called ONCE per addressed or
/// pattern-form format op (not per matched run) so a batch never re-scans the font table more
/// than once per distinct name.
pub(crate) fn resolve_font_faces(
    fonts: &mut [Vec<FaceName>; LANG_COUNT],
    name: &str,
) -> [u16; LANG_COUNT] {
    std::array::from_fn(|slot| find_or_insert_face(&mut fonts[slot], name))
}

/// Change alignment of paragraphs matching pattern, including cells and text boxes.
/// align: justify=0, left=1, right=2, center=3, distribute=4, divide=5. Return changed count.
pub fn set_para_align(doc: &mut Document, pattern: &str, align: u8) -> usize {
    if pattern.is_empty() {
        return 0;
    }
    let Document {
        header, sections, ..
    } = doc;
    let pshapes = &mut header.para_shapes;
    let mut n = 0;
    for section in sections.iter_mut() {
        for para in &mut section.paragraphs {
            n += align_para(para, pattern, align, pshapes);
        }
    }
    n
}

fn align_para(
    para: &mut Paragraph,
    pattern: &str,
    align: u8,
    pshapes: &mut Vec<ParaShape>,
) -> usize {
    let mut n = 0;
    if find_match(&para.chars, pattern, 0).is_some() {
        let mut ps = pshapes
            .get(para.para_shape.0 as usize)
            .cloned()
            .unwrap_or_default();
        // Alignment occupies attr1 bits 2-4.
        ps.attr1 = (ps.attr1 & !(0x7 << 2)) | (((u32::from(align)) & 0x7) << 2);
        para.para_shape = find_or_insert_para(pshapes, ps);
        para.line_segs.clear();
        n += 1;
    }
    for ctrl in &mut para.controls {
        match ctrl {
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        n += align_para(p, pattern, align, pshapes);
                    }
                }
            }
            Control::Generic(g) => {
                let before = n;
                for list in &mut g.paragraph_lists {
                    for p in &mut list.paragraphs {
                        n += align_para(p, pattern, align, pshapes);
                    }
                }
                if n > before {
                    // Changed object content invalidates captured XML; never emit stale XML.
                    g.hwpx_raw_xml = None;
                }
            }
            _ => {}
        }
    }
    n
}

pub(crate) fn find_or_insert_para(pshapes: &mut Vec<ParaShape>, ps: ParaShape) -> ParaShapeId {
    if let Some(i) = pshapes.iter().position(|s| *s == ps) {
        return ParaShapeId(i as u16);
    }
    pshapes.push(ps);
    ParaShapeId((pshapes.len() - 1) as u16)
}

/// Paragraph property change items (GK-4). None items keep their existing values.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParaProps {
    /// Line spacing — (kind, value). Kind 0=ratio%, 1=fixed(pt×100), 3=minimum.
    pub line_spacing: Option<(u8, i32)>,
    /// Indent/outdent (HWPUNIT).
    pub indent: Option<i32>,
    pub margin_left: Option<i32>,
    pub margin_right: Option<i32>,
    pub spacing_top: Option<i32>,
    pub spacing_bottom: Option<i32>,
    /// Alignment code written into `attr1` bits 2..4
    /// (justify=0, left=1, right=2, center=3, distribute=4).
    pub align: Option<u8>,
}

impl ParaProps {
    pub fn is_empty(&self) -> bool {
        self.line_spacing.is_none()
            && self.indent.is_none()
            && self.margin_left.is_none()
            && self.margin_right.is_none()
            && self.spacing_top.is_none()
            && self.spacing_bottom.is_none()
            && self.align.is_none()
    }
}

/// HWPUNIT -> IR (hwp5 PARA_SHAPE) unit conversion. Every entry point that stores a
/// [`ParaProps`] goes through this, so the same key and value produce the same stored
/// shape whichever flag wrote it.
pub(crate) fn to_ir_units(props: &ParaProps) -> ParaProps {
    ParaProps {
        line_spacing: props.line_spacing.map(|(kind, v)| {
            if kind == 0 {
                (kind, v) // ratio (%) needs no conversion
            } else {
                (kind, v.saturating_mul(2))
            }
        }),
        indent: props.indent.map(|v| v.saturating_mul(2)),
        margin_left: props.margin_left.map(|v| v.saturating_mul(2)),
        margin_right: props.margin_right.map(|v| v.saturating_mul(2)),
        spacing_top: props.spacing_top.map(|v| v.saturating_mul(2)),
        spacing_bottom: props.spacing_bottom.map(|v| v.saturating_mul(2)),
        // An alignment code is not a dimension - it is stored verbatim.
        align: props.align,
    }
}

/// Point one paragraph at a shape carrying `props` (an existing identical shape is reused).
/// `props` must already be in IR units - see [`to_ir_units`].
fn apply_para_props(para: &mut Paragraph, props: &ParaProps, pshapes: &mut Vec<ParaShape>) {
    let ps = pshapes
        .get(para.para_shape.0 as usize)
        .cloned()
        .unwrap_or_default();
    let ps = apply_para_shape_props(ps, props);
    para.para_shape = find_or_insert_para(pshapes, ps);
    para.line_segs.clear();
}

/// Apply paragraph properties to one shape without allocation or unit conversion.
/// `props` must already be in IR units; entry points call [`to_ir_units`] once.
pub(crate) fn apply_para_shape_props(mut ps: ParaShape, props: &ParaProps) -> ParaShape {
    if let Some((kind, value)) = props.line_spacing {
        ps.line_spacing_type = kind;
        ps.line_spacing = value;
        ps.line_spacing_old = value;
    }
    if let Some(v) = props.indent {
        ps.indent = v;
    }
    if let Some(v) = props.margin_left {
        ps.margin_left = v;
    }
    if let Some(v) = props.margin_right {
        ps.margin_right = v;
    }
    if let Some(v) = props.spacing_top {
        ps.spacing_top = v;
    }
    if let Some(v) = props.spacing_bottom {
        ps.spacing_bottom = v;
    }
    if let Some(align) = props.align {
        // Alignment occupies attr1 bits 2-4; clear then set, as in align_para.
        ps.attr1 = (ps.attr1 & !(0x7 << 2)) | ((u32::from(align) & 0x7) << 2);
    }
    ps
}

/// Changes the para shape properties of paragraphs containing `pattern` (GK-4 — recurses
/// through body, table cells, and text boxes). Points them at a new ParaShape with the changed
/// properties (or an existing identical one). Returns the number of paragraphs changed.
///
/// **Units**: the dimension fields of `props` (indent/margins/spacings) are taken as
/// HWPUNIT(1/7200") and stored converted to IR units (hwp5 PARA_SHAPE = 2× HWPUNIT). Line
/// spacing is stored as-is for ratio (0), and doubled for length kinds (1/3) (symmetric with
/// hwpx read's ×2 and write's ÷2 — based on genuine measurements).
pub fn set_para_props(doc: &mut Document, pattern: &str, props: &ParaProps) -> usize {
    if pattern.is_empty() || props.is_empty() {
        return 0;
    }
    let props = to_ir_units(props);
    let props = &props;
    let Document {
        header, sections, ..
    } = doc;
    let pshapes = &mut header.para_shapes;
    let mut n = 0;
    for section in sections.iter_mut() {
        for para in &mut section.paragraphs {
            n += props_para(para, pattern, props, pshapes);
        }
    }
    n
}

fn props_para(
    para: &mut Paragraph,
    pattern: &str,
    props: &ParaProps,
    pshapes: &mut Vec<ParaShape>,
) -> usize {
    let mut n = 0;
    if find_match(&para.chars, pattern, 0).is_some() {
        apply_para_props(para, props, pshapes);
        n += 1;
    }
    for ctrl in &mut para.controls {
        match ctrl {
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        n += props_para(p, pattern, props, pshapes);
                    }
                }
            }
            Control::Generic(g) => {
                let before = n;
                for list in &mut g.paragraph_lists {
                    for p in &mut list.paragraphs {
                        n += props_para(p, pattern, props, pshapes);
                    }
                }
                if n > before {
                    // Changed object content invalidates captured XML; never emit stale XML.
                    g.hwpx_raw_xml = None;
                }
            }
            _ => {}
        }
    }
    n
}

/// Applies `props` to **every** paragraph of the `(row, col)` cell of the `table`th table
/// (0-based, document order - the same addressing `set_cell` uses), with no text anchor.
/// Returns the number of paragraphs changed; empty `props` is a no-op returning 0.
///
/// Units follow [`set_para_props`]: dimensions are HWPUNIT and are scaled through the shared
/// [`to_ir_units`], so `--set-para` and `--set-cell-para` cannot drift.
pub fn set_cell_para_props(
    doc: &mut Document,
    table: usize,
    row: u16,
    col: u16,
    props: &ParaProps,
) -> Result<usize, String> {
    if props.is_empty() {
        return Ok(0);
    }
    let props = to_ir_units(props);
    // The table walker holds the document mutably while the closure needs the shape list,
    // so lend the list out for the duration and put it back afterwards.
    let mut pshapes = std::mem::take(&mut doc.header.para_shapes);
    let result = crate::edit::with_nth_table(doc, table, |t| {
        let cell = t
            .cells
            .iter_mut()
            .find(|c| c.row == row && c.col == col)
            .ok_or_else(|| format!("표에 셀 ({row}, {col})이 없습니다"))?;
        for para in &mut cell.paragraphs {
            apply_para_props(para, &props, &mut pshapes);
        }
        Ok(cell.paragraphs.len())
    })
    .unwrap_or_else(|| Err(format!("표 #{table}를 찾을 수 없습니다")));
    doc.header.para_shapes = pshapes;
    result
}

/// Page setup change items (GK-6). None items keep their existing values. Dimensions are HWPUNIT.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageProps {
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub margin_left: Option<i32>,
    pub margin_right: Option<i32>,
    pub margin_top: Option<i32>,
    pub margin_bottom: Option<i32>,
    /// true=landscape, false=portrait.
    pub landscape: Option<bool>,
}

/// Changes the page setup of every section definition (GK-6 — applied uniformly to multi-section
/// documents too). Returns the number of sections changed.
pub fn set_page_def(doc: &mut Document, props: &PageProps) -> usize {
    let mut n = 0;
    for section in &mut doc.sections {
        for para in &mut section.paragraphs {
            for ctrl in &mut para.controls {
                let Control::SectionDef(secd) = ctrl else {
                    continue;
                };
                let Some(page) = secd.page.as_mut() else {
                    continue;
                };
                if let Some(v) = props.width {
                    page.width.0 = v;
                }
                if let Some(v) = props.height {
                    page.height.0 = v;
                }
                if let Some(v) = props.margin_left {
                    page.margin_left.0 = v;
                }
                if let Some(v) = props.margin_right {
                    page.margin_right.0 = v;
                }
                if let Some(v) = props.margin_top {
                    page.margin_top.0 = v;
                }
                if let Some(v) = props.margin_bottom {
                    page.margin_bottom.0 = v;
                }
                if let Some(landscape) = props.landscape {
                    // attr bit0 = page orientation (landscape).
                    if landscape {
                        page.attr |= 1;
                    } else {
                        page.attr &= !1;
                    }
                }
                n += 1;
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_para_props_줄간격과_여백() {
        let mut doc = crate::from_markdown::from_markdown("본문 문단입니다.\n\n다른 문단\n");
        let props = ParaProps {
            line_spacing: Some((0, 130)),
            indent: Some(-1000),
            margin_left: Some(2000),
            spacing_top: Some(300),
            ..ParaProps::default()
        };
        assert_eq!(set_para_props(&mut doc, "본문", &props), 1);
        let para = &doc.sections[0].paragraphs[0];
        let ps = &doc.header.para_shapes[para.para_shape.0 as usize];
        // Ratio line spacing stays as-is; dimension fields are converted to IR (2×) units.
        assert_eq!(ps.line_spacing, 130);
        assert_eq!(ps.line_spacing_type, 0);
        assert_eq!(ps.indent, -2000);
        assert_eq!(ps.margin_left, 4000);
        assert_eq!(ps.spacing_top, 600);
        // The other paragraph keeps the default palette (ps2).
        let other = &doc.sections[0].paragraphs[1];
        assert_eq!(other.para_shape.0, 2);
    }

    #[test]
    fn set_para_props_고정줄간격은_2배환산() {
        let mut doc = crate::from_markdown::from_markdown("본문 문단입니다.\n");
        let props = ParaProps {
            line_spacing: Some((1, 1300)), // 13pt fixed (HWPUNIT)
            ..ParaProps::default()
        };
        assert_eq!(set_para_props(&mut doc, "본문", &props), 1);
        let para = &doc.sections[0].paragraphs[0];
        let ps = &doc.header.para_shapes[para.para_shape.0 as usize];
        assert_eq!(ps.line_spacing_type, 1);
        assert_eq!(ps.line_spacing, 2600, "고정 줄간격은 2배 단위 저장");
    }

    #[test]
    fn set_page_def_여백과_방향() {
        let mut doc = crate::from_markdown::from_markdown("본문\n");
        let props = PageProps {
            margin_left: Some(5668),
            landscape: Some(true),
            ..PageProps::default()
        };
        assert_eq!(set_page_def(&mut doc, &props), 1);
        let secd = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::SectionDef(s) => Some(s),
                _ => None,
            })
            .expect("구역 정의");
        let page = secd.page.as_ref().unwrap();
        assert_eq!(page.margin_left.0, 5668);
        assert_eq!(page.attr & 1, 1, "가로 방향");
    }
    use crate::from_markdown;

    fn body_path(index: usize) -> SegmentPath {
        SegmentPath {
            section: 0,
            indices: vec![index],
        }
    }

    #[test]
    fn set_break_before_refuses_the_first_paragraph_even_without_a_section_def() {
        // The first paragraph is refused by position, not only because it carries the section
        // setup: strip its controls and it must still be refused.
        let mut doc = from_markdown::from_markdown("하나\n\n둘\n\n셋\n");
        doc.sections[0].paragraphs[0].controls.clear();
        let error = set_break_before_at(&mut doc, &body_path(0), true).unwrap_err();
        assert!(error.contains("구역의 첫 문단"), "{error}");
        assert_eq!(doc.sections[0].paragraphs[0].header.break_type & 0x04, 0);
    }

    #[test]
    fn set_break_before_refuses_a_later_paragraph_that_owns_a_section_def() {
        let mut doc = from_markdown::from_markdown("하나\n\n둘\n\n셋\n");
        let secd = doc.sections[0].paragraphs[0]
            .controls
            .iter()
            .find(|c| matches!(c, Control::SectionDef(_)))
            .cloned()
            .expect("the first paragraph owns the section setup");
        doc.sections[0].paragraphs[2].controls.push(secd);
        let error = set_break_before_at(&mut doc, &body_path(2), true).unwrap_err();
        assert!(error.contains("구역의 첫 문단"), "{error}");
        assert_eq!(doc.sections[0].paragraphs[2].header.break_type & 0x04, 0);
        // A plain later paragraph is accepted and the repeat reports no change.
        assert_eq!(set_break_before_at(&mut doc, &body_path(1), true), Ok(true));
        assert_eq!(
            set_break_before_at(&mut doc, &body_path(1), true),
            Ok(false)
        );
        assert_eq!(
            set_break_before_at(&mut doc, &body_path(1), false),
            Ok(true)
        );
        assert_eq!(
            set_break_before_at(&mut doc, &body_path(1), false),
            Ok(false)
        );
    }

    #[test]
    fn set_indent_level_strips_a_no_break_space_lead_and_bounds_the_level() {
        let mut doc = from_markdown::from_markdown("하나\n\n둘 항목\n");
        let para = &mut doc.sections[0].paragraphs[1];
        para.chars.insert(0, HwpChar::Text('\u{00a0}'));
        para.chars.insert(0, HwpChar::Text(' '));
        assert_eq!(
            set_indent_level_at(&mut doc, &body_path(1), 1, 100, 50),
            Ok(true)
        );
        assert_eq!(doc.sections[0].paragraphs[1].plain_text(), "둘 항목");
        // Level 7 is the last accepted, 8 is refused, and a refusal changes nothing.
        assert_eq!(
            set_indent_level_at(&mut doc, &body_path(1), 7, 100, 50),
            Ok(true)
        );
        let snapshot = doc.clone();
        let error = set_indent_level_at(&mut doc, &body_path(1), 8, 100, 50).unwrap_err();
        assert!(error.contains("0..=7"), "{error}");
        assert_eq!(doc, snapshot);
    }

    fn ext(code: u16, id: &[u8; 4]) -> HwpChar {
        HwpChar::ExtCtrl {
            code,
            ctrl_id: *id,
            payload: vec![0; 12],
            ctrl_index: None,
        }
    }

    /// A paragraph from `parts` with explicit (WCHAR start, shape) runs.
    fn para_of(parts: Vec<HwpChar>, runs: &[(u32, u16)]) -> Paragraph {
        Paragraph {
            chars: parts,
            char_shape_runs: runs
                .iter()
                .map(|(start, id)| (*start, hwp_model::CharShapeId(*id)))
                .collect(),
            ..Paragraph::default()
        }
    }
    fn text(s: &str) -> Vec<HwpChar> {
        s.chars().map(HwpChar::Text).collect()
    }
    fn run_pairs(p: &Paragraph) -> Vec<(u32, u32)> {
        p.char_shape_runs
            .iter()
            .map(|(s, id)| (*s, u32::from(id.0)))
            .collect()
    }

    #[test]
    fn strip_leading_blanks_rebases_runs_so_the_first_survivor_keeps_its_shape() {
        // "  " (shape 0) + "Bold" (shape 1) + " tail" (shape 2): the blanks end on a run start.
        let mut parts = text("  Bold tail");
        parts.push(HwpChar::CharCtrl(hwp_model::ctrl_char::PARA_BREAK));
        let mut p = para_of(parts, &[(0, 0), (2, 1), (6, 2)]);
        p.line_segs.push(dummy_lineseg());
        assert!(strip_leading_blanks(&mut p));
        assert!(p.line_segs.is_empty(), "the stale line layout is dropped");
        assert_eq!(p.plain_text(), "Bold tail");
        assert_eq!(
            run_pairs(&p),
            [(0, 1), (4, 2)],
            "bold stays on B, the tail stays on the tail"
        );
        // Blanks inside a longer first run: the run keeps its shape and start.
        let mut p = para_of(text("  Item x"), &[(0, 7), (6, 8)]);
        assert!(strip_leading_blanks(&mut p));
        assert_eq!(run_pairs(&p), [(0, 7), (4, 8)]);
    }

    #[test]
    fn strip_leading_blanks_skips_only_invisible_controls() {
        let secd = || vec![ext(2, b"secd"), ext(2, b"cold")];
        // Section and column controls (16 WCHAR) lead: the blanks after them go, the controls stay.
        let mut chars = secd();
        chars.extend(text("  A"));
        let mut p = para_of(chars, &[(0, 0), (16, 1)]);
        assert!(strip_leading_blanks(&mut p));
        assert_eq!(
            p.chars.len(),
            3,
            "controls kept in place, two blanks removed"
        );
        assert!(matches!(p.chars[0], HwpChar::ExtCtrl { .. }));
        assert_eq!(
            run_pairs(&p),
            [(0, 0), (16, 1)],
            "runs start where they did: only blanks left"
        );
        // A field start, a bookmark and a page control are invisible as well.
        for (code, id) in [(3u16, b"%clk"), (22, b"bokm"), (21, b"pgct")] {
            let mut chars = vec![ext(code, id)];
            chars.extend(text("  A"));
            let mut p = para_of(chars, &[(0, 0)]);
            assert!(strip_leading_blanks(&mut p), "code {code}");
            assert_eq!(p.plain_text(), "A", "code {code}");
        }
        // An auto number, a tab and an object print something: the blanks after them are a
        // separator, not indentation, and stay. Blanks BEFORE them still lead the paragraph.
        for (code, id) in [(18u16, b"atno"), (11, b"tbl "), (17, b"fn  ")] {
            let mut chars = vec![ext(code, id)];
            chars.extend(text("  A"));
            let mut p = para_of(chars, &[(0, 0)]);
            assert!(!strip_leading_blanks(&mut p), "code {code}");
            assert_eq!(p.plain_text(), "  A", "code {code}");
            let mut chars = text("  ");
            chars.push(ext(code, id));
            chars.extend(text("A"));
            let mut p = para_of(chars, &[(0, 0)]);
            assert!(strip_leading_blanks(&mut p), "blanks before code {code}");
            assert_eq!(p.plain_text(), "A", "blanks before code {code}");
        }
        let mut chars = vec![HwpChar::InlineCtrl {
            code: hwp_model::ctrl_char::TAB,
            payload: vec![0; 12],
        }];
        chars.extend(text("  A"));
        let mut p = para_of(chars, &[(0, 0)]);
        assert!(!strip_leading_blanks(&mut p), "blanks after a tab stay");
    }

    #[test]
    fn strip_leading_blanks_leaves_a_spacer_only_paragraph_and_wide_blanks_alone() {
        // Nothing but blanks (a spacer line) is not an indent: the text is left as written.
        let mut p = para_of(text("   "), &[(0, 0)]);
        assert!(!strip_leading_blanks(&mut p));
        assert_eq!(p.plain_text(), "   ");
        let mut parts = text("  ");
        parts.push(HwpChar::CharCtrl(hwp_model::ctrl_char::PARA_BREAK));
        let mut p = para_of(parts, &[(0, 0)]);
        assert!(
            !strip_leading_blanks(&mut p),
            "blanks before the paragraph break only"
        );
        // A supplementary-plane character counts two WCHAR and shifts later runs by two.
        let mut chars = text("\u{3000}\u{a0} ");
        chars.push(HwpChar::Text('\u{1F600}'));
        chars.extend(text("x"));
        let mut p = para_of(chars, &[(0, 0), (5, 1)]);
        assert!(strip_leading_blanks(&mut p));
        assert_eq!(p.plain_text(), "\u{1F600}x");
        assert_eq!(
            run_pairs(&p),
            [(0, 0), (2, 1)],
            "the 2-WCHAR emoji stays on its shape and x moves back by the 3 removed WCHAR"
        );
    }

    /// Plans and applies a strip in one step, as `set_indent_level_at` does after its checks.
    fn strip_leading_blanks(para: &mut Paragraph) -> bool {
        let Some(plan) = leading_blank_plan(para) else {
            return false;
        };
        apply_blank_plan(para, &plan);
        true
    }

    #[test]
    fn set_break_before_clears_only_the_caches_the_break_moves() {
        // Five body paragraphs with caches; paragraph 3 already starts a page.
        let mut doc = from_markdown::from_markdown("a\n\nb\n\nc\n\nd\n\ne\n");
        let count = doc.sections[0].paragraphs.len();
        assert!(count >= 5, "fixture has five paragraphs");
        for para in &mut doc.sections[0].paragraphs {
            para.line_segs = vec![dummy_lineseg()];
        }
        doc.sections[0].paragraphs[3].header.break_type |= 0x04;
        assert_eq!(set_break_before_at(&mut doc, &body_path(1), true), Ok(true));
        let kept: Vec<bool> = doc.sections[0]
            .paragraphs
            .iter()
            .map(|p| !p.line_segs.is_empty())
            .collect();
        assert_eq!(
            &kept[..5],
            [true, false, false, true, true],
            "the target and the paragraphs up to the next break are re-measured, the rest keep \
             their caches"
        );
        // Turning a break off re-measures the same span: the paragraph now continues the page.
        for para in &mut doc.sections[0].paragraphs {
            para.line_segs = vec![dummy_lineseg()];
        }
        assert_eq!(
            set_break_before_at(&mut doc, &body_path(1), false),
            Ok(true)
        );
        let kept: Vec<bool> = doc.sections[0]
            .paragraphs
            .iter()
            .map(|p| !p.line_segs.is_empty())
            .collect();
        assert_eq!(&kept[..5], [true, false, false, true, true]);
        // A request already in effect touches no cache at all.
        for para in &mut doc.sections[0].paragraphs {
            para.line_segs = vec![dummy_lineseg()];
        }
        let before = doc.clone();
        assert_eq!(
            set_break_before_at(&mut doc, &body_path(1), false),
            Ok(false)
        );
        assert_eq!(doc, before);
        // The last paragraph has no successor to clear.
        assert_eq!(
            set_break_before_at(&mut doc, &body_path(count - 1), true),
            Ok(true)
        );
        assert!(doc.sections[0].paragraphs[count - 1].line_segs.is_empty());
        assert!(!doc.sections[0].paragraphs[count - 2].line_segs.is_empty());
    }

    fn range_tag() -> hwp_model::OpaqueRecord {
        hwp_model::OpaqueRecord {
            tag: crate::edit::PARA_RANGE_TAG,
            data: vec![0, 0, 0, 0, 5, 0, 0, 0, 1, 0, 0, 0],
            children: Vec::new(),
        }
    }

    /// A one-section document whose paragraph 1 is `parts`, with a line cache and one extra.
    fn doc_with_paragraph(parts: Vec<HwpChar>, extras: Vec<hwp_model::OpaqueRecord>) -> Document {
        let mut doc = from_markdown::from_markdown("하나\n\n둘\n\n셋\n");
        let para = &mut doc.sections[0].paragraphs[1];
        para.chars = parts;
        para.char_shape_runs = vec![(0, hwp_model::CharShapeId(0))];
        para.line_segs = vec![dummy_lineseg()];
        para.extras = extras;
        doc
    }

    #[test]
    fn set_indent_level_refuses_to_strip_blanks_under_a_range_tag_and_changes_nothing() {
        // T12-F3: PARA_RANGE_TAG records keep WCHAR offsets this edit would not move.
        let mut doc = doc_with_paragraph(text("  Marked"), vec![range_tag()]);
        let before = doc.clone();
        let error = set_indent_level_at(&mut doc, &body_path(1), 2, 100, 50).unwrap_err();
        assert!(
            error.contains("구간 표시") && error.contains("set_para"),
            "the refusal names the cause and the remedy: {error}"
        );
        assert_eq!(doc, before, "a refusal leaves the document unchanged");
        // Only a range tag blocks the strip. Another opaque record does not.
        let other = hwp_model::OpaqueRecord {
            tag: 0x10 + 99,
            ..range_tag()
        };
        let mut doc = doc_with_paragraph(text("  Plain"), vec![other]);
        assert_eq!(
            set_indent_level_at(&mut doc, &body_path(1), 1, 100, 0),
            Ok(true)
        );
        assert_eq!(doc.sections[0].paragraphs[1].plain_text(), "Plain");
        // A range tag is harmless when no blank would move: the shape is still applied.
        let mut doc = doc_with_paragraph(text("Marked"), vec![range_tag()]);
        assert_eq!(
            set_indent_level_at(&mut doc, &body_path(1), 1, 100, 0),
            Ok(true)
        );
        assert_eq!(
            doc.sections[0].paragraphs[1].extras,
            vec![range_tag()],
            "the tag is carried through untouched"
        );
    }

    #[test]
    fn set_indent_level_that_changes_nothing_keeps_the_native_line_cache() {
        // T12-F4: an already-applied request must not drop the paragraph's line layout.
        let mut doc = doc_with_paragraph(text("Item"), Vec::new());
        assert_eq!(
            set_indent_level_at(&mut doc, &body_path(1), 2, 100, 40),
            Ok(true)
        );
        assert!(
            doc.sections[0].paragraphs[1].line_segs.is_empty(),
            "a real change drops the stale layout"
        );
        doc.sections[0].paragraphs[1].line_segs = vec![dummy_lineseg()];
        let shapes = doc.header.para_shapes.len();
        let before = doc.clone();
        assert_eq!(
            set_indent_level_at(&mut doc, &body_path(1), 2, 100, 40),
            Ok(false)
        );
        assert_eq!(
            doc, before,
            "the repeat leaves the whole document as it was"
        );
        assert_eq!(doc.header.para_shapes.len(), shapes, "no shape is appended");
        // Level 0 with no hanging on a paragraph that already has that shape is a no-op too.
        let mut doc = doc_with_paragraph(text("Plain"), Vec::new());
        let before = doc.clone();
        assert_eq!(
            set_indent_level_at(&mut doc, &body_path(1), 0, 100, 0),
            Ok(false)
        );
        assert_eq!(
            doc, before,
            "level 0 on an untouched paragraph changes nothing"
        );
    }

    fn dummy_lineseg() -> hwp_model::LineSeg {
        hwp_model::LineSeg {
            text_start: 0,
            v_pos: 0,
            line_height: 1000,
            text_height: 1000,
            baseline_gap: 850,
            line_spacing: 600,
            col_start: 0,
            seg_width: 40000,
            flags: 0,
        }
    }

    #[test]
    fn apply_format_비트() {
        let cs = apply_format(
            CharShape::default(),
            &CharFormat {
                bold: Some(true),
                italic: Some(true),
                underline: Some(true),
                strike: Some(true),
                size_pt: Some(16.0),
                color: Some(0x0000_00FF),
                font: None,
            },
            None,
        );
        assert!(cs.is_bold() && cs.is_italic() && cs.has_underline() && cs.has_strike());
        assert_eq!(cs.base_size, 1600);
        assert_eq!(cs.text_color, 0x0000_00FF);
        // Turn off bold only.
        let off = apply_format(
            cs.clone(),
            &CharFormat {
                bold: Some(false),
                ..Default::default()
            },
            None,
        );
        assert!(
            !off.is_bold() && off.is_italic(),
            "bold만 꺼지고 나머지 유지"
        );
    }

    #[test]
    fn find_or_insert_중복_재사용() {
        let mut shapes = vec![CharShape::default()];
        let a = CharShape {
            base_size: 1600,
            ..CharShape::default()
        };
        let id1 = find_or_insert(&mut shapes, a.clone());
        assert_eq!(id1, CharShapeId(1));
        let id2 = find_or_insert(&mut shapes, a); // Equal shape: reuse.
        assert_eq!(id2, CharShapeId(1));
        assert_eq!(shapes.len(), 2);
    }

    #[test]
    fn find_or_insert_face_반복은_재사용() {
        let mut fonts = vec![FaceName::default()];
        let id1 = find_or_insert_face(&mut fonts, "맑은 고딕");
        assert_eq!(fonts.len(), 2, "새 이름은 1개만 추가");
        let id2 = find_or_insert_face(&mut fonts, "맑은 고딕");
        assert_eq!(id1, id2, "같은 이름은 같은 id로 재사용");
        assert_eq!(fonts.len(), 2, "반복 설정은 테이블을 키우지 않음");
    }

    /// Setting a font with set_char_format must point matched runs' CharShape.face_ids
    /// to that font in all seven language slots (A2).
    #[test]
    fn set_char_format_글꼴_7슬롯_균일_적용() {
        let mut doc = from_markdown::from_markdown("글꼴 테스트 문단입니다.");
        let n = set_char_format(
            &mut doc,
            "테스트",
            &CharFormat {
                font: Some("맑은 고딕".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(n, 1);
        let para = &doc.sections[0].paragraphs[0];
        let styled = para
            .char_shape_runs
            .iter()
            .find_map(|(_, id)| {
                let cs = &doc.header.char_shapes[id.0 as usize];
                (cs.face_ids[0] != 0).then_some(cs)
            })
            .expect("글꼴이 적용된 run이 있어야 함");
        for slot in 0..hwp_model::LANG_COUNT {
            let face = &doc.header.fonts[slot][styled.face_ids[slot] as usize];
            assert_eq!(
                face.name, "맑은 고딕",
                "슬롯 {slot}이 균일하게 설정되어야 함"
            );
        }
    }

    #[test]
    fn restyle_range_부분범위() {
        // Apply bold to [3,6) in [(0,id0)]: [(0,0),(3,new),(6,0)].
        let mut shapes = vec![CharShape::default()];
        let mut runs = vec![(0u32, CharShapeId(0))];
        restyle_range(
            &mut runs,
            3,
            6,
            &mut shapes,
            &CharFormat {
                bold: Some(true),
                ..Default::default()
            },
            None,
            12, // para_len
        );
        assert_eq!(runs.len(), 3, "경계 분할: {runs:?}");
        assert_eq!(runs[0], (0, CharShapeId(0)));
        assert_eq!(runs[1].0, 3);
        assert_eq!(runs[2], (6, CharShapeId(0)), "범위 뒤 원래 모양 복원");
        assert!(shapes[runs[1].1.0 as usize].is_bold());
        // The first run always starts at zero.
        assert_eq!(runs[0].0, 0);
    }

    /// `restyle_range_at`'s apply-time guard (T-07-26): a range resolved against the paragraph
    /// BEFORE it was shortened must error, not corrupt `char_shape_runs`. Asserting only on the
    /// error would pass against an implementation that errors after mutating — so this asserts
    /// `char_shape_runs` is byte-identical to what it was before the call too.
    #[test]
    fn restyle_range_at_주소_적용시점_경계_재검사() {
        let mut doc = from_markdown("첫 문단\n\n둘째 문단입니다길게씀\n");
        let path = crate::segment_id::SegmentPath {
            section: 0,
            indices: vec![1],
        };
        let original_len = doc.sections[0].paragraphs[1].wchar_len();
        // The range fit the original length; simulate a subsequently shortened paragraph.
        let w_end = original_len;
        doc.sections[0].paragraphs[1].chars.truncate(3);
        let before_runs = doc.sections[0].paragraphs[1].char_shape_runs.clone();

        let result = restyle_range_at(
            &mut doc,
            &path,
            0,
            w_end,
            &CharFormat {
                bold: Some(true),
                ..Default::default()
            },
        );
        assert!(result.is_err(), "짧아진 문단을 벗어난 범위는 오류여야 함");
        assert_eq!(
            doc.sections[0].paragraphs[1].char_shape_runs, before_runs,
            "오류 후에도 char_shape_runs는 변경되지 않아야 함"
        );
    }

    /// `apply_para_props_at` changes only the addressed paragraph's `ParaShapeId`; the sibling
    /// (identical-text) paragraph at a different path is untouched (07-02 Task 1).
    #[test]
    fn apply_para_props_at_주소_문단만_변경() {
        let mut doc = from_markdown("같은 문단\n\n같은 문단\n");
        let before_first = doc.sections[0].paragraphs[0].para_shape;
        let path = SegmentPath {
            section: 0,
            indices: vec![1],
        };
        let props = ParaProps {
            align: Some(3), // Center.
            ..ParaProps::default()
        };
        assert!(apply_para_props_at(&mut doc, &path, &props));
        assert_eq!(
            doc.sections[0].paragraphs[0].para_shape, before_first,
            "sibling paragraph's ParaShapeId must be unchanged"
        );
        let ps = &doc.header.para_shapes[doc.sections[0].paragraphs[1].para_shape.0 as usize];
        assert_eq!(
            ps.alignment(),
            3,
            "addressed paragraph must carry the new align"
        );
    }

    /// A repeated identical `apply_para_props_at` does not grow `header.para_shapes` — it reuses
    /// the same shape via `find_or_insert_para`, matching the pattern path's dedup guarantee.
    #[test]
    fn apply_para_props_at_중복_props는_shape_추가없음() {
        let mut doc = from_markdown("문단\n");
        let path = SegmentPath {
            section: 0,
            indices: vec![0],
        };
        let props = ParaProps {
            align: Some(3),
            ..ParaProps::default()
        };
        assert!(apply_para_props_at(&mut doc, &path, &props));
        let after_first = doc.header.para_shapes.len();
        assert!(apply_para_props_at(&mut doc, &path, &props));
        assert_eq!(
            doc.header.para_shapes.len(),
            after_first,
            "identical props must reuse the existing shape, not append a new one"
        );
    }

    /// `set_para_align_at` sets alignment on the resolved paragraph only; the sibling paragraph
    /// keeps its original alignment.
    #[test]
    fn set_para_align_at_주소_문단만_변경() {
        let mut doc = from_markdown("같은 문단\n\n같은 문단\n");
        let before_first = doc.sections[0].paragraphs[0].para_shape;
        let path = SegmentPath {
            section: 0,
            indices: vec![1],
        };
        assert!(set_para_align_at(&mut doc, &path, 3));
        assert_eq!(
            doc.sections[0].paragraphs[0].para_shape, before_first,
            "sibling paragraph's ParaShapeId must be unchanged"
        );
        let ps = &doc.header.para_shapes[doc.sections[0].paragraphs[1].para_shape.0 as usize];
        assert_eq!(ps.alignment(), 3);
    }

    /// `shift_head_level_at(+1)` must raise a numbered list item's `head_level` from 1 to 2.
    /// Fails while `ParaShape::set_head_level` is still the Task-2 RED no-op stub.
    #[test]
    fn shift_head_level_at_들여쓰기로_수준_증가() {
        let mut doc = from_markdown("1. 첫 항목\n");
        let path = SegmentPath {
            section: 0,
            indices: vec![0],
        };
        let ps_before =
            doc.header.para_shapes[doc.sections[0].paragraphs[0].para_shape.0 as usize].clone();
        assert_eq!(ps_before.head_type(), 2, "번호 매기기 목록이어야 함");
        assert_eq!(ps_before.head_level(), 1, "최상위 항목은 수준 1에서 시작");

        let new_level = shift_head_level_at(&mut doc, &path, 1).expect("들여쓰기 성공해야 함");
        assert_eq!(new_level, 2);
        let ps_after = &doc.header.para_shapes[doc.sections[0].paragraphs[0].para_shape.0 as usize];
        assert_eq!(ps_after.head_level(), 2);
    }

    /// Indent at the top of the representable range (7) and outdent at the bottom (1) both fail
    /// with a named error rather than clamping (A3); a failed op leaves the level unchanged.
    #[test]
    fn shift_head_level_at_경계에서_실패() {
        let mut doc = from_markdown("1. 항목\n");
        let path = SegmentPath {
            section: 0,
            indices: vec![0],
        };
        for _ in 0..6 {
            shift_head_level_at(&mut doc, &path, 1).unwrap();
        }
        let ps = &doc.header.para_shapes[doc.sections[0].paragraphs[0].para_shape.0 as usize];
        assert_eq!(ps.head_level(), 7);
        let err = shift_head_level_at(&mut doc, &path, 1).unwrap_err();
        assert!(
            err.contains("범위"),
            "경계 오류는 범위를 명시해야 함: {err}"
        );
        let ps_after = &doc.header.para_shapes[doc.sections[0].paragraphs[0].para_shape.0 as usize];
        assert_eq!(
            ps_after.head_level(),
            7,
            "실패한 들여쓰기는 수준을 바꾸면 안 됨"
        );

        let mut doc2 = from_markdown("1. 항목\n");
        let err2 = shift_head_level_at(&mut doc2, &path, -1).unwrap_err();
        assert!(
            err2.contains("범위"),
            "경계 오류는 범위를 명시해야 함: {err2}"
        );
    }

    /// A paragraph whose `head_type` is neither numbered nor bullet is rejected with its own
    /// message naming that it is not a list item — never inventing one.
    #[test]
    fn shift_head_level_at_비목록_문단_거부() {
        let mut doc = from_markdown("일반 문단\n");
        let path = SegmentPath {
            section: 0,
            indices: vec![0],
        };
        let err = shift_head_level_at(&mut doc, &path, 1).unwrap_err();
        assert!(
            err.contains("목록 항목"),
            "목록 항목이 아니라는 메시지여야 함: {err}"
        );
    }

    #[test]
    fn set_char_format_매칭_적용() {
        let mut doc = from_markdown("형식 테스트 문단입니다.");
        let para = &mut doc.sections[0].paragraphs[0];
        para.line_segs.push(dummy_lineseg());
        let n = set_char_format(
            &mut doc,
            "테스트",
            &CharFormat {
                bold: Some(true),
                color: Some(0x0000_00FF),
                ..Default::default()
            },
        );
        assert_eq!(n, 1);
        let para = &doc.sections[0].paragraphs[0];
        assert!(para.line_segs.is_empty(), "편집 문단 줄배치 비움");
        // A new run must apply the bold/color shape to the matching range.
        // The first paragraph starts with 16 WCHAR of secd/cold controls, so positions vary.
        let styled = para.char_shape_runs.iter().any(|(_, id)| {
            let cs = &doc.header.char_shapes[id.0 as usize];
            cs.is_bold() && cs.text_color == 0x0000_00FF
        });
        assert!(styled, "굵게+색 run 존재: {:?}", para.char_shape_runs);
        // Restore original shape zero after the range.
        assert!(
            para.char_shape_runs
                .iter()
                .any(|(_, id)| *id == CharShapeId(0)),
            "복원 run 존재"
        );
    }

    #[test]
    fn set_para_align_정렬() {
        let mut doc = from_markdown("가운데 정렬할 문단.");
        let para = &mut doc.sections[0].paragraphs[0];
        para.line_segs.push(dummy_lineseg());
        let before = doc.header.para_shapes.len();
        let n = set_para_align(&mut doc, "가운데", 3); // 3 = center.
        assert_eq!(n, 1);
        let para = &doc.sections[0].paragraphs[0];
        assert!(para.line_segs.is_empty());
        let ps = &doc.header.para_shapes[para.para_shape.0 as usize];
        assert_eq!(ps.alignment(), 3, "가운데 정렬");
        assert!(doc.header.para_shapes.len() >= before);
    }

    #[test]
    fn 매칭_없으면_무변경() {
        let mut doc = from_markdown("본문 문단입니다.");
        let before = doc.header.char_shapes.len();
        let n = set_char_format(
            &mut doc,
            "없는단어",
            &CharFormat {
                bold: Some(true),
                ..Default::default()
            },
        );
        assert_eq!(n, 0);
        assert_eq!(doc.header.char_shapes.len(), before, "shape 추가 없음");
    }
    // ── #221 --set-cell-para (CELL-02) ──────────────────────────────────────

    fn table_doc() -> Document {
        from_markdown("본문\n\n| 가 | 나 |\n|----|----|\n| 1 | 2 |\n")
    }

    fn cell_paras(doc: &Document, row: u16, col: u16) -> &[Paragraph] {
        doc.sections
            .iter()
            .flat_map(|s| &s.paragraphs)
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("표 없음")
            .cells
            .iter()
            .find(|c| c.row == row && c.col == col)
            .expect("셀 없음")
            .paragraphs
            .as_slice()
    }

    #[test]
    fn set_cell_para_props_셀_전체_문단에_적용() {
        let mut doc = table_doc();
        crate::edit::set_cell(&mut doc, 0, 1, 0, "첫째\n\n둘째\n\n셋째").unwrap();
        let props = ParaProps {
            line_spacing: Some((0, 150)),
            indent: Some(-1000),
            ..ParaProps::default()
        };
        assert_eq!(set_cell_para_props(&mut doc, 0, 1, 0, &props).unwrap(), 3);
        let paras = cell_paras(&doc, 1, 0);
        assert_eq!(paras.len(), 3);
        for p in paras {
            let ps = &doc.header.para_shapes[p.para_shape.0 as usize];
            assert_eq!(ps.line_spacing, 150);
            assert_eq!(ps.line_spacing_type, 0);
            assert_eq!(ps.indent, -2000, "치수는 IR 2배 단위");
            assert!(p.line_segs.is_empty(), "줄 배치 무효화");
        }
    }

    #[test]
    fn set_cell_para_props_는_set_para_props와_같은_단위환산() {
        // Equal keys/values store equal shapes through every flag, sharing to_ir_units.
        let props = ParaProps {
            line_spacing: Some((1, 1300)),
            indent: Some(-1000),
            margin_left: Some(2000),
            spacing_bottom: Some(300),
            ..ParaProps::default()
        };
        let mut by_anchor = table_doc();
        assert_eq!(set_para_props(&mut by_anchor, "1", &props), 1);
        let a = doc_shape_of_cell(&by_anchor, 1, 0);

        let mut by_cell = table_doc();
        assert_eq!(
            set_cell_para_props(&mut by_cell, 0, 1, 0, &props).unwrap(),
            1
        );
        let b = doc_shape_of_cell(&by_cell, 1, 0);
        assert_eq!(a, b, "두 진입점의 저장 모양이 달라졌다");
    }

    fn doc_shape_of_cell(doc: &Document, row: u16, col: u16) -> ParaShape {
        let p = &cell_paras(doc, row, col)[0];
        doc.header.para_shapes[p.para_shape.0 as usize].clone()
    }

    #[test]
    fn set_cell_para_props_정렬_다섯_이름() {
        // justify=0, left=1, right=2, center=3, distribute=4: clear then set attr1 bits 2-4.
        for align in [0u8, 1, 2, 3, 4] {
            let mut doc = table_doc();
            let props = ParaProps {
                align: Some(align),
                ..ParaProps::default()
            };
            assert_eq!(set_cell_para_props(&mut doc, 0, 1, 0, &props).unwrap(), 1);
            let ps = doc_shape_of_cell(&doc, 1, 0);
            assert_eq!(ps.alignment(), align, "정렬 코드 {align}");
        }
    }

    #[test]
    fn set_cell_para_props_빈_속성은_무변경() {
        let mut doc = table_doc();
        let before = doc.header.para_shapes.clone();
        let n = set_cell_para_props(&mut doc, 0, 1, 0, &ParaProps::default()).unwrap();
        assert_eq!(n, 0, "빈 속성은 0 보고");
        assert_eq!(doc.header.para_shapes, before, "모양 추가 없음");
    }

    #[test]
    fn set_cell_para_props_없는_주소는_오류() {
        let mut doc = table_doc();
        let props = ParaProps {
            align: Some(3),
            ..ParaProps::default()
        };
        assert!(
            set_cell_para_props(&mut doc, 0, 99, 99, &props).is_err(),
            "없는 셀"
        );
        assert!(
            set_cell_para_props(&mut doc, 9, 0, 0, &props).is_err(),
            "없는 표"
        );
    }
}
