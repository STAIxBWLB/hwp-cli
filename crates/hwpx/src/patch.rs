//! Fidelity-preserving text replacement by package surgery.
//!
//! The IR-based [`write`](crate::write) loses preview thumbnails (`Preview/PrvImage.png`), `hp:switch`
//! 2016 compatibility blocks and unmodeled entries (settings/DocOptions/scripts). This module changes
//! only body text in `Contents/section*.xml` and preview text, preserving every other entry
//! byte-for-byte through ZIP raw copying, including mimetype STORED and entry order.
//!
//! Limitation: plain string replacement cannot match across `<hp:t>` run boundaries. `{{name}}`
//! placeholders are an exception: [`coalesce_split_placeholders`] moves pieces within one paragraph into
//! the first run before replacement (#145). Use the IR path (`hwp edit`) for edits spanning paragraphs
//! or requiring reconstruction of XML events.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use hwp_model::WriteReport;

use crate::error::{HwpxError, Result};
use crate::package::{
    HwpxPackage, PackageLimits, inspect_archive, preflight_central_directory, read_bounded,
    verify_archive_integrity,
};
use crate::write::section::BinCollector;

static PATCH_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const HWPX_PARAGRAPH_NAMESPACE: &str = "http://www.hancom.co.kr/hwpml/2011/paragraph";

/// Replace the `{{ name }}` slots of `Contents/section*.xml` with their values and copy every
/// other entry unchanged. Slots are matched with [`hwp_model::slot_tokens`], the grammar
/// `hwp slots` reports with, so a padded `{{ name }}` is filled too, and requested names are
/// trimmed the same way (#362). Values are literal: one pass over the original text, never
/// rescanned. Returns name -> replacement count under the caller's spelling, every requested
/// name included (0 when not found).
pub fn fill_placeholders(
    input: &Path,
    output: &Path,
    values: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, usize>> {
    fill_placeholders_with_limits(input, output, values, &PackageLimits::default())
}

/// Replaces placeholders with caller-specified package limits.
pub fn fill_placeholders_with_limits(
    input: &Path,
    output: &Path,
    values: &BTreeMap<String, String>,
    limits: &PackageLimits,
) -> Result<BTreeMap<String, usize>> {
    let mut counts: BTreeMap<String, usize> = values.keys().map(|k| (k.clone(), 0)).collect();
    let slots = escaped_slots(values)?;
    process_package(input, output, limits, is_section_entry, |name, data| {
        let original = data.clone();
        let mut xml = String::from_utf8(data).map_err(invalid_data)?;
        // Pull any placeholder that formatting split across runs back into one run first, so the
        // plain replace below sees exactly what `hwp slots` reported (#145).
        let names: Vec<&str> = slots.keys().copied().collect();
        coalesce_split_placeholders(&mut xml, &names);
        let xml = replace_slots(&xml, &slots, &mut counts);
        let _ = name;
        let updated = xml.into_bytes();
        Ok((updated != original).then_some(updated))
    })?;
    Ok(counts)
}

/// Requested slots by trimmed name: the keys that name each, as the caller spelled them (each
/// counted per token filled), and the value escaped once for XML character data.
type SlotValues<'a> = BTreeMap<&'a str, (Vec<&'a str>, String)>;

fn escaped_slots(values: &BTreeMap<String, String>) -> Result<SlotValues<'_>> {
    let lookup = hwp_model::slot_lookup(values).map_err(|error| placeholder_error(&error))?;
    Ok(lookup
        .into_iter()
        .map(|(name, request)| (name, (request.keys, xml_escape(request.value))))
        .collect())
}

/// A raw-XML token's name as `hwp slots` reads it from the IR: entity and character references
/// decoded (`R&amp;D` is `R&D`, `&#44032;` is `가`), then trimmed. A malformed reference is left
/// as written.
fn xml_slot_name(raw: &str) -> std::borrow::Cow<'_, str> {
    match quick_xml::escape::unescape(raw) {
        Ok(std::borrow::Cow::Owned(name)) => std::borrow::Cow::Owned(name.trim().to_string()),
        _ => std::borrow::Cow::Borrowed(raw),
    }
}

/// Replace every requested slot token of `text` in one pass, so a value that itself looks like
/// a slot is never filled again. Counts under the caller's key.
fn replace_slots(
    text: &str,
    slots: &SlotValues<'_>,
    counts: &mut BTreeMap<String, usize>,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0usize;
    for token in hwp_model::slot_tokens(text) {
        let Some((keys, value)) = slots.get(xml_slot_name(token.name).as_ref()) else {
            continue;
        };
        out.push_str(&text[at..token.range.start]);
        out.push_str(value);
        at = token.range.end;
        for key in keys {
            *counts.entry((*key).to_string()).or_default() += 1;
        }
    }
    out.push_str(&text[at..]);
    out
}

/// One text element's content, as a byte range into the section XML.
struct TextSpan {
    content: Range<usize>,
}

/// Match an element tag by XML **local name**, the way `read::section` does, so a package that
/// binds the paragraph namespace to a prefix other than `hp` — or writes a tab or newline before
/// the attributes — is recognised here too. Otherwise `hwp slots` would report a split
/// placeholder that this pass could not see, which is the very disagreement #145 closes.
///
/// `tail` starts at `<` (open tag) or at `</` (close tag).
fn tag_local_name(tail: &str) -> Option<(&str, usize)> {
    let after_bracket = if let Some(rest) = tail.strip_prefix("</") {
        rest
    } else {
        tail.strip_prefix('<')?
    };
    let name_len = after_bracket
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(after_bracket.len());
    let qname = &after_bracket[..name_len];
    if qname.is_empty() || qname.starts_with('?') || qname.starts_with('!') {
        return None;
    }
    let local = qname.rsplit(':').next().unwrap_or(qname);
    let consumed = tail.len() - after_bracket.len() + name_len;
    Some((local, consumed))
}

/// Collect every text element's content range in document order.
fn text_spans(xml: &str) -> Vec<TextSpan> {
    let bytes = xml.as_bytes();
    let mut spans = Vec::new();
    let mut at = 0usize;
    while let Some(open) = xml[at..].find('<') {
        let tag_start = at + open;
        let Some((local, consumed)) = tag_local_name(&xml[tag_start..]) else {
            at = tag_start + 1;
            continue;
        };
        // Only an OPEN `<…:t>` starts a span; a close tag would otherwise be read as one and
        // swallow every run boundary up to the next text element.
        if local != "t" || xml[tag_start..].starts_with("</") {
            at = tag_start + consumed;
            continue;
        }
        let Some(gt) = xml[tag_start..].find('>') else {
            break;
        };
        let content_start = tag_start + gt + 1;
        // A self-closing `<hp:t/>` holds no text.
        if bytes.get(content_start - 2) == Some(&b'/') {
            at = content_start;
            continue;
        }
        let Some(close) = find_close_tag(xml, content_start, "t") else {
            break;
        };
        spans.push(TextSpan {
            content: content_start..close,
        });
        at = close;
    }
    spans
}

/// Byte offset of the `<` starting the next close tag whose local name is `local`.
fn find_close_tag(xml: &str, from: usize, local: &str) -> Option<usize> {
    let mut at = from;
    while let Some(open) = xml[at..].find("</") {
        let tag_start = at + open;
        match tag_local_name(&xml[tag_start..]) {
            Some((name, consumed)) => {
                if name == local {
                    return Some(tag_start);
                }
                at = tag_start + consumed;
            }
            None => at = tag_start + 2,
        }
    }
    None
}

/// True when nothing but run boundaries separates two text elements, i.e. they belong to the
/// same paragraph and a placeholder may legitimately span them. Any other element between them
/// — a paragraph, a table, a line break — ends the group, matching how the IR's placeholder
/// scanner breaks its segment at every non-text character.
fn only_run_boundaries(gap: &str) -> bool {
    let mut rest = gap;
    while let Some(open) = rest.find('<') {
        let tail = &rest[open..];
        let Some((local, _)) = tag_local_name(tail) else {
            return false;
        };
        if local != "t" && local != "run" {
            return false;
        }
        let Some(gt) = tail.find('>') else {
            return false;
        };
        rest = &tail[gt + 1..];
    }
    true
}

/// Rewrite `{{name}}` placeholders that inline formatting split across text runs so the whole
/// placeholder sits in the first run, where the plain string replace below can see it.
///
/// `hwp slots` reads the joined paragraph text, so it has always reported such a placeholder;
/// the raw-XML replace could not fill it, and the two commands disagreed (#145). Formatting
/// inside a slot name is the usual cause — `{{이*름*}}` compiles to `{{이` / `름` / `}}` in
/// three runs.
///
/// The coalesced placeholder — and so the value that replaces it — inherits the first run's
/// character shape. A placeholder split by formatting has no single shape of its own, and the
/// run its opening `{{` sits in is the only defensible pick.
///
/// Only requested names are touched, so a document with no split placeholder among them keeps
/// every byte it had.
fn coalesce_split_placeholders(xml: &mut String, names: &[&str]) {
    if names.is_empty() || !xml.contains("{{") {
        return;
    }
    let spans = text_spans(xml);
    if spans.len() < 2 {
        return;
    }

    // (start, end, replacement) edits, applied back-to-front so earlier offsets stay valid.
    let mut edits: Vec<(usize, usize, String)> = Vec::new();

    let mut group_start = 0usize;
    for boundary in 1..=spans.len() {
        let ends_group = boundary == spans.len()
            || !only_run_boundaries(
                &xml[spans[boundary - 1].content.end..spans[boundary].content.start],
            );
        if !ends_group {
            continue;
        }
        let group = &spans[group_start..boundary];
        group_start = boundary;
        if group.len() < 2 {
            continue;
        }

        // Joined text of the group, plus where each span starts inside it. Recording the span
        // boundaries rather than one entry per byte keeps this O(runs), not O(text length):
        // package policy allows a 64 MiB section, and a per-byte map of it would be gigabytes.
        let mut joined = String::new();
        let mut span_starts: Vec<usize> = Vec::with_capacity(group.len());
        for span in group {
            span_starts.push(joined.len());
            joined.push_str(&xml[span.content.clone()]);
        }
        if !joined.contains("{{") {
            continue;
        }
        // joined offset -> (index into `group`, offset inside that span's text)
        let locate = |offset: usize| -> (usize, usize) {
            let index = span_starts.partition_point(|start| *start <= offset) - 1;
            (index, offset - span_starts[index])
        };

        for token in hwp_model::slot_tokens(&joined) {
            if !names.contains(&xml_slot_name(token.name).as_ref()) {
                continue;
            }
            let needle = &joined[token.range.clone()];
            let (start, end) = (token.range.start, token.range.end);
            let (first_span, first_local) = locate(start);
            let (last_span, last_local) = locate(end - 1);
            if first_span == last_span {
                continue; // already whole in one run — the plain replace handles it
            }
            // First run keeps its prefix and gains the whole placeholder.
            let first = &group[first_span];
            edits.push((
                first.content.start + first_local,
                first.content.end,
                needle.to_string(),
            ));
            // Middle runs contributed nothing but placeholder text.
            for span in &group[first_span + 1..last_span] {
                edits.push((span.content.start, span.content.end, String::new()));
            }
            // Last run keeps whatever followed the placeholder.
            let last = &group[last_span];
            // `last_local` indexes the last span's own text, not the joined string.
            let last_text = &xml[last.content.clone()];
            let last_char_len = last_text[last_local..].chars().next().unwrap().len_utf8();
            edits.push((
                last.content.start,
                last.content.start + last_local + last_char_len,
                String::new(),
            ));
        }
    }

    edits.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    for (start, end, replacement) in edits {
        xml.replace_range(start..end, &replacement);
    }
}

/// Package-surgical TemplateSpec fill result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateFillCounts {
    pub placeholders: BTreeMap<String, usize>,
    pub fields: BTreeMap<String, usize>,
}

/// Fill `{{ name }}` placeholders ([`hwp_model::slot_tokens`]) and one unambiguous simple text field per
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
    let needles = escaped_slots(placeholders)?;
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
    needles: &SlotValues<'_>,
    counts: &mut BTreeMap<String, usize>,
) -> Result<()> {
    if !inside_text {
        reject_placeholder_outside_text(text, needles)?;
        output.push_str(text);
        return Ok(());
    }
    output.push_str(&replace_slots(text, needles, counts));
    Ok(())
}

fn reject_placeholder_outside_text(text: &str, needles: &SlotValues<'_>) -> Result<()> {
    if hwp_model::slot_tokens(text)
        .iter()
        .any(|token| needles.contains_key(xml_slot_name(token.name).as_ref()))
    {
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
            Ok(Event::Start(event)) if event.name().as_ref().as_bytes() == b"root" => {}
            Ok(Event::End(event)) if event.name().as_ref().as_bytes() == b"root" => {}
            Ok(Event::Start(event)) if event.name().as_ref().as_bytes() == b"hp:t" && !in_text => {
                in_text = true;
            }
            Ok(Event::End(event)) if event.name().as_ref().as_bytes() == b"hp:t" && in_text => {
                in_text = false;
            }
            Ok(Event::Empty(event)) if event.name().as_ref().as_bytes() == b"hp:t" && !in_text => {}
            Ok(Event::Empty(event))
                if event.name().as_ref().as_bytes() == b"hp:lineBreak" && in_text => {}
            Ok(Event::Text(text)) => {
                let raw = text.as_bytes();
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

/// Replaces (from, to) pairs in `Contents/section*.xml` in order, copying other entries unchanged.
/// Replaces `Preview/PrvText.txt` as plain text (UTF-8, falling back to UTF-16LE).
///
/// Replacements are sequential: an earlier result may match a later from value, so callers must put
/// longer names first (for example, a full university name before its abbreviation). Returns entry name
/// (section*.xml, PrvText.txt) -> replacement count, including zero.
pub fn replace_texts(
    input: &Path,
    output: &Path,
    replacements: &[(String, String)],
) -> Result<BTreeMap<String, usize>> {
    replace_texts_with_limits(input, output, replacements, &PackageLimits::default())
}

/// Replaces text with caller-specified package limits.
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

/// Converts a UTF-8 decode failure into an I/O error.
fn invalid_data(e: std::string::FromUtf8Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e)
}

/// Content entries to serialize again during surgical rewrites. Every entry not listed here (BinData,
/// META-INF, Preview, DocOptions, unselected sections, etc.) is raw-copied byte-for-byte from the
/// original package.
#[derive(Debug, Clone)]
pub struct DirtyEntries {
    /// Indices of `Contents/section{i}.xml` to reserialize; None means all sections.
    pub sections: Option<Vec<usize>>,
    /// Whether to regenerate `Contents/header.xml`.
    pub header: bool,
    /// Whether to regenerate `Contents/content.hpf`. Adding a BinData entry forces this on to update the
    /// manifest.
    pub content_hpf: bool,
}

impl DirtyEntries {
    /// Reserializes all content entries.
    pub fn all() -> Self {
        Self {
            sections: None,
            header: true,
            content_hpf: true,
        }
    }
}

/// Applies an edited IR surgically to the original HWPX package: reserialize only content entries marked
/// `dirty`, preserving the remaining entries byte-for-byte via raw copy. Append newly introduced
/// binaries as `BinData/*` entries and update content.hpf, regenerating it regardless of dirty when new
/// binaries exist.
///
/// Seed [`BinCollector`] with original OPF manifest IDs so binaryItemIDRef in reserialized sections
/// retains the original id/href.
pub fn rewrite_document_staged(
    input: &Path,
    output: &Path,
    doc: &hwp_model::Document,
    dirty: &DirtyEntries,
    limits: &PackageLimits,
) -> Result<WriteReport> {
    // Read the original package entry list and OPF manifest (href -> id). In-place edits are safe here
    // because the input file remains unchanged until publication.

    let mut source = HwpxPackage::open_with_limits(input, limits)?;
    let source_entry_names: BTreeSet<String> = source
        .entries()?
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    let manifest_ids = source
        .read_entry("Contents/content.hpf")
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|xml| opf_item_ids(&xml))
        .unwrap_or_default();
    drop(source);

    // Seed only BinData entries actually present in the original package. Streams introduced by the edit
    // have no source entry and are appended later instead.

    let mut bins = BinCollector::default();
    let mut used_ids: BTreeSet<String> = BTreeSet::new();
    for stream in &doc.bin_streams {
        if !stream.name.starts_with("BinData/") || !source_entry_names.contains(&stream.name) {
            continue;
        }
        let base_id = manifest_ids
            .get(&stream.name)
            .cloned()
            .unwrap_or_else(|| bin_stem_id(&stream.name));
        let id = unique_bin_id(base_id, &mut used_ids);
        let (_, mime) = crate::write::section::sniff(&stream.data);
        bins.seed(
            id,
            stream.name.clone(),
            mime.to_string(),
            stream.data.clone(),
        );
    }
    let seed_count = bins.items.len();

    // Reserialize only dirty sections from the IR.
    let section_indices: Vec<usize> = match &dirty.sections {
        Some(indices) => indices.clone(),
        None => (0..doc.sections.len()).collect(),
    };
    let mut report = WriteReport::new();
    let mut replacements: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for &i in &section_indices {
        let Some(section) = doc.sections.get(i) else {
            return Err(HwpxError::PackageIntegrity(format!(
                "dirty 섹션 인덱스가 문서 범위를 벗어납니다: {i}"
            )));
        };
        let xml = crate::write::section::write_section_with_report(
            doc,
            section,
            false, // Hangul recalculates stale line layouts after edits, avoiding security warnings.
            &mut bins,
            &mut report,
        );
        replacements.insert(format!("Contents/section{i}.xml"), xml.into_bytes());
    }

    // Assign new entries to streams absent from the package and not collected during section
    // serialization (unreferenced additions), using write/mod.rs passthrough rules.

    for stream in &doc.bin_streams {
        if stream.name.starts_with("BinData/") && source_entry_names.contains(&stream.name) {
            continue; // Already seeded.
        }
        if bins.items.iter().any(|(.., bytes)| *bytes == stream.data) {
            continue; // Identical bytes already registered.
        }
        let (ext, mime) = crate::write::section::sniff(&stream.data);
        bins.allocate(ext, mime, stream.data.clone());
    }
    // Items registered after seeding are new entries to append to the package.
    let appends: Vec<(String, Vec<u8>)> = bins.items[seed_count..]
        .iter()
        .map(|(_, href, _, bytes)| (href.clone(), bytes.clone()))
        .collect();

    // Regenerate content.hpf from the complete seeded/new manifest when dirty or after adding BinData.
    // Seed IDs are original manifest IDs, retaining the original listings for existing items.

    if dirty.content_hpf || !appends.is_empty() {
        let bin_meta: Vec<(String, String, String)> = bins
            .items
            .iter()
            .map(|(id, href, mime, _)| (id.clone(), href.clone(), mime.clone()))
            .collect();
        let xml = crate::write::templates::content_hpf(
            doc.sections.len(),
            &bin_meta,
            &doc.hwpx_opf_extra_items,
            &doc.metadata,
        );
        replacements.insert("Contents/content.hpf".to_string(), xml.into_bytes());
    }

    if dirty.header {
        let xml =
            crate::write::header::write_header(&doc.header, doc.sections.len().max(1), &mut report);
        replacements.insert("Contents/header.xml".to_string(), xml.into_bytes());
    }

    // Detect replacements absent from the original package, which would otherwise be silently lost.
    let mut consumed: BTreeSet<String> = BTreeSet::new();
    process_package_with_appends(
        input,
        output,
        limits,
        |name| replacements.contains_key(name),
        |name, _data| {
            consumed.insert(name.to_string());
            Ok(replacements.get(name).cloned())
        },
        appends,
    )?;
    if consumed.len() != replacements.len() {
        let missing: Vec<&String> = replacements
            .keys()
            .filter(|name| !consumed.contains(*name))
            .collect();
        return Err(HwpxError::PackageIntegrity(format!(
            "dirty 엔트리가 원본 패키지에 없습니다: {missing:?}"
        )));
    }
    Ok(report)
}

/// Reads the OPF (content.hpf) manifest's href -> id mapping for opf:item. Match seed IDs to the
/// original manifest, keeping reserialized binaryItemIDRef consistent with the raw-copied manifest.
/// Parse failures yield an empty mapping and let callers fall back to filename stems.
fn opf_item_ids(xml: &str) -> BTreeMap<String, String> {
    use quick_xml::events::Event;

    let mut map = BTreeMap::new();
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                if event.name().local_name().as_ref().as_bytes() != b"item" {
                    continue;
                }
                let mut id = None;
                let mut href = None;
                for attr in event.attributes().flatten() {
                    let value = attr.value.into_owned();
                    match attr.key.local_name().as_ref().as_bytes() {
                        b"id" => id = Some(value),
                        b"href" => href = Some(value),
                        _ => {}
                    }
                }
                if let (Some(id), Some(href)) = (id, href) {
                    map.insert(href, id);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    map
}

/// Fallback when no OPF entry exists: derive the item ID from the filename stem in `BinData/<file>`,
/// before its extension. Uses the same stem heuristic as [`hwp_model::Document::resolve_bin`].
fn bin_stem_id(name: &str) -> String {
    let fname = name.rsplit('/').next().unwrap_or(name);
    fname.split('.').next().unwrap_or(fname).to_string()
}

/// Makes an already-used manifest/stem ID unique with `_2`, `_3`, etc. suffixes.
fn unique_bin_id(base: String, used: &mut BTreeSet<String>) -> String {
    if used.insert(base.clone()) {
        return base;
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{base}_{n}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

/// Identifies entries to transform (body section XML).
fn is_section_entry(name: &str) -> bool {
    name.starts_with("Contents/section") && name.ends_with(".xml")
}

/// Shared package traversal: decompress and rewrite selected entries within limits, streaming raw
/// compressed copies for the rest.
///
/// First clone the input once into a private snapshot beside the output. All validation/transformation
/// then uses only the open snapshot handle, so replacing the source path or editing in place through the
/// same path/hardlink cannot change the package after validation.
fn process_package(
    input: &Path,
    output: &Path,
    limits: &PackageLimits,
    should_transform: impl FnMut(&str) -> bool,
    transform: impl FnMut(&str, Vec<u8>) -> Result<Option<Vec<u8>>>,
) -> Result<()> {
    process_package_with_appends(
        input,
        output,
        limits,
        should_transform,
        transform,
        Vec::new(),
    )
}

/// Extends [`process_package`] by appending new entries (`appends`) after the entry loop, used for
/// BinData introduced by surgical edits. Reject names colliding with existing entries to prevent
/// duplicate-entry packages.
fn process_package_with_appends(
    input: &Path,
    output: &Path,
    limits: &PackageLimits,
    mut should_transform: impl FnMut(&str) -> bool,
    mut transform: impl FnMut(&str, Vec<u8>) -> Result<Option<Vec<u8>>>,
    appends: Vec<(String, Vec<u8>)>,
) -> Result<()> {
    let destination = inspect_destination(output)?;
    let (_source_guard, snapshot) = snapshot_input(input, output, limits)?;
    let mut archive = zip::ZipArchive::new(snapshot)?;
    let entries = inspect_archive(&mut archive, limits)?;
    // raw_copy_file does not check CRC. Before creating the output stage, read every snapshot entry to
    // its actual EOF to validate declared size, deflate stream and CRC.

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
    let mut written_names: BTreeSet<String> = BTreeSet::new();

    for (i, info) in entries.into_iter().enumerate() {
        written_names.insert(info.name.clone());
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
                // A rewritten entry must not carry the wall clock. Untouched entries are
                // raw-copied and keep the source package's timestamps, but a transformed one is
                // written fresh, and `zip`'s default options stamp "now" — so two identical
                // edits seconds apart produced different bytes. `hwp edit --style-tables`
                // promises byte stability across re-application (D-08), which that defeats.
                // Same DOS ZIP epoch the compose writer pins (see write/mod.rs).
                let opts = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated)
                    .last_modified_time(zip::DateTime::default());
                zip.start_file(&info.name, opts)?;
                zip.write_all(&new_data)?;
            } else {
                output_total = limits.add_uncompressed_total(output_total, info.size)?;
                let raw = archive.by_index_raw(i)?;
                zip.raw_copy_file(raw)?;
                preserve_raw[i] = true;
            }
        } else {
            // Preserve preview, compatibility parts, BinData, mimetype (STORED), etc. byte-for-byte.
            output_total = limits.add_uncompressed_total(output_total, info.size)?;
            let raw = archive.by_index_raw(i)?;
            // verify_archive_integrity checked this entry in the same private snapshot, including actual
            // decompressed size and CRC. Raw copying preserves the validated compressed bytes and ZIP
            // metadata.

            zip.raw_copy_file(raw)?;
            preserve_raw[i] = true;
        }
    }
    // Append entries introduced by edits, such as new BinData, at the package end.
    for (name, data) in &appends {
        if !written_names.insert(name.clone()) {
            return Err(HwpxError::PackageIntegrity(format!(
                "추가 엔트리 이름이 기존 패키지와 중복됩니다: {name}"
            )));
        }
        limits.check_entry(name, data.len() as u64, None)?;
        output_total = limits.add_uncompressed_total(output_total, data.len() as u64)?;
        // A freshly appended entry must not carry the wall clock either — same reasoning as the
        // transformed-entry branch above (D-08 byte-stability): two identical edits seconds
        // apart must not produce different bytes just because a new BinData entry was appended.
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default());
        zip.start_file(name, opts)?;
        zip.write_all(data)?;
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
    // Windows FlushFileBuffers requires write permission; read-only handles yield ERROR_ACCESS_DENIED.
    // Unix permits fsync on read-only descriptors.

    OpenOptions::new()
        .write(true)
        .open(staged_guard.path())
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
    // generated contains the source entries in the same order, followed by zero or more additions.
    if generated.entries.len() < source.entries.len()
        || source.entries.len() != preserve_raw.len()
        || generated.entries.len() > usize::from(u16::MAX)
        || comment.len() > usize::from(u16::MAX)
    {
        return Err(HwpxError::PackageIntegrity(
            "ZIP raw metadata 재조립 범위가 올바르지 않습니다".to_string(),
        ));
    }

    let (_guard, mut rebuilt) = SiblingTemp::create(generated_path, "rebuild")?;
    let mut source_file = File::open(source_path)?;
    let mut generated_file = File::open(generated_path)?;
    let mut new_offsets = Vec::with_capacity(generated.entries.len());
    let mut source_entries = source.entries.iter();
    // Additional entries beyond the source range have no raw-preservation requirement; always copy from
    // generated.
    let mut preserved = preserve_raw.iter().copied().chain(std::iter::repeat(false));
    for generated_entry in &generated.entries {
        let offset = rebuilt.stream_position()?;
        if offset > u64::from(u32::MAX) {
            return Err(HwpxError::PackageIntegrity(
                "ZIP64 local offset은 exact raw 보존 범위를 벗어납니다".to_string(),
            ));
        }
        new_offsets.push(offset as u32);
        let (reader, range): (&mut File, &Range<u64>) =
            match (source_entries.next(), preserved.next().unwrap_or(false)) {
                (Some(source_entry), true) => (&mut source_file, &source_entry.local_range),
                _ => (&mut generated_file, &generated_entry.local_range),
            };
        copy_range(reader, &mut rebuilt, range)?;
    }

    let central_start = rebuilt.stream_position()?;
    if central_start > u64::from(u32::MAX) {
        return Err(HwpxError::PackageIntegrity(
            "ZIP64 central offset은 exact raw 보존 범위를 벗어납니다".to_string(),
        ));
    }
    let mut source_entries = source.entries.iter();
    let mut preserved = preserve_raw.iter().copied().chain(std::iter::repeat(false));
    for (generated_entry, new_offset) in generated.entries.iter().zip(&new_offsets) {
        // Entries within the source range must match generated in order and name, one to one. Later
        // entries are new additions with no corresponding source record.

        let source_entry = source_entries.next();
        if let Some(source_entry) = source_entry
            && central_name(&source_entry.central_record)?
                != central_name(&generated_entry.central_record)?
        {
            return Err(HwpxError::PackageIntegrity(
                "ZIP raw metadata 엔트리 이름이 일치하지 않습니다".to_string(),
            ));
        }
        let original = match (source_entry, preserved.next().unwrap_or(false)) {
            (Some(source_entry), true) => &source_entry.central_record,
            _ => &generated_entry.central_record,
        };
        if u32::from_le_bytes(original[42..46].try_into().expect("fixed central header"))
            == u32::MAX
        {
            return Err(HwpxError::PackageIntegrity(
                "ZIP64 central record는 exact raw 보존 범위를 벗어납니다".to_string(),
            ));
        }
        let mut record = original.clone();
        record[42..46].copy_from_slice(&new_offset.to_le_bytes());
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
    let entries = generated.entries.len() as u16;
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

/// Create a bounded snapshot from one open source handle and never reopen the source path. EOCD
/// preflight both before and after copying detects truncation/appending or same-size package replacement
/// during the snapshot's own validation.
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

/// Validate package integrity and core OWPML semantics before publishing the final stage, retaining
/// caller-specified limits throughout.
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

/// File identity. Unknown is unequal even to itself: unreadable identity cannot prove the file stayed
/// unchanged, so recheck must fail closed. Eq is intentionally not implemented because reflexivity does
/// not hold.
#[derive(Clone, Copy, Debug)]
enum FileIdentity {
    Known {
        first: u64,
        second: u64,
    },
    /// Only produced when Windows cannot open a handle; other platforms always construct an identity
    /// from metadata.
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

// Volume serial and file index are available only through handles (std Metadata exposes them only
// through nightly windows_by_handle). Open without following links, so replacement with a symlink after
// validation changes identity and fails recheck.

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
    // SAFETY: the live File owns the handle for this scope, and the output structure has the same
    // repr(C) layout as Win32 BY_HANDLE_FILE_INFORMATION.

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

/// Annotates I/O errors with the operation name so stages remain identifiable even if callers such as
/// MCP flatten the error chain.
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
    // SAFETY: both paths are NUL-terminated UTF-16 buffers alive throughout the call. The sibling stage
    // is on the same volume, so MoveFileExW replaces atomically.

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
        // Returning directory-fsync failures after publication would report failure with an
        // already-changed destination. Durability strengthening is therefore best effort.

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

/// Replaces (from, to) pairs in order, escaping XML text first when escape is true. Returns (result
/// string, total replacement count).
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

/// Replaces plain text in PrvText.txt, preferring UTF-8; on failure decode as UTF-16LE and return the
/// same encoding. Return Some even for zero replacements to report the entry's presence.
fn replace_in_prvtext(data: Vec<u8>, replacements: &[(String, String)]) -> (Vec<u8>, usize) {
    if let Ok(text) = String::from_utf8(data.clone()) {
        let (text, n) = replace_seq(text, replacements, false);
        return (text.into_bytes(), n);
    }
    // Lossy UTF-16LE fallback; replacement names are in the valid character range.
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

#[cfg(test)]
mod coalesce_tests {
    use super::*;

    const SPLIT: &str = r#"<hp:p id="2"><hp:run charPrIDRef="0"><hp:t xml:space="preserve">이름: {{이</hp:t></hp:run><hp:run charPrIDRef="2"><hp:t xml:space="preserve">름</hp:t></hp:run><hp:run charPrIDRef="0"><hp:t xml:space="preserve">}} 입니다.</hp:t></hp:run></hp:p>"#;

    /// Three runs, the middle one holding the split character.
    #[test]
    fn coalesces_a_placeholder_split_by_emphasis() {
        let mut xml = SPLIT.to_string();
        coalesce_split_placeholders(&mut xml, &["이름"]);
        assert!(xml.contains("{{이름}}"), "placeholder still split:\n{xml}");
        assert!(
            xml.contains("이름: {{이름}}") && xml.contains(" 입니다."),
            "surrounding text must survive intact:\n{xml}"
        );
    }

    /// The placeholder inherits the run its opening `{{` sat in, not the run that split it.
    #[test]
    fn coalesced_placeholder_keeps_the_first_run_shape() {
        let mut xml = SPLIT.to_string();
        coalesce_split_placeholders(&mut xml, &["이름"]);
        let at = xml.find("{{이름}}").expect("coalesced");
        let run_open = xml[..at].rfind("<hp:run").expect("enclosing run");
        assert!(
            xml[run_open..at].contains(r#"charPrIDRef="0""#),
            "expected the first run's shape, got:\n{}",
            &xml[run_open..at]
        );
    }

    /// A name nobody asked for is left exactly as it was — the byte-preserving guarantee.
    #[test]
    fn leaves_unrequested_placeholders_untouched() {
        let mut xml = SPLIT.to_string();
        coalesce_split_placeholders(&mut xml, &["다른이름"]);
        assert_eq!(xml, SPLIT);
    }

    /// A placeholder already whole in one run needs no rewriting.
    #[test]
    fn leaves_an_unsplit_placeholder_untouched() {
        const WHOLE: &str = r#"<hp:p id="1"><hp:run charPrIDRef="0"><hp:t xml:space="preserve">제목  {{제목}}</hp:t></hp:run></hp:p>"#;
        let mut xml = WHOLE.to_string();
        coalesce_split_placeholders(&mut xml, &["제목"]);
        assert_eq!(xml, WHOLE);
    }

    /// `{{` in one paragraph and `}}` in the next is not a placeholder. Joining across the
    /// paragraph boundary would invent one and corrupt both paragraphs.
    #[test]
    fn does_not_join_across_a_paragraph_boundary() {
        const TWO_PARAS: &str = r#"<hp:p id="1"><hp:run charPrIDRef="0"><hp:t xml:space="preserve">앞 {{이</hp:t></hp:run></hp:p><hp:p id="2"><hp:run charPrIDRef="0"><hp:t xml:space="preserve">름}} 뒤</hp:t></hp:run></hp:p>"#;
        let mut xml = TWO_PARAS.to_string();
        coalesce_split_placeholders(&mut xml, &["이름"]);
        assert_eq!(xml, TWO_PARAS);
    }

    /// A line break inside the paragraph ends the group for the same reason the IR's scanner
    /// breaks its segment at every non-text character.
    #[test]
    fn does_not_join_across_a_line_break() {
        const BROKEN: &str = r#"<hp:p id="1"><hp:run charPrIDRef="0"><hp:t xml:space="preserve">앞 {{이</hp:t></hp:run><hp:run charPrIDRef="0"><hp:lineBreak/></hp:run><hp:run charPrIDRef="0"><hp:t xml:space="preserve">름}} 뒤</hp:t></hp:run></hp:p>"#;
        let mut xml = BROKEN.to_string();
        coalesce_split_placeholders(&mut xml, &["이름"]);
        assert_eq!(xml, BROKEN);
    }

    /// Four runs, with the name split twice and text following in the last run.
    #[test]
    fn coalesces_a_placeholder_split_across_four_runs() {
        const FOUR: &str = r#"<hp:p id="1"><hp:run charPrIDRef="0"><hp:t xml:space="preserve">{{기</hp:t></hp:run><hp:run charPrIDRef="1"><hp:t xml:space="preserve">관</hp:t></hp:run><hp:run charPrIDRef="2"><hp:t xml:space="preserve">명</hp:t></hp:run><hp:run charPrIDRef="0"><hp:t xml:space="preserve">}} 귀하</hp:t></hp:run></hp:p>"#;
        let mut xml = FOUR.to_string();
        coalesce_split_placeholders(&mut xml, &["기관명"]);
        assert!(xml.contains("{{기관명}}"), "not coalesced:\n{xml}");
        assert!(xml.contains(" 귀하"), "trailing text lost:\n{xml}");
    }

    /// A package may bind the paragraph namespace to any prefix, and may put a newline or tab
    /// before a tag's attributes. `read::section` matches XML local names, so `hwp slots` sees
    /// such a document; this pass has to agree or the disagreement #145 closes just moves.
    #[test]
    fn recognises_text_elements_under_another_prefix() {
        const OTHER: &str = "<p:p id=\"1\"><p:run charPrIDRef=\"0\"><p:t\n  xml:space=\"preserve\">이름: {{이</p:t></p:run><p:run charPrIDRef=\"2\"><p:t xml:space=\"preserve\">름</p:t></p:run><p:run charPrIDRef=\"0\"><p:t xml:space=\"preserve\">}} 입니다.</p:t></p:run></p:p>";
        let mut xml = OTHER.to_string();
        coalesce_split_placeholders(&mut xml, &["이름"]);
        assert!(
            xml.contains("{{이름}}"),
            "prefix-independent match failed:\n{xml}"
        );
        assert!(xml.contains(" 입니다."), "trailing text lost:\n{xml}");
    }

    /// A megabyte-scale paragraph still coalesces correctly.
    ///
    /// This is a smoke test, not a memory bound: the O(runs) property comes from recording span
    /// starts instead of a per-byte position map, which no assertion here can observe. The first
    /// draft of this pass did keep a `(usize, usize)` per byte, and against the 64 MiB section
    /// the package policy allows that would have reached gigabytes — a real way to end the
    /// process the CLI and the MCP server share. What this test does catch is an offset bug that
    /// only shows up once a span is longer than the text around it.
    #[test]
    fn a_megabyte_paragraph_still_coalesces() {
        let filler = "가".repeat(400_000); // ~1.2 MB of UTF-8 in one run
        let xml_source = format!(
            "<hp:p id=\"1\"><hp:run charPrIDRef=\"0\"><hp:t xml:space=\"preserve\">{filler}{{{{이</hp:t></hp:run><hp:run charPrIDRef=\"1\"><hp:t xml:space=\"preserve\">름}}}} {filler}</hp:t></hp:run></hp:p>"
        );
        let mut xml = xml_source;
        coalesce_split_placeholders(&mut xml, &["이름"]);
        assert!(xml.contains("{{이름}}"), "large paragraph not coalesced");
    }

    /// Two split placeholders in one paragraph, so the edits must not disturb each other.
    #[test]
    fn coalesces_two_split_placeholders_in_one_paragraph() {
        const TWO: &str = r#"<hp:p id="1"><hp:run charPrIDRef="0"><hp:t xml:space="preserve">{{가</hp:t></hp:run><hp:run charPrIDRef="1"><hp:t xml:space="preserve">나}} 그리고 {{다</hp:t></hp:run><hp:run charPrIDRef="0"><hp:t xml:space="preserve">라}} 끝</hp:t></hp:run></hp:p>"#;
        let mut xml = TWO.to_string();
        coalesce_split_placeholders(&mut xml, &["가나", "다라"]);
        assert!(xml.contains("{{가나}}"), "first not coalesced:\n{xml}");
        assert!(xml.contains("{{다라}}"), "second not coalesced:\n{xml}");
        assert!(
            xml.contains(" 그리고 ") && xml.contains(" 끝"),
            "text lost:\n{xml}"
        );
    }

    /// A padded placeholder split across runs is coalesced with its padding, so the replace
    /// that follows sees the same token `hwp slots` reported (#362).
    #[test]
    fn coalesces_a_padded_placeholder_split_by_emphasis() {
        const PADDED: &str = r#"<hp:p id="1"><hp:run charPrIDRef="0"><hp:t xml:space="preserve">이름: {{ 이</hp:t></hp:run><hp:run charPrIDRef="2"><hp:t xml:space="preserve">름</hp:t></hp:run><hp:run charPrIDRef="0"><hp:t xml:space="preserve"> }} 입니다.</hp:t></hp:run></hp:p>"#;
        let mut xml = PADDED.to_string();
        coalesce_split_placeholders(&mut xml, &["이름"]);
        assert!(xml.contains("이름: {{ 이름 }}"), "not coalesced:\n{xml}");
        let mut counts = BTreeMap::new();
        let values = BTreeMap::from([("이름".to_string(), "홍길동".to_string())]);
        let filled = replace_slots(&xml, &escaped_slots(&values).unwrap(), &mut counts);
        assert!(filled.contains("이름: 홍길동") && filled.contains(" 입니다."));
        assert_eq!(counts["이름"], 1);
    }

    /// Every spelling of a name is one slot, and a value that looks like a slot is not
    /// filled again by a later name.
    #[test]
    fn replace_slots_fills_every_spelling_in_one_pass() {
        let values = BTreeMap::from([
            ("a".to_string(), "{{b}}".to_string()),
            ("b".to_string(), "B".to_string()),
        ]);
        let mut counts = BTreeMap::new();
        let slots = escaped_slots(&values).unwrap();
        let out = replace_slots("{{a}} {{ a }} {{b}} {{c}}", &slots, &mut counts);
        assert_eq!(out, "{{b}} {{b}} B {{c}}");
        assert_eq!(counts["a"], 2);
        assert_eq!(counts["b"], 1);
    }

    /// The TemplateSpec path fills a padded slot in text and still refuses one in markup.
    #[test]
    fn template_placeholders_use_the_slot_grammar() {
        let values = BTreeMap::from([("기관명".to_string(), "A&B".to_string())]);
        let text = r#"<hp:p xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph"><hp:run><hp:t>{{ 기관명 }} 귀하</hp:t></hp:run></hp:p>"#;
        let mut counts = BTreeMap::new();
        let out = replace_text_placeholders(text, &values, &mut counts).unwrap();
        assert!(out.contains("<hp:t>A&amp;B 귀하</hp:t>"), "{out}");
        assert_eq!(counts["기관명"], 1);

        let markup = r#"<hp:p xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" name="{{ 기관명 }}"><hp:run><hp:t>x</hp:t></hp:run></hp:p>"#;
        let error = replace_text_placeholders(markup, &values, &mut BTreeMap::new()).unwrap_err();
        assert!(error.to_string().contains("outside a text node"), "{error}");
    }

    /// Names are any text without braces or controls, requested keys are trimmed like names,
    /// and counts stay under the key as the caller spelled it (#362).
    #[test]
    fn wide_names_and_padded_keys_fill_and_count_under_the_callers_key() {
        let values = BTreeMap::from([
            (" 사업명(국문) ".to_string(), "A".to_string()),
            ("성 명".to_string(), "B".to_string()),
            ("기간: 시작".to_string(), "C".to_string()),
        ]);
        let slots = escaped_slots(&values).unwrap();
        let mut counts = BTreeMap::new();
        let out = replace_slots(
            "{{사업명(국문)}} {{ 성 명 }} {{기간: 시작}} {{a/b}}",
            &slots,
            &mut counts,
        );
        assert_eq!(out, "A B C {{a/b}}");
        assert_eq!(counts[" 사업명(국문) "], 1);
        assert_eq!(counts["성 명"], 1);
        let same = BTreeMap::from([
            ("a".to_string(), "1".to_string()),
            (" a".to_string(), "1".to_string()),
        ]);
        let mut counts = BTreeMap::new();
        replace_slots("{{a}}", &escaped_slots(&same).unwrap(), &mut counts);
        assert_eq!(
            (counts["a"], counts[" a"]),
            (1, 1),
            "both keys name the slot"
        );
        let clash = BTreeMap::from([
            ("a".to_string(), "1".to_string()),
            (" a".to_string(), "2".to_string()),
        ]);
        assert!(escaped_slots(&clash).is_err());
    }

    /// Raw-XML names are matched the way the IR reads them: entity and character references
    /// decoded (#363 review), in the fill, the split-run coalescing, the TemplateSpec text fill
    /// and its outside-text refusal.
    #[test]
    fn escaped_names_match_their_decoded_spelling() {
        let name = "R&D <1> \"b\" 가";
        let values = BTreeMap::from([(name.to_string(), "X&Y".to_string())]);
        let slots = escaped_slots(&values).unwrap();

        let mut counts = BTreeMap::new();
        let raw = "{{R&amp;D &lt;1&gt; &quot;b&quot; &#44032;}} 끝";
        assert_eq!(replace_slots(raw, &slots, &mut counts), "X&amp;Y 끝");
        assert_eq!(counts[name], 1);

        const SPLIT: &str = r#"<hp:p id="1"><hp:run charPrIDRef="0"><hp:t xml:space="preserve">{{R&amp;D &lt;1</hp:t></hp:run><hp:run charPrIDRef="1"><hp:t xml:space="preserve">&gt; &quot;b&quot; &#xAC00;}}</hp:t></hp:run></hp:p>"#;
        let mut xml = SPLIT.to_string();
        coalesce_split_placeholders(&mut xml, &[name]);
        let mut counts = BTreeMap::new();
        let filled = replace_slots(&xml, &slots, &mut counts);
        assert!(filled.contains("X&amp;Y"), "{filled}");
        assert_eq!(counts[name], 1);

        let text = r#"<hp:p xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph"><hp:run><hp:t>{{R&amp;D &lt;1&gt; &quot;b&quot; &#44032;}}</hp:t></hp:run></hp:p>"#;
        let mut counts = BTreeMap::new();
        let out = replace_text_placeholders(text, &values, &mut counts).unwrap();
        assert!(out.contains("<hp:t>X&amp;Y</hp:t>"), "{out}");
        let markup = r#"<hp:p xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" name="{{R&amp;D &lt;1&gt; &quot;b&quot; &#44032;}}"><hp:run><hp:t>x</hp:t></hp:run></hp:p>"#;
        let error = replace_text_placeholders(markup, &values, &mut BTreeMap::new()).unwrap_err();
        assert!(error.to_string().contains("outside a text node"), "{error}");
    }
}
