//! Synthetic core contracts; no native writer, parser, font or Hancom behavior is assumed.
use super::contract::{Binding, BindingPlan, BindingTarget, ProfileRepeat, Values};
use super::{
    FormOccurrence, apply_prepared_bindings, digest, prepare_binding_plan, scan_form_occurrences,
};
use hwp_model::{Control, Document, HwpChar, HwpUnit, Paragraph};

#[test]
fn prepare_apply_refuses_intervening_binary_and_preview_changes_omitted_by_json() {
    for preview in [false, true] {
        let mut doc = document();
        doc.sections[0].paragraphs[1] = paragraph("{{ key }}");
        doc.bin_streams.push(hwp_model::BinStream {
            name: "asset".into(),
            data: vec![1],
        });
        doc.hwpx_preview_image = Some(vec![1]);
        let item = occurrence(&doc, "placeholder", "key");
        let (plan, values) = plan(&doc, &item, "replacement", true);
        let prepared = prepare_binding_plan(&doc, &source_hash(), &plan, &values).unwrap();
        if preview {
            doc.hwpx_preview_image = Some(vec![2]);
        } else {
            doc.bin_streams[0].data = vec![2];
        }
        let before = doc.clone();
        assert!(apply_prepared_bindings(&mut doc, prepared).is_err());
        assert_eq!(doc, before);
    }
}

#[test]
fn opaque_paragraph_records_are_visible_but_never_relocated_by_a_binding() {
    let mut doc = document();
    doc.sections[0].paragraphs[1] = paragraph("{{ private }}");
    doc.sections[0].paragraphs[1]
        .extras
        .push(hwp_model::OpaqueRecord {
            tag: u16::MAX,
            data: vec![1, 2, 3],
            children: vec![],
        });
    let item = occurrence(&doc, "placeholder", "private");
    assert_unsupported(&item, "rich_target");
    let (plan, values) = plan(&doc, &item, "replacement", true);
    let before = doc.clone();
    assert!(prepare_binding_plan(&doc, &source_hash(), &plan, &values).is_err());
    assert_eq!(doc, before);
}

fn source_hash() -> String {
    digest(b"owner-authored model source identity")
}
fn paragraph(text: &str) -> Paragraph {
    Paragraph {
        chars: text
            .chars()
            .map(HwpChar::Text)
            .chain([HwpChar::CharCtrl(13)])
            .collect(),
        char_shape_runs: vec![(0, hwp_model::CharShapeId(0))],
        ..Default::default()
    }
}
fn document() -> Document {
    let mut doc = crate::from_markdown("Setup.\n\nTarget.\n\nUntouched.\n");
    doc.sections[0].paragraphs[1] = paragraph("Target.");
    doc.sections[0].paragraphs[2] = paragraph("Untouched.");
    doc
}
fn caption(para: Paragraph) -> hwp_model::Caption {
    hwp_model::Caption {
        side: hwp_model::CaptionSide::Bottom,
        direction: hwp_model::CaptionDirection::Horizontal,
        gap: 0,
        width: None,
        last_width: 0,
        paragraphs: vec![para],
    }
}
fn table(para: Paragraph) -> hwp_model::Table {
    hwp_model::Table {
        common_data: vec![],
        placement: None,
        attr: 0,
        rows: 1,
        cols: 1,
        cell_spacing: 0,
        inner_margins: [0; 4],
        row_cell_counts: vec![1],
        border_fill: hwp_model::BorderFillId(0),
        table_tail: vec![],
        cells: vec![hwp_model::Cell {
            list_attr: 0,
            col: 0,
            row: 0,
            col_span: 1,
            row_span: 1,
            width: HwpUnit(1000),
            height: HwpUnit(1000),
            margins: [0; 4],
            border_fill: hwp_model::BorderFillId(0),
            header_tail: vec![],
            paragraphs: vec![para],
        }],
        caption: None,
        extras: vec![],
    }
}
fn generic() -> hwp_model::GenericControl {
    hwp_model::GenericControl {
        ctrl_id: *b"gso ",
        data: vec![],
        paragraph_lists: vec![],
        extras: vec![],
        raw_children: vec![],
        gso_shapes: vec![],
        equation: None,
        column_def: None,
        caption: None,
        hwpx_raw_xml: None,
        container_box: None,
    }
}
fn picture() -> hwp_model::Picture {
    hwp_model::Picture {
        common_data: vec![],
        width: HwpUnit(100),
        height: HwpUnit(100),
        treat_as_char: true,
        z_order: 0,
        vert_offset: 0,
        horz_offset: 0,
        description: None,
        crop: None,
        flip: 0,
        rotation: None,
        brightness: 0,
        contrast: 0,
        effect_flags: 0,
        effects_raw: vec![],
        caption: None,
        bin_ref: hwp_model::BinRef::ItemRef("unused-model-image".into()),
        extras: vec![],
    }
}
fn list(para: Paragraph) -> hwp_model::ParagraphList {
    hwp_model::ParagraphList {
        header_data: vec![],
        paragraphs: vec![para],
    }
}
fn occurrence(doc: &Document, kind: &str, label: &str) -> FormOccurrence {
    let found: Vec<_> = scan_form_occurrences(doc, &source_hash())
        .unwrap()
        .occurrences
        .into_iter()
        .filter(|item| item.kind == kind && item.label == label)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "fixture must expose one exact kind/label occurrence"
    );
    found.into_iter().next().unwrap()
}
fn assert_unsupported(item: &FormOccurrence, cause: &str) {
    assert!(!item.bindable);
    assert_eq!(item.cause.as_deref(), Some(cause));
    assert!(
        item.next_step
            .as_deref()
            .is_some_and(|next| !next.is_empty())
    );
    assert_eq!(item.id.len(), 68);
    assert!(item.id.starts_with("occ-"));
}
fn plan(
    doc: &Document,
    item: &FormOccurrence,
    value: &str,
    allow_reflow: bool,
) -> (BindingPlan, Values) {
    let catalogue = scan_form_occurrences(doc, &source_hash()).unwrap();
    (
        BindingPlan {
            version: 1,
            source_sha256: source_hash(),
            structural_sha256: catalogue.structural_sha256,
            bindings: vec![Binding {
                key: "literal-key".into(),
                targets: vec![BindingTarget {
                    occurrence_id: item.id.clone(),
                    copies: vec![0],
                    max_chars: 20000,
                    allow_reflow,
                }],
            }],
            repeats: vec![],
            constraints: vec![],
        },
        [("literal-key".into(), value.into())].into_iter().collect(),
    )
}

#[test]
fn body_slot_discovery_has_exact_unicode_scalar_claim() {
    let mut doc = document();
    doc.sections[0].paragraphs[1] = paragraph("앞😀 {{이름}} tail");
    let before = doc.clone();
    let item = occurrence(&doc, "placeholder", "이름");
    assert!(item.bindable);
    assert_eq!(item.model_route, "s0/p1");
    assert_eq!((item.start, item.end), (3, 9));
    assert_eq!(item.old_text, "{{이름}}");
    assert_eq!(item.prior_sha256, digest("{{이름}}".as_bytes()));
    assert!(item.paragraph_id.is_some());
    assert_eq!(doc, before);
}
#[test]
fn cell_paragraph_and_slot_are_separate_review_claims() {
    let mut doc = document();
    doc.sections[0].paragraphs[1]
        .controls
        .push(Control::Table(table(paragraph("{{cell-key}}"))));
    let slot = occurrence(&doc, "placeholder", "cell-key");
    let cells: Vec<_> = scan_form_occurrences(&doc, &source_hash())
        .unwrap()
        .occurrences
        .into_iter()
        .filter(|item| item.kind == "cell_text")
        .collect();
    assert_eq!(cells.len(), 1);
    let cell = &cells[0];
    assert!(slot.bindable && cell.bindable);
    assert_ne!(slot.id, cell.id);
    assert_eq!(slot.indices, vec![1, 0, 0, 0]);
    assert_eq!(slot.table_id, cell.table_id);
    assert_eq!(
        (cell.row, cell.col, cell.table_index),
        (Some(0), Some(0), Some(0))
    );
}
#[test]
fn table_caption_slot_is_discovered_but_has_no_editable_paragraph_id() {
    let mut doc = document();
    let mut table = table(paragraph("cell"));
    table.caption = Some(caption(paragraph("{{table-caption}}")));
    doc.sections[0].paragraphs[1]
        .controls
        .push(Control::Table(table));
    let item = occurrence(&doc, "placeholder", "table-caption");
    assert_unsupported(&item, "caption_target");
    assert!(item.paragraph_id.is_none());
    assert!(item.table_index.is_none());
    assert_eq!(item.model_route, "s0/p1/c0/caption/p0");
}
#[test]
fn picture_caption_slot_is_discovered_but_unsupported() {
    let mut doc = document();
    let mut picture = picture();
    picture.caption = Some(caption(paragraph("{{picture-caption}}")));
    doc.sections[0].paragraphs[1]
        .controls
        .push(Control::Picture(picture));
    let item = occurrence(&doc, "placeholder", "picture-caption");
    assert_unsupported(&item, "caption_target");
    assert!(item.paragraph_id.is_none());
    assert_eq!(item.model_route, "s0/p1/c0/caption/p0");
}
#[test]
fn generic_caption_slot_is_discovered_but_unsupported() {
    let mut doc = document();
    let mut generic = generic();
    generic.caption = Some(caption(paragraph("{{generic-caption}}")));
    doc.sections[0].paragraphs[1]
        .controls
        .push(Control::Generic(generic));
    let item = occurrence(&doc, "placeholder", "generic-caption");
    assert_unsupported(&item, "caption_target");
    assert!(item.paragraph_id.is_none());
}
#[test]
fn generic_lists_have_distinct_flattened_edit_paths() {
    let mut doc = document();
    let mut generic = generic();
    generic.paragraph_lists = vec![
        list(paragraph("{{list-one}}")),
        list(paragraph("{{list-two}}")),
    ];
    doc.sections[0].paragraphs[1]
        .controls
        .push(Control::Generic(generic));
    let first = occurrence(&doc, "placeholder", "list-one");
    let second = occurrence(&doc, "placeholder", "list-two");
    assert!(first.bindable && second.bindable);
    assert_eq!(first.indices, vec![1, 0, 0]);
    assert_eq!(second.indices, vec![1, 0, 1]);
    assert_eq!(second.model_route, "s0/p1/c0/list1/p0");
    assert_ne!(first.id, second.id);
}
#[test]
fn nested_caption_discovery_retains_unsupported_ancestry() {
    let mut doc = document();
    let mut inner = generic();
    inner.caption = Some(caption(paragraph("{{nested-caption}}")));
    let outer_para = Paragraph {
        controls: vec![Control::Generic(inner)],
        ..paragraph("outer caption")
    };
    let mut table = table(paragraph("cell"));
    table.caption = Some(caption(outer_para));
    doc.sections[0].paragraphs[1]
        .controls
        .push(Control::Table(table));
    let item = occurrence(&doc, "placeholder", "nested-caption");
    assert_unsupported(&item, "caption_target");
    assert!(item.paragraph_id.is_none());
    assert_eq!(item.model_route, "s0/p1/c0/caption/p0/c0/caption/p0");
}
fn raw_list_case(captured_xml: bool) {
    let mut doc = document();
    let mut generic = generic();
    generic
        .paragraph_lists
        .push(list(paragraph("{{raw-list}}")));
    if captured_xml {
        generic.hwpx_raw_xml = Some("<captured/>".into());
    } else {
        generic.raw_children.push(hwp_model::OpaqueRecord {
            tag: 0x7777,
            data: vec![],
            children: vec![],
        });
    }
    doc.sections[0].paragraphs[1]
        .controls
        .push(Control::Generic(generic));
    let item = occurrence(&doc, "placeholder", "raw-list");
    assert_unsupported(&item, "raw_preserved_target");
    let (plan, values) = plan(&doc, &item, "replacement", true);
    let before = doc.clone();
    assert!(prepare_binding_plan(&doc, &source_hash(), &plan, &values).is_err());
    assert_eq!(doc, before);
}
#[test]
fn raw_children_generic_list_is_inventory_only() {
    raw_list_case(false);
}
#[test]
fn captured_xml_generic_list_is_inventory_only() {
    raw_list_case(true);
}
fn native_field_para() -> Paragraph {
    let mut doc = document();
    doc.sections[0].paragraphs[1] = paragraph("FIELD-ANCHOR");
    assert!(crate::field::create_field(
        &mut doc,
        "FIELD-ANCHOR",
        "owner-native-field",
        "value"
    ));
    doc.sections[0].paragraphs[1].clone()
}
fn assert_native_inventory(doc: &Document, inherited_cause: &str) {
    let item = occurrence(doc, "native_field", "owner-native-field");
    assert_unsupported(&item, inherited_cause);
    assert_eq!(item.next_step.as_deref(), Some("hwp fields"));
}
#[test]
fn native_field_is_individually_listed_and_not_bindable() {
    let mut doc = document();
    doc.sections[0].paragraphs[1] = native_field_para();
    assert_native_inventory(&doc, "native_field");
}
#[test]
fn native_field_in_caption_still_directs_to_fields_surface() {
    let mut doc = document();
    let mut generic = generic();
    generic.caption = Some(caption(native_field_para()));
    doc.sections[0].paragraphs[1]
        .controls
        .push(Control::Generic(generic));
    assert_native_inventory(&doc, "caption_target");
}
#[test]
fn native_field_in_raw_list_still_directs_to_fields_surface() {
    let mut doc = document();
    let mut generic = generic();
    generic.hwpx_raw_xml = Some("<captured/>".into());
    generic.paragraph_lists.push(list(native_field_para()));
    doc.sections[0].paragraphs[1]
        .controls
        .push(Control::Generic(generic));
    assert_native_inventory(&doc, "raw_preserved_target");
}
#[test]
fn mixed_style_slot_is_discovered_but_rich_and_unsupported() {
    let mut doc = document();
    doc.sections[0].paragraphs[1] = paragraph("{{rich}} tail");
    doc.sections[0].paragraphs[1]
        .char_shape_runs
        .push((8, hwp_model::CharShapeId(1)));
    let item = occurrence(&doc, "placeholder", "rich");
    assert_unsupported(&item, "rich_target");
}
#[test]
fn exact_unicode_splice_preserves_other_paragraph_styles_and_runs() {
    let mut doc = document();
    doc.sections[0].paragraphs[1] = paragraph("앞😀 {{이름}} tail");
    doc.sections[0].paragraphs[2].style = hwp_model::StyleId(1);
    doc.sections[0].paragraphs[2].char_shape_runs = vec![
        (0, hwp_model::CharShapeId(1)),
        (4, hwp_model::CharShapeId(2)),
    ];
    let untouched = doc.sections[0].paragraphs[2].clone();
    let header = doc.header.clone();
    let item = occurrence(&doc, "placeholder", "이름");
    let (plan, values) = plan(&doc, &item, "새값", false);
    let before = doc.clone();
    let prepared =
        prepare_binding_plan(&doc, &source_hash(), &plan, &values).expect("precise Unicode claim");
    assert_eq!(doc, before);
    assert_eq!(prepared.preview.changes.len(), 1);
    assert_eq!(prepared.preview.changes[0].old_text, "{{이름}}");
    let preview = apply_prepared_bindings(&mut doc, prepared).unwrap();
    assert_eq!(preview.changes[0].new_text, "새값");
    assert_eq!(doc.sections[0].paragraphs[1].plain_text(), "앞😀 새값 tail");
    assert_eq!(doc.sections[0].paragraphs[2], untouched);
    assert_eq!(doc.header, header);
}
#[test]
fn prepared_candidate_rejects_stale_source_hash_and_fingerprint_without_mutation() {
    let mut doc = document();
    doc.sections[0].paragraphs[1] = paragraph("{{target}}");
    let item = occurrence(&doc, "placeholder", "target");
    let (mut plan, values) = plan(&doc, &item, "value", false);
    let before = doc.clone();
    assert!(prepare_binding_plan(&doc, &digest(b"different source"), &plan, &values).is_err());
    assert_eq!(doc, before);
    plan.structural_sha256 = digest(b"different IR");
    assert!(prepare_binding_plan(&doc, &source_hash(), &plan, &values).is_err());
    assert_eq!(doc, before);
}
#[test]
fn prepared_apply_rejects_document_changed_after_prepare_atomically() {
    let mut doc = document();
    doc.sections[0].paragraphs[1] = paragraph("{{target}}");
    let item = occurrence(&doc, "placeholder", "target");
    let (plan, values) = plan(&doc, &item, "value", false);
    let prepared = prepare_binding_plan(&doc, &source_hash(), &plan, &values).unwrap();
    doc.sections[0].paragraphs[2] = paragraph("Changed meanwhile.");
    let changed = doc.clone();
    assert!(apply_prepared_bindings(&mut doc, prepared).is_err());
    assert_eq!(doc, changed);
}
#[test]
fn full_cell_and_slot_overlapping_claims_refuse_atomically() {
    let mut doc = document();
    doc.sections[0].paragraphs[1]
        .controls
        .push(Control::Table(table(paragraph("{{target}}"))));
    let catalogue = scan_form_occurrences(&doc, &source_hash()).unwrap();
    let slot = occurrence(&doc, "placeholder", "target");
    let cell = catalogue
        .occurrences
        .iter()
        .find(|item| item.kind == "cell_text")
        .unwrap();
    let (mut plan, values) = plan(&doc, &slot, "value", false);
    plan.bindings[0].targets.push(BindingTarget {
        occurrence_id: cell.id.clone(),
        copies: vec![0],
        max_chars: 20000,
        allow_reflow: false,
    });
    let before = doc.clone();
    let error = prepare_binding_plan(&doc, &source_hash(), &plan, &values)
        .err()
        .expect("overlapping distinct claims must refuse");
    assert!(error.contains("겹침") || error.contains("중복"));
    assert_eq!(doc, before);
}
#[test]
fn adjacent_slot_claims_are_disjoint_and_both_apply() {
    let mut doc = document();
    doc.sections[0].paragraphs[1] = paragraph("{{a}}{{b}}");
    let a = occurrence(&doc, "placeholder", "a");
    let b = occurrence(&doc, "placeholder", "b");
    assert_eq!(a.end, b.start);
    let (mut plan, values) = plan(&doc, &a, "X", false);
    plan.bindings[0].targets.push(BindingTarget {
        occurrence_id: b.id,
        copies: vec![0],
        max_chars: 20000,
        allow_reflow: false,
    });
    let prepared = prepare_binding_plan(&doc, &source_hash(), &plan, &values)
        .expect("adjacent half-open claims must not overlap");
    apply_prepared_bindings(&mut doc, prepared).unwrap();
    assert_eq!(doc.sections[0].paragraphs[1].plain_text(), "XX");
}
#[test]
fn nonreflow_budget_counts_wchars_and_explicit_growth_needs_permission() {
    let mut doc = document();
    doc.sections[0].paragraphs[1] = paragraph("{{x}}");
    let item = occurrence(&doc, "placeholder", "x");
    let (mut plan, values) = plan(&doc, &item, "😀😀😀", false);
    let before = doc.clone();
    assert!(prepare_binding_plan(&doc, &source_hash(), &plan, &values).is_err());
    assert_eq!(doc, before);
    plan.bindings[0].targets[0].allow_reflow = true;
    assert!(prepare_binding_plan(&doc, &source_hash(), &plan, &values).is_ok());
    plan.bindings[0].targets[0].max_chars = 2;
    assert!(prepare_binding_plan(&doc, &source_hash(), &plan, &values).is_err());
    assert_eq!(doc, before);
}
#[test]
fn copy_count_includes_original_and_uses_explicit_postclone_routes() {
    let mut doc = document();
    doc.sections[0].paragraphs[1] = paragraph("Clone anchor.");
    doc.sections[0].paragraphs[2]
        .controls
        .push(Control::Table(table(paragraph("Old."))));
    let catalogue = scan_form_occurrences(&doc, &source_hash()).unwrap();
    let item = catalogue
        .occurrences
        .iter()
        .find(|item| item.kind == "cell_text")
        .unwrap();
    let (mut plan, values) = plan(&doc, item, "New", false);
    plan.bindings[0].targets[0].copies = vec![0, 1, 2, 3];
    let anchor_path = crate::segment_id::SegmentPath {
        section: 0,
        indices: vec![1],
    };
    plan.repeats.push(ProfileRepeat {
        table_id: item.table_id.clone().unwrap(),
        after_paragraph_id: crate::segment_id::paragraph_id(
            &anchor_path,
            &doc.sections[0].paragraphs[1],
        ),
        copies: 4,
        profile_ids: (0..4).map(|index| format!("profile-{index}")).collect(),
    });
    let before = doc.clone();
    let prepared = prepare_binding_plan(&doc, &source_hash(), &plan, &values)
        .expect("one profile plus three real clones");
    assert_eq!(prepared.preview.copies.len(), 4);
    assert_eq!(prepared.preview.changes.len(), 4);
    let copy_indices: std::collections::BTreeSet<_> = prepared
        .preview
        .copies
        .iter()
        .map(|copy| copy.copy)
        .collect();
    assert_eq!(copy_indices, [0, 1, 2, 3].into_iter().collect());
    assert_eq!(doc, before);
    apply_prepared_bindings(&mut doc, prepared).unwrap();
    let tables: Vec<_> = doc.sections[0]
        .paragraphs
        .iter()
        .flat_map(|para| &para.controls)
        .filter_map(|control| match control {
            Control::Table(table) => Some(table),
            _ => None,
        })
        .collect();
    assert_eq!(tables.len(), 4);
    for table in tables {
        assert_eq!(table.cells[0].paragraphs[0].plain_text(), "New");
    }
}

#[test]
fn repeated_outer_profile_maps_nested_table_targets_to_original_and_copy() {
    let mut doc = document();
    doc.sections[0].paragraphs[1] = paragraph("Unique outer profile anchor.");
    let mut outer_cell = paragraph("Outer cell text stays unchanged.");
    outer_cell
        .controls
        .push(Control::Table(table(paragraph("Old nested value."))));
    doc.sections[0].paragraphs[2]
        .controls
        .push(Control::Table(table(outer_cell)));
    let before = doc.clone();
    let discovery = scan_form_occurrences(&doc, &source_hash()).unwrap();
    let nested = discovery
        .occurrences
        .iter()
        .find(|item| item.kind == "cell_text" && item.old_text == "Old nested value.")
        .expect("nested table plain value");
    assert!(nested.bindable);
    let outer_path = crate::segment_id::SegmentPath {
        section: 0,
        indices: vec![2, 0],
    };
    let Control::Table(outer) = &doc.sections[0].paragraphs[2].controls[0] else {
        panic!("outer source table")
    };
    let outer_id = crate::segment_id::table_id(&outer_path, outer);
    assert_ne!(
        nested.table_id.as_deref(),
        Some(outer_id.as_str()),
        "target belongs to nested table, repeat belongs to its outer profile"
    );
    let outer_text = outer.cells[0].paragraphs[0].chars.clone();
    let (mut plan, values) = plan(&doc, nested, "Nested changed.", true);
    plan.bindings[0].targets[0].copies = vec![0, 1];
    let anchor_path = crate::segment_id::SegmentPath {
        section: 0,
        indices: vec![1],
    };
    plan.repeats.push(ProfileRepeat {
        table_id: outer_id,
        after_paragraph_id: crate::segment_id::paragraph_id(
            &anchor_path,
            &doc.sections[0].paragraphs[1],
        ),
        copies: 2,
        profile_ids: vec!["outer-original".into(), "outer-copy".into()],
    });
    let prepared = prepare_binding_plan(&doc, &source_hash(), &plan, &values)
        .expect("nested occurrence must map through its outer profile's source-path prefix");
    assert_eq!(doc, before);
    assert_eq!(prepared.preview.changes.len(), 2);
    let copies: std::collections::BTreeSet<_> = prepared
        .preview
        .changes
        .iter()
        .map(|change| change.copy)
        .collect();
    assert_eq!(copies, [0, 1].into_iter().collect());
    apply_prepared_bindings(&mut doc, prepared).unwrap();
    let outers: Vec<_> = doc.sections[0]
        .paragraphs
        .iter()
        .flat_map(|para| &para.controls)
        .filter_map(|control| match control {
            Control::Table(table) => Some(table),
            _ => None,
        })
        .collect();
    assert_eq!(outers.len(), 2);
    for outer in outers {
        let host = &outer.cells[0].paragraphs[0];
        assert_eq!(host.chars, outer_text);
        let nested = host
            .controls
            .iter()
            .find_map(|control| match control {
                Control::Table(table) => Some(table),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            nested.cells[0].paragraphs[0].plain_text(),
            "Nested changed."
        );
    }
    assert_eq!(
        doc.sections[0].paragraphs[0],
        before.sections[0].paragraphs[0]
    );
    assert_eq!(
        doc.sections[0].paragraphs[1],
        before.sections[0].paragraphs[1]
    );
    assert_eq!(
        doc.sections[0].paragraphs[3].chars, before.sections[0].paragraphs[2].chars,
        "original body-owned text remains unchanged after the clone insertion"
    );
}

#[test]
fn later_repeat_group_insertion_updates_previously_recorded_copy_routes() {
    let mut doc = document();
    doc.sections[0].paragraphs[1] = paragraph("Unique A anchor.");
    doc.sections[0].paragraphs[2]
        .controls
        .push(Control::Table(table(paragraph("A original value."))));
    doc.sections[0]
        .paragraphs
        .push(paragraph("Unique B anchor."));
    let mut b_host = paragraph("B body stays.");
    b_host
        .controls
        .push(Control::Table(table(paragraph("B original value."))));
    doc.sections[0].paragraphs.push(b_host);
    doc.sections[0]
        .paragraphs
        .push(paragraph("Unchanged final body paragraph."));
    let before = doc.clone();
    let discovery = scan_form_occurrences(&doc, &source_hash()).unwrap();
    let a = discovery
        .occurrences
        .iter()
        .find(|item| item.kind == "cell_text" && item.old_text == "A original value.")
        .unwrap();
    let b = discovery
        .occurrences
        .iter()
        .find(|item| item.kind == "cell_text" && item.old_text == "B original value.")
        .unwrap();
    let (mut plan, mut values) = plan(&doc, a, "A new", false);
    plan.bindings[0].targets[0].copies = vec![0, 1];
    plan.bindings.push(Binding {
        key: "second-key".into(),
        targets: vec![BindingTarget {
            occurrence_id: b.id.clone(),
            copies: vec![0, 1],
            max_chars: 100,
            allow_reflow: false,
        }],
    });
    values.insert("second-key".into(), "B new".into());
    // Repeat B first, then insert A's copy earlier in the body, shifting B's recorded routes.
    for (item, anchor, prefix) in [(b, 3, "B"), (a, 1, "A")] {
        let path = crate::segment_id::SegmentPath {
            section: 0,
            indices: vec![anchor],
        };
        plan.repeats.push(ProfileRepeat {
            table_id: item.table_id.clone().unwrap(),
            after_paragraph_id: crate::segment_id::paragraph_id(
                &path,
                &doc.sections[0].paragraphs[anchor],
            ),
            copies: 2,
            profile_ids: vec![format!("{prefix}-original"), format!("{prefix}-copy")],
        });
    }
    let prepared = prepare_binding_plan(&doc, &source_hash(), &plan, &values)
        .expect("later insertion must shift earlier copies and original routes consistently");
    assert_eq!(doc, before);
    assert_eq!(prepared.preview.copies.len(), 4);
    assert_eq!(prepared.preview.changes.len(), 4);
    let b_copy = prepared
        .preview
        .copies
        .iter()
        .find(|copy| copy.profile_id == "B-copy")
        .unwrap();
    assert_eq!(b_copy.indices, vec![5, 0]);
    let b_original = prepared
        .preview
        .copies
        .iter()
        .find(|copy| copy.profile_id == "B-original")
        .unwrap();
    assert_eq!(b_original.indices, vec![6, 0]);
    let preview = apply_prepared_bindings(&mut doc, prepared).unwrap();
    for change in &preview.changes {
        let path = crate::segment_id::SegmentPath {
            section: change.section,
            indices: change.indices.clone(),
        };
        let paragraph = crate::address::paragraph_at_mut(&mut doc, &path).unwrap();
        let wanted = if change.profile_id.as_deref().unwrap().starts_with('A') {
            "A new"
        } else {
            "B new"
        };
        assert_eq!(paragraph.plain_text(), wanted);
    }
    assert_eq!(
        doc.sections[0].paragraphs[1],
        before.sections[0].paragraphs[1]
    );
    assert_eq!(
        doc.sections[0].paragraphs[4],
        before.sections[0].paragraphs[3]
    );
    assert_eq!(
        doc.sections[0].paragraphs.last(),
        before.sections[0].paragraphs.last()
    );
}
