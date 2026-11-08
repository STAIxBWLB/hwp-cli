//! Structural editing: paragraph insertion and deletion.
//!
//! Create minimal paragraph/cell IR like set_cell: the writer idempotently supplies the
//! paragraph terminator, caches start empty, and character/paragraph shapes inherit from the anchor.
//! Structural edits require the synthesis path, as used by convert/new and checked in Hangul,
//! to establish terminators, last-paragraph bits and counts for inserted paragraphs/rows.
//!
//! Table row/column operations live in crate::edit and share its recursive table indices
//! with set-cell. This module contains paragraph-level editing only.

use hwp_model::{
    CharShapeId, Control, Document, GenericControl, HwpChar, ParaShapeId, Paragraph, StyleId,
    ctrl_char,
};

use crate::edit::find_match;
use crate::segment_id::SegmentPath;

/// Build a minimal paragraph from text with inherited character/paragraph shapes; empty text yields an empty paragraph.
fn make_paragraph(
    text: &str,
    para_shape: ParaShapeId,
    style: StyleId,
    char_shape: CharShapeId,
) -> Paragraph {
    let mut chars: Vec<HwpChar> = text
        .chars()
        .map(|c| {
            if c == '\n' {
                HwpChar::CharCtrl(ctrl_char::LINE_BREAK)
            } else {
                HwpChar::Text(c)
            }
        })
        .collect();
    if !chars.is_empty() {
        chars.push(HwpChar::CharCtrl(ctrl_char::PARA_BREAK));
    }
    Paragraph {
        para_shape,
        style,
        chars,
        char_shape_runs: vec![(0, char_shape)],
        line_segs: Vec::new(),
        ..Paragraph::default()
    }
}

/// Paragraph template: para_shape, style and first char_shape.
fn para_template(p: &Paragraph) -> (ParaShapeId, StyleId, CharShapeId) {
    (
        p.para_shape,
        p.style,
        p.char_shape_runs.first().map_or(CharShapeId(0), |r| r.1),
    )
}

/// Apply f to every paragraph list: visit the supplied list first, then recurse into
/// table captions/cells, picture captions, object paragraph lists and object captions
/// contained by its paragraphs. Skip section-definition controls.
///
/// A true result from f means the list changed. Visit every level without early exit,
/// combining results with OR; f owns any state needed to stop its own work. Changes
/// inside an object invalidate its captured hwpx_raw_xml, which must be cleared
/// so the writer cannot emit stale XML.
///
/// Do not enter objects with nonempty raw_children containing original HWP5 records.
/// The HWP5 emit_control writer re-emits their raw_children bytes and ignores their
/// paragraph_lists, so editing inner IR would leave the saved file unchanged. No IR
/// resynthesis path exists: gso resynthesis differed from genuine files and caused corruption
/// (rule E6). degrade_hwpx_gso supports only safe degradation that hoists text-box text into the body.
/// Skip these objects rather than report silent success. text_in_unwritable_object
/// separately diagnoses anchors inside them.
pub(crate) fn walk_para_lists(
    paras: &mut Vec<Paragraph>,
    f: &mut dyn FnMut(&mut Vec<Paragraph>) -> bool,
) -> bool {
    let hit = f(paras);
    hit | walk_nested_para_lists(paras, f)
}

/// Visit only lists nested below the supplied list, excluding the list itself from walk_para_lists.
/// Used by insert_paragraph's two-stage search: section bodies first, then nested lists.
fn walk_nested_para_lists(
    paras: &mut [Paragraph],
    f: &mut dyn FnMut(&mut Vec<Paragraph>) -> bool,
) -> bool {
    let mut hit = false;
    for para in paras.iter_mut() {
        for ctrl in &mut para.controls {
            match ctrl {
                Control::Table(t) => {
                    if let Some(cap) = &mut t.caption {
                        hit |= walk_para_lists(&mut cap.paragraphs, f);
                    }
                    for cell in &mut t.cells {
                        hit |= walk_para_lists(&mut cell.paragraphs, f);
                    }
                }
                Control::Picture(pic) => {
                    if let Some(cap) = &mut pic.caption {
                        hit |= walk_para_lists(&mut cap.paragraphs, f);
                    }
                }
                // The writer re-emits the original subtree; inner IR edits would be discarded.
                Control::Generic(g) if !g.raw_children.is_empty() => {}
                Control::Generic(g) => {
                    let mut inner = false;
                    for list in &mut g.paragraph_lists {
                        inner |= walk_para_lists(&mut list.paragraphs, f);
                    }
                    if let Some(cap) = &mut g.caption {
                        inner |= walk_para_lists(&mut cap.paragraphs, f);
                    }
                    if inner {
                        g.hwpx_raw_xml = None;
                    }
                    hit |= inner;
                }
                Control::SectionDef(_) => {}
            }
        }
    }
    hit
}

/// Check for text inside objects skipped by walk_para_lists, such as text boxes/headers
/// with original HWP5 records. When insertion/deletion changes nothing, distinguish a
/// missing anchor from an anchor inside an unwritable object.
pub fn text_in_unwritable_object(doc: &Document, text: &str) -> bool {
    fn in_list(paras: &[Paragraph], text: &str, inside_raw: bool) -> bool {
        if inside_raw
            && paras
                .iter()
                .any(|p| find_match(&p.chars, text, 0).is_some())
        {
            return true;
        }
        paras.iter().any(|p| {
            p.controls.iter().any(|ctrl| match ctrl {
                Control::Table(t) => {
                    t.caption
                        .as_ref()
                        .is_some_and(|c| in_list(&c.paragraphs, text, inside_raw))
                        || t.cells
                            .iter()
                            .any(|c| in_list(&c.paragraphs, text, inside_raw))
                }
                Control::Picture(pic) => pic
                    .caption
                    .as_ref()
                    .is_some_and(|c| in_list(&c.paragraphs, text, inside_raw)),
                Control::Generic(g) => {
                    let raw = inside_raw || !g.raw_children.is_empty();
                    g.paragraph_lists
                        .iter()
                        .any(|l| in_list(&l.paragraphs, text, raw))
                        || g.caption
                            .as_ref()
                            .is_some_and(|c| in_list(&c.paragraphs, text, raw))
                }
                Control::SectionDef(_) => false,
            })
        })
    }
    doc.sections
        .iter()
        .any(|s| in_list(&s.paragraphs, text, false))
}

/// Insert a text paragraph after or before the first paragraph containing anchor; return whether inserted.
/// The new paragraph inherits the anchor's character/paragraph shapes. Search body,
/// table-cell, nested-table and caption paragraphs.
///
/// Search in two stages: all section bodies in section order, then nested lists in
/// document order. Retain the priority used before nested search existed; a simple
/// preorder walk would let a section-0 cell beat a section-1 body, silently changing
/// which paragraph a previously working call selects.
pub fn insert_paragraph(doc: &mut Document, anchor: &str, text: &str, before: bool) -> bool {
    // Both stages share the closure, so keep completion state in Cell and capture only &done.
    let done = std::cell::Cell::new(false);
    let mut visit = |list: &mut Vec<Paragraph>| -> bool {
        if done.get() {
            return false;
        }
        let Some(i) = list
            .iter()
            .position(|p| find_match(&p.chars, anchor, 0).is_some())
        else {
            return false;
        };
        let (ps, sty, cs) = para_template(&list[i]);
        let new = make_paragraph(text, ps, sty, cs);
        let at = if before { i } else { i + 1 };
        list.insert(at, new);
        crate::edit::fixup_last_para_flag(list);
        done.set(true);
        true
    };
    for section in &mut doc.sections {
        visit(&mut section.paragraphs);
        if done.get() {
            return true;
        }
    }
    for section in &mut doc.sections {
        walk_nested_para_lists(&mut section.paragraphs, &mut visit);
        if done.get() {
            return true;
        }
    }
    false
}

/// Delete matching paragraphs from body, cell, nested-table and caption lists. Retain
/// at least one paragraph per list because Hangul treats empty sections/cells/captions
/// as corrupt; preserve section-definition paragraphs. Return the number deleted.
pub fn delete_paragraph(doc: &mut Document, matching: &str) -> usize {
    let mut count = 0usize;
    for section in &mut doc.sections {
        let mut visit = |list: &mut Vec<Paragraph>| -> bool {
            let mut removed = 0usize;
            let mut i = 0;
            while i < list.len() {
                let p = &list[i];
                let is_secd = p
                    .controls
                    .iter()
                    .any(|c| matches!(c, Control::SectionDef(_)));
                if !is_secd && list.len() > 1 && find_match(&p.chars, matching, 0).is_some() {
                    list.remove(i);
                    removed += 1;
                } else {
                    i += 1;
                }
            }
            if removed == 0 {
                return false;
            }
            crate::edit::fixup_last_para_flag(list);
            count += removed;
            true
        };
        walk_para_lists(&mut section.paragraphs, &mut visit);
    }
    count
}

// ── Address-driven structural primitives (EDT-05, Phase 7 plan 07-03) ──────────────────

/// Returns the mutable paragraph list containing the paragraph named by `path`, together with
/// that paragraph's own local index within it. Mirrors `address::paragraph_at_mut`'s descent
/// (section -> top-level paragraph -> table cell / generic-control paragraph list via `.get_mut()`
/// only, same `raw_children` skip) but stops one step short: structural ops need to splice the
/// *list* (insert/remove), not mutate the paragraph itself.
fn list_at_mut<'d>(
    doc: &'d mut Document,
    path: &SegmentPath,
) -> Option<(&'d mut Vec<Paragraph>, usize)> {
    let section = doc.sections.get_mut(path.section)?;
    let indices = &path.indices;
    if indices.is_empty() {
        return None;
    }
    if indices.len() == 1 {
        return Some((&mut section.paragraphs, indices[0]));
    }
    let mut para = section.paragraphs.get_mut(indices[0])?;
    let mut i = 1;
    loop {
        let control = para.controls.get_mut(indices[i])?;
        match control {
            Control::Table(table) => {
                let cell_index = *indices.get(i + 1)?;
                let cell = table.cells.get_mut(cell_index)?;
                let p_index = *indices.get(i + 2)?;
                if i + 3 == indices.len() {
                    return Some((&mut cell.paragraphs, p_index));
                }
                para = cell.paragraphs.get_mut(p_index)?;
                i += 3;
            }
            Control::Generic(generic) if generic.raw_children.is_empty() => {
                let seq = *indices.get(i + 1)?;
                if i + 2 == indices.len() {
                    return flat_list_at_mut(generic, seq);
                }
                para = flat_paragraph_step_mut(generic, seq)?;
                i += 2;
            }
            _ => return None,
        }
    }
}

/// The specific `ParagraphList` (and local index within it) that flat sequential index `seq`
/// falls into, across `generic.paragraph_lists` concatenated in order — mirrors
/// `address.rs::flat_paragraph`'s numbering, one level up (list, not paragraph).
fn flat_list_at_mut(
    generic: &mut GenericControl,
    seq: usize,
) -> Option<(&mut Vec<Paragraph>, usize)> {
    let mut remaining = seq;
    for list in &mut generic.paragraph_lists {
        if remaining < list.paragraphs.len() {
            return Some((&mut list.paragraphs, remaining));
        }
        remaining -= list.paragraphs.len();
    }
    None
}

/// The paragraph at flat index `seq`, for descending further below a generic control that is
/// not `path`'s final step. Same numbering as [`flat_list_at_mut`].
fn flat_paragraph_step_mut(generic: &mut GenericControl, seq: usize) -> Option<&mut Paragraph> {
    let mut remaining = seq;
    for list in &mut generic.paragraph_lists {
        if remaining < list.paragraphs.len() {
            return Some(&mut list.paragraphs[remaining]);
        }
        remaining -= list.paragraphs.len();
    }
    None
}

/// The list a path belongs to, as an equality key: section plus every index except the
/// paragraph's own trailing one. Two paths with the same identity name the same containing list
/// (planner decision 3's "list identity").
fn list_identity(path: &SegmentPath) -> (usize, &[usize]) {
    let n = path.indices.len();
    (path.section, &path.indices[..n.saturating_sub(1)])
}

/// Moves the paragraph at `from` to index `to_index` of the list identified by `to_list` (only
/// `to_list`'s containing list matters — its own trailing index is not used; the caller supplies
/// `to_index` directly, already folding in `before`/`after` positioning). The SAME `Paragraph`
/// value is removed and re-spliced, never cloned, so `instance_id` survives unchanged — this is
/// exactly what distinguishes a move from a copy. Do not call `reassign_para_ids` or any clone
/// helper here; those exist for `clone_table`'s deep-copy case, not for a move. A moved
/// paragraph's anchored controls (tables, pictures) travel with it automatically, since the
/// whole `Paragraph` value moves and `controls` is never touched.
///
/// Refuses when: the source paragraph carries `Control::SectionDef` (matching
/// `delete_paragraph`'s existing invariant — relocating a section's own definition paragraph is
/// exactly as unsafe as deleting it); the source list would be left empty; the destination list
/// lies inside the moved paragraph; or `to_index` lands beyond the destination list's length,
/// computed AFTER accounting for the source's removal when both paths name the same list
/// (T-07-11: a checked bound, never a silent clamp).
///
/// `to_list` names the destination as the document is before the move. When the destination list
/// is nested under a later paragraph of the source's list (a cell of a table further down), the
/// source's removal shifts that paragraph up by one, and the destination is found at its shifted
/// path. The other way round, when the source is nested under a paragraph of the destination list
/// at or after `to_index` (a cell paragraph moved out before its own table), the insertion shifts
/// the source's path, and the source list is fixed up at its shifted path (#358).
pub fn move_paragraph(
    doc: &mut Document,
    from: &SegmentPath,
    to_list: &SegmentPath,
    to_index: usize,
) -> Result<(), String> {
    let same_list = list_identity(from) == list_identity(to_list);

    {
        let (src_list, src_idx) = list_at_mut(doc, from)
            .ok_or_else(|| "이동할 문단 주소를 찾을 수 없습니다".to_string())?;
        let Some(p) = src_list.get(src_idx) else {
            return Err("이동할 문단 주소를 찾을 수 없습니다".to_string());
        };
        if p.controls
            .iter()
            .any(|c| matches!(c, Control::SectionDef(_)))
        {
            return Err("구역정의 문단은 이동할 수 없습니다".to_string());
        }
        if src_list.len() <= 1 {
            return Err("리스트에 문단이 하나뿐이면 이동할 수 없습니다".to_string());
        }
    }

    // Validated non-empty by `list_at_mut` above.
    let depth = from.indices.len() - 1;
    let mut dst = to_list.clone();
    if from.section == to_list.section
        && to_list.indices.len() > from.indices.len()
        && to_list.indices[..depth] == from.indices[..depth]
    {
        match to_list.indices[depth].cmp(&from.indices[depth]) {
            std::cmp::Ordering::Greater => dst.indices[depth] -= 1,
            std::cmp::Ordering::Equal => {
                return Err("문단을 그 문단 안의 위치로 이동할 수 없습니다".to_string());
            }
            std::cmp::Ordering::Less => {}
        }
    }

    let dst_len = {
        let (dst_list, _) = list_at_mut(doc, to_list)
            .ok_or_else(|| "이동 대상 위치를 찾을 수 없습니다".to_string())?;
        dst_list.len()
    };
    let effective_dst_len = if same_list { dst_len - 1 } else { dst_len };
    if to_index > effective_dst_len {
        return Err(format!(
            "이동 대상 인덱스가 리스트 범위를 벗어났습니다 (index={to_index}, len={effective_dst_len})"
        ));
    }

    let moved = {
        let (src_list, src_idx) = list_at_mut(doc, from).expect("validated above");
        src_list.remove(src_idx)
    };
    {
        let (dst_list, _) = list_at_mut(doc, &dst).expect("validated above");
        dst_list.insert(to_index, moved);
    }

    {
        let (list, _) = list_at_mut(doc, &dst).expect("just inserted into this list");
        crate::edit::fixup_last_para_flag(list);
    }
    let dst_depth = dst.indices.len() - 1;
    let mut src = from.clone();
    if !same_list
        && from.section == dst.section
        && from.indices.len() > dst.indices.len()
        && from.indices[..dst_depth] == dst.indices[..dst_depth]
        && from.indices[dst_depth] >= to_index
    {
        src.indices[dst_depth] += 1;
    }
    if !same_list {
        let (list, _) = list_at_mut(doc, &src).expect("source list still exists");
        crate::edit::fixup_last_para_flag(list);
    }

    crate::address::invalidate_ancestors(doc, &src);
    crate::address::invalidate_ancestors(doc, &dst);
    Ok(())
}

/// Inserts a paragraph before or after `at`, carrying caller-supplied shape ids rather than
/// inheriting the anchor's template (D-08: a creating op carries its own properties, since a
/// later op can never address content this one just created). Mirrors [`insert_paragraph`]'s
/// splice-and-fixup idiom; only the target-finding changes, from a text match to a resolved
/// address.
pub fn insert_paragraph_at(
    doc: &mut Document,
    at: &SegmentPath,
    before: bool,
    text: &str,
    shape: (ParaShapeId, StyleId, CharShapeId),
) -> Result<(), String> {
    let (ps, sty, cs) = shape;
    insert_paragraphs_at(doc, at, before, vec![make_paragraph(text, ps, sty, cs)])
}

/// Splice compiled paragraphs into the addressed list, preserving its last-paragraph flags
/// and invalidating ancestor caches through the same path as single-paragraph insertion.
pub fn insert_paragraphs_at(
    doc: &mut Document,
    at: &SegmentPath,
    before: bool,
    paragraphs: Vec<Paragraph>,
) -> Result<(), String> {
    if paragraphs.is_empty() {
        return Err("삽입할 문단이 없습니다. 하나 이상의 문단 블록을 지정하세요".into());
    }
    let (list, idx) = list_at_mut(doc, at)
        .filter(|(list, index)| *index < list.len())
        .ok_or_else(|| {
            "삽입 위치 주소를 찾을 수 없습니다. 최신 문서에서 문단 주소를 다시 얻어 보세요"
                .to_owned()
        })?;
    // The first section paragraph owns its section/column setup. Moving it later
    // makes the native writers synthesize or omit setup on the new first paragraph.
    if before
        && list[idx]
            .controls
            .iter()
            .any(|control| matches!(control, Control::SectionDef(_)))
    {
        return Err("구역 정의 문단 앞에는 삽입할 수 없습니다. insert_blocks는 position=after, insert_para는 before=false로 이 문단 뒤에 삽입하세요".into());
    }
    let at_idx = if before { idx } else { idx + 1 };
    list.splice(at_idx..at_idx, paragraphs);
    crate::edit::fixup_last_para_flag(list);
    crate::address::invalidate_ancestors(doc, at);
    Ok(())
}

/// Deletes the paragraph at `at`. Keeps `delete_paragraph`'s two invariants for the one
/// addressed paragraph: refuses on a section-definition paragraph, and refuses when its list
/// would be left empty (a section/cell/caption with zero paragraphs is corruption to Hancom, A6).
pub fn delete_paragraph_at(doc: &mut Document, at: &SegmentPath) -> Result<(), String> {
    let (list, idx) =
        list_at_mut(doc, at).ok_or_else(|| "삭제 위치 주소를 찾을 수 없습니다".to_string())?;
    let Some(p) = list.get(idx) else {
        return Err("삭제 위치 주소를 찾을 수 없습니다".to_string());
    };
    if p.controls
        .iter()
        .any(|c| matches!(c, Control::SectionDef(_)))
    {
        return Err("구역정의 문단은 삭제할 수 없습니다".to_string());
    }
    if list.len() <= 1 {
        return Err("리스트에 문단이 하나뿐이면 삭제할 수 없습니다".to_string());
    }
    list.remove(idx);
    crate::edit::fixup_last_para_flag(list);
    crate::address::invalidate_ancestors(doc, at);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::from_markdown;

    #[test]
    fn 문단_삽입_삭제() {
        let mut doc = from_markdown("첫째 문단\n\n둘째 문단\n\n셋째 문단");
        let n0: usize = doc.sections.iter().map(|s| s.paragraphs.len()).sum();
        // Insert after the second paragraph.
        assert!(insert_paragraph(&mut doc, "둘째", "삽입된 문단", false));
        let n1: usize = doc.sections.iter().map(|s| s.paragraphs.len()).sum();
        assert_eq!(n1, n0 + 1);
        let txt = doc.plain_text();
        assert!(txt.contains("삽입된 문단"));
        // Insertion position: between the second and third paragraphs.
        let i2 = txt.find("둘째").unwrap();
        let ii = txt.find("삽입된").unwrap();
        let i3 = txt.find("셋째").unwrap();
        assert!(i2 < ii && ii < i3, "둘째 뒤·셋째 앞: {txt:?}");
        // Delete.
        let d = delete_paragraph(&mut doc, "삽입된 문단");
        assert_eq!(d, 1);
        assert!(!doc.plain_text().contains("삽입된 문단"));
    }

    #[test]
    fn 마지막_문단은_안지움() {
        let mut doc = from_markdown("유일 문단");
        // Retain a sole body paragraph carrying secd, avoiding an empty section.
        let before: usize = doc.sections.iter().map(|s| s.paragraphs.len()).sum();
        delete_paragraph(&mut doc, "유일");
        let after: usize = doc.sections.iter().map(|s| s.paragraphs.len()).sum();
        assert_eq!(before, after, "최소 1문단 유지");
    }

    // #220: find anchors in cells, nested tables and captions too.

    fn first_table_mut(doc: &mut Document) -> &mut hwp_model::Table {
        doc.sections
            .iter_mut()
            .flat_map(|s| &mut s.paragraphs)
            .flat_map(|p| &mut p.controls)
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("표 없음")
    }

    fn cell_texts(doc: &Document, row: u16, col: u16) -> Vec<String> {
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
            .iter()
            .map(|p| p.plain_text())
            .collect()
    }

    #[test]
    fn 셀_안_앵커에_삽입된다() {
        let mut doc = from_markdown("본문\n\n| 가 | 나 |\n|----|----|\n| 표안앵커 | 2 |\n");
        assert!(insert_paragraph(&mut doc, "표안앵커", "셀 새 문단", false));
        let texts = cell_texts(&doc, 1, 0);
        assert_eq!(texts.len(), 2, "셀 문단 2개: {texts:?}");
        assert!(texts[1].contains("셀 새 문단"), "앵커 뒤 삽입: {texts:?}");
        // After insertion, bit31 belongs only to the list's last paragraph.
        let t = first_table_mut(&mut doc);
        let cell = t.cells.iter().find(|c| c.row == 1 && c.col == 0).unwrap();
        for (i, p) in cell.paragraphs.iter().enumerate() {
            let last = i + 1 == cell.paragraphs.len();
            assert_eq!(
                p.header.chars_flags & 0x80 != 0,
                last,
                "문단 {i} 마지막 비트"
            );
        }
    }

    #[test]
    fn 중첩_표_셀_안_앵커에_삽입된다() {
        let mut doc = from_markdown("본문\n\n| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let inner = {
            let mut d = from_markdown("| 중첩앵커 |\n|---|\n| x |\n");
            first_table_mut(&mut d).clone()
        };
        {
            let outer = first_table_mut(&mut doc);
            let cell = outer
                .cells
                .iter_mut()
                .find(|c| c.row == 1 && c.col == 1)
                .unwrap();
            cell.paragraphs[0].controls.push(Control::Table(inner));
        }
        assert!(insert_paragraph(
            &mut doc,
            "중첩앵커",
            "중첩 새 문단",
            false
        ));
        // plain_text does not traverse nested tables; check their structure instead.
        let inner_cell = {
            let outer = first_table_mut(&mut doc);
            let host = outer
                .cells
                .iter()
                .find(|c| c.row == 1 && c.col == 1)
                .unwrap();
            host.paragraphs
                .iter()
                .flat_map(|p| &p.controls)
                .find_map(|c| match c {
                    Control::Table(t) => Some(t),
                    _ => None,
                })
                .expect("중첩 표 없음")
                .cells
                .iter()
                .find(|c| c.row == 0 && c.col == 0)
                .expect("중첩 셀 없음")
                .clone()
        };
        let texts: Vec<String> = inner_cell
            .paragraphs
            .iter()
            .map(|p| p.plain_text())
            .collect();
        assert_eq!(texts.len(), 2, "중첩 셀 문단 2개: {texts:?}");
        assert!(texts[1].contains("중첩 새 문단"), "{texts:?}");
    }

    #[test]
    fn 캡션_안_앵커에_삽입된다() {
        let mut doc = from_markdown("본문\n\n| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        {
            let t = first_table_mut(&mut doc);
            let template = t.cells[0].paragraphs[0].clone();
            let mut cap_para = template.clone();
            cap_para.chars = "캡션앵커"
                .chars()
                .map(HwpChar::Text)
                .chain(std::iter::once(HwpChar::CharCtrl(ctrl_char::PARA_BREAK)))
                .collect();
            t.caption = Some(hwp_model::Caption {
                side: hwp_model::CaptionSide::Bottom,
                direction: hwp_model::CaptionDirection::Horizontal,
                gap: 0,
                width: None,
                last_width: 0,
                paragraphs: vec![cap_para],
            });
        }
        assert!(insert_paragraph(
            &mut doc,
            "캡션앵커",
            "캡션 새 문단",
            false
        ));
        let t = first_table_mut(&mut doc);
        let cap = t.caption.as_ref().unwrap();
        assert_eq!(cap.paragraphs.len(), 2, "캡션 문단 2개");
        assert!(cap.paragraphs[1].plain_text().contains("캡션 새 문단"));
    }

    #[test]
    fn 본문_문단이_셀보다_먼저_잡힌다() {
        // When both body and cell contain the anchor, the body wins, preserving the target
        // of calls that selected top-level paragraphs before this change.
        let mut doc = from_markdown("공통앵커\n\n| 가 | 나 |\n|----|----|\n| 공통앵커 | 2 |\n");
        assert!(insert_paragraph(&mut doc, "공통앵커", "새 문단", false));
        // The cell still contains one paragraph.
        assert_eq!(cell_texts(&doc, 1, 0).len(), 1, "셀은 안 바뀜");
        // Insert in the body.
        let body: Vec<String> = doc.sections[0]
            .paragraphs
            .iter()
            .map(|p| p.plain_text())
            .collect();
        assert!(
            body.iter().any(|t| t.contains("새 문단")),
            "본문에 삽입: {body:?}"
        );
    }

    #[test]
    fn 뒤_섹션_본문이_앞_섹션_셀보다_먼저_잡힌다() {
        // Before nested search existed, insert_paragraph visited only section bodies in section order.
        // A simple preorder walk would let a section-0 cell beat a section-1 body and silently
        // change the target of existing calls. The two-stage search retains that priority.
        let mut doc = from_markdown("본문\n\n| 가 | 나 |\n|----|----|\n| 공통앵커 | 2 |\n");
        let mut second = doc.sections[0].clone();
        let mut anchor_para = second.paragraphs[0].clone();
        anchor_para.controls.clear();
        anchor_para.chars = "공통앵커"
            .chars()
            .map(HwpChar::Text)
            .chain(std::iter::once(HwpChar::CharCtrl(ctrl_char::PARA_BREAK)))
            .collect();
        second.paragraphs = vec![second.paragraphs[0].clone(), anchor_para];
        doc.sections.push(second);

        assert!(insert_paragraph(&mut doc, "공통앵커", "새 문단", false));
        assert_eq!(cell_texts(&doc, 1, 0).len(), 1, "앞 섹션 셀은 안 바뀜");
        let body: Vec<String> = doc.sections[1]
            .paragraphs
            .iter()
            .map(|p| p.plain_text())
            .collect();
        assert!(
            body.iter().any(|t| t.contains("새 문단")),
            "섹션 1 본문에 삽입: {body:?}"
        );
    }

    #[test]
    fn 셀_안_문단도_지운다_단_마지막은_남는다() {
        // Only one of two cell paragraphs matches, so it is deleted.
        let mut doc = from_markdown("본문\n\n| 가 | 나 |\n|----|----|\n| 유지 | 2 |\n");
        insert_paragraph(&mut doc, "유지", "지울문단", false);
        assert_eq!(cell_texts(&doc, 1, 0).len(), 2);
        assert_eq!(delete_paragraph(&mut doc, "지울문단"), 1);
        assert_eq!(cell_texts(&doc, 1, 0).len(), 1, "하나만 지움");

        // Retain a sole matching cell paragraph; Hangul treats an empty cell as corrupt.
        assert_eq!(delete_paragraph(&mut doc, "유지"), 0, "마지막 문단 보존");
        assert_eq!(cell_texts(&doc, 1, 0).len(), 1);
    }

    /// Attach an object with an original HWP5 raw_children subtree. The writer re-emits
    /// those bytes unchanged, so inner IR edits would not be saved.
    fn attach_raw_object(doc: &mut Document, text: &str) {
        let template = doc.sections[0].paragraphs[0].clone();
        let mut inner = template.clone();
        inner.chars = text
            .chars()
            .map(HwpChar::Text)
            .chain(std::iter::once(HwpChar::CharCtrl(ctrl_char::PARA_BREAK)))
            .collect();
        doc.sections[0].paragraphs[0]
            .controls
            .push(Control::Generic(hwp_model::GenericControl {
                ctrl_id: *b"head",
                data: vec![0u8; 8],
                paragraph_lists: vec![hwp_model::ParagraphList {
                    header_data: Vec::new(),
                    paragraphs: vec![inner],
                }],
                extras: Vec::new(),
                // Nonempty means HWP5 source records, re-emitted unchanged when saving.
                raw_children: vec![hwp_model::OpaqueRecord {
                    tag: 0x0048,
                    data: Vec::new(),
                    children: Vec::new(),
                }],
                gso_shapes: Vec::new(),
                equation: None,
                column_def: None,
                caption: None,
                hwpx_raw_xml: None,
                container_box: None,
            }));
    }

    #[test]
    fn 원본보존_개체_안은_건드리지_않는다() {
        let mut doc = from_markdown("본문");
        attach_raw_object(&mut doc, "개체앵커");
        // Neither insertion nor deletion enters it; never claim silent success.
        assert!(!insert_paragraph(&mut doc, "개체앵커", "새 문단", false));
        assert_eq!(delete_paragraph(&mut doc, "개체앵커"), 0);
        // The diagnostic distinguishes missing from unwritable anchors.
        assert!(text_in_unwritable_object(&doc, "개체앵커"));
        assert!(!text_in_unwritable_object(&doc, "본문"));
        assert!(!text_in_unwritable_object(&doc, "어디에도 없는 문자열"));
        let g = doc.sections[0].paragraphs[0]
            .controls
            .iter()
            .find_map(|c| match c {
                Control::Generic(g) if g.ctrl_id == *b"head" => Some(g),
                _ => None,
            })
            .expect("개체 없음");
        assert_eq!(g.paragraph_lists[0].paragraphs.len(), 1, "안쪽 IR 불변");
    }

    // ── move_paragraph / insert_paragraph_at / delete_paragraph_at (07-03 Task 1) ─────────

    #[test]
    fn move_paragraph_문단_이동_id_보존() {
        let mut doc = from_markdown("첫 문단\n\n둘째 문단\n\n셋째 문단");
        doc.sections[0].paragraphs[2].header.instance_id = 777;
        let from = SegmentPath {
            section: 0,
            indices: vec![2],
        };
        let to_list = SegmentPath {
            section: 0,
            indices: vec![0],
        };
        move_paragraph(&mut doc, &from, &to_list, 0).unwrap();
        let n: usize = doc.sections.iter().map(|s| s.paragraphs.len()).sum();
        assert_eq!(n, 3, "길이 불변");
        let moved = &doc.sections[0].paragraphs[0];
        assert!(
            moved.plain_text().contains("셋째"),
            "맨 앞으로 이동: {:?}",
            moved.plain_text()
        );
        assert_eq!(moved.header.instance_id, 777, "instance_id 불변(복제 아님)");
    }

    #[test]
    fn move_paragraph_앵커_컨트롤도_함께_이동() {
        let mut doc = from_markdown("본문\n\n| 가 | 나 |\n|----|----|\n| 1 | 2 |\n\n마지막 문단");
        let table_para_idx = doc.sections[0]
            .paragraphs
            .iter()
            .position(|p| p.controls.iter().any(|c| matches!(c, Control::Table(_))))
            .expect("표를 가진 문단이 있어야 함");
        let from = SegmentPath {
            section: 0,
            indices: vec![table_para_idx],
        };
        let to_list = SegmentPath {
            section: 0,
            indices: vec![0],
        };
        move_paragraph(&mut doc, &from, &to_list, 0).unwrap();
        let moved = &doc.sections[0].paragraphs[0];
        assert!(
            moved
                .controls
                .iter()
                .any(|c| matches!(c, Control::Table(_))),
            "표 컨트롤이 이동한 문단에 그대로 있어야 함"
        );
    }

    #[test]
    fn delete_paragraph_at_단일_문단_리스트는_거부() {
        let mut doc = from_markdown("본문\n\n| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let top_idx = doc.sections[0]
            .paragraphs
            .iter()
            .position(|p| p.controls.iter().any(|c| matches!(c, Control::Table(_))))
            .expect("표를 가진 문단이 있어야 함");
        let ctrl_idx = doc.sections[0].paragraphs[top_idx]
            .controls
            .iter()
            .position(|c| matches!(c, Control::Table(_)))
            .expect("표 컨트롤 인덱스");
        let at = SegmentPath {
            section: 0,
            indices: vec![top_idx, ctrl_idx, 0, 0],
        };
        let err = delete_paragraph_at(&mut doc, &at).unwrap_err();
        assert!(err.contains("하나뿐"), "{err}");
    }

    #[test]
    fn insert_paragraph_at_전달된_모양을_적용한다() {
        let mut doc = from_markdown("이웃 문단");
        let at = SegmentPath {
            section: 0,
            indices: vec![0],
        };
        let shape = (ParaShapeId(7), StyleId(3), CharShapeId(5));
        insert_paragraph_at(&mut doc, &at, false, "새 문단", shape).unwrap();
        assert_eq!(doc.sections[0].paragraphs.len(), 2, "삽입됨");
        let inserted = &doc.sections[0].paragraphs[1];
        assert_eq!(inserted.para_shape, ParaShapeId(7), "전달된 para_shape");
        assert_eq!(inserted.style, StyleId(3), "전달된 style");
        assert_eq!(
            inserted.char_shape_runs[0].1,
            CharShapeId(5),
            "전달된 char_shape (이웃 상속 아님)"
        );
        assert_ne!(
            inserted.para_shape, doc.sections[0].paragraphs[0].para_shape,
            "이웃과 다른 값이어야 상속이 아님을 증명"
        );
    }
}
