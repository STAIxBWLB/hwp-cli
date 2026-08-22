//! HTML fragment (contract: docs/design/18) → IR.
//!
//! Consumer side of the non-prose block (table/image) exchange path for the Maru part writer.
//! Accepts only the well-formed XHTML subset emitted by the producer (`html.rs`); contract
//! violations (unlisted tags, malformed XML, span mismatch, empty tables) are hard errors — no
//! guess-based recovery.
//!
//! Remaining presentation-only elements: `class`/`style` attributes (ignored) and
//! `<section class="footnotes">` (skipped entirely). Footnote markers (`sup` with a `fnref` id)
//! are reattached to GFM definition bodies on the md-mixed path (#47); standalone they are
//! taken as plain text only.

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

/// Number of char shapes (0~15) in the default_header palette. Additional char shapes from
/// from_html (underline/superscript combinations) are appended after these — the md-mixed path
/// (from_markdown) uses the same default_header, so ids do not collide on merge.
pub(crate) const PALETTE_LEN: u16 = 16;

/// HTML import options.
#[derive(Default)]
pub struct HtmlImportOptions<'a> {
    /// Base directory for relative-path images (`<img src="fig.png">`).
    pub base_dir: Option<&'a Path>,
    /// Sandbox roots binding image references (MCP `--root`, #56). Same contract as
    /// `MarkdownImportOptions::roots`: empty disables the check; with roots set, an image
    /// resolving outside every root is a hard error that fails the import.
    pub roots: &'a [PathBuf],
    /// Seed for embedded image bin names — prevents name collisions when combining multiple
    /// fragments into one document.
    pub(crate) bin_seed: usize,
    /// Pre-collected GFM footnote/endnote definition bodies (label → paragraphs). On the
    /// md-mixed path, fnref markers inside fragments are reattached to these as real
    /// footnote anchors (#47). None (standalone HTML) keeps the plain-text marker behavior.
    pub(crate) note_bodies: Option<&'a HashMap<String, Vec<Paragraph>>>,
}

/// Fragment parse failure. `Sandbox` (image reference outside the MCP `--root` sandbox, #56)
/// must stay a hard error even where a contract violation would degrade to a warning
/// (the from_markdown md-mixed path).
pub(crate) enum FragmentError {
    Contract(String),
    Sandbox(String),
    AuthoredList(String),
}

impl FragmentError {
    fn into_message(self) -> String {
        match self {
            Self::Contract(message) | Self::Sandbox(message) => message,
            Self::AuthoredList(error) => error,
        }
    }
}

/// Output of fragment parsing — used by both standalone document assembly (from_html) and
/// md-mixed merging (from_markdown).
#[derive(Default)]
pub(crate) struct HtmlBlocks {
    pub paragraphs: Vec<Paragraph>,
    pub bin_streams: Vec<BinStream>,
    /// Additional char shapes appended after the default_header palette. id = PALETTE_LEN + index.
    pub extra_char_shapes: Vec<CharShape>,
    /// List para shapes (index BASE_PARA_SHAPES + index) and numbering/bullet definitions.
    pub extra_para_shapes: Vec<ParaShape>,
    /// Additional fonts appended after the default_header fonts (2 per slot) — extended in the
    /// same order across all language slots (contract v2: font-family restoration from the style block).
    pub extra_fonts: Vec<hwp_model::FaceName>,
    pub numbering_levels: Vec<Vec<NumLevel>>,
    pub bullet_chars: Vec<char>,
    pub warnings: Vec<String>,
}

/// Converts an HTML fragment into a document (existing signature convention — no options).
pub fn from_html(html: &str) -> Result<Document, String> {
    from_html_with(html, &HtmlImportOptions::default())
}

/// Variant that takes options. Accepts both standalone documents (`<html>`~`</html>`) and fragments.
pub fn from_html_with(html: &str, opts: &HtmlImportOptions) -> Result<Document, String> {
    let blocks = parse_fragment(html, opts).map_err(FragmentError::into_message)?;
    for w in &blocks.warnings {
        eprintln!("경고: {w}");
    }
    let mut paragraphs = blocks.paragraphs;
    if paragraphs.is_empty() {
        // Close even an empty document with one paragraph. The writer guarantees the paragraph-end char.
        paragraphs.push(Paragraph::default());
    }
    // Inject section/column definitions into the first paragraph — prerequisite for hwp5/Hancom
    // compatibility (same as from_markdown).
    from_markdown::inject_section_controls(
        &mut paragraphs[0],
        None,
        crate::official::PageMarginOverrides::default(),
    );

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
        hwpx_preview_image: None,
        hwp5_xml_template: Vec::new(),
        hwp5_doc_history: Vec::new(),
        hwpx_extra_entries: Vec::new(),
        hwpx_bin_manifest: Vec::new(),
        hwpx_opf_extra_items: Vec::new(),
        hwpx_section_xmlns: Vec::new(),
    })
}

/// Produces the fragment parsing output (entry point of the from_markdown mixed path).
pub(crate) fn parse_fragment(
    html: &str,
    opts: &HtmlImportOptions,
) -> Result<HtmlBlocks, FragmentError> {
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
        roots: opts.roots,
        sandbox_error: None,
        list_error: None,
        note_bodies: opts.note_bodies,
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
    if let Err(e) = p.blocks(&mut reader, None) {
        // A recorded sandbox error is the error being returned (embed_image sets it and aborts
        // the parse immediately), so the variant switch cannot mislabel a contract violation.
        return Err(match p.sandbox_error {
            Some(message) => FragmentError::Sandbox(message),
            None => match p.list_error {
                Some(error) => FragmentError::AuthoredList(error),
                None => FragmentError::Contract(e),
            },
        });
    }
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

/// Inline mark state.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
struct Marks {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    sup: bool,
    sub: bool,
}

/// Block context — the top level and each table cell have independent paragraph buffers.
#[derive(Default)]
struct BlockCtx {
    paragraphs: Vec<Paragraph>,
    chars: Vec<HwpChar>,
    runs: Vec<(u32, CharShapeId)>,
    controls: Vec<Control>,
    wchar_pos: u32,
    style: u16,
}

/// One list level (frame).
struct ListFrame {
    para_shape_id: u16,
    item_open: bool,
    ordered_start: Option<u32>,
    item_count: u32,
}

/// Cell spec — position (finalized after occupancy-grid placement) and content.
struct CellSpec {
    col_span: u16,
    row_span: u16,
    paragraphs: Vec<Paragraph>,
}

struct Parser<'a> {
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
    /// Sandbox roots for image containment (MCP `--root`, #56). Empty = no check.
    roots: &'a [PathBuf],
    /// Set when the parse aborts on a sandbox violation — parse_fragment re-labels the error
    /// as FragmentError::Sandbox so the md-mixed path keeps it a hard error (#56).
    sandbox_error: Option<String>,
    list_error: Option<String>,
    /// GFM footnote definition bodies for fnref marker reattachment (None on the standalone path).
    note_bodies: Option<&'a HashMap<String, Vec<Paragraph>>>,
    warnings: Vec<String>,
    in_cell_depth: u32,
    // contract v2 style round-trip — <style> rule storage and restoration caches.
    cs_rules: HashMap<u16, HashMap<String, String>>,
    ps_rules: HashMap<u16, HashMap<String, String>>,
    cs_cache: HashMap<u16, CharShapeId>,
    ps_cache: HashMap<u16, u16>,
    /// Cache of (class shape id, marks) combination variants.
    variant_cache: HashMap<(u16, Marks), u16>,
    /// Restored shape of the active `<span class="csN">`.
    span_shape: Option<CharShapeId>,
    /// Restored shape of the current paragraph's `class="psN"`.
    para_class: Option<u16>,
    /// Copy of the default palette (basis for dedup and variants).
    palette: Vec<CharShape>,
    palette_para: Vec<ParaShape>,
    /// Language slot 0 font list (2 defaults + restored additions). Additions are emitted via HtmlBlocks.
    fonts: Vec<hwp_model::FaceName>,
    default_fonts: usize,
}

impl Parser<'_> {
    fn ctx(&mut self) -> &mut BlockCtx {
        self.ctx_stack.last_mut().expect("컨텍스트 스택 비어 있음")
    }

    /// Block loop. If `end` is given, runs until that closing tag (for table cells, li, figure, etc.).
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
                                match attr(&e, "start") {
                                    Some(value) => match value.parse::<u64>() {
                                        Ok(start) => Some(start),
                                        Err(_) => {
                                            let error = format!(
                                                "invalid ordered-list start value: {value}"
                                            );
                                            self.list_error = Some(error.clone());
                                            return Err(error);
                                        }
                                    },
                                    None => Some(1),
                                }
                            } else {
                                None
                            };
                            self.start_list(start).map_err(|error| {
                                self.list_error = Some(error.clone());
                                error
                            })?;
                            self.blocks(r, Some(&name))?;
                            self.end_list();
                        }
                        "li" => {
                            self.flush_paragraph(false);
                            if let Err(error) = self.start_list_item() {
                                self.list_error = Some(error.clone());
                                return Err(error);
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
                            // A new <style> block replaces the previous rule set — class ids are
                            // block-scoped, so the same id in a following block restores to the new rules.
                            self.cs_rules.clear();
                            self.ps_rules.clear();
                            self.cs_cache.clear();
                            self.ps_cache.clear();
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
                        b"hr" | b"meta" => {} // hr and head meta — no content
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
                _ => {} // Decl, DocType, Comment, PI, CData (outside the contract) are ignored
            }
        }
    }

    /// Inline loop — until the closing of the `end` tag.
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

    /// Handles one inline formatting tag (entered from both block and inline loops).
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
                // Footnote markers (`fnref` id): on the md-mixed path the pre-collected GFM
                // definition body is reattached as a real footnote/endnote anchor (#47) — the
                // marker subtree (digits + `#fn-N` back-link) is consumed. Without a body
                // (standalone HTML) the marker stays plain text without the sup mark.
                let marker_label =
                    attr(e, "id").and_then(|id| id.strip_prefix("fnref-").map(str::to_string));
                if let Some(body) = marker_label
                    .as_deref()
                    .and_then(|l| self.note_bodies.and_then(|m| m.get(l)).cloned())
                {
                    self.skip_subtree(r, "sup")?;
                    let label = marker_label.expect("marker label present");
                    let ctx = self.ctx();
                    let idx = ctx.controls.len() as u32;
                    let (ch, control) = from_markdown::footnote_anchor(&label, body, idx);
                    ctx.chars.push(ch);
                    ctx.wchar_pos += 8; // FOOTNOTE_ENDNOTE ExtCtrl = 8 WCHAR
                    ctx.controls.push(control);
                    return Ok(());
                }
                let saved = self.marks;
                if marker_label.is_none() {
                    self.marks.sup = true;
                }
                let result = self.inline(r, "sup");
                self.marks = saved;
                result
            }
            "span" => {
                // Style class span (contract v2 §8) — applies the restored shape, not marks, to the current run.
                // A <span> without a cs class (a wrapper added by an editor, etc.) passes through
                // transparently, preserving the outer shape.
                let saved = self.span_shape;
                if let Some(shape) = class_cs(e).and_then(|n| self.cs_shape(n)) {
                    self.span_shape = Some(shape);
                }
                let result = self.inline(r, "span");
                self.span_shape = saved;
                result
            }
            tag @ ("strong" | "em" | "b" | "i" | "u" | "s" | "sub") => {
                let saved = self.marks;
                match tag {
                    "strong" | "b" => self.marks.bold = true,
                    "em" | "i" => self.marks.italic = true,
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

    /// Table parsing — reconstructs cell positions with an occupancy grid and flags span mismatches as errors.
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
        // A cell must have at least 1 paragraph (nparas≥1) — even an empty cell gets an empty paragraph.
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

    /// Assigns cell positions with the occupancy grid and builds the Table.
    /// Empty slots are filled with empty cells (IR tiling invariant), so HTML tables with
    /// ragged row lengths are accepted.
    fn build_table(&mut self, rows: Vec<Vec<CellSpec>>) -> Result<Table, String> {
        if rows.is_empty() {
            return Err("행이 없는 표입니다".into());
        }
        let n_rows = rows.len();
        // Pass 1: place each cell in the first free slot and determine the column count.
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
                // Mark the span area (includes overlap check).
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
        // Pass 2: fill uncovered empty slots with empty cells to complete the tiling.
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
        let row_h = 1700i32; // 10pt text + cell top/bottom margins (same as from_markdown)
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
            caption: None,
            cells,
            extras: Vec::new(),
        };
        // Final gate — the 5-rule invariant measured from genuine files (area tiling, row-major,
        // row_cell_counts, etc.).
        crate::edit::validate_table_invariants(&table)?;
        Ok(table)
    }

    /// Puts the finished table into the current context as an anchor paragraph (one extended control).
    fn push_table(&mut self, table: Table) {
        self.flush_paragraph(false);
        let mut payload = vec![0u8; 12];
        payload[..4].copy_from_slice(b" lbt"); // reversed ctrl_id
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

    /// `<img>` — embeds data URIs and relative paths. SVG is validated and rasterized to a
    /// deterministic PNG (hwp-convert::svg — same policy as DocumentSpec v2: no native representation).
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
            // Sandbox containment (#56): with roots set, the check is verified against the
            // opened file handle and the bytes are read from that same handle, so a swapped
            // symlink cannot smuggle outside bytes in. A violation is a hard error that fails
            // the import — recorded so parse_fragment can label it FragmentError::Sandbox.
            // The message is deliberately generic (no resolved path, no src).
            let soft_open =
                |e: std::io::Error| format!("이미지 읽기 실패 {}: {e}", resolved.display());
            let mut file =
                match crate::image::open_image_under_roots(&resolved, self.roots, soft_open) {
                    Ok(file) => file,
                    Err(crate::image::ImageOpenError::Soft(message)) => return Err(message),
                    Err(crate::image::ImageOpenError::Hard(message)) => {
                        self.sandbox_error = Some(message.clone());
                        return Err(message);
                    }
                };
            if resolved
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
            {
                let mut text = String::new();
                std::io::Read::read_to_string(&mut file, &mut text)
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
                // 96dpi → HWPUNIT 75/px. Scales down preserving the aspect ratio if wider than the body.
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
                let mut data = Vec::new();
                std::io::Read::read_to_end(&mut file, &mut data)
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
            treat_as_char: true, // inline (as-char) placement — the writer synthesizes the shape record
            z_order: 0,
            vert_offset: 0,
            horz_offset: 0,
            description: None,
            crop: None,
            flip: 0,
            rotation: None,
            brightness: 0,
            contrast: 0,
            effect_flags: 0,
            effects_raw: Vec::new(),
            caption: None,
            bin_ref: BinRef::ItemRef(name.clone()),
            extras: Vec::new(),
        }));
        // gso anchor char (code 11) — same convention as insert_image. relink reassigns ctrl_index.
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

    fn start_list(&mut self, start: Option<u64>) -> Result<(), String> {
        self.flush_paragraph(false);
        let level = from_markdown::validate_official_list_depth(self.list_stack.len() + 1)
            .map_err(|error| error.to_string())?;
        let para_shape_id = match start {
            Some(s) => {
                let start = from_markdown::normalize_authored_list_start(s)?;
                let def_id = self.numbering_levels.len() as u16;
                let mut levels =
                    vec![NumLevel::default(); from_markdown::MAX_OFFICIAL_LIST_DEPTH as usize];
                levels[level as usize - 1].start = start;
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
            ordered_start: start
                .map(from_markdown::normalize_authored_list_start)
                .transpose()?,
            item_count: 0,
        });
        Ok(())
    }

    fn start_list_item(&mut self) -> Result<(), String> {
        let Some(frame) = self.list_stack.last_mut() else {
            return Ok(());
        };
        if let Some(start) = frame.ordered_start {
            let next_item = frame.item_count.checked_add(1).ok_or_else(|| {
                "authored ordered-list item count exceeds u32 maximum".to_string()
            })?;
            from_markdown::validate_authored_list_item(start, next_item)?;
            frame.item_count = next_item;
        }
        frame.item_open = true;
        Ok(())
    }

    fn end_list(&mut self) {
        self.flush_paragraph(false);
        self.list_stack.pop();
    }

    /// Para shape for list items (same rules as from_markdown::Builder).
    fn push_list_para_shape(&mut self, head_type: u32, level: u16, def_id: u16) -> u16 {
        let idx = BASE_PARA_SHAPES + self.extra_para_shapes.len() as u16;
        let step = 2000i32;
        self.extra_para_shapes.push(ParaShape {
            attr1: 0x180
                | (1 << 2)
                | (head_type << 23)
                | (u32::from(if level > 7 { 7 } else { level }) << 25),
            margin_left: i32::from(level) * step,
            indent: -step,
            line_spacing_old: 160,
            line_spacing: 160,
            border_fill_id: 2,
            numbering_id: def_id,
            list_level: (level > 7).then_some(level as u8),
            ..ParaShape::default()
        });
        idx
    }

    fn push_line_break(&mut self) {
        let ctx = self.ctx();
        ctx.chars.push(HwpChar::CharCtrl(10));
        ctx.wchar_pos += 1;
    }

    /// HTML text node — drops whitespace-only nodes between blocks and folds newlines into spaces.
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

    /// Char shape ID for the current marks/heading/link/span-class state.
    fn shape_id(&mut self) -> u16 {
        if self.in_link {
            return shapes::HYPERLINK;
        }
        if let Some(level) = self.heading {
            return shapes::HEADING_BASE + level - 1;
        }
        // Restored shape of `<span class="csN">` (contract v2) — allocates a variant once if marks are applied.
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
                shape.attr |= 1 << 2; // underline type 1 (under the character)
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
            // Palette combination (bold/italic/strike).
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
        // Underline/superscript combinations are not in the palette — allocated once after the palette (cached).
        if let Some(&id) = self.shape_cache.get(&m) {
            return id;
        }
        let id = PALETTE_LEN + self.extra_char_shapes.len() as u16;
        let mut attr = u32::from(m.bold) << 1 | u32::from(m.italic);
        if m.underline {
            attr |= 1 << 2; // underline type 1 (under the character)
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

    /// Closes the paragraph (same invariants as from_markdown::Builder::flush_paragraph_inner).
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
            id // `class="psN"` restored shape (contract v2)
        } else if heading.is_some() {
            1 // heading (unlike the md path, indent variants use a single v1 shape)
        } else if in_cell {
            0 // table cell (no spacing)
        } else {
            2 // body
        };
        let mut para = Paragraph {
            para_shape: ParaShapeId(para_shape),
            style: StyleId(ctx.style),
            chars: std::mem::take(&mut ctx.chars),
            char_shape_runs: runs,
            controls: std::mem::take(&mut ctx.controls),
            ..Paragraph::default()
        };
        // Links FIELD_START (hyperlink, etc.) ExtCtrl ↔ controls in order of appearance.
        crate::field::relink_ctrl_index(&mut para);
        ctx.wchar_pos = 0;
        ctx.paragraphs.push(para);
    }

    /// Reads the text of `<style>` (until it closes).
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
                // quick-xml 0.40 emits references as separate events — restores escaped font
                // names like `&lt;` to the original characters. Unknown references are preserved as-is.
                Ok(Event::GeneralRef(g)) => match resolve_entity(&g) {
                    Some(c) => css.push(c),
                    None => {
                        css.push('&');
                        css.push_str(&String::from_utf8_lossy(&g));
                        css.push(';');
                    }
                },
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

    /// Extracts only `.cs{n}`/`.ps{n}` rules from a `<style>` block (other rules and declarations are ignored).
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

    /// `.cs{n}` rule → CharShape restoration (allocated once after palette dedup).
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
        // Palette dedup — reuses the palette id for an identical shape.
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

    /// `.ps{n}` rule → ParaShape restoration (allocated once after palette dedup).
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
                // fixed line spacing is stored doubled in the IR (like the margin fields)
                ps.line_spacing = (pt * 200.0).round() as i32;
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
                .map(|mm| (mm * 14400.0 / 25.4).round() as i32)
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
        // Palette dedup.
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

    /// Font name → face id (appends if missing). Additions are appended to all slots in the same order.
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

    /// Skips everything until the closing of the `end` tag (tracks nested identical tags).
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

/// Finds the n of the `cs{n}`-prefixed class in the class attribute (contract v2).
fn class_cs(e: &BytesStart<'_>) -> Option<u16> {
    class_num(e, "cs")
}

/// Finds the n of the `ps{n}`-prefixed class in the class attribute (contract v2).
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

/// Attribute lookup (by local name, with entity resolution) — same rules as hwpx read/xml.rs.
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

/// Resolves general references like `&amp;` — same rules as hwpx read/mod.rs.
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

    /// Extracts paragraph text (test simplification helper).
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
        // The link display text uses the hyperlink char shape (blue + underline), so a <u> is attached.
        assert!(
            html.contains("<a href=\"https://x.io\">") && html.contains("링크</u>"),
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
        // On re-export it must come out as a data URI again.
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

    /// #56: `<img>` references are bound to the sandbox roots — an outside-root reference
    /// (absolute path or `../` escape) fails the import; inside-root references embed as
    /// usual; empty roots keeps the previous behavior (an absolute outside path still loads).
    #[test]
    fn 이미지_샌드박스_루트_검사() {
        let base = std::env::temp_dir().join(format!(
            "from_html_img_roots_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = base.join("sandbox");
        let sub = root.join("sub");
        let outside = base.join("outside");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        // Minimal PNG (magic + IHDR with 8x8).
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend(8u32.to_be_bytes());
        png.extend(8u32.to_be_bytes());
        png.extend([0u8; 8]);
        std::fs::write(sub.join("in.png"), &png).unwrap();
        let outside_png = outside.join("out.png");
        std::fs::write(&outside_png, &png).unwrap();
        // HTML attributes treat `\` literally, but forward slashes work everywhere (Windows CI).
        let outside_ref = outside_png.display().to_string().replace('\\', "/");
        // Roots are pre-canonicalized by the caller (mirrors the MCP startup).
        let roots = vec![std::fs::canonicalize(&root).unwrap()];

        // Inside the root: embeds as usual.
        let doc = from_html_with(
            "<p><img src=\"in.png\"/></p>",
            &HtmlImportOptions {
                base_dir: Some(&sub),
                roots: &roots,
                ..Default::default()
            },
        )
        .expect("루트 안 이미지는 임베드");
        assert_eq!(doc.bin_streams.len(), 1, "루트 안 이미지는 임베드");

        // Absolute path outside the root: hard error.
        let err = from_html_with(
            &format!("<p><img src=\"{outside_ref}\"/></p>"),
            &HtmlImportOptions {
                base_dir: Some(&sub),
                roots: &roots,
                ..Default::default()
            },
        )
        .expect_err("루트 밖 절대 경로는 하드 에러");
        assert!(err.contains("샌드박스"), "{err}");

        // `../` relative escape outside the root: hard error.
        let err = from_html_with(
            "<p><img src=\"../../outside/out.png\"/></p>",
            &HtmlImportOptions {
                base_dir: Some(&sub),
                roots: &roots,
                ..Default::default()
            },
        )
        .expect_err("'../' 탈출은 하드 에러");
        assert!(err.contains("샌드박스"), "{err}");

        // Empty roots: previous behavior — the absolute outside path still loads.
        let doc = from_html_with(
            &format!("<p><img src=\"{outside_ref}\"/></p>"),
            &HtmlImportOptions {
                base_dir: Some(&sub),
                ..Default::default()
            },
        )
        .expect("루트 미설정이면 기존 동작");
        assert_eq!(doc.bin_streams.len(), 1, "루트 미설정이면 절대 경로 로드");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// #56 (handle-bound check): an in-root `<img>` symlink whose target is outside the roots
    /// is a hard error — the verdict comes from the opened handle, not the request pathname.
    #[cfg(unix)]
    #[test]
    fn 이미지_샌드박스_심링크_탈출_차단() {
        let base = std::env::temp_dir().join(format!(
            "from_html_img_symlink_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = base.join("sandbox");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        // Minimal PNG (magic + IHDR with 8x8).
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend(8u32.to_be_bytes());
        png.extend(8u32.to_be_bytes());
        png.extend([0u8; 8]);
        let secret = outside.join("secret.png");
        std::fs::write(&secret, &png).unwrap();
        std::os::unix::fs::symlink(&secret, root.join("escape.png")).unwrap();
        let roots = vec![std::fs::canonicalize(&root).unwrap()];

        let err = from_html_with(
            "<p><img src=\"escape.png\"/></p>",
            &HtmlImportOptions {
                base_dir: Some(&root),
                roots: &roots,
                ..Default::default()
            },
        )
        .expect_err("루트 밖을 가리키는 심링크는 하드 에러");
        assert!(err.contains("샌드박스"), "{err}");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn 목록_파싱() {
        let doc = from_html("<ol><li>첫째</li><li>둘째</li></ol>").unwrap();
        let texts: Vec<String> = doc.sections[0].paragraphs.iter().map(para_text).collect();
        assert!(texts.iter().any(|t| t.contains("첫째")), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("둘째")), "{texts:?}");
        // A numbering-head para shape must have been allocated.
        assert!(!doc.header.numbering_levels.is_empty());
    }

    #[test]
    fn export_import_구조_왕복() {
        // IR → html → IR: merged tables and images must be structurally preserved.
        let mut doc = from_markdown("본문\n\n| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        // Swap to a (0,0)~(0,1) horizontal merge — so export emits a colspan.
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
        // to_html standalone output is also readable — the footnotes section is stripped.
        let doc = from_markdown("본문[^1]입니다.\n\n[^1]: 각주 내용\n");
        let full = to_html(&doc);
        let back = from_html(&full).unwrap();
        let all_text: String = back.sections[0].paragraphs.iter().map(para_text).collect();
        assert!(all_text.contains("본문"), "본문: {all_text}");
        assert!(!all_text.contains("각주 내용"), "정의는 폐기: {all_text}");
        // The fragment is readable as-is too.
        let frag = to_html_fragment(&doc);
        assert!(from_html(&frag).is_ok());
    }

    #[test]
    fn 스타일_왕복_속성_보존() {
        // contract v2: character (color, letter spacing) and paragraph (alignment, line spacing) shapes are preserved through html.
        let mut doc = from_markdown("본문 문단입니다.\n");
        doc.header.char_shapes[0].text_color = 0x0000_00FF; // COLORREF red
        doc.header.char_shapes[0].spacings = [5; hwp_model::LANG_COUNT];
        doc.header.para_shapes[2].attr1 = 0x180 | (3 << 2); // center
        doc.header.para_shapes[2].line_spacing = 130;
        doc.header.para_shapes[2].line_spacing_old = 130;
        let html = to_html(&doc);
        assert!(html.contains("color:#FF0000"), "{html}");
        assert!(html.contains("letter-spacing:0.05em"), "{html}");
        assert!(html.contains("text-align:center"), "{html}");
        assert!(html.contains("line-height:1.3"), "{html}");
        // spacing_bottom 600(IR, 2배 단위) = 300 HWPUNIT = 1.058mm — 물리 단위 정합.
        assert!(html.contains("margin-bottom:1.058mm"), "{html}");

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

        // Re-export stability — the same attributes are emitted again.
        let html2 = to_html(&back);
        assert!(html2.contains("color:#FF0000"), "재수출: {html2}");
        assert!(html2.contains("text-align:center"), "재수출: {html2}");
    }

    #[test]
    fn 스타일_왕복_팔레트_dedup() {
        // A document using only default shapes reuses the palette as-is, so no extra shapes are created.
        let doc = from_markdown("본문 문단입니다.\n");
        let html = to_html(&doc);
        let back = from_html(&html).unwrap();
        assert_eq!(back.header.char_shapes.len(), 16, "팔레트 외 글자모양 없음");
        assert_eq!(back.header.para_shapes.len(), 5, "팔레트 외 문단모양 없음");
    }

    #[test]
    fn 스타일_왕복_글꼴_복원() {
        // Font names not in the palette are restored as additional fonts.
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

    #[test]
    fn 링크_경계_span_교차_없음() {
        // Class spans in a paragraph with a link close together at the <a> boundary — crossing
        // markup (`<a><span>…</a></span>`) would be rejected by from_html (review regression).
        let doc = from_markdown("앞 [링크](https://x.io) 뒤\n");
        let html = to_html(&doc);
        let back = from_html(&html).unwrap();
        let all_text: String = back.sections[0].paragraphs.iter().map(para_text).collect();
        assert!(all_text.contains("링크"), "{all_text}");
        // Also check that the link field was restored.
        let has_link = back.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .any(|c| crate::field::hyperlink_url(c).is_some());
        assert!(has_link, "하이퍼링크 필드 복원");
    }

    #[test]
    fn 위험_글꼴_이름은_이스케이프() {
        // Prevents early `</style>` termination and markup injection — < & in font names go out as entities.
        let mut doc = from_markdown("본문\n");
        doc.header.fonts[0].push(hwp_model::FaceName {
            name: "x</style><script>".into(),
            attr: 0x01,
            ..hwp_model::FaceName::default()
        });
        doc.header.char_shapes[0].face_ids = [2; hwp_model::LANG_COUNT];
        let html = to_html(&doc);
        assert!(
            !html.contains("x</style><script>"),
            "조기 종료 시퀀스 노출: {html}"
        );
        assert!(html.contains("&lt;/style&gt;"), "이스케이프: {html}");
        // quick-xml reverses the entities, restoring the original name.
        let back = from_html(&html).unwrap();
        let name = back.header.fonts[0]
            .iter()
            .find(|f| f.name.contains("script"))
            .map(|f| f.name.clone());
        assert_eq!(name.as_deref(), Some("x</style><script>"), "왕복: {name:?}");
    }
}
