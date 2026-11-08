//! Public-API resource closure regressions for unreadable retained inputs (#417 F28).
//! Synthetic data exercises refusal to prove absence; it establishes no native format behavior.
use hwp_convert::document_split::{self as split, RangeAnchor, RangeOutcome, RangeSelector};
use hwp_model::{
    BinRef, BinStream, Control, Document, GenericControl, OpaqueRecord, Picture, RawEntry,
};

const UNUSED: &str = "BinData/unused.png";
const BORDER_FILL: u16 = 0x10 + 4;

fn document() -> Document {
    let mut doc = hwp_convert::from_markdown("Outside.\n\nSelected.\n\nAfter.\n");
    assert_eq!(doc.sections[0].paragraphs.len(), 3);
    doc.bin_streams = vec![BinStream {
        name: UNUSED.into(),
        data: b"owner-authored unused bytes".to_vec(),
    }];
    doc.hwpx_bin_manifest = vec![("unused".into(), UNUSED.into())];
    doc
}

fn extract(doc: &Document) -> RangeOutcome {
    split::extract_range(
        doc,
        "synthetic-source",
        &RangeSelector {
            start: RangeAnchor::Address {
                section: 0,
                paragraph: 1,
            },
            end: Some(RangeAnchor::Address {
                section: 0,
                paragraph: 1,
            }),
            end_inclusive: true,
            source_sha256: None,
        },
    )
    .expect("synthetic selected range must resolve before closure assertions")
}

fn assert_unverified(doc: &Document, code: &str) {
    let outcome = extract(doc);
    assert!(
        outcome
            .resources
            .kept_unverified
            .iter()
            .any(|name| name == UNUSED),
        "unreadable {code} must retain the unproven stream: {:?}",
        outcome.resources
    );
    assert!(!outcome.resources.excluded.iter().any(|name| name == UNUSED));
    assert!(
        outcome
            .resources
            .unverified
            .iter()
            .any(|site| site.code == code),
        "must report {code}: {:?}",
        outcome.resources
    );
    assert_eq!(
        outcome
            .fragment
            .bin_streams
            .iter()
            .find(|stream| stream.name == UNUSED)
            .unwrap()
            .data,
        b"owner-authored unused bytes"
    );
}

fn assert_excluded(doc: &Document) {
    let outcome = extract(doc);
    assert!(
        outcome.resources.unverified.is_empty(),
        "recognized non-image input must not add uncertainty: {:?}",
        outcome.resources
    );
    assert_eq!(outcome.resources.excluded, vec![UNUSED]);
    assert!(outcome.resources.kept_unverified.is_empty());
}

fn opaque(tag: u16, data: Vec<u8>) -> OpaqueRecord {
    OpaqueRecord {
        tag,
        data,
        children: vec![],
    }
}

fn generic() -> GenericControl {
    GenericControl {
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

fn xml_case(xml: &str) {
    let mut doc = document();
    let mut object = generic();
    object.hwpx_raw_xml = Some(xml.into());
    doc.sections[0].paragraphs[1]
        .controls
        .push(Control::Generic(object));
    assert_unverified(&doc, "raw_xml_unparsed");
}

fn metadata_reference(settings: bool) {
    let mut doc = document();
    let xml = "<root binaryItemIDRef=\"unused\"/>".into();
    if settings {
        doc.hwpx_settings_xml = Some(xml);
    } else {
        doc.hwpx_version_xml = Some(xml);
    }
    let outcome = extract(&doc);
    assert_eq!(
        outcome.resources.kept,
        vec![UNUSED],
        "retained metadata XML reference must be resolved as proven used"
    );
    assert!(outcome.resources.excluded.is_empty());
    assert_eq!(outcome.fragment.bin_streams, doc.bin_streams);
}

fn metadata_malformed(settings: bool) {
    let mut doc = document();
    let xml = "<root binaryItemIDRef=\"unused\"".into();
    if settings {
        doc.hwpx_settings_xml = Some(xml);
    } else {
        doc.hwpx_version_xml = Some(xml);
    }
    assert_unverified(&doc, "raw_xml_unparsed");
}

#[test]
fn settings_xml_binary_reference_keeps_stream() {
    metadata_reference(true);
}
#[test]
fn settings_xml_malformed_keeps_unverified_stream() {
    metadata_malformed(true);
}
#[test]
fn version_xml_binary_reference_keeps_stream() {
    metadata_reference(false);
}
#[test]
fn version_xml_malformed_keeps_unverified_stream() {
    metadata_malformed(false);
}

#[test]
fn truncated_raw_header_border_fill_keeps_unverified_stream() {
    let mut doc = document();
    doc.header.extras.push(opaque(BORDER_FILL, vec![0; 3]));
    assert_unverified(&doc, "border_fill_image");
}
#[test]
fn nested_raw_header_border_fill_keeps_unverified_stream() {
    let mut doc = document();
    let mut wrapper = opaque(0, vec![]);
    wrapper.children.push(opaque(BORDER_FILL, vec![0; 3]));
    doc.header.id_extras.push(wrapper);
    assert_unverified(&doc, "border_fill_image");
}
#[test]
fn empty_raw_header_document_data_keeps_unverified_stream() {
    let mut doc = document();
    doc.header.extras.push(opaque(split::TAG_DOC_DATA, vec![]));
    assert_unverified(&doc, "parameter_set");
}

macro_rules! raw_header_case {
    ($name:ident, $tag:expr, $code:literal) => {
        #[test]
        fn $name() {
            let mut doc = document();
            doc.header.extras.push(opaque($tag, vec![]));
            assert_unverified(&doc, $code);
        }
    };
}
raw_header_case!(
    raw_header_ctrl_data_keeps_unverified_stream,
    split::TAG_CTRL_DATA,
    "parameter_set"
);
raw_header_case!(
    raw_header_shape_keeps_unverified_stream,
    split::TAG_SHAPE_COMPONENT,
    "raw_shape"
);
raw_header_case!(
    raw_header_ole_keeps_unverified_stream,
    split::TAG_SHAPE_COMPONENT_OLE,
    "raw_ole"
);
raw_header_case!(
    raw_header_picture_keeps_unverified_stream,
    split::TAG_SHAPE_COMPONENT_PICTURE,
    "raw_picture"
);
raw_header_case!(
    raw_header_video_keeps_unverified_stream,
    split::TAG_VIDEO_DATA,
    "raw_video"
);
raw_header_case!(
    raw_header_bullet_keeps_unverified_stream,
    split::TAG_BULLET,
    "bullet_image"
);

fn modeled_picture_doc(record: OpaqueRecord, reference: BinRef) -> Document {
    let mut doc = document();
    doc.bin_streams.push(BinStream {
        name: "BinData/modeled.png".into(),
        data: b"own modeled bytes".to_vec(),
    });
    doc.hwpx_bin_manifest
        .push(("modeled".into(), "BinData/modeled.png".into()));
    let picture = Picture {
        common_data: vec![],
        width: hwp_model::HwpUnit(100),
        height: hwp_model::HwpUnit(100),
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
        bin_ref: reference,
        extras: vec![record],
    };
    doc.sections[0].paragraphs[1]
        .controls
        .push(Control::Picture(picture));
    doc
}

fn picture_shape(data: Vec<u8>) {
    let doc = modeled_picture_doc(
        opaque(split::TAG_SHAPE_COMPONENT, data),
        BinRef::ItemRef("modeled".into()),
    );
    assert_unverified(&doc, "raw_shape");
    assert_eq!(extract(&doc).resources.kept, vec!["BinData/modeled.png"]);
}

#[test]
fn modeled_picture_shape_shorter_than_object_ids_keeps_unverified_stream() {
    picture_shape(vec![0; 7]);
}
#[test]
fn modeled_picture_shape_missing_transform_count_keeps_unverified_stream() {
    picture_shape(vec![0; 49]);
}
#[test]
fn modeled_picture_shape_transform_offset_past_end_keeps_unverified_stream() {
    let mut data = vec![0; 52];
    data[50..52].copy_from_slice(&u16::MAX.to_le_bytes());
    picture_shape(data);
}

#[test]
fn generic_truncated_parameter_set_keeps_unverified_stream() {
    let mut doc = document();
    let mut object = generic();
    object
        .raw_children
        .push(opaque(split::TAG_CTRL_DATA, vec![0x1b, 2]));
    doc.sections[0].paragraphs[1]
        .controls
        .push(Control::Generic(object));
    assert_unverified(&doc, "parameter_set");
}
#[test]
fn malformed_raw_bullet_keeps_unverified_stream() {
    let mut doc = document();
    doc.header.bullets.push(RawEntry {
        data: vec![0; 3],
        children: vec![],
    });
    assert_unverified(&doc, "bullet_image");
}
#[test]
fn malformed_retained_xml_keeps_unverified_stream() {
    xml_case("<root><child></root>");
}
#[test]
fn malformed_retained_xml_attribute_keeps_unverified_stream() {
    xml_case("<root binaryItemIDRef=unused/>");
}
#[test]
fn unresolved_retained_xml_entity_keeps_unverified_stream() {
    xml_case("<root binaryItemIDRef=\"&unknown;\"/>");
}
#[test]
fn invalid_utf8_carried_xml_keeps_unverified_stream() {
    let mut doc = document();
    doc.hwpx_extra_entries
        .push(("DocOptions/unknown.xml".into(), vec![0xff]));
    assert_unverified(&doc, "carried_part_unread");
}
#[test]
fn non_xml_carried_part_keeps_unverified_stream() {
    let mut doc = document();
    doc.hwpx_extra_entries
        .push(("Scripts/unknown.bin".into(), b"opaque own bytes".to_vec()));
    assert_unverified(&doc, "carried_part_unread");
}

#[test]
fn known_plain_character_bullet_allows_unused_stream_exclusion() {
    let mut doc = document();
    // Existing B7 synthetic character-bullet fixture shape, not a new binary layout.
    let mut data = vec![0x08, 0, 0, 0, 0, 0, 0x32, 0];
    data.extend_from_slice(&[0xff; 4]);
    data.extend_from_slice(&('•' as u16).to_le_bytes());
    data.extend_from_slice(&[0; 11]);
    doc.header.bullets.push(RawEntry {
        data,
        children: vec![],
    });
    assert_excluded(&doc);
}
#[test]
fn known_bookmark_parameter_set_allows_unused_stream_exclusion() {
    let mut doc = document();
    let mut object = generic();
    object.ctrl_id = *b"bokm";
    object.raw_children.push(opaque(
        split::TAG_CTRL_DATA,
        hwp_convert::bookmark::make_bokm_ctrl_data("owner-authored bookmark"),
    ));
    doc.sections[0].paragraphs[1]
        .controls
        .push(Control::Generic(object));
    assert_excluded(&doc);
}
#[test]
fn modeled_no_image_fill_allows_unused_stream_exclusion() {
    let mut doc = document();
    doc.header.border_fills.push(hwp_model::BorderFill {
        fill_type: 0,
        ..Default::default()
    });
    assert_excluded(&doc);
}

#[test]
fn empty_retained_xml_keeps_unverified_stream() {
    xml_case("");
}
#[test]
fn whitespace_only_retained_xml_keeps_unverified_stream() {
    xml_case("   \n");
}
#[test]
fn text_only_retained_xml_keeps_unverified_stream() {
    xml_case("owner-authored text without an XML root");
}
#[test]
fn unclosed_retained_xml_root_keeps_unverified_stream() {
    xml_case("<root>");
}
#[test]
fn present_empty_settings_xml_keeps_unverified_stream() {
    let mut doc = document();
    doc.hwpx_settings_xml = Some(String::new());
    assert_unverified(&doc, "raw_xml_unparsed");
}
#[test]
fn present_empty_version_xml_keeps_unverified_stream() {
    let mut doc = document();
    doc.hwpx_version_xml = Some(String::new());
    assert_unverified(&doc, "raw_xml_unparsed");
}

fn numeric_picture_doc(raw: Vec<u8>, modeled_id: u16) -> Document {
    let mut doc = modeled_picture_doc(
        opaque(split::TAG_SHAPE_COMPONENT_PICTURE, raw),
        BinRef::Id(hwp_model::BinDataId(modeled_id)),
    );
    doc.bin_streams.truncate(1);
    doc.header.bin_data.clear();
    for id in [1u16, 2] {
        doc.header.bin_data.push(hwp_model::BinDataItem {
            attr: 1,
            storage_id: Some(id),
            extension: Some("png".into()),
            ..Default::default()
        });
        doc.bin_streams.push(BinStream {
            name: format!("BIN{id:04X}.png"),
            data: vec![id as u8],
        });
    }
    assert_eq!(
        doc.bin_stream_index(&BinRef::Id(hwp_model::BinDataId(modeled_id))),
        Some(usize::from(modeled_id))
    );
    doc
}

fn raw_picture_reference(id: u16) -> Vec<u8> {
    // Existing parse_picture reference field: the synthetic prefix is not genuine geometry.
    let mut data = vec![0; 73];
    data[71..73].copy_from_slice(&id.to_le_bytes());
    data
}

#[test]
fn modeled_picture_truncated_raw_picture_keeps_unverified_stream() {
    let doc = numeric_picture_doc(vec![0; 72], 1);
    assert_unverified(&doc, "raw_picture");
    assert_eq!(extract(&doc).resources.kept, vec!["BIN0001.png"]);
}
#[test]
fn modeled_picture_mismatching_raw_picture_reference_keeps_unverified_stream() {
    let doc = numeric_picture_doc(raw_picture_reference(1), 2);
    assert_unverified(&doc, "raw_picture");
    assert_eq!(extract(&doc).resources.kept, vec!["BIN0002.png"]);
}
#[test]
fn modeled_picture_matching_raw_picture_reference_allows_other_streams_exclusion() {
    let doc = numeric_picture_doc(raw_picture_reference(1), 1);
    let outcome = extract(&doc);
    assert!(outcome.resources.unverified.is_empty());
    assert!(outcome.resources.kept_unverified.is_empty());
    assert_eq!(outcome.resources.kept, vec!["BIN0001.png"]);
    assert_eq!(outcome.resources.excluded, vec![UNUSED, "BIN0002.png"]);
    assert_eq!(
        outcome.fragment.bin_streams,
        vec![doc.bin_streams[1].clone()]
    );
}

fn known_no_image_shape(fill: Option<u32>) -> OpaqueRecord {
    // Existing document_split shape_component fixture: one transform, border at byte 196.
    let mut data = vec![0; 52];
    data[..4].copy_from_slice(b"cip$");
    data[4..8].copy_from_slice(b"cip$");
    data[50..52].copy_from_slice(&1u16.to_le_bytes());
    data.resize(196, 0);
    if let Some(fill) = fill {
        data.resize(209, 0);
        data.extend_from_slice(&fill.to_le_bytes());
    }
    opaque(split::TAG_SHAPE_COMPONENT, data)
}

fn assert_clean_picture_shape(record: OpaqueRecord) {
    let doc = modeled_picture_doc(record, BinRef::ItemRef("modeled".into()));
    let outcome = extract(&doc);
    assert!(outcome.resources.unverified.is_empty());
    assert_eq!(outcome.resources.excluded, vec![UNUSED]);
    assert_eq!(outcome.resources.kept, vec!["BinData/modeled.png"]);
}
#[test]
fn known_picture_shape_ending_exactly_at_border_allows_unused_stream_exclusion() {
    assert_clean_picture_shape(known_no_image_shape(None));
}
#[test]
fn readable_picture_shape_without_image_fill_allows_unused_stream_exclusion() {
    assert_clean_picture_shape(known_no_image_shape(Some(0)));
}
#[test]
fn picture_shape_with_incomplete_fill_word_keeps_unverified_stream() {
    let mut record = known_no_image_shape(None);
    record.data.resize(212, 0);
    picture_shape(record.data);
}
#[test]
fn retained_xml_doctype_keeps_unverified_stream() {
    xml_case("<!DOCTYPE root><root/>");
}
#[test]
fn retained_xml_custom_entity_declaration_keeps_unverified_stream() {
    xml_case("<!DOCTYPE root [<!ENTITY own 'unused'>]><root binaryItemIDRef=\"&own;\"/>");
}
#[test]
fn predefined_and_numeric_xml_entities_without_binary_reference_allow_exclusion() {
    let mut doc = document();
    let mut object = generic();
    object.hwpx_raw_xml = Some("<root label=\"&amp;&#65;\"/>".into());
    doc.sections[0].paragraphs[1]
        .controls
        .push(Control::Generic(object));
    assert_excluded(&doc);
}
#[test]
fn numeric_xml_entity_in_binary_reference_resolves_proven_stream() {
    let mut doc = document();
    let mut object = generic();
    object.hwpx_raw_xml = Some("<root binaryItemIDRef=\"unus&#101;d\"/>".into());
    doc.sections[0].paragraphs[1]
        .controls
        .push(Control::Generic(object));
    let outcome = extract(&doc);
    assert_eq!(outcome.resources.kept, vec![UNUSED]);
    assert!(outcome.resources.unverified.is_empty());
    assert!(outcome.resources.excluded.is_empty());
}

#[test]
fn unknown_opaque_paragraph_tag_keeps_unverified_stream() {
    let mut doc = document();
    doc.sections[0].paragraphs[1]
        .extras
        .push(opaque(0x7777, vec![]));
    assert_unverified(&doc, "raw_record_unparsed");
}

#[test]
fn unknown_nested_opaque_header_tag_keeps_unverified_stream() {
    let mut doc = document();
    // COMPATIBLE_DOCUMENT is a known reference-free header carrier; its child is unknown.
    let mut wrapper = opaque(0x10 + 14, vec![]);
    wrapper.children.push(opaque(0x7777, vec![]));
    doc.header.extras.push(wrapper);
    assert_unverified(&doc, "raw_record_unparsed");
}

#[test]
fn known_reference_free_compatibility_record_allows_unused_stream_exclusion() {
    let mut doc = document();
    // Catalog classification, not a claim that this empty payload is a valid native record.
    doc.header.extras.push(opaque(0x10 + 14, vec![]));
    assert_excluded(&doc);
}

#[test]
fn typed_page_definition_slot_is_not_unreadable_xml() {
    let mut doc = document();
    doc.sections[0].paragraphs[1]
        .controls
        .push(Control::SectionDef(hwp_model::SectionDef {
            data: vec![],
            page: None,
            extras: vec![],
            secpr_raw_children: vec![hwp_model::SECPR_PAGEPR_SLOT.to_string()],
            footnote_shape_raw: None,
            endnote_shape_raw: None,
            page_border_fills_raw: vec![],
        }));
    assert_excluded(&doc);
}
