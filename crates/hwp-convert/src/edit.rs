//! 인메모리 IR 편집 프리미티브.
//!
//! 원본 문서를 읽어 메모리에서 텍스트/표 셀을 바꾼 뒤 다시 쓴다 — 이미지·opaque
//! 레코드 등 모든 비편집 데이터가 그대로 보존된다(JSON 파일 왕복과 달리 무손실).
//!
//! 편집된 문단은 줄 배치(PARA_LINE_SEG)·nchars·문단끝 0x0d 캐시가 낡으므로,
//! 쓸 때 반드시 writer의 합성 경로(hwp5: `WriteOptions.edited=true`)를 거쳐야
//! 한글이 수용한다. 이 모듈은 IR만 바꾸고, 불변식 재수립은 writer가 담당한다.

use hwp_model::{CharShapeId, Control, Document, HwpChar, Paragraph};

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
/// 셀의 첫 문단 서식을 템플릿으로 보존하고 내용만 교체한다.
pub fn set_cell(
    doc: &mut Document,
    table_index: usize,
    row: u16,
    col: u16,
    text: &str,
) -> Result<(), String> {
    with_nth_table(doc, table_index, |t| set_cell_in_table(t, row, col, text))
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
    if count == 0 {
        return Ok(());
    }
    with_nth_table(doc, table_index, |t| {
        add_rows_in_table(t, template_row, count)
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
/// 전체 표 폭은 유지된다([`add_table_column`]의 append 특수형). 병합 셀 표도 지원.
pub fn add_col(doc: &mut Document, table_index: usize) -> Result<(), String> {
    with_nth_table(doc, table_index, |t| {
        let at = t.cols;
        add_table_column_in_table(t, at)
    })
    .unwrap_or_else(|| Err(format!("표 #{table_index}를 찾을 수 없습니다")))
}

/// `table_index`번째 표의 at_col 위치(0-기반, 0..=cols)에 빈 열을 삽입한다. 삽입점을
/// 가로지르는 병합 셀은 col_span+1로 늘고, 그 외 행엔 빈 1×1 셀이 들어간다. **전체 표
/// 폭은 유지**(기존 열 비율 축소, 새 열=균등 몫, 정수 잔차는 마지막 열에 가산)한다.
pub fn add_table_column(doc: &mut Document, table_index: usize, at_col: u16) -> Result<(), String> {
    with_nth_table(doc, table_index, |t| add_table_column_in_table(t, at_col))
        .unwrap_or_else(|| Err(format!("표 #{table_index}를 찾을 수 없습니다")))
}

fn add_table_column_in_table(table: &mut hwp_model::Table, at_col: u16) -> Result<(), String> {
    let cols = table.cols;
    if at_col > cols {
        return Err(format!(
            "열 삽입 위치 {at_col}이 범위를 벗어남 (0..={cols})"
        ));
    }
    if table.cells.is_empty() || table.rows == 0 {
        return Err("빈 표에는 열을 추가할 수 없습니다".to_string());
    }
    if cols == u16::MAX {
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
        return add_col_append_uniform(table);
    }
    // 전체 폭 유지: 기존 열을 비율 축소하고 새 열이 균등 몫을 갖는다(#9 정책 계승).
    let colw = column_widths(table);
    let total: i64 = colw.iter().map(|&w| i64::from(w)).sum();
    if total <= 0 {
        return Err("표 총폭이 0이라 열 폭을 재분배할 수 없습니다".to_string());
    }
    let new_w = (total / (i64::from(cols) + 1)).max(1);
    let remain = total - new_w;
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
    // 최종 열 폭(길이 cols+1): 삽입 위치에 new_w 삽입.
    let mut final_colw: Vec<i64> = Vec::with_capacity(cols as usize + 1);
    final_colw.extend_from_slice(&scaled[..at_col as usize]);
    final_colw.push(new_w);
    final_colw.extend_from_slice(&scaled[at_col as usize..]);
    // 구조 갱신: 삽입점 가로지르는 병합 확장 + 이후 셀 이동.
    let rowh = row_heights(table);
    let tmpl = table.cells[0].clone();
    let mut rows_extended = vec![false; table.rows as usize];
    for c in &mut table.cells {
        let ac = c.col;
        let ec = c.col + c.col_span; // exclusive
        if ac >= at_col {
            c.col += 1;
        } else if at_col < ec {
            c.col_span += 1;
            for r in c.row..c.row + c.row_span {
                rows_extended[r as usize] = true;
            }
        }
    }
    // 확장에 덮이지 않은 행에 새 1×1 빈 셀.
    let mut next_inst = max_instance_id(table);
    for r in 0..table.rows {
        if !rows_extended[r as usize] {
            next_inst = next_inst.wrapping_add(1);
            table.cells.push(blank_cell(
                r,
                at_col,
                new_w as i32,
                rowh[r as usize],
                &tmpl,
                next_inst,
            ));
        }
    }
    table.cols += 1;
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

/// 병합 없는 표 **끝**에 열 추가 — #9 원본 알고리즘(행별 정확 폭 재분배). 각 행의 총폭을
/// 독립적으로 보존하므로 열 폭이 행마다 달라도(비그리드) 정확하다. 병합 표엔 쓰지 않는다
/// (열 정렬이 깨짐 — 그 경우는 [`add_table_column_in_table`]의 열 폭 기반 경로가 처리).
fn add_col_append_uniform(table: &mut hwp_model::Table) -> Result<(), String> {
    let cols = table.cols;
    let mut next_inst = max_instance_id(table);
    let mut new_cells = Vec::with_capacity(table.cells.len() + table.rows as usize);
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
        let new_w = (row_total / (i64::from(cols) + 1)).max(1);
        let scaled_target = row_total - new_w;
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
        let mut nc = row_cells[last_idx].clone();
        nc.col = cols;
        nc.width = hwp_model::HwpUnit(new_w as i32);
        let mut para = blank_para_like(row_cells.last().and_then(|c| c.paragraphs.first()));
        next_inst = next_inst.wrapping_add(1);
        para.header.instance_id = next_inst;
        nc.paragraphs = vec![para];
        new_cells.extend(row_cells);
        new_cells.push(nc);
    }
    table.cells = new_cells;
    table.cols += 1;
    for cnt in &mut table.row_cell_counts {
        *cnt += 1;
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

fn set_cell_in_table(
    table: &mut hwp_model::Table,
    row: u16,
    col: u16,
    text: &str,
) -> Result<(), String> {
    let cell = table
        .cells
        .iter_mut()
        .find(|c| c.row == row && c.col == col)
        .ok_or_else(|| format!("표에 셀 ({row}, {col})이 없습니다"))?;

    // 첫 문단을 서식 템플릿으로 — 문단/스타일/문자 모양/헤더 보존, 내용만 교체.
    let mut para = blank_para_like(cell.paragraphs.first());
    para.chars = text
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
    cell.paragraphs = vec![para];
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
    template_row: Option<u16>,
    count: usize,
) -> Result<(), String> {
    if table.rows == 0 {
        return Err("빈 표에는 행을 추가할 수 없습니다".to_string());
    }
    // 행 수는 u16 범위 — 남은 용량을 넘으면 거부(넘으면 count as u16 절단으로 cells/
    // row_cell_counts가 어긋나 표 레코드가 깨진다).
    let remaining = usize::from(u16::MAX) - usize::from(table.rows);
    if count > remaining {
        return Err(format!(
            "추가 행 수가 너무 많습니다: {count} (최대 {remaining}행 — 표 행 수는 u16 범위)"
        ));
    }
    // 템플릿 행 해소: 지정값(범위 검사) 또는 마지막의 '깨끗한'(병합 없는) 행.
    let tpl = match template_row {
        Some(r) if r < table.rows => r,
        Some(r) => {
            return Err(format!(
                "템플릿 행 {r}이 표 범위를 벗어남 (행 수: {})",
                table.rows
            ));
        }
        None => clean_template_row(table)
            .ok_or("복제할 병합 없는 행이 없습니다 — 템플릿 행을 지정하세요")?,
    };
    // 템플릿 행의 셀(열 순서) 수집. 병합 셀이 있거나 전 열을 채우지 않으면(세로 병합에
    // 덮인 부분 행) 거부 — 복제 시 그리드가 타일링되지 않아 누락 열이 생긴다.
    let tpl_cells: Vec<hwp_model::Cell> = table
        .cells
        .iter()
        .filter(|c| c.row == tpl)
        .cloned()
        .collect();
    if tpl_cells.is_empty() {
        return Err(format!("템플릿 행 {tpl}에 셀이 없습니다"));
    }
    // 템플릿 행은 전 열을 1×1로 채우는 깨끗한 행이어야 한다 — 병합 셀이 있거나
    // 세로 병합에 덮인 부분 행이면 복제 시 그리드가 타일링되지 않아 누락 열이 생긴다.
    if !is_clean_row(table, tpl) {
        return Err(format!(
            "템플릿 행 {tpl}에 병합 셀이 있거나 전체 열({})을 채우지 않아 복제 불가 — 병합 없는 행을 지정하세요",
            table.cols
        ));
    }
    // 복제 문단 instance_id 충돌 방지: hwp5 출신 편집 경로는 writer가 id를 재부여하지
    // 않으므로(synthesize=false), 표 내 최댓값 위로 고유 id를 부여한다(같은 템플릿
    // 문단을 N개 셀에 복제하면 비-0 id가 N+1개 중복돼 한글 개체 링크가 깨진다).
    let mut next_inst = table
        .cells
        .iter()
        .flat_map(|c| &c.paragraphs)
        .map(|p| p.header.instance_id)
        .max()
        .unwrap_or(0);
    // 새 행은 기존 최대 행 다음부터(행 우선 평탄 순서 유지 — append만, 중간 삽입 금지).
    let per_row = tpl_cells.len() as u16;
    let first_new = table.rows;
    for i in 0..count as u16 {
        for c in &tpl_cells {
            let mut nc = c.clone();
            nc.row = first_new + i;
            nc.col_span = 1;
            nc.row_span = 1;
            let mut para = blank_para_like(c.paragraphs.first());
            next_inst = next_inst.wrapping_add(1);
            para.header.instance_id = next_inst;
            nc.paragraphs = vec![para];
            table.cells.push(nc);
        }
    }
    table.rows += count as u16;
    for _ in 0..count {
        table.row_cell_counts.push(per_row);
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
    for section in &mut doc.sections {
        if let Some(i) = section
            .paragraphs
            .iter()
            .position(|p| find_match(&p.chars, anchor, 0).is_some())
        {
            section.paragraphs.insert(i + 1, anchor_para);
            return Ok(1);
        }
    }
    Err(format!("앵커를 찾을 수 없습니다: {anchor}"))
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
    fn 행_추가_세로병합_덮인_부분행_거부() {
        // 세로 병합에 덮여 전 열을 채우지 않는 행(셀 수 < cols)을 템플릿으로 지정하면
        // 거부해야 한다(복제 시 누락 열이 생겨 그리드가 깨짐).
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
        // 행 1은 부분 행 → 거부.
        let err = add_rows(&mut doc, 0, Some(1), 1).unwrap_err();
        assert!(err.contains("열"), "전 열 미충족 안내: {err}");
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
