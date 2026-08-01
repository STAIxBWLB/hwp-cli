//! HTML fragment(계약: docs/design/18) → IR.
//!
//! Maru 부분(part) 작성기의 비산문 블록(표·그림) 교환 경로의 소비자 쪽이다.
//! 생산자(`html.rs`)가 내는 well-formed XHTML 부분집합만 받으며, 계약 위반
//! (미열거 태그·malformed XML·span 불일치·빈 표)은 hard error다 — 추측 복구 금지.
//!
//! 남겨진 표현 전용 요소: `class`/`style` 속성(무시), 각주 마커(`fnref` id를 가진
//! `sup`는 평문으로만 취함)와 `<section class="footnotes">`(통째로 건너뜀).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use hwp_model::{
    BinRef, BinStream, BorderFillId, Cell, CharShape, CharShapeId, Control, DocMeta, Document,
    HwpChar, HwpUnit, NumLevel, ParaShape, ParaShapeId, Paragraph, Picture, Section, StyleId,
    Table, ctrl_char,
};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::from_markdown::{
    self, BASE_PARA_SHAPES, BODY_WIDTH, CELL_VALIGN_CENTER, TABLE_BORDER_FILL, shapes,
};

/// default_header 문자 모양 팔레트 개수(0~15). from_html의 추가 문자모양(밑줄·첨자
/// 조합)은 이 뒤에 붙는다 — md 혼합 경로(from_markdown)도 같은 default_header를 쓰므로
/// 병합 시 id가 충돌하지 않는다.
pub(crate) const PALETTE_LEN: u16 = 16;

/// HTML 들여오기 옵션.
#[derive(Default)]
pub struct HtmlImportOptions<'a> {
    /// 상대 경로 이미지(`<img src="fig.png">`)의 기준 디렉터리.
    pub base_dir: Option<&'a Path>,
    /// 임베드 이미지 bin 이름 시드 — 한 문서에 fragment를 여러 개 합칠 때 이름 충돌 방지.
    pub(crate) bin_seed: usize,
}

/// fragment 파싱 산출물 — 단독 문서 조립(from_html)과 md 혼합 병합(from_markdown) 양쪽이 쓴다.
#[derive(Default)]
pub(crate) struct HtmlBlocks {
    pub paragraphs: Vec<Paragraph>,
    pub bin_streams: Vec<BinStream>,
    /// default_header 팔레트 뒤에 붙는 추가 문자모양. id = PALETTE_LEN + 인덱스.
    pub extra_char_shapes: Vec<CharShape>,
    /// 목록 문단모양(인덱스 BASE_PARA_SHAPES + 인덱스)·번호/글머리 정의.
    pub extra_para_shapes: Vec<ParaShape>,
    /// default_header 글꼴(슬롯당 2개) 뒤에 붙는 추가 글꼴 — 모든 언어 슬롯에 같은
    /// 순서로 연장한다(계약 v2: style 블록의 font-family 복원).
    pub extra_fonts: Vec<hwp_model::FaceName>,
    pub numbering_levels: Vec<Vec<NumLevel>>,
    pub bullet_chars: Vec<char>,
    pub warnings: Vec<String>,
}

/// HTML fragment를 문서로 변환한다(기존 시그니처 관례 — 옵션 없음).
pub fn from_html(html: &str) -> Result<Document, String> {
    from_html_with(html, &HtmlImportOptions::default())
}

/// 옵션을 받는 변형. standalone 문서(`<html>`~`</html>`)와 fragment 둘 다 받는다.
pub fn from_html_with(html: &str, opts: &HtmlImportOptions) -> Result<Document, String> {
    let blocks = parse_fragment(html, opts)?;
    for w in &blocks.warnings {
        eprintln!("경고: {w}");
    }
    let mut paragraphs = blocks.paragraphs;
    if paragraphs.is_empty() {
        // 빈 문서도 문단 하나로 닫는다. 문단끝 문자는 writer가 보장한다.
        paragraphs.push(Paragraph::default());
    }
    // 첫 문단에 구역/단 정의 주입 — hwp5/한글 호환의 전제 조건(from_markdown과 동일).
    from_markdown::inject_section_controls(&mut paragraphs[0], None);

    let mut header = from_markdown::default_header();
    header.char_shapes.extend(blocks.extra_char_shapes);
    header.para_shapes.extend(blocks.extra_para_shapes);
    for slot in &mut header.fonts {
        slot.extend(blocks.extra_fonts.iter().cloned());
    }
    header.numbering_levels = blocks.numbering_levels;
    header.bullet_chars = blocks.bullet_chars;

    Ok(Document {
        meta: DocMeta {
            source_format: "html".to_string(),
            source_version: String::new(),
        },
        metadata: Default::default(),
        header,
        sections: vec![Section {
            paragraphs,
            extras: Vec::new(),
        }],
        bin_streams: blocks.bin_streams,
        hwpx_settings_xml: None,
        hwpx_version_xml: None,
        hwp5_xml_template: Vec::new(),
        hwp5_doc_history: Vec::new(),
    })
}

/// fragment 파싱 산출물을 만든다 (from_markdown 혼합 경로의 진입점).
pub(crate) fn parse_fragment(html: &str, opts: &HtmlImportOptions) -> Result<HtmlBlocks, String> {
    let default = from_markdown::default_header();
    let default_fonts = default.fonts[0].len();
    let mut p = Parser {
        ctx_stack: vec![BlockCtx::default()],
        marks: Marks::default(),
        heading: None,
        in_link: false,
        link_end: None,
        list_stack: Vec::new(),
        shape_cache: HashMap::new(),
        normal_template: default.char_shapes[shapes::NORMAL as usize].clone(),
        extra_char_shapes: Vec::new(),
        extra_para_shapes: Vec::new(),
        numbering_levels: Vec::new(),
        bullet_chars: Vec::new(),
        bin_streams: Vec::new(),
        bin_seed: opts.bin_seed,
        base_dir: opts.base_dir.map(Path::to_path_buf),
        warnings: Vec::new(),
        in_cell_depth: 0,
        cs_rules: HashMap::new(),
        ps_rules: HashMap::new(),
        cs_cache: HashMap::new(),
        ps_cache: HashMap::new(),
        variant_cache: HashMap::new(),
        span_shape: None,
        para_class: None,
        palette: default.char_shapes.clone(),
        palette_para: default.para_shapes.clone(),
        fonts: default.fonts[0].clone(),
        default_fonts,
    };
    let mut reader = Reader::from_str(html);
    p.blocks(&mut reader, None)?;
    let top = p.ctx_stack.pop().expect("최상위 컨텍스트 1개");
    Ok(HtmlBlocks {
        paragraphs: top.paragraphs,
        bin_streams: p.bin_streams,
        extra_char_shapes: p.extra_char_shapes,
        extra_para_shapes: p.extra_para_shapes,
        extra_fonts: p.fonts.split_off(p.default_fonts),
        numbering_levels: p.numbering_levels,
        bullet_chars: p.bullet_chars,
        warnings: p.warnings,
    })
}

/// 인라인 마크 상태.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
struct Marks {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    sup: bool,
    sub: bool,
}

/// 블록 컨텍스트 — 최상위·표 셀 각각이 독립 문단 버퍼를 갖는다.
#[derive(Default)]
struct BlockCtx {
    paragraphs: Vec<Paragraph>,
    chars: Vec<HwpChar>,
    runs: Vec<(u32, CharShapeId)>,
    controls: Vec<Control>,
    wchar_pos: u32,
    style: u16,
}

/// 목록 한 수준(프레임).
struct ListFrame {
    para_shape_id: u16,
    item_open: bool,
}

/// 셀 명세 — 위치(점유 격자 배치 후 확정)와 내용.
struct CellSpec {
    col_span: u16,
    row_span: u16,
    paragraphs: Vec<Paragraph>,
}

struct Parser {
    ctx_stack: Vec<BlockCtx>,
    marks: Marks,
    heading: Option<u16>,
    in_link: bool,
    link_end: Option<HwpChar>,
    list_stack: Vec<ListFrame>,
    shape_cache: HashMap<Marks, u16>,
    normal_template: CharShape,
    extra_char_shapes: Vec<CharShape>,
    extra_para_shapes: Vec<ParaShape>,
    numbering_levels: Vec<Vec<NumLevel>>,
    bullet_chars: Vec<char>,
    bin_streams: Vec<BinStream>,
    bin_seed: usize,
    base_dir: Option<PathBuf>,
    warnings: Vec<String>,
    in_cell_depth: u32,
    // 계약 v2 스타일 왕복 — <style> 규칙 저장·복원 캐시.
    cs_rules: HashMap<u16, HashMap<String, String>>,
    ps_rules: HashMap<u16, HashMap<String, String>>,
    cs_cache: HashMap<u16, CharShapeId>,
    ps_cache: HashMap<u16, u16>,
    /// (클스 모양 id, 마크) 조합 변형 캐시.
    variant_cache: HashMap<(u16, Marks), u16>,
    /// 활성 `<span class="csN">`의 복원 모양.
    span_shape: Option<CharShapeId>,
    /// 현재 문단의 `class="psN"` 복원 모양.
    para_class: Option<u16>,
    /// 기본 팔레트 사본 (dedup·변형 기준).
    palette: Vec<CharShape>,
    palette_para: Vec<ParaShape>,
    /// 언어 슬롯 0 글꼴 목록 (기본 2개 + 복원 추가분). 추가분은 HtmlBlocks로 방출.
    fonts: Vec<hwp_model::FaceName>,
    default_fonts: usize,
}

impl Parser {
    fn ctx(&mut self) -> &mut BlockCtx {
        self.ctx_stack.last_mut().expect("컨텍스트 스택 비어 있음")
    }

    /// 블록 루프. `end`가 있으면 그 닫힘 태그까지(표 셀·li·figure 등의 난이).
    fn blocks(&mut self, r: &mut Reader<&[u8]>, end: Option<&str>) -> Result<(), String> {
        loop {
            match r.read_event() {
                Ok(Event::Start(e)) => {
                    let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                    match name.as_str() {
                        "p" | "figcaption" => {
                            self.para_class = class_ps(&e).and_then(|n| self.ps_shape(n));
                            self.inline(r, &name)?;
                            self.flush_paragraph(false);
                        }
                        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                            let level = name[1..].parse::<u16>().map_err(|_| "제목 수준")?;
                            self.heading = Some(level);
                            self.ctx().style = level;
                            self.para_class = class_ps(&e).and_then(|n| self.ps_shape(n));
                            self.inline(r, &name)?;
                            self.flush_paragraph(false);
                            self.heading = None;
                            self.ctx().style = 0;
                        }
                        "ul" | "ol" => {
                            let start = if name == "ol" {
                                attr(&e, "start")
                                    .and_then(|s| s.parse::<u64>().ok())
                                    .or(Some(1))
                            } else {
                                None
                            };
                            self.start_list(start);
                            self.blocks(r, Some(&name))?;
                            self.end_list();
                        }
                        "li" => {
                            self.flush_paragraph(false);
                            if let Some(frame) = self.list_stack.last_mut() {
                                frame.item_open = true;
                            }
                            self.blocks(r, Some("li"))?;
                            self.flush_paragraph(false);
                            if let Some(frame) = self.list_stack.last_mut() {
                                frame.item_open = false;
                            }
                        }
                        "table" => {
                            let table = self.table(r)?;
                            self.push_table(table);
                        }
                        "figure" => self.blocks(r, Some("figure"))?,
                        "section" => {
                            if attr(&e, "class").as_deref() == Some("footnotes") {
                                self.skip_subtree(r, "section")?;
                            } else {
                                return Err("<section>은 class=\"footnotes\"만 지원합니다".into());
                            }
                        }
                        "html" | "body" | "head" => self.blocks(r, Some(&name))?,
                        "title" => self.skip_subtree(r, "title")?,
                        "style" => {
                            let css = self.read_style_text(r)?;
                            self.parse_style_rules(&css);
                        }
                        "br" => self.push_line_break(),
                        "img" => self.embed_image(&e)?,
                        "strong" | "em" | "u" | "s" | "sup" | "sub" | "a" | "span" => {
                            self.inline_tag(r, &e)?;
                        }
                        other => return Err(format!("지원하지 않는 태그: <{other}>")),
                    }
                }
                Ok(Event::Empty(e)) => {
                    match e.local_name().as_ref() {
                        b"br" => self.push_line_break(),
                        b"img" => self.embed_image(&e)?,
                        b"hr" | b"meta" => {} // hr·head의 meta — 내용 없음
                        other => {
                            return Err(format!(
                                "지원하지 않는 태그: <{}>",
                                String::from_utf8_lossy(other)
                            ));
                        }
                    }
                }
                Ok(Event::Text(t)) => {
                    let s = t
                        .xml10_content()
                        .map_err(|e| format!("텍스트 디코딩 실패: {e}"))?;
                    self.push_html_text(&s);
                }
                Ok(Event::GeneralRef(g)) => {
                    let c =
                        resolve_entity(&g).ok_or_else(|| "알 수 없는 엔티티 참조".to_string())?;
                    self.push_text(&c.to_string());
                }
                Ok(Event::End(e)) => {
                    let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                    if Some(name.as_str()) == end {
                        self.flush_paragraph(false);
                        return Ok(());
                    }
                    return Err(format!("닫힘 태그 불일치: </{name}>"));
                }
                Ok(Event::Eof) => {
                    if let Some(tag) = end {
                        return Err(format!("닫히지 않은 태그: <{tag}>"));
                    }
                    self.flush_paragraph(false);
                    return Ok(());
                }
                Err(e) => return Err(format!("XML 파싱 실패: {e}")),
                _ => {} // Decl·DocType·Comment·PI·CData(계약 외)는 무시
            }
        }
    }

    /// 인라인 루프 — `end` 태그의 닫힘까지.
    fn inline(&mut self, r: &mut Reader<&[u8]>, end: &str) -> Result<(), String> {
        loop {
            match r.read_event() {
                Ok(Event::Start(e)) => {
                    let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                    match name.as_str() {
                        "strong" | "em" | "u" | "s" | "sup" | "sub" | "a" | "span" => {
                            self.inline_tag(r, &e)?;
                        }
                        "img" => self.embed_image(&e)?,
                        "br" => self.push_line_break(),
                        other => {
                            return Err(format!(
                                "인라인 안에 블록/미지원 태그: <{other}> (in <{end}>)"
                            ));
                        }
                    }
                }
                Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                    b"br" => self.push_line_break(),
                    b"img" => self.embed_image(&e)?,
                    other => {
                        return Err(format!(
                            "지원하지 않는 태그: <{}>",
                            String::from_utf8_lossy(other)
                        ));
                    }
                },
                Ok(Event::Text(t)) => {
                    let s = t
                        .xml10_content()
                        .map_err(|e| format!("텍스트 디코딩 실패: {e}"))?;
                    self.push_html_text(&s);
                }
                Ok(Event::GeneralRef(g)) => {
                    let c =
                        resolve_entity(&g).ok_or_else(|| "알 수 없는 엔티티 참조".to_string())?;
                    self.push_text(&c.to_string());
                }
                Ok(Event::End(e)) => {
                    let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                    if name == end {
                        return Ok(());
                    }
                    return Err(format!("닫힘 태그 불일치: </{name}> (in <{end}>)"));
                }
                Ok(Event::Eof) => return Err(format!("닫히지 않은 태그: <{end}>")),
                Err(e) => return Err(format!("XML 파싱 실패: {e}")),
                _ => {}
            }
        }
    }

    /// 인라인 서식 태그 하나를 처리한다 (블록·인라인 양쪽에서 진입).
    fn inline_tag(&mut self, r: &mut Reader<&[u8]>, e: &BytesStart<'_>) -> Result<(), String> {
        let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
        match name.as_str() {
            "a" => {
                let href = attr(e, "href").ok_or("<a>에 href가 없습니다")?;
                let (start, end_char, control) = crate::field::hyperlink_field_parts(&href);
                let ctx = self.ctx();
                ctx.chars.push(start);
                ctx.wchar_pos += 8; // FIELD_START ExtCtrl = 8 WCHAR
                ctx.controls.push(control);
                self.in_link = true;
                self.link_end = Some(end_char);
                self.inline(r, "a")?;
                if let Some(end_char) = self.link_end.take() {
                    let ctx = self.ctx();
                    ctx.chars.push(end_char);
                    ctx.wchar_pos += 8; // FIELD_END InlineCtrl = 8 WCHAR
                }
                self.in_link = false;
                Ok(())
            }
            "sup" => {
                // 각주 마커(`fnref` id)는 표현 전용 — sup 마크를 켜지 않고 평문으로만 취한다.
                let marker = attr(e, "id").is_some_and(|id| id.starts_with("fnref-"));
                let saved = self.marks;
                if !marker {
                    self.marks.sup = true;
                }
                let result = self.inline(r, "sup");
                self.marks = saved;
                result
            }
            "span" => {
                // 스타일 클래스 span (계약 v2 §8) — 마크가 아니라 복원 모양을 현재 run에 적용.
                let shape = class_cs(e).and_then(|n| self.cs_shape(n));
                let saved = self.span_shape;
                self.span_shape = shape;
                let result = self.inline(r, "span");
                self.span_shape = saved;
                result
            }
            tag @ ("strong" | "em" | "u" | "s" | "sub") => {
                let saved = self.marks;
                match tag {
                    "strong" => self.marks.bold = true,
                    "em" => self.marks.italic = true,
                    "u" => self.marks.underline = true,
                    "s" => self.marks.strike = true,
                    "sub" => self.marks.sub = true,
                    _ => unreachable!(),
                }
                let result = self.inline(r, tag);
                self.marks = saved;
                result
            }
            other => Err(format!("지원하지 않는 태그: <{other}>")),
        }
    }

    /// 표 파싱 — 점유 격자로 셀 위치를 역산하고 span 불일치를 에러로 잡는다.
    fn table(&mut self, r: &mut Reader<&[u8]>) -> Result<Table, String> {
        let mut rows: Vec<Vec<CellSpec>> = Vec::new();
        loop {
            match r.read_event() {
                Ok(Event::Start(e)) => match e.local_name().as_ref() {
                    b"thead" | b"tbody" => {}
                    b"tr" => rows.push(self.table_row(r)?),
                    other => {
                        return Err(format!(
                            "<table> 안에 미지원 태그: <{}>",
                            String::from_utf8_lossy(other)
                        ));
                    }
                },
                Ok(Event::End(e)) => match e.local_name().as_ref() {
                    b"thead" | b"tbody" => {}
                    b"table" => break,
                    other => {
                        return Err(format!(
                            "닫힘 태그 불일치: </{}> (in <table>)",
                            String::from_utf8_lossy(other)
                        ));
                    }
                },
                Ok(Event::Text(t)) => {
                    let s = t
                        .xml10_content()
                        .map_err(|e| format!("텍스트 디코딩: {e}"))?;
                    if !s.trim().is_empty() {
                        return Err("<table> 안에 텍스트가 바로 있습니다".into());
                    }
                }
                Ok(Event::Eof) => return Err("닫히지 않은 태그: <table>".into()),
                Err(e) => return Err(format!("XML 파싱 실패: {e}")),
                _ => {}
            }
        }
        self.build_table(rows)
    }

    fn table_row(&mut self, r: &mut Reader<&[u8]>) -> Result<Vec<CellSpec>, String> {
        let mut cells = Vec::new();
        loop {
            match r.read_event() {
                Ok(Event::Start(e)) => match e.local_name().as_ref() {
                    b"td" | b"th" => cells.push(self.table_cell(r, &e)?),
                    other => {
                        return Err(format!(
                            "<tr> 안에 미지원 태그: <{}>",
                            String::from_utf8_lossy(other)
                        ));
                    }
                },
                Ok(Event::End(e)) => match e.local_name().as_ref() {
                    b"tr" => break,
                    other => {
                        return Err(format!(
                            "닫힘 태그 불일치: </{}> (in <tr>)",
                            String::from_utf8_lossy(other)
                        ));
                    }
                },
                Ok(Event::Text(t)) => {
                    let s = t
                        .xml10_content()
                        .map_err(|e| format!("텍스트 디코딩: {e}"))?;
                    if !s.trim().is_empty() {
                        return Err("<tr> 안에 텍스트가 바로 있습니다".into());
                    }
                }
                Ok(Event::Eof) => return Err("닫히지 않은 태그: <tr>".into()),
                Err(e) => return Err(format!("XML 파싱 실패: {e}")),
                _ => {}
            }
        }
        Ok(cells)
    }

    fn table_cell(
        &mut self,
        r: &mut Reader<&[u8]>,
        e: &BytesStart<'_>,
    ) -> Result<CellSpec, String> {
        let span = |name: &str| -> Result<u16, String> {
            match attr(e, name) {
                None => Ok(1),
                Some(v) => {
                    let n = v
                        .parse::<u16>()
                        .map_err(|_| format!("{name} 값이 숫자가 아닙니다: {v}"))?;
                    if n == 0 {
                        return Err(format!("{name}=0은 허용되지 않습니다"));
                    }
                    Ok(n)
                }
            }
        };
        let col_span = span("colspan")?;
        let row_span = span("rowspan")?;
        let end = match e.local_name().as_ref() {
            b"th" => "th",
            _ => "td",
        };
        self.ctx_stack.push(BlockCtx::default());
        self.in_cell_depth += 1;
        let result = self.blocks(r, Some(end));
        self.in_cell_depth -= 1;
        let mut ctx = self.ctx_stack.pop().expect("셀 컨텍스트");
        result?;
        // 셀은 문단 1개 이상 필수(nparas≥1) — 빈 셀도 빈 문단을 갖는다.
        if ctx.paragraphs.is_empty() {
            ctx.paragraphs.push(Paragraph {
                char_shape_runs: vec![(0, CharShapeId(shapes::NORMAL))],
                ..Paragraph::default()
            });
        }
        Ok(CellSpec {
            col_span,
            row_span,
            paragraphs: ctx.paragraphs,
        })
    }

    /// 점유 격자로 셀 위치를 배정하고 Table을 만든다.
    /// 빈 칸은 빈 셀로 채워(IR 타일링 불변식) 행 길이가 다른 HTML 표도 받는다.
    fn build_table(&mut self, rows: Vec<Vec<CellSpec>>) -> Result<Table, String> {
        if rows.is_empty() {
            return Err("행이 없는 표입니다".into());
        }
        let n_rows = rows.len();
        // 1패스: 셀을 첫 빈 칸에 놓으며 열 수를 확정한다.
        let mut covered: Vec<Vec<bool>> = vec![Vec::new(); n_rows];
        let mut placed: Vec<(usize, usize, CellSpec)> = Vec::new(); // (row, col, spec)
        for (ri, row) in rows.into_iter().enumerate() {
            for spec in row {
                let mut ci = 0usize;
                loop {
                    while covered[ri].len() <= ci {
                        covered[ri].push(false);
                    }
                    if !covered[ri][ci] {
                        break;
                    }
                    ci += 1;
                }
                // span 영역을 마킹한다(겹침 검사 포함).
                for dr in 0..spec.row_span as usize {
                    if ri + dr >= n_rows {
                        return Err(format!(
                            "rowspan이 표 밖으로 나갑니다 (행 {ri}, rowspan {})",
                            spec.row_span
                        ));
                    }
                    for dc in 0..spec.col_span as usize {
                        let slot = &mut covered[ri + dr];
                        while slot.len() <= ci + dc {
                            slot.push(false);
                        }
                        if slot[ci + dc] && (dr != 0 || dc != 0) {
                            return Err(format!(
                                "병합 영역이 겹칩니다 (행 {ri}, 열 {ci}, span {}x{})",
                                spec.col_span, spec.row_span
                            ));
                        }
                        slot[ci + dc] = true;
                    }
                }
                placed.push((ri, ci, spec));
            }
        }
        let cols = covered.iter().map(Vec::len).max().unwrap_or(1).max(1);
        // 2패스: 덮이지 않은 빈 칸을 빈 셀로 채워 타일링을 완성한다.
        for (ri, row) in covered.iter().enumerate() {
            for ci in 0..cols {
                let covered_slot = row.get(ci).copied().unwrap_or(false);
                if !covered_slot {
                    placed.push((
                        ri,
                        ci,
                        CellSpec {
                            col_span: 1,
                            row_span: 1,
                            paragraphs: vec![Paragraph {
                                char_shape_runs: vec![(0, CharShapeId(shapes::NORMAL))],
                                ..Paragraph::default()
                            }],
                        },
                    ));
                }
            }
        }
        placed.sort_by_key(|(ri, ci, _)| (*ri, *ci));

        let col_w = BODY_WIDTH / cols as i32;
        let row_h = 1700i32; // 10pt 텍스트 + 셀 위아래 여백 (from_markdown과 동일)
        let mut counts = vec![0u16; n_rows];
        let mut cells = Vec::with_capacity(placed.len());
        for (ri, ci, spec) in placed {
            counts[ri] += 1;
            cells.push(Cell {
                list_attr: CELL_VALIGN_CENTER,
                col: ci as u16,
                row: ri as u16,
                col_span: spec.col_span,
                row_span: spec.row_span,
                width: HwpUnit(col_w * i32::from(spec.col_span)),
                height: HwpUnit(row_h * i32::from(spec.row_span)),
                margins: [510, 510, 141, 141],
                border_fill: BorderFillId(TABLE_BORDER_FILL),
                header_tail: Vec::new(),
                paragraphs: spec.paragraphs,
            });
        }
        let table = Table {
            common_data: Vec::new(),
            placement: None,
            attr: 0,
            rows: n_rows as u16,
            cols: cols as u16,
            cell_spacing: 0,
            inner_margins: [510, 510, 141, 141],
            row_cell_counts: counts,
            border_fill: BorderFillId(TABLE_BORDER_FILL),
            table_tail: Vec::new(),
            cells,
            extras: Vec::new(),
        };
        // 최종 게이트 — 정품 실측 5규칙 불변식(면적 타일링·행우선·row_cell_counts 등).
        crate::edit::validate_table_invariants(&table)?;
        Ok(table)
    }

    /// 완성된 표를 앵커 문단(확장 컨트롤 1개)으로 현재 컨텍스트에 넣는다.
    fn push_table(&mut self, table: Table) {
        self.flush_paragraph(false);
        let mut payload = vec![0u8; 12];
        payload[..4].copy_from_slice(b" lbt"); // 역순 ctrl_id
        let para = Paragraph {
            chars: vec![
                HwpChar::ExtCtrl {
                    code: 11,
                    ctrl_id: *b"tbl ",
                    payload,
                    ctrl_index: Some(0),
                },
                HwpChar::CharCtrl(13),
            ],
            char_shape_runs: vec![(0, CharShapeId(shapes::NORMAL))],
            controls: vec![Control::Table(table)],
            ..Paragraph::default()
        };
        self.ctx().paragraphs.push(para);
    }

    /// `<img>` — data URI·상대 경로를 임베드한다. SVG는 검증+결정론적 PNG 래스터화
    /// (hwp-convert::svg — DocumentSpec v2와 같은 정책: 네이티브 표현 부재).
    fn embed_image(&mut self, e: &BytesStart<'_>) -> Result<(), String> {
        let src = attr(e, "src").ok_or("<img>에 src가 없습니다")?;
        let (data, ext, w, h) = if let Some(rest) = src.strip_prefix("data:") {
            let comma = rest.find(',').ok_or("data URI 형식이 아닙니다(',' 없음)")?;
            let (meta, payload) = rest.split_at(comma);
            if !meta.ends_with(";base64") {
                return Err(format!("data URI는 base64만 지원합니다: {meta}"));
            }
            let data =
                crate::base64::decode(&payload[1..]).map_err(|e| format!("base64 디코딩: {e}"))?;
            if data.is_empty() {
                return Err("빈 이미지 데이터입니다".into());
            }
            let (ext, _) = crate::image::image_kind(&data);
            if ext == "bin" {
                return Err("지원하지 않는 이미지 형식입니다".into());
            }
            let (w, h) =
                crate::image::display_size(&data, &crate::image::ImageSize::Natural, BODY_WIDTH);
            (data, ext, w, h)
        } else {
            if src.starts_with("http://") || src.starts_with("https://") {
                return Err(format!("원격 이미지 URL은 지원하지 않습니다: {src}"));
            }
            let raw = src.strip_prefix("file://").unwrap_or(&src);
            let path = Path::new(raw);
            let resolved: PathBuf = if path.is_absolute() {
                path.to_path_buf()
            } else {
                match &self.base_dir {
                    Some(dir) => dir.join(path),
                    None => {
                        return Err(format!(
                            "상대 경로 이미지의 기준 디렉터리를 알 수 없습니다: {src}"
                        ));
                    }
                }
            };
            if resolved
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
            {
                let text = std::fs::read_to_string(&resolved)
                    .map_err(|e| format!("SVG 읽기 실패 {}: {e}", resolved.display()))?;
                let canonical = crate::svg::sanitize_svg(&text)
                    .map_err(|e| format!("SVG 검증 거부 ({}): {e}", resolved.display()))?;
                let (sw, sh) = crate::svg::size_px(&canonical)?;
                if !(sw.is_finite() && sh.is_finite() && sw > 0.0 && sh > 0.0) {
                    return Err(format!(
                        "SVG 크기가 유효하지 않습니다: {}",
                        resolved.display()
                    ));
                }
                // 96dpi → HWPUNIT 75/px. 본문 폭을 넘으면 비율 유지 축소.
                let mut w_px = sw.round().max(1.0) as u32;
                let mut h_px = sh.round().max(1.0) as u32;
                let mut w = (sw * 75.0).max(1.0) as i32;
                let mut h = (sh * 75.0).max(1.0) as i32;
                if w > BODY_WIDTH {
                    let ratio = BODY_WIDTH as f32 / w as f32;
                    w = BODY_WIDTH;
                    h = ((h as f32) * ratio).max(1.0) as i32;
                    w_px = ((w_px as f32) * ratio).max(1.0) as u32;
                    h_px = ((h_px as f32) * ratio).max(1.0) as u32;
                }
                let png = crate::svg::rasterize_svg_png(
                    &canonical,
                    w_px,
                    h_px,
                    crate::svg::MAX_SVG_BYTES as u64,
                )
                .map_err(|e| format!("SVG 래스터화 실패 ({}): {e}", resolved.display()))?;
                self.warnings.push(format!(
                    "SVG 이미지를 결정론적 PNG로 래스터화했습니다: {src}"
                ));
                (png, "png", w, h)
            } else {
                let data = std::fs::read(&resolved)
                    .map_err(|e| format!("이미지 읽기 실패 {}: {e}", resolved.display()))?;
                if data.is_empty() {
                    return Err("빈 이미지 데이터입니다".into());
                }
                let (ext, _) = crate::image::image_kind(&data);
                if ext == "bin" {
                    return Err("지원하지 않는 이미지 형식입니다".into());
                }
                let (w, h) = crate::image::display_size(
                    &data,
                    &crate::image::ImageSize::Natural,
                    BODY_WIDTH,
                );
                (data, ext, w, h)
            }
        };
        let name = format!(
            "html_image{}.{ext}",
            self.bin_seed + self.bin_streams.len() + 1
        );
        let ctx = self.ctx();
        let idx = ctx.controls.len() as u32;
        ctx.controls.push(Control::Picture(Picture {
            common_data: Vec::new(),
            width: HwpUnit(w.max(1)),
            height: HwpUnit(h.max(1)),
            treat_as_char: true, // 인라인(글자처럼) 배치 — writer가 도형 레코드 합성
            z_order: 0,
            vert_offset: 0,
            horz_offset: 0,
            description: None,
            bin_ref: BinRef::ItemRef(name.clone()),
            extras: Vec::new(),
        }));
        // gso 앵커 문자(code 11) — insert_image와 동일 규약. relink가 ctrl_index 재배치.
        ctx.chars.push(HwpChar::ExtCtrl {
            code: 11,
            ctrl_id: *b"gso ",
            payload: crate::field::rev_payload(b"gso "),
            ctrl_index: Some(idx),
        });
        ctx.wchar_pos += 8;
        self.bin_streams.push(BinStream { name, data });
        Ok(())
    }

    fn start_list(&mut self, start: Option<u64>) {
        self.flush_paragraph(false);
        let level = (self.list_stack.len() as u16 + 1).min(7);
        let para_shape_id = match start {
            Some(s) => {
                let def_id = self.numbering_levels.len() as u16;
                let mut levels = vec![NumLevel::default(); 7];
                levels[(level as usize - 1).min(6)].start = s.max(1) as u32;
                self.numbering_levels.push(levels);
                self.push_list_para_shape(2, level, def_id)
            }
            None => {
                let def_id = self.bullet_chars.len() as u16;
                self.bullet_chars.push(if level >= 2 { '·' } else { '-' });
                self.push_list_para_shape(3, level, def_id)
            }
        };
        self.list_stack.push(ListFrame {
            para_shape_id,
            item_open: false,
        });
    }

    fn end_list(&mut self) {
        self.flush_paragraph(false);
        self.list_stack.pop();
    }

    /// 목록 항목용 문단 모양(from_markdown::Builder와 동일 규칙).
    fn push_list_para_shape(&mut self, head_type: u32, level: u16, def_id: u16) -> u16 {
        let idx = BASE_PARA_SHAPES + self.extra_para_shapes.len() as u16;
        let step = 2000i32;
        self.extra_para_shapes.push(ParaShape {
            attr1: 0x180 | (1 << 2) | (head_type << 23) | (u32::from(level) << 25),
            margin_left: i32::from(level) * step,
            indent: -step,
            line_spacing_old: 160,
            line_spacing: 160,
            border_fill_id: 2,
            numbering_id: def_id,
            ..ParaShape::default()
        });
        idx
    }

    fn push_line_break(&mut self) {
        let ctx = self.ctx();
        ctx.chars.push(HwpChar::CharCtrl(10));
        ctx.wchar_pos += 1;
    }

    /// HTML 텍스트 노드 — 블록 사이의 공백-only 노드는 버리고, 개행은 공백으로 접는다.
    fn push_html_text(&mut self, s: &str) {
        if s.trim().is_empty() {
            return;
        }
        self.push_text(&s.replace('\n', " "));
    }

    fn push_text(&mut self, text: &str) {
        let shape = CharShapeId(self.shape_id());
        let ctx = self.ctx();
        if ctx.runs.last().map(|(_, s)| *s) != Some(shape) {
            ctx.runs.push((ctx.wchar_pos, shape));
        }
        for c in text.chars() {
            match c {
                '\t' => {
                    ctx.wchar_pos += 8;
                    ctx.chars.push(HwpChar::InlineCtrl {
                        code: ctrl_char::TAB,
                        payload: vec![0; 12],
                    });
                }
                c if (c as u32) < 0x20 => {}
                c => {
                    ctx.wchar_pos += c.len_utf16() as u32;
                    ctx.chars.push(HwpChar::Text(c));
                }
            }
        }
    }

    /// 현재 마크/제목/링크/스팬 클래스 상태의 문자 모양 ID.
    fn shape_id(&mut self) -> u16 {
        if self.in_link {
            return shapes::HYPERLINK;
        }
        if let Some(level) = self.heading {
            return shapes::HEADING_BASE + level - 1;
        }
        // `<span class="csN">`의 복원 모양 (계약 v2) — 마크가 얹히면 변형을 1회 할당.
        if let Some(base) = self.span_shape {
            let m = self.marks;
            if m == Marks::default() {
                return base.0;
            }
            if let Some(&id) = self.variant_cache.get(&(base.0, m)) {
                return id;
            }
            let mut shape = self.shape_by_id(base.0).clone();
            if m.bold {
                shape.attr |= 1 << 1;
            }
            if m.italic {
                shape.attr |= 1;
            }
            if m.underline {
                shape.attr |= 1 << 2; // 밑줄 종류 1(글자 아래)
            }
            if m.sup {
                shape.attr |= 1 << 15;
            }
            if m.sub {
                shape.attr |= 1 << 16;
            }
            shape.strike |= m.strike;
            let id = PALETTE_LEN + self.extra_char_shapes.len() as u16;
            self.extra_char_shapes.push(shape);
            self.variant_cache.insert((base.0, m), id);
            return id;
        }
        let m = self.marks;
        if !m.underline && !m.sup && !m.sub {
            // 팔레트 조합(굵게/기울임/취소선).
            return match (m.bold, m.italic, m.strike) {
                (false, false, false) => shapes::NORMAL,
                (true, false, false) => shapes::BOLD,
                (false, true, false) => shapes::ITALIC,
                (true, true, false) => shapes::BOLD_ITALIC,
                (false, false, true) => shapes::STRIKE,
                (true, false, true) => shapes::BOLD_STRIKE,
                (false, true, true) => shapes::ITALIC_STRIKE,
                (true, true, true) => shapes::BOLD_ITALIC_STRIKE,
            };
        }
        // 밑줄·첨자 조합은 팔레트에 없다 — 팔레트 뒤에 1회 할당(캐시).
        if let Some(&id) = self.shape_cache.get(&m) {
            return id;
        }
        let id = PALETTE_LEN + self.extra_char_shapes.len() as u16;
        let mut attr = u32::from(m.bold) << 1 | u32::from(m.italic);
        if m.underline {
            attr |= 1 << 2; // 밑줄 종류 1(글자 아래)
        }
        if m.sup {
            attr |= 1 << 15;
        }
        if m.sub {
            attr |= 1 << 16;
        }
        self.extra_char_shapes.push(CharShape {
            attr,
            strike: m.strike,
            ..self.normal_template.clone()
        });
        self.shape_cache.insert(m, id);
        id
    }

    /// 문단을 닫는다(from_markdown::Builder::flush_paragraph_inner와 동일 불변식).
    fn flush_paragraph(&mut self, force: bool) {
        let para_class = self.para_class.take();
        let list_shape = self
            .list_stack
            .last()
            .filter(|f| f.item_open)
            .map(|f| f.para_shape_id);
        let heading = self.heading;
        let in_cell = self.in_cell_depth > 0;
        let ctx = self.ctx();
        if ctx.chars.is_empty() && ctx.runs.is_empty() && !force {
            return;
        }
        let mut runs = std::mem::take(&mut ctx.runs);
        if runs.is_empty() {
            runs.push((0, CharShapeId(shapes::NORMAL)));
        }
        let para_shape = if let Some(id) = list_shape {
            id
        } else if let Some(id) = para_class {
            id // `class="psN"` 복원 모양 (계약 v2)
        } else if heading.is_some() {
            1 // 제목(들여쓰기 변형은 md 경로와 달리 v1 단일 모양)
        } else if in_cell {
            0 // 표 셀(간격 없음)
        } else {
            2 // 본문
        };
        let mut para = Paragraph {
            para_shape: ParaShapeId(para_shape),
            style: StyleId(ctx.style),
            chars: std::mem::take(&mut ctx.chars),
            char_shape_runs: runs,
            controls: std::mem::take(&mut ctx.controls),
            ..Paragraph::default()
        };
        // FIELD_START(하이퍼링크 등) ExtCtrl ↔ controls 등장순서 연결.
        crate::field::relink_ctrl_index(&mut para);
        ctx.wchar_pos = 0;
        ctx.paragraphs.push(para);
    }

    /// `<style>`의 텍스트를 읽는다 (닫힘까지).
    fn read_style_text(&mut self, r: &mut Reader<&[u8]>) -> Result<String, String> {
        let mut css = String::new();
        loop {
            match r.read_event() {
                Ok(Event::Text(t)) => {
                    let s = t
                        .xml10_content()
                        .map_err(|e| format!("텍스트 디코딩 실패: {e}"))?;
                    css.push_str(&s);
                }
                Ok(Event::End(e)) => {
                    if e.local_name().as_ref() == b"style" {
                        return Ok(css);
                    }
                }
                Ok(Event::Eof) => return Err("닫히지 않은 태그: <style>".into()),
                Err(e) => return Err(format!("XML 파싱 실패: {e}")),
                _ => {}
            }
        }
    }

    /// `<style>` 블록에서 `.cs{n}`/`.ps{n}` 규칙만 추출한다 (나머지 규칙·선언은 무시).
    fn parse_style_rules(&mut self, css: &str) {
        for rule in css.split('}') {
            let Some((selector, body)) = rule.split_once('{') else {
                continue;
            };
            let selector = selector.trim();
            let props: HashMap<String, String> = body
                .split(';')
                .filter_map(|kv| {
                    let (k, v) = kv.split_once(':')?;
                    Some((k.trim().to_string(), v.trim().to_string()))
                })
                .collect();
            if props.is_empty() {
                continue;
            }
            if let Some(n) = selector
                .strip_prefix(".cs")
                .and_then(|s| s.trim().parse::<u16>().ok())
            {
                self.cs_rules.insert(n, props);
            } else if let Some(n) = selector
                .strip_prefix(".ps")
                .and_then(|s| s.trim().parse::<u16>().ok())
            {
                self.ps_rules.insert(n, props);
            }
        }
    }

    /// `.cs{n}` 규칙 → CharShape 복원 (팔레트 dedup 후 1회 할당).
    fn cs_shape(&mut self, n: u16) -> Option<CharShapeId> {
        if let Some(&id) = self.cs_cache.get(&n) {
            return Some(id);
        }
        let props = self.cs_rules.get(&n)?.clone();
        let mut shape = self.normal_template.clone();
        if let Some(ff) = props.get("font-family")
            && let Some(name) = ff
                .split(',')
                .next()
                .map(|s| s.trim().trim_matches('"').trim_matches('\''))
            && !name.is_empty()
        {
            let face = self.face_id_for(name);
            shape.face_ids = [face; hwp_model::LANG_COUNT];
        }
        if let Some(pt) = props
            .get("font-size")
            .and_then(|v| v.strip_suffix("pt"))
            .and_then(|v| v.parse::<f32>().ok())
        {
            shape.base_size = (pt * 100.0).round() as i32;
            shape.rel_sizes = [100; hwp_model::LANG_COUNT];
        }
        if let Some(c) = props.get("color").and_then(|v| parse_hex_color(v)) {
            shape.text_color = c;
        }
        if let Some(c) = props
            .get("background-color")
            .and_then(|v| parse_hex_color(v))
        {
            shape.shade_color = c;
        }
        if let Some(em) = props
            .get("letter-spacing")
            .and_then(|v| v.strip_suffix("em"))
            .and_then(|v| v.parse::<f32>().ok())
        {
            shape.spacings = [(em * 100.0).round() as i8; hwp_model::LANG_COUNT];
        }
        // 팔레트 dedup — 같은 모양이면 팔레트 id를 쓴다.
        if let Some(idx) = self.palette.iter().position(|p| *p == shape) {
            let id = CharShapeId(idx as u16);
            self.cs_cache.insert(n, id);
            return Some(id);
        }
        let id = PALETTE_LEN + self.extra_char_shapes.len() as u16;
        self.extra_char_shapes.push(shape);
        let id = CharShapeId(id);
        self.cs_cache.insert(n, id);
        Some(id)
    }

    /// `.ps{n}` 규칙 → ParaShape 복원 (팔레트 dedup 후 1회 할당).
    fn ps_shape(&mut self, n: u16) -> Option<u16> {
        if let Some(&id) = self.ps_cache.get(&n) {
            return Some(id);
        }
        let props = self.ps_rules.get(&n)?.clone();
        let align_bits: u32 = match props.get("text-align").map(String::as_str) {
            Some("left") => 1,
            Some("right") => 2,
            Some("center") => 3,
            _ => 0,
        };
        let mut ps = ParaShape {
            attr1: 0x180 | (align_bits << 2),
            line_spacing_old: 160,
            line_spacing: 160,
            border_fill_id: 2,
            ..ParaShape::default()
        };
        if let Some(lh) = props.get("line-height") {
            if lh == "normal" {
                ps.line_spacing_type = 2;
                ps.line_spacing = 0;
                ps.line_spacing_old = 0;
            } else if let Some(pt) = lh.strip_suffix("pt").and_then(|v| v.parse::<f32>().ok()) {
                ps.line_spacing_type = 1;
                ps.line_spacing = (pt * 100.0).round() as i32;
                ps.line_spacing_old = ps.line_spacing;
            } else if let Ok(ratio) = lh.parse::<f32>() {
                ps.line_spacing_type = 0;
                ps.line_spacing = (ratio * 100.0).round() as i32;
                ps.line_spacing_old = ps.line_spacing;
            }
        }
        let mm = |key: &str| -> Option<i32> {
            props
                .get(key)
                .and_then(|v| v.strip_suffix("mm"))
                .and_then(|v| v.parse::<f32>().ok())
                .map(|mm| (mm * 7200.0 / 25.4).round() as i32)
        };
        if let Some(v) = mm("margin-left") {
            ps.margin_left = v;
        }
        if let Some(v) = mm("margin-right") {
            ps.margin_right = v;
        }
        if let Some(v) = mm("margin-top") {
            ps.spacing_top = v;
        }
        if let Some(v) = mm("margin-bottom") {
            ps.spacing_bottom = v;
        }
        if let Some(v) = mm("text-indent") {
            ps.indent = v;
        }
        // 팔레트 dedup.
        if let Some(idx) = self.palette_para.iter().position(|p| *p == ps) {
            self.ps_cache.insert(n, idx as u16);
            return Some(idx as u16);
        }
        let id = BASE_PARA_SHAPES + self.extra_para_shapes.len() as u16;
        self.extra_para_shapes.push(ps);
        self.ps_cache.insert(n, id);
        Some(id)
    }

    fn shape_by_id(&self, id: u16) -> &CharShape {
        if (id as usize) < self.palette.len() {
            &self.palette[id as usize]
        } else {
            &self.extra_char_shapes[(id - PALETTE_LEN) as usize]
        }
    }

    /// 글꼴 이름 → face id (없으면 추가). 추가분은 슬롯 전체에 같은 순서로 붙는다.
    fn face_id_for(&mut self, name: &str) -> u16 {
        if let Some(idx) = self.fonts.iter().position(|f| f.name == name) {
            return idx as u16;
        }
        let id = self.fonts.len() as u16;
        self.fonts.push(hwp_model::FaceName {
            name: name.to_string(),
            attr: 0x01,
            ..hwp_model::FaceName::default()
        });
        id
    }

    /// `end` 태그의 닫힘까지 통째로 건너뛴다(중첩 동일 태그 추적).
    fn skip_subtree(&mut self, r: &mut Reader<&[u8]>, end: &str) -> Result<(), String> {
        let mut depth = 1usize;
        loop {
            match r.read_event() {
                Ok(Event::Start(e)) => {
                    if e.local_name().as_ref() == end.as_bytes() {
                        depth += 1;
                    }
                }
                Ok(Event::End(e)) => {
                    if e.local_name().as_ref() == end.as_bytes() {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(());
                        }
                    }
                }
                Ok(Event::Eof) => return Err(format!("닫히지 않은 태그: <{end}>")),
                Err(e) => return Err(format!("XML 파싱 실패: {e}")),
                _ => {}
            }
        }
    }
}

/// class 속성에서 `cs{n}` 접두 클래스의 n을 찾는다 (계약 v2).
fn class_cs(e: &BytesStart<'_>) -> Option<u16> {
    class_num(e, "cs")
}

/// class 속성에서 `ps{n}` 접두 클래스의 n을 찾는다 (계약 v2).
fn class_ps(e: &BytesStart<'_>) -> Option<u16> {
    class_num(e, "ps")
}

fn class_num(e: &BytesStart<'_>, prefix: &str) -> Option<u16> {
    attr(e, "class")?
        .split_whitespace()
        .find_map(|c| c.strip_prefix(prefix)?.parse::<u16>().ok())
}

/// `#RRGGBB` → COLORREF(0x00BBGGRR).
fn parse_hex_color(s: &str) -> Option<u32> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let r = u32::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u32::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u32::from_str_radix(&hex[4..6], 16).ok()?;
    Some(r | (g << 8) | (b << 16))
}

/// 속성 조회(로컬 이름 기준, 엔티티 해석 포함) — hwpx read/xml.rs와 동일 규칙.
fn attr(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        let key = a.key.local_name();
        if key.as_ref() == name.as_bytes() {
            let raw = String::from_utf8_lossy(&a.value).into_owned();
            Some(
                quick_xml::escape::unescape(&raw)
                    .map(|s| s.into_owned())
                    .unwrap_or(raw),
            )
        } else {
            None
        }
    })
}

/// `&amp;` 등 일반 참조 해석 — hwpx read/mod.rs와 동일 규칙.
fn resolve_entity(g: &quick_xml::events::BytesRef<'_>) -> Option<char> {
    g.resolve_char_ref()
        .ok()
        .flatten()
        .or_else(|| match &g[..] {
            b"amp" => Some('&'),
            b"lt" => Some('<'),
            b"gt" => Some('>'),
            b"quot" => Some('"'),
            b"apos" => Some('\''),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::from_markdown::from_markdown;
    use crate::html::{to_html, to_html_fragment};

    /// 문단 텍스트 추출(테스트 단순화용).
    fn para_text(p: &Paragraph) -> String {
        p.chars
            .iter()
            .filter_map(|c| match c {
                HwpChar::Text(c) => Some(*c),
                _ => None,
            })
            .collect()
    }

    fn first_table(doc: &Document) -> &Table {
        doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("표 컨트롤")
    }

    #[test]
    fn 표_span_역산() {
        let doc = from_html(
            "<table><tr><th colspan=\"2\">가로</th><td rowspan=\"2\">세로</td></tr>\
             <tr><td>a</td><td>b</td></tr></table>",
        )
        .unwrap();
        let t = first_table(&doc);
        assert_eq!((t.rows, t.cols), (2, 3));
        assert_eq!(t.cells.len(), 4, "앵커 셀만 저장: {:?}", t.cells.len());
        assert_eq!(t.row_cell_counts, vec![2, 2]);
        let c00 = &t.cells[0];
        assert_eq!((c00.col_span, c00.row_span), (2, 1));
        assert_eq!(para_text(&c00.paragraphs[0]), "가로");
        let c02 = t.cells.iter().find(|c| c.row == 0 && c.col == 2).unwrap();
        assert_eq!((c02.col_span, c02.row_span), (1, 2));
    }

    #[test]
    fn 표_불완전_행은_빈_셀로_타일링() {
        let doc =
            from_html("<table><tr><td>a</td></tr><tr><td>b</td><td>c</td></tr></table>").unwrap();
        let t = first_table(&doc);
        assert_eq!((t.rows, t.cols), (2, 2));
        assert_eq!(t.cells.len(), 4, "빈 칸 채움: {}", t.cells.len());
    }

    #[test]
    fn 계약_위반은_에러() {
        assert!(from_html("<div>x</div>").is_err(), "미지원 태그");
        assert!(from_html("<table></table>").is_err(), "빈 표");
        assert!(
            from_html("<table><tr><td colspan=\"0\">x</td></tr></table>").is_err(),
            "span 0"
        );
        assert!(from_html("<p>닫지 않음").is_err(), "malformed");
        assert!(
            from_html("<table><tr><td rowspan=\"3\">x</td></tr></table>").is_err(),
            "rowspan 표 밖"
        );
    }

    #[test]
    fn 인라인_마크와_링크() {
        let doc = from_html(
            "<p><strong>굵게</strong>와 <u>밑줄</u>, <a href=\"https://x.io\">링크</a></p>",
        )
        .unwrap();
        let html = to_html(&doc);
        assert!(html.contains("<strong>굵게</strong>"), "굵게: {html}");
        assert!(html.contains("<u>밑줄</u>"), "밑줄: {html}");
        // 링크 표시 텍스트는 하이퍼링크 문자모양(파랑+밑줄)이라 <u>가 붙는다.
        assert!(
            html.contains("<a href=\"https://x.io\">") && html.contains("링크</u></a>"),
            "링크: {html}"
        );
    }

    #[test]
    fn data_uri_이미지_왕복() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend(96u32.to_be_bytes());
        png.extend(96u32.to_be_bytes());
        png.extend([0u8; 8]);
        let uri = format!("data:image/png;base64,{}", crate::base64::encode(&png));
        let doc = from_html(&format!("<p>그림<img src=\"{uri}\"/></p>")).unwrap();
        assert_eq!(doc.bin_streams.len(), 1);
        assert_eq!(doc.bin_streams[0].data, png);
        // 재수출필 때도 data URI로 나와야 한다.
        let html = to_html(&doc);
        assert!(html.contains("data:image/png;base64,"), "재임베드: {html}");
    }

    #[test]
    fn svg_이미지는_png로_래스터화() {
        let dir = std::env::temp_dir().join(format!(
            "from_html_svg_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("fig.svg"),
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 100 100\">\
             <rect x=\"10\" y=\"10\" width=\"80\" height=\"60\" fill=\"#FF0000\"/></svg>",
        )
        .unwrap();
        let doc = from_html_with(
            "<p>그림<img src=\"fig.svg\"/></p>",
            &HtmlImportOptions {
                base_dir: Some(&dir),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(doc.bin_streams.len(), 1);
        assert_eq!(&doc.bin_streams[0].data[..4], b"\x89PNG");
        assert!(doc.bin_streams[0].name.ends_with(".png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 위험_svg는_에러() {
        let dir = std::env::temp_dir().join(format!(
            "from_html_svg_bad_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("evil.svg"),
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1 1\">\
             <script>alert(1)</script></svg>",
        )
        .unwrap();
        let err = from_html_with(
            "<p><img src=\"evil.svg\"/></p>",
            &HtmlImportOptions {
                base_dir: Some(&dir),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.contains("SVG 검증 거부"), "에러: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 목록_파싱() {
        let doc = from_html("<ol><li>첫째</li><li>둘째</li></ol>").unwrap();
        let texts: Vec<String> = doc.sections[0].paragraphs.iter().map(para_text).collect();
        assert!(texts.iter().any(|t| t.contains("첫째")), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("둘째")), "{texts:?}");
        // 번호 머리 문단모양이 할당됐어야 한다.
        assert!(!doc.header.numbering_levels.is_empty());
    }

    #[test]
    fn export_import_구조_왕복() {
        // IR → html → IR: 병합 표와 이미지가 구조적으로 보존돼야 한다.
        let mut doc = from_markdown("본문\n\n| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        // (0,0)~(0,1) 가로 병합으로 바꿔치기 — export가 colspan을 내게.
        {
            let t = doc.sections[0].paragraphs[1]
                .controls
                .iter_mut()
                .find_map(|c| match c {
                    Control::Table(t) => Some(t),
                    _ => None,
                })
                .unwrap();
            let removed = t.cells.remove(1); // (0,1)
            let _ = removed;
            t.cells[0].col_span = 2;
            t.row_cell_counts = vec![1, 2];
        }
        let html = to_html(&doc);
        assert!(html.contains("colspan=\"2\""), "export colspan: {html}");
        let back = from_html(&html).unwrap();
        let t = first_table(&back);
        assert_eq!((t.rows, t.cols), (2, 2));
        assert_eq!(t.cells.len(), 3);
        assert_eq!(t.cells[0].col_span, 2);
        assert_eq!(t.row_cell_counts, vec![1, 2]);
    }

    #[test]
    fn standalone과_footnotes_수용() {
        // to_html standalone 출력도 읽힌다 — footnotes 섹션은 걷어낸다.
        let doc = from_markdown("본문[^1]입니다.\n\n[^1]: 각주 내용\n");
        let full = to_html(&doc);
        let back = from_html(&full).unwrap();
        let all_text: String = back.sections[0].paragraphs.iter().map(para_text).collect();
        assert!(all_text.contains("본문"), "본문: {all_text}");
        assert!(!all_text.contains("각주 내용"), "정의는 폐기: {all_text}");
        // fragment도 그대로 읽힌다.
        let frag = to_html_fragment(&doc);
        assert!(from_html(&frag).is_ok());
    }

    #[test]
    fn 스타일_왕복_속성_보존() {
        // 계약 v2: 글자(색·자간)·문단(정렬·줄간격) 모양이 html을 거쳐 보존된다.
        let mut doc = from_markdown("본문 문단입니다.\n");
        doc.header.char_shapes[0].text_color = 0x0000_00FF; // COLORREF 빨강
        doc.header.char_shapes[0].spacings = [5; hwp_model::LANG_COUNT];
        doc.header.para_shapes[2].attr1 = 0x180 | (3 << 2); // 가울데
        doc.header.para_shapes[2].line_spacing = 130;
        doc.header.para_shapes[2].line_spacing_old = 130;
        let html = to_html(&doc);
        assert!(html.contains("color:#FF0000"), "{html}");
        assert!(html.contains("letter-spacing:0.05em"), "{html}");
        assert!(html.contains("text-align:center"), "{html}");
        assert!(html.contains("line-height:1.3"), "{html}");

        let back = from_html(&html).unwrap();
        let para = &back.sections[0].paragraphs[0];
        let (_, cs_id) = para.char_shape_runs[0];
        let shape = &back.header.char_shapes[cs_id.0 as usize];
        assert_eq!(shape.text_color, 0x0000_00FF, "글자색 왕복");
        assert_eq!(shape.spacings[0], 5, "자간 왕복");
        let ps = &back.header.para_shapes[para.para_shape.0 as usize];
        assert_eq!(ps.alignment(), 3, "정렬 왕복");
        assert_eq!(ps.line_spacing, 130, "줄간격 왕복");
        assert_eq!(ps.line_spacing_type, 0, "줄간격 종류 왕복");

        // 재수출 안정성 — 같은 속성이 다시 실린다.
        let html2 = to_html(&back);
        assert!(html2.contains("color:#FF0000"), "재수출: {html2}");
        assert!(html2.contains("text-align:center"), "재수출: {html2}");
    }

    #[test]
    fn 스타일_왕복_팔레트_dedup() {
        // 기본 모양만 쓰는 문서는 팔레트가 그대로 재사용돼 추가 모양이 생기지 않는다.
        let doc = from_markdown("본문 문단입니다.\n");
        let html = to_html(&doc);
        let back = from_html(&html).unwrap();
        assert_eq!(back.header.char_shapes.len(), 16, "팔레트 외 글자모양 없음");
        assert_eq!(back.header.para_shapes.len(), 5, "팔레트 외 문단모양 없음");
    }

    #[test]
    fn 스타일_왕복_글꼴_복원() {
        // 팔레트에 없는 글꼴 이름은 추가 글꼴로 복원된다.
        let mut doc = from_markdown("본문 문단입니다.\n");
        doc.header.fonts[0].push(hwp_model::FaceName {
            name: "마루고딕".into(),
            attr: 0x01,
            ..hwp_model::FaceName::default()
        });
        doc.header.char_shapes[0].face_ids = [2; hwp_model::LANG_COUNT];
        let html = to_html(&doc);
        assert!(html.contains("font-family:\"마루고딕\",serif"), "{html}");
        let back = from_html(&html).unwrap();
        let para = &back.sections[0].paragraphs[0];
        let (_, cs_id) = para.char_shape_runs[0];
        let shape = &back.header.char_shapes[cs_id.0 as usize];
        let face = &back.header.fonts[0][shape.face_ids[0] as usize];
        assert_eq!(face.name, "마루고딕", "글꼴 복원");
    }
}
