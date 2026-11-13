//! In-memory IR editing primitives.
//!
//! Read an original document, edit text/table cells in memory, then rewrite it. Preserve all
//! unedited data, including images and opaque records, unlike a JSON round-trip.
//!
//! Edited paragraph caches (PARA_LINE_SEG, nchars and ending 0x0d) are stale. Write through the
//! synthesized writer path (HWP5 WriteOptions.edited=true) for Hangul acceptance. This module
//! changes only the IR; the writer restores invariants.

use hwp_model::{CharShapeId, Control, Document, HwpChar, Paragraph};

use crate::segment_id::SegmentPath;

/// A writable form-cell coordinate discovered without changing the document.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FormCellCandidate {
    pub table: usize,
    pub row: u16,
    pub col: u16,
}

/// Normalise a form label for the deliberately narrow label-fill contract.
///
/// Only surrounding Unicode whitespace and terminal ASCII/full-width colons are
/// removed. In particular, this does not case-fold or Unicode-normalise input.
pub fn normalize_form_label(label: &str) -> String {
    label
        .trim()
        .trim_end_matches([':', '：'])
        .trim_end()
        .to_string()
}

/// Find writable form cells for a matching label.
///
/// Table indices use the same recursive document order as [`set_cell`]. A
/// label may address the immediately adjacent cell, or the first data cell
/// directly below a label in a complete table header row. An empty or matching
/// `{{label}}` cell is an explicit form-value marker, so it keeps adjacent-form
/// precedence even when a later row makes the first row look like a complete
/// header.
pub fn find_form_cells_by_label(
    doc: &mut Document,
    label: &str,
    table: Option<usize>,
) -> Vec<FormCellCandidate> {
    let wanted = normalize_form_label(label);
    if wanted.is_empty() {
        return Vec::new();
    }

    let mut candidates: Vec<FormCellCandidate> =
        walk_form_cells(doc, table, |visible, adjacent, below| {
            if normalize_form_label(visible) != wanted {
                return None;
            }
            // A matching placeholder is part of the documented form-value
            // layout, not a second column heading. Prefer it before the
            // complete-header rule; this preserves the legacy adjacent form
            // contract without treating a multi-column header as a form.
            let target = match (adjacent, below) {
                (Some(adjacent), Some(_)) if is_explicit_form_value(adjacent, &wanted) => {
                    FormTarget::Adjacent
                }
                (_, Some(_)) => FormTarget::Below,
                (Some(_), None) => FormTarget::Adjacent,
                (None, None) => return None,
            };
            Some(((), target))
        })
        .into_iter()
        .map(|(_, candidate)| candidate)
        .collect();
    candidates.sort_unstable();
    candidates.dedup();
    candidates
}

/// The neighbour of a label cell that a form policy writes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FormTarget {
    /// The cell to the right, in the same row.
    Adjacent,
    /// In a complete header row, the first data cell below.
    Below,
}

/// The table walk every label finder shares ([`find_form_cells_by_label`] and
/// the form fill). For every table in recursive document order, `pick` sees
/// each cell's visible text, its span-aware right neighbour and, when row 0 is
/// a complete header, the first data cell below it, and names the neighbour to
/// write together with a tag of its own. Readonly: discovery cannot clear opaque
/// HWPX XML or otherwise mutate the document.
pub(crate) fn walk_form_cells<T>(
    doc: &mut Document,
    table: Option<usize>,
    mut pick: impl FnMut(
        &str,
        Option<&hwp_model::Cell>,
        Option<&hwp_model::Cell>,
    ) -> Option<(T, FormTarget)>,
) -> Vec<(T, FormCellCandidate)> {
    let mut out = Vec::new();
    let mut table_index = 0usize;
    loop {
        let found = with_nth_table_readonly(doc, table_index, |current| {
            if table.is_some_and(|scope| scope != table_index) {
                return Vec::new();
            }
            let header_is_complete = current
                .cells
                .iter()
                .filter(|cell| cell.row == 0)
                .all(|cell| !normalize_form_label(&cell_text(cell)).is_empty());
            let mut matches = Vec::new();
            for label_cell in &current.cells {
                let right_col = label_cell.col.saturating_add(label_cell.col_span);
                let adjacent = current
                    .cells
                    .iter()
                    .find(|cell| cell.row == label_cell.row && cell.col == right_col);
                let below = if header_is_complete && label_cell.row == 0 {
                    let first_data_row = label_cell.row.saturating_add(label_cell.row_span);
                    current
                        .cells
                        .iter()
                        .find(|cell| cell.row == first_data_row && cell.col == label_cell.col)
                } else {
                    None
                };
                let Some((tag, target)) = pick(&cell_text(label_cell), adjacent, below) else {
                    continue;
                };
                let target = match target {
                    FormTarget::Adjacent => adjacent,
                    FormTarget::Below => below,
                };
                if let Some(target) = target {
                    matches.push((
                        tag,
                        FormCellCandidate {
                            table: table_index,
                            row: target.row,
                            col: target.col,
                        },
                    ));
                }
            }
            matches
        });
        let Some(found) = found else {
            break;
        };
        out.extend(found);
        table_index += 1;
    }
    out
}

/// A cell's visible text: its paragraphs' plain text joined by `\n`.
pub(crate) fn cell_text(cell: &hwp_model::Cell) -> String {
    cell.paragraphs
        .iter()
        .map(Paragraph::plain_text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_explicit_form_value(cell: &hwp_model::Cell, wanted: &str) -> bool {
    let normalized = normalize_form_label(&cell_text(cell));
    let tokens = hwp_model::slot_tokens(&normalized);
    normalized.is_empty()
        || matches!(tokens.as_slice(), [token]
            if token.range == (0..normalized.len()) && token.name == wanted)
}

/// Opaque HWP5 range marks (format 5.0 section 4.3.5); no saved layout is available locally.
pub(crate) const PARA_RANGE_TAG: u16 = 0x10 + 54;
pub const RANGE_TAG_SHIFT_REFUSAL: &str = "미해석 PARA_RANGE_TAG 구간 표시가 있어 WCHAR 위치가 바뀌거나 표시를 잃는 문단 재작성을 지원하지 않습니다. 같은 WCHAR 길이의 텍스트 치환을 사용하거나 한글에서 구간 표시를 제거한 파일로 다시 실행하세요.";

pub(crate) fn paragraph_has_range_tags(para: &Paragraph) -> bool {
    para.extras
        .iter()
        .any(|record| record.tag == PARA_RANGE_TAG)
}

pub(crate) fn paragraph_tree_has_range_tags(para: &Paragraph) -> bool {
    fn raw_has_tags(records: &[hwp_model::OpaqueRecord]) -> bool {
        records
            .iter()
            .any(|record| record.tag == PARA_RANGE_TAG || raw_has_tags(&record.children))
    }
    if raw_has_tags(&para.extras) {
        return true;
    }
    para.controls.iter().any(|control| {
        let caption = match control {
            Control::Table(table) => {
                if table
                    .cells
                    .iter()
                    .any(|cell| cell.paragraphs.iter().any(paragraph_tree_has_range_tags))
                {
                    return true;
                }
                &table.caption
            }
            Control::Picture(picture) => &picture.caption,
            Control::Generic(generic) => {
                if raw_has_tags(&generic.raw_children)
                    || generic
                        .paragraph_lists
                        .iter()
                        .any(|list| list.paragraphs.iter().any(paragraph_tree_has_range_tags))
                {
                    return true;
                }
                &generic.caption
            }
            Control::SectionDef(_) => return false,
        };
        caption
            .as_ref()
            .is_some_and(|caption| caption.paragraphs.iter().any(paragraph_tree_has_range_tags))
    })
}

pub(crate) fn document_has_range_tags(doc: &Document) -> bool {
    doc.sections
        .iter()
        .any(|section| section.paragraphs.iter().any(paragraph_tree_has_range_tags))
}

pub(crate) fn ensure_range_tag_shift_allowed(
    para: &Paragraph,
    from_w: u32,
    to_w: u32,
) -> Result<(), String> {
    if from_w != to_w && paragraph_has_range_tags(para) {
        return Err(RANGE_TAG_SHIFT_REFUSAL.to_string());
    }
    Ok(())
}

/// Matches the existing first-anchor insertion walk: own text, cells and Generic lists.
/// Captions are not targets of these anchor-based mutators.
pub(crate) fn ensure_anchor_shift_allowed(
    doc: &Document,
    anchor: &str,
    inserted_w: u32,
) -> Result<(), String> {
    fn first<'a>(para: &'a Paragraph, anchor: &str) -> Option<&'a Paragraph> {
        if find_match(&para.chars, anchor, 0).is_some() {
            return Some(para);
        }
        for control in &para.controls {
            match control {
                Control::Table(table) => {
                    for cell in &table.cells {
                        for paragraph in &cell.paragraphs {
                            if let Some(found) = first(paragraph, anchor) {
                                return Some(found);
                            }
                        }
                    }
                }
                Control::Generic(generic) => {
                    for list in &generic.paragraph_lists {
                        for paragraph in &list.paragraphs {
                            if let Some(found) = first(paragraph, anchor) {
                                return Some(found);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }
    for section in &doc.sections {
        for para in &section.paragraphs {
            if let Some(found) = first(para, anchor) {
                return ensure_range_tag_shift_allowed(found, 0, inserted_w);
            }
        }
    }
    Ok(())
}

fn replacement_chars(to: &str) -> Vec<HwpChar> {
    to.replace("\r\n", "\n")
        .chars()
        .filter_map(|c| match c {
            '\n' => Some(HwpChar::CharCtrl(hwp_model::ctrl_char::LINE_BREAK)),
            '\t' => Some(HwpChar::InlineCtrl {
                code: hwp_model::ctrl_char::TAB,
                payload: vec![0; 12],
            }),
            c if (c as u32) < 0x20 => None,
            c => Some(HwpChar::Text(c)),
        })
        .collect()
}

/// Validates every original range before any back-to-front splice, including net-zero batches.
pub(crate) fn check_splice_edits(
    para: &Paragraph,
    edits: &[(std::ops::Range<usize>, String)],
) -> Result<(), String> {
    if paragraph_has_range_tags(para) {
        for (range, replacement) in edits {
            let from_w = para.chars[range.clone()]
                .iter()
                .map(HwpChar::wchar_width)
                .sum();
            let to_w = replacement_chars(replacement)
                .iter()
                .map(HwpChar::wchar_width)
                .sum();
            ensure_range_tag_shift_allowed(para, from_w, to_w)?;
        }
    }
    Ok(())
}

/// Replaces matches in contiguous Text spans across body, cells and text boxes.
/// With `all=false`, only the first match is changed. A width-changing tagged target
/// refuses before any target is mutated; the successful result is the replacement count.
pub fn replace_text(doc: &mut Document, from: &str, to: &str, all: bool) -> Result<usize, String> {
    if from.is_empty() {
        return Ok(0);
    }
    if document_has_range_tags(doc) {
        replace_text_pass(doc, from, to, all, false)?;
    }
    replace_text_pass(doc, from, to, all, true)
}

fn replace_text_pass(
    doc: &mut Document,
    from: &str,
    to: &str,
    all: bool,
    apply: bool,
) -> Result<usize, String> {
    let mut budget = if all { usize::MAX } else { 1 };
    let mut count = 0;
    for section in &mut doc.sections {
        for para in &mut section.paragraphs {
            count += replace_in_para(para, from, to, &mut budget, apply)?;
            if budget == 0 {
                return Ok(count);
            }
        }
    }
    Ok(count)
}

fn replace_in_para(
    para: &mut Paragraph,
    from: &str,
    to: &str,
    budget: &mut usize,
    apply: bool,
) -> Result<usize, String> {
    let mut n = replace_in_chars(para, from, to, budget, apply)?;
    for ctrl in &mut para.controls {
        if *budget == 0 {
            break;
        }
        match ctrl {
            Control::Table(table) => {
                for cell in &mut table.cells {
                    for paragraph in &mut cell.paragraphs {
                        if *budget == 0 {
                            break;
                        }
                        n += replace_in_para(paragraph, from, to, budget, apply)?;
                    }
                }
            }
            Control::Generic(generic) => {
                let before = n;
                for list in &mut generic.paragraph_lists {
                    for paragraph in &mut list.paragraphs {
                        if *budget == 0 {
                            break;
                        }
                        n += replace_in_para(paragraph, from, to, budget, apply)?;
                    }
                }
                if apply && n > before {
                    generic.hwpx_raw_xml = None;
                }
            }
            _ => {}
        }
    }
    Ok(n)
}

fn replace_in_chars(
    para: &mut Paragraph,
    from: &str,
    to: &str,
    budget: &mut usize,
    apply: bool,
) -> Result<usize, String> {
    let from_w = utf16_len(from);
    let from_chars = from.chars().count();
    let to_w = replacement_chars(to).iter().map(HwpChar::wchar_width).sum();
    let mut count = 0;
    let mut start = 0usize;
    while *budget > 0 {
        let Some((char_idx, wpos)) = find_match(&para.chars, from, start) else {
            break;
        };
        let inserted = if apply {
            splice_text(para, char_idx..char_idx + from_chars, wpos, from_w, to)?
        } else {
            ensure_range_tag_shift_allowed(para, from_w, to_w)?;
            from_chars
        };
        count += 1;
        *budget -= 1;
        start = char_idx + inserted;
    }
    Ok(count)
}

/// Replaces one text span after validating its normalized WCHAR width. Range records stay opaque.
/// CRLF/LF become line breaks, tabs become 8-WCHAR inline controls and other C0 chars are dropped.
/// Later shape-run boundaries shift by the width delta; stale line layout is cleared.
pub(crate) fn splice_text(
    para: &mut Paragraph,
    range: std::ops::Range<usize>,
    wpos: u32,
    from_w: u32,
    to: &str,
) -> Result<usize, String> {
    let to_hwp = replacement_chars(to);
    let to_w = to_hwp.iter().map(HwpChar::wchar_width).sum();
    ensure_range_tag_shift_allowed(para, from_w, to_w)?;
    let inserted = to_hwp.len();
    if !paragraph_has_range_tags(para) || para.chars[range.clone()] != to_hwp {
        para.chars.splice(range, to_hwp);
        adjust_runs(&mut para.char_shape_runs, wpos, from_w, to_w);
        para.line_segs.clear();
    }
    Ok(inserted)
}

/// Applies disjoint original spans back to front, after validating the whole paragraph batch.
pub(crate) fn splice_edits(
    para: &mut Paragraph,
    edits: Vec<(std::ops::Range<usize>, String)>,
) -> Result<(), String> {
    check_splice_edits(para, &edits)?;
    for (range, replacement) in edits.into_iter().rev() {
        let wpos = para.chars[..range.start]
            .iter()
            .map(HwpChar::wchar_width)
            .sum();
        let from_w = para.chars[range.clone()]
            .iter()
            .map(HwpChar::wchar_width)
            .sum();
        splice_text(para, range, wpos, from_w, &replacement)?;
    }
    Ok(())
}

/// Addressed replacement touches one writable paragraph and preserves ancestors on refusal.
pub fn replace_text_at(
    doc: &mut Document,
    path: &SegmentPath,
    from: &str,
    to: &str,
) -> Result<usize, String> {
    if from.is_empty() {
        return Ok(0);
    }
    let Some(para) = crate::address::paragraph_at_mut(doc, path) else {
        return Ok(0);
    };
    let mut budget = usize::MAX;
    replace_in_chars(para, from, to, &mut budget, false)?;
    budget = usize::MAX;
    let count = replace_in_chars(para, from, to, &mut budget, true)?;
    if count > 0 {
        crate::address::invalidate_ancestors(doc, path);
    }
    Ok(count)
}

/// Replaces exactly the text one addressed run range covers, provided it equals `from`.
///
/// Unlike [`replace_text_at`], which rewrites every left-to-right match in a paragraph, this
/// touches one span: the address is resolved at [`crate::address::Granularity::Run`], so `chars`
/// narrows it and no `chars` means the run's whole text (D-01/A5). `from` is not a selector here
/// but a drift check (issue #525): a client that selected one run cannot silently rewrite a
/// different one after the document moved.
///
/// Returns `Ok(1)` when the span was rewritten. Returns `Ok(0)` when the span is text but is not
/// exactly `from` — the drift check failed, nothing changed, and the caller reports the request
/// as unapplied (the CLI then fails closed unless `--allow-partial`). Returns `Err` when the
/// address does not name a text span at all: unresolvable, a `chars` bound inside a character, or
/// an interior control (no text form to match). Every message names the address and no document
/// text.
///
/// Trailing control characters are outside the addressed text: an HWP5 paragraph stores its break
/// as its last character, so the last run's published bound includes it (#517).
///
/// This resolves `addr` against `doc` **as it is now** and hands the resolved window to
/// [`replace_text_in_window`]. A caller that already resolved the address earlier — the CLI batch
/// apply loop, whose preflight resolution against the loaded document is authoritative (D-06) —
/// must call that function with its own path instead of resolving a second time here: resolving
/// again would read the raw address against whatever the batch has already written.
pub fn replace_text_in_range(
    doc: &mut Document,
    addr: &crate::address::Address,
    from: &str,
    to: &str,
) -> Result<usize, String> {
    use crate::address::{Granularity, TargetKind};

    if from.is_empty() {
        return Ok(0);
    }
    let label = addr;
    let target = crate::address::resolve(doc, addr, Granularity::Run).map_err(|error| {
        format!(
            "치환 주소 {label}를 해석할 수 없습니다: {error}. 주소의 run 인덱스와 chars 범위를 확인하세요"
        )
    })?;
    let TargetKind::Run {
        w_start,
        w_end,
        paragraph_wchar_len,
        ..
    } = target.kind
    else {
        return Err(format!(
            "치환 주소 {label}는 run 범위로 해석되지 않았습니다. run 주소나 chars 범위를 지정하세요"
        ));
    };
    replace_text_in_window(
        doc,
        label,
        &target.path,
        w_start,
        w_end,
        paragraph_wchar_len,
        from,
        to,
    )
}

/// Splices `to` over the WCHAR window `[w_start, w_end)` of the paragraph at `path`, provided the
/// window is exactly `from` and that paragraph is still `paragraph_wchar_len` WCHAR long.
///
/// This is the apply half of [`replace_text_in_range`], split out for callers that resolved the
/// address themselves: an `id`/`at` address names a run by its position, so the batch apply loop
/// resolves it during preflight, against the document as loaded (D-06), and then maps that path
/// through the earlier ops' inserts and removals before splicing here. `path` is therefore the
/// paragraph's **current** path, `label` is the address the caller was given (used in messages
/// only, never to locate anything), and `paragraph_wchar_len` is the length the preflight
/// resolution observed.
///
/// The length is checked here, not at resolve time: an earlier op of the same batch (`replace`,
/// `set_indent_level` stripping blanks) can change the paragraph's length while leaving it at the
/// same path, which moves the characters the offsets name (#420). That is a refusal, like the
/// `set_format` arm's: nothing is written and the message names the address and both lengths. A
/// paragraph that moved to another index is the caller's structural drift to handle; a removal
/// that took it away is already rejected by the preflight removal clause.
///
/// Returns `Ok(1)` when the window was rewritten and `Ok(0)` when it is text but is not `from`;
/// see [`replace_text_in_range`] for the refusal cases and the trailing-control rule.
#[allow(clippy::too_many_arguments)]
pub fn replace_text_in_window(
    doc: &mut Document,
    label: &crate::address::Address,
    path: &SegmentPath,
    w_start: u32,
    w_end: u32,
    paragraph_wchar_len: u32,
    from: &str,
    to: &str,
) -> Result<usize, String> {
    if from.is_empty() {
        return Ok(0);
    }
    let replaced = {
        let Some(para) = crate::address::paragraph_at_mut(doc, path) else {
            return Err(format!(
                "치환 주소 {label}의 문단 {path}를 찾을 수 없습니다. 주소를 다시 읽고 재시도하세요"
            ));
        };
        let current_len = para.wchar_len();
        if current_len != paragraph_wchar_len {
            return Err(format!(
                "치환 주소 {label}의 문단 {path}은 앞선 연산(replace, set_indent_level 등)으로 글자 수가 {paragraph_wchar_len}에서 {current_len}(으)로 바뀌어 범위 [{w_start}, {w_end})가 다른 글자를 가리킵니다. 아무것도 쓰지 않았습니다. 연산 순서를 바꾸거나 별도 편집으로 나누어 실행하세요"
            ));
        }
        let (start, end) = source_char_indices(para, w_start, w_end).ok_or_else(|| {
            format!(
                "치환 주소 {label}의 chars 범위 [{w_start}, {w_end})가 문자 경계에 맞지 않습니다. 글자 중간을 자르지 않는 범위를 지정하세요"
            )
        })?;
        // A stored paragraph break (or any other control) at the span's end is not text.
        let mut text_end = end;
        while text_end > start && !matches!(para.chars[text_end - 1], HwpChar::Text(_)) {
            text_end -= 1;
        }
        if para.chars[start..text_end]
            .iter()
            .any(|ch| !matches!(ch, HwpChar::Text(_)))
        {
            return Err(format!(
                "치환 주소 {label}의 범위에 텍스트가 아닌 문자가 있습니다. 텍스트만 덮는 chars 범위를 지정하세요"
            ));
        }
        let text: String = para.chars[start..text_end]
            .iter()
            .filter_map(|ch| match ch {
                HwpChar::Text(c) => Some(*c),
                _ => None,
            })
            .collect();
        if text != from {
            return Ok(0);
        }
        let from_w: u32 = para.chars[start..text_end]
            .iter()
            .map(HwpChar::wchar_width)
            .sum();
        splice_text(para, start..text_end, w_start, from_w, to)?;
        1
    };
    if replaced > 0 {
        crate::address::invalidate_ancestors(doc, path);
    }
    Ok(replaced)
}

/// The character-vector range whose WCHAR offsets cover `[w_start, w_end)`, or `None` when
/// either bound falls inside a character (a `chars` window that splits one).
fn source_char_indices(para: &Paragraph, w_start: u32, w_end: u32) -> Option<(usize, usize)> {
    let mut acc = 0u32;
    let mut start = None;
    let mut end = None;
    for (index, ch) in para.chars.iter().enumerate() {
        if acc == w_start {
            start = Some(index);
        }
        acc += ch.wchar_width();
        if acc == w_end {
            end = Some(index + 1);
            break;
        }
    }
    match (start, end) {
        (Some(start), Some(end)) if start < end => Some((start, end)),
        _ => None,
    }
}

/// Finds the first from match after start_idx in contiguous Text. Returns (character-vector index,
/// paragraph-relative WCHAR offset).
pub(crate) fn find_match(chars: &[HwpChar], from: &str, start_idx: usize) -> Option<(usize, u32)> {
    let mut wpos: u32 = chars[..start_idx.min(chars.len())]
        .iter()
        .map(HwpChar::wchar_width)
        .sum();
    let mut i = start_idx;
    while i < chars.len() {
        if matches!(chars[i], HwpChar::Text(_)) {
            let seg_start = i;
            let seg_wstart = wpos;
            let mut seg = String::new();
            let mut j = i;
            while let Some(HwpChar::Text(c)) = chars.get(j) {
                seg.push(*c);
                j += 1;
            }
            if let Some(byte_off) = seg.find(from) {
                let prefix = &seg[..byte_off];
                let char_off = prefix.chars().count();
                let wchar_off = utf16_len(prefix);
                return Some((seg_start + char_off, seg_wstart + wchar_off));
            }
            wpos += utf16_len(&seg);
            i = j;
        } else {
            wpos += chars[i].wchar_width();
            i += 1;
        }
    }
    None
}

/// Adjusts char_shape_run boundaries for replacement at WCHAR position p, old length lo and new
/// length ln. Remove internal boundaries so replacement text inherits the shape active at p; shift
/// later boundaries by the length delta.
pub(crate) fn adjust_runs(runs: &mut Vec<(u32, CharShapeId)>, p: u32, lo: u32, ln: u32) {
    let delta = i64::from(ln) - i64::from(lo);
    let mut out: Vec<(u32, CharShapeId)> = Vec::with_capacity(runs.len());
    for &(pos, id) in runs.iter() {
        let np = if pos <= p {
            pos
        } else if pos >= p + lo {
            (i64::from(pos) + delta).max(0) as u32
        } else {
            continue; // Remove boundaries inside the replaced range.
        };
        match out.last() {
            Some(&(lp, _)) if lp == np => {} // Duplicate position: keep the first.
            Some(&(_, lid)) if lid == id => {} // Consecutive identical shape: omit the redundant boundary.
            _ => out.push((np, id)),
        }
    }
    if out.is_empty() {
        out.push((0, CharShapeId::default()));
    }
    *runs = out;
}

/// Replaces cell (row, col) text in table_index, the zero-based document-order table index. Use the
/// cell's first paragraph as a formatting template. Values separated by blank lines (two or more
/// LF) become one paragraph per block; see split_cell_blocks.
pub fn set_cell(
    doc: &mut Document,
    table_index: usize,
    row: u16,
    col: u16,
    text: &str,
) -> Result<(), String> {
    if document_has_range_tags(doc) && check_set_cell_edit(doc, table_index, row, col, text)? {
        return Ok(());
    }
    // The hwp5-sourced edit path runs the writer with synthesize=false, so neither
    // assign_instance_ids nor set_last_para_flag runs and paragraphs created here
    // must carry their own document-unique ids. Read the global maximum before the
    // table borrow so the new ids cannot collide with a paragraph elsewhere.
    //
    // A8 requires every instance id to be non-zero and unique, so the whole range is
    // reserved with checked arithmetic before anything is mutated: on overflow the call
    // fails and the document is left exactly as it was, rather than wrapping to zero or
    // back onto an id already in use.
    let blocks = split_cell_blocks(text);
    let overflow = || {
        "문단 instance_id를 더 배정할 수 없습니다 (u32 한계) — 셀 값을 나누지 않고 넣으세요"
            .to_string()
    };
    // Paragraph 0 keeps the template id, so only the blocks after it need new ones - a
    // single-block value allocates nothing and cannot overflow.
    let needed = blocks.len().saturating_sub(1) as u32;
    let next_instance_id = if needed == 0 {
        0
    } else {
        let next = doc_max_instance_id(doc)
            .checked_add(1)
            .ok_or_else(overflow)?;
        next.checked_add(needed - 1).ok_or_else(overflow)?;
        next
    };
    with_nth_table(doc, table_index, |t| {
        set_cell_in_table(t, row, col, &blocks, next_instance_id)
    })
    .unwrap_or_else(|| Err(format!("표 #{table_index}를 찾을 수 없습니다")))
}

/// Adds count empty rows to zero-based table_index, cloning template_row (zero-based, defaulting to
/// the last unmerged row) to preserve cell width/margins/borders/list/shape formatting while
/// clearing contents. Fill new rows, starting at the previous row count, through set_cell. HWP5
/// output must use WriteOptions.edited=true to restore line-layout, paragraph-end and nchars
/// invariants for Hangul acceptance.
pub fn add_rows(
    doc: &mut Document,
    table_index: usize,
    template_row: Option<u16>,
    count: usize,
) -> Result<(), String> {
    add_rows_at(doc, table_index, None, count, template_row)
}

/// Insert `count` empty rows before the `at` boundary (0-based; omitted or `rows`
/// means append) of table `table_index` (0-based) (#77). Vertical merges crossing the
/// boundary grow their `row_span` by `count`; every other inserted coordinate gets a
/// styled 1x1 cell whose style donor is the cell covering that column at
/// `template_row` (for coordinates covered by a merge, the anchor cell) — text and
/// controls are never cloned. When `template_row` is omitted: append keeps the legacy
/// clean-row resolver, positioned insertion uses the nearest row at or before the
/// boundary (else the boundary row). On validation/overflow failure the table is left
/// untouched (applied to a clone, swapped in only on success — atomic).
pub fn add_rows_at(
    doc: &mut Document,
    table_index: usize,
    at: Option<u16>,
    count: usize,
    template_row: Option<u16>,
) -> Result<(), String> {
    if count == 0 {
        return Ok(());
    }
    with_nth_table(doc, table_index, |t| {
        add_rows_in_table(t, at, count, template_row)
    })
    .unwrap_or_else(|| Err(format!("표 #{table_index}를 찾을 수 없습니다")))
}

/// Returns (row count, column count) of zero-based table_index, used by data-driven fills to
/// determine added rows. Read-only; retain hwpx_raw_xml.
pub fn table_dims(doc: &mut Document, table_index: usize) -> Option<(u16, u16)> {
    with_nth_table_readonly(doc, table_index, |t| (t.rows, t.cols))
}

/// Deletes zero-based row from table_index, renumbering subsequent rows and updating
/// row_cell_counts. Refuses merged cells or rows covered by vertical merges to avoid breaking the
/// grid.
pub fn delete_table_row(doc: &mut Document, table_index: usize, row: u16) -> Result<(), String> {
    with_nth_table(doc, table_index, |t| delete_row_in_table(t, row))
        .unwrap_or_else(|| Err(format!("표 #{table_index}를 찾을 수 없습니다")))
}

fn delete_row_in_table(table: &mut hwp_model::Table, row: u16) -> Result<(), String> {
    if row >= table.rows {
        return Err(format!("행 {row}이 없습니다 (행 {}개)", table.rows));
    }
    if table.rows <= 1 {
        return Err("마지막 행은 삭제할 수 없습니다".to_string());
    }
    if !is_clean_row(table, row) {
        return Err(format!(
            "행 {row}에 병합 셀이 있거나 세로 병합에 덮여 있어 삭제를 지원하지 않습니다"
        ));
    }
    table.cells.retain(|c| c.row != row);
    for c in &mut table.cells {
        if c.row > row {
            c.row -= 1;
        }
    }
    table.rows -= 1;
    if (row as usize) < table.row_cell_counts.len() {
        table.row_cell_counts.remove(row as usize);
    }
    Ok(())
}

// Table-cell merge/split and column insertion/deletion (GK-1/GK-2).
//
// Maintain the storage rules confirmed across 1,816 genuine Hangul merged tables, unanimously in
// HWP5 and HWPX:
// 1. Store only the upper-left anchor of a merged region; omit covered cells completely, in both
//    HWP5 LIST_HEADER and HWPX hp:tc.
// 2. The sum of col_span * row_span equals rows * cols; anchor areas tile the grid exactly.
// 3. Store cells row-major, lexicographically by anchor (row,col), skipping covered columns.
// 4. row_cell_counts[r] counts anchors whose row==r; count row_span>1 cells only in their anchor
//    row.
// 5. Merged-cell cellSz covers the whole region, summing constituent column widths/row heights.
// Recheck with validate_table_invariants after editing; return Err rather than emit a corrupt
// table.
// Share #9's recursive with_nth_table locator and its set-cell-compatible indices.

/// Fallback width for a completely unknown table, absent from normal samples: approximate A4 body
/// width.
const BODY_WIDTH_APPROX: i32 = 42520;
/// Fallback empty-row height: 10pt text plus cell margins.
const ROW_HEIGHT_APPROX: i32 = 1700;

/// Logical rows-by-cols grid containing each position's owning cell index, including the anchor for
/// covered positions. Return Err for overlap, gaps or out-of-bounds cells to detect structural
/// corruption.
fn build_grid(table: &hwp_model::Table) -> Result<Vec<Vec<usize>>, String> {
    let rows = table.rows as usize;
    let cols = table.cols as usize;
    let mut grid = vec![vec![usize::MAX; cols]; rows];
    for (i, c) in table.cells.iter().enumerate() {
        let (r0, c0) = (c.row as usize, c.col as usize);
        let rs = c.row_span.max(1) as usize;
        let cs = c.col_span.max(1) as usize;
        if r0 + rs > rows || c0 + cs > cols {
            return Err(format!(
                "셀 ({r0},{c0}) span({cs}×{rs})이 표({rows}×{cols}) 범위 초과"
            ));
        }
        for row in grid.iter_mut().take(r0 + rs).skip(r0) {
            for slot in row.iter_mut().take(c0 + cs).skip(c0) {
                if *slot != usize::MAX {
                    return Err("셀 겹침 — 그리드 위치가 두 셀에 속함".to_string());
                }
                *slot = i;
            }
        }
    }
    for (r, row) in grid.iter().enumerate() {
        for (c, slot) in row.iter().enumerate() {
            if *slot == usize::MAX {
                return Err(format!("그리드 빈칸: ({r},{c}) — 셀 누락"));
            }
        }
    }
    Ok(grid)
}

/// Column widths in HWPUNIT: derive from single-column cells, distribute residual width evenly
/// where only spanning cells exist, then approximate remaining unknowns by the average.
fn column_widths(table: &hwp_model::Table) -> Vec<i32> {
    let cols = table.cols as usize;
    let mut w = vec![0i32; cols];
    for c in &table.cells {
        if c.col_span <= 1 {
            let ci = c.col as usize;
            if ci < cols {
                w[ci] = w[ci].max(c.width.0);
            }
        }
    }
    for c in &table.cells {
        if c.col_span > 1 {
            let ci = c.col as usize;
            let end = (ci + c.col_span as usize).min(cols);
            let unknown: Vec<usize> = (ci..end).filter(|&j| w[j] == 0).collect();
            if !unknown.is_empty() {
                let known: i32 = (ci..end).map(|j| w[j]).sum();
                let rem = (c.width.0 - known).max(0);
                let each = rem / unknown.len() as i32;
                let last = rem - each * (unknown.len() as i32 - 1);
                for (k, &j) in unknown.iter().enumerate() {
                    w[j] = if k + 1 == unknown.len() { last } else { each };
                }
            }
        }
    }
    let total: i32 = w.iter().sum();
    let fallback = if total > 0 && cols > 0 {
        (total / cols as i32).max(1)
    } else {
        (BODY_WIDTH_APPROX / cols.max(1) as i32).max(1)
    };
    for x in &mut w {
        if *x == 0 {
            *x = fallback;
        }
    }
    w
}

/// Row heights in HWPUNIT, the vertical counterpart of column_widths.
fn row_heights(table: &hwp_model::Table) -> Vec<i32> {
    let rows = table.rows as usize;
    let mut h = vec![0i32; rows];
    for c in &table.cells {
        if c.row_span <= 1 {
            let ri = c.row as usize;
            if ri < rows {
                h[ri] = h[ri].max(c.height.0);
            }
        }
    }
    for c in &table.cells {
        if c.row_span > 1 {
            let ri = c.row as usize;
            let end = (ri + c.row_span as usize).min(rows);
            let unknown: Vec<usize> = (ri..end).filter(|&j| h[j] == 0).collect();
            if !unknown.is_empty() {
                let known: i32 = (ri..end).map(|j| h[j]).sum();
                let rem = (c.height.0 - known).max(0);
                let each = rem / unknown.len() as i32;
                let last = rem - each * (unknown.len() as i32 - 1);
                for (k, &j) in unknown.iter().enumerate() {
                    h[j] = if k + 1 == unknown.len() { last } else { each };
                }
            }
        }
    }
    let total: i32 = h.iter().sum();
    let fallback = if total > 0 && rows > 0 {
        (total / rows as i32).max(1)
    } else {
        ROW_HEIGHT_APPROX
    };
    for x in &mut h {
        if *x == 0 {
            *x = fallback;
        }
    }
    h
}

/// Maximum table-local instance_id, used for unique nonzero IDs in new empty paragraphs, as in
/// add_rows.
fn max_instance_id(table: &hwp_model::Table) -> u32 {
    table
        .cells
        .iter()
        .flat_map(|c| &c.paragraphs)
        .map(|p| p.header.instance_id)
        .max()
        .unwrap_or(0)
}

/// Whether a paragraph contains actual Text, used to remove empty paragraphs during cell merges.
fn has_text(p: &Paragraph) -> bool {
    p.chars.iter().any(|c| matches!(c, HwpChar::Text(_)))
}

/// Sets nchars bit31 (chars_flags 0x80) only on the last paragraph in a list, following B4.
pub(crate) fn fixup_last_para_flag(paras: &mut [Paragraph]) {
    let n = paras.len();
    for (i, p) in paras.iter_mut().enumerate() {
        if i + 1 == n {
            p.header.chars_flags |= 0x80;
        } else {
            p.header.chars_flags &= !0x80;
        }
    }
}

/// An empty 1x1 cell inherits margins, borders, list_attr and shapes from its template. Populate
/// one paragraph, one character-shape run, the last-paragraph bit (blank_para_like), and a unique
/// instance_id (A5..A7 gates).
fn blank_cell(
    row: u16,
    col: u16,
    width: i32,
    height: i32,
    tmpl: &hwp_model::Cell,
    inst: u32,
) -> hwp_model::Cell {
    let mut p = blank_para_like(tmpl.paragraphs.first());
    p.header.instance_id = inst;
    hwp_model::Cell {
        list_attr: tmpl.list_attr,
        col,
        row,
        col_span: 1,
        row_span: 1,
        width: hwp_model::HwpUnit(width),
        height: hwp_model::HwpUnit(height),
        margins: tmpl.margins,
        border_fill: tmpl.border_fill,
        header_tail: Vec::new(),
        paragraphs: vec![p],
    }
}

/// Recalculates row_cell_counts from the cell list, counting anchors per row.
fn recount_rows(table: &mut hwp_model::Table) {
    let mut counts = vec![0u16; table.rows as usize];
    for c in &table.cells {
        if (c.row as usize) < counts.len() {
            counts[c.row as usize] += 1;
        }
    }
    table.row_cell_counts = counts;
}

/// Sorts cells row-major by anchor (row,col), the genuine storage-order invariant.
fn sort_cells_row_major(table: &mut hwp_model::Table) {
    table.cells.sort_by_key(|c| (c.row, c.col));
}

/// Revalidates table invariants after edits as a corruption gate; return Err on violations.
pub(crate) fn validate_table_invariants(table: &hwp_model::Table) -> Result<(), String> {
    build_grid(table)?; // Overlap, gaps and bounds.
    let area: usize = table
        .cells
        .iter()
        .map(|c| c.col_span.max(1) as usize * c.row_span.max(1) as usize)
        .sum();
    let full = table.rows as usize * table.cols as usize;
    if area != full {
        return Err(format!("면적 합 {area} != rows×cols {full}"));
    }
    for w in table.cells.windows(2) {
        if (w[0].row, w[0].col) > (w[1].row, w[1].col) {
            return Err("셀이 행 우선 순서가 아님".to_string());
        }
    }
    if table.row_cell_counts.len() != table.rows as usize {
        return Err(format!(
            "row_cell_counts 길이 {} != rows {}",
            table.row_cell_counts.len(),
            table.rows
        ));
    }
    let mut counts = vec![0u16; table.rows as usize];
    for c in &table.cells {
        counts[c.row as usize] += 1;
    }
    if counts != table.row_cell_counts {
        return Err(format!(
            "row_cell_counts 불일치: 계산 {counts:?} != 저장 {:?}",
            table.row_cell_counts
        ));
    }
    Ok(())
}

/// Merges the inclusive zero-based rectangle (r1,c1)..(r2,c2) in the nth table. The upper-left
/// anchor acquires the span and absorbs paragraphs from covered cells, which leave the list. Return
/// Err for partial overlap with existing merges or an out-of-bounds region.
pub fn merge_cells(
    doc: &mut Document,
    table_index: usize,
    r1: u16,
    c1: u16,
    r2: u16,
    c2: u16,
) -> Result<(), String> {
    with_nth_table(doc, table_index, |t| {
        merge_cells_in_table(t, r1, c1, r2, c2)
    })
    .unwrap_or_else(|| Err(format!("표 #{table_index}를 찾을 수 없습니다")))
}

fn merge_cells_in_table(
    table: &mut hwp_model::Table,
    r1: u16,
    c1: u16,
    r2: u16,
    c2: u16,
) -> Result<(), String> {
    let (r1, r2) = (r1.min(r2), r1.max(r2));
    let (c1, c2) = (c1.min(c2), c1.max(c2));
    if r2 >= table.rows || c2 >= table.cols {
        return Err(format!(
            "병합 영역 ({r1},{c1})-({r2},{c2})이 표({}×{}) 범위 초과",
            table.rows, table.cols
        ));
    }
    if r1 == r2 && c1 == c2 {
        return Err("병합 영역이 셀 1개입니다 (2개 이상 필요)".to_string());
    }
    let grid = build_grid(table)?;
    let anchor_idx = grid[r1 as usize][c1 as usize];
    if table.cells[anchor_idx].row != r1 || table.cells[anchor_idx].col != c1 {
        return Err(format!(
            "병합 영역 좌상단 ({r1},{c1})이 셀 경계와 어긋남 — 앵커 셀의 좌상단이어야 함"
        ));
    }
    let mut remove = vec![false; table.cells.len()];
    for r in r1..=r2 {
        for c in c1..=c2 {
            let idx = grid[r as usize][c as usize];
            let cell = &table.cells[idx];
            if cell.row < r1
                || cell.col < c1
                || cell.row + cell.row_span - 1 > r2
                || cell.col + cell.col_span - 1 > c2
            {
                return Err(format!(
                    "병합 영역이 기존 셀 경계와 어긋남 (셀 ({},{}) span {}×{}) — 부분 겹침 금지",
                    cell.row, cell.col, cell.col_span, cell.row_span
                ));
            }
            if idx != anchor_idx {
                remove[idx] = true;
            }
        }
    }
    let colw = column_widths(table);
    let rowh = row_heights(table);
    let new_w: i32 = (c1 as usize..=c2 as usize).map(|j| colw[j]).sum();
    let new_h: i32 = (r1 as usize..=r2 as usize).map(|j| rowh[j]).sum();
    let mut order: Vec<usize> = (0..table.cells.len())
        .filter(|&i| i == anchor_idx || remove[i])
        .collect();
    order.sort_by_key(|&i| (table.cells[i].row, table.cells[i].col));
    let mut merged: Vec<Paragraph> = Vec::new();
    for &i in &order {
        for p in &table.cells[i].paragraphs {
            let mut p = p.clone();
            p.line_segs.clear();
            merged.push(p);
        }
    }
    let mut kept: Vec<Paragraph> = merged.iter().filter(|p| has_text(p)).cloned().collect();
    if kept.is_empty() {
        kept.push(
            merged
                .into_iter()
                .next()
                .unwrap_or_else(|| blank_para_like(None)),
        );
    }
    fixup_last_para_flag(&mut kept);
    {
        let a = &mut table.cells[anchor_idx];
        a.col_span = c2 - c1 + 1;
        a.row_span = r2 - r1 + 1;
        a.width = hwp_model::HwpUnit(new_w);
        a.height = hwp_model::HwpUnit(new_h);
        a.paragraphs = kept;
    }
    let mut k = 0usize;
    table.cells.retain(|_| {
        let keep = !remove[k];
        k += 1;
        keep
    });
    sort_cells_row_major(table);
    recount_rows(table);
    validate_table_invariants(table)
}

/// Splits the span>1 anchor at (row,col) in the nth table into 1x1 cells. Preserve the upper-left
/// anchor's position/content and fill the remaining covered positions with empty cells (A5..A7),
/// dividing cellSz evenly.
pub fn split_cell(
    doc: &mut Document,
    table_index: usize,
    row: u16,
    col: u16,
) -> Result<(), String> {
    with_nth_table(doc, table_index, |t| split_cell_in_table(t, row, col))
        .unwrap_or_else(|| Err(format!("표 #{table_index}를 찾을 수 없습니다")))
}

fn split_cell_in_table(table: &mut hwp_model::Table, row: u16, col: u16) -> Result<(), String> {
    if row >= table.rows || col >= table.cols {
        return Err(format!(
            "셀 ({row},{col})이 표({}×{}) 범위 초과",
            table.rows, table.cols
        ));
    }
    let idx = table
        .cells
        .iter()
        .position(|c| c.row == row && c.col == col)
        .ok_or_else(|| {
            format!("({row},{col})은 앵커 셀이 아닙니다 — 병합 셀의 좌상단만 분할할 수 있습니다")
        })?;
    let (cs, rs, tw, th) = {
        let c = &table.cells[idx];
        (c.col_span, c.row_span, c.width.0, c.height.0)
    };
    if cs <= 1 && rs <= 1 {
        return Err(format!("셀 ({row},{col})은 병합되지 않았습니다"));
    }
    let (cs_i, rs_i) = (cs.max(1) as i32, rs.max(1) as i32);
    let base_w = (tw / cs_i).max(1);
    let base_h = (th / rs_i).max(1);
    let col_w = |dc: u16| {
        if dc as i32 == cs_i - 1 {
            (tw - base_w * (cs_i - 1)).max(1)
        } else {
            base_w
        }
    };
    let row_h = |dr: u16| {
        if dr as i32 == rs_i - 1 {
            (th - base_h * (rs_i - 1)).max(1)
        } else {
            base_h
        }
    };
    let tmpl = table.cells[idx].clone();
    let mut next_inst = max_instance_id(table);
    {
        let a = &mut table.cells[idx];
        a.col_span = 1;
        a.row_span = 1;
        a.width = hwp_model::HwpUnit(col_w(0));
        a.height = hwp_model::HwpUnit(row_h(0));
        for p in &mut a.paragraphs {
            p.line_segs.clear();
        }
        fixup_last_para_flag(&mut a.paragraphs);
    }
    for dr in 0..rs {
        for dc in 0..cs {
            if dr == 0 && dc == 0 {
                continue;
            }
            next_inst = next_inst.wrapping_add(1);
            table.cells.push(blank_cell(
                row + dr,
                col + dc,
                col_w(dc),
                row_h(dr),
                &tmpl,
                next_inst,
            ));
        }
    }
    sort_cells_row_major(table);
    recount_rows(table);
    validate_table_invariants(table)
}

/// Appends one column to zero-based table_index, retaining MCP/legacy CLI compatibility.
/// Total table width is preserved (the append special case of [`add_table_columns`]).
/// Merged tables are supported.
pub fn add_col(doc: &mut Document, table_index: usize) -> Result<(), String> {
    add_table_columns(doc, table_index, None, 1)
}

/// Insert a single empty column before `at_col` (0-based, 0..=cols) of table
/// `table_index` — the count=1 special case of [`add_table_columns`].
pub fn add_table_column(doc: &mut Document, table_index: usize, at_col: u16) -> Result<(), String> {
    add_table_columns(doc, table_index, Some(at_col), 1)
}

/// Insert `count` empty columns before the `at_col` boundary (0-based; omitted or
/// `cols` means append) of table `table_index` (#77). Each inserted column follows the
/// existing single-column policy: horizontal merges crossing the boundary grow their
/// `col_span`, other rows get a blank 1x1 cell, and the total table width is preserved
/// by proportionally shrinking existing columns. Bounds and u16 overflow are checked
/// before any mutation, and the loop runs on a clone swapped in only on success — a
/// failure leaves the table untouched.
pub fn add_table_columns(
    doc: &mut Document,
    table_index: usize,
    at_col: Option<u16>,
    count: u16,
) -> Result<(), String> {
    if count == 0 {
        return Err("추가 열 수는 1 이상이어야 합니다".to_string());
    }
    with_nth_table(doc, table_index, |t| {
        let cols = t.cols;
        let at = at_col.unwrap_or(cols);
        if at > cols {
            return Err(format!("열 삽입 위치 {at}가 범위를 벗어남 (0..={cols})"));
        }
        if u32::from(cols) + u32::from(count) > u32::from(u16::MAX) {
            return Err(format!(
                "추가 열 수가 너무 많습니다: {count} (열 수는 u16 범위)"
            ));
        }
        let mut work = t.clone();
        add_table_column_in_table(&mut work, at, count)?;
        *t = work;
        Ok(())
    })
    .unwrap_or_else(|| Err(format!("표 #{table_index}를 찾을 수 없습니다")))
}

fn add_table_column_in_table(
    table: &mut hwp_model::Table,
    at_col: u16,
    count: u16,
) -> Result<(), String> {
    let cols = table.cols;
    if at_col > cols {
        return Err(format!(
            "열 삽입 위치 {at_col}이 범위를 벗어남 (0..={cols})"
        ));
    }
    if table.cells.is_empty() || table.rows == 0 {
        return Err("빈 표에는 열을 추가할 수 없습니다".to_string());
    }
    if u32::from(cols) + u32::from(count) > u32::from(u16::MAX) {
        return Err("열 수가 u16 범위를 넘습니다".to_string());
    }
    build_grid(table)?; // Prevalidate.
    // For appending to an unmerged table, retain #9's exact per-row redistribution, independently
    // preserving every row's total width, including nonuniform rows unlike genuine grids. Merged
    // tables and interior insertion require aligned columns and use the column-width path below.
    let has_merge = table
        .cells
        .iter()
        .any(|c| c.col_span != 1 || c.row_span != 1);
    if at_col == cols && !has_merge {
        return add_col_append_uniform(table, count);
    }
    // Total-width preservation in a single pass: existing columns shrink
    // proportionally and the inserted band takes one uniform share per new
    // column (#9 policy). Looping the single-column inserter would be quadratic
    // for large counts and could produce zero-width intermediates.
    let colw = column_widths(table);
    let total: i64 = colw.iter().map(|&w| i64::from(w)).sum();
    if total <= 0 {
        return Err("표 총폭이 0이라 열 폭을 재분배할 수 없습니다".to_string());
    }
    let count64 = i64::from(count);
    let new_w = total / (i64::from(cols) + count64);
    let remain = total - new_w * count64;
    let mut scaled = vec![0i64; cols as usize];
    let mut acc = 0i64;
    for i in 0..cols as usize {
        scaled[i] = if i + 1 == cols as usize {
            remain - acc
        } else {
            i64::from(colw[i]) * remain / total
        };
        acc += scaled[i];
    }
    // Final column widths (length cols+count): insert new_w x count at `at_col`.
    let mut final_colw: Vec<i64> = Vec::with_capacity(cols as usize + count as usize);
    final_colw.extend_from_slice(&scaled[..at_col as usize]);
    final_colw.extend(std::iter::repeat_n(new_w, count as usize));
    final_colw.extend_from_slice(&scaled[at_col as usize..]);
    // Every column must keep at least 1 HWP unit — clamping would silently grow
    // the table past its preserved width.
    if final_colw.iter().any(|&w| w < 1) {
        return Err(format!(
            "표 폭({total})이 부족해 열 {count}개를 삽입할 수 없습니다 (열당 최소 1단위)"
        ));
    }
    // Structure update: extend merges crossing the band + shift later cells.
    let rowh = row_heights(table);
    let tmpl = table.cells[0].clone();
    let mut band_covered = vec![vec![false; count as usize]; table.rows as usize];
    for c in &mut table.cells {
        let ac = c.col;
        let ec = c.col + c.col_span; // exclusive
        if ac >= at_col {
            c.col += count;
        } else if at_col < ec {
            // A merge crossing the band extends across all of it.
            c.col_span += count;
            for r in c.row..c.row + c.row_span {
                for slot in &mut band_covered[r as usize] {
                    *slot = true;
                }
            }
        }
    }
    // New blank 1x1 cells at band coordinates not covered by an extended span.
    let mut next_inst = max_instance_id(table);
    for r in 0..table.rows {
        for k in 0..count {
            if band_covered[r as usize][k as usize] {
                continue;
            }
            next_inst = next_inst.wrapping_add(1);
            table.cells.push(blank_cell(
                r,
                at_col + k,
                new_w as i32,
                rowh[r as usize],
                &tmpl,
                next_inst,
            ));
        }
    }
    table.cols += count;
    // Each cell width is the sum of final_colw for its spanned columns, preserving
    // whole-table/per-row totals exactly.
    for c in &mut table.cells {
        let s: i64 = (c.col as usize..(c.col + c.col_span) as usize)
            .map(|j| final_colw.get(j).copied().unwrap_or(0))
            .sum();
        c.width = hwp_model::HwpUnit(s.max(1) as i32);
    }
    sort_cells_row_major(table);
    recount_rows(table);
    validate_table_invariants(table)
}

/// Append `count` columns to a merge-free table — the count generalization of the
/// original #9 algorithm (exact per-row width redistribution). Each row's total
/// width is preserved independently, so non-uniform (non-grid) rows stay exact.
/// Not used for merged tables (column alignment breaks there — the column-width
/// based path in [`add_table_column_in_table`] handles those).
fn add_col_append_uniform(table: &mut hwp_model::Table, count: u16) -> Result<(), String> {
    let cols = table.cols;
    let count64 = i64::from(count);
    let mut next_inst = max_instance_id(table);
    let mut new_cells =
        Vec::with_capacity(table.cells.len() + table.rows as usize * count as usize);
    for r in 0..table.rows {
        let mut row_cells: Vec<hwp_model::Cell> =
            table.cells.iter().filter(|c| c.row == r).cloned().collect();
        row_cells.sort_by_key(|c| c.col);
        let row_total: i64 = row_cells.iter().map(|c| i64::from(c.width.0)).sum();
        if row_total <= 0 {
            return Err(format!(
                "행 {r}의 총폭이 0이라 열 폭을 재분배할 수 없습니다"
            ));
        }
        let new_w = row_total / (i64::from(cols) + count64);
        let scaled_target = row_total - new_w * count64;
        let last_idx = row_cells.len() - 1;
        let mut acc: i64 = 0;
        for (i, c) in row_cells.iter_mut().enumerate() {
            let w = i64::from(c.width.0);
            let nw = if i == last_idx {
                scaled_target - acc
            } else {
                w * scaled_target / row_total
            };
            c.width = hwp_model::HwpUnit(nw as i32);
            acc += nw;
        }
        // Every column must keep at least 1 HWP unit — clamping would silently
        // grow the table past its preserved width.
        if new_w < 1 || row_cells.iter().any(|c| c.width.0 < 1) {
            return Err(format!(
                "행 {r}의 폭({row_total})이 부족해 열 {count}개를 삽입할 수 없습니다 (열당 최소 1단위)"
            ));
        }
        new_cells.extend(row_cells);
        for k in 0..count {
            let mut nc = new_cells[new_cells.len() - 1].clone();
            nc.col = cols + k;
            nc.width = hwp_model::HwpUnit(new_w as i32);
            // The cloned LIST_HEADER tail embeds the donor width; the new cell
            // gets a fresh width, so let the writer re-synthesize the tail.
            nc.header_tail = Vec::new();
            let mut para = blank_para_like(new_cells.last().and_then(|c| c.paragraphs.first()));
            next_inst = next_inst.wrapping_add(1);
            para.header.instance_id = next_inst;
            nc.paragraphs = vec![para];
            new_cells.push(nc);
        }
    }
    table.cells = new_cells;
    table.cols += count;
    for cnt in &mut table.row_cell_counts {
        *cnt += count;
    }
    Ok(())
}

/// Deletes zero-based col from table_index. Crossing merged cells shrink col_span by one; 1x1 cells
/// solely in that column are removed. Preserve total table width by redistributing the removed
/// width proportionally. Refuse deletion of the last remaining column.
pub fn delete_table_column(doc: &mut Document, table_index: usize, col: u16) -> Result<(), String> {
    with_nth_table(doc, table_index, |t| delete_table_column_in_table(t, col))
        .unwrap_or_else(|| Err(format!("표 #{table_index}를 찾을 수 없습니다")))
}

fn delete_table_column_in_table(table: &mut hwp_model::Table, col: u16) -> Result<(), String> {
    if col >= table.cols {
        return Err(format!("열 {col}이 없습니다 (열 {}개)", table.cols));
    }
    if table.cols <= 1 {
        return Err("마지막 열은 삭제할 수 없습니다".to_string());
    }
    build_grid(table)?; // Prevalidate.
    let colw = column_widths(table);
    let total: i64 = colw.iter().map(|&w| i64::from(w)).sum();
    let remain_total = total - i64::from(colw[col as usize]);
    // Move, shrink or remove cells.
    let mut to_remove: Vec<usize> = Vec::new();
    for (i, c) in table.cells.iter_mut().enumerate() {
        let ac = c.col;
        let ec = c.col + c.col_span; // exclusive
        if ac > col {
            c.col -= 1;
        } else if ec <= col {
            // Left of the removed column: unchanged.
        } else if c.col_span > 1 {
            c.col_span -= 1;
        } else {
            to_remove.push(i);
        }
    }
    for &i in to_remove.iter().rev() {
        table.cells.remove(i);
    }
    table.cols -= 1;
    // Remaining columns absorb the removed column's width proportionally, retaining total width.
    let old_remaining: Vec<usize> = (0..colw.len()).filter(|&j| j != col as usize).collect();
    let ncols = table.cols as usize;
    let mut final_colw = vec![0i64; ncols];
    let mut acc = 0i64;
    for (newj, &oldj) in old_remaining.iter().enumerate() {
        final_colw[newj] = if newj + 1 == ncols {
            total - acc
        } else if remain_total > 0 {
            i64::from(colw[oldj]) * total / remain_total
        } else {
            (total / ncols.max(1) as i64).max(1)
        };
        acc += final_colw[newj];
    }
    for c in &mut table.cells {
        let s: i64 = (c.col as usize..(c.col + c.col_span) as usize)
            .map(|j| final_colw.get(j).copied().unwrap_or(0))
            .sum();
        c.width = hwp_model::HwpUnit(s.max(1) as i32);
    }
    sort_cells_row_major(table);
    recount_rows(table);
    validate_table_invariants(table)
}

/// Applies key=value metadata: title, author, subject or keywords. Empty values clear a field to
/// None. Unknown keys or malformed specifications return Err.
pub fn apply_meta(doc: &mut Document, spec: &str) -> Result<(), String> {
    let (key, value) = spec
        .split_once('=')
        .ok_or_else(|| format!("메타데이터 형식은 \"키=값\" 입니다: {spec:?}"))?;
    let val = (!value.is_empty()).then(|| value.to_string());
    match key.trim() {
        "title" => doc.metadata.title = val,
        "author" => doc.metadata.author = val,
        "subject" => doc.metadata.subject = val,
        "keywords" => doc.metadata.keywords = val,
        other => {
            return Err(format!(
                "메타데이터 키는 title|author|subject|keywords 입니다: {other:?}"
            ));
        }
    }
    Ok(())
}

/// Finds zero-based document-order table index and applies f, recursively through body, cells and
/// text boxes. Returns Some(f's result) when found, otherwise None.
/// If f changes a table inside a Generic control with captured hwpx_raw_xml, clear that stale XML.
/// This prevents stale output and instead produces a fail-closed preservation error if no semantic
/// emitter exists.
pub(crate) fn with_nth_table<R, F: FnOnce(&mut hwp_model::Table) -> R>(
    doc: &mut Document,
    index: usize,
    f: F,
) -> Option<R> {
    walk_nth_table_root(doc, index, true, f)
}

/// Read-only variant used by table_dims; retain hwpx_raw_xml because content is unchanged.
pub(crate) fn with_nth_table_readonly<R, F: FnOnce(&mut hwp_model::Table) -> R>(
    doc: &mut Document,
    index: usize,
    f: F,
) -> Option<R> {
    walk_nth_table_root(doc, index, false, f)
}

/// #296: table-placement counts (total, inline, floating). Use reader-provided HWP5/HWPX placement
/// where present; synthesized placement=None follows the HWPX writer's inline fallback. The
/// read-only walker never mutates the document.
pub fn table_placement_stats(doc: &mut Document) -> (usize, usize, usize) {
    let (mut total, mut inline, mut floating) = (0usize, 0usize, 0usize);
    let mut i = 0usize;
    while let Some(treat) = with_nth_table_readonly(doc, i, |t| {
        t.placement
            .as_ref()
            .map(|p| p.treat_as_char)
            .unwrap_or_else(|| {
                if t.common_data.len() >= 4 {
                    u32::from_le_bytes([
                        t.common_data[0],
                        t.common_data[1],
                        t.common_data[2],
                        t.common_data[3],
                    ]) & 1
                        == 1
                } else {
                    true // Synthesized table: matches the writer's inline default.
                }
            })
    }) {
        total += 1;
        if treat {
            inline += 1;
        } else {
            floating += 1;
        }
        i += 1;
    }
    (total, inline, floating)
}

/// #296: switches treat-as-character table placement. index is the zero-based recursive
/// document-order index, matching label-table; None selects every table. Return None when there are
/// no targets, otherwise Some(number changed). Tables already matching the request remain
/// untouched, so reapplication is a byte-stable no-op.
pub fn set_table_placement(
    doc: &mut Document,
    index: Option<usize>,
    placement: crate::from_markdown::TablePlacement,
) -> Option<usize> {
    let mut found = 0usize;
    let mut changed = 0usize;
    let mut i = 0usize;
    loop {
        let target = index.unwrap_or(i);
        let needs =
            with_nth_table_readonly(doc, target, |t| !table_placement_matches(t, placement));
        match needs {
            None => break, // No table at this index; stop searching.
            Some(false) => found += 1,
            Some(true) => {
                let did = with_nth_table(doc, target, |t| apply_table_placement(t, placement))
                    .unwrap_or(false);
                found += 1;
                changed += usize::from(did);
            }
        }
        if index.is_some() {
            break;
        }
        i += 1;
    }
    (found > 0).then_some(changed)
}

/// Whether requested placement already matches both placement and original HWP5 common_data bits.
fn table_placement_matches(t: &hwp_model::Table, tp: crate::from_markdown::TablePlacement) -> bool {
    let floating = tp == crate::from_markdown::TablePlacement::Floating;
    let placement_ok = t
        .placement
        .as_ref()
        .is_some_and(|p| p.treat_as_char != floating && p.flow_with_text != floating);
    let raw_ok = common_data_placement_matches(&t.common_data, !floating);
    placement_ok && raw_ok
}

/// Whether HWP5 common_data placement bits match treat. attr is little-endian [0..4],
/// bit0=treat-as-character, bit13=wrap with body text, matching GsoPlacement::synth_attr.
/// Synthesized tables without common_data pass this check; placement alone determines them.
fn common_data_placement_matches(common_data: &[u8], treat: bool) -> bool {
    if common_data.len() < 4 {
        return true;
    }
    let attr = u32::from_le_bytes([
        common_data[0],
        common_data[1],
        common_data[2],
        common_data[3],
    ]);
    (attr & 1 == u32::from(treat)) && ((attr >> 13) & 1 == u32::from(treat))
}

/// Applies the change only when table_placement_matches is false. Leave already-correct fields
/// untouched for byte-stable reapplication.
fn apply_table_placement(
    t: &mut hwp_model::Table,
    tp: crate::from_markdown::TablePlacement,
) -> bool {
    let floating = tp == crate::from_markdown::TablePlacement::Floating;
    let (treat, flow) = (!floating, !floating);
    let mut changed = false;
    match &mut t.placement {
        Some(pl) => {
            if pl.treat_as_char != treat || pl.flow_with_text != flow {
                pl.treat_as_char = treat;
                pl.flow_with_text = flow;
                changed = true;
            }
        }
        None => {
            // For synthesized Markdown tables, setting placement disables the writer's synthesized
            // fallback (pageBreak=CELL, repeatHeader=1). Set those defaults explicitly in attr
            // (0b110).
            t.placement = Some(tp.gso_placement());
            t.attr |= 0b110;
            changed = true;
        }
    }
    // The HWP5 writer prioritizes raw common_data, so patch its placement bits as well.
    if t.common_data.len() >= 4 {
        let attr = u32::from_le_bytes([
            t.common_data[0],
            t.common_data[1],
            t.common_data[2],
            t.common_data[3],
        ]);
        let patched = (attr & !0b10_0000_0000_0001) | u32::from(treat) | (u32::from(flow) << 13);
        if patched != attr {
            t.common_data[..4].copy_from_slice(&patched.to_le_bytes());
            changed = true;
        }
    }
    changed
}

fn walk_nth_table_root<R, F: FnOnce(&mut hwp_model::Table) -> R>(
    doc: &mut Document,
    index: usize,
    mutating: bool,
    f: F,
) -> Option<R> {
    let mut seen = 0;
    let mut f = Some(f);
    let mut out = None;
    for section in &mut doc.sections {
        for para in &mut section.paragraphs {
            walk_nth_table(para, index, mutating, &mut seen, &mut f, &mut out);
            if out.is_some() {
                return out;
            }
        }
    }
    out
}

fn walk_nth_table<R, F: FnOnce(&mut hwp_model::Table) -> R>(
    para: &mut Paragraph,
    index: usize,
    mutating: bool,
    seen: &mut usize,
    f: &mut Option<F>,
    out: &mut Option<R>,
) {
    for ctrl in &mut para.controls {
        if out.is_some() {
            return;
        }
        match ctrl {
            Control::Table(t) => {
                if *seen == index {
                    if let Some(func) = f.take() {
                        *out = Some(func(t));
                    }
                    *seen += 1;
                    return;
                }
                *seen += 1;
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        walk_nth_table(p, index, mutating, seen, f, out);
                        if out.is_some() {
                            return;
                        }
                    }
                }
            }
            Control::Generic(g) => {
                for list in &mut g.paragraph_lists {
                    for p in &mut list.paragraphs {
                        walk_nth_table(p, index, mutating, seen, f, out);
                        if out.is_some() {
                            if mutating {
                                // The object's content changed; clear captured XML to prevent stale
                                // output.
                                g.hwpx_raw_xml = None;
                            }
                            return;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Split a `--set-cell` value into one block per paragraph: CRLF is normalised to
/// LF, a run of two or more LF is a paragraph boundary, each block loses only its
/// leading and trailing LF, empty blocks are dropped, and an empty result falls
/// back to a single empty block so an empty value keeps producing one empty
/// paragraph. A single LF inside a block stays an in-paragraph line break.
fn split_cell_blocks(text: &str) -> Vec<String> {
    let normalised = text.replace("\r\n", "\n");
    let mut blocks: Vec<String> = normalised
        .split("\n\n")
        .map(|b| b.trim_matches('\n').to_string())
        .filter(|b| !b.is_empty())
        .collect();
    if blocks.is_empty() {
        blocks.push(String::new());
    }
    blocks
}

/// Checks the whole cell subtree before the rebuilding walk can invalidate ancestors.
/// A tagged exact no-op preserves the existing paragraphs and their opaque records.
pub(crate) fn check_set_cell_edit(
    doc: &mut Document,
    table_index: usize,
    row: u16,
    col: u16,
    text: &str,
) -> Result<bool, String> {
    let blocks = split_cell_blocks(text);
    with_nth_table_readonly(doc, table_index, |table| {
        let cell = table
            .cells
            .iter()
            .find(|cell| cell.row == row && cell.col == col)
            .ok_or_else(|| format!("표에 셀 ({row}, {col})이 없습니다"))?;
        if !cell.paragraphs.iter().any(paragraph_tree_has_range_tags) {
            return Ok(false);
        }
        let identical = cell.paragraphs.len() == blocks.len()
            && cell.paragraphs.iter().zip(&blocks).all(|(para, block)| {
                if !para.controls.is_empty() {
                    return false;
                }
                let chars = para
                    .chars
                    .strip_suffix(&[HwpChar::CharCtrl(hwp_model::ctrl_char::PARA_BREAK)])
                    .unwrap_or(&para.chars);
                let expected: Vec<_> = block
                    .chars()
                    .map(|c| {
                        if c == '\n' {
                            HwpChar::CharCtrl(hwp_model::ctrl_char::LINE_BREAK)
                        } else {
                            HwpChar::Text(c)
                        }
                    })
                    .collect();
                chars == expected
            });
        if identical {
            Ok(true)
        } else {
            Err(RANGE_TAG_SHIFT_REFUSAL.to_string())
        }
    })
    .unwrap_or_else(|| Err(format!("표 #{table_index}를 찾을 수 없습니다")))
}

/// `next_instance_id` must already be a reserved, overflow-checked range of
/// `blocks.len() - 1` ids - see [`set_cell`], which refuses before mutating anything.
fn set_cell_in_table(
    table: &mut hwp_model::Table,
    row: u16,
    col: u16,
    blocks: &[String],
    next_instance_id: u32,
) -> Result<(), String> {
    let cell = table
        .cells
        .iter_mut()
        .find(|c| c.row == row && c.col == col)
        .ok_or_else(|| format!("표에 셀 ({row}, {col})이 없습니다"))?;

    // Clone the first paragraph as a formatting template, preserving paragraph/style/character
    // shapes and header. The complete paragraph list is replaced below, so capture the template
    // first.
    let template = cell.paragraphs.first().cloned();
    let mut paras: Vec<Paragraph> = blocks
        .iter()
        .enumerate()
        .map(|(i, block)| {
            let mut para = blank_para_like(template.as_ref());
            if i > 0 {
                // Paragraph 0 keeps the template's own instance id, so a
                // single-block value still produces the IR it produced before the
                // split; the rest take document-global ids because the surgical
                // writer will not assign them (see `set_cell`).
                para.header.instance_id = next_instance_id + (i as u32 - 1);
            }
            para.chars = block
                .chars()
                .map(|c| {
                    if c == '\n' {
                        HwpChar::CharCtrl(hwp_model::ctrl_char::LINE_BREAK)
                    } else {
                        HwpChar::Text(c)
                    }
                })
                .collect();
            if !para.chars.is_empty() {
                para.chars
                    .push(HwpChar::CharCtrl(hwp_model::ctrl_char::PARA_BREAK));
            }
            para
        })
        .collect();
    // B4: only the last paragraph of the list carries nchars bit31.
    fixup_last_para_flag(&mut paras);
    cell.paragraphs = paras;
    Ok(())
}

/// Empty paragraph for row insertion/cell replacement: retain the template's paragraph/style/first
/// character shape/header, clearing contents and line layouts for writer synthesis. Hangul
/// synthesis requires at least one paragraph per cell and one character-shape run, so empty chars
/// suffice; the writer handles nchars=1 with omitted PARA_TEXT.
///
/// Return with nchars bit31 set for the last paragraph in a list: most callers put only this
/// paragraph in cell.paragraphs. Multi-paragraph callers (set_cell_in_table) must call
/// fixup_last_para_flag after assembling the list to leave the bit only on its final paragraph.
/// HWP5 edits use synthesize=false and do not run writer set_last_para_flag; failing to maintain it
/// here makes Hangul reject the cell as corrupt (B4).
///
/// Clear the template header's ctrl_mask. It names the template's extended/inline controls, but
/// this paragraph has no controls; inheriting it would declare nonexistent controls and cause
/// Hangul corruption. With zero, the writer recalculates from characters.
fn blank_para_like(template: Option<&Paragraph>) -> Paragraph {
    let mut header = template.map(|p| p.header.clone()).unwrap_or_default();
    header.chars_flags |= 0x80;
    header.ctrl_mask = 0;
    Paragraph {
        para_shape: template.map(|p| p.para_shape).unwrap_or_default(),
        style: template.map(|p| p.style).unwrap_or_default(),
        chars: Vec::new(),
        char_shape_runs: vec![(
            0,
            template
                .and_then(|p| p.char_shape_runs.first().map(|r| r.1))
                .unwrap_or_default(),
        )],
        line_segs: Vec::new(),
        controls: Vec::new(),
        header,
        extras: Vec::new(),
    }
}

fn add_rows_in_table(
    table: &mut hwp_model::Table,
    at: Option<u16>,
    count: usize,
    template_row: Option<u16>,
) -> Result<(), String> {
    if table.rows == 0 {
        return Err("빈 표에는 행을 추가할 수 없습니다".to_string());
    }
    let rows = table.rows;
    // Insertion boundary: omitted or `rows` means append; beyond `rows` is refused.
    let b = match at {
        None => rows,
        Some(a) if a <= rows => a,
        Some(a) => {
            return Err(format!("행 삽입 위치 {a}가 범위를 벗어남 (0..={rows})"));
        }
    };
    // Row counts are u16: refuse beyond remaining capacity, or truncating count to u16 misaligns
    // cells/row_cell_counts and corrupts table records.
    let remaining = usize::from(u16::MAX) - usize::from(table.rows);
    if count > remaining {
        return Err(format!(
            "추가 행 수가 너무 많습니다: {count} (최대 {remaining}행 — 표 행 수는 u16 범위)"
        ));
    }
    if let Some(r) = template_row
        && r >= rows
    {
        return Err(format!("템플릿 행 {r}이 표 범위를 벗어남 (행 수: {rows})"));
    }
    // Atomicity: apply to a clone and swap in only on success.
    let mut work = table.clone();
    add_rows_in_table_inner(&mut work, b, count, template_row)?;
    *table = work;
    Ok(())
}

fn add_rows_in_table_inner(
    table: &mut hwp_model::Table,
    b: u16,
    count: usize,
    template_row: Option<u16>,
) -> Result<(), String> {
    let positioned = b < table.rows;
    // Template row resolution: explicit value (range checked above) / legacy
    // clean-row resolver for omitted append / nearest row at or before the boundary
    // (else the boundary row) for omitted positioned insertion — grid projection
    // lets a merged row donate styles too.
    let tpl = match template_row {
        Some(r) => r,
        None if !positioned => clean_template_row(table)
            .ok_or("복제할 병합 없는 행이 없습니다 — 템플릿 행을 지정하세요")?,
        None => {
            if b > 0 {
                b - 1
            } else {
                b
            }
        }
    };
    // Style donor for column c: the template row's own 1x1 cells when it is clean
    // (identical to legacy), otherwise the anchor cell covering (tpl, c) in the
    // logical grid — merged or vertically covered rows can donate too (only styles
    // are taken; text and controls are never cloned).
    let grid = if positioned || !is_clean_row(table, tpl) {
        Some(build_grid(table)?)
    } else {
        None
    };
    let donor_cells: Vec<hwp_model::Cell> = match &grid {
        Some(g) => (0..table.cols as usize)
            .map(|c| table.cells[g[tpl as usize][c]].clone())
            .collect(),
        None => {
            let mut v: Vec<hwp_model::Cell> = table
                .cells
                .iter()
                .filter(|c| c.row == tpl)
                .cloned()
                .collect();
            v.sort_by_key(|c| c.col);
            v
        }
    };
    if donor_cells.is_empty() {
        return Err(format!("템플릿 행 {tpl}에 셀이 없습니다"));
    }
    let colw = column_widths(table);
    let rowh = row_heights(table);
    // Prevent cloned paragraph instance_id collisions. HWP5 edits use synthesize=false and the
    // writer does not reassign IDs, so assign unique IDs above the table-local maximum. Cloning one
    // template into N cells otherwise repeats one nonzero ID N+1 times and breaks Hangul object
    // links.
    let mut next_inst = max_instance_id(table);
    let count16 = count as u16;
    // 1) Positioned insertion: shift cells at/below the boundary, extend vertical
    //    merges crossing it, and mark the inserted-band coordinates they cover
    //    (no new cell is created under a covering span).
    let mut covered = vec![vec![false; table.cols as usize]; count];
    if positioned {
        for cell in &mut table.cells {
            let r0 = cell.row;
            let r1 = cell.row + cell.row_span; // exclusive
            if r0 >= b {
                cell.row += count16;
            } else if b < r1 {
                cell.row_span += count16;
                // Merged cellSz = sum of the covered row heights, so extending
                // across the inserted band adds count x template-row height.
                cell.height = hwp_model::HwpUnit(cell.height.0 + rowh[tpl as usize] * count as i32);
                for slot in covered.iter_mut() {
                    for c in cell.col..cell.col + cell.col_span {
                        slot[c as usize] = true;
                    }
                }
            }
        }
    }
    // 2) Create styled 1x1 cells at uncovered coordinates of the inserted band.
    for (nr, slot) in covered.iter().enumerate() {
        let r = b + nr as u16;
        for c in 0..table.cols {
            if slot[c as usize] {
                continue;
            }
            let donor = &donor_cells[c as usize];
            let mut nc = donor.clone();
            nc.row = r;
            nc.col = c;
            nc.col_span = 1;
            nc.row_span = 1;
            // A 1x1 donor keeps its own width/height (identical to legacy append).
            // A merged donor holds the whole region's extent, so project to the
            // column width / template row height instead.
            if donor.col_span > 1 {
                nc.width = hwp_model::HwpUnit(colw[c as usize]);
            }
            if donor.row_span > 1 {
                nc.height = hwp_model::HwpUnit(rowh[tpl as usize]);
            }
            if donor.col_span > 1 || donor.row_span > 1 {
                // The hwp5 LIST_HEADER tail embeds the donor's (region) width;
                // after projecting the geometry that byte pattern is stale.
                // Clear it so the writer synthesizes a tail for the new width.
                nc.header_tail = Vec::new();
            }
            let mut para = blank_para_like(donor.paragraphs.first());
            next_inst = next_inst.wrapping_add(1);
            para.header.instance_id = next_inst;
            nc.paragraphs = vec![para];
            table.cells.push(nc);
        }
    }
    // 3) Metadata refresh + invariant re-check (the legacy append path only pushes,
    //    exactly as before).
    table.rows += count16;
    if grid.is_some() {
        sort_cells_row_major(table);
        recount_rows(table);
        validate_table_invariants(table)?;
    } else {
        for _ in 0..count {
            table.row_cell_counts.push(table.cols);
        }
    }
    Ok(())
}

/// Whether row r is clean: all columns have 1x1 cells with no merges (row_span/col_span==1) and no
/// coverage from a vertical merge (row_cell_counts==cols). Shared by row cloning/deletion and
/// column-insertion guards.
fn is_clean_row(table: &hwp_model::Table, r: u16) -> bool {
    table.row_cell_counts.get(r as usize).copied() == Some(table.cols)
        && table
            .cells
            .iter()
            .filter(|c| c.row == r)
            .all(|c| c.col_span == 1 && c.row_span == 1)
}

/// Default clone template: the last clean, unmerged row filling every column
/// (row_cell_counts==cols). Rows covered by vertical merges have fewer cells than columns and are
/// excluded automatically.
fn clean_template_row(table: &hwp_model::Table) -> Option<u16> {
    (0..table.rows).rev().find(|&r| is_clean_row(table, r))
}

pub(crate) fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

/// Inserts a uniform table after the first body paragraph containing the `anchor` text (GK-3).
/// `rows` is a row×column text grid — cell defaults match from_markdown's GFM tables
/// (equal body-width columns, row height 1700, vertical center, solid borders). Returns the number inserted (0 or 1).
pub fn add_table(doc: &mut Document, anchor: &str, rows: &[Vec<String>]) -> Result<usize, String> {
    use crate::from_markdown::{BODY_WIDTH, CELL_VALIGN_CENTER, TABLE_BORDER_FILL};
    use hwp_model::{BorderFillId, Cell, HwpUnit, Table};

    if anchor.is_empty() {
        return Err("앵커 텍스트가 없습니다".into());
    }
    if rows.is_empty() || rows.iter().all(Vec::is_empty) {
        return Err("표 행 데이터가 없습니다".into());
    }
    let n_rows = rows.len() as u16;
    let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1) as u16;
    let col_w = BODY_WIDTH / i32::from(cols);
    let mut cells = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        for c in 0..cols {
            let text = row.get(c as usize).cloned().unwrap_or_default();
            cells.push(Cell {
                list_attr: CELL_VALIGN_CENTER,
                col: c,
                row: r as u16,
                col_span: 1,
                row_span: 1,
                width: HwpUnit(col_w),
                height: HwpUnit(1700),
                margins: [510, 510, 141, 141],
                border_fill: BorderFillId(TABLE_BORDER_FILL),
                header_tail: Vec::new(),
                paragraphs: vec![Paragraph {
                    chars: text.chars().map(HwpChar::Text).collect(),
                    char_shape_runs: vec![(0, CharShapeId(0))],
                    ..Paragraph::default()
                }],
            });
        }
    }
    let table = Table {
        common_data: Vec::new(),
        placement: None,
        attr: 0,
        rows: n_rows,
        cols,
        cell_spacing: 0,
        inner_margins: [510, 510, 141, 141],
        row_cell_counts: vec![cols; n_rows as usize],
        border_fill: BorderFillId(TABLE_BORDER_FILL),
        table_tail: Vec::new(),
        caption: None,
        cells,
        extras: Vec::new(),
    };
    validate_table_invariants(&table)?;

    let mut payload = vec![0u8; 12];
    payload[..4].copy_from_slice(b" lbt"); // reversed ctrl_id
    let anchor_para = Paragraph {
        chars: vec![
            HwpChar::ExtCtrl {
                code: 11,
                ctrl_id: *b"tbl ",
                payload,
                ctrl_index: Some(0),
            },
            HwpChar::CharCtrl(13),
        ],
        char_shape_runs: vec![(0, CharShapeId(0))],
        controls: vec![Control::Table(table)],
        ..Paragraph::default()
    };
    Ok(insert_para_after_anchor(doc, anchor, anchor_para)? as usize)
}

/// Shared anchor semantics for `add_table`/`clone_table`: insert `para`
/// immediately after the first top-level section paragraph whose text contains
/// `anchor`. Top-level only — anchors inside cells/text boxes never match.
fn insert_para_after_anchor(
    doc: &mut Document,
    anchor: &str,
    para: Paragraph,
) -> Result<u32, String> {
    for section in &mut doc.sections {
        if let Some(i) = section
            .paragraphs
            .iter()
            .position(|p| find_match(&p.chars, anchor, 0).is_some())
        {
            section.paragraphs.insert(i + 1, para);
            return Ok(1);
        }
    }
    Err(anchor_not_found_message(anchor))
}

/// The exact refusal `add_table`/`clone_table` return when no top-level paragraph holds
/// `anchor`. Callers that know the anchor classify by equality with this, never by parsing.
pub fn anchor_not_found_message(anchor: &str) -> String {
    format!("앵커를 찾을 수 없습니다: {anchor}")
}

/// How a cloned table treats source cell content (#78).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneTextMode {
    /// Keep one empty styled paragraph per logical cell; drop all source text
    /// and content controls (fields, bookmarks, images, equations, nested
    /// content). Geometry, spans, borders, fills, and styles are preserved.
    Blank,
    /// Clone every supported paragraph and control subtree (nested tables and
    /// pictures), remapping all paragraph/control/object instance ids above the
    /// document maxima. Aborts atomically on opaque controls (`Generic`) whose
    /// raw identity bytes the model cannot safely remap.
    Keep,
}

/// Deep-clone table `source_table` (0-based, recursive order of appearance) and
/// insert the clone immediately after the first top-level paragraph containing
/// `anchor` (#78). The source table is never modified; any failure (missing
/// anchor, invalid index, unsafe opaque child, id overflow) leaves the document
/// untouched — all fallible work happens on the clone before insertion.
pub fn clone_table(
    doc: &mut Document,
    source_table: usize,
    anchor: &str,
    mode: CloneTextMode,
) -> Result<u32, String> {
    if anchor.is_empty() {
        return Err("앵커 텍스트가 없습니다".into());
    }
    let (mut clone, payload) = take_table_clone(doc, source_table)
        .ok_or_else(|| format!("표 #{source_table}를 찾을 수 없습니다"))?;
    if mode == CloneTextMode::Keep {
        ensure_cloneable(&clone)?;
    } else {
        // Blank: one empty styled paragraph per logical cell and caption; spans,
        // geometry, and cell-level formatting stay untouched.
        if let Some(cap) = &mut clone.caption {
            cap.paragraphs = vec![blank_para_like(cap.paragraphs.first())];
        }
        for cell in &mut clone.cells {
            cell.paragraphs = vec![blank_para_like(cell.paragraphs.first())];
        }
    }
    // Paragraph instance ids must be unique document-wide on the HWP path (the
    // hwp5-sourced edit path runs with synthesize=false, so the writer will not
    // reassign them) — blank-mode paragraphs inherit the source ids via
    // blank_para_like, so both modes remap here.
    let mut next_para = doc_max_instance_id(doc)
        .checked_add(1)
        .ok_or_else(|| "문단 인스턴스 id 오버플로".to_string())?;
    reassign_clone_para_ids(&mut clone, &mut next_para)?;
    // Object identity: patch the gso common instance id (common_data @32) or,
    // for hwpx-sourced tables, bump the placement z-order so the writer's
    // `0x5000_0000 | z_order` synthesis cannot collide with the source.
    let mut next_obj = doc_max_object_id(doc)
        .checked_add(1)
        .ok_or_else(|| "개체 id 오버플로".to_string())?
        .max(1);
    let mut next_z = doc_max_table_z_order(doc).saturating_add(1);
    remap_clone_object_ids(&mut clone, &mut next_obj, &mut next_z)?;
    validate_table_invariants(&clone)?;

    let mut anchor_para = Paragraph {
        chars: vec![
            HwpChar::ExtCtrl {
                code: 11,
                ctrl_id: *b"tbl ",
                payload,
                ctrl_index: Some(0),
            },
            HwpChar::CharCtrl(13),
        ],
        char_shape_runs: vec![(0, CharShapeId(0))],
        controls: vec![Control::Table(clone)],
        ..Paragraph::default()
    };
    anchor_para.header.instance_id = next_para;
    insert_para_after_anchor(doc, anchor, anchor_para)
}

/// Clone the nth table (recursive depth-first order shared with
/// `walk_nth_table`) out of the document, together with the ExtCtrl payload of
/// its anchor paragraph (fallback: the same 12-byte default `add_table` uses).
fn take_table_clone(doc: &Document, index: usize) -> Option<(hwp_model::Table, Vec<u8>)> {
    let mut seen = 0usize;
    for section in &doc.sections {
        for para in &section.paragraphs {
            if let Some(found) = find_table_clone_in_para(para, index, &mut seen) {
                return Some(found);
            }
        }
    }
    None
}

fn find_table_clone_in_para(
    para: &Paragraph,
    index: usize,
    seen: &mut usize,
) -> Option<(hwp_model::Table, Vec<u8>)> {
    for ctrl in &para.controls {
        match ctrl {
            Control::Table(t) => {
                if *seen == index {
                    let payload = para
                        .chars
                        .iter()
                        .find_map(|ch| {
                            if let HwpChar::ExtCtrl {
                                ctrl_id, payload, ..
                            } = ch
                                && ctrl_id == b"tbl "
                            {
                                return Some(payload.clone());
                            }
                            None
                        })
                        .unwrap_or_else(|| {
                            let mut p = vec![0u8; 12];
                            p[..4].copy_from_slice(b" lbt"); // reversed ctrl_id
                            p
                        });
                    return Some((t.clone(), payload));
                }
                *seen += 1;
                for cell in &t.cells {
                    for p in &cell.paragraphs {
                        if let Some(found) = find_table_clone_in_para(p, index, seen) {
                            return Some(found);
                        }
                    }
                }
            }
            Control::Generic(g) => {
                for list in &g.paragraph_lists {
                    for p in &list.paragraphs {
                        if let Some(found) = find_table_clone_in_para(p, index, seen) {
                            return Some(found);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Keep-mode safety gate: any `Generic` control carries raw identity bytes
/// (CTRL_HEADER data, preserved subtrees, raw XML) the model cannot remap, so
/// the clone aborts rather than silently duplicating or dropping it (#78).
fn ensure_cloneable(table: &hwp_model::Table) -> Result<(), String> {
    if let Some(cap) = &table.caption {
        for p in &cap.paragraphs {
            ensure_cloneable_para(p)?;
        }
    }
    for cell in &table.cells {
        for p in &cell.paragraphs {
            ensure_cloneable_para(p)?;
        }
    }
    Ok(())
}

fn ensure_cloneable_para(para: &Paragraph) -> Result<(), String> {
    for ctrl in &para.controls {
        match ctrl {
            Control::Table(t) => ensure_cloneable(t)?,
            Control::Picture(pic) => {
                if let Some(cap) = &pic.caption {
                    for p in &cap.paragraphs {
                        ensure_cloneable_para(p)?;
                    }
                }
            }
            Control::Generic(g) => {
                let _ = g;
                return Err(format!(
                    "keep 모드에서 안전하게 복제할 수 없는 개체({})가 표에 포함되어 있습니다",
                    String::from_utf8_lossy(&ctrl.ctrl_id())
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Maximum paragraph instance id across the whole document — body, table cells
/// (nested tables included), captions, and Generic paragraph lists. The
/// existing `max_instance_id` is table-local; clone remapping must be global.
fn doc_max_instance_id(doc: &Document) -> u32 {
    let mut max = 0u32;
    for section in &doc.sections {
        max_instance_id_in_paras(&section.paragraphs, &mut max);
    }
    max
}

fn max_instance_id_in_paras(paras: &[Paragraph], max: &mut u32) {
    for p in paras {
        *max = (*max).max(p.header.instance_id);
        for ctrl in &p.controls {
            match ctrl {
                Control::Table(t) => {
                    if let Some(cap) = &t.caption {
                        max_instance_id_in_paras(&cap.paragraphs, max);
                    }
                    for cell in &t.cells {
                        max_instance_id_in_paras(&cell.paragraphs, max);
                    }
                }
                Control::Picture(pic) => {
                    if let Some(cap) = &pic.caption {
                        max_instance_id_in_paras(&cap.paragraphs, max);
                    }
                }
                Control::Generic(g) => {
                    // Shape/drawing captions carry paragraphs too — include them
                    // so the document maximum really is global.
                    if let Some(cap) = &g.caption {
                        max_instance_id_in_paras(&cap.paragraphs, max);
                    }
                    for list in &g.paragraph_lists {
                        max_instance_id_in_paras(&list.paragraphs, max);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Reassign every paragraph instance id inside the clone from `next` upward.
fn reassign_clone_para_ids(table: &mut hwp_model::Table, next: &mut u32) -> Result<(), String> {
    if let Some(cap) = &mut table.caption {
        reassign_para_ids(&mut cap.paragraphs, next)?;
    }
    for cell in &mut table.cells {
        reassign_para_ids(&mut cell.paragraphs, next)?;
    }
    Ok(())
}

fn reassign_para_ids(paras: &mut [Paragraph], next: &mut u32) -> Result<(), String> {
    for p in paras {
        p.header.instance_id = *next;
        *next = next
            .checked_add(1)
            .ok_or_else(|| "문단 인스턴스 id 오버플로".to_string())?;
        for ctrl in &mut p.controls {
            match ctrl {
                Control::Table(t) => reassign_clone_para_ids(t, next)?,
                Control::Picture(pic) => {
                    if let Some(cap) = &mut pic.caption {
                        reassign_para_ids(&mut cap.paragraphs, next)?;
                    }
                }
                Control::Generic(g) => {
                    for list in &mut g.paragraph_lists {
                        reassign_para_ids(&mut list.paragraphs, next)?;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// gso common instance id (4 bytes LE at offset 32), when the raw bytes carry
/// one — see `hwp5::write::emit_table`/`is_materialized_generated_picture`.
fn gso_common_instance_id(common: &[u8]) -> Option<u32> {
    common
        .get(32..36)
        .map(|b| u32::from_le_bytes(b.try_into().expect("slice len checked")))
}

/// A table's object identity as the HWP writer will emit it: the preserved
/// gso common id, or the id synthesized from the hwpx placement z-order.
fn table_object_id(t: &hwp_model::Table) -> u32 {
    gso_common_instance_id(&t.common_data).unwrap_or_else(|| {
        t.placement
            .as_ref()
            .map(|pl| synthesized_table_object_id(pl.z_order))
            .unwrap_or(0)
    })
}

/// The instance id the HWP writer synthesizes for a placement-only table (no raw gso common
/// bytes): `0x5000_0000` plus the low 16 bits of its z-order (`hwp5::write`, table common
/// properties). Two such tables with equal low z-order bits share one id.
pub(crate) fn synthesized_table_object_id(z_order: i32) -> u32 {
    0x5000_0000 | (z_order as u32 & 0xffff)
}

/// Maximum gso object id across all tables and pictures in the document.
pub(crate) fn doc_max_object_id(doc: &Document) -> u32 {
    let mut max = 0u32;
    for_each_object_id(doc, &mut |id| max = max.max(id));
    max
}

/// Every gso object id the writers would emit for the document's tables, pictures and gso
/// containers, as [`doc_max_object_id`] sees them.
pub(crate) fn doc_object_ids(doc: &Document) -> std::collections::BTreeSet<u32> {
    let mut ids = std::collections::BTreeSet::new();
    for_each_object_id(doc, &mut |id| {
        ids.insert(id);
    });
    ids
}

fn for_each_object_id(doc: &Document, visit: &mut dyn FnMut(u32)) {
    for section in &doc.sections {
        for para in &section.paragraphs {
            object_ids_in_para(para, visit);
        }
    }
}

fn object_ids_in_para(para: &Paragraph, visit: &mut dyn FnMut(u32)) {
    for ctrl in &para.controls {
        match ctrl {
            Control::Table(t) => {
                visit(table_object_id(t));
                if let Some(cap) = &t.caption {
                    for p in &cap.paragraphs {
                        object_ids_in_para(p, visit);
                    }
                }
                for cell in &t.cells {
                    for p in &cell.paragraphs {
                        object_ids_in_para(p, visit);
                    }
                }
            }
            Control::Picture(pic) => {
                if let Some(id) = gso_common_instance_id(&pic.common_data) {
                    visit(id);
                }
                if let Some(cap) = &pic.caption {
                    for p in &cap.paragraphs {
                        object_ids_in_para(p, visit);
                    }
                }
            }
            Control::Generic(g) => {
                // gso containers carry the same gso common layout — include
                // their id so a clone never collides with a drawing either.
                if g.ctrl_id == *b"gso "
                    && let Some(id) = gso_common_instance_id(&g.data)
                {
                    visit(id);
                }
                for list in &g.paragraph_lists {
                    for p in &list.paragraphs {
                        object_ids_in_para(p, visit);
                    }
                }
                if let Some(cap) = &g.caption {
                    for p in &cap.paragraphs {
                        object_ids_in_para(p, visit);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Maximum placement z-order among all tables (drives the hwpx-sourced id
/// synthesis `0x5000_0000 | z_order`; bumping it keeps the clone unique).
pub(crate) fn doc_max_table_z_order(doc: &Document) -> i32 {
    let mut max = 0i32;
    for section in &doc.sections {
        for para in &section.paragraphs {
            max_table_z_order_in_para(para, &mut max);
        }
    }
    max
}

fn max_table_z_order_in_para(para: &Paragraph, max: &mut i32) {
    for ctrl in &para.controls {
        match ctrl {
            Control::Table(t) => {
                if t.common_data.is_empty()
                    && let Some(pl) = &t.placement
                {
                    *max = (*max).max(pl.z_order);
                }
                if let Some(cap) = &t.caption {
                    for p in &cap.paragraphs {
                        max_table_z_order_in_para(p, max);
                    }
                }
                for cell in &t.cells {
                    for p in &cell.paragraphs {
                        max_table_z_order_in_para(p, max);
                    }
                }
            }
            Control::Picture(pic) => {
                if let Some(cap) = &pic.caption {
                    for p in &cap.paragraphs {
                        max_table_z_order_in_para(p, max);
                    }
                }
            }
            Control::Generic(g) => {
                for list in &g.paragraph_lists {
                    for p in &list.paragraphs {
                        max_table_z_order_in_para(p, max);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Give the cloned table (and, recursively, every nested table/picture) a fresh
/// object identity so the clone shares no mutable id with the source (#78).
/// hwp5-sourced objects get the next free gso common id; hwpx-sourced tables
/// get a bumped placement z-order (the writer derives their id from it).
/// Pictures/tables without raw common bytes synthesize id 0 — the pre-existing
/// writer fallback, unchanged here.
fn remap_clone_object_ids(
    table: &mut hwp_model::Table,
    next_id: &mut u32,
    next_z: &mut i32,
) -> Result<(), String> {
    if !table.common_data.is_empty() {
        let id = *next_id;
        *next_id = next_id
            .checked_add(1)
            .ok_or_else(|| "개체 id 오버플로".to_string())?;
        if table.common_data.len() >= 36 {
            table.common_data[32..36].copy_from_slice(&id.to_le_bytes());
        }
    } else if let Some(pl) = &mut table.placement {
        pl.z_order = *next_z;
        *next_z = next_z
            .checked_add(1)
            .ok_or_else(|| "개체 z-order 오버플로".to_string())?;
    }
    // Captions can host nested tables/pictures — remap them too.
    if let Some(cap) = &mut table.caption {
        for p in &mut cap.paragraphs {
            remap_clone_object_ids_in_para(p, next_id, next_z)?;
        }
    }
    for cell in &mut table.cells {
        for p in &mut cell.paragraphs {
            remap_clone_object_ids_in_para(p, next_id, next_z)?;
        }
    }
    Ok(())
}

fn remap_clone_object_ids_in_para(
    para: &mut Paragraph,
    next_id: &mut u32,
    next_z: &mut i32,
) -> Result<(), String> {
    for ctrl in &mut para.controls {
        match ctrl {
            Control::Table(t) => remap_clone_object_ids(t, next_id, next_z)?,
            Control::Picture(pic) => {
                if pic.common_data.len() >= 36 {
                    let id = *next_id;
                    *next_id = next_id
                        .checked_add(1)
                        .ok_or_else(|| "개체 id 오버플로".to_string())?;
                    pic.common_data[32..36].copy_from_slice(&id.to_le_bytes());
                }
                if let Some(cap) = &mut pic.caption {
                    for p in &mut cap.paragraphs {
                        remap_clone_object_ids_in_para(p, next_id, next_z)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Kind of object deletion (GK-8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    /// Picture in the paragraph containing the anchor text
    Image,
    /// Table in the paragraph containing the anchor text
    Table,
    /// The n-th table (0-based, in recursive order of appearance)
    TableNth(usize),
    /// Field named selector
    Field,
    /// Bookmark named selector
    Bookmark,
}

/// Deletes objects — removes the control together with its anchor chars (through FIELD_END for
/// fields) and adjusts WCHAR positions (GK-8). Recurses through body, table cells, and text
/// boxes. Returns the number deleted.
pub fn delete_object(
    doc: &mut Document,
    kind: ObjectKind,
    selector: &str,
) -> Result<usize, String> {
    if document_has_range_tags(doc) {
        delete_object_pass(doc, kind, selector, false)?;
    }
    delete_object_pass(doc, kind, selector, true)
}

fn delete_object_pass(
    doc: &mut Document,
    kind: ObjectKind,
    selector: &str,
    apply: bool,
) -> Result<usize, String> {
    let mut count = 0;
    let mut table_seen = 0usize;
    for section in &mut doc.sections {
        for para in &mut section.paragraphs {
            count += delete_object_in_para(para, kind, selector, &mut table_seen, apply)?;
        }
    }
    Ok(count)
}

fn delete_object_in_para(
    para: &mut Paragraph,
    kind: ObjectKind,
    selector: &str,
    table_seen: &mut usize,
    apply: bool,
) -> Result<usize, String> {
    // Collects the indexes of the controls to delete.
    let targets: Vec<usize> = para
        .controls
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let hit = match (kind, c) {
                (ObjectKind::Image, Control::Picture(_)) => {
                    find_match(&para.chars, selector, 0).is_some()
                }
                (ObjectKind::Table, Control::Table(_)) => {
                    find_match(&para.chars, selector, 0).is_some()
                }
                (ObjectKind::TableNth(nth), Control::Table(_)) => {
                    let seen = *table_seen;
                    *table_seen += 1;
                    seen == nth
                }
                (ObjectKind::TableNth(_), _) => false,
                (ObjectKind::Field, Control::Generic(g))
                    if crate::field::is_field_ctrl_id(&g.ctrl_id) =>
                {
                    crate::field::field_meta(c).0.as_deref() == Some(selector)
                }
                (ObjectKind::Bookmark, Control::Generic(g)) if g.ctrl_id == *b"bokm" => {
                    crate::bookmark::bookmark_name(c).as_deref() == Some(selector)
                }
                _ => false,
            };
            hit.then_some(i)
        })
        .collect();
    // Recursion: table cells and Generic paragraph lists.
    let mut n = 0;
    for control in &mut para.controls {
        match control {
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        n += delete_object_in_para(p, kind, selector, table_seen, apply)?;
                    }
                }
            }
            Control::Generic(g) => {
                let before = n;
                for list in &mut g.paragraph_lists {
                    for p in &mut list.paragraphs {
                        n += delete_object_in_para(p, kind, selector, table_seen, apply)?;
                    }
                }
                if apply && n > before {
                    // The object's content changed; clear captured XML to prevent stale output.
                    g.hwpx_raw_xml = None;
                }
            }
            _ => {}
        }
    }
    if targets.is_empty() {
        return Ok(n);
    }
    if apply {
        n += remove_controls(para, &targets)?;
    } else {
        check_control_removal(para, &targets)?;
        n += targets.len();
    }
    Ok(n)
}

/// Removes the controls at the `targets` indexes and their anchor chars (ExtCtrl + FIELD_END of
/// a field start), validating the width change before the removal walk mutates anything.
fn check_control_removal(para: &Paragraph, targets: &[usize]) -> Result<Vec<(u32, u32)>, String> {
    let mut removed = Vec::new();
    let mut orig_pos = 0;
    let mut pending_field_end = false;
    for ch in &para.chars {
        let width = ch.wchar_width();
        let drop = match ch {
            HwpChar::ExtCtrl {
                code, ctrl_index, ..
            } => {
                let hit = ctrl_index.is_some_and(|i| targets.contains(&(i as usize)));
                pending_field_end = hit && *code == 3;
                hit
            }
            HwpChar::InlineCtrl { code, .. } => {
                let hit = pending_field_end && *code == hwp_model::ctrl_char::FIELD_END;
                if hit {
                    pending_field_end = false;
                }
                hit
            }
            _ => false,
        };
        if drop {
            removed.push((orig_pos, width));
        }
        orig_pos += width;
    }
    ensure_range_tag_shift_allowed(para, removed.iter().map(|(_, width)| width).sum(), 0)?;
    Ok(removed)
}

fn remove_controls(para: &mut Paragraph, targets: &[usize]) -> Result<usize, String> {
    let removed = check_control_removal(para, targets)?;
    let mut orig_pos = 0;
    let kept = std::mem::take(&mut para.chars)
        .into_iter()
        .filter(|ch| {
            let keep = !removed.iter().any(|(pos, _)| *pos == orig_pos);
            orig_pos += ch.wchar_width();
            keep
        })
        .collect();
    para.chars = kept;
    // 2) Remove the controls (in reverse order to preserve indexes).
    let mut sorted = targets.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    let n = sorted.len();
    for i in sorted {
        para.controls.remove(i);
    }
    // 3) Adjust char_shape_runs positions.
    for (pos, _) in &mut para.char_shape_runs {
        let shift: u32 = removed
            .iter()
            .filter(|(start, width)| start + width <= *pos)
            .map(|(_, width)| width)
            .sum();
        *pos -= shift;
    }
    para.char_shape_runs.dedup();
    // 4) Reconnect ExtCtrl ↔ controls in order of appearance.
    crate::field::relink_ctrl_index(para);
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `replace_text_at` rewrites matches inside the resolved paragraph only; an identical
    /// string in a sibling paragraph is untouched, and the returned count reflects only the
    /// addressed paragraph (07-02 Task 1).
    #[test]
    fn replace_text_at_주소_문단만_변경() {
        let mut doc = crate::from_markdown::from_markdown("같은 문단\n\n같은 문단\n");
        let path = SegmentPath {
            section: 0,
            indices: vec![1],
        };
        let n = replace_text_at(&mut doc, &path, "같은", "다른").unwrap();
        assert_eq!(n, 1);
        assert_eq!(doc.sections[0].paragraphs[0].plain_text(), "같은 문단");
        assert_eq!(doc.sections[0].paragraphs[1].plain_text(), "다른 문단");
    }

    /// An address that does not resolve to a paragraph returns 0 and mutates nothing.
    #[test]
    fn replace_text_at_없는_주소는_0() {
        let mut doc = crate::from_markdown::from_markdown("문단\n");
        let before = doc.clone();
        let path = SegmentPath {
            section: 0,
            indices: vec![99],
        };
        let n = replace_text_at(&mut doc, &path, "문단", "다른").unwrap();
        assert_eq!(n, 0);
        assert_eq!(doc, before);
    }

    /// The body paragraph laid out the way the writer serializes `**AAA** **AAA**`: text
    /// "AAA AAA" split into three runs at WCHAR offsets 0..3, 3..4 and 4..7, so the same text
    /// sits in two runs with a boundary between them. The second paragraph is used because the
    /// first carries the importer's leading section/column control chars, which a written file
    /// does not have in the body.
    fn repeated_run_doc() -> Document {
        let mut doc = crate::from_markdown::from_markdown("first\n\nAAA AAA\n");
        let para = &mut doc.sections[0].paragraphs[1];
        assert_eq!(para.plain_text(), "AAA AAA");
        assert_eq!(
            para.chars
                .iter()
                .map(HwpChar::wchar_width)
                .collect::<Vec<_>>(),
            vec![1u32; 7],
            "body paragraph must hold seven plain characters"
        );
        let bold = para.char_shape_runs[0].1;
        let plain = CharShapeId(bold.0.wrapping_add(1));
        para.char_shape_runs = vec![(0, bold), (3, plain), (4, bold)];
        doc
    }

    fn at_run(run: usize, chars: Option<(u32, u32)>) -> crate::address::Address {
        crate::address::Address {
            section: 0,
            indices: vec![1, run],
            checksum: None,
            chars,
        }
    }

    /// A `chars` window rewrites exactly the addressed span (#525): the identical text in the
    /// sibling run keeps its characters and its shape.
    #[test]
    fn replace_text_in_range_rewrites_only_the_window() {
        let mut doc = repeated_run_doc();
        let before = doc.sections[0].paragraphs[1].char_shape_runs.clone();

        let n = replace_text_in_range(&mut doc, &at_run(2, Some((4, 7))), "AAA", "ZZZ").unwrap();

        assert_eq!(n, 1);
        let para = &doc.sections[0].paragraphs[1];
        assert_eq!(para.plain_text(), "AAA ZZZ");
        assert_eq!(
            para.char_shape_runs, before,
            "an equal-width window must not move a run boundary"
        );
    }

    /// `at.run` with no `chars` rewrites the whole run, so the other occurrence stays.
    #[test]
    fn replace_text_in_range_whole_run_without_chars() {
        let mut doc = repeated_run_doc();
        let n = replace_text_in_range(&mut doc, &at_run(0, None), "AAA", "ZZZ").unwrap();
        assert_eq!(n, 1);
        assert_eq!(doc.sections[0].paragraphs[1].plain_text(), "ZZZ AAA");
    }

    /// A window whose text is not `from` is a failed drift check, reported as 0 with nothing
    /// changed - never a rewrite of whatever now sits at the address (#525).
    #[test]
    fn replace_text_in_range_reports_a_drifted_window_as_zero() {
        let mut doc = repeated_run_doc();
        let before = doc.clone();
        let n = replace_text_in_range(&mut doc, &at_run(2, Some((4, 7))), "BBB", "ZZZ").unwrap();
        assert_eq!(n, 0);
        assert_eq!(doc, before);
    }

    /// A window bound that falls inside a two-unit character cannot name text, so it is refused
    /// rather than rounded to the nearest boundary, and the message names the address.
    #[test]
    fn replace_text_in_range_refuses_a_window_inside_a_character() {
        // "𝔸" is a single character of two UTF-16 code units, so offset 1 lands inside it.
        let mut doc = crate::from_markdown::from_markdown("first\n\n𝔸AAA\n");
        let para = &mut doc.sections[0].paragraphs[1];
        assert_eq!(
            para.chars
                .iter()
                .map(HwpChar::wchar_width)
                .collect::<Vec<_>>(),
            vec![2u32, 1, 1, 1],
            "the astral character must count as two WCHAR units"
        );
        let error = replace_text_in_range(&mut doc, &at_run(0, Some((1, 4))), "AAA", "ZZZ")
            .expect_err("a bound inside a character must be refused");
        assert!(error.contains("at 0.1.0 chars=[1, 4)"), "{error}");
        assert_eq!(doc.sections[0].paragraphs[1].plain_text(), "𝔸AAA");
    }

    #[test]
    fn add_table_앵커_뒤_삽입() {
        let mut doc = crate::from_markdown::from_markdown("머리\n\n끝\n");
        let rows = vec![
            vec!["가".to_string(), "나".to_string()],
            vec!["1".to_string(), "2".to_string()],
        ];
        assert_eq!(add_table(&mut doc, "머리", &rows).unwrap(), 1);
        let para = &doc.sections[0].paragraphs[1]; // right after the anchor
        let Control::Table(t) = &para.controls[0] else {
            panic!("표 컨트롤")
        };
        assert_eq!((t.rows, t.cols), (2, 2));
        assert_eq!(t.row_cell_counts, vec![2, 2]);
        let text: String = t.cells[2].paragraphs[0]
            .chars
            .iter()
            .filter_map(|c| match c {
                HwpChar::Text(c) => Some(*c),
                _ => None,
            })
            .collect();
        assert_eq!(text, "1");
        // Missing anchor → error.
        assert!(add_table(&mut doc, "없는앵커", &rows).is_err());
    }

    /// Test helper: collect every table in the document (recursive order).
    fn all_tables(doc: &Document) -> Vec<&hwp_model::Table> {
        fn walk<'a>(paras: &'a [Paragraph], out: &mut Vec<&'a hwp_model::Table>) {
            for p in paras {
                for ctrl in &p.controls {
                    match ctrl {
                        Control::Table(t) => {
                            out.push(t);
                            for cell in &t.cells {
                                walk(&cell.paragraphs, out);
                            }
                        }
                        Control::Generic(g) => {
                            for list in &g.paragraph_lists {
                                walk(&list.paragraphs, out);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        let mut out = Vec::new();
        for section in &doc.sections {
            walk(&section.paragraphs, &mut out);
        }
        out
    }

    /// Source for clone tests: "머리" para, a 2x2 table, "끝" para.
    fn clone_test_doc() -> Document {
        crate::from_markdown::from_markdown("머리\n\n| 가 | 나 |\n|---|---|\n| 1 | 2 |\n\n끝\n")
    }

    /// Give every source paragraph a distinct non-zero instance id so the
    /// clone's remapping is observable.
    fn seed_instance_ids(doc: &mut Document) {
        let mut next = 100u32;
        fn walk(paras: &mut [Paragraph], next: &mut u32) {
            for p in paras {
                p.header.instance_id = *next;
                *next += 1;
                for ctrl in &mut p.controls {
                    match ctrl {
                        Control::Table(t) => {
                            for cell in &mut t.cells {
                                walk(&mut cell.paragraphs, next);
                            }
                        }
                        Control::Generic(g) => {
                            for list in &mut g.paragraph_lists {
                                walk(&mut list.paragraphs, next);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        for section in &mut doc.sections {
            walk(&mut section.paragraphs, &mut next);
        }
    }

    #[test]
    fn clone_table_blank_구조보존_내용제거() {
        let mut doc = clone_test_doc();
        seed_instance_ids(&mut doc);
        let source_max = doc_max_instance_id(&doc);
        assert_eq!(
            clone_table(&mut doc, 0, "끝", CloneTextMode::Blank).unwrap(),
            1
        );

        let tables = all_tables(&doc);
        assert_eq!(tables.len(), 2);
        let (src, clone) = (tables[0], tables[1]);
        // Source untouched.
        assert_eq!(cell_text(&src.cells[2]), "1");
        // Geometry/merge topology preserved.
        assert_eq!((clone.rows, clone.cols), (src.rows, src.cols));
        assert_eq!(clone.row_cell_counts, src.row_cell_counts);
        for (a, b) in src.cells.iter().zip(&clone.cells) {
            assert_eq!(
                (a.col, a.row, a.col_span, a.row_span),
                (b.col, b.row, b.col_span, b.row_span)
            );
            assert_eq!(
                (a.width, a.height, a.border_fill),
                (b.width, b.height, b.border_fill)
            );
        }
        // Blank: one empty paragraph per cell, no text, no controls.
        for cell in &clone.cells {
            assert_eq!(cell.paragraphs.len(), 1);
            assert!(cell_text(cell).is_empty());
            assert!(cell.paragraphs[0].controls.is_empty());
        }
        // Instance ids remapped above the source document maximum.
        let clone_ids: Vec<u32> = clone
            .cells
            .iter()
            .map(|c| c.paragraphs[0].header.instance_id)
            .collect();
        assert!(clone_ids.iter().all(|&id| id > source_max));
        let mut dedup = clone_ids.clone();
        dedup.sort_unstable();
        dedup.dedup();
        assert_eq!(dedup.len(), clone_ids.len());
        // Source ids unchanged.
        assert!(
            src.cells
                .iter()
                .all(|c| c.paragraphs[0].header.instance_id <= source_max)
        );
    }

    #[test]
    fn clone_table_keep_내용보존_id재부여() {
        let mut doc = clone_test_doc();
        seed_instance_ids(&mut doc);
        let source_max = doc_max_instance_id(&doc);
        assert_eq!(
            clone_table(&mut doc, 0, "머리", CloneTextMode::Keep).unwrap(),
            1
        );

        let tables = all_tables(&doc);
        assert_eq!(tables.len(), 2);
        // The anchor "머리" precedes the source table, so the clone lands first.
        let (clone, src) = (tables[0], tables[1]);
        assert_eq!(cell_text(&clone.cells[2]), "1");
        assert_eq!(cell_text(&src.cells[2]), "1");
        assert!(
            clone
                .cells
                .iter()
                .all(|c| c.paragraphs[0].header.instance_id > source_max)
        );
    }

    #[test]
    fn clone_table_keep_opaque_개체_원자적_거부() {
        let mut doc = clone_test_doc();
        with_nth_table(&mut doc, 0, |t| {
            t.cells[0].paragraphs[0]
                .controls
                .push(Control::Generic(hwp_model::GenericControl {
                    ctrl_id: *b"eqed",
                    data: vec![1, 2, 3],
                    paragraph_lists: Vec::new(),
                    extras: Vec::new(),
                    raw_children: Vec::new(),
                    gso_shapes: Vec::new(),
                    equation: None,
                    column_def: None,
                    caption: None,
                    hwpx_raw_xml: None,
                    container_box: None,
                }));
        });
        let err = clone_table(&mut doc, 0, "끝", CloneTextMode::Keep).unwrap_err();
        assert!(err.contains("eqed"), "bounded ctrl_id error: {err}");
        // Atomic: nothing was inserted.
        assert_eq!(all_tables(&doc).len(), 1);
        // Blank mode strips the same control instead of failing.
        assert_eq!(
            clone_table(&mut doc, 0, "끝", CloneTextMode::Blank).unwrap(),
            1
        );
        let tables = all_tables(&doc);
        assert_eq!(tables.len(), 2);
        assert!(
            tables[1]
                .cells
                .iter()
                .all(|c| c.paragraphs.iter().all(|p| p.controls.is_empty()))
        );
    }

    #[test]
    fn clone_table_object_id_재부여() {
        let mut doc = clone_test_doc();
        // Pretend the source table came from hwp5: raw gso common with id 7 @32.
        with_nth_table(&mut doc, 0, |t| {
            t.common_data = vec![0u8; 44];
            t.common_data[32..36].copy_from_slice(&7u32.to_le_bytes());
        });
        assert_eq!(
            clone_table(&mut doc, 0, "끝", CloneTextMode::Blank).unwrap(),
            1
        );
        let tables = all_tables(&doc);
        assert_eq!(
            u32::from_le_bytes(tables[0].common_data[32..36].try_into().unwrap()),
            7,
            "source id untouched"
        );
        assert_eq!(
            u32::from_le_bytes(tables[1].common_data[32..36].try_into().unwrap()),
            8,
            "clone gets the next free id"
        );
        // A second clone keeps rising — no collision with either existing table.
        // (Both clones anchor after "끝", so the newer one lands ahead of the older.)
        assert_eq!(
            clone_table(&mut doc, 0, "끝", CloneTextMode::Blank).unwrap(),
            1
        );
        let tables = all_tables(&doc);
        assert_eq!(tables.len(), 3);
        let mut ids: Vec<u32> = tables
            .iter()
            .map(|t| u32::from_le_bytes(t.common_data[32..36].try_into().unwrap()))
            .collect();
        assert_eq!(ids[0], 7, "source id untouched");
        ids.sort_unstable();
        assert_eq!(ids, vec![7, 8, 9]);
    }

    #[test]
    fn clone_table_keep_캡션_중첩_개체_id_재부여() {
        let mut doc = clone_test_doc();
        // Table 0 gets a caption whose paragraph holds a nested hwp5-sourced
        // table (gso common id 7) — captions must join the id remap too.
        let inner = with_nth_table_readonly(&mut doc, 0, |t| t.clone()).unwrap();
        with_nth_table(&mut doc, 0, |t| {
            let mut inner = inner;
            inner.common_data = vec![0u8; 44];
            inner.common_data[32..36].copy_from_slice(&7u32.to_le_bytes());
            t.caption = Some(hwp_model::Caption {
                side: hwp_model::CaptionSide::Bottom,
                direction: hwp_model::CaptionDirection::Horizontal,
                gap: 283,
                width: None,
                last_width: 0,
                paragraphs: vec![Paragraph {
                    controls: vec![Control::Table(inner)],
                    ..Paragraph::default()
                }],
            });
        });
        assert_eq!(
            clone_table(&mut doc, 0, "끝", CloneTextMode::Keep).unwrap(),
            1
        );
        let tables = all_tables(&doc);
        // "끝" follows the source table, so order is [source, clone].
        let cap_id = |t: &hwp_model::Table| {
            let Control::Table(inner) = &t.caption.as_ref().unwrap().paragraphs[0].controls[0]
            else {
                panic!("caption nested table")
            };
            u32::from_le_bytes(inner.common_data[32..36].try_into().unwrap())
        };
        assert_eq!(cap_id(tables[0]), 7, "source caption object untouched");
        assert_eq!(cap_id(tables[1]), 8, "clone caption object remapped");
    }

    #[test]
    fn clone_table_중첩_인덱스와_오류() {
        let mut doc = clone_test_doc();
        // Nest a copy of table 0 inside its own first cell → indices 0 (outer), 1 (nested).
        let inner = with_nth_table_readonly(&mut doc, 0, |t| t.clone()).unwrap();
        with_nth_table(&mut doc, 0, |t| {
            t.cells[0].paragraphs[0]
                .controls
                .push(Control::Table(inner));
        });
        assert_eq!(all_tables(&doc).len(), 2);
        // Cloning index 1 clones the nested table, not the outer one.
        assert_eq!(
            clone_table(&mut doc, 1, "끝", CloneTextMode::Blank).unwrap(),
            1
        );
        let tables = all_tables(&doc);
        assert_eq!(tables.len(), 3);
        assert!(tables[2].cells[0].paragraphs[0].controls.is_empty());
        // Errors: bad index, missing anchor, empty anchor — document unchanged.
        let before = all_tables(&doc).len();
        assert!(clone_table(&mut doc, 99, "끝", CloneTextMode::Blank).is_err());
        assert!(clone_table(&mut doc, 0, "없는앵커", CloneTextMode::Blank).is_err());
        assert!(clone_table(&mut doc, 0, "", CloneTextMode::Blank).is_err());
        assert_eq!(all_tables(&doc).len(), before);
    }

    #[test]
    fn delete_object_표와_그림() {
        let mut doc = crate::from_markdown::from_markdown("본문\n\n| 가 |\n|---|\n| 1 |\n");
        let png = {
            let mut p = b"\x89PNG\r\n\x1a\n".to_vec();
            p.extend([0, 0, 0, 13]);
            p.extend(b"IHDR");
            p.extend(8u32.to_be_bytes());
            p.extend(8u32.to_be_bytes());
            p.extend([0u8; 8]);
            p
        };
        let tmp = std::env::temp_dir().join(format!(
            "delete_object_test_{}_{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&tmp, &png).unwrap();
        crate::image::insert_image(&mut doc, "본문", &tmp, crate::image::ImageSize::Natural)
            .unwrap();
        assert_eq!(
            delete_object(&mut doc, ObjectKind::Image, "본문").unwrap(),
            1
        );
        let has_pic = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .any(|c| matches!(c, Control::Picture(_)));
        assert!(!has_pic, "그림 삭제됨");
        let _ = std::fs::remove_file(&tmp);
        // Deleting the nth table — removing the 0-th (the GFM table) makes the table disappear.
        assert_eq!(
            delete_object(&mut doc, ObjectKind::TableNth(0), "").unwrap(),
            1
        );
        let has_table = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .any(|c| matches!(c, Control::Table(_)));
        assert!(!has_table, "표 삭제됨");
        // A missing index deletes 0.
        assert_eq!(
            delete_object(&mut doc, ObjectKind::TableNth(3), "").unwrap(),
            0
        );
    }

    #[test]
    fn delete_object_필드와_책갈피() {
        let mut doc = crate::from_markdown::from_markdown("여기 앵커입니다.\n");
        assert!(crate::field::create_field(&mut doc, "앵커", "이름", "값").unwrap());
        assert!(crate::bookmark::create_bookmark(&mut doc, "앵커", "책갈피1").unwrap());
        assert_eq!(
            delete_object(&mut doc, ObjectKind::Field, "이름").unwrap(),
            1
        );
        assert_eq!(
            delete_object(&mut doc, ObjectKind::Bookmark, "책갈피1").unwrap(),
            1
        );
        // No leftover fields/bookmarks may remain.
        assert!(crate::field::list_fields(&doc).is_empty());
        assert!(crate::bookmark::list_bookmarks(&doc).is_empty());
        // No leftover FIELD_END chars either.
        let stray = doc.sections[0].paragraphs.iter().any(|p| {
            p.chars.iter().any(|c| matches!(c, HwpChar::InlineCtrl { code, .. } if *code == hwp_model::ctrl_char::FIELD_END))
        });
        assert!(!stray, "FIELD_END 잔여");
    }
    use crate::from_markdown;
    use hwp_model::LineSeg;

    fn dummy_lineseg() -> LineSeg {
        LineSeg {
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
    fn 편집된_문단만_줄배치_무효화() {
        // Surgical editing clears only the edited paragraph's line layout, preserving unedited
        // caches so Hangul retains table-row heights, etc.
        let mut doc = from_markdown("바꿀문단 있음\n\n그대로 둘 문단\n");
        for p in &mut doc.sections[0].paragraphs {
            p.line_segs.push(dummy_lineseg());
        }
        let n = replace_text(&mut doc, "바꿀문단", "변경됨", true).unwrap();
        assert_eq!(n, 1);
        let paras = &doc.sections[0].paragraphs;
        let edited = paras
            .iter()
            .find(|p| p.plain_text().contains("변경됨"))
            .unwrap();
        let kept = paras
            .iter()
            .find(|p| p.plain_text().contains("그대로"))
            .unwrap();
        assert!(edited.line_segs.is_empty(), "편집 문단 줄 배치는 비워야 함");
        assert_eq!(kept.line_segs.len(), 1, "미편집 문단 줄 배치는 보존해야 함");
    }

    #[test]
    fn 본문_치환_길이변화_run보정() {
        let mut doc = from_markdown("부서명을 적으세요\n");
        let n = replace_text(&mut doc, "부서명", "기획팀입니다", true).unwrap();
        assert_eq!(n, 1);
        let text = doc.plain_text();
        assert!(text.contains("기획팀입니다을 적으세요"), "got: {text:?}");
        // Character-shape runs start at zero and increase monotonically.
        for section in &doc.sections {
            for p in &section.paragraphs {
                if let Some(first) = p.char_shape_runs.first() {
                    assert_eq!(first.0, 0, "첫 run은 0에서 시작");
                }
                let positions: Vec<u32> = p.char_shape_runs.iter().map(|r| r.0).collect();
                let mut sorted = positions.clone();
                sorted.sort_unstable();
                assert_eq!(positions, sorted, "run 위치 단조 증가");
            }
        }
    }

    #[test]
    fn 치환문이_찾기문_포함_무한루프_없음() {
        // "한라대학교" -> "제주한라대학교": replacement contains its search text; prevent infinite rematching.
        let mut doc = from_markdown("한라대학교 보고서\n");
        let n = replace_text(&mut doc, "한라대학교", "제주한라대학교", true).unwrap();
        assert_eq!(n, 1);
        let text = doc.plain_text();
        assert!(text.contains("제주한라대학교 보고서"), "got: {text:?}");
        assert!(!text.contains("제주제주"), "중복 치환됨: {text:?}");
    }

    #[test]
    fn 치환_전체_vs_단일() {
        let mut doc = from_markdown("가 가 가\n");
        let single = replace_text(&mut doc.clone(), "가", "나", false).unwrap();
        assert_eq!(single, 1);
        let all = replace_text(&mut doc, "가", "나", true).unwrap();
        assert_eq!(all, 3);
        assert!(doc.plain_text().contains("나 나 나"));
    }

    #[test]
    fn 표_셀_설정() {
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        set_cell(&mut doc, 0, 1, 0, "바뀐값").unwrap();
        let text = doc.plain_text();
        assert!(text.contains("바뀐값"), "got: {text:?}");
        // The cell contains exactly one paragraph with content plus paragraph end.
        assert!(set_cell(&mut doc, 0, 99, 99, "x").is_err());
        assert!(set_cell(&mut doc, 5, 0, 0, "x").is_err());
    }

    fn cell_at(t: &hwp_model::Table, row: u16, col: u16) -> &hwp_model::Cell {
        t.cells
            .iter()
            .find(|c| c.row == row && c.col == col)
            .expect("셀 없음")
    }

    #[test]
    fn set_cell_블록_분할_규칙() {
        // Normalize CRLF; split at two or more LF; trim only leading/trailing LF; discard empty
        // blocks.
        assert_eq!(split_cell_blocks("A\n\nB"), vec!["A", "B"]);
        assert_eq!(split_cell_blocks("A\r\n\r\nB"), vec!["A", "B"]);
        assert_eq!(split_cell_blocks("A\n\n\nB"), vec!["A", "B"]);
        assert_eq!(split_cell_blocks("A\n\n\n\n\nB"), vec!["A", "B"]);
        // One LF within a block remains an in-paragraph line break.
        assert_eq!(split_cell_blocks("A\nB"), vec!["A\nB"]);
        // An empty value or only blank lines yields one empty paragraph, retaining prior behavior.
        assert_eq!(split_cell_blocks(""), vec![""]);
        assert_eq!(split_cell_blocks("\n\n\n"), vec![""]);
    }

    #[test]
    fn set_cell_단일_블록은_문단_1개() {
        // Preserve behavior before block splitting: one block yields one paragraph with the
        // template instance_id.
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let before = cell_at(first_table(&doc), 1, 0).paragraphs[0]
            .header
            .instance_id;
        set_cell(&mut doc, 0, 1, 0, "한 줄\n두 줄").unwrap();
        let cell = cell_at(first_table(&doc), 1, 0);
        assert_eq!(cell.paragraphs.len(), 1, "단일 블록 → 문단 1개");
        assert_eq!(
            cell.paragraphs[0].header.instance_id, before,
            "문단 0은 템플릿 instance_id 유지"
        );
        assert_eq!(
            cell.paragraphs[0].header.chars_flags & 0x80,
            0x80,
            "마지막 문단 비트"
        );
        // In-paragraph line breaks remain LINE_BREAK controls.
        assert!(
            cell.paragraphs[0]
                .chars
                .iter()
                .any(|c| matches!(c, HwpChar::CharCtrl(hwp_model::ctrl_char::LINE_BREAK))),
            "단일 LF는 줄바꿈 문자"
        );
    }

    #[test]
    fn set_cell_빈_줄로_나뉜_값은_문단_여러_개() {
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        set_cell(&mut doc, 0, 1, 0, "첫째\n\n둘째\n\n셋째").unwrap();
        let cell = cell_at(first_table(&doc), 1, 0);
        assert_eq!(cell.paragraphs.len(), 3, "블록 3개 → 문단 3개");
        let texts: Vec<String> = cell
            .paragraphs
            .iter()
            .map(|p| {
                p.chars
                    .iter()
                    .filter_map(|c| match c {
                        HwpChar::Text(ch) => Some(*ch),
                        _ => None,
                    })
                    .collect()
            })
            .collect();
        assert_eq!(texts, vec!["첫째", "둘째", "셋째"], "블록 순서 보존");
        // B4: only the last paragraph carries bit31.
        for (i, p) in cell.paragraphs.iter().enumerate() {
            let last = i + 1 == cell.paragraphs.len();
            assert_eq!(
                p.header.chars_flags & 0x80 != 0,
                last,
                "문단 {i} 마지막 비트"
            );
        }
        // A8: unique instance_id per paragraph. Paragraph 0 inherits the template ID, zero for this
        // from_markdown fixture, so only new paragraphs 1..N require nonzero IDs.
        let ids: Vec<u32> = cell
            .paragraphs
            .iter()
            .map(|p| p.header.instance_id)
            .collect();
        assert!(
            ids[1..].iter().all(|id| *id != 0),
            "새 문단 id 비-0: {ids:?}"
        );
        // IDs must be unique document-wide, beyond the cell. Paragraph 0 inherits its template ID
        // and is the sole exception, so check only newly allocated IDs.
        let mut all = Vec::new();
        collect_instance_ids(&doc, &mut all);
        for id in &ids[1..] {
            assert_eq!(
                all.iter().filter(|other| *other == id).count(),
                1,
                "새 id {id}가 문서에서 유일하지 않다: {all:?}"
            );
        }
        // Leave line layouts empty for writer synthesis (B2/B3).
        assert!(cell.paragraphs.iter().all(|p| p.line_segs.is_empty()));
    }

    fn collect_instance_ids(doc: &Document, out: &mut Vec<u32>) {
        fn walk(paras: &[Paragraph], out: &mut Vec<u32>) {
            for p in paras {
                out.push(p.header.instance_id);
                for ctrl in &p.controls {
                    match ctrl {
                        Control::Table(t) => {
                            if let Some(cap) = &t.caption {
                                walk(&cap.paragraphs, out);
                            }
                            for cell in &t.cells {
                                walk(&cell.paragraphs, out);
                            }
                        }
                        Control::Picture(pic) => {
                            if let Some(cap) = &pic.caption {
                                walk(&cap.paragraphs, out);
                            }
                        }
                        Control::Generic(g) => {
                            if let Some(cap) = &g.caption {
                                walk(&cap.paragraphs, out);
                            }
                            for list in &g.paragraph_lists {
                                walk(&list.paragraphs, out);
                            }
                        }
                        Control::SectionDef(_) => {}
                    }
                }
            }
        }
        for section in &doc.sections {
            walk(&section.paragraphs, out);
        }
    }

    /// A8: allocating instance IDs beyond u32 would wrap to zero or collide with a live ID. Refuse
    /// atomically without changing anything when overflow would occur.
    #[test]
    fn set_cell_는_instance_id_넘침을_거절한다() {
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        // Place the document maximum at the u32 limit; no additional paragraph can be allocated.
        doc.sections[0].paragraphs[0].header.instance_id = u32::MAX;
        let before = doc.clone();
        let err = set_cell(&mut doc, 0, 1, 0, "첫째\n\n둘째").unwrap_err();
        assert!(err.contains("instance_id"), "{err}");
        assert_eq!(doc, before, "거절 시 문서 불변");

        // One remaining slot permits one new paragraph, i.e. two blocks.
        doc.sections[0].paragraphs[0].header.instance_id = u32::MAX - 1;
        set_cell(&mut doc, 0, 1, 0, "첫째\n\n둘째").unwrap();
        let cell = cell_at(first_table(&doc), 1, 0);
        assert_eq!(cell.paragraphs[1].header.instance_id, u32::MAX);

        // Two required slots, i.e. three blocks, are refused again.
        let before = doc.clone();
        let err = set_cell(&mut doc, 0, 1, 1, "첫째\n\n둘째\n\n셋째").unwrap_err();
        assert!(err.contains("instance_id"), "{err}");
        assert_eq!(doc, before, "거절 시 문서 불변");

        // A one-block value needs no new ID and succeeds independently of overflow.
        set_cell(&mut doc, 0, 1, 1, "한 문단").unwrap();
    }

    #[test]
    fn blank_para_like_는_ctrl_mask를_지운다() {
        // Inheriting the template control mask with an empty controls list declares nonexistent
        // controls and causes Hangul corruption.
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        {
            let t = doc.sections[0]
                .paragraphs
                .iter_mut()
                .flat_map(|p| &mut p.controls)
                .find_map(|c| match c {
                    Control::Table(t) => Some(t),
                    _ => None,
                })
                .expect("표 없음");
            let cell = t
                .cells
                .iter_mut()
                .find(|c| c.row == 1 && c.col == 0)
                .expect("셀 없음");
            cell.paragraphs[0].header.ctrl_mask = 0x0000_0800;
        }
        set_cell(&mut doc, 0, 1, 0, "A\n\nB").unwrap();
        let cell = cell_at(first_table(&doc), 1, 0);
        assert!(
            cell.paragraphs.iter().all(|p| p.header.ctrl_mask == 0),
            "새 문단이 템플릿 ctrl_mask를 물려받음"
        );
    }

    #[test]
    fn set_cell_반복_적용은_멱등() {
        // Setting the same value twice preserves paragraph text/count, an idempotency probe.
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        set_cell(&mut doc, 0, 1, 0, "A\n\nB").unwrap();
        let once = cell_at(first_table(&doc), 1, 0).clone();
        set_cell(&mut doc, 0, 1, 0, "A\n\nB").unwrap();
        let twice = cell_at(first_table(&doc), 1, 0);
        assert_eq!(twice.paragraphs.len(), once.paragraphs.len());
        assert_eq!(
            twice
                .paragraphs
                .iter()
                .map(|p| p.chars.clone())
                .collect::<Vec<_>>(),
            once.paragraphs
                .iter()
                .map(|p| p.chars.clone())
                .collect::<Vec<_>>()
        );
    }

    fn first_table(doc: &Document) -> &hwp_model::Table {
        doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("표 없음")
    }

    #[test]
    fn 행_추가_구조_불변식() {
        // A 2x2 table plus three rows has rows=5, cells=10, row_cell_counts length 5 and sum 10.
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let before = first_table(&doc);
        let (r0, cells0, cols) = (before.rows, before.cells.len(), before.cols);
        add_rows(&mut doc, 0, None, 3).unwrap();
        let t = first_table(&doc);
        assert_eq!(t.rows, r0 + 3, "rows 증가");
        assert_eq!(t.cells.len(), cells0 + 3 * cols as usize, "셀 수 증가");
        assert_eq!(
            t.row_cell_counts.len(),
            t.rows as usize,
            "row_cell_counts 길이 == rows"
        );
        assert_eq!(
            t.row_cell_counts.iter().map(|c| *c as usize).sum::<usize>(),
            t.cells.len(),
            "row_cell_counts 합 == 셀 수 (hwp5 extract assert)"
        );
        // New rows follow the old maximum row, retaining append-only row-major order.
        let rows_in_order: Vec<u16> = t.cells.iter().map(|c| c.row).collect();
        let mut sorted = rows_in_order.clone();
        sorted.sort_unstable();
        assert_eq!(rows_in_order, sorted, "cells 행 우선(단조 비감소) 순서");
        // Each new cell has one empty paragraph, one character-shape run (Hangul synthesis gate),
        // and span 1.
        for c in t.cells.iter().filter(|c| c.row >= r0) {
            assert_eq!(c.paragraphs.len(), 1, "새 셀 문단 1개");
            assert!(c.paragraphs[0].chars.is_empty(), "새 셀 비어 있음");
            assert_eq!(c.paragraphs[0].char_shape_runs.len(), 1, "문자모양 run 1개");
            assert!(c.paragraphs[0].line_segs.is_empty(), "줄 배치 무효화");
            assert_eq!((c.col_span, c.row_span), (1, 1), "병합 없음");
        }
    }

    #[test]
    fn 행_추가_후_채우기() {
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let r0 = first_table(&doc).rows; // Two rows: header plus data.
        add_rows(&mut doc, 0, None, 1).unwrap();
        // The new row index is r0; fill values there.
        set_cell(&mut doc, 0, r0, 0, "새값A").unwrap();
        set_cell(&mut doc, 0, r0, 1, "새값B").unwrap();
        let text = doc.plain_text();
        assert!(
            text.contains("새값A") && text.contains("새값B"),
            "got: {text:?}"
        );
    }

    #[test]
    fn 행_추가_서식_보존() {
        // Cell width/margins/borders/paragraph shapes are copied from the template row.
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let r0 = first_table(&doc).rows;
        let tpl: Vec<_> = {
            let t = first_table(&doc);
            t.cells
                .iter()
                .filter(|c| c.row == r0 - 1)
                .map(|c| (c.col, c.width, c.margins, c.border_fill))
                .collect()
        };
        add_rows(&mut doc, 0, None, 1).unwrap();
        let t = first_table(&doc);
        for (col, w, m, bf) in tpl {
            let nc = t
                .cells
                .iter()
                .find(|c| c.row == r0 && c.col == col)
                .expect("새 셀");
            assert_eq!(nc.width, w, "폭 보존");
            assert_eq!(nc.margins, m, "여백 보존");
            assert_eq!(nc.border_fill, bf, "테두리 보존");
        }
    }

    #[test]
    fn 행_추가_엣지케이스() {
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        // count=0 leaves everything unchanged.
        let before = first_table(&doc).rows;
        add_rows(&mut doc, 0, Some(0), 0).unwrap();
        assert_eq!(first_table(&doc).rows, before);
        // Missing table.
        assert!(add_rows(&mut doc, 9, None, 1).is_err());
        // Template row out of bounds.
        assert!(add_rows(&mut doc, 0, Some(99), 1).is_err());
    }

    #[test]
    fn 행_추가_u16_초과_거부() {
        // Refuse counts exceeding remaining u16 capacity instead of truncating and corrupting
        // records.
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let err = add_rows(&mut doc, 0, None, 70_000).unwrap_err();
        assert!(err.contains("u16"), "u16 범위 안내: {err}");
        // Refusal leaves the table unchanged.
        assert_eq!(first_table(&doc).rows, 2);
    }

    #[test]
    fn 행_추가_새문단_고유_instance_id_와_마지막비트() {
        // Cloned paragraphs need distinct nonzero instance_id values and nchars bit31 for their
        // final paragraph. The HWP5 edit path neither reassigns IDs nor sets this flag.
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        // Assign nonzero instance_id values to existing cell paragraphs, mimicking HWP5 input.
        for (i, c) in first_table_mut(&mut doc).cells.iter_mut().enumerate() {
            for p in &mut c.paragraphs {
                p.header.instance_id = (i as u32 + 1) * 100;
            }
        }
        add_rows(&mut doc, 0, None, 2).unwrap();
        let t = first_table(&doc);
        let new_paras: Vec<&Paragraph> = t
            .cells
            .iter()
            .filter(|c| c.row >= 2)
            .flat_map(|c| &c.paragraphs)
            .collect();
        assert_eq!(new_paras.len(), 4, "새 셀 4개(2행×2열)");
        let ids: Vec<u32> = new_paras.iter().map(|p| p.header.instance_id).collect();
        assert!(ids.iter().all(|&id| id != 0), "instance_id 비-0: {ids:?}");
        let mut uniq = ids.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), ids.len(), "instance_id 전부 고유: {ids:?}");
        for p in &new_paras {
            assert_ne!(p.header.chars_flags & 0x80, 0, "새 문단 nchars bit31");
        }
    }

    #[test]
    fn 행_추가_세로병합_덮인_부분행_템플릿_허용() {
        // #77: a template row partially covered by a vertical merge is now usable —
        // each column's style donor is the cell covering (tpl, col) in the logical
        // grid (the merge anchor for covered coordinates). Only styles are donated;
        // text is never cloned and spans are forced to 1x1.
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        {
            let t = first_table_mut(&mut doc);
            // Vertically merge (0,0) across two rows and remove cell (1,0). Row 1 then has only
            // cell (1,1), one anchor with cols=2.
            if let Some(c00) = t.cells.iter_mut().find(|c| c.row == 0 && c.col == 0) {
                c00.row_span = 2;
            }
            t.cells.retain(|c| !(c.row == 1 && c.col == 0));
            t.row_cell_counts = vec![2, 1];
        }
        add_rows(&mut doc, 0, Some(1), 1).unwrap();
        let t = first_table(&doc);
        assert_eq!(t.rows, 3);
        validate_table_invariants(t).unwrap();
        // New row 2 is fully tiled with blank styled 1x1 cells.
        let new: Vec<&hwp_model::Cell> = t.cells.iter().filter(|c| c.row == 2).collect();
        assert_eq!(new.len(), 2);
        for c in new {
            assert_eq!((c.col_span, c.row_span), (1, 1));
            assert_eq!(c.paragraphs.len(), 1);
            assert!(c.paragraphs[0].chars.is_empty(), "텍스트 미복제");
        }
    }

    /// Extract the text of a cell's first paragraph (test helper).
    fn cell_text(c: &hwp_model::Cell) -> String {
        c.paragraphs
            .first()
            .map(|p| {
                p.chars
                    .iter()
                    .filter_map(|ch| match ch {
                        HwpChar::Text(ch) => Some(*ch),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn 행_위치삽입_맨앞과_중간() {
        // #77: positioned insertion prepends at 0 and shifts existing rows down;
        // inserted rows are blank, original content moves with its row.
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        add_rows_at(&mut doc, 0, Some(0), 1, None).unwrap();
        let t = first_table(&doc);
        assert_eq!(t.rows, 3);
        validate_table_invariants(t).unwrap();
        let row0: Vec<&hwp_model::Cell> = t.cells.iter().filter(|c| c.row == 0).collect();
        assert_eq!(row0.len(), 2);
        assert!(
            row0.iter().all(|c| cell_text(c).is_empty()),
            "new row blank"
        );
        assert_eq!(
            cell_text(t.cells.iter().find(|c| c.row == 1 && c.col == 0).unwrap()),
            "가",
            "original header shifted to row 1"
        );
        // Middle: insert 2 rows before row 2 (now the "1 | 2" row).
        add_rows_at(&mut doc, 0, Some(2), 2, None).unwrap();
        let t = first_table(&doc);
        assert_eq!(t.rows, 5);
        validate_table_invariants(t).unwrap();
        for r in 2..=3u16 {
            let cells: Vec<_> = t.cells.iter().filter(|c| c.row == r).collect();
            assert_eq!(cells.len(), 2);
            assert!(cells.iter().all(|c| cell_text(c).is_empty()));
        }
        assert_eq!(
            cell_text(t.cells.iter().find(|c| c.row == 4 && c.col == 0).unwrap()),
            "1",
            "original data row shifted below the inserted band"
        );
        // `at == rows` is accepted as append.
        add_rows_at(&mut doc, 0, Some(5), 1, None).unwrap();
        assert_eq!(first_table(&doc).rows, 6);
        validate_table_invariants(first_table(&doc)).unwrap();
    }

    #[test]
    fn 행_위치삽입_세로병합_경계_확장() {
        // A vertical merge crossing the insertion boundary grows by `count` and no
        // new cell is created underneath it; other columns get blank styled cells.
        let mut doc =
            from_markdown("| 가 | 나 | 다 |\n|---|---|---|\n| 1 | 2 | 3 |\n| 4 | 5 | 6 |\n");
        {
            let t = first_table_mut(&mut doc);
            // (0,0) spans all 3 rows in column 0; remove the covered cells.
            if let Some(c00) = t.cells.iter_mut().find(|c| c.row == 0 && c.col == 0) {
                c00.row_span = 3;
                c00.height = hwp_model::HwpUnit(c00.height.0 * 3);
            }
            t.cells.retain(|c| !(c.col == 0 && c.row > 0));
            t.row_cell_counts = vec![3, 2, 2];
        }
        add_rows_at(&mut doc, 0, Some(1), 2, None).unwrap();
        let t = first_table(&doc);
        assert_eq!(t.rows, 5);
        validate_table_invariants(t).unwrap();
        let anchor = t.cells.iter().find(|c| c.row == 0 && c.col == 0).unwrap();
        assert_eq!(anchor.row_span, 5, "crossing span extended by count");
        // Merged cellSz = sum of covered row heights: the band adds
        // count x template-row height (1700 each here).
        assert_eq!(anchor.height.0, 1700 * 5, "span height covers the band");
        for r in 1..=2u16 {
            assert!(
                t.cells.iter().all(|c| !(c.row == r && c.col == 0)),
                "no cell under the covering span at row {r}"
            );
            let created: Vec<_> = t.cells.iter().filter(|c| c.row == r).collect();
            assert_eq!(created.len(), 2);
            assert!(created.iter().all(|c| cell_text(c).is_empty()));
        }
    }

    #[test]
    fn 행_위치삽입_가로병합_템플릿과_폭투영() {
        // Explicit merged template row: the horizontal-merge anchor donates styles
        // and its region width is projected back to the per-column width.
        let mut doc = from_markdown("| 가 | 나 | 다 |\n|---|---|---|\n| 1 | 2 | 3 |\n");
        merge_cells(&mut doc, 0, 0, 0, 0, 2).unwrap(); // header row: 3-wide merge
        // Give the merged anchor a non-empty LIST_HEADER tail (as hwp5-sourced
        // documents have) to prove the projected cells do not inherit it stale.
        {
            let t = first_table_mut(&mut doc);
            let anchor = t
                .cells
                .iter_mut()
                .find(|c| c.row == 0 && c.col == 0)
                .unwrap();
            anchor.header_tail = vec![0x5a; 12];
        }
        // Insert one row before row 1 using merged row 0 as the style template.
        add_rows_at(&mut doc, 0, Some(1), 1, Some(0)).unwrap();
        let t = first_table(&doc);
        assert_eq!(t.rows, 3);
        validate_table_invariants(t).unwrap();
        let new: Vec<&hwp_model::Cell> = t.cells.iter().filter(|c| c.row == 1).collect();
        assert_eq!(new.len(), 3, "merged donor projected to three 1x1 cells");
        assert!(new.iter().all(|c| cell_text(c).is_empty()));
        // Each new cell gets its own column's width (row 2 holds the original 1x1
        // cells, so it is the per-column reference).
        for c in &new {
            let reference = t
                .cells
                .iter()
                .find(|x| x.row == 2 && x.col == c.col)
                .expect("original row cell");
            assert_eq!(c.width, reference.width, "per-column width projection");
        }
        // The donor tail embeds the merged region's width; projected cells must
        // not carry it stale (the writer re-synthesizes one for the new width).
        assert!(
            new.iter().all(|c| c.header_tail.is_empty()),
            "stale donor tail cleared"
        );
    }

    #[test]
    fn 행_위치삽입_오류와_원자성() {
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let snapshot = first_table(&doc).clone();
        // Boundary beyond the row count.
        assert!(add_rows_at(&mut doc, 0, Some(3), 1, None).is_err());
        // Template row out of range.
        assert!(add_rows_at(&mut doc, 0, Some(1), 1, Some(9)).is_err());
        // u16 overflow of the total row count.
        assert!(add_rows_at(&mut doc, 0, Some(1), 70_000, None).is_err());
        // Unknown table index.
        assert!(add_rows_at(&mut doc, 9, None, 1, None).is_err());
        let after = first_table(&doc);
        assert_eq!(snapshot.rows, after.rows);
        assert_eq!(snapshot.cells.len(), after.cells.len());
        assert_eq!(snapshot.row_cell_counts, after.row_cell_counts);
    }

    #[test]
    fn 열_추가_카운트_위치와_오류() {
        // #77: counted, positioned column insertion preserves the total width.
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let total_before: i32 = first_table(&doc)
            .cells
            .iter()
            .filter(|c| c.row == 0)
            .map(|c| c.width.0)
            .sum();
        add_table_columns(&mut doc, 0, Some(1), 2).unwrap();
        let t = first_table(&doc);
        assert_eq!(t.cols, 4);
        validate_table_invariants(t).unwrap();
        let total_after: i32 = t
            .cells
            .iter()
            .filter(|c| c.row == 0)
            .map(|c| c.width.0)
            .sum();
        assert_eq!(total_before, total_after, "total width preserved");
        let blanks = t
            .cells
            .iter()
            .filter(|c| (c.col == 1 || c.col == 2) && cell_text(c).is_empty())
            .count();
        assert_eq!(blanks, 4, "two inserted columns x two rows, all blank");
        // Errors: zero count, boundary beyond the column count, u16 overflow.
        assert!(add_table_columns(&mut doc, 0, None, 0).is_err());
        assert!(add_table_columns(&mut doc, 0, Some(9), 1).is_err());
        assert!(add_table_columns(&mut doc, 0, None, u16::MAX).is_err());
        assert_eq!(
            first_table(&doc).cols,
            4,
            "failures leave the table untouched"
        );
    }

    #[test]
    fn 열_추가_카운트_병합표_스팬확장() {
        // Counted insertion through a horizontal merge: the crossing anchor grows
        // its col_span by `count`.
        let mut doc = from_markdown("| 가 | 나 | 다 |\n|---|---|---|\n| 1 | 2 | 3 |\n");
        merge_cells(&mut doc, 0, 0, 0, 0, 2).unwrap(); // header: (0,0)-(0,2)
        add_table_columns(&mut doc, 0, Some(1), 2).unwrap();
        let t = first_table(&doc);
        assert_eq!(t.cols, 5);
        validate_table_invariants(t).unwrap();
        let anchor = t.cells.iter().find(|c| c.row == 0 && c.col == 0).unwrap();
        assert_eq!(anchor.col_span, 5, "crossing merge extended by count");
        // Row 1 gets two new blank 1x1 cells at columns 1 and 2.
        for c in 1..=2u16 {
            let cell = t
                .cells
                .iter()
                .find(|x| x.row == 1 && x.col == c)
                .expect("new cell");
            assert_eq!((cell.col_span, cell.row_span), (1, 1));
            assert!(cell_text(cell).is_empty());
        }
    }

    #[test]
    fn 열_추가_최소폭_부족_거부() {
        // Each column must keep >= 1 HWP unit: a 2x2 table of unit-width cells
        // cannot take 2 more columns without silently growing the total width.
        let mut doc = width_table(&[&[1, 1], &[1, 1]]);
        let before = first_table(&doc).clone();
        let err = add_table_columns(&mut doc, 0, None, 2).unwrap_err();
        assert!(err.contains("부족"), "min-width guidance: {err}");
        let err = add_table_columns(&mut doc, 0, Some(0), 2).unwrap_err();
        assert!(err.contains("부족"), "min-width guidance: {err}");
        let after = first_table(&doc);
        assert_eq!(before.cols, after.cols);
        assert_eq!(before.cells.len(), after.cells.len());
    }

    /// Builds a simple-grid table with caller-specified cell widths per row.
    fn width_table(widths: &[&[i32]]) -> Document {
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let t = first_table_mut(&mut doc);
        let base = t.cells[0].clone();
        t.rows = widths.len() as u16;
        t.cols = widths[0].len() as u16;
        t.cells.clear();
        t.row_cell_counts.clear();
        for (r, row) in widths.iter().enumerate() {
            t.row_cell_counts.push(row.len() as u16);
            for (c, w) in row.iter().enumerate() {
                let mut cell = base.clone();
                cell.row = r as u16;
                cell.col = c as u16;
                cell.width = hwp_model::HwpUnit(*w);
                t.cells.push(cell);
            }
        }
        doc
    }

    #[test]
    fn 열_추가_구조_불변식() {
        // A 2x2 table plus one column has cols=3, six cells, row_cell_counts [3,3], row-major
        // order.
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let cells0 = first_table(&doc).cells.len();
        add_col(&mut doc, 0).unwrap();
        let t = first_table(&doc);
        assert_eq!(t.cols, 3);
        assert_eq!(t.cells.len(), cells0 + t.rows as usize);
        assert_eq!(t.row_cell_counts, vec![3, 3]);
        let rows_in_order: Vec<u16> = t.cells.iter().map(|c| c.row).collect();
        let mut sorted = rows_in_order.clone();
        sorted.sort_unstable();
        assert_eq!(rows_in_order, sorted, "행 우선 순서 유지");
        // Each cell in the appended last column has one empty paragraph.
        for c in t.cells.iter().filter(|c| c.col == 2) {
            assert_eq!(c.paragraphs.len(), 1);
            assert!(c.paragraphs[0].chars.is_empty());
        }
    }

    #[test]
    fn 열_추가_폭_합_정확보존() {
        // Every row's total width remains exact after insertion, with equal shares and residual in
        // the last cell.
        let mut doc = width_table(&[&[100, 50, 51], &[200, 200, 202]]);
        let before: Vec<i64> = (0..2)
            .map(|r| {
                first_table(&doc)
                    .cells
                    .iter()
                    .filter(|c| c.row == r)
                    .map(|c| i64::from(c.width.0))
                    .sum()
            })
            .collect();
        add_col(&mut doc, 0).unwrap();
        let t = first_table(&doc);
        for (r, expect) in before.iter().enumerate() {
            let sum: i64 = t
                .cells
                .iter()
                .filter(|c| c.row as usize == r)
                .map(|c| i64::from(c.width.0))
                .sum();
            assert_eq!(&sum, expect, "행 {r} 총폭 보존");
        }
        // New column width = row total width / (old column count+1).
        assert_eq!(
            t.cells
                .iter()
                .find(|c| c.row == 0 && c.col == 3)
                .unwrap()
                .width
                .0,
            201 / 4
        );
        // Every width is positive.
        assert!(t.cells.iter().all(|c| c.width.0 > 0));
    }

    #[test]
    fn 열_추가_병합표_지원() {
        // GK-2: merged tables also support added columns, replacing #9's former merged-table
        // refusal. After horizontally merging (0,0)..(0,1), append a column; retain the merge and
        // structural validity.
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        merge_cells(&mut doc, 0, 0, 0, 0, 1).unwrap(); // Merge the two header cells.
        add_col(&mut doc, 0).unwrap();
        let t = first_table(&doc);
        assert_eq!(t.cols, 3, "열 추가됨");
        // Retain anchor (0,0); total cell area equals rows*cols.
        assert!(
            t.cells
                .iter()
                .any(|c| c.row == 0 && c.col == 0 && c.col_span == 2)
        );
        assert_eq!(
            t.cells
                .iter()
                .map(|c| c.col_span as usize * c.row_span as usize)
                .sum::<usize>(),
            t.rows as usize * t.cols as usize
        );
        validate_table_invariants(t).unwrap();
    }

    #[test]
    fn 행_삭제_병합행_거부() {
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        {
            let t = first_table_mut(&mut doc);
            t.rows = 3;
            t.row_cell_counts = vec![2, 2, 1];
            // Vertical merge covering (2,0): row 1 anchor (1,0) has rowspan=2; row 2 has one cell
            // because it is covered.
            if let Some(c10) = t.cells.iter_mut().find(|c| c.row == 1 && c.col == 0) {
                c10.row_span = 2;
            }
            let mut c2 = t.cells[1].clone();
            c2.row = 2;
            c2.col = 1;
            t.cells.push(c2);
        }
        // Refuse deletion of covered row 2.
        let err = delete_table_row(&mut doc, 0, 2).unwrap_err();
        assert!(err.contains("병합"), "병합 행 거부 안내: {err}");
        // Clean row 0 can be deleted, but row 1 is refused because a rowspan starts at (1,0).
        let err1 = delete_table_row(&mut doc, 0, 1).unwrap_err();
        assert!(err1.contains("병합"));
        delete_table_row(&mut doc, 0, 0).unwrap();
        assert_eq!(first_table(&doc).rows, 2);
    }

    #[test]
    fn 표_연산_재귀_인덱싱() {
        // Nested tables use the same depth-first indices as set-cell for row/column operations.
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        // Insert a nested 1x1 table into outer cell (1,0).
        let inner = {
            let t = first_table(&doc);
            let mut inner = t.clone();
            inner.rows = 1;
            inner.cols = 1;
            inner.cells.truncate(1);
            let mut c = inner.cells[0].clone();
            c.row = 0;
            c.col = 0;
            inner.cells = vec![c];
            inner.row_cell_counts = vec![1];
            inner
        };
        {
            let t = first_table_mut(&mut doc);
            let cell = t
                .cells
                .iter_mut()
                .find(|c| c.row == 1 && c.col == 0)
                .unwrap();
            cell.paragraphs[0].controls.push(Control::Table(inner));
        }
        // Index 1 identifies the nested table in depth-first order; row insertion must use the same
        // index as set-cell.
        add_rows(&mut doc, 1, None, 1).unwrap();
        let outer = first_table(&doc);
        let inner_t = outer
            .cells
            .iter()
            .find(|c| c.row == 1 && c.col == 0)
            .and_then(|c| {
                c.paragraphs[0].controls.iter().find_map(|ct| match ct {
                    Control::Table(t) => Some(t),
                    _ => None,
                })
            })
            .expect("중첩 표");
        assert_eq!(inner_t.rows, 2, "중첩 표에 행 추가됨(재귀 인덱싱)");
    }

    fn first_table_mut(doc: &mut Document) -> &mut hwp_model::Table {
        doc.sections[0]
            .paragraphs
            .iter_mut()
            .flat_map(|p| &mut p.controls)
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("표 없음")
    }

    // Merge/split and column operations (GK-1/GK-2).

    fn table_3x3() -> Document {
        from_markdown("| a | b | c |\n|---|---|---|\n| d | e | f |\n| g | h | i |\n")
    }
    fn table_2x2() -> Document {
        from_markdown("| a | b |\n|---|---|\n| c | d |\n")
    }
    fn row_widths(t: &hwp_model::Table) -> Vec<i32> {
        (0..t.rows)
            .map(|r| {
                t.cells
                    .iter()
                    .filter(|c| c.row == r)
                    .map(|c| c.width.0)
                    .sum()
            })
            .collect()
    }

    #[test]
    fn 셀_병합_가로_기본() {
        let mut doc = table_2x2();
        let (w0, w1) = {
            let t = first_table(&doc);
            let g = |r, c| {
                t.cells
                    .iter()
                    .find(|x| x.row == r && x.col == c)
                    .unwrap()
                    .width
                    .0
            };
            (g(0, 0), g(0, 1))
        };
        merge_cells(&mut doc, 0, 0, 0, 0, 1).unwrap();
        let t = first_table(&doc);
        assert_eq!(t.cells.len(), 3, "피병합 셀 제거 → 4→3");
        let a = t.cells.iter().find(|c| c.row == 0 && c.col == 0).unwrap();
        assert_eq!((a.col_span, a.row_span), (2, 1));
        assert_eq!(a.width.0, w0 + w1, "병합 폭 = 구성 열 폭 합");
        assert_eq!(t.row_cell_counts, vec![1, 2]);
        let txt: String = a
            .paragraphs
            .iter()
            .flat_map(|p| p.chars.iter())
            .filter_map(|c| match c {
                HwpChar::Text(ch) => Some(*ch),
                _ => None,
            })
            .collect();
        assert!(
            txt.contains('a') && txt.contains('b'),
            "병합 내용 보존: {txt:?}"
        );
    }

    #[test]
    fn 셀_병합_세로() {
        let mut doc = table_3x3();
        merge_cells(&mut doc, 0, 0, 0, 2, 0).unwrap();
        let t = first_table(&doc);
        let a = t.cells.iter().find(|c| c.row == 0 && c.col == 0).unwrap();
        assert_eq!((a.col_span, a.row_span), (1, 3));
        assert_eq!(t.cells.len(), 7);
        assert_eq!(t.row_cell_counts, vec![3, 2, 2]);
    }

    #[test]
    fn 셀_병합_사각영역_면적불변() {
        let mut doc = table_3x3();
        merge_cells(&mut doc, 0, 0, 0, 1, 1).unwrap();
        let t = first_table(&doc);
        let a = t.cells.iter().find(|c| c.row == 0 && c.col == 0).unwrap();
        assert_eq!((a.col_span, a.row_span), (2, 2));
        assert_eq!(t.cells.len(), 6);
        assert_eq!(
            t.cells
                .iter()
                .map(|c| c.col_span as usize * c.row_span as usize)
                .sum::<usize>(),
            9,
            "면적 합=rows×cols"
        );
    }

    #[test]
    fn 셀_병합_부분겹침_범위밖_거부() {
        let mut doc = table_3x3();
        merge_cells(&mut doc, 0, 0, 0, 0, 1).unwrap();
        assert!(
            merge_cells(&mut doc, 0, 0, 1, 1, 1).is_err(),
            "부분 겹침 거부"
        );
        let mut doc2 = table_2x2();
        assert!(merge_cells(&mut doc2, 0, 0, 0, 5, 5).is_err(), "범위 밖");
        assert!(merge_cells(&mut doc2, 0, 1, 1, 1, 1).is_err(), "1셀 영역");
    }

    #[test]
    fn 셀_분할_병합_왕복() {
        let mut doc = table_3x3();
        merge_cells(&mut doc, 0, 0, 0, 1, 1).unwrap();
        split_cell(&mut doc, 0, 0, 0).unwrap();
        let t = first_table(&doc);
        assert_eq!(t.cells.len(), 9, "분할 후 9셀 복원");
        assert!(t.cells.iter().all(|c| c.col_span == 1 && c.row_span == 1));
        assert_eq!(t.row_cell_counts, vec![3, 3, 3]);
        for c in &t.cells {
            assert_eq!(
                c.paragraphs[0].char_shape_runs.len(),
                1,
                "빈 셀 run 1개(A7)"
            );
        }
    }

    #[test]
    fn 셀_분할_비병합_거부() {
        let mut doc = table_3x3();
        assert!(split_cell(&mut doc, 0, 1, 1).is_err(), "1×1 분할 불가");
        merge_cells(&mut doc, 0, 0, 0, 0, 1).unwrap();
        assert!(
            split_cell(&mut doc, 0, 0, 1).is_err(),
            "커버 위치는 앵커 아님"
        );
    }

    #[test]
    fn 열_추가_전체폭_유지() {
        // Preserve total table width, including per-row totals, exactly after adding a column,
        // retaining #9's tbl9 policy.
        let mut doc = table_3x3();
        let before = row_widths(first_table(&doc));
        add_table_column(&mut doc, 0, 1).unwrap();
        let t = first_table(&doc);
        assert_eq!(t.cols, 4);
        assert_eq!(t.cells.len(), 12);
        assert_eq!(t.row_cell_counts, vec![4, 4, 4]);
        assert_eq!(row_widths(t), before, "행별 총폭 정확 보존");
    }

    #[test]
    fn 열_추가_끝에_append() {
        let mut doc = table_3x3();
        let before = row_widths(first_table(&doc));
        add_col(&mut doc, 0).unwrap(); // Append at the end, the MCP/default CLI path.
        let t = first_table(&doc);
        assert_eq!(t.cols, 4);
        assert_eq!(t.cells.iter().filter(|c| c.col == 3).count(), 3);
        assert_eq!(row_widths(t), before, "append도 전체 폭 유지");
    }

    #[test]
    fn 열_추가_가로병합_확장() {
        let mut doc = table_3x3();
        merge_cells(&mut doc, 0, 0, 0, 0, 2).unwrap();
        add_table_column(&mut doc, 0, 1).unwrap();
        let t = first_table(&doc);
        assert_eq!(t.cols, 4);
        let a = t.cells.iter().find(|c| c.row == 0 && c.col == 0).unwrap();
        assert_eq!(a.col_span, 4, "삽입점 가로지르는 병합 확장");
        assert_eq!(t.row_cell_counts, vec![1, 4, 4]);
    }

    #[test]
    fn 열_삭제_기본_전체폭유지() {
        let mut doc = table_3x3();
        let before = row_widths(first_table(&doc));
        add_table_column(&mut doc, 0, 3).unwrap(); // Four columns.
        delete_table_column(&mut doc, 0, 1).unwrap(); // Back to three columns.
        let t = first_table(&doc);
        assert_eq!(t.cols, 3);
        assert_eq!(t.cells.len(), 9);
        assert_eq!(t.row_cell_counts, vec![3, 3, 3]);
        assert_eq!(row_widths(t), before, "열 추가+삭제 후 전체 폭 유지");
    }

    #[test]
    fn 열_삭제_병합축소_단일열제거() {
        // Shrink a merge.
        let mut doc = table_3x3();
        merge_cells(&mut doc, 0, 0, 0, 0, 2).unwrap();
        delete_table_column(&mut doc, 0, 1).unwrap();
        let t = first_table(&doc);
        assert_eq!(t.cols, 2);
        let a = t.cells.iter().find(|c| c.row == 0 && c.col == 0).unwrap();
        assert_eq!(a.col_span, 2, "병합 셀 축소");
        // Remove a single-column cell.
        let mut doc2 = table_3x3();
        delete_table_column(&mut doc2, 0, 1).unwrap();
        let t2 = first_table(&doc2);
        assert_eq!(t2.cols, 2);
        assert_eq!(t2.cells.len(), 6);
        // Refuse deleting the last column.
        let mut doc3 = from_markdown("| a |\n|---|\n| b |\n");
        assert!(
            delete_table_column(&mut doc3, 0, 0).is_err(),
            "마지막 열 거부"
        );
    }

    #[test]
    fn 불변식_연쇄조작() {
        let mut doc = table_3x3();
        merge_cells(&mut doc, 0, 0, 0, 1, 1).unwrap();
        add_table_column(&mut doc, 0, 0).unwrap();
        split_cell(&mut doc, 0, 0, 1).unwrap();
        delete_table_column(&mut doc, 0, 3).unwrap();
        validate_table_invariants(first_table(&doc)).unwrap();
    }
}

#[cfg(test)]
#[path = "edit_range_tag_tests.rs"]
mod edit_range_tag_tests;
