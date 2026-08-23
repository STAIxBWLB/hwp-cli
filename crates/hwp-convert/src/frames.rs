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

use hwp_model::{BorderFillId, Cell, CharShapeId, Control, HwpChar, HwpUnit, ParaShapeId, Paragraph, Table};

use crate::from_markdown::{BODY_WIDTH, CELL_VALIGN_CENTER, TABLE_BORDER_FILL};

/// Per-frame `key=value` fields collected from repeatable CLI/MCP flags (D-01).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameFields {
    /// `--doc-head` (두문): 기관명, 수신, 경유.
    pub doc_head: BTreeMap<String, String>,
    /// `--doc-foot` (결문): 발신명의, 기안자, 검토자, 결재자, 협조자, 시행번호, 시행일자,
    /// 접수번호, 접수일자, 주소, 홈페이지, 전화, 팩스, 이메일, 공개구분.
    pub doc_foot: BTreeMap<String, String>,
    /// `--notice-head` (공고문 머리). Key set wired in a later plan.
    pub notice_head: BTreeMap<String, String>,
    /// `--notice-foot` (공고문 꼬리). Key set wired in a later plan.
    pub notice_foot: BTreeMap<String, String>,
    /// `--press-head` (보도자료 머리). Key set wired in a later plan.
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

/// Locked Korean slot names per frame (`skills/hwp/templates/`). An empty slice means the frame's
/// key set is not wired yet (`notice_head`/`notice_foot`/`press_head`, plan 02) — every key is
/// refused until then rather than silently accepted (deviation would be an accept-and-ignore bug).
fn allowed_keys(frame: &str) -> &'static [&'static str] {
    const DOC_HEAD: &[&str] = &["기관명", "수신", "경유"];
    const DOC_FOOT: &[&str] = &[
        "발신명의", "기안자", "검토자", "결재자", "협조자", "시행번호", "시행일자", "접수번호",
        "접수일자", "주소", "홈페이지", "전화", "팩스", "이메일", "공개구분",
    ];
    match frame {
        "doc_head" => DOC_HEAD,
        "doc_foot" => DOC_FOOT,
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
    fn fill(frame: &str, specs: &[String], out: &mut BTreeMap<String, String>) -> Result<(), String> {
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

/// Builds a single-column table control (one row per string), wrapped in the anchor-paragraph
/// shape `table_paragraph()` uses (D-02). `rows` must be non-empty.
pub(crate) fn frame_table(rows: &[String]) -> Paragraph {
    let row_h = 1700i32; // 10pt text + cell top/bottom margins, same basis as table_paragraph
    let mut cells = Vec::with_capacity(rows.len());
    for (r, text) in rows.iter().enumerate() {
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
            paragraphs: vec![text_paragraph(text, ParaShapeId(0), CharShapeId(0))],
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

/// Builds the 두문 (document letterhead) block from `--doc-head` fields, when supplied.
///
/// Row order, each emitted only when its key is present: 기관명 → 수신 → (경유). Returns an empty
/// vec when `doc_head` carries no fields — callers must not splice an empty result.
pub fn leading_frames(fields: &FrameFields) -> Vec<Paragraph> {
    if fields.doc_head.is_empty() {
        return Vec::new();
    }
    let mut rows = Vec::new();
    if let Some(agency) = fields.doc_head.get("기관명") {
        rows.push(agency.clone());
    }
    if let Some(recipient) = fields.doc_head.get("수신") {
        rows.push(format!("수신  {recipient}"));
    }
    if let Some(via) = fields.doc_head.get("경유") {
        rows.push(format!("(경유)  {via}"));
    }
    vec![frame_table(&rows)]
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
        // notice_head/notice_foot/press_head land in a later plan; any key must still refuse
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
        fields.doc_head.insert("경유".to_string(), "총무과".to_string());
        fields.doc_head.insert("수신".to_string(), "총장".to_string());
        fields.doc_head.insert("기관명".to_string(), "예시대학교".to_string());
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
}
