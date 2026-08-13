//! Scoped round-trip inspector for bytes emitted by our PDF backend
//! (`crates/hwp-render/src/pdf.rs`). This is not a general PDF text extractor.
//!
//! Supported structures are deliberately limited to backend invariants:
//! - uncompressed indirect objects (`N 0 obj` ... `endobj`) with classic xref
//! - Type0 + Identity-H subset CID fonts and ToUnicode CMaps (bfchar/bfrange)
//! - FlateDecode or uncompressed streams with an explicit `/Length`
//! - `Tf`/`Tj`/`TJ` text operators with two-byte big-endian GID strings

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Read as _;

use anyhow::{Context as _, Result, bail};

/// Inspection result extracted from a PDF.
pub struct PdfInspection {
    /// Page count after verifying `/Count` against the actual kid count.
    pub page_count: usize,
    /// ToUnicode-decoded text in content-stream order, with newlines between pages.
    pub decoded_text: String,
}

/// Parse PDF bytes and return the page count and ToUnicode-decoded text.
/// Every GID shown in a content stream must exist in the corresponding CMap.
pub fn inspect_pdf(data: &[u8]) -> Result<PdfInspection> {
    if !data.starts_with(b"%PDF-") {
        bail!("PDF 헤더가 없습니다");
    }
    let objects = parse_objects(data)?;

    let mut page_count = None;
    let mut kids = Vec::new();
    let mut pages: HashMap<u32, PageObject> = HashMap::new();
    let mut tounicode_of_type0: HashMap<u32, u32> = HashMap::new();
    for object in &objects {
        match name_after(&object.tokens, "Type") {
            Some("Pages") => {
                page_count = Some(
                    int_after(&object.tokens, "Count")
                        .and_then(|count| usize::try_from(count).ok())
                        .context("페이지 트리 /Count가 없습니다")?,
                );
                kids = refs_in_array_after(&object.tokens, "Kids")
                    .context("페이지 트리 /Kids가 없습니다")?;
            }
            Some("Page") => {
                let contents =
                    ref_after(&object.tokens, "Contents").context("페이지 /Contents가 없습니다")?;
                let fonts = font_resource_map(&object.tokens)?;
                pages.insert(object.id, PageObject { contents, fonts });
            }
            Some("Font") if name_after(&object.tokens, "Subtype") == Some("Type0") => {
                let tounicode = ref_after(&object.tokens, "ToUnicode")
                    .context("Type0 폰트 /ToUnicode가 없습니다")?;
                tounicode_of_type0.insert(object.id, tounicode);
            }
            _ => {}
        }
    }
    let page_count = page_count.context("페이지 트리가 없습니다")?;

    // Type0 object -> ToUnicode CMap (GID -> Unicode string).
    let mut cmaps: HashMap<u32, HashMap<u16, String>> = HashMap::new();
    for (&type0, &tounicode) in &tounicode_of_type0 {
        let object = find_object(&objects, tounicode)?;
        let bytes = stream_bytes(data, object)?;
        cmaps.insert(
            type0,
            parse_tounicode(&bytes).with_context(|| format!("ToUnicode {tounicode} 파싱 실패"))?,
        );
    }

    // Decode content in page-tree order.
    let mut decoded_text = String::new();
    for (index, &kid) in kids.iter().enumerate() {
        let page = pages
            .get(&kid)
            .with_context(|| format!("페이지 kid {kid} 객체가 없습니다"))?;
        let mut font_cmaps: HashMap<&str, &HashMap<u16, String>> = HashMap::new();
        for (name, type0) in &page.fonts {
            let cmap = cmaps
                .get(type0)
                .with_context(|| format!("폰트 리소스 {name}의 ToUnicode가 없습니다"))?;
            font_cmaps.insert(name.as_str(), cmap);
        }
        let content_object = find_object(&objects, page.contents)?;
        let content = stream_bytes(data, content_object)?;
        if index > 0 {
            decoded_text.push('\n');
        }
        decoded_text.push_str(&decode_content(&content, &font_cmaps)?);
    }
    if kids.len() != page_count {
        bail!("페이지 트리 /Count와 실제 kid 수가 다릅니다");
    }
    Ok(PdfInspection {
        page_count,
        decoded_text,
    })
}

struct PageObject {
    contents: u32,
    /// Resource name ("F0", ...) -> Type0 object number.
    fonts: Vec<(String, u32)>,
}

struct IndirectObject {
    id: u32,
    tokens: Vec<Token>,
    /// Raw stream byte range before decoding.
    stream: Option<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Name(String),
    Int(i64),
    Real(f64),
    Str(Vec<u8>),
    DictOpen,
    DictClose,
    ArrOpen,
    ArrClose,
    Kw(String),
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    /// Position immediately after the token, used to locate stream data.
    end: usize,
}

impl Tok {
    fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }
}

/// Scan indirect objects sequentially. Stream bytes are skipped using `/Length`
/// so embedded `endobj` or ` obj` byte sequences cannot confuse the scanner.
fn parse_objects(data: &[u8]) -> Result<Vec<IndirectObject>> {
    let mut objects = Vec::new();
    let mut pos = 0usize;
    while let Some(found) = find_subslice(&data[pos..], b" obj") {
        let marker = pos + found;
        let Some(id) = parse_object_header(data, marker) else {
            pos = marker + 1;
            continue;
        };
        let body_start = marker + 4;
        let (tokens, stream_start, resume) = tokenize_object(data, body_start)?;
        let (stream, next) = match stream_start {
            Some(start) => {
                let length = int_after(&tokens, "Length")
                    .and_then(|length| usize::try_from(length).ok())
                    .context("스트림 /Length가 없습니다")?;
                let end = start.checked_add(length).context("스트림 길이 오버플로")?;
                if end > data.len() {
                    bail!("스트림이 파일 끝을 넘습니다");
                }
                let mut tail = end;
                if data.get(tail) == Some(&b'\r') {
                    tail += 1;
                }
                if data.get(tail) == Some(&b'\n') {
                    tail += 1;
                }
                if !data[tail..].starts_with(b"endstream") {
                    bail!("스트림 /Length가 endstream과 일치하지 않습니다");
                }
                let endobj = find_subslice(&data[tail..], b"endobj")
                    .map(|offset| tail + offset)
                    .context("스트림 객체의 endobj가 없습니다")?;
                (Some((start, end)), endobj + 6)
            }
            None => (None, resume),
        };
        objects.push(IndirectObject { id, tokens, stream });
        pos = next;
    }
    if objects.is_empty() {
        bail!("간접 객체를 찾지 못했습니다");
    }
    Ok(objects)
}

/// Parse an `N 0 obj` header immediately before the marker.
fn parse_object_header(data: &[u8], marker: usize) -> Option<u32> {
    if marker == 0 || data[marker - 1] != b'0' {
        return None;
    }
    let mut start = marker - 1;
    if start == 0 || !data[start - 1].is_ascii_whitespace() {
        return None;
    }
    start -= 1;
    let digits_end = start;
    while start > 0 && data[start - 1].is_ascii_digit() {
        start -= 1;
    }
    if start == digits_end {
        return None;
    }
    if start > 0 && !matches!(data[start - 1], b'\n' | b'\r') {
        return None;
    }
    std::str::from_utf8(&data[start..digits_end])
        .ok()?
        .parse()
        .ok()
}

/// Tokenize an object body. For streams, consume the EOL after `stream` and
/// return the data start; otherwise stop immediately after `endobj`.
fn tokenize_object(data: &[u8], start: usize) -> Result<(Vec<Token>, Option<usize>, usize)> {
    let mut tokens = Vec::new();
    let mut pos = start;
    while let Some((token, next)) = next_token(data, pos)? {
        if matches!(&token.tok, Tok::Kw(kw) if kw == "stream") {
            let mut data_start = token.end;
            if data.get(data_start) == Some(&b'\r') {
                data_start += 1;
            }
            if data.get(data_start) != Some(&b'\n') {
                bail!("stream 키워드 뒤에 EOL이 없습니다");
            }
            return Ok((tokens, Some(data_start + 1), data_start + 1));
        }
        let is_endobj = matches!(&token.tok, Tok::Kw(kw) if kw == "endobj");
        let end = token.end;
        tokens.push(token);
        if is_endobj {
            tokens.pop();
            return Ok((tokens, None, end));
        }
        pos = next;
    }
    bail!("객체가 endobj 없이 끝났습니다")
}

/// Tokenize a complete content stream, treating EOF as normal termination.
fn tokenize_all(data: &[u8]) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut pos = 0usize;
    while let Some((token, next)) = next_token(data, pos)? {
        pos = next;
        tokens.push(token);
    }
    Ok(tokens)
}

fn next_token(data: &[u8], mut pos: usize) -> Result<Option<(Token, usize)>> {
    loop {
        match data.get(pos) {
            Some(byte) if byte.is_ascii_whitespace() => pos += 1,
            Some(b'%') => {
                while let Some(byte) = data.get(pos) {
                    if matches!(byte, b'\n' | b'\r') {
                        break;
                    }
                    pos += 1;
                }
            }
            Some(_) => break,
            None => return Ok(None),
        }
    }
    let start = pos;
    let byte = data[pos];
    let (tok, end) = match byte {
        b'(' => {
            let (bytes, end) = parse_literal_string(data, pos)?;
            (Tok::Str(bytes), end)
        }
        b'<' if data.get(pos + 1) == Some(&b'<') => (Tok::DictOpen, pos + 2),
        b'<' => {
            let (bytes, end) = parse_hex_string(data, pos)?;
            (Tok::Str(bytes), end)
        }
        b'>' if data.get(pos + 1) == Some(&b'>') => (Tok::DictClose, pos + 2),
        b'[' => (Tok::ArrOpen, pos + 1),
        b']' => (Tok::ArrClose, pos + 1),
        b'/' => {
            let mut end = pos + 1;
            while end < data.len() && !is_delimiter(data[end]) {
                end += 1;
            }
            let name = String::from_utf8(data[pos + 1..end].to_vec())
                .context("이름이 UTF-8이 아닙니다")?;
            (Tok::Name(name), end)
        }
        b'+' | b'-' | b'.' | b'0'..=b'9' => {
            let mut end = pos;
            while end < data.len() && !is_delimiter(data[end]) {
                end += 1;
            }
            let text = std::str::from_utf8(&data[pos..end]).context("숫자가 UTF-8이 아닙니다")?;
            if let Ok(value) = text.parse::<i64>() {
                (Tok::Int(value), end)
            } else {
                let value: f64 = text.parse().context("숫자 토큰이 파싱되지 않습니다")?;
                (Tok::Real(value), end)
            }
        }
        _ if !is_delimiter(byte) => {
            let mut end = pos;
            while end < data.len() && !is_delimiter(data[end]) {
                end += 1;
            }
            let keyword = std::str::from_utf8(&data[pos..end])
                .context("키워드가 UTF-8이 아닙니다")?
                .to_string();
            (Tok::Kw(keyword), end)
        }
        _ => bail!("토큰화할 수 없는 바이트 0x{byte:02x} (위치 {start})"),
    };
    Ok(Some((Token { tok, end }, end)))
}

fn is_delimiter(byte: u8) -> bool {
    byte.is_ascii_whitespace()
        || matches!(
            byte,
            b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
        )
}

/// Parse literal strings with balanced parentheses, standard escapes, up to
/// three octal digits, and backslash-newline continuation.
fn parse_literal_string(data: &[u8], start: usize) -> Result<(Vec<u8>, usize)> {
    let mut bytes = Vec::new();
    let mut depth = 1usize;
    let mut pos = start + 1;
    while pos < data.len() {
        let byte = data[pos];
        pos += 1;
        match byte {
            b'(' => {
                depth += 1;
                bytes.push(byte);
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((bytes, pos));
                }
                bytes.push(byte);
            }
            b'\\' => {
                let Some(&escaped) = data.get(pos) else {
                    bail!("리터럴 문자열이 역슬래시로 끝났습니다");
                };
                pos += 1;
                match escaped {
                    b'n' => bytes.push(b'\n'),
                    b'r' => bytes.push(b'\r'),
                    b't' => bytes.push(b'\t'),
                    b'b' => bytes.push(0x08),
                    b'f' => bytes.push(0x0C),
                    b'(' | b')' | b'\\' => bytes.push(escaped),
                    b'\n' => {}
                    b'\r' => {
                        if data.get(pos) == Some(&b'\n') {
                            pos += 1;
                        }
                    }
                    b'0'..=b'7' => {
                        let mut value = u32::from(escaped - b'0');
                        for _ in 0..2 {
                            match data.get(pos) {
                                Some(digit @ b'0'..=b'7') => {
                                    value = value * 8 + u32::from(digit - b'0');
                                    pos += 1;
                                }
                                _ => break,
                            }
                        }
                        bytes.push(u8::try_from(value & 0xFF).unwrap_or(b'?'));
                    }
                    other => bytes.push(other),
                }
            }
            _ => bytes.push(byte),
        }
    }
    bail!("리터럴 문자열이 닫히지 않았습니다")
}

fn parse_hex_string(data: &[u8], start: usize) -> Result<(Vec<u8>, usize)> {
    let mut bytes = Vec::new();
    let mut high = None;
    let mut pos = start + 1;
    while pos < data.len() {
        let byte = data[pos];
        pos += 1;
        if byte == b'>' {
            if let Some(nibble) = high {
                bytes.push(nibble << 4);
            }
            return Ok((bytes, pos));
        }
        if byte.is_ascii_whitespace() {
            continue;
        }
        let Some(value) = (byte as char).to_digit(16) else {
            bail!("16진 문자열에 잘못된 바이트 0x{byte:02x}가 있습니다");
        };
        match high {
            None => high = Some(value as u8),
            Some(nibble) => {
                bytes.push(nibble << 4 | value as u8);
                high = None;
            }
        }
    }
    bail!("16진 문자열이 닫히지 않았습니다")
}

fn stream_bytes<'a>(data: &'a [u8], object: &IndirectObject) -> Result<Cow<'a, [u8]>> {
    let (start, end) = object
        .stream
        .with_context(|| format!("객체 {}는 스트림이 아닙니다", object.id))?;
    let raw = &data[start..end];
    let flate = object
        .tokens
        .iter()
        .any(|token| matches!(&token.tok, Tok::Name(name) if name == "FlateDecode"));
    if !flate {
        return Ok(Cow::Borrowed(raw));
    }
    let mut decoded = Vec::new();
    flate2::read::ZlibDecoder::new(raw)
        .read_to_end(&mut decoded)
        .context("FlateDecode 압축 해제 실패")?;
    Ok(Cow::Owned(decoded))
}

fn find_object(objects: &[IndirectObject], id: u32) -> Result<&IndirectObject> {
    objects
        .iter()
        .find(|object| object.id == id)
        .with_context(|| format!("객체 {id}를 찾지 못했습니다"))
}

fn name_after<'t>(tokens: &'t [Token], name: &str) -> Option<&'t str> {
    let pos = tokens
        .iter()
        .position(|token| matches!(&token.tok, Tok::Name(key) if key == name))?;
    match &tokens.get(pos + 1)?.tok {
        Tok::Name(value) => Some(value.as_str()),
        _ => None,
    }
}

fn int_after(tokens: &[Token], name: &str) -> Option<i64> {
    let pos = tokens
        .iter()
        .position(|token| matches!(&token.tok, Tok::Name(key) if key == name))?;
    tokens.get(pos + 1)?.tok.as_int()
}

fn ref_after(tokens: &[Token], name: &str) -> Option<u32> {
    let pos = tokens
        .iter()
        .position(|token| matches!(&token.tok, Tok::Name(key) if key == name))?;
    let object = tokens.get(pos + 1)?.tok.as_int()?;
    tokens.get(pos + 2)?.tok.as_int()?;
    if !matches!(&tokens.get(pos + 3)?.tok, Tok::Kw(kw) if kw == "R") {
        return None;
    }
    u32::try_from(object).ok()
}

/// Parse references from an array such as `/Kids [1 0 R 2 0 R]`.
fn refs_in_array_after(tokens: &[Token], name: &str) -> Option<Vec<u32>> {
    let pos = tokens
        .iter()
        .position(|token| matches!(&token.tok, Tok::Name(key) if key == name))?;
    if !matches!(&tokens.get(pos + 1)?.tok, Tok::ArrOpen) {
        return None;
    }
    let mut refs = Vec::new();
    let mut cursor = pos + 2;
    while !matches!(&tokens.get(cursor)?.tok, Tok::ArrClose) {
        let object = tokens.get(cursor)?.tok.as_int()?;
        tokens.get(cursor + 1)?.tok.as_int()?;
        if !matches!(&tokens.get(cursor + 2)?.tok, Tok::Kw(kw) if kw == "R") {
            return None;
        }
        refs.push(u32::try_from(object).ok()?);
        cursor += 3;
    }
    Some(refs)
}

/// Parse a page `/Resources` font dictionary into resource-name -> Type0 object.
fn font_resource_map(tokens: &[Token]) -> Result<Vec<(String, u32)>> {
    let pos = tokens
        .iter()
        .position(|token| matches!(&token.tok, Tok::Name(key) if key == "Font"))
        .context("페이지 리소스에 /Font가 없습니다")?;
    if !matches!(
        &tokens.get(pos + 1).map(|token| &token.tok),
        Some(Tok::DictOpen)
    ) {
        bail!("/Font가 사전이 아닙니다");
    }
    let mut fonts = Vec::new();
    let mut cursor = pos + 2;
    loop {
        match tokens.get(cursor).map(|token| &token.tok) {
            Some(Tok::DictClose) => break,
            Some(Tok::Name(name)) => {
                let object = tokens
                    .get(cursor + 1)
                    .and_then(|token| token.tok.as_int())
                    .context("폰트 리소스 값이 참조가 아닙니다")?;
                tokens
                    .get(cursor + 2)
                    .and_then(|token| token.tok.as_int())
                    .context("폰트 리소스 값이 참조가 아닙니다")?;
                if !matches!(&tokens.get(cursor + 3).map(|token| &token.tok), Some(Tok::Kw(kw)) if kw == "R")
                {
                    bail!("폰트 리소스 값이 참조가 아닙니다");
                }
                fonts.push((name.clone(), u32::try_from(object)?));
                cursor += 4;
            }
            _ => bail!("/Font 사전이 닫히지 않았습니다"),
        }
    }
    Ok(fonts)
}

/// Decode shown strings with the current font's ToUnicode CMap.
fn decode_content(content: &[u8], fonts: &HashMap<&str, &HashMap<u16, String>>) -> Result<String> {
    let tokens = tokenize_all(content)?;
    let mut current_font: Option<&str> = None;
    let mut pending: Vec<&[u8]> = Vec::new();
    let mut in_array = false;
    let mut out = String::new();
    for (index, token) in tokens.iter().enumerate() {
        match &token.tok {
            Tok::Str(bytes) => pending.push(bytes),
            Tok::ArrOpen => {
                pending.clear();
                in_array = true;
            }
            Tok::ArrClose => in_array = false,
            Tok::Kw(keyword) => {
                match keyword.as_str() {
                    "Tf" => {
                        current_font = index
                            .checked_sub(2)
                            .and_then(|pos| tokens.get(pos))
                            .and_then(|token| match &token.tok {
                                Tok::Name(name) => Some(name.as_str()),
                                _ => None,
                            });
                        if current_font.is_none() {
                            bail!("Tf의 폰트 피연산자가 없습니다");
                        }
                    }
                    "Tj" => {
                        let bytes = pending.last().context("Tj의 문자열이 없습니다")?;
                        out.push_str(&decode_shown(bytes, current_font, fonts)?);
                    }
                    "TJ" => {
                        for bytes in &pending {
                            out.push_str(&decode_shown(bytes, current_font, fonts)?);
                        }
                    }
                    _ => {}
                }
                pending.clear();
            }
            _ if !in_array => pending.clear(),
            _ => {}
        }
    }
    Ok(out)
}

/// Decode a two-byte big-endian GID string; missing mappings are errors.
fn decode_shown(
    bytes: &[u8],
    current_font: Option<&str>,
    fonts: &HashMap<&str, &HashMap<u16, String>>,
) -> Result<String> {
    if !bytes.len().is_multiple_of(2) {
        bail!("shown 문자열 길이가 짝수가 아닙니다");
    }
    let font = current_font.context("Tf 없이 텍스트를 그렸습니다")?;
    let cmap = fonts
        .get(font)
        .with_context(|| format!("알 수 없는 폰트 리소스 {font}입니다"))?;
    let mut out = String::new();
    for pair in bytes.chunks_exact(2) {
        let gid = u16::from_be_bytes([pair[0], pair[1]]);
        let text = cmap
            .get(&gid)
            .with_context(|| format!("ToUnicode에 GID {gid}가 없습니다"))?;
        out.push_str(text);
    }
    Ok(out)
}

/// Parse bfchar/bfrange entries into a GID -> Unicode string map.
fn parse_tounicode(cmap: &[u8]) -> Result<HashMap<u16, String>> {
    let text = std::str::from_utf8(cmap).context("ToUnicode CMap이 UTF-8이 아닙니다")?;
    #[derive(PartialEq)]
    enum Mode {
        None,
        Bfchar,
        Bfrange,
    }
    let mut mode = Mode::None;
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.ends_with("beginbfchar") {
            mode = Mode::Bfchar;
            continue;
        }
        if line.ends_with("beginbfrange") {
            mode = Mode::Bfrange;
            continue;
        }
        if line.starts_with("endbfchar") || line.starts_with("endbfrange") {
            mode = Mode::None;
            continue;
        }
        let groups = hex_groups(line);
        match mode {
            Mode::Bfchar if groups.len() >= 2 => {
                map.insert(parse_gid(groups[0])?, parse_utf16(groups[1])?);
            }
            Mode::Bfrange if groups.len() >= 3 => {
                let low = parse_gid(groups[0])?;
                let high = parse_gid(groups[1])?;
                if high < low {
                    bail!("bfrange 범위가 역전되었습니다");
                }
                if groups.len() == 3 {
                    // Simple form: increment consecutive code points.
                    let base = parse_utf16(groups[2])?;
                    let mut chars = base.chars();
                    let (Some(first), None) = (chars.next(), chars.next()) else {
                        bail!("bfrange 증가 기준이 단일 문자가 아닙니다");
                    };
                    for gid in low..=high {
                        let code = u32::from(first) + u32::from(gid - low);
                        let ch = char::from_u32(code).context("bfrange 코드포인트 오버플로")?;
                        map.insert(gid, ch.to_string());
                    }
                } else {
                    // Array form: [<d1> <d2> ...].
                    if groups.len() - 2 != usize::from(high - low) + 1 {
                        bail!("bfrange 배열 길이가 범위와 다릅니다");
                    }
                    for (offset, group) in groups[2..].iter().enumerate() {
                        let gid = low + u16::try_from(offset)?;
                        map.insert(gid, parse_utf16(group)?);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(map)
}

/// Collect `<...>` hexadecimal groups from one line.
fn hex_groups(line: &str) -> Vec<&str> {
    let mut groups = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('>') else {
            break;
        };
        groups.push(&after[..close]);
        rest = &after[close + 1..];
    }
    groups
}

fn parse_gid(hex: &str) -> Result<u16> {
    if hex.len() != 4 {
        bail!("GID는 4자리 16진이어야 합니다: {hex}");
    }
    Ok(u16::from_str_radix(hex, 16)?)
}

/// Decode a UTF-16BE hexadecimal string, including surrogate pairs.
fn parse_utf16(hex: &str) -> Result<String> {
    if hex.is_empty() || !hex.len().is_multiple_of(4) {
        bail!("UTF-16BE 16진 길이가 올바르지 않습니다: {hex}");
    }
    let units = (0..hex.len() / 4)
        .map(|index| u16::from_str_radix(&hex[index * 4..index * 4 + 4], 16))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(char::decode_utf16(units).collect::<std::result::Result<String, _>>()?)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_string_decodes_escapes_and_octal() {
        let (bytes, end) = parse_literal_string(br"(\000\005A\(B\\))", 0).unwrap();
        assert_eq!(bytes, vec![0x00, 0x05, b'A', b'(', b'B', b'\\']);
        assert_eq!(end, 16);
        let (nested, _) = parse_literal_string(b"(a(b)c)", 0).unwrap();
        assert_eq!(nested, b"a(b)c");
        let (continuation, _) = parse_literal_string(b"(ab\\\r\ncd)", 0).unwrap();
        assert_eq!(continuation, b"abcd");
    }

    #[test]
    fn hex_string_decodes_with_padding() {
        let (bytes, _) = parse_hex_string(b"<00C8>", 0).unwrap();
        assert_eq!(bytes, vec![0x00, 0xC8]);
        let (padded, _) = parse_hex_string(b"<0>", 0).unwrap();
        assert_eq!(padded, vec![0x00]);
    }

    #[test]
    fn tounicode_parses_bfchar_and_bfrange() {
        let cmap = b"1 beginbfchar\n<0003> <0020>\n<0004> <D55C>\nendbfchar\n2 beginbfrange\n<0005> <0006> <AC00>\n<0007> <0008> [<D55C><D55D>]\nendbfrange\n";
        let map = parse_tounicode(cmap).unwrap();
        assert_eq!(map.get(&3).map(String::as_str), Some(" "));
        assert_eq!(map.get(&4).map(String::as_str), Some("한"));
        assert_eq!(map.get(&5).map(String::as_str), Some("가"));
        assert_eq!(map.get(&6).map(String::as_str), Some("각"));
        assert_eq!(map.get(&7).map(String::as_str), Some("한"));
        assert_eq!(map.get(&8).map(String::as_str), Some("핝"));
    }

    #[test]
    fn shown_gids_must_exist_in_tounicode() {
        let mut cmap = HashMap::new();
        cmap.insert(1u16, "가".to_string());
        let mut fonts = HashMap::new();
        fonts.insert("F0", &cmap);
        let decoded = decode_shown(&[0x00, 0x01], Some("F0"), &fonts).unwrap();
        assert_eq!(decoded, "가");
        assert!(decode_shown(&[0x00, 0x02], Some("F0"), &fonts).is_err());
        assert!(decode_shown(&[0x00], Some("F0"), &fonts).is_err());
        assert!(decode_shown(&[0x00, 0x01], Some("F9"), &fonts).is_err());
    }

    #[test]
    fn text_showing_operators_decode_tj_and_tj_arrays() {
        let mut cmap = HashMap::new();
        cmap.insert(1u16, "가".to_string());
        cmap.insert(2u16, "나".to_string());
        cmap.insert(3u16, "다".to_string());
        let mut fonts = HashMap::new();
        fonts.insert("F0", &cmap);

        let content = b"/F0 10 Tf <0001> Tj [<0002> -120 <0003>] TJ";
        assert_eq!(decode_content(content, &fonts).unwrap(), "가나다");
    }
}
