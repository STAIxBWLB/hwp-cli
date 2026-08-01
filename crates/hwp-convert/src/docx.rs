//! IR → DOCX (OOXML) one-way export (GJ-1).
//!
//! Mapping (content fidelity, no page-layout reproduction):
//! - "개요 N" style paragraphs → HeadingN paragraph styles
//! - CharShape → run properties (b/i/u/strike/vertAlign, sz, color, shd, rFonts, spacing)
//! - ParaShape → pPr (jc, line spacing, ind, spacing before/after, numPr)
//! - Tables → `w:tbl` with gridSpan (colspan) and vMerge (rowspan); nested tables kept
//! - Pictures → `word/media/*` + inline drawings (extent in EMU)
//! - Hyperlinks → external rels + `w:hyperlink`
//! - Footnotes/endnotes → footnotes.xml/endnotes.xml + references
//! - Equations → script text as-is (v1 fallback; OMML mapping is out of scope)

use std::io::Write as _;

use hwp_model::{CharShape, Control, Document, HwpChar, NumFmt, Paragraph, ctrl_char};

/// DOCX의 모든 문서를 ZIP(OPC)로 직렬화한다.
pub fn to_docx(doc: &Document) -> std::io::Result<Vec<u8>> {
    let mut b = Builder {
        doc,
        body: String::new(),
        images: Vec::new(),
        link_rels: Vec::new(),
        footnotes: Vec::new(),
        endnotes: Vec::new(),
        foot_n: 0,
        end_n: 0,
    };
    for section in &doc.sections {
        for para in &section.paragraphs {
            b.paragraph(para);
        }
    }
    b.body.push_str(&sect_pr(doc));

    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let map_err = |e: zip::result::ZipError| std::io::Error::other(e);

    let mut entries: Vec<(String, Vec<u8>)> = vec![
        (
            "[Content_Types].xml".into(),
            content_types_xml(&b).into_bytes(),
        ),
        ("_rels/.rels".into(), ROOT_RELS.into()),
        (
            "word/document.xml".into(),
            document_xml(&b.body).into_bytes(),
        ),
        ("word/styles.xml".into(), styles_xml(doc).into_bytes()),
        (
            "word/_rels/document.xml.rels".into(),
            b.doc_rels_xml().into_bytes(),
        ),
        ("docProps/core.xml".into(), core_xml(doc).into_bytes()),
    ];
    if b.uses_numbering(doc) {
        entries.push(("word/numbering.xml".into(), numbering_xml(doc).into_bytes()));
    }
    if !b.footnotes.is_empty() {
        entries.push(("word/footnotes.xml".into(), b.notes_xml(false).into_bytes()));
    }
    if !b.endnotes.is_empty() {
        entries.push(("word/endnotes.xml".into(), b.notes_xml(true).into_bytes()));
    }
    for (i, img) in b.images.iter().enumerate() {
        entries.push((
            format!("word/media/image{}.{}", i + 1, img.ext),
            img.data.clone(),
        ));
    }
    for (name, data) in entries {
        zip.start_file(name, deflated).map_err(map_err)?;
        zip.write_all(&data)?;
    }
    let cursor = zip.finish().map_err(map_err)?;
    Ok(cursor.into_inner())
}

struct ImageItem {
    data: Vec<u8>,
    ext: &'static str,
    cx_emu: i64,
    cy_emu: i64,
}

/// 외부 하이퍼링크 rel 하나 (id, URL).
struct LinkRel {
    id: String,
    url: String,
}

struct Builder<'d> {
    doc: &'d Document,
    body: String,
    images: Vec<ImageItem>,
    link_rels: Vec<LinkRel>,
    /// (id, 본문 XML) — footnotes.xml/endnotes.xml에 들어갈 노트.
    footnotes: Vec<(u32, String)>,
    endnotes: Vec<(u32, String)>,
    foot_n: u32,
    end_n: u32,
}

impl Builder<'_> {
    fn uses_numbering(&self, doc: &Document) -> bool {
        !doc.header.numbering_levels.is_empty() || !doc.header.bullet_chars.is_empty()
    }

    fn paragraph(&mut self, para: &Paragraph) {
        let mut inline = String::new();
        let mut blocks = String::new();
        self.inline(para, &mut inline, &mut blocks);
        if inline.is_empty() && blocks.is_empty() {
            return;
        }
        if !inline.is_empty() {
            self.body.push_str("<w:p>");
            self.body.push_str(&self.ppr(para));
            self.body.push_str(&inline);
            self.body.push_str("</w:p>");
        }
        self.body.push_str(&blocks);
    }

    /// 문단 속성 (스타일·번호·정렬·간격·들여쓰기).
    fn ppr(&self, para: &Paragraph) -> String {
        let mut ppr = String::from("<w:pPr>");
        // "개요 N" 스타일 → HeadingN.
        if let Some(level) = self
            .doc
            .header
            .styles
            .get(para.style.0 as usize)
            .and_then(|s| s.name.strip_prefix("개요 "))
            .and_then(|n| n.trim().parse::<u8>().ok())
            .filter(|n| (1..=6).contains(n))
        {
            ppr.push_str(&format!("<w:pStyle w:val=\"Heading{level}\"/>"));
        }
        if let Some(ps) = self.doc.header.para_shapes.get(para.para_shape.0 as usize) {
            // 번호/글머리 목록.
            let (ht, level) = (ps.head_type(), ps.head_level());
            if ht == 2 || ht == 3 {
                let num_id =
                    u32::from(ps.numbering_id) + 1 + if ht == 3 { BULLET_NUM_BASE } else { 0 };
                ppr.push_str(&format!(
                    "<w:numPr><w:ilvl w:val=\"{}\"/><w:numId w:val=\"{num_id}\"/></w:numPr>",
                    level.saturating_sub(1)
                ));
            }
            // 정렬.
            let jc = match ps.alignment() {
                1 => "left",
                2 => "right",
                3 => "center",
                4 | 5 => "distribute",
                _ => "both",
            };
            ppr.push_str(&format!("<w:jc w:val=\"{jc}\"/>"));
            // 줄간격 — PERCENT는 lineRule=auto(240=단일), 길이 종류는 exact/atLeast.
            match ps.line_spacing_type {
                1 => ppr.push_str(&format!(
                    "<w:spacing w:line=\"{}\" w:lineRule=\"exact\"/>",
                    ps.line_spacing / 10
                )),
                3 => ppr.push_str(&format!(
                    "<w:spacing w:line=\"{}\" w:lineRule=\"atLeast\"/>",
                    ps.line_spacing / 10
                )),
                _ if ps.line_spacing > 0 && ps.line_spacing != 160 => ppr.push_str(&format!(
                    "<w:spacing w:line=\"{}\" w:lineRule=\"auto\"/>",
                    ps.line_spacing * 240 / 100
                )),
                _ => {}
            }
            // 문단 간격·들여쓰기 — IR은 hwp5 2배 단위라 twips는 값/10.
            let mut ind = String::new();
            if ps.margin_left != 0 {
                ind.push_str(&format!(" w:left=\"{}\"", ps.margin_left / 10));
            }
            if ps.margin_right != 0 {
                ind.push_str(&format!(" w:right=\"{}\"", ps.margin_right / 10));
            }
            if ps.indent > 0 {
                ind.push_str(&format!(" w:firstLine=\"{}\"", ps.indent / 10));
            } else if ps.indent < 0 {
                ind.push_str(&format!(" w:hanging=\"{}\"", -ps.indent / 10));
            }
            if !ind.is_empty() {
                ppr.push_str(&format!("<w:ind{ind}/>"));
            }
            if ps.spacing_top != 0 || ps.spacing_bottom != 0 {
                ppr.push_str(&format!(
                    "<w:spacing w:before=\"{}\" w:after=\"{}\"/>",
                    (ps.spacing_top / 10).max(0),
                    (ps.spacing_bottom / 10).max(0)
                ));
            }
        }
        ppr.push_str("</w:pPr>");
        ppr
    }

    /// 문단의 인라인 내용. 표 등 블록은 `blocks`로 분리한다.
    fn inline(&mut self, para: &Paragraph, out: &mut String, blocks: &mut String) {
        let mut wchar_pos = 0u32;
        let mut current: Option<usize> = None; // 활성 char shape id
        let mut link_open: Option<String> = None; // 활성 hyperlink rel id
        let mut text_buf = String::new(); // 연속 Text — <w:t>로 flush
        macro_rules! flush_text {
            () => {
                if !text_buf.is_empty() {
                    out.push_str("<w:t xml:space=\"preserve\">");
                    out.push_str(&text_buf);
                    out.push_str("</w:t>");
                    text_buf.clear();
                }
            };
        }
        for ch in &para.chars {
            if let HwpChar::Text(_) = ch {
                let want = shape_id_at(self.doc, para, wchar_pos);
                if want != current {
                    flush_text!();
                    close_run(out, &mut current);
                    open_run(out, self.doc, want, &mut current);
                }
            }
            match ch {
                HwpChar::Text(c) => push_escaped(&mut text_buf, *c),
                HwpChar::CharCtrl(code) => {
                    flush_text!();
                    match *code {
                        ctrl_char::LINE_BREAK => out.push_str("<w:br/>"),
                        ctrl_char::HYPHEN => out.push('-'),
                        ctrl_char::NB_SPACE | ctrl_char::FW_SPACE => out.push(' '),
                        _ => {}
                    }
                }
                HwpChar::InlineCtrl { code, .. } => {
                    flush_text!();
                    if *code == ctrl_char::FIELD_END {
                        if let Some(rel) = link_open.take() {
                            close_run(out, &mut current);
                            out.push_str("</w:hyperlink>");
                            let _ = rel;
                        }
                    } else if *code == ctrl_char::TAB {
                        out.push_str("<w:tab/>");
                    }
                }
                HwpChar::ExtCtrl {
                    code, ctrl_index, ..
                } => {
                    flush_text!();
                    if let Some(idx) = ctrl_index
                        && let Some(control) = para.controls.get(*idx as usize)
                    {
                        if *code == ctrl_char::FIELD_START
                            && let Some(url) = crate::field::hyperlink_url(control)
                        {
                            close_run(out, &mut current);
                            let rel_id = format!("rIdLink{}", self.link_rels.len() + 1);
                            self.link_rels.push(LinkRel {
                                id: rel_id.clone(),
                                url,
                            });
                            out.push_str(&format!(
                                "<w:hyperlink r:id=\"{rel_id}\" w:history=\"1\">"
                            ));
                            link_open = Some(rel_id);
                            current = None; // 링크 run을 새로 연다
                        } else {
                            close_run(out, &mut current);
                            self.control(control, *code, out, blocks);
                        }
                    }
                }
            }
            wchar_pos += ch.wchar_width();
        }
        flush_text!();
        close_run(out, &mut current);
        if link_open.is_some() {
            out.push_str("</w:hyperlink>");
        }
    }

    fn control(&mut self, control: &Control, code: u16, out: &mut String, blocks: &mut String) {
        match control {
            Control::SectionDef(_) => {}
            Control::Picture(pic) => {
                if let Some(data) = self.doc.resolve_bin(&pic.bin_ref) {
                    let idx = self.images.len();
                    let (ext, _) = crate::image::image_kind(data);
                    self.images.push(ImageItem {
                        data: data.to_vec(),
                        ext,
                        cx_emu: i64::from(pic.width.0) * 127,
                        cy_emu: i64::from(pic.height.0) * 127,
                    });
                    let rel = format!("rIdImg{}", idx + 1);
                    let (cx, cy) = (self.images[idx].cx_emu, self.images[idx].cy_emu);
                    out.push_str(&format!(
                        "<w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
                         <wp:extent cx=\"{cx}\" cy=\"{cy}\"/>\
                         <wp:docPr id=\"{}\" name=\"image{}\"/>\
                         <a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
                         <pic:pic><pic:nvPicPr><pic:cNvPr id=\"0\" name=\"image{idx}\"/>\
                         <pic:cNvPicPr/></pic:nvPicPr><pic:blipFill>\
                         <a:blip r:embed=\"{rel}\"/>\
                         <a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
                         <pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
                         <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>\
                         </a:graphicData></a:graphic></wp:inline></w:drawing></w:r>",
                        idx + 100,
                        idx + 1,
                    ));
                }
            }
            Control::Table(table) => self.table(table, blocks),
            Control::Generic(g) => {
                // 각주/미주 → footnotes.xml/endnotes.xml + 참조 run.
                if code == ctrl_char::FOOTNOTE_ENDNOTE && matches!(&g.ctrl_id, b"fn  " | b"en  ") {
                    let mut note_body = String::new();
                    for list in &g.paragraph_lists {
                        for p in &list.paragraphs {
                            let mut inl = String::new();
                            let mut blk = String::new();
                            self.inline(p, &mut inl, &mut blk);
                            let inl = inl.trim();
                            if !inl.is_empty() {
                                note_body.push_str(&format!(
                                    "<w:p><w:pPr><w:pStyle w:val=\"FootnoteText\"/></w:pPr>{inl}</w:p>"
                                ));
                            }
                        }
                    }
                    let endnote = g.ctrl_id == *b"en  ";
                    let id = if endnote {
                        self.end_n += 1;
                        self.end_n + 1 // 0/1은 separator 예약
                    } else {
                        self.foot_n += 1;
                        self.foot_n + 1
                    };
                    if endnote {
                        self.endnotes.push((id, note_body));
                        out.push_str(&format!(
                            "<w:r><w:rPr><w:rStyle w:val=\"FootnoteReference\"/></w:rPr><w:endnoteReference w:id=\"{id}\"/></w:r>"
                        ));
                    } else {
                        self.footnotes.push((id, note_body));
                        out.push_str(&format!(
                            "<w:r><w:rPr><w:rStyle w:val=\"FootnoteReference\"/></w:rPr><w:footnoteReference w:id=\"{id}\"/></w:r>"
                        ));
                    }
                    return;
                }
                // 수식 → 스크립트 원문 그대로 (v1 폴리백).
                if let Some(eq) = &g.equation {
                    for c in eq.script.chars() {
                        push_escaped(out, c);
                    }
                    return;
                }
                if code == ctrl_char::HEADER_FOOTER || code == ctrl_char::HIDDEN_COMMENT {
                    return;
                }
                for list in &g.paragraph_lists {
                    for p in &list.paragraphs {
                        self.paragraph_in_block(p, out, blocks);
                    }
                }
            }
        }
    }

    /// 글상자 등의 난이 문단 — 블록 컨텍스트에서 문단을 그린다.
    fn paragraph_in_block(&mut self, para: &Paragraph, out: &mut String, blocks: &mut String) {
        let mut inl = String::new();
        self.inline(para, &mut inl, blocks);
        let inl = inl.trim();
        if !inl.is_empty() {
            if !out.is_empty() && !out.ends_with([' ', '>']) {
                out.push(' ');
            }
            out.push_str(inl);
        }
    }

    /// 표 — gridSpan(colspan)·vMerge(rowspan). 셀 안 중첩 표 보존.
    fn table(&mut self, table: &hwp_model::Table, blocks: &mut String) {
        let rows = table.rows.max(1) as usize;
        let cols = table.cols.max(1) as usize;
        let mut covered = vec![vec![false; cols]; rows];
        let col_twips = crate::from_markdown::BODY_WIDTH / cols as i32 / 5;
        blocks.push_str(
            "<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/>\
             <w:tblBorders><w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"000000\"/>\
             <w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"000000\"/>\
             <w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"000000\"/>\
             <w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"000000\"/>\
             <w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"000000\"/>\
             <w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"000000\"/>\
             </w:tblBorders><w:tblLayout w:type=\"fixed\"/></w:tblPr><w:tblGrid>",
        );
        for _ in 0..cols {
            blocks.push_str(&format!("<w:gridCol w:w=\"{col_twips}\"/>"));
        }
        blocks.push_str("</w:tblGrid>");
        for r in 0..rows {
            blocks.push_str("<w:tr>");
            for c in 0..cols {
                if covered[r][c] {
                    blocks.push_str(&format!(
                        "<w:tc><w:tcPr><w:tcW w:w=\"{col_twips}\" w:type=\"dxa\"/><w:vMerge/></w:tcPr><w:p/></w:tc>"
                    ));
                    continue;
                }
                let Some(cell) = table
                    .cells
                    .iter()
                    .find(|cell| cell.row as usize == r && cell.col as usize == c)
                else {
                    blocks.push_str(&format!(
                        "<w:tc><w:tcPr><w:tcW w:w=\"{col_twips}\" w:type=\"dxa\"/></w:tcPr><w:p/></w:tc>"
                    ));
                    continue;
                };
                for dr in 0..cell.row_span.max(1) as usize {
                    for dc in 0..cell.col_span.max(1) as usize {
                        if let Some(slot) =
                            covered.get_mut(r + dr).and_then(|row| row.get_mut(c + dc))
                        {
                            *slot = true;
                        }
                    }
                }
                let mut tcpr = format!("<w:tcW w:w=\"{col_twips}\" w:type=\"dxa\"/>");
                if cell.col_span > 1 {
                    tcpr.push_str(&format!("<w:gridSpan w:val=\"{}\"/>", cell.col_span));
                }
                if cell.row_span > 1 {
                    tcpr.push_str("<w:vMerge w:val=\"restart\"/>");
                }
                blocks.push_str(&format!("<w:tc><w:tcPr>{tcpr}</w:tcPr>"));
                let mut wrote_p = false;
                for p in &cell.paragraphs {
                    let mut inl = String::new();
                    let mut blk = String::new();
                    self.inline(p, &mut inl, &mut blk);
                    if !inl.trim().is_empty() {
                        blocks.push_str("<w:p>");
                        blocks.push_str(&self.ppr(p));
                        blocks.push_str(&inl);
                        blocks.push_str("</w:p>");
                        wrote_p = true;
                    }
                    blocks.push_str(&blk);
                    if !blk.is_empty() {
                        wrote_p = true;
                    }
                }
                if !wrote_p {
                    blocks.push_str("<w:p/>");
                }
                blocks.push_str("</w:tc>");
            }
            blocks.push_str("</w:tr>");
        }
        blocks.push_str("</w:tbl>");
    }

    /// document.xml.rels — 스타일/번호/노트/그림/하이퍼링크 관계.
    fn doc_rels_xml(&self) -> String {
        let mut rels = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
        );
        rels.push_str(
            "<Relationship Id=\"rIdStyles\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>",
        );
        if self.uses_numbering(self.doc) {
            rels.push_str(
                "<Relationship Id=\"rIdNumbering\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering\" Target=\"numbering.xml\"/>",
            );
        }
        if !self.footnotes.is_empty() {
            rels.push_str(
                "<Relationship Id=\"rIdFootnotes\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes\" Target=\"footnotes.xml\"/>",
            );
        }
        if !self.endnotes.is_empty() {
            rels.push_str(
                "<Relationship Id=\"rIdEndnotes\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/endnotes\" Target=\"endnotes.xml\"/>",
            );
        }
        for (i, _) in self.images.iter().enumerate() {
            rels.push_str(&format!(
                "<Relationship Id=\"rIdImg{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/image{}.{}\"/>",
                i + 1,
                i + 1,
                self.images[i].ext
            ));
        }
        for link in &self.link_rels {
            rels.push_str(&format!(
                "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"{}\" TargetMode=\"External\"/>",
                link.id,
                escape(&link.url)
            ));
        }
        rels.push_str("</Relationships>");
        rels
    }

    /// footnotes.xml/endnotes.xml — id 0/1은 separator 예약.
    fn notes_xml(&self, endnote: bool) -> String {
        let (root, item, reference) = if endnote {
            ("endnotes", "endnote", "endnoteRef")
        } else {
            ("footnotes", "footnote", "footnoteRef")
        };
        let notes = if endnote {
            &self.endnotes
        } else {
            &self.footnotes
        };
        let mut out = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <w:{root} {NS_DECL}>\
             <w:{item} w:type=\"separator\" w:id=\"0\"><w:p><w:r><w:separator/></w:r></w:p></w:{item}>\
             <w:{item} w:type=\"continuationSeparator\" w:id=\"1\"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:{item}>"
        );
        for (id, body) in notes {
            out.push_str(&format!(
                "<w:{item} w:id=\"{id}\"><w:p><w:pPr><w:pStyle w:val=\"FootnoteText\"/></w:pPr>\
                 <w:r><w:rPr><w:rStyle w:val=\"FootnoteReference\"/></w:rPr><w:{reference}/></w:r></w:p>{body}</w:{item}>"
            ));
        }
        out.push_str(&format!("</w:{root}>"));
        out
    }
}

/// 번호 정의의 numId 베이스 — bullet 정의는 번호 정의 뒤에 붙는다.
const BULLET_NUM_BASE: u32 = 128;

fn shape_id_at(doc: &Document, para: &Paragraph, pos: u32) -> Option<usize> {
    let id = para
        .char_shape_runs
        .iter()
        .rev()
        .find(|(start, _)| *start <= pos)
        .map(|(_, id)| *id)?;
    (doc.header.char_shapes.get(id.0 as usize).is_some()).then_some(id.0 as usize)
}

/// run을 연다 — 현재 모양의 rPr을 쓴다. `current`는 활성 shape id 추적.
fn open_run(out: &mut String, doc: &Document, want: Option<usize>, current: &mut Option<usize>) {
    let rpr = want
        .and_then(|id| doc.header.char_shapes.get(id))
        .map(|s| run_props(doc, s))
        .unwrap_or_default();
    out.push_str("<w:r>");
    out.push_str(&rpr);
    *current = want;
}

fn close_run(out: &mut String, current: &mut Option<usize>) {
    if current.is_some() {
        out.push_str("</w:r>");
        *current = None;
    }
}

/// CharShape → w:rPr.
fn run_props(doc: &Document, s: &CharShape) -> String {
    let mut r = String::from("<w:rPr>");
    if let Some(face) = doc.header.fonts[0].get(s.face_ids[0] as usize) {
        r.push_str(&format!(
            "<w:rFonts w:ascii=\"{0}\" w:hAnsi=\"{0}\" w:eastAsia=\"{0}\"/>",
            escape(&face.name)
        ));
    }
    if s.is_bold() {
        r.push_str("<w:b/><w:bCs/>");
    }
    if s.is_italic() {
        r.push_str("<w:i/><w:iCs/>");
    }
    if s.has_underline() {
        r.push_str("<w:u w:val=\"single\"/>");
    }
    if s.has_strike() {
        r.push_str("<w:strike/>");
    }
    if s.is_superscript() {
        r.push_str("<w:vertAlign w:val=\"superscript\"/>");
    }
    if s.is_subscript() {
        r.push_str("<w:vertAlign w:val=\"subscript\"/>");
    }
    // 크기: base_size(1/100pt) × rel_sizes(%) → half-points.
    let hp = (i64::from(s.base_size) * i64::from(s.rel_sizes[0]) / 50).max(2);
    r.push_str(&format!("<w:sz w:val=\"{hp}\"/><w:szCs w:val=\"{hp}\"/>"));
    // 자간 % → 현재 크기 기준 pt의 twips.
    if s.spacings[0] != 0 {
        let twips =
            i64::from(s.spacings[0]) * i64::from(s.base_size) * i64::from(s.rel_sizes[0]) / 50000;
        if twips != 0 {
            r.push_str(&format!("<w:spacing w:val=\"{twips}\"/>"));
        }
    }
    if s.text_color != 0 {
        r.push_str(&format!(
            "<w:color w:val=\"{}\"/>",
            colorref_hex(s.text_color)
        ));
    }
    if s.has_shade() {
        r.push_str(&format!(
            "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{}\"/>",
            colorref_hex(s.shade_color)
        ));
    }
    r.push_str("</w:rPr>");
    r
}

/// COLORREF(0x00BBGGRR) → RRGGBB (docx는 # 없는 6자리).
fn colorref_hex(v: u32) -> String {
    format!(
        "{:02X}{:02X}{:02X}",
        v & 0xFF,
        (v >> 8) & 0xFF,
        (v >> 16) & 0xFF
    )
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

const NS_DECL: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" \
xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"";

fn document_xml(body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:document {NS_DECL}><w:body>{body}</w:body></w:document>"
    )
}

/// 마지막 구역 정의의 페이지 설정 → sectPr (twips).
fn sect_pr(doc: &Document) -> String {
    let page = doc
        .sections
        .iter()
        .flat_map(|s| &s.paragraphs)
        .flat_map(|p| &p.controls)
        .find_map(|c| match c {
            Control::SectionDef(sd) => sd.page.as_ref(),
            _ => None,
        });
    let Some(page) = page else {
        return "<w:sectPr/>".into();
    };
    // PageDef는 HWPUNIT(2배 아님) — twips는 ×0.2.
    let tw = |v: i32| (i64::from(v) / 5).max(0);
    let orient = if page.attr & 1 != 0 {
        " w:orient=\"landscape\""
    } else {
        ""
    };
    format!(
        "<w:sectPr><w:pgSz w:w=\"{}\" w:h=\"{}\"{orient}/>\
         <w:pgMar w:top=\"{}\" w:right=\"{}\" w:bottom=\"{}\" w:left=\"{}\" w:header=\"{}\" w:footer=\"{}\" w:gutter=\"{}\"/></w:sectPr>",
        tw(page.width.0),
        tw(page.height.0),
        tw(page.margin_top.0),
        tw(page.margin_right.0),
        tw(page.margin_bottom.0),
        tw(page.margin_left.0),
        tw(page.margin_header.0),
        tw(page.margin_footer.0),
        tw(page.gutter.0),
    )
}

fn styles_xml(doc: &Document) -> String {
    let body_font = doc.header.fonts[0]
        .first()
        .map(|f| f.name.clone())
        .unwrap_or_else(|| "함초롬바탕".to_string());
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:styles {NS_DECL}>\
         <w:docDefaults><w:rPrDefault><w:rPr>\
         <w:rFonts w:ascii=\"{0}\" w:hAnsi=\"{0}\" w:eastAsia=\"{0}\"/><w:sz w:val=\"20\"/>\
         </w:rPr></w:rPrDefault></w:docDefaults>\
         <w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/></w:style>",
        escape(&body_font)
    );
    let sizes = [36, 30, 26, 24, 22, 22]; // half-points (18/15/13/12/11/11pt)
    for (i, sz) in sizes.iter().enumerate() {
        let n = i + 1;
        out.push_str(&format!(
            "<w:style w:type=\"paragraph\" w:styleId=\"Heading{n}\">\
             <w:name w:val=\"heading {n}\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/>\
             <w:pPr><w:keepNext/><w:outlineLvl w:val=\"{i}\"/></w:pPr>\
             <w:rPr><w:b/><w:bCs/><w:sz w:val=\"{sz}\"/><w:szCs w:val=\"{sz}\"/></w:rPr></w:style>"
        ));
    }
    out.push_str(
        "<w:style w:type=\"paragraph\" w:styleId=\"FootnoteText\">\
         <w:name w:val=\"footnote text\"/><w:basedOn w:val=\"Normal\"/>\
         <w:rPr><w:sz w:val=\"18\"/></w:rPr></w:style>\
         <w:style w:type=\"character\" w:styleId=\"FootnoteReference\">\
         <w:name w:val=\"footnote reference\"/><w:rPr><w:vertAlign w:val=\"superscript\"/></w:rPr></w:style>",
    );
    out.push_str("</w:styles>");
    out
}

/// numbering.xml — 번호 정의(0-기반 인덱스) + bullet 정의(BULLET_NUM_BASE~).
fn numbering_xml(doc: &Document) -> String {
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:numbering {NS_DECL}>"
    );
    for (i, levels) in doc.header.numbering_levels.iter().enumerate() {
        out.push_str(&format!("<w:abstractNum w:abstractNumId=\"{i}\">"));
        for (ilvl, level) in levels.iter().enumerate() {
            let (fmt, text) = num_fmt(level);
            out.push_str(&format!(
                "<w:lvl w:ilvl=\"{ilvl}\"><w:start w:val=\"{}\"/><w:numFmt w:val=\"{fmt}\"/>\
                 <w:lvlText w:val=\"{text}\"/><w:lvlJc w:val=\"left\"/></w:lvl>",
                level.start
            ));
        }
        out.push_str("</w:abstractNum>");
    }
    for (i, ch) in doc.header.bullet_chars.iter().enumerate() {
        let id = BULLET_NUM_BASE as usize + i;
        out.push_str(&format!(
            "<w:abstractNum w:abstractNumId=\"{id}\"><w:lvl w:ilvl=\"0\">\
             <w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/>\
             <w:lvlText w:val=\"{ch}\"/><w:lvlJc w:val=\"left\"/>\
             <w:rPr><w:rFonts w:ascii=\"Symbol\" w:hAnsi=\"Symbol\" w:hint=\"default\"/></w:rPr></w:lvl></w:abstractNum>"
        ));
    }
    for (i, _) in doc.header.numbering_levels.iter().enumerate() {
        out.push_str(&format!(
            "<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"{i}\"/></w:num>",
            i + 1
        ));
    }
    for (i, _) in doc.header.bullet_chars.iter().enumerate() {
        let id = BULLET_NUM_BASE as usize + i;
        out.push_str(&format!(
            "<w:num w:numId=\"{id}\"><w:abstractNumId w:val=\"{id}\"/></w:num>"
        ));
    }
    out.push_str("</w:numbering>");
    out
}

/// NumLevel → OOXML numFmt + lvlText (`^N` → `%N`).
fn num_fmt(level: &hwp_model::NumLevel) -> (&'static str, String) {
    let fmt = match level.fmt {
        NumFmt::Digit => "decimal",
        NumFmt::HangulSyllable => "koreanCounting",
        NumFmt::HangulJamo => "koreanDigital",
        NumFmt::CircledDigit => "decimalEnclosedCircle",
        NumFmt::LatinUpper => "upperLetter",
        NumFmt::LatinLower => "lowerLetter",
        NumFmt::RomanUpper => "upperRoman",
        NumFmt::RomanLower => "lowerRoman",
    };
    let mut text = if level.template.is_empty() {
        "%1.".to_string()
    } else {
        level.template.clone()
    };
    // `^N` 자리표시를 OOXML `%N`으로 — 템플릿의 ^1..^7을 순서대로 치환.
    for n in 1..=7u8 {
        text = text.replace(&format!("^{n}"), &format!("%{n}"));
    }
    (fmt, escape(&text))
}

fn content_types_xml(b: &Builder) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Default Extension=\"png\" ContentType=\"image/png\"/>\
         <Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>\
         <Default Extension=\"jpg\" ContentType=\"image/jpeg\"/>\
         <Default Extension=\"gif\" ContentType=\"image/gif\"/>\
         <Default Extension=\"bmp\" ContentType=\"image/bmp\"/>\
         <Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
         <Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
         <Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>",
    );
    if b.uses_numbering(b.doc) {
        out.push_str(
            "<Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/>",
        );
    }
    if !b.footnotes.is_empty() {
        out.push_str(
            "<Override PartName=\"/word/footnotes.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml\"/>",
        );
    }
    if !b.endnotes.is_empty() {
        out.push_str(
            "<Override PartName=\"/word/endnotes.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml\"/>",
        );
    }
    out.push_str("</Types>");
    out
}

const ROOT_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/>\
</Relationships>";

fn core_xml(doc: &Document) -> String {
    let m = &doc.metadata;
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" \
         xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">",
    );
    if let Some(t) = m.title.as_deref().filter(|s| !s.is_empty()) {
        out.push_str(&format!("<dc:title>{}</dc:title>", escape(t)));
    }
    if let Some(a) = m.author.as_deref().filter(|s| !s.is_empty()) {
        out.push_str(&format!("<dc:creator>{}</dc:creator>", escape(a)));
    }
    if let Some(s) = m.subject.as_deref().filter(|s| !s.is_empty()) {
        out.push_str(&format!("<dc:subject>{}</dc:subject>", escape(s)));
    }
    if let Some(k) = m.keywords.as_deref().filter(|s| !s.is_empty()) {
        out.push_str(&format!("<cp:keywords>{}</cp:keywords>", escape(k)));
    }
    out.push_str("</cp:coreProperties>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    fn unzip(bytes: &[u8], name: &str) -> Option<String> {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
        let mut f = zip.by_name(name).ok()?;
        let mut s = String::new();
        f.read_to_string(&mut s).ok()?;
        Some(s)
    }

    fn all_parts_well_formed(bytes: &[u8]) -> bool {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).unwrap();
            if !f.name().ends_with(".xml") && !f.name().ends_with(".rels") {
                continue;
            }
            let mut s = String::new();
            if f.read_to_string(&mut s).is_err() {
                return false;
            }
            let mut reader = quick_xml::Reader::from_str(&s);
            loop {
                match reader.read_event() {
                    Ok(quick_xml::events::Event::Eof) => break,
                    Err(_) => return false,
                    _ => {}
                }
            }
        }
        true
    }

    #[test]
    fn 문서_구조와_본문() {
        let doc = crate::from_markdown::from_markdown(
            "# 제목\n\n본문 **굵게** 문단입니다.\n\n| 가 | 나 |\n|---|---|\n| 1 | 2 |\n",
        );
        let bytes = to_docx(&doc).unwrap();
        assert!(all_parts_well_formed(&bytes), "모든 XML well-formed");
        let document = unzip(&bytes, "word/document.xml").unwrap();
        assert!(
            document.contains("<w:pStyle w:val=\"Heading1\"/>"),
            "제목: {document}"
        );
        assert!(document.contains("<w:b/>"), "굵게: {document}");
        assert!(document.contains("본문"), "본문: {document}");
        assert!(document.contains("<w:tbl>"), "표: {document}");
        assert!(
            unzip(&bytes, "word/styles.xml")
                .unwrap()
                .contains("Heading1")
        );
        assert!(unzip(&bytes, "[Content_Types].xml").is_some());
        assert!(unzip(&bytes, "_rels/.rels").is_some());
        assert!(unzip(&bytes, "word/_rels/document.xml.rels").is_some());
    }

    #[test]
    fn 병합셀_span과_vmerge() {
        let mut doc = crate::from_markdown::from_markdown("표\n");
        {
            let para = &mut doc.sections[0].paragraphs[0];
            // 직접 병합 셀 구성 — GFM은 병합을 못 만든다.
            use hwp_model::{BorderFillId, Cell, HwpUnit, Table};
            let cell = |row, col, cs, rs, text: &str| Cell {
                list_attr: 0,
                col,
                row,
                col_span: cs,
                row_span: rs,
                width: HwpUnit(0),
                height: HwpUnit(0),
                margins: [0; 4],
                border_fill: BorderFillId(0),
                header_tail: vec![],
                paragraphs: vec![Paragraph {
                    chars: text.chars().map(HwpChar::Text).collect(),
                    char_shape_runs: vec![(0, hwp_model::CharShapeId(0))],
                    ..Paragraph::default()
                }],
            };
            let table = Table {
                common_data: vec![],
                placement: None,
                attr: 0,
                rows: 2,
                cols: 3,
                cell_spacing: 0,
                inner_margins: [0; 4],
                row_cell_counts: vec![2, 2],
                border_fill: BorderFillId(0),
                table_tail: vec![],
                cells: vec![
                    cell(0, 0, 2, 1, "가로"),
                    cell(0, 2, 1, 2, "세로"),
                    cell(1, 0, 1, 1, "a"),
                    cell(1, 1, 1, 1, "b"),
                ],
                extras: vec![],
            };
            let idx = para.controls.len() as u32;
            para.controls.push(Control::Table(table));
            para.chars.push(HwpChar::ExtCtrl {
                code: ctrl_char::OBJECT,
                ctrl_id: *b"tbl ",
                payload: vec![],
                ctrl_index: Some(idx),
            });
        }
        let bytes = to_docx(&doc).unwrap();
        let document = unzip(&bytes, "word/document.xml").unwrap();
        assert!(
            document.contains("<w:gridSpan w:val=\"2\"/>"),
            "colspan: {document}"
        );
        assert!(
            document.contains("<w:vMerge w:val=\"restart\"/>"),
            "rowspan: {document}"
        );
        assert!(document.contains("<w:vMerge/>"), "덮인 칸: {document}");
    }

    #[test]
    fn 각주와_하이퍼링크와_목록() {
        let doc = crate::from_markdown::from_markdown(
            "본문[^1] [링크](https://example.com)\n\n1. 첫째\n2. 둘째\n\n[^1]: 각주 내용\n",
        );
        let bytes = to_docx(&doc).unwrap();
        let document = unzip(&bytes, "word/document.xml").unwrap();
        assert!(
            document.contains("<w:footnoteReference w:id=\"2\"/>"),
            "각주 참조"
        );
        assert!(
            document.contains("<w:hyperlink r:id=\"rIdLink1\""),
            "링크: {document}"
        );
        assert!(document.contains("<w:numPr>"), "목록: {document}");
        let footnotes = unzip(&bytes, "word/footnotes.xml").unwrap();
        assert!(footnotes.contains("각주 내용"), "각주 본문: {footnotes}");
        assert!(footnotes.contains("w:type=\"separator\""), "separator");
        let rels = unzip(&bytes, "word/_rels/document.xml.rels").unwrap();
        assert!(
            rels.contains("Target=\"https://example.com\" TargetMode=\"External\""),
            "rels: {rels}"
        );
        assert!(
            unzip(&bytes, "word/numbering.xml")
                .unwrap()
                .contains("decimal")
        );
    }

    #[test]
    fn 이미지_임베드와_extent() {
        let mut doc = crate::from_markdown::from_markdown("그림: 여기\n");
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend(96u32.to_be_bytes());
        png.extend(96u32.to_be_bytes());
        png.extend([0u8; 8]);
        let p = std::env::temp_dir().join(format!("docx_img_{}.png", std::process::id()));
        std::fs::write(&p, &png).unwrap();
        crate::image::insert_image(&mut doc, "여기", &p, crate::image::ImageSize::Natural).unwrap();
        let bytes = to_docx(&doc).unwrap();
        let document = unzip(&bytes, "word/document.xml").unwrap();
        assert!(document.contains("<w:drawing>"), "드로잉: {document}");
        assert!(
            document.contains("<a:blip r:embed=\"rIdImg1\"/>"),
            "blip: {document}"
        );
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        assert!(zip.by_name("word/media/image1.png").is_ok(), "media 엔트리");
        let _ = std::fs::remove_file(&p);
    }
}
