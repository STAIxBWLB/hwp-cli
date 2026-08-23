//! Document frame construction: `--doc-head`/`--doc-foot`/`--notice-head`/`--notice-foot`/
//! `--press-head` (GONG-03, D-01/D-02/D-04/D-10).
//!
//! Every frame block is a table wrapped in one anchor paragraph — the same shape
//! `from_markdown.rs::table_paragraph()` uses for GFM tables (D-02). This module is a sibling of
//! `from_markdown.rs`, not an addition to it (D-10): frame construction lives here, and
//! `from_markdown.rs` only splices the result into `b.paragraphs`.
//!
//! Every frame in this phase is single-column (no cell merging, plan 01 rationale): the shipped
//! `gian-external.md` template already renders the same information as flat lines, so a
//! merged-cell frame would be a second Hancom-compatibility bet this phase does not need.

use std::collections::BTreeMap;

use hwp_model::{
    BorderFillId, Cell, CharShape, CharShapeId, Control, HwpChar, HwpUnit, ParaShape, ParaShapeId,
    Paragraph, Table,
};

use crate::format::{find_or_insert, find_or_insert_para};
use crate::from_markdown::{BODY_WIDTH, CELL_VALIGN_CENTER, TABLE_BORDER_FILL};

/// Per-frame `key=value` fields collected from repeatable CLI/MCP flags (D-01).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameFields {
    /// `--doc-head` (두문): 기관명, 수신, 경유.
    pub doc_head: BTreeMap<String, String>,
    /// `--doc-foot` (결문): 발신명의, 기안자, 검토자, 결재자, 협조자, 시행번호, 시행일자,
    /// 접수번호, 접수일자, 주소, 홈페이지, 전화, 팩스, 이메일, 공개구분.
    pub doc_foot: BTreeMap<String, String>,
    /// `--notice-head` (공고문 머리): 기관명, 공고번호.
    pub notice_head: BTreeMap<String, String>,
    /// `--notice-foot` (공고문 꼬리): 공고일자, 발신명의.
    pub notice_foot: BTreeMap<String, String>,
    /// `--press-head` (보도자료 머리): 기관명, 보도시점, 배포일, 담당부서, 담당자, 연락처.
    pub press_head: BTreeMap<String, String>,
}

impl FrameFields {
    /// True when no frame flag was supplied at all.
    pub fn is_empty(&self) -> bool {
        self.doc_head.is_empty()
            && self.doc_foot.is_empty()
            && self.notice_head.is_empty()
            && self.notice_foot.is_empty()
            && self.press_head.is_empty()
    }
}

/// Locked Korean slot names per frame (`skills/hwp/templates/`). An unknown key for a known frame
/// is refused rather than silently accepted (deviation would be an accept-and-ignore bug).
fn allowed_keys(frame: &str) -> &'static [&'static str] {
    const DOC_HEAD: &[&str] = &["기관명", "수신", "경유"];
    const DOC_FOOT: &[&str] = &[
        "발신명의",
        "기안자",
        "검토자",
        "결재자",
        "협조자",
        "시행번호",
        "시행일자",
        "접수번호",
        "접수일자",
        "주소",
        "홈페이지",
        "전화",
        "팩스",
        "이메일",
        "공개구분",
    ];
    const NOTICE_HEAD: &[&str] = &["기관명", "공고번호"];
    const NOTICE_FOOT: &[&str] = &["공고일자", "발신명의"];
    const PRESS_HEAD: &[&str] = &[
        "기관명",
        "보도시점",
        "배포일",
        "담당부서",
        "담당자",
        "연락처",
    ];
    match frame {
        "doc_head" => DOC_HEAD,
        "doc_foot" => DOC_FOOT,
        "notice_head" => NOTICE_HEAD,
        "notice_foot" => NOTICE_FOOT,
        "press_head" => PRESS_HEAD,
        _ => &[],
    }
}

/// Parses one repeatable `k=v` frame spec for the named frame (`"doc_head"`, `"doc_foot"`, ...).
///
/// Splits on the FIRST `=` (`str::split_once`) so a value containing `=` survives verbatim.
/// Fails closed — never panics, never `.unwrap()`s — on a spec with no `=` or a key outside the
/// frame's allowlist, naming the offending spec in the returned Korean message (T-02.4-01,
/// T-02.4-02).
pub fn parse_field(frame: &str, spec: &str) -> Result<(String, String), String> {
    let Some((key, value)) = spec.split_once('=') else {
        return Err(format!(
            "잘못된 프레임 지정입니다 (key=value 형식이어야 합니다): {spec}"
        ));
    };
    if !allowed_keys(frame).contains(&key) {
        return Err(format!("{frame}에 알 수 없는 키입니다: {spec}"));
    }
    Ok((key.to_string(), value.to_string()))
}

/// Parses the five repeatable frame-flag lists into one [`FrameFields`], sharing one validator
/// between the CLI and MCP surfaces (D-01). Fails closed on the first malformed or unknown spec.
pub fn parse_frame_fields(
    doc_head: &[String],
    doc_foot: &[String],
    notice_head: &[String],
    notice_foot: &[String],
    press_head: &[String],
) -> Result<FrameFields, String> {
    fn fill(
        frame: &str,
        specs: &[String],
        out: &mut BTreeMap<String, String>,
    ) -> Result<(), String> {
        for spec in specs {
            let (key, value) = parse_field(frame, spec)?;
            out.insert(key, value);
        }
        Ok(())
    }
    let mut fields = FrameFields::default();
    fill("doc_head", doc_head, &mut fields.doc_head)?;
    fill("doc_foot", doc_foot, &mut fields.doc_foot)?;
    fill("notice_head", notice_head, &mut fields.notice_head)?;
    fill("notice_foot", notice_foot, &mut fields.notice_foot)?;
    fill("press_head", press_head, &mut fields.press_head)?;
    Ok(fields)
}

/// One plain-text cell paragraph — a single run of text, the shape used everywhere a table cell
/// needs unstyled text (mirrors the empty-cell fallback in `table_paragraph()`).
fn text_paragraph(text: &str, para_shape: ParaShapeId, char_shape: CharShapeId) -> Paragraph {
    Paragraph {
        para_shape,
        chars: text.chars().map(HwpChar::Text).collect(),
        char_shape_runs: vec![(0, char_shape)],
        ..Paragraph::default()
    }
}

/// One row of a frame table: text plus the paragraph/char shape its single run uses. Most rows
/// are unstyled ([`FrameRow::plain`]); 발신명의 (task 2) is the one row needing a distinct
/// centered/bold shape pair.
pub(crate) struct FrameRow {
    text: String,
    para_shape: ParaShapeId,
    char_shape: CharShapeId,
}

impl FrameRow {
    /// A row with the default cell shape (justify, 10pt body) — every row except the centered
    /// 발신명의 line.
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            para_shape: ParaShapeId(0),
            char_shape: CharShapeId(0),
        }
    }
}

/// Builds a single-column table control (one row per [`FrameRow`]), wrapped in the anchor-paragraph
/// shape `table_paragraph()` uses (D-02). `rows` must be non-empty.
pub(crate) fn frame_table(rows: &[FrameRow]) -> Paragraph {
    let row_h = 1700i32; // 10pt text + cell top/bottom margins, same basis as table_paragraph
    let mut cells = Vec::with_capacity(rows.len());
    for (r, row) in rows.iter().enumerate() {
        cells.push(Cell {
            list_attr: CELL_VALIGN_CENTER,
            col: 0,
            row: r as u16,
            col_span: 1,
            row_span: 1,
            width: HwpUnit(BODY_WIDTH),
            height: HwpUnit(row_h),
            margins: [510, 510, 141, 141],
            border_fill: BorderFillId(TABLE_BORDER_FILL),
            header_tail: Vec::new(),
            paragraphs: vec![text_paragraph(&row.text, row.para_shape, row.char_shape)],
        });
    }
    let n_rows = rows.len().max(1);
    let table = Table {
        common_data: Vec::new(),
        placement: None,
        attr: 0,
        rows: n_rows as u16,
        cols: 1,
        cell_spacing: 0,
        inner_margins: [510, 510, 141, 141],
        row_cell_counts: vec![1u16; n_rows],
        border_fill: BorderFillId(TABLE_BORDER_FILL),
        table_tail: Vec::new(),
        caption: None,
        cells,
        extras: Vec::new(),
    };

    let mut payload = vec![0u8; 12];
    payload[..4].copy_from_slice(b" lbt"); // reversed ctrl_id, same convention as table_paragraph
    Paragraph {
        chars: vec![
            HwpChar::ExtCtrl {
                code: 11,
                ctrl_id: *b"tbl ",
                payload,
                ctrl_index: Some(0),
            },
            HwpChar::CharCtrl(13),
        ],
        char_shape_runs: vec![(0, CharShapeId(0))],
        controls: vec![Control::Table(table)],
        ..Paragraph::default()
    }
}

/// Wraps a 공고번호 value in the `제`/`호` form unless the supplied value already carries both,
/// so a caller who passes the full `제2025-282호` form gets it through verbatim.
fn wrap_notice_number(value: &str) -> String {
    if value.starts_with('제') && value.ends_with('호') {
        value.to_string()
    } else {
        format!("제{value}호")
    }
}

/// Builds the 공고문 head rows: `{기관명} 공고` then `제{공고번호}호`, each emitted only when its
/// key is present.
fn notice_head_rows(fields: &BTreeMap<String, String>) -> Vec<FrameRow> {
    let mut rows = Vec::new();
    if let Some(agency) = fields.get("기관명") {
        rows.push(FrameRow::plain(format!("{agency} 공고")));
    }
    if let Some(number) = fields.get("공고번호") {
        rows.push(FrameRow::plain(wrap_notice_number(number)));
    }
    rows
}

/// Builds the 보도자료 head box rows: `보도자료` (always, once the flag is used at all), then
/// `{기관명}`, the 보도시점/배포일 line and the 담당 contact line, each emitted only when its
/// underlying key(s) are present.
fn press_head_rows(fields: &BTreeMap<String, String>) -> Vec<FrameRow> {
    let mut rows = vec![FrameRow::plain("보도자료")];
    if let Some(agency) = fields.get("기관명") {
        rows.push(FrameRow::plain(agency.clone()));
    }
    if fields.contains_key("보도시점") || fields.contains_key("배포일") {
        rows.push(FrameRow::plain(format!(
            "보도시점  {}    배포일  {}",
            fields.get("보도시점").map_or("", String::as_str),
            fields.get("배포일").map_or("", String::as_str),
        )));
    }
    if fields.contains_key("담당부서")
        || fields.contains_key("담당자")
        || fields.contains_key("연락처")
    {
        rows.push(FrameRow::plain(format!(
            "담당  {} {} ({})",
            fields.get("담당부서").map_or("", String::as_str),
            fields.get("담당자").map_or("", String::as_str),
            fields.get("연락처").map_or("", String::as_str),
        )));
    }
    rows
}

/// Builds the leading frame blocks — 두문 (`--doc-head`), 공고문 head (`--notice-head`) and/or
/// 보도자료 head box (`--press-head`) — from whichever fields were supplied. Each populated frame
/// contributes its own table, in that fixed order. Returns an empty vec when no leading frame
/// field was supplied at all — callers must not splice an empty result.
pub fn leading_frames(fields: &FrameFields) -> Vec<Paragraph> {
    let mut out = Vec::new();
    if !fields.doc_head.is_empty() {
        let mut rows = Vec::new();
        if let Some(agency) = fields.doc_head.get("기관명") {
            rows.push(FrameRow::plain(agency.clone()));
        }
        if let Some(recipient) = fields.doc_head.get("수신") {
            rows.push(FrameRow::plain(format!("수신  {recipient}")));
        }
        if let Some(via) = fields.doc_head.get("경유") {
            rows.push(FrameRow::plain(format!("(경유)  {via}")));
        }
        out.push(frame_table(&rows));
    }
    if !fields.notice_head.is_empty() {
        out.push(frame_table(&notice_head_rows(&fields.notice_head)));
    }
    if !fields.press_head.is_empty() {
        out.push(frame_table(&press_head_rows(&fields.press_head)));
    }
    out
}

/// Own text of a paragraph's characters only (no recursion into any controls it carries) — used
/// only by the `끝.` guard below, which inspects top-level body paragraphs.
fn own_text(paragraph: &Paragraph) -> String {
    paragraph
        .chars
        .iter()
        .filter_map(|c| match c {
            HwpChar::Text(ch) => Some(*ch),
            _ => None,
        })
        .collect()
}

/// Whether the last non-empty top-level paragraph in `body` already ends with `끝.` — the check
/// that makes the `끝.` guard idempotent by construction (applying it twice adds nothing) rather
/// than by a marker.
fn ends_with_kkeut(body: &[Paragraph]) -> bool {
    body.iter()
        .rev()
        .map(own_text)
        .find(|text| !text.is_empty())
        .is_some_and(|text| text.ends_with("끝."))
}

/// Allocates (or reuses, by value-dedup) the centered `ParaShape` and 22pt bold `CharShape` the
/// 발신명의 line needs. Cloning `[0]` as the base (rather than building fresh) preserves the
/// shade/shadow defaults `default_header()` documents as required to avoid the historical "black
/// bar" bug. `find_or_insert`/`find_or_insert_para` append-only-on-miss (Pitfall 5), so reapplying
/// the same `--doc-foot` never grows either table (the two-run byte-equality guard).
fn centered_bold_22pt(
    para_shapes: &mut Vec<ParaShape>,
    char_shapes: &mut Vec<CharShape>,
) -> (ParaShapeId, CharShapeId) {
    let base_para = para_shapes[0].clone();
    let centered = ParaShape {
        // Clear alignment bits 2-4 then set them to 3 (center) — ParaShape::alignment() (Pattern
        // 4 / Pitfall 2: horizontal alignment lives here, not on Cell.list_attr).
        attr1: (base_para.attr1 & !(0x7 << 2)) | (3 << 2),
        ..base_para
    };
    let para_id = find_or_insert_para(para_shapes, centered);

    let base_char = char_shapes[0].clone();
    let bold_22pt = CharShape {
        base_size: 2200,
        attr: base_char.attr | (1 << 1), // bold bit
        ..base_char
    };
    let char_id = find_or_insert(char_shapes, bold_22pt);
    (para_id, char_id)
}

/// A named value, or the bare label alone when the value is absent/empty (D-04 placeholder
/// styling — the approval system renders the filled grid; templates hold placeholders only).
fn labeled(label: &str, value: Option<&String>) -> String {
    match value {
        Some(v) if !v.is_empty() => format!("{label} {v}"),
        _ => label.to_string(),
    }
}

/// Builds the optional `끝.` guard paragraph followed by the 결문 (document footer) block from
/// `--doc-foot` fields, plus the 공고문 foot block from `--notice-foot` fields, whichever were
/// supplied. `body` is the paragraph sequence assembled so far (the 두문 table, if any, plus the
/// parsed body) — inspected only for the `끝.` guard, which applies to `doc_foot` alone (the
/// shipped `notice.md` template carries no `끝.` line after 발신명의).
///
/// 결문 row order (always emitted once `doc_foot` carries any field, except 발신명의 which needs
/// its own key): 발신명의 → 결재 → 협조 → 시행/접수 → 주소/홈페이지 → 연락처 (D-03/D-04). The
/// 결재/협조 rows are placeholder rows, never a filled multi-column approval grid (D-04).
///
/// 공고 foot row order (each emitted only when its key is present): 공고일자 → 발신명의, the
/// latter reusing the same centered/22pt/bold shape as 결문's 발신명의 (value-deduped, so the
/// same name never grows either shape table).
pub fn trailing_frames(
    fields: &FrameFields,
    body: &[Paragraph],
    para_shapes: &mut Vec<ParaShape>,
    char_shapes: &mut Vec<CharShape>,
) -> Vec<Paragraph> {
    let mut out = Vec::new();

    if !fields.doc_foot.is_empty() {
        let f = &fields.doc_foot;
        if !ends_with_kkeut(body) {
            out.push(text_paragraph("끝.", ParaShapeId(0), CharShapeId(0)));
        }

        let mut rows: Vec<FrameRow> = Vec::new();
        if let Some(name) = f.get("발신명의") {
            let (para_shape, char_shape) = centered_bold_22pt(para_shapes, char_shapes);
            rows.push(FrameRow {
                text: name.clone(),
                para_shape,
                char_shape,
            });
        }
        rows.push(FrameRow::plain(format!(
            "결재  {}    {}    {}",
            labeled("기안자", f.get("기안자")),
            labeled("검토자", f.get("검토자")),
            labeled("결재자", f.get("결재자")),
        )));
        rows.push(FrameRow::plain(match f.get("협조자") {
            Some(v) if !v.is_empty() => format!("협조  {v}"),
            _ => "협조".to_string(),
        }));
        rows.push(FrameRow::plain(format!(
            "시행  {}({})    접수  {}({})",
            f.get("시행번호").map_or("", String::as_str),
            f.get("시행일자").map_or("", String::as_str),
            f.get("접수번호").map_or("", String::as_str),
            f.get("접수일자").map_or("", String::as_str),
        )));
        rows.push(FrameRow::plain(format!(
            "우{}  /  {}",
            f.get("주소").map_or("", String::as_str),
            f.get("홈페이지").map_or("", String::as_str),
        )));
        rows.push(FrameRow::plain(format!(
            "전화 {}  /  팩스 {}  /  {}  /  공개구분 {}",
            f.get("전화").map_or("", String::as_str),
            f.get("팩스").map_or("", String::as_str),
            f.get("이메일").map_or("", String::as_str),
            f.get("공개구분").map_or("", String::as_str),
        )));

        out.push(frame_table(&rows));
    }

    if !fields.notice_foot.is_empty() {
        let f = &fields.notice_foot;
        let mut rows: Vec<FrameRow> = Vec::new();
        if let Some(date) = f.get("공고일자") {
            rows.push(FrameRow::plain(date.clone()));
        }
        if let Some(name) = f.get("발신명의") {
            let (para_shape, char_shape) = centered_bold_22pt(para_shapes, char_shapes);
            rows.push(FrameRow {
                text: name.clone(),
                para_shape,
                char_shape,
            });
        }
        if !rows.is_empty() {
            out.push(frame_table(&rows));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_field_splits_on_first_equals_only() {
        let (key, value) = parse_field("doc_head", "수신=총장=대리").unwrap();
        assert_eq!(key, "수신");
        assert_eq!(value, "총장=대리");
    }

    #[test]
    fn parse_field_rejects_missing_equals() {
        let err = parse_field("doc_head", "기관명").unwrap_err();
        assert!(err.contains("기관명"), "{err}");
    }

    #[test]
    fn parse_field_rejects_unknown_key() {
        let err = parse_field("doc_head", "없는키=x").unwrap_err();
        assert!(err.contains("없는키=x"), "{err}");
    }

    #[test]
    fn parse_field_rejects_unwired_frame_keys() {
        // notice_head has a real key set (기관명, 공고번호); an unrelated key must still refuse
        // rather than silently accept-and-ignore.
        let err = parse_field("notice_head", "아무값=x").unwrap_err();
        assert!(err.contains("notice_head"), "{err}");
    }

    #[test]
    fn leading_frames_empty_when_no_doc_head() {
        assert!(leading_frames(&FrameFields::default()).is_empty());
    }

    #[test]
    fn leading_frames_orders_기관명_수신_경유() {
        let mut fields = FrameFields::default();
        fields
            .doc_head
            .insert("경유".to_string(), "총무과".to_string());
        fields
            .doc_head
            .insert("수신".to_string(), "총장".to_string());
        fields
            .doc_head
            .insert("기관명".to_string(), "예시대학교".to_string());
        let paragraphs = leading_frames(&fields);
        assert_eq!(paragraphs.len(), 1);
        let Control::Table(table) = &paragraphs[0].controls[0] else {
            panic!("expected a table control");
        };
        assert_eq!(table.cells.len(), 3);
        let cell_text = |cell: &Cell| -> String {
            cell.paragraphs[0]
                .chars
                .iter()
                .filter_map(|c| match c {
                    HwpChar::Text(ch) => Some(*ch),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(cell_text(&table.cells[0]), "예시대학교");
        assert_eq!(cell_text(&table.cells[1]), "수신  총장");
        assert_eq!(cell_text(&table.cells[2]), "(경유)  총무과");
    }

    fn cell_text(cell: &Cell) -> String {
        cell.paragraphs[0]
            .chars
            .iter()
            .filter_map(|c| match c {
                HwpChar::Text(ch) => Some(*ch),
                _ => None,
            })
            .collect()
    }

    fn default_header_shapes() -> (Vec<ParaShape>, Vec<CharShape>) {
        let header = crate::from_markdown::default_header();
        (header.para_shapes, header.char_shapes)
    }

    #[test]
    fn trailing_frames_empty_when_no_doc_foot() {
        let (mut para_shapes, mut char_shapes) = default_header_shapes();
        assert!(
            trailing_frames(
                &FrameFields::default(),
                &[],
                &mut para_shapes,
                &mut char_shapes
            )
            .is_empty()
        );
    }

    #[test]
    fn trailing_frames_always_emits_결재_then_협조_placeholder_rows() {
        let (mut para_shapes, mut char_shapes) = default_header_shapes();
        let mut fields = FrameFields::default();
        fields
            .doc_foot
            .insert("발신명의".to_string(), "예시대학교총장".to_string());
        let paragraphs = trailing_frames(&fields, &[], &mut para_shapes, &mut char_shapes);
        // [끝., 결문 table] — no body paragraph precedes it, so the guard always fires.
        assert_eq!(paragraphs.len(), 2);
        assert_eq!(own_text(&paragraphs[0]), "끝.");
        let Control::Table(table) = &paragraphs[1].controls[0] else {
            panic!("expected a table control");
        };
        // Row 0: 발신명의, centered + 22pt bold.
        assert_eq!(cell_text(&table.cells[0]), "예시대학교총장");
        let para_shape = &para_shapes[table.cells[0].paragraphs[0].para_shape.0 as usize];
        assert_eq!(para_shape.alignment(), 3);
        let char_shape = &char_shapes[table.cells[0].paragraphs[0].char_shape_runs[0].1.0 as usize];
        assert_eq!(char_shape.base_size, 2200);
        assert_eq!(char_shape.attr & (1 << 1), 1 << 1, "bold bit must be set");

        // Row 1: 결재 placeholder (no names supplied).
        assert!(cell_text(&table.cells[1]).starts_with("결재"));
        // Row 2: 협조 on its own row, separate from 결재 (D-04).
        assert_eq!(cell_text(&table.cells[2]), "협조");
    }

    #[test]
    fn trailing_frames_끝_guard_is_idempotent() {
        let (mut para_shapes, mut char_shapes) = default_header_shapes();
        let mut fields = FrameFields::default();
        fields
            .doc_foot
            .insert("발신명의".to_string(), "예시대학교총장".to_string());

        let plain_body = vec![text_paragraph(
            "본문 마지막 줄",
            ParaShapeId(2),
            CharShapeId(0),
        )];
        let with_guard = trailing_frames(&fields, &plain_body, &mut para_shapes, &mut char_shapes);
        assert_eq!(own_text(&with_guard[0]), "끝.");

        let already_ended = vec![text_paragraph(
            "본문 마지막 줄 끝.",
            ParaShapeId(2),
            CharShapeId(0),
        )];
        let without_guard =
            trailing_frames(&fields, &already_ended, &mut para_shapes, &mut char_shapes);
        // Only the 결문 table — applying the rule twice adds nothing.
        assert_eq!(without_guard.len(), 1);
    }

    #[test]
    fn trailing_frames_dedupes_shapes_across_two_runs() {
        let (mut para_shapes, mut char_shapes) = default_header_shapes();
        let mut fields = FrameFields::default();
        fields
            .doc_foot
            .insert("발신명의".to_string(), "예시대학교총장".to_string());

        let _ = trailing_frames(&fields, &[], &mut para_shapes, &mut char_shapes);
        let len_after_first = (para_shapes.len(), char_shapes.len());
        let _ = trailing_frames(&fields, &[], &mut para_shapes, &mut char_shapes);
        assert_eq!(
            (para_shapes.len(), char_shapes.len()),
            len_after_first,
            "re-applying the same --doc-foot must not grow either shape table"
        );
    }

    #[test]
    fn leading_frames_builds_notice_head_wrapping_the_number() {
        let mut fields = FrameFields::default();
        fields
            .notice_head
            .insert("기관명".to_string(), "예시대학교".to_string());
        fields
            .notice_head
            .insert("공고번호".to_string(), "2025-282".to_string());
        let paragraphs = leading_frames(&fields);
        assert_eq!(paragraphs.len(), 1);
        let Control::Table(table) = &paragraphs[0].controls[0] else {
            panic!("expected a table control");
        };
        assert_eq!(cell_text(&table.cells[0]), "예시대학교 공고");
        assert_eq!(cell_text(&table.cells[1]), "제2025-282호");
    }

    #[test]
    fn leading_frames_notice_head_number_not_double_wrapped() {
        let mut fields = FrameFields::default();
        fields
            .notice_head
            .insert("공고번호".to_string(), "제2025-282호".to_string());
        let paragraphs = leading_frames(&fields);
        let Control::Table(table) = &paragraphs[0].controls[0] else {
            panic!("expected a table control");
        };
        assert_eq!(cell_text(&table.cells[0]), "제2025-282호");
    }

    #[test]
    fn leading_frames_builds_press_head_box() {
        let mut fields = FrameFields::default();
        for (k, v) in [
            ("기관명", "예시대학교"),
            ("보도시점", "즉시"),
            ("배포일", "2026.8.23."),
            ("담당부서", "홍보실"),
            ("담당자", "홍길동"),
            ("연락처", "02-000-0000"),
        ] {
            fields.press_head.insert(k.to_string(), v.to_string());
        }
        let paragraphs = leading_frames(&fields);
        assert_eq!(paragraphs.len(), 1);
        let Control::Table(table) = &paragraphs[0].controls[0] else {
            panic!("expected a table control");
        };
        let rows: Vec<String> = table.cells.iter().map(cell_text).collect();
        assert_eq!(rows[0], "보도자료");
        assert_eq!(rows[1], "예시대학교");
        assert_eq!(rows[2], "보도시점  즉시    배포일  2026.8.23.");
        assert_eq!(rows[3], "담당  홍보실 홍길동 (02-000-0000)");
    }

    #[test]
    fn trailing_frames_builds_notice_foot_without_공고일자() {
        let (mut para_shapes, mut char_shapes) = default_header_shapes();
        let mut fields = FrameFields::default();
        fields
            .notice_foot
            .insert("발신명의".to_string(), "예시대학교총장".to_string());
        let paragraphs = trailing_frames(&fields, &[], &mut para_shapes, &mut char_shapes);
        assert_eq!(paragraphs.len(), 1, "no 끝. guard for notice_foot");
        let Control::Table(table) = &paragraphs[0].controls[0] else {
            panic!("expected a table control");
        };
        assert_eq!(table.cells.len(), 1);
        assert_eq!(cell_text(&table.cells[0]), "예시대학교총장");
        let para_shape = &para_shapes[table.cells[0].paragraphs[0].para_shape.0 as usize];
        assert_eq!(para_shape.alignment(), 3, "발신명의 row must be centered");
    }

    #[test]
    fn trailing_frames_notice_foot_orders_공고일자_then_발신명의() {
        let (mut para_shapes, mut char_shapes) = default_header_shapes();
        let mut fields = FrameFields::default();
        fields
            .notice_foot
            .insert("공고일자".to_string(), "2026. 8. 23.".to_string());
        fields
            .notice_foot
            .insert("발신명의".to_string(), "예시대학교총장".to_string());
        let paragraphs = trailing_frames(&fields, &[], &mut para_shapes, &mut char_shapes);
        let Control::Table(table) = &paragraphs[0].controls[0] else {
            panic!("expected a table control");
        };
        assert_eq!(cell_text(&table.cells[0]), "2026. 8. 23.");
        assert_eq!(cell_text(&table.cells[1]), "예시대학교총장");
    }
}
