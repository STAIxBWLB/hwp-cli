//! IR → ODT (OpenDocument Text) 단방향 내보내기.
//!
//! 단락/개요(제목)/표/이미지/굵게·기울임·밑줄·취소선 + 문서 메타데이터를 옮긴다.
//! **내용·구조 충실도**이며 페이지 레이아웃(여백·단·머리말 위치)은 재현하지 않는다.
//! 패키징은 hwpx 작성기와 같은 mimetype-우선 STORED 규칙을 따른다.

use std::io::Write as _;

use hwp_model::{CharShape, Control, Document, HwpChar, Paragraph, ctrl_char};

/// 문서를 ODT 바이트(zip)로 직렬화한다.
pub fn to_odt(doc: &Document) -> std::io::Result<Vec<u8>> {
    let mut b = Builder {
        doc,
        body: String::new(),
        images: Vec::new(),
        footnote_n: 0,
    };
    for section in &doc.sections {
        for para in &section.paragraphs {
            b.paragraph(para);
        }
    }
    let content = content_xml(&b.body);
    let meta = meta_xml(doc);
    let manifest = manifest_xml(&b.images);

    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let map_err = |e: zip::result::ZipError| std::io::Error::other(e);

    // mimetype은 반드시 첫 엔트리 + 무압축.
    zip.start_file("mimetype", stored).map_err(map_err)?;
    zip.write_all(b"application/vnd.oasis.opendocument.text")?;

    for (name, data) in [
        ("content.xml", content.as_bytes()),
        ("styles.xml", STYLES_XML.as_bytes()),
        ("meta.xml", meta.as_bytes()),
        ("META-INF/manifest.xml", manifest.as_bytes()),
    ] {
        zip.start_file(name, deflated).map_err(map_err)?;
        zip.write_all(data)?;
    }
    for (i, img) in b.images.iter().enumerate() {
        zip.start_file(format!("Pictures/image{i}.{}", img.ext), deflated)
            .map_err(map_err)?;
        zip.write_all(&img.data)?;
    }
    let cursor = zip.finish().map_err(map_err)?;
    Ok(cursor.into_inner())
}

struct ImageItem {
    data: Vec<u8>,
    ext: &'static str,
    mime: &'static str,
    /// cm 단위 표시 크기 (0이면 생략).
    w_cm: f64,
    h_cm: f64,
}

struct Builder<'d> {
    doc: &'d Document,
    body: String,
    images: Vec<ImageItem>,
    footnote_n: u32,
}

impl Builder<'_> {
    fn paragraph(&mut self, para: &Paragraph) {
        let heading = self
            .doc
            .header
            .styles
            .get(para.style.0 as usize)
            .and_then(|s| s.name.strip_prefix("개요 "))
            .and_then(|n| n.trim().parse::<usize>().ok())
            .filter(|n| (1..=6).contains(n));

        let mut inline = String::new();
        let mut blocks = String::new();
        self.inline(para, &mut inline, &mut blocks);
        let inline = inline.trim_end();
        if inline.is_empty() && blocks.is_empty() {
            return;
        }
        if !inline.is_empty() {
            match heading {
                Some(level) => {
                    self.body.push_str(&format!(
                        "<text:h text:style-name=\"H{level}\" text:outline-level=\"{level}\">{inline}</text:h>"
                    ));
                }
                None => self.body.push_str(&format!(
                    "<text:p text:style-name=\"Body\">{inline}</text:p>"
                )),
            }
        }
        self.body.push_str(&blocks);
    }

    /// 문단의 인라인 내용을 ODF로 만든다. 표 등 블록은 `blocks`로 분리.
    fn inline(&mut self, para: &Paragraph, out: &mut String, blocks: &mut String) {
        let mut wchar_pos = 0u32;
        let mut style = Style::default();
        for ch in &para.chars {
            if let HwpChar::Text(_) = ch {
                let want = shape_at(self.doc, para, wchar_pos)
                    .map(Style::from_shape)
                    .unwrap_or_default();
                if want != style {
                    close_spans(out, &mut style);
                    open_spans(out, want);
                    style = want;
                }
            }
            match ch {
                HwpChar::Text(c) => push_escaped(out, *c),
                HwpChar::CharCtrl(code) => match *code {
                    ctrl_char::LINE_BREAK => {
                        close_spans(out, &mut style);
                        out.push_str("<text:line-break/>");
                    }
                    ctrl_char::HYPHEN => out.push('-'),
                    ctrl_char::NB_SPACE | ctrl_char::FW_SPACE => out.push(' '),
                    _ => {}
                },
                HwpChar::InlineCtrl { code, .. } => {
                    if *code == ctrl_char::TAB {
                        out.push_str("<text:tab/>");
                    }
                }
                HwpChar::ExtCtrl {
                    code, ctrl_index, ..
                } => {
                    if let Some(idx) = ctrl_index
                        && let Some(control) = para.controls.get(*idx as usize)
                    {
                        close_spans(out, &mut style);
                        self.control(control, *code, out, blocks);
                    }
                }
            }
            wchar_pos += ch.wchar_width();
        }
        close_spans(out, &mut style);
    }

    fn control(&mut self, control: &Control, code: u16, out: &mut String, blocks: &mut String) {
        match control {
            Control::SectionDef(_) => {}
            Control::Picture(pic) => {
                if let Some(data) = self.doc.resolve_bin(&pic.bin_ref) {
                    let (ext, mime) = image_kind(data);
                    let idx = self.images.len();
                    self.images.push(ImageItem {
                        data: data.to_vec(),
                        ext,
                        mime,
                        w_cm: pic.width.to_mm() / 10.0,
                        h_cm: pic.height.to_mm() / 10.0,
                    });
                    let size = if self.images[idx].w_cm > 0.0 && self.images[idx].h_cm > 0.0 {
                        format!(
                            " svg:width=\"{:.3}cm\" svg:height=\"{:.3}cm\"",
                            self.images[idx].w_cm, self.images[idx].h_cm
                        )
                    } else {
                        String::new()
                    };
                    out.push_str(&format!(
                        "<draw:frame draw:style-name=\"Img\" text:anchor-type=\"as-char\"{size}>\
                         <draw:image xlink:href=\"Pictures/image{idx}.{ext}\" xlink:type=\"simple\" \
                         xlink:show=\"embed\" xlink:actuate=\"onLoad\"/></draw:frame>"
                    ));
                }
            }
            Control::Table(table) => self.table(table, blocks),
            Control::Generic(g) => {
                // 각주/미주 → text:note (GH-3) — 본문 마커는 note-citation이 그린다.
                if code == ctrl_char::FOOTNOTE_ENDNOTE && matches!(&g.ctrl_id, b"fn  " | b"en  ") {
                    self.footnote_n += 1;
                    let n = self.footnote_n;
                    let class = if g.ctrl_id == *b"fn  " {
                        "footnote"
                    } else {
                        "endnote"
                    };
                    let mut note_body = String::new();
                    for list in &g.paragraph_lists {
                        for p in &list.paragraphs {
                            let mut inl = String::new();
                            let mut blk = String::new();
                            self.inline(p, &mut inl, &mut blk);
                            let inl = inl.trim();
                            if !inl.is_empty() {
                                note_body.push_str(&format!(
                                    "<text:p text:style-name=\"Body\">{inl}</text:p>"
                                ));
                            }
                        }
                    }
                    out.push_str(&format!(
                        "<text:note text:id=\"ftn{n}\" text:note-class=\"{class}\">\
                         <text:note-citation>{n}</text:note-citation>\
                         <text:note-body>{note_body}</text:note-body></text:note>"
                    ));
                    return;
                }
                if code == ctrl_char::HEADER_FOOTER || code == ctrl_char::HIDDEN_COMMENT {
                    return;
                }
                // 글상자 등: 내부 문단 텍스트를 인라인으로 흡수.
                for list in &g.paragraph_lists {
                    for p in &list.paragraphs {
                        let mut sub = String::new();
                        self.inline(p, &mut sub, blocks);
                        let sub = sub.trim();
                        if !sub.is_empty() {
                            if !out.is_empty() && !out.ends_with([' ', '>']) {
                                out.push(' ');
                            }
                            out.push_str(sub);
                        }
                    }
                }
            }
        }
    }

    /// 표 — 병합 셀이 덮는 칸은 `covered-table-cell`, origin 셀은 number-*-spanned(GH-4).
    /// 셀 내용은 문단 인라인 + 블록(중첩 표·그림)을 보존한다(GH-5 — 이전엔 블록 버퍼 폐기).
    fn table(&mut self, table: &hwp_model::Table, blocks: &mut String) {
        let rows = table.rows.max(1) as usize;
        let cols = table.cols.max(1) as usize;
        // 병합 셀이 덮는 칸 표시 격자.
        let mut covered = vec![vec![false; cols]; rows];
        blocks.push_str("<table:table table:style-name=\"Tbl\">");
        blocks.push_str(&format!(
            "<table:table-column table:number-columns-repeated=\"{cols}\"/>"
        ));
        for r in 0..rows {
            blocks.push_str("<table:table-row>");
            for c in 0..cols {
                if covered[r][c] {
                    blocks.push_str("<table:covered-table-cell/>");
                    continue;
                }
                let Some(cell) = table
                    .cells
                    .iter()
                    .find(|cell| cell.row as usize == r && cell.col as usize == c)
                else {
                    blocks.push_str(
                        "<table:table-cell office:value-type=\"string\">\
                         <text:p text:style-name=\"Body\"/></table:table-cell>",
                    );
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
                let mut span_attrs = String::new();
                if cell.col_span > 1 {
                    span_attrs.push_str(&format!(
                        " table:number-columns-spanned=\"{}\"",
                        cell.col_span
                    ));
                }
                if cell.row_span > 1 {
                    span_attrs
                        .push_str(&format!(" table:number-rows-spanned=\"{}\"", cell.row_span));
                }
                let mut content = String::new();
                for p in &cell.paragraphs {
                    let mut inl = String::new();
                    let mut blk = String::new();
                    self.inline(p, &mut inl, &mut blk);
                    let inl = inl.trim();
                    if !inl.is_empty() {
                        content
                            .push_str(&format!("<text:p text:style-name=\"Body\">{inl}</text:p>"));
                    }
                    content.push_str(&blk);
                }
                if content.is_empty() {
                    content.push_str("<text:p text:style-name=\"Body\"/>");
                }
                blocks.push_str(&format!(
                    "<table:table-cell office:value-type=\"string\"{span_attrs}>\
                     {content}</table:table-cell>"
                ));
            }
            blocks.push_str("</table:table-row>");
        }
        blocks.push_str("</table:table>");
    }
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
    /// 적용할 ODF 텍스트 스타일 이름들 (중첩 span).
    fn span_styles(self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.bold {
            v.push("TB");
        }
        if self.italic {
            v.push("TI");
        }
        if self.underline {
            v.push("TU");
        }
        if self.strike {
            v.push("TS");
        }
        v
    }
}

fn open_spans(out: &mut String, s: Style) {
    for name in s.span_styles() {
        out.push_str(&format!("<text:span text:style-name=\"{name}\">"));
    }
}

fn close_spans(out: &mut String, s: &mut Style) {
    for _ in s.span_styles() {
        out.push_str("</text:span>");
    }
    *s = Style::default();
}

fn shape_at<'d>(doc: &'d Document, para: &Paragraph, pos: u32) -> Option<&'d CharShape> {
    let id = para
        .char_shape_runs
        .iter()
        .rev()
        .find(|(start, _)| *start <= pos)
        .map(|(_, id)| *id)?;
    doc.header.char_shapes.get(id.0 as usize)
}

fn image_kind(data: &[u8]) -> (&'static str, &'static str) {
    match data {
        [0x89, b'P', b'N', b'G', ..] => ("png", "image/png"),
        [0xFF, 0xD8, ..] => ("jpg", "image/jpeg"),
        [b'G', b'I', b'F', ..] => ("gif", "image/gif"),
        [b'B', b'M', ..] => ("bmp", "image/bmp"),
        _ => ("png", "image/png"),
    }
}

fn push_escaped(out: &mut String, c: char) {
    match c {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        _ => out.push(c),
    }
}

fn esc(s: &str) -> String {
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

const CONTENT_NS: &str = "xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" \
xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" \
xmlns:draw=\"urn:oasis:names:tc:opendocument:xmlns:drawing:1.0\" \
xmlns:fo=\"urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0\" \
xmlns:style=\"urn:oasis:names:tc:opendocument:xmlns:style:1.0\" \
xmlns:svg=\"urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0\" \
xmlns:xlink=\"http://www.w3.org/1999/xlink\"";

/// content.xml — 자동 스타일 + 본문.
fn content_xml(body: &str) -> String {
    let heading_sizes = [20, 18, 16, 14, 13, 12];
    let mut styles = String::from("<style:style style:name=\"Body\" style:family=\"paragraph\"/>");
    for (i, sz) in heading_sizes.iter().enumerate() {
        styles.push_str(&format!(
            "<style:style style:name=\"H{}\" style:family=\"paragraph\"><style:text-properties fo:font-size=\"{sz}pt\" fo:font-weight=\"bold\"/></style:style>",
            i + 1
        ));
    }
    styles.push_str(
        "<style:style style:name=\"TB\" style:family=\"text\"><style:text-properties fo:font-weight=\"bold\"/></style:style>\
         <style:style style:name=\"TI\" style:family=\"text\"><style:text-properties fo:font-style=\"italic\"/></style:style>\
         <style:style style:name=\"TU\" style:family=\"text\"><style:text-properties style:text-underline-style=\"solid\" style:text-underline-width=\"auto\" style:text-underline-color=\"font-color\"/></style:style>\
         <style:style style:name=\"TS\" style:family=\"text\"><style:text-properties style:text-line-through-style=\"solid\"/></style:style>\
         <style:style style:name=\"Img\" style:family=\"graphic\"><style:graphic-properties text:anchor-type=\"as-char\"/></style:style>\
         <style:style style:name=\"Tbl\" style:family=\"table\"><style:table-properties table:align=\"margins\"/></style:style>",
    );
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <office:document-content {CONTENT_NS} office:version=\"1.2\">\
         <office:automatic-styles>{styles}</office:automatic-styles>\
         <office:body><office:text>{body}</office:text></office:body>\
         </office:document-content>"
    )
}

const STYLES_XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-styles xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" xmlns:style=\"urn:oasis:names:tc:opendocument:xmlns:style:1.0\" xmlns:fo=\"urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0\" office:version=\"1.2\"><office:styles><style:default-style style:family=\"paragraph\"><style:text-properties style:font-name-asian=\"함초롬바탕\"/></style:default-style></office:styles></office:document-styles>";

fn meta_xml(doc: &Document) -> String {
    let mut m = String::new();
    if let Some(t) = doc.metadata.title.as_deref().filter(|s| !s.is_empty()) {
        m.push_str(&format!("<dc:title>{}</dc:title>", esc(t)));
    }
    if let Some(a) = doc.metadata.author.as_deref().filter(|s| !s.is_empty()) {
        m.push_str(&format!("<dc:creator>{}</dc:creator>", esc(a)));
    }
    if let Some(s) = doc.metadata.subject.as_deref().filter(|s| !s.is_empty()) {
        m.push_str(&format!("<dc:subject>{}</dc:subject>", esc(s)));
    }
    if let Some(k) = doc.metadata.keywords.as_deref().filter(|s| !s.is_empty()) {
        m.push_str(&format!("<meta:keyword>{}</meta:keyword>", esc(k)));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <office:document-meta xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
         xmlns:meta=\"urn:oasis:names:tc:opendocument:xmlns:meta:1.0\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\" office:version=\"1.2\">\
         <office:meta><meta:generator>hwp-cli</meta:generator>{m}</office:meta></office:document-meta>"
    )
}

fn manifest_xml(images: &[ImageItem]) -> String {
    let mut entries = String::from(
        "<manifest:file-entry manifest:full-path=\"/\" manifest:media-type=\"application/vnd.oasis.opendocument.text\"/>\
         <manifest:file-entry manifest:full-path=\"content.xml\" manifest:media-type=\"text/xml\"/>\
         <manifest:file-entry manifest:full-path=\"styles.xml\" manifest:media-type=\"text/xml\"/>\
         <manifest:file-entry manifest:full-path=\"meta.xml\" manifest:media-type=\"text/xml\"/>",
    );
    for (i, img) in images.iter().enumerate() {
        entries.push_str(&format!(
            "<manifest:file-entry manifest:full-path=\"Pictures/image{i}.{}\" manifest:media-type=\"{}\"/>",
            img.ext, img.mime
        ));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <manifest:manifest xmlns:manifest=\"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0\" manifest:version=\"1.2\">{entries}</manifest:manifest>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::from_markdown;
    use std::io::Read as _;

    fn unzip(bytes: &[u8], name: &str) -> Option<String> {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
        let mut f = zip.by_name(name).ok()?;
        let mut s = String::new();
        f.read_to_string(&mut s).ok()?;
        Some(s)
    }

    #[test]
    fn produces_valid_odt_structure() {
        let doc = from_markdown::from_markdown(
            "# 제목\n\n본문 단락\n\n| a | b |\n| - | - |\n| 1 | 2 |\n",
        );
        let bytes = to_odt(&doc).unwrap();
        // mimetype이 첫 엔트리 + STORED.
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let first = zip.by_index(0).unwrap();
        assert_eq!(first.name(), "mimetype");
        assert_eq!(first.compression(), zip::CompressionMethod::Stored);
        drop(first);
        let content = unzip(&bytes, "content.xml").unwrap();
        assert!(content.contains("<text:h"));
        assert!(content.contains("<table:table"));
        assert!(content.contains("text:outline-level"));
    }

    #[test]
    fn writes_metadata_to_meta_xml() {
        let mut doc = from_markdown::from_markdown("본문\n");
        doc.metadata.title = Some("제목 X".into());
        doc.metadata.author = Some("이영준".into());
        let bytes = to_odt(&doc).unwrap();
        let meta = unzip(&bytes, "meta.xml").unwrap();
        assert!(meta.contains("<dc:title>제목 X</dc:title>"));
        assert!(meta.contains("<dc:creator>이영준</dc:creator>"));
    }

    fn text_paragraph(text: &str) -> Paragraph {
        Paragraph {
            chars: text.chars().map(HwpChar::Text).collect(),
            ..Paragraph::default()
        }
    }

    fn cell(row: u16, col: u16, col_span: u16, row_span: u16, text: &str) -> hwp_model::Cell {
        use hwp_model::{BorderFillId, HwpUnit};
        hwp_model::Cell {
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

    fn table_of(rows: u16, cols: u16, cells: Vec<hwp_model::Cell>) -> hwp_model::Table {
        use hwp_model::BorderFillId;
        hwp_model::Table {
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
            caption: None,
            cells,
            extras: vec![],
        }
    }

    fn insert_table(doc: &mut Document, table: hwp_model::Table) {
        let paragraph = &mut doc.sections[0].paragraphs[0];
        let index = paragraph.controls.len() as u32;
        paragraph.controls.push(Control::Table(table));
        paragraph.chars.push(HwpChar::ExtCtrl {
            code: ctrl_char::OBJECT,
            ctrl_id: *b"tbl ",
            payload: vec![],
            ctrl_index: Some(index),
        });
    }

    #[test]
    fn 병합셀_spanned와_covered() {
        let mut doc = from_markdown::from_markdown("표\n");
        insert_table(
            &mut doc,
            table_of(
                2,
                3,
                vec![
                    cell(0, 0, 2, 1, "가로병합"),
                    cell(0, 2, 1, 2, "세로병합"),
                    cell(1, 0, 1, 1, "a"),
                    cell(1, 1, 1, 1, "b"),
                ],
            ),
        );
        let bytes = to_odt(&doc).unwrap();
        let content = unzip(&bytes, "content.xml").unwrap();
        assert!(
            content.contains("table:number-columns-spanned=\"2\""),
            "colspan: {content}"
        );
        assert!(
            content.contains("table:number-rows-spanned=\"2\""),
            "rowspan: {content}"
        );
        assert_eq!(
            content.matches("<table:covered-table-cell/>").count(),
            2,
            "덮인 칸: {content}"
        );
    }

    #[test]
    fn 셀_내_중첩_표_보존() {
        let mut doc = from_markdown::from_markdown("표\n");
        let mut inner_holder = text_paragraph("셀텍스트");
        let inner = table_of(1, 1, vec![cell(0, 0, 1, 1, "중첩")]);
        inner_holder.controls.push(Control::Table(inner));
        inner_holder.chars.push(HwpChar::ExtCtrl {
            code: ctrl_char::OBJECT,
            ctrl_id: *b"tbl ",
            payload: vec![],
            ctrl_index: Some(0),
        });
        let outer_cell = hwp_model::Cell {
            paragraphs: vec![inner_holder],
            ..cell(0, 0, 1, 1, "")
        };
        insert_table(&mut doc, table_of(1, 1, vec![outer_cell]));
        let bytes = to_odt(&doc).unwrap();
        let content = unzip(&bytes, "content.xml").unwrap();
        assert_eq!(
            content.matches("<table:table ").count(),
            2,
            "중첩 표: {content}"
        );
        assert!(content.contains("중첩") && content.contains("셀텍스트"));
    }

    #[test]
    fn 각주가_text_note로_방출() {
        let doc = from_markdown::from_markdown("본문 문단[^1]입니다.\n\n[^1]: 각주 내용\n");
        let bytes = to_odt(&doc).unwrap();
        let content = unzip(&bytes, "content.xml").unwrap();
        assert!(
            content.contains("<text:note text:id=\"ftn1\" text:note-class=\"footnote\">"),
            "note: {content}"
        );
        assert!(
            content.contains("<text:note-citation>1</text:note-citation>"),
            "citation: {content}"
        );
        assert!(content.contains("각주 내용"), "note body: {content}");
    }
}
