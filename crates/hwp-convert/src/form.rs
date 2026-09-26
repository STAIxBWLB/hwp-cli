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
use regex::{Captures, Regex};

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
                for caps in INLINE_LABEL.captures_iter(&seg) {
                    merge(&mut fields, &caps[1], FormFieldSource::InlineLabel);
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

/// Fill the slots and form fields of `doc` from `values` in one pass. Returns the fills per key
/// as the caller spelled it; 0 means the key matched nothing.
///
/// Keys are compared after [`normalize_label`], and two keys that normalize alike are refused.
/// In order, each step on the text the previous one left:
///
/// 1. `{{ name }}` slots, everywhere, matched by normalized name as in kordoc, in one literal
///    pass ([`crate::replace_slots`]).
/// 2. Outside table cells, `라벨: ...` becomes `라벨: 값`: kordoc replaces the text after the
///    colon up to a comma, semicolon, line end or 100 characters.
/// 3. In table cells: `라벨(  )` blanks, `□옵션` checkboxes (checked when the value is truthy;
///    a falsy value leaves the key unmatched, as in kordoc) and `(라벨:  )` annotations.
/// 4. The value cell of every label cell whose normalized text matches a key (kordoc's
///    `find_matching_key`). The value cell is resolved on the unfilled document: the adjacent
///    cell when it is blank (empty or one slot), else the cell below a complete header row when
///    that is blank, else the adjacent cell unless it is itself a keyword label. When two keys
///    reach one cell, the first keeps it.
pub fn fill_form_fields(
    doc: &mut Document,
    values: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, usize>, String> {
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
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();

    let mut targets: Vec<(String, FormCellCandidate)> = Vec::new();
    for (key, candidate) in walk_form_cells(doc, None, |text, adjacent, below| {
        if !is_label_cell(text) {
            return None;
        }
        let key = find_matching_key(&normalize_label(text), &lookup)?;
        let blank = |cell: &Cell| is_blank_value(&cell_text(cell));
        let target = match (adjacent, below) {
            (Some(adjacent), _) if blank(adjacent) => FormTarget::Adjacent,
            (_, Some(below)) if blank(below) => FormTarget::Below,
            (Some(adjacent), _) if !is_keyword_label(&cell_text(adjacent)) => FormTarget::Adjacent,
            _ => return None,
        };
        Some((key, target))
    }) {
        if !targets.iter().any(|(_, taken)| *taken == candidate) {
            targets.push((key, candidate));
        }
    }

    // Slots are matched the kordoc way, by normalized name (`{{성 명}}` is key `성명`), in the
    // one literal pass `hwp fill` uses.
    let slot_values: BTreeMap<String, String> = crate::scan_placeholders(doc)
        .into_iter()
        .filter_map(|slot| {
            let value = lookup.get(&normalize_label(&slot.name))?.clone();
            Some((slot.name, value))
        })
        .collect();
    for (name, count) in crate::replace_slots(doc, &slot_values)? {
        *counts.entry(normalize_label(&name)).or_default() += count;
    }
    for section in &mut doc.sections {
        for para in &mut section.paragraphs {
            fill_text(para, false, &lookup, &mut counts);
        }
    }
    for (key, candidate) in &targets {
        crate::set_cell(
            doc,
            candidate.table,
            candidate.row,
            candidate.col,
            &lookup[key],
        )?;
        *counts.entry(key.clone()).or_default() += 1;
    }

    Ok(values
        .keys()
        .map(|name| {
            let count = counts.get(&normalize_label(name)).copied().unwrap_or(0);
            (name.clone(), count)
        })
        .collect())
}

/// Steps 2 and 3 of [`fill_form_fields`] on `para` and its nested paragraphs. Returns whether
/// anything changed, so a text box on the way drops its now stale raw XML.
fn fill_text(
    para: &mut Paragraph,
    in_cell: bool,
    values: &BTreeMap<String, String>,
    counts: &mut BTreeMap<String, usize>,
) -> bool {
    let mut changed = if in_cell {
        // Evaluated in turn, as kordoc chains them on one cell's text.
        let paren = splice_matches(para, &PAREN_BLANK, counts, |caps| {
            let prefix = caps.get(1)?;
            let suffix = caps.get(2).map_or("", |m| m.as_str());
            let whole = normalize_label(&format!("{}{suffix}", prefix.as_str()));
            let key = if values.contains_key(&whole) {
                whole
            } else {
                let key = normalize_label(prefix.as_str());
                values.contains_key(&key).then_some(key)?
            };
            // The blank between `(` and `)`.
            let blank = prefix.end() + 1..caps.get(0)?.end() - suffix.len() - 1;
            Some((blank, values[&key].clone(), key))
        });
        let checkbox = splice_matches(para, &CHECKBOX, counts, |caps| {
            let key = normalize_label(&caps[1]);
            let start = caps.get(0)?.start();
            is_truthy_checkbox(values.get(&key)?)
                .then(|| (start..start + '□'.len_utf8(), "☑".to_string(), key))
        });
        let annotation = splice_matches(para, &ANNOTATION_BLANK, counts, |caps| {
            let label = caps.get(1)?;
            let key = normalize_label(label.as_str());
            let value = values.get(&key)?;
            Some((
                label.end()..caps.get(0)?.end() - 1,
                format!(": {value}"),
                key,
            ))
        });
        paren | checkbox | annotation
    } else {
        splice_matches(para, &INLINE_LABEL, counts, |caps| {
            let label = caps.get(1)?;
            let key = normalize_label(label.as_str());
            let value = values.get(&key)?;
            // kordoc rewrites the match to `label: value`; the label itself stays in place.
            Some((label.end()..caps.get(0)?.end(), format!(": {value}"), key))
        })
    };
    for ctrl in &mut para.controls {
        match ctrl {
            Control::Table(table) => {
                for cell in &mut table.cells {
                    for p in &mut cell.paragraphs {
                        changed |= fill_text(p, true, values, counts);
                    }
                }
            }
            Control::Generic(generic) => {
                let mut inner = false;
                for list in &mut generic.paragraph_lists {
                    for p in &mut list.paragraphs {
                        inner |= fill_text(p, in_cell, values, counts);
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

/// Run `re` over each text segment of `para` and splice what `edit` returns for a match: a byte
/// range of the segment, its replacement, and the key to count. Returns whether it spliced.
fn splice_matches(
    para: &mut Paragraph,
    re: &Regex,
    counts: &mut BTreeMap<String, usize>,
    mut edit: impl FnMut(&Captures) -> Option<(Range<usize>, String, String)>,
) -> bool {
    let mut edits = Vec::new();
    for (seg_start, seg) in text_segments(para) {
        for caps in re.captures_iter(&seg) {
            if let Some((range, replacement, key)) = edit(&caps) {
                let start = seg_start + seg[..range.start].chars().count();
                let end = start + seg[range].chars().count();
                edits.push((start..end, replacement));
                *counts.entry(key).or_default() += 1;
            }
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

static INLINE_LABEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([가-힣A-Za-z]{2,10})\s*[:：]\s*([^\n,;]{0,100})").unwrap());
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
        .unwrap();
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
        .unwrap();
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
        let counts = fill_form_fields(&mut doc, &values(&[("학년반", "3-2")])).unwrap();
        assert_eq!(counts["학년반"], 1);
        assert!(doc.plain_text().contains("학년(3-2)반"));
    }

    /// A falsy checkbox value leaves the box, and kordoc reports the key unmatched.
    #[test]
    fn a_falsy_checkbox_value_stays_unmatched() {
        let mut doc = from_markdown("| □동의 | x |\n|---|---|\n");
        let counts = fill_form_fields(&mut doc, &values(&[("동의", "no")])).unwrap();
        assert_eq!(counts["동의"], 0);
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
        fill_form_fields(&mut doc, &values(&[("사업명", "새사업")])).unwrap();
        let text = doc.plain_text();
        assert!(
            text.contains("사업명\t새사업") && text.contains("추진배경"),
            "{text}"
        );

        let mut doc = from_markdown("| 성명 | 소속 | x |\n|---|---|---|\n| 1 | 2 | 3 |\n");
        let counts = fill_form_fields(&mut doc, &values(&[("성명", "홍길동")])).unwrap();
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
        .unwrap();
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
}
