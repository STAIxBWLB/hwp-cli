//! Opaque range-tag edit guards. Sentinel bytes deliberately encode no record layout.

use std::collections::BTreeMap;
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use hwp_model::opaque::OpaqueRecord;
use hwp_model::{Caption, CaptionDirection, CaptionSide, GenericControl, LineSeg, ParagraphList};

const RANGE_TAG: u16 = 0x46;
const OPAQUE_TAG: &[u8] = b"opaque owner-authored range-tag sentinel";

/// Invoke the same tests before and after formerly infallible mutators gain Result returns.
/// Legacy success values become Ok; this adapter never makes a refusal decision itself.
trait OperationResult {
    fn operation_result(self) -> Result<(), String>;
}

impl OperationResult for usize {
    fn operation_result(self) -> Result<(), String> {
        Ok(())
    }
}

impl OperationResult for bool {
    fn operation_result(self) -> Result<(), String> {
        Ok(())
    }
}

impl OperationResult for () {
    fn operation_result(self) -> Result<(), String> {
        Ok(())
    }
}

impl<T, E: Display> OperationResult for Result<T, E> {
    fn operation_result(self) -> Result<(), String> {
        self.map(|_| ()).map_err(|error| error.to_string())
    }
}

fn paragraph(text: &str) -> Paragraph {
    let mut chars: Vec<_> = text.chars().map(HwpChar::Text).collect();
    chars.push(HwpChar::CharCtrl(hwp_model::ctrl_char::PARA_BREAK));
    Paragraph {
        chars,
        char_shape_runs: vec![(0, CharShapeId(0))],
        line_segs: vec![LineSeg {
            text_start: 0,
            v_pos: 123,
            line_height: 1000,
            text_height: 900,
            baseline_gap: 700,
            line_spacing: 500,
            col_start: 0,
            seg_width: 10000,
            flags: 1,
        }],
        ..Paragraph::default()
    }
}

fn tag(para: &mut Paragraph) {
    para.extras.push(OpaqueRecord {
        tag: RANGE_TAG,
        data: OPAQUE_TAG.to_vec(),
        children: Vec::new(),
    });
}

fn document(texts: &[&str]) -> Document {
    let mut doc = crate::from_markdown("fixture header");
    doc.sections[0].paragraphs = texts.iter().map(|text| paragraph(text)).collect();
    doc.bin_streams.push(hwp_model::BinStream {
        name: "untouched.bin".into(),
        data: b"untouched binary payload".to_vec(),
    });
    doc
}

fn generic(inner: Paragraph, caption: Option<Paragraph>) -> Control {
    Control::Generic(GenericControl {
        ctrl_id: *b"gso ",
        data: Vec::new(),
        paragraph_lists: vec![ParagraphList {
            header_data: b"original list header".to_vec(),
            paragraphs: vec![inner],
        }],
        extras: Vec::new(),
        raw_children: Vec::new(),
        gso_shapes: Vec::new(),
        equation: None,
        column_def: None,
        caption: caption.map(|para| Caption {
            side: CaptionSide::Bottom,
            direction: CaptionDirection::Horizontal,
            gap: 0,
            width: None,
            last_width: 0,
            paragraphs: vec![para],
        }),
        hwpx_raw_xml: Some("<opaque-mirror/>".into()),
        container_box: None,
    })
}

fn remove_tags(para: &mut Paragraph) {
    para.extras.retain(|record| record.tag != RANGE_TAG);
    for control in &mut para.controls {
        match control {
            Control::Table(table) => {
                if let Some(caption) = &mut table.caption {
                    for para in &mut caption.paragraphs {
                        remove_tags(para);
                    }
                }
                for cell in &mut table.cells {
                    for para in &mut cell.paragraphs {
                        remove_tags(para);
                    }
                }
            }
            Control::Picture(picture) => {
                if let Some(caption) = &mut picture.caption {
                    for para in &mut caption.paragraphs {
                        remove_tags(para);
                    }
                }
            }
            Control::Generic(generic) => {
                for list in &mut generic.paragraph_lists {
                    for para in &mut list.paragraphs {
                        remove_tags(para);
                    }
                }
                if let Some(caption) = &mut generic.caption {
                    for para in &mut caption.paragraphs {
                        remove_tags(para);
                    }
                }
            }
            Control::SectionDef(_) => {}
        }
    }
}

fn assert_refused(result: impl OperationResult) {
    let error = result
        .operation_result()
        .expect_err("an edit that shifts or discards opaque range tags must refuse");
    assert!(
        error.contains("구간") || error.contains("PARA_RANGE_TAG"),
        "the refusal must identify the opaque range-tag cause: {error}"
    );
    assert!(
        error.contains("지우") || error.contains("제거") || error.contains("같은"),
        "the refusal must offer annotation removal or a same-length remedy: {error}"
    );
}

fn assert_document_refusal<R: OperationResult>(
    mut doc: Document,
    mut operation: impl FnMut(&mut Document) -> R,
) {
    let mut positive = doc.clone();
    for section in &mut positive.sections {
        for para in &mut section.paragraphs {
            remove_tags(para);
        }
    }
    let before_positive = positive.clone();
    operation(&mut positive)
        .operation_result()
        .expect("the untagged positive fixture must support the same real operation");
    assert_ne!(
        positive, before_positive,
        "the fixture must contain a real selected target"
    );
    let before = doc.clone();
    assert_refused(operation(&mut doc));
    assert_eq!(
        doc, before,
        "refusal must preserve body, opaque records, headers, binaries, raw mirrors and caches"
    );
}

#[test]
fn range_tags_private_splice_text_refuses_before_mutation() {
    let mut para = paragraph("ABCD");
    tag(&mut para);
    let before = para.clone();
    assert_refused(splice_text(&mut para, 0..1, 0, 1, "LONG"));
    assert_eq!(para, before);
}

#[test]
fn range_tags_private_splice_edits_preflights_every_edit() {
    let mut para = paragraph("ABCD");
    tag(&mut para);
    let before = para.clone();
    // Back-to-front application would otherwise change D before refusing the earlier growth.
    assert_refused(splice_edits(
        &mut para,
        vec![(0..1, "LONG".into()), (3..4, "X".into())],
    ));
    assert_eq!(para, before);
}

#[test]
fn range_tags_private_splice_edits_refuses_net_zero_opposing_deltas() {
    let mut para = paragraph("ABCDEF");
    tag(&mut para);
    let before = para.clone();
    // Total width remains six, but offsets between the edits would move.
    assert_refused(splice_edits(
        &mut para,
        vec![(0..1, "AA".into()), (4..6, "Z".into())],
    ));
    assert_eq!(para, before);
}

#[test]
fn range_tags_global_replace_is_atomic_for_a_late_tagged_match() {
    let mut doc = document(&["needle first", "needle second"]);
    tag(&mut doc.sections[0].paragraphs[1]);
    assert_document_refusal(doc, |doc| {
        replace_text(doc, "needle", "expanded value", true)
    });
}

#[test]
fn range_tags_addressed_replace_refuses_without_invalidating_ancestors() {
    let mut inner = paragraph("needle");
    tag(&mut inner);
    let mut doc = document(&["host"]);
    doc.sections[0].paragraphs[0]
        .controls
        .push(generic(inner, None));
    let path = SegmentPath {
        section: 0,
        indices: vec![0, 0, 0],
    };
    assert_document_refusal(doc, |doc| {
        replace_text_at(doc, &path, "needle", "expanded value")
    });
}

#[test]
fn range_tags_global_replace_preflights_nested_paragraphs() {
    let mut inner = paragraph("needle");
    tag(&mut inner);
    let mut doc = document(&["needle first", "host"]);
    doc.sections[0].paragraphs[1]
        .controls
        .push(generic(inner, None));
    assert_document_refusal(doc, |doc| replace_text(doc, "needle", "longer value", true));
}

fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).into(), (*value).into()))
        .collect()
}

#[test]
fn range_tags_slot_fill_refuses_net_zero_splice_deltas() {
    let mut doc = document(&["{{a}} {{b}}"]);
    tag(&mut doc.sections[0].paragraphs[0]);
    assert_document_refusal(doc, |doc| {
        crate::replace_slots(doc, &values(&[("a", "123456"), ("b", "1234")]))
    });
}

#[test]
fn range_tags_slot_fill_is_atomic_for_a_late_target() {
    let mut doc = document(&["{{name}}", "{{name}}"]);
    tag(&mut doc.sections[0].paragraphs[1]);
    assert_document_refusal(doc, |doc| {
        crate::replace_slots(doc, &values(&[("name", "X")]))
    });
}

#[test]
fn range_tags_form_fill_refuses_net_zero_splice_deltas() {
    let mut doc = document(&["{{a}} {{b}}"]);
    tag(&mut doc.sections[0].paragraphs[0]);
    assert_document_refusal(doc, |doc| {
        crate::fill_form_fields(doc, &values(&[("a", "123456"), ("b", "1234")]))
    });
}

fn table_document() -> Document {
    crate::from_markdown("| 항목 | 값 |\n| --- | --- |\n| 성명 | old |\n")
}

#[test]
fn range_tags_form_fill_preflights_late_whole_cell_replacement() {
    let mut doc = table_document();
    doc.sections[0].paragraphs.insert(0, paragraph("{{early}}"));
    let targets = find_form_cells_by_label(&mut doc, "성명", None);
    assert_eq!(
        targets.len(),
        1,
        "the labeled value-cell fixture must resolve"
    );
    assert_eq!((targets[0].row, targets[0].col), (1, 1));
    with_nth_table(&mut doc, 0, |table| {
        let cell = table
            .cells
            .iter_mut()
            .find(|cell| cell.row == 1 && cell.col == 1)
            .unwrap();
        tag(&mut cell.paragraphs[0]);
    })
    .unwrap();
    assert_document_refusal(doc, |doc| {
        crate::fill_form_fields(doc, &values(&[("early", "X"), ("성명", "LONG")]))
    });
}

fn field_document(two: bool) -> Document {
    let mut doc = document(if two {
        &["first:", "second:"]
    } else {
        &["first:"]
    });
    crate::create_field(&mut doc, "first:", "entry", "A")
        .operation_result()
        .unwrap();
    if two {
        crate::create_field(&mut doc, "second:", "entry", "BBB")
            .operation_result()
            .unwrap();
    }
    assert_eq!(crate::list_fields(&doc).len(), if two { 2 } else { 1 });
    doc
}

#[test]
fn range_tags_named_field_fill_is_atomic_for_a_late_target() {
    let mut doc = field_document(true);
    tag(&mut doc.sections[0].paragraphs[1]);
    assert_document_refusal(doc, |doc| crate::set_field(doc, "entry", "longer"));
}

#[test]
fn range_tags_named_field_fill_refuses_opposing_deltas_in_one_paragraph() {
    let mut doc = document(&["first: second:"]);
    for (anchor, value) in [("first:", "A"), ("second:", "BBB")] {
        crate::create_field(&mut doc, anchor, "entry", value)
            .operation_result()
            .unwrap();
    }
    assert_eq!(crate::list_fields(&doc).len(), 2);
    tag(&mut doc.sections[0].paragraphs[0]);
    assert_document_refusal(doc, |doc| crate::set_field(doc, "entry", "ZZ"));
}

#[test]
fn range_tags_set_cell_refuses_discarding_opaque_tags() {
    let mut doc = table_document();
    with_nth_table(&mut doc, 0, |table| {
        let cell = table
            .cells
            .iter_mut()
            .find(|cell| cell.row == 1 && cell.col == 1)
            .unwrap();
        tag(&mut cell.paragraphs[0]);
    })
    .unwrap();
    assert_document_refusal(doc, |doc| set_cell(doc, 0, 1, 1, "replacement"));
}

#[test]
fn range_tags_set_cell_checks_tags_in_discarded_nested_captions() {
    let mut doc = table_document();
    with_nth_table(&mut doc, 0, |table| {
        let cell = table
            .cells
            .iter_mut()
            .find(|cell| cell.row == 1 && cell.col == 1)
            .unwrap();
        let mut caption = paragraph("caption annotation");
        tag(&mut caption);
        cell.paragraphs[0]
            .controls
            .push(generic(paragraph("inner text"), Some(caption)));
    })
    .unwrap();
    assert_document_refusal(doc, |doc| set_cell(doc, 0, 1, 1, "replacement"));
}

fn assert_anchor_insertion_refuses(kind: &str) {
    let mut doc = document(&["anchor"]);
    tag(&mut doc.sections[0].paragraphs[0]);
    assert_document_refusal(doc, |doc| match kind {
        "field" => crate::create_field(doc, "anchor", "created", "value").operation_result(),
        "hyperlink" => {
            crate::create_hyperlink(doc, "anchor", "https://example.com", "link").operation_result()
        }
        "bookmark" => crate::create_bookmark(doc, "anchor", "created").operation_result(),
        _ => unreachable!(),
    });
}

#[test]
fn range_tags_field_creation_refuses_before_mutation() {
    assert_anchor_insertion_refuses("field");
}

#[test]
fn range_tags_hyperlink_creation_refuses_before_header_mutation() {
    assert_anchor_insertion_refuses("hyperlink");
}

#[test]
fn range_tags_bookmark_creation_refuses_before_mutation() {
    assert_anchor_insertion_refuses("bookmark");
}

struct ImageFixture(PathBuf);

impl ImageFixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "hwp470-image-{}-{sequence}.png",
            std::process::id()
        ));
        // Header-only synthetic pixels match image.rs's existing image_pixel_size fixture.
        // A positive real-operation control below verifies this insertion fixture is accepted.
        let mut bytes = vec![137, 80, 78, 71, 13, 10, 26, 10];
        bytes.extend_from_slice(&[0, 0, 0, 13]);
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&2u32.to_be_bytes());
        bytes.extend_from_slice(&2u32.to_be_bytes());
        std::fs::write(&path, bytes).unwrap();
        Self(path)
    }
}

impl Drop for ImageFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn range_tags_image_insertion_refuses_before_binary_mutation() {
    let image = ImageFixture::new();
    let mut doc = document(&["anchor"]);
    tag(&mut doc.sections[0].paragraphs[0]);
    assert_document_refusal(doc, |doc| {
        crate::image::insert_image(doc, "anchor", &image.0, crate::image::ImageSize::Natural)
    });
}

#[test]
fn range_tags_seal_insertion_refuses_before_binary_mutation() {
    let image = ImageFixture::new();
    let mut doc = document(&["anchor"]);
    tag(&mut doc.sections[0].paragraphs[0]);
    assert_document_refusal(doc, |doc| {
        crate::image::insert_seal(doc, "anchor", &image.0, Some(5.0), |_, _| None)
    });
}

#[test]
fn range_tags_object_deletion_refuses_before_removing_field_anchors() {
    let mut doc = field_document(false);
    tag(&mut doc.sections[0].paragraphs[0]);
    assert_document_refusal(doc, |doc| delete_object(doc, ObjectKind::Field, "entry"));
}

#[test]
fn range_tags_object_deletion_is_atomic_for_a_late_tagged_match() {
    let mut doc = field_document(true);
    tag(&mut doc.sections[0].paragraphs[1]);
    assert_document_refusal(doc, |doc| delete_object(doc, ObjectKind::Field, "entry"));
}

#[test]
fn range_tags_named_field_fill_checks_non_bmp_wchar_widths() {
    let mut doc = field_document(false);
    tag(&mut doc.sections[0].paragraphs[0]);
    assert_document_refusal(doc, |doc| crate::set_field(doc, "entry", "😀"));
}

#[test]
fn range_tags_set_cell_refusal_preserves_ancestor_raw_mirror() {
    let mut table_doc = table_document();
    with_nth_table(&mut table_doc, 0, |table| {
        let cell = table
            .cells
            .iter_mut()
            .find(|cell| cell.row == 1 && cell.col == 1)
            .unwrap();
        tag(&mut cell.paragraphs[0]);
    })
    .unwrap();
    let table = table_doc.sections[0]
        .paragraphs
        .iter()
        .flat_map(|para| &para.controls)
        .find(|control| matches!(control, Control::Table(_)))
        .unwrap()
        .clone();
    let mut inner = paragraph("nested table host");
    inner.controls.push(table);
    let mut doc = document(&["outer host"]);
    doc.sections[0].paragraphs[0]
        .controls
        .push(generic(inner, None));
    assert_document_refusal(doc, |doc| set_cell(doc, 0, 1, 1, "replacement"));
}

#[test]
fn range_tags_private_remove_controls_refuses_before_mutation() {
    let mut doc = document(&["anchor"]);
    crate::create_bookmark(&mut doc, "anchor", "bookmark")
        .operation_result()
        .unwrap();
    assert_eq!(doc.sections[0].paragraphs[0].controls.len(), 1);
    let mut para = doc.sections[0].paragraphs.remove(0);
    tag(&mut para);
    let before = para.clone();
    assert_refused(remove_controls(&mut para, &[0]));
    assert_eq!(para, before);
}

#[test]
fn range_tags_equal_normalized_wchar_splices_preserve_opaque_bytes() {
    for (text, range, old_width, replacement) in [
        ("AB", 0..2, 2, "😀"),
        ("12345678", 0..8, 8, "\t"),
        ("A", 0..1, 1, "B\r"),
        ("AB", 0..2, 2, "A\r\n"),
    ] {
        let mut para = paragraph(text);
        tag(&mut para);
        let extras = para.extras.clone();
        let old_len = para.wchar_len();
        splice_text(&mut para, range, 0, old_width, replacement)
            .operation_result()
            .expect("equal actual WCHAR widths do not shift range offsets");
        assert_eq!(para.extras, extras);
        assert_eq!(para.wchar_len(), old_len);
    }
}

#[test]
fn range_tags_normalized_width_growth_is_not_hidden_by_scalar_counts() {
    for replacement in ["\t", "😀"] {
        let mut para = paragraph("A");
        tag(&mut para);
        let before = para.clone();
        assert_refused(splice_text(&mut para, 0..1, 0, 1, replacement));
        assert_eq!(para, before);
    }
}

#[test]
fn range_tags_unselected_caption_does_not_block_body_replace() {
    let mut doc = document(&["needle"]);
    let mut caption = paragraph("unselected caption");
    tag(&mut caption);
    doc.sections[0].paragraphs[0].controls.push(generic(
        paragraph("unselected inner text"),
        Some(caption.clone()),
    ));
    replace_text(&mut doc, "needle", "expanded value", true)
        .operation_result()
        .unwrap();
    let Control::Generic(generic) = &doc.sections[0].paragraphs[0].controls[0] else {
        unreachable!()
    };
    assert_eq!(generic.caption.as_ref().unwrap().paragraphs[0], caption);
}

#[test]
fn range_tags_no_match_and_same_text_requests_are_allowed() {
    let mut doc = document(&["unchanged"]);
    tag(&mut doc.sections[0].paragraphs[0]);
    let before = doc.clone();
    replace_text(&mut doc, "absent", "long replacement", true)
        .operation_result()
        .unwrap();
    assert_eq!(doc, before);
    replace_text(&mut doc, "unchanged", "unchanged", true)
        .operation_result()
        .expect("a same-text request must not be rejected for range tags");
    assert_eq!(
        doc.sections[0].paragraphs[0].extras,
        before.sections[0].paragraphs[0].extras
    );
}

#[test]
fn field_tab_growth_refuses_tagged_named_field_edit() {
    let mut doc = field_document(false);
    tag(&mut doc.sections[0].paragraphs[0]);
    assert_document_refusal(doc, |doc| crate::set_field(doc, "entry", "\t"));
}

#[test]
fn field_tab_guard_keeps_same_width_non_tab_controls_available() {
    for value in ["B", "\n"] {
        let mut doc = field_document(false);
        tag(&mut doc.sections[0].paragraphs[0]);
        let extras = doc.sections[0].paragraphs[0].extras.clone();
        assert_eq!(crate::set_field(&mut doc, "entry", value).unwrap(), 1);
        assert_eq!(crate::list_fields(&doc)[0].value, value);
        assert_eq!(doc.sections[0].paragraphs[0].extras, extras);
    }
}

#[test]
fn field_tab_guard_allows_eight_wchar_replacement() {
    let mut doc = field_document(false);
    crate::set_field(&mut doc, "entry", "12345678").unwrap();
    tag(&mut doc.sections[0].paragraphs[0]);
    let extras = doc.sections[0].paragraphs[0].extras.clone();
    let result = crate::set_field(&mut doc, "entry", "\t");
    assert!(
        result.is_ok(),
        "equal serialized WCHAR widths must remain allowed: {result:?}"
    );
    assert_eq!(crate::list_fields(&doc)[0].value, "\t");
    assert_eq!(doc.sections[0].paragraphs[0].extras, extras);
}
