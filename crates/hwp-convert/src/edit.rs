//! 인메모리 IR 편집 프리미티브.
//!
//! 원본 문서를 읽어 메모리에서 텍스트/표 셀을 바꾼 뒤 다시 쓴다 — 이미지·opaque
//! 레코드 등 모든 비편집 데이터가 그대로 보존된다(JSON 파일 왕복과 달리 무손실).
//!
//! 편집된 문단은 줄 배치(PARA_LINE_SEG)·nchars·문단끝 0x0d 캐시가 낡으므로,
//! 쓸 때 반드시 writer의 합성 경로(hwp5: `WriteOptions.edited=true`)를 거쳐야
//! 한글이 수용한다. 이 모듈은 IR만 바꾸고, 불변식 재수립은 writer가 담당한다.

use hwp_model::{CharShapeId, Control, Document, HwpChar, Paragraph};

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
/// Table indices use the same recursive document order as [`set_cell`]. This
/// intentionally calls the readonly table walker so discovery cannot clear
/// opaque HWPX XML or otherwise mutate the document. A label may address the
/// immediately adjacent cell, or the first data cell directly below a label in
/// a complete table header row. An empty or matching `{{label}}` cell is an
/// explicit form-value marker, so it keeps adjacent-form precedence even when
/// a later row makes the first row look like a complete header.
pub fn find_form_cells_by_label(
    doc: &mut Document,
    label: &str,
    table: Option<usize>,
) -> Vec<FormCellCandidate> {
    let wanted = normalize_form_label(label);
    if wanted.is_empty() {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    let mut table_index = 0usize;
    loop {
        let found = with_nth_table_readonly(doc, table_index, |current| {
            if table.is_some_and(|scope| scope != table_index) {
                return Vec::new();
            }
            let mut matches = Vec::new();
            let header_is_complete =
                current
                    .cells
                    .iter()
                    .filter(|cell| cell.row == 0)
                    .all(|cell| {
                        !normalize_form_label(
                            &cell
                                .paragraphs
                                .iter()
                                .map(Paragraph::plain_text)
                                .collect::<Vec<_>>()
                                .join("\n"),
                        )
                        .is_empty()
                    });
            for label_cell in &current.cells {
                let visible = label_cell
                    .paragraphs
                    .iter()
                    .map(Paragraph::plain_text)
                    .collect::<Vec<_>>()
                    .join("\n");
                if normalize_form_label(&visible) != wanted {
                    continue;
                }
                let right_col = label_cell.col.saturating_add(label_cell.col_span);
                let adjacent = current
                    .cells
                    .iter()
                    .find(|cell| cell.row == label_cell.row && cell.col == right_col);
                let header = if header_is_complete && label_cell.row == 0 {
                    let first_data_row = label_cell.row.saturating_add(label_cell.row_span);
                    current
                        .cells
                        .iter()
                        .find(|cell| cell.row == first_data_row && cell.col == label_cell.col)
                } else {
                    None
                };

                // A matching placeholder is part of the documented form-value
                // layout, not a second column heading. Prefer it before the
                // complete-header rule; this preserves the legacy adjacent form
                // contract without treating a multi-column header as a form.
                let target = match (adjacent, header) {
                    (Some(adjacent), Some(_)) if is_explicit_form_value(adjacent, &wanted) => {
                        Some(adjacent)
                    }
                    (_, Some(header)) => Some(header),
                    (Some(adjacent), None) => Some(adjacent),
                    (None, None) => None,
                };
                if let Some(target) = target {
                    matches.push(FormCellCandidate {
                        table: table_index,
                        row: target.row,
                        col: target.col,
                    });
                }
            }
            matches
        });
        let Some(found) = found else {
            break;
        };
        candidates.extend(found);
        table_index += 1;
    }
    candidates.sort_unstable();
    candidates.dedup();
    candidates
}

fn is_explicit_form_value(cell: &hwp_model::Cell, wanted: &str) -> bool {
    let visible = cell
        .paragraphs
        .iter()
        .map(Paragraph::plain_text)
        .collect::<Vec<_>>()
        .join("\n");
    let normalized = normalize_form_label(&visible);
    normalized.is_empty() || normalized == format!("{{{{{wanted}}}}}")
}

/// 문서 전체에서 `from`을 `to`로 치환한다(본문·표 셀·글상자 문단 재귀).
/// `all`이 거짓이면 첫 1건만 바꾼다. 반환값은 치환 횟수.
///
/// 한 문단의 연속된 일반 문자(Text) 안에서만 매칭한다 — 컨트롤 문자(표 앵커·
/// 문단끝 등)가 끼면 그 경계에서 매칭이 끊긴다(서식·구조 보존).
pub fn replace_text(doc: &mut Document, from: &str, to: &str, all: bool) -> usize {
    if from.is_empty() {
        return 0;
    }
    let mut budget = if all { usize::MAX } else { 1 };
    let mut count = 0;
    for section in &mut doc.sections {
        for para in &mut section.paragraphs {
            count += replace_in_para(para, from, to, &mut budget);
            if budget == 0 {
                return count;
            }
        }
    }
    count
}

fn replace_in_para(para: &mut Paragraph, from: &str, to: &str, budget: &mut usize) -> usize {
    let mut n = replace_in_chars(para, from, to, budget);
    for ctrl in &mut para.controls {
        if *budget == 0 {
            break;
        }
        match ctrl {
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        if *budget == 0 {
                            break;
                        }
                        n += replace_in_para(p, from, to, budget);
                    }
                }
            }
            Control::Generic(g) => {
                let before = n;
                for list in &mut g.paragraph_lists {
                    for p in &mut list.paragraphs {
                        if *budget == 0 {
                            break;
                        }
                        n += replace_in_para(p, from, to, budget);
                    }
                }
                if n > before {
                    // 내용이 바뀐 개체의 원문 XML은 낡았다 — stale 방출 금지.
                    g.hwpx_raw_xml = None;
                }
            }
            _ => {}
        }
    }
    n
}

/// 한 문단의 `chars` 안에서 치환을 반복한다(budget 한도). char_shape_run 위치를
/// 보정한다. 줄 배치는 비워 두고(낡음) writer가 재합성하게 한다.
fn replace_in_chars(para: &mut Paragraph, from: &str, to: &str, budget: &mut usize) -> usize {
    let from_w = utf16_len(from);
    let from_chars = from.chars().count();
    let to_chars = to.chars().count();
    let mut count = 0;
    // 삽입한 `to` 다음부터 이어서 탐색한다 — `to`가 `from`을 포함하면(예:
    // "한라대학교"→"제주한라대학교") 처음부터 재탐색 시 삽입한 텍스트 안에서
    // 다시 매칭돼 무한 루프에 빠진다.
    let mut start = 0usize;
    while *budget > 0 {
        let Some((char_idx, wpos)) = find_match(&para.chars, from, start) else {
            break;
        };
        let to_hwp: Vec<HwpChar> = to
            .chars()
            .map(|c| {
                if c == '\n' {
                    HwpChar::CharCtrl(hwp_model::ctrl_char::LINE_BREAK)
                } else {
                    HwpChar::Text(c)
                }
            })
            .collect();
        let to_w = utf16_len(to);
        para.chars.splice(char_idx..char_idx + from_chars, to_hwp);
        adjust_runs(&mut para.char_shape_runs, wpos, from_w, to_w);
        para.line_segs.clear();
        count += 1;
        *budget -= 1;
        start = char_idx + to_chars;
    }
    count
}

/// 연속된 Text 문자열에서 `start_idx` 이후 `from`의 첫 위치를 찾는다.
/// 반환: (chars 벡터 내 시작 인덱스, 문단 내 WCHAR 오프셋).
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

/// 치환 위치 `p`(WCHAR), 옛 길이 `lo`, 새 길이 `ln`에 맞춰 char_shape_run 경계를
/// 옮긴다. 치환 구간 내부 경계는 제거하고(치환 텍스트는 p에서 활성인 모양을 상속),
/// 이후 경계는 길이 변화만큼 평행 이동한다.
pub(crate) fn adjust_runs(runs: &mut Vec<(u32, CharShapeId)>, p: u32, lo: u32, ln: u32) {
    let delta = i64::from(ln) - i64::from(lo);
    let mut out: Vec<(u32, CharShapeId)> = Vec::with_capacity(runs.len());
    for &(pos, id) in runs.iter() {
        let np = if pos <= p {
            pos
        } else if pos >= p + lo {
            (i64::from(pos) + delta).max(0) as u32
        } else {
            continue; // 치환 구간 내부 경계 제거
        };
        match out.last() {
            Some(&(lp, _)) if lp == np => {}   // 같은 위치 중복 — 첫 것 유지
            Some(&(_, lid)) if lid == id => {} // 같은 모양 연속 — 잉여 경계 제거
            _ => out.push((np, id)),
        }
    }
    if out.is_empty() {
        out.push((0, CharShapeId::default()));
    }
    *runs = out;
}

/// `table_index`번째 표(문서 등장 순서, 0-기반)의 (row, col) 셀 텍스트를 바꾼다.
/// 셀의 첫 문단 서식을 템플릿으로 보존하고 내용만 교체한다. 빈 줄(LF 두 개 이상)로
/// 나뉜 값은 블록마다 문단 하나가 된다 — [`split_cell_blocks`] 참고.
pub fn set_cell(
    doc: &mut Document,
    table_index: usize,
    row: u16,
    col: u16,
    text: &str,
) -> Result<(), String> {
    // The hwp5-sourced edit path runs the writer with synthesize=false, so neither
    // assign_instance_ids nor set_last_para_flag runs and paragraphs created here
    // must carry their own document-unique ids. Read the global maximum before the
    // table borrow so the new ids cannot collide with a paragraph elsewhere.
    let next_instance_id = doc_max_instance_id(doc).wrapping_add(1).max(1);
    with_nth_table(doc, table_index, |t| {
        set_cell_in_table(t, row, col, text, next_instance_id)
    })
    .unwrap_or_else(|| Err(format!("표 #{table_index}를 찾을 수 없습니다")))
}

/// `table_index`번째 표(0-기반)에 빈 행을 `count`개 추가한다. `template_row`(0-기반,
/// 생략 시 마지막의 병합 없는 행)를 복제해 셀 서식(폭·여백·테두리·문자/문단 모양)을
/// 보존하고 내용은 비운다 — 추가된 행(인덱스 `기존행수`부터)은 이후 [`set_cell`]로
/// 채운다. hwp5 출력은 반드시 edited 합성 경로(`WriteOptions.edited=true`)를 거쳐야
/// 한글이 수용한다(줄 배치·문단끝·nchars 불변식 재합성).
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

/// `table_index`번째 표(0-기반)의 (행 수, 열 수)를 반환한다. 데이터 구동 채우기가
/// 추가할 행 수를 계산할 때 쓴다(현재 행 수 조회). 읽기 전용 — `hwpx_raw_xml`을 건드리지 않는다.
pub fn table_dims(doc: &mut Document, table_index: usize) -> Option<(u16, u16)> {
    with_nth_table_readonly(doc, table_index, |t| (t.rows, t.cols))
}

/// `table_index`번째 표(0-기반)의 `row`행을 삭제한다(이후 행 재번호, row_cell_counts
/// 갱신). 병합 셀이 있거나 세로 병합에 덮인 행은 그리드가 깨지므로 거부한다.
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

// ── 표 셀 병합/분할 · 열 추가/삭제 (GK-1 · GK-2) ─────────────────────────────
//
// 정답지 실측(정품 한글 1,816개 병합 표, hwp5+hwpx 만장일치)으로 확정한 저장 규칙을
// 유지한다:
//   1. 병합 영역은 **좌상단 앵커 셀 1개만** 저장하고 피병합(covered) 셀은 목록에서
//      완전히 생략한다(hwp5 LIST_HEADER·hwpx `<hp:tc>` 공통).
//   2. Σ(col_span×row_span) == rows×cols (앵커 셀들의 면적이 그리드를 정확히 타일링).
//   3. 셀 순서는 행 우선(앵커 (row,col) 사전식). 피병합 열은 그 행에서 건너뛴다.
//   4. row_cell_counts[r] = 앵커 row==r 셀 개수. row_span>1 셀은 앵커 행에만 계상.
//   5. 병합 셀 cellSz = 영역 전체 폭/높이(구성 열 폭 합·행 높이 합).
// 조작 후 [`validate_table_invariants`]로 재확인한다(깨지면 Err — 손상 표 미방출).
// 표 로케이터는 #9의 [`with_nth_table`](재귀·set-cell 인덱스 일치)을 공용한다.

/// 완전 미상 표(정상 표엔 안 나옴)의 폭 근사 기준 — A4 본문 폭 대략치.
const BODY_WIDTH_APPROX: i32 = 42520;
/// 빈 행 높이 근사 — 10pt 텍스트 + 셀 여백.
const ROW_HEIGHT_APPROX: i32 = 1700;

/// 논리 그리드(rows×cols): 각 위치를 소유한 셀 인덱스(피병합 위치도 앵커 인덱스로 채움).
/// 셀 겹침·빈칸·범위 초과가 있으면 Err(표 구조 파손 감지).
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

/// 그리드 열별 폭(HWPUNIT). 단일 열 셀에서 확정하고, 다중 열 셀만 걸친 열은 잔여를
/// 균등 분배하며, 그래도 미상이면 평균으로 근사한다.
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

/// 그리드 행별 높이(HWPUNIT). [`column_widths`]의 세로 대응.
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

/// 표 내 최대 instance_id (새 빈 문단에 고유 비-0 id 부여용 — [`add_rows`] 규칙과 동일).
fn max_instance_id(table: &hwp_model::Table) -> u32 {
    table
        .cells
        .iter()
        .flat_map(|c| &c.paragraphs)
        .map(|p| p.header.instance_id)
        .max()
        .unwrap_or(0)
}

/// 문단에 실제 텍스트(Text 문자)가 있는지 — 병합 시 빈 문단 정리에 쓴다.
fn has_text(p: &Paragraph) -> bool {
    p.chars.iter().any(|c| matches!(c, HwpChar::Text(_)))
}

/// 리스트 마지막 문단만 nchars bit31(chars_flags 0x80)을 세운다(B4 규칙).
fn fixup_last_para_flag(paras: &mut [Paragraph]) {
    let n = paras.len();
    for (i, p) in paras.iter_mut().enumerate() {
        if i + 1 == n {
            p.header.chars_flags |= 0x80;
        } else {
            p.header.chars_flags &= !0x80;
        }
    }
}

/// 빈 1×1 셀 — 템플릿에서 여백·테두리·list_attr·모양을 상속하고, 문단 1개·문자모양
/// run 1개·마지막 문단 비트([`blank_para_like`])·고유 instance_id를 채운다(A5~A7 게이트).
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

/// row_cell_counts를 셀 목록에서 재계산(앵커 row별 셀 수).
fn recount_rows(table: &mut hwp_model::Table) {
    let mut counts = vec![0u16; table.rows as usize];
    for c in &table.cells {
        if (c.row as usize) < counts.len() {
            counts[c.row as usize] += 1;
        }
    }
    table.row_cell_counts = counts;
}

/// 셀 목록을 행 우선(앵커 (row,col))으로 정렬(정품 저장 순서 불변식).
fn sort_cells_row_major(table: &mut hwp_model::Table) {
    table.cells.sort_by_key(|c| (c.row, c.col));
}

/// 표 불변식 재검증(조작 후 손상 방지 게이트). 위반이면 Err.
pub(crate) fn validate_table_invariants(table: &hwp_model::Table) -> Result<(), String> {
    build_grid(table)?; // 겹침·빈칸·범위
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

/// N번째 표에서 사각 영역 (r1,c1)-(r2,c2)를 병합한다(0-기반, 경계 포함). 좌상단 앵커가
/// span을 획득하고 피병합 셀의 문단 내용을 이어받으며, 피병합 셀은 목록에서 제거된다.
/// 영역이 기존 병합과 부분 겹침이거나 범위 밖이면 Err.
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

/// N번째 표의 (row,col) 앵커 셀(span>1)을 1×1 셀들로 분해한다. 앵커는 좌상단 위치와
/// 내용을 유지하고, 나머지 커버 위치엔 빈 셀을 만든다(A5~A7). cellSz 균등 분배.
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

/// `table_index`번째 표(0-기반) **끝에** 열을 하나 추가한다(mcp·기존 CLI 호환).
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
    build_grid(table)?; // 사전 검증
    // 병합 없는 표를 **끝에** 추가하는 경우는 #9의 행별 정확 재분배를 그대로 쓴다(각 행의
    // 총폭을 독립 보존 — 정품 그리드가 아닌 비균일 행도 정확). 병합 표·위치 삽입은 열
    // 정렬이 필요하므로 아래 열 폭 기반 경로로 처리한다.
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
    // 모든 셀 width = Σ final_colw[셀이 차지하는 열들] (전체·행 총폭 정확 보존).
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

/// `table_index`번째 표의 col 열(0-기반)을 삭제한다. 그 열을 가로지르는 병합 셀은
/// col_span−1로 줄고, 그 열에만 있던 1×1 셀은 제거된다. **전체 표 폭은 유지**(삭제 열
/// 폭을 남은 열에 비율로 재분배)한다. 마지막 1열이면 Err.
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
    build_grid(table)?; // 사전 검증
    let colw = column_widths(table);
    let total: i64 = colw.iter().map(|&w| i64::from(w)).sum();
    let remain_total = total - i64::from(colw[col as usize]);
    // 구조: 셀 이동/축소/제거.
    let mut to_remove: Vec<usize> = Vec::new();
    for (i, c) in table.cells.iter_mut().enumerate() {
        let ac = c.col;
        let ec = c.col + c.col_span; // exclusive
        if ac > col {
            c.col -= 1;
        } else if ec <= col {
            // 삭제 열 왼쪽 — 그대로.
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
    // 남은 열(옛 인덱스 col 제외) 새 폭: 삭제 열 폭을 비율로 흡수(전체 폭 유지).
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

/// `"키=값"` 메타데이터 지정을 문서에 적용한다. 키: `title`|`author`|`subject`|`keywords`.
/// 값이 비면 해당 필드를 `None`으로 지운다. 알 수 없는 키/형식은 `Err`.
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

/// 문서 등장 순서 `index`번째 표를 찾아 `f`를 적용한다(0-기반). 본문·표 셀·글상자
/// 문단을 재귀로 훑는다. 표를 찾으면 `Some(f의 결과)`, 못 찾으면 `None`.
/// `f`가 개체 원문 XML(`hwpx_raw_xml`)을 품은 Generic 안의 표를 바꾸면 그 원문은
/// 낡으므로 지운다 — writer가 stale XML을 방출하는 것을 막는다(방출할 emitter가
/// 없어 fail-closed 보존 오류로 이어진다).
fn with_nth_table<R, F: FnOnce(&mut hwp_model::Table) -> R>(
    doc: &mut Document,
    index: usize,
    f: F,
) -> Option<R> {
    walk_nth_table_root(doc, index, true, f)
}

/// 읽기 전용 변형([`table_dims`]) — 내용을 바꾸지 않으므로 `hwpx_raw_xml`을 지우지 않는다.
fn with_nth_table_readonly<R, F: FnOnce(&mut hwp_model::Table) -> R>(
    doc: &mut Document,
    index: usize,
    f: F,
) -> Option<R> {
    walk_nth_table_root(doc, index, false, f)
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
                                // 내용이 바뀐 개체의 원문 XML은 낡았다 — stale 방출 금지.
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

fn set_cell_in_table(
    table: &mut hwp_model::Table,
    row: u16,
    col: u16,
    text: &str,
    next_instance_id: u32,
) -> Result<(), String> {
    let cell = table
        .cells
        .iter_mut()
        .find(|c| c.row == row && c.col == col)
        .ok_or_else(|| format!("표에 셀 ({row}, {col})이 없습니다"))?;

    // 첫 문단을 서식 템플릿으로 — 문단/스타일/문자 모양/헤더 보존, 내용만 교체.
    // 셀의 문단 목록은 아래에서 통째로 교체되므로 템플릿을 미리 복제해 둔다.
    let template = cell.paragraphs.first().cloned();
    let mut paras: Vec<Paragraph> = split_cell_blocks(text)
        .into_iter()
        .enumerate()
        .map(|(i, block)| {
            let mut para = blank_para_like(template.as_ref());
            if i > 0 {
                // Paragraph 0 keeps the template's own instance id, so a
                // single-block value still produces the IR it produced before the
                // split; the rest take document-global ids because the surgical
                // writer will not assign them (see `set_cell`).
                para.header.instance_id = next_instance_id.wrapping_add(i as u32 - 1);
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

/// 표 행 추가/셀 설정용 빈 문단 — 템플릿 문단의 문단/스타일/첫 글자모양/헤더를
/// 보존하고 내용은 비운다(줄 배치도 비워 writer가 재합성). 한글 합성 게이트는
/// 셀당 문단 ≥1·문자모양 run ≥1만 요구하므로 빈 chars로 충분하다(writer가
/// nchars=1·PARA_TEXT 생략을 처리).
///
/// 이 문단은 항상 셀의 **유일·마지막** 문단이 되므로(set_cell·add_rows 모두
/// `cell.paragraphs = vec![이 문단]`), nchars bit31(리스트 마지막 문단 표식)을
/// 강제한다. hwp5 출신 편집 경로는 writer가 set_last_para_flag를 돌리지 않으므로
/// (synthesize=false) 여기서 세우지 않으면 다중 문단 셀을 복제할 때 비트가 빠진다.
fn blank_para_like(template: Option<&Paragraph>) -> Paragraph {
    let mut header = template.map(|p| p.header.clone()).unwrap_or_default();
    header.chars_flags |= 0x80;
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
    // 행 수는 u16 범위 — 남은 용량을 넘으면 거부(넘으면 count as u16 절단으로 cells/
    // row_cell_counts가 어긋나 표 레코드가 깨진다).
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
    // 복제 문단 instance_id 충돌 방지: hwp5 출신 편집 경로는 writer가 id를 재부여하지
    // 않으므로(synthesize=false), 표 내 최댓값 위로 고유 id를 부여한다(같은 템플릿
    // 문단을 N개 셀에 복제하면 비-0 id가 N+1개 중복돼 한글 개체 링크가 깨진다).
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

/// 행 r이 전 열을 1×1 셀로 채우는 '깨끗한' 행인지 — 병합 셀이 없고(row/col_span==1)
/// 세로 병합에 덮이지도 않음(row_cell_counts==cols). 행 복제·삭제·열 추가 가드 공용.
fn is_clean_row(table: &hwp_model::Table, r: u16) -> bool {
    table.row_cell_counts.get(r as usize).copied() == Some(table.cols)
        && table
            .cells
            .iter()
            .filter(|c| c.row == r)
            .all(|c| c.col_span == 1 && c.row_span == 1)
}

/// 복제 기본 템플릿: 마지막의 '깨끗한' 행 — 전 열을 채우고(row_cell_counts==cols)
/// 병합 셀이 없는 행. 세로 병합에 덮인 행은 셀 수가 cols보다 적어 자동 제외된다.
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
    Err(format!("앵커를 찾을 수 없습니다: {anchor}"))
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
            .map(|pl| 0x5000_0000 | (pl.z_order as u32 & 0xffff))
            .unwrap_or(0)
    })
}

/// Maximum gso object id across all tables and pictures in the document.
pub(crate) fn doc_max_object_id(doc: &Document) -> u32 {
    let mut max = 0u32;
    for section in &doc.sections {
        for para in &section.paragraphs {
            max_object_id_in_para(para, &mut max);
        }
    }
    max
}

fn max_object_id_in_para(para: &Paragraph, max: &mut u32) {
    for ctrl in &para.controls {
        match ctrl {
            Control::Table(t) => {
                *max = (*max).max(table_object_id(t));
                if let Some(cap) = &t.caption {
                    for p in &cap.paragraphs {
                        max_object_id_in_para(p, max);
                    }
                }
                for cell in &t.cells {
                    for p in &cell.paragraphs {
                        max_object_id_in_para(p, max);
                    }
                }
            }
            Control::Picture(pic) => {
                if let Some(id) = gso_common_instance_id(&pic.common_data) {
                    *max = (*max).max(id);
                }
                if let Some(cap) = &pic.caption {
                    for p in &cap.paragraphs {
                        max_object_id_in_para(p, max);
                    }
                }
            }
            Control::Generic(g) => {
                // gso containers carry the same gso common layout — include
                // their id so a clone never collides with a drawing either.
                if g.ctrl_id == *b"gso "
                    && let Some(id) = gso_common_instance_id(&g.data)
                {
                    *max = (*max).max(id);
                }
                for list in &g.paragraph_lists {
                    for p in &list.paragraphs {
                        max_object_id_in_para(p, max);
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
pub fn delete_object(doc: &mut Document, kind: ObjectKind, selector: &str) -> usize {
    let mut count = 0;
    let mut table_seen = 0usize;
    for section in &mut doc.sections {
        for para in &mut section.paragraphs {
            count += delete_object_in_para(para, kind, selector, &mut table_seen);
        }
    }
    count
}

fn delete_object_in_para(
    para: &mut Paragraph,
    kind: ObjectKind,
    selector: &str,
    table_seen: &mut usize,
) -> usize {
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
                        n += delete_object_in_para(p, kind, selector, table_seen);
                    }
                }
            }
            Control::Generic(g) => {
                let before = n;
                for list in &mut g.paragraph_lists {
                    for p in &mut list.paragraphs {
                        n += delete_object_in_para(p, kind, selector, table_seen);
                    }
                }
                if n > before {
                    // 내용이 바뀐 개체의 원문 XML은 낡았다 — stale 방출 금지.
                    g.hwpx_raw_xml = None;
                }
            }
            _ => {}
        }
    }
    if targets.is_empty() {
        return n;
    }
    n += remove_controls(para, &targets);
    n
}

/// Removes the controls at the `targets` indexes and their anchor chars (ExtCtrl + FIELD_END of
/// a field start), and adjusts char_shape_runs positions. Returns the number removed.
fn remove_controls(para: &mut Paragraph, targets: &[usize]) -> usize {
    // 1) Remove anchor chars — the ExtCtrl pointing at a target control + the field's FIELD_END.
    let mut removed: Vec<(u32, u32)> = Vec::new();
    let mut orig_pos = 0u32;
    let mut kept = Vec::with_capacity(para.chars.len());
    // Whether we are looking for the FIELD_END (code 4) matching a deleted FIELD_START (code 3).
    // Text/CharCtrl in between pass through untouched.
    let mut pending_field_end = false;
    for ch in std::mem::take(&mut para.chars) {
        let width = ch.wchar_width();
        let drop = match &ch {
            HwpChar::ExtCtrl {
                code, ctrl_index, ..
            } => {
                let hit = ctrl_index.is_some_and(|i| targets.contains(&(i as usize)));
                // Only a field start (FIELD_START=3) opens FIELD_END tracking — pictures, tables, and bookmarks have none.
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
        } else {
            kept.push(ch);
        }
        orig_pos += width;
    }
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
    n
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(delete_object(&mut doc, ObjectKind::Image, "본문"), 1);
        let has_pic = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .any(|c| matches!(c, Control::Picture(_)));
        assert!(!has_pic, "그림 삭제됨");
        let _ = std::fs::remove_file(&tmp);
        // Deleting the nth table — removing the 0-th (the GFM table) makes the table disappear.
        assert_eq!(delete_object(&mut doc, ObjectKind::TableNth(0), ""), 1);
        let has_table = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .any(|c| matches!(c, Control::Table(_)));
        assert!(!has_table, "표 삭제됨");
        // A missing index deletes 0.
        assert_eq!(delete_object(&mut doc, ObjectKind::TableNth(3), ""), 0);
    }

    #[test]
    fn delete_object_필드와_책갈피() {
        let mut doc = crate::from_markdown::from_markdown("여기 앵커입니다.\n");
        assert!(crate::field::create_field(&mut doc, "앵커", "이름", "값"));
        assert!(crate::bookmark::create_bookmark(
            &mut doc,
            "앵커",
            "책갈피1"
        ));
        assert_eq!(delete_object(&mut doc, ObjectKind::Field, "이름"), 1);
        assert_eq!(delete_object(&mut doc, ObjectKind::Bookmark, "책갈피1"), 1);
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
        // 외과적 편집: 편집한 문단의 줄 배치만 비우고, 미편집 문단은 보존해야
        // (한글이 표 행 높이 등을 그대로 유지하도록).
        let mut doc = from_markdown("바꿀문단 있음\n\n그대로 둘 문단\n");
        for p in &mut doc.sections[0].paragraphs {
            p.line_segs.push(dummy_lineseg());
        }
        let n = replace_text(&mut doc, "바꿀문단", "변경됨", true);
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
        let n = replace_text(&mut doc, "부서명", "기획팀입니다", true);
        assert_eq!(n, 1);
        let text = doc.plain_text();
        assert!(text.contains("기획팀입니다을 적으세요"), "got: {text:?}");
        // char_shape_run은 0에서 시작하고 단조 증가해야 한다.
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
        // "한라대학교" → "제주한라대학교": to가 from을 포함 → 재탐색 무한루프 방지.
        let mut doc = from_markdown("한라대학교 보고서\n");
        let n = replace_text(&mut doc, "한라대학교", "제주한라대학교", true);
        assert_eq!(n, 1);
        let text = doc.plain_text();
        assert!(text.contains("제주한라대학교 보고서"), "got: {text:?}");
        assert!(!text.contains("제주제주"), "중복 치환됨: {text:?}");
    }

    #[test]
    fn 치환_전체_vs_단일() {
        let mut doc = from_markdown("가 가 가\n");
        let single = replace_text(&mut doc.clone(), "가", "나", false);
        assert_eq!(single, 1);
        let all = replace_text(&mut doc, "가", "나", true);
        assert_eq!(all, 3);
        assert!(doc.plain_text().contains("나 나 나"));
    }

    #[test]
    fn 표_셀_설정() {
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        set_cell(&mut doc, 0, 1, 0, "바뀐값").unwrap();
        let text = doc.plain_text();
        assert!(text.contains("바뀐값"), "got: {text:?}");
        // 셀이 1개 문단(내용+문단끝)만 갖는지.
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
        // CRLF 정규화 · LF 2개 이상이 경계 · 앞뒤 LF만 제거 · 빈 블록 폐기.
        assert_eq!(split_cell_blocks("A\n\nB"), vec!["A", "B"]);
        assert_eq!(split_cell_blocks("A\r\n\r\nB"), vec!["A", "B"]);
        assert_eq!(split_cell_blocks("A\n\n\nB"), vec!["A", "B"]);
        assert_eq!(split_cell_blocks("A\n\n\n\n\nB"), vec!["A", "B"]);
        // 블록 안의 단일 LF는 문단 내 줄바꿈으로 남는다.
        assert_eq!(split_cell_blocks("A\nB"), vec!["A\nB"]);
        // 빈 값 · 빈 줄뿐인 값은 빈 문단 1개(기존 동작).
        assert_eq!(split_cell_blocks(""), vec![""]);
        assert_eq!(split_cell_blocks("\n\n\n"), vec![""]);
    }

    #[test]
    fn set_cell_단일_블록은_문단_1개() {
        // 분할 도입 전 동작 보존: 블록 1개면 문단 1개, 템플릿 instance_id 그대로.
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
        // 문단 내 줄바꿈은 LINE_BREAK로 남는다.
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
        // B4: 마지막 문단만 bit31.
        for (i, p) in cell.paragraphs.iter().enumerate() {
            let last = i + 1 == cell.paragraphs.len();
            assert_eq!(
                p.header.chars_flags & 0x80 != 0,
                last,
                "문단 {i} 마지막 비트"
            );
        }
        // A8: 문단마다 고유 instance_id. 문단 0은 템플릿 id를 물려받으므로(여기서는
        // from_markdown 문서라 0), 새로 만든 1..N만 비-0을 요구한다.
        let ids: Vec<u32> = cell
            .paragraphs
            .iter()
            .map(|p| p.header.instance_id)
            .collect();
        assert!(
            ids[1..].iter().all(|id| *id != 0),
            "새 문단 id 비-0: {ids:?}"
        );
        let mut uniq = ids.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), ids.len(), "instance_id 중복: {ids:?}");
        // 줄 배치는 비워 writer가 재합성한다(B2/B3).
        assert!(cell.paragraphs.iter().all(|p| p.line_segs.is_empty()));
    }

    #[test]
    fn set_cell_반복_적용은_멱등() {
        // 같은 값을 두 번 넣으면 문단 텍스트·개수가 그대로다(멱등성 프로브).
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
        // 2행 2열 표 → 3행 추가 → rows=5, cells=10, row_cell_counts 길이=5·합=10.
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
        // 새 행은 기존 최대 행 다음부터, 행 우선 평탄 순서 유지(append만).
        let rows_in_order: Vec<u16> = t.cells.iter().map(|c| c.row).collect();
        let mut sorted = rows_in_order.clone();
        sorted.sort_unstable();
        assert_eq!(rows_in_order, sorted, "cells 행 우선(단조 비감소) 순서");
        // 새 셀은 빈 문단 1개·문자모양 run 1개(한글 합성 게이트)·span 1.
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
        let r0 = first_table(&doc).rows; // 2 (헤더+데이터)
        add_rows(&mut doc, 0, None, 1).unwrap();
        // 새 행 인덱스 = r0, 거기에 값 채움.
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
        // 새 셀의 폭·여백·테두리·문단모양이 템플릿 행에서 복제되는지.
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
        // count=0은 무변경.
        let before = first_table(&doc).rows;
        add_rows(&mut doc, 0, Some(0), 0).unwrap();
        assert_eq!(first_table(&doc).rows, before);
        // 없는 표.
        assert!(add_rows(&mut doc, 9, None, 1).is_err());
        // 범위 밖 템플릿 행.
        assert!(add_rows(&mut doc, 0, Some(99), 1).is_err());
    }

    #[test]
    fn 행_추가_u16_초과_거부() {
        // count가 남은 u16 용량을 넘으면 절단 손상 대신 깔끔히 거부(레코드 깨짐 방지).
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let err = add_rows(&mut doc, 0, None, 70_000).unwrap_err();
        assert!(err.contains("u16"), "u16 범위 안내: {err}");
        // 표는 변경되지 않아야(거부 전 무변경).
        assert_eq!(first_table(&doc).rows, 2);
    }

    #[test]
    fn 행_추가_새문단_고유_instance_id_와_마지막비트() {
        // 복제 문단은 (1) 서로 다른 비-0 instance_id, (2) nchars bit31(마지막 문단)을
        // 가져야 한다 — hwp5 출신 편집 경로는 writer가 재부여/세팅하지 않으므로.
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        // 기존 셀 문단에 비-0 instance_id 부여(hwp5 출신 모사).
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
            // (0,0)을 세로 2행 병합으로, (1,0) 셀 제거 → 행 1은 (1,1)만(셀 1개, cols=2).
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

    /// 셀 폭을 원하는 대로 갖는 표를 만든다(행별 width 지정, 단순 그리드).
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
        // 2x2 표 → 열 추가 → cols=3, 셀 6개, row_cell_counts [3,3], 행 우선 순서.
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
        // 새 열(마지막 열) 셀은 빈 문단 1개.
        for c in t.cells.iter().filter(|c| c.col == 2) {
            assert_eq!(c.paragraphs.len(), 1);
            assert!(c.paragraphs[0].chars.is_empty());
        }
    }

    #[test]
    fn 열_추가_폭_합_정확보존() {
        // 행 총폭이 열 추가 전후로 정확히 일치해야 한다(균등 몫 + 잔차 마지막 셀).
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
        // 새 열 폭 = 행총폭/(기존열수+1).
        assert_eq!(
            t.cells
                .iter()
                .find(|c| c.row == 0 && c.col == 3)
                .unwrap()
                .width
                .0,
            201 / 4
        );
        // 모든 폭은 양수.
        assert!(t.cells.iter().all(|c| c.width.0 > 0));
    }

    #[test]
    fn 열_추가_병합표_지원() {
        // GK-2 통합: 병합 표도 열 추가를 지원한다(과거 #9의 '병합 거부'를 대체).
        // (0,0)-(0,1) 가로 병합 후 끝에 열 추가 → 병합 유지·구조 유효.
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        merge_cells(&mut doc, 0, 0, 0, 0, 1).unwrap(); // 헤더 2칸 병합
        add_col(&mut doc, 0).unwrap();
        let t = first_table(&doc);
        assert_eq!(t.cols, 3, "열 추가됨");
        // 병합 앵커(0,0)은 유지, 면적 합=rows×cols.
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
            // (2,0)을 덮는 세로 병합: (1,0) rowspan=2, 행 2는 셀 1개(덮인 행).
            if let Some(c10) = t.cells.iter_mut().find(|c| c.row == 1 && c.col == 0) {
                c10.row_span = 2;
            }
            let mut c2 = t.cells[1].clone();
            c2.row = 2;
            c2.col = 1;
            t.cells.push(c2);
        }
        // 덮인 행(2) 삭제 거부.
        let err = delete_table_row(&mut doc, 0, 2).unwrap_err();
        assert!(err.contains("병합"), "병합 행 거부 안내: {err}");
        // 깨끗한 행(0)은 삭제 가능… 단 (1,0) rowspan이 행1에서 시작 → 행1도 거부.
        let err1 = delete_table_row(&mut doc, 0, 1).unwrap_err();
        assert!(err1.contains("병합"));
        delete_table_row(&mut doc, 0, 0).unwrap();
        assert_eq!(first_table(&doc).rows, 2);
    }

    #[test]
    fn 표_연산_재귀_인덱싱() {
        // 중첩 표가 있으면 set-cell과 같은 깊이 우선 인덱스로 행/열 연산이 걸린다.
        let mut doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        // 바깥 표의 (1,0) 셀 안에 1x1 중첩 표 삽입.
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
        // 인덱스 1 = 중첩 표(깊이 우선). set-cell과 같은 번호로 행 추가가 걸려야 한다.
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

    // ── 병합/분할·열 조작 (GK-1·GK-2) ──────────────────────────────────

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
        // 전체 표 폭(행별 총폭)이 열 추가 후에도 정확히 보존돼야(#9 tbl9 정책 계승).
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
        add_col(&mut doc, 0).unwrap(); // 끝에 추가(mcp·기본 CLI 경로)
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
        add_table_column(&mut doc, 0, 3).unwrap(); // 4열
        delete_table_column(&mut doc, 0, 1).unwrap(); // 3열 복귀
        let t = first_table(&doc);
        assert_eq!(t.cols, 3);
        assert_eq!(t.cells.len(), 9);
        assert_eq!(t.row_cell_counts, vec![3, 3, 3]);
        assert_eq!(row_widths(t), before, "열 추가+삭제 후 전체 폭 유지");
    }

    #[test]
    fn 열_삭제_병합축소_단일열제거() {
        // 병합 축소.
        let mut doc = table_3x3();
        merge_cells(&mut doc, 0, 0, 0, 0, 2).unwrap();
        delete_table_column(&mut doc, 0, 1).unwrap();
        let t = first_table(&doc);
        assert_eq!(t.cols, 2);
        let a = t.cells.iter().find(|c| c.row == 0 && c.col == 0).unwrap();
        assert_eq!(a.col_span, 2, "병합 셀 축소");
        // 단일 열 셀 제거.
        let mut doc2 = table_3x3();
        delete_table_column(&mut doc2, 0, 1).unwrap();
        let t2 = first_table(&doc2);
        assert_eq!(t2.cols, 2);
        assert_eq!(t2.cells.len(), 6);
        // 마지막 열 거부.
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
