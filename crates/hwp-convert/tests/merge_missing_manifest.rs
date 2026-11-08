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
