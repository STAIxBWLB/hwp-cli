//! 텍스트 셰이핑.
//!
//! 파이프라인: 문자 모양 경계 → HWP 언어 분류 재분할 → 폰트 해석 →
//! rustybuzz 셰이핑 → 자간/장평 후처리.

use std::collections::HashMap;
use std::sync::Arc;

use hwp_model::{CharShape, Document, HwpChar, LANG_COUNT, Paragraph, ctrl_char};

use crate::fonts::{FontStore, LoadedFont};
use crate::issues::{RenderIssueAccumulator, RenderIssueCode};

/// 셰이핑된 글리프 하나 (단위: pt).
#[derive(Debug, Clone, Copy)]
pub struct Glyph {
    pub id: u16,
    pub x_advance: f32,
    pub x_offset: f32,
    pub y_offset: f32,
    /// 원문 문자 (셰이핑 cluster 기준). 합자·재배열 시 위치 인덱스와 어긋나므로
    /// PDF ToUnicode는 위치 추정 대신 이 값을 우선 쓴다.
    pub ch: Option<char>,
}

/// 같은 (폰트, 크기, 스타일)로 셰이핑된 글리프 런.
pub struct ShapedRun {
    pub font: Arc<LoadedFont>,
    pub size_pt: f32,
    /// 장평 (1.0 = 100%)
    pub x_scale: f32,
    pub color: u32,
    pub bold: bool,
    pub italic: bool,
    /// 밑줄 (글자 아래)
    pub underline: bool,
    /// 취소선
    pub strike: bool,
    /// 밑줄 색 (COLORREF, 0xFFFFFFFF = 글자색 따름)
    pub underline_color: u32,
    /// 글자 음영(배경 하이라이트) 색 (COLORREF, 0xFFFFFFFF = 없음)
    pub shade_color: u32,
    /// 그림자 색 (Some이면 그림자 그림)
    pub shadow: Option<u32>,
    /// 외곽선(빈 글자 — 채움 없이 윤곽선만)
    pub outline: bool,
    /// 양각(3D 돋움)
    pub emboss: bool,
    /// 음각(3D 새김)
    pub engrave: bool,
    pub glyphs: Vec<Glyph>,
    pub width_pt: f32,
    pub text: String,
    /// 이 런의 첫 글자 WCHAR 위치 (줄바꿈 시 글리프→WCHAR 매핑용 — lineseg 합성).
    pub start_wchar: u32,
}

impl ShapedRun {
    /// 글리프 [start, end) 구간으로 부분 런을 만든다 (줄바꿈용).
    /// CJK는 1글자=1글리프라 안전하다. 라틴 합자 분리는 허용 오차.
    pub fn slice(&self, start: usize, end: usize) -> ShapedRun {
        let glyphs: Vec<Glyph> = self.glyphs[start..end].to_vec();
        let width_pt = glyphs.iter().map(|g| g.x_advance).sum();
        ShapedRun {
            font: self.font.clone(),
            size_pt: self.size_pt,
            x_scale: self.x_scale,
            color: self.color,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            strike: self.strike,
            underline_color: self.underline_color,
            shade_color: self.shade_color,
            shadow: self.shadow,
            outline: self.outline,
            emboss: self.emboss,
            engrave: self.engrave,
            glyphs,
            width_pt,
            text: String::new(), // 부분 런의 원문 추적은 PDF 백엔드(M7)에서
            start_wchar: self.start_wchar + start as u32, // CJK 1글자=1글리프 가정
        }
    }
}

/// 인라인 항목: 글리프 런 또는 고정 폭 진행(탭).
pub enum InlineItem {
    Run(ShapedRun),
    /// 다음 탭 위치까지 진행 (v1: 고정 간격)
    Tab,
    /// 강제 줄바꿈 (CharCtrl LINE_BREAK=10) — 같은 문단 안에서 줄을 나눈다(코드블록 등).
    /// 값 = 줄바꿈 다음 줄이 시작하는 WCHAR 위치(lineseg 합성의 text_start용).
    LineBreak(u32),
}

/// 유니코드 → HWP 7언어 슬롯 분류 (U3 — 경계 문자는 실측 보정 예정).
fn lang_slot_of(c: char) -> usize {
    match c as u32 {
        // 한글 음절/자모/호환 자모
        0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F => 0,
        // CJK 한자
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF => 2,
        // 가나
        0x3040..=0x30FF | 0x31F0..=0x31FF => 3,
        // 라틴/숫자/기본 구두점 — 영문 슬롯
        0x0000..=0x024F => 1,
        // 그 외 기호
        _ => 5,
    }
}

/// 문단의 (wchar 위치 → 문자 모양 ID) 해석.
fn shape_id_at(para: &Paragraph, pos: u32) -> u16 {
    para.char_shape_runs
        .iter()
        .rev()
        .find(|(start, _)| *start <= pos)
        .map(|(_, id)| id.0)
        .unwrap_or(0)
}

/// 문단 전체(또는 wchar 구간)를 셰이핑한다.
/// `range`는 WCHAR 오프셋 [start, end) — lineseg 줄 단위 분할에 사용.
pub fn shape_range(
    store: &mut FontStore,
    doc: &Document,
    para: &Paragraph,
    range: (u32, u32),
    warnings: &mut RenderIssueAccumulator,
) -> Vec<InlineItem> {
    shape_range_notes(store, doc, para, range, &HashMap::new(), warnings)
}

/// `shape_range`에 각주/미주 마커(ctrl_index→번호)를 더한 버전. 본문 경로만 사용.
pub fn shape_range_notes(
    store: &mut FontStore,
    doc: &Document,
    para: &Paragraph,
    range: (u32, u32),
    marks: &HashMap<u32, u32>,
    warnings: &mut RenderIssueAccumulator,
) -> Vec<InlineItem> {
    shape_range_dynamic(store, doc, para, range, marks, None, warnings)
}

/// `shape_range_notes` with page-aware automatic-number substitution.
///
/// Page-kind `atno` controls are resolved at layout time because the same
/// header/footer paragraph is reused on every page. Other automatic-number
/// kinds remain untouched for their dedicated renderers.
pub(crate) fn shape_range_page(
    store: &mut FontStore,
    doc: &Document,
    para: &Paragraph,
    range: (u32, u32),
    marks: &HashMap<u32, u32>,
    page_number: Option<u32>,
    warnings: &mut RenderIssueAccumulator,
) -> Vec<InlineItem> {
    shape_range_dynamic(store, doc, para, range, marks, page_number, warnings)
}

fn shape_range_dynamic(
    store: &mut FontStore,
    doc: &Document,
    para: &Paragraph,
    range: (u32, u32),
    marks: &HashMap<u32, u32>,
    page_number: Option<u32>,
    warnings: &mut RenderIssueAccumulator,
) -> Vec<InlineItem> {
    // 1. (문자모양, 언어) 경계로 텍스트 조각 수집
    struct Piece {
        shape_id: u16,
        lang: usize,
        text: String,
        start: u32,
    }
    let mut pieces: Vec<Piece> = Vec::new();
    let mut items: Vec<(usize, InlineItem)> = Vec::new(); // (pieces 삽입 위치, 탭)
    let mut pos = 0u32;

    for ch in &para.chars {
        let w = ch.wchar_width();
        let in_range = pos >= range.0 && pos < range.1;
        if in_range {
            match ch {
                HwpChar::Text(c) => {
                    let shape_id = shape_id_at(para, pos);
                    let lang = lang_slot_of(*c);
                    match pieces.last_mut() {
                        Some(last) if last.shape_id == shape_id && last.lang == lang => {
                            last.text.push(*c);
                        }
                        _ => pieces.push(Piece {
                            shape_id,
                            lang,
                            text: c.to_string(),
                            start: pos,
                        }),
                    }
                }
                HwpChar::InlineCtrl { code, .. } if *code == ctrl_char::TAB => {
                    items.push((pieces.len(), InlineItem::Tab));
                    // 탭 뒤는 새 조각으로
                    pieces.push(Piece {
                        shape_id: shape_id_at(para, pos + 8),
                        lang: 0,
                        text: String::new(),
                        start: pos + 8,
                    });
                }
                // 각주/미주 앵커: 윗첨자 번호 마커를 본문 위치에 넣는다.
                HwpChar::ExtCtrl {
                    ctrl_index: Some(ci),
                    ..
                } if marks.contains_key(ci) => {
                    if let Some(run) = note_mark_run(store, doc, para, pos, marks[ci]) {
                        items.push((pieces.len(), InlineItem::Run(run)));
                    }
                }
                // 쪽 자동번호(atno): 같은 머리말/꼬리말 문단도 페이지마다 현재 번호로
                // 다시 셰이핑한다. 앞뒤 본문 조각이 합쳐져 순서가 뒤바뀌지 않도록
                // 합성 런 뒤에 빈 조각 경계를 둔다.
                HwpChar::ExtCtrl {
                    ctrl_id,
                    ctrl_index: Some(ci),
                    ..
                } if ctrl_id == b"atno" && page_number.is_some() => {
                    let number = page_number.expect("is_some");
                    if let Some(auto) = page_auto_control(para, *ci as usize) {
                        let text = crate::page_number::format_page_number(
                            number,
                            auto.format,
                            auto.user_char,
                            auto.prefix_char,
                            auto.suffix_char,
                            warnings,
                        );
                        if let Some(run) =
                            auto_number_run(store, doc, para, pos, &text, auto.superscript)
                        {
                            items.push((pieces.len(), InlineItem::Run(run)));
                            pieces.push(Piece {
                                shape_id: shape_id_at(para, pos + 8),
                                lang: 0,
                                text: String::new(),
                                start: pos + 8,
                            });
                        }
                    }
                }
                // 강제 줄바꿈: 같은 문단 안에서 줄을 나눈다(코드블록·shift+enter).
                HwpChar::CharCtrl(code) if *code == ctrl_char::LINE_BREAK => {
                    items.push((pieces.len(), InlineItem::LineBreak(pos + 1)));
                    // 줄바꿈 뒤 텍스트는 새 조각으로(앞 조각에 병합 방지).
                    pieces.push(Piece {
                        shape_id: shape_id_at(para, pos + 1),
                        lang: 0,
                        text: String::new(),
                        start: pos + 1,
                    });
                }
                _ => {} // 그 외 컨트롤은 v1 렌더 제외
            }
        }
        pos += w;
    }

    // 2. 조각별 셰이핑
    let mut out = Vec::new();
    let mut item_iter = items.into_iter().peekable();
    for (piece_idx, piece) in pieces.into_iter().enumerate() {
        while let Some((at, _)) = item_iter.peek() {
            if *at <= piece_idx {
                let (_, item) = item_iter.next().expect("peek 확인됨");
                out.push(item);
            } else {
                break;
            }
        }
        if piece.text.is_empty() {
            continue;
        }
        let shape = doc.header.char_shapes.get(piece.shape_id as usize);
        let runs = shape_piece(store, doc, shape, piece.lang, &piece.text, piece.start);
        if runs.is_empty() {
            warnings.push(
                RenderIssueCode::ShapingFailed,
                piece.text.len().to_le_bytes(),
            );
        }
        for run in runs {
            out.push(InlineItem::Run(run));
        }
    }
    for (_, item) in item_iter {
        out.push(item);
    }
    out
}

fn page_auto_control(
    para: &Paragraph,
    control_index: usize,
) -> Option<crate::page_number::AutoNumber> {
    let hwp_model::Control::Generic(control) = para.controls.get(control_index)? else {
        return None;
    };
    if control.ctrl_id != *b"atno" {
        return None;
    }
    let auto = crate::page_number::parse_atno(&control.data)?;
    (auto.kind == 0).then_some(auto)
}

fn auto_number_run(
    store: &mut FontStore,
    doc: &Document,
    para: &Paragraph,
    pos: u32,
    text: &str,
    superscript: bool,
) -> Option<ShapedRun> {
    let base_id = shape_id_at(para, pos);
    let mut shape = doc
        .header
        .char_shapes
        .get(base_id as usize)
        .cloned()
        .unwrap_or_else(|| CharShape {
            ratios: [100; LANG_COUNT],
            rel_sizes: [100; LANG_COUNT],
            base_size: 1000,
            shade_color: 0xFFFF_FFFF,
            ..CharShape::default()
        });
    if superscript {
        shape.attr |= 1 << 15;
    }
    let lang = if text.chars().any(|c| ('가'..='힣').contains(&c)) {
        0
    } else {
        1
    };
    // Keep the whole decorated value in one run. Missing decoration glyphs
    // become tofu rather than making later characters disappear.
    let face_id = shape.face_ids.get(lang).copied().unwrap_or(0);
    let font = store.resolve(doc, lang, face_id)?;
    shape_with_font(&font, &shape, lang, text, pos, shape.is_bold())
}

/// 한 조각을 셰이핑한다. 해석된 주 글꼴이 일부 글자를 갖지 않으면(.notdef) 그
/// 글자만 커버리지 폴백 글꼴로 바꿔, 글꼴 경계마다 별도 [`ShapedRun`]으로 나눈다
/// (예: macOS "휴먼명조"에 ❍(U+274D)가 없을 때 함초롬/Noto로 폴백 — 두부(□) 방지).
fn shape_piece(
    store: &mut FontStore,
    doc: &Document,
    shape: Option<&CharShape>,
    lang: usize,
    text: &str,
    start_wchar: u32,
) -> Vec<ShapedRun> {
    let default_shape = CharShape::default();
    let cs = shape.unwrap_or(&default_shape);
    let face_id = cs.face_ids.get(lang).copied().unwrap_or(0);
    let Some(primary) = store.resolve(doc, lang, face_id) else {
        return Vec::new();
    };

    // 요청 글꼴이 굵은(heavy) 계열인데 대체 글꼴엔 굵은 페이스가 없으면 faux-bold로
    // 보강한다(예: HY견고딕/헤드라인M → 함초롬돋움 regular).
    let face_name = doc
        .header
        .fonts
        .get(lang)
        .and_then(|f| f.get(face_id as usize))
        .map(|f| f.name.as_str())
        .unwrap_or("");
    let bold = cs.is_bold() || is_heavy_name(face_name);

    // 글자별 글꼴 배정: primary가 글리프를 가지면 primary, 아니면 커버리지 폴백.
    let primary_face = rustybuzz::ttf_parser::Face::parse(&primary.data, primary.index).ok();
    let primary_covers = |c: char| {
        // 공백·제어는 항상 primary 사용(불필요한 폴백 분할 방지).
        c.is_whitespace()
            || primary_face
                .as_ref()
                .and_then(|f| f.glyph_index(c))
                .is_some_and(|g| g.0 != 0)
    };

    // (글꼴, 텍스트, 시작 wchar) 세그먼트로 분할. start_wchar는 UTF-16 코드유닛
    // 오프셋이므로 문자마다 len_utf16()(BMP=1, 그 외=2)만큼 진행한다(비-BMP 글자에서
    // start_wchar 어긋나 링크범위·lineseg 매핑이 깨지지 않게).
    let mut segments: Vec<(Arc<LoadedFont>, String, u32)> = Vec::new();
    let mut cur = start_wchar;
    for c in text.chars() {
        let font = if primary_covers(c) {
            primary.clone()
        } else {
            store.font_covering(c).unwrap_or_else(|| primary.clone())
        };
        match segments.last_mut() {
            Some((f, t, _)) if Arc::ptr_eq(f, &font) => t.push(c),
            _ => segments.push((font, c.to_string(), cur)),
        }
        cur += c.len_utf16() as u32;
    }

    segments
        .into_iter()
        .filter_map(|(font, seg_text, seg_start)| {
            shape_with_font(&font, cs, lang, &seg_text, seg_start, bold)
        })
        .collect()
}

/// 주어진 글꼴 하나로 텍스트를 셰이핑해 [`ShapedRun`]을 만든다(첨자·자간·장평·
/// 음영/그림자/외곽선/양각/음각 효과 필드까지 채운다). `bold`는 호출부가 결정
/// (요청이 굵음이거나, 굵은 페이스 없는 heavy 글꼴이라 faux-bold가 필요할 때 true).
fn shape_with_font(
    font: &Arc<LoadedFont>,
    cs: &CharShape,
    lang: usize,
    text: &str,
    start_wchar: u32,
    bold: bool,
) -> Option<ShapedRun> {
    let face = rustybuzz::Face::from_slice(&font.data, font.index)?;
    let upem = face.units_per_em() as f32;

    // 크기: 기준 크기 × 언어별 상대 크기%
    let base = if cs.base_size > 0 { cs.base_size } else { 1000 };
    let rel = cs.rel_sizes.get(lang).copied().unwrap_or(100).max(1);
    let full_size = (base as f32 / 100.0) * (rel as f32 / 100.0);
    // 위/아래 첨자: 크기 ~65% 축소 + 베이스라인 이동(원 크기 기준). 수동 글자위치(offsets%) 가산.
    let (sup, sub) = (cs.is_superscript(), cs.is_subscript());
    let size_pt = if sup || sub {
        full_size * 0.65
    } else {
        full_size
    };
    let scale = size_pt / upem;
    let y_raise = {
        let mut r = full_size * cs.char_offset(lang) as f32 / 100.0;
        if sup {
            r += full_size * 0.34;
        }
        if sub {
            r -= full_size * 0.16;
        }
        r
    };

    // 자간: 글자 크기 기준 % (U4 — 반올림 방식은 실측 보정 예정)
    let spacing_pt = size_pt * cs.spacings.get(lang).copied().unwrap_or(0) as f32 / 100.0;
    let x_scale = cs.ratios.get(lang).copied().unwrap_or(100).max(1) as f32 / 100.0;

    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(text);
    let output = rustybuzz::shape(&face, &[], buffer);

    let mut glyphs = Vec::with_capacity(output.len());
    let mut width = 0.0f32;
    for (info, gpos) in output.glyph_infos().iter().zip(output.glyph_positions()) {
        let advance = gpos.x_advance as f32 * scale * x_scale + spacing_pt;
        glyphs.push(Glyph {
            id: info.glyph_id as u16,
            x_advance: advance,
            x_offset: gpos.x_offset as f32 * scale * x_scale,
            y_offset: gpos.y_offset as f32 * scale + y_raise,
            ch: text
                .get(info.cluster as usize..)
                .and_then(|s| s.chars().next()),
        });
        width += advance;
    }

    Some(ShapedRun {
        font: font.clone(),
        size_pt,
        x_scale,
        color: cs.text_color,
        bold,
        italic: cs.is_italic(),
        underline: cs.has_underline(),
        strike: cs.has_strike(),
        underline_color: cs.underline_color,
        shade_color: if cs.has_shade() {
            cs.shade_color
        } else {
            0xFFFF_FFFF
        },
        shadow: cs.has_shadow().then_some(cs.shadow_color),
        outline: cs.has_outline(),
        emboss: cs.is_emboss(),
        engrave: cs.is_engrave(),
        glyphs,
        width_pt: width,
        text: text.to_string(),
        start_wchar,
    })
}

/// 글꼴 이름이 굵은(heavy/bold) 계열인지 — 대체 글꼴 faux-bold 판단용.
fn is_heavy_name(name: &str) -> bool {
    const HEAVY: &[&str] = &[
        "견고딕",
        "견명조",
        "헤드라인",
        "굵은",
        "Heavy",
        "Black",
        "ExtraBold",
        "Ultra",
        "Bold",
    ];
    HEAVY.iter().any(|k| name.contains(k))
}

/// 하이퍼링크 색(COLORREF 0x00BBGGRR = 파랑).
const LINK_BLUE: u32 = 0x00CC_0000;

/// 문단 안 하이퍼링크(`%hlk` 필드)의 링크 텍스트 WCHAR 범위 [start, end) 목록.
/// FIELD_START(ExtCtrl code 3, ctrl_id %hlk) ~ FIELD_END(InlineCtrl code 4) 사이.
pub fn hyperlink_ranges(para: &Paragraph) -> Vec<(u32, u32)> {
    const FIELD_START: u16 = 3;
    const FIELD_END: u16 = 4;
    let mut ranges = Vec::new();
    let mut wpos = 0u32;
    for (i, ch) in para.chars.iter().enumerate() {
        if let HwpChar::ExtCtrl { code, ctrl_id, .. } = ch
            && *code == FIELD_START
            && ctrl_id == b"%hlk"
        {
            let start = wpos + ch.wchar_width();
            let mut end = start;
            for next in &para.chars[i + 1..] {
                if let HwpChar::InlineCtrl { code, .. } = next
                    && *code == FIELD_END
                {
                    break;
                }
                end += next.wchar_width();
            }
            if end > start {
                ranges.push((start, end));
            }
        }
        wpos += ch.wchar_width();
    }
    ranges
}

/// 링크 범위에 드는 Run에 밑줄+링크색을 입힌다(필드 경계 컨트롤이 조각을 끊어
/// 링크 텍스트는 자체 Run이므로 start_wchar로 판정). 빈 범위면 무동작.
pub fn apply_link_style(items: &mut [InlineItem], links: &[(u32, u32)]) {
    if links.is_empty() {
        return;
    }
    for item in items.iter_mut() {
        if let InlineItem::Run(run) = item
            && links
                .iter()
                .any(|&(a, b)| run.start_wchar >= a && run.start_wchar < b)
        {
            run.underline = true;
            run.color = LINK_BLUE;
            run.underline_color = LINK_BLUE;
        }
    }
}

/// 임의 문자열을 기본 글자모양으로 셰이핑한다(수식 근사 등 합성 텍스트용).
/// 한글이 섞이면 한글 슬롯, 아니면 라틴 슬롯 폰트를 쓴다.
pub fn shape_plain(
    store: &mut FontStore,
    doc: &Document,
    text: &str,
    size_pt: f32,
    color: u32,
    italic: bool,
) -> Option<ShapedRun> {
    let cs = CharShape {
        base_size: (size_pt * 100.0) as i32,
        ratios: [100; LANG_COUNT],
        rel_sizes: [100; LANG_COUNT],
        attr: u32::from(italic), // bit0 = 기울임(수식 변수 이탤릭)
        text_color: color,
        // 0xFFFFFFFF=음영 없음. 기본 0이면 "불투명 검정 배경"으로 해석돼 마커가
        // 검은 박스로 덮인다(각주·수식·목록 마커 공통 — 검은바 트랩).
        shade_color: 0xFFFF_FFFF,
        ..CharShape::default()
    };
    let lang = if text.chars().any(|c| ('가'..='힣').contains(&c)) {
        0
    } else {
        1
    };
    // 합성 텍스트(수식/마커)는 단일 Run만 배치하므로 주 글꼴로 통짜 셰이핑한다.
    // shape_piece의 글자별 폴백은 여러 Run으로 쪼개지는데, 여기서 첫 세그먼트만 취하면
    // 나머지 글자가 사라진다(내용 누락). 미지원 글자는 tofu로 두되 누락은 막는다
    // (글자별 폴백은 본문 shape_range 경로 전용).
    let face_id = cs.face_ids.get(lang).copied().unwrap_or(0);
    let font = store.resolve(doc, lang, face_id)?;
    shape_with_font(&font, &cs, lang, text, 0, cs.is_bold())
}

/// 각주/미주 본문 마커(윗첨자 번호). 주변 글자모양을 따라 ~65% 크기로 줄이고
/// 베이스라인을 위로 올린다. 글리프↔WCHAR 매핑(start_wchar)은 앵커 위치로 둔다.
fn note_mark_run(
    store: &mut FontStore,
    doc: &Document,
    para: &Paragraph,
    pos: u32,
    number: u32,
) -> Option<ShapedRun> {
    let base_id = shape_id_at(para, pos);
    let base = doc.header.char_shapes.get(base_id as usize).cloned();
    let base_size = base
        .as_ref()
        .map(|c| if c.base_size > 0 { c.base_size } else { 1000 })
        .unwrap_or(1000);
    let mut cs = base.unwrap_or_else(|| CharShape {
        ratios: [100; LANG_COUNT],
        rel_sizes: [100; LANG_COUNT],
        shade_color: 0xFFFF_FFFF,
        ..CharShape::default()
    });
    cs.base_size = ((base_size as f32) * 0.65).max(500.0) as i32;
    cs.attr = 0; // 마커엔 굵게/기울임 등 합성 효과 불필요
    let text = number.to_string();
    // 마커는 단일 런(숫자) — 첫 세그먼트만 취한다.
    let mut run = shape_piece(store, doc, Some(&cs), 1, &text, pos)
        .into_iter()
        .next()?;
    // 윗첨자: 위로 올림(y-up 좌표, 백엔드가 baseline-y에서 y_offset만큼 올림).
    let raise = (base_size as f32 / 100.0) * 0.34;
    for g in &mut run.glyphs {
        g.y_offset += raise;
    }
    Some(run)
}

#[cfg(test)]
mod link_tests {
    use super::*;
    use crate::fonts::LoadedFont;
    use std::sync::Arc;

    fn field_start() -> HwpChar {
        HwpChar::ExtCtrl {
            code: 3,
            ctrl_id: *b"%hlk",
            payload: vec![0; 12],
            ctrl_index: Some(0),
        }
    }
    fn field_end() -> HwpChar {
        HwpChar::InlineCtrl {
            code: 4,
            payload: vec![0; 12],
        }
    }

    #[test]
    fn 하이퍼링크_범위() {
        let para = Paragraph {
            chars: vec![
                HwpChar::Text('a'),
                field_start(),
                HwpChar::Text('네'),
                HwpChar::Text('이'),
                HwpChar::Text('버'),
                field_end(),
                HwpChar::Text('b'),
            ],
            ..Paragraph::default()
        };
        // a=1 WCHAR, ExtCtrl=8 → 링크 시작 1+8=9, '네이버'=3 → (9, 12).
        assert_eq!(hyperlink_ranges(&para), vec![(9, 12)]);
        let plain = Paragraph {
            chars: vec![HwpChar::Text('a')],
            ..Paragraph::default()
        };
        assert!(hyperlink_ranges(&plain).is_empty());
    }

    fn run_at(start: u32) -> InlineItem {
        InlineItem::Run(ShapedRun {
            font: Arc::new(LoadedFont {
                data: Arc::new(Vec::new()),
                index: 0,
                family: String::new(),
            }),
            size_pt: 10.0,
            x_scale: 1.0,
            color: 0,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            underline_color: 0xFFFF_FFFF,
            shade_color: 0xFFFF_FFFF,
            shadow: None,
            outline: false,
            emboss: false,
            engrave: false,
            glyphs: Vec::new(),
            width_pt: 0.0,
            text: String::new(),
            start_wchar: start,
        })
    }

    #[test]
    fn 링크_스타일_적용() {
        let mut items = vec![run_at(0), run_at(9), run_at(20)];
        apply_link_style(&mut items, &[(9, 12)]);
        let und: Vec<bool> = items
            .iter()
            .map(|i| matches!(i, InlineItem::Run(r) if r.underline))
            .collect();
        assert_eq!(und, vec![false, true, false]); // 9만 링크 범위
        if let InlineItem::Run(r) = &items[1] {
            assert_eq!(r.color, LINK_BLUE);
            assert_eq!(r.underline_color, LINK_BLUE);
        }
        // 빈 범위는 무동작.
        let mut items2 = vec![run_at(9)];
        apply_link_style(&mut items2, &[]);
        assert!(matches!(&items2[0], InlineItem::Run(r) if !r.underline));
    }

    #[test]
    fn shape_plain_전체_텍스트_보존() {
        // 회귀 가드: shape_plain 이 (구버전처럼) 폴백 첫 세그먼트만 취해 나머지 글자를
        // 버리지 않고 전체 텍스트를 셰이핑해야 한다. 폰트 부재 환경은 resolve→None 으로
        // 스킵(폰트 있는 CI에서 검출).
        let doc = hwp_convert::from_markdown("x"); // default_header(함초롬바탕) 폰트 포함
        let mut store = FontStore::new();
        let text = "abc 123 가나다";
        if let Some(run) = shape_plain(&mut store, &doc, text, 10.0, 0, false) {
            assert_eq!(run.text, text, "전체 텍스트 보존(세그먼트 누락 없음)");
            assert!(!run.glyphs.is_empty(), "글리프 생성됨");
        }
    }
}
