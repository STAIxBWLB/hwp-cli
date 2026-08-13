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
    /// Underline kind from `CharShape::underline_kind`: 0 none, 1 below, 3 above.
    pub underline_kind: u8,
    /// Zero-based `BorderType2` underline shape.
    pub underline_shape: u8,
    /// 취소선
    pub strike: bool,
    /// Zero-based strike shape from `CharShape::strike_shape_code`.
    pub strike_shape: u8,
    /// Emphasis kind from `CharShape::emphasis_kind` (0 none, 1..=12).
    pub emphasis: u8,
    /// 밑줄 색 (COLORREF, 0xFFFFFFFF = 글자색 따름)
    pub underline_color: u32,
    /// 글자 음영(배경 하이라이트) 색 (COLORREF, 0xFFFFFFFF = 없음)
    pub shade_color: u32,
    /// 그림자 색 (Some이면 그림자 그림)
    pub shadow: Option<u32>,
    /// Shadow gap percentages. `(0, 0)` selects the backend-compatible fallback.
    pub shadow_gap: (i8, i8),
    /// 외곽선(빈 글자 — 채움 없이 윤곽선만)
    pub outline: bool,
    /// 양각(3D 돋움)
    pub emboss: bool,
    /// 음각(3D 새김)
    pub engrave: bool,
    /// One-based character border/background reference; 0 means unspecified.
    pub border_fill_id: u16,
    pub glyphs: Vec<Glyph>,
    pub width_pt: f32,
    pub text: String,
    /// 이 런의 첫 글자 WCHAR 위치 (줄바꿈 시 글리프→WCHAR 매핑용 — lineseg 합성).
    pub start_wchar: u32,
}

impl ShapedRun {
    /// Returns the shadow offset in points.
    ///
    /// Nonzero `shadow_gap` values are percentages of the font size following
    /// OWPML offsetX/offsetY conventions. `(0, 0)` retains the legacy 0.06em
    /// fallback pending Hancom verification.
    pub fn shadow_offset(&self) -> (f32, f32) {
        if self.shadow_gap == (0, 0) {
            let d = self.size_pt * 0.06;
            (d, d)
        } else {
            (
                self.size_pt * f32::from(self.shadow_gap.0) / 100.0,
                self.size_pt * f32::from(self.shadow_gap.1) / 100.0,
            )
        }
    }

    /// Build a partial run for the glyph range `[start, end)` (line wrapping).
    ///
    /// Source text is reconstructed from shaping clusters so wrapped runs keep
    /// a complete PDF ToUnicode trace, including ligatures and combining text.
    pub fn slice(&self, start: usize, end: usize) -> ShapedRun {
        let sources = glyph_source_sequences(self);
        self.slice_with_sources(start, end, &sources)
    }

    pub(crate) fn slice_with_sources(
        &self,
        start: usize,
        end: usize,
        sources: &[String],
    ) -> ShapedRun {
        let glyphs: Vec<Glyph> = self.glyphs[start..end].to_vec();
        let width_pt = glyphs.iter().map(|g| g.x_advance).sum();
        let text = sources[start..end].concat();
        let wchar_offset = sources[..start]
            .iter()
            .flat_map(|source| source.chars())
            .map(|ch| ch.len_utf16() as u32)
            .sum::<u32>();
        ShapedRun {
            font: self.font.clone(),
            size_pt: self.size_pt,
            x_scale: self.x_scale,
            color: self.color,
            bold: self.bold,
            italic: self.italic,
            underline_kind: self.underline_kind,
            underline_shape: self.underline_shape,
            strike: self.strike,
            strike_shape: self.strike_shape,
            emphasis: self.emphasis,
            underline_color: self.underline_color,
            shade_color: self.shade_color,
            shadow: self.shadow,
            shadow_gap: self.shadow_gap,
            outline: self.outline,
            emboss: self.emboss,
            engrave: self.engrave,
            border_fill_id: self.border_fill_id,
            glyphs,
            width_pt,
            text,
            start_wchar: self.start_wchar + wchar_offset,
        }
    }
}

/// Return the source Unicode sequence represented by each emitted glyph.
///
/// Rustybuzz clusters are byte offsets into the input string. A ligature gets
/// the complete multi-scalar cluster, while a cluster expanded to multiple
/// glyphs distributes its source scalars once, in glyph order. This is also
/// used by PDF ToUnicode generation and line justification.
pub(crate) fn glyph_source_sequences(run: &ShapedRun) -> Vec<String> {
    let Some(face) = rustybuzz::Face::from_slice(&run.font.data, run.font.index) else {
        return distribute_source(&run.text, run.glyphs.len());
    };
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(&run.text);
    let shaped = rustybuzz::shape(&face, &[], buffer);
    if shaped.len() != run.glyphs.len() {
        return distribute_source(&run.text, run.glyphs.len());
    }
    sources_from_clusters(&run.text, shaped.glyph_infos())
}

fn sources_from_clusters(text: &str, infos: &[rustybuzz::GlyphInfo]) -> Vec<String> {
    if infos.is_empty() {
        return Vec::new();
    }
    let mut starts: Vec<usize> = infos.iter().map(|info| info.cluster as usize).collect();
    starts.sort_unstable();
    starts.dedup();

    let mut out = vec![String::new(); infos.len()];
    let mut group_start = 0usize;
    while group_start < infos.len() {
        let cluster = infos[group_start].cluster;
        let mut group_end = group_start + 1;
        while group_end < infos.len() && infos[group_end].cluster == cluster {
            group_end += 1;
        }
        let start = cluster as usize;
        let end = starts
            .iter()
            .copied()
            .find(|candidate| *candidate > start)
            .unwrap_or(text.len());
        let source = text.get(start..end).unwrap_or("");
        let distributed = distribute_source(source, group_end - group_start);
        out[group_start..group_end].clone_from_slice(&distributed);
        group_start = group_end;
    }
    out
}

fn distribute_source(source: &str, glyph_count: usize) -> Vec<String> {
    if glyph_count == 0 {
        return Vec::new();
    }
    let chars: Vec<char> = source.chars().collect();
    if chars.is_empty() {
        return vec!["\u{fffd}".to_string(); glyph_count];
    }
    if chars.len() >= glyph_count {
        let first_len = chars.len() - glyph_count + 1;
        let mut out = Vec::with_capacity(glyph_count);
        out.push(chars[..first_len].iter().collect());
        out.extend(chars[first_len..].iter().map(char::to_string));
        return out;
    }
    let mut out: Vec<String> = chars.into_iter().map(|ch| ch.to_string()).collect();
    out.resize(glyph_count, "\u{fffd}".to_string());
    out
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
                // 하이픈(24)·묶음 빈칸(30) (GG-20): 실제 글리프로 폭을 갖는다(종전엔
                // 폭 0으로 생략). 하이픈은 '-', 묶음 빈칸은 현재 문자 모양의 공백 글리프.
                // 줄바꿈은 글리프 그리디라(05 §7) 공백 우선 분리 로직 자체가 없으므로
                // 묶음 빈칸에 별도 분리 금지 장치가 필요 없다.
                HwpChar::CharCtrl(code)
                    if matches!(*code, ctrl_char::HYPHEN | ctrl_char::NB_SPACE) =>
                {
                    let c = if *code == ctrl_char::HYPHEN { '-' } else { ' ' };
                    let shape_id = shape_id_at(para, pos);
                    let lang = lang_slot_of(c);
                    match pieces.last_mut() {
                        Some(last) if last.shape_id == shape_id && last.lang == lang => {
                            last.text.push(c);
                        }
                        _ => pieces.push(Piece {
                            shape_id,
                            lang,
                            text: c.to_string(),
                            start: pos,
                        }),
                    }
                }
                // 고정폭 빈칸(31) (GG-20): 비례 공백 글리프와 무관하게 유효 크기의
                // 1em(상대 크기·장평 반영). 탭처럼 별도 런으로 끼우고 뒤 조각 경계를 둔다.
                HwpChar::CharCtrl(code) if *code == ctrl_char::FW_SPACE => {
                    let shape = doc.header.char_shapes.get(shape_id_at(para, pos) as usize);
                    if let Some(run) = fw_space_run(store, doc, shape, pos) {
                        items.push((pieces.len(), InlineItem::Run(run)));
                        pieces.push(Piece {
                            shape_id: shape_id_at(para, pos + 1),
                            lang: 0,
                            text: String::new(),
                            start: pos + 1,
                        });
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

/// 고정폭 빈칸(FW_SPACE) 런: 공백 글리프를 셰이핑한 뒤 폭을 유효 크기의 1em
/// (상대 크기·장평 반영, 자간 제외)으로 덮어쓴다. 비례 공백 글리프 폭과 무관하다.
fn fw_space_run(
    store: &mut FontStore,
    doc: &Document,
    shape: Option<&CharShape>,
    pos: u32,
) -> Option<ShapedRun> {
    let mut run = shape_piece(store, doc, shape, 1, " ", pos)
        .into_iter()
        .next()?;
    let em = run.size_pt * run.x_scale;
    for g in &mut run.glyphs {
        g.x_advance = em;
    }
    run.width_pt = em * run.glyphs.len() as f32;
    Some(run)
}

/// 자간(GG-4): 유효 크기(HWPUNIT — 상대 크기·첨자 축소 반영) × %를 정수 영역에서
/// half-up 반올림(동점은 +∞ 쪽, 음수 포함)한 뒤 pt로 환산한다.
/// 마지막 글자의 trailing 자간 포함 여부는 한컴 실측 라운드에서 확인한다.
fn letter_spacing_pt(base_hu: i32, rel: u8, script: bool, pct: i8) -> f32 {
    let mut size_hu = base_hu * i32::from(rel) / 100;
    if script {
        size_hu = size_hu * 65 / 100;
    }
    ((size_hu * i32::from(pct) + 50).div_euclid(100)) as f32 / 100.0
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

    // 자간(GG-4): HWPUNIT 정수 영역에서 half-up 반올림 후 pt로 환산한다
    // (종전 pt 실수 곱셈의 반올림 오차 제거 — 한컴은 HWPUNIT 정수 도메인).
    let spacing_pt = letter_spacing_pt(
        base,
        rel,
        sup || sub,
        cs.spacings.get(lang).copied().unwrap_or(0),
    );
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
        underline_kind: cs.underline_kind(),
        underline_shape: cs.underline_shape_code(),
        strike: cs.has_strike(),
        strike_shape: cs.strike_shape_code(),
        emphasis: cs.emphasis_kind(),
        underline_color: cs.underline_color,
        shade_color: if cs.has_shade() {
            cs.shade_color
        } else {
            0xFFFF_FFFF
        },
        shadow: cs.has_shadow().then_some(cs.shadow_color),
        shadow_gap: cs.shadow_gap,
        outline: cs.has_outline(),
        emboss: cs.is_emboss(),
        engrave: cs.is_engrave(),
        border_fill_id: cs.border_fill_id,
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
            run.underline_kind = 1;
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

    fn synthetic_run(text: &str, glyph_ids: &[u16]) -> ShapedRun {
        let glyphs: Vec<Glyph> = glyph_ids
            .iter()
            .copied()
            .map(|id| Glyph {
                id,
                x_advance: 10.0,
                x_offset: 0.0,
                y_offset: 0.0,
            })
            .collect();
        ShapedRun {
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
            underline_kind: 0,
            underline_shape: 0,
            strike: false,
            strike_shape: 0,
            emphasis: 0,
            underline_color: 0xFFFF_FFFF,
            shade_color: 0xFFFF_FFFF,
            shadow: None,
            shadow_gap: (0, 0),
            outline: false,
            emboss: false,
            engrave: false,
            border_fill_id: 0,
            width_pt: glyphs.iter().map(|glyph| glyph.x_advance).sum(),
            glyphs,
            text: text.to_string(),
            start_wchar: 0,
        }
    }

    fn glyph_info(cluster: u32) -> rustybuzz::GlyphInfo {
        let mut info = rustybuzz::GlyphInfo::default();
        info.cluster = cluster;
        info
    }

    fn empty_font() -> Arc<LoadedFont> {
        Arc::new(LoadedFont {
            data: Arc::new(Vec::new()),
            index: 0,
            family: String::new(),
        })
    }

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
            font: empty_font(),
            size_pt: 10.0,
            x_scale: 1.0,
            color: 0,
            bold: false,
            italic: false,
            underline_kind: 0,
            underline_shape: 0,
            strike: false,
            strike_shape: 0,
            emphasis: 0,
            underline_color: 0xFFFF_FFFF,
            shade_color: 0xFFFF_FFFF,
            shadow: None,
            shadow_gap: (0, 0),
            outline: false,
            emboss: false,
            engrave: false,
            border_fill_id: 0,
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
            .map(|i| matches!(i, InlineItem::Run(r) if r.underline_kind == 1))
            .collect();
        assert_eq!(und, vec![false, true, false]); // 9만 링크 범위
        if let InlineItem::Run(r) = &items[1] {
            assert_eq!(r.color, LINK_BLUE);
            assert_eq!(r.underline_color, LINK_BLUE);
        }
        // 빈 범위는 무동작.
        let mut items2 = vec![run_at(9)];
        apply_link_style(&mut items2, &[]);
        assert!(matches!(&items2[0], InlineItem::Run(r) if r.underline_kind == 0));
    }

    #[test]
    fn shadow_offset_uses_serialized_gap() {
        // An unspecified gap retains the legacy 0.06em fallback.
        let run = synthetic_run("x", &[1]);
        let (dx, dy) = run.shadow_offset();
        assert!(
            (dx - 0.6).abs() < 0.01 && (dy - 0.6).abs() < 0.01,
            "default 0.06em"
        );
        // Serialized percentages scale with the font size (GG-11).
        let mut run = synthetic_run("x", &[1]);
        run.shadow_gap = (10, -5);
        let (dx, dy) = run.shadow_offset();
        assert!((dx - 1.0).abs() < 0.01, "x = 10% of 10pt");
        assert!((dy + 0.5).abs() < 0.01, "y = -5% of 10pt");
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

    /// 컨트롤 문자 셰이핑 측정용 — 한 문단을 만들어 shape_range로 셰이핑한다.
    fn shape_chars(doc: &Document, store: &mut FontStore, chars: Vec<HwpChar>) -> Vec<InlineItem> {
        let para = Paragraph {
            chars,
            ..Paragraph::default()
        };
        let mut warns = RenderIssueAccumulator::new();
        shape_range(store, doc, &para, (0, para.wchar_len()), &mut warns)
    }

    fn runs_width(items: &[InlineItem]) -> f32 {
        items
            .iter()
            .map(|i| match i {
                InlineItem::Run(r) => r.width_pt,
                _ => 0.0,
            })
            .sum()
    }

    #[test]
    fn 컨트롤_문자_너비_gg20() {
        let doc = hwp_convert::from_markdown("x");
        let mut store = FontStore::new();
        // 폰트 부재 환경은 스킵(셰이핑 결과 없음).
        if shape_chars(&doc, &mut store, vec![HwpChar::Text('a')]).is_empty() {
            return;
        }
        // 묶음 빈칸(30): 일반 공백과 같은 폭. 줄바꿈/분리 항목 없이 같은 런에 합쳐진다
        // (줄바꿈은 글리프 그리디라 공백 우선 분리 자체가 없다 — 05 §7).
        let nb = shape_chars(
            &doc,
            &mut store,
            vec![
                HwpChar::Text('a'),
                HwpChar::CharCtrl(ctrl_char::NB_SPACE),
                HwpChar::Text('b'),
            ],
        );
        let sp = shape_chars(
            &doc,
            &mut store,
            vec![HwpChar::Text('a'), HwpChar::Text(' '), HwpChar::Text('b')],
        );
        assert_eq!(nb.len(), 1, "묶음 빈칸은 분리 항목을 만들지 않는다");
        if let InlineItem::Run(r) = &nb[0] {
            assert_eq!(r.text, "a b");
        }
        assert!(
            (runs_width(&nb) - runs_width(&sp)).abs() < 0.01,
            "묶음 빈칸 폭 = 공백 글리프 폭"
        );
        // 하이픈(24): '-' 글리프 자연 폭.
        let hy = shape_chars(&doc, &mut store, vec![HwpChar::CharCtrl(ctrl_char::HYPHEN)]);
        let dash = shape_chars(&doc, &mut store, vec![HwpChar::Text('-')]);
        if let InlineItem::Run(r) = &hy[0] {
            assert_eq!(r.text, "-");
        }
        assert!(
            (runs_width(&hy) - runs_width(&dash)).abs() < 0.01,
            "하이픈 폭 = '-' 글리프 폭"
        );
        // 고정폭 빈칸(31): 비례 공백 글리프와 무관하게 유효 크기의 1em.
        let fw = shape_chars(
            &doc,
            &mut store,
            vec![HwpChar::CharCtrl(ctrl_char::FW_SPACE)],
        );
        let cs = &doc.header.char_shapes[0];
        let em = cs.base_size as f32 / 100.0 * f32::from(cs.rel_sizes[1]) / 100.0
            * f32::from(cs.ratios[1])
            / 100.0;
        assert!(
            (runs_width(&fw) - em).abs() < 0.01,
            "고정폭 빈칸 = 1em({em}), 실제 {}",
            runs_width(&fw)
        );
    }

    #[test]
    fn 자간_hwpunit_반올림_gg4() {
        // 10pt(1000 HWPUNIT), -7% → -70 HWPUNIT → -0.7pt
        assert!((letter_spacing_pt(1000, 100, false, -7) + 0.7).abs() < 1e-6);
        // half-up(동점은 +∞ 쪽): 100.5 → 101, -100.5 → -100
        assert!((letter_spacing_pt(1005, 100, false, 10) - 1.01).abs() < 1e-6);
        assert!((letter_spacing_pt(1005, 100, false, -10) + 1.0).abs() < 1e-6);
        // 상대 크기·첨자 축소(65%) 반영
        assert!((letter_spacing_pt(1000, 50, false, 10) - 0.5).abs() < 1e-6);
        assert!((letter_spacing_pt(1000, 100, true, 10) - 0.65).abs() < 1e-6);
    }

    #[test]
    fn shaping_clusters_preserve_ligatures_combining_text_and_slices() {
        let ligature_infos = [glyph_info(0), glyph_info(1), glyph_info(4), glyph_info(5)];
        let sources = sources_from_clusters("office", &ligature_infos);
        assert_eq!(sources, ["o", "ffi", "c", "e"]);

        let combining_infos = [glyph_info(0), glyph_info(0)];
        let sources = sources_from_clusters("x\u{301}", &combining_infos);
        assert_eq!(sources.concat(), "x\u{301}");
        assert!(sources.iter().all(|source| source != "\u{fffd}"));

        let ligature = synthetic_run("ffi", &[42]);
        assert_eq!(ligature.slice(0, 1).text, "ffi");
    }
}
