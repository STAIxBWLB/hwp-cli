//! Korean public-form fields: discovery for `hwp slots --forms` and fill for `hwp fill --forms`.
//!
//! Ported from kordoc (MIT, Copyright (c) 2026 chrisryugj, <https://github.com/chrisryugj/kordoc>)
//! by way of Maru's `kordoc_lite`; see `NOTICE`. The label keywords, patterns, confidences and
//! merge rules are kordoc's. What they run on differs: kordoc reads one raw XML text node at a
//! time, this reads IR paragraph text, so a label or slot that formatting split across runs is
//! still found; and a fill splices only the blank, so char shapes and the rest of the cell stay.
//! Value cells come from the table walk `hwp edit --set-cell-by-label` uses
//! ([`walk_form_cells`]), with the target policy described at [`fill_form_fields`].

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::LazyLock;

use hwp_model::{Cell, Control, Document, Paragraph};
use regex::Regex;

use crate::edit::{FormCellCandidate, FormTarget, cell_text, splice_edits, walk_form_cells};
use crate::field::text_segments;

/// Where a form field was seen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormFieldSource {
    /// A `{{ name }}` slot.
    Placeholder,
    /// `라벨: 값` in a paragraph outside table cells.
    InlineLabel,
    /// A table cell that reads as a form label.
    FormLabel,
}

impl FormFieldSource {
    /// The name kordoc reports the source under.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Placeholder => "placeholder",
            Self::InlineLabel => "inlineLabel",
            Self::FormLabel => "formLabel",
        }
    }

    /// kordoc's confidence for a field of this source.
    pub fn confidence(self) -> f64 {
        match self {
            Self::Placeholder => 1.0,
            Self::InlineLabel => 0.64,
            Self::FormLabel => 0.72,
        }
    }

    /// Merge rank: a sighting of a higher rank takes over a field's label and source.
    fn rank(self) -> u8 {
        match self {
            Self::Placeholder => 1,
            Self::InlineLabel => 2,
            Self::FormLabel => 3,
        }
    }
}

/// A fillable field, merged over every sighting of its key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormField {
    /// [`normalize_label`] of the label; fill values are matched against it.
    pub key: String,
    /// The label as written, trailing colons and whitespace trimmed.
    pub label: String,
    /// The highest-ranked source the key was seen as.
    pub source: FormFieldSource,
    /// Sightings over all sources.
    pub occurrences: usize,
    /// The key is also a `{{ }}` slot, which a fill should not leave behind.
    pub required: bool,
}

/// List the form fields of `doc`: `{{ }}` slots, inline `라벨: 값` outside table cells, and
/// table cells that read as labels. Sorted by key.
pub fn scan_form_fields(doc: &Document) -> Vec<FormField> {
    let mut paragraphs = Vec::new();
    let mut cells = Vec::new();
    for section in &doc.sections {
        for para in &section.paragraphs {
            collect(para, false, &mut paragraphs, &mut cells);
        }
    }
    let mut fields = BTreeMap::new();
    // Text before cells, as kordoc scans: among sightings of one rank the first label stays.
    for (para, in_cell) in paragraphs {
        for (_, seg) in text_segments(para) {
            for token in hwp_model::slot_tokens(&seg) {
                merge(&mut fields, token.name, FormFieldSource::Placeholder);
            }
            if !in_cell {
                for inline in inline_labels(&seg) {
                    merge(
                        &mut fields,
                        &seg[inline.label],
                        FormFieldSource::InlineLabel,
                    );
                }
            }
        }
    }
    for cell in cells {
        let text = cell_text(cell);
        if is_label_cell(&text) {
            merge(&mut fields, &trim_label(&text), FormFieldSource::FormLabel);
        }
    }
    fields.into_values().collect()
}

fn collect<'a>(
    para: &'a Paragraph,
    in_cell: bool,
    paragraphs: &mut Vec<(&'a Paragraph, bool)>,
    cells: &mut Vec<&'a Cell>,
) {
    paragraphs.push((para, in_cell));
    for ctrl in &para.controls {
        match ctrl {
            Control::Table(table) => {
                for cell in &table.cells {
                    cells.push(cell);
                    for p in &cell.paragraphs {
                        collect(p, true, paragraphs, cells);
                    }
                }
            }
            Control::Generic(generic) => {
                for list in &generic.paragraph_lists {
                    for p in &list.paragraphs {
                        collect(p, in_cell, paragraphs, cells);
                    }
                }
            }
            _ => {}
        }
    }
}

fn merge(fields: &mut BTreeMap<String, FormField>, label: &str, source: FormFieldSource) {
    let key = normalize_label(label);
    if key.is_empty() {
        return;
    }
    let field = fields.entry(key.clone()).or_insert_with(|| FormField {
        key,
        label: trim_label(label),
        source,
        occurrences: 0,
        required: false,
    });
    field.occurrences += 1;
    if source.rank() > field.source.rank() {
        field.label = trim_label(label);
        field.source = source;
    }
    field.required |= source == FormFieldSource::Placeholder;
}

/// What [`fill_form_fields`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormFill {
    /// Fills per key as the caller spelled it. A key that normalizes to nothing is left out,
    /// as kordoc drops it.
    pub counts: BTreeMap<String, usize>,
    /// Keys that matched nothing. A checkbox left unchecked by a falsy value did match.
    pub unmatched: Vec<String>,
    /// Content the fill overwrote or left alone, one line each.
    pub warnings: Vec<String>,
}

/// Fill the slots and form fields of `doc` from `values` in one pass.
///
/// Keys are compared after [`normalize_label`], and two keys that normalize alike are refused.
/// Every edit reads the unfilled text, so a value is never rewritten by a later step:
///
/// 1. The value cell of every label cell whose normalized text matches a key (kordoc's
///    `find_matching_key`) is resolved: the adjacent cell when it is blank (empty or one slot),
///    else the cell below a complete header row when that is blank, else the adjacent cell
///    unless it is itself a keyword label. A non-blank cell is overwritten with a warning; a
///    cell holding controls (a field, a picture, a table) is never written, with a warning.
/// 2. Outside table cells, the text after `라벨:` is replaced: up to a comma, semicolon, line
///    end, the next `라벨:` on the line, or 100 characters. Inside cells, `라벨(  )` blanks,
///    `□옵션` checkboxes (checked when the value is truthy; a falsy value leaves the box but
///    counts as matched) and `(라벨:  )` annotations are filled.
/// 3. `{{ name }}` slots, matched by normalized name as in kordoc, in one literal pass
///    ([`crate::replace_slots`]).
/// 4. The value cells from step 1, written with [`crate::set_cell`].
///
/// A slot owns its text: a label whose value text or value cell holds a requested slot is left
/// to the slot, so one place is filled once. When two keys reach one place, the first keeps it
/// and a warning names the other.
pub fn fill_form_fields(
    doc: &mut Document,
    values: &BTreeMap<String, String>,
) -> Result<FormFill, String> {
    let mut lookup: BTreeMap<String, String> = BTreeMap::new();
    let mut spelled: BTreeMap<String, &str> = BTreeMap::new();
    for (name, value) in values {
        let key = normalize_label(name);
        if key.is_empty() {
            continue;
        }
        if let Some(other) = spelled.insert(key.clone(), name) {
            return Err(format!(
                "두 키가 같은 필드를 가리킵니다: {other:?}, {name:?} (공백·쌍점·괄호는 무시하고 비교)"
            ));
        }
        lookup.insert(key, value.clone());
    }
    let mut run = FillRun {
        values: &lookup,
        counts: BTreeMap::new(),
        warnings: Vec::new(),
    };

    // 1. Value cells, on the unfilled document.
    let found = walk_form_cells(doc, None, |text, adjacent, below| {
        if !is_label_cell(text) {
            return None;
        }
        let key = find_matching_key(&normalize_label(text), &lookup)?;
        let blank = |cell: &Cell| !has_controls(cell) && is_blank_value(&cell_text(cell));
        let (target, cell) = match (adjacent, below) {
            (Some(adjacent), _) if blank(adjacent) => (FormTarget::Adjacent, adjacent),
            (_, Some(below)) if blank(below) => (FormTarget::Below, below),
            (Some(adjacent), _) if !is_keyword_label(&cell_text(adjacent)) => {
                (FormTarget::Adjacent, adjacent)
            }
            _ => return None,
        };
        let text = cell_text(cell);
        let target_cell = TargetCell {
            key,
            controls: has_controls(cell),
            blank: is_blank_value(&text),
            slot_keys: requested_slot_keys(&text, &lookup),
            text,
        };
        Some((target_cell, target))
    });
    let mut targets: Vec<(String, FormCellCandidate)> = Vec::new();
    let mut taken: Vec<(FormCellCandidate, String)> = Vec::new();
    for (cell, at) in found {
        let place = format!("표{} ({},{})", at.table, at.row, at.col);
        if let Some((_, owner)) = taken.iter().find(|(candidate, _)| *candidate == at) {
            if *owner != cell.key {
                run.warnings.push(format!(
                    "{place}: 키 {:?}도 이 칸을 가리키지만 먼저 찾은 {owner:?}가 채웁니다",
                    cell.key
                ));
            }
            continue;
        }
        taken.push((at, cell.key.clone()));
        if cell.controls {
            run.warnings.push(format!(
                "{place}: 누름틀·그림·표 같은 개체가 있는 칸이라 {:?} 값을 쓰지 않습니다",
                cell.key
            ));
        } else if !cell.slot_keys.is_empty() {
            // The slot in the cell fills it (step 3).
            if !cell.slot_keys.contains(&cell.key) {
                run.warnings.push(format!(
                    "{place}: 자리표시자가 있는 칸이라 {:?} 값 대신 자리표시자를 채웁니다",
                    cell.key
                ));
            }
        } else {
            if !cell.blank {
                run.warnings.push(format!(
                    "{place}: 기존 내용을 {:?} 값으로 덮어씁니다: {:?}",
                    cell.key,
                    cell.text.trim()
                ));
            }
            targets.push((cell.key, at));
        }
    }

    // 2. Inline labels and cell blanks, on the unfilled text.
    for section in &mut doc.sections {
        for para in &mut section.paragraphs {
            run.fill_text(para, false);
        }
    }

    // 3. Slots, keyed the kordoc way (`{{성 명}}` is key `성명`).
    let slot_values: BTreeMap<String, String> = crate::scan_placeholders(doc)
        .into_iter()
        .filter_map(|slot| {
            let value = lookup.get(&normalize_label(&slot.name))?.clone();
            Some((slot.name, value))
        })
        .collect();
    for (name, count) in crate::replace_slots(doc, &slot_values)? {
        *run.counts.entry(normalize_label(&name)).or_default() += count;
    }

    // 4. Value cells.
    for (key, at) in &targets {
        crate::set_cell(doc, at.table, at.row, at.col, &lookup[key])?;
        *run.counts.entry(key.clone()).or_default() += 1;
    }

    let mut fill = FormFill {
        warnings: run.warnings,
        ..FormFill::default()
    };
    for (key, name) in spelled {
        match run.counts.get(&key) {
            Some(count) => {
                fill.counts.insert(name.to_string(), *count);
            }
            None => {
                fill.counts.insert(name.to_string(), 0);
                fill.unmatched.push(name.to_string());
            }
        }
    }
    Ok(fill)
}

/// A value cell resolved in step 1 of [`fill_form_fields`], read before anything is filled.
struct TargetCell {
    key: String,
    text: String,
    controls: bool,
    blank: bool,
    /// Requested keys of the slots the cell holds.
    slot_keys: Vec<String>,
}

/// Normalized keys of the requested slots in `text`.
fn requested_slot_keys(text: &str, values: &BTreeMap<String, String>) -> Vec<String> {
    hwp_model::slot_tokens(text)
        .into_iter()
        .map(|token| normalize_label(token.name))
        .filter(|key| values.contains_key(key))
        .collect()
}

/// A cell whose paragraphs hold any control (a field, a picture, a table, ...).
fn has_controls(cell: &Cell) -> bool {
    cell.paragraphs.iter().any(|p| !p.controls.is_empty())
}

/// State of one [`fill_form_fields`] run. A key in `counts` matched, even with 0 fills.
struct FillRun<'a> {
    values: &'a BTreeMap<String, String>,
    counts: BTreeMap<String, usize>,
    warnings: Vec<String>,
}

impl FillRun<'_> {
    fn count(&mut self, key: &str, n: usize) {
        *self.counts.entry(key.to_string()).or_default() += n;
    }

    /// Step 2 of [`fill_form_fields`] on `para` and its nested paragraphs. Returns whether
    /// anything changed, so a text box on the way drops its now stale raw XML.
    fn fill_text(&mut self, para: &mut Paragraph, in_cell: bool) -> bool {
        let mut changed = if in_cell {
            // In turn, as kordoc chains them on one cell's text.
            let paren = splice_segments(para, |seg| self.paren_blanks(seg));
            let checkbox = splice_segments(para, |seg| self.checkboxes(seg));
            let annotation = splice_segments(para, |seg| self.annotations(seg));
            paren | checkbox | annotation
        } else {
            splice_segments(para, |seg| self.inline_values(seg))
        };
        for ctrl in &mut para.controls {
            match ctrl {
                Control::Table(table) => {
                    for cell in &mut table.cells {
                        for p in &mut cell.paragraphs {
                            changed |= self.fill_text(p, true);
                        }
                    }
                }
                Control::Generic(generic) => {
                    let mut inner = false;
                    for list in &mut generic.paragraph_lists {
                        for p in &mut list.paragraphs {
                            inner |= self.fill_text(p, in_cell);
                        }
                    }
                    if inner {
                        generic.hwpx_raw_xml = None;
                    }
                    changed |= inner;
                }
                _ => {}
            }
        }
        changed
    }

    /// `라벨: ...` becomes `라벨: 값`. The separator and the old value are spliced apart, so
    /// the value keeps the char shape of the text it replaces, not of the colon.
    fn inline_values(&mut self, seg: &str) -> Vec<(Range<usize>, String)> {
        let mut edits = Vec::new();
        for inline in inline_labels(seg) {
            let key = normalize_label(&seg[inline.label.clone()]);
            let Some(value) = self.values.get(&key) else {
                continue;
            };
            let slot_keys = requested_slot_keys(&seg[inline.value.clone()], self.values);
            if !slot_keys.is_empty() {
                // The slot after the label fills it (step 3).
                if !slot_keys.contains(&key) {
                    self.warnings.push(format!(
                        "\"{}:\" 뒤에 자리표시자가 있어 {key:?} 값 대신 자리표시자를 채웁니다",
                        &seg[inline.label]
                    ));
                }
                continue;
            }
            self.count(&key, 1);
            if inline.value.is_empty() {
                edits.push((inline.separator, format!(": {value}")));
            } else {
                if &seg[inline.separator.clone()] != ": " {
                    edits.push((inline.separator, ": ".to_string()));
                }
                edits.push((inline.value, value.clone()));
            }
        }
        edits
    }

    /// `라벨(  )접미` becomes `라벨(값)접미`, keyed `라벨접미`, else `라벨`.
    fn paren_blanks(&mut self, seg: &str) -> Vec<(Range<usize>, String)> {
        let mut edits = Vec::new();
        for caps in PAREN_BLANK.captures_iter(seg) {
            let (Some(whole), Some(prefix)) = (caps.get(0), caps.get(1)) else {
                continue;
            };
            let suffix = caps.get(2).map_or("", |m| m.as_str());
            let both = normalize_label(&format!("{}{suffix}", prefix.as_str()));
            let key = if self.values.contains_key(&both) {
                both
            } else {
                normalize_label(prefix.as_str())
            };
            let Some(value) = self.values.get(&key) else {
                continue;
            };
            // The blank between `(` and `)`.
            edits.push((
                prefix.end() + 1..whole.end() - suffix.len() - 1,
                value.clone(),
            ));
            self.count(&key, 1);
        }
        edits
    }

    /// `□옵션` becomes `☑옵션` when the value is truthy.
    fn checkboxes(&mut self, seg: &str) -> Vec<(Range<usize>, String)> {
        let mut edits = Vec::new();
        for caps in CHECKBOX.captures_iter(seg) {
            let Some(whole) = caps.get(0) else {
                continue;
            };
            let key = normalize_label(&caps[1]);
            let Some(value) = self.values.get(&key) else {
                continue;
            };
            if is_truthy_checkbox(value) {
                edits.push((
                    whole.start()..whole.start() + '□'.len_utf8(),
                    "☑".to_string(),
                ));
                self.count(&key, 1);
            } else {
                // Found and deliberately left unchecked: matched, not missing.
                self.count(&key, 0);
                self.warnings.push(format!(
                    "{}: 값 {value:?}이 참이 아니라 체크하지 않습니다",
                    whole.as_str()
                ));
            }
        }
        edits
    }

    /// `(라벨:  )` becomes `(라벨: 값)`.
    fn annotations(&mut self, seg: &str) -> Vec<(Range<usize>, String)> {
        let mut edits = Vec::new();
        for caps in ANNOTATION_BLANK.captures_iter(seg) {
            let (Some(whole), Some(label)) = (caps.get(0), caps.get(1)) else {
                continue;
            };
            let key = normalize_label(label.as_str());
            let Some(value) = self.values.get(&key) else {
                continue;
            };
            edits.push((label.end()..whole.end() - 1, format!(": {value}")));
            self.count(&key, 1);
        }
        edits
    }
}

/// One inline `라벨: 값` in a text segment, as byte ranges.
struct InlineLabel {
    label: Range<usize>,
    /// The colon and the whitespace around it.
    separator: Range<usize>,
    /// kordoc's value: up to a comma, semicolon, line end or 100 characters. It also stops at
    /// the next `라벨:` on the line, so filling one label never deletes the next (#365 review).
    value: Range<usize>,
}

fn inline_labels(seg: &str) -> Vec<InlineLabel> {
    let slots = hwp_model::slot_tokens(seg);
    let mut labels = Vec::new();
    let mut at = 0usize;
    while let Some(caps) = INLINE_HEAD.captures_at(seg, at) {
        let (Some(whole), Some(label)) = (caps.get(0), caps.get(1)) else {
            break;
        };
        // `라벨:` inside a slot name (`{{기간: 시작}}`) is part of the slot.
        if slots.iter().any(|slot| slot.range.contains(&label.start())) {
            at = whole.end();
            continue;
        }
        let start = whole.end();
        let mut end = seg[start..]
            .char_indices()
            .take(100)
            .take_while(|(_, c)| !matches!(c, '\n' | ',' | ';'))
            .last()
            .map_or(start, |(i, c)| start + i + c.len_utf8());
        if let Some(next) = NEXT_LABEL
            .find_at(seg, start)
            .filter(|next| next.start() < end)
        {
            end = start + seg[start..next.start()].trim_end().len();
        }
        // A slot later in the value is a field of its own. One right after the label is the
        // label's value, which the slot fills.
        if let Some(slot) = slots
            .iter()
            .find(|slot| slot.range.start > start && slot.range.start < end)
        {
            end = start + seg[start..slot.range.start].trim_end().len();
        }
        labels.push(InlineLabel {
            label: label.range(),
            separator: label.end()..start,
            value: start..end,
        });
        at = end.max(whole.end());
    }
    labels
}

/// Splice, in each text segment of `para`, the edits `find` returns for it: byte ranges of
/// the segment, in order and not overlapping, with their replacements. Returns whether it
/// spliced anything.
fn splice_segments(
    para: &mut Paragraph,
    mut find: impl FnMut(&str) -> Vec<(Range<usize>, String)>,
) -> bool {
    let mut edits = Vec::new();
    for (seg_start, seg) in text_segments(para) {
        for (range, replacement) in find(&seg) {
            let start = seg_start + seg[..range.start].chars().count();
            let end = start + seg[range].chars().count();
            edits.push((start..end, replacement));
        }
    }
    let changed = !edits.is_empty();
    splice_edits(para, edits);
    changed
}

/// A value cell a fill may write without overwriting content: empty, or one slot.
fn is_blank_value(text: &str) -> bool {
    let text = text.trim();
    let tokens = hwp_model::slot_tokens(text);
    text.is_empty() || (tokens.len() == 1 && tokens[0].range == (0..text.len()))
}

/// kordoc's key normalization: whitespace, colons, parentheses and `·` removed.
pub fn normalize_label(label: &str) -> String {
    label.trim().replace(
        [':', '：', ' ', '\t', '\n', '\r', '(', ')', '（', '）', '·'],
        "",
    )
}

fn trim_label(label: &str) -> String {
    label
        .trim()
        .trim_end_matches([':', '：', ' ', '\t', '\n', '\r'])
        .to_string()
}

/// Strip surrounding whitespace and trailing footnote marks, the way kordoc reads a cell.
fn cell_label(text: &str) -> &str {
    text.trim()
        .trim_end_matches(|ch: char| "¹²³⁴⁵⁶⁷⁸⁹⁰*※".contains(ch))
        .trim()
}

/// kordoc's label-cell test: short, and a known keyword, a few Hangul syllables, or `라벨:`.
fn is_label_cell(text: &str) -> bool {
    let trimmed = cell_label(text);
    if trimmed.is_empty() || trimmed.chars().count() > 30 {
        return false;
    }
    if LABEL_KEYWORDS
        .iter()
        .any(|keyword| trimmed.contains(keyword))
    {
        return true;
    }
    let compact_len = trimmed.chars().filter(|ch| !ch.is_whitespace()).count();
    let hangulish = trimmed
        .chars()
        .all(|ch| ch.is_whitespace() || "():：（）·".contains(ch) || ('가'..='힣').contains(&ch));
    if hangulish && (2..=8).contains(&compact_len) && !trimmed.chars().any(|ch| ch.is_ascii_digit())
    {
        return true;
    }
    LABEL_COLON.is_match(trimmed)
}

/// A short cell holding a label keyword: never a value cell.
fn is_keyword_label(text: &str) -> bool {
    let trimmed = cell_label(text);
    !trimmed.is_empty()
        && trimmed.chars().count() <= 15
        && LABEL_KEYWORDS
            .iter()
            .any(|keyword| trimmed.contains(keyword))
}

fn is_truthy_checkbox(value: &str) -> bool {
    matches!(
        value.trim(),
        "" | "☑" | "✓" | "✔" | "v" | "V" | "true" | "1" | "yes" | "o" | "O"
    )
}

/// kordoc's `find_matching_key`: the exact key, else the longest key that is a prefix of the
/// label (or has the label as its prefix) with the shorter side at least 60% of the longer.
fn find_matching_key(label: &str, values: &BTreeMap<String, String>) -> Option<String> {
    if values.contains_key(label) {
        return Some(label.to_string());
    }
    let label_chars = label.chars().count();
    let mut best_key = None;
    let mut best_len = 0usize;
    for key in values.keys() {
        let key_chars = key.chars().count();
        if label.starts_with(key.as_str()) && key_chars * 10 >= label_chars * 6 {
            if key.len() > best_len {
                best_len = key.len();
                best_key = Some(key.clone());
            }
        } else if key.starts_with(label)
            && label_chars * 10 >= key_chars * 6
            && label.len() > best_len
        {
            best_len = label.len();
            best_key = Some(key.clone());
        }
    }
    best_key
}

/// The head of kordoc's inline-label pattern, `([가-힣A-Za-z]{2,10})\s*[:：]\s*([^\n,;]{0,100})`;
/// [`inline_labels`] bounds the value.
static INLINE_HEAD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([가-힣A-Za-z]{2,10})\s*[:：]\s*").unwrap());
/// A later label on the same line ends a value. Its colon must be followed by whitespace or the
/// end, so `https://` or `Note:x` inside a value does not cut it.
static NEXT_LABEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[가-힣A-Za-z]{2,10}\s*[:：](?:\s|$)").unwrap());
static LABEL_COLON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[가-힣A-Za-z\s]+[:：]$").unwrap());
static PAREN_BLANK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([가-힣A-Za-z]+)\(\s{1,}\)([가-힣A-Za-z]*)").unwrap());
static CHECKBOX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"□([가-힣A-Za-z]+)").unwrap());
static ANNOTATION_BLANK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(([가-힣A-Za-z]+)[:：]\s{1,}\)").unwrap());

const LABEL_KEYWORDS: &[&str] = &[
    "성명",
    "이름",
    "주소",
    "전화",
    "전화번호",
    "휴대폰",
    "핸드폰",
    "연락처",
    "생년월일",
    "주민등록번호",
    "소속",
    "직위",
    "직급",
    "부서",
    "이메일",
    "팩스",
    "학교",
    "학년",
    "반",
    "번호",
    "신청인",
    "대표자",
    "담당자",
    "작성자",
    "확인자",
    "승인자",
    "일시",
    "날짜",
    "기간",
    "장소",
    "목적",
    "사유",
    "비고",
    "금액",
    "수량",
    "단가",
    "합계",
    "계",
    "소계",
    "등록기준지",
    "본적",
    "위임인",
    "청구사유",
    "소명자료",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::from_markdown::from_markdown;

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn field<'a>(fields: &'a [FormField], key: &str) -> &'a FormField {
        fields
            .iter()
            .find(|f| f.key == key)
            .unwrap_or_else(|| panic!("no field {key}: {fields:?}"))
    }

    /// kordoc_lite `scans_placeholders_and_form_labels`: a slot and a label cell of one key
    /// merge into one formLabel field seen twice.
    #[test]
    fn scans_placeholders_and_form_labels() {
        let doc = from_markdown("{{제목}}\n\n{{성명}}\n\n| 성명 | |\n|---|---|\n| 주소 | |\n");
        let fields = scan_form_fields(&doc);
        let seong = field(&fields, "성명");
        assert_eq!(seong.occurrences, 2);
        assert_eq!(seong.source, FormFieldSource::FormLabel);
        assert!(seong.required, "a slot sighting makes the key required");
        let title = field(&fields, "제목");
        assert_eq!(title.source, FormFieldSource::Placeholder);
        assert_eq!(title.source.confidence(), 1.0);
        assert!(!field(&fields, "주소").required);
    }

    #[test]
    fn scans_inline_labels_only_outside_cells() {
        let doc = from_markdown("담당자: 홍길동, 연락처: 010\n\n| 작성일: 오늘 | x |\n|---|---|\n");
        let fields = scan_form_fields(&doc);
        assert_eq!(
            field(&fields, "담당자").source,
            FormFieldSource::InlineLabel
        );
        assert_eq!(field(&fields, "담당자").source.confidence(), 0.64);
        assert!(fields.iter().any(|f| f.key == "연락처"));
        assert!(
            !fields
                .iter()
                .any(|f| f.key == "작성일" && f.source == FormFieldSource::InlineLabel),
            "{fields:?}"
        );
    }

    /// kordoc_lite `fills_placeholders_adjacent_cells_and_inline_labels`.
    #[test]
    fn fills_placeholders_adjacent_cells_and_inline_labels() {
        let mut doc = from_markdown("{{제목}}\n\n담당자: \n\n| 성명 | |\n|---|---|\n| 주소 | |\n");
        let counts = fill_form_fields(
            &mut doc,
            &values(&[
                ("제목", "사업계획"),
                ("성명", "홍길동"),
                ("담당자", "이영준"),
            ]),
        )
        .unwrap()
        .counts;
        assert!(counts.values().sum::<usize>() >= 3, "{counts:?}");
        let text = doc.plain_text();
        assert!(text.contains("사업계획"), "{text}");
        assert!(text.contains("성명\t홍길동"), "{text}");
        assert!(text.contains("담당자: 이영준"), "{text}");
    }

    /// One column, so no pattern cell has a neighbour the label-cell step could also write:
    /// `학년(  )반` and `(비고:   )` read as label cells too, as in kordoc.
    #[test]
    fn fills_blanks_checkboxes_and_annotations_in_cells() {
        let mut doc = from_markdown("| 학년(  )반 |\n|---|\n| □동의 □비동의 |\n| (비고:   ) |\n");
        let counts = fill_form_fields(
            &mut doc,
            &values(&[("학년", "3"), ("동의", "v"), ("비고", "없음")]),
        )
        .unwrap()
        .counts;
        let text = doc.plain_text();
        assert!(text.contains("학년(3)반"), "{text}");
        assert!(text.contains("☑동의 □비동의"), "{text}");
        assert!(text.contains("(비고: 없음)"), "{text}");
        assert_eq!(counts["학년"], 1);
        assert_eq!(counts["동의"], 1);
        assert_eq!(counts["비고"], 1);
    }

    #[test]
    fn paren_blank_prefers_the_prefix_and_suffix_key() {
        let mut doc = from_markdown("| 학년(  )반 |\n|---|\n| x |\n");
        let counts = fill_form_fields(&mut doc, &values(&[("학년반", "3-2")]))
            .unwrap()
            .counts;
        assert_eq!(counts["학년반"], 1);
        assert!(doc.plain_text().contains("학년(3-2)반"));
    }

    /// A falsy checkbox value leaves the box. The box was found, so the key matched (kordoc
    /// reported it unmatched), and a warning says why it stays unchecked.
    #[test]
    fn a_falsy_checkbox_value_is_matched_but_left_unchecked() {
        let mut doc = from_markdown("| □동의 | x |\n|---|---|\n");
        let fill = fill_form_fields(&mut doc, &values(&[("동의", "no")])).unwrap();
        assert_eq!(fill.counts["동의"], 0);
        assert!(fill.unmatched.is_empty(), "{fill:?}");
        assert!(
            fill.warnings.iter().any(|w| w.contains("□동의")),
            "{fill:?}"
        );
        assert!(doc.plain_text().contains("□동의"));
    }

    #[test]
    fn inline_label_is_found_across_a_formatting_split() {
        let mut doc = from_markdown("**담당**자: 미정\n");
        let fields = scan_form_fields(&doc);
        assert_eq!(
            field(&fields, "담당자").source,
            FormFieldSource::InlineLabel
        );
        fill_form_fields(&mut doc, &values(&[("담당자", "이영준")])).unwrap();
        assert!(doc.plain_text().contains("담당자: 이영준"));
    }

    /// Only the text after the colon is replaced, so the label keeps its own char shape.
    #[test]
    fn inline_fill_keeps_the_label_shape() {
        let mut doc = from_markdown("**담당자**: 미정\n");
        let before = doc.sections[0].paragraphs[0].char_shape_runs.clone();
        fill_form_fields(&mut doc, &values(&[("담당자", "이영준")])).unwrap();
        let para = &doc.sections[0].paragraphs[0];
        assert_eq!(para.plain_text(), "담당자: 이영준");
        assert_eq!(
            para.char_shape_runs[0], before[0],
            "the bold label run stays"
        );
    }

    /// A column-header table fills the row below; a filled key-value table overwrites the
    /// adjacent value; a keyword label to the right is never written.
    #[test]
    fn value_cell_policy() {
        let mut doc = from_markdown("| 성명 | 소속 |\n|---|---|\n| | |\n");
        fill_form_fields(&mut doc, &values(&[("성명", "홍길동")])).unwrap();
        assert!(
            doc.plain_text().contains("성명\t소속\n홍길동"),
            "{}",
            doc.plain_text()
        );

        let mut doc = from_markdown("| 사업명 | 기존값 |\n|---|---|\n| 추진배경 | 내용 |\n");
        let fill = fill_form_fields(&mut doc, &values(&[("사업명", "새사업")])).unwrap();
        assert!(
            fill.warnings
                .iter()
                .any(|w| w.contains("표0 (0,1)") && w.contains("기존값")),
            "an overwrite is reported: {fill:?}"
        );
        let text = doc.plain_text();
        assert!(
            text.contains("사업명\t새사업") && text.contains("추진배경"),
            "{text}"
        );

        let mut doc = from_markdown("| 성명 | 소속 | x |\n|---|---|---|\n| 1 | 2 | 3 |\n");
        let counts = fill_form_fields(&mut doc, &values(&[("성명", "홍길동")]))
            .unwrap()
            .counts;
        assert_eq!(counts["성명"], 0, "{}", doc.plain_text());
    }

    /// kordoc parity for slot names with spaces or parentheses: `{{성 명}}` is key `성명`.
    #[test]
    fn slot_names_normalize_like_kordoc() {
        let mut doc = from_markdown("{{성 명}} / {{사업명(국문)}} / {{성명}}\n");
        let fields = scan_form_fields(&doc);
        assert_eq!(field(&fields, "성명").occurrences, 2);
        assert_eq!(
            field(&fields, "사업명국문").source,
            FormFieldSource::Placeholder
        );
        let counts = fill_form_fields(
            &mut doc,
            &values(&[("성명", "홍길동"), ("사업명(국문)", "X")]),
        )
        .unwrap()
        .counts;
        assert_eq!(counts["성명"], 2);
        assert_eq!(counts["사업명(국문)"], 1);
        assert_eq!(doc.plain_text().trim(), "홍길동 / X / 홍길동");
    }

    #[test]
    fn two_keys_that_normalize_alike_are_refused() {
        let mut doc = from_markdown("x\n");
        let error = fill_form_fields(&mut doc, &values(&[("성명", "a"), ("성 명", "b")]));
        assert!(error.is_err());
    }

    #[test]
    fn label_detection_matches_kordoc() {
        assert!(is_label_cell("성명"));
        assert!(is_label_cell("  주 소 : "));
        assert!(is_label_cell("신청인 주소※"));
        assert!(is_label_cell("Name:"));
        assert!(!is_label_cell("2024년"));
        assert!(!is_label_cell(""));
        assert!(!is_label_cell(&"가".repeat(31)));
        assert!(is_keyword_label("연락처"));
        assert!(!is_keyword_label("홍길동"));
        assert_eq!(normalize_label(" 성 명 (한글) : "), "성명한글");
        assert_eq!(trim_label("성명 : "), "성명");
        let keys = values(&[("성명", ""), ("주소지", "")]);
        assert_eq!(find_matching_key("성명", &keys).as_deref(), Some("성명"));
        assert_eq!(find_matching_key("성명한글", &keys).as_deref(), None);
        assert_eq!(find_matching_key("성명*", &keys).as_deref(), Some("성명"));
        assert_eq!(find_matching_key("주소", &keys).as_deref(), Some("주소지"));
    }

    fn shape_at(para: &Paragraph, index: usize) -> hwp_model::CharShapeId {
        let wpos: u32 = para.chars[..index]
            .iter()
            .map(hwp_model::HwpChar::wchar_width)
            .sum();
        para.char_shape_runs
            .iter()
            .rev()
            .find(|(pos, _)| *pos <= wpos)
            .unwrap()
            .1
    }

    /// A cell holding a control (here a 누름틀) is never written: kordoc never removed markup.
    #[test]
    fn a_value_cell_with_controls_is_left_alone() {
        let mut doc = from_markdown("| 성명 | 누름 |\n|---|---|\n| 주소 | |\n");
        assert!(crate::create_field(&mut doc, "누름", "이름란", ""));
        let before = doc.clone();
        let fill = fill_form_fields(&mut doc, &values(&[("성명", "홍길동")])).unwrap();
        assert_eq!(fill.unmatched, ["성명"]);
        assert!(
            fill.warnings.iter().any(|w| w.contains("표0 (0,1)")),
            "{fill:?}"
        );
        assert_eq!(doc, before, "the field cell keeps its control and text");
    }

    /// The value keeps the char shape of the text it replaces, not of a bold colon.
    #[test]
    fn inline_value_keeps_its_own_shape_when_the_colon_is_bold() {
        let mut doc = from_markdown("**담당자:** 미정\n");
        let before = doc.sections[0].paragraphs[0].clone();
        fill_form_fields(&mut doc, &values(&[("담당자", "이영준")])).unwrap();
        let para = &doc.sections[0].paragraphs[0];
        let at = |p: &Paragraph, c: char| {
            p.chars
                .iter()
                .position(|x| *x == hwp_model::HwpChar::Text(c))
                .unwrap()
        };
        assert_eq!(para.plain_text(), "담당자: 이영준");
        let value = shape_at(para, at(para, '이'));
        assert_eq!(value, shape_at(&before, at(&before, '미')), "value shape");
        assert_ne!(
            value,
            shape_at(para, at(para, '담')),
            "the label stays bold"
        );
        assert_eq!(
            shape_at(para, at(para, ':')),
            shape_at(para, at(para, '담'))
        );
    }

    /// A value ends at the next `라벨:` on the line, so filling one label keeps the next.
    #[test]
    fn inline_value_stops_at_the_next_label() {
        let mut doc = from_markdown("작성일: 2026. 8. 20. 담당자: 홍길동\n");
        let fields = scan_form_fields(&doc);
        assert!(fields.iter().any(|f| f.key == "담당자"), "{fields:?}");
        fill_form_fields(&mut doc, &values(&[("작성일", "2026. 9. 1.")])).unwrap();
        assert_eq!(
            doc.plain_text().trim(),
            "작성일: 2026. 9. 1. 담당자: 홍길동"
        );
    }

    /// A slot owns its place: a label whose value is the same slot is filled once, and one
    /// whose value is another requested slot yields to it with a warning.
    #[test]
    fn a_slot_and_a_label_on_one_place_fill_once() {
        let mut doc = from_markdown("담당자: {{담당자}}\n\n| 성명 | {{성명}} |\n|---|---|\n");
        let fill = fill_form_fields(
            &mut doc,
            &values(&[("담당자", "이영준"), ("성명", "홍길동")]),
        )
        .unwrap();
        assert_eq!(fill.counts["담당자"], 1);
        assert_eq!(fill.counts["성명"], 1);
        let text = doc.plain_text();
        assert!(
            text.contains("담당자: 이영준\n") && text.contains("성명\t홍길동"),
            "{text}"
        );

        let mut doc = from_markdown("| 성명 | {{name}} |\n|---|---|\n");
        let fill =
            fill_form_fields(&mut doc, &values(&[("성명", "홍길동"), ("name", "Hong")])).unwrap();
        assert_eq!(fill.counts["name"], 1);
        assert_eq!(fill.unmatched, ["성명"]);
        assert!(
            fill.warnings.iter().any(|w| w.contains("\"성명\"")),
            "{fill:?}"
        );
        assert!(doc.plain_text().contains("성명\tHong"));
    }

    /// `라벨:` inside a slot name belongs to the slot, and a slot later in a value ends it.
    #[test]
    fn inline_labels_leave_slots_alone() {
        let mut doc = from_markdown("{{기간: 시작}}\n\n기간: 3일 {{비고}}\n");
        let fill =
            fill_form_fields(&mut doc, &values(&[("기간", "5일"), ("비고", "없음")])).unwrap();
        assert_eq!(fill.counts["기간"], 1);
        let text = doc.plain_text();
        assert!(text.contains("{{기간: 시작}}"), "{text}");
        assert!(text.contains("기간: 5일 없음"), "{text}");
    }

    /// Inserted text follows the IR rules: CRLF and LF are line breaks, a tab the tab control.
    #[test]
    fn spliced_values_normalize_line_ends_and_tabs() {
        let mut doc = from_markdown("담당자: 미정\n");
        fill_form_fields(&mut doc, &values(&[("담당자", "A\tB\r\nC\r")])).unwrap();
        let para = &doc.sections[0].paragraphs[0];
        assert!(!para.chars.contains(&hwp_model::HwpChar::Text('\r')));
        assert!(!para.chars.contains(&hwp_model::HwpChar::Text('\t')));
        assert_eq!(para.plain_text(), "담당자: A\tB\nC");
    }

    /// A key that normalizes to nothing is dropped, as kordoc does, not reported unmatched.
    #[test]
    fn a_key_that_normalizes_to_nothing_is_dropped() {
        let mut doc = from_markdown("담당자: 미정\n");
        let fill = fill_form_fields(&mut doc, &values(&[(" : ", "x"), ("담당자", "A")])).unwrap();
        assert!(!fill.counts.contains_key(" : "));
        assert!(fill.unmatched.is_empty());
    }
}
