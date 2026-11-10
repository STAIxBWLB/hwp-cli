//! Synthetic caption-owned pictures through the real HWPX-to-HWP CLI conversion.
//! These tests establish serialized ownership and assets, not native Hangul acceptance.

use std::path::Path;
use std::process::Command;

use hwp_model::{
    Caption, CaptionDirection, CaptionSide, Control, Document, GenericControl, HwpChar,
    OpaqueRecord, Paragraph, Picture, ShapeGeom, ShapeKind,
};

#[path = "common/temp_path.rs"]
mod temp_path;

#[derive(Clone, Copy)]
enum Owner {
    Table,
    Picture,
    Shape,
    TableAndShape,
}

fn caption(paragraph: Paragraph) -> Caption {
    Caption {
        side: CaptionSide::Bottom,
        direction: CaptionDirection::Horizontal,
        gap: 250,
        width: None,
        last_width: 0,
        paragraphs: vec![paragraph],
    }
}

fn picture_paragraph(picture: Picture) -> Paragraph {
    Paragraph {
        chars: vec![HwpChar::ExtCtrl {
            code: 11,
            ctrl_id: *b"gso ",
            payload: Vec::new(),
            ctrl_index: Some(0),
        }],
        controls: vec![Control::Picture(picture)],
        ..Paragraph::default()
    }
}

fn template(image: &Path) -> (Picture, Vec<hwp_model::BinStream>) {
    let mut document = hwp_convert::from_markdown("Image anchor.");
    hwp_convert::insert_image(
        &mut document,
        "Image anchor.",
        image,
        hwp_convert::ImageSize::Mm(10.0, 10.0),
    )
    .unwrap();
    let picture = document.sections[0]
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            Control::Picture(picture) => Some(picture.clone()),
            _ => None,
        })
        .expect("synthetic image setup creates a picture");
    (picture, document.bin_streams)
}

fn shape_with_caption(caption: Caption) -> GenericControl {
    GenericControl {
        ctrl_id: *b"gso ",
        data: Vec::new(),
        paragraph_lists: Vec::new(),
        extras: Vec::new(),
        raw_children: Vec::new(),
        gso_shapes: vec![ShapeGeom {
            kind: ShapeKind::Rect,
            x: 100,
            y: 200,
            w: 3000,
            h: 1500,
            points: Vec::new(),
            fill: 0xFFFF_FFFF,
            fill_gradient: None,
            border_color: 0,
            border_width: 10,
            round_ratio: 0,
            border_style: 0,
            arrow_start: 0,
            arrow_end: 0,
            anchored: true,
            description: None,
        }],
        equation: None,
        column_def: None,
        caption: Some(caption),
        hwpx_raw_xml: None,
        container_box: None,
    }
}

fn owner_caption(document: &Document, owner: Owner) -> Option<&Caption> {
    document
        .sections
        .iter()
        .flat_map(|section| &section.paragraphs)
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match (owner, control) {
            (Owner::Table | Owner::TableAndShape, Control::Table(table)) => table.caption.as_ref(),
            (Owner::Picture, Control::Picture(picture)) => picture.caption.as_ref(),
            (Owner::Shape, Control::Generic(generic)) => generic.caption.as_ref(),
            _ => None,
        })
}

fn caption_picture<'a>(caption: &'a Caption, stage: &str) -> &'a Picture {
    caption
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            Control::Picture(picture) => Some(picture),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{stage} must retain its nested picture"))
}

fn has_record(records: &[OpaqueRecord], tag: u16) -> bool {
    records
        .iter()
        .any(|record| record.tag == tag || has_record(&record.children, tag))
}

fn assert_picture(document: &Document, picture: &Picture, expected: &[u8], native: bool) {
    assert_eq!(
        document.resolve_bin(&picture.bin_ref),
        Some(expected),
        "caption picture must resolve the expected image bytes"
    );
    if native {
        assert!(
            has_record(&picture.extras, hwp5::record::tag::SHAPE_COMPONENT),
            "caption picture must retain its synthesized SHAPE_COMPONENT record"
        );
        assert!(
            has_record(&picture.extras, hwp5::record::tag::SHAPE_COMPONENT_PICTURE),
            "caption picture must retain its synthesized SHAPE_COMPONENT_PICTURE record"
        );
    }
}

fn body_picture(document: &Document) -> Option<&Picture> {
    document
        .sections
        .iter()
        .flat_map(|section| &section.paragraphs)
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            Control::Picture(picture) => Some(picture),
            _ => None,
        })
}

fn picture_id(picture: &Picture) -> u32 {
    fn record_id(records: &[OpaqueRecord]) -> Option<u32> {
        for record in records {
            if record.tag == hwp5::record::tag::SHAPE_COMPONENT_PICTURE {
                // Current build_picture_extras writes its instance ID at bytes 74..78.
                return Some(u32::from_le_bytes(record.data[74..78].try_into().unwrap()));
            }
            if let Some(id) = record_id(&record.children) {
                return Some(id);
            }
        }
        None
    }
    record_id(&picture.extras).expect("synthesized caption picture has a picture-record ID")
}

fn convert_caption_picture(label: &str, owner: Owner, nested: bool) {
    run_caption_conversion(label, owner, nested, false);
}

fn run_caption_conversion(label: &str, owner: Owner, nested: bool, strict: bool) {
    let dir = temp_path::TempPath::new("hwp455", label);
    std::fs::create_dir_all(&*dir).unwrap();
    // Generate owner-authored pixels, independent of the local/private fixture corpus.
    let image = dir.join("pixel.png");
    image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]))
        .save(&image)
        .unwrap();
    let bytes = std::fs::read(&image).unwrap();
    let (picture, streams) = template(&image);
    let mut inner = picture.clone();
    if nested {
        inner.caption = Some(caption(picture_paragraph(picture.clone())));
    }
    let owned_caption = caption(picture_paragraph(inner));
    let mut document = if matches!(owner, Owner::Table | Owner::TableAndShape) {
        hwp_convert::from_markdown("| Header |\n| --- |\n| Value |\n")
    } else {
        hwp_convert::from_markdown("Body anchor.")
    };
    document.bin_streams = streams;
    match owner {
        Owner::Table | Owner::TableAndShape => {
            let table = document.sections[0]
                .paragraphs
                .iter_mut()
                .flat_map(|paragraph| &mut paragraph.controls)
                .find_map(|control| match control {
                    Control::Table(table) => Some(table),
                    _ => None,
                })
                .expect("synthetic table setup");
            table.caption = Some(owned_caption);
        }
        Owner::Picture => {
            let mut outer = picture;
            outer.caption = Some(owned_caption);
            document.sections[0]
                .paragraphs
                .push(picture_paragraph(outer));
        }
        Owner::Shape => {
            let mut paragraph = picture_paragraph(picture);
            paragraph.controls[0] = Control::Generic(shape_with_caption(owned_caption));
            document.sections[0].paragraphs.push(paragraph);
        }
    }
    let shape_bytes = if matches!(owner, Owner::TableAndShape) {
        let second_image = dir.join("shape.png");
        image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 255, 0, 255]))
            .save(&second_image)
            .unwrap();
        let second_bytes = std::fs::read(&second_image).unwrap();
        let (mut shape_picture, _) = template(&second_image);
        shape_picture.bin_ref = hwp_model::BinRef::ItemRef("shape-caption.png".to_string());
        document.bin_streams.push(hwp_model::BinStream {
            name: "shape-caption.png".to_string(),
            data: second_bytes.clone(),
        });
        let mut paragraph = Paragraph::default();
        paragraph.chars.push(HwpChar::ExtCtrl {
            code: 11,
            ctrl_id: *b"gso ",
            payload: Vec::new(),
            ctrl_index: Some(0),
        });
        paragraph
            .controls
            .push(Control::Generic(shape_with_caption(caption(
                picture_paragraph(shape_picture),
            ))));
        document.sections[0].paragraphs.push(paragraph);
        Some(second_bytes)
    } else {
        None
    };
    let input = dir.join("source.hwpx");
    hwpx::write_document(&document, &input).unwrap();
    let source = hwpx::read_document(&input).unwrap().document;
    let before = owner_caption(&source, owner).expect("HWPX setup must retain caption ownership");
    let source_picture = caption_picture(before, "HWPX setup caption");
    assert_picture(&source, source_picture, &bytes, false);
    if nested {
        let inner_caption = source_picture
            .caption
            .as_ref()
            .expect("HWPX setup must retain nested caption ownership");
        assert_picture(
            &source,
            caption_picture(inner_caption, "HWPX setup nested caption"),
            &bytes,
            false,
        );
    }
    if let Some(shape_bytes) = &shape_bytes {
        let shape_caption = owner_caption(&source, Owner::Shape)
            .expect("combined HWPX setup must retain its shape caption");
        assert_picture(
            &source,
            caption_picture(shape_caption, "HWPX setup shape caption"),
            shape_bytes,
            false,
        );
        assert_ne!(
            bytes, *shape_bytes,
            "combined fixture uses distinct image assets"
        );
    }
    let output = dir.join("converted.hwp");
    let report = dir.join("loss.json");
    let source_bytes = std::fs::read(&input).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_hwp"));
    command
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .arg("--loss-report")
        .arg(&report);
    if strict {
        command.arg("--strict");
    }
    let result = command.output().unwrap();
    if strict {
        assert!(
            !result.status.success(),
            "strict conversion must refuse the unsupported shape wrapper"
        );
        let message = String::from_utf8_lossy(&result.stderr);
        assert!(
            message.contains("opaque_control_unrepresentable"),
            "strict refusal must retain the wrapper-loss cause: {message}"
        );
        assert!(
            !output.exists(),
            "strict refusal must not publish a converted document"
        );
        assert_eq!(
            std::fs::read(&input).unwrap(),
            source_bytes,
            "strict refusal must leave the input unchanged"
        );
        let ledger: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
        assert!(
            ledger["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["code"] == "opaque_control_unrepresentable"
                    && event["count"].as_u64().unwrap() >= 1),
            "strict refusal must publish the typed wrapper loss: {ledger}"
        );
        return;
    }
    assert!(
        result.status.success(),
        "CLI conversion setup must succeed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let converted = hwp5::read_document(&output).unwrap().document;
    let converted_picture = if matches!(owner, Owner::Shape) {
        body_picture(&converted)
            .expect("omitted semantic shape must preserve its caption picture in the body")
    } else {
        let after = owner_caption(&converted, owner)
            .expect("converted owner must retain its caption and caption picture");
        caption_picture(after, "HWP read-back caption")
    };
    assert_picture(&converted, converted_picture, &bytes, true);
    if matches!(owner, Owner::Shape | Owner::TableAndShape) {
        let ledger: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
        assert!(
            ledger["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["code"] == "opaque_control_unrepresentable"
                    && event["count"].as_u64().unwrap() >= 1),
            "omitted shape wrapper must remain disclosed: {ledger}"
        );
    }
    if let Some(shape_bytes) = &shape_bytes {
        let hoisted = body_picture(&converted)
            .expect("combined conversion must hoist the shape caption picture");
        assert_picture(&converted, hoisted, shape_bytes, true);
        assert_ne!(
            picture_id(converted_picture),
            picture_id(hoisted),
            "caption synthesis must assign distinct picture IDs across table and hoisted shape captions"
        );
    }
    if nested {
        let inner_caption = converted_picture
            .caption
            .as_ref()
            .expect("converted picture must retain its nested caption");
        assert_picture(
            &converted,
            caption_picture(inner_caption, "HWP read-back nested caption"),
            &bytes,
            true,
        );
    }
}

#[test]
fn hwpx_table_caption_picture_survives_hwp_conversion() {
    convert_caption_picture("table", Owner::Table, false);
}

#[test]
fn hwpx_picture_caption_picture_survives_hwp_conversion() {
    convert_caption_picture("picture", Owner::Picture, false);
}

#[test]
fn hwpx_semantic_shape_caption_picture_survives_hwp_conversion() {
    convert_caption_picture("shape", Owner::Shape, false);
}

#[test]
fn hwpx_nested_picture_captions_survive_hwp_conversion() {
    convert_caption_picture("nested", Owner::Table, true);
}

#[test]
fn hwpx_table_and_shape_caption_pictures_survive_together() {
    convert_caption_picture("combined", Owner::TableAndShape, false);
}

#[test]
fn strict_shape_caption_conversion_keeps_wrapper_loss_and_refuses_publication() {
    run_caption_conversion("strict-shape", Owner::Shape, false, true);
}
