//! [`Section`] → `Contents/sectionN.xml`.
//!
//! 런 상태 기계: 문자 모양 경계에서 `<hp:run>`을 전환하며 텍스트를
//! 흘려보내고, 확장 컨트롤 위치에서 표/그림/머리말 등을 직렬화한다.
//! 미지원 컨트롤(글상자 등)은 드롭하되 경고로 집계한다.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use hwp_model::{
    BinRef, Cell, Control, Document, Equation, GenericControl, HwpChar, PageDef, Paragraph,
    Picture, Section, SectionDef, ShapeKind, Table,
};

use crate::write::templates::{color_attr, esc};

/// 동봉할 바이너리(이미지) 수집기.
#[derive(Default)]
pub struct BinCollector {
    /// (item id, href, mime, bytes)
    pub items: Vec<(String, String, String, Vec<u8>)>,
}

impl BinCollector {
    /// BinRef를 해석해 패키지 항목으로 등록하고 item id를 돌려준다.
    fn register(&mut self, doc: &Document, bin_ref: &BinRef) -> Option<String> {
        let bytes = doc.resolve_bin(bin_ref)?.to_vec();
        // 같은 바이트는 재사용
        if let Some((id, ..)) = self.items.iter().find(|(.., b)| *b == bytes) {
            return Some(id.clone());
        }
        let (ext, mime) = sniff(&bytes);
        let id = format!("image{}", self.items.len() + 1);
        let href = format!("BinData/{id}.{ext}");
        self.items.push((id.clone(), href, mime.to_string(), bytes));
        Some(id)
    }
}

fn sniff(data: &[u8]) -> (&'static str, &'static str) {
    match data {
        [0x89, b'P', b'N', b'G', ..] => ("png", "image/png"),
        [0xFF, 0xD8, ..] => ("jpg", "image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => ("gif", "image/gif"),
        [b'B', b'M', ..] => ("bmp", "image/bmp"),
        _ => ("bin", "application/octet-stream"),
    }
}

pub fn write_section(
    doc: &Document,
    section: &Section,
    preserve_linesegs: bool,
    bins: &mut BinCollector,
    warnings: &mut Vec<String>,
) -> String {
    let mut out = String::with_capacity(16 * 1024);
    out.push_str(
        r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core">"##,
    );
    let mut ids = IdSeq::default();
    for (pi, para) in section.paragraphs.iter().enumerate() {
        // 첫 문단에 구역 정의가 없으면 기본 secPr 주입
        let inject = pi == 0
            && !para
                .controls
                .iter()
                .any(|c| matches!(c, Control::SectionDef(_)));
        write_paragraph(
            &mut out,
            doc,
            para,
            &mut ids,
            bins,
            inject,
            preserve_linesegs,
            warnings,
        );
    }
    out.push_str("</hs:sec>");
    out
}

#[derive(Default)]
struct IdSeq {
    next: u32,
    /// 각주/미주 번호(종류별 1부터 — footNote@number·autoNum@num에 사용).
    footnote: u32,
    endnote: u32,
}

impl IdSeq {
    fn next(&mut self) -> u32 {
        self.next += 1;
        self.next
    }
}

/// 한 run에 방출할 수 있는 그리기 도형의 최대 수. 한글은 run당 앞쪽 ~21개 도형만
/// 렌더하고 나머지를 버리므로(실기 확정), 여유를 두고 이 수를 넘으면 run을 분할한다.
const SHAPE_RUN_LIMIT: usize = 12;

/// 방출된 XML 조각에서 최상위 그리기 도형 요소 수를 센다. `<hp:line `은 뒤에 공백을 둬
/// `<hp:lineShape`·`<hp:lineseg`·`<hp:lineBreak`와 구분한다(도형 요소는 항상 속성이 따름).
/// 수식도 개체라 같이 센다 — 한글의 run당 한도가 수식까지 포함하는지는 미확정이나,
/// 과다 계상은 run이 더 일찍 갈라질 뿐이고 과소 계상은 개체 유실이라 안전한 쪽을 택한다.
fn count_shape_tags(s: &str) -> usize {
    const OPENS: [&str; 9] = [
        "<hp:rect ",
        "<hp:ellipse ",
        "<hp:line ",
        "<hp:arc ",
        "<hp:polygon ",
        "<hp:curve ",
        "<hp:pic ",
        "<hp:connectLine ",
        "<hp:equation ",
    ];
    OPENS.iter().map(|t| s.matches(t).count()).sum()
}

/// 문단 하나를 직렬화한다. `inject_secpr`이면 첫 런에 기본 구역 정의를 넣는다.
#[allow(clippy::too_many_arguments)]
fn write_paragraph(
    out: &mut String,
    doc: &Document,
    para: &Paragraph,
    ids: &mut IdSeq,
    bins: &mut BinCollector,
    inject_secpr: bool,
    preserve_linesegs: bool,
    warnings: &mut Vec<String>,
) {
    let _ = write!(
        out,
        r##"<hp:p id="{}" paraPrIDRef="{}" styleIDRef="{}" pageBreak="{}" columnBreak="{}" merged="0">"##,
        ids.next(),
        para.para_shape.0,
        para.style.0,
        u8::from(para.header.break_type & 0x04 != 0),
        u8::from(para.header.break_type & 0x08 != 0),
    );

    let first_shape = para.char_shape_runs.first().map_or(0, |(_, id)| id.0);
    let mut run_open = false;
    let mut cur_shape = first_shape;
    let mut text_buf = String::new();
    let mut wchar_pos = 0u32;
    let mut emitted_any_run = false;
    // 열려 있는 필드(FIELD_START)의 id — FIELD_END의 beginIDRef로 연결(필드 비중첩 가정).
    let mut current_field_id: Option<u32> = None;
    // 현재 run에 방출한 그리기 도형 수 — 한글은 run당 앞쪽 ~21개만 그리고 나머지를 버린다
    // (annual 6쪽 링 미렌더 실기 확정: 도형 35개/run → 22번째 이후 타원 전부 누락). 한계
    // 전에 run을 강제 분할해 모든 도형이 렌더되게 한다.
    let mut run_shapes = 0usize;
    // 문단 내 인라인 탭의 대기 XML 큐 + 탭 순번. 탭은 반드시 <hp:t> **안**의 중첩
    // <hp:tab .../>(정품 mixed content)로 방출해야 한다 — t 밖 형제 bare 탭은 한글이
    // 폭 0으로 무시한다(D3 밀착 결함). InlineCtrl(9) 시점에 문단 탭 정의로 XML을 만들어
    // 큐에 넣고 '\t' 센티넬을 텍스트 버퍼에 넣으면, flush_text가 열린 <hp:t> 안에서
    // 센티넬을 큐의 XML로 치환한다(위치 보존).
    let mut pending_tabs: Vec<String> = Vec::new();
    let mut tab_ordinal = 0usize;

    macro_rules! open_run {
        ($shape:expr) => {
            if !run_open || cur_shape != $shape {
                if run_open {
                    flush_text(out, &mut text_buf, &mut pending_tabs);
                    out.push_str("</hp:run>");
                }
                let _ = write!(out, r##"<hp:run charPrIDRef="{}">"##, $shape);
                run_open = true;
                emitted_any_run = true;
                cur_shape = $shape;
                run_shapes = 0;
            }
        };
    }

    // 도형 방출 전 호출: 현재 run이 도형 한계에 다다르면 같은 char_shape로 run을 새로 연다.
    macro_rules! shape_break {
        () => {
            if run_open && run_shapes >= SHAPE_RUN_LIMIT {
                out.push_str("</hp:run>");
                let _ = write!(out, r##"<hp:run charPrIDRef="{}">"##, cur_shape);
                run_shapes = 0;
            }
        };
    }

    if inject_secpr {
        open_run!(first_shape);
        write_default_sec_pr(out, None);
        write_col_ctrl(out, None);
    }

    for ch in &para.chars {
        match ch {
            HwpChar::Text(c) => {
                let shape = shape_id_at(para, wchar_pos);
                open_run!(shape);
                text_buf.push(*c);
            }
            HwpChar::CharCtrl(code) => match *code {
                // 강제 줄바꿈: 정품 한글은 <hp:lineBreak/>를 <hp:t> **안**에 둔다
                // (`<hp:t>앞<hp:lineBreak/>뒤</hp:t>`). t 바깥에 두면 한글이 줄바꿈으로
                // 인식하지 않는다(실기 확인). '\n' 센티넬을 버퍼에 넣고 flush_text가
                // <hp:t> 안에서 <hp:lineBreak/>로 변환한다(정상 텍스트엔 '\n' 없음).
                10 => {
                    open_run!(cur_shape);
                    text_buf.push('\n');
                }
                24 => text_buf.push('-'),
                30 => text_buf.push('\u{00A0}'),
                31 => text_buf.push(' '),
                _ => {}
            },
            HwpChar::InlineCtrl { code, .. } => match *code {
                9 => {
                    // 탭 XML을 지금 만들어(문단 탭 정의의 N번째 항목) 큐에 넣고 '\t'
                    // 센티넬만 버퍼에 넣는다. 실제 방출은 flush_text가 <hp:t> 안에서 한다.
                    open_run!(cur_shape);
                    pending_tabs.push(tab_xml(doc, para, tab_ordinal));
                    tab_ordinal += 1;
                    text_buf.push('\t');
                }
                4 => {
                    // FIELD_END — 앞의 fieldBegin과 beginIDRef로 연결.
                    if let Some(fid) = current_field_id.take() {
                        open_run!(cur_shape);
                        flush_text(out, &mut text_buf, &mut pending_tabs);
                        let _ = write!(
                            out,
                            r##"<hp:ctrl><hp:fieldEnd beginIDRef="{fid}" fieldid="{fid}"/></hp:ctrl>"##,
                        );
                    }
                }
                _ => {}
            },
            HwpChar::ExtCtrl { ctrl_index, .. } => {
                let Some(control) = ctrl_index.and_then(|i| para.controls.get(i as usize)) else {
                    wchar_pos += ch.wchar_width();
                    continue;
                };
                match control {
                    Control::SectionDef(def) => {
                        open_run!(cur_shape);
                        flush_text(out, &mut text_buf, &mut pending_tabs);
                        write_default_sec_pr(out, Some(def));
                    }
                    Control::Generic(g) if g.ctrl_id == *b"cold" => {
                        open_run!(cur_shape);
                        flush_text(out, &mut text_buf, &mut pending_tabs);
                        write_col_ctrl(out, g.column_def.as_ref());
                    }
                    Control::Generic(g) if g.ctrl_id == *b"head" || g.ctrl_id == *b"foot" => {
                        open_run!(cur_shape);
                        flush_text(out, &mut text_buf, &mut pending_tabs);
                        write_header_footer(out, doc, g, ids, bins, preserve_linesegs, warnings);
                    }
                    Control::Table(table) => {
                        open_run!(cur_shape);
                        flush_text(out, &mut text_buf, &mut pending_tabs);
                        write_table(out, doc, table, ids, bins, preserve_linesegs, warnings);
                    }
                    Control::Picture(pic) => {
                        open_run!(cur_shape);
                        flush_text(out, &mut text_buf, &mut pending_tabs);
                        shape_break!();
                        let before = out.len();
                        write_picture(out, doc, pic, ids, bins, warnings);
                        run_shapes += count_shape_tags(&out[before..]);
                    }
                    Control::Generic(g) if hwp_convert::field::is_field_ctrl_id(&g.ctrl_id) => {
                        // 필드(누름틀·계산식·하이퍼링크 등) — fieldBegin 방출. 값 텍스트는
                        // 뒤따르는 Text가 <hp:t>로, FIELD_END(InlineCtrl 4)가 fieldEnd로 닫는다.
                        open_run!(cur_shape);
                        flush_text(out, &mut text_buf, &mut pending_tabs);
                        let (name, command) = hwp_convert::field::field_meta(control);
                        let fid = ids.next();
                        current_field_id = Some(fid);
                        let ty = hwp_convert::field::owpml_field_type(&g.ctrl_id);
                        let _ = write!(
                            out,
                            r##"<hp:ctrl><hp:fieldBegin id="{fid}" type="{ty}" name="{}" editable="1" dirty="0" zorder="-1" fieldid="{fid}" metaTag="""##,
                            esc(name.as_deref().unwrap_or("")),
                        );
                        if let Some(cmd) = &command {
                            let _ = write!(
                                out,
                                r##"><hp:parameters cnt="1" name=""><hp:stringParam name="Command">{}</hp:stringParam></hp:parameters></hp:fieldBegin></hp:ctrl>"##,
                                esc(cmd),
                            );
                        } else {
                            out.push_str("/></hp:ctrl>");
                        }
                    }
                    Control::Generic(g) if g.ctrl_id == *b"bokm" => {
                        // 책갈피(지점 표식) — <hp:bookmark name="…"/>. 필드와 달리 END 없음.
                        open_run!(cur_shape);
                        flush_text(out, &mut text_buf, &mut pending_tabs);
                        let name =
                            hwp_convert::bookmark::bookmark_name(control).unwrap_or_default();
                        let _ = write!(
                            out,
                            r##"<hp:ctrl><hp:bookmark name="{}"/></hp:ctrl>"##,
                            esc(&name)
                        );
                    }
                    Control::Generic(g) if g.ctrl_id == *b"pgnp" && g.data.len() >= 12 => {
                        // 쪽번호 위치 — reader build_pgnp의 역(12B: props[format|pos<<8] +
                        // 6B 0 + side_char u16). 정답지: 한글 export <hp:pageNum>.
                        open_run!(cur_shape);
                        flush_text(out, &mut text_buf, &mut pending_tabs);
                        let props =
                            u32::from_le_bytes([g.data[0], g.data[1], g.data[2], g.data[3]]);
                        let side = u16::from_le_bytes([g.data[10], g.data[11]]);
                        let side_s = char::from_u32(u32::from(side))
                            .filter(|c| *c != '\0')
                            .map(String::from)
                            .unwrap_or_default();
                        let _ = write!(
                            out,
                            r##"<hp:ctrl><hp:pageNum pos="{}" formatType="DIGIT" sideChar="{}"/></hp:ctrl>"##,
                            page_num_pos_name(((props >> 8) & 0xFF) as u8),
                            esc(&side_s),
                        );
                    }
                    Control::Generic(g) if g.ctrl_id == *b"pghd" && g.data.len() >= 4 => {
                        // 쪽 감추기 — reader build_pghd의 역(4B 비트맵).
                        open_run!(cur_shape);
                        flush_text(out, &mut text_buf, &mut pending_tabs);
                        let mask = u32::from_le_bytes([g.data[0], g.data[1], g.data[2], g.data[3]]);
                        let b = |bit: u32| u8::from(mask & (1 << bit) != 0);
                        let _ = write!(
                            out,
                            r##"<hp:ctrl><hp:pageHiding hideHeader="{}" hideFooter="{}" hideMasterPage="{}" hideBorder="{}" hideFill="{}" hidePageNum="{}"/></hp:ctrl>"##,
                            b(0),
                            b(1),
                            b(2),
                            b(3),
                            b(4),
                            b(5),
                        );
                    }
                    Control::Generic(g) if g.ctrl_id == *b"nwno" && g.data.len() >= 6 => {
                        // 새 번호 지정 — reader build_nwno의 역(종류 u32 + num u16).
                        open_run!(cur_shape);
                        flush_text(out, &mut text_buf, &mut pending_tabs);
                        let num = u16::from_le_bytes([g.data[4], g.data[5]]);
                        let _ = write!(
                            out,
                            r##"<hp:ctrl><hp:newNum num="{num}" numType="PAGE"/></hp:ctrl>"##,
                        );
                    }
                    Control::Generic(g) if g.ctrl_id == *b"atno" => {
                        // 자동 번호(쪽) — 코퍼스 export에 인라인 정답지가 없어 표준형으로
                        // 방출(v1). 페이로드는 reader build_atno가 실측 표준 12B로 복원.
                        open_run!(cur_shape);
                        flush_text(out, &mut text_buf, &mut pending_tabs);
                        out.push_str(r##"<hp:ctrl><hp:autoNum numType="PAGE"/></hp:ctrl>"##);
                    }
                    Control::Generic(g) if !g.gso_shapes.is_empty() => {
                        // hwpx-출신 구조화 도형(rect/ellipse/line/…) — ShapeGeom 재직렬화.
                        open_run!(cur_shape);
                        flush_text(out, &mut text_buf, &mut pending_tabs);
                        shape_break!();
                        let before = out.len();
                        write_ir_shapes(out, doc, g, ids, bins, preserve_linesegs, warnings);
                        run_shapes += count_shape_tags(&out[before..]);
                    }
                    Control::Generic(g) if g.ctrl_id == *b"gso " => {
                        // hwp5-출신 gso: 글상자(rect+drawText — 텍스트/필드/책갈피 보존)와
                        // 장식 도형(SHAPE_COMPONENT → 도형 요소) 모두 방출.
                        open_run!(cur_shape);
                        flush_text(out, &mut text_buf, &mut pending_tabs);
                        shape_break!();
                        let before = out.len();
                        write_gso(out, doc, g, ids, bins, preserve_linesegs, warnings);
                        run_shapes += count_shape_tags(&out[before..]);
                    }
                    Control::Generic(g) if g.ctrl_id == *b"fn  " || g.ctrl_id == *b"en  " => {
                        // 각주/미주 — <hp:footNote>/<hp:endNote> + 본문 subList. reader는
                        // 속성을 무시하고 subList 문단만 수집하므로 표준 속성으로 방출한다.
                        open_run!(cur_shape);
                        flush_text(out, &mut text_buf, &mut pending_tabs);
                        write_foot_end_note(out, doc, g, ids, bins, preserve_linesegs, warnings);
                    }
                    Control::Generic(g) if g.equation.is_some() => {
                        // 수식 — hp:ctrl이 아닌 run 직속 개체(리더 parse_equation의 역).
                        // hwpx 출신(hp:script)·hwp5 출신(EQEDIT 스크립트, parse_eqed) 모두
                        // IR Equation을 채우므로 같은 경로로 방출한다.
                        open_run!(cur_shape);
                        flush_text(out, &mut text_buf, &mut pending_tabs);
                        shape_break!();
                        let eq = g.equation.as_ref().expect("is_some 가드");
                        let before = out.len();
                        write_equation(out, eq, ids);
                        run_shapes += count_shape_tags(&out[before..]);
                    }
                    Control::Generic(g) => {
                        warnings.push(format!(
                            "DROP: hwpx 쓰기 미지원 컨트롤 드롭: {:?}",
                            String::from_utf8_lossy(&g.ctrl_id)
                        ));
                    }
                }
            }
        }
        wchar_pos += ch.wchar_width();
    }

    if run_open {
        flush_text(out, &mut text_buf, &mut pending_tabs);
        out.push_str("</hp:run>");
    }
    // 줄 배치 정보 보존 (무수정 왕복 전용 — 기본은 제거, 한글이 재계산)
    if preserve_linesegs && !para.line_segs.is_empty() {
        out.push_str("<hp:linesegarray>");
        for seg in &para.line_segs {
            let _ = write!(
                out,
                r##"<hp:lineseg textpos="{}" vertpos="{}" vertsize="{}" textheight="{}" baseline="{}" spacing="{}" horzpos="{}" horzsize="{}" flags="{}"/>"##,
                seg.text_start,
                seg.v_pos,
                seg.line_height,
                seg.text_height,
                seg.baseline_gap,
                seg.line_spacing,
                seg.col_start,
                seg.seg_width,
                seg.flags,
            );
        }
        out.push_str("</hp:linesegarray>");
    }
    if !emitted_any_run {
        // 빈 문단도 런 하나는 가져야 한다 (기준 표본 패턴)
        let _ = write!(
            out,
            r##"<hp:run charPrIDRef="{first_shape}"><hp:t/></hp:run>"##
        );
    }
    out.push_str("</hp:p>");
}

/// 인라인 `<hp:tab>`의 `type` 속성 = hwp5 탭 종류 코드 + 1.
///
/// 정품 실측(정답지 대조 확정): 왼쪽 탭(kind 0)→`type="1"`, 오른쪽 탭(kind 1)→`type="2"`.
/// OWPML 인라인 탭은 헤더 `tabItem`의 문자열 종류(LEFT/RIGHT/…)와 달리 1-기반 정수
/// 열거를 쓴다. 가운데(2)→3·소수점(3)→4는 같은 +1 규칙의 외삽이다.
fn tab_type_attr(kind: u8) -> u8 {
    kind.saturating_add(1)
}

/// 인라인 `<hp:tab>`의 `leader` 속성 = 채움(리더) 코드 → OWPML 인라인 리더 열거.
///
/// 정품 실측 확정: 없음(fill 0)→`leader="0"`, DASH(fill 2)→`leader="3"`. 인라인 리더
/// 열거는 표25 테두리선 종류와 순서가 달라(DASH가 3) 그대로 쓸 수 없다. SOLID(1)→1·
/// DOT(3)→2는 두 실측점과 모순 없는 자기일관 근사(미확인)이며, 그 밖의 코드는 관찰값
/// DASH(3)로 강등한다(write/header `tab_leader_name`과 동일 철학 — 한글이 tab leader로
/// 허용하는지 미확인인 값을 방출하지 않는다).
fn tab_leader_attr(fill: u8) -> u8 {
    match fill {
        0 => 0, // 없음(실측)
        1 => 1, // 실선(근사)
        3 => 2, // 점(근사)
        _ => 3, // DASH(실측) + 미확인 코드 강등
    }
}

/// 인라인 탭의 기본 폭(HWPUNIT). 정품 기본 탭 폭 실측값 4000. 한글은 파일을 열 때
/// 탭 폭을 텍스트 렌더 결과로 **재계산**하므로(정품 목차 탭 폭이 제목 길이에 반비례:
/// 짧은 "개요" 33718 → 긴 "현황 및 문제점" 24718) 이 값은 근사 자리표시자다.
const DEFAULT_TAB_WIDTH: i32 = 4000;

/// 문단의 `ordinal`번째(0-기반) 탭에 대응하는 `<hp:tab .../>` XML.
///
/// 종류/채움은 문단 글자모양이 참조하는 탭 정의(`tab_def_id` → `tab_stops`)의 같은 순번
/// 항목에서 가져온다(정품: 목차 문단 → tabPr id=3 → RIGHT/DASH → `type="2" leader="3"`).
/// 정의가 없거나 항목이 모자라면 정품 기본 탭(왼쪽·채움 없음 → `type="1" leader="0"`).
/// 속성 순서(width→leader→type)도 정품 방출과 맞춘다.
fn tab_xml(doc: &Document, para: &Paragraph, ordinal: usize) -> String {
    let item = doc
        .header
        .para_shapes
        .get(para.para_shape.0 as usize)
        .map(|ps| ps.tab_def_id)
        .and_then(|id| doc.header.tab_stops.get(id as usize))
        .and_then(|td| td.items.get(ordinal));
    let (kind, fill) = item.map_or((0, 0), |it| (it.kind, it.fill));
    format!(
        r##"<hp:tab width="{}" leader="{}" type="{}"/>"##,
        DEFAULT_TAB_WIDTH,
        tab_leader_attr(fill),
        tab_type_attr(kind),
    )
}

/// 기본 인라인 탭 XML(대기 큐에 짝이 없는 방어선용 — 과거 오염 IR의 raw 0x09).
const DEFAULT_TAB_XML: &str = r##"<hp:tab width="4000" leader="0" type="1"/>"##;

fn flush_text(out: &mut String, buf: &mut String, pending_tabs: &mut Vec<String>) {
    if buf.is_empty() {
        return;
    }
    // 정상 경로의 buf 내용은 텍스트 + '\n'(강제 줄바꿈 센티넬) + '\t'(탭 센티넬)이다.
    // '\t'는 <hp:t> **안**에서 대기 큐의 <hp:tab .../>로 치환하고(정품 mixed content),
    // 그 밖의 C0 제어문자는 제거한다 — raw 0x09를 <hp:t> 안에 그대로 내보내면 한글이
    // 파일을 열지 못하고(D3 먹통), t 밖 형제 탭은 폭 0으로 무시된다(D3 밀착).
    let mut t_open = false;
    let mut seg = String::new();
    // 이번 flush에서 소비한 대기 탭 수(끝에서 큐 앞부분을 그만큼 제거).
    let mut tabs_used = 0usize;
    macro_rules! ensure_t {
        () => {
            if !t_open {
                out.push_str(r##"<hp:t xml:space="preserve">"##);
                t_open = true;
            }
        };
    }
    for c in buf.chars() {
        match c {
            // 강제 줄바꿈 센티넬 — <hp:t> 안의 <hp:lineBreak/>.
            '\n' => {
                ensure_t!();
                out.push_str(&esc(&seg));
                seg.clear();
                out.push_str("<hp:lineBreak/>");
            }
            // 탭 센티넬 — 앞 텍스트를 흘린 뒤 같은 <hp:t> 안에 중첩 <hp:tab .../>를 낸다.
            // 대기 큐에서 이 탭의 XML을 순서대로 꺼낸다. 큐에 짝이 없으면(방어선: 과거
            // 오염 IR의 raw 탭) 기본 탭을 같은 형식으로 낸다.
            '\t' => {
                ensure_t!();
                out.push_str(&esc(&seg));
                seg.clear();
                match pending_tabs.get(tabs_used) {
                    Some(x) => {
                        out.push_str(x);
                        tabs_used += 1;
                    }
                    None => out.push_str(DEFAULT_TAB_XML),
                }
            }
            // 그 외 C0 제어문자는 문서를 깨뜨리므로 제거.
            c if (c as u32) < 0x20 => {}
            c => seg.push(c),
        }
    }
    if !seg.is_empty() {
        ensure_t!();
        out.push_str(&esc(&seg));
    }
    if t_open {
        out.push_str("</hp:t>");
    }
    pending_tabs.drain(0..tabs_used);
    buf.clear();
}

fn shape_id_at(para: &Paragraph, pos: u32) -> u16 {
    para.char_shape_runs
        .iter()
        .rev()
        .find(|(start, _)| *start <= pos)
        .map(|(_, id)| id.0)
        .unwrap_or(0)
}

/// 기본 A4 PageDef (구역 정의가 없는 문서 방어).
fn default_page() -> PageDef {
    PageDef {
        width: hwp_model::HwpUnit(59528),
        height: hwp_model::HwpUnit(84186),
        margin_left: hwp_model::HwpUnit(8504),
        margin_right: hwp_model::HwpUnit(8504),
        margin_top: hwp_model::HwpUnit(5668),
        margin_bottom: hwp_model::HwpUnit(4252),
        margin_header: hwp_model::HwpUnit(4252),
        margin_footer: hwp_model::HwpUnit(4252),
        gutter: hwp_model::HwpUnit(0),
        attr: 0,
    }
}

/// `<hp:secPr>` 여는 태그(상수 속성). 원문 pass-through·상수 템플릿 두 경로가 공유한다.
const SEC_PR_OPEN: &str = r##"<hp:secPr id="" textDirection="HORIZONTAL" spaceColumns="1134" tabStop="8000" tabStopVal="4000" tabStopUnit="HWPUNIT" outlineShapeIDRef="1" memoShapeIDRef="0" textVerticalWidthHead="0" masterPageCnt="0">"##;

/// `<hp:pagePr>`+`<hp:margin>`을 페이지 정의로부터 방출한다(상수 템플릿과 바이트 동일 형식).
fn write_page_pr(out: &mut String, p: &PageDef) {
    let landscape = if p.attr & 1 != 0 {
        "NARROWLY"
    } else {
        "WIDELY"
    };
    let _ = write!(
        out,
        r##"<hp:pagePr landscape="{landscape}" width="{}" height="{}" gutterType="LEFT_ONLY"><hp:margin header="{}" footer="{}" gutter="{}" left="{}" right="{}" top="{}" bottom="{}"/></hp:pagePr>"##,
        p.width.0,
        p.height.0,
        p.margin_header.0,
        p.margin_footer.0,
        p.gutter.0,
        p.margin_left.0,
        p.margin_right.0,
        p.margin_top.0,
        p.margin_bottom.0,
    );
}

/// `<hp:secPr>`의 머리(grid/startNum/visibility/lineNumberShape/pagePr)를 방출한다.
/// 상수 템플릿 경로와 hwp5 raw 해석 경로가 공유한다(출력 바이트 동일 형식).
fn write_sec_pr_head(out: &mut String, p: &PageDef) {
    let landscape = if p.attr & 1 != 0 {
        "NARROWLY"
    } else {
        "WIDELY"
    };
    out.push_str(SEC_PR_OPEN);
    let _ = write!(
        out,
        r##"<hp:grid lineGrid="0" charGrid="0" wonggojiFormat="0"/><hp:startNum pageStartsOn="BOTH" page="0" pic="0" tbl="0" equation="0"/><hp:visibility hideFirstHeader="0" hideFirstFooter="0" hideFirstMasterPage="0" border="SHOW_ALL" fill="SHOW_ALL" hideFirstPageNum="0" hideFirstEmptyLine="0" showLineNumber="0"/><hp:lineNumberShape restartType="0" countBy="0" distance="0" startNumber="0"/><hp:pagePr landscape="{landscape}" width="{}" height="{}" gutterType="LEFT_ONLY"><hp:margin header="{}" footer="{}" gutter="{}" left="{}" right="{}" top="{}" bottom="{}"/></hp:pagePr>"##,
        p.width.0,
        p.height.0,
        p.margin_header.0,
        p.margin_footer.0,
        p.gutter.0,
        p.margin_left.0,
        p.margin_right.0,
        p.margin_top.0,
        p.margin_bottom.0,
    );
}

/// 상수 각주 모양(hello_world 표본 실측) — hwp5 raw가 없을 때의 안전값.
const CONST_FOOT_NOTE_PR: &str = r##"<hp:footNotePr><hp:autoNumFormat type="DIGIT" userChar="" prefixChar="" suffixChar=")" supscript="0"/><hp:noteLine length="-1" type="SOLID" width="0.12 mm" color="#000000"/><hp:noteSpacing betweenNotes="283" belowLine="567" aboveLine="850"/><hp:numbering type="CONTINUOUS" newNum="1"/><hp:placement place="EACH_COLUMN" beneathText="0"/></hp:footNotePr>"##;

/// 상수 미주 모양(hello_world 표본 실측) — hwp5 raw가 없을 때의 안전값.
const CONST_END_NOTE_PR: &str = r##"<hp:endNotePr><hp:autoNumFormat type="DIGIT" userChar="" prefixChar="" suffixChar=")" supscript="0"/><hp:noteLine length="14692344" type="SOLID" width="0.12 mm" color="#000000"/><hp:noteSpacing betweenNotes="0" belowLine="567" aboveLine="850"/><hp:numbering type="CONTINUOUS" newNum="1"/><hp:placement place="END_OF_DOCUMENT" beneathText="0"/></hp:endNotePr>"##;

/// 상수 쪽 테두리/배경 3종(BOTH/EVEN/ODD) — hwp5 raw가 없을 때의 안전값.
const CONST_PAGE_BORDER_FILL: [&str; 3] = [
    r##"<hp:pageBorderFill type="BOTH" borderFillIDRef="1" textBorder="PAPER" headerInside="0" footerInside="0" fillArea="PAPER"><hp:offset left="1417" right="1417" top="1417" bottom="1417"/></hp:pageBorderFill>"##,
    r##"<hp:pageBorderFill type="EVEN" borderFillIDRef="1" textBorder="PAPER" headerInside="0" footerInside="0" fillArea="PAPER"><hp:offset left="1417" right="1417" top="1417" bottom="1417"/></hp:pageBorderFill>"##,
    r##"<hp:pageBorderFill type="ODD" borderFillIDRef="1" textBorder="PAPER" headerInside="0" footerInside="0" fillArea="PAPER"><hp:offset left="1417" right="1417" top="1417" bottom="1417"/></hp:pageBorderFill>"##,
];

/// `<hp:secPr>`를 방출한다.
///
/// 출처별 단일 방출(이중 방출 금지):
/// 1. `secpr_raw_children`가 있으면(hwpx 출신) 원문 자식을 등장 순서대로 pass-through하고
///    [`SECPR_PAGEPR_SLOT`] 자리에서만 페이지 정의로 pagePr을 재생성한다(GC-5).
/// 2. hwp5 raw 필드(FOOTNOTE_SHAPE·PAGE_BORDER_FILL)가 있으면(hwp5 출신) 상수 대신
///    실측 footNotePr/endNotePr/pageBorderFill을 재구성해 방출한다 — 교차 변환 손실 차단.
/// 3. 둘 다 없으면(합성·구형 IR·secPr 주입) 기존 상수 템플릿을 방출한다 — 출력 바이트 불변.
fn write_default_sec_pr(out: &mut String, def: Option<&SectionDef>) {
    let fallback = default_page();
    let p = def.and_then(|d| d.page.as_ref()).unwrap_or(&fallback);

    // 1) hwpx 출신 원문 pass-through.
    if let Some(d) = def
        && !d.secpr_raw_children.is_empty()
    {
        out.push_str(SEC_PR_OPEN);
        for child in &d.secpr_raw_children {
            if child == hwp_model::SECPR_PAGEPR_SLOT {
                write_page_pr(out, p);
            } else {
                out.push_str(child);
            }
        }
        out.push_str("</hp:secPr>");
        return;
    }

    // 2) hwp5 출신 raw 해석.
    if let Some(d) = def
        && (d.footnote_shape_raw.is_some()
            || d.endnote_shape_raw.is_some()
            || !d.page_border_fills_raw.is_empty())
    {
        write_sec_pr_head(out, p);
        write_note_pr(out, "hp:footNotePr", d.footnote_shape_raw.as_deref(), false);
        write_note_pr(out, "hp:endNotePr", d.endnote_shape_raw.as_deref(), true);
        write_page_border_fills(out, &d.page_border_fills_raw);
        out.push_str("</hp:secPr>");
        return;
    }

    // 3) 상수 템플릿 — 기존 출력과 바이트 동일.
    write_sec_pr_head(out, p);
    out.push_str(CONST_FOOT_NOTE_PR);
    out.push_str(CONST_END_NOTE_PR);
    for s in CONST_PAGE_BORDER_FILL {
        out.push_str(s);
    }
    out.push_str("</hp:secPr>");
}

/// FOOTNOTE_SHAPE(28B) raw를 `<hp:footNotePr>`/`<hp:endNotePr>`로 재구성한다.
/// 레이아웃(gc23 조사 보고서 확정, 정품 전수 실측): 속성 u32, WCHAR×3(사용자기호/앞/뒤장식),
/// 시작번호 u16, 구분선 길이 HWPUNIT(i32), 여백 u16×3(위/아래/주석사이), 구분선 종류 u8,
/// 굵기 u8, 색 COLORREF. raw가 없거나 손상되면 상수 기본값으로 대체(secPr 유효성 보장).
fn write_note_pr(out: &mut String, tag: &str, raw: Option<&[u8]>, is_end: bool) {
    let Some(r) = raw.filter(|b| b.len() >= 28) else {
        out.push_str(if is_end {
            CONST_END_NOTE_PR
        } else {
            CONST_FOOT_NOTE_PR
        });
        return;
    };
    let attr = u32::from_le_bytes([r[0], r[1], r[2], r[3]]);
    let user_char = u16::from_le_bytes([r[4], r[5]]);
    let prefix_char = u16::from_le_bytes([r[6], r[7]]);
    let suffix_char = u16::from_le_bytes([r[8], r[9]]);
    let start_num = u16::from_le_bytes([r[10], r[11]]);
    let line_len = i32::from_le_bytes([r[12], r[13], r[14], r[15]]);
    let above = u16::from_le_bytes([r[16], r[17]]);
    let below = u16::from_le_bytes([r[18], r[19]]);
    let between = u16::from_le_bytes([r[20], r[21]]);
    let line_type = r[22];
    let line_width = r[23];
    let color = u32::from_le_bytes([r[24], r[25], r[26], r[27]]);

    let num_fmt = note_num_format(attr & 0xFF);
    let supscript = (attr >> 12) & 1;
    let numbering = match (attr >> 10) & 0x3 {
        1 => "ON_SECTION",
        2 => "ON_PAGE",
        _ => "CONTINUOUS",
    };
    // 미주는 배치가 다단(bits8-9)이 아니라 문서 끝 관례 — 정품 실측 END_OF_DOCUMENT.
    let place = if is_end {
        "END_OF_DOCUMENT"
    } else {
        match (attr >> 8) & 0x3 {
            1 => "MERGED_COLUMN",
            2 => "RIGHT_MARGIN",
            _ => "EACH_COLUMN",
        }
    };
    let _ = write!(
        out,
        r##"<{tag}><hp:autoNumFormat type="{num_fmt}" userChar="{}" prefixChar="{}" suffixChar="{}" supscript="{supscript}"/><hp:noteLine length="{line_len}" type="{}" width="{}" color="{}"/><hp:noteSpacing betweenNotes="{between}" belowLine="{below}" aboveLine="{above}"/><hp:numbering type="{numbering}" newNum="{start_num}"/><hp:placement place="{place}" beneathText="0"/></{tag}>"##,
        wchar_attr(user_char),
        wchar_attr(prefix_char),
        wchar_attr(suffix_char),
        note_line_type(line_type),
        note_line_width_mm(line_width),
        note_line_color(color),
    );
}

/// PAGE_BORDER_FILL(14B) raw 목록을 `<hp:pageBorderFill>` 3종(BOTH/EVEN/ODD)으로 방출한다.
/// 레이아웃(gc23 확정): 속성 u32(bit0 위치기준=종이·bit1 머리말·bit2 꼬리말·bits3-4 채울영역)
/// 다음에 gap u16×4(left/right/top/bottom), 끝에 테두리ID u16(1-기반). 순서가 곧 BOTH/EVEN/ODD.
/// 테두리ID는 hwpx borderFillIDRef와 1:1 대응(표 셀 테두리 승계 규약과 동일 — GE-7 전례).
/// raw가 없거나 3개 미만이면 부족분은 상수로 채워 3종을 항상 방출한다.
fn write_page_border_fills(out: &mut String, raws: &[Vec<u8>]) {
    const TYPES: [&str; 3] = ["BOTH", "EVEN", "ODD"];
    for (i, ty) in TYPES.iter().enumerate() {
        let Some(r) = raws.get(i).filter(|b| b.len() >= 14) else {
            out.push_str(CONST_PAGE_BORDER_FILL[i]);
            continue;
        };
        let attr = u32::from_le_bytes([r[0], r[1], r[2], r[3]]);
        let left = u16::from_le_bytes([r[4], r[5]]);
        let right = u16::from_le_bytes([r[6], r[7]]);
        let top = u16::from_le_bytes([r[8], r[9]]);
        let bottom = u16::from_le_bytes([r[10], r[11]]);
        let bf_id = u16::from_le_bytes([r[12], r[13]]).max(1);
        let text_border = if attr & 1 != 0 { "PAPER" } else { "PAGE" };
        let header_inside = (attr >> 1) & 1;
        let footer_inside = (attr >> 2) & 1;
        let fill_area = match (attr >> 3) & 0x3 {
            1 => "PAGE",
            2 => "BORDER",
            _ => "PAPER",
        };
        let _ = write!(
            out,
            r##"<hp:pageBorderFill type="{ty}" borderFillIDRef="{bf_id}" textBorder="{text_border}" headerInside="{header_inside}" footerInside="{footer_inside}" fillArea="{fill_area}"><hp:offset left="{left}" right="{right}" top="{top}" bottom="{bottom}"/></hp:pageBorderFill>"##,
        );
    }
}

/// hwp5 번호 모양 코드 → hwpx autoNumFormat@type. 확신 가능한 흔한 값만 매핑하고
/// 그 밖은 DIGIT으로 강등(한글 유효성 우선).
fn note_num_format(fmt: u32) -> &'static str {
    match fmt {
        0 => "DIGIT",
        1 => "CIRCLE_DIGIT",
        2 => "ROMAN_CAPITAL",
        3 => "ROMAN_SMALL",
        4 => "LATIN_CAPITAL",
        5 => "LATIN_SMALL",
        _ => "DIGIT",
    }
}

/// 구분선 종류 코드 → hwpx noteLine@type. 미상/0은 SOLID로 강등(구분선은 항상 그려짐).
fn note_line_type(code: u8) -> &'static str {
    match code {
        2 => "DASH",
        3 => "DOT",
        4 => "DASH_DOT",
        5 => "DASH_DOT_DOT",
        6 => "LONG_DASH",
        _ => "SOLID",
    }
}

/// 구분선 굵기 인덱스 → "N mm" (한글 굵기 표 재사용). 정수는 "0.4 mm", 소수는 "0.12 mm".
fn note_line_width_mm(code: u8) -> String {
    let line = hwp_model::BorderLine {
        line_type: 1,
        width: code,
        color: 0,
    };
    let mm = line.width_mm();
    if (mm - mm.round()).abs() < f32::EPSILON {
        format!("{} mm", mm.round() as i32)
    } else {
        format!("{mm} mm")
    }
}

/// 구분선 색 COLORREF → "#rrggbb". 없음(0xFFFFFFFF)은 검정으로 대체(구분선은 색이 필요).
fn note_line_color(c: u32) -> String {
    if c == 0xFFFF_FFFF {
        "#000000".to_string()
    } else {
        color_attr(c)
    }
}

/// WCHAR(u16) → 속성값. 0(없음)은 빈 문자열, 그 외는 문자로 변환 후 XML 이스케이프.
fn wchar_attr(wc: u16) -> String {
    if wc == 0 {
        return String::new();
    }
    match char::from_u32(u32::from(wc)) {
        Some(c) => esc(&c.to_string()),
        None => String::new(),
    }
}

fn write_col_ctrl(out: &mut String, col: Option<&hwp_model::ColumnDef>) {
    // ColumnDef가 있으면 그 값을, 없으면 단일 단 기본값을 방출(왕복 보존).
    let (ty, layout, count, same, gap) = match col {
        Some(c) => (
            match c.kind {
                1 => "BALANCED",
                2 => "PARALLEL",
                _ => "NEWSPAPER",
            },
            match c.direction {
                1 => "RIGHT",
                2 => "MIRROR",
                _ => "LEFT",
            },
            c.count.max(1),
            u8::from(c.same_width),
            c.gap,
        ),
        None => ("NEWSPAPER", "LEFT", 1, 1, 0),
    };
    let _ = write!(
        out,
        r##"<hp:ctrl><hp:colPr id="" type="{ty}" layout="{layout}" colCount="{count}" sameSz="{same}" sameGap="{gap}"/></hp:ctrl>"##,
    );
}

/// 머리말/꼬리말 적용쪽(applyPageType) 역매핑. `g.data` 선두 UINT32의 bits0-1이
/// 적용될 페이지 종류다(0=양쪽·1=짝수·2=홀수). hwp5-origin은 CTRL_HEADER 속성(표
/// 141), hwpx-origin은 read 측 `head_foot_data`가 만든 8B 페이로드(apply@0)로, 두
/// 경로 모두 선두 u32에 적용쪽이 실린다 — read의 정매핑을 그대로 뒤집는다.
fn header_footer_apply_page(data: &[u8]) -> &'static str {
    let apply = if data.len() >= 4 {
        u32::from_le_bytes([data[0], data[1], data[2], data[3]]) & 0x3
    } else {
        0
    };
    match apply {
        1 => "EVEN",
        2 => "ODD",
        _ => "BOTH",
    }
}

#[allow(clippy::too_many_arguments)]
fn write_header_footer(
    out: &mut String,
    doc: &Document,
    g: &GenericControl,
    ids: &mut IdSeq,
    bins: &mut BinCollector,
    preserve_linesegs: bool,
    warnings: &mut Vec<String>,
) {
    let el = if g.ctrl_id == *b"head" {
        "header"
    } else {
        "footer"
    };
    let apply = header_footer_apply_page(&g.data);
    let _ = write!(
        out,
        r##"<hp:ctrl><hp:{el} id="{}" applyPageType="{apply}">"##,
        ids.next()
    );
    for list in &g.paragraph_lists {
        out.push_str(
            r##"<hp:subList id="" textDirection="HORIZONTAL" lineWrap="BREAK" vertAlign="TOP" linkListIDRef="0" linkListNextIDRef="0" textWidth="0" textHeight="0" hasTextRef="0" hasNumRef="0">"##,
        );
        for para in &list.paragraphs {
            write_paragraph(
                out,
                doc,
                para,
                ids,
                bins,
                false,
                preserve_linesegs,
                warnings,
            );
        }
        out.push_str("</hp:subList>");
    }
    let _ = write!(out, "</hp:{el}></hp:ctrl>");
}

/// 각주/미주 — `<hp:footNote>`/`<hp:endNote>` + 본문 `<hp:subList>`.
/// reader(section.rs)는 `footNote`/`endNote` 요소를 `fn `/`en ` GenericControl로
/// 되읽고 subList 문단을 paragraph_lists로 수집한다 — 속성은 무시하므로 표준값을 쓴다.
#[allow(clippy::too_many_arguments)]
fn write_foot_end_note(
    out: &mut String,
    doc: &Document,
    g: &GenericControl,
    ids: &mut IdSeq,
    bins: &mut BinCollector,
    preserve_linesegs: bool,
    warnings: &mut Vec<String>,
) {
    // 한글 저장본 실측 형태(정답지=커밋 픽스처): number는 종류별 1-기반 수열,
    // suffixChar="41", instId는 문서 고유값(정품도 임의 u32 — 큰 베이스로 합성).
    let foot = g.ctrl_id == *b"fn  ";
    let (el, num_type) = if foot {
        ids.footnote += 1;
        ("footNote", "FOOTNOTE")
    } else {
        ids.endnote += 1;
        ("endNote", "ENDNOTE")
    };
    let number = if foot { ids.footnote } else { ids.endnote };
    let inst_id = 0x4000_0000 + ids.next();
    let _ = write!(
        out,
        r##"<hp:ctrl><hp:{el} number="{number}" suffixChar="41" instId="{inst_id}">"##
    );
    for list in &g.paragraph_lists {
        out.push_str(
            r##"<hp:subList id="" textDirection="HORIZONTAL" lineWrap="BREAK" vertAlign="TOP" linkListIDRef="0" linkListNextIDRef="0" textWidth="0" textHeight="0" hasTextRef="0" hasNumRef="0">"##,
        );
        for para in &list.paragraphs {
            // 노트 본문의 autoNum은 atno arm이 numType="PAGE" 상수로 쓰므로, 여기서
            // 종류·번호로 교체한다(정품: 노트 첫 run의 번호 필드).
            let mut buf = String::new();
            write_paragraph(
                &mut buf,
                doc,
                para,
                ids,
                bins,
                false,
                preserve_linesegs,
                warnings,
            );
            const PAGE_SNIP: &str = r##"<hp:ctrl><hp:autoNum numType="PAGE"/></hp:ctrl>"##;
            if buf.contains(PAGE_SNIP) {
                let note_snip = format!(
                    r##"<hp:ctrl><hp:autoNum num="{number}" numType="{num_type}"><hp:autoNumFormat type="DIGIT" userChar="" prefixChar="" suffixChar=")" supscript="0"/></hp:autoNum></hp:ctrl>"##
                );
                buf = buf.replacen(PAGE_SNIP, &note_snip, 1);
            }
            out.push_str(&buf);
        }
        out.push_str("</hp:subList>");
    }
    let _ = write!(out, "</hp:{el}></hp:ctrl>");
}

/// 수식 → `<hp:equation>`(run 직속). 리더 `parse_equation`이 읽는 것은 `hp:script`
/// 자식과 `hp:sz`·`hp:pos`뿐이라 왕복은 이 셋으로 닫힌다. 나머지 속성은 개체 공통
/// 스캐폴드(`write_shape_element`·`hp:pic`과 동일 순서: sz → pos → outMargin)와
/// 수식 전용 상수(baseUnit·version·font)로 채운다.
///
/// ⚠ 수식 전용 상수는 **한글 정답지 미확보 상태의 표준 추정값**이다(코퍼스에 수식 든
/// hwpx가 없다). 실기에서 수식이 깨져 보이면 정품 저장본의 `<hp:equation>` 속성으로
/// 교체할 것 — 12-feature-gaps.md GE-14 참조.
fn write_equation(out: &mut String, eq: &Equation, ids: &mut IdSeq) {
    // 인라인(글자처럼 취급)은 본문 흐름을 따르고, 부유는 겹침 허용 — gso_pos_xml과 같은
    // 실측 규칙(부유에 flowWithText=1을 주면 한글이 개체를 배치하지 못한다).
    let (treat, flow, overlap, wrap) = if eq.inline {
        (1, 1, 0, "SQUARE")
    } else {
        (0, 0, 1, "IN_FRONT_OF_TEXT")
    };
    let id = ids.next();
    let _ = write!(
        out,
        r##"<hp:equation id="{id}" zOrder="0" numberingType="EQUATION" textWrap="{wrap}" textFlow="BOTH_SIDES" lock="0" dropcapstyle="None" version="Equation Version 60" baseLine="85" textColor="#000000" baseUnit="1000" lineMode="0" font="HYhwpEQ"><hp:sz width="{}" widthRelTo="ABSOLUTE" height="{}" heightRelTo="ABSOLUTE" protect="0"/><hp:pos treatAsChar="{treat}" affectLSpacing="0" flowWithText="{flow}" allowOverlap="{overlap}" holdAnchorAndSO="0" vertRelTo="PARA" horzRelTo="PARA" vertAlign="TOP" horzAlign="LEFT" vertOffset="{}" horzOffset="{}"/><hp:outMargin left="0" right="0" top="0" bottom="0"/><hp:script>{}</hp:script></hp:equation>"##,
        eq.width.max(0),
        eq.height.max(0),
        eq.y,
        eq.x,
        esc(&eq.script),
    );
}

/// hwp5 gso 공통 개체 헤더(20B+): attr(u32)@0, 세로 오프셋@4, 가로 오프셋@8, 폭@12, 높이@16,
/// **z-order@20**. hwp5 `parse_picture_gso`/hwp-render `parse_gso_box`와 동일 레이아웃(역의존
/// 불가라 로컬 복제). z-order는 도형 겹침 순서 — 이를 `zOrder="0"`로 뭉개면 한글이 다중 도형을
/// undefined 순서로 그려 덮개 도형이 내용을 가린다(annual 표지 빈 화면 원인). 헤더가 짧아
/// z-order가 없으면 0.
fn parse_gso_header(data: &[u8]) -> Option<(u32, i32, i32, i32, i32, i32)> {
    if data.len() < 20 {
        return None;
    }
    let rd = |o: usize| i32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
    let zorder = if data.len() >= 24 { rd(20) } else { 0 };
    Some((rd(0) as u32, rd(4), rd(8), rd(12), rd(16), zorder))
}

/// COLORREF(0x00BBGGRR) → "#RRGGBB" (reader `parse_color`의 역).
fn color_hex(c: u32) -> String {
    format!(
        "#{:02X}{:02X}{:02X}",
        c & 0xFF,
        (c >> 8) & 0xFF,
        (c >> 16) & 0xFF
    )
}

// gso 배치/선 스타일 코드 → OWPML 이름 (reader의 vert_rel_to_code/line_style_code 등의 역).
fn vert_rel_to_name(code: u8) -> &'static str {
    match code {
        1 => "PAGE",
        2 => "PARA",
        _ => "PAPER",
    }
}
fn horz_rel_to_name(code: u8) -> &'static str {
    match code {
        1 => "PAGE",
        2 => "COLUMN",
        3 => "PARA",
        _ => "PAPER",
    }
}
fn vert_align_name(code: u8) -> &'static str {
    match code {
        1 => "CENTER",
        2 => "BOTTOM",
        _ => "TOP",
    }
}
fn horz_align_name(code: u8) -> &'static str {
    match code {
        1 => "CENTER",
        2 => "RIGHT",
        _ => "LEFT",
    }
}
fn line_style_name(code: u8) -> &'static str {
    match code {
        1 => "DASH",
        2 => "DOT",
        3 => "DASH_DOT",
        4 => "DASH_DOT_DOT",
        5 => "LONG_DASH",
        _ => "SOLID",
    }
}
fn arrow_name(code: u8) -> &'static str {
    if code == 0 { "NORMAL" } else { "ARROW" }
}

/// 개체 공통 자식(offset/orgSz/curSz/flip/rotationInfo/단위행렬) — 정품 line/pic 스캐폴드 복제.
fn write_obj_scaffold(out: &mut String, w: i32, h: i32, cur_w: i32, cur_h: i32) {
    let _ = write!(
        out,
        r##"<hp:offset x="0" y="0"/><hp:orgSz width="{w}" height="{h}"/><hp:curSz width="{cur_w}" height="{cur_h}"/><hp:flip horizontal="0" vertical="0"/><hp:rotationInfo angle="0" centerX="{}" centerY="{}" rotateimage="1"/><hp:renderingInfo><hc:transMatrix e1="1" e2="0" e3="0" e4="0" e5="1" e6="0"/><hc:scaMatrix e1="1" e2="0" e3="0" e4="0" e5="1" e6="0"/><hc:rotMatrix e1="1" e2="0" e3="0" e4="0" e5="1" e6="0"/></hp:renderingInfo>"##,
        w / 2,
        h / 2,
    );
}

/// 글상자 텍스트: `<hp:drawText><hp:subList>문단들</hp:subList></hp:drawText>`.
/// 모든 paragraph_lists를 하나의 subList로 병합(다단 글상자 v1 근사 — 텍스트 무손실).
/// 필드/책갈피는 write_paragraph 안의 arm이 fieldBegin/bookmark로 함께 방출한다.
#[allow(clippy::too_many_arguments)]
fn write_draw_text(
    out: &mut String,
    doc: &Document,
    g: &GenericControl,
    ids: &mut IdSeq,
    bins: &mut BinCollector,
    width: i32,
    _preserve_linesegs: bool,
    warnings: &mut Vec<String>,
) {
    if g.paragraph_lists.is_empty() {
        return;
    }
    // lastWidth=박스 폭, vertAlign=CENTER(정품 실측). 안쪽 여백(textMargin)도 정품 필수.
    let _ = write!(
        out,
        r##"<hp:drawText lastWidth="{width}" name="" editable="0"><hp:subList id="" textDirection="HORIZONTAL" lineWrap="BREAK" vertAlign="CENTER" linkListIDRef="0" linkListNextIDRef="0" textWidth="0" textHeight="0" hasTextRef="0" hasNumRef="0">"##,
    );
    for list in &g.paragraph_lists {
        for para in &list.paragraphs {
            // 도형 텍스트는 항상 linesegarray를 방출한다(정품 실측 — 한글은 글상자 문단에
            // 줄배치를 항상 담는다). line_segs가 없으면 no-op이라 안전. 본문(전역
            // preserve_linesegs)과 무관하게 강제.
            write_paragraph(out, doc, para, ids, bins, false, true, warnings);
        }
    }
    out.push_str(
        r##"</hp:subList><hp:textMargin left="283" right="283" top="283" bottom="283"/></hp:drawText>"##,
    );
}

/// hwp5 쪽번호 위치 코드 → OWPML pos 속성(reader `build_pgnp` 역매핑).
fn page_num_pos_name(code: u8) -> &'static str {
    match code {
        1 => "TOP_LEFT",
        2 => "TOP_CENTER",
        3 => "TOP_RIGHT",
        4 => "BOTTOM_LEFT",
        5 => "BOTTOM_CENTER",
        6 => "BOTTOM_RIGHT",
        7 => "OUTSIDE_TOP",
        8 => "OUTSIDE_BOTTOM",
        9 => "INSIDE_TOP",
        10 => "INSIDE_BOTTOM",
        _ => "NONE",
    }
}

/// gso 공통 헤더의 attr 비트 + 오프셋으로 `<hp:pos …/>` 를 만든다(⑱ 역매핑 — 쌍 대조 검증).
fn gso_pos_xml(attr: u32, voff: i32, hoff: i32) -> String {
    let treat = attr & 1;
    let vrel = ((attr >> 3) & 0x3) as u8;
    let valign = ((attr >> 5) & 0x7) as u8;
    let hrel = ((attr >> 8) & 0x3) as u8;
    let halign = ((attr >> 10) & 0x7) as u8;
    // 부유 도형(treatAsChar=0)은 flowWithText=0·allowOverlap=1(정품 실측). 본문흐름(=1/0)
    // 이면 한글이 다수 도형을 배치 못 해 빈 화면. 인라인(treatAsChar=1)은 1/0 유지.
    let (flow, overlap) = if treat == 1 { (1, 0) } else { (0, 1) };
    format!(
        r##"<hp:pos treatAsChar="{treat}" affectLSpacing="0" flowWithText="{flow}" allowOverlap="{overlap}" holdAnchorAndSO="0" vertRelTo="{}" horzRelTo="{}" vertAlign="{}" horzAlign="{}" vertOffset="{voff}" horzOffset="{hoff}"/>"##,
        vert_rel_to_name(vrel),
        horz_rel_to_name(hrel),
        vert_align_name(valign),
        horz_align_name(halign),
    )
}

/// 도형 하나를 OWPML 요소로 방출한다(스캐폴드+lineShape+채움+점+선택 drawText+sz/pos).
/// hwpx-출신(Arm A)과 hwp5-출신(write_gso) 모두 이 함수를 거친다.
#[allow(clippy::too_many_arguments)]
fn write_shape_element(
    out: &mut String,
    doc: &Document,
    s: &hwp_model::ShapeGeom,
    ids: &mut IdSeq,
    bins: &mut BinCollector,
    sz: (i32, i32),
    pos_xml: &str,
    zorder: i32,
    text: Option<&GenericControl>,
    preserve_linesegs: bool,
    warnings: &mut Vec<String>,
) {
    let el = match s.kind {
        ShapeKind::Rect => "rect",
        ShapeKind::Ellipse => "ellipse",
        ShapeKind::Line => "line",
        ShapeKind::Polygon => "polygon",
        ShapeKind::Curve => "curve",
        ShapeKind::Arc => "arc",
    };
    // textWrap=IN_FRONT_OF_TEXT: 정품(테스트2.hwpx) 부유 도형 실측. TOP_AND_BOTTOM(본문
    // 흐름 삽입)이면 한글이 다수 도형을 배치 못 해 빈 화면이 된다(실기 확정).
    let _ = write!(
        out,
        r##"<hp:{el} id="{}" zOrder="{zorder}" numberingType="PICTURE" textWrap="IN_FRONT_OF_TEXT" textFlow="BOTH_SIDES" lock="0" dropcapstyle="None" href="" groupLevel="0" instid="{}""##,
        ids.next(),
        ids.next(),
    );
    // 도형별 여는태그 추가 속성(정품 실측): Rect=ratio, Ellipse=호속성 3종, Arc=type.
    match s.kind {
        ShapeKind::Rect => {
            let _ = write!(out, r##" ratio="{}""##, s.round_ratio);
        }
        ShapeKind::Ellipse => {
            out.push_str(r##" intervalDirty="0" hasArcPr="0" arcType="NORMAL""##);
        }
        ShapeKind::Arc => {
            out.push_str(r##" type="NORMAL""##);
        }
        _ => {}
    }
    out.push('>');
    // curSz: 타원/호는 정품이 (0,0)(미리사이즈 없음 표식). 사각형 등은 (w,h) 유지.
    let (cur_w, cur_h) = match s.kind {
        ShapeKind::Ellipse | ShapeKind::Arc => (0, 0),
        _ => (sz.0, sz.1),
    };
    write_obj_scaffold(out, sz.0, sz.1, cur_w, cur_h);
    if s.border_width <= 0 {
        out.push_str(
            r##"<hp:lineShape color="#000000" width="0" style="NONE" endCap="FLAT" headStyle="NORMAL" tailStyle="NORMAL" headfill="1" tailfill="1" headSz="SMALL_SMALL" tailSz="SMALL_SMALL" outlineStyle="NORMAL" alpha="0"/>"##,
        );
    } else {
        let _ = write!(
            out,
            r##"<hp:lineShape color="{}" width="{}" style="{}" endCap="FLAT" headStyle="{}" tailStyle="{}" headfill="1" tailfill="1" headSz="SMALL_SMALL" tailSz="SMALL_SMALL" outlineStyle="NORMAL" alpha="0"/>"##,
            color_hex(s.border_color),
            s.border_width,
            line_style_name(s.border_style),
            arrow_name(s.arrow_start),
            arrow_name(s.arrow_end),
        );
    }
    // fillBrush는 **채움이 있을 때만** 방출한다. 무채움(s.fill=0xFFFF_FFFF)을 불투명
    // 흰색으로 내보내면(㉙ 버그) 투명이어야 할 가이드 도형이 불투명 흰 원반이 되어 한글
    // 에서 뒤 내용을 덮는다(annual 6쪽 링 다이어그램 미렌더 원인 — fill 플래그 대조 확정).
    // 도넛 구멍은 solid 흰색(0x00FFFFFF)이라 fillBrush 유지, 가이드원(무채움)만 투명.
    if let Some(gr) = &s.fill_gradient {
        // reader parse_gradation의 역: type/angle 속성 + color 자식들.
        let _ = write!(
            out,
            r##"<hc:fillBrush><hc:gradation type="{}" angle="{}" centerX="0" centerY="0" step="255" colorNum="{}" stepCenter="50" alpha="0">"##,
            if gr.radial { "RADIAL" } else { "LINEAR" },
            gr.angle_deg.round() as i32,
            gr.stops.len(),
        );
        for (_, c) in &gr.stops {
            let _ = write!(out, r##"<hc:color value="{}"/>"##, color_hex(*c));
        }
        out.push_str("</hc:gradation></hc:fillBrush>");
    } else if s.fill != 0xFFFF_FFFF {
        let _ = write!(
            out,
            r##"<hc:fillBrush><hc:winBrush faceColor="{}" hatchColor="#000000" alpha="0"/></hc:fillBrush>"##,
            color_hex(s.fill),
        );
    }
    // shadow(type=NONE)도 정품 실측 필수 요소.
    out.push_str(r##"<hp:shadow type="NONE" color="#B2B2B2" offsetX="0" offsetY="0" alpha="0"/>"##);
    if let Some(g) = text {
        write_draw_text(out, doc, g, ids, bins, sz.0, preserve_linesegs, warnings);
    }
    // 기하 좌표점은 drawText 뒤(정품 순서). Rect/Ellipse는 bbox 4모서리 pt0~3 —
    // 이 점이 없으면 한글이 도형 외곽을 몰라 렌더하지 않는다(빈 화면 원인).
    match s.kind {
        ShapeKind::Line => {
            let (p0, p1) = if s.points.len() >= 2 {
                (s.points[0], s.points[1])
            } else {
                ((0, 0), (s.w, s.h))
            };
            let _ = write!(
                out,
                r##"<hc:startPt x="{}" y="{}"/><hc:endPt x="{}" y="{}"/>"##,
                p0.0, p0.1, p1.0, p1.1,
            );
        }
        ShapeKind::Polygon | ShapeKind::Curve => {
            for (pi, (px, py)) in s.points.iter().enumerate() {
                let _ = write!(out, r##"<hc:pt{pi} x="{px}" y="{py}"/>"##);
            }
        }
        ShapeKind::Rect => {
            // 사각형은 bbox 4모서리 pt0~3(정품 실측).
            let (w, h) = (sz.0, sz.1);
            let _ = write!(
                out,
                r##"<hc:pt0 x="0" y="0"/><hc:pt1 x="{w}" y="0"/><hc:pt2 x="{w}" y="{h}"/><hc:pt3 x="0" y="{h}"/>"##,
            );
        }
        ShapeKind::Ellipse => {
            // 타원은 중심+축끝점+호각(정품 실측 — pt0~3가 아님). 완전 타원이라 start/end=0.
            let (w, h) = (sz.0, sz.1);
            let (cx, cy) = (w / 2, h / 2);
            let _ = write!(
                out,
                r##"<hc:center x="{cx}" y="{cy}"/><hc:ax1 x="{w}" y="{cy}"/><hc:ax2 x="{cx}" y="0"/><hc:start1 x="0" y="0"/><hc:end1 x="0" y="0"/><hc:start2 x="0" y="0"/><hc:end2 x="0" y="0"/>"##,
            );
        }
        ShapeKind::Arc => {
            // 호는 중심+축끝점 2개(정품 실측). 파싱된 3점(center,ax1,ax2) 사용, 없으면 bbox 근사.
            if s.points.len() >= 3 {
                let (c, a1, a2) = (s.points[0], s.points[1], s.points[2]);
                let _ = write!(
                    out,
                    r##"<hc:center x="{}" y="{}"/><hc:ax1 x="{}" y="{}"/><hc:ax2 x="{}" y="{}"/>"##,
                    c.0, c.1, a1.0, a1.1, a2.0, a2.1,
                );
            } else {
                let (w, h) = (sz.0, sz.1);
                let _ = write!(
                    out,
                    r##"<hc:center x="0" y="0"/><hc:ax1 x="0" y="{h}"/><hc:ax2 x="{w}" y="0"/>"##,
                );
            }
        }
    }
    let _ = write!(
        out,
        r##"<hp:sz width="{}" widthRelTo="ABSOLUTE" height="{}" heightRelTo="ABSOLUTE" protect="0"/>{pos_xml}<hp:outMargin left="0" right="0" top="0" bottom="0"/></hp:{el}>"##,
        sz.0, sz.1,
    );
}

/// hwp5-출신 gso를 방출한다. 텍스트가 있으면 글상자(`<hp:rect>`+drawText, 테두리/채움은
/// SHAPE_COMPONENT 첫 도형 스타일에서 복원), 없으면 장식 도형들을 도형 요소로 방출.
/// 기하/배치는 gso 공통 헤더 + shapes_from_raw(실쌍 대조 검증) — 도형 해석 실패 시 드롭 경고.
#[allow(clippy::too_many_arguments)]
fn write_gso(
    out: &mut String,
    doc: &Document,
    g: &GenericControl,
    ids: &mut IdSeq,
    bins: &mut BinCollector,
    preserve_linesegs: bool,
    warnings: &mut Vec<String>,
) {
    let Some((attr, voff, hoff, w, h, zorder)) = parse_gso_header(&g.data) else {
        warnings.push("DROP: gso 공통 헤더 파싱 실패 — 드롭".to_string());
        return;
    };
    let shapes = hwp_convert::gso::shapes_from_raw(&g.raw_children);
    let has_text = !g.paragraph_lists.is_empty();
    if has_text {
        // 글상자: rect 하나 + 첫 도형의 테두리/채움 스타일(없으면 무테두리).
        let style = shapes.first();
        let rect = hwp_model::ShapeGeom {
            kind: ShapeKind::Rect,
            x: 0,
            y: 0,
            w,
            h,
            points: Vec::new(),
            fill: style.map_or(0xFFFF_FFFF, |s| s.fill),
            fill_gradient: style.and_then(|s| s.fill_gradient.clone()),
            border_color: style.map_or(0xFFFF_FFFF, |s| s.border_color),
            border_width: style.map_or(0, |s| s.border_width),
            round_ratio: style.map_or(0, |s| s.round_ratio),
            border_style: style.map_or(0, |s| s.border_style),
            arrow_start: 0,
            arrow_end: 0,
            anchored: attr & 1 != 0,
        };
        let pos = gso_pos_xml(attr, voff, hoff);
        write_shape_element(
            out,
            doc,
            &rect,
            ids,
            bins,
            (w, h),
            &pos,
            zorder * Z_SCALE,
            Some(g),
            preserve_linesegs,
            warnings,
        );
    } else if shapes.is_empty() {
        warnings.push("DROP: gso 도형 해석 실패(ARC/이미지채움 등) — 드롭".to_string());
    } else {
        // 장식 도형: 도형별 요소. 배치 = gso 오프셋 + 박스 내 도형 오프셋.
        // ★그룹 도형(도넛=회색+흰 구멍 등, 한 gso 다중 도형)은 gso z-order를 공유하면
        // z 충돌 → 한글이 하나만 그리고 나머지를 스킵(도넛 미렌더 원인, 실기 확정).
        // 전체 z를 Z_SCALE 배로 늘리고 도형 인덱스를 더해 고유화(상대 순서 보존).
        for (i, s) in shapes.iter().enumerate() {
            let pos = gso_pos_xml(attr, voff + s.y, hoff + s.x);
            write_shape_element(
                out,
                doc,
                s,
                ids,
                bins,
                (s.w.max(1), s.h.max(1)),
                &pos,
                zorder * Z_SCALE + i as i32,
                None,
                preserve_linesegs,
                warnings,
            );
        }
    }
}

/// gso z-order 스케일 배수 — 그룹 내 도형에 고유 z를 주면서(base*Z_SCALE+index) gso 간
/// 상대 순서를 보존한다. 한 gso 최대 도형 수 여유(<64)로 인접 gso와 충돌 없음.
const Z_SCALE: i32 = 64;

/// hwpx-출신 구조화 도형(ShapeGeom) → OWPML 요소(reader `collect_shape`의 역).
/// 텍스트(paragraph_lists)는 첫 도형에 drawText로 부착한다. 배치(relTo 등)는 ShapeGeom이
/// 보존하지 않아 PAPER 절대 좌표로 근사(x/y는 reader가 pos 오프셋으로 왕복).
#[allow(clippy::too_many_arguments)]
fn write_ir_shapes(
    out: &mut String,
    doc: &Document,
    g: &GenericControl,
    ids: &mut IdSeq,
    bins: &mut BinCollector,
    preserve_linesegs: bool,
    warnings: &mut Vec<String>,
) {
    for (i, s) in g.gso_shapes.iter().enumerate() {
        // 글자처럼(anchored)이면 정품 인라인 관례(PARA/COLUMN), 아니면 PAPER 절대 좌표.
        let (treat, vrel, hrel) = if s.anchored {
            (1, "PARA", "COLUMN")
        } else {
            (0, "PAPER", "PAPER")
        };
        let pos = format!(
            r##"<hp:pos treatAsChar="{treat}" affectLSpacing="0" flowWithText="1" allowOverlap="0" holdAnchorAndSO="0" vertRelTo="{vrel}" horzRelTo="{hrel}" vertAlign="TOP" horzAlign="LEFT" vertOffset="{}" horzOffset="{}"/>"##,
            s.y, s.x,
        );
        let text = if i == 0 { Some(g) } else { None };
        // hwpx-출신 ShapeGeom엔 z-order가 없어 도형 순서로 증가 부여(전부 0보다 개선).
        write_shape_element(
            out,
            doc,
            s,
            ids,
            bins,
            (s.w, s.h),
            &pos,
            i as i32,
            text,
            preserve_linesegs,
            warnings,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn write_table(
    out: &mut String,
    doc: &Document,
    table: &Table,
    ids: &mut IdSeq,
    bins: &mut BinCollector,
    // 표 셀 줄 배치는 전역 옵션과 무관하게 항상 방출한다(아래 참조). 시그니처는
    // 다른 write_* 와 대칭 유지를 위해 남긴다.
    _preserve_linesegs: bool,
    warnings: &mut Vec<String>,
) {
    let cols = table.cols.max(1) as usize;
    let rows = table.rows.max(1) as usize;
    // 그리드 추정 (렌더러와 동일 규칙)
    let mut col_w = vec![0i64; cols];
    let mut row_h = vec![0i64; rows];
    for cell in &table.cells {
        let (c, r) = (cell.col as usize, cell.row as usize);
        if cell.col_span == 1 && c < cols {
            col_w[c] = col_w[c].max(i64::from(cell.width.0));
        }
        if cell.row_span == 1 && r < rows {
            row_h[r] = row_h[r].max(i64::from(cell.height.0));
        }
    }
    let total_w: i64 = col_w.iter().sum();
    let total_h: i64 = row_h.iter().sum();

    let m = table.inner_margins;
    // 표 개체 속성(attr, 표 75): bits0-1=쪽 경계에서 나눔(NONE=0/TABLE=1/CELL=2),
    // bit2=제목 줄 자동 반복, bit3=자동 너비 조정 안 함. 원본(hwp5/hwpx 출신)의 값을
    // 그대로 방출한다 — 하드코딩(CELL/1/0)은 원본이 "나누지 않음(0)"인 표까지 CELL로
    // 강제해 충실도를 깬다. 합성 표(md/synth 출신 — common_data·placement 모두 비어
    // attr=0)만 정품 기본값(CELL+제목반복)으로 폴백한다.
    let synthesized = table.common_data.is_empty() && table.placement.is_none();
    let (page_break, repeat_header, no_adjust) = if synthesized {
        ("CELL", 1u32, 0u32)
    } else {
        let pb = match table.attr & 0b11 {
            1 => "TABLE",
            2 => "CELL",
            _ => "NONE",
        };
        (pb, (table.attr >> 2) & 1, (table.attr >> 3) & 1)
    };
    // 배치 승계(GsoPlacement): hwp5/hwpx 출신은 원본 개체 공통 속성이 담겨 있다.
    // treatAsChar=0(부유) 표만 페이지에 걸쳐 분할되고, treatAsChar=1(글자처럼)은
    // "한 글자"로 배치돼 분할 불가라 하단을 관통한다(정답지 직대조 확정 — GE-8 진범).
    // sz도 원본 개체 폭/높이를 유지한다(행높이 합산 재계산은 페이지 걸침 표에서 과다).
    // 합성 표(md/synth — placement=None)만 인라인 기본값·재계산으로 폴백한다.
    let pl = table.placement.as_ref();
    let sz_w = pl
        .map(|p| p.width)
        .filter(|&w| w > 0)
        .map_or(total_w, i64::from);
    let sz_h = pl
        .map(|p| p.height)
        .filter(|&h| h > 0)
        .map_or(total_h, i64::from);
    let treat_as_char = pl.map_or(1, |p| u32::from(p.treat_as_char));
    let affect_lspacing = pl.map_or(0, |p| u32::from(p.affect_line_spacing));
    let flow_with_text = pl.map_or(1, |p| u32::from(p.flow_with_text));
    let vert_rel = pl.map_or("PARA", |p| vert_rel_to_name(p.vert_rel_to));
    let horz_rel = pl.map_or("PARA", |p| horz_rel_to_name(p.horz_rel_to));
    let vert_align = pl.map_or("TOP", |p| vert_align_name(p.vert_align));
    let horz_align = pl.map_or("LEFT", |p| horz_align_name(p.horz_align));
    let vert_offset = pl.map_or(0, |p| p.vert_offset);
    let horz_offset = pl.map_or(0, |p| p.horz_offset);
    // zOrder·outMargin도 원본 값 승계(픽스처 실측: zOrder 0~10, outMargin 141 —
    // 상수로 뭉개면 도형 겹침 순서·바깥 여백이 원본과 어긋난다). 합성 표는 기존 기본값.
    let z_order = pl.map_or(0, |p| p.z_order);
    let om = pl.map_or([283u16; 4], |p| p.out_margins);
    let _ = write!(
        out,
        r##"<hp:tbl id="{}" zOrder="{z_order}" numberingType="TABLE" textWrap="TOP_AND_BOTTOM" textFlow="BOTH_SIDES" lock="0" dropcapstyle="None" pageBreak="{page_break}" repeatHeader="{repeat_header}" rowCnt="{}" colCnt="{}" cellSpacing="{}" borderFillIDRef="{}" noAdjust="{no_adjust}"><hp:sz width="{sz_w}" widthRelTo="ABSOLUTE" height="{sz_h}" heightRelTo="ABSOLUTE" protect="0"/><hp:pos treatAsChar="{treat_as_char}" affectLSpacing="{affect_lspacing}" flowWithText="{flow_with_text}" allowOverlap="0" holdAnchorAndSO="0" vertRelTo="{vert_rel}" horzRelTo="{horz_rel}" vertAlign="{vert_align}" horzAlign="{horz_align}" vertOffset="{vert_offset}" horzOffset="{horz_offset}"/><hp:outMargin left="{}" right="{}" top="{}" bottom="{}"/><hp:inMargin left="{}" right="{}" top="{}" bottom="{}"/>"##,
        ids.next(),
        table.rows,
        table.cols,
        table.cell_spacing,
        table.border_fill.0.max(1),
        om[0],
        om[1],
        om[2],
        om[3],
        m[0],
        m[1],
        m[2],
        m[3],
    );

    // 행별 그룹화 (셀은 행 우선 순서로 보존되어 있음)
    let mut by_row: BTreeMap<u16, Vec<&Cell>> = BTreeMap::new();
    for cell in &table.cells {
        by_row.entry(cell.row).or_default().push(cell);
    }
    for (_, cells) in by_row {
        out.push_str("<hp:tr>");
        for cell in cells {
            let _ = write!(
                out,
                r##"<hp:tc name="" header="0" hasMargin="0" protect="0" editable="0" dirty="0" borderFillIDRef="{}"><hp:subList id="" textDirection="HORIZONTAL" lineWrap="BREAK" vertAlign="CENTER" linkListIDRef="0" linkListNextIDRef="0" textWidth="0" textHeight="0" hasTextRef="0" hasNumRef="0">"##,
                cell.border_fill.0.max(1),
            );
            for para in &cell.paragraphs {
                // 표 셀 문단은 항상 linesegarray를 방출한다(정품 실측 — 한글 자신의
                // hwp→hwpx 변환·정품 hwpx 모두 표 셀에 줄 배치를 100% 담는다). 쪽에
                // 걸치는 긴 표를 한글이 셀 안에서 나누려면 각 줄의 세로 위치(vertpos)가
                // 필요하다. 이게 없으면 셀이 통째로 원자 취급돼 페이지 하단을 넘쳐
                // 잘린다(GE-8 실기 결함). write_paragraph 내부 가드가 line_segs가 있을
                // 때만 방출하므로, 편집으로 줄 배치를 지운 문단(edit.rs가 clear)·md
                // 출신(줄 배치 없음)은 자동으로 비게 돼 "변조" 경고 위험이 없다.
                write_paragraph(out, doc, para, ids, bins, false, true, warnings);
            }
            let cm = cell.margins;
            let _ = write!(
                out,
                r##"</hp:subList><hp:cellAddr colAddr="{}" rowAddr="{}"/><hp:cellSpan colSpan="{}" rowSpan="{}"/><hp:cellSz width="{}" height="{}"/><hp:cellMargin left="{}" right="{}" top="{}" bottom="{}"/></hp:tc>"##,
                cell.col,
                cell.row,
                cell.col_span,
                cell.row_span,
                cell.width.0,
                cell.height.0,
                cm[0],
                cm[1],
                cm[2],
                cm[3],
            );
        }
        out.push_str("</hp:tr>");
    }
    out.push_str("</hp:tbl>");
}

fn write_picture(
    out: &mut String,
    doc: &Document,
    pic: &Picture,
    ids: &mut IdSeq,
    bins: &mut BinCollector,
    warnings: &mut Vec<String>,
) {
    let Some(item) = bins.register(doc, &pic.bin_ref) else {
        warnings.push(format!(
            "DROP: 그림 데이터를 찾지 못해 드롭: {:?}",
            pic.bin_ref
        ));
        return;
    };
    let (w, h) = (pic.width.0.max(1), pic.height.0.max(1));
    let id = ids.next();
    // 부유(글 앞) 그림 — 도장 등 본문 위에 겹쳐야 하는 개체(treatAsChar=0).
    // 인라인(treatAsChar=1)은 정품 실측(A1_work_report의 로고 hp:pic) 그대로 두고,
    // 부유일 때만 겹침이 되도록 속성을 바꾼다: textWrap=IN_FRONT_OF_TEXT +
    // flowWithText=0 + allowOverlap=1 (테스트2·도형정답지2 정품 부유 도형 실측 조합).
    // SQUARE+allowOverlap=0(구 동작)은 한글이 본문을 밀어내 겹치지 못한다(D1 결함).
    // 위치는 문단 기준(vertRelTo/horzRelTo=PARA)이며 pic의 세로/가로 오프셋과
    // z-순서를 반영한다(insert_seal이 앵커 위로 계산한 값을 그대로 방출).
    if pic.treat_as_char {
        let _ = write!(
            out,
            r##"<hp:pic id="{id}" zOrder="0" numberingType="PICTURE" textWrap="SQUARE" textFlow="BOTH_SIDES" lock="0" dropcapstyle="None" href="" groupLevel="0" instid="{id}" reverse="0"><hp:offset x="0" y="0"/><hp:orgSz width="{w}" height="{h}"/><hp:curSz width="{w}" height="{h}"/><hp:flip horizontal="0" vertical="0"/><hp:rotationInfo angle="0" centerX="{}" centerY="{}" rotateimage="1"/><hp:renderingInfo><hc:transMatrix e1="1" e2="0" e3="0" e4="0" e5="1" e6="0"/><hc:scaMatrix e1="1" e2="0" e3="0" e4="0" e5="1" e6="0"/><hc:rotMatrix e1="1" e2="0" e3="0" e4="0" e5="1" e6="0"/></hp:renderingInfo><hc:img binaryItemIDRef="{item}" bright="0" contrast="0" effect="REAL_PIC" alpha="0"/><hp:imgRect><hc:pt0 x="0" y="0"/><hc:pt1 x="{w}" y="0"/><hc:pt2 x="{w}" y="{h}"/><hc:pt3 x="0" y="{h}"/></hp:imgRect><hp:imgClip left="0" right="{w}" top="0" bottom="{h}"/><hp:inMargin left="0" right="0" top="0" bottom="0"/><hp:imgDim dimwidth="{w}" dimheight="{h}"/><hp:sz width="{w}" widthRelTo="ABSOLUTE" height="{h}" heightRelTo="ABSOLUTE" protect="0"/><hp:pos treatAsChar="1" affectLSpacing="0" flowWithText="1" allowOverlap="0" holdAnchorAndSO="0" vertRelTo="PARA" horzRelTo="PARA" vertAlign="TOP" horzAlign="LEFT" vertOffset="0" horzOffset="0"/><hp:outMargin left="0" right="0" top="0" bottom="0"/></hp:pic>"##,
            w / 2,
            h / 2,
        );
    } else {
        let (voff, hoff, zorder) = (pic.vert_offset, pic.horz_offset, pic.z_order);
        let _ = write!(
            out,
            r##"<hp:pic id="{id}" zOrder="{zorder}" numberingType="PICTURE" textWrap="IN_FRONT_OF_TEXT" textFlow="BOTH_SIDES" lock="0" dropcapstyle="None" href="" groupLevel="0" instid="{id}" reverse="0"><hp:offset x="0" y="0"/><hp:orgSz width="{w}" height="{h}"/><hp:curSz width="{w}" height="{h}"/><hp:flip horizontal="0" vertical="0"/><hp:rotationInfo angle="0" centerX="{}" centerY="{}" rotateimage="1"/><hp:renderingInfo><hc:transMatrix e1="1" e2="0" e3="0" e4="0" e5="1" e6="0"/><hc:scaMatrix e1="1" e2="0" e3="0" e4="0" e5="1" e6="0"/><hc:rotMatrix e1="1" e2="0" e3="0" e4="0" e5="1" e6="0"/></hp:renderingInfo><hc:img binaryItemIDRef="{item}" bright="0" contrast="0" effect="REAL_PIC" alpha="0"/><hp:imgRect><hc:pt0 x="0" y="0"/><hc:pt1 x="{w}" y="0"/><hc:pt2 x="{w}" y="{h}"/><hc:pt3 x="0" y="{h}"/></hp:imgRect><hp:imgClip left="0" right="{w}" top="0" bottom="{h}"/><hp:inMargin left="0" right="0" top="0" bottom="0"/><hp:imgDim dimwidth="{w}" dimheight="{h}"/><hp:sz width="{w}" widthRelTo="ABSOLUTE" height="{h}" heightRelTo="ABSOLUTE" protect="0"/><hp:pos treatAsChar="0" affectLSpacing="0" flowWithText="0" allowOverlap="1" holdAnchorAndSO="0" vertRelTo="PARA" horzRelTo="PARA" vertAlign="TOP" horzAlign="LEFT" vertOffset="{voff}" horzOffset="{hoff}"/><hp:outMargin left="0" right="0" top="0" bottom="0"/></hp:pic>"##,
            w / 2,
            h / 2,
        );
    }
}
