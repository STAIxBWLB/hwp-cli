//! IR → 독립 실행형 HTML (HTML fragment 계약의 생산자).
//!
//! `markdown.rs`의 매핑을 1:1로 미러링하되 HTML 시맨틱 태그를 쓴다:
//! - "개요 N" 스타일 문단 → `<h1>`..`<h6>`
//! - 문자 모양 → `<strong>`/`<em>`/`<u>`/`<s>` (markdown은 굵게·기울임만, HTML은 밑줄·취소선도 보존)
//! - 하이퍼링크(%hlk 필드) → `<a href="URL">표시텍스트</a>` (URL은 속성 이스케이프)
//! - 이미지(Picture) → `<img src="data:<mime>;base64,…"/>` (자기완결 임베드)
//! - 표 → `<table>`/`<tr>`/`<th>`/`<td>` — 병합 셀은 colspan/rowspan으로 방출(GH-4)하고
//!   셀 내 블록(중첩 표·이미지)도 보존한다(GH-5)
//! - 각주/미주 → 본문 `<sup>` 앵커 마커 + 문서 끝 정의 (표현 전용 — `from_html`은
//!   평문으로만 읽는다. 의미 왕복은 범위 밖)
//! - 글자·문단 모양 → `.cs{n}`/`.ps{n}` CSS 규칙 + `class` 속성 (계약 v2 §8 — 글꼴·
//!   크기·색·음영·자간·정렬·줄간격·여백이 `from_html`과 왕복한다)
//! - 줄나눔(10) → `<br/>`, 탭 → 공백
//!
//! 출력은 **well-formed XHTML**이다(빈 태그는 self-closing) — `from_html`(quick-xml)이
//! 그대로 다시 읽을 수 있는 fragment 계약의 생산자 쪽이다(계약: docs/design/18).
//! 임베드된 CJK 폰트 CSS가 포함된 standalone 문서를 생성한다.

use hwp_model::{Cell, CharShape, Control, Document, HwpChar, Paragraph, Table, ctrl_char};

const CSS: &str = "\
body { font-family: \"함초롬바탕\",\"HCR Batang\",\"Noto Serif CJK KR\",serif;\
 max-width: 50rem; margin: 2rem auto; padding: 0 1rem; line-height: 1.7; }\n\
table { border-collapse: collapse; width: 100%; margin: 1rem 0; }\n\
th, td { border: 1px solid #999; padding: 0.35rem 0.6rem; }\n\
th { background: #f2f2f2; }\n\
h1,h2,h3,h4,h5,h6 { font-family: \"함초롬돋움\",\"HCR Dotum\",\"Noto Sans CJK KR\",sans-serif; }\n\
section.footnotes { font-size: 0.9em; color: #444; }\n";

/// 각주/미주 정의 하나 (본문 마커 라벨 + 정의 HTML).
struct Note {
    label: String,
    html: String,
}

/// 렌더 상태 — 각주/미주 카운터와 수집된 정의.
#[derive(Default)]
struct Ctx {
    foot_n: u32,
    end_n: u32,
    notes: Vec<Note>,
    /// 사용된 글자/문단 모양 id 수집 — `<style>`의 cs/ps 규칙 방출용 (계약 v2 §8).
    used_char_shapes: std::collections::BTreeSet<u16>,
    used_para_shapes: std::collections::BTreeSet<u16>,
    /// 셀·각주 본문은 클래스를 싣지 않는다(계약 v2 §8.3).
    no_style_class: bool,
}

/// IR 전체를 standalone HTML 문서로 직렬화한다.
pub fn to_html(doc: &Document) -> String {
    let (body, footnotes, rules) = render_body(doc);
    // 문서 메타데이터 제목 우선, 없으면 첫 개요 단락으로 폴리백.
    let title_text = doc
        .metadata
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| first_heading(doc))
        .unwrap_or_default();
    let title = escape(&title_text);
    let mut out =
        String::with_capacity(body.len() + footnotes.len() + CSS.len() + rules.len() + 256);
    out.push_str("<!DOCTYPE html>\n<html lang=\"ko\"><head><meta charset=\"utf-8\"/>\n<title>");
    out.push_str(&title);
    out.push_str("</title>\n<style>\n");
    out.push_str(CSS);
    out.push_str(&rules);
    out.push_str("</style></head>\n<body>\n");
    out.push_str(&body);
    out.push_str(&footnotes);
    out.push_str("</body></html>\n");
    out
}

/// 본문 fragment만 (head 없이) 반환한다. 각주/미주 정의 섹션과 — 스타일 규칙이 있으면 —
/// 선두 `<style>` 요소를 포함한다(계약 v2 §8.1: fragment의 자기완결).
pub fn to_html_fragment(doc: &Document) -> String {
    let (body, footnotes, rules) = render_body(doc);
    let mut out = String::new();
    if !rules.is_empty() {
        out.push_str("<style>\n");
        out.push_str(&rules);
        out.push_str("</style>\n");
    }
    out.push_str(&body);
    out.push_str(&footnotes);
    out
}

/// 본문·각주/미주 정의·스타일 규칙(cs/ps)을 함께 렌더한다.
fn render_body(doc: &Document) -> (String, String, String) {
    let mut ctx = Ctx::default();
    let mut body = String::new();
    for section in &doc.sections {
        for para in &section.paragraphs {
            render_paragraph(doc, para, &mut ctx, &mut body);
        }
    }
    let rules = style_rules(doc, &ctx);
    (body, render_footnotes(&ctx), rules)
}

/// 사용된 모양 id를 `.cs{n}`/`.ps{n}` 규칙으로 방출한다 (계약 v2 §8.2).
fn style_rules(doc: &Document, ctx: &Ctx) -> String {
    let mut css = String::new();
    for id in &ctx.used_char_shapes {
        let Some(s) = doc.header.char_shapes.get(*id as usize) else {
            continue;
        };
        let mut rules = String::new();
        if let Some(face) = doc.header.fonts[0].get(s.face_ids[0] as usize) {
            rules.push_str(&format!(
                "font-family:\"{}\",serif;",
                css_text_escape(&face.name)
            ));
        }
        let pt = s.base_size as f32 / 100.0 * f32::from(s.rel_sizes[0]) / 100.0;
        rules.push_str(&format!("font-size:{}pt;", trim_num(pt)));
        if s.text_color != 0 {
            rules.push_str(&format!("color:{};", colorref_hex(s.text_color)));
        }
        if s.has_shade() {
            rules.push_str(&format!(
                "background-color:{};",
                colorref_hex(s.shade_color)
            ));
        }
        if s.spacings[0] != 0 {
            rules.push_str(&format!(
                "letter-spacing:{}em;",
                trim_num(f32::from(s.spacings[0]) / 100.0)
            ));
        }
        css.push_str(&format!(".cs{id}{{{rules}}}\n"));
    }
    for id in &ctx.used_para_shapes {
        let Some(p) = doc.header.para_shapes.get(*id as usize) else {
            continue;
        };
        let align = match p.alignment() {
            1 => "left",
            2 => "right",
            3 => "center",
            _ => "justify", // 0 양쪽 + 4/5 배분·나눔 근사
        };
        let line_height = match p.line_spacing_type {
            0 => trim_num(p.line_spacing as f32 / 100.0),
            1 | 3 => format!("{}pt", trim_num(p.line_spacing as f32 / 100.0)),
            _ => "normal".to_string(), // 2 여백만 근사
        };
        let rules = format!(
            "text-align:{align};line-height:{line_height};margin-left:{}mm;margin-right:{}mm;margin-top:{}mm;margin-bottom:{}mm;text-indent:{}mm;",
            trim_num(hwp_mm(p.margin_left)),
            trim_num(hwp_mm(p.margin_right)),
            trim_num(hwp_mm(p.spacing_top)),
            trim_num(hwp_mm(p.spacing_bottom)),
            trim_num(hwp_mm(p.indent)),
        );
        css.push_str(&format!(".ps{id}{{{rules}}}\n"));
    }
    css
}

/// `<style>` 안에 들어갈 텍스트(글꼴 이름 등)를 이스케이프한다.
/// 계약은 XHTML이라 `<style>` 내용도 XML 텍스트다 — `&` `<` `>`를 엔티티로 바꾸면
/// 소비자(quick-xml)가 자동으로 되돌린다. 따옴표는 작은따옴표로 치환한다
/// (`</style>` 조기 종료·마크업 주입 방지).
fn css_text_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push('\''),
            _ => out.push(c),
        }
    }
    out
}

/// COLORREF(0x00BBGGRR) → #RRGGBB.
fn colorref_hex(v: u32) -> String {
    format!(
        "#{:02X}{:02X}{:02X}",
        v & 0xFF,
        (v >> 8) & 0xFF,
        (v >> 16) & 0xFF
    )
}

/// HWPUNIT → mm.
fn hwp_mm(v: i32) -> f32 {
    v as f32 * 25.4 / 7200.0
}

/// 수치 문자열 — 소수 셋째 자리에서 반올림하고 끝 0을 뗀다.
fn trim_num(v: f32) -> String {
    let rounded = (v * 1000.0).round() / 1000.0;
    if rounded == rounded.trunc() {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}

/// 각주/미주 정의 섹션. 정의가 없으면 빈 문자열.
fn render_footnotes(ctx: &Ctx) -> String {
    if ctx.notes.is_empty() {
        return String::new();
    }
    let mut out = String::from("<section class=\"footnotes\">\n<hr/>\n<ol>\n");
    for note in &ctx.notes {
        out.push_str(&format!(
            "<li id=\"fn-{}\">{} <a href=\"#fnref-{}\">↩</a></li>\n",
            note.label, note.html, note.label
        ));
    }
    out.push_str("</ol>\n</section>\n");
    out
}

fn first_heading(doc: &Document) -> Option<String> {
    for section in &doc.sections {
        for para in &section.paragraphs {
            let is_heading = doc
                .header
                .styles
                .get(para.style.0 as usize)
                .and_then(|s| s.name.strip_prefix("개요 "))
                .and_then(|n| n.trim().parse::<usize>().ok())
                .is_some();
            if is_heading {
                let mut sink = String::new();
                // 제목 추출 전용 임시 Ctx — 각주 수집은 본 렌더(render_body)에 맡긴다.
                let text = render_inline(doc, para, &mut Ctx::default(), &mut sink);
                let text = strip_tags(&text);
                if !text.trim().is_empty() {
                    return Some(text.trim().to_string());
                }
            }
        }
    }
    None
}

fn render_paragraph(doc: &Document, para: &Paragraph, ctx: &mut Ctx, out: &mut String) {
    let heading = doc
        .header
        .styles
        .get(para.style.0 as usize)
        .and_then(|s| s.name.strip_prefix("개요 "))
        .and_then(|n| n.trim().parse::<usize>().ok())
        .filter(|n| (1..=6).contains(n));

    // 블록(표 등)은 별도 버퍼에 모아 문단 텍스트 뒤에 append — 출력 순서 문단→블록 보존
    // (odt.rs 와 동일). 이전엔 render_inline 이 블록을 out 에 바로 써 표가 <p> 앞에 나왔다.
    let mut blocks = String::new();
    let body = render_inline(doc, para, ctx, &mut blocks);
    let body = body.trim_end();
    if !body.is_empty() {
        ctx.used_para_shapes.insert(para.para_shape.0);
        let class = format!(" class=\"ps{}\"", para.para_shape.0);
        if let Some(level) = heading {
            out.push_str(&format!("<h{level}{class}>"));
            out.push_str(body);
            out.push_str(&format!("</h{level}>\n"));
        } else {
            out.push_str(&format!("<p{class}>"));
            out.push_str(body);
            out.push_str("</p>\n");
        }
    }
    // 블록은 텍스트 유무와 무관하게 항상 flush(표만 있는 문단이 누락되지 않게).
    out.push_str(&blocks);
}

/// 문단의 인라인 내용을 HTML 문자열로 반환한다.
/// 표 등 블록 컨트롤은 `out`에 직접 쓴다 (문단과 분리).
fn render_inline(doc: &Document, para: &Paragraph, ctx: &mut Ctx, out: &mut String) -> String {
    let mut body = String::new();
    let mut wchar_pos = 0u32;
    let mut style = Style::default();
    // 활성 글자 모양 클래스 span (계약 v2 §8 — 셀·각주 본문은 no_style_class로 미발행).
    let mut span_id: Option<u16> = None;
    // 하이퍼링크 필드 열림 상태. FIELD_START에서 `<a>`를 열고 FIELD_END에서 닫는다.
    let mut link_open = false;

    for ch in &para.chars {
        if let HwpChar::Text(_) = ch {
            let (want, want_span) = match shape_at(doc, para, wchar_pos) {
                Some((id, s)) => {
                    if ctx.no_style_class {
                        (Style::from_shape(s), None)
                    } else {
                        ctx.used_char_shapes.insert(id);
                        (Style::from_shape(s), Some(id))
                    }
                }
                None => (Style::default(), None),
            };
            if want != style || want_span != span_id {
                close_marks(&mut body, &mut style);
                close_class_span(&mut body, &mut span_id);
                open_class_span(&mut body, want_span, &mut span_id);
                open_marks(&mut body, want);
                style = want;
            }
        }
        match ch {
            HwpChar::Text(c) => push_escaped(&mut body, *c),
            HwpChar::CharCtrl(code) => match *code {
                ctrl_char::LINE_BREAK => {
                    close_marks(&mut body, &mut style);
                    body.push_str("<br/>\n");
                }
                ctrl_char::HYPHEN => body.push('-'),
                ctrl_char::NB_SPACE | ctrl_char::FW_SPACE => body.push(' '),
                _ => {}
            },
            HwpChar::InlineCtrl { code, .. } => {
                if *code == ctrl_char::FIELD_END {
                    if link_open {
                        // 클래스 span도 함께 닫는다 — `<a>`와 span이 교차하는
                        // 교차 마크업(XHTML 위반)을 막는다 (계약 v2).
                        close_marks(&mut body, &mut style);
                        close_class_span(&mut body, &mut span_id);
                        body.push_str("</a>");
                        link_open = false;
                    }
                } else if *code == ctrl_char::TAB {
                    body.push(' ');
                }
            }
            HwpChar::ExtCtrl {
                code, ctrl_index, ..
            } => {
                if let Some(idx) = ctrl_index
                    && let Some(control) = para.controls.get(*idx as usize)
                {
                    if *code == ctrl_char::FIELD_START
                        && let Some(url) = crate::field::hyperlink_url(control)
                    {
                        close_marks(&mut body, &mut style);
                        close_class_span(&mut body, &mut span_id);
                        body.push_str("<a href=\"");
                        body.push_str(&escape(&url));
                        body.push_str("\">");
                        link_open = true;
                    } else {
                        render_control(doc, control, *code, ctx, &mut body, out);
                    }
                }
            }
        }
        wchar_pos += ch.wchar_width();
    }
    if link_open {
        close_marks(&mut body, &mut style);
        close_class_span(&mut body, &mut span_id);
        body.push_str("</a>");
    }
    close_marks(&mut body, &mut style);
    close_class_span(&mut body, &mut span_id);
    body
}

/// 글자 모양 클래스 span (계약 v2 §8) — 마크 태그를 감싼다.
fn open_class_span(body: &mut String, want: Option<u16>, cur: &mut Option<u16>) {
    if let Some(id) = want {
        body.push_str(&format!("<span class=\"cs{id}\">"));
    }
    *cur = want;
}

fn close_class_span(body: &mut String, cur: &mut Option<u16>) {
    if cur.is_some() {
        body.push_str("</span>");
    }
    *cur = None;
}

fn render_control(
    doc: &Document,
    control: &Control,
    code: u16,
    ctx: &mut Ctx,
    body: &mut String,
    out: &mut String,
) {
    match control {
        Control::SectionDef(_) => {}
        Control::Picture(pic) => match doc.resolve_bin(&pic.bin_ref) {
            // 자기완결: 이미지 바이트를 data URI로 임베드한다. alt는 "image" 유지.
            Some(data) => {
                let (_, mime) = crate::image::image_kind(data);
                body.push_str("<img alt=\"image\" src=\"data:");
                body.push_str(mime);
                body.push_str(";base64,");
                body.push_str(&crate::base64::encode(data));
                body.push_str("\"/>");
            }
            None => body.push_str("<img alt=\"image\"/>"),
        },
        Control::Table(table) => render_table(doc, table, ctx, out),
        Control::Generic(g) => {
            // 각주/미주 → 본문 `<sup>` 앵커 마커 + 문서 끝 정의 (본문 인라인 흡수 대체, GH-3).
            if code == ctrl_char::FOOTNOTE_ENDNOTE && matches!(&g.ctrl_id, b"fn  " | b"en  ") {
                let label = if g.ctrl_id == *b"fn  " {
                    ctx.foot_n += 1;
                    ctx.foot_n.to_string()
                } else {
                    ctx.end_n += 1;
                    format!("e{}", ctx.end_n)
                };
                let html = note_text(doc, g, ctx);
                ctx.notes.push(Note {
                    label: label.clone(),
                    html,
                });
                body.push_str(&format!(
                    "<sup id=\"fnref-{label}\"><a href=\"#fn-{label}\">{label}</a></sup>"
                ));
                return;
            }
            if code == ctrl_char::HEADER_FOOTER || code == ctrl_char::HIDDEN_COMMENT {
                return;
            }
            for list in &g.paragraph_lists {
                for p in &list.paragraphs {
                    let mut sub_out = String::new();
                    let inline = render_inline(doc, p, ctx, &mut sub_out);
                    let inline = inline.trim();
                    if !inline.is_empty() {
                        if !body.is_empty() && !body.ends_with([' ', '>']) {
                            body.push(' ');
                        }
                        body.push_str(inline);
                    }
                    out.push_str(&sub_out);
                }
            }
        }
    }
}

/// 표 — 병합 셀이 덮는 칸을 건드리지 않고 origin 셀에 colspan/rowspan을 방출한다(GH-4).
/// 셀 내용은 문단 인라인 + 블록(중첩 표·이미지)을 등장 순서대로 보존한다(GH-5).
/// 첫 행은 `<th>` 관례(표현 전용 — from_html은 th/td를 구별하지 않는다).
fn render_table(doc: &Document, table: &Table, ctx: &mut Ctx, out: &mut String) {
    let rows = table.rows.max(1) as usize;
    let cols = table.cols.max(1) as usize;
    // 병합 셀이 덮는 칸 표시 격자.
    let mut covered = vec![vec![false; cols]; rows];
    out.push_str("<table>\n");
    for r in 0..rows {
        out.push_str("<tr>");
        for c in 0..cols {
            if covered[r][c] {
                continue; // 앞선 병합 셀이 덮은 칸
            }
            let Some(cell) = table
                .cells
                .iter()
                .find(|cell| cell.row as usize == r && cell.col as usize == c)
            else {
                out.push_str("<td></td>");
                continue;
            };
            for dr in 0..cell.row_span.max(1) as usize {
                for dc in 0..cell.col_span.max(1) as usize {
                    if let Some(slot) = covered.get_mut(r + dr).and_then(|row| row.get_mut(c + dc))
                    {
                        *slot = true;
                    }
                }
            }
            let mut attrs = String::new();
            if cell.col_span > 1 {
                attrs.push_str(&format!(" colspan=\"{}\"", cell.col_span));
            }
            if cell.row_span > 1 {
                attrs.push_str(&format!(" rowspan=\"{}\"", cell.row_span));
            }
            let tag = if r == 0 { "th" } else { "td" };
            let content = render_cell(doc, cell, ctx);
            out.push_str(&format!("<{tag}{attrs}>{content}</{tag}>"));
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</table>\n");
}

/// 셀 내용 — 문단 인라인과 블록 fragment를 등장 순서대로 `<br/>`로 잇는다.
/// 이전에는 블록 버퍼(cell_out)를 만들고 버려 중첩 표·이미지가 유실됐다(GH-5).
/// 셀 내용은 스타일 클래스를 싣지 않는다(계약 v2 §8.3).
fn render_cell(doc: &Document, cell: &Cell, ctx: &mut Ctx) -> String {
    let saved = ctx.no_style_class;
    ctx.no_style_class = true;
    let mut content = String::new();
    for p in &cell.paragraphs {
        let mut blocks = String::new();
        let inline = render_inline(doc, p, ctx, &mut blocks);
        for fragment in [inline, blocks] {
            let fragment = fragment.trim();
            if fragment.is_empty() {
                continue;
            }
            if !content.is_empty() {
                content.push_str("<br/>");
            }
            content.push_str(fragment);
        }
    }
    ctx.no_style_class = saved;
    content
}

/// 각주/미주 본문 — 문단 인라인을 `<br/>`로 잇는다 (표현 전용, 클래스 미발행).
fn note_text(doc: &Document, g: &hwp_model::GenericControl, ctx: &mut Ctx) -> String {
    let saved = ctx.no_style_class;
    ctx.no_style_class = true;
    let mut parts: Vec<String> = Vec::new();
    for list in &g.paragraph_lists {
        for p in &list.paragraphs {
            let mut blocks = String::new();
            let inline = render_inline(doc, p, ctx, &mut blocks);
            for fragment in [inline, blocks] {
                let fragment = fragment.trim();
                if !fragment.is_empty() {
                    parts.push(fragment.to_string());
                }
            }
        }
    }
    ctx.no_style_class = saved;
    parts.join("<br/>")
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Style {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

impl Style {
    fn from_shape(s: &CharShape) -> Self {
        Style {
            bold: s.is_bold(),
            italic: s.is_italic(),
            underline: s.has_underline(),
            strike: s.has_strike(),
        }
    }
}

fn open_marks(body: &mut String, s: Style) {
    if s.bold {
        body.push_str("<strong>");
    }
    if s.italic {
        body.push_str("<em>");
    }
    if s.underline {
        body.push_str("<u>");
    }
    if s.strike {
        body.push_str("<s>");
    }
}

fn close_marks(body: &mut String, s: &mut Style) {
    // 여는 순서(strong→em→u→s)의 역순으로 닫는다.
    if s.strike {
        body.push_str("</s>");
    }
    if s.underline {
        body.push_str("</u>");
    }
    if s.italic {
        body.push_str("</em>");
    }
    if s.bold {
        body.push_str("</strong>");
    }
    *s = Style::default();
}

fn shape_at<'d>(doc: &'d Document, para: &Paragraph, pos: u32) -> Option<(u16, &'d CharShape)> {
    let id = para
        .char_shape_runs
        .iter()
        .rev()
        .find(|(start, _)| *start <= pos)
        .map(|(_, id)| *id)?;
    doc.header.char_shapes.get(id.0 as usize).map(|s| (id.0, s))
}

fn push_escaped(out: &mut String, c: char) {
    match c {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        _ => out.push(c),
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// HTML 태그를 제거해 평문만 남긴다 (제목 추출용, 단순 처리).
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::from_markdown::from_markdown;
    use hwp_model::{BorderFillId, HwpUnit};

    /// 표 컨트롤 앵커 문자 (markdown.rs 테스트와 같은 패턴).
    fn control_anchor(index: u32) -> HwpChar {
        HwpChar::ExtCtrl {
            code: ctrl_char::OBJECT,
            ctrl_id: *b"tbl ",
            payload: vec![],
            ctrl_index: Some(index),
        }
    }

    fn insert_table(paragraph: &mut Paragraph, table: Table) {
        let index = paragraph.controls.len() as u32;
        paragraph.controls.push(Control::Table(table));
        paragraph.chars.push(control_anchor(index));
    }

    fn text_paragraph(text: &str) -> Paragraph {
        Paragraph {
            chars: text.chars().map(HwpChar::Text).collect(),
            ..Paragraph::default()
        }
    }

    fn cell(row: u16, col: u16, col_span: u16, row_span: u16, text: &str) -> Cell {
        Cell {
            list_attr: 0,
            col,
            row,
            col_span,
            row_span,
            width: HwpUnit(0),
            height: HwpUnit(0),
            margins: [0; 4],
            border_fill: BorderFillId(0),
            header_tail: vec![],
            paragraphs: vec![text_paragraph(text)],
        }
    }

    fn table(rows: u16, cols: u16, cells: Vec<Cell>) -> Table {
        Table {
            common_data: vec![],
            placement: None,
            attr: 0,
            rows,
            cols,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: vec![cols; rows as usize],
            border_fill: BorderFillId(0),
            table_tail: vec![],
            cells,
            extras: vec![],
        }
    }

    #[test]
    fn 제목_본문_표_렌더() {
        let doc =
            from_markdown("# 제목\n\n본문 문단입니다.\n\n| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let html = to_html(&doc);
        assert!(html.contains("<!DOCTYPE html>"));
        // 헤딩 char-shape이 굵게라 본문이 <strong>으로 감싸일 수 있음 — 구조만 확인.
        assert!(html.contains("<h1 class=") && html.contains("제목"));
        assert!(
            html.contains("<p class=\"ps2\">") && html.contains("본문 문단입니다."),
            "{html}"
        );
        assert!(html.contains("<table>"));
        assert!(html.contains("<th>") && html.contains("가"));
        assert!(html.contains("<td>") && html.contains("1</td>"));
        // 계약 v2: 사용된 모양의 cs/ps 규칙이 실린다.
        assert!(html.contains(".ps2{"), "ps 규칙: {html}");
        assert!(html.contains(".cs"), "cs 규칙: {html}");
    }

    #[test]
    fn 특수문자_이스케이프() {
        let doc = from_markdown("a < b & c > d\n");
        let html = to_html(&doc);
        assert!(html.contains("a &lt; b &amp; c &gt; d"));
    }

    #[test]
    fn 하이퍼링크_앵커_렌더() {
        let doc = from_markdown("자세히는 [여기](https://example.com/a?b=1&c=2)를 볼라\n");
        let html = to_html(&doc);
        // href는 속성 이스케이프(&amp;), 표시 텍스트는 <a>…</a>로 감싼다.
        assert!(
            html.contains("<a href=\"https://example.com/a?b=1&amp;c=2\">"),
            "href 이스케이프: {html}"
        );
        assert!(
            html.contains("여기") && html.contains("</a>"),
            "링크 닫힘: {html}"
        );
    }

    #[test]
    fn 이미지_data_uri_임베드() {
        let mut doc = from_markdown("사진: 여기");
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend(96u32.to_be_bytes());
        png.extend(96u32.to_be_bytes());
        png.extend([0u8; 8]);
        let p = std::env::temp_dir().join("html_img_embed.png");
        std::fs::write(&p, &png).unwrap();
        crate::image::insert_image(&mut doc, "사진:", &p, crate::image::ImageSize::Natural)
            .unwrap();
        let html = to_html(&doc);
        let expect = format!(
            "<img alt=\"image\" src=\"data:image/png;base64,{}\"/>",
            crate::base64::encode(&png)
        );
        assert!(html.contains(&expect), "data URI 임베드: {html}");
    }

    #[test]
    fn fragment_헤드없음() {
        let doc = from_markdown("# 제목\n\n본문\n");
        let frag = to_html_fragment(&doc);
        assert!(!frag.contains("<!DOCTYPE"));
        assert!(!frag.contains("<head>"));
        // 계약 v2: fragment는 선두 <style>로 자기완결된다(계약 §8.1).
        assert!(frag.starts_with("<style>"), "{frag}");
        assert!(frag.contains("제목"));
        // standalone에는 head/style이 있어야 한다 (대조).
        let full = to_html(&doc);
        assert!(full.contains("<head>") && full.contains("<style>"));
    }

    #[test]
    fn 병합셀_colspan_rowspan_방출() {
        // 2×2 표에서 (0,0)이 2열 병합, (1,0)이 2행 병합…이 아니라 단순하게:
        // (0,0) colspan=2, (0,2) 별도 — 대신 2×3 표로 구성.
        let mut doc = from_markdown("표\n");
        let t = table(
            2,
            3,
            vec![
                cell(0, 0, 2, 1, "가로병합"),
                cell(0, 2, 1, 2, "세로병합"),
                cell(1, 0, 1, 1, "a"),
                cell(1, 1, 1, 1, "b"),
            ],
        );
        insert_table(&mut doc.sections[0].paragraphs[0], t);
        let html = to_html(&doc);
        assert!(
            html.contains("<th colspan=\"2\">가로병합</th>"),
            "colspan: {html}"
        );
        assert!(
            html.contains("<th rowspan=\"2\">세로병합</th>"),
            "rowspan: {html}"
        );
        // 병합이 덮은 칸은 빈 셀로 채우지 않는다 — 두 행의 셀 수가 달라야 한다.
        let row2 = html.split("</tr>").nth(1).unwrap();
        assert_eq!(row2.matches("<td").count(), 2, "덮인 칸 미방출: {html}");
    }

    #[test]
    fn 셀_내_중첩_표_보존() {
        let mut doc = from_markdown("표\n");
        // 바깥 셀에 중첩 표를 넣는다.
        let mut inner_holder = text_paragraph("셀텍스트");
        let inner = table(1, 1, vec![cell(0, 0, 1, 1, "중첩")]);
        insert_table(&mut inner_holder, inner);
        let outer_cell = Cell {
            paragraphs: vec![inner_holder],
            ..cell(0, 0, 1, 1, "")
        };
        let outer = table(1, 1, vec![outer_cell]);
        insert_table(&mut doc.sections[0].paragraphs[0], outer);
        let html = to_html(&doc);
        // 중첩 표가 <table> 두 겹으로 존재해야 한다 (이전엔 블록 버퍼 폐기로 유실).
        assert_eq!(html.matches("<table>").count(), 2, "중첩 표 보존: {html}");
        assert!(html.contains("중첩"), "중첩 셀 텍스트: {html}");
        assert!(html.contains("셀텍스트"), "바깥 셀 텍스트: {html}");
    }

    #[test]
    fn 각주_미주_마커와_정의() {
        let doc = from_markdown("본문 문단[^1]입니다.\n\n[^1]: 각주 내용\n");
        let html = to_html(&doc);
        assert!(
            html.contains("<sup id=\"fnref-1\"><a href=\"#fn-1\">1</a></sup>"),
            "본문 마커: {html}"
        );
        assert!(
            html.contains("<section class=\"footnotes\">"),
            "정의 섹션: {html}"
        );
        assert!(
            html.contains("<li id=\"fn-1\">각주 내용"),
            "정의 항목: {html}"
        );
        // fragment에도 정의가 포함된다(자기완결).
        let frag = to_html_fragment(&doc);
        assert!(frag.contains("<section class=\"footnotes\">"));
    }

    #[test]
    fn xhtml_빈태그_self_closing() {
        // md 하드브레이크(행말 공백 2칸) → LINE_BREAK → <br/>.
        let doc = from_markdown("첫 줄  \n둘째 줄\n");
        let html = to_html(&doc);
        // 줄바꿈이 <br/>로 방출되고 <br> 꼴이 없어야 한다.
        assert!(!html.contains("<br>"), "self-closing br: {html}");
        assert!(html.contains("<br/>"), "br 방출: {html}");
    }
}
