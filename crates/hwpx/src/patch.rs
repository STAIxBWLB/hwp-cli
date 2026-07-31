//! 충실도 보존 텍스트 치환 (패키지 외과 수술).
//!
//! IR을 거치는 [`write`](crate::write)는 미리보기 썸네일(`Preview/PrvImage.png`)·
//! `hp:switch` 2016 호환 블록·미모델 엔트리(settings/DocOptions/scripts)를 잃는다.
//! 이 모듈은 본문 `Contents/section*.xml`(과 미리보기 텍스트)의 텍스트만 외과적으로
//! 치환하고 나머지 엔트리는 ZIP raw 복사로 **바이트 보존**한다(mimetype STORED·순서 포함).
//!
//! 한계: 대상 문자열이 `<hp:t>` 런/문단 경계를 가로지류면(예: `<hp:t>{{기</hp:t>
//! <hp:t>관명}}</hp:t>`) 문자열 치환이 매칭하지 못한다. XML 이벤트 단위 재구성이
//! 필요한 편집은 IR 경로(`hwp edit`)를 쓴다.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{HwpxError, Result};
use crate::package::{
    HwpxPackage, PackageLimits, inspect_archive, preflight_central_directory, read_bounded,
    verify_archive_integrity,
};

static PATCH_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const HWPX_PARAGRAPH_NAMESPACE: &str = "http://www.hancom.co.kr/hwpml/2011/paragraph";

/// `Contents/section*.xml`의 `{{name}}`을 값으로 치환하고, 그 외 엔트리는 원본 그대로
/// 복사한다. 반환: 이름 → 치환 횟수(요청한 모든 이름 포함, 미발견은 0).
pub fn fill_placeholders(
    input: &Path,
    output: &Path,
    values: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, usize>> {
    fill_placeholders_with_limits(input, output, values, &PackageLimits::default())
}

/// 사용자 지정 패키지 제한으로 자리표시자를 치환한다.
pub fn fill_placeholders_with_limits(
    input: &Path,
    output: &Path,
    values: &BTreeMap<String, String>,
    limits: &PackageLimits,
) -> Result<BTreeMap<String, usize>> {
    let mut counts: BTreeMap<String, usize> = values.keys().map(|k| (k.clone(), 0)).collect();
    process_package(input, output, limits, is_section_entry, |name, data| {
        let original = data.clone();
        let mut xml = String::from_utf8(data).map_err(invalid_data)?;
        for (k, v) in values {
            let needle = format!("{{{{{k}}}}}"); // {{k}}
            let n = xml.matches(needle.as_str()).count();
            if n > 0 {
                xml = xml.replace(needle.as_str(), &xml_escape(v));
                if let Some(c) = counts.get_mut(k) {
                    *c += n;
                }
            }
        }
        let _ = name;
        let updated = xml.into_bytes();
        Ok((updated != original).then_some(updated))
    })?;
    Ok(counts)
}

/// Package-surgical TemplateSpec fill result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateFillCounts {
    pub placeholders: BTreeMap<String, usize>,
    pub fields: BTreeMap<String, usize>,
}

/// Fill exact `{{name}}` placeholders and one unambiguous simple text field per
/// requested field name while raw-copying every untouched package entry.
///
/// A field region is accepted only when its value contains text and line-break
/// XML. Nested controls, pictures, tables, equations, or malformed begin/end
/// pairs fail before the staged package is published.
pub fn fill_template_values(
    input: &Path,
    output: &Path,
    placeholders: &BTreeMap<String, String>,
    fields: &BTreeMap<String, String>,
) -> Result<TemplateFillCounts> {
    fill_template_values_with_limits(
        input,
        output,
        placeholders,
        fields,
        &PackageLimits::default(),
    )
}

/// [`fill_template_values`] with caller-supplied package limits.
pub fn fill_template_values_with_limits(
    input: &Path,
    output: &Path,
    placeholders: &BTreeMap<String, String>,
    fields: &BTreeMap<String, String>,
    limits: &PackageLimits,
) -> Result<TemplateFillCounts> {
    let mut placeholder_counts = placeholders
        .keys()
        .map(|name| (name.clone(), 0usize))
        .collect::<BTreeMap<_, _>>();
    let mut field_counts = fields
        .keys()
        .map(|name| (name.clone(), 0usize))
        .collect::<BTreeMap<_, _>>();
    process_package(input, output, limits, is_section_entry, |_, data| {
        let original = data.clone();
        let mut xml = String::from_utf8(data).map_err(invalid_data)?;
        if !placeholders.is_empty() {
            xml = replace_text_placeholders(&xml, placeholders, &mut placeholder_counts)?;
        }
        if !fields.is_empty() {
            let (updated, counts) = replace_simple_fields(xml, fields)?;
            xml = updated;
            for (name, count) in counts {
                *field_counts.entry(name).or_default() += count;
            }
        }
        let updated = xml.into_bytes();
        Ok((updated != original).then_some(updated))
    })?;
    Ok(TemplateFillCounts {
        placeholders: placeholder_counts,
        fields: field_counts,
    })
}

/// Replace placeholders only in character data below an element whose local
/// name is `t`. Tags, attributes, comments, processing instructions and CDATA
/// are metadata for this operation: a requested placeholder there is rejected
/// instead of silently mutating document structure.
fn replace_text_placeholders(
    xml: &str,
    placeholders: &BTreeMap<String, String>,
    counts: &mut BTreeMap<String, usize>,
) -> Result<String> {
    let needles = placeholders
        .iter()
        .map(|(name, value)| (name, format!("{{{{{name}}}}}"), xml_escape(value)))
        .collect::<Vec<_>>();
    let mut output = String::with_capacity(xml.len());
    let mut cursor = 0usize;
    let mut scope = XmlTextScope::default();

    while cursor < xml.len() {
        let Some(relative) = xml[cursor..].find('<') else {
            append_placeholder_text(
                &mut output,
                &xml[cursor..],
                scope.inside_text(),
                &needles,
                counts,
            )?;
            break;
        };
        let tag_start = cursor + relative;
        append_placeholder_text(
            &mut output,
            &xml[cursor..tag_start],
            scope.inside_text(),
            &needles,
            counts,
        )?;

        let tag_end = find_markup_end(xml, tag_start)?;
        let markup = &xml[tag_start..tag_end];
        reject_placeholder_outside_text(markup, &needles)?;
        output.push_str(markup);
        scope.update(markup)?;
        cursor = tag_end;
    }

    if !scope.frames.is_empty() {
        return Err(placeholder_error("XML element is not closed"));
    }
    Ok(output)
}

fn append_placeholder_text(
    output: &mut String,
    text: &str,
    inside_text: bool,
    needles: &[(&String, String, String)],
    counts: &mut BTreeMap<String, usize>,
) -> Result<()> {
    if !inside_text {
        reject_placeholder_outside_text(text, needles)?;
        output.push_str(text);
        return Ok(());
    }

    let mut replaced = text.to_string();
    for (name, needle, value) in needles {
        let count = replaced.matches(needle).count();
        if count > 0 {
            replaced = replaced.replace(needle, value);
            *counts.entry((*name).clone()).or_default() += count;
        }
    }
    output.push_str(&replaced);
    Ok(())
}

fn reject_placeholder_outside_text(
    text: &str,
    needles: &[(&String, String, String)],
) -> Result<()> {
    if needles.iter().any(|(_, needle, _)| text.contains(needle)) {
        return Err(placeholder_error(
            "requested placeholder occurs outside a text node",
        ));
    }
    Ok(())
}

fn find_markup_end(xml: &str, start: usize) -> Result<usize> {
    let suffix = &xml[start..];
    for (prefix, terminator) in [("<!--", "-->"), ("<![CDATA[", "]]>"), ("<?", "?>")] {
        if suffix.starts_with(prefix) {
            return suffix
                .find(terminator)
                .map(|offset| start + offset + terminator.len())
                .ok_or_else(|| placeholder_error("XML markup is not closed"));
        }
    }

    let mut quote = None;
    for (offset, character) in suffix.char_indices().skip(1) {
        match (quote, character) {
            (Some(active), current) if active == current => quote = None,
            (None, '\'' | '"') => quote = Some(character),
            (None, '>') => return Ok(start + offset + 1),
            _ => {}
        }
    }
    Err(placeholder_error("XML tag is not closed"))
}

#[derive(Default)]
struct XmlTextScope {
    namespaces: BTreeMap<String, String>,
    frames: Vec<XmlElementFrame>,
    text_depth: usize,
}

struct XmlElementFrame {
    qualified_name: String,
    is_text: bool,
    namespace_changes: Vec<(String, Option<String>)>,
}

impl XmlTextScope {
    fn inside_text(&self) -> bool {
        self.text_depth > 0
    }

    fn update(&mut self, markup: &str) -> Result<()> {
        if markup.starts_with("<!--")
            || markup.starts_with("<![CDATA[")
            || markup.starts_with("<?")
            || markup.starts_with("<!")
        {
            return Ok(());
        }
        let inner = markup[1..markup.len() - 1].trim();
        if let Some(closing) = inner.strip_prefix('/') {
            let qualified_name = xml_name(closing);
            let frame = self
                .frames
                .pop()
                .ok_or_else(|| placeholder_error("unexpected XML end tag"))?;
            if frame.qualified_name != qualified_name {
                return Err(placeholder_error("XML end tag does not match start tag"));
            }
            if frame.is_text {
                self.text_depth = self
                    .text_depth
                    .checked_sub(1)
                    .ok_or_else(|| placeholder_error("text element nesting underflow"))?;
            }
            self.restore_namespaces(frame.namespace_changes);
            return Ok(());
        }

        let qualified_name = xml_name(inner);
        if qualified_name.is_empty() {
            return Err(placeholder_error("XML element name is empty"));
        }
        let declarations = namespace_declarations(inner, qualified_name.len())?;
        let mut namespace_changes = Vec::with_capacity(declarations.len());
        for (prefix, uri) in declarations {
            let previous = self.namespaces.insert(prefix.clone(), uri);
            namespace_changes.push((prefix, previous));
        }
        let (prefix, local_name) = qualified_name
            .split_once(':')
            .map_or(("", qualified_name), |(prefix, local)| (prefix, local));
        let is_text = local_name == "t"
            && self.namespaces.get(prefix).map(String::as_str) == Some(HWPX_PARAGRAPH_NAMESPACE);
        let self_closing = inner.trim_end().ends_with('/');
        if self_closing {
            self.restore_namespaces(namespace_changes);
        } else {
            if is_text {
                self.text_depth = self
                    .text_depth
                    .checked_add(1)
                    .ok_or_else(|| placeholder_error("text element nesting overflow"))?;
            }
            self.frames.push(XmlElementFrame {
                qualified_name: qualified_name.to_string(),
                is_text,
                namespace_changes,
            });
        }
        Ok(())
    }

    fn restore_namespaces(&mut self, changes: Vec<(String, Option<String>)>) {
        for (prefix, previous) in changes.into_iter().rev() {
            if let Some(uri) = previous {
                self.namespaces.insert(prefix, uri);
            } else {
                self.namespaces.remove(&prefix);
            }
        }
    }
}

fn xml_name(value: &str) -> &str {
    value
        .split(|character: char| character.is_ascii_whitespace() || character == '/')
        .next()
        .unwrap_or_default()
}

fn namespace_declarations(inner: &str, name_len: usize) -> Result<Vec<(String, String)>> {
    let bytes = inner.as_bytes();
    let mut cursor = name_len;
    let mut declarations = Vec::new();
    while cursor < bytes.len() {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] == b'/' {
            break;
        }
        let name_start = cursor;
        while cursor < bytes.len()
            && !bytes[cursor].is_ascii_whitespace()
            && bytes[cursor] != b'='
            && bytes[cursor] != b'/'
        {
            cursor += 1;
        }
        let attribute_name = &inner[name_start..cursor];
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b'=' {
            return Err(placeholder_error("XML attribute has no value"));
        }
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || !matches!(bytes[cursor], b'\'' | b'"') {
            return Err(placeholder_error("XML attribute value is not quoted"));
        }
        let quote = bytes[cursor];
        cursor += 1;
        let value_start = cursor;
        while cursor < bytes.len() && bytes[cursor] != quote {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            return Err(placeholder_error("XML attribute quote is not closed"));
        }
        if attribute_name == "xmlns" || attribute_name.starts_with("xmlns:") {
            let prefix = attribute_name.strip_prefix("xmlns:").unwrap_or("");
            let uri = quick_xml::escape::unescape(&inner[value_start..cursor])
                .map_err(|_| placeholder_error("XML namespace escape is invalid"))?;
            declarations.push((prefix.to_string(), uri.into_owned()));
        }
        cursor += 1;
    }
    Ok(declarations)
}

fn placeholder_error(message: &str) -> HwpxError {
    HwpxError::PackageIntegrity(format!("template placeholder fill rejected: {message}"))
}

#[derive(Debug)]
struct FieldRegion {
    name: String,
    start: usize,
    end: usize,
    replacement: String,
}

fn replace_simple_fields(
    xml: String,
    fields: &BTreeMap<String, String>,
) -> Result<(String, BTreeMap<String, usize>)> {
    let mut regions = Vec::new();
    let mut counts = fields
        .keys()
        .map(|name| (name.clone(), 0usize))
        .collect::<BTreeMap<_, _>>();
    let mut cursor = 0usize;
    while let Some(relative) = xml[cursor..].find("<hp:fieldBegin") {
        let begin = cursor + relative;
        let tag_end = xml[begin..]
            .find('>')
            .map(|offset| begin + offset + 1)
            .ok_or_else(|| field_error("fieldBegin tag is not closed"))?;
        let tag = &xml[begin..tag_end];
        let Some(name) = xml_attribute(tag, "name")? else {
            cursor = tag_end;
            continue;
        };
        let Some(value) = fields.get(&name) else {
            cursor = tag_end;
            continue;
        };
        let id = xml_attribute(tag, "id")?
            .ok_or_else(|| field_error("requested fieldBegin has no id"))?;
        let begin_ctrl_end = xml[tag_end..]
            .find("</hp:ctrl>")
            .map(|offset| tag_end + offset + "</hp:ctrl>".len())
            .ok_or_else(|| field_error("requested fieldBegin control is not closed"))?;
        let mut search = begin_ctrl_end;
        let (field_end_start, field_ctrl_start) = loop {
            let candidate = xml[search..]
                .find("<hp:fieldEnd")
                .map(|offset| search + offset)
                .ok_or_else(|| field_error("requested field has no matching fieldEnd"))?;
            let candidate_end = xml[candidate..]
                .find('>')
                .map(|offset| candidate + offset + 1)
                .ok_or_else(|| field_error("fieldEnd tag is not closed"))?;
            let end_tag = &xml[candidate..candidate_end];
            if xml_attribute(end_tag, "beginIDRef")?.as_deref() == Some(id.as_str()) {
                let ctrl_start = xml[begin_ctrl_end..candidate]
                    .rfind("<hp:ctrl")
                    .map(|offset| begin_ctrl_end + offset)
                    .ok_or_else(|| field_error("matching fieldEnd is not inside hp:ctrl"))?;
                break (candidate, ctrl_start);
            }
            search = candidate_end;
        };
        let region = &xml[begin_ctrl_end..field_ctrl_start];
        validate_simple_field_fragment(region)?;
        let replacement = text_fragment(value);
        regions.push(FieldRegion {
            name: name.clone(),
            start: begin_ctrl_end,
            end: field_ctrl_start,
            replacement,
        });
        *counts.entry(name).or_default() += 1;
        cursor = field_end_start + 1;
    }

    for (name, count) in &counts {
        if *count > 1 {
            return Err(field_error(&format!(
                "requested field name is ambiguous ({name}, {count} occurrences)"
            )));
        }
    }
    regions.sort_by_key(|region| region.start);
    for pair in regions.windows(2) {
        if pair[0].end > pair[1].start {
            return Err(field_error("requested field regions overlap"));
        }
    }
    let mut output = xml;
    for region in regions.into_iter().rev() {
        let _ = region.name;
        output.replace_range(region.start..region.end, &region.replacement);
    }
    Ok((output, counts))
}

fn xml_attribute(tag: &str, name: &str) -> Result<Option<String>> {
    for quote in ['"', '\''] {
        let needle = format!(" {name}={quote}");
        if let Some(start) = tag.find(&needle) {
            let value_start = start + needle.len();
            let value_end = tag[value_start..]
                .find(quote)
                .map(|offset| value_start + offset)
                .ok_or_else(|| field_error("field attribute quote is not closed"))?;
            let value = quick_xml::escape::unescape(&tag[value_start..value_end])
                .map_err(|_| field_error("field attribute escape is invalid"))?;
            return Ok(Some(value.into_owned()));
        }
    }
    Ok(None)
}

fn validate_simple_field_fragment(fragment: &str) -> Result<()> {
    use quick_xml::events::Event;

    let wrapped = format!(
        r#"<root xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph">{fragment}</root>"#
    );
    let mut reader = quick_xml::Reader::from_str(&wrapped);
    reader.config_mut().trim_text(false);
    let mut in_text = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) if event.name().as_ref() == b"root" => {}
            Ok(Event::End(event)) if event.name().as_ref() == b"root" => {}
            Ok(Event::Start(event)) if event.name().as_ref() == b"hp:t" && !in_text => {
                in_text = true;
            }
            Ok(Event::End(event)) if event.name().as_ref() == b"hp:t" && in_text => {
                in_text = false;
            }
            Ok(Event::Empty(event)) if event.name().as_ref() == b"hp:t" && !in_text => {}
            Ok(Event::Empty(event)) if event.name().as_ref() == b"hp:lineBreak" && in_text => {}
            Ok(Event::Text(text)) => {
                let raw: &[u8] = text.as_ref();
                if !in_text && !raw.iter().all(u8::is_ascii_whitespace) {
                    return Err(field_error(
                        "requested field value contains non-text or malformed XML",
                    ));
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) | Err(_) => {
                return Err(field_error(
                    "requested field value contains non-text or malformed XML",
                ));
            }
        }
    }
    if in_text {
        return Err(field_error("requested field text is not closed"));
    }
    Ok(())
}

fn text_fragment(value: &str) -> String {
    let mut fragment = String::from(r#"<hp:t xml:space="preserve">"#);
    for (index, line) in value.split('\n').enumerate() {
        if index > 0 {
            fragment.push_str("<hp:lineBreak/>");
        }
        fragment.push_str(&xml_escape(line));
    }
    fragment.push_str("</hp:t>");
    fragment
}

fn field_error(message: &str) -> HwpxError {
    HwpxError::PackageIntegrity(format!("template field fill rejected: {message}"))
}

/// `Contents/section*.xml`에서 (from→to) 쌍을 **순서대로** 치환하고, 그 외 엔트리는
/// 원본 그대로 복사한다. `Preview/PrvText.txt`는 평문(UTF-8, 아니면 UTF-16LE)으로 치환한다.
///
/// 치환은 순차 적용이다 — 앞 쌍의 치환 결과가 뒤 쌍의 from에 매칭될 수 있으므로
/// 호출자는 **긴 이름을 먼저** 넣어야 한다(예: "제주한라대학교" → "제주한라대").
/// 반환: 변환 대상 엔트리(section*.xml, PrvText.txt) → 치환 건수(0 포함).
pub fn replace_texts(
    input: &Path,
    output: &Path,
    replacements: &[(String, String)],
) -> Result<BTreeMap<String, usize>> {
    replace_texts_with_limits(input, output, replacements, &PackageLimits::default())
}

/// 사용자 지정 패키지 제한으로 텍스트를 치환한다.
pub fn replace_texts_with_limits(
    input: &Path,
    output: &Path,
    replacements: &[(String, String)],
    limits: &PackageLimits,
) -> Result<BTreeMap<String, usize>> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    process_package(
        input,
        output,
        limits,
        |name| is_section_entry(name) || name == "Preview/PrvText.txt",
        |name, data| {
            if is_section_entry(name) {
                let xml = String::from_utf8(data).map_err(invalid_data)?;
                let (xml, n) = replace_seq(xml, replacements, true);
                counts.insert(name.to_string(), n);
                return Ok(Some(xml.into_bytes()));
            }
            let (data, n) = replace_in_prvtext(data, replacements);
            counts.insert(name.to_string(), n);
            Ok(Some(data))
        },
    )?;
    Ok(counts)
}

/// UTF-8 디코드 실패를 io 오류로 변환.
fn invalid_data(e: std::string::FromUtf8Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e)
}

/// 변환 대상 엔트리 판별 (본문 섹션 XML).
fn is_section_entry(name: &str) -> bool {
    name.starts_with("Contents/section") && name.ends_with(".xml")
}

/// 패키지 순회 공통 뼈대: 변환 대상만 제한 안에서 압축 해제해 다시 쓰고,
/// 나머지는 압축 바이트를 스트리밍 raw copy한다.
///
/// 입력은 먼저 출력 옆의 비공개 snapshot으로 한 번만 복제한다. 이후 검증·변환은
/// 그 snapshot의 열린 핸들만 사용하므로, 원본 경로가 교체되거나 같은 경로/하드링크로
/// 제자리 호출해도 검증한 패키지와 실제 변환 패키지가 달라지지 않는다.
fn process_package(
    input: &Path,
    output: &Path,
    limits: &PackageLimits,
    mut should_transform: impl FnMut(&str) -> bool,
    mut transform: impl FnMut(&str, Vec<u8>) -> Result<Option<Vec<u8>>>,
) -> Result<()> {
    let destination = inspect_destination(output)?;
    let (_source_guard, snapshot) = snapshot_input(input, output, limits)?;
    let mut archive = zip::ZipArchive::new(snapshot)?;
    let entries = inspect_archive(&mut archive, limits)?;
    // raw_copy_file은 CRC를 확인하지 않으므로, 출력 stage를 만들기 전에 snapshot의
    // 모든 엔트리를 실제 EOF까지 읽어 선언 크기·deflate·CRC를 함께 검증한다.
    verify_archive_integrity(&mut archive, &entries, limits)?;
    let source_layout = capture_zip_layout(_source_guard.path())?;
    let source_comment = archive.comment().to_vec();

    #[cfg(test)]
    run_after_snapshot_hook();

    let (mut staged_guard, out) = SiblingTemp::create(output, "stage")?;
    let mut zip = zip::ZipWriter::new(out);
    zip.set_raw_comment(source_comment.clone().into_boxed_slice())?;
    let mut output_total = 0u64;
    let mut preserve_raw = vec![false; entries.len()];

    for (i, info) in entries.into_iter().enumerate() {
        if should_transform(&info.name) {
            let limit = limits.entry_uncompressed_limit(&info.name);
            let transformed = {
                let mut entry = archive.by_index(i)?;
                let data = read_bounded(&mut entry, &info.name, info.size, limit)?;
                transform(&info.name, data)?
            };
            if let Some(new_data) = transformed {
                limits.check_entry(&info.name, new_data.len() as u64, None)?;
                output_total =
                    limits.add_uncompressed_total(output_total, new_data.len() as u64)?;
                let opts = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated);
                zip.start_file(&info.name, opts)?;
                zip.write_all(&new_data)?;
            } else {
                output_total = limits.add_uncompressed_total(output_total, info.size)?;
                let raw = archive.by_index_raw(i)?;
                zip.raw_copy_file(raw)?;
                preserve_raw[i] = true;
            }
        } else {
            // 미리보기·compat·BinData·mimetype(STORED) 등 전부 바이트 보존.
            output_total = limits.add_uncompressed_total(output_total, info.size)?;
            let raw = archive.by_index_raw(i)?;
            // 위 verify_archive_integrity가 같은 private snapshot의 이 엔트리를
            // 실제 압축 해제 크기와 CRC까지 검증했다. raw copy는 검증된 압축
            // 바이트와 ZIP 메타데이터를 그대로 보존한다.
            zip.raw_copy_file(raw)?;
            preserve_raw[i] = true;
        }
    }
    let staged_file = zip.finish()?;
    staged_file.sync_all()?;
    drop(staged_file);

    rebuild_with_preserved_entries(
        _source_guard.path(),
        staged_guard.path(),
        &source_layout,
        &preserve_raw,
        &source_comment,
    )?;

    validate_staged_package(staged_guard.path(), limits)?;
    apply_destination_permissions(staged_guard.path(), &destination)?;
    File::open(staged_guard.path())
        .and_then(|file| file.sync_all())
        .map_err(|error| labeled_io("staged sync", error))?;
    recheck_destination(output, &destination)?;
    atomic_publish(staged_guard.path(), output)
        .map_err(|error| labeled_io("atomic_publish", error))?;
    staged_guard.disarm();
    sync_parent_directory(output);
    Ok(())
}

const CENTRAL_HEADER_SIGNATURE: &[u8; 4] = b"PK\x01\x02";
const LOCAL_HEADER_SIGNATURE: &[u8; 4] = b"PK\x03\x04";
const END_OF_CENTRAL_SIGNATURE: &[u8; 4] = b"PK\x05\x06";
const CENTRAL_FIXED_BYTES: usize = 46;

struct ZipEntryLayout {
    local_range: Range<u64>,
    central_record: Vec<u8>,
}

struct ZipLayout {
    entries: Vec<ZipEntryLayout>,
}

fn capture_zip_layout(path: &Path) -> Result<ZipLayout> {
    let mut archive = zip::ZipArchive::new(File::open(path)?)?;
    let mut metadata = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        metadata.push((entry.header_start(), entry.central_header_start()));
    }
    if metadata.is_empty() {
        return Err(HwpxError::PackageIntegrity(
            "ZIP 엔트리가 없어 raw metadata를 보존할 수 없습니다".to_string(),
        ));
    }
    let central_start = metadata
        .iter()
        .map(|(_, central)| *central)
        .min()
        .ok_or_else(|| HwpxError::PackageIntegrity("ZIP central directory가 없습니다".into()))?;
    let mut local_order = metadata
        .iter()
        .enumerate()
        .map(|(index, (start, _))| (*start, index))
        .collect::<Vec<_>>();
    local_order.sort_unstable();
    if local_order.first().map(|(start, _)| *start) != Some(0) {
        return Err(HwpxError::PackageIntegrity(
            "ZIP preamble이 있는 패키지는 exact raw 보존을 지원하지 않습니다".to_string(),
        ));
    }
    for pair in local_order.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(HwpxError::PackageIntegrity(
                "ZIP local header offset이 중복됩니다".to_string(),
            ));
        }
    }

    let mut local_ranges = vec![0..0; metadata.len()];
    for (position, (start, index)) in local_order.iter().enumerate() {
        let end = local_order
            .get(position + 1)
            .map_or(central_start, |(next, _)| *next);
        if *start >= end || end > central_start {
            return Err(HwpxError::PackageIntegrity(
                "ZIP local entry 범위가 올바르지 않습니다".to_string(),
            ));
        }
        local_ranges[*index] = *start..end;
    }

    let mut file = File::open(path)?;
    let mut entries = Vec::with_capacity(metadata.len());
    for (index, (_, central_header_start)) in metadata.into_iter().enumerate() {
        file.seek(SeekFrom::Start(local_ranges[index].start))?;
        let mut local_signature = [0u8; 4];
        file.read_exact(&mut local_signature)?;
        if &local_signature != LOCAL_HEADER_SIGNATURE {
            return Err(HwpxError::PackageIntegrity(
                "ZIP local header signature가 올바르지 않습니다".to_string(),
            ));
        }
        entries.push(ZipEntryLayout {
            local_range: local_ranges[index].clone(),
            central_record: read_central_record(&mut file, central_header_start)?,
        });
    }
    Ok(ZipLayout { entries })
}

fn read_central_record(file: &mut File, start: u64) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(start))?;
    let mut fixed = [0u8; CENTRAL_FIXED_BYTES];
    file.read_exact(&mut fixed)?;
    if &fixed[..4] != CENTRAL_HEADER_SIGNATURE {
        return Err(HwpxError::PackageIntegrity(
            "ZIP central header signature가 올바르지 않습니다".to_string(),
        ));
    }
    let variable = usize::from(u16::from_le_bytes([fixed[28], fixed[29]]))
        + usize::from(u16::from_le_bytes([fixed[30], fixed[31]]))
        + usize::from(u16::from_le_bytes([fixed[32], fixed[33]]));
    let mut record = Vec::with_capacity(CENTRAL_FIXED_BYTES + variable);
    record.extend_from_slice(&fixed);
    record.resize(CENTRAL_FIXED_BYTES + variable, 0);
    file.read_exact(&mut record[CENTRAL_FIXED_BYTES..])?;
    Ok(record)
}

fn central_name(record: &[u8]) -> Result<&[u8]> {
    if record.len() < CENTRAL_FIXED_BYTES || &record[..4] != CENTRAL_HEADER_SIGNATURE {
        return Err(HwpxError::PackageIntegrity(
            "ZIP central record가 잘렸습니다".to_string(),
        ));
    }
    let name_len = usize::from(u16::from_le_bytes([record[28], record[29]]));
    record
        .get(CENTRAL_FIXED_BYTES..CENTRAL_FIXED_BYTES + name_len)
        .ok_or_else(|| HwpxError::PackageIntegrity("ZIP central name이 잘렸습니다".to_string()))
}

fn rebuild_with_preserved_entries(
    source_path: &Path,
    generated_path: &Path,
    source: &ZipLayout,
    preserve_raw: &[bool],
    comment: &[u8],
) -> Result<()> {
    let generated = capture_zip_layout(generated_path)?;
    if source.entries.len() != generated.entries.len()
        || source.entries.len() != preserve_raw.len()
        || source.entries.len() > usize::from(u16::MAX)
        || comment.len() > usize::from(u16::MAX)
    {
        return Err(HwpxError::PackageIntegrity(
            "ZIP raw metadata 재조립 범위가 올바르지 않습니다".to_string(),
        ));
    }

    let (_guard, mut rebuilt) = SiblingTemp::create(generated_path, "rebuild")?;
    let mut source_file = File::open(source_path)?;
    let mut generated_file = File::open(generated_path)?;
    let mut new_offsets = Vec::with_capacity(source.entries.len());
    for (index, preserve) in preserve_raw.iter().copied().enumerate() {
        let offset = rebuilt.stream_position()?;
        if offset > u64::from(u32::MAX) {
            return Err(HwpxError::PackageIntegrity(
                "ZIP64 local offset은 exact raw 보존 범위를 벗어납니다".to_string(),
            ));
        }
        new_offsets.push(offset as u32);
        let (reader, range): (&mut File, &Range<u64>) = if preserve {
            (&mut source_file, &source.entries[index].local_range)
        } else {
            (&mut generated_file, &generated.entries[index].local_range)
        };
        copy_range(reader, &mut rebuilt, range)?;
    }

    let central_start = rebuilt.stream_position()?;
    if central_start > u64::from(u32::MAX) {
        return Err(HwpxError::PackageIntegrity(
            "ZIP64 central offset은 exact raw 보존 범위를 벗어납니다".to_string(),
        ));
    }
    for (index, preserve) in preserve_raw.iter().copied().enumerate() {
        let original = if preserve {
            &source.entries[index].central_record
        } else {
            &generated.entries[index].central_record
        };
        if central_name(original)? != central_name(&generated.entries[index].central_record)? {
            return Err(HwpxError::PackageIntegrity(
                "ZIP raw metadata 엔트리 이름이 일치하지 않습니다".to_string(),
            ));
        }
        if u32::from_le_bytes(original[42..46].try_into().expect("fixed central header"))
            == u32::MAX
        {
            return Err(HwpxError::PackageIntegrity(
                "ZIP64 central record는 exact raw 보존 범위를 벗어납니다".to_string(),
            ));
        }
        let mut record = original.clone();
        record[42..46].copy_from_slice(&new_offsets[index].to_le_bytes());
        rebuilt.write_all(&record)?;
    }
    let central_end = rebuilt.stream_position()?;
    let central_size = central_end
        .checked_sub(central_start)
        .and_then(|size| u32::try_from(size).ok())
        .ok_or_else(|| {
            HwpxError::PackageIntegrity(
                "ZIP64 central size는 exact raw 보존 범위를 벗어납니다".to_string(),
            )
        })?;
    let entries = source.entries.len() as u16;
    rebuilt.write_all(END_OF_CENTRAL_SIGNATURE)?;
    rebuilt.write_all(&0u16.to_le_bytes())?;
    rebuilt.write_all(&0u16.to_le_bytes())?;
    rebuilt.write_all(&entries.to_le_bytes())?;
    rebuilt.write_all(&entries.to_le_bytes())?;
    rebuilt.write_all(&central_size.to_le_bytes())?;
    rebuilt.write_all(&(central_start as u32).to_le_bytes())?;
    rebuilt.write_all(&(comment.len() as u16).to_le_bytes())?;
    rebuilt.write_all(comment)?;
    rebuilt.sync_all()?;
    rebuilt.seek(SeekFrom::Start(0))?;

    let mut destination = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(generated_path)?;
    std::io::copy(&mut rebuilt, &mut destination)?;
    destination.sync_all()?;
    Ok(())
}

fn copy_range(input: &mut File, output: &mut File, range: &Range<u64>) -> Result<()> {
    input.seek(SeekFrom::Start(range.start))?;
    let expected = range.end.checked_sub(range.start).ok_or_else(|| {
        HwpxError::PackageIntegrity("ZIP entry range가 역전되었습니다".to_string())
    })?;
    let copied = std::io::copy(&mut input.take(expected), output)?;
    if copied != expected {
        return Err(HwpxError::PackageIntegrity(
            "ZIP raw entry가 복사 중 잘렸습니다".to_string(),
        ));
    }
    Ok(())
}

/// 원본을 한 번 연 핸들에서 제한된 크기로 snapshot하고, 이후에는 원본 경로를
/// 다시 열지 않는다. 복제 전·후 EOCD preflight를 모두 수행하므로 복제 중
/// truncate/append 또는 같은 크기의 패키지 교체도 snapshot 자체 검증에서 실패한다.
fn snapshot_input(
    input: &Path,
    output: &Path,
    limits: &PackageLimits,
) -> Result<(SiblingTemp, File)> {
    let mut source = File::open(input)?;
    preflight_central_directory(&mut source, limits)?;
    let expected_len = source.metadata()?.len();
    source.seek(SeekFrom::Start(0))?;

    let (guard, mut snapshot) = SiblingTemp::create(output, "source")?;
    let actual_len = std::io::copy(
        &mut std::io::Read::take(&mut source, expected_len.saturating_add(1)),
        &mut snapshot,
    )?;
    if actual_len != expected_len {
        return Err(HwpxError::PackageIntegrity(format!(
            "입력 snapshot 중 원본 크기가 변경되었습니다 \
             ({actual_len} != {expected_len} 바이트)"
        )));
    }
    snapshot.flush()?;
    snapshot.seek(SeekFrom::Start(0))?;
    preflight_central_directory(&mut snapshot, limits)?;
    snapshot.seek(SeekFrom::Start(0))?;
    Ok((guard, snapshot))
}

/// 최종 stage를 공개하기 전에 패키지 무결성과 OWPML의 핵심 의미 구조를 함께
/// 검증한다. custom limit 호출도 같은 제한을 끝까지 유지한다.
fn validate_staged_package(path: &Path, limits: &PackageLimits) -> Result<()> {
    let mut package = HwpxPackage::open_with_limits(path, limits)?;
    package.verify_integrity()?;

    let version = String::from_utf8(package.read_entry("version.xml")?).map_err(invalid_data)?;
    if version.trim().is_empty() {
        return Err(HwpxError::PackageIntegrity(
            "version.xml이 비어 있습니다".to_string(),
        ));
    }
    package.version_info()?;

    let header =
        String::from_utf8(package.read_entry("Contents/header.xml")?).map_err(invalid_data)?;
    crate::read::header::parse_header(&header)?;

    let sections = package.section_entries()?;
    if sections.is_empty() {
        return Err(HwpxError::PackageIntegrity(
            "본문 section XML이 없습니다".to_string(),
        ));
    }
    for name in sections {
        let xml = String::from_utf8(package.read_entry(&name)?).map_err(invalid_data)?;
        crate::read::section::parse_section(&xml)?;
    }
    Ok(())
}

#[derive(Clone)]
enum DestinationSnapshot {
    Missing,
    Regular {
        identity: FileIdentity,
        len: u64,
        modified: Option<std::time::SystemTime>,
        permissions: fs::Permissions,
    },
}

fn inspect_destination(path: &Path) -> Result<DestinationSnapshot> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("출력 경로가 심볼릭 링크입니다: {}", path.display()),
        )
        .into()),
        Ok(metadata) if metadata.file_type().is_file() => Ok(DestinationSnapshot::Regular {
            identity: file_identity(path, &metadata),
            len: metadata.len(),
            modified: metadata.modified().ok(),
            permissions: metadata.permissions(),
        }),
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("출력 경로가 일반 파일이 아닙니다: {}", path.display()),
        )
        .into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(DestinationSnapshot::Missing)
        }
        Err(error) => Err(error.into()),
    }
}

fn recheck_destination(path: &Path, snapshot: &DestinationSnapshot) -> Result<()> {
    match (snapshot, fs::symlink_metadata(path)) {
        (DestinationSnapshot::Missing, Err(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(())
        }
        (
            DestinationSnapshot::Regular {
                identity,
                len,
                modified,
                ..
            },
            Ok(metadata),
        ) if metadata.file_type().is_file()
            && file_identity(path, &metadata) == *identity
            && metadata.len() == *len
            && metadata.modified().ok() == *modified =>
        {
            Ok(())
        }
        (_, Err(error)) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "검증 중 출력 대상이 변경되어 게시를 중단합니다: {}",
                path.display()
            ),
        )
        .into()),
    }
}

fn apply_destination_permissions(path: &Path, snapshot: &DestinationSnapshot) -> Result<()> {
    if let DestinationSnapshot::Regular { permissions, .. } = snapshot {
        fs::set_permissions(path, permissions.clone())?;
    }
    Ok(())
}

/// 파일 신원. `Unknown`은 자기 자신과도 같지 않다 — 신원을 못 읽었으면 "그대로다"를
/// 증명할 수 없으므로 recheck가 실패(fail-closed)해야 한다. 그래서 `Eq`는 구현하지
/// 않는다(반사성이 성립하지 않음).
#[derive(Clone, Copy, Debug)]
enum FileIdentity {
    Known {
        first: u64,
        second: u64,
    },
    /// Windows에서 핸들을 못 열었을 때만 나온다. 다른 플랫폼은 metadata로 항상 구성된다.
    #[cfg_attr(not(windows), allow(dead_code))]
    Unknown,
}

impl PartialEq for FileIdentity {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                FileIdentity::Known { first, second },
                FileIdentity::Known {
                    first: other_first,
                    second: other_second,
                },
            ) => first == other_first && second == other_second,
            _ => false,
        }
    }
}

#[cfg(unix)]
fn file_identity(_path: &Path, metadata: &fs::Metadata) -> FileIdentity {
    use std::os::unix::fs::MetadataExt;
    FileIdentity::Known {
        first: metadata.dev(),
        second: metadata.ino(),
    }
}

// volume serial + file index는 핸들에서만 얻는다(std의 Metadata 경로는 nightly 전용
// `windows_by_handle`). 링크는 따라가지 않고 열어, 검사 뒤 심볼릭 링크로 바뀌었으면
// 다른 신원이 나와 recheck가 실패하도록 한다.
#[cfg(windows)]
fn file_identity(path: &Path, _metadata: &fs::Metadata) -> FileIdentity {
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::AsRawHandle as _;

    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    const FILE_SHARE_DELETE: u32 = 0x4;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    #[repr(C)]
    #[derive(Default)]
    struct ByHandleFileInformation {
        file_attributes: u32,
        creation_time_low: u32,
        creation_time_high: u32,
        last_access_time_low: u32,
        last_access_time_high: u32,
        last_write_time_low: u32,
        last_write_time_high: u32,
        volume_serial_number: u32,
        file_size_high: u32,
        file_size_low: u32,
        number_of_links: u32,
        file_index_high: u32,
        file_index_low: u32,
    }

    unsafe extern "system" {
        fn GetFileInformationByHandle(
            file: *mut std::ffi::c_void,
            information: *mut ByHandleFileInformation,
        ) -> i32;
    }

    let Ok(file) = fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
    else {
        return FileIdentity::Unknown;
    };
    let mut information = ByHandleFileInformation::default();
    // SAFETY: 핸들은 이 스코프에서 살아 있는 File 소유이고, 출력 구조체는 Win32
    // BY_HANDLE_FILE_INFORMATION과 같은 repr(C) 레이아웃이다.
    let loaded = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) };
    if loaded == 0 {
        return FileIdentity::Unknown;
    }
    FileIdentity::Known {
        first: u64::from(information.volume_serial_number),
        second: (u64::from(information.file_index_high) << 32)
            | u64::from(information.file_index_low),
    }
}

#[cfg(not(any(unix, windows)))]
fn file_identity(_path: &Path, metadata: &fs::Metadata) -> FileIdentity {
    FileIdentity::Known {
        first: metadata.len(),
        second: 0,
    }
}

struct SiblingTemp {
    path: PathBuf,
    armed: bool,
}

impl SiblingTemp {
    fn create(destination: &Path, kind: &str) -> Result<(Self, File)> {
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let file_name = destination.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("출력 파일 이름이 없습니다: {}", destination.display()),
            )
        })?;
        let display_name = file_name.to_string_lossy();

        for _ in 0..1_024 {
            let sequence = PATCH_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(
                ".{display_name}.hwp-patch-{}-{sequence}-{kind}.hwpx",
                std::process::id()
            ));
            match open_private_new(&path) {
                Ok(file) => {
                    return Ok((Self { path, armed: true }, file));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(labeled_io("staged create", error).into()),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "충돌하지 않는 patch 임시 파일을 만들 수 없습니다: {}",
                parent.display()
            ),
        )
        .into())
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for SiblingTemp {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// io 오류에 연산 이름을 남긴다 — 상위(MCP 등)에서 체인이 평탄화돼도 어느 단계인지 보인다.
fn labeled_io(operation: &str, error: std::io::Error) -> std::io::Error {
    std::io::Error::new(error.kind(), format!("{operation}: {error}"))
}

fn open_private_new(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(unix)]
fn atomic_publish(staged: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(staged, destination)
}

#[cfg(windows)]
fn atomic_publish(staged: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    let from: Vec<u16> = staged
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let to: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: 두 경로는 NUL 종료 UTF-16 버퍼이며 호출 동안 살아 있다. sibling
    // stage이므로 MoveFileExW는 같은 볼륨에서 원자적으로 교체한다.
    let result = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn atomic_publish(staged: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(staged, destination)
}

fn sync_parent_directory(path: &Path) {
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        // 게시 성공 뒤 directory fsync 실패를 반환 오류로 바꾸면 "실패했지만
        // 목적지는 변경됨"이 되므로, durability 보강은 최선 노력으로 수행한다.
        let _ = File::open(parent).and_then(|directory| directory.sync_all());
    }
}

#[cfg(test)]
thread_local! {
    static AFTER_SNAPSHOT_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
fn run_after_snapshot_hook() {
    AFTER_SNAPSHOT_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
}

/// (from→to) 쌍을 순서대로 치환한다. `escape`면 XML 텍스트 규칙으로 이스케이프 후 치환.
/// 반환: (결과 문자열, 총 치환 건수).
fn replace_seq(
    mut text: String,
    replacements: &[(String, String)],
    escape: bool,
) -> (String, usize) {
    let mut total = 0;
    for (from, to) in replacements {
        if from.is_empty() {
            continue;
        }
        let (needle, value) = if escape {
            (xml_escape(from), xml_escape(to))
        } else {
            (from.clone(), to.clone())
        };
        let n = text.matches(needle.as_str()).count();
        if n > 0 {
            text = text.replace(needle.as_str(), &value);
            total += n;
        }
    }
    (text, total)
}

/// PrvText.txt 평문 치환. UTF-8이 우선이고, 실패 시 UTF-16LE로 디코드해 치환 후
/// 같은 인코딩으로 되돌린다. 치환 건수 0이어도 Some을 돌려준다(엔트리 존재 보고용).
fn replace_in_prvtext(data: Vec<u8>, replacements: &[(String, String)]) -> (Vec<u8>, usize) {
    if let Ok(text) = String::from_utf8(data.clone()) {
        let (text, n) = replace_seq(text, replacements, false);
        return (text.into_bytes(), n);
    }
    // UTF-16LE 폴백: 손실 허용 디코드(치환 대상 이름은 정상 문자 영역에 있음).
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let text = String::from_utf16_lossy(&units);
    let (text, n) = replace_seq(text, replacements, false);
    let encoded: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    (encoded, n)
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use zip::CompressionMethod;
    use zip::write::SimpleFileOptions;

    #[test]
    fn public_fill은_snapshot_뒤_원본이_바뀌어도_검증한_snapshot만_변환() {
        let id = PATCH_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir();
        let input = base.join(format!(
            "hwpx_snapshot_input_{}_{id}.hwpx",
            std::process::id()
        ));
        let replacement = base.join(format!(
            "hwpx_snapshot_replacement_{}_{id}.hwpx",
            std::process::id()
        ));
        let output = base.join(format!(
            "hwpx_snapshot_output_{}_{id}.hwpx",
            std::process::id()
        ));
        write_fixture(&input, "{{name}}-snapshot-A");
        write_fixture(&replacement, "{{name}}-replacement-B");

        let hook_input = input.clone();
        let hook_replacement = replacement.clone();
        AFTER_SNAPSHOT_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::copy(&hook_replacement, &hook_input).unwrap();
            }));
        });

        let mut values = BTreeMap::new();
        values.insert("name".to_string(), "완료".to_string());
        fill_placeholders(&input, &output, &values).expect("public fill");

        let output_xml = read_entry(&output, "Contents/section0.xml");
        assert!(output_xml.contains("완료-snapshot-A"));
        assert!(!output_xml.contains("replacement-B"));
        assert!(
            read_entry(&input, "Contents/section0.xml").contains("replacement-B"),
            "hook이 실제 원본 경로를 교체했는지 확인"
        );

        let _ = fs::remove_file(input);
        let _ = fs::remove_file(replacement);
        let _ = fs::remove_file(output);
    }

    fn write_fixture(path: &Path, text: &str) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        for (name, data, method) in [
            (
                "mimetype",
                b"application/hwp+zip".as_slice(),
                CompressionMethod::Stored,
            ),
            (
                "version.xml",
                br#"<version major="1" minor="4" micro="0" buildNumber="0"/>"#.as_slice(),
                CompressionMethod::Deflated,
            ),
            (
                "Contents/header.xml",
                b"<hh:head/>".as_slice(),
                CompressionMethod::Deflated,
            ),
        ] {
            zip.start_file(
                name,
                SimpleFileOptions::default().compression_method(method),
            )
            .unwrap();
            zip.write_all(data).unwrap();
        }
        zip.start_file(
            "Contents/section0.xml",
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )
        .unwrap();
        write!(
            zip,
            "<hs:sec><hp:p><hp:run><hp:t>{text}</hp:t></hp:run></hp:p></hs:sec>"
        )
        .unwrap();
        zip.finish().unwrap();
    }

    fn read_entry(path: &Path, name: &str) -> String {
        let mut zip = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
        let mut text = String::new();
        zip.by_name(name)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }
}
