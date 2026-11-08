//! Missing manifest targets must remain unresolved across both merge tiers (#417 F29).
use hwp_convert::document_merge::merge_documents;
use hwp_model::{BinRef, BinStream, Control, Document, HwpUnit, Picture};

fn input(name: &str, bytes: Option<&[u8]>) -> Document {
    let mut doc = hwp_convert::from_markdown("Owner-authored image paragraph.\n");
    doc.sections[0].paragraphs[0]
        .controls
        .push(Control::Picture(Picture {
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
            bin_ref: BinRef::ItemRef("logo".into()),
            extras: vec![],
        }));
    doc.hwpx_bin_manifest = vec![("logo".into(), name.into())];
    if let Some(bytes) = bytes {
        doc.bin_streams.push(BinStream {
            name: name.into(),
            data: bytes.to_vec(),
        });
    }
    doc
}
fn picture(doc: &Document, section: usize) -> &Picture {
    doc.sections[section].paragraphs[0]
        .controls
        .iter()
        .find_map(|control| match control {
            Control::Picture(picture) => Some(picture),
            _ => None,
        })
        .unwrap()
}
fn missing_manifest_case(general: bool) {
    let a = input("BinData/p1.png", Some(b"owner-authored A bytes"));
    let mut b = input("BinData/missing.png", None);
    if general {
        b.header.styles.push(hwp_model::Style {
            name: "ExtraStyle".into(),
            english_name: "ExtraStyle".into(),
            ..Default::default()
        });
        b.header.border_fills.push(Default::default());
    }
    assert_eq!(
        a.resolve_bin(&picture(&a, 0).bin_ref),
        Some(b"owner-authored A bytes".as_slice())
    );
    assert!(
        b.resolve_bin(&picture(&b, 0).bin_ref).is_none(),
        "source B must be unresolved before merge"
    );
    let before_b = b.clone();
    let outcome = merge_documents(&[a, b]).unwrap();
    assert_eq!(
        outcome.general_path_used, general,
        "fixture must exercise the requested merge tier"
    );
    assert!(
        outcome
            .document
            .resolve_bin(&picture(&outcome.document, 1).bin_ref)
            .is_none(),
        "B's missing manifest target must never bind to A's logo after merge"
    );
    assert_no_missing_substring(&outcome.document);
    assert_eq!(
        outcome
            .document
            .resolve_bin(&picture(&outcome.document, 0).bin_ref),
        Some(b"owner-authored A bytes".as_slice())
    );
    assert!(
        before_b
            .resolve_bin(&picture(&before_b, 0).bin_ref)
            .is_none()
    );
}
#[test]
fn fast_tier_preserves_missing_manifest_target_as_unresolved() {
    missing_manifest_case(false);
}
#[test]
fn general_tier_preserves_missing_manifest_target_as_unresolved() {
    missing_manifest_case(true);
}

fn missing_spelling(doc: &Document) -> &str {
    match &picture(doc, 1).bin_ref {
        BinRef::ItemRef(spelling) => spelling,
        other => panic!("missing HWPX item must retain an ItemRef spelling: {other:?}"),
    }
}
fn assert_no_missing_substring(doc: &Document) {
    let spelling = missing_spelling(doc).to_ascii_lowercase();
    assert!(!spelling.is_empty());
    for stream in &doc.bin_streams {
        assert!(
            !stream.name.to_ascii_lowercase().contains(&spelling),
            "missing spelling must not match the HWP5 legacy substring lookup: {spelling:?} in {:?}",
            stream.name
        );
    }
}
fn later_input_collision_case(general: bool, force_stream_collision: bool) {
    let mut a = input("BinData/p1.png", Some(b"owner-authored A bytes"));
    let mut b = input("BinData/missing.png", None);
    if general {
        b.header.styles.push(hwp_model::Style {
            name: "ExtraStyle".into(),
            english_name: "ExtraStyle".into(),
            ..Default::default()
        });
        b.header.border_fills.push(Default::default());
    }
    assert!(b.resolve_bin(&picture(&b, 0).bin_ref).is_none());
    let probe = merge_documents(&[a.clone(), b.clone()]).unwrap();
    let spelling = missing_spelling(&probe.document).to_owned();
    let name = format!("BinData/{spelling}.png");
    if force_stream_collision {
        a.bin_streams.push(BinStream {
            name: name.clone(),
            data: b"owner-authored collision decoy".to_vec(),
        });
    }
    let mut c = input(&name, Some(b"owner-authored C bytes"));
    c.hwpx_bin_manifest[0].0 = "own-c-image".into();
    let Control::Picture(picture) = c.sections[0].paragraphs[0]
        .controls
        .iter_mut()
        .find(|control| matches!(control, Control::Picture(_)))
        .unwrap()
    else {
        unreachable!()
    };
    picture.bin_ref = BinRef::ItemRef("own-c-image".into());
    assert_eq!(
        c.resolve_bin(&picture_ref(&c, 0)),
        Some(b"owner-authored C bytes".as_slice())
    );
    let outcome = merge_documents(&[a, b, c]).unwrap();
    assert_eq!(outcome.general_path_used, general);
    let merged = &outcome.document;
    assert!(
        merged.resolve_bin(&picture_ref(merged, 1)).is_none(),
        "later C must never give B's missing target a meaning"
    );
    assert_eq!(
        merged.resolve_bin(&picture_ref(merged, 2)),
        Some(b"owner-authored C bytes".as_slice())
    );
    assert_no_missing_substring(merged);
    if force_stream_collision {
        let own_stream = merged.bin_stream_index(&picture_ref(merged, 2)).unwrap();
        assert_ne!(
            merged.bin_streams[own_stream].name, name,
            "source-name collision must exercise actual later graft prefixing"
        );
    }
}
fn picture_ref(doc: &Document, section: usize) -> BinRef {
    picture(doc, section).bin_ref.clone()
}
#[test]
fn fast_tier_later_input_cannot_resolve_previous_missing_target() {
    later_input_collision_case(false, false);
}
#[test]
fn general_tier_later_input_cannot_resolve_previous_missing_target() {
    later_input_collision_case(true, false);
}
#[test]
fn fast_tier_later_prefixed_stream_cannot_alias_missing_substring() {
    later_input_collision_case(false, true);
}
#[test]
fn general_tier_later_prefixed_stream_cannot_alias_missing_substring() {
    later_input_collision_case(true, true);
}

#[derive(Clone, Copy)]
enum ReferencePath {
    Body,
    Cell,
    TableCaption,
    PictureCaption,
    GenericList,
    GenericCaption,
}
fn caption(paragraph: hwp_model::Paragraph) -> hwp_model::Caption {
    hwp_model::Caption {
        side: hwp_model::CaptionSide::Bottom,
        direction: hwp_model::CaptionDirection::Horizontal,
        gap: 0,
        width: None,
        last_width: 0,
        paragraphs: vec![paragraph],
    }
}
fn table_container() -> hwp_model::Table {
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
            width: HwpUnit(100),
            height: HwpUnit(100),
            margins: [0; 4],
            border_fill: hwp_model::BorderFillId(0),
            header_tail: vec![],
            paragraphs: vec![],
        }],
        caption: None,
        extras: vec![],
    }
}
fn generic_container() -> hwp_model::GenericControl {
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
fn collect_picture_refs(paragraphs: &[hwp_model::Paragraph], refs: &mut Vec<BinRef>) {
    for paragraph in paragraphs {
        for control in &paragraph.controls {
            let caption = match control {
                Control::Picture(picture) => {
                    refs.push(picture.bin_ref.clone());
                    &picture.caption
                }
                Control::Table(table) => {
                    for cell in &table.cells {
                        collect_picture_refs(&cell.paragraphs, refs);
                    }
                    &table.caption
                }
                Control::Generic(generic) => {
                    for list in &generic.paragraph_lists {
                        collect_picture_refs(&list.paragraphs, refs);
                    }
                    &generic.caption
                }
                Control::SectionDef(_) => continue,
            };
            if let Some(caption) = caption {
                collect_picture_refs(&caption.paragraphs, refs);
            }
        }
    }
}
fn section_picture_refs(doc: &Document, section: usize) -> Vec<BinRef> {
    let mut refs = vec![];
    collect_picture_refs(&doc.sections[section].paragraphs, &mut refs);
    refs
}
fn unlisted_reference_path_case(path: ReferencePath, general: bool) {
    let a = input("BinData/p1.png", Some(b"owner-authored A bytes"));
    let mut b = input("BinData/missing.png", None);
    b.hwpx_bin_manifest.clear();
    let leaf_picture = picture(&b, 0).clone();
    b.sections[0].paragraphs[0]
        .controls
        .retain(|control| !matches!(control, Control::Picture(_)));
    let leaf = hwp_model::Paragraph {
        controls: vec![Control::Picture(leaf_picture.clone())],
        ..Default::default()
    };
    let container = match path {
        ReferencePath::Body => Control::Picture(leaf_picture),
        ReferencePath::Cell => {
            let mut table = table_container();
            table.cells[0].paragraphs.push(leaf);
            Control::Table(table)
        }
        ReferencePath::TableCaption => {
            let mut table = table_container();
            table.caption = Some(caption(leaf));
            Control::Table(table)
        }
        ReferencePath::PictureCaption => {
            let mut owner = leaf_picture;
            owner.bin_ref = BinRef::ItemRef("own-container".into());
            owner.caption = Some(caption(leaf));
            b.bin_streams.push(BinStream {
                name: "own-container".into(),
                data: b"owner-authored caption container".to_vec(),
            });
            Control::Picture(owner)
        }
        ReferencePath::GenericList => {
            let mut generic = generic_container();
            generic.paragraph_lists.push(hwp_model::ParagraphList {
                header_data: vec![],
                paragraphs: vec![leaf],
            });
            Control::Generic(generic)
        }
        ReferencePath::GenericCaption => {
            let mut generic = generic_container();
            generic.caption = Some(caption(leaf));
            Control::Generic(generic)
        }
    };
    b.sections[0].paragraphs[0].controls.push(container);
    if general {
        b.header.styles.push(hwp_model::Style {
            name: "ExtraStyle".into(),
            english_name: "ExtraStyle".into(),
            ..Default::default()
        });
        b.header.border_fills.push(Default::default());
    }
    let source_refs = section_picture_refs(&b, 0);
    assert_eq!(source_refs.last(), Some(&BinRef::ItemRef("logo".into())));
    assert!(b.resolve_bin(source_refs.last().unwrap()).is_none());
    let outcome = merge_documents(&[a, b]).unwrap();
    assert_eq!(outcome.general_path_used, general);
    let merged = &outcome.document;
    let refs = section_picture_refs(merged, 1);
    assert_eq!(
        refs.len(),
        source_refs.len(),
        "the traversal case must preserve every modeled picture"
    );
    let missing = refs.last().unwrap();
    assert!(
        merged.resolve_bin(missing).is_none(),
        "an unlisted B picture must not inherit A's manifest binding in this source-reference path"
    );
    let BinRef::ItemRef(spelling) = missing else {
        panic!("missing item must retain a string reference")
    };
    assert!(!spelling.is_empty());
    for stream in &merged.bin_streams {
        assert!(
            !stream
                .name
                .to_ascii_lowercase()
                .contains(&spelling.to_ascii_lowercase()),
            "missing spelling must be safe for legacy substring lookup"
        );
    }
    assert_eq!(
        merged.resolve_bin(&picture_ref(merged, 0)),
        Some(b"owner-authored A bytes".as_slice())
    );
}
macro_rules! unlisted_path_test {
    ($name:ident, $path:ident, $general:expr) => {
        #[test]
        fn $name() {
            unlisted_reference_path_case(ReferencePath::$path, $general);
        }
    };
}
unlisted_path_test!(fast_unlisted_body_picture_stays_unresolved, Body, false);
unlisted_path_test!(general_unlisted_body_picture_stays_unresolved, Body, true);
unlisted_path_test!(fast_unlisted_cell_picture_stays_unresolved, Cell, false);
unlisted_path_test!(general_unlisted_cell_picture_stays_unresolved, Cell, true);
unlisted_path_test!(
    fast_unlisted_table_caption_picture_stays_unresolved,
    TableCaption,
    false
);
unlisted_path_test!(
    general_unlisted_table_caption_picture_stays_unresolved,
    TableCaption,
    true
);
unlisted_path_test!(
    fast_unlisted_picture_caption_picture_stays_unresolved,
    PictureCaption,
    false
);
unlisted_path_test!(
    general_unlisted_picture_caption_picture_stays_unresolved,
    PictureCaption,
    true
);
unlisted_path_test!(
    fast_unlisted_generic_list_picture_stays_unresolved,
    GenericList,
    false
);
unlisted_path_test!(
    general_unlisted_generic_list_picture_stays_unresolved,
    GenericList,
    true
);
unlisted_path_test!(
    fast_unlisted_generic_caption_picture_stays_unresolved,
    GenericCaption,
    false
);
unlisted_path_test!(
    general_unlisted_generic_caption_picture_stays_unresolved,
    GenericCaption,
    true
);

fn valid_suffix_case(general: bool) {
    let mut a = input("BinData/p1.png", Some(b"owner-authored A bytes"));
    let suffix = "nested/p2.png";
    a.hwpx_bin_manifest
        .push((suffix.into(), "BinData/p1.png".into()));
    let mut b = input(
        "BinData/nested/p2.png",
        Some(b"owner-authored B suffix bytes"),
    );
    b.hwpx_bin_manifest.clear();
    let Control::Picture(picture) = b.sections[0].paragraphs[0]
        .controls
        .iter_mut()
        .find(|control| matches!(control, Control::Picture(_)))
        .unwrap()
    else {
        unreachable!()
    };
    picture.bin_ref = BinRef::ItemRef(suffix.into());
    if general {
        b.header.styles.push(hwp_model::Style {
            name: "ExtraStyle".into(),
            english_name: "ExtraStyle".into(),
            ..Default::default()
        });
        b.header.border_fills.push(Default::default());
    }
    assert_eq!(
        b.resolve_bin(&picture_ref(&b, 0)),
        Some(b"owner-authored B suffix bytes".as_slice())
    );
    let outcome = merge_documents(&[a, b]).unwrap();
    assert_eq!(outcome.general_path_used, general);
    assert_eq!(
        outcome
            .document
            .resolve_bin(&picture_ref(&outcome.document, 1)),
        Some(b"owner-authored B suffix bytes".as_slice()),
        "a valid multi-component suffix must retain its own input's bytes despite A's conflicting manifest spelling"
    );
    assert_eq!(
        outcome
            .document
            .resolve_bin(&picture_ref(&outcome.document, 0)),
        Some(b"owner-authored A bytes".as_slice())
    );
}
#[test]
fn fast_valid_multicomponent_suffix_reference_retains_own_bytes() {
    valid_suffix_case(false);
}
#[test]
fn general_valid_multicomponent_suffix_reference_retains_own_bytes() {
    valid_suffix_case(true);
}
