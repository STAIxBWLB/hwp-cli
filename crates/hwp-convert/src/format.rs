//! 서식 편집 — 글자 모양(굵게/기울임/밑줄/취소선/크기/색)·문단 정렬.
//!
//! 매칭된 텍스트 범위의 char_shape_run을 새 CharShape로 가리키게 하거나(글자),
//! 문단의 para_shape를 정렬만 바꾼 ParaShape로 교체한다(문단). 헤더 테이블에
//! shape를 append하는 건 안전하다 — writer가 ID_MAPPINGS 카운트를 `.len()`에서
//! 자동 유도한다. 편집한 문단의 줄 배치는 비워(낡음) writer가 재합성하게 한다.

use hwp_model::{CharShape, CharShapeId, Control, Document, ParaShape, ParaShapeId, Paragraph};

use crate::edit::{find_match, utf16_len};

/// 글자 모양 변경 요청. None인 항목은 기존 값 유지.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CharFormat {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    /// 글자 크기 pt (base_size = pt×100).
    pub size_pt: Option<f32>,
    /// 글자색 COLORREF(0x00BBGGRR).
    pub color: Option<u32>,
}

impl CharFormat {
    pub fn is_empty(&self) -> bool {
        self.bold.is_none()
            && self.italic.is_none()
            && self.underline.is_none()
            && self.strike.is_none()
            && self.size_pt.is_none()
            && self.color.is_none()
    }
}

/// `pattern`과 일치하는 모든 텍스트 범위의 글자 모양을 바꾼다(본문·표 셀·글상자
/// 재귀). 반환값은 적용한 범위 수.
pub fn set_char_format(doc: &mut Document, pattern: &str, fmt: &CharFormat) -> usize {
    if pattern.is_empty() || fmt.is_empty() {
        return 0;
    }
    let Document {
        header, sections, ..
    } = doc;
    let shapes = &mut header.char_shapes;
    let mut n = 0;
    for section in sections.iter_mut() {
        for para in &mut section.paragraphs {
            n += restyle_para(para, pattern, fmt, shapes);
        }
    }
    n
}

fn restyle_para(
    para: &mut Paragraph,
    pattern: &str,
    fmt: &CharFormat,
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
            para_len,
        );
        para.line_segs.clear();
        n += 1;
        start = char_idx + pat_chars;
    }
    // 표 셀·글상자 문단 재귀(치환 편집과 동일 패턴).
    for ctrl in &mut para.controls {
        match ctrl {
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        n += restyle_para(p, pattern, fmt, shapes);
                    }
                }
            }
            Control::Generic(g) => {
                let before = n;
                for list in &mut g.paragraph_lists {
                    for p in &mut list.paragraphs {
                        n += restyle_para(p, pattern, fmt, shapes);
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

/// `[w_start, w_end)` 범위의 char_shape_run을 서식 적용 모양으로 바꾼다.
/// 범위 안 각 run의 기존 모양을 base로 두고 요청 비트만 토글(부분 서식 보존).
fn restyle_range(
    runs: &mut Vec<(u32, CharShapeId)>,
    w_start: u32,
    w_end: u32,
    shapes: &mut Vec<CharShape>,
    fmt: &CharFormat,
    para_len: u32,
) {
    if w_end <= w_start {
        return;
    }
    let start_id = id_at(runs, w_start);
    let end_id = id_at(runs, w_end);
    // 범위 경계 보강(없을 때만).
    if !runs.iter().any(|(p, _)| *p == w_start) {
        runs.push((w_start, start_id));
    }
    if w_end < para_len && !runs.iter().any(|(p, _)| *p == w_end) {
        runs.push((w_end, end_id));
    }
    runs.sort_by_key(|(p, _)| *p);
    // 범위 안 run id를 서식 적용 shape로 교체.
    for (p, id) in runs.iter_mut() {
        if *p >= w_start && *p < w_end {
            let base = shapes.get(id.0 as usize).cloned().unwrap_or_default();
            *id = find_or_insert(shapes, apply_format(base, fmt));
        }
    }
    normalize_runs(runs);
}

/// 위치 `pos`에서 활성인 char_shape id(= pos 이하 마지막 run).
fn id_at(runs: &[(u32, CharShapeId)], pos: u32) -> CharShapeId {
    runs.iter()
        .rev()
        .find(|(p, _)| *p <= pos)
        .map(|(_, id)| *id)
        .unwrap_or(CharShapeId(0))
}

/// run 불변식 재수립: 정렬·동일 위치 제거·인접 동일 id 병합·첫 run pos=0.
fn normalize_runs(runs: &mut Vec<(u32, CharShapeId)>) {
    runs.sort_by_key(|(p, _)| *p);
    let mut out: Vec<(u32, CharShapeId)> = Vec::with_capacity(runs.len());
    for &(pos, id) in runs.iter() {
        match out.last_mut() {
            Some(last) if last.0 == pos => last.1 = id, // 같은 위치: 나중 것 유지
            Some(last) if last.1 == id => {}            // 인접 동일 id 제거
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

/// base 모양에 요청 서식을 적용한 새 모양(요청 항목만 바꿈).
fn apply_format(mut cs: CharShape, fmt: &CharFormat) -> CharShape {
    if let Some(b) = fmt.bold {
        toggle(&mut cs.attr, 1 << 1, b);
    }
    if let Some(i) = fmt.italic {
        toggle(&mut cs.attr, 1, i);
    }
    if let Some(u) = fmt.underline {
        cs.attr &= !(0x3 << 2); // 밑줄 종류 비트 2~3
        if u {
            cs.attr |= 1 << 2; // 1 = 글자 아래
        }
    }
    if let Some(s) = fmt.strike {
        cs.attr &= !(0x7 << 18); // 취소선 비트 18~20 (바이트 왕복 보존)
        if s {
            cs.attr |= 1 << 18;
        }
        cs.strike = s; // 의미 플래그 — has_strike/렌더/hwpx SOLID 쓰기 근거
    }
    if let Some(sz) = fmt.size_pt {
        cs.base_size = (sz * 100.0).round().max(100.0) as i32;
    }
    if let Some(c) = fmt.color {
        cs.text_color = c;
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

/// `pattern`을 가진 문단의 정렬을 바꾼다(본문·표 셀·글상자 재귀).
/// align: 0=양쪽, 1=왼쪽, 2=오른쪽, 3=가운데, 4=배분, 5=나눔. 반환=바꾼 문단 수.
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
        // 정렬은 attr1 비트 2~4.
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
                    // 내용이 바뀐 개체의 원문 XML은 낡았다 — stale 방출 금지.
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
}

impl ParaProps {
    pub fn is_empty(&self) -> bool {
        self.line_spacing.is_none()
            && self.indent.is_none()
            && self.margin_left.is_none()
            && self.margin_right.is_none()
            && self.spacing_top.is_none()
            && self.spacing_bottom.is_none()
    }
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
    // HWPUNIT → IR (hwp5 PARA_SHAPE) unit conversion.
    let props = ParaProps {
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
    };
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
        let mut ps = pshapes
            .get(para.para_shape.0 as usize)
            .cloned()
            .unwrap_or_default();
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
        para.para_shape = find_or_insert_para(pshapes, ps);
        para.line_segs.clear();
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
                    // 내용이 바뀐 개체의 원문 XML은 낡았다 — stale 방출 금지.
                    g.hwpx_raw_xml = None;
                }
            }
            _ => {}
        }
    }
    n
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
            },
        );
        assert!(cs.is_bold() && cs.is_italic() && cs.has_underline() && cs.has_strike());
        assert_eq!(cs.base_size, 1600);
        assert_eq!(cs.text_color, 0x0000_00FF);
        // 끄기: bold만 false.
        let off = apply_format(
            cs.clone(),
            &CharFormat {
                bold: Some(false),
                ..Default::default()
            },
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
        let id2 = find_or_insert(&mut shapes, a); // 동일 → 재사용
        assert_eq!(id2, CharShapeId(1));
        assert_eq!(shapes.len(), 2);
    }

    #[test]
    fn restyle_range_부분범위() {
        // runs=[(0,id0)], 범위 [3,6)에 굵게 적용 → [(0,0),(3,new),(6,0)].
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
            12, // para_len
        );
        assert_eq!(runs.len(), 3, "경계 분할: {runs:?}");
        assert_eq!(runs[0], (0, CharShapeId(0)));
        assert_eq!(runs[1].0, 3);
        assert_eq!(runs[2], (6, CharShapeId(0)), "범위 뒤 원래 모양 복원");
        assert!(shapes[runs[1].1.0 as usize].is_bold());
        // 첫 run은 항상 pos 0.
        assert_eq!(runs[0].0, 0);
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
        // 매칭 범위가 굵게+색 모양을 가리키는 run이 새로 생겨야 한다.
        // (첫 문단은 secd/cold 컨트롤 16 WCHAR가 앞에 있어 위치는 고정 아님.)
        let styled = para.char_shape_runs.iter().any(|(_, id)| {
            let cs = &doc.header.char_shapes[id.0 as usize];
            cs.is_bold() && cs.text_color == 0x0000_00FF
        });
        assert!(styled, "굵게+색 run 존재: {:?}", para.char_shape_runs);
        // 범위 뒤는 원래 모양(0)으로 복원.
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
        let n = set_para_align(&mut doc, "가운데", 3); // 3=가운데
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
}
