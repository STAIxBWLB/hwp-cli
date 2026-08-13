//! 합성 문서(md/hwpx 출신)의 줄 배치(PARA_LINE_SEG) 합성.
//!
//! 5.1.x 한글은 본문 문단에 줄 배치 캐시가 없으면 글자를 0 높이로 그려
//! '검은 바'/'빈 내용'/'손상'으로 표시한다. 단순 합성(문단당 1줄)은 여러 줄
//! 문단·긴 표 셀에서 겹침/높이 붕괴를 일으킨다. 여기서는 폰트 셰이핑
//! (shape_range)으로 글자 폭을 측정해 본문 폭 기준 그리디 줄바꿈을 하고,
//! 줄 수만큼 PARA_LINE_SEG를 생성한다. v_pos는 섹션/셀 내 누적.
//!
//! 정확도의 핵심은 한글과 동일한 폰트(함초롬바탕)로 셰이핑하는 것이다.

use hwp_model::{Control, Document, LineSeg, Paragraph, Table};

use crate::fonts::FontStore;
use crate::issues::RenderIssueAccumulator;
use crate::layout::TAB_INTERVAL_PT;
use crate::shape::{InlineItem, shape_range};

/// 표 블록의 고정 세로 여유 (HWPUNIT). 정품 한글이 표 전체에 더하는 상수.
///
/// 정품 실측(첫째 문단입니다.hwp 5.1.1.0, work_report.hwp 5.0.2.4)에서 두
/// 파일 모두 `표 advance − Σ행높이 = 566`으로 일치한다(=2.0mm, 566.93 HWPUNIT).
/// 표 안쪽 위/아래 여백(상141·하141)·셀 좌우여백과 무관하게 같은 값이라,
/// 행 높이 합산과 별도로 표마다 한 번 더해지는 표 고유의 외곽 여유로 본다.
const TABLE_BLOCK_PADDING: i32 = 566;

/// 합성 문서 전체에 줄 배치를 합성한다 (본문·표 셀 문단).
/// `store`는 함초롬바탕이 로드된 FontStore여야 한글과 줄바꿈이 일치한다.
pub fn synthesize_linesegs(
    doc: &mut Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
) {
    let snap = doc.clone();
    for si in 0..doc.sections.len() {
        let page = snap.sections[si]
            .section_def()
            .and_then(|d| d.page.as_ref());
        let body_width = page.map_or(42520, |pg| {
            pg.width.0 - pg.margin_left.0 - pg.margin_right.0
        });
        // 페이지 본문 높이(상·하 여백 제외). 줄/표가 이 높이를 넘으면 다음 페이지로
        // 넘겨 v_pos를 0부터 다시 쌓는다 — 정품 멀티페이지는 페이지 상대 v_pos다
        // (정품 한라대 hwpx 실측: 본문 vertpos가 페이지마다 0으로 리셋, 최댓값
        // 59668 < 본문높이). 페이지 리셋 없이 단조 누적하면(섹션 누적) v_pos가
        // 페이지 높이를 한참 초과해(예: 354408) 한글이 '손상'으로 판정한다.
        let content_h = page
            .map_or(75686, |pg| {
                pg.height.0 - pg.margin_top.0 - pg.margin_bottom.0
            })
            .max(1);
        let mut v_pos = 0i32;
        for pi in 0..doc.sections[si].paragraphs.len() {
            // 문단 위/아래 간격(spacing_top/bottom)을 v_pos에 반영한다. 한글은 줄
            // 배치 v_pos로 문단 세로 위치를 그리므로, 간격이 빠지면 문단 사이 여백
            // 없이 압축돼 보인다(제목 위 여백 사라짐 등 '세로 위치 어긋남'의 원인).
            // 문단 사이 간격 = 앞 문단 아래 간격 + 이 문단 위 간격(가산). 단 섹션
            // 첫 문단의 위 간격은 페이지 상단이라 적용하지 않는다(정품: 첫 문단 v_pos=0).
            let (sp_top, sp_bottom) = snap
                .header
                .para_shapes
                .get(snap.sections[si].paragraphs[pi].para_shape.0 as usize)
                .map_or((0, 0), |ps| (ps.spacing_top, ps.spacing_bottom));
            if pi > 0 {
                v_pos += sp_top;
            }
            // 셀 안 문단 줄 배치를 먼저 채운다(셀 줄 수를 표 높이 계산이 읽어야 한다).
            fill_nested(si, pi, &snap, doc, store, warnings);
            // 이 문단의 표 총높이.
            let mut table_total = 0i32;
            for ctrl in &doc.sections[si].paragraphs[pi].controls {
                if let Control::Table(t) = ctrl {
                    table_total += table_height(t);
                }
            }
            // 표가 현재 페이지 잔여 공간에 안 들어가면 표 전체를 다음 페이지로 내린다.
            if table_total > 0 && v_pos > 0 && v_pos + table_total > content_h {
                v_pos = 0;
            }
            // 표 앵커 문단의 줄 배치는 진입 시점 커서(=직전 문단 누적 후)에 놓인다.
            // 정품 첫째문단.hwp: 본문 문단(advance 1600) → 표 앵커 문단 v_pos=1600.
            let anchor_v = v_pos;
            let src = &snap.sections[si].paragraphs[pi];
            let segs = compute_linesegs(
                store, &snap, src, body_width, content_h, &mut v_pos, warnings,
            );
            doc.sections[si].paragraphs[pi].line_segs = segs;
            // 표가 있는 문단은 한 줄(line_advance)이 아니라 표 높이만큼 커서를 내려야
            // 다음 본문 문단이 표와 겹치지 않는다(겹치면 한글이 '손상' 판정). 앵커
            // 문단은 compute_linesegs가 이미 line_advance를 1회 더했으므로, 커서를
            // 진입값 + 표 높이로 덮어쓴다(여러 표가 한 문단에 있으면 높이를 누적).
            if table_total > 0 {
                v_pos = anchor_v + table_total;
            }
            // 문단 아래 간격: 다음 문단 첫 줄을 그만큼 더 내린다.
            v_pos += sp_bottom;
        }
    }
}

/// 표 셀 안 문단에도 줄 배치를 합성한다 (셀 폭 기준, 셀마다 v_pos 리셋).
fn fill_nested(
    si: usize,
    pi: usize,
    snap: &Document,
    doc: &mut Document,
    store: &mut FontStore,
    warnings: &mut RenderIssueAccumulator,
) {
    let nctrl = doc.sections[si].paragraphs[pi].controls.len();
    for ci in 0..nctrl {
        // 글상자(hwpx-출신 Generic: gso_shapes 보유) 안 문단 — 박스 폭 기준,
        // 리스트마다 v_pos 리셋(박스 상대). hwp5-출신 글상자는 raw_children 원본이
        // 방출되므로 무관(문단 line_segs 이미 보유 시 건너뜀).
        if let Control::Generic(snap_g) = &snap.sections[si].paragraphs[pi].controls[ci] {
            if !snap_g.gso_shapes.is_empty() && !snap_g.paragraph_lists.is_empty() {
                // LIST_HEADER 안쪽 여백(283×2)을 뺀 본문 폭.
                let bw = (snap_g.gso_shapes[0].w - 566).max(1);
                let lists: Vec<usize> = snap_g
                    .paragraph_lists
                    .iter()
                    .map(|l| l.paragraphs.len())
                    .collect();
                for (li, &npara) in lists.iter().enumerate() {
                    let mut bv = 0i32;
                    for lpi in 0..npara {
                        let Control::Generic(snap_g) =
                            &snap.sections[si].paragraphs[pi].controls[ci]
                        else {
                            unreachable!();
                        };
                        let src = &snap_g.paragraph_lists[li].paragraphs[lpi];
                        if !src.line_segs.is_empty() {
                            continue;
                        }
                        let segs =
                            compute_linesegs(store, snap, src, bw, i32::MAX, &mut bv, warnings);
                        if let Control::Generic(g) =
                            &mut doc.sections[si].paragraphs[pi].controls[ci]
                        {
                            g.paragraph_lists[li].paragraphs[lpi].line_segs = segs;
                        }
                    }
                }
            }
            continue;
        }
        let Control::Table(snap_t) = &snap.sections[si].paragraphs[pi].controls[ci] else {
            continue;
        };
        // 셀별 (본문 폭, 문단 수)을 먼저 수집 (snap 불변 참조).
        let cells: Vec<(i32, usize)> = snap_t
            .cells
            .iter()
            .map(|c| {
                let w = (c.width.0 - i32::from(c.margins[0]) - i32::from(c.margins[1])).max(1);
                (w, c.paragraphs.len())
            })
            .collect();
        for (celli, &(cw, npara)) in cells.iter().enumerate() {
            let mut cv = 0i32;
            for cpi in 0..npara {
                let Control::Table(snap_t) = &snap.sections[si].paragraphs[pi].controls[ci] else {
                    unreachable!();
                };
                let csrc = &snap_t.cells[celli].paragraphs[cpi];
                // 셀 내부는 페이지 분할 안 함(content_h=MAX): 셀 줄 v_pos는 셀
                // 상대 누적이고, 페이지 넘침은 표 단위로 synthesize_linesegs가 처리.
                let segs = compute_linesegs(store, snap, csrc, cw, i32::MAX, &mut cv, warnings);
                if let Control::Table(t) = &mut doc.sections[si].paragraphs[pi].controls[ci] {
                    t.cells[celli].paragraphs[cpi].line_segs = segs;
                }
            }
        }
    }
}

/// 표 한 개의 세로 높이(HWPUNIT)를 정품 한글 규칙으로 계산한다.
///
/// 셀 안 문단의 줄 배치(line_segs)는 이 함수 호출 전에 fill_nested가 채워 둔다.
/// 정품 실측(첫째 문단입니다.hwp·work_report.hwp)으로 도출한 공식:
///
/// ```text
/// 행 높이 rowH = cell.top_margin + cell.bottom_margin + 줄블록
/// 줄블록(N줄) = (마지막 줄.v_pos) + (마지막 줄.line_height)
///            = (N-1)*line_advance + line_height
/// 표 높이 = Σ_행 max(rowH over 그 행의 셀) + TABLE_BLOCK_PADDING(566)
/// ```
///
/// 근거: 첫째문단.hwp(3행, 셀 1줄, 여백 상141/하141, lh=1000, la=1600)
/// → 3*(141+1000+141)+566 = 4412 = 정품 표 advance(6012−1600). work_report.hwp
/// 첫 표(1행 2열, 한 셀 2줄)도 (141+(3200+2000)+141)+566 = 5482+566 = 6048 = 정품
/// advance와 일치. 두 파일 모두 상수 566(=2.0mm)으로 떨어진다.
///
/// 병합 셀(row_span>1)은 시작 행 하나에만 높이를 싣지 않고 건너뛴다(각 행의
/// row_span==1 셀들로 행 높이를 잡는다 — 정품도 행 높이는 단일 행 셀 기준).
/// 행을 채우는 단일 행 셀이 하나도 없으면(전부 병합) 안전하게 폴백 높이를 쓴다.
fn table_height(table: &Table) -> i32 {
    // 행별 최대 셀 높이(rowH). 인덱스 = Cell.row.
    let mut row_heights = vec![0i32; usize::from(table.rows)];
    for cell in &table.cells {
        // 병합 셀은 시작 행에만 단일 높이를 강제하지 않는다(아래 폴백이 처리).
        if cell.row_span != 1 {
            continue;
        }
        let r = usize::from(cell.row);
        if r >= row_heights.len() {
            continue;
        }
        // 셀 안 모든 문단의 줄블록 합(여러 문단이면 누적). 마지막 줄은 line_height,
        // 그 위 줄들은 line_advance(=v_pos 증분)로 이미 v_pos에 반영돼 있다.
        let mut block = 0i32;
        for para in &cell.paragraphs {
            block += para_line_block(para);
        }
        let cell_h = i32::from(cell.margins[2]) + block + i32::from(cell.margins[3]);
        if cell_h > row_heights[r] {
            row_heights[r] = cell_h;
        }
    }
    // row_span>1 셀만 있는 행(높이 0)은 폴백 1줄 높이로 보정해 겹침을 막는다.
    let fallback = 141 + 1000 + 141; // 상여백 + 1줄(lh) + 하여백 (정품 기본)
    let sum: i32 = row_heights
        .iter()
        .map(|&h| if h > 0 { h } else { fallback })
        .sum();
    sum + TABLE_BLOCK_PADDING
}

/// 셀 안 문단 하나의 줄블록 높이(HWPUNIT) = 마지막 줄.v_pos + 마지막 줄.line_height.
/// 셀 v_pos는 셀 내부 0부터 누적되므로(fill_nested), 마지막 줄.v_pos가 곧
/// (줄수−1)*line_advance 이고 거기에 마지막 줄 높이를 더하면 문단 전체 세로 크기다.
fn para_line_block(para: &Paragraph) -> i32 {
    match para.line_segs.last() {
        Some(seg) => seg.v_pos + seg.line_height,
        // 줄 배치가 없으면(이론상 없음) 기본 1줄 높이로 폴백.
        None => 1000,
    }
}

/// 한 문단의 줄 배치를 계산한다. `v_pos`는 섹션/셀 내 세로 누적 커서.
/// 빈 문단도 줄 배치 1개를 가진다(한글 본문 표시 전제).
fn compute_linesegs(
    store: &mut FontStore,
    doc: &Document,
    para: &Paragraph,
    body_width: i32,
    content_h: i32,
    v_pos: &mut i32,
    warnings: &mut RenderIssueAccumulator,
) -> Vec<LineSeg> {
    // 줄 높이/간격은 문단 첫 글자 모양의 기준 크기에서 유도(정품 가나다 실측:
    // line_height=base, baseline_gap=base*0.85, line_spacing=base*0.6=줄간격 160%).
    let base = para
        .char_shape_runs
        .first()
        .and_then(|(_, id)| doc.header.char_shapes.get(id.0 as usize))
        .map_or(
            1000,
            |cs| if cs.base_size > 0 { cs.base_size } else { 1000 },
        );
    // 줄간격은 문단 모양에서 유도한다. 종류는 attr1 bits0-1(0 비율%, 1 고정, 3 최소),
    // 값은 line_spacing_old(@24). 길이 종류(고정/최소)는 HWPUNIT의 2배 단위라 ÷2.
    // 미지정이면 정품 기본 160%(가나다 실측). (예전엔 160% 고정이라 문단별
    // 줄간격(130/170 등)을 무시해 페이지네이션이 어긋났다.)
    let (line_advance, line_spacing) = {
        let ps = doc.header.para_shapes.get(para.para_shape.0 as usize);
        let ls_type = ps.map_or(0, |p| (p.attr1 & 0x3) as u8);
        let ls_val = ps.map_or(0, |p| p.line_spacing_old);
        let adv = match ls_type {
            1 | 3 if ls_val > 0 => (ls_val / 2).max(base),
            _ if ls_val > 0 => base * ls_val / 100,
            _ => base * 160 / 100,
        };
        (adv, (adv - base).max(0))
    };
    let baseline_gap = base * 85 / 100;
    let seg_width = body_width.max(1);
    let limit_pt = seg_width as f32 / 100.0;
    let total = para.wchar_len();

    let make = |start: u32, v: i32| LineSeg {
        text_start: start,
        v_pos: v,
        line_height: base,
        text_height: base,
        baseline_gap,
        line_spacing,
        col_start: 0,
        seg_width,
        flags: 0x0006_0000,
    };

    // 한 줄을 배치하고 커서를 진행한다. 줄이 페이지 본문 높이를 넘으면 다음 페이지
    // 상단(v_pos=0)부터 다시 쌓는다(정품 멀티페이지 = 페이지 상대 v_pos). 셀 내부는
    // content_h=MAX로 호출돼 리셋이 일어나지 않는다.
    let place = |segs: &mut Vec<LineSeg>, v_pos: &mut i32, start: u32| {
        if *v_pos > 0 && *v_pos + base > content_h {
            *v_pos = 0;
        }
        segs.push(make(start, *v_pos));
        *v_pos += line_advance;
    };

    // 폰트 셰이핑으로 글자 폭을 재고, 본문 폭 기준 그리디 줄바꿈.
    // place_wrapped(layout.rs)와 동일한 글리프 x_advance 누적 규칙.
    let items = shape_range(store, doc, para, (0, total), warnings);
    // Match place_wrapped: prefer explicit tab stops, then use the default interval.
    let tabs = crate::tab::tab_stops(doc, para);
    let mut segs = Vec::new();
    let mut line_start = 0u32;
    let mut acc = 0.0f32;
    let mut content = false;
    for item in &items {
        match item {
            InlineItem::Run(run) => {
                for (gi, g) in run.glyphs.iter().enumerate() {
                    if content && acc + g.x_advance > limit_pt {
                        place(&mut segs, v_pos, line_start);
                        line_start = run.start_wchar + gi as u32;
                        acc = 0.0;
                    }
                    acc += g.x_advance;
                    content = true;
                }
            }
            InlineItem::Tab => {
                acc = crate::tab::next_tab(&tabs, acc, TAB_INTERVAL_PT);
                content = true;
            }
            // 강제 줄바꿈(CharCtrl 10): 현재 줄을 확정하고 다음 줄을 줄바꿈 뒤 위치에서 시작.
            InlineItem::LineBreak(next_start) => {
                place(&mut segs, v_pos, line_start);
                line_start = *next_start;
                acc = 0.0;
                content = false;
            }
        }
    }
    // 마지막 줄(빈 문단이면 유일한 줄).
    place(&mut segs, v_pos, line_start);

    // 문단 좌여백/첫 줄 들여쓰기(내어쓰기 음수 허용)를 col_start(HWPUNIT)로 인코딩한다.
    // 정품 line_seg는 horzpos에 (좌여백 + 첫 줄 indent)를 담는다: 첫 줄 = 좌여백+indent,
    // 이후 줄(줄바꿈 포함) = 좌여백. IR 여백류는 2×HWPUNIT라 ÷2. 텍스트 영역 좌변 밖으론
    // 안 나가게 0으로 클램프. 좌여백·들여쓰기가 0이면 col_start=0으로 종전과 바이트 동일.
    let ps = doc.header.para_shapes.get(para.para_shape.0 as usize);
    let ml_hu = ps.map_or(0, |p| p.margin_left / 2).max(0);
    let first_hu = ps.map_or(0, |p| (p.margin_left + p.indent) / 2).max(0);
    if let Some(seg0) = segs.first_mut() {
        seg0.col_start = first_hu;
    }
    for seg in segs.iter_mut().skip(1) {
        seg.col_start = ml_hu;
    }
    segs
}
